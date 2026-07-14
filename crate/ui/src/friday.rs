//! Embedded Friday voice receiver.
//!
//! The listener is intentionally owned by the GUI rather than the SSH
//! supervisor: hiding the window in the Windows tray keeps it alive, while the
//! Start/Stop control can release `127.0.0.1:17322` without affecting tunnels.

use std::{
    io::Read,
    path::PathBuf,
    process::{Child, Command},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose};
use serde::Deserialize;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

pub const LISTEN_ADDR: &str = "127.0.0.1:17322";
const MAX_BODY_BYTES: u64 = 10 * 1024 * 1024;
const MAX_CONCURRENT_REQUESTS: usize = 8;
const POLL_INTERVAL: Duration = Duration::from_millis(200);

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FridayState {
    Starting,
    Listening,
    Stopping,
    #[default]
    Stopped,
    Failed,
}

#[derive(Deserialize)]
struct Payload {
    #[serde(rename = "type")]
    kind: String,
    data: String,
    #[serde(default = "default_rate")]
    rate: f32,
}

fn default_rate() -> f32 {
    1.0
}

enum WorkerEvent {
    Started { player: String },
    Stopped,
    Failed(String),
}

struct ActiveRequestGuard(Arc<AtomicUsize>);

impl Drop for ActiveRequestGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

struct WorkerHandle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl WorkerHandle {
    fn request_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        self.request_stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Runtime controller consumed by `AutosshApp`.
pub struct FridayReceiver {
    state: FridayState,
    player: Option<String>,
    error: Option<String>,
    worker: Option<WorkerHandle>,
    events_tx: Sender<WorkerEvent>,
    events_rx: Receiver<WorkerEvent>,
}

impl FridayReceiver {
    pub fn new() -> Self {
        let (events_tx, events_rx) = mpsc::channel();
        Self {
            state: FridayState::Stopped,
            player: None,
            error: None,
            worker: None,
            events_tx,
            events_rx,
        }
    }

    pub fn state(&self) -> FridayState {
        self.state
    }

    pub fn player(&self) -> Option<&str> {
        self.player.as_deref()
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn is_active(&self) -> bool {
        !matches!(self.state, FridayState::Stopped | FridayState::Failed)
    }

    pub fn start(&mut self) {
        self.poll();
        if self.worker.is_some() {
            return;
        }

        self.state = FridayState::Starting;
        self.player = None;
        self.error = None;

        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let events = self.events_tx.clone();
        let thread = thread::spawn(move || match serve(worker_stop, &events) {
            Ok(()) => {
                let _ = events.send(WorkerEvent::Stopped);
            }
            Err(error) => {
                let _ = events.send(WorkerEvent::Failed(error));
            }
        });
        self.worker = Some(WorkerHandle {
            stop,
            thread: Some(thread),
        });
    }

    pub fn stop(&mut self) {
        let Some(worker) = self.worker.as_ref() else {
            self.state = FridayState::Stopped;
            return;
        };
        worker.request_stop();
        self.state = FridayState::Stopping;
    }

    /// Drain worker events. Called once per egui frame while Friday is active.
    pub fn poll(&mut self) {
        while let Ok(event) = self.events_rx.try_recv() {
            match event {
                WorkerEvent::Started { player } => {
                    self.player = Some(player);
                    self.state = if self
                        .worker
                        .as_ref()
                        .is_some_and(|worker| worker.stop.load(Ordering::Relaxed))
                    {
                        FridayState::Stopping
                    } else {
                        FridayState::Listening
                    };
                }
                WorkerEvent::Stopped => {
                    self.player = None;
                    self.state = FridayState::Stopped;
                }
                WorkerEvent::Failed(error) => {
                    self.player = None;
                    self.error = Some(error);
                    self.state = FridayState::Failed;
                }
            }
        }

        if self.worker.as_ref().is_some_and(WorkerHandle::is_finished) {
            self.worker.take();
        }
    }
}

impl Drop for FridayReceiver {
    fn drop(&mut self) {
        self.worker.take();
    }
}

fn serve(stop: Arc<AtomicBool>, events: &Sender<WorkerEvent>) -> Result<(), String> {
    let player = resolve_mpv().ok_or_else(|| {
        "mpv not found; install it or set FRIDAY_MPV to its executable".to_string()
    })?;
    if stop.load(Ordering::Relaxed) {
        return Ok(());
    }

    let server = Server::http(LISTEN_ADDR)
        .map_err(|error| format!("cannot listen on {LISTEN_ADDR}: {error}"))?;
    let _ = events.send(WorkerEvent::Started {
        player: player.clone(),
    });
    let active_requests = Arc::new(AtomicUsize::new(0));

    while !stop.load(Ordering::Relaxed) {
        match server.recv_timeout(POLL_INTERVAL) {
            Ok(Some(request)) => {
                let previous = active_requests.fetch_add(1, Ordering::AcqRel);
                if previous >= MAX_CONCURRENT_REQUESTS {
                    active_requests.fetch_sub(1, Ordering::AcqRel);
                    let _ = request.respond(text_response(503, "receiver busy\n"));
                    continue;
                }
                let player = player.clone();
                let active_requests = Arc::clone(&active_requests);
                // Keep the listener controllable even when a client sends a
                // slow or incomplete body. The guard bounds such detached
                // handlers, and dropping the listener still frees port 17322.
                thread::spawn(move || {
                    let _guard = ActiveRequestGuard(active_requests);
                    handle_request(request, &player);
                });
            }
            Ok(None) => {}
            Err(error) => return Err(format!("Friday listener failed: {error}")),
        }
    }
    Ok(())
}

fn handle_request(mut request: Request, player: &str) {
    let method = request.method().clone();
    let url = request.url().to_string();

    let response = match (&method, url.as_str()) {
        (Method::Get, "/") | (Method::Get, "/health") | (Method::Get, "/ping") => {
            text_response(200, "ok\n")
        }
        (Method::Post, "/speak") => {
            match read_body_limited(&mut request).and_then(|body| parse_and_play(&body, player)) {
                Ok(()) => text_response(200, "ok\n"),
                Err(error) => text_response(error.status(), &format!("error: {error}\n")),
            }
        }
        (_, "/") | (_, "/health") | (_, "/ping") => method_not_allowed("GET"),
        (_, "/speak") => method_not_allowed("POST"),
        _ => text_response(404, "not found\n"),
    };
    let _ = request.respond(response);
}

#[derive(Debug)]
enum SpeakError {
    BadRequest(String),
    TooLarge(String),
    Internal(String),
}

impl SpeakError {
    fn status(&self) -> u16 {
        match self {
            Self::BadRequest(_) => 400,
            Self::TooLarge(_) => 413,
            Self::Internal(_) => 500,
        }
    }
}

impl std::fmt::Display for SpeakError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadRequest(message) | Self::TooLarge(message) | Self::Internal(message) => {
                formatter.write_str(message)
            }
        }
    }
}

