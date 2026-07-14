//! mpv wrapper: resolve a working executable, build a platform-correct
//! `Command`, and stage incoming MP3 data under a unique temporary path.

use std::{
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub const LISTEN_ADDR: &str = "127.0.0.1:17322";
pub(super) const POLL_INTERVAL: Duration = Duration::from_millis(200);

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// `PATH` lookup for `mpv` (Windows: `mpv.exe` first).
pub(super) fn resolve_mpv() -> Option<String> {
    if let Ok(path) = std::env::var("FRIDAY_MPV")
        && !path.trim().is_empty()
    {
        return find_program(&path);
    }
    ["mpv", "mpv.exe"].into_iter().find_map(find_program)
}

/// Resolve an executable without launching it. Probing with `--version` can
/// hang on a broken executable and would make the Start/Stop UI unresponsive.
fn find_program(program: &str) -> Option<String> {
    let path = std::path::Path::new(program);
    if path.components().count() > 1 {
        return path.is_file().then(|| program.to_string());
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|directory| directory.join(program))
        .find(|candidate| candidate.is_file())
        .map(|candidate| candidate.to_string_lossy().into_owned())
}

#[cfg(not(target_os = "windows"))]
pub(super) fn player_command(program: &str) -> Command {
    Command::new(program)
}

#[cfg(target_os = "windows")]
pub(super) fn player_command(program: &str) -> Command {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

pub(super) fn temporary_mp3_path() -> PathBuf {
    let id = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("friday-{}-{nanos}-{id}.mp3", std::process::id()))
}
