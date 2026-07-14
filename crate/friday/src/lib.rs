//! Embedded Friday voice receiver.
//!
//! The listener is intentionally owned by the GUI rather than the SSH
//! supervisor: hiding the window in the Windows tray keeps it alive, while the
//! Start/Stop control can release `127.0.0.1:17322` without affecting tunnels.
//!
//! Module layout:
//!
//! | Module     | Responsibility                                                |
//! |------------|----------------------------------------------------------------|
//! | `player`   | mpv path resolution, platform-correct `Command`, temp MP3 I/O. |
//! | `http`     | Bind the listener, parse `/speak` payloads, dispatch playback.  |
//! | `receiver` | State machine and worker lifecycle driven by the GUI.           |
//!
//! Hosts only need [`FridayReceiver`], [`FridayState`], and [`LISTEN_ADDR`];
//! the rest stays crate-private.

mod http;
mod player;
mod receiver;

pub use player::LISTEN_ADDR;
pub use receiver::{FridayReceiver, FridayState};
