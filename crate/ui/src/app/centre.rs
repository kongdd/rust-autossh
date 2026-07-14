//! Centre pane: keepalive/retry editor and Save / Start All / Stop All actions.

use eframe::egui::{self, Color32, RichText};

use crate::log::{FG_DIM, FG_ERROR, FG_MUTED, FG_PRIMARY, FG_SUCCESS, FG_WARNING};
use crate::modal::{GlobalGroup, Modal};
use friday::{FridayState, LISTEN_ADDR};

use super::AutosshApp;

impl AutosshApp {
    pub fn render_centre_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
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
            self.render_globals(ui);
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
            (
                0,
                GlobalGroup::KeepaliveInterval,
                (format!("{} s", ka.interval), ka.interval.to_string()),
            ),
            (
                1,
                GlobalGroup::KeepaliveCount,
                (ka.count_max.to_string(), ka.count_max.to_string()),
            ),
            (
                2,
                GlobalGroup::KeepaliveTimeout,
                (
                    format!("{} s", ka.connect_timeout),
                    ka.connect_timeout.to_string(),
                ),
            ),
        ];
        let retry: [(usize, GlobalGroup, (String, String)); 3] = [
            (
                3,
                GlobalGroup::RetryInitial,
                (
                    format!("{} s", r.initial_seconds),
                    r.initial_seconds.to_string(),
                ),
            ),
            (
                4,
                GlobalGroup::RetryMaximum,
                (
                    format!("{} s", r.maximum_seconds),
                    r.maximum_seconds.to_string(),
                ),
            ),
            (
                5,
                GlobalGroup::RetryStable,
                (
                    format!("{} s", r.stable_seconds),
                    r.stable_seconds.to_string(),
                ),
            ),
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
                            self.render_global_row(
                                ui,
                                *idx,
                                &mut sel,
                                *group,
                                display.clone(),
                                edit.clone(),
                            );
                        }
                    });
                }
            });
            ui.add_space(8.0);
            ui.label(
                RichText::new(
                    "shared by every connection; click to highlight, double-click to edit",
                )
                .small()
                .color(FG_MUTED),
            );
            ui.add_space(8.0);
            self.render_friday(ui);
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

    fn render_friday(&mut self, ui: &mut egui::Ui) {
        let state = self.friday.state();
        let player = self.friday.player().map(str::to_owned);
        let error = self.friday.error().map(str::to_owned);
        let (status, color) = match state {
            FridayState::Starting => ("starting", FG_WARNING),
            FridayState::Listening => ("listening", FG_SUCCESS),
            FridayState::Stopping => ("stopping", FG_WARNING),
            FridayState::Stopped => ("stopped", FG_MUTED),
            FridayState::Failed => ("failed", FG_ERROR),
        };

        let action = ui
            .group(|ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.heading("Friday voice receiver");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("● {status}"))
                                .strong()
                                .color(color),
                        );
                    });
                });
                ui.label(
                    RichText::new(format!("Local endpoint: http://{LISTEN_ADDR}/speak"))
                        .monospace()
                        .color(FG_PRIMARY),
                );
                ui.label(
                    RichText::new(
                        "Receives MP3 audio from friday.ts and plays it with mpv. The listener only binds to localhost.",
                    )
                    .small()
                    .color(FG_MUTED),
                );
                if let Some(player) = player {
                    ui.label(
                        RichText::new(format!("player: {player}"))
                            .small()
                            .color(FG_MUTED),
                    );
                }
                if let Some(error) = error {
                    ui.label(RichText::new(error).small().color(FG_ERROR));
                }
                ui.add_space(4.0);

                match state {
                    FridayState::Stopped | FridayState::Failed => ui
                        .button("▶  Start listener")
                        .clicked()
                        .then_some(true),
                    FridayState::Listening => ui
                        .button("■  Stop listener")
                        .clicked()
                        .then_some(false),
                    FridayState::Starting => {
                        ui.add_enabled(false, egui::Button::new("Starting…"));
                        None
                    }
                    FridayState::Stopping => {
                        ui.add_enabled(false, egui::Button::new("Stopping…"));
                        None
                    }
                }
            })
            .inner;

        match action {
            Some(true) => self.friday.start(),
            Some(false) => self.friday.stop(),
            None => {}
        }
    }

    fn render_global_row(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        selected: &mut usize,
        group: GlobalGroup,
        display: String,
        edit: String,
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
                    ui.label(
                        RichText::new(&display)
                            .strong()
                            .color(FG_PRIMARY)
                            .monospace(),
                    );
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
}
