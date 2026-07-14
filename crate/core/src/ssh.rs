use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Prevent `ssh.exe` from allocating a visible console when launched by the
/// native GUI or the background supervisor.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
// Do not combine this with DETACHED_PROCESS: Windows explicitly ignores
// CREATE_NO_WINDOW when DETACHED_PROCESS is present. sshpass is a console
// executable, so CREATE_NO_WINDOW alone is required to suppress its black box.
#[cfg(windows)]
const NO_CONSOLE_FLAGS: u32 = CREATE_NO_WINDOW;

use crate::config::{ConnectionConfig, ForwardMode, KeepaliveConfig};

/// Apply every cross-platform tweak that keeps the GUI free of console
/// flicker: stdin/stdout/stderr are sealed against the parent, the C locale
/// stops ssh from emitting mojibake, and (on Windows) the child is started
/// with `CREATE_NO_WINDOW` so console executables such as sshpass and ssh
/// cannot allocate a visible window.
///
/// Centralising the flag assembly makes it impossible to forget the step on
/// one of the three spawn paths the probe uses (direct ssh, sshpass, and the
/// `sshpass -V` availability check).
fn configure_quiet_spawn(command: &mut Command) {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .env("LC_ALL", "C")
        .env("LANG", "C");
    #[cfg(windows)]
    command.creation_flags(NO_CONSOLE_FLAGS);
}

pub fn spawn(connection: &ConnectionConfig, keepalive: &KeepaliveConfig) -> std::io::Result<Child> {
    if connection.has_password() {
        // `sshpass -e` reads the password from the `SSHPASS` env var and feeds
        // it to ssh's stdin/tty, never appearing on the command line.
        spawn_with_sshpass(connection, keepalive)
    } else {
        spawn_direct(connection, keepalive)
    }
}

fn spawn_direct(
    connection: &ConnectionConfig,
    keepalive: &KeepaliveConfig,
) -> std::io::Result<Child> {
    let program = connection.ssh_path.clone().unwrap_or_else(default_ssh_path);
    let mut command = Command::new(program);
    command.args(args(connection, keepalive));
    configure_quiet_spawn(&mut command);
    command.spawn()
}

/// Spawn ssh indirectly via `sshpass -e` so password auth is non-interactive.
/// `BatchMode=yes` is dropped (it forbids password prompts) and `SSHPASS` is
/// exported via the environment so the password never lands in `ps`.
fn spawn_with_sshpass(
    connection: &ConnectionConfig,
    keepalive: &KeepaliveConfig,
) -> std::io::Result<Child> {
    let ssh = connection.ssh_path.clone().unwrap_or_else(default_ssh_path);
    let mut command = Command::new("sshpass");
    command
        .arg("-e")
        .arg(ssh)
        .args(args(connection, keepalive))
        .env("SSHPASS", connection.password.clone().unwrap_or_default());
    configure_quiet_spawn(&mut command);
    command.spawn()
}

pub fn args(connection: &ConnectionConfig, keepalive: &KeepaliveConfig) -> Vec<String> {
    // `BatchMode=yes` forbids password prompts, so it must be dropped when a
    // password is configured — ssh then tries publickey first and falls back to
    // password auth (via sshpass) if no usable key is available.
    let mut args = vec!["-N".into(), "-T".into()];
    if !connection.has_password() {
        args.push("-o".into());
        args.push("BatchMode=yes".into());
    }
    args.extend([
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
        "-o".into(),
        format!("ConnectTimeout={}", keepalive.connect_timeout),
        "-o".into(),
        format!("ServerAliveInterval={}", keepalive.interval),
        "-o".into(),
        format!("ServerAliveCountMax={}", keepalive.count_max),
        "-o".into(),
        "LogLevel=DEBUG1".into(),
    ]);
    if let Some(port) = connection.port {
        args.push("-p".into());
        args.push(port.to_string());
    }
    args.extend(connection.extra_args.clone());
    for forward in connection.forwards.iter().filter(|forward| forward.enabled) {
        args.push(
            match forward.mode {
                ForwardMode::Local => "-L",
                ForwardMode::Remote => "-R",
                ForwardMode::Dynamic => "-D",
            }
            .into(),
        );
        args.push(forward.forward.clone());
    }
    args.push(connection.destination().clone());
    args
}

// ─── connectivity test ────────────────────────────────────────────────────────

/// Result of a one-shot SSH connectivity probe (`ssh … true`).
#[derive(Debug, Clone)]
pub struct TestOutput {
    pub ok: bool,
    pub message: String,
}

