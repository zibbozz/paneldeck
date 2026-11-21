//! Interaktives Tutorial (#143, 3.0.0): kurze Tour und Kapitel als geführte Schritte in der echten Oberfläche.
//!
//! * **Ziele** (`marks`): Bedienelemente melden pro Bild ihr Rechteck unter einer `Spot`-Kennung, nur während einer Tour und
//!   nur im Hauptfenster. Die Tour liest sie am Ende des Bilds und zeichnet Abdunklung, Rahmen und Sprechblase (`overlay`).
//! * **Schritte** (`steps`): Tabelle mit Texten in beiden Sprachen, Version, Ziel, Vorbereitungen und Erkennung.
//! * **Erkennung**: kein Rückruf in der Oberfläche – pro Bild wird ein kleiner Zustand erhoben (`Probe`: Zahl der
//!   Sitzungen/Bereiche im Tutorial-Projekt, offene Palette, Reiter, To-dos …) und mit dem Zustand beim Betreten verglichen.
//! * **Daten**: Die Tour läuft in einem eigenen Projekt „Tutorial“ mit eigenen Sitzungen; In der Tour angelegte To-dos und
//!   das Experten-Projekt der Tour werden gemerkt. Am Ende (und nach dem Beenden) fragt Paneldeck, ob das alles gelöscht
//!   (Standard) oder behalten wird. Fortschritt in `tutorial.json` (`store`, intern).
//! * **Test-Schnittstelle** (`hook`): nur im Test-Build mit dem Cargo-Feature `tutorial-test`; die Release-.exe enthält sie nicht.

mod marks;
mod overlay;
mod prereq;
mod steps;
mod store;

#[cfg(feature = "tutorial-test")]
mod hook;
#[cfg(test)]
mod tests;

pub(super) use marks::{spot_rect, spot_since, Spot, TutMark};

use super::*;
use steps::{Need, Prep, Probe, Step, Txt};

/// Befehle aus der Hilfe (Befehls-Palette, Einstellungen → Info, leerer Terminal Hub, Hinweis in der Fußzeile).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Cmd {
    /// Tour von vorn starten (Kennung aus `steps::TOURS`).
    Start(&'static str),
    /// Zuletzt laufende Tour dort fortsetzen, wo sie beendet wurde (sonst die kurze Tour).
    Continue,
    /// Tour „Was ist neu“.
    WhatsNew,
    /// Hinweis „Neu in …“ ausblenden.
    DismissHint,
}

/// Eine laufende Tour.
struct Run {
    /// Kennung der Tour (`steps::WHATS_NEW` für „Was ist neu“).
    tour: &'static str,
    title: Txt,
    steps: Vec<&'static Step>,
    idx: usize,
    /// Zustand beim Betreten des Schritts (Grundlage der Erkennung).
    start: Probe,
    /// Bilder seit dem Betreten.
    frames: u32,
    /// Aktion erkannt (kurz „Erledigt“, dann weiter).
    done_at: Option<Instant>,
    /// Fehlende Voraussetzung des aktuellen Schritts.
    missing: Option<Need>,
}

impl Run {
    fn step(&self) -> &'static Step {
        self.steps[self.idx.min(self.steps.len() - 1)]
    }
}

/// Zustand des Tutorials in der App (nur das Hauptfenster benutzt ihn).
#[derive(Default)]
pub(super) struct Tut {
    loaded: bool,
    p: store::Progress,
    run: Option<Run>,
    /// Rückfrage „Tutorial-Projekt löschen?“ offen: `Some(true)` nach dem Ende, `Some(false)` nach dem Beenden.
    ask: Option<bool>,
    /// Karte „Später“ in diesem Lauf.
    card_hidden: bool,
    /// Karte in diesem Bild gezeigt (Test-Schnittstelle).
    card_shown: bool,
    /// Automatische Tour „Was ist neu“ schon geprüft.
    auto_checked: bool,
    /// Zahlen aus der Datenbank (To-dos, erledigte, Tickets) und wann sie zuletzt gelesen wurden.
    db_counts: (usize, usize, usize),
    db_at: Option<Instant>,
    #[cfg(feature = "tutorial-test")]
    hook: hook::Hook,
}

/// Text in der Sprache `lang`.
fn t(lang: Lang, x: Txt) -> &'static str {
    lang.t(x.de, x.en)
}

