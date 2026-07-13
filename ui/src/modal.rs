//! Modal dialog types and their UI functions.
//!
//! Each modal captures its own state and exposes a `CloseAction` that the
//! caller (`AutosshApp`) interprets to apply or discard changes.

use std::path::PathBuf;

use autossh_core::{
    Config, ConnectionConfig, ForwardConfig, ForwardMode, KeepaliveConfig, RetryConfig,
};
use eframe::egui::{self, Color32, RichText};

pub use crate::ssh_config::SshHostEntry;
use crate::ssh_config::parse_ssh_config;

use crate::log::{FG_DIM, FG_ERROR, FG_MUTED, FG_PRIMARY, FG_SUCCESS, FG_WARNING};

// ─── Pickable trait (shared by both import dialogs) ───────────────────────

trait Pickable {
    fn picked(&self) -> bool;
    fn set_picked(&mut self, v: bool);
    fn is_dup(&self) -> bool;
}

/// Render a scrollable checklist with select-all, used by both import dialogs.
fn pick_list<T: Pickable>(
    ui: &mut egui::Ui, items: &mut [T],
    render: impl Fn(&T, &mut egui::Ui),
) {
    let mut all = items.iter().filter(|c| !c.is_dup()).all(|c| c.picked());
    ui.horizontal(|ui| {
        if ui.checkbox(&mut all, "select all").changed() {
            for c in items.iter_mut().filter(|c| !c.is_dup()) { c.set_picked(all); }
        }
    });
    egui::ScrollArea::vertical()
        .max_height(220.0).auto_shrink([false, false])
        .show(ui, |ui| {
            for c in items.iter_mut() {
                let mut sel = c.picked();
                ui.horizontal(|ui| {
                    ui.add_enabled(!c.is_dup(), egui::Checkbox::new(&mut sel, ""));
                    if sel != c.picked() { c.set_picked(sel); }
                    render(c, ui);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if c.is_dup() { ui.colored_label(FG_WARNING, "(duplicate)"); }
                        else if c.picked() { ui.colored_label(FG_PRIMARY, "✓"); }
                    });
                });
            }
        });
}

impl Pickable for CandidateConnection {
    fn picked(&self) -> bool { self.selected }
    fn set_picked(&mut self, v: bool) { self.selected = v; }
    fn is_dup(&self) -> bool { self.duplicate }
}

impl Pickable for SshHostEntry {
    fn picked(&self) -> bool { self.selected }
    fn set_picked(&mut self, v: bool) { self.selected = v; }
    fn is_dup(&self) -> bool { self.duplicate }
}

// ─── enums ─────────────────────────────────────────────────────────────────────

/// Which global field is being edited in the `EditGlobal` modal.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum GlobalGroup {
    KeepaliveInterval,
    KeepaliveCount,
    KeepaliveTimeout,
    RetryInitial,
    RetryMaximum,
    RetryStable,
}

impl GlobalGroup {
    pub fn label(self) -> &'static str {
        match self {
            Self::KeepaliveInterval => "keepalive.interval (s)",
            Self::KeepaliveCount => "keepalive.count_max",
            Self::KeepaliveTimeout => "keepalive.connect_timeout (s)",
            Self::RetryInitial => "retry.initial_seconds (s)",
            Self::RetryMaximum => "retry.maximum_seconds (s)",
            Self::RetryStable => "retry.stable_seconds (s)",
        }
    }
}

