use super::*;
use std::path::PathBuf;

fn connection() -> ConnectionConfig {
    ConnectionConfig {
        name: "primary".into(),
        host: Some("user@example.test".into()),
        enabled: true,
        ssh_path: None,
        keepalive: KeepaliveConfig::default(),
        retry: RetryConfig::default(),
        extra_args: vec!["-v".into()],
        forwards: vec![ForwardConfig {
            mode: ForwardMode::Local,
            forward: "8080:127.0.0.1:8080".into(),
        }],
    }
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
fn rejects_dynamic_forward_without_port() {
    let mut config = Config {
        log: LogConfig::default(),
        connections: vec![connection()],
    };
    config.connections[0].forwards = vec![ForwardConfig {
        mode: ForwardMode::Dynamic,
        forward: "0.0.0.0:".into(),
    }];
    assert!(config.validate().is_err());
}

#[test]
fn rejects_dynamic_forward_with_extra_target_segments() {
    // `-D` only takes `[bind:]port`; anything with a third colon is rejected so
    // users do not silently produce an invalid `ssh -D` invocation.
    let mut config = Config {
        log: LogConfig::default(),
        connections: vec![connection()],
    };
    config.connections[0].forwards = vec![ForwardConfig {
        mode: ForwardMode::Dynamic,
        forward: "1080:127.0.0.1:8080".into(),
    }];
    assert!(config.validate().is_err());
}

#[test]
fn accepts_dynamic_forward_specs() {
    let mut config = Config {
        log: LogConfig::default(),
        connections: vec![connection()],
    };
    config.connections[0].forwards = vec![
        ForwardConfig {
            mode: ForwardMode::Dynamic,
            forward: "1080".into(),
        },
        ForwardConfig {
            mode: ForwardMode::Dynamic,
            forward: "0.0.0.0:1080".into(),
        },
        ForwardConfig {
            mode: ForwardMode::Dynamic,
            forward: "[::1]:1080".into(),
        },
    ];
    assert!(config.validate().is_ok());
}

#[test]
fn save_and_load_roundtrip() {
    let dir = std::env::temp_dir().join("autossh-ui-roundtrip");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let _ = std::fs::remove_file(&path);
    let mut config = Config {
        log: LogConfig {
            file: Some(PathBuf::from("/tmp/autossh.log")),
        },
        connections: vec![connection()],
    };
    // Vary keepalive/retry so the roundtrip exercises non-default values too.
    config.connections[0].keepalive.interval = 42;
    config.connections[0].retry.maximum_seconds = 90;
    config.save(&path).unwrap();
    let reloaded = Config::load(&path).unwrap();
    assert_eq!(config, reloaded);
    let _ = std::fs::remove_file(&path);
}
