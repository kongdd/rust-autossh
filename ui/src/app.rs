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
    path::PathBuf,
    time::{Duration, Instant},
};

use autossh_core::{Config, KeepaliveConfig, RetryConfig};
use eframe::egui::{self, Color32, RichText};

use crate::log::{format_unix_ts, LogEntry, LogScroll, LOG_BUFFER_LIMIT};
use crate::modal::{
    run_add_dialog_ui, run_edit_dialog_ui, run_import_dialog_ui,
    run_ssh_import_dialog_ui, AddDialogState, CloseAction, EditableForward,
    EditDialogState, GlobalGroup, Modal, SshImportState,
    ImportDialogState, state_from_connection,
};
use crate::supervisor::{locate_supervisor, SupervisorHandle};

// ─── palette ───────────────────────────────────────────────────────────────────

pub(crate) const FG_PRIMARY: Color32 = Color32::from_rgb(0, 220, 220);
pub(crate) const FG_SUCCESS: Color32 = Color32::from_rgb(0, 200, 120);
pub(crate) const FG_WARNING: Color32 = Color32::from_rgb(245, 200, 70);
pub(crate) const FG_ERROR: Color32 = Color32::from_rgb(245, 90, 90);
pub(crate) const FG_MUTED: Color32 = Color32::from_rgb(140, 145, 160);
pub(crate) const FG_DIM: Color32 = Color32::from_rgb(90, 95, 110);

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
        if self.supervisor.is_some() {
            return;
        }
        let Some(binary) = locate_supervisor() else {
            self.flash("cannot find rust-autossh binary beside this UI; check PATH");
            return;
        };
        match SupervisorHandle::start(&binary, &self.config_path) {
            Ok(handle) => {
                self.flash(format!("supervisor started: {}", binary.display()));
                self.supervisor = Some(handle);
            }
            Err(error) => {
                self.flash(format!("cannot start supervisor: {error:#}"));
            }
        }
    }

    fn poll_supervisor(&mut self) {
        let Some(handle) = self.supervisor.as_ref() else {
            return;
        };
        handle.drain(&mut self.logs);
        if self.logs.len() > LOG_BUFFER_LIMIT {
            let excess = self.logs.len() - (LOG_BUFFER_LIMIT - 100);
            self.logs.drain(..excess);
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
        self.poll_supervisor();
        self.prune_msg();
        self.render_dashboard(ctx);
        self.render_connections_panel(ctx);
        self.render_logs_panel(ctx);
        self.render_centre_panel(ctx);
        self.render_modal(ctx);
        if self.supervisor.is_some() {
            // Re-paint continuously while the supervisor is alive so new lines
            // appear without user interaction.
            ctx.request_repaint_after(Duration::from_millis(150));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
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
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(format!("{} {}", "●", supervisor_state.0))
                            .color(supervisor_state.1)
                            .strong(),
                    );
                    ui.separator();
                    ui.label(
                        RichText::new(dirty_chip.0).color(dirty_chip.1).strong(),
                    );
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
        egui::SidePanel::left("connections")
            .resizable(true)
            .default_width(260.0)
            .width_range(200.0..=420.0)
            .show(ctx, |ui| {
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
                        if ui.button("＋  Add").clicked() {
                            self.modal = Modal::Add(AddDialogState::default());
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
                            RichText::new("press “＋  Add” to start")
                                .small()
                                .color(FG_DIM),
                        );
                    });
                    return;
                }

                let mut delete_at: Option<usize> = None;
                let mut edit_at: Option<usize> = None;
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (i, conn) in self.config.connections.iter().enumerate() {
                            let selected = i == self.selected_connection;
                            let frame = egui::Frame::group(ui.style())
                                .fill(if selected {
                                    Color32::from_rgb(34, 56, 70)
                                } else {
                                    Color32::from_rgb(24, 28, 34)
                                })
                                .stroke(egui::Stroke::new(
                                    if selected { 1.0 } else { 0.5 },
                                    if selected { FG_PRIMARY } else { FG_DIM },
                                ))
                                .rounding(egui::Rounding::same(4.0))
                                .inner_margin(egui::Margin::symmetric(10.0, 8.0));
                            frame.show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    let dot_color = if conn.enabled {
                                        FG_SUCCESS
                                    } else {
                                        FG_WARNING
                                    };
                                    ui.colored_label(dot_color, "●");
                                    ui.vertical(|ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new(&conn.name)
                                                    .strong()
                                                    .color(Color32::WHITE),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    if ui.small_button("delete").clicked() {
                                                        delete_at = Some(i);
                                                    }
                                                },
                                            );
                                        });
                                        ui.label(
                                            RichText::new(conn.destination())
                                                .small()
                                                .color(FG_MUTED),
                                        );
                                        ui.label(
                                            RichText::new(format!(
                                                "{} forwards",
                                                conn.forwards.len()
                                            ))
                                            .small()
                                            .color(FG_PRIMARY),
                                        );
                                    });
                                });
                                let interact =
                                    ui.interact(ui.max_rect(), ui.id().with(i), egui::Sense::click());
                                if interact.clicked() {
                                    self.selected_connection = i;
                                }
                                if interact.double_clicked() {
                                    edit_at = Some(i);
                                }
                            });
                            ui.add_space(4.0);
                        }
                    });

                if let Some(i) = delete_at {
                    let removed = self.config.connections.remove(i);
                    self.dirty = true;
                    self.flash(format!("deleted {}", removed.name));
                    if self.selected_connection >= self.config.connections.len()
                        && self.selected_connection > 0
                    {
                        self.selected_connection -= 1;
                    }
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
                    if self.supervisor.is_none() && ui.button("▶  Start supervisor").clicked() {
                        self.start_supervisor();
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

        let mut selected_group = self.selected_global.min(5);
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(8.0);
            ui.columns(2, |columns| {
                columns[0].group(|ui| {
                    ui.add_space(4.0);
                    ui.heading("Keepalive");
                    ui.add_space(4.0);
                    self.render_global_row(
                        ui,
                        0,
                        &mut selected_group,
                        GlobalGroup::KeepaliveInterval,
                        format!("{} s", ka.interval),
                    );
                    self.render_global_row(
                        ui,
                        1,
                        &mut selected_group,
                        GlobalGroup::KeepaliveCount,
                        format!("{}", ka.count_max),
                    );
                    self.render_global_row(
                        ui,
                        2,
                        &mut selected_group,
                        GlobalGroup::KeepaliveTimeout,
                        format!("{} s", ka.connect_timeout),
                    );
                });
                columns[1].group(|ui| {
                    ui.add_space(4.0);
                    ui.heading("Retry");
                    ui.add_space(4.0);
                    self.render_global_row(
                        ui,
                        3,
                        &mut selected_group,
                        GlobalGroup::RetryInitial,
                        format!("{} s", r.initial_seconds),
                    );
                    self.render_global_row(
                        ui,
                        4,
                        &mut selected_group,
                        GlobalGroup::RetryMaximum,
                        format!("{} s", r.maximum_seconds),
                    );
                    self.render_global_row(
                        ui,
                        5,
                        &mut selected_group,
                        GlobalGroup::RetryStable,
                        format!("{} s", r.stable_seconds),
                    );
                });
            });
            ui.add_space(8.0);
            ui.label(
                RichText::new(
                    "values are shared by every connection; click a row to highlight, double-click to edit",
                )
                .small()
                .color(FG_MUTED),
            );
            ui.add_space(8.0);
            ui.collapsing("Active connections", |ui| {
                for (i, c) in self.config.connections.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{}.", i + 1)).color(FG_MUTED));
                        ui.label(RichText::new(&c.name).strong().color(Color32::WHITE));
                        ui.label(RichText::new(format!("→ {}", c.destination())).color(FG_MUTED));
                    });
                }
            });
        });
        self.selected_global = selected_group;
    }

    /// One keepalive/retry row. Click sets the selection; double-click
    /// (or the inline "edit" link) opens the editor modal for that field.
    fn render_global_row(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        selected: &mut usize,
        group: GlobalGroup,
        value: String,
    ) {
        let is_sel = *selected == index;
        let frame = egui::Frame::group(ui.style())
            .fill(if is_sel {
                Color32::from_rgb(34, 56, 70)
            } else {
                Color32::from_rgb(24, 28, 34)
            })
            .stroke(egui::Stroke::new(
                if is_sel { 1.0 } else { 0.5 },
                if is_sel { FG_PRIMARY } else { FG_DIM },
            ))
            .rounding(egui::Rounding::same(4.0))
            .inner_margin(egui::Margin::symmetric(10.0, 6.0));
        let response = frame.show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new(group.label()).color(FG_MUTED).small());
                    ui.label(RichText::new(&value).strong().color(FG_PRIMARY).monospace());
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add(
                            egui::Label::new(RichText::new("✎  edit").color(FG_PRIMARY).small())
                                .sense(egui::Sense::click()),
                        )
                        .clicked()
                    {
                        *selected = index;
                        self.modal = Modal::EditGlobal {
                            group,
                            value: value.clone(),
                        };
                    }
                });
            });
        });
        let interact = response.response.interact(egui::Sense::click());
        if interact.clicked() {
            *selected = index;
        }
        if interact.double_clicked() {
            *selected = index;
            self.modal = Modal::EditGlobal {
                group,
                value: value.clone(),
            };
        }
    }

    fn render_help(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.heading("Quick reference");
        ui.add_space(4.0);
        for (key, desc) in HELP_ROWS {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(RichText::new(format!("{:<16}", key)).strong().color(FG_PRIMARY));
                ui.label(*desc);
            });
        }
        ui.add_space(12.0);
        ui.label(RichText::new("Failure handling").strong().size(14.0));
        ui.add_space(4.0);
        ui.label("When a connection's ssh process exits or fails to start, the supervisor:");
        ui.label("  • reads exit code from the supervisor log line tagged with the connection name");
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
                        if ui
                            .selectable_label(self.log_scroll.follow, "follow")
                            .clicked()
                        {
                            self.log_scroll.follow = true;
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
                                        .color(FG_DIM),
                                );
                                // severity badge
                                let (badge_text, badge_bg) = (
                                    format!(" {} ", entry.severity.label()),
                                    entry.severity.badge(),
                                );
                                let badge_fg = entry.severity.foreground();
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
                                            .color(FG_PRIMARY),
                                    );
                                    let message = entry
                                        .text
                                        .rsplit_once(':')
                                        .map(|(_, rest)| rest.trim_start())
                                        .unwrap_or(&entry.text);
                                    ui.label(
                                        RichText::new(format!(": {message}"))
                                            .color(Color32::from_rgb(220, 222, 230)),
                                    );
                                } else {
                                    let message = entry
                                        .text
                                        .splitn(3, ' ')
                                        .nth(2)
                                        .unwrap_or(&entry.text);
                                    ui.label(
                                        RichText::new(message.to_string())
                                            .color(Color32::from_rgb(220, 222, 230)),
                                    );
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
                self.config.connections.push(autossh_core::ConnectionConfig {
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
                    self.config.connections.push(autossh_core::ConnectionConfig {
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
                    self.config.connections.push(autossh_core::ConnectionConfig {
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
        "Start supervisor",
        "spawns rust-autossh run; streams stderr into the bottom log pane.",
    ),
    (
        "Save",
        "atomic write to the TOML config file; the supervisor hot-reloads within ~2 s.",
    ),
    (
        "Add connection",
        "name + host + one or more forwards, each typed as <listen>:<host>:<port>.",
    ),
    (
        "Import hosts",
        "pulls `[[connections]]` blocks out of another autossh TOML file.",
    ),
    (
        "Globals panel",
        "shared keepalive/retry values; click a row to highlight, ✎ edit / double-click to change.",
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