/// What the user did inside the dialog — used by the caller to decide whether
/// and how to commit the state.
#[derive(Clone, Debug, Default)]
pub enum CloseAction {
    #[default]
    None,
    Commit,
    Cancel(&'static str),
}

// ─── modal enum ────────────────────────────────────────────────────────────────

/// Modal dialogs for adding a connection, editing a single global value,
/// or importing hosts from another TOML config file.
#[derive(Default)]
pub enum Modal {
    #[default]
    None,
    Add(AddDialogState),
    /// Editing an existing connection; `idx` is the row in `config.connections`.
    EditConnection {
        idx: usize,
        state: AddDialogState,
    },
    EditGlobal {
        group: GlobalGroup,
        value: String,
    },
    Import(ImportDialogState),
    ImportSsh(SshImportState),
}

// ─── dialog state types ────────────────────────────────────────────────────────

pub struct AddDialogState {
    pub name: String,
    pub host: String,
    /// Each entry is editable in-place inside the dialog (mode dropdown, free
    /// text, delete). Converted to `Vec<ForwardConfig>` only on commit.
    pub forwards: Vec<EditableForward>,
    pub checked: Vec<bool>,
    pub draft_mode: ForwardMode,
    pub draft_forward: String,
    pub close: CloseAction,
}

impl Default for AddDialogState {
    fn default() -> Self {
        Self {
            name: String::new(),
            host: String::new(),
            forwards: Vec::new(),
            checked: Vec::new(),
            // New connections default to remote forwards (the canonical autossh
            // pattern of opening an inbound tunnel to a client).
            draft_mode: ForwardMode::Remote,
            draft_forward: String::new(),
            close: CloseAction::None,
        }
    }
}

#[derive(Clone)]
pub struct EditableForward {
    pub mode: ForwardMode,
    pub forward: String,
}

impl EditableForward {
    pub fn into_forward(self) -> ForwardConfig {
        ForwardConfig {
            mode: self.mode,
            forward: self.forward,
        }
    }
}

pub struct EditDialogState {
    pub group: GlobalGroup,
    pub value: String,
    pub close: CloseAction,
}

#[derive(Default)]
pub struct ImportDialogState {
    pub path_input: String,
    pub status: ImportStatus,
    pub candidates: Vec<CandidateConnection>,
    pub close: CloseAction,
}

#[derive(Debug, Default)]
pub enum ImportStatus {
    #[default]
    Idle,
    Loaded(PathBuf),
    Failed(String),
}

#[derive(Default)]
pub struct SshImportState {
    pub source_path: PathBuf,
    pub candidates: Vec<SshHostEntry>,
    pub status: SshImportStatus,
    pub close: CloseAction,
}

#[derive(Debug, Default)]
pub enum SshImportStatus {
    #[default]
    Idle,
    Loaded,
    Failed(String),
}

pub struct CandidateConnection {
    pub name: String,
    pub host: String,
    pub forwards: Vec<ForwardConfig>,
    pub keepalive: KeepaliveConfig,
    pub retry: RetryConfig,
    pub selected: bool,
    pub duplicate: bool,
}

// ─── helpers ───────────────────────────────────────────────────────────────────

/// Compact label used in the per-row mode dropdown (existing forwards + draft).
fn forward_mode_label(mode: ForwardMode) -> &'static str {
    match mode {
        ForwardMode::Local => "L local",
        ForwardMode::Remote => "R remote",
        ForwardMode::Dynamic => "D dynamic",
    }
}

/// Mode-aware placeholder: hint states which side listens vs targets.
fn forward_hint(mode: ForwardMode) -> &'static str {
    match mode {
        ForwardMode::Local => "0.0.0.0:8080:127.0.0.1:80  (listen on client → target on server)",
        ForwardMode::Remote => "10022:127.0.0.1:22  (listen on server → target on client)",
        ForwardMode::Dynamic => "1080  (or  0.0.0.0:1080 for an open SOCKS proxy)",
    }
}

/// Build an `AddDialogState` from an existing connection (e.g. for editing).
pub fn state_from_connection(c: &ConnectionConfig) -> AddDialogState {
    let n = c.forwards.len();
    AddDialogState {
        name: c.name.clone(),
        host: c.host.clone().unwrap_or_else(|| c.name.clone()),
        forwards: c
            .forwards
            .iter()
            .map(|f| EditableForward {
                mode: f.mode,
                forward: f.forward.clone(),
            })
            .collect(),
        checked: vec![false; n],
        // Carry the same default forward type as the add dialog for new lines.
        draft_mode: ForwardMode::Remote,
        draft_forward: String::new(),
        close: CloseAction::None,
    }
}

// ─── dialog UI: add / edit connection ─────────────────────────────────────────

