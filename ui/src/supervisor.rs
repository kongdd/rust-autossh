//! Supervisor child process management.
//!
//! Spawns `rust-autossh run` as a child, reads its stderr through a
//! `BufReader` + background thread, and pushes parsed `LogEntry` values
//! into an `mpsc` channel that the UI polls each frame.

use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Mutex,
    sync::mpsc,
    thread::{self, JoinHandle},
};

use anyhow::{Context, Result};

use crate::log::{parse_log_line, LogEntry};

/// Locate the `rust-autossh` supervisor binary.
///
/// Checks (in order):
/// 1. Sibling of the current executable
/// 2. Each directory in `$PATH`
pub fn locate_supervisor() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let parent = exe.parent()?;
    let binary = if cfg!(windows) {
        "rust-autossh.exe"
    } else {
        "rust-autossh"
    };
    let sibling = parent.join(binary);
    if sibling.exists() {
        return Some(sibling);
    }
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(binary);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

/// Wraps the supervisor child process and a background stderr reader thread.
pub struct SupervisorHandle {
    /// `None` once reaped/taken.
    child: Mutex<Option<Child>>,
    rx: mpsc::Receiver<LogEntry>,
    _reader: JoinHandle<()>,
}

impl SupervisorHandle {
    /// Spawn `binary run --config <config_path>` and begin reading stderr.
    pub fn start(binary: &PathBuf, config_path: &PathBuf) -> Result<Self> {
        let mut cmd = Command::new(binary);
        cmd.arg("run")
            .arg("--config")
            .arg(config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .with_context(|| format!("cannot start supervisor at {}", binary.display()))?;
        let stderr = child
            .stderr
            .take()
            .context("supervisor stderr not captured")?;
        let (tx, rx) = mpsc::channel();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if tx.send(parse_log_line(&line)).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child: Mutex::new(Some(child)),
            rx,
            _reader: reader,
        })
    }

    /// Drain all buffered log lines into `sink` (non-blocking).
    pub fn drain(&self, sink: &mut Vec<LogEntry>) {
        while let Ok(entry) = self.rx.try_recv() {
            sink.push(entry);
        }
    }

    /// Check whether the child process is still alive.
    pub fn alive(&self) -> bool {
        let mut guard = match self.child.lock() {
            Ok(g) => g,
            Err(_) => return false,
        };
        let Some(child) = guard.as_mut() else {
            return false;
        };
        matches!(child.try_wait(), Ok(None))
    }
}

impl Drop for SupervisorHandle {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.child.lock()
            && let Some(mut child) = guard.take()
        {
            let _ = child.kill();
        }
    }
}
