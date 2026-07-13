use std::collections::HashMap;

use crate::config::{ConnectionConfig, ForwardConfig, ForwardMode};

pub(crate) fn describe_configured_forwards(connection: &ConnectionConfig) -> String {
    let forwards = connection
        .forwards
        .iter()
        .map(ForwardInfo::from_config)
        .collect::<Vec<_>>();
    describe_forward_list(&forwards)
}

pub(crate) struct SshStderrAnnotator {
    forwards: Vec<ForwardInfo>,
    channels: HashMap<u32, ChannelInfo>,
    pending_local: Option<ChannelInfo>,
    pending_remote: Option<ChannelInfo>,
    last_forwarded_channel: Option<u32>,
}

impl SshStderrAnnotator {
    pub(crate) fn new(connection: &ConnectionConfig) -> Self {
        Self {
            forwards: connection
                .forwards
                .iter()
                .map(ForwardInfo::from_config)
                .collect(),
            channels: HashMap::new(),
            pending_local: None,
            pending_remote: None,
            last_forwarded_channel: None,
        }
    }

    pub(crate) fn annotate(&mut self, line: &str) -> Option<String> {
        self.learn(line);
        if let Some(channel) = parse_channel_open_failed(line) {
            return Some(self.annotate_channel_failure(line, channel));
        }
        if let Some((host, port)) = parse_connect_to_failure(line) {
            return Some(self.annotate_connect_to_failure(line, &host, &port));
        }
        if is_debug_line(line) {
            return None;
        }
        Some(line.to_owned())
    }

    fn learn(&mut self, line: &str) {
        if let Some(channel) = parse_channel_free(line) {
            self.channels.remove(&channel);
            if self.last_forwarded_channel == Some(channel) {
                self.last_forwarded_channel = None;
            }
        }

        if let Some((listen_port, target_host, target_port)) = parse_local_forward_request(line) {
            let forward = self.find_local_forward(&listen_port, &target_host, &target_port);
            let listen = forward
                .as_ref()
                .and_then(ForwardInfo::listen_display)
                .unwrap_or_else(|| listen_port.clone());
            self.pending_local = Some(ChannelInfo {
                mode: ForwardMode::Local,
                listen: Some(listen),
                target: Some(format_endpoint(&target_host, &target_port)),
                originator: None,
                forward,
            });
        }

        if let Some(channel) = parse_channel_new_kind(line, "direct-tcpip")
            && let Some(info) = self.pending_local.take()
        {
            self.channels.insert(channel, info);
        }

        if let Some((listen_host, listen_port, origin_host, origin_port)) =
            parse_remote_forward_request(line)
        {
            let forward = self.find_remote_forward(&listen_host, &listen_port);
            let listen = forward
                .as_ref()
                .and_then(ForwardInfo::listen_display)
                .unwrap_or_else(|| format_endpoint(&listen_host, &listen_port));
            let target = forward.as_ref().and_then(ForwardInfo::target_display);
            self.pending_remote = Some(ChannelInfo {
                mode: ForwardMode::Remote,
                listen: Some(listen),
                target,
                originator: Some(format_endpoint(&origin_host, &origin_port)),
                forward,
            });
        }

        if let Some((target_host, target_port)) = parse_connect_next_start(line) {
            let target = format_endpoint(&target_host, &target_port);
            let forward = self.find_forward_by_target(&target_host, &target_port);
            if let Some(info) = self.pending_remote.as_mut() {
                info.target = Some(target.clone());
                if info.forward.is_none() {
                    info.forward = forward.clone();
                }
            }
            if let Some(channel) = self.last_forwarded_channel
                && let Some(info) = self.channels.get_mut(&channel)
            {
                info.target = Some(target);
                if info.forward.is_none() {
                    info.forward = forward;
                }
            }
        }

        if let Some(channel) = parse_channel_new_kind(line, "forwarded-tcpip")
            && let Some(info) = self.pending_remote.take()
        {
            self.channels.insert(channel, info);
            self.last_forwarded_channel = Some(channel);
        }
    }

    fn annotate_channel_failure(&self, line: &str, channel: u32) -> String {
        if let Some(info) = self.channels.get(&channel) {
            format!("{line}; {}", info.describe())
        } else {
            format!(
                "{line}; channel mapping unavailable; configured forwards=[{}]",
                describe_forward_list(&self.forwards)
            )
        }
    }

    fn annotate_connect_to_failure(
        &self,
        line: &str,
        target_host: &str,
        target_port: &str,
    ) -> String {
        if let Some(forward) = self.find_forward_by_target(target_host, target_port) {
            format!("{line}; forward={}", forward.display())
        } else {
            line.to_owned()
        }
    }