pub fn run_add_dialog_ui(ui: &mut egui::Ui, state: &mut AddDialogState) {
    // Esc closes the dialog whether the user has filled fields or not.
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.close = CloseAction::Cancel("cancelled");
    }

    // ── host identity ─────────────────────────────────────────────────────
    ui.add_space(2.0);
    ui.label(RichText::new("Name").strong().color(FG_PRIMARY));
    ui.add(
        egui::TextEdit::singleline(&mut state.name)
            .hint_text("home-server")
            .desired_width(f32::INFINITY),
    );
    ui.add_space(6.0);
    ui.label(
        RichText::new("Host (SSH alias or user@host)")
            .strong()
            .color(FG_PRIMARY),
    );
    ui.add(
        egui::TextEdit::singleline(&mut state.host)
            .hint_text("user@example.com")
            .desired_width(f32::INFINITY),
    );

    // ── forwarded ports ───────────────────────────────────────────────────
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Forwarded ports").strong().color(FG_PRIMARY));
        ui.label(
            RichText::new(format!("{} configured", state.forwards.len()))
                .small()
                .color(FG_MUTED),
        );
    });
    ui.group(|ui| {
        // existing forwards: each is its own editable row.
        if state.forwards.is_empty() {
            ui.label(
                RichText::new("no ports yet — fill the row at the bottom and press ＋ add")
                    .small()
                    .color(FG_DIM),
            );
        }
        state.checked.resize(state.forwards.len(), false);
        for (i, fr) in state.forwards.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{}.", i + 1)).color(FG_MUTED));
                let mut checked = *state.checked.get(i).unwrap_or(&false);
                if ui.checkbox(&mut checked, "").changed() && i < state.checked.len() {
                    state.checked[i] = checked;
                }
                egui::ComboBox::from_id_salt(("add_mode", i))
                    .selected_text(forward_mode_label(fr.mode))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut fr.mode, ForwardMode::Local, forward_mode_label(ForwardMode::Local));
                        ui.selectable_value(&mut fr.mode, ForwardMode::Remote, forward_mode_label(ForwardMode::Remote));
                        ui.selectable_value(&mut fr.mode, ForwardMode::Dynamic, forward_mode_label(ForwardMode::Dynamic));
                    });
                ui.add(
                    egui::TextEdit::singleline(&mut fr.forward)
                        .hint_text(forward_hint(fr.mode))
                        .desired_width(f32::INFINITY),
                );
            });
        }
        let n_checked = state.checked.iter().filter(|&&c| c).count();
        if n_checked > 0 {
            ui.horizontal(|ui| {
                if ui
                    .button(
                        RichText::new(format!("\u{1f5d1}  delete checked ({n_checked})"))
                            .color(FG_ERROR),
                    )
                    .clicked()
                {
                    let mut i = state.forwards.len();
                    while i > 0 {
                        i -= 1;
                        if i < state.checked.len() && state.checked[i] {
                            state.forwards.remove(i);
                            state.checked.remove(i);
                        }
                    }
                }
            });
        }

        ui.add_space(4.0);
        ui.separator();
        ui.add_space(4.0);

        // row for adding a new port (always visible so users can keep extending)
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("draft_mode")
                .selected_text(forward_mode_label(state.draft_mode))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut state.draft_mode, ForwardMode::Local, forward_mode_label(ForwardMode::Local));
                    ui.selectable_value(&mut state.draft_mode, ForwardMode::Remote, forward_mode_label(ForwardMode::Remote));
                    ui.selectable_value(&mut state.draft_mode, ForwardMode::Dynamic, forward_mode_label(ForwardMode::Dynamic));
                });
            let resp = ui.add(
                egui::TextEdit::singleline(&mut state.draft_forward)
                    .hint_text(forward_hint(state.draft_mode))
                    .desired_width(f32::INFINITY),
            );
            // egui 0.29 singleline TextEdit surrenders focus on Enter but
            // also consumes the key event — lost_focus() alone is reliable.
            let submitted = resp.lost_focus();
            if ui.button("+ add").clicked() || submitted {
                let trimmed = state.draft_forward.trim().to_string();
                if !trimmed.is_empty() {
                    state.forwards.push(EditableForward {
                        mode: state.draft_mode,
                        forward: trimmed,
                    });
                    state.draft_forward.clear();
                }
            }
        });
    });

    // ── footer ─────────────────────────────────────────────────────────────
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let can_commit = !state.name.trim().is_empty()
            && !state.host.trim().is_empty()
            && !state.forwards.is_empty();
        if ui
            .add_enabled(
                can_commit,
                egui::Button::new(RichText::new("✔  Add connection").strong()),
            )
            .clicked()
        {
            state.close = CloseAction::Commit;
        }
        if ui.button("cancel").clicked() {
            state.close = CloseAction::Cancel("cancelled");
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new("Esc to close").small().color(FG_DIM));
        });
    });
}

