//! Reiter der Sitzungsdialoge (#292): *Allgemein* | *Claude* | *Zugriff* | *Erweitert* – im Dialog „Neue Sitzung“, in
//! den Einstellungen der Sitzung, im Vorlagen-Editor und (ohne *Allgemein*) unter Einstellungen → Claude → Standards.
//!
//! - Alle Reiter haben dieselbe feste Höhe ([`body`]); nur auf sehr kleinen Bildschirmen scrollt der Inhalt (#241:
//!   Erstellen/Abbrechen bleiben sichtbar).
//! - Ein Reiter zeigt die Zahl der Werte, die von den Standards abweichen („Zugriff (2)“), und einen Punkt: orange bei
//!   einer Warnung, rot bei einem ungültigen Wert ([`TabInfo`]).
//! - Tastatur ([`handle_keys`]): Strg+Tab / Strg+Umschalt+Tab und Alt+1 … Alt+4 wechseln den Reiter (der Dialog
//!   verbraucht die Tasten, die globalen Kürzel feuern nicht – [`grab_keys`] holt sie vor den Kürzeln ab). Enter
//!   bestätigt aus jedem Reiter ([`enter_confirms`]); in mehrzeiligen Feldern ([`multiline`]) gibt Enter einen
//!   Zeilenumbruch, dort bestätigt Strg+Enter.
//! - Eine einklappbare Vorschau der Kommandozeile mit Kopierknopf ([`preview`]).
//!
//! Kennt keine App; die Inhalte der Reiter zeichnen die Dialoge selbst.

use crate::state::Lang;
use eframe::egui::{self, Color32, Id, Key, Modifiers, RichText, Ui};

/// Ein Reiter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Tab {
    General,
    Claude,
    Access,
    Advanced,
}

impl Tab {
    pub(super) fn label(self, lang: Lang) -> &'static str {
        match self {
            Tab::General => lang.t("Allgemein", "General"),
            Tab::Claude => "Claude",
            Tab::Access => lang.t("Zugriff", "Access"),
            Tab::Advanced => lang.t("Erweitert", "Advanced"),
        }
    }
}

/// Reiter der Sitzungsdialoge: ohne Claude nur *Allgemein*.
pub(super) fn session_tabs(claude: bool) -> Vec<Tab> {
    if claude { vec![Tab::General, Tab::Claude, Tab::Access, Tab::Advanced] } else { vec![Tab::General] }
}

/// Reiter der Standards (Einstellungen → Claude).
pub(super) const DEFAULTS_TABS: [Tab; 3] = [Tab::Claude, Tab::Access, Tab::Advanced];

/// Markierung eines Reiters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(super) enum Mark {
    #[default]
    None,
    /// Warnung (orange).
    Warn,
    /// Ungültiger Wert (rot).
    Error,
}

/// Beschriftung eines Reiters: Zahl der abweichenden Werte und Markierung.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TabInfo {
    pub tab: Tab,
    pub count: usize,
    pub mark: Mark,
}

impl TabInfo {
    pub(super) fn plain(tab: Tab) -> TabInfo {
        TabInfo { tab, count: 0, mark: Mark::None }
    }

    /// Text des Reiters, z. B. „Zugriff (2)“.
    pub(super) fn text(&self, lang: Lang) -> String {
        if self.count > 0 { format!("{} ({})", self.tab.label(lang), self.count) } else { self.tab.label(lang).to_string() }
    }
}

const ORANGE: Color32 = Color32::from_rgb(0xe5, 0xa0, 0x4b);
const RED: Color32 = Color32::from_rgb(0xe0, 0x6c, 0x75);

fn tab_id(salt: &str) -> Id {
    Id::new(("opts_tab", salt.to_string()))
}

/// Gewählter Reiter (Standard: der erste der Liste).
pub(super) fn current(ctx: &egui::Context, salt: &str, tabs: &[Tab]) -> Tab {
    let t = ctx.data(|d| d.get_temp::<Tab>(tab_id(salt)));
    match t {
        Some(t) if tabs.contains(&t) => t,
        _ => tabs.first().copied().unwrap_or(Tab::General),
    }
}

