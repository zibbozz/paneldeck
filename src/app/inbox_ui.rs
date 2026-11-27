//! Karte „Wartet auf dich“ in der Seitenleiste (#390): Claude-Sitzungen und Chats, die auf eine Freigabe oder Eingabe
//! warten.
//!
//! * **Öffnen** steht vorn in der Akzentfarbe, **Ja**/**Nein** folgen ohne Farbe – niemand soll freigeben, ohne
//!   hinzusehen. Den Tastaturfokus bekommt nie von selbst ein Knopf; Tab erreicht zuerst *Öffnen*.
//! * Darunter steht, **was verlangt wird**: das Werkzeug (Bash, Edit …) und der Befehl bzw. Pfad aus der Meldung von
//!   Claude (`hooks::ToolRequest`) bzw. aus dem wartenden Aufruf des Chats. Das ist fremder Text: als „aus der
//!   Sitzung“ gekennzeichnet, gedämpft in Festbreitenschrift, nie als Markup gelesen, auf [`REQUEST_ROWS`] Zeilen
//!   gekürzt und vollständig im Tooltip.
//! * „Ja“/„Nein“ sind kurz nach dem Erscheinen einer Frage gesperrt ([`SETTLE`]) – gegen Klicks, die eigentlich
//!   etwas anderem an derselben Stelle galten.

use super::*;
use crate::hooks::ToolRequest;
use egui::text::{LayoutJob, TextFormat, TextWrapping};

/// So viele Zeilen der Anfrage zeigt die Karte (der Rest steht im Tooltip).
pub(super) const REQUEST_ROWS: usize = 3;
/// So lange nach dem Erscheinen einer Frage bleiben „Ja“/„Nein“ gesperrt (wie [`Action::Approve`] im Hub).
pub(super) const SETTLE: Duration = Duration::from_millis(500);

/// Wie eine Frage beantwortet wird.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Answer {
    /// Claude im Terminal: Enter bzw. Esc ([`Action::Approve`]).
    Terminal,
    /// Chat: Entscheidung zu genau diesem wartenden Aufruf ([`Action::ChatDecide`]).
    Chat(String),
    /// Keine Ja/Nein-Frage (Eingabe, Fragen des Modells, Wahl der Stufe): nur *Öffnen*.
    Open,
}

/// Eine Zeile der Karte.
#[derive(Clone, Debug)]
pub(super) struct InboxItem {
    pub id: u64,
    pub name: String,
    /// Meldung (z. B. „Claude needs your permission to use Bash“) – fremder Text; gezeigt, wenn `request` fehlt.
    pub msg: String,
    pub request: Option<ToolRequest>,
    /// Woher die Anfrage stammt (Beschriftung neben dem Werkzeug): Claude-Sitzung oder Modell des Chats.
    pub from_model: bool,
    pub answer: Answer,
    /// Seit dem Erscheinen der Frage; `None` = unbekannt (gilt als lange genug).
    pub since: Option<Duration>,
}

/// Füllfarbe von *Öffnen*: die Akzentfarbe des Themes.
pub(super) fn open_fill() -> Color32 {
    style::brand().gamma_multiply(0.85)
}

