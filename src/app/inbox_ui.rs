//! Karte „Wartet auf dich“ in der Seitenleiste (#390): Claude-Sitzungen und Chats, die auf eine Freigabe oder Eingabe
//! warten.
//!
//! * **Öffnen** steht vorn in der Akzentfarbe, **Ja**/**Nein** folgen ohne Farbe – niemand soll freigeben, ohne
//!   hinzusehen. Den Tastaturfokus bekommt nie von selbst ein Knopf; Tab erreicht zuerst *Öffnen*.
//! * Darunter steht, **was verlangt wird**: das Werkzeug (Bash, Edit …) und der Befehl bzw. Pfad aus der Meldung von
//!   Claude (`hooks::ToolRequest`) bzw. aus dem wartenden Aufruf des Chats. Das ist fremder Text: als „aus der
//!   Sitzung“ gekennzeichnet, gedämpft in Festbreitenschrift, nie als Markup gelesen, Leerraum-Polster
//!   zusammengezogen, Zeilenumbrüche als `⏎`; was nicht in [`REQUEST_ROWS`] Zeilen passt, zeigt Anfang **und** Ende
//!   mit der Zahl der verborgenen Zeichen, vollständig im Tooltip.
//! * **Nie eine Anfrage zeigen, die vielleicht nicht die gefragte ist:** Ja/Nein gibt es nur, wenn genau ein Aufruf in
//!   Frage kommt, seine Kurzfassung im Dialog auf dem Bildschirm steht (`ToolRequest::confirmed`) und sie ganz in die
//!   Karte passt. Sonst zeigt die Karte die Unsicherheit (mehrere Kandidaten gleichrangig, „vermutlich“, gekürzt, nur die
//!   Meldung) und nur *Öffnen*.
//! * „Ja“/„Nein“ sind kurz nach dem Erscheinen einer Frage gesperrt ([`SETTLE`]) – gegen Klicks, die eigentlich
//!   etwas anderem an derselben Stelle galten. Der Hub prüft beim Klick noch einmal ([`Action::Approve`]).

use super::*;
use crate::hooks::{ToolCall, ToolRequest};
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use std::sync::Arc;

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
    /// Meldung (z. B. „Claude needs your permission to use Bash“) – fremder Text; gezeigt, wenn keine Anfrage feststeht.
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

/// Was die Karte zur Anfrage zeigen kann.
enum Shown<'a> {
    /// Keine Anfrage (oder eine ohne Inhalt): die Meldung.
    Message,
    /// Mehrere Aufrufe kommen in Frage – gleichrangig.
    Several(&'a ToolRequest),
    /// Genau einer; `sure` = auf dem Bildschirm bestätigt.
    One(&'a ToolRequest, &'a ToolCall, bool),
}

fn shown(r: Option<&ToolRequest>) -> Shown<'_> {
    match r {
        Some(r) if r.calls.len() > 1 => Shown::Several(r),
        Some(r) => match r.single().filter(|c| !c.detail.is_empty()) {
            Some(c) => Shown::One(r, c, r.confirmed),
            None => Shown::Message,
        },
        None => Shown::Message,
    }
}

fn item_ui(ui: &mut Ui, it: &InboxItem, lang: Lang, actions: &mut Vec<Action>) {
    ui.label(RichText::new(&it.name).strong());
    // Ja/Nein nur für genau einen, bestätigten, ganz sichtbaren Aufruf.
    let mut answer_seq: Option<u64> = None;
    match shown(it.request.as_ref()) {
        Shown::Message => {
            if !it.msg.is_empty() {
                clipped(ui, &crate::hooks::untrusted_text(&it.msg, crate::hooks::DETAIL_MAX), false);
            }
        }
        Shown::Several(r) => {
            source_row(ui, &r.tool, it.from_model, None, lang);
            let n = r.calls.len();
            let text = match lang {
                Lang::De => format!("Einer von {n} Aufrufen – welcher, ist nicht sicher. Im Terminal prüfen."),
                Lang::En => format!("One of {n} calls – it is not certain which. Check it in the terminal."),
            };
            ui.add(egui::Label::new(RichText::new(text).small().color(style::ui().warning)).wrap());
            for c in &r.calls {
                one_row(ui, &c.detail);
            }
        }
        Shown::One(r, c, sure) => {
            let note = if sure { lang.t("stimmt mit dem Terminal überein", "matches the terminal") } else { lang.t("vermutlich – im Terminal noch nicht gesehen", "probably – not seen in the terminal yet") };
            source_row(ui, &r.tool, it.from_model, Some((note, sure)), lang);
            let fits = detail_view(ui, &c.detail, lang);
            if sure && fits {
                answer_seq = Some(c.seq);
            }
        }
    }
    ui.horizontal_wrapped(|ui| {
        // Zuerst ansehen: *Öffnen* vorn und farbig, die Antworten danach ohne Farbe.
        let open = egui::Button::new(RichText::new(lang.t("Öffnen", "Open")).strong()).fill(open_fill());
        if ui.add(open).on_hover_text(lang.t("Sitzung anzeigen, um die Frage zu prüfen", "Show the session to check the request")).clicked() {
            actions.push(Action::Activate(it.id));
        }
        let Some(seq) = answer_seq.filter(|_| it.answer != Answer::Open) else { return };
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
                _ => Action::Approve(it.id, ok, seq),
            });
        }
    });
}

