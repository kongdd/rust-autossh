use super::*;
use crate::config::{ForwardConfig, KeepaliveConfig, RetryConfig};

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
        forwards: vec![
            ForwardConfig {
                enabled: true,
                mode: ForwardMode::Local,
                forward: "8080:127.0.0.1:8080".into(),
                description: None,
            },
            ForwardConfig {
                enabled: true,
                mode: ForwardMode::Remote,
                forward: "10022:127.0.0.1:22".into(),
                description: None,
            },
        ],
    }
}

#[test]
fn creates_multiple_ssh_forward_arguments() {
    let args = args(&connection(), &KeepaliveConfig::default());
    assert_eq!(args[0..2], ["-N", "-T"]);
    assert!(
        args.windows(2)
            .any(|part| part == ["-L", "8080:127.0.0.1:8080"])
    );
    assert!(
        args.windows(2)
            .any(|part| part == ["-R", "10022:127.0.0.1:22"])
    );
    assert!(
        args.windows(2)
            .any(|part| part == ["-o", "LogLevel=DEBUG1"])
    );
    assert_eq!(args[args.len() - 1], "user@example.test");
}

#[test]
fn emits_dynamic_forward_argument_as_dash_d() {
    let mut conn = connection();
    conn.forwards.push(ForwardConfig {
        enabled: true,
        mode: ForwardMode::Dynamic,
        forward: "1080".into(),
        description: None,
    });
    let args = args(&conn, &KeepaliveConfig::default());
    assert!(
        args.windows(2).any(|part| part == ["-D", "1080"]),
        "missing -D port pair in {args:?}"
    );
}

#[test]
fn disabled_forwards_are_omitted() {
    let mut conn = connection();
    conn.forwards[0].enabled = false;
    let args = args(&conn, &KeepaliveConfig::default());
    assert!(!args.iter().any(|arg| arg == "-L"));
    assert!(args.iter().any(|arg| arg == "-R"));
}

#[test]
fn emits_dynamic_forward_with_bind_address() {
    let mut conn = connection();
    conn.forwards = vec![ForwardConfig {
        enabled: true,
        mode: ForwardMode::Dynamic,
        forward: "0.0.0.0:1080".into(),
        description: None,
    }];
    let args = args(&conn, &KeepaliveConfig::default());
    assert!(args.windows(2).any(|part| part == ["-D", "0.0.0.0:1080"]));
}

#[test]
fn name_remains_the_default_destination() {
    let mut connection = connection();
    connection.host = None;
    assert_eq!(
        args(&connection, &KeepaliveConfig::default())
            .last()
            .unwrap(),
        "primary"
    );
}

#[test]
fn user_prependes_user_at_when_host_has_no_at() {
    let mut connection = connection();
    connection.host = Some("example.test".into());
    connection.user = Some("alice".into());
    assert_eq!(connection.destination(), "alice@example.test");
}

#[test]
fn user_is_ignored_when_host_already_contains_user_at() {
    // `host` already embeds the user; a separate `user` field should NOT
    // double it into `user@user@host`.
    let mut connection = connection();
    connection.host = Some("bob@example.test".into());
    connection.user = Some("alice".into());
    assert_eq!(connection.destination(), "bob@example.test");
}

#[test]
fn empty_user_is_equivalent_to_none() {
    let mut connection = connection();
    connection.host = Some("example.test".into());
    connection.user = Some("   ".into());
    assert_eq!(connection.destination(), "example.test");
}

#[test]
fn has_password_is_false_when_unset_or_blank() {
    let mut connection = connection();
    assert!(!connection.has_password());
    connection.password = Some("".into());
    assert!(!connection.has_password());
    connection.password = Some("   ".into());
    assert!(
        connection.has_password(),
        "whitespace-only is still a credential boundary"
    );
}

#[test]
fn args_drop_batchmode_when_password_is_set() {
    let mut connection = connection();
    connection.password = Some("s3cret".into());
    let args = args(&connection, &KeepaliveConfig::default());
    assert!(
        !args.windows(2).any(|w| w == ["-o", "BatchMode=yes"]),
        "BatchMode must be dropped when a password is set: {args:?}"
    );
}

#[test]
fn args_keep_batchmode_for_key_auth() {
    // Default (no password) preserves the historical key-only auth path.
    let args = args(&connection(), &KeepaliveConfig::default());
    assert!(
        args.windows(2).any(|w| w == ["-o", "BatchMode=yes"]),
        "BatchMode must remain for key/agent auth: {args:?}"
    );
}

#[test]
fn args_include_configured_ssh_port() {
    let mut conn = connection();
    conn.port = Some(2202);
    assert!(
        &args(&conn, &KeepaliveConfig::default())
            .windows(2)
            .any(|part| part == ["-p", "2202"])
    );
    assert!(
        test_args(&conn, &KeepaliveConfig::default())
            .windows(2)
            .any(|part| part == ["-p", "2202"])
    );
}

#[test]
fn test_args_omit_forwards_and_run_a_noop_command() {
    // The probe must authenticate and exit, not open a tunnel: no `-N`, no
    // forwards, and a trailing `true` so the server runs nothing.
    let args = test_args(&connection(), &KeepaliveConfig::default());
    assert!(
        args.windows(2).any(|part| part == ["-o", "BatchMode=yes"]),
        "expected BatchMode=yes in {args:?}"
    );
    assert!(
        args.iter().any(|a| a.starts_with("ConnectTimeout=")),
        "expected ConnectTimeout in {args:?}"
    );
    assert!(
        !args.iter().any(|a| a == "-N"),
        "probe must not request no-command"
    );
    assert!(
        !args.iter().any(|a| a == "-L" || a == "-R" || a == "-D"),
        "probe must not forward"
    );
    assert_eq!(args[args.len() - 2], "user@example.test");
    assert_eq!(args[args.len() - 1], "true");
}