/// Die Karte zeichnen; Klicks landen in `actions`.
pub(super) fn inbox_card(ui: &mut Ui, items: &[InboxItem], lang: Lang, actions: &mut Vec<Action>) {
    if items.is_empty() {
        return;
    }
    egui::Frame::new()
        .fill(style::brand().gamma_multiply(0.12))
        .stroke(Stroke::new(1.0_f32, style::brand().gamma_multiply(0.5)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(8))
        .show(ui, |ui| {
            ui.label(RichText::new(format!("{} ({})", lang.t("Wartet auf dich", "Waiting for you"), items.len())).strong().small());
            for it in items {
                ui.add_space(S1);
                ui.push_id(("inbox", it.id), |ui| item_ui(ui, it, lang, actions));
            }
        });
}

fn item_ui(ui: &mut Ui, it: &InboxItem, lang: Lang, actions: &mut Vec<Action>) {
    ui.label(RichText::new(&it.name).strong());
    match &it.request {
        Some(r) => request_ui(ui, r, it.from_model, lang),
        None if !it.msg.is_empty() => {
            clipped(ui, &crate::hooks::untrusted_text(&it.msg, crate::hooks::DETAIL_MAX), false);
        }
        None => {}
    }
    ui.horizontal_wrapped(|ui| {
        // Zuerst ansehen: *Öffnen* vorn und farbig, die Antworten danach ohne Farbe.
        let open = egui::Button::new(RichText::new(lang.t("Öffnen", "Open")).strong()).fill(open_fill());
        if ui.add(open).on_hover_text(lang.t("Sitzung anzeigen, um die Frage zu prüfen", "Show the session to check the request")).clicked() {
            actions.push(Action::Activate(it.id));
        }
        if it.answer == Answer::Open {
            return;
        }
        let settled = it.since.is_none_or(|d| d >= SETTLE);
        if let Some(d) = it.since.filter(|_| !settled) {
            ui.ctx().request_repaint_after(SETTLE - d);
        }
        let (yes_tip, no_tip) = match it.answer {
            Answer::Chat(_) => (lang.t("Erlaubt diesen Aufruf einmal", "Allows this call once"), lang.t("Lehnt diesen Aufruf ab", "Denies this call")),
            _ => (lang.t("Sendet Enter (Standardauswahl „Ja“)", "Sends Enter (default choice “Yes”)"), lang.t("Sendet Esc", "Sends Esc")),
        };
        let wait = lang.t("Gleich – die Frage ist gerade erst erschienen.", "One moment – the request just appeared.");
        let yes = ui.add_enabled(settled, egui::Button::new(lang.t("Ja", "Yes"))).on_hover_text(yes_tip).on_disabled_hover_text(wait);
        let no = ui.add_enabled(settled, egui::Button::new(lang.t("Nein", "No"))).on_hover_text(no_tip).on_disabled_hover_text(wait);
        for (r, ok) in [(yes, true), (no, false)] {
            if !r.clicked() {
                continue;
            }
            actions.push(match &it.answer {
                Answer::Chat(call) => Action::ChatDecide(it.id, call.clone(), if ok { chatmod::Decision::Allow } else { chatmod::Decision::Deny }),
                _ => Action::Approve(it.id, ok),
            });
        }
    });
}

/// Werkzeug, Herkunft und Kurzfassung der Anfrage.
fn request_ui(ui: &mut Ui, r: &ToolRequest, from_model: bool, lang: Lang) {
    let dim = style::dim(ui);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let tool = crate::hooks::untrusted_text(&r.tool, 200);
        ui.add(egui::Label::new(RichText::new(tool).monospace().strong().size(12.0)).wrap());
        let from = if from_model { lang.t("vom Modell", "from the model") } else { lang.t("aus der Sitzung", "from the session") };
        let tip = if from_model {
            lang.t("Angaben aus dem Werkzeugaufruf des Modells – nicht vertrauenswürdig. Vollständig im Chat.", "Taken from the model's tool call – untrusted. In full in the chat.")
        } else {
            lang.t("Von Claude gemeldet – nicht vertrauenswürdig. Im Terminal prüfen, bevor du freigibst.", "Reported by Claude – untrusted. Check it in the terminal before you approve.")
        };
        ui.label(RichText::new(from).small().italics().color(dim)).on_hover_text(tip);
    });
    if !r.detail.is_empty() {
        clipped(ui, &r.detail, true);
    }
    if !r.others.is_empty() {
        let n = r.others.len();
        let text = match lang {
            Lang::De => format!("+{n} weitere offene Aufrufe dieses Werkzeugs – die Frage kann auch einem davon gelten"),
            Lang::En => format!("+{n} more open calls of this tool – the request may be for one of them"),
        };
        let all = r.others.join("\n\n");
        ui.add(egui::Label::new(RichText::new(text).small().color(style::ui().warning)).wrap()).on_hover_ui(|ui| {
            ui.set_max_width(460.0);
            ui.label(RichText::new(all).monospace().small());
        });
    }
}

