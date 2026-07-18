//! Supervises OpenSSH tunnel processes; does not implement SSH itself.

use std::path::PathBuf;

mod config;
mod logger;
mod ssh;
mod ssh_log;
mod supervisor;

pub use config::Config;
pub use config::{
    ConnectionConfig, ForwardConfig, ForwardMode, KeepaliveConfig, LogConfig, RetryConfig,
};
pub use ssh::{TestOutput, test_connection};
pub use supervisor::run;

/// Config path resolution order:
///   1. `<exe-dir>/config.toml` — portable binary + private config.
///   2. `~/.config/autossh/config.toml` — standard XDG location.
///
/// `ensure_config` only writes example when both are missing, so putting a
/// `config.toml` next to the exe is enough to override without touching `$HOME`.
pub fn default_config_path() -> PathBuf {
    if let Some(exe) = std::env::current_exe().ok() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("config.toml");
            // `is_file` over `exists` so a `config.toml/` directory doesn't shadow.
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    home_config_path()
}

/// Standard XDG config location (`~/.config/autossh/config.toml`).
/// Separated so `ensure_config` and tests can reference it without the exe probe.
pub fn home_config_path() -> PathBuf {
    let home = home_dir();
    PathBuf::from(home)
        .join(".config")
        .join("autossh")
        .join("config.toml")
}

/// Resolve the user home directory in a platform-aware order so the config
/// path is `~/.config/autossh/...` on Linux and `%USERPROFILE%\.config\autossh\...`
/// on Windows — same relative layout, just a different root.
fn home_dir() -> std::ffi::OsString {
    #[cfg(windows)]
    {
        if let Some(p) = std::env::var_os("USERPROFILE") {
            return p;
        }
    }
    if let Some(p) = std::env::var_os("HOME") {
        return p;
    }
    ".".into()
}