pub(super) fn set(ctx: &egui::Context, salt: &str, tab: Tab) {
    ctx.data_mut(|d| d.insert_temp(tab_id(salt), tab));
}

/// Tastenwunsch, den [`grab_keys`] vor den globalen Kürzeln abgeholt hat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TabKey {
    Next,
    Prev,
    Nth(usize),
}

fn stash_id() -> Id {
    Id::new("opts_tab_key")
}

fn take_keys(i: &mut egui::InputState) -> Option<TabKey> {
    // Spezifischere zuerst: Strg+Umschalt+Tab würde sonst auch zu Strg+Tab passen.
    if i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::Tab) {
        return Some(TabKey::Prev);
    }
    if i.consume_key(Modifiers::CTRL, Key::Tab) {
        return Some(TabKey::Next);
    }
    for (n, k) in [Key::Num1, Key::Num2, Key::Num3, Key::Num4].into_iter().enumerate() {
        if i.consume_key(Modifiers::ALT, k) {
            return Some(TabKey::Nth(n));
        }
    }
    None
}

/// Vor den globalen Kürzeln aufrufen, solange ein Dialog mit Reitern offen ist: verbraucht die Tasten zum Wechseln der
/// Reiter und merkt sie für [`handle_keys`] (sonst wechselte z. B. Alt+1 die Sitzung).
pub(super) fn grab_keys(ctx: &egui::Context) {
    if let Some(k) = ctx.input_mut(take_keys) {
        ctx.data_mut(|d| d.insert_temp(stash_id(), Some(k)));
    }
}

/// Tasten zum Wechseln der Reiter auswerten (am Anfang des Dialogs, vor dem Zeichnen). Rückgabe: Reiter gewechselt.
pub(super) fn handle_keys(ctx: &egui::Context, salt: &str, tabs: &[Tab]) -> bool {
    let stashed = ctx.data_mut(|d| d.remove_temp::<Option<TabKey>>(stash_id())).flatten();
    let Some(k) = stashed.or_else(|| ctx.input_mut(take_keys)) else { return false };
    if tabs.is_empty() {
        return false;
    }
    let cur = tabs.iter().position(|t| *t == current(ctx, salt, tabs)).unwrap_or(0);
    let next = match k {
        TabKey::Next => (cur + 1) % tabs.len(),
        TabKey::Prev => (cur + tabs.len() - 1) % tabs.len(),
        TabKey::Nth(n) if n < tabs.len() => n,
        TabKey::Nth(_) => return false,
    };
    set(ctx, salt, tabs[next]);
    true
}

/// Leiste der Reiter. Rückgabe: der gewählte Reiter.
pub(super) fn strip(ui: &mut Ui, lang: Lang, salt: &str, infos: &[TabInfo]) -> Tab {
    let tabs: Vec<Tab> = infos.iter().map(|i| i.tab).collect();
    let mut cur = current(ui.ctx(), salt, &tabs);
    ui.horizontal(|ui| {
        for (n, info) in infos.iter().enumerate() {
            let mut text = RichText::new(info.text(lang));
            if cur == info.tab {
                text = text.strong();
            }
            let r = ui.add(egui::Button::selectable(cur == info.tab, text)).on_hover_text(format!("Alt+{}", n + 1));
            if info.mark != Mark::None {
                // Punkt oben rechts am Reiter.
                let c = if info.mark == Mark::Error { RED } else { ORANGE };
                ui.painter().circle_filled(r.rect.right_top() + egui::vec2(-3.0, 3.0), 3.0, c);
            }
            #[cfg(test)]
            ui.data_mut(|d| d.insert_temp(Id::new(("tab_mark", salt.to_string(), n)), (r.rect, info.text(lang), info.mark)));
            if r.clicked() {
                cur = info.tab;
                set(ui.ctx(), salt, cur);
            }
        }
    });
    ui.separator();
    cur
}

/// Feste Höhe der Reiter-Inhalte (Punkte), solange der Bildschirm reicht.
pub(super) const BODY_H: f32 = 470.0;
/// Breite der Dialoge mit Reitern.
pub(super) const DIALOG_W: f32 = 640.0;

/// Breite eines Dialogs: [`DIALOG_W`], nie breiter als der Bildschirm abzüglich Rand.
pub(super) fn dialog_width(ctx: &egui::Context) -> f32 {
    DIALOG_W.min(ctx.content_rect().width() - 48.0).max(280.0)
}