/// Werkzeug, Herkunft („aus der Sitzung“/„vom Modell“) und ggf. der Stand der Bestätigung.
fn source_row(ui: &mut Ui, tool: &str, from_model: bool, note: Option<(&str, bool)>, lang: Lang) {
    let dim = style::dim(ui);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let tool = crate::hooks::untrusted_text(tool, 200);
        ui.add(egui::Label::new(RichText::new(tool).monospace().strong().size(12.0)).wrap());
        let from = if from_model { lang.t("vom Modell", "from the model") } else { lang.t("aus der Sitzung", "from the session") };
        let tip = if from_model {
            lang.t("Angaben aus dem Werkzeugaufruf des Modells – nicht vertrauenswürdig. Vollständig im Chat.", "Taken from the model's tool call – untrusted. In full in the chat.")
        } else {
            lang.t("Von Claude gemeldet – nicht vertrauenswürdig. Im Terminal prüfen, bevor du freigibst.", "Reported by Claude – untrusted. Check it in the terminal before you approve.")
        };
        ui.label(RichText::new(from).small().italics().color(dim)).on_hover_text(tip);
        if let Some((note, sure)) = note.filter(|_| !from_model) {
            let col = if sure { dim } else { style::ui().warning };
            ui.label(RichText::new(note).small().color(col));
        }
    });
}

fn text_job(text: &str, font: FontId, color: Color32, width: f32, rows: usize) -> LayoutJob {
    let mut job = LayoutJob::single_section(text.to_string(), TextFormat { font_id: font, color, ..TextFormat::default() });
    job.wrap = TextWrapping { max_width: width, max_rows: rows, break_anywhere: true, overflow_character: Some('…') };
    job
}

fn mono() -> FontId {
    FontId::monospace(11.5)
}

/// Gedämpfter Kasten für fremden Text; der ganze Text im Tooltip.
fn untrusted_box(ui: &mut Ui, full: &str, add: impl FnOnce(&mut Ui)) {
    let r = egui::Frame::new().fill(ui.visuals().extreme_bg_color).corner_radius(CornerRadius::same(4)).inner_margin(Margin::symmetric(6, 3)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        add(ui);
    });
    let full = full.to_string();
    r.response.interact(Sense::hover()).on_hover_ui(move |ui| {
        ui.set_max_width(460.0);
        ui.label(RichText::new(full).monospace().small());
    });
}

/// Kurzfassung eines Aufrufs: zusammengezogen ([`crate::hooks::compact_text`]); passt sie nicht in [`REQUEST_ROWS`]
/// Zeilen, Anfang und Ende mit der Zahl der verborgenen Zeichen. `true`, wenn alles zu sehen ist.
fn detail_view(ui: &mut Ui, detail: &str, lang: Lang) -> bool {
    let compact = crate::hooks::compact_text(detail);
    let dim = style::dim(ui);
    let width = (ui.available_width() - 14.0).max(40.0);
    let full = ui.painter().layout_job(text_job(&compact, mono(), dim, width, REQUEST_ROWS));
    if !full.elided {
        untrusted_box(ui, detail, |ui| {
            ui.add(egui::Label::new(full).sense(Sense::hover()));
        });
        return true;
    }
    let head = ui.painter().layout_job(text_job(&compact, mono(), dim, width, REQUEST_ROWS - 1));
    let shown_head: usize = head.rows.iter().map(|r| r.char_count_including_newline()).sum::<usize>().saturating_sub(1);
    let tail = tail_fit(ui, &compact, width, dim);
    let total = compact.chars().count();
    let hidden = total.saturating_sub(shown_head + tail.1);
    untrusted_box(ui, detail, |ui| {
        ui.add(egui::Label::new(head).sense(Sense::hover()));
        ui.add(egui::Label::new(tail.0).sense(Sense::hover()));
    });
    let text = match lang {
        Lang::De => format!("{hidden} Zeichen verborgen – im Terminal prüfen"),
        Lang::En => format!("{hidden} characters hidden – check the terminal"),
    };
    ui.add(egui::Label::new(RichText::new(text).small().color(style::ui().warning)).wrap());
    false
}