// ─── dialog UI: edit global value ─────────────────────────────────────────────

pub fn run_edit_dialog_ui(ui: &mut egui::Ui, state: &mut EditDialogState) {
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.close = CloseAction::Cancel("cancelled");
    }

    ui.add_space(4.0);
    ui.label(
        RichText::new(state.group.label())
            .strong()
            .color(FG_PRIMARY),
    );
    let resp = ui.add(egui::TextEdit::singleline(&mut state.value).desired_width(f32::INFINITY));
    // egui 0.29 singleline surrenders focus on Enter but consumes the event.
    let submitted = resp.lost_focus();
    ui.add_space(8.0);
    ui.label(
        RichText::new("Applied value is broadcast to every connection on save.")
            .small()
            .color(FG_MUTED),
    );
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ui.button("✔  Apply").clicked() || submitted {
            if state.value.trim().parse::<u64>().is_ok() {
                state.close = CloseAction::Commit;
            } else {
                state.close = CloseAction::Cancel("not a non-negative integer");
            }
        }
        if ui.button("cancel").clicked() {
            state.close = CloseAction::Cancel("cancelled");
        }
    });
}

// ─── dialog UI: import from TOML ──────────────────────────────────────────────

pub fn run_import_dialog_ui(ui: &mut egui::Ui, state: &mut ImportDialogState, existing: &[String]) {
    // Esc closes the dialog regardless of its current state (loading, failed, populated).
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.close = CloseAction::Cancel("cancelled");
    }

    // ── header: path + load
    ui.add_space(4.0);
    ui.label(RichText::new("Source TOML file").strong().color(FG_PRIMARY));
    let resp = ui.add(
        egui::TextEdit::singleline(&mut state.path_input)
            .hint_text("~/path/to/another-autossh-config.toml")
            .desired_width(f32::INFINITY),
    );
    // egui 0.29 singleline surrenders focus on Enter but consumes the event.
    let submitted = resp.lost_focus();
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        if ui.button("📂  Load").clicked() || submitted {
            state.try_load(existing);
        }
    });
    ui.add_space(4.0);
    match &state.status {
        ImportStatus::Idle => {
            ui.label(RichText::new("(no source loaded)").small().color(FG_DIM));
        }
        ImportStatus::Loaded(path) => {
            ui.label(
                RichText::new(format!("✓ loaded {}", path.display()))
                    .small()
                    .color(FG_SUCCESS),
            );
        }
        ImportStatus::Failed(message) => {
            ui.colored_label(FG_ERROR, message);
        }
    }
    ui.add_space(8.0);
    ui.label(
        RichText::new(format!(
            "{} candidate connection(s)",
            state.candidates.len()
        ))
        .small()
        .color(FG_MUTED),
    );
    ui.separator();

    // ── body: candidates or empty/loading/error placeholder
    if state.candidates.is_empty() {
        let placeholder_text = match &state.status {
            ImportStatus::Failed(_) => "load failed — fix the path or press Esc / cancel to close",
            ImportStatus::Idle => "load a TOML file above to list candidates",
            ImportStatus::Loaded(_) => "the file had no [[connections]] blocks",
        };
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(placeholder_text).small().color(FG_DIM));
        });
    } else {
        pick_list(ui, &mut state.candidates, |c, ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(&c.name).strong());
                ui.label(
                    RichText::new(format!("{}  ·  {} forwards", c.host, c.forwards.len()))
                        .small().color(FG_MUTED),
                );
            });
        });
    }

    // ── footer: import + cancel buttons (always visible, even on load failure)
    ui.add_space(8.0);
    ui.separator();
    let any_selectable = state.candidates.iter().any(|c| c.selected && !c.duplicate);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                any_selectable,
                egui::Button::new(RichText::new("✔  Import selected").strong()),
            )
            .clicked()
        {
            state.close = CloseAction::Commit;
        }
        if ui.button("cancel").clicked() {
            state.close = CloseAction::Cancel("cancelled");
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new("Esc to close").small().color(FG_DIM));
        });
    });
}

