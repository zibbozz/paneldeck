//! Reiter der Sitzungsdialoge (#292): *Allgemein* | *Claude* | *Zugriff* | *Erweitert* | *Umgebung* (#384) – im Dialog „Neue Sitzung“, in
//! den Einstellungen der Sitzung, im Vorlagen-Editor und (ohne *Allgemein*) unter Einstellungen → Claude → Neue Sitzungen.
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
    /// Umgebungsvariablen der Sitzung (#384).
    Environment,
}

impl Tab {
    pub(super) fn label(self, lang: Lang) -> &'static str {
        match self {
            Tab::General => lang.t("Allgemein", "General"),
            Tab::Claude => "Claude",
            Tab::Access => lang.t("Zugriff", "Access"),
            Tab::Advanced => lang.t("Erweitert", "Advanced"),
            Tab::Environment => lang.t("Umgebung", "Environment"),
        }
    }
}

/// Reiter der Sitzungsdialoge: ohne Claude nur *Allgemein* und *Umgebung*.
pub(super) fn session_tabs(claude: bool) -> Vec<Tab> {
    session_tabs_env(claude, true)
}

/// Wie [`session_tabs`]; `env` = mit *Umgebung* (nicht bei Chats und Gruppen-Vorlagen).
pub(super) fn session_tabs_env(claude: bool, env: bool) -> Vec<Tab> {
    let mut v = vec![Tab::General];
    if claude {
        v.extend([Tab::Claude, Tab::Access, Tab::Advanced]);
    }
    if env {
        v.push(Tab::Environment);
    }
    v
}

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
#[allow(non_snake_case)]
fn RED() -> Color32 {
    crate::style::ui().error
}

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
    /// Alt+Ziffer (0-basiert); gilt erst, wenn Alt losgelassen ist und nur eine Ziffer kam (Alt+Ziffernblock-Codes wie
    /// Alt+0228 wechseln so keinen Reiter).
    Digit(usize),
}

fn stash_id(salt: &str) -> Id {
    Id::new(("opts_tab_key", salt.to_string()))
}

fn alt_seq_id(salt: &str) -> Id {
    Id::new(("opts_tab_alt", salt.to_string()))
}

fn active_id(salt: &str) -> Id {
    Id::new(("opts_tab_active", salt.to_string()))
}

/// Ebene eines Dialogfensters mit der Id `window` (wie sie `egui::Window` bzw. `style::movable` vergibt).
pub(super) fn window_layer(window: Id) -> egui::LayerId {
    egui::LayerId::new(egui::Order::Middle, window)
}

/// Der Dialog bekommt die Tastatur (beim Öffnen; danach, bis woanders hingeklickt oder der Fokus woandershin geht).
pub(super) fn activate(ctx: &egui::Context, salt: &str) {
    ctx.data_mut(|d| d.insert_temp(active_id(salt), true));
}

/// Hat der Dialog (Fenster `window`) gerade die Tastatur? Ein fokussiertes Element entscheidet nach seiner Ebene (das
/// Fenster selbst, ein Popup darüber – sonst nicht); ohne Fokus zählt der letzte Mausklick (ins Fenster oder in ein
/// Popup: ja, in ein Terminal oder ein anderes Fenster: nein). So nimmt ein nicht modaler Dialog (Einstellungen der
/// Sitzung) keine Tasten, die ins Terminal getippt werden.
pub(super) fn active(ctx: &egui::Context, salt: &str, window: Id) -> bool {
    let layer = window_layer(window);
    let ours = |l: egui::LayerId| l == layer || matches!(l.order, egui::Order::Foreground | egui::Order::Tooltip | egui::Order::Debug);
    if let Some(pos) = ctx.input(|i| if i.pointer.any_pressed() { i.pointer.interact_pos() } else { None }) {
        let inside = ctx.layer_id_at(pos).is_some_and(ours);
        ctx.data_mut(|d| d.insert_temp(active_id(salt), inside));
    }
    // Ein Fokuswechsel (z. B. mit Tab in den Dialog oder aus ihm heraus) entscheidet neu; ein Klick daneben bleibt
    // gültig, auch wenn der Fokus noch auf einem Knopf des Dialogs liegt.
    let focused = ctx.memory(|m| m.focused());
    let last_id = Id::new(("opts_tab_focus", salt.to_string()));
    if ctx.data(|d| d.get_temp::<Option<Id>>(last_id)) != Some(focused) {
        ctx.data_mut(|d| d.insert_temp(last_id, focused));
        if let Some(r) = focused.and_then(|f| ctx.read_response(f)) {
            let inside = ours(r.layer_id);
            ctx.data_mut(|d| d.insert_temp(active_id(salt), inside));
        }
    }
    ctx.data(|d| d.get_temp::<bool>(active_id(salt))).unwrap_or(false)
}

