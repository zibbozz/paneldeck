//! Leerer Terminal Hub: Paneldeck startet ohne offenes Terminal (#111), ein neues Fenster ebenso (#110).
//! Mittig ein kurzer Hinweis mit den üblichen Wegen, eine Sitzung zu starten (dieselben Aktionen wie Kürzel und Palette).

use super::*;

/// Kürzel, die im leeren Hub als Hinweis stehen (schon in der Sprache des Nutzers formatiert).
pub(super) struct EmptyHints {
    pub palette: String,
    pub new: String,
    pub split: String,
}

/// Überschrift des leeren Hubs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum EmptyKind {
    /// Erster Start: noch gar keine Sitzung (nur im Hauptfenster).
    Welcome,
    /// Weiteres Fenster ohne eigene Sitzung (#110) – auch wenn andere Fenster Sitzungen haben.
    Window,
    /// Im aktiven Projekt dieses Fensters ist keine Sitzung.
    Project,
    /// Es gibt Sitzungen, aber alle Bereiche sind minimiert (sie laufen weiter, stehen in der Seitenleiste).
    Minimized,
}

/// Überschrift für Fenster `window`: `any_session` = irgendeine Sitzung in irgendeinem Fenster, `in_window` = eine in diesem.
pub(super) fn empty_kind(window: u64, any_session: bool, in_window: bool) -> EmptyKind {
    match (window, any_session, in_window) {
        (0, false, _) => EmptyKind::Welcome,
        (w, _, false) if w != 0 => EmptyKind::Window,
        _ => EmptyKind::Project,
    }
}

/// Wo die Knöpfe gezeichnet wurden (für Tests ohne Maus).
#[derive(Default)]
pub(super) struct EmptyHits {
    pub new: Option<Rect>,
    pub claude: Option<Rect>,
    pub chat: Option<Rect>,
}

/// Mittiger Hinweis „keine Sitzung“ im Bereich `rect`; `kind` bestimmt die Überschrift (siehe `empty_kind`).
pub(super) fn empty_state(ui: &mut Ui, rect: Rect, lang: Lang, claude_mode: bool, kind: EmptyKind, hints: &EmptyHints, actions: &mut Vec<Action>) -> EmptyHits {
    let mut hits = EmptyHits::default();
    ui.scope_builder(UiBuilder::new().max_rect(Rect::from_center_size(rect.center(), Vec2::new(480.0, 300.0))), |ui| {
        ui.vertical_centered(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
            ui.painter().rect_filled(r, 14.0, style::ACCENT.gamma_multiply(0.18));
            style::paint_icon(ui.painter(), SIcon::Spark, r.center(), style::ACCENT);
            ui.add_space(S3);
            ui.heading(match kind {
                EmptyKind::Welcome => lang.t("Willkommen bei Paneldeck", "Welcome to Paneldeck"),
                EmptyKind::Window => lang.t("Noch keine Sitzung in diesem Fenster", "No session in this window yet"),
                EmptyKind::Project => lang.t("Keine Sitzung in diesem Projekt", "No session in this project"),
                EmptyKind::Minimized => lang.t("Alle Bereiche sind minimiert", "All panes are minimized"),
            });
            ui.add_space(S1);
            let sub = if kind == EmptyKind::Minimized {
                lang.t("Die Sitzungen laufen weiter – ein Klick in der Seitenleiste holt eine zurück.", "The sessions keep running – click one in the sidebar to bring it back.")
            } else {
                lang.t("Starte ein Terminal oder eine Claude-Sitzung – alles an einem Ort.", "Start a terminal or a Claude session – all in one place.")
            };
            ui.label(RichText::new(sub).color(style::dim(ui)));
            ui.add_space(S3);
            ui.horizontal(|ui| {
                ui.add_space((ui.available_width() - if claude_mode { 450.0 } else { 300.0 }) / 2.0);
                let b = ui.add(egui::Button::new(RichText::new(lang.t("Neues Terminal", "New terminal")).strong()).fill(style::ACCENT.gamma_multiply(0.85)).min_size(Vec2::new(140.0, 32.0)));
                hits.new = Some(b.rect);
                if b.clicked() {
                    actions.push(Action::NewQuick);
                }
                if claude_mode {
                    let c = ui.add(egui::Button::new(lang.t("Neue Claude-Sitzung …", "New Claude session …")).min_size(Vec2::new(140.0, 32.0)));
                    hits.claude = Some(c.rect);
                    if c.clicked() {
                        actions.push(Action::NewDialog(true));
                    }
                }
                let ch = ui.add(egui::Button::new(lang.t("Neuer Chat", "New chat")).min_size(Vec2::new(140.0, 32.0)));
                hits.chat = Some(ch.rect);
                if ch.clicked() {
                    actions.push(Action::NewChat);
                }
            });
            ui.add_space(S3 * 1.5);
            let hint = |ui: &mut Ui, keys: &str, what: &str| {
                ui.horizontal(|ui| {
                    ui.add_space((ui.available_width() - 330.0).max(0.0) / 2.0);
                    ui.label(RichText::new(keys).monospace().color(style::dim(ui)));
                    ui.label(RichText::new(what).color(style::dim(ui)));
                });
            };
            hint(ui, &hints.palette, lang.t("Befehle & Suche", "Commands & search"));
            hint(ui, &hints.new, lang.t("Neue Sitzung", "New session"));
            hint(ui, &hints.split, lang.t("Bereich teilen", "Split pane"));
        });
    });
    hits
}

