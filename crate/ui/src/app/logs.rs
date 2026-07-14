//! Bottom tail-style log stream with severity badges.

use eframe::egui::{self, RichText};

use crate::log::{FG_DIM, FG_MUTED, LOG_BUFFER_LIMIT, format_unix_ts};

use super::AutosshApp;

impl AutosshApp {
    pub fn render_logs_panel(&mut self, root: &mut egui::Ui) {
        egui::Panel::bottom("logs")
            .resizable(true)
            .default_size(220.0)
            .size_range(120.0..=460.0)
            .show_inside(root, |ui| {
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
                                    .corner_radius(egui::CornerRadius::same(3))
                                    .inner_margin(egui::Margin::same(0))
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
