//! Native GUI for `autossh-core`: edit the configuration (Connections +
//! Globals) and watch the supervisor runtime log. Backed by `eframe` (egui).
//!
//! Architecture:
//!
//! ```text
//!   ┌────────────────────┐  stderr  ┌────────────────────┐  mpsc  ┌────────────────┐
//!   │ autossh-core (run) ├─────────►│ BufReader::lines    ├───────►│ App.logs Vec   │
//!   └────────────────────┘          └────────────────────┘        └────────────────┘
//! ```
//!
//! The supervisor is spawned once the user clicks "Start supervisor" (or on
//! startup if an attach file exists), and its stderr is colour-coded by
//! severity into the bottom log pane.
//!
//! Release builds on Windows use the `windows` subsystem so no `cmd.exe` flash
//! window appears; debug builds keep the console attached for stderr traces.
//!
//! ## Module layout
//!
//! | Module          | Responsibility                          |
//! |-----------------|------------------------------------------|
//! | `app`           | `AutosshApp` state, rendering, apply     |
//! | `log`           | `Severity` / `LogEntry` / parsing        |
//! | `ssh_config`    | Parse `~/.ssh/config` into host entries  |
//! | `supervisor`    | Spawn child, stream stderr via mpsc      |
//! | `modal`         | Dialog state types and UI functions      |

#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod app;
mod log;
mod modal;
mod ssh_config;
mod supervisor;

use std::path::PathBuf;

use anyhow::Result;
use eframe::egui::{self, Color32};

use crate::app::AutosshApp;

// ─── entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let config_path = parse_args();
    let app = AutosshApp::load(config_path)?;
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("rust-autossh")
            .with_inner_size([1024.0, 720.0])
            .with_min_inner_size([820.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "autossh-ui",
        native_options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(visuals());
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe exited with {e:?}"))?;
    Ok(())
}

// ─── CLI argument parsing ─────────────────────────────────────────────────────

fn parse_args() -> PathBuf {
    let mut args = std::env::args().skip(1);
    let mut path = default_path();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-c" | "--config" => {
                path = PathBuf::from(args.next().expect("missing value for --config"));
            }
            "-h" | "--help" => {
                eprintln!("usage: autossh-ui [--config PATH]");
                std::process::exit(0);
            }
            _ => {}
        }
    }
    path
}

/// Default config path: `$HOME/.config/autossh/config.toml`.
pub fn default_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .unwrap_or_else(|| ".".into());
    PathBuf::from(home).join(".config/autossh/config.toml")
}

// ─── theme ─────────────────────────────────────────────────────────────────────

/// Visuals tuned for a darker palette than the OS default so the colour-coded
/// log badges stay legible.
fn visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    let visuals = &mut v;
    visuals.override_text_color = Some(Color32::from_rgb(225, 228, 235));
    visuals.window_rounding = egui::Rounding::same(6.0);
    visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(20, 22, 28);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(34, 38, 46);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(48, 54, 64);
    visuals.widgets.active.bg_fill = Color32::from_rgb(60, 70, 84);
    visuals.selection.bg_fill = Color32::from_rgb(20, 100, 130);
    visuals.selection.stroke = egui::Stroke::new(1.0, Color32::from_rgb(0, 220, 220));
    visuals.hyperlink_color = Color32::from_rgb(0, 220, 220);
    v
}
