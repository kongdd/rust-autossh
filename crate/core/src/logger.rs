use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use chrono::Local;

use crate::config::LogConfig;

#[derive(Clone)]
pub struct Logger(Arc<Mutex<LogSink>>);

struct LogSink {
    path: Option<PathBuf>,
    file: Option<File>,
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
                    File::create(path)
                        .with_context(|| format!("cannot create log file {}", path.display()))?,
                )
            }
            None => None,
        };
        Ok(Self(Arc::new(Mutex::new(LogSink {
            path,
            file,
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
        let stamp = Local::now().format("%Y-%m-%d %H:%M:%S");
        let line = format!("[{stamp}] {level:<5} {message}\n");
        eprint!("{line}");

        let mut sink = self.0.lock().unwrap_or_else(|error| error.into_inner());
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
        let result = sink
            .file
            .as_mut()
            .map(|file| file.write_all(line.as_bytes()).and_then(|()| file.flush()));
        match result {
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
#[path = "../tests/unit/logger.rs"]
mod tests;