// ─── dialog UI: import from SSH config ────────────────────────────────────────

pub fn run_ssh_import_dialog_ui(
    ui: &mut egui::Ui,
    state: &mut SshImportState,
    existing: &[String],
) {
    // Esc closes the dialog whether the load succeeded, failed, or hasn't run.
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.close = CloseAction::Cancel("cancelled");
    }

    ui.add_space(4.0);
    ui.label(
        RichText::new("Source OpenSSH config")
            .strong()
            .color(FG_PRIMARY),
    );
    ui.label(
        RichText::new(state.source_path.display().to_string())
            .monospace()
            .color(Color32::from_rgb(180, 200, 220)),
    );
    ui.add_space(6.0);
    if ui.button("📂  Load").clicked() {
        state.try_load(existing);
    }
    ui.add_space(4.0);
    match &state.status {
        SshImportStatus::Idle => {
            ui.label(RichText::new("(no source loaded)").small().color(FG_DIM));
        }
        SshImportStatus::Loaded => {
            ui.label(
                RichText::new(format!("✓ loaded {} host(s)", state.candidates.len()))
                    .small()
                    .color(FG_SUCCESS),
            );
        }
        SshImportStatus::Failed(message) => {
            ui.colored_label(FG_ERROR, message);
        }
    }

    // hint about the autossh-required placeholder
    ui.add_space(4.0);
    ui.label(
        RichText::new(
            "autossh requires at least one port per connection; imported hosts will get a placeholder R 10022:127.0.0.1:22 that you can edit later.",
        )
        .small()
        .color(FG_MUTED),
    );
    ui.separator();

    // body
    if state.candidates.is_empty() {
        let placeholder_text = match &state.status {
            SshImportStatus::Failed(_) => "load failed — press Esc / cancel to close",
            SshImportStatus::Idle => "press Load to read SSH host aliases",
            SshImportStatus::Loaded => "the file declared no usable Host blocks",
        };
        ui.add_space(20.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new(placeholder_text).small().color(FG_DIM));
        });
    } else {
        pick_list(ui, &mut state.candidates, |c, ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(&c.alias).strong());
                ui.label(
                    RichText::new(format!("{}  ·  port {}", c.destination, c.port))
                        .small().color(FG_MUTED),
                );
            });
        });
    }

    // footer (always visible)
    ui.add_space(8.0);
    ui.separator();
    let any_selectable = state.candidates.iter().any(|c| c.selected && !c.duplicate);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                any_selectable,
                egui::Button::new(RichText::new("✔  Import selected").strong()),
            )
            .clicked()
        {
            state.close = CloseAction::Commit;
        }
        if ui.button("cancel").clicked() {
            state.close = CloseAction::Cancel("cancelled");
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new("Esc to close").small().color(FG_DIM));
        });
    });
}

// ─── import dialog logic ───────────────────────────────────────────────────────

impl ImportDialogState {
    /// Parse the user-supplied path, populate `candidates` and bump the status.
    /// Names that already exist in the current config are flagged as duplicates
    /// and unselected by default so the user can only import what's truly new.
    pub fn try_load(&mut self, existing: &[String]) {
        let trimmed = self.path_input.trim();
        if trimmed.is_empty() {
            self.status = ImportStatus::Failed("path is empty".into());
            self.candidates.clear();
            return;
        }
        let path = PathBuf::from(trimmed);
        match Config::load(&path) {
            Ok(config) => {
                self.candidates = config
                    .connections
                    .into_iter()
                    .map(|c| {
                        let duplicate = existing.iter().any(|n| n == &c.name);
                        let name = c.name.clone();
                        let host = c.destination().to_string();
                        CandidateConnection {
                            name,
                            host,
                            forwards: c.forwards,
                            keepalive: c.keepalive,
                            retry: c.retry,
                            selected: !duplicate,
                            duplicate,
                        }
                    })
                    .collect();
                self.status = ImportStatus::Loaded(path);
            }
            Err(error) => {
                self.candidates.clear();
                self.status = ImportStatus::Failed(format!("{error:#}"));
            }
        }
    }
}

