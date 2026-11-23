//! Plan-Modus in der Chat-Oberfläche (#358): Schalter *Plan* mit Stufe neben *Senden* (Umschalt+Tab im Eingabefeld),
//! Hinweis in der Kopfzeile, die Plan-Karte mit *Ausführen* / *Änderungen anfragen* / *Verwerfen* / *Exportieren* – und
//! die Ausführung dieser Knöpfe in der App. Zustand und Regeln stehen in `chat::plan`.

use super::chat_ui::ChatOut;
use super::*;
use crate::chat::plan::{self, Plan, PlanAction, PlanLevel, PlanStatus};
use crate::markdown::{self, RenderOpts};
use eframe::egui::{Color32, CornerRadius, Event, Key, Rect, RichText, Stroke, Ui, UiBuilder};

/// Breite von Schalter und Stufenwahl links neben *Senden*.
pub(super) const PLAN_W: f32 = 140.0;

/// Id des Kommentarfelds von *Änderungen anfragen* (egui-Speicher: `Some(text)` = Feld offen).
fn comment_id(session: u64) -> egui::Id {
    egui::Id::new(("chat_plan_comment", session))
}

/// Id des Textfelds darin.
fn comment_field(session: u64) -> egui::Id {
    comment_id(session).with("field")
}

/// Umschalt+Tab im fokussierten Eingabefeld: Plan-Modus umschalten statt den Fokus zurückzusetzen (wie in Claude Code).
/// Vor dem Eingabefeld aufrufen. `true`, wenn umgeschaltet wurde.
pub(super) fn shift_tab(ui: &Ui, chat: &mut ChatState) -> bool {
    let hit = ui.input_mut(|i| {
        let pos = i.events.iter().position(|e| matches!(e, Event::Key { key: Key::Tab, pressed: true, modifiers, .. } if modifiers.shift && !modifiers.ctrl && !modifiers.alt && !modifiers.command));
        pos.map(|p| i.events.remove(p)).is_some()
    });
    if hit {
        // egui hat den Fokuswechsel schon zu Beginn des Bildes vorgemerkt: zurücknehmen, das Eingabefeld behält den Fokus.
        ui.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
        chat.toggle_plan();
    }
    hit
}

/// Platzhalter des Eingabefelds bei eingeschaltetem Plan-Modus.
pub(super) fn input_hint(lang: Lang) -> &'static str {
    lang.t("Plan-Modus: Aufgabe beschreiben … (Enter sendet, Umschalt+Tab: Plan aus)", "Plan mode: describe the task … (Enter sends, Shift+Tab: plan off)")
}

/// Schalter *Plan* und Stufenwahl in `rect` (links neben *Senden*). Der Zustand ist sichtbar: eingeschaltet ist der
/// Schalter hervorgehoben.
pub(super) fn plan_toggle(ui: &mut Ui, rect: Rect, session: u64, chat: &mut ChatState, lang: Lang) {
    let on = chat.planning.on;
    let accent = crate::style::ui().accent;
    ui.scope_builder(UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let text = if on { RichText::new(lang.t("Plan ✓", "Plan ✓")).strong().color(ui.visuals().strong_text_color()) } else { RichText::new("Plan") };
        let mut b = egui::Button::new(text).selected(on).min_size(Vec2::new(56.0, rect.height()));
        if on {
            b = b.fill(accent.gamma_multiply(0.45)).stroke(Stroke::new(1.0_f32, accent));
        }
        let tip = lang.t(
            "Plan-Modus für die nächste Nachricht (Umschalt+Tab): Das Modell recherchiert nur lesend und schreibt einen Plan; geändert wird erst nach „Ausführen“.",
            "Plan mode for the next message (Shift+Tab): the model researches read-only and writes a plan; nothing changes until you press Execute.",
        );
        if ui.add(b).on_hover_text(tip).clicked() {
            chat.toggle_plan();
        }
        let level = chat.planning.level;
        egui::ComboBox::from_id_salt(("chat_plan_level", session)).selected_text(RichText::new(level.label(lang)).small()).width((rect.width() - 64.0).max(60.0)).show_ui(ui, |ui| {
            for l in PlanLevel::ALL {
                if ui.selectable_label(l == level, l.label(lang)).on_hover_text(l.hint(lang)).clicked() {
                    chat.set_plan_level(l);
                }
            }
        });
    });
}