/// Esc beendet die Tour, wenn nichts anderes sie braucht: kein Popup, Menü, Modal oder Fenster offen und kein Textfeld mit
/// dem Fokus (ein Terminal mit Fokus zählt nicht – die Tour hat Vorrang).
fn esc_for_tour(ctx: &egui::Context) -> bool {
    if egui::Popup::is_any_open(ctx) {
        return false;
    }
    let own = [Id::new(overlay::BUBBLE_ID), Id::new(overlay::CARD_ID)];
    let busy = ctx.memory(|m| m.top_modal_layer().is_some() || m.areas().visible_layer_ids().iter().any(|l| l.order == egui::Order::Middle && !own.contains(&l.id)));
    if busy {
        return false;
    }
    !ctx.memory(|m| m.focused()).is_some_and(|id| egui::TextEdit::load_state(ctx, id).is_some())
}

/// Platzhalter `{key:<id>}` durch die aktuellen Kürzel ersetzen (`key` liefert den Text, leer = kein Kürzel).
pub(super) fn fill_keys(text: &str, lang: Lang, key: impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("{key:") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 5..];
        match after.find('}') {
            Some(j) => {
                let k = key(&after[..j]);
                out.push_str(if k.is_empty() { lang.t("ohne Kürzel", "no shortcut") } else { &k });
                rest = &after[j + 1..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Prüfsumme über eigene Namen und Farben von Sitzungen (Umbenennen/Farbe erkennen; automatische Namen zählen nicht).
fn names_hash<'a>(it: impl Iterator<Item = (&'a u64, &'a Entry)>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for (id, e) in it {
        id.hash(&mut h);
        e.meta.custom_name.hash(&mut h);
        if e.meta.custom_name {
            e.meta.name.hash(&mut h);
        }
        e.meta.color.hash(&mut h);
    }
    h.finish()
}

impl App {
    /// Anfang jedes Bilds: gemeldete Ziele verwerfen; Esc für die laufende Tour abfangen (bevor ein Terminal es bekommt).
    pub(super) fn tut_begin(&mut self, ctx: &egui::Context) {
        let want = self.tut.run.as_ref().and_then(|r| r.step().spot);
        marks::begin_frame(self.tut.run.is_some(), want);
        if self.tut.run.is_some() && self.tut.ask.is_none() && self.tut_esc_free() && esc_for_tour(ctx) && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.tut_quit();
        }
    }

    /// Ist kein Dialog der App offen, der Esc selbst braucht?
    fn tut_esc_free(&self) -> bool {
        !self.settings_open
            && self.palette.is_none()
            && self.new_dialog.is_none()
            && self.new_chat.is_none()
            && self.confirm_close.is_none()
            && self.text_prompt.is_none()
            && self.rename.is_none()
            && !self.history_open
            && !self.prompts_open
            && !self.templates_open
            && self.session_settings.is_none()
    }

    fn tut_load(&mut self) {
        if self.tut.loaded {
            return;
        }
        self.tut.loaded = true;
        self.tut.p = store::load(&self.dir);
        store::note_version(&mut self.tut.p, env!("CARGO_PKG_VERSION"));
        // Projekt der Tour gibt es nicht mehr (z. B. „Sitzungen merken“ aus): vergessen.
        if self.tut.p.project.is_some_and(|id| !self.projects.iter().any(|p| p.id == id)) {
            self.tut.p.project = None;
        }
        store::save(&self.dir, &self.tut.p);
    }

    fn tut_save(&self) {
        store::save(&self.dir, &self.tut.p);
    }

    /// Ende jedes Bilds im Hauptfenster: Tour weiterführen und zeichnen, Karte beim ersten Start, Rückfrage am Ende.
    pub(super) fn tut_frame(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if self.wins.cur != 0 {
            return;
        }
        self.tut_load();
        // Die Test-Schnittstelle schreibt den Stand vom Ende des vorigen Bilds (`card_shown`) und führt Befehle aus.
        #[cfg(feature = "tutorial-test")]
        self.tut_hook(ctx, actions);
        self.tut.card_shown = false;
        let lang = self.settings.language;
        if let Some(finished) = self.tut.ask {
            if let Some(c) = overlay::end_ask(ctx, lang, finished, &self.tut_what()) {
                self.tut_answer(c, actions);
            }
            return;
        }
        if self.tut.run.is_some() {
            self.tut_step(ctx);
            return;
        }
        if self.tut_card_due() {
            self.tut.card_shown = true;
            match overlay::card(ctx, lang) {
                Some(overlay::CardClick::Start) => self.tut_cmd(ctx, Cmd::Start("basics")),
                Some(overlay::CardClick::Later) => {
                    self.tut.card_hidden = true;
                    self.tut.p.card = store::Card::Later;
                    self.tut_save();
                }
                Some(overlay::CardClick::Never) => {
                    self.tut.p.card = store::Card::Never;
                    self.tut_save();
                }
                None => {}
            }
        }
        // „Was ist neu“ nach einem Update automatisch, wenn eingeschaltet (Standard: aus, dann nur der Hinweis).
        if !self.tut.auto_checked && self.mode == titlebar::Mode::Terminals {
            self.tut.auto_checked = true;
            if self.settings.tutorial_whats_new && !self.tut.p.whats_new_after.is_empty() && !self.tut_card_due() {
                self.tut_cmd(ctx, Cmd::WhatsNew);
            }
        }
    }

    /// Karte „Kurze Tour?“ jetzt zeigen? Nur im Terminal Hub ohne offene Dialoge, nicht im Tray und nicht für README-Bilder.
    fn tut_card_due(&self) -> bool {
        !cfg!(feature = "readme-shots")
            && self.tut.p.card != store::Card::Never
            && !self.tut.card_hidden
            && self.mode == titlebar::Mode::Terminals
            && !self.in_tray
            && self.tut_esc_free()
            && self.cleanup.is_none()
    }

    /// Befehl aus der Hilfe ausführen.
    pub(super) fn tut_cmd(&mut self, ctx: &egui::Context, cmd: Cmd) {
        // Die Tour läuft nur im Hauptfenster.
        if self.wins.cur != 0 {
            return;
        }
        self.tut_load();
        match cmd {
            Cmd::Start(id) => self.tut_start(ctx, id, false),
            Cmd::Continue => {
                let last = self.tut.p.last_tour.clone();
                let resumable = steps::tour(&last).is_some_and(|_| self.tut.p.tours.get(&last).is_some_and(|tp| !tp.finished && !tp.step.is_empty()));
                if resumable {
                    let id = steps::tour(&last).map(|t| t.id).unwrap_or("basics");
                    self.tut_start(ctx, id, true);
                } else {
                    self.tut_start(ctx, "basics", false);
                }
            }
            Cmd::WhatsNew => self.tut_start(ctx, steps::WHATS_NEW, false),
            Cmd::DismissHint => {
                self.tut.p.whats_new_after.clear();
                self.tut_save();
            }
        }
    }

    /// Tour `id` starten (`resume`: beim gemerkten Schritt).
    fn tut_start(&mut self, ctx: &egui::Context, id: &str, resume: bool) {
        let current = env!("CARGO_PKG_VERSION");
        let (tour, title, list): (&'static str, Txt, Vec<&'static Step>) = if id == steps::WHATS_NEW {
            let after = if self.tut.p.whats_new_after.is_empty() { self.tut.p.last_version.clone() } else { self.tut.p.whats_new_after.clone() };
            (steps::WHATS_NEW, steps::WHATS_NEW_TITLE, steps::whats_new_steps(&after, current))
        } else {
            match steps::tour(id) {
                Some(t) => (t.id, t.title, t.steps.iter().collect()),
                None => return,
            }
        };
        if list.is_empty() {
            return;
        }
        // Eine laufende Tour endet still (ohne Rückfrage; das Projekt bleibt für die neue).
        if self.tut.run.is_some() {
            self.tut_stop(false);
        }
        self.tut.ask = None;
        let idx = if resume {
            let at = self.tut.p.tours.get(tour).map(|tp| tp.step.clone()).unwrap_or_default();
            list.iter().position(|s| s.id == at).unwrap_or(0)
        } else {
            0
        };
        self.tut.p.card = store::Card::Never;
        self.tut.p.last_tour = tour.to_string();
        if self.tut.p.expert_before.is_none() {
            self.plan.ensure_loaded(self.db.as_ref());
            self.tut.p.expert_before = Some(self.plan.expert);
        }
        // Neue To-dos ab hier gehören zur Tour.
        if let Some(db) = self.db.as_ref() {
            self.tut.p.todo_base = db.conn.query_row("SELECT COALESCE(MAX(id), 0) FROM todo_items", [], |r| r.get(0)).unwrap_or(0);
        }
        // Jede Tour läuft im Projekt „Tutorial“ (neue Sitzungen landen dort, die eigenen bleiben unberührt).
        self.tut_enter_project();
        self.tut.db_at = None;
        self.tut.run = Some(Run { tour, title, steps: list, idx, start: Probe::default(), frames: 0, done_at: None, missing: None });
        self.tut_save();
        ctx.request_repaint();
    }

    /// Projekt „Tutorial“ anlegen, falls es fehlt; liefert seine Kennung.
    fn tut_ensure_project(&mut self) -> u64 {
        if let Some(id) = self.tut.p.project.filter(|id| self.projects.iter().any(|p| p.id == *id)) {
            return id;
        }
        let id = self.projects.iter().map(|p| p.id).max().unwrap_or(0) + 1;
        // Standardprofil wie das aktive Projekt: Tutorial-Terminals starten wie die gewohnten.
        let profile = self.projects.iter().find(|p| p.id == self.active_project).map(|p| p.profile).unwrap_or(0);
        self.projects.push(Project { id, name: "Tutorial".to_string(), color: hub_color(id), profile, ..Project::default() });
        self.tut.p.project = Some(id);
        id
    }

    /// Tour anhalten: `finished` = bis zum Ende gelaufen. Expertenmodus zurück, Rückfrage zum Projekt, falls es eins gibt.
    fn tut_stop(&mut self, finished: bool) {
        let Some(run) = self.tut.run.take() else { return };
        let tp = self.tut.p.tours.entry(run.tour.to_string()).or_default();
        if finished {
            tp.finished = true;
            tp.step.clear();
        } else {
            tp.step = run.step().id.to_string();
        }
        if finished && run.tour == steps::WHATS_NEW {
            self.tut.p.whats_new_after.clear();
        }
        if let (Some(before), Some(db)) = (self.tut.p.expert_before.take(), self.db.as_ref()) {
            if self.plan.expert != before {
                self.plan.set_expert(db, before);
            }
        }
        self.palette = None;
        self.tut_save();
    }

    /// „×“/Esc: Tour beenden (Fortschritt bleibt) und fragen, ob das Tutorial-Projekt bleibt.
    fn tut_quit(&mut self) {
        self.tut_stop(false);
        self.tut.ask = self.tut_has_data().then_some(false);
    }

    fn tut_finish(&mut self) {
        self.settings_open = false;
        self.tut_stop(true);
        self.tut.ask = self.tut_has_data().then_some(true);
    }

    /// Gibt es etwas, das die Tour angelegt hat (Projekt, To-dos, Experten-Projekt)?
    fn tut_has_data(&self) -> bool {
        self.tut.p.project.is_some_and(|id| self.projects.iter().any(|p| p.id == id)) || !self.tut.p.todos.is_empty() || self.tut.p.plan_project.is_some()
    }

    /// Was zum Tutorial-Projekt gehört (für die Rückfrage).
    fn tut_what(&self) -> String {
        let lang = self.settings.language;
        let pid = self.tut.p.project;
        let n = self.entries.values().filter(|e| Some(e.meta.project) == pid).count();
        let mut parts = vec![match lang {
            Lang::De => format!("Projekt „Tutorial“ mit {n} Sitzung{}", if n == 1 { "" } else { "en" }),
            Lang::En => format!("project “Tutorial” with {n} session{}", if n == 1 { "" } else { "s" }),
        }];
        let m = self.tut.p.todos.len();
        if m > 0 {
            parts.push(match lang {
                Lang::De => format!("{m} To-do{}", if m == 1 { "" } else { "s" }),
                Lang::En => format!("{m} to-do{}", if m == 1 { "" } else { "s" }),
            });
        }
        if self.tut.p.plan_project.is_some() {
            parts.push(lang.t("das Experten-Projekt „Tutorial“ mit seinen Tickets", "the expert project “Tutorial” with its tickets").to_string());
        }
        format!("{}.", parts.join(", "))
    }

    /// Antwort auf die Rückfrage am Ende.
    fn tut_answer(&mut self, c: overlay::EndClick, actions: &mut Vec<Action>) {
        self.tut.ask = None;
        match c {
            overlay::EndClick::Delete => self.tut_cleanup(actions),
            overlay::EndClick::Keep => self.tut.p.prev_project = None,
        }
        self.tut_save();
    }

    /// Alles löschen, was die Tour angelegt hat: Sitzungen und Projekt „Tutorial“, To-dos, Experten-Projekt.
    fn tut_cleanup(&mut self, actions: &mut Vec<Action>) {
        if let Some(pid) = self.tut.p.project.take() {
            let ids: Vec<u64> = self.entries.iter().filter(|(_, e)| e.meta.project == pid).map(|(i, _)| *i).collect();
            if !ids.is_empty() {
                actions.push(Action::CloseMany(ids));
            }
            if let Some(prev) = self.tut.p.prev_project.take().filter(|p| *p != pid && self.projects.iter().any(|x| x.id == *p)) {
                actions.push(Action::SwitchProject(prev));
            }
            if self.projects.iter().any(|p| p.id == pid) {
                actions.push(Action::DeleteProject(pid));
            }
        }
        self.tut.p.prev_project = None;
        if let Some(db) = self.db.as_ref() {
            for id in std::mem::take(&mut self.tut.p.todos) {
                let _ = crate::tasks::delete_item(&db.conn, id);
            }
            if let Some(id) = self.tut.p.plan_project.take() {
                let _ = crate::planning::projects::delete_project(&db.conn, id);
            }
            self.tasks.invalidate();
            self.plan.invalidate();
        }
    }

    /// Zustand für die Erkennung erheben. Zahlen aus der Datenbank höchstens alle 300 ms.
    fn tut_probe(&mut self) -> Probe {
        let pid = self.tut.p.project;
        let mine: Vec<(&u64, &Entry)> = self.entries.iter().filter(|(_, e)| Some(e.meta.project) == pid).collect();
        let sessions = mine.len();
        let minimized = mine.iter().filter(|(_, e)| e.meta.minimized).count();
        let names = names_hash(mine.into_iter());
        let panes = if pid == Some(self.active_project) { self.layout.as_ref().map(Node::leaf_count).unwrap_or(0) } else { 0 };
        if self.tut.db_at.is_none_or(|t| t.elapsed() >= Duration::from_millis(300)) {
            self.tut.db_at = Some(Instant::now());
            if let Some(db) = self.db.as_ref() {
                let (n, d) = crate::tasks::counts(&db.conn).unwrap_or((0, 0));
                let tickets: i64 = match self.tut.p.plan_project {
                    Some(id) => db.conn.query_row("SELECT COUNT(*) FROM tickets WHERE project_id = ?1", [id], |r| r.get(0)).unwrap_or(0),
                    None => 0,
                };
                self.tut.db_counts = (n, d, tickets.max(0) as usize);
                // Neue To-dos seit dem Start gehören zur Tour (die Nummern werden nie wiederverwendet).
                let base = self.tut.p.todo_base;
                let new: Vec<i64> = db
                    .conn
                    .prepare("SELECT id FROM todo_items WHERE id > ?1")
                    .and_then(|mut s| s.query_map([base], |r| r.get::<_, i64>(0)).map(|rows| rows.flatten().collect()))
                    .unwrap_or_default();
                let mut changed = false;
                for id in new {
                    if !self.tut.p.todos.contains(&id) {
                        self.tut.p.todos.push(id);
                        changed = true;
                    }
                }
                if changed {
                    self.tut_save();
                }
            }
        }
        let (todos, todos_done, tickets) = self.tut.db_counts;
        Probe {
            sessions,
            panes,
            minimized,
            names,
            palette: self.palette.is_some(),
            settings: self.settings_open.then_some(self.settings_tab),
            tasks: self.mode == titlebar::Mode::Tasks,
            expert: self.plan.expert,
            todos,
            todos_done,
            tickets,
            plan_tab: self.plan.tab_key(),
            history: self.history_open,
            prompts: self.prompts_open,
            composer: self.composer_open,
            files: self.files_open,
            new_chat: self.new_chat.is_some(),
        }
    }

    /// Vorbereitungen eines Schritts anwenden (nur, was nicht schon passt).
    fn tut_prep(&mut self, ctx: &egui::Context, preps: &[Prep]) {
        for p in preps {
            match *p {
                Prep::Terminals => self.set_mode(titlebar::Mode::Terminals),
                Prep::Tasks => self.set_mode(titlebar::Mode::Tasks),
                Prep::NoSettings => self.settings_open = false,
                Prep::Settings(tab) => {
                    self.settings_open = true;
                    self.settings_tab = tab;
                }
                Prep::TutProject => {
                    self.tut_enter_project();
                }
                Prep::Session => {
                    let pid = self.tut_enter_project();
                    let ids: Vec<u64> = self.entries.iter().filter(|(_, e)| e.meta.project == pid && e.meta.window == 0).map(|(i, _)| *i).collect();
                    match ids.first() {
                        None => {
                            self.new_session(ctx, NewOpts { project: Some(pid), ..NewOpts::default() });
                        }
                        Some(first) if self.layout.is_none() => self.activate(*first),
                        Some(_) => {}
                    }
                }
                Prep::Palette(open) => {
                    if open && self.palette.is_none() {
                        self.palette = Some(Palette { query: String::new(), sel: 0 });
                    } else if !open {
                        self.palette = None;
                    }
                }
                Prep::NoTools => {
                    self.history_open = false;
                    self.prompts_open = false;
                    self.composer_open = false;
                    self.cost_open = false;
                }
                Prep::Expert(on) => {
                    self.plan.ensure_loaded(self.db.as_ref());
                    if let Some(db) = self.db.as_ref() {
                        if self.plan.expert != on {
                            self.plan.set_expert(db, on);
                        }
                    }
                }
                Prep::ExpertRestore => {
                    if let (Some(before), Some(db)) = (self.tut.p.expert_before, self.db.as_ref()) {
                        if self.plan.expert != before {
                            self.plan.set_expert(db, before);
                        }
                    }
                }
                Prep::PlanProject => self.tut_plan_project(),
                Prep::PlanTab(k) => self.plan.set_tab_key(k),
            }
        }
    }

    /// Ins Projekt „Tutorial“ wechseln (vorheriges Projekt merken); liefert seine Kennung.
    fn tut_enter_project(&mut self) -> u64 {
        let pid = self.tut_ensure_project();
        if self.active_project != pid {
            if self.tut.p.prev_project.is_none() {
                self.tut.p.prev_project = Some(self.active_project);
            }
            self.switch_project(pid);
        }
        pid
    }

    /// Experten-Projekt der Tour anlegen (freier Schlüssel `TUT`, `TUT2` …) und wählen.
    fn tut_plan_project(&mut self) {
        let Some(db) = self.db.as_ref() else { return };
        let exists = self.tut.p.plan_project.is_some_and(|id| crate::planning::projects::get_project(&db.conn, id).is_ok());
        if !exists {
            self.tut.p.plan_project = None;
            for n in 1..100 {
                let key = if n == 1 { "TUT".to_string() } else { format!("TUT{n}") };
                let mut np = crate::planning::projects::NewProject::new(&key, "Tutorial", "#61AFEF");
                np.with_defaults = true;
                np.use_sprints = true;
                np.use_versions = true;
                np.description = self.settings.language.t("Vom Tutorial angelegt – wird am Ende gelöscht, wenn du es nicht behältst.", "Created by the tutorial – deleted at the end unless you keep it.").to_string();
                if let Ok(id) = crate::planning::projects::create_project(&db.conn, &np) {
                    self.tut.p.plan_project = Some(id);
                    break;
                }
            }
            self.tut_save();
        }
        if let (Some(id), Some(db)) = (self.tut.p.plan_project, self.db.as_ref()) {
            self.plan.select_project(db, id);
        }
    }

    /// Einen Schritt weiterführen: Vorbereiten, Erkennen, Zeichnen, Klicks.
    fn tut_step(&mut self, ctx: &egui::Context) {
        let lang = self.settings.language;
        let animate = self.settings.animations;
        let Some(run) = self.tut.run.as_ref() else { return };
        let step = run.step();
        if run.frames == 0 {
            self.tut_prep(ctx, step.prep);
            let servers = self.settings.llm_servers.len();
            if let Some(r) = self.tut.run.as_mut() {
                r.missing = step.need.filter(|n| prereq::missing(*n, servers));
            }
        }
        let now = self.tut_probe();
        let Some(run) = self.tut.run.as_mut() else { return };
        run.frames = run.frames.saturating_add(1);
        // Zustand erst nach den Vorbereitungen festhalten (sie wirken erst in den nächsten Bildern).
        if run.frames == 3 {
            run.start = now.clone();
        }
        if run.frames > 3 && run.done_at.is_none() && step.done.is_some_and(|f| f(&run.start, &now)) {
            run.done_at = Some(Instant::now());
        }
        if let Some(at) = run.done_at {
            let wait = Duration::from_millis(if animate { 700 } else { 250 });
            if at.elapsed() >= wait {
                self.tut_go(1, false);
                ctx.request_repaint();
                return;
            }
            ctx.request_repaint_after(Duration::from_millis(60));
        }
        let shown = step.spot.and_then(marks::seen).map(|s| s.shown).filter(|r| r.is_positive());
        if run.frames < 4 || (step.spot.is_some() && shown.is_none() && run.frames < 8) {
            // Ziel taucht nach den Vorbereitungen meist erst ein, zwei Bilder später auf: so lange nichts zeigen.
            ctx.request_repaint();
            return;
        }
        let (index, total) = (run.idx + 1, run.steps.len());
        let missing_spot = step.spot.is_some() && shown.is_none();
        let note = run.missing.map(|n| (t(lang, n.note()), n.link()));
        let tour_title = t(lang, run.title);
        let done = run.done_at.is_some();
        let body = fill_keys(t(lang, step.body), lang, |k| self.binding_text(k));
        let b = overlay::Bubble { lang, tour: tour_title, title: t(lang, step.title), body: &body, note, index, total, waits: step.done.is_some(), done, missing: missing_spot };
        overlay::dim(ctx, shown, animate);
        let (click, _) = overlay::bubble(ctx, shown, &b);
        if let Some(c) = click {
            self.tut_click(c);
            ctx.request_repaint();
        }
    }

    /// Klick in der Blase (auch von der Test-Schnittstelle).
    fn tut_click(&mut self, c: overlay::Click) {
        match c {
            overlay::Click::Back => self.tut_go(-1, false),
            overlay::Click::Next => self.tut_go(1, false),
            overlay::Click::Skip => self.tut_go(1, true),
            overlay::Click::Quit => self.tut_quit(),
        }
    }

    /// Einen Schritt vor (`delta` = 1) oder zurück (-1); `skipped` = übersprungen. Nach dem letzten Schritt: fertig.
    fn tut_go(&mut self, delta: i32, skipped: bool) {
        let Some(run) = self.tut.run.as_mut() else { return };
        let cur = run.step().id;
        let tp = self.tut.p.tours.entry(run.tour.to_string()).or_default();
        if skipped && !tp.skipped.iter().any(|s| s == cur) {
            tp.skipped.push(cur.to_string());
        }
        let next = run.idx as i64 + delta as i64;
        if next >= run.steps.len() as i64 {
            self.tut_finish();
            return;
        }
        run.idx = next.max(0) as usize;
        run.frames = 0;
        run.done_at = None;
        run.missing = None;
        tp.step = run.step().id.to_string();
        self.tut_save();
    }

    /// Einträge der Befehls-Palette („Tutorial: …“).
    pub(super) fn tut_palette_items(&self) -> Vec<(String, String, Action)> {
        let lang = self.settings.language;
        let pre = lang.t("Tutorial:", "Tutorial:");
        let mut v = Vec::new();
        for tour in steps::TOURS {
            v.push((format!("{pre} {}", t(lang, tour.title)), t(lang, tour.summary).to_string(), Action::Tutorial(Cmd::Start(tour.id))));
        }
        if self.tut_resumable().is_some() {
            v.insert(1, (format!("{pre} {}", lang.t("Fortsetzen", "Continue")), String::new(), Action::Tutorial(Cmd::Continue)));
        }
        if !self.tut.p.whats_new_after.is_empty() {
            v.push((format!("{pre} {}", lang.t("Was ist neu", "What's new")), env!("CARGO_PKG_VERSION").to_string(), Action::Tutorial(Cmd::WhatsNew)));
        }
        v
    }

    /// Titel der Tour, die „Fortsetzen“ fortsetzt (nicht beendet, mit gemerktem Schritt).
    fn tut_resumable(&self) -> Option<&'static str> {
        let last = &self.tut.p.last_tour;
        let tp = self.tut.p.tours.get(last)?;
        if tp.finished || tp.step.is_empty() {
            return None;
        }
        steps::tour(last).map(|t| t.id)
    }

    /// Knopf im leeren Terminal Hub: „Tour starten“ bzw. „Tour fortsetzen“ (nicht während einer Tour).
    pub(super) fn tut_hub_label(&self) -> Option<String> {
        if self.tut.run.is_some() || self.wins.cur != 0 {
            return None;
        }
        let lang = self.settings.language;
        Some(if self.tut_resumable().is_some() { lang.t("Tour fortsetzen", "Continue the tour") } else { lang.t("Kurze Tour starten", "Take the short tour") }.to_string())
    }

    /// Hinweis in der Fußzeile der Seitenleiste nach einem Update (Tour „Was ist neu“), sonst `None`.
    pub(super) fn tut_hint(&self) -> Option<String> {
        if self.tut.run.is_some() || self.tut.p.whats_new_after.is_empty() || self.wins.cur != 0 {
            return None;
        }
        let v = env!("CARGO_PKG_VERSION");
        Some(match self.settings.language {
            Lang::De => format!("Neu in {v} – Tour starten?"),
            Lang::En => format!("New in {v} – start tour?"),
        })
    }

    /// Abschnitt „Tutorial“ unter Einstellungen → Info.
    pub(super) fn tut_about_ui(&mut self, ui: &mut Ui) -> Option<Cmd> {
        self.tut_load();
        let lang = self.settings.language;
        let mut out = None;
        let top = ui.cursor().min.y;
        ui.add_space(S2);
        ui.separator();
        ui.add_space(S1);
        ui.label(RichText::new(lang.t("Tutorial", "Tutorial")).strong());
        ui.label(RichText::new(lang.t(
            "Geführte Schritte in der echten Oberfläche, in einem eigenen Projekt „Tutorial“. Auch in der Befehls-Palette („Tutorial“) und im leeren Terminal Hub.",
            "Guided steps in the real interface, in a project “Tutorial” of its own. Also in the command palette (“Tutorial”) and in the empty Terminal Hub.",
        )).small().color(style::dim(ui)));
        ui.add_space(S1);
        ui.horizontal_wrapped(|ui| {
            if ui.add(egui::Button::new(RichText::new(t(lang, steps::TOURS[0].title)).strong()).fill(style::ui().accent.gamma_multiply(0.35))).on_hover_text(t(lang, steps::TOURS[0].summary)).clicked() {
                out = Some(Cmd::Start(steps::TOURS[0].id));
            }
            if self.tut_resumable().is_some() && ui.button(lang.t("Fortsetzen", "Continue")).clicked() {
                out = Some(Cmd::Continue);
            }
            if !self.tut.p.whats_new_after.is_empty() && ui.button(lang.t("Was ist neu", "What's new")).clicked() {
                out = Some(Cmd::WhatsNew);
            }
        });
        ui.label(RichText::new(lang.t("Kapitel:", "Chapters:")).small().color(style::dim(ui)));
        ui.horizontal_wrapped(|ui| {
            for tour in &steps::TOURS[1..] {
                let done = self.tut.p.tours.get(tour.id).is_some_and(|tp| tp.finished);
                let label = if done { format!("✓ {}", t(lang, tour.title)) } else { t(lang, tour.title).to_string() };
                if ui.button(label).on_hover_text(t(lang, tour.summary)).clicked() {
                    out = Some(Cmd::Start(tour.id));
                }
            }
        });
        ui.horizontal(|ui| {
            style::toggle(ui, &mut self.settings.tutorial_whats_new);
            ui.label(RichText::new(lang.t("Nach einem Update die Tour „Was ist neu“ von selbst starten (aus: nur ein Hinweis in der Seitenleiste)", "Start the “What's new” tour by itself after an update (off: just a hint in the sidebar)")).small());
        });
        spot_since(ui, Spot::AboutTutorial, top);
        out
    }
}