impl SshImportState {
    pub fn try_load(&mut self, existing: &[String]) {
        match parse_ssh_config(&self.source_path) {
            Ok(candidates) => {
                let entries: Vec<SshHostEntry> = candidates
                    .into_iter()
                    .map(|mut entry| {
                        // 默认一个都不选，由用户通过 select all 或逐条勾选
                        entry.duplicate = existing.iter().any(|n| n == &entry.alias);
                        entry.selected = false;
                        entry
                    })
                    .collect();
                // Stable order: original parse order is preserved.
                if entries.is_empty() {
                    self.status = SshImportStatus::Loaded;
                } else {
                    self.candidates = entries;
                    self.status = SshImportStatus::Loaded;
                }
            }
            Err(error) => {
                self.candidates.clear();
                self.status = SshImportStatus::Failed(format!("{error:#}"));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use autossh_core::{
        ConnectionConfig, ForwardConfig, ForwardMode, KeepaliveConfig, RetryConfig,
    };

    fn make_connection(name: &str, host: &str, forward: &str) -> ConnectionConfig {
        ConnectionConfig {
            name: name.into(),
            host: Some(host.into()),
            enabled: true,
            ssh_path: None,
            keepalive: KeepaliveConfig::default(),
            retry: RetryConfig::default(),
            extra_args: Vec::new(),
            forwards: vec![ForwardConfig {
                mode: ForwardMode::Local,
                forward: forward.into(),
            }],
        }
    }

    #[test]
    fn import_dialog_flags_duplicates_and_unselects_them() {
        let dir = std::env::temp_dir().join("autossh-ui-import-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("source.toml");
        let _ = std::fs::remove_file(&path);
        let src = Config {
            log: Default::default(),
            connections: vec![
                make_connection("home", "c005", "8080:127.0.0.1:8080"),
                make_connection("backup", "c006", "10022:127.0.0.1:22"),
            ],
        };
        src.save(&path).unwrap();

        let mut dialog = ImportDialogState {
            path_input: path.display().to_string(),
            ..Default::default()
        };
        let existing = vec!["home".to_string()];
        dialog.try_load(&existing);

        assert_eq!(dialog.candidates.len(), 2);
        match &dialog.status {
            ImportStatus::Loaded(_) => {}
            _ => panic!("expected Loaded status, got {:?}", dialog.status),
        }
        let home = dialog
            .candidates
            .iter()
            .find(|c| c.name == "home")
            .expect("home should be present");
        assert!(home.duplicate, "home collides with existing");
        assert!(!home.selected, "duplicates default to unselected");
        let backup = dialog
            .candidates
            .iter()
            .find(|c| c.name == "backup")
            .expect("backup should be present");
        assert!(!backup.duplicate);
        assert!(backup.selected, "new candidates default to selected");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_dialog_surfaces_load_errors() {
        let mut dialog = ImportDialogState {
            path_input: "/nonexistent/path/__does_not_exist.toml".to_string(),
            ..Default::default()
        };
        dialog.try_load(&[]);
        match &dialog.status {
            ImportStatus::Failed(_) => {}
            other => panic!("expected Failed status, got {other:?}"),
        }
        assert!(dialog.candidates.is_empty());
    }

    #[test]
    fn state_from_connection_round_trips_fields() {
        let conn = make_connection("home", "c005", "10022:127.0.0.1:22");
        let state = state_from_connection(&conn);
        assert_eq!(state.name, "home");
        assert_eq!(state.host, "c005");
        assert_eq!(state.forwards.len(), 1);
        assert_eq!(state.forwards[0].forward, "10022:127.0.0.1:22");
        assert_eq!(state.forwards[0].mode, ForwardMode::Local);
        // fresh edit starts with no draft so the user is not surprised
        assert!(state.draft_forward.is_empty());
    }

    #[test]
    fn edit_connection_falls_back_to_name_when_host_missing() {
        // ssh-import style entries sometimes have host=None; falling back
        // to the connection name keeps the dialog usable.
        let conn = ConnectionConfig {
            name: "home".into(),
            host: None,
            enabled: true,
            ssh_path: None,
            keepalive: KeepaliveConfig::default(),
            retry: RetryConfig::default(),
            extra_args: Vec::new(),
            forwards: vec![],
        };
        let state = state_from_connection(&conn);
        assert_eq!(state.host, "home");
    }
}
