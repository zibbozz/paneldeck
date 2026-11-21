//! Test-Schnittstelle des Tutorials (#143) – **nur im Test-Build** mit dem Cargo-Feature `tutorial-test`
//! (`cargo build --release --features tutorial-test`). Die Release-.exe enthält diesen Code nicht (`tools/release.sh` prüft,
//! dass der Name der Umgebungsvariablen nicht in der Programmdatei steht); Agenten sehen ihn nie, er ist kein MCP-Werkzeug.
//!
//! Mit `PANELDECK_TUTORIAL_TEST=<ordner>` liest die Instanz in jedem Bild Befehle aus `<ordner>\tutorial-cmd.txt` (eine
//! Zeile je Befehl; die Datei wird danach gelöscht) und schreibt ihren Zustand nach `<ordner>\tutorial-status.json`, sobald
//! er sich ändert. Befehle:
//!
//! * `start <tour>` · `continue` · `whatsnew` · `dismiss` – wie die Hilfe
//! * `next` · `back` · `skip` · `quit` – wie die Knöpfe der Blase
//! * `end delete` · `end keep` – Rückfrage am Ende
//! * `card start|later|never` – Karte beim ersten Start
//! * `whatsnew_from <version>` – so tun, als sei die zuletzt benutzte Version `<version>` (Tour „Was ist neu“ prüfen)
//!
//! Der Zustand nennt Tour, Schritt, Ziel (gefunden/Rechteck), Erkennung, Voraussetzung, Rückfrage, Karte, Hinweis,
//! Tutorial-Projekt und den Katalog aller Touren und Schritte (damit der Smoke-Test jeden Schritt prüfen kann).

use super::*;
use serde_json::{json, Value};
use std::path::PathBuf;

/// Name der Umgebungsvariablen (nur im Test-Build vorhanden).
pub(super) const ENV: &str = "PANELDECK_TUTORIAL_TEST";
const CMD_FILE: &str = "tutorial-cmd.txt";
const STATUS_FILE: &str = "tutorial-status.json";

#[derive(Default)]
pub(super) struct Hook {
    checked: bool,
    dir: Option<PathBuf>,
    last: String,
}

impl Hook {
    fn dir(&mut self) -> Option<PathBuf> {
        if !self.checked {
            self.checked = true;
            self.dir = std::env::var_os(ENV).filter(|v| !v.is_empty()).map(PathBuf::from);
            if let Some(d) = &self.dir {
                let _ = std::fs::create_dir_all(d);
            }
        }
        self.dir.clone()
    }
}

/// Befehle aus der Datei lesen und die Datei löschen.
fn take_commands(dir: &std::path::Path) -> Vec<String> {
    let f = dir.join(CMD_FILE);
    let Ok(text) = std::fs::read_to_string(&f) else { return Vec::new() };
    let _ = std::fs::remove_file(&f);
    crate::state::strip_bom(&text).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
}

/// Katalog aller Touren und Schritte.
fn catalogue() -> Value {
    let tours: Vec<Value> = steps::TOURS
        .iter()
        .map(|t| {
            let list: Vec<Value> = t.steps.iter().map(|s| json!({"id": s.id, "since": s.since, "spot": s.spot.map(|x| x.id()), "waits": s.done.is_some(), "need": s.need.map(|n| format!("{n:?}").to_lowercase())})).collect();
            json!({"id": t.id, "title_en": t.title.en, "title_de": t.title.de, "steps": list})
        })
        .collect();
    json!(tours)
}

