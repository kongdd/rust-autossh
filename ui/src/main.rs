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
            install_windows_icon_fonts(&cc.egui_ctx);
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

/// Default config path.
///
/// Linux/macOS: `$HOME/.config/autossh/config.toml` (XDG Base Directory).
/// Windows: `%APPDATA%\autossh\config.toml`. USERPROFILE is *not* a valid
/// fallback on Windows because `~\.config` does not exist by default and
/// silently creates an inaccessible directory the first time the UI runs.
pub fn default_path() -> PathBuf {
    #[cfg(windows)]
    {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            return PathBuf::from(appdata).join("autossh").join("config.toml");
        }
    }
    #[cfg(not(windows))]
    {
        let home = std::env::var_os("HOME").unwrap_or_else(|| ".".into());
        return PathBuf::from(home).join(".config").join("autossh").join("config.toml");
    }
    #[allow(unreachable_code)]
    PathBuf::from(".").join("autossh").join("config.toml")
}

// ─── fonts and theme ──────────────────────────────────────────────────────────

/// eframe ships portable default fonts rather than using the Windows font
/// fallback chain. Register the Windows system fonts explicitly so that:
///   * UI glyphs (`●`, `✓`, `✎`, `＋`) do not turn into tofu boxes, and
///   * CJK characters in connection names / hosts render correctly without
///     the user having to install a separate font.
#[cfg(target_os = "windows")]
fn install_windows_icon_fonts(ctx: &egui::Context) {
    use eframe::egui::{FontData, FontDefinitions, FontFamily};

    let font_dir = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("Fonts");
    let mut fonts = FontDefinitions::default();

    // (font_name, file, position) — first=true inserts at index 0 (highest priority),
    // first=false appends as a fallback. Symbol + Emoji + a CJK font cover the
    // glyphs that show up in practice; msyh.ttc is present on every Windows 7+
    // install and contains Microsoft YaHei UI plus the UI variant.
    for (name, filename, first) in [
        ("Segoe UI Symbol", "seguisym.ttf", true),
        ("Segoe UI Emoji", "seguiemj.ttf", false),
        ("Microsoft YaHei UI", "msyh.ttc", false),
    ] {
        let Ok(bytes) = std::fs::read(font_dir.join(filename)) else {
            continue;
        };
        fonts
            .font_data
            .insert(name.to_owned(), FontData::from_owned(bytes));
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            let family_fonts = fonts.families.entry(family).or_default();
            if first {
                family_fonts.insert(0, name.to_owned());
            } else {
                family_fonts.push(name.to_owned());
            }
        }
    }
    ctx.set_fonts(fonts);
}

/// No system-font configuration is needed outside Windows.
#[cfg(not(target_os = "windows"))]
fn install_windows_icon_fonts(_ctx: &egui::Context) {}

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
