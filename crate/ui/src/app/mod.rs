//! Application state and the `eframe::App` glue.
//!
//! `AutosshApp` holds the configuration, the supervisor handle, the log buffer,
//! the modal state machine, and the Windows tray icon. Per-frame work runs in
//! three phases inside [`eframe::App::update`]:
//!
//! 1. **State mutation** — `update_windows_tray`, `poll_supervisor`,
//!    [`prune_msg`](Self::prune_msg), modal dispatch.
//! 2. **Rendering** — `render_dashboard`, `render_connections_panel`,
//!    `render_centre_panel`, `render_logs_panel`, `render_modal`.
//! 3. **Repaint scheduling** — request a follow-up frame while the supervisor
//!    is alive so log lines stream in without user interaction.

use std::{
    collections::HashSet,
    path::PathBuf,
    time::{Duration, Instant},
};

use autossh_core::{Config, KeepaliveConfig, RetryConfig};
use eframe::egui;

use crate::log::{LOG_BUFFER_LIMIT, LogScroll};
use crate::modal::Modal;
use crate::supervisor::SupervisorHandle;
use friday::FridayReceiver;

pub mod centre;
pub mod connections;
pub mod dashboard;
pub mod logs;
pub mod modal_host;
pub mod supervisor;

// ─── app state ─────────────────────────────────────────────────────────────────

pub struct AutosshApp {
    pub config_path: PathBuf,
    pub config: Config,
    pub dirty: bool,
    pub selected_connection: usize,
    pub selected_global: usize,

    pub supervisor: Option<SupervisorHandle>,
    pub friday: FridayReceiver,
    pub logs: Vec<crate::log::LogEntry>,
    log_scroll: LogScroll,

    modal: Modal,
    msg: Option<(String, Instant)>,

    /// Tracks which connections are checked for batch delete.
    checked_conn: HashSet<usize>,

    /// Kept alive for the lifetime of the app; dropping it removes the icon.
    #[cfg(target_os = "windows")]
    windows_tray: Option<crate::tray::WindowsTray>,
    #[cfg(target_os = "windows")]
    hidden_in_tray: bool,
}

impl AutosshApp {
    pub fn load(config_path: PathBuf) -> anyhow::Result<Self> {
        let config = if config_path.exists() {
            Config::load(&config_path)?
        } else {
            Config::default()
        };
        Ok(Self {
            config_path,
            config,
            dirty: false,
            selected_connection: 0,
            selected_global: 0,
            supervisor: None,
            friday: FridayReceiver::new(),
            logs: Vec::new(),
            log_scroll: LogScroll::default(),
            modal: Modal::None,
            msg: None,
            checked_conn: HashSet::new(),
            #[cfg(target_os = "windows")]
            windows_tray: None,
            #[cfg(target_os = "windows")]
            hidden_in_tray: false,
        })
    }

    #[cfg(target_os = "windows")]
    pub fn install_windows_tray(&mut self, ctx: &egui::Context) -> anyhow::Result<()> {
        self.windows_tray = Some(crate::tray::WindowsTray::new(ctx)?);
        Ok(())
    }

    // ─── config helpers (used by centre + supervisor + modal_host) ──

    pub fn keepalive(&self) -> KeepaliveConfig {
        self.config.keepalive.clone()
    }

    pub fn retry(&self) -> RetryConfig {
        self.config.retry.clone()
    }

    pub fn apply_globals(&mut self, ka: &KeepaliveConfig, r: &RetryConfig) {
        self.config.keepalive = ka.clone();
        self.config.retry = r.clone();
        self.dirty = true;
    }

    // ─── toast message ──

    pub fn flash(&mut self, text: impl Into<String>) {
        self.msg = Some((text.into(), Instant::now()));
    }

    fn prune_msg(&mut self) {
        if let Some((_, t)) = &self.msg
            && t.elapsed() > Duration::from_secs(4)
        {
            self.msg = None;
        }
    }
}

// ─── eframe entry ──────────────────────────────────────────────────────────────

impl eframe::App for AutosshApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ── Phase 1: state mutation ──
        #[cfg(target_os = "windows")]
        self.update_windows_tray(ctx);
        self.poll_supervisor();
        self.friday.poll();
        self.prune_msg();
        self.render_modal(ctx);
        // ── Phase 2: read-only rendering ──
        self.render_dashboard(ctx);
        self.render_logs_panel(ctx);
        self.render_connections_panel(ctx);
        self.render_centre_panel(ctx);
        if self.supervisor.is_some() || self.friday.is_active() {
            // Re-paint continuously while a background worker is alive so new
            // log lines and Friday listener transitions appear immediately.
            ctx.request_repaint_after(Duration::from_millis(150));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Drop the supervisor now, rather than relying on process teardown.
        // Its Drop implementation also terminates all spawned SSH descendants.
        self.supervisor.take();
        self.friday.stop();
        // Best-effort save so the user does not silently lose edits.
        if self.dirty {
            let _ = self.config.save(&self.config_path);
        }
    }
}

// ─── Windows tray auto-hide ────────────────────────────────────────────────────

impl AutosshApp {
    #[cfg(target_os = "windows")]
    fn update_windows_tray(&mut self, ctx: &egui::Context) {
        use crate::tray::TrayCommand;

        let command = self
            .windows_tray
            .as_ref()
            .and_then(crate::tray::WindowsTray::try_recv);

        match command {
            Some(TrayCommand::Show) => {
                // Restore before focusing: Windows ignores Focus for hidden or
                // minimized windows.
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                self.hidden_in_tray = false;
                return;
            }
            Some(TrayCommand::Exit) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            None => {}
        }

        // A minimized window would normally remain in the taskbar. Hide it
        // completely instead; the tray icon remains available for restoring it.
        let minimized = ctx.input(|input| input.viewport().minimized.unwrap_or(false));
        if minimized && !self.hidden_in_tray {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            self.hidden_in_tray = true;
        }
    }
}

// ─── log buffer trimming (kept here because it owns `logs`/`log_scroll`) ─────

impl AutosshApp {
    fn poll_supervisor(&mut self) {
        let Some(handle) = self.supervisor.as_ref() else {
            return;
        };
        let mut entries = Vec::new();
        handle.drain(&mut entries);
        self.logs
            .extend(entries.into_iter().filter(crate::log::is_displayable));
        if self.logs.len() > LOG_BUFFER_LIMIT {
            let excess = self.logs.len() - (LOG_BUFFER_LIMIT - 100);
            self.logs.drain(..excess);
        }
    }
}
