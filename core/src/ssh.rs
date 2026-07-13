use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Prevent `ssh.exe` from allocating a visible console when launched by the
/// native GUI or the background supervisor.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

use crate::config::{ConnectionConfig, ForwardMode};

pub fn spawn(connection: &ConnectionConfig) -> std::io::Result<Child> {
    let program = connection.ssh_path.clone().unwrap_or_else(default_ssh_path);
    let mut command = Command::new(program);
    command
        .args(args(connection))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command.spawn()
}

pub fn args(connection: &ConnectionConfig) -> Vec<String> {
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
        "-o".into(),
        "LogLevel=DEBUG1".into(),
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
    args.push(connection.destination().to_owned());
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
#[path = "../tests/unit/ssh.rs"]
mod tests;