/// Das Ende von `text`, so lang, wie es mit vorangestelltem `…` in eine Zeile passt; dazu die Zahl der Zeichen.
fn tail_fit(ui: &Ui, text: &str, width: f32, color: Color32) -> (Arc<egui::Galley>, usize) {
    let chars: Vec<char> = text.chars().collect();
    let lay = |k: usize| {
        let s: String = std::iter::once('…').chain(chars[chars.len() - k..].iter().copied()).collect();
        ui.painter().layout_no_wrap(s, mono(), color)
    };
    let (mut lo, mut hi) = (1usize, chars.len().min(400));
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if lay(mid).size().x <= width {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    (lay(lo), lo)
}

/// Ein Kandidat unter mehreren: eine Zeile, vollständig im Tooltip.
fn one_row(ui: &mut Ui, detail: &str) {
    let dim = style::dim(ui);
    let width = (ui.available_width() - 14.0).max(40.0);
    let g = ui.painter().layout_job(text_job(&crate::hooks::compact_text(detail), mono(), dim, width, 1));
    untrusted_box(ui, detail, |ui| {
        ui.add(egui::Label::new(g).sense(Sense::hover()));
    });
}

/// Fremden Text gekürzt auf [`REQUEST_ROWS`] Zeilen zeigen (gedämpft; `mono` = Festbreitenschrift auf eigenem
/// Hintergrund), vollständig im Tooltip. Nie als Markup: der Text wird als einfacher Abschnitt gesetzt.
fn clipped(ui: &mut Ui, text: &str, mono_font: bool) -> egui::Response {
    let dim = style::dim(ui);
    let font = if mono_font { mono() } else { FontId::proportional(11.5) };
    let width = ui.available_width().max(40.0);
    let galley = ui.painter().layout_job(text_job(text, font, dim, width, REQUEST_ROWS));
    let full = text.to_string();
    ui.add(egui::Label::new(galley).sense(Sense::hover())).on_hover_ui(move |ui| {
        ui.set_max_width(460.0);
        ui.label(RichText::new(full).small());
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
                    // Der wartende Aufruf ist bekannt: sicher (gezeigt werden alle Argumente).
                    let detail = crate::hooks::tool_detail(&call.name, &call.arguments).unwrap_or_default();
                    it.request = Some(ToolRequest { tool, calls: vec![ToolCall { seq: 0, id: Some(call.id.clone()), detail }], confirmed: true });
                    // Erstes Erscheinen genau dieses Aufrufs (für die Sperre gegen Fehlklicks).
                    let seen = ui.ctx().data_mut(|d| *d.get_temp_mut_or_insert_with(Id::new(("inbox_seen", *id, call.id.clone())), || now));
                    it.since = Some(Duration::from_secs_f64((now - seen).max(0.0)));
                    it.answer = Answer::Chat(call.id.clone());
                    it.msg = lang.t("Das Modell möchte ein Werkzeug nutzen.", "The model wants to use a tool.").to_string();
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
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(280.0, 900.0))), time: Some(self.t), events, ..Default::default() };
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
        fn has(&self, text: &str) -> bool {
            self.texts().iter().any(|t| t == text)
        }
        fn galleys(&self) -> Vec<Arc<egui::Galley>> {
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
        /// Ja/Nein vorhanden?
        fn answers(&self) -> bool {
            self.has("Yes") || self.has("No") || self.has("Ja") || self.has("Nein")
        }
    }

    fn call(seq: u64, detail: &str) -> ToolCall {
        ToolCall { seq, id: Some(format!("toolu_{seq}")), detail: detail.into() }
    }

    /// Eine Claude-Frage mit genau einem, auf dem Bildschirm bestätigten Aufruf.
    fn bash(detail: &str) -> InboxItem {
        InboxItem {
            id: 7,
            name: "api-server".into(),
            msg: "Claude needs your permission to use Bash".into(),
            request: Some(ToolRequest { tool: "Bash".into(), calls: vec![call(3, detail)], confirmed: true }),
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
        for (lang, open, yes, no, sure) in [(Lang::En, "Open", "Yes", "No", "matches the terminal"), (Lang::De, "Öffnen", "Ja", "Nein", "stimmt mit dem Terminal überein")] {
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
            assert!(h.has(sure), "{:?}", h.texts());
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
        let acts = h.frame(&items, Lang::En, vec![key(Key::Enter)]);
        assert!(matches!(acts.as_slice(), [Action::Activate(7)]), "erster Tab-Halt ist Öffnen ({first:?})");
    }

    /// Ja/Nein tragen die Nummer des gezeigten Aufrufs (der Hub prüft sie beim Klick).
    #[test]
    fn buttons_send_activate_and_approve_for_the_shown_call() {
        let items = [bash("ls")];
        for (label, want) in [("Open", "activate"), ("Yes", "yes"), ("No", "no")] {
            let mut h = H::new();
            h.frame(&items, Lang::En, vec![]);
            let acts = h.click(&items, Lang::En, label);
            let ok = match want {
                "activate" => matches!(acts.as_slice(), [Action::Activate(7)]),
                "yes" => matches!(acts.as_slice(), [Action::Approve(7, true, 3)]),
                _ => matches!(acts.as_slice(), [Action::Approve(7, false, 3)]),
            };
            assert!(ok, "{label}: {}", acts.len());
        }
    }

    /// (a) Mehrere Kandidaten: alle gleichrangig, Hinweis auf die Unsicherheit, nur *Öffnen*.
    #[test]
    fn several_candidates_are_shown_as_equals_with_open_only() {
        let r = ToolRequest { tool: "Bash".into(), calls: vec![call(1, "npm run dev"), call(2, "rm -rf build")], confirmed: false };
        let items = [InboxItem { request: Some(r), ..bash("") }];
        for (lang, hint) in [(Lang::En, "One of 2 calls"), (Lang::De, "Einer von 2 Aufrufen")] {
            let mut h = H::new();
            h.frame(&items, lang, vec![]);
            assert!(h.has("npm run dev") && h.has("rm -rf build"), "{:?}", h.texts());
            assert!(h.texts().iter().any(|t| t.starts_with(hint)), "{:?}", h.texts());
            let (a, b) = (h.rect_of("npm run dev").unwrap(), h.rect_of("rm -rf build").unwrap());
            let size = |t: &str| h.galleys().into_iter().find(|g| g.text() == t).unwrap().size();
            assert!((size("npm run dev").y - size("rm -rf build").y).abs() < 0.5 && a.min.y < b.min.y, "gleichrangig, älterer zuerst");
            assert!(!h.answers(), "nur Öffnen");
        }
    }

    /// Ein Kandidat, den der Bildschirm (noch) nicht bestätigt: „vermutlich“, nur *Öffnen*.
    #[test]
    fn an_unconfirmed_candidate_is_marked_probable_with_open_only() {
        let items = [InboxItem { request: Some(ToolRequest { confirmed: false, ..bash("ls").request.unwrap() }), ..bash("") }];
        for (lang, note) in [(Lang::En, "probably – not seen in the terminal yet"), (Lang::De, "vermutlich – im Terminal noch nicht gesehen")] {
            let mut h = H::new();
            h.frame(&items, lang, vec![]);
            assert!(h.has("ls") && h.has(note), "{:?}", h.texts());
            assert!(!h.answers());
        }
    }

    /// Ohne Anfrage oder mit leerer Kurzfassung: die Meldung, nur *Öffnen*.
    #[test]
    fn without_a_usable_request_the_message_is_shown_with_open_only() {
        for request in [None, Some(ToolRequest { tool: "Bash".into(), calls: vec![call(1, "")], confirmed: true })] {
            let items = [InboxItem { request, ..bash("") }];
            let mut h = H::new();
            h.frame(&items, Lang::En, vec![]);
            assert!(h.has("Claude needs your permission to use Bash"), "{:?}", h.texts());
            assert!(h.has("Open") && !h.answers());
        }
    }

    /// Lange Anfrage: Anfang und Ende sichtbar, verborgene Zeichen gezählt, ganz im Tooltip – und nur *Öffnen*.
    #[test]
    fn a_long_request_shows_head_and_tail_and_offers_open_only() {
        let long = format!("{}&& rm -rf C:\\important", "echo step && ".repeat(60));
        let items = [bash(&long)];
        for (lang, hidden) in [(Lang::En, "characters hidden – check the terminal"), (Lang::De, "Zeichen verborgen – im Terminal prüfen")] {
            let mut h = H::new();
            h.frame(&items, lang, vec![]);
            h.frame(&items, lang, vec![]);
            let texts = h.texts();
            assert!(texts.iter().any(|t| t.starts_with("echo step")), "Anfang: {texts:?}");
            assert!(texts.iter().any(|t| t.starts_with('…') && t.ends_with("rm -rf C:\\important")), "Ende: {texts:?}");
            let note = texts.iter().find(|t| t.ends_with(hidden)).expect("Hinweis");
            let n: usize = note.split(' ').next().unwrap().parse().unwrap();
            assert!(n > 400 && n < long.len(), "{n}");
            assert!(!h.answers(), "gekürzt: nur Öffnen");
            // Tooltip mit dem ganzen Text
            let p = h.rect_of(texts.iter().find(|t| t.starts_with('…')).unwrap()).unwrap().center();
            h.frame(&items, lang, vec![Event::PointerMoved(p)]);
            for _ in 0..3 {
                h.frame(&items, lang, vec![]);
            }
            assert!(h.galleys().iter().any(|g| g.text() == long && !g.elided && g.rows.len() > REQUEST_ROWS), "Tooltip zeigt alles");
        }
    }

    /// Leerraum-Polster kann den gefährlichen Teil nicht verstecken; Zeilenumbrüche sind sichtbar.
    #[test]
    fn padding_is_collapsed_and_newlines_are_visible() {
        let padded = format!("echo harmless{}&& rm -rf /", " ".repeat(800));
        let mut h = H::new();
        h.frame(&[bash(&padded)], Lang::En, vec![]);
        assert!(h.has("echo harmless && rm -rf /"), "{:?}", h.texts());
        assert!(h.answers(), "passt ganz: Ja/Nein");
        h.frame(&[bash("echo a\nrm -rf /")], Lang::En, vec![]);
        assert!(h.has("echo a ⏎ rm -rf /"), "{:?}", h.texts());
    }

    /// Markup bleibt Text.
    #[test]
    fn markup_is_plain_text() {
        let items = [bash("<b>**rm**</b> [x](http://evil)")];
        let mut h = H::new();
        h.frame(&items, Lang::En, vec![]);
        assert!(h.has("<b>**rm**</b> [x](http://evil)"), "{:?}", h.texts());
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
        assert!(matches!(h.click(&items, Lang::En, "Yes").as_slice(), [Action::Approve(7, true, 3)]));
    }

    /// Chat-Freigabe in der Karte: Werkzeug mit Ziel, alle Argumente als JSON, „vom Modell“; Ja/Nein entscheiden genau
    /// diesen Aufruf. Zu lange Argumente und Fragen des Modells: nur *Öffnen*.
    #[test]
    fn chat_approvals_show_tool_and_arguments_and_decide_that_call() {
        let args = serde_json::json!({"session": 4, "text": "dir /s"});
        let detail = crate::hooks::tool_detail("send_text", &args).unwrap();
        let chat = InboxItem {
            id: 9,
            name: "Chat".into(),
            msg: "The model wants to use a tool.".into(),
            request: Some(ToolRequest { tool: "send_text → session 4 \"build\"".into(), calls: vec![ToolCall { seq: 0, id: Some("call-1".into()), detail: detail.clone() }], confirmed: true }),
            from_model: true,
            answer: Answer::Chat("call-1".into()),
            since: None,
        };
        for (lang, from, yes, no) in [(Lang::En, "from the model", "Yes", "No"), (Lang::De, "vom Modell", "Ja", "Nein")] {
            let items = [chat.clone()];
            let mut h = H::new();
            h.frame(&items, lang, vec![]);
            for want in ["send_text → session 4 \"build\"", detail.as_str(), from] {
                assert!(h.has(want), "{want}: {:?}", h.texts());
            }
            let acts = h.click(&items, lang, yes);
            assert!(matches!(acts.as_slice(), [Action::ChatDecide(9, c, chatmod::Decision::Allow)] if c == "call-1"));
            let acts = h.click(&items, lang, no);
            assert!(matches!(acts.as_slice(), [Action::ChatDecide(9, c, chatmod::Decision::Deny)] if c == "call-1"));
        }
        let long = crate::hooks::tool_detail("send_text", &serde_json::json!({"session": 4, "text": "x ".repeat(400)})).unwrap();
        let big = InboxItem { request: Some(ToolRequest { calls: vec![ToolCall { seq: 0, id: Some("call-1".into()), detail: long }], ..chat.request.clone().unwrap() }), ..chat.clone() };
        let mut h = H::new();
        h.frame(&[big], Lang::En, vec![]);
        assert!(!h.answers(), "gekürzt: nur Öffnen");
        let ask = InboxItem { request: None, msg: "The model has questions for you.".into(), answer: Answer::Open, ..chat };
        h.frame(&[ask], Lang::En, vec![]);
        assert!(h.has("Open") && h.has("The model has questions for you.") && !h.answers(), "{:?}", h.texts());
    }
}