fn take_keys(i: &mut egui::InputState) -> Vec<TabKey> {
    let mut out = Vec::new();
    // Spezifischere zuerst: Strg+Umschalt+Tab würde sonst auch zu Strg+Tab passen.
    while i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::Tab) {
        out.push(TabKey::Prev);
    }
    while i.consume_key(Modifiers::CTRL, Key::Tab) {
        out.push(TabKey::Next);
    }
    for (n, k) in [Key::Num0, Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9].into_iter().enumerate() {
        // Alle Ziffern mit Alt nehmen, damit Alt+Ziffernblock-Codes (mehrere Ziffern) erkannt werden.
        while i.consume_key(Modifiers::ALT, k) {
            out.push(TabKey::Digit((n + 9) % 10));
        }
    }
    out
}

/// Vor den globalen Kürzeln aufrufen, solange der Dialog `salt` (Fenster `window`) offen ist: hat er die Tastatur
/// ([`active`]), verbraucht er die Tasten zum Wechseln der Reiter und merkt sie für [`handle_keys`] (sonst wechselte z. B.
/// Alt+1 die Sitzung). Sonst bleiben sie dem Terminal und den Kürzeln.
pub(super) fn grab_keys(ctx: &egui::Context, salt: &str, window: Id) {
    if !active(ctx, salt, window) {
        return;
    }
    let keys = ctx.input_mut(take_keys);
    if !keys.is_empty() {
        ctx.data_mut(|d| d.get_temp_mut_or_default::<Vec<TabKey>>(stash_id(salt)).extend(keys));
    }
}

/// Tasten zum Wechseln der Reiter auswerten (am Anfang des Dialogs, vor dem Zeichnen; nur mit der Tastatur, siehe
/// [`active`]). Rückgabe: Reiter gewechselt.
pub(super) fn handle_keys(ctx: &egui::Context, salt: &str, window: Id, tabs: &[Tab]) -> bool {
    let mut keys = ctx.data_mut(|d| d.remove_temp::<Vec<TabKey>>(stash_id(salt))).unwrap_or_default();
    if active(ctx, salt, window) {
        keys.extend(ctx.input_mut(take_keys));
    }
    if tabs.is_empty() {
        return false;
    }
    let before = current(ctx, salt, tabs);
    let mut seq: Vec<usize> = ctx.data(|d| d.get_temp(alt_seq_id(salt))).unwrap_or_default();
    for k in keys {
        let cur = tabs.iter().position(|t| *t == current(ctx, salt, tabs)).unwrap_or(0);
        match k {
            TabKey::Next => set(ctx, salt, tabs[(cur + 1) % tabs.len()]),
            TabKey::Prev => set(ctx, salt, tabs[(cur + tabs.len() - 1) % tabs.len()]),
            TabKey::Digit(n) => seq.push(n),
        }
    }
    if !seq.is_empty() && !ctx.input(|i| i.modifiers.alt) {
        if let [n] = seq[..] {
            if n < tabs.len() {
                set(ctx, salt, tabs[n]);
            }
        }
        seq.clear();
    }
    ctx.data_mut(|d| d.insert_temp(alt_seq_id(salt), seq));
    current(ctx, salt, tabs) != before
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
                let c = if info.mark == Mark::Error { RED() } else { ORANGE };
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
        .on_hover_text(lang.t("Setzt die Werte dieses Reiters auf deine Claude-Standards zurück (Einstellungen → Claude → Neue Sitzungen).", "Resets this tab's values to your Claude defaults (Settings → Claude → New sessions)."))
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
        ui.label(RichText::new(lang.t("Ohne --resume/--continue und --settings, die Paneldeck beim Start ergänzt.", "Without --resume/--continue and --settings, which Paneldeck adds at start.")).small().color(crate::style::dim(ui)));
    }
}

