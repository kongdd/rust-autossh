//! Log entry types and parsing for supervisor stderr.

use egui::Color32;

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
    pub text: String,
    pub ts_secs: Option<u64>,
    pub severity: Severity,
    pub connection: Option<String>,
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
    LogEntry {
        text: raw.to_string(),
        ts_secs,
        severity,
        connection,
    }
}

/// Extract the Unix timestamp from a leading `[...]` bracket.
fn parse_ts(line: &str) -> Option<u64> {
    let after = line.strip_prefix('[')?;
    let end = after.find(']')?;
    after[..end].parse::<u64>().ok()
}

/// Extract the connection name from a structured log line such as
/// `[1715000000] ERROR home-server: ssh exited with 1`.
/// Returns `None` for lines that carry no named connection context.
fn parse_connection(line: &str) -> Option<String> {
    let after_prefix = line
        .trim_start_matches('[')
        .split(']')
        .nth(1)?
        .trim_start();
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

/// Render a Unix epoch timestamp as `HH:MM:SS` (24-hour, wraps at 86400 s).
pub fn format_unix_ts(secs: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60,
    )
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
            parse_log_line("[0] ERROR home-server: ssh exited with 1").connection.as_deref(),
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
    fn format_unix_ts_renders_hms_and_wraps_a_day() {
        assert_eq!(format_unix_ts(48_059), "13:20:59");
        assert_eq!(format_unix_ts(0), "00:00:00");
        assert_eq!(format_unix_ts(48_059 + 86_400), "13:20:59");
    }

    #[test]
    fn severity_label_is_stable() {
        assert_eq!(Severity::Info.label(), "INFO");
        assert_eq!(Severity::Warn.label(), "WARN");
        assert_eq!(Severity::Error.label(), "ERROR");
    }
}
