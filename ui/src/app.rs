//! Main application state and rendering.
//!
//! `AutosshApp` holds the configuration, supervisor handle, log buffer, and
//! modal state. Its `eframe::App::update` renders four panels:
//!
//! * **Dashboard** — top bar with status chips (save, supervisor, log/conn counts)
//! * **Connections** — left sidebar listing every connection with inline delete/edit
//! * **Centre** — Globals panel or Help page
//! * **Logs** — bottom panel with severity-badged supervisor output
//!
//! Modal dialogs (add/edit/import) are rendered as `egui::Window` pop-ups.

use std::{
    collections::HashSet,
    path::PathBuf,
    time::{Duration, Instant},
};

use autossh_core::{Config, KeepaliveConfig, RetryConfig};
use eframe::egui::{self, Color32, RichText};

use crate::log::{
    FG_DIM, FG_ERROR, FG_MUTED, FG_PRIMARY, FG_SUCCESS, FG_WARNING, LOG_BUFFER_LIMIT,
    LogEntry, LogScroll, format_unix_ts, is_displayable,
};
use crate::modal::{
    AddDialogState, CloseAction, EditDialogState, EditableForward, GlobalGroup, ImportDialogState,
    Modal, SshImportState, run_add_dialog_ui, run_edit_dialog_ui, run_import_dialog_ui,
    run_ssh_import_dialog_ui, state_from_connection,
};
use crate::supervisor::{SupervisorHandle, locate_supervisor};

/// Which pane the centre area is showing. Toggled via the segmented control
/// in the dashboard.
#[derive(Copy, Clone, PartialEq, Eq, Default)]
enum CentrePane {
    #[default]
    Globals,
    Help,
}

// ─── app state ─────────────────────────────────────────────────────────────────

pub struct AutosshApp {
    pub config_path: PathBuf,
    pub config: Config,
    pub dirty: bool,
    pub selected_connection: usize,
    pub selected_global: usize,
    centre: CentrePane,

    pub supervisor: Option<SupervisorHandle>,
    pub logs: Vec<LogEntry>,
    log_scroll: LogScroll,

    modal: Modal,
    msg: Option<(String, Instant)>,

