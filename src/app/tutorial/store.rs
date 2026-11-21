//! Fortschritt des Tutorials in `tutorial.json` im Datenordner (#143). Internes Format (keine öffentliche API): es darf
//! sich ändern; unbekannte oder unlesbare Inhalte führen zu einem frischen Stand. `settings.json`/`state.json` bleiben
//! unberührt (bis auf den Schalter „Was ist neu“-Tour in den Einstellungen).

use crate::state;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(super) const FILE: &str = "tutorial.json";

/// Karte „Kurze Tour?“ beim ersten Start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Card {
    /// Noch nicht beantwortet (wird angeboten).
    #[default]
    Open,
    /// „Später“: beim nächsten Start wieder anbieten.
    Later,
    /// „Nie“ oder eine Tour wurde gestartet: nicht mehr anbieten.
    Never,
}

/// Fortschritt einer Tour.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct TourProgress {
    /// Kennung des Schritts, bei dem „Fortsetzen“ weitermacht (leer = von vorn).
    pub step: String,
    /// Bis zum Ende durchlaufen.
    pub finished: bool,
    /// Übersprungene Schritte (Kennungen).
    pub skipped: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Progress {
    pub version: u32,
    pub card: Card,
    /// Programmversion beim letzten Start (für „Was ist neu“).
    pub last_version: String,
    /// Steht eine Tour „Was ist neu“ an: Schritte nach dieser Version (leer = keine).
    pub whats_new_after: String,
    /// Fortschritt je Tour (Kennung der Tour).
    pub tours: BTreeMap<String, TourProgress>,
    /// Zuletzt laufende Tour (für „Fortsetzen“).
    pub last_tour: String,
    /// Projekt „Tutorial“ im Terminal Hub (Kennung), solange es existiert.
    pub project: Option<u64>,
    /// Projekt vor dem Start der Tour (dorthin geht es beim Aufräumen zurück).
    pub prev_project: Option<u64>,
    /// Experten-Projekt der Tour in der Datenbank.
    pub plan_project: Option<i64>,
    /// In der Tour angelegte To-dos (werden beim Aufräumen gelöscht).
    pub todos: Vec<i64>,
    /// Höchste To-do-Nummer beim Start der Tour (neuere zählen zur Tour).
    pub todo_base: i64,
    /// Expertenmodus vor der Tour.
    pub expert_before: Option<bool>,
}

impl Default for Progress {
    fn default() -> Self {
        Progress {
            version: 1,
            card: Card::Open,
            last_version: String::new(),
            whats_new_after: String::new(),
            tours: BTreeMap::new(),
            last_tour: String::new(),
            project: None,
            prev_project: None,
            plan_project: None,
            todos: Vec::new(),
            todo_base: 0,
            expert_before: None,
        }
    }
}

impl Progress {
    /// Tour, die „Fortsetzen“ weiterführt: die zuletzt laufende, wenn sie einen gemerkten Schritt hat. `finished` zählt
    /// dabei nicht – es bleibt nach dem ersten vollständigen Durchlauf stehen (Haken in der Liste), auch wenn eine neue
    /// Runde derselben Tour mittendrin beendet wurde; zu Ende gelaufen leert `step` (siehe `tut_stop`).
    pub(super) fn resumable(&self) -> Option<&str> {
        let tp = self.tours.get(&self.last_tour)?;
        (!tp.step.is_empty()).then_some(self.last_tour.as_str())
    }
}

pub(super) fn path(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

/// Laden; fehlt die Datei oder ist sie unlesbar, ein frischer Stand (`load_json` sichert eine kaputte Datei vorher).
pub(super) fn load(dir: &Path) -> Progress {
    let mut p: Progress = state::load_json(&path(dir)).unwrap_or_default();
    p.version = 1;
    p
}

pub(super) fn save(dir: &Path, p: &Progress) {
    state::save_json(&path(dir), p);
}

/// Beim Start: Version merken und entscheiden, ob eine Tour „Was ist neu“ ansteht (Schritte nach der zuletzt benutzten
/// Version). Ein erster Start (noch keine Version bekannt) bekommt keine – dort gibt es die Karte „Kurze Tour?“.
pub(super) fn note_version(p: &mut Progress, current: &str) {
    if p.last_version.is_empty() {
        p.last_version = current.to_string();
        return;
    }
    if super::steps::ver(&p.last_version) < super::steps::ver(current) {
        if !super::steps::whats_new_steps(&p.last_version, current).is_empty() && p.whats_new_after.is_empty() {
            p.whats_new_after = p.last_version.clone();
        }
        p.last_version = current.to_string();
    }
}
