//! Log entry types, shared colour palette, and supervisor stderr parsing.

use chrono::{Local, NaiveDateTime, TimeZone};
use eframe::egui::Color32;

// ─── shared colour palette ──────────────────────────────────────────────────

pub(crate) const FG_PRIMARY: Color32 = Color32::from_rgb(0, 220, 220);
pub(crate) const FG_SUCCESS: Color32 = Color32::from_rgb(0, 200, 120);
pub(crate) const FG_WARNING: Color32 = Color32::from_rgb(245, 200, 70);
pub(crate) const FG_ERROR: Color32 = Color32::from_rgb(245, 90, 90);
pub(crate) const FG_MUTED: Color32 = Color32::from_rgb(140, 145, 160);
pub(crate) const FG_DIM: Color32 = Color32::from_rgb(90, 95, 110);

/// How many log lines to keep in-memory before dropping old ones (oldest first).
pub const LOG_BUFFER_LIMIT: usize = 2000;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warn,
    Error,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
    pub fn foreground(self) -> Color32 {
        match self {
            Self::Info => Color32::BLACK,
            Self::Warn => Color32::BLACK,
            Self::Error => Color32::WHITE,
        }
    }
    pub fn badge(self) -> Color32 {
        match self {
            Self::Info => Color32::from_rgb(0, 200, 120),
            Self::Warn => Color32::from_rgb(245, 200, 70),
            Self::Error => Color32::from_rgb(245, 90, 90),
        }
    }
}

#[derive(Clone)]
pub struct LogEntry {
    pub ts_secs: Option<u64>,
    pub severity: Severity,
    pub connection: Option<String>,
    /// Short, human-readable event shown in the log panel.
    pub message: String,
}

impl LogEntry {
    /// Color lifecycle messages independently from their log severity: a
    /// started connection is positive feedback, while a stopped/exited one
    /// needs immediate attention even when the supervisor logged it as WARN.
    pub fn event_color(&self) -> Color32 {
        if self.message.starts_with("connected ·") {
            Color32::from_rgb(0, 200, 120)
        } else if self.message.starts_with("ssh exited ·")
            || self.message.starts_with("supervisor stopped")
        {
            Color32::from_rgb(245, 90, 90)
        } else {
            match self.severity {
                Severity::Info => Color32::from_rgb(0, 220, 220),
                Severity::Warn => Color32::from_rgb(245, 200, 70),
                Severity::Error => Color32::from_rgb(245, 90, 90),
            }
        }
    }
}

/// Controls whether the log scroll area tracks the bottom or holds a fixed
/// offset from the bottom.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct LogScroll {
    pub follow: bool,
    pub offset_from_bottom: usize,
}

impl Default for LogScroll {
    fn default() -> Self {
        Self {
            follow: true,
            offset_from_bottom: 0,
        }
    }
}

// ─── parsing ───────────────────────────────────────────────────────────────────

/// Parse one line of the structured supervisor stderr into a `LogEntry`.
pub fn parse_log_line(raw: &str) -> LogEntry {
    let ts_secs = parse_ts(raw);
    let severity = if raw.contains(" ERROR ") {
        Severity::Error
    } else if raw.contains(" WARN ") {
        Severity::Warn
    } else {
        Severity::Info
    };
    let connection = parse_connection(raw);
    let message = concise_message(raw, connection.as_deref());
    LogEntry {
        ts_secs,
        severity,
        connection,
        message,
    }
}

/// Keep only the event payload.  Splitting at the last `:` loses useful SSH
/// details such as ports and exit statuses; strip only the known log prefix.
fn concise_message(raw: &str, connection: Option<&str>) -> String {
    let payload = raw
        .split_once(']')
        .map(|(_, rest)| rest.trim_start())
        .unwrap_or(raw);
    let payload = ["INFO ", "WARN ", "ERROR "]
        .iter()
        .find_map(|prefix| payload.strip_prefix(prefix))
        .unwrap_or(payload);
    let payload = connection
        .and_then(|name| {
            payload
                .strip_prefix(name)
                .and_then(|rest| rest.strip_prefix(':'))
        })
        .map(str::trim_start)
        .unwrap_or(payload);
    let payload = payload.strip_prefix("ssh: ").unwrap_or(payload);

    if let Some(rest) = payload.strip_prefix("ssh process started ") {
        format!("connected · {rest}")
    } else if let Some(status) = payload.strip_prefix("ssh exited with ") {
        format!("ssh exited · {status}")
    } else {
        payload.to_owned()
    }
}

/// Authentication-method chatter is emitted by SSH during every reconnect but
/// does not help an operator decide what to fix.  The connection lifecycle and
/// actual forwarding failures remain visible.
pub fn is_displayable(entry: &LogEntry) -> bool {
    !entry.message.contains("using \"publickey\"")
}

