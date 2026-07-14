//! Windows system-tray integration.
//!
//! Tray callbacks may run while the native window is hidden. They therefore
//! wake egui explicitly after forwarding a small command to the app thread.

use std::sync::mpsc::{self, Receiver};

use eframe::egui;
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

const MENU_SHOW: &str = "autossh-show";
const MENU_EXIT: &str = "autossh-exit";

#[derive(Clone, Copy, Debug)]
pub enum TrayCommand {
    Show,
    Exit,
}

pub struct WindowsTray {
    // The icon disappears when the final TrayIcon handle is dropped.
    _icon: TrayIcon,
    commands: Receiver<TrayCommand>,
}

impl WindowsTray {
    pub fn new(ctx: &egui::Context) -> anyhow::Result<Self> {
        let show = MenuItem::with_id(MENU_SHOW, "Open rust-autossh", true, None);
        let separator = PredefinedMenuItem::separator();
        let exit = MenuItem::with_id(MENU_EXIT, "Exit", true, None);
        let menu = Menu::with_items(&[&show, &separator, &exit])?;

        let (sender, commands) = mpsc::channel();

        let menu_sender = sender.clone();
        let menu_ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let command = match event.id.as_ref() {
                MENU_SHOW => Some(TrayCommand::Show),
                MENU_EXIT => Some(TrayCommand::Exit),
                _ => None,
            };
            if let Some(command) = command {
                let _ = menu_sender.send(command);
                menu_ctx.request_repaint();
            }
        }));

        let tray_ctx = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            let show_window = matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            );
            if show_window {
                let _ = sender.send(TrayCommand::Show);
                tray_ctx.request_repaint();
            }
        }));

        let icon = TrayIconBuilder::new()
            .with_tooltip("rust-autossh")
            .with_icon(tray_icon()?)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()?;

        Ok(Self {
            _icon: icon,
            commands,
        })
    }

    pub fn try_recv(&self) -> Option<TrayCommand> {
        self.commands.try_recv().ok()
    }
}

/// Draw a compact high-contrast icon without adding an image decoder or an
/// external runtime asset. Windows scales this 32×32 RGBA image for the tray.
fn tray_icon() -> Result<Icon, tray_icon::BadIcon> {
    const SIZE: usize = 32;
    let mut rgba = vec![0; SIZE * SIZE * 4];

    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as i32 - 15;
            let dy = y as i32 - 15;
            if dx * dx + dy * dy <= 14 * 14 {
                set_pixel(&mut rgba, x, y, [24, 132, 166, 255]);
            }
        }
    }

    // Two white opposing arrows suggest bidirectional SSH forwarding.
    for x in 8..24 {
        for y in 10..13 {
            set_pixel(&mut rgba, x, y, [245, 248, 250, 255]);
        }
        for y in 19..22 {
            set_pixel(&mut rgba, x, y, [245, 248, 250, 255]);
        }
    }
    for offset in 0..6 {
        for thickness in 0..3 {
            set_pixel(
                &mut rgba,
                7 + offset,
                11 - offset + thickness,
                [245, 248, 250, 255],
            );
            set_pixel(
                &mut rgba,
                24 - offset,
                20 + offset - thickness,
                [245, 248, 250, 255],
            );
        }
    }

    Icon::from_rgba(rgba, SIZE as u32, SIZE as u32)
}

fn set_pixel(rgba: &mut [u8], x: usize, y: usize, colour: [u8; 4]) {
    let start = (y * 32 + x) * 4;
    rgba[start..start + 4].copy_from_slice(&colour);
}