impl App {
    /// Befehle ausführen und Zustand schreiben (jedes Bild im Hauptfenster).
    pub(super) fn tut_hook(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(dir) = self.tut.hook.dir() else { return };
        for c in take_commands(&dir) {
            let mut it = c.splitn(2, ' ');
            let (verb, arg) = (it.next().unwrap_or(""), it.next().unwrap_or("").trim());
            match (verb, arg) {
                ("start", id) => {
                    if let Some(t) = steps::tour(id) {
                        self.tut_cmd(ctx, Cmd::Start(t.id));
                    } else if id == steps::WHATS_NEW {
                        self.tut_cmd(ctx, Cmd::WhatsNew);
                    }
                }
                ("continue", _) => self.tut_cmd(ctx, Cmd::Continue),
                ("whatsnew", _) => self.tut_cmd(ctx, Cmd::WhatsNew),
                ("dismiss", _) => self.tut_cmd(ctx, Cmd::DismissHint),
                ("next", _) => self.tut_click(overlay::Click::Next),
                ("back", _) => self.tut_click(overlay::Click::Back),
                ("skip", _) => self.tut_click(overlay::Click::Skip),
                ("quit", _) => {
                    if self.tut.run.is_some() {
                        self.tut_click(overlay::Click::Quit);
                    }
                }
                ("end", "delete") if self.tut.ask.is_some() => self.tut_answer(overlay::EndClick::Delete, actions),
                ("end", "keep") if self.tut.ask.is_some() => self.tut_answer(overlay::EndClick::Keep, actions),
                ("card", a) => {
                    match a {
                        "start" => self.tut_cmd(ctx, Cmd::Start("basics")),
                        "later" => {
                            self.tut.card_hidden = true;
                            self.tut.p.card = store::Card::Later;
                        }
                        "never" => self.tut.p.card = store::Card::Never,
                        _ => {}
                    }
                    self.tut_save();
                }
                ("whatsnew_from", v) => {
                    self.tut.p.whats_new_after = v.to_string();
                    self.tut_save();
                }
                _ => {}
            }
            ctx.request_repaint();
        }
        let status = self.tut_status();
        let text = serde_json::to_string_pretty(&status).unwrap_or_default();
        if text != self.tut.hook.last {
            let _ = crate::state::write_atomic(&dir.join(STATUS_FILE), text.as_bytes());
            self.tut.hook.last = text;
        }
        // Weiter zeichnen, damit Befehle auch im verborgenen/minimierten Fenster ankommen.
        ctx.request_repaint_after(Duration::from_millis(150));
    }

    fn tut_status(&self) -> Value {
        let pid = self.tut.p.project;
        let exists = pid.is_some_and(|id| self.projects.iter().any(|p| p.id == id));
        let sessions = self.entries.values().filter(|e| Some(e.meta.project) == pid).count();
        let run = self.tut.run.as_ref().map(|r| {
            let s = r.step();
            let seen = s.spot.and_then(marks::seen);
            let ready = r.frames >= 8 || (r.frames >= 4 && (s.spot.is_none() || seen.is_some_and(|x| x.shown.is_positive())));
            json!({
                "tour": r.tour,
                "step": s.id,
                "index": r.idx + 1,
                "total": r.steps.len(),
                "since": s.since,
                "spot": s.spot.map(|x| x.id()),
                "spot_found": seen.is_some_and(|x| x.shown.is_positive()),
                "spot_rect": seen.map(|x| [x.rect.min.x, x.rect.min.y, x.rect.width(), x.rect.height()]),
                "waits": s.done.is_some(),
                "done": r.done_at.is_some(),
                "need_missing": r.missing.map(|n| format!("{n:?}").to_lowercase()),
                "ready": ready,
                "steps": r.steps.iter().map(|x| x.id).collect::<Vec<_>>(),
            })
        });
        json!({
            "running": run.is_some(),
            "run": run,
            "ask": self.tut.ask.map(|f| if f { "finished" } else { "paused" }),
            "card": self.tut.card_shown,
            "hint": self.tut_hint(),
            "lang": if self.settings.language == Lang::De { "de" } else { "en" },
            "mode": if self.mode == titlebar::Mode::Tasks { "tasks" } else { "terminals" },
            "expert": self.plan.expert,
            "project": {"id": pid, "exists": exists, "sessions": sessions, "active": pid == Some(self.active_project)},
            "todos": self.tut.p.todos,
            "plan_project": self.tut.p.plan_project,
            "progress": serde_json::to_value(&self.tut.p.tours).unwrap_or(Value::Null),
            "whats_new_after": self.tut.p.whats_new_after,
            "catalogue": catalogue(),
        })
    }
}