/// Extract a Unix timestamp from a leading `[...]` bracket (local wall-clock or legacy epoch).
fn parse_ts(line: &str) -> Option<u64> {
    let after = line.strip_prefix('[')?;
    let end = after.find(']')?;
    let token = &after[..end];
    if let Ok(secs) = token.parse::<u64>() {
        return Some(secs);
    }
    NaiveDateTime::parse_from_str(token, "%Y-%m-%d %H:%M:%S")
        .ok()
        .and_then(|naive| Local.from_local_datetime(&naive).single())
        .map(|dt| dt.timestamp().max(0) as u64)
}

/// Extract the connection name from a structured log line such as
/// `[1715000000] ERROR home-server: ssh exited with 1`.
/// Returns `None` for lines that carry no named connection context.
fn parse_connection(line: &str) -> Option<String> {
    let after_prefix = line.trim_start_matches('[').split(']').nth(1)?.trim_start();
    let after_level = after_prefix
        .strip_prefix("INFO ")
        .or_else(|| after_prefix.strip_prefix("WARN "))
        .or_else(|| after_prefix.strip_prefix("ERROR "))?;
    let token = after_level.split_whitespace().next()?;
    if !token.ends_with(':') {
        return None;
    }
    let name = token.trim_end_matches(':');
    if name.is_empty() || name.contains('[') {
        return None;
    }
    Some(name.to_string())
}

/// Render a Unix epoch timestamp as local `HH:MM:SS` (24-hour).
pub fn format_unix_ts(secs: u64) -> String {
    Local
        .timestamp_opt(secs as i64, 0)
        .single()
        .map(|dt| dt.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "??:??:??".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_is_inferred_from_level_token() {
        assert_eq!(parse_log_line("[0] INFO loaded").severity, Severity::Info);
        assert_eq!(parse_log_line("[0] WARN flaky").severity, Severity::Warn);
        assert_eq!(parse_log_line("[0] ERROR boom").severity, Severity::Error);
    }

    #[test]
    fn connection_name_requires_trailing_colon() {
        assert_eq!(
            parse_log_line("[0] ERROR home-server: ssh exited with 1")
                .connection
                .as_deref(),
            Some("home-server"),
        );
        assert_eq!(
            parse_log_line("[0] INFO loaded 2 connection(s)").connection,
            None,
        );
    }

    #[test]
    fn ts_is_parsed_from_leading_bracket() {
        assert_eq!(
            parse_log_line("[1715000000] INFO hi").ts_secs,
            Some(1715000000)
        );
        assert_eq!(parse_log_line("not bracketed").ts_secs, None);
    }

    #[test]
    fn format_unix_ts_renders_local_hms() {
        let secs = Local
            .with_ymd_and_hms(2024, 1, 15, 13, 20, 59)
            .single()
            .expect("valid local datetime")
            .timestamp() as u64;
        assert_eq!(format_unix_ts(secs), "13:20:59");
    }

    #[test]
    fn parse_ts_accepts_local_wall_clock_and_legacy_epoch() {
        assert_eq!(
            parse_ts("[2024-01-15 13:20:59] INFO hi"),
            Local
                .with_ymd_and_hms(2024, 1, 15, 13, 20, 59)
                .single()
                .map(|dt| dt.timestamp() as u64)
        );
        assert_eq!(parse_ts("[1715000000] INFO hi"), Some(1_715_000_000));
    }

    #[test]
    fn severity_label_is_stable() {
        assert_eq!(Severity::Info.label(), "INFO");
        assert_eq!(Severity::Warn.label(), "WARN");
        assert_eq!(Severity::Error.label(), "ERROR");
    }

    #[test]
    fn lifecycle_messages_use_distinct_colors() {
        let started = parse_log_line("[0] INFO home: ssh process started (pid 1)");
        let stopped = parse_log_line("[0] WARN home: ssh exited with exit status: 255");
        assert_ne!(started.event_color(), stopped.event_color());
        assert_eq!(started.event_color(), Color32::from_rgb(0, 200, 120));
        assert_eq!(stopped.event_color(), Color32::from_rgb(245, 90, 90));
    }

    #[test]
    fn message_preserves_ssh_ports_and_exit_statuses() {
        let entry = parse_log_line(
            "[1715000000] WARN home: ssh: remote port forwarding failed for listen port 10022",
        );
        assert_eq!(
            entry.message,
            "remote port forwarding failed for listen port 10022"
        );

        let entry = parse_log_line("[1715000000] WARN home: ssh exited with exit status: 255");
        assert_eq!(entry.message, "ssh exited · exit status: 255");
    }

    #[test]
    fn publickey_chatter_is_hidden() {
        let entry =
            parse_log_line("[0] WARN home: ssh: Authenticated to host using \"publickey\".");
        assert!(!is_displayable(&entry));
    }
}