    /// Tracks which connections are checked for batch delete.
    checked_conn: HashSet<usize>,
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
            centre: CentrePane::default(),
            supervisor: None,
            logs: Vec::new(),
            log_scroll: LogScroll::default(),
            modal: Modal::None,
            msg: None,
            checked_conn: HashSet::new(),
        })
    }

    fn keepalive(&self) -> KeepaliveConfig {
        self.config
            .connections
            .first()
            .map(|c| c.keepalive.clone())
            .unwrap_or_default()
    }

    fn retry(&self) -> RetryConfig {
        self.config
            .connections
            .first()
            .map(|c| c.retry.clone())
            .unwrap_or_default()
    }

    fn apply_globals(&mut self, ka: &KeepaliveConfig, r: &RetryConfig) {
        for c in &mut self.config.connections {
            c.keepalive = ka.clone();
            c.retry = r.clone();
        }
    }

    fn flash(&mut self, text: impl Into<String>) {
        self.msg = Some((text.into(), Instant::now()));
    }

    fn prune_msg(&mut self) {
        if let Some((_, t)) = &self.msg
            && t.elapsed() > Duration::from_secs(4)
        {
            self.msg = None;
        }
    }

    fn start_supervisor(&mut self) {
        // A supervisor that exited is stale; discard its handle before a new
        // Start All so the button can recover without restarting the UI.
        if self.supervisor_running() {
            return;
        }
        self.supervisor.take();
        let Some(binary) = locate_supervisor() else {
            self.flash("cannot find rust-autossh binary beside this UI; check PATH");
            return;
        };
        match SupervisorHandle::start(&binary, &self.config_path) {
            Ok(handle) => {
                self.flash(format!("started all: {}", binary.display()));
                self.supervisor = Some(handle);
            }
            Err(error) => {
                self.flash(format!("cannot start all: {error:#}"));
            }
        }
    }

    fn stop_supervisor(&mut self) {
        if let Some(handle) = self.supervisor.take() {
            // Graceful shutdown so supervisor can flush closing log lines
            // (SIGTERM → drain → SIGKILL fallback).
            handle.shutdown(&mut self.logs);
            self.flash("stopped all");
        }
    }

    fn poll_supervisor(&mut self) {
        let Some(handle) = self.supervisor.as_ref() else {
            return;
        };
        let mut entries = Vec::new();
        handle.drain(&mut entries);
        self.logs.extend(entries.into_iter().filter(is_displayable));
        if self.logs.len() > LOG_BUFFER_LIMIT {
            let excess = self.logs.len() - (LOG_BUFFER_LIMIT - 100);
            self.logs.drain(..excess);
        }
    }

    fn supervisor_running(&self) -> bool {
        self.supervisor
            .as_ref()
            .is_some_and(SupervisorHandle::alive)
    }

    /// Persist the desired state immediately: the core supervisor watches the
    /// config file and starts/stops just this worker after its next poll.
    fn toggle_connection(&mut self, index: usize) {
        let running = self.supervisor_running();
        let Some(connection) = self.config.connections.get_mut(index) else {
            return;
        };
        let was_enabled = connection.enabled;
        let name = connection.name.clone();
        // An enabled connection is only running while its supervisor is alive.
        // Thus an inactive row always means Start, even if it was enabled in a
        // config loaded before the supervisor was launched.
        let start = !running || !was_enabled;
        connection.enabled = start;

        if let Err(error) = self.config.save(&self.config_path) {
            if let Some(connection) = self.config.connections.get_mut(index) {
                connection.enabled = was_enabled;
            }
            self.flash(format!(
                "cannot {} {name}: {error:#}",
                if start { "start" } else { "stop" }
            ));
            return;
        }
        self.dirty = false;

        if start {
            self.flash(format!("starting {name}"));
            if !running {
                self.start_supervisor();
            }
        } else {
            self.flash(format!("stopping {name}"));
        }
    }

    fn save(&mut self) {
        match self.config.save(&self.config_path) {
            Ok(()) => {
                self.dirty = false;
                self.flash(format!("saved {}", self.config_path.display()));
            }
            Err(error) => {
                self.flash(format!("save failed: {error:#}"));
            }
        }
    }
}

// ─── eframe entry ──────────────────────────────────────────────────────────────

impl eframe::App for AutosshApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ── Phase 1: 所有状态变更在此完成 ──
        self.poll_supervisor();
        self.prune_msg();
        self.render_modal(ctx);
        // ── Phase 2: 渲染（只读 self，零写）──
        self.render_dashboard(ctx);
        self.render_logs_panel(ctx);
        self.render_connections_panel(ctx);
        self.render_centre_panel(ctx);
        if self.supervisor.is_some() {
            // Re-paint continuously while the supervisor is alive so new lines
            // appear without user interaction.
            ctx.request_repaint_after(Duration::from_millis(150));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Drop the supervisor now, rather than relying on process teardown.
        // Its Drop implementation also terminates all spawned SSH descendants.
        self.supervisor.take();
        // Best-effort save so the user does not silently lose edits.
        if self.dirty {
            let _ = self.config.save(&self.config_path);
        }
    }
}

// ─── panel: dashboard ──────────────────────────────────────────────────────────

impl AutosshApp {
    fn render_dashboard(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("dashboard").show(ctx, |ui| {
            let dirty_chip = if self.dirty {
                ("● unsaved", FG_WARNING)
            } else {
                ("✓ saved", FG_SUCCESS)
            };
            let supervisor_state = match self.supervisor.as_ref() {
                None => ("not started", FG_MUTED),
                Some(handle) if handle.alive() => ("running", FG_SUCCESS),
                Some(_) => ("exited", FG_ERROR),
            };
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(
                    RichText::new("rust-autossh")
                        .strong()
                        .size(16.0)
                        .color(FG_PRIMARY),
                );
                ui.add_space(12.0);
                ui.label(
                    RichText::new(self.config_path.display().to_string())
                        .color(Color32::from_rgb(180, 200, 220)),
                );
                if let Some((message, _)) = &self.msg {
                    let color = if message.starts_with("starting ")
                        || message.starts_with("supervisor started")
                    {
                        FG_SUCCESS
                    } else if message.starts_with("stopping ") {
                        FG_ERROR
                    } else {
                        FG_MUTED
                    };
                    ui.separator();
                    ui.label(RichText::new(message).small().color(color));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(format!("{} {}", "●", supervisor_state.0))
                            .color(supervisor_state.1)
                            .strong(),
                    );
                    ui.separator();
                    ui.label(RichText::new(dirty_chip.0).color(dirty_chip.1).strong());
                    ui.separator();
                    ui.label(
                        RichText::new(format!("logs: {}", self.logs.len()))
                            .color(FG_PRIMARY)
                            .strong(),
                    );
                    ui.separator();
                    ui.label(
                        RichText::new(format!("conns: {}", self.config.connections.len()))
                            .color(FG_PRIMARY)
                            .strong(),
                    );
                });
            });
        });
    }
}

