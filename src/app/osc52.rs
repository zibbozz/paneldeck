//! Programme kopieren per OSC 52 in die Zwischenablage (#181), z. B. Claude Code in einer WSL- oder SSH-Sitzung.
//!
//! Der PTY-Thread sammelt die Wünsche in der Sitzung (`terminal::Listener`), der Hauptdurchgang holt sie ab
//! (`App::poll_shell` → [`App::osc52_poll`]). Regeln:
//! - nur Schreiben: Lesewünsche (`ClipboardLoad`) werden nie beantwortet;
//! - abschaltbar über `Settings::osc52_copy`;
//! - nur die fokussierte Sitzung des Paneldeck-Fensters im Vordergrund darf kopieren (Hintergrund-Tabs, minimierte
//!   Bereiche, abgedockte/andere Fenster und ein versteckter Hub nicht) – auch gegen Agenten, die per `send_text` eine
//!   Sitzung im Hintergrund etwas kopieren lassen;
//! - höchstens ein Schreibvorgang je Sitzung alle [`WRITE_GAP`]; der neueste Text wird nachgereicht;
//! - leere Texte und Texte über `terminal::OSC52_MAX_BYTES` werden verworfen.
//!
//! Die Entscheidung selbst ist rein ([`decide`]) und ohne egui testbar.

use super::toasts::ToastKind;
use super::App;
use crate::terminal::{ClipboardRequest, OSC52_MAX_BYTES};
use eframe::egui;
use std::borrow::Cow;
use std::time::{Duration, Instant};

/// Mindestabstand zwischen zwei Schreibvorgängen derselben Sitzung in die Zwischenablage.
pub const WRITE_GAP: Duration = Duration::from_millis(300);
/// Mindestabstand zwischen zwei Toasts derselben Sitzung (Programme, die oft kopieren, sollen nicht fluten).
pub const TOAST_GAP: Duration = Duration::from_secs(5);
/// Dauerfeuer: mehr als [`SPAM_LIMIT`] Kopierwünsche innerhalb von [`SPAM_WINDOW`] ergeben eine einmalige Warnung.
pub const SPAM_WINDOW: Duration = Duration::from_secs(10);
pub const SPAM_LIMIT: u32 = 30;

/// Zustand einer Sitzung für OSC 52 (im Hub, je Sitzung).
#[derive(Clone, Debug, Default)]
pub struct ClipState {
    /// Noch nicht geschriebener, neuester Text (wartet auf das Ende von [`WRITE_GAP`]).
    pub pending: Option<String>,
    pub last_write: Option<Instant>,
    pub last_toast: Option<Instant>,
    spam_start: Option<Instant>,
    spam_count: u32,
    spam_warned: bool,
}

impl ClipState {
    /// `n` neue Kopierwünsche zählen; `true`, wenn jetzt (einmalig je Sitzung) vor Dauerfeuer gewarnt werden soll.
    pub fn note_requests(&mut self, n: u32, now: Instant) -> bool {
        match self.spam_start {
            Some(s) if now.saturating_duration_since(s) < SPAM_WINDOW => self.spam_count = self.spam_count.saturating_add(n),
            _ => {
                self.spam_start = Some(now);
                self.spam_count = n;
            }
        }
        if self.spam_count > SPAM_LIMIT && !self.spam_warned {
            self.spam_warned = true;
            return true;
        }
        false
    }
}

/// Was mit dem wartenden Text einer Sitzung geschieht.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Verwerfen (nichts wartet, Text leer, Einstellung aus oder Sitzung nicht fokussiert).
    Drop,
    /// Noch warten (Ratenbegrenzung); danach erneut entscheiden.
    Wait(Duration),
    /// Jetzt in die Zwischenablage schreiben, mit oder ohne Toast.
    Write { toast: bool },
}

/// Ist wieder ein Toast für die Sitzung fällig?
pub fn toast_due(last: Option<Instant>, now: Instant) -> bool {
    match last {
        Some(t) => now.saturating_duration_since(t) >= TOAST_GAP,
        None => true,
    }
}

/// Reine Entscheidung für den wartenden Text einer Sitzung.
pub fn decide(enabled: bool, focused: bool, st: &ClipState, now: Instant) -> Decision {
    if !st.pending.as_deref().is_some_and(|t| !t.is_empty()) || !enabled || !focused {
        return Decision::Drop;
    }
    if let Some(t) = st.last_write {
        let el = now.saturating_duration_since(t);
        if el < WRITE_GAP {
            return Decision::Wait(WRITE_GAP - el);
        }
    }
    Decision::Write { toast: toast_due(st.last_toast, now) }
}