/// Kopfzeile: „Plan-Modus · nur lesen“ während einer Plan-Runde, „Plan wartet auf dich“ bei einem vorgeschlagenen Plan.
pub(super) fn plan_chip(ui: &mut Ui, chat: &ChatState, lang: Lang) {
    let warn = crate::style::ui().warning;
    if let (Some(level), true) = (chat.planning.round, chat.busy()) {
        let text = format!("{} · {} · {}", lang.t("Plan-Modus", "Plan mode"), level.label(lang), lang.t("nur lesen", "read-only"));
        ui.label(RichText::new(text).small().color(warn)).on_hover_text(lang.t(
            "Das Modell recherchiert nur mit lesenden Werkzeugen; schreibende sind bis „Ausführen“ gesperrt.",
            "The model researches with read-only tools only; writing tools are locked until you press Execute.",
        ));
    } else if chat.plan_decidable() {
        ui.label(RichText::new(lang.t("Plan wartet auf dich", "Plan waiting for you")).small().color(warn));
    }
}

/// Text und Farbe des Stands auf der Karte.
fn status_text(p: &Plan, lang: Lang) -> (&'static str, Color32) {
    let ui = crate::style::ui();
    match p.status {
        PlanStatus::Proposed => (lang.t("wartet auf deine Entscheidung", "waiting for your decision"), ui.warning),
        PlanStatus::Revising => (lang.t("Änderungen angefragt – das Modell überarbeitet den Plan …", "changes requested – the model is revising the plan …"), ui.warning),
        PlanStatus::Approved => (lang.t("✓ freigegeben – die Schritte stehen in der To-do-Liste", "✓ approved – the steps are in the to-do list"), ui.success),
        PlanStatus::Discarded => (lang.t("verworfen", "discarded"), ui.error),
    }
}