// ─── panel: connections ────────────────────────────────────────────────────────

impl AutosshApp {
    fn render_connections_panel(&mut self, ctx: &egui::Context) {
        // Keep the top row balanced: Connections and Centre each take half
        // of the width above the full-width Logs panel.
        let top_column_width = ctx.available_rect().width() / 2.0;
        egui::SidePanel::left("connections")
            .resizable(false)
            .exact_width(top_column_width)
            .show(ctx, |ui| {
                let mut delete_selected = false;
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.heading("Connections");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("📥  Import").clicked() {
                            let initial = crate::default_path()
                                .parent()
                                .map(PathBuf::from)
                                .unwrap_or_default();
                            self.modal = Modal::Import(ImportDialogState {
                                path_input: initial.display().to_string(),
                                ..ImportDialogState::default()
                            });
                        }
                        if ui.button("🖥  SSH hosts").clicked() {
                            self.modal = Modal::ImportSsh(SshImportState {
                                source_path: crate::ssh_config::default_ssh_config_path(),
                                ..SshImportState::default()
                            });
                        }
                        // Use ASCII `+`: the full-width plus (`＋`) renders as a
                        // missing-glyph box in the Windows system font fallback.
                        if ui.button("+  Add").clicked() {
                            self.modal = Modal::Add(AddDialogState::default());
                        }
                        let n = self.checked_conn.len();
                        if ui
                            .add_enabled(
                                n > 0,
                                egui::Button::new(
                                    RichText::new(format!("🗑  delete ({n})")).color(FG_ERROR),
                                ),
                            )
                            .clicked()
                        {
                            delete_selected = true;
                        }
                    });
                });
                ui.add_space(4.0);
                ui.separator();

                if self.config.connections.is_empty() {
                    ui.add_space(20.0);
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new("(no connections)").color(FG_MUTED));
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("press “+  Add” to start")
                                .small()
                                .color(FG_DIM),
                        );
                    });
                    return;
                }

                let mut toggle_at: Option<usize> = None;
                let mut edit_at: Option<usize> = None;
                let supervisor_running = self.supervisor_running();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (i, conn) in self.config.connections.iter().enumerate() {
                            let sel = i == self.selected_connection;
                            let (fill, sw, sc) = if sel {
                                (Color32::from_rgb(34, 56, 70), 1.0, FG_PRIMARY)
                            } else {
                                (Color32::from_rgb(24, 28, 34), 0.5, FG_DIM)
                            };
                            egui::Frame::group(ui.style())
                                .fill(fill).stroke(egui::Stroke::new(sw, sc))
                                .rounding(egui::Rounding::same(4.0))
                                .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                                .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    // checkbox
                                    let mut cb = self.checked_conn.contains(&i);
                                    if ui.checkbox(&mut cb, "")
                                        .on_hover_text("Select for batch deletion")
                                        .changed()
                                    {
                                        if cb { self.checked_conn.insert(i); }
                                        else { self.checked_conn.remove(&i); }
                                    }
                                    // status dot
                                    let running = supervisor_running && conn.enabled;
                                    ui.colored_label(
                                        if running { FG_SUCCESS } else { FG_MUTED }, "●",
                                    );
                                    // clickable details (select on click, edit on double-click)
                                    let mut mk = |text: RichText| {
                                        ui.add(egui::Label::new(text).sense(egui::Sense::click()))
                                    };
                                    let r1 = mk(RichText::new(&conn.name).strong());
                                    let r2 = mk(RichText::new(conn.destination()).small().color(FG_MUTED));
                                    let r3 = mk(RichText::new(format!("{} forwards", conn.forwards.len()))
                                        .small().color(FG_PRIMARY));
                                    let clicked = r1.clicked() || r2.clicked() || r3.clicked();
                                    let dbl = r1.double_clicked() || r2.double_clicked() || r3.double_clicked();
                                    if clicked { self.selected_connection = i; }
                                    if dbl { edit_at = Some(i); }

                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let label = if running { "■  Stop" } else { "▶  Start" };
                                            if ui.small_button(label).clicked() { toggle_at = Some(i); }
                                        },
                                    );
                                });
                            });
                            ui.add_space(4.0);
                        }
                    });

                if let Some(i) = toggle_at {
                    self.toggle_connection(i);
                }
                if delete_selected {
                    let mut indices: Vec<usize> = self.checked_conn.iter().copied().collect();
                    indices.sort_unstable_by(|a, b| b.cmp(a)); // descending
                    let count = indices.len();
                    for i in indices {
                        self.config.connections.remove(i);
                    }
                    self.checked_conn.clear();
                    self.dirty = true;
                    if self.selected_connection >= self.config.connections.len()
                        && self.selected_connection > 0
                    {
                        self.selected_connection = self.config.connections.len().saturating_sub(1);
                    }
                    self.flash(format!("deleted {count} connection(s)"));
                }
                if let Some(i) = edit_at {
                    let conn = self.config.connections[i].clone();
                    self.selected_connection = i;
                    self.modal = Modal::EditConnection {
                        idx: i,
                        state: state_from_connection(&conn),
                    };
                }
            });
    }
}

