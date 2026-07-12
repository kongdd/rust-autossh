//! Supervises OpenSSH tunnel processes; does not implement SSH itself.

pub mod config;
pub mod logger;
pub mod ssh;
pub mod supervisor;

pub use config::Config;
pub use supervisor::run;

#[cfg(test)]
mod tests {
    use super::config::*;

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
        let args = super::ssh::args(&connection());
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
        assert_eq!(super::ssh::args(&connection).last().unwrap(), "primary");
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
    fn snapshot_detects_same_size_content_change() {
        use std::{fs, process};
        let path = std::env::temp_dir().join(format!("rust-autossh-{}", process::id()));
        fs::write(&path, b"first").unwrap();
        let first = super::supervisor::config_snapshot(&path);
        fs::write(&path, b"other").unwrap();
        assert_ne!(first, super::supervisor::config_snapshot(&path));
        fs::remove_file(path).unwrap();
    }
}
