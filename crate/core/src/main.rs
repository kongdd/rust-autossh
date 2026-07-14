use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, atomic::AtomicBool},
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

#[cfg(windows)]
const SERVICE_NAME: &str = "rust-autossh";

#[derive(Parser)]
#[command(
    name = "rust-autossh",
    version,
    about = "Supervise OpenSSH port-forwarding tunnels"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<CommandName>,
}

#[derive(Subcommand)]
enum CommandName {
    /// Run tunnel supervisors in the foreground. Ctrl+C stops all ssh processes.
    Run {
        #[arg(short, long, default_value_os_t = autossh_core::default_config_path())]
        config: PathBuf,
    },
    /// Validate configuration without starting any SSH process.
    Check {
        #[arg(short, long, default_value_os_t = autossh_core::default_config_path())]
        config: PathBuf,
    },
    /// Run as a Windows service. This is started by the Service Control Manager.
    Service {
        #[arg(short, long, default_value_os_t = autossh_core::default_config_path())]
        config: PathBuf,
    },
    /// Open the configuration file in the default editor (code / code.exe).
    Edit {
        #[arg(short, long, default_value_os_t = autossh_core::default_config_path())]
        config: PathBuf,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command.unwrap_or(CommandName::Run {
        config: autossh_core::default_config_path(),
    }) {
        CommandName::Run { config } => run_foreground(config),
        CommandName::Check { config } => check_config(config),
        CommandName::Service { config } => run_service(config),
        CommandName::Edit { config } => edit_config(config),
    }
}

fn edit_config(config: PathBuf) -> Result<()> {
    ensure_config(&config)?;
    let editor = if cfg!(windows) { "code.exe" } else { "code" };
    let status = Command::new(editor)
        .arg(&config)
        .status()
        .with_context(|| format!("cannot start {editor}"))?;
    if !status.success() {
        bail!("{editor} exited with {status}");
    }
    Ok(())
}

fn ensure_config(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
    }
    fs::write(path, EXAMPLE_CONFIG).with_context(|| format!("cannot write {}", path.display()))?;
    eprintln!("Created {}. Edit it, then run again.", path.display());
    std::process::exit(0);
}

fn check_config(config: PathBuf) -> Result<()> {
    ensure_config(&config)?;
    let config = autossh_core::Config::load(&config)?;
    println!("valid: {} connection(s)", config.connections.len());
    Ok(())
}

fn run_foreground(config: PathBuf) -> Result<()> {
    ensure_config(&config)?;
    let stop = Arc::new(AtomicBool::new(false));
    let signal = stop.clone();
    ctrlc::set_handler(move || signal.store(true, std::sync::atomic::Ordering::Relaxed))
        .context("cannot install Ctrl+C handler")?;
    autossh_core::run(config, stop)
}

#[cfg(windows)]
fn run_service(config: PathBuf) -> Result<()> {
    windows_service_host::run(config)
}

/// Windows-only Service Control Manager integration.  Keeping the callback at module scope
/// follows `windows-service`'s required FFI entry-point pattern.
#[cfg(windows)]
mod windows_service_host {
    use std::{
        ffi::OsString,
        fs::OpenOptions,
        io::Write,
        path::{Path, PathBuf},
        sync::{
            Arc, OnceLock,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use anyhow::{Context, Result};
    use windows_service::{
        define_windows_service,
        service::{
            ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
            ServiceType,
        },
        service_control_handler::{self, ServiceControlHandlerResult},
        service_dispatcher,
    };

    use super::SERVICE_NAME;

    static CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();

    pub fn run(config: PathBuf) -> Result<()> {
        CONFIG_PATH
            .set(config)
            .map_err(|_| anyhow::anyhow!("service configuration was already set"))?;
        service_dispatcher::start(SERVICE_NAME, ffi_service_main).context(
            "cannot connect to the Windows Service Control Manager (use `run` outside a service)",
        )
    }

    define_windows_service!(ffi_service_main, service_main);

    fn service_main(_arguments: Vec<OsString>) {
        let stop = Arc::new(AtomicBool::new(false));
        let control_stop = stop.clone();
        let path = CONFIG_PATH
            .get()
            .expect("configuration path missing")
            .clone();
        let status = match service_control_handler::register(SERVICE_NAME, move |event| match event
        {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                control_stop.store(true, Ordering::Relaxed);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }) {
            Ok(status) => status,
            Err(error) => {
                report_service_error(
                    &path,
                    &anyhow::anyhow!("cannot register service control handler: {error}"),
                );
                return;
            }
        };
        if let Err(error) = status.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        }) {
            report_service_error(
                &path,
                &anyhow::anyhow!("cannot report running service status: {error}"),
            );
            return;
        }
        let exit_code = match autossh_core::run(path.clone(), stop) {
            Ok(()) => ServiceExitCode::NO_ERROR,
            Err(error) => {
                report_service_error(&path, &error);
                ServiceExitCode::ServiceSpecific(1)
            }
        };
        if let Err(error) = status.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code,
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        }) {
            report_service_error(
                &path,
                &anyhow::anyhow!("cannot report stopped service status: {error}"),
            );
        }
    }

    fn report_service_error(config_path: &Path, error: &anyhow::Error) {
        let message = format!("rust-autossh service stopped: {error:#}\n");
        eprint!("{message}");
        let log_path = config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("rust-autossh.service-error.log");
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
            let _ = file.write_all(message.as_bytes());
        }
    }
}

#[cfg(not(windows))]
fn run_service(_config: PathBuf) -> Result<()> {
    bail!("the `service` command is only available on Windows")
}

const EXAMPLE_CONFIG: &str = r##"# Each [[connections]] block starts one ssh process with any number of -L / -R forwards.
# keepalive / retry apply to ALL connections; per-connection overrides are
# intentional not supported so the supervisor lifecycle stays predictable.
# Edit this file, then run `rust-autossh run` again.

keepalive = { interval = 60, count_max = 3, connect_timeout = 15 }
retry     = { initial_seconds = 1, maximum_seconds = 60, stable_seconds = 60 }

[[connections]]
name = "primary"
host = "myhost"
forwards = [
  { mode = "local",  forward = "8080:127.0.0.1:8080" },
  { mode = "remote", forward = "10022:127.0.0.1:22" },
]
"##;
