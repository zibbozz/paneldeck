//! Rückfrage vor dem Einfügen mehrerer Zeilen (#183, wie Windows Terminal): Ohne Bracketed Paste (DECSET 2004) führt
//! die Shell jede Zeile eines eingefügten Textes sofort aus. Abschaltbar über `Settings::warn_multiline_paste`
//! (Standard an). Programme mit Bracketed Paste (PSReadLine, bash/zsh, Claude Code) bekommen den Text ohne Rückfrage –
//! außer im Broadcast, wenn ein Zielbereich kein Bracketed Paste hat und die Zeilen ausführen würde (#220; geprüft erst
//! beim Einfügen, `App::ask_paste`).
//!
//! Ablauf: `Session::paste_user` hält den Text zurück, `show_pane` holt ihn ab (`Action::PasteAsk`), der Dialog
//! erscheint in dem Fenster, in dem eingefügt wurde (Hauptfenster, weiteres Fenster oder abgedockte Sitzung).

use super::closing::{confirm_window_keep, Outcome};
use super::*;

/// Wartender Text und wo die Rückfrage erscheint.
pub(super) struct PasteAsk {
    pub(super) id: u64,
    pub(super) text: String,
    /// Fenster (`wins.cur`), in dem eingefügt wurde.
    pub(super) win: u64,
    /// Aus dem Fenster einer abgedockten Sitzung.
    pub(super) detached: bool,
    /// Seit wann die Rückfrage steht: „Einfügen“ (Knopf und Enter) erst nach [`CONFIRM_DELAY`] – ein Enter, das schon
    /// unterwegs war, bestätigt die Sicherheitsabfrage nicht.
    pub(super) since: std::time::Instant,
}

/// Wartezeit, bevor die Rückfrage bestätigt werden kann.
const CONFIRM_DELAY: std::time::Duration = std::time::Duration::from_millis(1000);

/// Höchstens so viele Zeilen (je höchstens [`PREVIEW_CHARS`] Zeichen) zeigt die Vorschau.
const PREVIEW_LINES: usize = 5;
const PREVIEW_CHARS: usize = 100;

/// Vorschau des Textes: die ersten Zeilen, gekürzt, ohne Steuerzeichen.
pub(super) fn preview(text: &str) -> String {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut out: Vec<String> = Vec::new();
    for l in text.lines().take(PREVIEW_LINES) {
        let mut s: String = l
            .chars()
            .map(|c| if c == '\t' { ' ' } else { c })
            .filter(|&c| !c.is_control() && !crate::terminal::is_hidden_format_char(c))
            .take(PREVIEW_CHARS)
            .collect();
        if l.chars().count() > PREVIEW_CHARS {
            s.push('…');
        }
        out.push(s);
    }
    if text.lines().count() > PREVIEW_LINES {
        out.push("…".to_string());
    }
    out.join("\n")
}

/// Rückfrage zeigen? Ohne `check_targets` immer (die Sitzung selbst hat kein Bracketed Paste); sonst nur, wenn ein
/// Broadcast-Ziel keins hat – die Ziele werden erst dann abgefragt (`targets_unbracketed`, sperrt jede Emulation).
fn must_ask(check_targets: bool, targets_unbracketed: impl FnOnce() -> bool) -> bool {
    !check_targets || targets_unbracketed()
}

/// Gehört die Rückfrage `ask` in das gerade gezeichnete Fenster? `viewport` = Id der abgedockten Sitzung, deren Fenster
/// gezeichnet wird (`None` = Hauptfenster oder weiteres Fenster `cur`); `detached` = die Sitzung ist (noch) abgedockt.
fn ask_is_here(ask: &PasteAsk, detached: bool, cur: u64, viewport: Option<u64>) -> bool {
    let in_detached = ask.detached && detached;
    match viewport {
        Some(v) => in_detached && v == ask.id,
        None => !in_detached && ask.win == cur,
    }
}

