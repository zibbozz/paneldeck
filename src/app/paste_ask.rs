//! Rückfrage vor dem Einfügen mehrerer Zeilen (#183, wie Windows Terminal): Ohne Bracketed Paste (DECSET 2004) führt
//! die Shell jede Zeile eines eingefügten Textes sofort aus. Abschaltbar über `Settings::warn_multiline_paste`
//! (Standard an). Programme mit Bracketed Paste (PSReadLine, bash/zsh, Claude Code) bekommen den Text ohne Rückfrage.
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

impl App {
    /// Text aus `Session::paste_user` übernehmen (aus `apply`, im Fenster des Bereichs).
    pub(super) fn ask_paste(&mut self, id: u64, text: String, detached: bool) {
        self.paste_ask = Some(PasteAsk { id, text, win: self.wins.cur, detached, since: std::time::Instant::now() });
    }

    /// Rückfrage zeigen (je Fenster; `viewport` = Id der abgedockten Sitzung, deren Fenster gerade gezeichnet wird).
    pub(super) fn paste_ask_ui(&mut self, ctx: &egui::Context, viewport: Option<u64>) {
        let Some(ask) = self.paste_ask.take() else { return };
        let in_detached = ask.detached && self.detached.contains(&ask.id);
        let here = match viewport {
            Some(v) => in_detached && v == ask.id,
            None => !in_detached && ask.win == self.wins.cur,
        };
        // Wird das Fenster vorher geschlossen, bleibt der Text liegen, bis ein neues Einfügen ihn ersetzt (nie gesendet).
        if !here {
            self.paste_ask = Some(ask);
            return;
        }
        let Some(s) = self.entries.get(&ask.id).and_then(|e| e.live.as_ref()) else { return };
        let lang = self.settings.language;
        let lines = crate::terminal::paste_line_count(&ask.text).unwrap_or(1);
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
                "Abschaltbar unter Einstellungen → Darstellung.",
                "Can be turned off in Settings → Appearance.",
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
