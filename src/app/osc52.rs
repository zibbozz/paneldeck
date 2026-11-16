//! Programme kopieren per OSC 52 in die Zwischenablage (#181), z. B. Claude Code in einer WSL- oder SSH-Sitzung.
//!
//! Der PTY-Thread legt den Wunsch in der Sitzung ab (`terminal::Listener`), der Hauptdurchgang holt ihn hier ab
//! (`App::poll_shell`). Nur Schreiben: Lesewünsche (`ClipboardLoad`) werden nie beantwortet. Abschaltbar über
//! `Settings::osc52_copy`; Texte über `terminal::OSC52_MAX_BYTES` werden verworfen.

use super::toasts::ToastKind;
use super::App;
use crate::terminal::{ClipboardRequest, OSC52_MAX_BYTES};
use eframe::egui;
use std::time::{Duration, Instant};

/// Mindestabstand zwischen zwei Toasts derselben Sitzung (Programme, die oft kopieren, sollen nicht fluten).
pub const TOAST_GAP: Duration = Duration::from_secs(5);

/// Ist wieder ein Toast für die Sitzung fällig?
pub fn toast_due(last: Option<Instant>, now: Instant) -> bool {
    match last {
        Some(t) => now.saturating_duration_since(t) >= TOAST_GAP,
        None => true,
    }
}

fn mb(n: usize) -> String {
    format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
}

impl App {
    /// Einen Kopierwunsch der Sitzung `sid` ausführen (Hauptdurchgang).
    pub(super) fn osc52_copy(&mut self, ctx: &egui::Context, sid: u64, name: &str, req: ClipboardRequest) {
        if !self.settings.osc52_copy {
            return;
        }
        let lang = self.settings.language;
        let now = Instant::now();
        let due = toast_due(self.osc52_toasts.get(&sid).copied(), now);
        if due {
            self.osc52_toasts.retain(|k, _| self.entries.contains_key(k));
            self.osc52_toasts.insert(sid, now);
        }
        match req {
            ClipboardRequest::Text(text) => {
                // Direkt über die Windows-Zwischenablage: wirkt auch, wenn das Fenster minimiert oder im Tray ist
                // (egui setzt `copy_text` erst beim Zeichnen des Fensters um).
                if !crate::clip::set_text(&text) {
                    ctx.copy_text(text);
                }
                if due {
                    self.push_toast(format!("{} {name}", lang.t("In die Zwischenablage kopiert von", "Copied to the clipboard by")), ToastKind::Info, None);
                }
            }
            ClipboardRequest::TooLarge(n) if due => {
                let text = format!(
                    "{name}: {} ({}, {} {})",
                    lang.t("Kopieren in die Zwischenablage abgelehnt, Text zu groß", "Copy to the clipboard refused, text too large"),
                    mb(n),
                    lang.t("Grenze", "limit"),
                    mb(OSC52_MAX_BYTES)
                );
                self.push_toast(text, ToastKind::Warning, None);
            }
            ClipboardRequest::TooLarge(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
