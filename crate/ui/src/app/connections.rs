//! Left-side connections panel — checkbox-list of every connection with
//! inline start/stop, single-click select, and double-click edit.

use eframe::egui::{self, Color32, RichText};

use crate::log::{FG_DIM, FG_ERROR, FG_MUTED, FG_PRIMARY, FG_SUCCESS};
use crate::modal::{Modal, state_from_connection};

use super::AutosshApp;

impl AutosshApp {
    pub fn render_connections_panel(&mut self, ctx: &egui::Context) {
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
                                .map(std::path::PathBuf::from)
                                .unwrap_or_default();
                            self.modal = Modal::Import(crate::modal::ImportDialogState {
                                path_input: initial.display().to_string(),
                                ..crate::modal::ImportDialogState::default()
                            });
                        }
                        if ui.button("🖥  SSH hosts").clicked() {
                            self.modal = Modal::ImportSsh(crate::modal::SshImportState {
                                source_path: crate::ssh_config::default_ssh_config_path(),
                                ..crate::modal::SshImportState::default()
                            });
                        }
                        // Use ASCII `+`: the full-width plus (`＋`) renders as a
                        // missing-glyph box in the Windows system font fallback.
                        if ui.button("+  Add").clicked() {
                            self.modal = Modal::Add(crate::modal::AddDialogState::default());
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
                                .fill(fill)
                                .stroke(egui::Stroke::new(sw, sc))
                                .rounding(egui::Rounding::same(4.0))
                                .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        // checkbox
                                        let mut cb = self.checked_conn.contains(&i);
                                        if ui
                                            .checkbox(&mut cb, "")
                                            .on_hover_text("Select for batch deletion")
                                            .changed()
                                        {
                                            if cb {
                                                self.checked_conn.insert(i);
                                            } else {
                                                self.checked_conn.remove(&i);
                                            }
                                        }
                                        // status dot
                                        let running = supervisor_running && conn.enabled;
                                        ui.colored_label(
                                            if running { FG_SUCCESS } else { FG_MUTED },
                                            "●",
                                        );
                                        // clickable details (select on click, edit on double-click)
                                        let mut mk = |text: RichText| {
                                            ui.add(
                                                egui::Label::new(text).sense(egui::Sense::click()),
                                            )
                                        };
                                        let r1 = mk(RichText::new(&conn.name).strong());
                                        let destination = match conn.port {
                                            Some(port) => format!("{}:{port}", conn.destination()),
                                            None => conn.destination(),
                                        };
                                        let details = match conn.description.as_deref() {
                                            Some(description) if !description.trim().is_empty() => {
                                                format!("{destination}  ·  {description}")
                                            }
                                            _ => destination,
                                        };
                                        let r2 = mk(RichText::new(details).small().color(FG_MUTED));
                                        let r3 = mk(RichText::new(format!(
                                            "{} forwards",
                                            conn.forwards.len()
                                        ))
                                        .small()
                                        .color(FG_PRIMARY));
                                        let clicked = r1.clicked() || r2.clicked() || r3.clicked();
                                        let dbl = r1.double_clicked()
                                            || r2.double_clicked()
                                            || r3.double_clicked();
                                        if clicked {
                                            self.selected_connection = i;
                                        }
                                        if dbl {
                                            edit_at = Some(i);
                                        }

                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                let label = if running {
                                                    "■  Stop"
                                                } else {
                                                    "▶  Start"
                                                };
                                                if ui.small_button(label).clicked() {
                                                    toggle_at = Some(i);
                                                }
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
