use std::{
    path::PathBuf,
    sync::{Arc, atomic::AtomicBool},
};

#[cfg(windows)]
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

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
        #[arg(short, long, default_value_os_t = default_config_path())]
        config: PathBuf,
    },
    /// Validate configuration without starting any SSH process.
    Check {
        #[arg(short, long, default_value_os_t = default_config_path())]
        config: PathBuf,
    },
    /// Run as a Windows service. This is started by the Service Control Manager.
    Service {
        #[arg(short, long, default_value_os_t = default_config_path())]
        config: PathBuf,
    },
    /// Register an automatic Windows service. Run from an elevated shell.
    Install {
        #[arg(short, long, default_value_os_t = default_config_path())]
        config: PathBuf,
    },
    /// Ask the Windows Service Control Manager to start the service.
    Start,
    /// Ask the Windows Service Control Manager to stop the service.
    Stop,
    /// Remove the Windows service. Run from an elevated shell.
    Uninstall,
}

fn main() -> Result<()> {
    match Cli::parse().command.unwrap_or(CommandName::Run {
        config: default_config_path(),
    }) {
        CommandName::Run { config } => run_foreground(config),
        CommandName::Check { config } => check_config(config),
        CommandName::Service { config } => run_service(config),
        CommandName::Install { config } => install_service(config),
        CommandName::Start => sc_command(["start", SERVICE_NAME]),
        CommandName::Stop => sc_command(["stop", SERVICE_NAME]),
        CommandName::Uninstall => sc_command(["delete", SERVICE_NAME]),
    }
}

fn check_config(config: PathBuf) -> Result<()> {
    let config = rust_autossh::Config::load(&config)?;
    println!("valid: {} connection(s)", config.connections.len());
    Ok(())
}

fn run_foreground(config: PathBuf) -> Result<()> {
    let stop = Arc::new(AtomicBool::new(false));
    let signal = stop.clone();
    ctrlc::set_handler(move || signal.store(true, std::sync::atomic::Ordering::SeqCst))
        .context("cannot install Ctrl+C handler")?;
    rust_autossh::run(config, stop)
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
        let status = match service_control_handler::register(SERVICE_NAME, move |event| match event
        {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                control_stop.store(true, Ordering::SeqCst);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }) {
            Ok(status) => status,
            Err(_) => return,
        };
        let _ = status.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        });
        let path = CONFIG_PATH
            .get()
            .expect("configuration path missing")
            .clone();
        let exit_code = match rust_autossh::run(path.clone(), stop) {
            Ok(()) => ServiceExitCode::NO_ERROR,
            Err(error) => {
                report_service_error(&path, &error);
                ServiceExitCode::ServiceSpecific(1)
            }
        };
        let _ = status.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Stopped,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code,
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        });
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

#[cfg(windows)]
fn install_service(config: PathBuf) -> Result<()> {
    let exe = std::env::current_exe().context("cannot determine executable path")?;
    let config = config
        .canonicalize()
        .with_context(|| format!("cannot resolve configuration {}", config.display()))?;
    let bin_path = format!(
        "\"{}\" service --config \"{}\"",
        exe.display(),
        config.display()
    );
    sc_command([
        "create",
        SERVICE_NAME,
        &format!("binPath= {bin_path}"),
        "start= auto",
        "DisplayName= rust-autossh",
    ])?;
    sc_command(["description", SERVICE_NAME, "OpenSSH tunnel supervisor"])
}

#[cfg(not(windows))]
fn install_service(_config: PathBuf) -> Result<()> {
    bail!("Windows service management is only available on Windows")
}

#[cfg(windows)]
fn sc_command<const N: usize>(arguments: [&str; N]) -> Result<()> {
    let status = Command::new("sc.exe")
        .args(arguments)
        .status()
        .context("cannot run sc.exe; use an elevated PowerShell")?;
    if status.success() {
        Ok(())
    } else {
        bail!("sc.exe exited with {status}")
    }
}

#[cfg(not(windows))]
fn sc_command<const N: usize>(_arguments: [&str; N]) -> Result<()> {
    bail!("Windows service management is only available on Windows")
}

fn default_config_path() -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(std::env::var_os("PROGRAMDATA").unwrap_or_else(|| "C:\\ProgramData".into()))
            .join("rust-autossh")
            .join("config.toml")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/etc/rust-autossh/config.toml")
    }
}