    fn find_local_forward(
        &self,
        listen_port: &str,
        target_host: &str,
        target_port: &str,
    ) -> Option<ForwardInfo> {
        self.forwards
            .iter()
            .find(|forward| forward.matches_local_request(listen_port, target_host, target_port))
            .or_else(|| {
                self.forwards.iter().find(|forward| {
                    forward.mode == ForwardMode::Local
                        && forward.listen_port.as_deref() == Some(listen_port)
                })
            })
            .cloned()
    }

    fn find_remote_forward(&self, listen_host: &str, listen_port: &str) -> Option<ForwardInfo> {
        self.forwards
            .iter()
            .find(|forward| forward.matches_remote_request(listen_host, listen_port))
            .cloned()
    }

    fn find_forward_by_target(&self, target_host: &str, target_port: &str) -> Option<ForwardInfo> {
        self.forwards
            .iter()
            .find(|forward| forward.matches_target(target_host, target_port))
            .cloned()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ForwardInfo {
    mode: ForwardMode,
    raw: String,
    listen_host: Option<String>,
    listen_port: Option<String>,
    target_host: Option<String>,
    target_port: Option<String>,
}

impl ForwardInfo {
    fn from_config(forward: &ForwardConfig) -> Self {
        let parsed = parse_forward_spec(&forward.forward);
        Self {
            mode: forward.mode,
            raw: forward.forward.clone(),
            listen_host: parsed.as_ref().and_then(|parsed| parsed.0.clone()),
            listen_port: parsed.as_ref().map(|parsed| parsed.1.clone()),
            target_host: parsed.as_ref().map(|parsed| parsed.2.clone()),
            target_port: parsed.map(|parsed| parsed.3),
        }
    }

    fn flag(&self) -> &'static str {
        match self.mode {
            ForwardMode::Local => "-L",
            ForwardMode::Remote => "-R",
        }
    }

    fn listen_display(&self) -> Option<String> {
        let port = self.listen_port.as_ref()?;
        match &self.listen_host {
            Some(host) if !host.is_empty() => Some(format!("{host}:{port}")),
            _ => Some(port.clone()),
        }
    }

    fn target_display(&self) -> Option<String> {
        Some(format_endpoint(
            self.target_host.as_ref()?,
            self.target_port.as_ref()?,
        ))
    }

    fn display(&self) -> String {
        match (self.listen_display(), self.target_display()) {
            (Some(listen), Some(target)) => format!("{} {listen} -> {target}", self.flag()),
            _ => format!("{} {}", self.flag(), self.raw),
        }
    }

    fn matches_local_request(
        &self,
        listen_port: &str,
        target_host: &str,
        target_port: &str,
    ) -> bool {
        self.mode == ForwardMode::Local
            && self.listen_port.as_deref() == Some(listen_port)
            && self.target_port.as_deref() == Some(target_port)
            && self
                .target_host
                .as_ref()
                .is_some_and(|host| hosts_match(host, target_host))
    }

    fn matches_remote_request(&self, listen_host: &str, listen_port: &str) -> bool {
        if self.mode != ForwardMode::Remote || self.listen_port.as_deref() != Some(listen_port) {
            return false;
        }
        match &self.listen_host {
            Some(config_host) if !config_host.is_empty() && !is_wildcard_host(config_host) => {
                hosts_match(config_host, listen_host)
            }
            _ => true,
        }
    }

    fn matches_target(&self, target_host: &str, target_port: &str) -> bool {
        self.target_port.as_deref() == Some(target_port)
            && self
                .target_host
                .as_ref()
                .is_some_and(|host| hosts_match(host, target_host))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ChannelInfo {
    mode: ForwardMode,
    listen: Option<String>,
    target: Option<String>,
    originator: Option<String>,
    forward: Option<ForwardInfo>,
}

impl ChannelInfo {
    fn describe(&self) -> String {
        let mut parts = Vec::new();
        let forward = self
            .forward
            .as_ref()
            .map(ForwardInfo::display)
            .unwrap_or_else(|| {
                let flag = match self.mode {
                    ForwardMode::Local => "-L",
                    ForwardMode::Remote => "-R",
                };
                match (&self.listen, &self.target) {
                    (Some(listen), Some(target)) => format!("{flag} {listen} -> {target}"),
                    (Some(listen), None) => format!("{flag} {listen} -> ?"),
                    (None, Some(target)) => format!("{flag} ? -> {target}"),
                    (None, None) => format!("{flag} ?"),
                }
            });
        parts.push(format!("forward={forward}"));
        if let Some(originator) = &self.originator {
            parts.push(format!("originator={originator}"));
        }
        parts.join("; ")
    }
}

fn describe_forward_list(forwards: &[ForwardInfo]) -> String {
    if forwards.is_empty() {
        return "none".into();
    }
    forwards
        .iter()
        .map(ForwardInfo::display)
        .collect::<Vec<_>>()
        .join(", ")
}

fn parse_forward_spec(spec: &str) -> Option<(Option<String>, String, String, String)> {
    let parts = split_forward_spec(spec);
    match parts.as_slice() {
        [listen_port, target_host, target_port] if !listen_port.is_empty() => Some((
            None,
            listen_port.clone(),
            target_host.clone(),
            target_port.clone(),
        )),
        [listen_host, listen_port, target_host, target_port] if !listen_port.is_empty() => Some((
            if listen_host.is_empty() {
                None
            } else {
                Some(listen_host.clone())
            },
            listen_port.clone(),
            target_host.clone(),
            target_port.clone(),
        )),
        _ => None,
    }
}

fn split_forward_spec(spec: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_brackets = false;
    for ch in spec.chars() {
        match ch {
            '[' => {
                in_brackets = true;
                current.push(ch);
            }
            ']' => {
                in_brackets = false;
                current.push(ch);
            }
            ':' if !in_brackets => {
                parts.push(current);
                current = String::new();
            }
            _ => current.push(ch),
        }
    }
    parts.push(current);
    parts
}

fn parse_channel_open_failed(line: &str) -> Option<u32> {
    line.contains(": open failed:")
        .then(|| parse_channel_number(line))
        .flatten()
}

fn parse_channel_free(line: &str) -> Option<u32> {
    line.contains(": free:")
        .then(|| parse_channel_number(line))
        .flatten()
}

fn parse_channel_new_kind(line: &str, kind: &str) -> Option<u32> {
    (line.contains(&format!(": new [{kind}]")) || line.contains(&format!(": new {kind}")))
        .then(|| parse_channel_number(line))
        .flatten()
}

fn parse_channel_number(line: &str) -> Option<u32> {
    let rest = line.split_once("channel ")?.1;
    let digit_count = rest.chars().take_while(char::is_ascii_digit).count();
    if digit_count == 0 || !rest[digit_count..].starts_with(':') {
        return None;
    }
    rest[..digit_count].parse().ok()
}

fn parse_local_forward_request(line: &str) -> Option<(String, String, String)> {
    let rest = line.split_once("Connection to port ")?.1;
    let (listen_port, rest) = rest.split_once(" forwarding to ")?;
    let (target_host, rest) = rest.split_once(" port ")?;
    Some((
        listen_port.trim().to_owned(),
        target_host.trim().to_owned(),
        take_token(rest)?,
    ))
}

fn parse_remote_forward_request(line: &str) -> Option<(String, String, String, String)> {
    let rest = line
        .split_once("client_request_forwarded_tcpip: listen ")?
        .1;
    let (listen, originator) = rest.split_once(", originator ")?;
    let (listen_host, listen_port) = parse_host_port_phrase(listen)?;
    let (origin_host, origin_port) = parse_host_port_phrase(originator)?;
    Some((listen_host, listen_port, origin_host, origin_port))
}

fn parse_connect_next_start(line: &str) -> Option<(String, String)> {
    let rest = line.split_once("connect_next: start for host ")?.1;
    let host = rest.split_whitespace().next()?.to_owned();
    if let Some(port) = rest.rsplit_once(":").and_then(|(_, port)| take_token(port)) {
        return Some((host, port));
    }
    let (_, port) = rest.rsplit_once(" port ")?;
    Some((host, take_token(port)?))
}

fn parse_connect_to_failure(line: &str) -> Option<(String, String)> {
    if !line.contains(": failed") {
        return None;
    }
    let rest = line.split_once("connect_to ")?.1;
    let (host, port) = rest.split_once(" port ")?;
    Some((host.trim().to_owned(), take_token(port)?))
}

fn parse_host_port_phrase(text: &str) -> Option<(String, String)> {
    let (host, port) = text.trim().split_once(" port ")?;
    Some((host.trim().to_owned(), take_token(port)?))
}

fn take_token(text: &str) -> Option<String> {
    let token = text
        .trim()
        .split(|ch: char| ch == ':' || ch == ',' || ch == ')' || ch.is_whitespace())
        .next()?
        .trim_end_matches('.');
    (!token.is_empty()).then(|| token.to_owned())
}

fn format_endpoint(host: &str, port: &str) -> String {
    format!("{host}:{port}")
}

fn is_debug_line(line: &str) -> bool {
    line.starts_with("debug1:") || line.starts_with("debug2:") || line.starts_with("debug3:")
}

fn hosts_match(left: &str, right: &str) -> bool {
    let left = normalize_host(left);
    let right = normalize_host(right);
    left == right || (is_loopback_name(&left) && is_loopback_name(&right))
}

fn normalize_host(host: &str) -> String {
    host.trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase()
}

fn is_loopback_name(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1")
}

fn is_wildcard_host(host: &str) -> bool {
    matches!(normalize_host(host).as_str(), "*" | "0.0.0.0" | "::")
}

#[cfg(test)]
#[path = "../tests/unit/ssh_log.rs"]
mod tests;