/// Inhalt eines Reiters mit fester Höhe `min(BODY_H, max_h)`; scrollt nur, wenn der Platz nicht reicht. Für die Tests
/// wird gemerkt, ob gescrollt werden müsste.
pub(super) fn body<R>(ui: &mut Ui, salt: &str, tab: Tab, max_h: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let h = BODY_H.min(max_h).max(80.0);
    let out = egui::ScrollArea::vertical().id_salt(("opts_tab_body", salt, tab)).max_height(h).min_scrolled_height(h).auto_shrink([false, false]).show(ui, |ui| {
        ui.set_min_height(h);
        add(ui)
    });
    let overflow = out.content_size.y > out.inner_rect.height() + 1.0;
    ui.data_mut(|d| d.insert_temp(Id::new(("opts_tab_overflow", salt.to_string())), overflow));
    out.inner
}

/// Musste der zuletzt gezeichnete Reiter scrollen?
#[cfg(test)]
pub(super) fn overflowed(ctx: &egui::Context, salt: &str) -> bool {
    ctx.data(|d| d.get_temp::<bool>(Id::new(("opts_tab_overflow", salt.to_string())))).unwrap_or(false)
}

/// Knopf „Auf Standards zurücksetzen“ eines Reiters.
pub(super) fn reset_button(ui: &mut Ui, lang: Lang, enabled: bool) -> bool {
    ui.add_enabled(enabled, egui::Button::new(lang.t("Auf Standards zurücksetzen", "Reset to defaults")).small())
        .on_hover_text(lang.t("Setzt die Werte dieses Reiters auf deine Claude-Standards zurück (Einstellungen → Claude).", "Resets this tab's values to your Claude defaults (Settings → Claude)."))
        .clicked()
}

/// Einklappbare Vorschau der Kommandozeile mit Kopierknopf. `line` wird nur bei offener Vorschau berechnet (prüft
/// Pfade im Dateisystem).
pub(super) fn preview(ui: &mut Ui, lang: Lang, salt: &str, line: impl FnOnce() -> String) {
    let id = Id::new(("opts_preview", salt.to_string()));
    let mut open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let arrow = if open { "▾" } else { "▸" };
    let r = ui.add(egui::Button::new(RichText::new(format!("{arrow} {}", lang.t("Kommandozeile", "Command line"))).small()).frame(false));
    if r.clicked() {
        open = !open;
    }
    ui.data_mut(|d| d.insert_temp(id, open));
    if open {
        let text = line();
        ui.horizontal(|ui| {
            ui.add(egui::Label::new(RichText::new(&text).monospace().small()).wrap());
            if ui.small_button("⧉").on_hover_text(lang.t("Kopieren", "Copy")).clicked() {
                ui.ctx().copy_text(text.clone());
            }
        });
        ui.label(RichText::new(lang.t("Ohne --session-id/--resume und --settings, die Paneldeck beim Start ergänzt.", "Without --session-id/--resume and --settings, which Paneldeck adds at start.")).small().color(crate::style::dim(ui)));
    }
}

fn multiline_id() -> Id {
    Id::new("opts_multiline_ids")
}

/// Mehrzeiliges Feld anmelden: dort gibt Enter einen Zeilenumbruch und bestätigt den Dialog nicht.
pub(super) fn multiline(ui: &Ui, r: &egui::Response) {
    ui.data_mut(|d| d.get_temp_mut_or_default::<Vec<Id>>(multiline_id()).push(r.id));
}

/// Zu Beginn des Dialogs aufrufen: Enter (ohne Modifikator, kein Popup offen, Fokus nicht in einem mehrzeiligen Feld)
/// oder Strg+Enter bestätigt. Liest die mehrzeiligen Felder des letzten Bilds und setzt die Liste zurück.
pub(super) fn enter_confirms(ctx: &egui::Context) -> bool {
    let ml = ctx.data_mut(|d| d.remove_temp::<Vec<Id>>(multiline_id())).unwrap_or_default();
    if egui::Popup::is_any_open(ctx) {
        return false;
    }
    let focused = ctx.memory(|m| m.focused());
    let in_ml = focused.is_some_and(|f| ml.contains(&f));
    ctx.input_mut(|i| {
        if i.consume_key(Modifiers::CTRL, Key::Enter) {
            return true;
        }
        !in_ml && i.modifiers.is_none() && i.key_pressed(Key::Enter) && i.consume_key(Modifiers::NONE, Key::Enter)
    })
}

