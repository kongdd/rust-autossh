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
    sync::{atomic::AtomicBool, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Do not create a console window for a child of the Windows GUI process.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

use anyhow::{Context, Result};

use crate::log::{LogEntry, parse_log_line};

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
    /// Once true, Drop avoids SIGKILL (shutdown already handled).
    shutdown: AtomicBool,
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
        #[cfg(windows)]
        cmd.creation_flags(CREATE_NO_WINDOW);
        let mut child = cmd
            .spawn()
            .with_context(|| format!("cannot start supervisor at {}", binary.display()))?;
        let stderr = child
            .stderr
            .take()
            .context("supervisor stderr not captured")?;
        let (tx, rx) = mpsc::channel();
        let reader = thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                let Ok(bytes_read) = reader.read_until(b'\n', &mut buf) else {
                    break;
                };
                if bytes_read == 0 {
                    break;
                }
                if buf.ends_with(b"\n") {
                    buf.pop();
                    if buf.ends_with(b"\r") {
                        buf.pop();
                    }
                }
                let line = String::from_utf8(buf.clone()).unwrap_or_else(|_| {
                    let (text, _, _) = encoding_rs::GBK.decode(&buf);
                    text.into_owned()
                });
                if tx.send(parse_log_line(&line)).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child: Mutex::new(Some(child)),
            rx,
            _reader: reader,
            shutdown: AtomicBool::new(false),
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
        match guard.as_mut().map(|child| child.try_wait()) {
            Some(Ok(None)) => true,
            Some(Ok(Some(_))) => {
                // Avoid retaining a stale PID: Windows may reuse it before Drop.
                guard.take();
                false
            }
            // Preserve the handle on an OS error so Drop can still terminate
            // the process tree during application shutdown.
            Some(Err(_)) | None => false,
        }
    }

    /// Graceful shutdown: send SIGTERM (Unix), drain remaining log lines
    /// while waiting up to 3 s for the process to exit.  Falls back to
    /// `terminate_process_tree` if the child is still alive.
    pub fn shutdown(&self, sink: &mut Vec<LogEntry>) {
        // 1. Drain anything that is already buffered.
        self.drain(sink);

        // 2. Signal the supervisor process so it can write its shutdown
        //    log lines ("shutdown requested", "supervisor stopped", …).
        #[cfg(unix)]
        {
            if let Ok(guard) = self.child.lock() {
                if let Some(ref child) = *guard {
                    let _ = Command::new("kill")
                        .arg("-TERM")
                        .arg(child.id().to_string())
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
            }
        }

        // 3. Wait up to 3 s for the process to exit, draining lines.
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            self.drain(sink);
            if !self.alive() {
                self.drain(sink);
                self.shutdown
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        // 4. Still alive after timeout — force kill.
        if let Ok(mut guard) = self.child.lock()
            && let Some(mut child) = guard.take()
        {
            if child.try_wait().ok().flatten().is_none() {
                terminate_process_tree(&mut child);
            }
        }
        self.drain(sink);
        self.shutdown
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Drop for SupervisorHandle {
    fn drop(&mut self) {
        if self.shutdown.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        if let Ok(mut guard) = self.child.lock()
            && let Some(mut child) = guard.take()
        {
            terminate_process_tree(&mut child);
        }
    }
}

/// Terminate the supervisor and every SSH process it spawned. `taskkill /T`
/// is required on Windows because force-killing the supervisor alone would
/// otherwise leave its SSH children orphaned.
fn terminate_process_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let mut taskkill = Command::new("taskkill");
        taskkill
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW);
        if taskkill.status().is_ok() {
            let _ = child.wait();
            return;
        }
    }

    let _ = child.kill();
    let _ = child.wait();
}