fn multiline_id(salt: &str) -> Id {
    Id::new(("opts_multiline_ids", salt.to_string()))
}

/// Mehrzeiliges Feld des Dialogs `salt` anmelden: dort gibt Enter einen Zeilenumbruch und bestätigt den Dialog nicht.
pub(super) fn multiline(ui: &Ui, salt: &str, r: &egui::Response) {
    ui.data_mut(|d| d.get_temp_mut_or_default::<Vec<Id>>(multiline_id(salt)).push(r.id));
}

/// Zu Beginn des Dialogs aufrufen (nur mit der Tastatur, [`active`]): Strg+Enter bestätigt immer; Enter (ohne
/// Modifikator) nur, wenn kein Element oder ein einzeiliges Textfeld den Fokus hat – auf einem Knopf, Häkchen, einer
/// Auswahl oder einem Listeneintrag löst Enter dieses Element aus, in einem mehrzeiligen Feld gibt es einen
/// Zeilenumbruch. Bei offenem Popup bestätigt Enter nur aus einem Textfeld (z. B. Tag-Vorschläge ohne Auswahl). Liest
/// die mehrzeiligen Felder des letzten Bilds und setzt die Liste zurück.
pub(super) fn enter_confirms(ctx: &egui::Context, salt: &str, window: Id) -> bool {
    let ml = ctx.data_mut(|d| d.remove_temp::<Vec<Id>>(multiline_id(salt))).unwrap_or_default();
    if !active(ctx, salt, window) {
        return false;
    }
    let focused = ctx.memory(|m| m.focused());
    let text_field = focused.is_some_and(|f| egui::TextEdit::load_state(ctx, f).is_some() && !ml.contains(&f));
    let plain_ok = match focused {
        None => !egui::Popup::is_any_open(ctx),
        Some(_) => text_field,
    };
    ctx.input_mut(|i| {
        if i.consume_key(Modifiers::CTRL, Key::Enter) {
            return true;
        }
        plain_ok && i.modifiers.is_none() && i.key_pressed(Key::Enter) && i.consume_key(Modifiers::NONE, Key::Enter)
    })
}

/// Hinweis neben gesperrtem „Erstellen“/„Speichern“: in welchen Reitern ein Wert ungültig ist (`None`: keiner).
pub(super) fn invalid_hint(lang: Lang, infos: &[TabInfo]) -> Option<String> {
    let bad: Vec<String> = infos.iter().filter(|i| i.mark == Mark::Error).map(|i| lang.q(i.tab.label(lang))).collect();
    if bad.is_empty() {
        return None;
    }
    Some(if lang == Lang::De { format!("Ungültiger Wert im Reiter {}", bad.join(", ")) } else { format!("Invalid value in tab {}", bad.join(", ")) })
}

/// Hinweis zeigen (rot, klein) und als Tooltip des gesperrten Knopfs.
pub(super) fn show_invalid(ui: &mut Ui, hint: &Option<String>, button: &egui::Response) {
    if let Some(h) = hint {
        ui.label(RichText::new(h).small().color(RED()));
        button.clone().on_disabled_hover_text(h.as_str());
    }
}