/// Notizen: eine Zeile, die beim Tippen bis auf 4 Zeilen wächst.
pub(super) fn notes_edit(ui: &mut Ui, text: &mut String, width: f32) -> egui::Response {
    let lines = text.lines().count().max(1) + usize::from(text.ends_with('\n'));
    let r = ui.add(egui::TextEdit::multiline(text).desired_rows(lines.clamp(1, 4)).desired_width(width));
    multiline(ui, &r);
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, RawInput};

    fn key(k: Key, m: Modifiers) -> Event {
        Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m }
    }

    fn run(ctx: &egui::Context, events: Vec<Event>, f: impl FnMut(&egui::Context)) {
        let _ = ctx.run(RawInput { events, ..Default::default() }, f);
    }

    #[test]
    fn keys_switch_tabs_and_are_consumed() {
        let ctx = egui::Context::default();
        let tabs = session_tabs(true);
        let step = |events: Vec<Event>| {
            let mut left = 0;
            run(&ctx, events, |ctx| {
                handle_keys(ctx, "t", &tabs);
                left = ctx.input(|i| i.events.len());
            });
            (current(&ctx, "t", &tabs), left)
        };
        assert_eq!(step(vec![]).0, Tab::General);
        assert_eq!(step(vec![key(Key::Tab, Modifiers::CTRL)]), (Tab::Claude, 0), "gewechselt und verbraucht");
        assert_eq!(step(vec![key(Key::Tab, Modifiers::CTRL | Modifiers::SHIFT)]).0, Tab::General);
        assert_eq!(step(vec![key(Key::Tab, Modifiers::CTRL | Modifiers::SHIFT)]).0, Tab::Advanced, "rundum");
        assert_eq!(step(vec![key(Key::Num3, Modifiers::ALT)]).0, Tab::Access);
        assert_eq!(step(vec![key(Key::Num1, Modifiers::ALT)]).0, Tab::General);
        // Ohne Claude gibt es nur „Allgemein“.
        assert_eq!(current(&ctx, "t", &session_tabs(false)), Tab::General);
        // Vorab abgeholt (vor den globalen Kürzeln): wirkt im Dialog.
        run(&ctx, vec![key(Key::Num4, Modifiers::ALT)], |ctx| {
            grab_keys(ctx);
            assert_eq!(ctx.input(|i| i.events.len()), 0);
            handle_keys(ctx, "t", &session_tabs(true));
        });
        assert_eq!(current(&ctx, "t", &session_tabs(true)), Tab::Advanced);
        for t in [Tab::General, Tab::Access, Tab::Advanced] {
            assert_ne!(t.label(Lang::De), t.label(Lang::En));
        }
        assert_eq!(TabInfo { tab: Tab::Access, count: 2, mark: Mark::Warn }.text(Lang::En), "Access (2)");
    }

    #[test]
    fn enter_confirms_except_in_multiline_fields() {
        let ctx = egui::Context::default();
        let mut text = String::from("a");
        let mut ok = false;
        let mut frame = |events: Vec<Event>, focus: bool| {
            run(&ctx, events, |ctx| {
                ok = enter_confirms(ctx);
                egui::CentralPanel::default().show(ctx, |ui| {
                    let r = notes_edit(ui, &mut text, 200.0);
                    if focus {
                        r.request_focus();
                    }
                });
            });
            ok
        };
        frame(vec![], false);
        assert!(frame(vec![key(Key::Enter, Modifiers::NONE)], false), "Enter bestätigt");
        frame(vec![], true);
        frame(vec![], false);
        assert!(!frame(vec![key(Key::Enter, Modifiers::NONE)], false), "im mehrzeiligen Feld: Zeilenumbruch");
        assert!(frame(vec![key(Key::Enter, Modifiers::CTRL)], false), "Strg+Enter bestätigt immer");
    }
}
