use super::*;

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
