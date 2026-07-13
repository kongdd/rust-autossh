//! Parse OpenSSH `ssh_config(5)` into flat host entries.
//!
//! SSH config has no port-forward directives, so the returned entries only
//! carry `user@host:port` and a placeholder reverse tunnel is added after
//! import so the minimal autossh config validates.

use std::path::PathBuf;

use anyhow::Context;

/// One host pulled from `~/.ssh/config`.
#[derive(Debug)]
pub struct SshHostEntry {
    pub alias: String,
    pub destination: String,
    pub port: u16,
    pub selected: bool,
    pub duplicate: bool,
}

/// Resolved `~/.ssh/config`, fall back to `%USERPROFILE%\.ssh\config` on Windows.
pub fn default_ssh_config_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .unwrap_or_else(|| ".".into());
    PathBuf::from(home).join(".ssh").join("config")
}

/// Parses OpenSSH `ssh_config(5)` into a flat list of host entries.
/// Skips `Host *` and aliases containing `*` / `?`.
pub fn parse_ssh_config(path: &std::path::Path) -> anyhow::Result<Vec<SshHostEntry>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("cannot read SSH config {}", path.display()))?;

    let mut entries: Vec<SshHostEntry> = Vec::new();
    let mut aliases: Vec<String> = Vec::new();
    let mut host = String::new();
    let mut user = String::new();
    let mut port: u16 = 22;

    let push_aliases = |entries: &mut Vec<SshHostEntry>,
                        aliases: &[String],
                        host: &str,
                        user: &str,
                        port: u16| {
        for alias in aliases {
            if alias.contains('*') || alias.contains('?') {
                continue;
            }
            let hostname = if host.is_empty() {
                alias.clone()
            } else {
                host.to_string()
            };
            let destination = if user.is_empty() {
                format!("{hostname}:{port}")
            } else {
                format!("{user}@{hostname}:{port}")
            };
            entries.push(SshHostEntry {
                alias: alias.clone(),
                destination,
                port,
                selected: true,
                duplicate: false,
            });
        }
    };

    for raw in text.lines() {
        // Strip trailing `#` comments; OpenSSH treats `#` as a line comment.
        let line = match raw.split_once('#') {
            Some((before, _)) => before,
            None => raw,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let key = match parts.next() {
            Some(k) => k.to_ascii_lowercase(),
            None => continue,
        };
        let value = parts
            .next()
            .unwrap_or("")
            .trim()
            .trim_matches('"')
            .to_string();
        match key.as_str() {
            "host" => {
                push_aliases(&mut entries, &aliases, &host, &user, port);
                aliases = value
                    .split_whitespace()
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect();
                host.clear();
                user.clear();
                port = 22;
            }
            "hostname" => host = value,
            "user" => user = value,
            "port" => port = value.parse().unwrap_or(22),
            // Ignore directives we don't surface (IdentityFile,
            // ServerAliveInterval, ProxyJump, etc.).
            _ => {}
        }
    }
    push_aliases(&mut entries, &aliases, &host, &user, port);
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_connection(name: &str, host: &str, forward: &str) -> autossh_core::ConnectionConfig {
        autossh_core::ConnectionConfig {
            name: name.into(),
            host: Some(host.into()),
            enabled: true,
            ssh_path: None,
            keepalive: autossh_core::KeepaliveConfig::default(),
            retry: autossh_core::RetryConfig::default(),
            extra_args: Vec::new(),
            forwards: vec![autossh_core::ForwardConfig {
                mode: autossh_core::ForwardMode::Local,
                forward: forward.into(),
            }],
        }
    }

    #[test]
    fn parse_ssh_config_extracts_aliases_and_skips_wildcard() {
        let dir = std::env::temp_dir().join("autossh-ui-ssh-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config");
        let _ = std::fs::remove_file(&path);
        std::fs::write(
            &path,
            "# home servers\n\
             Host c005 c006\n\
             \tHostName 192.168.1.5\n\
             \tUser alice\n\
             \tPort 22\n\
             \n\
             Host c007\n\
             \tHostname 10.0.0.7\n\
             \tUser bob\n\
             \tPort 2222\n\
             \n\
             # Catch-all: not a real host\n\
             Host *\n\
             \tServerAliveInterval 30\n",
        )
        .unwrap();

        let entries = parse_ssh_config(&path).unwrap();
        assert_eq!(entries.len(), 3, "got {entries:?}");
        let aliases: Vec<&str> = entries.iter().map(|e| e.alias.as_str()).collect();
        assert_eq!(
            aliases,
            vec!["c005", "c006", "c007"],
            "wildcard + catch-all must be skipped",
        );
        let c005 = &entries[0];
        assert_eq!(c005.port, 22);
        assert_eq!(c005.destination, "alice@192.168.1.5:22");
        assert!(c005.selected);
        let c007 = &entries[2];
        assert_eq!(c007.port, 2222);
        assert_eq!(c007.destination, "bob@10.0.0.7:2222");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn parse_ssh_config_handles_default_port_and_user() {
        let dir = std::env::temp_dir().join("autossh-ui-ssh-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config");
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, "Host solo\n  HostName solo.example.com\n").unwrap();
        let entries = parse_ssh_config(&path).unwrap();
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert_eq!(e.port, 22, "missing Port should default to 22");
        assert_eq!(e.destination, "solo.example.com:22");
        assert!(e.selected);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn parse_ssh_config_surfaces_missing_file_error() {
        let bad = std::env::temp_dir().join("autossh-ui-ssh-tests/does_not_exist");
        let result = parse_ssh_config(&bad);
        assert!(result.is_err(), "missing path should return Err");
    }
}