/// Plan-Karte: die Antwort mit dem aktuellen Plan, gerahmt, mit Stufe, Fassung, Stand und den Knöpfen.
/// `decidable` = der Plan wartet und keine Runde läuft (sonst sind die Knöpfe aus).
pub(super) fn plan_card(ui: &mut Ui, session: u64, p: &Plan, doc: &markdown::Doc, decidable: bool, lang: Lang, out: &mut ChatOut) {
    let accent = crate::style::ui().accent;
    let dim = crate::style::dim(ui);
    let stroke = if p.status == PlanStatus::Discarded { Stroke::new(1.0_f32, dim) } else { Stroke::new(1.5_f32, accent.gamma_multiply(0.8)) };
    egui::Frame::new().fill(ui.visuals().faint_bg_color).stroke(stroke).corner_radius(CornerRadius::same(crate::style::RADIUS)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(lang.t("Plan", "Plan")).strong().color(accent));
            let mut meta = format!("{} {}", lang.t("Stufe", "level"), p.level.label(lang));
            if p.revision > 1 {
                meta.push_str(&format!(" · {} {}", lang.t("Fassung", "revision"), p.revision));
            }
            ui.label(RichText::new(meta).small().color(dim));
            let (st, col) = status_text(p, lang);
            ui.label(RichText::new(st).small().color(col));
        });
        ui.add_space(4.0);
        let opts = RenderOpts { soft_breaks_as_newlines: true, copy_button: Some(lang.t("Kopieren", "Copy")) };
        let o = markdown::render(ui, doc, &opts);
        if let Some(u) = o.clicked_url {
            ui.ctx().open_url(egui::OpenUrl::new_tab(u));
        }
        ui.add_space(6.0);
        if let Some(f) = &p.file {
            ui.label(RichText::new(format!("{} {f}", lang.t("Gespeichert im Datenordner:", "Saved in the data folder:"))).small().color(dim));
        }
        let cid = comment_id(session);
        let mut comment: Option<String> = ui.data(|d| d.get_temp(cid));
        if p.status != PlanStatus::Proposed {
            comment = None;
        }
        let mut close = false;
        match comment.as_mut() {
            Some(text) => {
                ui.label(RichText::new(lang.t("Was soll sich ändern?", "What should change?")).small());
                let r = ui.add(egui::TextEdit::multiline(text).id(comment_field(session)).desired_rows(2).desired_width(f32::INFINITY).hint_text(lang.t("z. B. „Tests zuerst“, „ohne neue Abhängigkeit“ …", "e.g. “tests first”, “no new dependency” …")));
                if r.gained_focus() {
                    out.focus = true;
                }
                ui.horizontal(|ui| {
                    let ok = decidable && !text.trim().is_empty();
                    if ui.add_enabled(ok, egui::Button::new(lang.t("Änderungen senden", "Send changes"))).clicked() {
                        out.plan = Some(PlanAction::RequestChanges(text.trim().to_string()));
                        close = true;
                    } else if ui.button(lang.t("Abbrechen", "Cancel")).clicked() {
                        close = true;
                    }
                });
            }
            None => {
                ui.horizontal_wrapped(|ui| {
                    if p.status == PlanStatus::Proposed {
                        let busy_tip = lang.t("Erst, wenn die Antwort fertig ist.", "Only when the answer is done.");
                        let exec = egui::Button::new(RichText::new(lang.t("Ausführen", "Execute")).strong()).fill(crate::style::ui().chat_send);
                        let r = ui
                            .add_enabled(decidable, exec)
                            .on_hover_text(lang.t("Plan freigeben: Schreib-Werkzeuge frei, die Schritte kommen in die To-do-Liste.", "Approve the plan: writing tools are unlocked, the steps go to the to-do list."))
                            .on_disabled_hover_text(busy_tip);
                        if r.clicked() {
                            out.plan = Some(PlanAction::Execute);
                        }
                        if ui.add_enabled(decidable, egui::Button::new(lang.t("Änderungen anfragen", "Request changes"))).clicked() {
                            comment = Some(String::new());
                        }
                        if ui.add_enabled(decidable, egui::Button::new(lang.t("Verwerfen", "Discard"))).clicked() {
                            out.plan = Some(PlanAction::Discard);
                        }
                    }
                    if ui.button(lang.t("Exportieren …", "Export …")).on_hover_text(lang.t("Plan als Markdown-Datei speichern", "Save the plan as a Markdown file")).clicked() {
                        out.plan = Some(PlanAction::Export);
                    }
                });
            }
        }
        ui.data_mut(|d| match comment.filter(|_| !close) {
            Some(c) => d.insert_temp(cid, c),
            None => d.remove::<String>(cid),
        });
    });
}

impl App {
    /// Knopf der Plan-Karte ausführen (#358). *Ausführen* gibt den Plan frei, füllt die To-do-Liste, sendet die
    /// Ausführungs-Nachricht und speichert den Plan als Markdown im Datenordner (`plans/`).
    pub(super) fn chat_plan_action(&mut self, ctx: &egui::Context, id: u64, act: &PlanAction) {
        if *act == PlanAction::Export {
            return self.export_plan(id);
        }
        // Wie beim Senden: ohne gewählte Stufe des Modells geht keine Runde los (die Frage erscheint im Chat).
        if matches!(act, PlanAction::Execute | PlanAction::RequestChanges(_)) && self.chat_level_unchosen(id) {
            if let Some(c) = self.entries.get_mut(&id).and_then(|e| e.chat.as_mut()) {
                c.level_question = Some(chatmod::LevelAsker::Agent);
                c.touch();
            }
            return;
        }
        let lang = self.settings.language;
        let backend = self.chat_backend.clone();
        let rctx = ctx.clone();
        let repaint: chatmod::Repaint = std::sync::Arc::new(move || rctx.request_repaint());
        let tcx = self.turn_context(id);
        let dir = self.dir.clone();
        let Some(e) = self.entries.get_mut(&id) else { return };
        let title = e.display_name();
        let meta = e.meta.chat.clone().unwrap_or_default();
        let Some(c) = e.chat.as_mut() else { return };
        let done = match act {
            PlanAction::Execute => c.execute_plan(plan::execute_message(lang), backend.as_ref(), &meta, &tcx, repaint),
            PlanAction::RequestChanges(comment) => c.request_plan_changes(&plan::changes_message(lang, comment), backend.as_ref(), &meta, &tcx, repaint),
            PlanAction::Discard => c.discard_plan(),
            PlanAction::Export => false,
        };
        let mut failed = None;
        if done {
            // Vom Nutzer ausgelöst: eine frühere Agenten-Herkunft gilt nicht mehr (wie beim Senden).
            if !matches!(act, PlanAction::Discard) {
                c.origin_sessions = Vec::new();
            }
            if let Some(p) = c.file.plan.clone().filter(|_| *act == PlanAction::Execute) {
                match plan::save(&dir, id, &p, &title) {
                    Ok(rel) => {
                        if let Some(p) = c.file.plan.as_mut() {
                            p.file = Some(rel);
                        }
                        c.touch();
                    }
                    Err(err) => failed = Some(err.to_string()),
                }
            }
        }
        e.status = chat_claude_status(c.status());
        if done {
            self.save_chat(id);
        }
        if let Some(err) = failed {
            self.notify_text(format!("{}: {err}", lang.t("Plan konnte nicht gespeichert werden", "The plan could not be saved")));
        }
    }