// ─── panel: centre ─────────────────────────────────────────────────────────────

impl AutosshApp {
    fn render_centre_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.selectable_value(&mut self.centre, CentrePane::Globals, "Globals");
                ui.selectable_value(&mut self.centre, CentrePane::Help, "Help");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("💾  Save").clicked() {
                        self.save();
                    }
                    let all_running = self.supervisor_running();
                    let all_label = if all_running {
                        "■  Stop All"
                    } else {
                        "▶  Start All"
                    };
                    if ui.button(all_label).clicked() {
                        if all_running {
                            self.stop_supervisor();
                        } else {
                            self.start_supervisor();
                        }
                    }
                });
            });
            ui.separator();
            match self.centre {
                CentrePane::Globals => self.render_globals(ui),
                CentrePane::Help => self.render_help(ui),
            }
        });
    }

    fn render_globals(&mut self, ui: &mut egui::Ui) {
        let ka = self.keepalive();
        let r = self.retry();
        let mut sel = self.selected_global.min(5);

        // (display, edit): display keeps the human-readable suffix for the
        // readout; edit is the raw number so the EditGlobal modal can parse it
        // back into a u64. Mixing the two is the bug that turned "30 s" into
        // an un-parseable initial value.
        let keepalive: [(usize, GlobalGroup, (String, String)); 3] = [
            (0, GlobalGroup::KeepaliveInterval, (format!("{} s", ka.interval), ka.interval.to_string())),
            (1, GlobalGroup::KeepaliveCount, (ka.count_max.to_string(), ka.count_max.to_string())),
            (2, GlobalGroup::KeepaliveTimeout, (format!("{} s", ka.connect_timeout), ka.connect_timeout.to_string())),
        ];
        let retry: [(usize, GlobalGroup, (String, String)); 3] = [
            (3, GlobalGroup::RetryInitial, (format!("{} s", r.initial_seconds), r.initial_seconds.to_string())),
            (4, GlobalGroup::RetryMaximum, (format!("{} s", r.maximum_seconds), r.maximum_seconds.to_string())),
            (5, GlobalGroup::RetryStable, (format!("{} s", r.stable_seconds), r.stable_seconds.to_string())),
        ];

        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(8.0);
            ui.columns(2, |cols| {
                for (i, rows) in [&keepalive, &retry].iter().enumerate() {
                    cols[i].group(|ui| {
                        ui.add_space(4.0);
                        ui.heading(if i == 0 { "Keepalive" } else { "Retry" });
                        ui.add_space(4.0);
                        for (idx, group, (display, edit)) in rows.iter() {
                            self.render_global_row(ui, *idx, &mut sel, *group, display.clone(), edit.clone());
                        }
                    });
                }
            });
            ui.add_space(8.0);
            ui.label(
                RichText::new("shared by every connection; click to highlight, double-click to edit")
                    .small()
                    .color(FG_MUTED),
            );
            ui.add_space(8.0);
            ui.collapsing("Active connections", |ui| {
                for (i, c) in self.config.connections.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{}.", i + 1)).color(FG_MUTED));
                        ui.label(RichText::new(&c.name).strong());
                        ui.label(RichText::new(format!("→ {}", c.destination())).color(FG_MUTED));
                    });
                }
            });
        });
        self.selected_global = sel;
    }

    fn render_global_row(
        &mut self, ui: &mut egui::Ui, index: usize, selected: &mut usize,
        group: GlobalGroup, display: String, edit: String,
    ) {
        let is_sel = *selected == index;
        let (fill, sw, sc) = if is_sel {
            (Color32::from_rgb(34, 56, 70), 1.0, FG_PRIMARY)
        } else {
            (Color32::from_rgb(24, 28, 34), 0.5, FG_DIM)
        };
        let response = egui::Frame::group(ui.style())
            .fill(fill)
            .stroke(egui::Stroke::new(sw, sc))
            .rounding(egui::Rounding::same(4.0))
            .inner_margin(egui::Margin::symmetric(10.0, 6.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.vertical(|ui| {
                    ui.label(RichText::new(group.label()).color(FG_MUTED).small());
                    ui.label(RichText::new(&display).strong().color(FG_PRIMARY).monospace());
                });
            });
        let interact = response.response.interact(egui::Sense::click());
        if interact.clicked() {
            *selected = index;
        }
        if interact.double_clicked() {
            *selected = index;
            self.modal = Modal::EditGlobal { group, value: edit };
        }
    }

    fn render_help(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.heading("Quick reference");
        ui.add_space(4.0);
        for (key, desc) in HELP_ROWS {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!("{:<16}", key))
                        .strong()
                        .color(FG_PRIMARY),
                );
                ui.label(*desc);
            });
        }
        ui.add_space(12.0);
        ui.label(RichText::new("Failure handling").strong().size(14.0));
        ui.add_space(4.0);
        ui.label("When a connection's ssh process exits or fails to start, the supervisor:");
        ui.label(
            "  • reads exit code from the supervisor log line tagged with the connection name",
        );
        ui.label(
            "  • sleeps `retry.initial_seconds`, doubling each failure up to `retry.maximum_seconds`",
        );
        ui.label("  • resets the delay once the connection stays alive for `retry.stable_seconds`");
        ui.label(
            "Use the Logs panel at the bottom to watch what each connection is doing in real time.",
        );
    }
}

