use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};

use crate::config::LogConfig;

#[derive(Clone)]
pub struct Logger(Arc<Mutex<LogSink>>);

struct LogSink {
    path: Option<PathBuf>,
    max_bytes: u64,
    file: Option<File>,
    rotation_retry_at: Option<Instant>,
    io_error_reported: bool,
}

impl Logger {
    pub fn new(config_path: &Path, log: &LogConfig) -> Result<Self> {
        let path = log.file.as_ref().map(|path| {
            if path.is_absolute() {
                path.clone()
            } else {
                config_path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(path)
            }
        });
        let file = match &path {
            Some(path) => {
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                Some(
                    OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(path)
                        .with_context(|| format!("cannot open log file {}", path.display()))?,
                )
            }
            None => None,
        };
        Ok(Self(Arc::new(Mutex::new(LogSink {
            path,
            max_bytes: log.rotate_mib.saturating_mul(1024 * 1024),
            file,
            rotation_retry_at: None,
            io_error_reported: false,
        }))))
    }

    pub fn info(&self, message: impl AsRef<str>) {
        self.write("INFO", message.as_ref());
    }
    pub fn warn(&self, message: impl AsRef<str>) {
        self.write("WARN", message.as_ref());
    }
    pub fn error(&self, message: impl AsRef<str>) {
        self.write("ERROR", message.as_ref());
    }

    fn write(&self, level: &str, message: &str) {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let line = format!("[{seconds}] {level:<5} {message}\n");
        eprint!("{line}");
        let mut sink = self.0.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(path) = sink.path.clone()
            && sink.max_bytes > 0
            && sink
                .rotation_retry_at
                .is_none_or(|retry_at| Instant::now() >= retry_at)
            && fs::metadata(&path)
                .map(|metadata| metadata.len())
                .unwrap_or(0)
                >= sink.max_bytes
        {
            // Windows cannot rename a file while the current process holds it open.
            sink.file = None;
            let rotated = path.with_extension("log.1");
            let _ = fs::remove_file(&rotated);
            if let Err(error) = fs::rename(&path, &rotated) {
                eprintln!("log rotation failed for {}: {error}", path.display());
                sink.rotation_retry_at = Some(Instant::now() + Duration::from_secs(60));
            } else {
                sink.rotation_retry_at = None;
            }
        }
        if sink.file.is_none()
            && let Some(path) = sink.path.clone()
        {
            match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(file) => sink.file = Some(file),
                Err(error) => report_io_error(
                    &mut sink,
                    format!("cannot open {}: {error}", path.display()),
                ),
            }
        }
        let write_result = sink
            .file
            .as_mut()
            .map(|file| file.write_all(line.as_bytes()).and_then(|()| file.flush()));
        match write_result {
            Some(Ok(())) => sink.io_error_reported = false,
            Some(Err(error)) => {
                sink.file = None;
                report_io_error(&mut sink, format!("cannot write log: {error}"));
            }
            None => {}
        }
    }
}

fn report_io_error(sink: &mut LogSink, message: String) {
    if !sink.io_error_reported {
        eprintln!("log file unavailable: {message}");
        sink.io_error_reported = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process;

    #[test]
    fn failed_rotation_is_throttled_and_retried() {
        let directory = std::env::temp_dir().join(format!("rust-autossh-log-{}", process::id()));
        let path = directory.join("autossh.log");
        let rotated = path.with_extension("log.1");
        fs::create_dir_all(&rotated).unwrap();
        fs::write(&path, "existing").unwrap();
        let logger = Logger::new(
            Path::new("config.toml"),
            &LogConfig {
                file: Some(path.clone()),
                rotate_mib: 0,
            },
        )
        .unwrap();
        logger.0.lock().unwrap().max_bytes = 1;

        logger.info("first");
        logger.info("second");

        let mut sink = logger.0.lock().unwrap();
        assert!(sink.rotation_retry_at.is_some());
        sink.rotation_retry_at = Some(Instant::now());
        drop(sink);
        fs::remove_dir(&rotated).unwrap();

        logger.info("third");

        assert!(logger.0.lock().unwrap().rotation_retry_at.is_none());
        let rotated_text = fs::read_to_string(&rotated).unwrap();
        assert!(rotated_text.contains("first"));
        assert!(rotated_text.contains("second"));
        assert!(fs::read_to_string(&path).unwrap().contains("third"));
        fs::remove_dir_all(directory).unwrap();
    }
}
