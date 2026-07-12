use super::*;
use crate::config::{ForwardConfig, KeepaliveConfig, RetryConfig};

fn connection() -> ConnectionConfig {
    ConnectionConfig {
        name: "primary".into(),
        host: Some("user@example.test".into()),
        enabled: true,
        ssh_path: None,
        keepalive: KeepaliveConfig::default(),
        retry: RetryConfig::default(),
        extra_args: vec!["-v".into()],
        forwards: vec![
            ForwardConfig {
                mode: ForwardMode::Local,
                forward: "8080:127.0.0.1:8080".into(),
            },
            ForwardConfig {
                mode: ForwardMode::Remote,
                forward: "10022:127.0.0.1:22".into(),
            },
        ],
    }
}

#[test]
fn creates_multiple_ssh_forward_arguments() {
    let args = args(&connection());
    assert_eq!(args[0..2], ["-N", "-T"]);
    assert!(
        args.windows(2)
            .any(|part| part == ["-L", "8080:127.0.0.1:8080"])
    );
    assert!(
        args.windows(2)
            .any(|part| part == ["-R", "10022:127.0.0.1:22"])
    );
    assert_eq!(args[args.len() - 1], "user@example.test");
}

#[test]
fn name_remains_the_default_destination() {
    let mut connection = connection();
    connection.host = None;
    assert_eq!(args(&connection).last().unwrap(), "primary");
}