/// Einzelne `\n` zu `\r\n` (Windows-Text in CF_UNICODETEXT); vorhandene `\r\n` bleiben.
pub fn to_crlf(s: &str) -> Cow<'_, str> {
    if !s.contains('\n') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + s.len() / 16);
    let mut prev = '\0';
    for c in s.chars() {
        if c == '\n' && prev != '\r' {
            out.push('\r');
        }
        out.push(c);
        prev = c;
    }
    Cow::Owned(out)
}

/// Nur für den Ende-zu-Ende-Test (`tools/smoke-mcp.ps1`, Hub minimiert und ohne Fokus): `PANELDECK_TEST_OSC52_ANY_FOCUS=1`
/// in der Umgebung des Hub-Prozesses hebt die Fokus-Regel auf. Programme in einem Terminal können die Umgebung des
/// Hubs nicht ändern; einmal beim ersten Gebrauch gelesen.
fn test_any_focus() -> bool {
    static V: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *V.get_or_init(|| std::env::var("PANELDECK_TEST_OSC52_ANY_FOCUS").as_deref() == Ok("1"))
}

fn mb(n: usize) -> String {
    format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
}

impl App {
    /// Name einer Sitzung für Meldungen: der vom Nutzer vergebene/gespeicherte Name oder die Shell – nie der Titel,
    /// den ein Programm per OSC 0/2 setzt (ein Programm könnte sich sonst als andere Sitzung ausgeben).
    fn osc52_name(&self, sid: u64) -> String {
        match self.entries.get(&sid) {
            Some(e) if !e.meta.name.trim().is_empty() => e.meta.name.clone(),
            Some(e) => e.eff_shell().to_string(),
            None => String::new(),
        }
    }

