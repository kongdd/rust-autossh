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

/// Default config path, identical on every platform: `~/.config/autossh/config.toml`.
///
/// Linux/macOS resolve `~` from `$HOME` (XDG Base Directory). Windows resolves
/// it from `%USERPROFILE%` (falling back to `HOME` for MSYS/Git Bash shells
/// that set it). Keeping the same relative path on both platforms means the
/// docs, examples, and scripts in `README.md` work unchanged everywhere; the
/// `.config` directory is created on first run by `ensure_config`.
pub fn default_config_path() -> PathBuf {
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