impl App {
    /// Text aus `Session::paste_user` übernehmen (aus `apply`, im Fenster des Bereichs). `check_targets`: Die Sitzung
    /// selbst hat Bracketed Paste, gefragt wird nur, wenn ein Broadcast-Ziel keins hat – sonst sofort einfügen.
    pub(super) fn ask_paste(&mut self, id: u64, text: String, detached: bool, check_targets: bool) {
        let ask = must_ask(check_targets, || {
            self.broadcast && self.broadcast_targets().iter().any(|i| self.entries.get(i).and_then(|e| e.live.as_ref()).is_some_and(|s| !s.bracketed_paste()))
        });
        if !ask {
            if let Some(s) = self.entries.get(&id).and_then(|e| e.live.as_ref()) {
                s.paste_now(&text);
            }
            return;
        }
        self.paste_ask = Some(PasteAsk { id, text, win: self.wins.cur, detached, since: std::time::Instant::now() });
    }

    /// Steht die Rückfrage in diesem Fenster (`viewport` wie bei `paste_ask_ui`)? Dann behält dort kein Terminal den
    /// Fokus (`central`), auch nicht im ersten Bild, bevor die Rückfrage gezeichnet ist (#217).
    pub(super) fn paste_ask_here(&self, viewport: Option<u64>) -> bool {
        self.paste_ask.as_ref().is_some_and(|ask| ask_is_here(ask, self.detached.contains(&ask.id), self.wins.cur, viewport))
    }

