use super::*;
use std::path::PathBuf;

fn connection() -> ConnectionConfig {
    ConnectionConfig {
        name: "primary".into(),
        description: None,
        host: Some("user@example.test".into()),
        user: None,
        password: None,
        port: None,
        enabled: true,
        ssh_path: None,
        extra_args: vec!["-v".into()],
        forwards: vec![ForwardConfig {
            enabled: true,
            mode: ForwardMode::Local,
            forward: "8080:127.0.0.1:8080".into(),
            description: None,
        }],
    }
}

#[test]
fn rejects_invalid_retry_range() {
    let mut config = Config {
        log: LogConfig::default(),
        keepalive: KeepaliveConfig::default(),
        retry: RetryConfig::default(),
        connections: vec![connection()],
    };
    config.retry.maximum_seconds = 0;
    assert!(config.validate().is_err());
}

#[test]
fn rejects_invalid_keepalive_values() {
    let mut config = Config {
        log: LogConfig::default(),
        keepalive: KeepaliveConfig {
            interval: 0,
            ..KeepaliveConfig::default()
        },
        retry: RetryConfig::default(),
        connections: vec![connection()],
    };
    assert!(config.validate().is_err());
}

#[test]
fn rejects_ssh_port_zero() {
    let config = Config {
        log: LogConfig::default(),
        keepalive: KeepaliveConfig::default(),
        retry: RetryConfig::default(),
        connections: [connection()]
            .into_iter()
            .map(|mut c| {
                c.port = Some(0);
                c
            })
            .collect(),
    };
    assert!(config.validate().is_err());
}

#[test]
fn rejects_dynamic_forward_without_port() {
    let mut connection = connection();
    connection.forwards = vec![ForwardConfig {
        enabled: true,
        mode: ForwardMode::Dynamic,
        forward: "0.0.0.0:".into(),
        description: None,
    }];
    let config = Config {
        log: LogConfig::default(),
        keepalive: KeepaliveConfig::default(),
        retry: RetryConfig::default(),
        connections: vec![connection],
    };
    assert!(config.validate().is_err());
}

#[test]
fn rejects_dynamic_forward_with_extra_target_segments() {
    // `-D` only takes `[bind:]port`; anything with a third colon is rejected so
    // users do not silently produce an invalid `ssh -D` invocation.
    let mut connection = connection();
    connection.forwards = vec![ForwardConfig {
        enabled: true,
        mode: ForwardMode::Dynamic,
        forward: "1080:127.0.0.1:8080".into(),
        description: None,
    }];
    let config = Config {
        log: LogConfig::default(),
        keepalive: KeepaliveConfig::default(),
        retry: RetryConfig::default(),
        connections: vec![connection],
    };
    assert!(config.validate().is_err());
}

#[test]
fn accepts_dynamic_forward_specs() {
    let mut connection = connection();
    connection.forwards = vec![
        ForwardConfig {
            enabled: true,
            mode: ForwardMode::Dynamic,
            forward: "1080".into(),
            description: None,
        },
        ForwardConfig {
            enabled: true,
            mode: ForwardMode::Dynamic,
            forward: "0.0.0.0:1080".into(),
            description: None,
        },
        ForwardConfig {
            enabled: true,
            mode: ForwardMode::Dynamic,
            forward: "[::1]:1080".into(),
            description: None,
        },
    ];
    let config = Config {
        log: LogConfig::default(),
        keepalive: KeepaliveConfig::default(),
        retry: RetryConfig::default(),
        connections: vec![connection],
    };
    assert!(config.validate().is_ok());
}

#[test]
fn missing_forward_enabled_defaults_to_true() {
    let config: Config = toml::from_str(
        r#"
        keepalive = { interval = 30 }
        retry     = { maximum_seconds = 90 }
        [[connections]]
        name = "legacy"
        forwards = [{ mode = "local", forward = "8080:127.0.0.1:8080" }]
        "#,
    )
    .unwrap();
    assert_eq!(config.keepalive.interval, 30);
    assert_eq!(config.retry.maximum_seconds, 90);
    assert!(config.connections[0].forwards[0].enabled);
}

#[test]
fn save_and_load_roundtrip() {
    let dir = std::env::temp_dir().join("autossh-ui-roundtrip");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let _ = std::fs::remove_file(&path);
    let mut connection = connection();
    connection.description = Some("test gateway".into());
    connection.port = Some(2202);
    connection.forwards[0].description = Some("home web".into());
    connection.forwards[0].enabled = false;
    let mut config = Config {
        log: LogConfig {
            file: Some(PathBuf::from("/tmp/autossh.log")),
        },
        keepalive: KeepaliveConfig {
            interval: 42,
            ..KeepaliveConfig::default()
        },
        retry: RetryConfig {
            maximum_seconds: 90,
            ..RetryConfig::default()
        },
        connections: vec![connection],
    };
    config.save(&path).unwrap();
    let reloaded = Config::load(&path).unwrap();
    assert_eq!(config, reloaded);
    assert_eq!(
        reloaded.connections[0].forwards[0].description.as_deref(),
        Some("home web")
    );
    assert!(!reloaded.connections[0].forwards[0].enabled);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn top_level_globals_round_trip() {
    // ensure keepalive/retry serialise at the top level (not under [[connections]]).
    let dir = std::env::temp_dir().join("autossh-ui-globals-shape");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let _ = std::fs::remove_file(&path);
    let config = Config {
        log: LogConfig::default(),
        keepalive: KeepaliveConfig {
            interval: 45,
            ..KeepaliveConfig::default()
        },
        retry: RetryConfig {
            initial_seconds: 2,
            ..RetryConfig::default()
        },
        connections: vec![connection()],
    };
    config.save(&path).unwrap();
    let body = std::fs::read_to_string(&path).unwrap();
    assert!(
        body.contains("[keepalive]") || body.contains("keepalive = {"),
        "TOML: {body}"
    );
    let reloaded = Config::load(&path).unwrap();
    assert_eq!(reloaded.keepalive.interval, 45);
    assert_eq!(reloaded.retry.initial_seconds, 2);
    let _ = std::fs::remove_file(&path);
}