    /// *Exportieren*: Plan als Markdown-Datei speichern (Dateidialog; Abbrechen speichert nichts).
    fn export_plan(&mut self, id: u64) {
        let lang = self.settings.language;
        let Some(e) = self.entries.get(&id) else { return };
        let Some(p) = e.chat.as_ref().and_then(|c| c.file.plan.clone()) else { return };
        let title = e.display_name();
        let Some(path) = notify::save_any(None, &plan::export_name(&title, &p)) else { return };
        match std::fs::write(&path, plan::file_text(&p, &title)) {
            Ok(()) => self.notify_text(format!("{} {}", lang.t("Plan gespeichert:", "Plan saved:"), path.display())),
            Err(err) => self.notify_text(format!("{}: {err}", lang.t("Speichern fehlgeschlagen", "Saving failed"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::chat_ui::{chat_body, input_id};
    use super::*;
    use crate::chat::store::ChatMsg;
    use crate::chat::{ChatFile, Role};
    use eframe::egui::{Modifiers, PointerButton, Pos2, RawInput};

    struct H {
        ctx: egui::Context,
        t: f64,
        shapes: Vec<egui::epaint::ClippedShape>,
    }

    impl H {
        fn new() -> H {
            H { ctx: egui::Context::default(), t: 0.0, shapes: Vec::new() }
        }
        fn frame(&mut self, chat: &mut ChatState, lang: Lang, focus: bool, events: Vec<Event>) -> ChatOut {
            self.t += 0.05;
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 900.0))), time: Some(self.t), events, ..Default::default() };
            let mut res = ChatOut::default();
            let full = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let r = ui.available_rect_before_wrap();
                    res = chat_body(ui, r, 1, chat, "m", lang, focus);
                });
            });
            self.shapes = full.shapes;
            res
        }
        fn texts(&self) -> Vec<String> {
            self.shapes.iter().filter_map(|s| if let egui::Shape::Text(t) = &s.shape { Some(t.galley.text().to_string()) } else { None }).collect()
        }
        fn has(&self, text: &str) -> bool {
            self.texts().iter().any(|t| t == text)
        }
        fn pos_of(&self, text: &str) -> Option<Pos2> {
            self.shapes.iter().rev().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == text => Some(t.visual_bounding_rect().center()),
                _ => None,
            })
        }
        /// Auf den Text klicken; Ergebnis: alle Ausgaben der Bilder zusammen.
        fn click(&mut self, chat: &mut ChatState, lang: Lang, text: &str) -> ChatOut {
            let pos = self.pos_of(text).unwrap_or_else(|| panic!("„{text}“ nicht gefunden in {:?}", self.texts()));
            let btn = |pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
            let mut o = self.frame(chat, lang, false, vec![Event::PointerMoved(pos)]);
            for e in [btn(true), btn(false)] {
                let r = self.frame(chat, lang, false, vec![e]);
                o.plan = o.plan.or(r.plan);
                o.send |= r.send;
            }
            self.frame(chat, lang, false, vec![]);
            o
        }
    }

    const PLAN: &str = "# Plan: Tidy up\n\n## Steps\n1. Read the code\n2. Clean it up";

    fn with_plan(status: PlanStatus) -> ChatState {
        let mut c = ChatState::new(ChatFile { messages: vec![ChatMsg::new(Role::User, "tidy"), ChatMsg::new(Role::Assistant, PLAN)], ..ChatFile::default() }, None);
        c.file.plan = Some(Plan { level: PlanLevel::High, status, markdown: PLAN.into(), revision: 2, created: 1, ..Plan::default() });
        c
    }

    fn key(k: Key, m: Modifiers) -> Event {
        Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: m }
    }

    #[test]
    fn the_plan_card_offers_execute_request_changes_discard_and_export() {
        for lang in [Lang::En, Lang::De] {
            let mut c = with_plan(PlanStatus::Proposed);
            let mut h = H::new();
            h.frame(&mut c, lang, false, vec![]);
            h.frame(&mut c, lang, false, vec![]);
            let t = |de: &'static str, en: &'static str| lang.t(de, en);
            for label in [t("Ausführen", "Execute"), t("Änderungen anfragen", "Request changes"), t("Verwerfen", "Discard"), t("Exportieren …", "Export …"), t("Plan wartet auf dich", "Plan waiting for you")] {
                assert!(h.has(label), "{label} fehlt: {:?}", h.texts());
            }
            assert!(h.texts().iter().any(|s| s.contains(t("Fassung", "revision")) && s.contains(PlanLevel::High.label(lang))), "Stufe und Fassung: {:?}", h.texts());
            assert_eq!(h.click(&mut c, lang, t("Ausführen", "Execute")).plan, Some(PlanAction::Execute));
            assert_eq!(h.click(&mut c, lang, t("Verwerfen", "Discard")).plan, Some(PlanAction::Discard));
            assert_eq!(h.click(&mut c, lang, t("Exportieren …", "Export …")).plan, Some(PlanAction::Export));
            // Änderungen anfragen: Kommentarfeld, leer gesperrt, mit Text gesendet
            assert_eq!(h.click(&mut c, lang, t("Änderungen anfragen", "Request changes")).plan, None);
            assert!(h.has(t("Was soll sich ändern?", "What should change?")));
            assert_eq!(h.click(&mut c, lang, t("Änderungen senden", "Send changes")).plan, None, "leerer Kommentar sendet nichts");
            h.ctx.memory_mut(|m| m.request_focus(comment_field(1)));
            h.frame(&mut c, lang, false, vec![]);
            h.frame(&mut c, lang, false, vec![Event::Text("tests first".into())]);
            h.frame(&mut c, lang, false, vec![]);
            assert_eq!(h.click(&mut c, lang, t("Änderungen senden", "Send changes")).plan, Some(PlanAction::RequestChanges("tests first".into())));
            assert!(!h.has(t("Was soll sich ändern?", "What should change?")), "Feld wieder zu");
        }
    }

    #[test]
    fn card_states_after_approval_discard_and_while_busy() {
        let mut c = with_plan(PlanStatus::Approved);
        c.file.plan.as_mut().unwrap().file = Some("plans/1-2.md".into());
        let mut h = H::new();
        h.frame(&mut c, Lang::En, false, vec![]);
        h.frame(&mut c, Lang::En, false, vec![]);
        assert!(h.has("✓ approved – the steps are in the to-do list") && h.has("Export …"));
        assert!(h.texts().iter().any(|t| t.contains("plans/1-2.md")), "Datei genannt");
        assert!(!h.has("Execute") && !h.has("Discard"), "nach der Freigabe keine Entscheidung mehr");
        assert!(h.has("Read the code"), "die Karte zeigt den Plan");
        let mut c = with_plan(PlanStatus::Discarded);
        h.frame(&mut c, Lang::En, false, vec![]);
        h.frame(&mut c, Lang::En, false, vec![]);
        assert!(h.has("discarded") && !h.has("Execute"));
        // Andere Nachrichten sind keine Karte
        let mut c = with_plan(PlanStatus::Proposed);
        c.file.messages[1].content = "# Plan: other".into();
        h.frame(&mut c, Lang::En, false, vec![]);
        h.frame(&mut c, Lang::En, false, vec![]);
        assert!(!h.has("Execute"));
        // Während einer Runde: Knöpfe aus, Hinweis in der Kopfzeile
        let mut c = with_plan(PlanStatus::Revising);
        c.planning.round = Some(PlanLevel::Max);
        let slow = crate::chat::EchoBackend { delay: std::time::Duration::from_millis(200) };
        c.draft = "go".into();
        c.planning.on = true;
        c.planning.level = PlanLevel::Max;
        assert!(c.send(&slow, &crate::chat::default_chat_meta(), &crate::chat::TurnContext::default(), std::sync::Arc::new(|| {})));
        h.frame(&mut c, Lang::En, false, vec![]);
        h.frame(&mut c, Lang::En, false, vec![]);
        assert!(h.has("Plan mode · max · read-only"), "{:?}", h.texts());
        assert!(h.has("changes requested – the model is revising the plan …"));
        c.stop();
    }

    #[test]
    fn shift_tab_toggles_the_plan_mode_and_keeps_the_focus() {
        let mut c = ChatState::new(ChatFile::default(), None);
        let mut h = H::new();
        h.frame(&mut c, Lang::En, true, vec![]);
        h.frame(&mut c, Lang::En, false, vec![]);
        assert!(h.ctx.memory(|m| m.has_focus(input_id(1))));
        assert!(!c.planning.on && h.has("Plan"));
        h.frame(&mut c, Lang::En, false, vec![key(Key::Tab, Modifiers::SHIFT)]);
        h.frame(&mut c, Lang::En, false, vec![]);
        assert!(c.planning.on, "Umschalt+Tab schaltet ein");
        assert!(h.ctx.memory(|m| m.has_focus(input_id(1))), "das Eingabefeld behält den Fokus");
        assert!(h.has("Plan ✓"), "Zustand sichtbar: {:?}", h.texts());
        assert!(h.has("Plan mode: describe the task … (Enter sends, Shift+Tab: plan off)"));
        h.frame(&mut c, Lang::En, false, vec![key(Key::Tab, Modifiers::SHIFT)]);
        assert!(!c.planning.on, "und wieder aus");
        assert!(c.draft.is_empty(), "kein Tab im Text");
        // Ohne Fokus im Eingabefeld tut Umschalt+Tab nichts
        h.ctx.memory_mut(|m| m.surrender_focus(input_id(1)));
        h.frame(&mut c, Lang::En, false, vec![]);
        h.frame(&mut c, Lang::En, false, vec![key(Key::Tab, Modifiers::SHIFT)]);
        assert!(!c.planning.on);
    }

    #[test]
    fn toggle_button_and_level_picker() {
        for lang in [Lang::En, Lang::De] {
            let mut c = ChatState::new(ChatFile::default(), None);
            let mut h = H::new();
            h.frame(&mut c, lang, false, vec![]);
            h.click(&mut c, lang, "Plan");
            assert!(c.planning.on);
            h.click(&mut c, lang, lang.t("Plan ✓", "Plan ✓"));
            assert!(!c.planning.on);
            // Stufe wählen schaltet den Plan-Modus ein
            h.click(&mut c, lang, PlanLevel::Medium.label(lang));
            for l in PlanLevel::ALL {
                assert!(h.has(l.label(lang)), "{} in der Auswahl", l.label(lang));
            }
            h.click(&mut c, lang, PlanLevel::XHigh.label(lang));
            assert_eq!((c.planning.on, c.planning.level), (true, PlanLevel::XHigh));
            assert!(h.has(PlanLevel::XHigh.label(lang)));
        }
    }

    #[test]
    fn plan_level_texts_are_bilingual() {
        for l in PlanLevel::ALL {
            for f in [PlanLevel::label, PlanLevel::hint] {
                assert!(!f(l, Lang::De).is_empty() && !f(l, Lang::En).is_empty());
            }
            assert_ne!(l.hint(Lang::De), l.hint(Lang::En));
        }
        assert_ne!(plan::execute_message(Lang::De), plan::execute_message(Lang::En));
        assert!(plan::changes_message(Lang::En, " more tests ").ends_with("more tests"));
    }
}