/// Fremden Text gekürzt auf [`REQUEST_ROWS`] Zeilen zeigen (gedämpft; `mono` = Festbreitenschrift auf eigenem
/// Hintergrund), vollständig im Tooltip. Nie als Markup: der Text wird als einfacher Abschnitt gesetzt.
fn clipped(ui: &mut Ui, text: &str, mono: bool) -> egui::Response {
    let dim = style::dim(ui);
    let font = if mono { FontId::monospace(11.5) } else { FontId::proportional(11.5) };
    let width = (ui.available_width() - if mono { 12.0 } else { 0.0 }).max(40.0);
    let mut job = LayoutJob::single_section(text.to_string(), TextFormat { font_id: font, color: dim, ..TextFormat::default() });
    job.wrap = TextWrapping { max_width: width, max_rows: REQUEST_ROWS, break_anywhere: true, overflow_character: Some('…') };
    let galley = ui.painter().layout_job(job);
    let label = |ui: &mut Ui| ui.add(egui::Label::new(galley).sense(Sense::hover()));
    let r = if mono {
        egui::Frame::new().fill(ui.visuals().extreme_bg_color).corner_radius(CornerRadius::same(4)).inner_margin(Margin::symmetric(6, 3)).show(ui, label).inner
    } else {
        label(ui)
    };
    let full = text.to_string();
    r.on_hover_ui(move |ui| {
        ui.set_max_width(460.0);
        ui.label(if mono { RichText::new(full).monospace().small() } else { RichText::new(full).small() });
    })
}