/// Arguments for a connectivity test: no forwards, no `-N`, run a no-op
/// remote command so the server authenticates and exits immediately.
/// Forwards are intentionally omitted — we only want to verify auth + reachability.
pub fn test_args(connection: &ConnectionConfig, keepalive: &KeepaliveConfig) -> Vec<String> {
    let mut args = vec![
        "-o".into(),
        format!("ConnectTimeout={}", keepalive.connect_timeout),
        "-o".into(),
        "LogLevel=ERROR".into(),
    ];
    // Same BatchMode suppression rule as `args()` — password auth can only be
    // exercised when ssh is actually allowed to prompt.
    if !connection.has_password() {
        args.push("-o".into());
        args.push("BatchMode=yes".into());
    }
    if let Some(port) = connection.port {
        args.push("-p".into());
        args.push(port.to_string());
    }
    args.extend(connection.extra_args.clone());
    args.push(connection.destination().clone());
    // A no-op remote command so ssh authenticates, runs nothing, and exits.
    args.push("true".into());
    args
}

/// Spawn ssh, wait up to `timeout`, and return success/failure with ssh's
/// stderr appended when available — it carries the “Permission denied” /
/// “Connection refused” messages users need to diagnose a failure.
///
/// Runs with `BatchMode=yes` to mirror production behaviour: the test passes
/// only when key/agent auth works, which is exactly the path the supervisor
/// uses once the connection is enabled.
pub fn test_connection(
    connection: &ConnectionConfig,
    keepalive: &KeepaliveConfig,
    timeout: Duration,
) -> TestOutput {
    // Same password handling as `spawn` so the probe exercises the auth path
    // the supervisor will actually use once the connection is enabled.
    let need_sshpass = connection.has_password();
    if need_sshpass && !sshpass_available() {
        return TestOutput {
            ok: false,
            message: "password auth requires `sshpass` on PATH; install it and retry".into(),
        };
    }
    let mut command = if need_sshpass {
        let ssh = connection.ssh_path.clone().unwrap_or_else(default_ssh_path);
        let mut command = Command::new("sshpass");
        command
            .arg("-e")
            .arg(ssh)
            .args(test_args(connection, keepalive))
            .env("SSHPASS", connection.password.clone().unwrap_or_default());
        command
    } else {
        let program = connection.ssh_path.clone().unwrap_or_else(default_ssh_path);
        let mut command = Command::new(program);
        command.args(test_args(connection, keepalive));
        command
    };
    // Same helper the supervisor uses so the test command stays invisible
    // (no console flicker on Windows, no spurious stderr from sshpass).
    configure_quiet_spawn(&mut command);

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return TestOutput {
                ok: false,
                message: format!("cannot spawn ssh: {error}"),
            };
        }
    };

    // Read stderr on a separate thread so the child cannot deadlock by
    // filling the pipe while we busy-wait on `try_wait`.
    let stderr = child.stderr.take().expect("stderr was piped");
    let reader = std::thread::Builder::new()
        .name("ssh-test-stderr".into())
        .spawn(move || {
            use std::io::Read;
            let mut buf = String::new();
            let _ = std::io::BufReader::new(stderr).read_to_string(&mut buf);
            buf
        })
        .ok();

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => break None,
        }
    };

    let stderr_text = reader.and_then(|h| h.join().ok()).unwrap_or_default();
    let trimmed = stderr_text.trim();
    let tail = if trimmed.is_empty() {
        String::new()
    } else {
        format!(" — {trimmed}")
    };

    match status {
        Some(status) if status.success() => TestOutput {
            ok: true,
            message: format!("connected to {}{}", connection.destination(), tail),
        },
        Some(status) => TestOutput {
            ok: false,
            message: format!("ssh exited with {status}{tail}"),
        },
        None => TestOutput {
            ok: false,
            message: format!("test timed out after {}s{tail}", timeout.as_secs()),
        },
    }
}

/// Check whether `sshpass` can be started from PATH.
///
/// This deliberately checks process creation rather than the exit status of
/// `sshpass -V`: some Windows builds expose a different version flag (or return
/// a non-zero status for it) even though the executable itself is usable.
/// Do not memoise this result; it lets a running GUI recognise an executable
/// copied into an existing PATH directory after an earlier failed probe.
fn sshpass_available() -> bool {
    // Run the availability check through the same helper as the real spawn
    // so we don't briefly allocate a console window just to ask "are you
    // there?" of `sshpass -V`. Without CREATE_NO_WINDOW on Windows, this
    // single line produces the only visible flicker in the test-connection
    // flow.
    let mut command = std::process::Command::new("sshpass");
    command.arg("-V");
    configure_quiet_spawn(&mut command);
    command.status().is_ok()
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
#[path = "../tests/unit/ssh.rs"]
mod tests;
