//! Formular für die Fragen des Modells im Chat (`ask_user`, #356): je Frage die Antworten als Auswahlknöpfe
//! (Einfachauswahl) bzw. Kästchen (Mehrfachauswahl), empfohlene zuerst mit „(Empfohlen)“, immer „Andere …“ mit freiem
//! Text, darunter ein Knopf „Absenden“ und „Abbrechen“. Den Zustand hält `chat::ask::AskForm`; die Antwort schickt
//! `ChatState::answer_ask` an den wartenden Aufruf.
//!
//! Wie bei den Freigaben gilt Enter nicht aus dem Eingabefeld des Chats heraus – abgesendet wird nur mit dem Knopf.

use crate::chat::ask::{AskForm, AskReply};
use crate::state::Lang;
use crate::style::{self, S1, S2};
use eframe::egui::{self, CornerRadius, FontId, Id, Margin, RichText, Stroke, TextFormat, Ui};

/// Text eines Auswahlknopfs: Antwort, bei Empfehlung „(Empfohlen)“ in der Akzentfarbe.
fn option_text(ui: &Ui, label: &str, recommended: bool, lang: Lang) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let body = FontId::proportional(13.5);
    job.append(label, 0.0, TextFormat { font_id: body.clone(), color: ui.visuals().text_color(), ..Default::default() });
    if recommended {
        job.append(lang.t("(Empfohlen)", "(Recommended)"), 6.0, TextFormat { font_id: body, color: style::ui().accent, ..Default::default() });
    }
    job
}

/// Id des Freitextfelds „Andere …“ der Frage `q`.
pub(super) fn other_id(session: u64, call_id: &str, q: usize) -> Id {
    Id::new(("chat_ask_other", session, call_id, q))
}