    /// Rückfrage zeigen (je Fenster; `viewport` = Id der abgedockten Sitzung, deren Fenster gerade gezeichnet wird).
    pub(super) fn paste_ask_ui(&mut self, ctx: &egui::Context, viewport: Option<u64>) {
        // Wird das Fenster vorher geschlossen, bleibt der Text liegen, bis ein neues Einfügen ihn ersetzt (nie gesendet).
        if !self.paste_ask_here(viewport) {
            return;
        }
        let Some(ask) = self.paste_ask.take() else { return };
        let Some(s) = self.entries.get(&ask.id).and_then(|e| e.live.as_ref()) else { return };
        let lang = self.settings.language;
        // Der Text wartet unverändert (#220); gezählt wird wie ohne Bracketed Paste – so würde eine Shell ihn ausführen.
        let lines = crate::terminal::paste_line_count(&crate::terminal::paste_user_text(&ask.text)).unwrap_or(1);
        let wait = CONFIRM_DELAY.saturating_sub(ask.since.elapsed());
        let secs = if wait.is_zero() { 0 } else { 1 };
        if secs > 0 {
            ctx.request_repaint_after(wait);
        }
        let o = confirm_window_keep(ctx, Id::new("paste_ask"), lang.t("Mehrere Zeilen einfügen?", "Paste multiple lines?"), lang.t("Einfügen", "Paste"), None, secs, lang, 460.0, |ui| {
            ui.label(if lang == Lang::De {
                format!("Der Text enthält Zeilenumbrüche ({lines} {}). Die Shell kann jede Zeile sofort als Befehl ausführen.", if lines == 1 { "Zeile" } else { "Zeilen" })
            } else {
                format!("The text contains line breaks ({lines} {}). The shell may run each line as a command right away.", if lines == 1 { "line" } else { "lines" })
            });
            ui.add_space(S1);
            egui::Frame::NONE.fill(style::dim(ui).gamma_multiply(0.12)).inner_margin(6.0).corner_radius(4.0).show(ui, |ui| {
                ui.label(RichText::new(preview(&ask.text)).monospace().small());
            });
            ui.add_space(S1);
            ui.label(RichText::new(lang.t(
                "Abschaltbar unter Einstellungen → Terminal → Zwischenablage & Einfügen.",
                "Can be turned off in Settings → Terminal → Clipboard & paste.",
            )).small().color(style::dim(ui)));
        });
        match o {
            Outcome::Go => {
                s.paste_now(&ask.text);
                self.want_focus = Some(ask.id);
            }
            // `Keep` gibt es nur mit zweitem Knopf (Schließen-Dialoge, #168).
            Outcome::Cancel | Outcome::Keep => self.want_focus = Some(ask.id),
            Outcome::Open => self.paste_ask = Some(ask),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #217: `central` gibt den Fokus ab, sobald die Rückfrage in seinem Fenster steht – dieselbe Zuordnung wie beim
    /// Zeichnen der Rückfrage (Hauptfenster/weiteres Fenster bzw. abgedockte Sitzung).
    #[test]
    fn ask_belongs_to_its_window() {
        let ask = |win, detached| PasteAsk { id: 7, text: "a\nb".into(), win, detached, since: std::time::Instant::now() };
        // Im Hauptfenster eingefügt: nur dort, nicht in einem weiteren Fenster oder einem abgedockten.
        assert!(ask_is_here(&ask(0, false), false, 0, None));
        assert!(!ask_is_here(&ask(0, false), false, 1, None));
        assert!(!ask_is_here(&ask(0, false), false, 0, Some(7)));
        // Aus der abgedockten Sitzung: nur in deren Fenster; wieder angedockt im Fenster, in dem eingefügt wurde.
        assert!(ask_is_here(&ask(0, true), true, 0, Some(7)));
        assert!(!ask_is_here(&ask(0, true), true, 0, Some(8)));
        assert!(!ask_is_here(&ask(0, true), true, 0, None));
        assert!(ask_is_here(&ask(0, true), false, 0, None));
    }

    /// #220: Die Rückfrage hängt auch an den Broadcast-Zielen; gezählt wird am unveränderten Text.
    #[test]
    fn broadcast_targets_without_bracketed_paste_ask_too() {
        use crate::terminal::paste_needs_ask;
        assert!(paste_needs_ask("a\nb", false, true, false));
        assert!(!paste_needs_ask("a\nb", true, true, false), "Bracketed Paste ohne Broadcast: keine Rückfrage");
        assert!(paste_needs_ask("a\nb", true, true, true), "im Broadcast entscheiden die Ziele");
        // Die Ziele werden erst beim Einfügen und nur bei Bedarf abgefragt.
        assert!(must_ask(false, || panic!("ohne check_targets nicht abfragen")));
        assert!(must_ask(true, || true), "ein Ziel ohne Bracketed Paste führte die Zeilen aus");
        assert!(!must_ask(true, || false), "alle Ziele mit Bracketed Paste: sofort einfügen");
        assert!(!paste_needs_ask("a\nb", false, false, true), "abgeschaltet");
        assert!(!paste_needs_ask("cmd\n", true, true, true), "eine Zeile mit Zeilenende bekommt das Ziel ohne Ende");
        assert!(!paste_needs_ask("eine Zeile", false, true, true));
    }

    #[test]
    fn preview_is_short_and_clean() {
        assert_eq!(preview("a\r\nb\rc\n"), "a\nb\nc");
        let long = "x".repeat(150);
        let p = preview(&format!("{long}\n2\n3\n4\n5\n6\n7"));
        let lines: Vec<&str> = p.lines().collect();
        assert_eq!(lines.len(), PREVIEW_LINES + 1);
        assert_eq!(lines[0].chars().count(), PREVIEW_CHARS + 1);
        assert!(lines[0].ends_with('…') && lines[PREVIEW_LINES] == "…");
        assert_eq!(preview("a\x1b[31mb\u{9b}c\td"), "a[31mbc d");
        // Schreibrichtung und unsichtbare Zeichen täuschen in der Vorschau nichts vor
        assert_eq!(preview("rm -rf \u{202e}fdsa\u{202c} \u{200b}x\u{2028}y"), "rm -rf fdsa xy");
    }
}
