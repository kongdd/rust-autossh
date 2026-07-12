use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Top-level TOML configuration.
#[derive(Debug, Deserialize, Clone, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub log: LogConfig,
    #[serde(default)]
    pub connections: Vec<ConnectionConfig>,
}

/// One OpenSSH process connected to one host. A connection can own several
/// local and remote forwards, sharing its keepalive and retry settings.
#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConnectionConfig {
    /// Unique log identifier.
    pub name: String,
    /// SSH destination (`user@host` or Host alias). Defaults to `name` for compatibility.
    pub host: Option<String>,
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
    /// An optional explicit path to ssh (ssh.exe on Windows).
    pub ssh_path: Option<PathBuf>,
    #[serde(default)]
    pub keepalive: KeepaliveConfig,
    #[serde(default)]
    pub retry: RetryConfig,
    #[serde(default)]
    pub extra_args: Vec<String>,
    pub forwards: Vec<ForwardConfig>,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ForwardConfig {
    /// `remote` → `ssh -R`; `local` → `ssh -L`.
    pub mode: ForwardMode,
    /// `10022:127.0.0.1:22`, etc.
    pub forward: String,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ForwardMode {
    Local,
    Remote,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KeepaliveConfig {
    #[serde(default = "default_keepalive_interval")]
    pub interval: u64,
    #[serde(default = "default_keepalive_count")]
    pub count_max: u32,
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout: u64,
}

#[derive(Debug, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RetryConfig {
    #[serde(default = "default_retry_initial")]
    pub initial_seconds: u64,
    #[serde(default = "default_retry_maximum")]
    pub maximum_seconds: u64,
    #[serde(default = "default_retry_stable")]
    pub stable_seconds: u64,
}

#[derive(Debug, Deserialize, Clone, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LogConfig {
    pub file: Option<PathBuf>,
    /// Rotate after this many MiB. Zero disables rotation.
    #[serde(default = "default_rotate_mib")]
    pub rotate_mib: u64,
}

fn enabled_by_default() -> bool {
    true
}
fn default_keepalive_interval() -> u64 {
    30
}
fn default_keepalive_count() -> u32 {
    3
}
fn default_connect_timeout() -> u64 {
    15
}
fn default_retry_initial() -> u64 {
    1
}
fn default_retry_maximum() -> u64 {
    60
}
fn default_retry_stable() -> u64 {
    60
}
fn default_rotate_mib() -> u64 {
    10
}

impl Default for KeepaliveConfig {
    fn default() -> Self {
        Self {
            interval: default_keepalive_interval(),
            count_max: default_keepalive_count(),
            connect_timeout: default_connect_timeout(),
        }
    }
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            initial_seconds: default_retry_initial(),
            maximum_seconds: default_retry_maximum(),
            stable_seconds: default_retry_stable(),
        }
    }
}

impl ConnectionConfig {
    pub fn destination(&self) -> &str {
        self.host.as_deref().unwrap_or(&self.name)
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)
            .with_context(|| format!("cannot read configuration {}", path.display()))?;
        let config: Self =
            toml::from_str(&text).with_context(|| format!("invalid TOML in {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.connections.is_empty() {
            bail!("configuration has no connections");
        }
        let mut names = HashSet::new();
        for connection in &self.connections {
            if connection.name.trim().is_empty() {
                bail!("connection name must not be empty");
            }
            if connection.destination().trim().is_empty() {
                bail!("connection {} has an empty host", connection.name);
            }
            if connection.forwards.is_empty() {
                bail!("connection {} has no forwards", connection.name);
            }
            if connection
                .forwards
                .iter()
                .any(|forward| forward.forward.trim().is_empty())
            {
                bail!("connection {} has an empty forward", connection.name);
            }
            if !names.insert(&connection.name) {
                bail!("duplicate connection name: {}", connection.name);
            }
            if connection.keepalive.interval == 0
                || connection.keepalive.count_max == 0
                || connection.keepalive.connect_timeout == 0
            {
                bail!(
                    "connection {}: keepalive values must be greater than zero",
                    connection.name
                );
            }
            if connection.retry.initial_seconds == 0
                || connection.retry.maximum_seconds < connection.retry.initial_seconds
                || connection.retry.stable_seconds == 0
            {
                bail!("connection {}: invalid retry settings", connection.name);
            }
        }
        Ok(())
    }
}