/// Zeichnet das Formular; `Some(…)`, wenn der Nutzer absendet (nur, wenn alles beantwortet ist) oder abbricht.
pub(super) fn ask_form(ui: &mut Ui, session: u64, form: &mut AskForm, lang: Lang) -> Option<AskReply> {
    let accent = style::ui().accent;
    let mut reply = None;
    egui::Frame::new().fill(accent.gamma_multiply(0.10)).stroke(Stroke::new(1.0_f32, accent.gamma_multiply(0.6))).corner_radius(CornerRadius::same(style::RADIUS)).inner_margin(Margin::same(10)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        let title = if form.questions.len() == 1 { lang.t("Das Modell hat eine Frage an dich", "The model has a question for you") } else { lang.t("Das Modell hat Fragen an dich", "The model has questions for you") };
        ui.label(RichText::new(title).strong());
        for q in 0..form.questions.len() {
            ui.add_space(S2);
            let (question, header, multi) = {
                let x = &form.questions[q];
                (x.question.clone(), x.header.clone(), x.multi)
            };
            ui.horizontal_wrapped(|ui| {
                if !header.is_empty() {
                    egui::Frame::new().fill(ui.visuals().faint_bg_color).corner_radius(CornerRadius::same(4)).inner_margin(Margin::symmetric(5, 1)).show(ui, |ui| {
                        ui.label(RichText::new(&header).small().color(style::dim(ui)));
                    });
                }
                ui.label(RichText::new(&question).strong().size(13.5));
            });
            let hint = if multi { lang.t("Beliebig viele wählen", "Choose any number") } else { lang.t("Eine Antwort wählen", "Choose one") };
            ui.label(RichText::new(hint).small().color(style::dim(ui)));
            ui.add_space(S1);
            for i in 0..form.questions[q].options.len() {
                let (label, description, recommended) = {
                    let o = &form.questions[q].options[i];
                    (o.label.clone(), o.description.clone(), o.recommended)
                };
                let selected = form.answers[q].selected[i];
                let text = option_text(ui, &label, recommended, lang);
                ui.push_id(("chat_ask_opt", q, i), |ui| {
                    if multi {
                        let mut v = selected;
                        if ui.checkbox(&mut v, text).changed() {
                            form.toggle(q, i);
                        }
                    } else if ui.radio(selected, text).clicked() && !selected {
                        form.toggle(q, i);
                    }
                    if !description.is_empty() {
                        ui.horizontal(|ui| {
                            ui.add_space(ui.spacing().icon_width + ui.spacing().icon_spacing);
                            ui.add(egui::Label::new(RichText::new(&description).small().color(style::dim(ui))).wrap());
                        });
                    }
                });
            }
            // „Andere …“ mit freiem Text: Tippen wählt es aus.
            let other = form.answers[q].other;
            let other_label = lang.t("Andere …", "Other …");
            ui.push_id(("chat_ask_other_row", q), |ui| {
                ui.horizontal(|ui| {
                    if multi {
                        let mut v = other;
                        if ui.checkbox(&mut v, other_label).changed() {
                            form.set_other(q, v);
                        }
                    } else if ui.radio(other, other_label).clicked() && !other {
                        form.set_other(q, true);
                    }
                    let call_id = form.call_id.clone();
                    let edit = egui::TextEdit::singleline(&mut form.answers[q].other_text)
                        .id(other_id(session, &call_id, q))
                        .hint_text(lang.t("eigene Antwort", "your own answer"))
                        .char_limit(crate::chat::ask::MAX_OTHER_CHARS)
                        .desired_width(ui.available_width().min(360.0));
                    let r = ui.add(edit);
                    if r.changed() && !form.answers[q].other_text.trim().is_empty() && !form.answers[q].other {
                        form.set_other(q, true);
                    }
                });
            });
        }
        ui.add_space(S2);
        let complete = form.complete();
        ui.horizontal_wrapped(|ui| {
            let submit = ui.add_enabled(complete, egui::Button::new(RichText::new(lang.t("Absenden", "Submit")).strong()).fill(style::ui().chat_send));
            if submit.clicked() {
                reply = Some(AskReply::Submit);
            }
            if ui.button(lang.t("Abbrechen", "Cancel")).on_hover_text(lang.t("Dem Modell sagen, dass du nicht antworten möchtest", "Tell the model that you do not want to answer")).clicked() {
                reply = Some(AskReply::Cancel);
            }
            if !complete {
                let missing = (0..form.questions.len()).filter(|q| !form.answered(*q)).count();
                let text = if form.answers.iter().any(|a| a.other && a.other_text.trim().is_empty()) {
                    lang.t("„Andere …“ braucht einen Text.", "“Other …” needs a text.").to_string()
                } else if lang == Lang::De {
                    format!("Noch {missing} offen – bitte jede Frage beantworten.")
                } else {
                    format!("{missing} still open – please answer every question.")
                };
                ui.label(RichText::new(text).small().color(style::dim(ui)));
            }
        });
    });
    reply
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::ask::parse;
    use eframe::egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
    use serde_json::json;

    struct H {
        ctx: egui::Context,
        t: f64,
        shapes: Vec<egui::epaint::ClippedShape>,
    }

    impl H {
        fn new() -> H {
            H { ctx: egui::Context::default(), t: 0.0, shapes: Vec::new() }
        }
        fn frame(&mut self, form: &mut AskForm, lang: Lang, events: Vec<Event>) -> Option<AskReply> {
            self.t += 0.05;
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 900.0))), time: Some(self.t), events, ..Default::default() };
            let mut out = None;
            let full = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    out = ask_form(ui, 1, form, lang);
                });
            });
            self.shapes = full.shapes;
            out
        }
        fn texts(&self) -> Vec<String> {
            self.shapes.iter().filter_map(|s| if let egui::Shape::Text(t) = &s.shape { Some(t.galley.text().to_string()) } else { None }).collect()
        }
        fn pos_of(&self, text: &str) -> Pos2 {
            self.shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text() == text => Some(t.visual_bounding_rect().center()),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("„{text}“ fehlt: {:?}", self.texts()))
        }
        /// Text anklicken; Ergebnis des letzten Bilds.
        fn click(&mut self, form: &mut AskForm, lang: Lang, text: &str) -> Option<AskReply> {
            let pos = self.pos_of(text);
            let btn = |pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
            let mut got = self.frame(form, lang, vec![Event::PointerMoved(pos)]);
            for e in [btn(true), btn(false)] {
                got = got.or(self.frame(form, lang, vec![e]));
            }
            got.or(self.frame(form, lang, vec![]))
        }
    }

    fn form(v: serde_json::Value) -> AskForm {
        let (qs, fixes) = parse(&v).unwrap();
        AskForm::new("c1", qs, fixes, None)
    }

    fn two() -> AskForm {
        form(json!({"questions": [
            {"question": "Which database?", "header": "DB", "options": [{"label": "PostgreSQL", "description": "the server one"}, {"label": "SQLite", "recommended": true}]},
            {"question": "Which extras?", "multi_select": true, "options": ["Docker", "CI", {"label": "Docs", "recommended": true}]}
        ]}))
    }

    #[test]
    fn recommended_options_come_first_in_both_languages() {
        for (lang, rec, submit, cancel, other) in [(Lang::En, "(Recommended)", "Submit", "Cancel", "Other …"), (Lang::De, "(Empfohlen)", "Absenden", "Abbrechen", "Andere …")] {
            let mut f = two();
            let mut h = H::new();
            h.frame(&mut f, lang, vec![]);
            h.frame(&mut f, lang, vec![]);
            let t = h.texts();
            let sqlite = t.iter().position(|x| *x == format!("SQLite{rec}") || *x == format!("SQLite {rec}")).unwrap_or_else(|| panic!("{t:?}"));
            let pg = t.iter().position(|x| x == "PostgreSQL").unwrap();
            assert!(sqlite < pg, "empfohlene zuerst: {t:?}");
            assert!(t.iter().any(|x| x == "the server one"), "Beschreibung: {t:?}");
            assert!(t.iter().any(|x| x == "DB"), "Überschrift: {t:?}");
            for want in [submit, cancel, other] {
                assert!(t.iter().any(|x| x == want), "{want}: {t:?}");
            }
            assert_eq!(t.iter().filter(|x| *x == other).count(), 2, "„Andere“ je Frage");
        }
    }

    /// Text eines Knopfs mit Empfehlung, wie er gezeichnet wird.
    fn rec_text(h: &H, label: &str) -> String {
        h.texts().into_iter().find(|x| x.starts_with(label) && x.contains('(')).unwrap_or_else(|| panic!("{label}: {:?}", h.texts()))
    }

    #[test]
    fn submit_needs_every_answer_and_sends_the_choice() {
        let mut f = two();
        let mut h = H::new();
        h.frame(&mut f, Lang::En, vec![]);
        h.frame(&mut f, Lang::En, vec![]);
        assert!(h.texts().iter().any(|x| x.contains("2 still open")), "{:?}", h.texts());
        assert_eq!(h.click(&mut f, Lang::En, "Submit"), None, "unvollständig: Absenden gesperrt");
        let sqlite = rec_text(&h, "SQLite");
        assert_eq!(h.click(&mut f, Lang::En, &sqlite), None);
        assert_eq!(f.answers[0].selected, [true, false], "SQLite steht vorn");
        h.click(&mut f, Lang::En, "PostgreSQL");
        assert_eq!(f.answers[0].selected, [false, true], "Einfachauswahl");
        h.click(&mut f, Lang::En, "Docker");
        h.click(&mut f, Lang::En, "CI");
        assert!(f.complete());
        assert_eq!(h.click(&mut f, Lang::En, "Submit"), Some(AskReply::Submit));
        let r = f.result();
        assert_eq!(r["answers"][0]["selected"], json!(["PostgreSQL"]));
        assert_eq!(r["answers"][1]["selected"], json!(["Docker", "CI"]));
    }

    #[test]
    fn other_takes_free_text_and_cancel_always_works() {
        let mut f = form(json!({"questions": [{"question": "Name?", "options": ["Ada", "Bob"]}]}));
        let mut h = H::new();
        h.frame(&mut f, Lang::En, vec![]);
        h.frame(&mut f, Lang::En, vec![]);
        // Ins Freitextfeld tippen wählt „Andere“
        let id = other_id(1, "c1", 0);
        h.ctx.memory_mut(|m| m.request_focus(id));
        h.frame(&mut f, Lang::En, vec![]);
        h.frame(&mut f, Lang::En, vec![Event::Text("Carol".into())]);
        assert_eq!(f.answers[0].other_text, "Carol");
        assert!(f.answers[0].other && f.complete());
        // Eine Antwort wählen hebt „Andere“ auf (Einfachauswahl), „Andere“ anklicken wieder
        h.click(&mut f, Lang::En, "Ada");
        assert!(!f.answers[0].other && f.answers[0].selected[0]);
        h.click(&mut f, Lang::En, "Other …");
        assert!(f.answers[0].other && !f.answers[0].selected[0]);
        assert_eq!(h.click(&mut f, Lang::En, "Submit"), Some(AskReply::Submit));
        assert_eq!(f.result()["answers"][0]["other"], "Carol");
        // Abbrechen geht ohne Antwort
        let mut f = form(json!({"questions": [{"question": "Name?", "options": ["Ada", "Bob"]}]}));
        let mut h = H::new();
        h.frame(&mut f, Lang::De, vec![]);
        h.frame(&mut f, Lang::De, vec![]);
        assert_eq!(h.click(&mut f, Lang::De, "Abbrechen"), Some(AskReply::Cancel));
        // „Andere“ ohne Text: Hinweis
        let mut f = form(json!({"questions": [{"question": "Name?", "multi_select": true, "options": ["Ada", "Bob"]}]}));
        f.set_other(0, true);
        let mut h = H::new();
        h.frame(&mut f, Lang::De, vec![]);
        h.frame(&mut f, Lang::De, vec![]);
        assert!(h.texts().iter().any(|x| x == "„Andere …“ braucht einen Text."), "{:?}", h.texts());
    }
}
