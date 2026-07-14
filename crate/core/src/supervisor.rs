use std::{
    collections::HashMap,
    fs,
    io::BufRead,
    io::BufReader,
    path::{Path, PathBuf},
    process::Child,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};

#[cfg(windows)]
use encoding_rs::{BIG5, EUC_KR, Encoding, GBK, SHIFT_JIS, WINDOWS_1252};
#[cfg(windows)]
use std::sync::OnceLock;
#[cfg(windows)]
use windows_sys::Win32::Globalization::GetACP;

use crate::config::{Config, ConnectionConfig, KeepaliveConfig, RetryConfig};
use crate::logger::Logger;
use crate::ssh;
use crate::ssh_log::{SshStderrAnnotator, describe_configured_forwards};

const CONFIG_POLL_INTERVAL: Duration = Duration::from_secs(2);
const CHILD_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Runs all enabled connections until `stop` is set. Configuration changes
/// restart only changed workers; a log configuration change restarts all workers.
pub fn run(config_path: PathBuf, stop: Arc<AtomicBool>) -> Result<()> {
    let config = Config::load(&config_path)?;
    let mut log_config = config.log.clone();
    let mut logger = Logger::new(&config_path, &log_config)?;
    logger.info(format!(
        "loaded {} connection(s) from {}",
        config.connections.len(),
        config_path.display()
    ));
    let mut supervisor = Supervisor::start(
        config.connections,
        config.keepalive,
        config.retry,
        logger.clone(),
    )?;
    let mut snapshot = config_snapshot(&config_path);
    let mut last_reload_error = None;

    while !stop.load(Ordering::Relaxed) {
        thread::sleep(CONFIG_POLL_INTERVAL);
        let current_snapshot = config_snapshot(&config_path);
        if current_snapshot == snapshot {
            continue;
        }
        snapshot = current_snapshot;

        let new_config = match Config::load(&config_path) {
            Ok(config) => config,
            Err(error) => {
                report_reload_error(&logger, &mut last_reload_error, error.to_string());
                continue;
            }
        };
        let log_changed = new_config.log != log_config;
        let new_logger = if log_changed {
            match Logger::new(&config_path, &new_config.log) {
                Ok(logger) => logger,
                Err(error) => {
                    report_reload_error(
                        &logger,
                        &mut last_reload_error,
                        format!("cannot apply log configuration: {error:#}"),
                    );
                    continue;
                }
            }
        } else {
            logger.clone()
        };

        logger.info("configuration changed; reconciling connection workers");
        let result = supervisor.reconfigure(
            new_config.connections,
            new_config.keepalive,
            new_config.retry,
            new_logger.clone(),
            log_changed,
        );
        if log_changed {
            log_config = new_config.log;
            logger = new_logger;
        }
        match result {
            Ok(()) => last_reload_error = None,
            Err(error) => {
                // Retry transient worker-start failures without requiring another file edit.
                snapshot = None;
                report_reload_error(
                    &logger,
                    &mut last_reload_error,
                    format!("cannot apply all connection changes: {error:#}"),
                );
            }
        }
    }
    logger.info("shutdown requested; stopping connection workers");
    supervisor.stop_all();
    Ok(())
}

fn report_reload_error(logger: &Logger, previous: &mut Option<String>, message: String) {
    if previous.as_deref() != Some(&message) {
        logger.error(format!(
            "configuration reload rejected; keeping unaffected connections: {message}"
        ));
        *previous = Some(message);
    }
}

pub(crate) fn config_snapshot(path: &Path) -> Option<Vec<u8>> {
    fs::read(path).ok()
}

struct Worker {
    config: ConnectionConfig,
    stop: Arc<AtomicBool>,
    handle: thread::JoinHandle<()>,
}

impl Worker {
    fn spawn(
        connection: ConnectionConfig,
        keepalive: KeepaliveConfig,
        retry: RetryConfig,
        logger: Logger,
    ) -> Result<Self> {
        let name = connection.name.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let handle = thread::Builder::new()
            .name(format!("connection-{name}"))
            .spawn({
                let worker_config = connection.clone();
                move || supervise_connection(worker_config, keepalive, retry, worker_stop, logger)
            })
            .with_context(|| format!("cannot start worker for connection {name}"))?;
        Ok(Self {
            config: connection,
            stop,
            handle,
        })
    }

    fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    fn join(self) {
        let _ = self.handle.join();
    }
}

struct Supervisor {
    workers: HashMap<String, Worker>,
}

impl Supervisor {
    fn start(
        connections: Vec<ConnectionConfig>,
        keepalive: KeepaliveConfig,
        retry: RetryConfig,
        logger: Logger,
    ) -> Result<Self> {
        let mut supervisor = Self {
            workers: HashMap::new(),
        };
        for connection in connections
            .into_iter()
            .filter(|connection| connection.enabled)
        {
            let name = connection.name.clone();
            match Worker::spawn(connection, keepalive.clone(), retry.clone(), logger.clone()) {
                Ok(worker) => {
                    supervisor.workers.insert(name, worker);
                }
                Err(error) => {
                    supervisor.stop_all();
                    return Err(error);
                }
            }
        }
        Ok(supervisor)
    }

