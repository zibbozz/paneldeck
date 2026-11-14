//! Tray-Symbol (Windows): Klick = Fenster zeigen, Menü: Anzeigen / Beenden, Tooltip mit Status.

use eframe::egui;
use std::sync::mpsc::Sender;

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub enum TrayCmd {
    Show,
    Quit,
}

#[cfg(windows)]
pub struct Tray {
    icon: tray_icon::TrayIcon,
    last_tip: String,
}

#[cfg(windows)]
impl Tray {
    pub fn create(ctx: &egui::Context, tx: Sender<TrayCmd>, show_label: &str, quit_label: &str) -> Option<Tray> {
        use tray_icon::menu::{Menu, MenuEvent, MenuItem};
        use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

        let png = include_bytes!("../assets/icon64.png");
        let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.to_rgba8();
        let icon = tray_icon::Icon::from_rgba(img.to_vec(), img.width(), img.height()).ok()?;

        let menu = Menu::new();
        let show = MenuItem::new(show_label, true, None);
        let quit = MenuItem::new(quit_label, true, None);
        menu.append(&show).ok()?;
        menu.append(&quit).ok()?;
        let (show_id, quit_id) = (show.id().clone(), quit.id().clone());

        let (c1, t1) = (ctx.clone(), tx.clone());
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                let _ = t1.send(TrayCmd::Show);
                c1.request_repaint();
            }
        }));
        let c2 = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if e.id == show_id {
                let _ = tx.send(TrayCmd::Show);
            } else if e.id == quit_id {
                let _ = tx.send(TrayCmd::Quit);
            }
            c2.request_repaint();
        }));

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .with_tooltip("Paneldeck")
            .with_icon(icon)
            .build()
            .ok()?;
        Some(Tray { icon, last_tip: "Paneldeck".into() })
    }

    pub fn set_tooltip(&mut self, tip: &str) {
        if self.last_tip != tip {
            let _ = self.icon.set_tooltip(Some(tip));
            self.last_tip = tip.to_string();
        }
    }
}

#[cfg(not(windows))]
pub struct Tray;

#[cfg(not(windows))]
impl Tray {
    pub fn create(_ctx: &egui::Context, _tx: Sender<TrayCmd>, _show: &str, _quit: &str) -> Option<Tray> {
        None
    }
    pub fn set_tooltip(&mut self, _tip: &str) {}
}