impl App {
    /// Karte „Wartet auf dich“ (Seitenleiste): Claude-Sitzungen mit offener Rückfrage und Chats mit offener Freigabe,
    /// Frage oder Wahl der Stufe.
    pub(super) fn inbox_ui(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let lang = self.settings.language;
        let now = ui.input(|i| i.time);
        let mut items: Vec<InboxItem> = Vec::new();
        for (id, e) in &self.entries {
            if !e.needs_you() {
                continue;
            }
            if let Some(c) = &e.chat {
                let mut it = InboxItem { id: *id, name: e.display_name(), msg: String::new(), request: None, from_model: true, answer: Answer::Open, since: None };
                if let Some(call) = c.pending_approval() {
                    let tool = match &c.approval_target {
                        Some(t) => format!("{} → {t}", call.name),
                        None => call.name.clone(),
                    };
                    it.request = Some(ToolRequest { tool, detail: crate::hooks::tool_detail(&call.arguments).unwrap_or_default(), others: Vec::new() });
                    // Erstes Erscheinen genau dieses Aufrufs (für die Sperre gegen Fehlklicks).
                    let seen = ui.ctx().data_mut(|d| *d.get_temp_mut_or_insert_with(Id::new(("inbox_seen", *id, call.id.clone())), || now));
                    it.since = Some(Duration::from_secs_f64((now - seen).max(0.0)));
                    it.answer = Answer::Chat(call.id.clone());
                } else if c.awaiting_answer() {
                    it.msg = lang.t("Das Modell hat Fragen an dich.", "The model has questions for you.").to_string();
                } else {
                    it.msg = lang.t("Wähle, was dieses Modell in Paneldeck darf.", "Choose what this model may do in Paneldeck.").to_string();
                }
                items.push(it);
            } else if e.is_claude() {
                let permission = e.status == ClaudeStatus::Permission;
                items.push(InboxItem {
                    id: *id,
                    name: e.display_name(),
                    msg: e.status_msg.clone(),
                    request: e.perm_request().cloned(),
                    from_model: false,
                    answer: if permission { Answer::Terminal } else { Answer::Open },
                    since: e.perm_since.map(|t| t.elapsed()),
                });
            }
        }
        if items.is_empty() {
            return;
        }
        inbox_card(ui, &items, lang, actions);
        ui.add_space(S2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, PointerButton, RawInput, Vec2};

    struct H {
        ctx: egui::Context,
        t: f64,
        shapes: Vec<egui::epaint::ClippedShape>,
    }

    impl H {
        fn new() -> H {
            H { ctx: egui::Context::default(), t: 0.0, shapes: Vec::new() }
        }
        fn frame(&mut self, items: &[InboxItem], lang: Lang, events: Vec<Event>) -> Vec<Action> {
            self.t += 0.6;
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(280.0, 700.0))), time: Some(self.t), events, ..Default::default() };
            let mut actions = Vec::new();
            let full = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| inbox_card(ui, items, lang, &mut actions));
            });
            self.shapes = full.shapes;
            actions
        }
        fn texts(&self) -> Vec<String> {
            self.galleys().into_iter().map(|g| g.text().to_string()).collect()
        }
        fn galleys(&self) -> Vec<std::sync::Arc<egui::Galley>> {
            self.shapes.iter().filter_map(|s| if let egui::Shape::Text(t) = &s.shape { Some(t.galley.clone()) } else { None }).collect()
        }
        fn rect_of(&self, text: &str) -> Option<Rect> {
            self.shapes.iter().find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == text => Some(t.visual_bounding_rect()),
                _ => None,
            })
        }
        /// Füllfarbe des Knopfes mit dieser Beschriftung (das kleinste gefüllte Rechteck um den Text).
        fn fill_of(&self, text: &str) -> Option<Color32> {
            let r = self.rect_of(text)?;
            self.shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Rect(rs) if rs.rect.contains_rect(r) && rs.rect.width() < 200.0 => Some((rs.rect.area(), rs.fill)),
                    _ => None,
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, f)| f)
        }
        fn click(&mut self, items: &[InboxItem], lang: Lang, text: &str) -> Vec<Action> {
            let p = self.rect_of(text).unwrap_or_else(|| panic!("{text} fehlt: {:?}", self.texts())).center();
            let btn = |pressed| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
            let mut out = self.frame(items, lang, vec![Event::PointerMoved(p)]);
            for e in [btn(true), btn(false)] {
                out.extend(self.frame(items, lang, vec![e]));
            }
            out
        }
        fn focused(&self) -> Option<Id> {
            self.ctx.memory(|m| m.focused())
        }
    }

    fn bash(detail: &str) -> InboxItem {
        InboxItem {
            id: 7,
            name: "api-server".into(),
            msg: "Claude needs your permission to use Bash".into(),
            request: Some(ToolRequest { tool: "Bash".into(), detail: detail.into(), others: Vec::new() }),
            from_model: false,
            answer: Answer::Terminal,
            since: None,
        }
    }

    fn key(k: Key) -> Event {
        Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }
    }

    /// Reihenfolge Öffnen – Ja – Nein, nur Öffnen in der Akzentfarbe, beide Sprachen.
    #[test]
    fn open_comes_first_in_the_accent_colour_and_yes_no_are_plain() {
        for (lang, open, yes, no) in [(Lang::En, "Open", "Yes", "No"), (Lang::De, "Öffnen", "Ja", "Nein")] {
            let items = [bash("cargo test --release")];
            let mut h = H::new();
            h.frame(&items, lang, vec![]);
            h.frame(&items, lang, vec![]);
            let (o, y, n) = (h.rect_of(open).expect("Öffnen"), h.rect_of(yes).expect("Ja"), h.rect_of(no).expect("Nein"));
            assert!(o.max.x < y.min.x && y.max.x < n.min.x, "Reihenfolge {o:?} {y:?} {n:?}");
            assert_eq!(h.fill_of(open), Some(open_fill()), "Öffnen in der Akzentfarbe");
            let plain = h.ctx.style().visuals.widgets.inactive.weak_bg_fill;
            assert_eq!(h.fill_of(yes), Some(plain), "Ja ohne eigene Farbe");
            assert_eq!(h.fill_of(no), Some(plain), "Nein ohne eigene Farbe");
            assert_ne!(open_fill(), plain);
            assert!(!h.texts().iter().any(|t| t.contains("Standardauswahl") || t.contains("default choice")), "Tooltip erst beim Zeigen");
        }
    }

    /// Kein Knopf hat von selbst den Fokus, Enter gibt nichts frei, und Tab erreicht zuerst *Öffnen*.
    #[test]
    fn keyboard_never_lands_on_yes_first() {
        let items = [bash("rm -rf target")];
        let mut h = H::new();
        h.frame(&items, Lang::En, vec![]);
        assert_eq!(h.focused(), None, "kein Fokus von selbst");
        assert!(h.frame(&items, Lang::En, vec![key(Key::Enter)]).is_empty(), "Enter ohne Fokus tut nichts");
        h.frame(&items, Lang::En, vec![key(Key::Tab)]);
        h.frame(&items, Lang::En, vec![]);
        let first = h.focused().expect("Tab fokussiert einen Knopf");
        // Enter auf dem fokussierten Knopf: *Öffnen*, nicht *Ja*.
        let acts = h.frame(&items, Lang::En, vec![key(Key::Enter)]);
        assert!(matches!(acts.as_slice(), [Action::Activate(7)]), "erster Tab-Halt ist Öffnen ({first:?})");
    }

    #[test]
    fn buttons_send_activate_and_approve_for_terminals() {
        let items = [bash("ls")];
        for (label, want) in [("Open", "activate"), ("Yes", "yes"), ("No", "no")] {
            let mut h = H::new();
            h.frame(&items, Lang::En, vec![]);
            let acts = h.click(&items, Lang::En, label);
            let ok = match want {
                "activate" => matches!(acts.as_slice(), [Action::Activate(7)]),
                "yes" => matches!(acts.as_slice(), [Action::Approve(7, true)]),
                _ => matches!(acts.as_slice(), [Action::Approve(7, false)]),
            };
            assert!(ok, "{label}: {}", acts.len());
        }
    }

    /// Werkzeug und Befehl stehen in der Karte, als fremder Text gekennzeichnet; ein langer Befehl ist auf drei Zeilen
    /// gekürzt und steht vollständig im Tooltip.
    #[test]
    fn the_request_is_shown_marked_truncated_and_complete_in_the_tooltip() {
        let long = format!("{} && rm -rf C:\\important", "echo step && ".repeat(60));
        let items = [bash(&long)];
        for (lang, from) in [(Lang::En, "from the session"), (Lang::De, "aus der Sitzung")] {
            let mut h = H::new();
            h.frame(&items, lang, vec![]);
            h.frame(&items, lang, vec![]);
            let texts = h.texts();
            assert!(texts.iter().any(|t| t == "Bash"), "Werkzeug: {texts:?}");
            assert!(texts.iter().any(|t| t == from), "Herkunft: {texts:?}");
            assert!(!texts.iter().any(|t| t.starts_with("Claude needs")), "die Meldung tritt hinter die Anfrage zurück");
            let g = h.galleys().into_iter().find(|g| g.text() == long).expect("Anfrage gezeichnet");
            assert!(g.elided && g.rows.len() <= REQUEST_ROWS, "gekürzt: {} Zeilen", g.rows.len());
            assert!(g.rows.len() >= 2, "mehr als eine Zeile");
            // Tooltip mit dem ganzen Text
            let p = h.rect_of(&long).unwrap().center();
            h.frame(&items, lang, vec![Event::PointerMoved(p)]);
            for _ in 0..3 {
                h.frame(&items, lang, vec![]);
            }
            let full: Vec<_> = h.galleys().into_iter().filter(|g| g.text() == long).collect();
            assert!(full.iter().any(|g| !g.elided), "Tooltip zeigt alles: {}", full.len());
        }
    }

    /// Markup bleibt Text, und ohne zuzuordnende Anfrage zeigt die Karte die Meldung.
    #[test]
    fn markup_is_plain_text_and_the_message_is_the_fallback() {
        let items = [bash("<b>**rm**</b> [x](http://evil)")];
        let mut h = H::new();
        h.frame(&items, Lang::En, vec![]);
        assert!(h.texts().iter().any(|t| t == "<b>**rm**</b> [x](http://evil)"), "{:?}", h.texts());
        let items = [InboxItem { request: None, ..bash("") }];
        h.frame(&items, Lang::En, vec![]);
        assert!(h.texts().iter().any(|t| t == "Claude needs your permission to use Bash"), "{:?}", h.texts());
    }

    /// Kurz nach dem Erscheinen sind Ja/Nein gesperrt (Fehlklick), Öffnen nicht.
    #[test]
    fn yes_and_no_wait_a_moment_after_the_request_appears() {
        let items = [InboxItem { since: Some(Duration::from_millis(50)), ..bash("ls") }];
        let mut h = H::new();
        h.frame(&items, Lang::En, vec![]);
        assert!(h.click(&items, Lang::En, "Yes").is_empty());
        assert!(h.click(&items, Lang::En, "No").is_empty());
        assert!(matches!(h.click(&items, Lang::En, "Open").as_slice(), [Action::Activate(7)]));
        let items = [InboxItem { since: Some(SETTLE), ..bash("ls") }];
        assert!(matches!(h.click(&items, Lang::En, "Yes").as_slice(), [Action::Approve(7, true)]));
    }

    /// Chat-Freigabe in der Karte: Werkzeug mit Ziel, Kurzfassung der Argumente, „vom Modell“; Ja/Nein entscheiden genau
    /// diesen Aufruf. Fragen des Modells haben nur *Öffnen*.
    #[test]
    fn chat_approvals_show_tool_and_arguments_and_decide_that_call() {
        let args = serde_json::json!({"session": 4, "text": "dir /s"});
        let chat = InboxItem {
            id: 9,
            name: "Chat".into(),
            msg: String::new(),
            request: Some(ToolRequest { tool: "send_text → session 4 \"build\"".into(), detail: crate::hooks::tool_detail(&args).unwrap(), others: Vec::new() }),
            from_model: true,
            answer: Answer::Chat("call-1".into()),
            since: None,
        };
        for (lang, from, yes, no) in [(Lang::En, "from the model", "Yes", "No"), (Lang::De, "vom Modell", "Ja", "Nein")] {
            let items = [chat.clone()];
            let mut h = H::new();
            h.frame(&items, lang, vec![]);
            let texts = h.texts();
            for want in ["send_text → session 4 \"build\"", "dir /s", from] {
                assert!(texts.iter().any(|t| t == want), "{want}: {texts:?}");
            }
            let acts = h.click(&items, lang, yes);
            assert!(matches!(acts.as_slice(), [Action::ChatDecide(9, c, chatmod::Decision::Allow)] if c == "call-1"));
            let acts = h.click(&items, lang, no);
            assert!(matches!(acts.as_slice(), [Action::ChatDecide(9, c, chatmod::Decision::Deny)] if c == "call-1"));
        }
        let ask = InboxItem { request: None, msg: "The model has questions for you.".into(), answer: Answer::Open, ..chat };
        let mut h = H::new();
        h.frame(&[ask.clone()], Lang::En, vec![]);
        let texts = h.texts();
        assert!(texts.iter().any(|t| t == "Open") && !texts.iter().any(|t| t == "Yes" || t == "No"), "{texts:?}");
    }

    /// Weitere offene Aufrufe desselben Werkzeugs werden genannt (die Frage kann einem davon gelten).
    #[test]
    fn other_open_calls_of_the_tool_are_mentioned() {
        let items = [InboxItem { request: Some(ToolRequest { tool: "Bash".into(), detail: "rm -rf x".into(), others: vec!["npm test".into()] }), ..bash("") }];
        let mut h = H::new();
        h.frame(&items, Lang::En, vec![]);
        assert!(h.texts().iter().any(|t| t.starts_with("+1 more open calls of this tool")), "{:?}", h.texts());
        h.frame(&items, Lang::De, vec![]);
        assert!(h.texts().iter().any(|t| t.starts_with("+1 weitere offene Aufrufe")), "{:?}", h.texts());
    }
}