// ─── panel: logs ───────────────────────────────────────────────────────────────

impl AutosshApp {
    fn render_logs_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("logs")
            .resizable(true)
            .default_height(220.0)
            .height_range(120.0..=460.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.heading("Logs");
                    ui.label(
                        RichText::new(format!("{} lines", self.logs.len()))
                            .small()
                            .color(FG_MUTED),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("clear").clicked() {
                            self.logs.clear();
                        }
                        ui.separator();
                        if ui.checkbox(&mut self.log_scroll.follow, "follow").changed()
                            && self.log_scroll.follow
                        {
                            self.log_scroll.offset_from_bottom = 0;
                        }
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .stick_to_bottom(self.log_scroll.follow)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if self.logs.is_empty() {
                            ui.add_space(20.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new(
                                        "no log lines yet — start the supervisor to see its output here",
                                    )
                                    .small()
                                    .color(FG_MUTED),
                                );
                            });
                            return;
                        }
                        let total = self.logs.len();
                        let viewport_h = ui.available_height().max(1.0) as usize;
                        let start = if self.log_scroll.follow || total <= viewport_h {
                            total.saturating_sub(viewport_h)
                        } else {
                            let bottom = total.saturating_sub(self.log_scroll.offset_from_bottom);
                            bottom.saturating_sub(viewport_h)
                        };
                        if self.logs.len() > LOG_BUFFER_LIMIT - 100 {
                            ui.label(
                                RichText::new(format!(
                                    " … {} earlier lines dropped … ",
                                    self.logs.len()
                                ))
                                .small()
                                .color(FG_DIM),
                            );
                        }
                        for entry in &self.logs[start..total] {
                            ui.horizontal(|ui| {
                                ui.add_space(4.0);
                                let time = entry
                                    .ts_secs
                                    .map(format_unix_ts)
                                    .unwrap_or_else(|| "        ".to_string());
                                ui.label(
                                    RichText::new(format!("{time} "))
                                        .monospace()
                                        .color(ui.visuals().weak_text_color()),
                                );
                                // severity badge
                                let (badge_text, badge_bg) = (
                                    format!(" {} ", entry.severity.label()),
                                    entry.severity.badge(),
                                );
                                let badge_fg = entry.severity.foreground();
                                let event_color = entry.event_color();
                                egui::Frame::group(ui.style())
                                    .fill(badge_bg)
                                    .rounding(egui::Rounding::same(3.0))
                                    .inner_margin(egui::Margin::same(0.0))
                                    .show(ui, |ui| {
                                        ui.label(
                                            RichText::new(badge_text)
                                                .strong()
                                                .monospace()
                                                .color(badge_fg),
                                        );
                                    });
                                ui.add_space(4.0);
                                if let Some(conn) = &entry.connection {
                                    ui.label(
                                        RichText::new(conn.clone())
                                            .strong()
                                            .color(event_color),
                                    );
                                    ui.label(
                                        RichText::new(format!(": {}", entry.message))
                                            .color(event_color),
                                    );
                                } else {
                                    ui.label(RichText::new(&entry.message).color(event_color));
                                }
                            });
                        }
                    });
            });
    }
}

