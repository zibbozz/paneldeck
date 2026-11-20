//! Tray-Symbol (Windows, Linux; macOS: Symbol in der Menüleiste, #150): Klick = Fenster zeigen, Menü: Anzeigen /
//! Beenden, Tooltip mit Status.

use eframe::egui;
use std::sync::mpsc::Sender;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrayCmd {
    Show,
    Quit,
}

/// Windows: Infobereich der Taskleiste; macOS (#150): `NSStatusItem` in der Menüleiste (tray-icon; wird im Hauptthread
/// angelegt, nachdem die Ereignisschleife läuft – `App::new` erfüllt beides).
#[cfg(any(windows, target_os = "macos"))]
pub struct Tray {
    icon: tray_icon::TrayIcon,
    last_tip: String,
}

#[cfg(any(windows, target_os = "macos"))]
impl Tray {
    pub fn create(ctx: &egui::Context, tx: Sender<TrayCmd>, show_label: &str, quit_label: &str) -> Option<Tray> {
        use tray_icon::menu::{Menu, MenuEvent, MenuItem};
        use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

        let png = include_bytes!("../../assets/icon64.png");
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

/// Linux (#149): StatusNotifierItem über D-Bus (ksni; KDE, Xfce, Cinnamon, MATE, … – GNOME braucht die Erweiterung
/// „AppIndicator and KStatusNotifierItem Support“). Gibt es keinen Tray-Dienst, liefert `create` `None` und Paneldeck
/// läuft ohne Tray-Symbol weiter (Schließen/Minimieren → Tray wirken dann nicht).
#[cfg(all(unix, not(target_os = "macos")))]
pub struct Tray {
    handle: ksni::blocking::Handle<SniTray>,
    last_tip: String,
}

#[cfg(all(unix, not(target_os = "macos")))]
pub struct SniTray {
    ctx: egui::Context,
    tx: Sender<TrayCmd>,
    show: String,
    quit: String,
    tip: String,
    icon: Vec<ksni::Icon>,
}

#[cfg(all(unix, not(target_os = "macos")))]
impl SniTray {
    fn send(&self, c: TrayCmd) {
        let _ = self.tx.send(c);
        self.ctx.request_repaint();
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
impl ksni::Tray for SniTray {
    fn id(&self) -> String {
        "paneldeck".into()
    }
    fn title(&self) -> String {
        "Paneldeck".into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon.clone()
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: self.tip.clone(), ..Default::default() }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(TrayCmd::Show);
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        vec![
            StandardItem { label: self.show.clone(), activate: Box::new(|t: &mut Self| t.send(TrayCmd::Show)), ..Default::default() }.into(),
            ksni::MenuItem::Separator,
            StandardItem { label: self.quit.clone(), activate: Box::new(|t: &mut Self| t.send(TrayCmd::Quit)), ..Default::default() }.into(),
        ]
    }
}

/// RGBA → ARGB32 in Netzwerk-Reihenfolge (Format von StatusNotifierItem).
#[cfg(all(unix, not(target_os = "macos")))]
fn argb(img: &image::RgbaImage) -> Vec<u8> {
    img.pixels().flat_map(|p| [p[3], p[0], p[1], p[2]]).collect()
}

#[cfg(all(unix, not(target_os = "macos")))]
impl Tray {
    pub fn create(ctx: &egui::Context, tx: Sender<TrayCmd>, show_label: &str, quit_label: &str) -> Option<Tray> {
        use ksni::blocking::TrayMethods;
        let png = include_bytes!("../../assets/icon64.png");
        let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.to_rgba8();
        let icon = vec![ksni::Icon { width: img.width() as i32, height: img.height() as i32, data: argb(&img) }];
        let t = SniTray { ctx: ctx.clone(), tx, show: show_label.into(), quit: quit_label.into(), tip: "Paneldeck".into(), icon };
        let handle = t.spawn().ok()?;
        Some(Tray { handle, last_tip: "Paneldeck".into() })
    }

    pub fn set_tooltip(&mut self, tip: &str) {
        if self.last_tip != tip {
            let tip = tip.to_string();
            self.last_tip = tip.clone();
            self.handle.update(move |t| t.tip = tip);
        }
    }
}

