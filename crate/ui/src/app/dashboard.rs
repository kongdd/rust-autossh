//! Top status bar: title, config path, save/supervisor/log counters, and the
//! ephemeral `flash()` toast message.

use eframe::egui::{self, Color32, RichText};

use crate::log::{FG_ERROR, FG_MUTED, FG_PRIMARY, FG_SUCCESS, FG_WARNING};

use super::AutosshApp;

impl AutosshApp {
    pub fn render_dashboard(&mut self, ctx: &egui::Context) {
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
                        FG_WARNING
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