// ─── modal dispatch ────────────────────────────────────────────────────────────

impl AutosshApp {
    fn render_modal(&mut self, ctx: &egui::Context) {
        let modal = std::mem::replace(&mut self.modal, Modal::None);
        match modal {
            Modal::None => {}
            Modal::Add(state) => {
                let mut state = state;
                egui::Window::new("Add connection")
                    .collapsible(false)
                    .resizable(false)
                    .default_size([520.0, 460.0])
                    .show(ctx, |ui| {
                        run_add_dialog_ui(ui, &mut state);
                    });
                self.apply_add_dialog_state(state);
            }
            Modal::EditConnection { idx, state } => {
                let mut state = state;
                let name = self
                    .config
                    .connections
                    .get(idx)
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                egui::Window::new(format!("Edit connection ({} → {})", idx + 1, name))
                    .collapsible(false)
                    .resizable(false)
                    .default_size([520.0, 460.0])
                    .show(ctx, |ui| {
                        run_add_dialog_ui(ui, &mut state);
                    });
                self.apply_edit_connection_state(idx, state);
            }
            Modal::EditGlobal { group, value } => {
                let mut state = EditDialogState {
                    group,
                    value,
                    close: CloseAction::None,
                };
                egui::Window::new(format!("Edit {}", group.label()))
                    .collapsible(false)
                    .resizable(false)
                    .default_size([380.0, 160.0])
                    .show(ctx, |ui| {
                        run_edit_dialog_ui(ui, &mut state);
                    });
                self.apply_edit_dialog_state(state);
            }
            Modal::Import(mut state) => {
                let existing: Vec<String> = self
                    .config
                    .connections
                    .iter()
                    .map(|c| c.name.clone())
                    .collect();
                egui::Window::new("Import hosts from a TOML config")
                    .collapsible(false)
                    .resizable(true)
                    .default_size([520.0, 440.0])
                    .show(ctx, |ui| {
                        run_import_dialog_ui(ui, &mut state, &existing);
                    });
                self.apply_import_dialog_state(state);
            }
            Modal::ImportSsh(mut state) => {
                let existing: Vec<String> = self
                    .config
                    .connections
                    .iter()
                    .map(|c| c.name.clone())
                    .collect();
                egui::Window::new("Import SSH hosts from ~/.ssh/config")
                    .collapsible(false)
                    .resizable(true)
                    .default_size([520.0, 440.0])
                    .show(ctx, |ui| {
                        run_ssh_import_dialog_ui(ui, &mut state, &existing);
                    });
                self.apply_ssh_import_dialog_state(state);
            }
        }
    }
}

