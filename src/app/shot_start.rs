//! Nur mit dem Cargo-Feature `readme-shots`: beim Start in den Reiter „Todo-Liste“ wechseln (siehe `planning_ui/shot_view.rs`).

use super::{planning_ui, titlebar, App};

impl App {
    /// Setzt beim ersten Bild den Hauptbereich „Todo-Liste“, wenn `PANELDECK_SHOT` gesetzt ist.
    pub(super) fn dev_shot_mode(&mut self) {
        if planning_ui::shot_view::pending_spec().is_some() && self.mode != titlebar::Mode::Tasks {
            self.mode = titlebar::Mode::Tasks;
        }
    }
}
