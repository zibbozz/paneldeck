//! Strg+Z / Strg+Y / Strg+Umschalt+Z auf den Seiten, die sonst keine eigene Rückgängig-Taste haben: der Reiter „Backlog“
//! (Fallenlassen und „Verschieben nach …“ von Sprint-Zugehörigkeit samt Rang) und die Übersicht des System-Projekts „Todolist“
//! (Umwandlung einer Karte in ein Ticket). Alle benutzen die **gemeinsame** Historie der Ticket-Oberfläche
//! (`TicketUi::history`, wie Board und Ticketliste) – ein Strg+Z geht also auch über den Reiterwechsel hinweg weiter zurück.
//!
//! Dieselben Sperren wie auf dem Board: Solange ein Textfeld den Fokus hat, ein Dialog oder das Schnellmenü offen ist oder
//! gerade gezogen wird, gehört die Taste dem Feld bzw. passiert nichts.

use super::{Cx, PlanUi, Tab};
use crate::state::Lang;
use eframe::egui::{self, Key, Modifiers};
use std::time::Instant;

/// Gedrückt: `Some(false)` = rückgängig, `Some(true)` = wiederholen. Die Taste wird verbraucht.
fn pressed(ctx: &egui::Context) -> Option<bool> {
    ctx.input_mut(|i| {
        // Wiederholen zuerst: Strg+Umschalt+Z würde sonst als Strg+Z gelten (zusätzliches Umschalt wird ignoriert)
        let redo = i.consume_key(Modifiers::CTRL | Modifiers::SHIFT, Key::Z) || i.consume_key(Modifiers::CTRL, Key::Y);
        if redo {
            Some(true)
        } else if i.consume_key(Modifiers::CTRL, Key::Z) {
            Some(false)
        } else {
            None
        }
    })
}

impl PlanUi {
    /// Sperren, die für alle Seiten gelten: Dialoge der Ticket-Oberfläche und der Anwendung, Schnellmenü, Textfeld mit Fokus.
    fn undo_blocked(&self, ctx: &egui::Context) -> bool {
        self.tickets.any_modal() || self.quick.is_open() || self.wizard.is_some() || self.settings.is_some() || self.delete_project.is_some() || self.del.is_some() || ctx.wants_keyboard_input()
    }

    /// Reiter „Backlog“ (Unteransicht Backlog): Strg+Z / Strg+Y für Verschieben und Umsortieren.
    pub(super) fn sprint_keys(&mut self, ctx: &egui::Context, cx: &Cx) {
        let st = &self.sprints;
        if st.is_dragging() || st.form.is_some() || st.del.is_some() || st.complete.is_some() || st.cal_dlg.is_some() || self.releases.form.is_some() || self.undo_blocked(ctx) {
            return;
        }
        if let Some(redo) = pressed(ctx) {
            self.sprint_undo(cx, redo);
        }
    }

    /// Nimmt die letzte Änderung der gemeinsamen Historie zurück (`redo = false`) bzw. stellt sie wieder her; Meldung wie auf dem Board.
    pub(super) fn sprint_undo(&mut self, cx: &Cx, redo: bool) {
        let lang = cx.lang;
        let label = if redo { self.tickets.history.next_redo() } else { self.tickets.history.next_undo() }.map(|op| super::ticket_state::op_label(op, lang));
        let r = if redo { self.tickets.history.redo(cx.conn) } else { self.tickets.history.undo(cx.conn) };
        match r {
            Ok(true) => {
                let l = label.unwrap_or_default();
                self.note(match (lang, redo) {
                    (Lang::De, false) => format!("Rückgängig: {l}"),
                    (Lang::En, false) => format!("Undone: {l}"),
                    (Lang::De, true) => format!("Wiederholt: {l}"),
                    (Lang::En, true) => format!("Redone: {l}"),
                });
            }
            Ok(false) => {}
            Err(e) => {
                let what = if redo { lang.t("Wiederholen nicht möglich", "Cannot redo") } else { lang.t("Rückgängig nicht möglich", "Cannot undo") };
                self.msg = Some(super::Msg { text: format!("{what}: {e}"), ok: false, at: Instant::now() });
            }
        }
        self.tickets.invalidate();
        self.todo.dirty = true;
        self.sprint_changed();
    }

    /// Übersicht des System-Projekts „Todolist“: Strg+Z / Strg+Y nimmt z. B. die Umwandlung einer Karte zurück (Karte wieder offen).
    pub(super) fn todo_keys(&mut self, ctx: &egui::Context, cx: &Cx) {
        if self.tab != Tab::Overview || self.undo_blocked(ctx) {
            return;
        }
        if let Some(redo) = pressed(ctx) {
            self.ticket_undo(cx, redo);
        }
    }
}