// ─── apply methods (modal commit) ──────────────────────────────────────────────

impl AutosshApp {
    fn apply_add_dialog_state(&mut self, state: AddDialogState) {
        match state.close {
            CloseAction::None => {
                self.modal = Modal::Add(state);
            }
            CloseAction::Commit => {
                let forwards: Vec<_> = state
                    .forwards
                    .into_iter()
                    .map(EditableForward::into_forward)
                    .collect();
                self.config
                    .connections
                    .push(autossh_core::ConnectionConfig {
                        name: state.name.trim().to_string(),
                        host: Some(state.host.trim().to_string()),
                        enabled: true,
                        ssh_path: None,
                        keepalive: self.keepalive(),
                        retry: self.retry(),
                        extra_args: Vec::new(),
                        forwards,
                    });
                self.dirty = true;
                self.selected_connection = self.config.connections.len() - 1;
                self.flash(format!("added {}", state.name.trim()));
            }
            CloseAction::Cancel(message) => {
                self.flash(message);
            }
        }
    }

    /// Same dialog as Add, but commits overwrite `connections[idx]` instead
    /// of appending. `idx` is captured at modal-open time; if the user
    /// cancels or merely moves entries around, the row is left intact.
    fn apply_edit_connection_state(&mut self, idx: usize, state: AddDialogState) {
        match state.close {
            CloseAction::None => {
                // still editing
                self.modal = Modal::EditConnection { idx, state };
            }
            CloseAction::Commit => {
                if idx >= self.config.connections.len() {
                    // the connection was deleted while the dialog was open.
                    self.flash("connection vanished; edit discarded");
                    return;
                }
                let forwards: Vec<_> = state
                    .forwards
                    .into_iter()
                    .map(EditableForward::into_forward)
                    .collect();
                self.config.connections[idx].name = state.name.trim().to_string();
                self.config.connections[idx].host = Some(state.host.trim().to_string());
                self.config.connections[idx].forwards = forwards;
                self.dirty = true;
                self.flash(format!("updated {}", state.name.trim()));
            }
            CloseAction::Cancel(message) => {
                self.flash(message);
            }
        }
    }

    fn apply_edit_dialog_state(&mut self, state: EditDialogState) {
        match state.close {
            CloseAction::None => {
                self.modal = Modal::EditGlobal {
                    group: state.group,
                    value: state.value,
                };
            }
            CloseAction::Commit => {
                let Ok(n) = state.value.trim().parse::<u64>() else {
                    self.flash("not a non-negative integer; cancelled");
                    return;
                };
                let mut ka = self.keepalive();
                let mut r = self.retry();
                match state.group {
                    GlobalGroup::KeepaliveInterval => ka.interval = n,
                    GlobalGroup::KeepaliveCount => ka.count_max = n as u32,
                    GlobalGroup::KeepaliveTimeout => ka.connect_timeout = n,
                    GlobalGroup::RetryInitial => r.initial_seconds = n,
                    GlobalGroup::RetryMaximum => r.maximum_seconds = n,
                    GlobalGroup::RetryStable => r.stable_seconds = n,
                }
                self.apply_globals(&ka, &r);
                self.dirty = true;
                self.flash(format!("{} → {n}", state.group.label()));
            }
            CloseAction::Cancel(message) => {
                self.flash(message);
            }
        }
    }

