use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
};

use crate::config::{ConnectionConfig, ForwardMode};

pub fn spawn(connection: &ConnectionConfig) -> std::io::Result<Child> {
    let program = connection.ssh_path.clone().unwrap_or_else(default_ssh_path);
    Command::new(program)
        .args(args(connection))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
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
