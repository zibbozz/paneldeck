//! Meldungen von Programmen im Terminal (#183): Fortschritt (OSC 9;4) für Seitenleiste und Taskleiste,
//! Benachrichtigungen (OSC 9 / OSC 777;notify) und die Glocke (BEL).
//!
//! Entscheidung des Nutzers: Benachrichtigungen und Glocke nur für eine **nicht angesehene** Sitzung (nicht die
//! fokussierte Sitzung des Fensters im Vordergrund) – dann Markierung in der Seitenleiste und, je nach Einstellung,
//! Toast/Blinken (Fenster nicht im Vordergrund) bzw. Hinweis in der App. Je Sitzung begrenzt (`vtnotify::RateLimit`).
//! Die Texte sind schon in `osc.rs` bereinigt; der Toast trägt immer den Sitzungsnamen als Titel.

use super::*;
use crate::vtnotify::RateLimit;

/// Titel des Toasts einer Programm-Meldung: „Paneldeck · <Sitzung>“ (Name schon bereinigt).
pub(super) fn toast_title(session: &str) -> String {
    format!("Paneldeck · {session}")
}

/// Text einer Benachrichtigung aus Titel und Text des Programms.
pub(super) fn note_text(title: &str, body: &str) -> String {
    match (title.is_empty(), body.is_empty()) {
        (false, false) => format!("{title} – {body}"),
        (true, _) => body.to_string(),
        (false, true) => title.to_string(),
    }
}

impl App {
    /// Je Bild: Fortschritt übernehmen, Benachrichtigungen und Glocke auswerten, Taskleiste nachführen.
    pub(super) fn poll_vt(&mut self, ctx: &egui::Context) {
        let (window_focused, focused_id) = self.seen_focus();
        let now = Instant::now();
        let lang = self.settings.language;
        let (notes_on, bell_toast) = (self.settings.program_notify, self.settings.bell_toast);
        let mut out: Vec<(u64, String, String)> = Vec::new();
        for (id, e) in self.entries.iter_mut() {
            let Some(s) = e.live.as_ref() else {
                e.progress = None;
                continue;
            };
            // Immer abholen: eine alte Meldung darf nicht später (z. B. nach dem Einschalten) doch noch erscheinen.
            let (progress, notes) = {
                let mut st = s.shell_state.lock().unwrap_or_else(|e| e.into_inner());
                // Unbestimmter Fortschritt ohne Neuigkeiten verfällt (ein abgestürztes Programm ließe sonst ewig animieren).
                if crate::vtnotify::progress_expired(st.progress, st.progress_at, now) {
                    st.progress = None;
                }
                (st.progress, std::mem::take(&mut st.notes))
            };
            let bell = s.bell.swap(false, std::sync::atomic::Ordering::Relaxed);
            // Roh für `list_sessions`; Seitenleiste und Taskleiste beachten die Einstellung „Fortschritt anzeigen“.
            e.progress = if s.has_exited() { None } else { progress };
            if window_focused && *id == focused_id {
                continue;
            }
            // Glocke: die Markierung immer, Toast/Hinweis nur mit der Einstellung (Entscheidung des Nutzers).
            if bell {
                e.attention = true;
            }
            let mut msgs: Vec<String> = Vec::new();
            if notes_on {
                msgs.extend(notes.iter().map(|(t, b)| note_text(t, b)));
            }
            if bell && bell_toast {
                msgs.push(lang.t("Das Programm hat die Glocke geläutet", "The program rang the bell").to_string());
            }
            if msgs.is_empty() {
                continue;
            }
            e.attention = true;
            // Claude mit Hooks meldet „braucht dich“/„fertig“ schon selbst (`poll_hooks`): keine doppelten Toasts.
            if e.is_claude() && e.hook_seen {
                continue;
            }
            let lim = self.vt_limits.entry(*id).or_insert_with(|| RateLimit::new(now));
            for m in msgs {
                if lim.allow(now) {
                    out.push((*id, crate::vtnotify::sanitize(&e.display_name(), crate::vtnotify::TITLE_MAX), m));
                }
            }
        }
        let entries = &self.entries;
        self.vt_limits.retain(|id, _| entries.contains_key(id));
        for (sid, name, body) in out {
            if !window_focused {
                if self.settings.notify_toast {
                    // Absender immer erkennbar: die App, dann die Sitzung (der Text stammt vom Programm).
                    notify::toast(&toast_title(&name), &body, self.settings.notify_sound);
                }
                if self.settings.notify_flash {
                    ctx.send_viewport_cmd_to(self.attention_viewport(sid), egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
                }
            } else {
                self.notify_text(format!("{name}: {body}"));
            }
        }
        self.taskbar_tick();
    }

    /// Fortschritt je Fenster an die Taskleiste melden (zusammengefasst über die Sitzungen des Fensters).
    fn taskbar_tick(&mut self) {
        let mut wins: Vec<(isize, Option<crate::vtnotify::Progress>)> = Vec::new();
        let show = self.settings.progress_indicator;
        let mut add = |hwnd: Option<isize>, win: u64, entries: &BTreeMap<u64, Entry>| {
            if let Some(h) = hwnd {
                let p = if show { crate::taskbar::aggregate(entries.values().filter(|e| e.meta.window == win).filter_map(|e| e.progress)) } else { None };
                wins.push((h, p));
            }
        };
        add(self.hwnd, 0, &self.entries);
        for (id, h) in self.extra_hwnds() {
            add(Some(h), id, &self.entries);
        }
        self.taskbar.update(&wins);
    }
}

#[cfg(test)]
mod tests {
    use super::note_text;

    #[test]
    fn note_texts() {
        assert_eq!(note_text("Build", "fertig"), "Build – fertig");
        assert_eq!(note_text("", "fertig"), "fertig");
        assert_eq!(note_text("Build", ""), "Build");
        assert_eq!(super::toast_title("pwsh"), "Paneldeck · pwsh");
    }
}
