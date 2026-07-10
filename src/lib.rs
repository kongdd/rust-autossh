//! Supervises OpenSSH tunnel processes; it does not implement SSH itself.

use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

const CONFIG_POLL_INTERVAL: Duration = Duration::from_secs(2);
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub log: LogConfig,
    #[serde(default)]
    pub connections: Vec<ConnectionConfig>,
}

/// One OpenSSH process connected to one host. A connection can own several
/// local and remote forwards, sharing its keepalive and retry settings.
#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ConnectionConfig {
    /// Log identifier and SSH destination host (`user@host` or Host alias).
    pub name: String,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    /// An optional explicit path to ssh.exe (or ssh on Linux).
    pub ssh_path: Option<PathBuf>,
    #[serde(default)]
    pub keepalive: KeepaliveConfig,
    #[serde(default)]
    pub retry: RetryConfig,
    /// Extra arguments inserted before forwarding options and destination host.
    #[serde(default)]
    pub extra_args: Vec<String>,
    pub forwards: Vec<ForwardConfig>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct ForwardConfig {
    /// `remote` maps to `ssh -R`; `local` maps to `ssh -L`.
    pub mode: ForwardMode,
    /// Forward specification accepted by OpenSSH, e.g. `10022:127.0.0.1:22`.
    pub forward: String,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ForwardMode {
    Local,
    Remote,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct KeepaliveConfig {
    #[serde(default = "default_keepalive_interval")]
    pub interval: u64,
    #[serde(default = "default_keepalive_count")]
    pub count_max: u32,
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout: u64,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct RetryConfig {
    #[serde(default = "default_retry_initial")]
    pub initial_seconds: u64,
    #[serde(default = "default_retry_maximum")]
    pub maximum_seconds: u64,
    #[serde(default = "default_retry_stable")]
    pub stable_seconds: u64,
}

#[derive(Debug, Deserialize, Clone, Default)]
#[serde(deny_unknown_fields)]
pub struct LogConfig {
    /// Omit to write only to stderr. Relative paths are resolved against the config file.
    pub file: Option<PathBuf>,
    /// Rotate after this many MiB. Set zero to disable rotation.
    #[serde(default = "default_rotate_mib")]
    pub rotate_mib: u64,
}

fn enabled_by_default() -> bool {
    true
}
fn default_keepalive_interval() -> u64 {
    30
}
fn default_keepalive_count() -> u32 {
    3
}
fn default_connect_timeout() -> u64 {
    15
}
fn default_retry_initial() -> u64 {
    1
}
fn default_retry_maximum() -> u64 {
    60
}
fn default_retry_stable() -> u64 {
    60
}
fn default_rotate_mib() -> u64 {
    10
}

impl Default for KeepaliveConfig {
    fn default() -> Self {
        Self {
            interval: default_keepalive_interval(),
            count_max: default_keepalive_count(),
            connect_timeout: default_connect_timeout(),
        }
    }
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            initial_seconds: default_retry_initial(),
            maximum_seconds: default_retry_maximum(),
            stable_seconds: default_retry_stable(),
        }
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("cannot read configuration {}", path.display()))?;
        let config: Self =
            toml::from_str(&text).with_context(|| format!("invalid TOML in {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        if self.connections.is_empty() {
            bail!("configuration has no connections");
        }
        let mut names = HashSet::new();
        for connection in &self.connections {
            if connection.name.trim().is_empty() {
                bail!("connection name must not be empty");
            }
            if connection.forwards.is_empty() {
                bail!("connection {} has no forwards", connection.name);
            }
            if connection
                .forwards
                .iter()
                .any(|forward| forward.forward.trim().is_empty())
            {
                bail!("connection {} has an empty forward", connection.name);
            }
            if !names.insert(&connection.name) {
                bail!("duplicate connection name: {}", connection.name);
            }
            if connection.keepalive.interval == 0
                || connection.keepalive.count_max == 0
                || connection.keepalive.connect_timeout == 0
            {
                bail!(
                    "connection {}: keepalive values must be greater than zero",
                    connection.name
                );
            }
            if connection.retry.initial_seconds == 0
                || connection.retry.maximum_seconds < connection.retry.initial_seconds
                || connection.retry.stable_seconds == 0
            {
                bail!("connection {}: invalid retry settings", connection.name);
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct Logger(Arc<Mutex<LogSink>>);

struct LogSink {
    path: Option<PathBuf>,
    max_bytes: u64,
    file: Option<File>,
}

impl Logger {
    pub fn new(config_path: &Path, log: &LogConfig) -> Result<Self> {
        let path = log.file.as_ref().map(|path| {
            if path.is_absolute() {
                path.clone()
            } else {
                config_path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(path)
            }
        });
        let file = match &path {
            Some(path) => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                Some(
                    OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .with_context(|| format!("cannot open log file {}", path.display()))?,
                )
            }
            None => None,
        };
        Ok(Self(Arc::new(Mutex::new(LogSink {
            path,
            max_bytes: log.rotate_mib.saturating_mul(1024 * 1024),
            file,
        }))))
    }

    pub fn info(&self, message: impl AsRef<str>) {
        self.write("INFO", message.as_ref());
    }
    pub fn warn(&self, message: impl AsRef<str>) {
        self.write("WARN", message.as_ref());
    }
    pub fn error(&self, message: impl AsRef<str>) {
        self.write("ERROR", message.as_ref());
    }

    fn write(&self, level: &str, message: &str) {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let line = format!("[{seconds}] {level:<5} {message}\n");
        eprint!("{line}");
        let mut sink = self.0.lock().expect("logger mutex poisoned");
        if let Some(path) = sink.path.clone()
            && sink.max_bytes > 0
            && fs::metadata(&path).map(|m| m.len()).unwrap_or(0) >= sink.max_bytes
        {
            // Windows cannot rename file while current process holds it open.
            sink.file = None;
            let rotated = path.with_extension("log.1");
            let _ = fs::remove_file(&rotated);
            if let Err(error) = fs::rename(&path, &rotated) {
                eprintln!("log rotation failed: {error}");
            }
            sink.file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .ok();
        }
        if let Some(file) = &mut sink.file {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }
}

/// Runs all enabled tunnels until `stop` is set. Any configuration content
/// change restarts all workers with the latest valid definitions.
pub fn run(config_path: PathBuf, stop: Arc<AtomicBool>) -> Result<()> {
    let mut config = Config::load(&config_path)?;
    let mut logger = Logger::new(&config_path, &config.log)?;
    logger.info(format!(
        "loaded {} connection(s) from {}",
        config.connections.len(),
        config_path.display()
    ));
    let mut supervisor = Supervisor::start(config, logger.clone())?;
    let mut snapshot = config_snapshot(&config_path);
    let mut last_reload_error = None;

    while !stop.load(Ordering::SeqCst) {
        thread::sleep(CONFIG_POLL_INTERVAL);
        let current_snapshot = config_snapshot(&config_path);
        if current_snapshot != snapshot {
            snapshot = current_snapshot;
            match Config::load(&config_path) {
                Ok(new_config) => {
                    logger.info("configuration changed; restarting tunnel workers");
                    supervisor.stop_and_join();
                    config = new_config;
                    logger = Logger::new(&config_path, &config.log)?;
                    supervisor = Supervisor::start(config.clone(), logger.clone())?;
                    last_reload_error = None;
                }
                Err(error) => {
                    let message = error.to_string();
                    if last_reload_error.as_deref() != Some(message.as_str()) {
                        logger.error(format!(
                            "configuration reload rejected; keeping current tunnels: {message}"
                        ));
                        last_reload_error = Some(message);
                    }
                }
            }
        }
    }
    logger.info("shutdown requested; stopping tunnel workers");
    supervisor.stop_and_join();
    Ok(())
}

fn config_snapshot(path: &Path) -> Option<Vec<u8>> {
    fs::read(path).ok()
}

struct Supervisor {
    stop: Arc<AtomicBool>,
    workers: Vec<thread::JoinHandle<()>>,
}

impl Supervisor {
    fn start(config: Config, logger: Logger) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let mut workers = Vec::new();
        for connection in config
            .connections
            .into_iter()
            .filter(|connection| connection.enabled)
        {
            let name = connection.name.clone();
            let worker_stop = Arc::clone(&stop);
            let worker_logger = logger.clone();
            let worker = thread::Builder::new()
                .name(format!("connection-{name}"))
                .spawn(move || supervise_connection(connection, worker_stop, worker_logger))
                .with_context(|| format!("cannot start worker for connection {name}"))?;
            workers.push(worker);
        }
        Ok(Self { stop, workers })
    }

    fn stop_and_join(self) {
        self.stop.store(true, Ordering::SeqCst);
        for worker in self.workers {
            let _ = worker.join();
        }
    }
}

fn supervise_connection(connection: ConnectionConfig, stop: Arc<AtomicBool>, logger: Logger) {
    let mut delay = connection.retry.initial_seconds;
    logger.info(format!("{}: supervisor started", connection.name));
    while !stop.load(Ordering::SeqCst) {
        let started = Instant::now();
        let mut shutdown = false;
        match spawn_ssh(&connection) {
            Ok(mut child) => {
                logger.info(format!(
                    "{}: ssh process started (pid {})",
                    connection.name,
                    child.id()
                ));
                let stderr_reader =
                    capture_stderr(&mut child, connection.name.clone(), logger.clone());
                match wait_child(&mut child, &stop) {
                    Ok(Some(status)) => {
                        logger.warn(format!("{}: ssh exited with {status}", connection.name))
                    }
                    Ok(None) => {
                        terminate_child(&mut child, &connection.name, &logger);
                        shutdown = true;
                    }
                    Err(error) => {
                        logger.error(format!(
                            "{}: cannot wait for ssh: {error:#}",
                            connection.name
                        ));
                        terminate_child(&mut child, &connection.name, &logger);
                    }
                }
                if let Some(reader) = stderr_reader {
                    let _ = reader.join();
                }
            }
            Err(error) => logger.error(format!("{}: cannot start ssh: {error:#}", connection.name)),
        }
        if shutdown {
            break;
        }

        if started.elapsed() >= Duration::from_secs(connection.retry.stable_seconds) {
            delay = connection.retry.initial_seconds;
            logger.info(format!(
                "{}: stable connection; retry delay reset",
                connection.name
            ));
        }
        logger.info(format!("{}: reconnecting in {delay}s", connection.name));
        if sleep_until_stopped(Duration::from_secs(delay), &stop) {
            break;
        }
        delay = delay
            .saturating_mul(2)
            .min(connection.retry.maximum_seconds);
    }
    logger.info(format!("{}: supervisor stopped", connection.name));
}

fn wait_child(child: &mut Child, stop: &AtomicBool) -> Result<Option<std::process::ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait().context("cannot poll ssh process")? {
            return Ok(Some(status));
        }
        if stop.load(Ordering::SeqCst) {
            return Ok(None);
        }
        thread::sleep(CHILD_POLL_INTERVAL);
    }
}

fn capture_stderr(
    child: &mut Child,
    name: String,
    logger: Logger,
) -> Option<thread::JoinHandle<()>> {
    let stderr = child.stderr.take()?;
    thread::Builder::new()
        .name(format!("ssh-stderr-{name}"))
        .spawn(move || {
            for line in BufReader::new(stderr).lines() {
                match line {
                    Ok(line) => logger.warn(format!("{name}: ssh: {line}")),
                    Err(error) => {
                        logger.warn(format!("{name}: cannot read ssh stderr: {error}"));
                        break;
                    }
                }
            }
        })
        .ok()
}

fn terminate_child(child: &mut Child, name: &str, logger: &Logger) {
    if let Err(error) = child.kill() {
        logger.warn(format!("{name}: cannot terminate ssh process: {error}"));
    }
    let _ = child.wait();
}

fn sleep_until_stopped(delay: Duration, stop: &AtomicBool) -> bool {
    let deadline = Instant::now() + delay;
    while Instant::now() < deadline {
        if stop.load(Ordering::SeqCst) {
            return true;
        }
        thread::sleep((deadline - Instant::now()).min(CHILD_POLL_INTERVAL));
    }
    false
}

pub fn spawn_ssh(connection: &ConnectionConfig) -> std::io::Result<Child> {
    let program = connection.ssh_path.clone().unwrap_or_else(default_ssh_path);
    Command::new(program)
        .args(ssh_args(connection))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
}

pub fn ssh_args(connection: &ConnectionConfig) -> Vec<String> {
    let mut args = vec![
        "-N".into(),
        "-T".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
        "-o".into(),
        format!("ConnectTimeout={}", connection.keepalive.connect_timeout),
        "-o".into(),
        format!("ServerAliveInterval={}", connection.keepalive.interval),
        "-o".into(),
        format!("ServerAliveCountMax={}", connection.keepalive.count_max),
    ];
    args.extend(connection.extra_args.clone());
    for forward in &connection.forwards {
        args.push(
            match forward.mode {
                ForwardMode::Local => "-L",
                ForwardMode::Remote => "-R",
            }
            .into(),
        );
        args.push(forward.forward.clone());
    }
    args.push(connection.name.clone());
    args
}

fn default_ssh_path() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(r"C:\Windows\System32\OpenSSH\ssh.exe")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("ssh")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> ConnectionConfig {
        ConnectionConfig {
            name: "user@example.test".into(),
            enabled: true,
            ssh_path: None,
            keepalive: KeepaliveConfig::default(),
            retry: RetryConfig::default(),
            extra_args: vec!["-v".into()],
            forwards: vec![
                ForwardConfig {
                    mode: ForwardMode::Local,
                    forward: "8080:127.0.0.1:8080".into(),
                },
                ForwardConfig {
                    mode: ForwardMode::Remote,
                    forward: "10022:127.0.0.1:22".into(),
                },
            ],
        }
    }

    #[test]
    fn creates_multiple_ssh_forward_arguments() {
        let args = ssh_args(&connection());
        assert_eq!(args[0..2], ["-N", "-T"]);
        assert!(
            args.windows(2)
                .any(|part| part == ["-L", "8080:127.0.0.1:8080"])
        );
        assert!(
            args.windows(2)
                .any(|part| part == ["-R", "10022:127.0.0.1:22"])
        );
        assert_eq!(args[args.len() - 1], "user@example.test");
    }

    #[test]
    fn rejects_invalid_retry_range() {
        let mut config = Config {
            log: LogConfig::default(),
            connections: vec![connection()],
        };
        config.connections[0].retry.maximum_seconds = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn snapshot_detects_same_size_content_change() {
        let path = std::env::temp_dir().join(format!("rust-autossh-{}", std::process::id()));
        fs::write(&path, b"first").unwrap();
        let first = config_snapshot(&path);
        fs::write(&path, b"other").unwrap();
        assert_ne!(first, config_snapshot(&path));
        fs::remove_file(path).unwrap();
    }
}
