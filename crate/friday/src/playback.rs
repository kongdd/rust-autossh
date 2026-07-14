//! Track in-flight mpv children so Stop can cut playback immediately.

use std::{
    path::PathBuf,
    process::Child,
    sync::{Arc, Mutex},
};

pub(super) struct PlaybackRegistry {
    inner: Mutex<Vec<(Child, PathBuf)>>,
}

impl PlaybackRegistry {
    pub(super) fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Vec::new()),
        })
    }

    pub(super) fn register(&self, child: Child, path: PathBuf) {
        if let Ok(mut slots) = self.inner.lock() {
            slots.push((child, path));
        }
    }

    /// Drop finished players and remove their temp MP3 files.
    pub(super) fn reap_finished(&self) {
        let Ok(mut slots) = self.inner.lock() else {
            return;
        };
        slots.retain_mut(|(child, path)| match child.try_wait() {
            Ok(Some(_)) => {
                let _ = std::fs::remove_file(path);
                false
            }
            Ok(None) => true,
            Err(_) => {
                let _ = std::fs::remove_file(path);
                false
            }
        });
    }

    /// Kill every active mpv instance (receiver Stop).
    pub(super) fn kill_all(&self) {
        let Ok(mut slots) = self.inner.lock() else {
            return;
        };
        for (mut child, path) in slots.drain(..) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(path);
        }
    }
}