/// Notizen: eine Zeile, die beim Tippen bis auf 4 Zeilen wächst.
pub(super) fn notes_edit(ui: &mut Ui, salt: &str, text: &mut String, width: f32) -> egui::Response {
    let lines = text.lines().count().max(1) + usize::from(text.ends_with('\n'));
    let r = ui.add(egui::TextEdit::multiline(text).desired_rows(lines.clamp(1, 4)).desired_width(width));
    multiline(ui, salt, &r);
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, Pos2, RawInput, Rect};

    fn key(k: Key, m: Modifiers) -> Event {
        Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m }
    }

    fn run(ctx: &egui::Context, events: Vec<Event>, mods: Modifiers, f: impl FnMut(&egui::Context)) {
        let _ = ctx.run(RawInput { events, modifiers: mods, screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))), ..Default::default() }, f);
    }

    const WIN: &str = "t_win";

    /// Ein Fenster mit einem Textfeld und einem Knopf; gibt die Rechtecke zurück.
    fn window(ctx: &egui::Context, text: &mut String, multi: bool) -> Option<(Rect, Id)> {
        let mut out = None;
        egui::Window::new("t").id(Id::new(WIN)).fixed_pos(Pos2::new(100.0, 100.0)).show(ctx, |ui| {
            let t = if multi { notes_edit(ui, "t", text, 200.0) } else { ui.text_edit_singleline(text) };
            let b = ui.button("b");
            out = Some((t.rect, b.id));
        });
        out
    }

    #[test]
    fn keys_switch_tabs_only_with_focus_and_are_consumed() {
        let ctx = egui::Context::default();
        let tabs = session_tabs(true);
        activate(&ctx, "t");
        let step = |events: Vec<Event>, mods: Modifiers| {
            let mut left = 0;
            run(&ctx, events, mods, |ctx| {
                handle_keys(ctx, "t", Id::new(WIN), &tabs);
                left = ctx.input(|i| i.events.len());
            });
            (current(&ctx, "t", &tabs), left)
        };
        assert_eq!(step(vec![], Modifiers::NONE).0, Tab::General);
        assert_eq!(step(vec![key(Key::Tab, Modifiers::CTRL)], Modifiers::NONE), (Tab::Claude, 0), "gewechselt und verbraucht");
        assert_eq!(step(vec![key(Key::Tab, Modifiers::CTRL | Modifiers::SHIFT)], Modifiers::NONE).0, Tab::General);
        assert_eq!(step(vec![key(Key::Tab, Modifiers::CTRL | Modifiers::SHIFT)], Modifiers::NONE).0, Tab::Environment, "rundum");
        assert_eq!(step(vec![key(Key::Num3, Modifiers::ALT)], Modifiers::NONE).0, Tab::Access);
        // Alt gehalten: erst beim Loslassen; Alt+Ziffernblock-Code (mehrere Ziffern) wechselt nicht.
        assert_eq!(step(vec![key(Key::Num1, Modifiers::ALT)], Modifiers::ALT).0, Tab::Access);
        assert_eq!(step(vec![], Modifiers::NONE).0, Tab::General);
        assert_eq!(step(vec![key(Key::Num0, Modifiers::ALT)], Modifiers::ALT).0, Tab::General);
        assert_eq!(step(vec![key(Key::Num2, Modifiers::ALT)], Modifiers::ALT).0, Tab::General);
        assert_eq!(step(vec![key(Key::Num8, Modifiers::ALT)], Modifiers::ALT).0, Tab::General);
        assert_eq!(step(vec![], Modifiers::NONE).0, Tab::General, "Alt+0228 ist ein Zeichen, kein Reiterwechsel");
        // Ohne Claude gibt es nur „Allgemein“ und „Umgebung“ (#384), bei Chats nur „Allgemein“.
        assert_eq!(current(&ctx, "t", &session_tabs(false)), Tab::General);
        assert_eq!(session_tabs(false), vec![Tab::General, Tab::Environment]);
        assert_eq!(session_tabs_env(false, false), vec![Tab::General]);
        assert_eq!(*session_tabs(true).last().unwrap(), Tab::Environment);
        // Vorab abgeholt (vor den globalen Kürzeln): wirkt im Dialog.
        run(&ctx, vec![key(Key::Num4, Modifiers::ALT)], Modifiers::NONE, |ctx| {
            grab_keys(ctx, "t", Id::new(WIN));
            assert_eq!(ctx.input(|i| i.events.len()), 0);
            handle_keys(ctx, "t", Id::new(WIN), &session_tabs(true));
        });
        assert_eq!(current(&ctx, "t", &session_tabs(true)), Tab::Advanced);
        // Ohne Tastatur (z. B. ins Terminal geklickt): Tasten bleiben liegen, nichts wechselt.
        ctx.data_mut(|d| d.insert_temp(active_id("t"), false));
        assert_eq!(step(vec![key(Key::Tab, Modifiers::CTRL)], Modifiers::NONE), (Tab::Advanced, 1));
        run(&ctx, vec![key(Key::Num1, Modifiers::ALT)], Modifiers::NONE, |ctx| {
            grab_keys(ctx, "t", Id::new(WIN));
            assert_eq!(ctx.input(|i| i.events.len()), 1, "nicht verbraucht");
        });
        for t in [Tab::General, Tab::Access, Tab::Advanced, Tab::Environment] {
            assert_ne!(t.label(Lang::De), t.label(Lang::En));
        }
        assert_eq!(TabInfo { tab: Tab::Access, count: 2, mark: Mark::Warn }.text(Lang::En), "Access (2)");
        let infos = [TabInfo::plain(Tab::General), TabInfo { tab: Tab::Access, count: 1, mark: Mark::Error }];
        assert_eq!(invalid_hint(Lang::En, &infos).as_deref(), Some("Invalid value in tab “Access”"));
        assert!(invalid_hint(Lang::De, &infos).unwrap().contains("Zugriff"));
        assert_eq!(invalid_hint(Lang::En, &infos[..1]), None);
    }

    #[test]
    fn enter_confirms_only_from_text_fields_of_the_focused_dialog() {
        for multi in [false, true] {
            let ctx = egui::Context::default();
            let mut text = String::from("a");
            let rects = std::cell::Cell::new(None);
            let mut frame = |events: Vec<Event>, mods: Modifiers| {
                let mut ok = false;
                run(&ctx, events, mods, |ctx| {
                    ok = enter_confirms(ctx, "t", Id::new(WIN));
                    rects.set(window(ctx, &mut text, multi));
                });
                ok
            };
            frame(vec![], Modifiers::NONE);
            frame(vec![], Modifiers::NONE);
            let (t, b) = rects.get().unwrap();
            let click = |p: Pos2| vec![Event::PointerMoved(p), Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }, Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }];
            // Ohne Klick in den Dialog: nichts.
            assert!(!frame(vec![key(Key::Enter, Modifiers::NONE)], Modifiers::NONE), "Dialog ohne Tastatur");
            // Ins Textfeld geklickt.
            frame(click(t.center()), Modifiers::NONE);
            frame(vec![], Modifiers::NONE);
            assert_eq!(frame(vec![key(Key::Enter, Modifiers::NONE)], Modifiers::NONE), !multi, "einzeilig bestätigt, mehrzeilig Zeilenumbruch");
            assert!(frame(vec![key(Key::Enter, Modifiers::CTRL)], Modifiers::CTRL), "Strg+Enter bestätigt immer");
            // Knopf fokussiert (Tab): Enter löst den Knopf aus, nicht den Dialog.
            ctx.memory_mut(|m| m.request_focus(b));
            frame(vec![], Modifiers::NONE);
            assert!(!frame(vec![key(Key::Enter, Modifiers::NONE)], Modifiers::NONE), "Fokus auf einem Knopf");
            // Klick daneben (Hintergrund, z. B. ein Terminal): der Dialog hat die Tastatur nicht mehr.
            frame(click(Pos2::new(700.0, 550.0)), Modifiers::NONE);
            frame(vec![], Modifiers::NONE);
            assert!(!frame(vec![key(Key::Enter, Modifiers::CTRL)], Modifiers::CTRL), "nicht im Dialog");
        }
    }
}
