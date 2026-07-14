use super::*;
use crate::config::{ForwardConfig, ForwardMode, KeepaliveConfig, RetryConfig};

fn connection() -> ConnectionConfig {
    ConnectionConfig {
        name: "test".into(),
        description: None,
        host: None,
        user: None,
        password: None,
        port: None,
        enabled: true,
        ssh_path: Some("ssh-command-that-does-not-exist".into()),
        extra_args: Vec::new(),
        forwards: vec![ForwardConfig {
            enabled: true,
            mode: ForwardMode::Local,
            forward: "8080:127.0.0.1:8080".into(),
            description: None,
        }],
    }
}

#[test]
fn snapshot_detects_same_size_content_change() {
    use std::{fs, process};
    let path = std::env::temp_dir().join(format!("rust-autossh-{}", process::id()));
    fs::write(&path, b"first").unwrap();
    let first = config_snapshot(&path);
    fs::write(&path, b"other").unwrap();
    assert_ne!(first, config_snapshot(&path));
    fs::remove_file(path).unwrap();
}

#[test]
fn decodes_utf8_stderr_and_removes_line_endings() {
    assert_eq!(decode_ssh_stderr("连接失败\r\n".as_bytes()), "连接失败");
}

#[test]
fn reconfigure_restarts_only_changed_workers() {
    let logger = Logger::new(Path::new("config.toml"), &Default::default()).unwrap();
    let original = connection();
    let mut supervisor = Supervisor::start(
        vec![original.clone()],
        KeepaliveConfig::default(),
        RetryConfig::default(),
        logger.clone(),
    )
    .unwrap();
    let first_thread = supervisor.workers["test"].handle.thread().id();

    supervisor
        .reconfigure(
            vec![original.clone()],
            KeepaliveConfig::default(),
            RetryConfig::default(),
            logger.clone(),
            false,
        )
        .unwrap();
    assert_eq!(
        first_thread,
        supervisor.workers["test"].handle.thread().id()
    );

    let mut changed = original;
    changed.forwards[0].forward = "8081:127.0.0.1:8081".into();
    supervisor
        .reconfigure(
            vec![changed],
            KeepaliveConfig::default(),
            RetryConfig::default(),
            logger,
            false,
        )
        .unwrap();
    assert_ne!(
        first_thread,
        supervisor.workers["test"].handle.thread().id()
    );
    supervisor.stop_all();
}
