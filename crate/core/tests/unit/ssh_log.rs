use super::*;
use crate::config::{ConnectionConfig, ForwardConfig, ForwardMode};

fn connection() -> ConnectionConfig {
    ConnectionConfig {
        name: "test".into(),
        description: None,
        host: None,
        user: None,
        password: None,
        port: None,
        enabled: true,
        ssh_path: None,
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
fn annotates_local_channel_failures_with_forward_port() {
    let mut annotator = SshStderrAnnotator::new(&connection());

    assert_eq!(
        annotator.annotate(
            "debug1: Connection to port 8080 forwarding to 127.0.0.1 port 8080 requested."
        ),
        None
    );
    assert_eq!(
        annotator.annotate("debug1: channel 2: new [direct-tcpip]"),
        None
    );

    let line = annotator
        .annotate("channel 2: open failed: connect failed: Connection refused")
        .unwrap();
    assert!(line.contains("forward=-L 8080 -> 127.0.0.1:8080"));
}

#[test]
fn annotates_remote_channel_failures_with_forward_and_originator() {
    let mut connection = connection();
    connection.forwards[0].mode = ForwardMode::Remote;
    connection.forwards[0].forward = "10022:127.0.0.1:22".into();
    let mut annotator = SshStderrAnnotator::new(&connection);

    assert_eq!(
        annotator.annotate(
            "debug1: client_request_forwarded_tcpip: listen localhost port 10022, originator 203.0.113.5 port 51234"
        ),
        None
    );
    assert_eq!(
        annotator.annotate("debug1: channel 3: new [forwarded-tcpip]"),
        None
    );

    let line = annotator
        .annotate("channel 3: open failed: connect failed: Connection refused")
        .unwrap();
    assert!(line.contains("forward=-R 10022 -> 127.0.0.1:22"));
    assert!(line.contains("originator=203.0.113.5:51234"));
}

#[test]
fn falls_back_to_configured_forwards_without_channel_context() {
    let mut annotator = SshStderrAnnotator::new(&connection());
    let line = annotator
        .annotate("channel 9: open failed: connect failed: Connection refused")
        .unwrap();

    assert!(line.contains("channel mapping unavailable"));
    assert!(line.contains("configured forwards=[-L 8080 -> 127.0.0.1:8080]"));
}

#[test]
fn dynamic_forward_displays_bind_and_port_without_target() {
    let mut conn = connection();
    conn.forwards = vec![ForwardConfig {
        enabled: true,
        mode: ForwardMode::Dynamic,
        forward: "0.0.0.0:1080".into(),
        description: None,
    }];
    let mut annotator = SshStderrAnnotator::new(&conn);
    let line = annotator
        .annotate("channel 1: open failed: administratively prohibited")
        .unwrap();
    assert!(
        line.contains("-D 0.0.0.0:1080"),
        "expected -D display, got: {line}"
    );
    assert!(
        !line.contains("->"),
        "dynamic forwards have no configured target, got: {line}"
    );
}

#[test]
fn dynamic_forward_accepts_bare_port_spec() {
    let mut conn = connection();
    conn.forwards = vec![ForwardConfig {
        enabled: true,
        mode: ForwardMode::Dynamic,
        forward: "1080".into(),
        description: None,
    }];
    let mut annotator = SshStderrAnnotator::new(&conn);
    let line = annotator
        .annotate("channel 1: open failed: administratively prohibited")
        .unwrap();
    assert!(line.contains("-D 1080"));
}
