//! Nur mit dem Cargo-Feature `readme-shots`: beim Start in den Reiter „Todo-Liste“ wechseln (siehe `planning_ui/shot_view.rs`).
//! Außerdem für Bildschirmfotos ohne Maus: `PANELDECK_SHOT_SETTINGS=<Reiter>` öffnet die Einstellungen in diesem Reiter
//! (7 = Agenten-Schnittstelle), `PANELDECK_SHOT_USAGE=<transkript.jsonl>` hängt ein Claude-Transkript an alle Sitzungen
//! (Gesamtverbrauch unten in der Seitenleiste).

use super::{planning_ui, titlebar, App};
use std::sync::atomic::{AtomicBool, Ordering};

static SETTINGS_DONE: AtomicBool = AtomicBool::new(false);

impl App {
    /// Setzt beim ersten Bild den Hauptbereich „Todo-Liste“, wenn `PANELDECK_SHOT` gesetzt ist.
    pub(super) fn dev_shot_mode(&mut self) {
        if planning_ui::shot_view::pending_spec().is_some() && self.mode != titlebar::Mode::Tasks {
            self.mode = titlebar::Mode::Tasks;
        }
        if let Ok(tab) = std::env::var("PANELDECK_SHOT_SETTINGS") {
            if !SETTINGS_DONE.swap(true, Ordering::Relaxed) {
                self.settings_open = true;
                self.settings_tab = tab.trim().parse().unwrap_or(0);
            }
        }
        if let Ok(p) = std::env::var("PANELDECK_SHOT_USAGE") {
            for id in self.entries.keys().copied().collect::<Vec<_>>() {
                self.usage.set_path(id, std::path::PathBuf::from(&p));
            }
        }
    }
}
