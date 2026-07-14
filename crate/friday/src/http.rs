//! HTTP listener: bind `127.0.0.1:17322`, serve health endpoints, and dispatch
//! `/speak` payloads to an mpv subprocess.

use std::{
    io::Read,
    path::PathBuf,
    process::Child,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
};

use base64::{Engine as _, engine::general_purpose};
use serde::Deserialize;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

use crate::player::{LISTEN_ADDR, POLL_INTERVAL, player_command, resolve_mpv, temporary_mp3_path};

const MAX_BODY_BYTES: u64 = 10 * 1024 * 1024;
const MAX_CONCURRENT_REQUESTS: usize = 8;

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

pub(super) struct ActiveRequestGuard(Arc<AtomicUsize>);

impl Drop for ActiveRequestGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(super) fn serve(
    stop: Arc<AtomicBool>,
    events: &crate::receiver::EventSink,
) -> Result<(), String> {
    let player = resolve_mpv().ok_or_else(|| {
        "mpv not found; install it or set FRIDAY_MPV to its executable".to_string()
    })?;
    if stop.load(Ordering::Relaxed) {
        return Ok(());
    }

    let server = Server::http(LISTEN_ADDR)
        .map_err(|error| format!("cannot listen on {LISTEN_ADDR}: {error}"))?;
    let _ = events.started(player.clone());
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
                Err(error) => text_response(error.status, &format!("error: {error}\n")),
            }
        }
        (_, "/") | (_, "/health") | (_, "/ping") => method_not_allowed("GET"),
        (_, "/speak") => method_not_allowed("POST"),
        _ => text_response(404, "not found\n"),
    };
    let _ = request.respond(response);
}

#[derive(Debug)]
struct SpeakError {
    status: u16,
    message: String,
}

impl SpeakError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            message: message.into(),
        }
    }

    fn too_large(message: impl Into<String>) -> Self {
        Self {
            status: 413,
            message: message.into(),
        }
    }

    fn internal(message: impl Into<String>) -> Self {
        Self {
            status: 500,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for SpeakError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

fn read_body_limited(request: &mut Request) -> Result<Vec<u8>, SpeakError> {
    let mut body = Vec::new();
    request
        .as_reader()
        .take(MAX_BODY_BYTES + 1)
        .read_to_end(&mut body)
        .map_err(|error| SpeakError::bad_request(format!("cannot read request body: {error}")))?;
    if body.len() as u64 > MAX_BODY_BYTES {
        return Err(SpeakError::too_large(format!(
            "request body exceeds {MAX_BODY_BYTES} bytes"
        )));
    }
    Ok(body)
}

fn parse_and_play(body: &[u8], player: &str) -> Result<(), SpeakError> {
    let payload: Payload = serde_json::from_slice(body)
        .map_err(|error| SpeakError::bad_request(format!("invalid JSON: {error}")))?;
    if payload.kind != "mp3" {
        return Err(SpeakError::bad_request(format!(
            "unsupported type: {}",
            payload.kind
        )));
    }
    if !(0.5..=2.0).contains(&payload.rate) {
        return Err(SpeakError::bad_request(format!(
            "rate out of range: {}",
            payload.rate
        )));
    }
    play_mp3(&payload.data, payload.rate, player)
}

fn play_mp3(data: &str, rate: f32, player: &str) -> Result<(), SpeakError> {
    let bytes = general_purpose::STANDARD
        .decode(data)
        .map_err(|error| SpeakError::bad_request(format!("invalid base64 audio: {error}")))?;
    let path = temporary_mp3_path();
    std::fs::write(&path, bytes)
        .map_err(|error| SpeakError::internal(format!("cannot write temporary MP3: {error}")))?;

    let child = player_command(player)
        .arg("--no-video")
        .arg("--really-quiet")
        .arg(format!("--speed={rate}"))
        .arg(&path)
        .spawn()
        .map_err(|error| {
            let _ = std::fs::remove_file(&path);
            SpeakError::internal(format!("cannot start mpv: {error}"))
        })?;
    thread::spawn(move || wait_and_remove(child, path));
    Ok(())
}

fn wait_and_remove(mut child: Child, path: PathBuf) {
    let _ = child.wait();
    let _ = std::fs::remove_file(path);
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