    /// Neue Kopierwünsche übernehmen und wartende Texte schreiben (Hauptdurchgang, jedes Frame).
    pub(super) fn osc52_poll(&mut self, ctx: &egui::Context, incoming: Vec<(u64, ClipboardRequest)>) {
        let lang = self.settings.language;
        let enabled = self.settings.osc52_copy;
        let (window_focused, focused_id) = self.seen_focus();
        let any_focus = test_any_focus();
        let is_focused = |sid: u64| any_focus || (window_focused && sid == focused_id);
        let now = Instant::now();
        self.osc52.retain(|k, _| self.entries.contains_key(k));
        let mut notes: Vec<(u64, String, ToastKind)> = Vec::new();
        for (sid, req) in incoming {
            if !enabled || !is_focused(sid) {
                if let Some(st) = self.osc52.get_mut(&sid) {
                    st.pending = None;
                }
                continue;
            }
            let st = self.osc52.entry(sid).or_default();
            if let Some(t) = req.text {
                st.pending = Some(t);
            }
            if st.note_requests(req.count, now) {
                let t = lang.t(
                    "kopiert sehr oft in die Zwischenablage; nur der neueste Text wird übernommen (höchstens alle 0,3 s).",
                    "copies to the clipboard very often; only the newest text is taken (at most every 0.3 s).",
                );
                notes.push((sid, t.to_string(), ToastKind::Warning));
            }
            if let Some(n) = req.too_large {
                if toast_due(st.last_toast, now) {
                    st.last_toast = Some(now);
                    let t = format!(
                        "{} ({}, {} {})",
                        lang.t("Kopieren in die Zwischenablage abgelehnt, Text zu groß", "Copy to the clipboard refused, text too large"),
                        mb(n),
                        lang.t("Grenze", "limit"),
                        mb(OSC52_MAX_BYTES)
                    );
                    notes.push((sid, t, ToastKind::Warning));
                }
            }
        }
        let waiting: Vec<u64> = self.osc52.iter().filter(|(_, s)| s.pending.is_some()).map(|(k, _)| *k).collect();
        for sid in waiting {
            let focused = is_focused(sid);
            let Some(st) = self.osc52.get_mut(&sid) else { continue };
            match decide(enabled, focused, st, now) {
                Decision::Drop => st.pending = None,
                Decision::Wait(d) => ctx.request_repaint_after(d),
                Decision::Write { toast } => {
                    let text = st.pending.take().unwrap_or_default();
                    st.last_write = Some(now);
                    if toast {
                        st.last_toast = Some(now);
                    }
                    // Direkt über die Windows-Zwischenablage: wirkt unabhängig vom Zeichnen des Fensters
                    // (egui setzt `copy_text` erst danach um).
                    let text = to_crlf(&text).into_owned();
                    if !crate::clip::set_text(&text) {
                        ctx.copy_text(text);
                    }
                    if toast {
                        notes.push((sid, lang.t("In die Zwischenablage kopiert von", "Copied to the clipboard by").to_string(), ToastKind::Info));
                    }
                }
            }
        }
        for (sid, text, kind) in notes {
            let name = self.osc52_name(sid);
            let msg = if kind == ToastKind::Info { format!("{text} {name}") } else { format!("{name}: {text}") };
            self.push_toast(msg, kind, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(t: &str) -> ClipState {
        ClipState { pending: Some(t.into()), ..Default::default() }
    }

    #[test]
    fn toast_throttle() {
        let t0 = Instant::now();
        assert!(toast_due(None, t0));
        assert!(!toast_due(Some(t0), t0));
        assert!(!toast_due(Some(t0), t0 + Duration::from_secs(4)));
        assert!(toast_due(Some(t0), t0 + TOAST_GAP));
        // Uhr „rückwärts“ (sollte nicht vorkommen) ergibt keinen Toast und keine Panik.
        assert!(!toast_due(Some(t0 + Duration::from_secs(1)), t0));
        assert_eq!(mb(OSC52_MAX_BYTES), "1.0 MB");
    }

    #[test]
    fn decide_setting_off_and_focus_rule() {
        let t0 = Instant::now();
        let st = pending("x");
        assert_eq!(decide(true, true, &st, t0), Decision::Write { toast: true });
        // Einstellung aus: verwerfen.
        assert_eq!(decide(false, true, &st, t0), Decision::Drop);
        // Nicht fokussiert (Hintergrund-Tab, anderes/abgedocktes Fenster, Hub versteckt): verwerfen.
        assert_eq!(decide(true, false, &st, t0), Decision::Drop);
        // Nichts oder leerer Text: verwerfen (die Zwischenablage wird nie geleert).
        assert_eq!(decide(true, true, &ClipState::default(), t0), Decision::Drop);
        assert_eq!(decide(true, true, &pending(""), t0), Decision::Drop);
    }

    #[test]
    fn decide_rate_limit_and_trailing_write() {
        let t0 = Instant::now();
        let mut st = pending("neu");
        st.last_write = Some(t0);
        st.last_toast = Some(t0);
        // Innerhalb der Sperre warten (Rest der Zeit), danach schreiben – ohne zweiten Toast.
        assert_eq!(decide(true, true, &st, t0 + Duration::from_millis(100)), Decision::Wait(Duration::from_millis(200)));
        assert_eq!(decide(true, true, &st, t0 + WRITE_GAP), Decision::Write { toast: false });
        // Toast wieder nach TOAST_GAP.
        assert_eq!(decide(true, true, &st, t0 + TOAST_GAP), Decision::Write { toast: true });
        // Fokusverlust während des Wartens: verwerfen statt nachreichen.
        assert_eq!(decide(true, false, &st, t0 + Duration::from_millis(100)), Decision::Drop);
    }

    #[test]
    fn spam_warning_once() {
        let t0 = Instant::now();
        let mut st = ClipState::default();
        assert!(!st.note_requests(SPAM_LIMIT, t0));
        assert!(st.note_requests(1, t0 + Duration::from_secs(1)));
        assert!(!st.note_requests(100, t0 + Duration::from_secs(2)), "nur einmal");
        // Langsames Kopieren ergibt nie eine Warnung.
        let mut st = ClipState::default();
        for i in 0..200u64 {
            assert!(!st.note_requests(1, t0 + Duration::from_millis(i * 500)));
        }
    }

    #[test]
    fn crlf_conversion() {
        assert_eq!(to_crlf("a"), "a");
        assert!(matches!(to_crlf("a"), Cow::Borrowed(_)));
        assert_eq!(to_crlf("a\nb\r\nc\n"), "a\r\nb\r\nc\r\n");
        assert_eq!(to_crlf("\n\n"), "\r\n\r\n");
        assert_eq!(to_crlf("ä\nö"), "ä\r\nö");
    }
}
