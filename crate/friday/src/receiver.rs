//! `FridayReceiver`: GUI-facing state machine plus the worker thread that
//! hosts the HTTP listener. The GUI drives it once per frame via
//! `start` / `stop` / `poll`.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
};

use crate::http::serve;

/// Compact sink around the worker's mpsc sender. Exposes narrow verbs so
/// `http::serve` does not depend on the `WorkerEvent` enum directly.
#[derive(Clone)]
pub(super) struct EventSink(Sender<WorkerEvent>);

impl EventSink {
    /// Returns `true` if the receiver is still around to hear us.
    pub(super) fn started(&self, player: String) -> bool {
        self.0.send(WorkerEvent::Started { player }).is_ok()
    }

    pub(super) fn stopped(&self) -> bool {
        self.0.send(WorkerEvent::Stopped).is_ok()
    }

    pub(super) fn failed(&self, message: String) -> bool {
        self.0.send(WorkerEvent::Failed(message)).is_ok()
    }
}

pub(super) enum WorkerEvent {
    Started { player: String },
    Stopped,
    Failed(String),
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
        let events = EventSink(self.events_tx.clone());
        let thread = thread::spawn(move || match serve(worker_stop, &events) {
            Ok(()) => {
                let _ = events.stopped();
            }
            Err(error) => {
                let _ = events.failed(error);
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FridayState {
    Starting,
    Listening,
    Stopping,
    #[default]
    Stopped,
    Failed,
}