fn read_body_limited(request: &mut Request) -> Result<Vec<u8>, SpeakError> {
    let mut body = Vec::new();
    request
        .as_reader()
        .take(MAX_BODY_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|error| SpeakError::BadRequest(format!("cannot read request body: {error}")))?;
    if body.len() as u64 > MAX_BODY_BYTES {
        return Err(SpeakError::TooLarge(format!(
            "request body exceeds {MAX_BODY_BYTES} bytes"
        )));
    }
    Ok(body)
}

fn parse_and_play(body: &[u8], player: &str) -> Result<(), SpeakError> {
    let payload: Payload = serde_json::from_slice(body)
        .map_err(|error| SpeakError::BadRequest(format!("invalid JSON: {error}")))?;
    if payload.kind != "mp3" {
        return Err(SpeakError::BadRequest(format!(
            "unsupported type: {}",
            payload.kind
        )));
    }
    if !(0.5..=2.0).contains(&payload.rate) {
        return Err(SpeakError::BadRequest(format!(
            "rate out of range: {}",
            payload.rate
        )));
    }
    play_mp3(&payload.data, payload.rate, player)
}

fn play_mp3(data: &str, rate: f32, player: &str) -> Result<(), SpeakError> {
    let bytes = general_purpose::STANDARD
        .decode(data)
        .map_err(|error| SpeakError::BadRequest(format!("invalid base64 audio: {error}")))?;
    let path = temporary_mp3_path();
    std::fs::write(&path, bytes)
        .map_err(|error| SpeakError::Internal(format!("cannot write temporary MP3: {error}")))?;

    let child = player_command(player)
        .arg("--no-video")
        .arg("--really-quiet")
        .arg(format!("--speed={rate}"))
        .arg(&path)
        .spawn();
    let child = match child {
        Ok(child) => child,
        Err(error) => {
            let _ = std::fs::remove_file(&path);
            return Err(SpeakError::Internal(format!("cannot start mpv: {error}")));
        }
    };
    thread::spawn(move || wait_and_remove(child, path));
    Ok(())
}

fn wait_and_remove(mut child: Child, path: PathBuf) {
    let _ = child.wait();
    let _ = std::fs::remove_file(path);
}

fn resolve_mpv() -> Option<String> {
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
fn player_command(program: &str) -> Command {
    Command::new(program)
}

#[cfg(target_os = "windows")]
fn player_command(program: &str) -> Command {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

fn temporary_mp3_path() -> PathBuf {
    let id = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("friday-{}-{nanos}-{id}.mp3", std::process::id()))
}

fn text_response(status: u16, body: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    Response::from_string(body).with_status_code(StatusCode(status))
}

fn method_not_allowed(allow: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    let mut response = text_response(405, "method not allowed\n");
    response.add_header(Header::from_bytes("Allow", allow).expect("static HTTP header is valid"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsupported_payloads_before_starting_player() {
        let error = parse_and_play(br#"{"type":"wav","data":"","rate":1.0}"#, "mpv").unwrap_err();
        assert_eq!(error.to_string(), "unsupported type: wav");
    }

    #[test]
    fn rejects_playback_rate_outside_supported_range() {
        let error = parse_and_play(br#"{"type":"mp3","data":"","rate":2.1}"#, "mpv").unwrap_err();
        assert_eq!(error.to_string(), "rate out of range: 2.1");
    }
}