impl App {
    /// Kürzel für den leeren Hub (folgen eigenen Belegungen und der Sprache).
    pub(super) fn empty_heading(&self) -> EmptyKind {
        // Nur, wenn wirklich alles minimiert ist (nicht bloß weggewechselte Sitzungen); sonst wie bisher.
        let docked = self.docked_ids();
        if !docked.is_empty() && docked.iter().all(|id| self.is_minimized(*id)) {
            return EmptyKind::Minimized;
        }
        empty_kind(self.wins.cur, !self.entries.is_empty(), self.entries.values().any(|e| self.in_win(e)))
    }

    pub(super) fn empty_hints(&self) -> EmptyHints {
        EmptyHints { palette: self.binding_text("palette"), new: self.binding_text("new"), split: format!("{} / {}", self.binding_text("split_r"), self.binding_text("split_d")) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, PointerButton, RawInput};

    struct H {
        ctx: egui::Context,
        time: f64,
        hits: EmptyHits,
        actions: Vec<Action>,
        texts: Vec<String>,
    }

    impl H {
        fn new() -> H {
            H { ctx: egui::Context::default(), time: 0.0, hits: EmptyHits::default(), actions: Vec::new(), texts: Vec::new() }
        }
        fn frame(&mut self, lang: Lang, claude: bool, events: Vec<Event>) {
            self.time += 0.1;
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0))), time: Some(self.time), events, ..Default::default() };
            let hints = EmptyHints { palette: "Ctrl+Shift+P".into(), new: "Ctrl+Shift+T".into(), split: "Ctrl+Shift+D / Ctrl+Shift+E".into() };
            let mut acts = Vec::new();
            let mut hits = EmptyHits::default();
            let out = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let rect = ui.available_rect_before_wrap();
                    hits = empty_state(ui, rect, lang, claude, EmptyKind::Welcome, &hints, &mut acts);
                });
            });
            self.texts = out.shapes.iter().filter_map(|s| if let egui::Shape::Text(t) = &s.shape { Some(t.galley.text().to_string()) } else { None }).collect();
            self.hits = hits;
            self.actions.extend(acts);
        }
        fn click(&mut self, lang: Lang, claude: bool, pos: Pos2) {
            let btn = |pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
            self.frame(lang, claude, vec![Event::PointerMoved(pos)]);
            self.frame(lang, claude, vec![btn(true)]);
            self.frame(lang, claude, vec![btn(false)]);
        }
    }

    #[test]
    fn empty_hub_offers_new_terminal_and_claude_in_both_languages() {
        for (lang, new, claude) in [(Lang::En, "New terminal", "New Claude session …"), (Lang::De, "Neues Terminal", "Neue Claude-Sitzung …")] {
            let mut h = H::new();
            h.frame(lang, true, vec![]);
            h.frame(lang, true, vec![]);
            assert!(h.texts.iter().any(|t| t == new), "{lang:?}: {:?}", h.texts);
            assert!(h.texts.iter().any(|t| t == claude), "{lang:?}: {:?}", h.texts);
            let p = h.hits.new.expect("Knopf gezeichnet").center();
            h.click(lang, true, p);
            assert!(matches!(h.actions.as_slice(), [Action::NewQuick]), "Klick auf „Neues Terminal“ legt eine Sitzung an");
            let p = h.hits.claude.expect("Claude-Knopf gezeichnet").center();
            h.click(lang, true, p);
            assert!(matches!(h.actions.as_slice(), [Action::NewQuick, Action::NewDialog(true)]));
            let chat = if lang == Lang::De { "Neuer Chat" } else { "New chat" };
            assert!(h.texts.iter().any(|t| t == chat), "{lang:?}: {:?}", h.texts);
            let p = h.hits.chat.expect("Chat-Knopf gezeichnet").center();
            h.click(lang, true, p);
            assert!(matches!(h.actions.as_slice(), [Action::NewQuick, Action::NewDialog(true), Action::NewChat]));
        }
    }

    #[test]
    fn heading_follows_the_sessions_of_this_window() {
        assert_eq!(empty_kind(0, false, false), EmptyKind::Welcome, "erster Start");
        assert_eq!(empty_kind(0, true, false), EmptyKind::Project, "Hauptfenster leer, andere Fenster haben Sitzungen");
        assert_eq!(empty_kind(2, true, false), EmptyKind::Window, "neues Fenster: keine eigene Sitzung");
        assert_eq!(empty_kind(2, false, false), EmptyKind::Window, "kein Willkommen in einem weiteren Fenster");
        assert_eq!(empty_kind(2, true, true), EmptyKind::Project, "Fenster hat Sitzungen, nur nicht im aktiven Projekt");
    }

    #[test]
    fn without_claude_mode_only_the_terminal_button_is_shown() {
        let mut h = H::new();
        h.frame(Lang::En, false, vec![]);
        h.frame(Lang::En, false, vec![]);
        assert!(h.hits.new.is_some());
        assert!(h.hits.claude.is_none());
        assert!(h.hits.chat.is_some(), "Chat gibt es auch ohne Claude-Modus");
        assert!(h.texts.iter().any(|t| t == "Ctrl+Shift+T"), "Kürzel stehen als Hinweis da: {:?}", h.texts);
    }
}