    fn reconfigure(
        &mut self,
        connections: Vec<ConnectionConfig>,
        keepalive: KeepaliveConfig,
        retry: RetryConfig,
        logger: Logger,
        force_restart: bool,
    ) -> Result<()> {
        let desired: HashMap<_, _> = connections
            .into_iter()
            .filter(|connection| connection.enabled)
            .map(|connection| (connection.name.clone(), connection))
            .collect();
        let obsolete: Vec<_> = self
            .workers
            .iter()
            .filter(|(name, worker)| force_restart || desired.get(*name) != Some(&worker.config))
            .map(|(name, _)| name.clone())
            .collect();
        let stopped: Vec<_> = obsolete
            .into_iter()
            .filter_map(|name| self.workers.remove(&name))
            .collect();
        for worker in &stopped {
            worker.request_stop();
        }
        for worker in stopped {
            worker.join();
        }

        let mut errors = Vec::new();
        for (name, connection) in desired {
            if self.workers.contains_key(&name) {
                continue;
            }
            match Worker::spawn(connection, keepalive.clone(), retry.clone(), logger.clone()) {
                Ok(worker) => {
                    self.workers.insert(name, worker);
                }
                Err(error) => errors.push(error.to_string()),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            bail!(errors.join("; "))
        }
    }

    fn stop_all(&mut self) {
        let workers: Vec<_> = self.workers.drain().map(|(_, worker)| worker).collect();
        for worker in &workers {
            worker.request_stop();
        }
        for worker in workers {
            worker.join();
        }
    }
}

fn supervise_connection(
    connection: ConnectionConfig,
    keepalive: KeepaliveConfig,
    retry: RetryConfig,
    stop: Arc<AtomicBool>,
    logger: Logger,
) {
    let mut delay = retry.initial_seconds;
    logger.info(format!("{}: supervisor started", connection.name));
    while !stop.load(Ordering::Relaxed) {
        let started = Instant::now();
        let mut shutdown = false;
        match ssh::spawn(&connection, &keepalive) {
            Ok(mut child) => {
                logger.info(format!(
                    "{}: ssh process started (pid {}); destination={}; forwards=[{}]",
                    connection.name,
                    child.id(),
                    connection.destination(),
                    describe_configured_forwards(&connection)
                ));
                let stderr_reader = capture_stderr(&mut child, connection.clone(), logger.clone());
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

        if started.elapsed() >= Duration::from_secs(retry.stable_seconds) {
            delay = retry.initial_seconds;
            logger.info(format!(
                "{}: stable connection; retry delay reset",
                connection.name
            ));
        }
        logger.info(format!("{}: reconnecting in {delay}s", connection.name));
        if sleep_until_stopped(Duration::from_secs(delay), &stop) {
            break;
        }
        delay = delay.saturating_mul(2).min(retry.maximum_seconds);
    }
    logger.info(format!("{}: supervisor stopped", connection.name));
}

fn wait_child(child: &mut Child, stop: &AtomicBool) -> Result<Option<std::process::ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait().context("cannot poll ssh process")? {
            return Ok(Some(status));
        }
        if stop.load(Ordering::Relaxed) {
            return Ok(None);
        }
        thread::sleep(CHILD_POLL_INTERVAL);
    }
}

fn capture_stderr(
    child: &mut Child,
    connection: ConnectionConfig,
    logger: Logger,
) -> Option<thread::JoinHandle<()>> {
    let stderr = child.stderr.take()?;
    let name = connection.name.clone();
    thread::Builder::new()
        .name(format!("ssh-stderr-{name}"))
        .spawn(move || {
            let mut annotator = SshStderrAnnotator::new(&connection);
            let mut reader = BufReader::new(stderr);
            let mut bytes = Vec::new();
            loop {
                match reader.read_until(b'\n', &mut bytes) {
                    Ok(0) => break,
                    Ok(_) => {
                        let line = decode_ssh_stderr(&bytes);
                        if let Some(line) = annotator.annotate(&line) {
                            logger.warn(format!("{name}: ssh: {line}"));
                        }
                        bytes.clear();
                    }
                    Err(error) => {
                        logger.warn(format!("{name}: cannot read ssh stderr: {error}"));
                        break;
                    }
                }
            }
        })
        .ok()
}

/// Decode one stderr record. Windows OpenSSH writes localised diagnostics in
/// the system ANSI code page when stderr is a pipe; `LC_ALL=C` does not change
/// that behaviour. Prefer UTF-8 (the usual remote-server encoding), then use
/// the Windows ANSI code page for local SSH diagnostics.
fn decode_ssh_stderr(bytes: &[u8]) -> String {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    if let Ok(line) = std::str::from_utf8(bytes) {
        return line.to_owned();
    }

    #[cfg(windows)]
    {
        let (line, _, _) = windows_ansi_encoding().decode(bytes);
        return line.into_owned();
    }

    #[cfg(not(windows))]
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(windows)]
fn windows_ansi_encoding() -> &'static Encoding {
    static ENCODING: OnceLock<&'static Encoding> = OnceLock::new();
    *ENCODING.get_or_init(|| {
        let code_page = unsafe { GetACP() };
        match code_page {
            932 => SHIFT_JIS,
            936 => GBK,
            949 => EUC_KR,
            950 => BIG5,
            1250..=1258 => Encoding::for_label(format!("windows-{code_page}").as_bytes())
                .unwrap_or(WINDOWS_1252),
            _ => WINDOWS_1252,
        }
    })
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
        if stop.load(Ordering::Relaxed) {
            return true;
        }
        thread::sleep((deadline - Instant::now()).min(CHILD_POLL_INTERVAL));
    }
    false
}

#[cfg(test)]
#[path = "../tests/unit/supervisor.rs"]
mod tests;