    fn apply_import_dialog_state(&mut self, state: ImportDialogState) {
        match state.close {
            CloseAction::None => {
                self.modal = Modal::Import(state);
            }
            CloseAction::Commit => {
                let existing: std::collections::HashSet<String> = self
                    .config
                    .connections
                    .iter()
                    .map(|c| c.name.clone())
                    .collect();
                let mut imported = 0usize;
                let mut skipped = 0usize;
                let mut next_selected = self.selected_connection;
                for cand in state.candidates {
                    if !cand.selected {
                        continue;
                    }
                    if existing.contains(&cand.name) {
                        skipped += 1;
                        continue;
                    }
                    self.config
                        .connections
                        .push(autossh_core::ConnectionConfig {
                            name: cand.name.clone(),
                            host: Some(cand.host),
                            enabled: true,
                            ssh_path: None,
                            keepalive: cand.keepalive,
                            retry: cand.retry,
                            extra_args: Vec::new(),
                            forwards: cand.forwards,
                        });
                    imported += 1;
                    next_selected = self.config.connections.len() - 1;
                }
                if imported > 0 {
                    self.dirty = true;
                    self.selected_connection = next_selected;
                    self.flash(format!("imported {imported} connection(s)"));
                }
                if skipped > 0 {
                    self.flash(format!("skipped {skipped} duplicate name(s)"));
                }
                if imported == 0 && skipped == 0 {
                    self.flash("nothing selected to import");
                }
            }
            CloseAction::Cancel(_) => {}
        }
    }

    fn apply_ssh_import_dialog_state(&mut self, state: SshImportState) {
        match state.close {
            CloseAction::None => {
                self.modal = Modal::ImportSsh(state);
            }
            CloseAction::Commit => {
                let existing: std::collections::HashSet<String> = self
                    .config
                    .connections
                    .iter()
                    .map(|c| c.name.clone())
                    .collect();
                let mut imported = 0usize;
                let mut skipped = 0usize;
                let mut next_selected = self.selected_connection;
                for cand in state.candidates {
                    if !cand.selected || cand.duplicate {
                        if cand.duplicate {
                            skipped += 1;
                        }
                        continue;
                    }
                    if existing.contains(&cand.alias) {
                        skipped += 1;
                        continue;
                    }
                    // SSH config has no forwards; seed a placeholder reverse
                    // tunnel so the config validates, then the user can edit.
                    let placeholder = autossh_core::ForwardConfig {
                        mode: autossh_core::ForwardMode::Remote,
                        forward: "10022:127.0.0.1:22".to_string(),
                    };
                    self.config
                        .connections
                        .push(autossh_core::ConnectionConfig {
                            name: cand.alias.clone(),
                            host: Some(cand.destination.clone()),
                            enabled: true,
                            ssh_path: None,
                            keepalive: self.keepalive(),
                            retry: self.retry(),
                            extra_args: Vec::new(),
                            forwards: vec![placeholder],
                        });
                    imported += 1;
                    next_selected = self.config.connections.len() - 1;
                }
                if imported > 0 {
                    self.dirty = true;
                    self.selected_connection = next_selected;
                    self.flash(format!(
                        "imported {imported} SSH host(s) (placeholder 10022 reverse tunnel added)",
                    ));
                }
                if skipped > 0 {
                    self.flash(format!("skipped {skipped} duplicate alias(es)"));
                }
                if imported == 0 && skipped == 0 {
                    self.flash("nothing selected to import");
                }
            }
            CloseAction::Cancel(_) => {}
        }
    }
}

// ─── help text ─────────────────────────────────────────────────────────────────

const HELP_ROWS: &[(&str, &str)] = &[
    (
        "Start All / Stop All",
        "starts or stops the supervisor and all enabled connections; streams stderr into the bottom log pane.",
    ),
    (
        "Save",
        "atomic write to the TOML config file; the supervisor hot-reloads within ~2 s.",
    ),
    (
        "Add connection",
        "name + host + one or more forwards; L/R use `[bind:]port:host:port`, D (SOCKS) uses `[bind:]port`.",
    ),
    (
        "Import hosts",
        "pulls `[[connections]]` blocks out of another autossh TOML file.",
    ),
    (
        "Globals panel",
        "shared keepalive/retry values; click a row to highlight, double-click to change.",
    ),
    (
        "Logs panel",
        "tail-style stream with severity badges (INFO / WARN / ERROR).",
    ),
    (
        "Layout",
        "left = connections, centre = globals / help, bottom = live log.",
    ),
];
