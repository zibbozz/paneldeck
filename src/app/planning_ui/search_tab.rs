//! Vorläufiger Reiter „Suche“ im Projekt: die Abfrage-Leiste (`query_bar`) samt Hilfeseite, gespeicherten Filtern und
//! Schlüsselwort-Editor, damit die Bausteine bedienbar sind, solange die Suchmaschine noch nicht angeschlossen ist.
//!
//! **Einhängepunkte (klein gehalten):**
//! * `Tab::Search` in `mod.rs` (Reiterliste `PlanUi::tabs`, Verteiler `PlanUi::body`) – der Reiter selbst ist [`PlanUi::search_tab`].
//! * **Anschlusspunkt der Suche:** [`PlanUi::run_search`] bekommt jede abgeschickte Abfrage. Dort ruft später die Ticket-Liste die
//!   Ausführung (`planning::search`) auf; bis dahin merkt es sich die Abfrage nur und der Reiter zeigt einen Platzhalter.
//! * Die Abfrage-Leiste selbst ist überall nutzbar (Ticket-Liste, Board-Quelle, Dashboard-Kachel): `query_bar(ui, &mut state, &QueryBarCtx)`.
//!
//! **Schema:** [`build_query_schema`] baut das Schema der Leiste aus den Feldern des Projekts, den Pseudo-Feldern (`key type status
//! title summary parent created updated flagged starred project sprint changedBy`) und den Schlüsselwörtern (`apply_to_schema`).
//! Der Such-Agent hat mit `planning::search::schema_for` eine eigene Fassung; beide werden später vereinheitlicht.
//! `summary` ist ein Alias für `title`, weil die Hilfe-Beispiele es so schreiben.

use super::query_bar::{QueryBarCtx, QueryBarState, query_bar};
use super::widgets::{self, hint};
use super::{Cx, PlanUi, Tab};
use crate::planning::fields::{FieldKind, list_fields};
use crate::planning::projects::list_projects;
use crate::planning::statuses::list_statuses;
use crate::planning::types::list_types;
use crate::planning::views::keywords::apply_to_schema;
use crate::query::{FieldType, Schema};
use crate::state::Lang;
use crate::style::{self, S2, S3};
use eframe::egui::{RichText, Ui};
use rusqlite::Connection;

/// Wer zuletzt geändert hat (Werte des Pseudo-Feldes `changedBy`).
pub const CHANGED_BY_VALUES: [&str; 3] = ["ui", "agent", "import"];
/// Schlüssel in `meta`, unter dem der Verlauf der Abfragen liegt.
const HISTORY_META: &str = "query_history";

/// Zustand des Reiters „Suche“.
pub(super) struct State {
    pub bar: QueryBarState,
    /// (Projekt, Sprache, Schema) – neu gebaut, wenn sich etwas ändert (`stale`, anderes Projekt, andere Sprache)
    schema: Option<(i64, Lang, Schema)>,
    pub stale: bool,
    history_loaded: bool,
    /// Die zuletzt abgeschickte Abfrage (Platzhalter-Anzeige, bis die Suche angeschlossen ist)
    last: Option<String>,
}

impl Default for State {
    fn default() -> Self {
        State { bar: QueryBarState::new("search_tab"), schema: None, stale: true, history_loaded: false, last: None }
    }
}

// ------------------------------------------------------------------ Schema

/// Ein gemeinsames Feld über mehrere Typen: gleicher Name, gleiche Art (Dropdown-Werte werden vereinigt).
fn merge_field(fields: &mut Vec<(String, FieldType, bool)>, name: String, ty: FieldType, multi: bool) {
    let Some(existing) = fields.iter_mut().find(|(n, _, _)| n.to_lowercase() == name.to_lowercase()) else {
        fields.push((name, ty, multi));
        return;
    };
    // Gleicher Name, andere Art: das erste gewinnt (die Prüfung meldet dann beim anderen Typ nichts Falsches).
    if let (FieldType::Option(have), FieldType::Option(more)) = (&mut existing.1, &ty) {
        for v in more {
            if !have.iter().any(|h| h.to_lowercase() == v.to_lowercase()) {
                have.push(v.clone());
            }
        }
    }
}

/// Die Felder aller Typen eines Projekts als Abfrage-Felder (gleichnamige zusammengefasst).
pub fn custom_fields(conn: &Connection, project_id: i64, lang: Lang) -> Vec<(String, FieldType, bool)> {
    let mut out: Vec<(String, FieldType, bool)> = Vec::new();
    for t in list_types(conn, project_id).unwrap_or_default() {
        for f in list_fields(conn, t.id).unwrap_or_default() {
            let names = || f.options.values.iter().map(|v| v.display(lang)).collect::<Vec<_>>();
            let (ty, multi) = match f.kind {
                FieldKind::Text | FieldKind::Rich => (FieldType::Text, false),
                FieldKind::Checkbox => (FieldType::Bool, false),
                FieldKind::Number => (FieldType::Number, false),
                FieldKind::Ticket => (FieldType::TicketRef, false),
                FieldKind::Terminal => (FieldType::Terminal, false),
                FieldKind::Date => (FieldType::Date, false),
                FieldKind::Datetime => (FieldType::DateTime, false),
                FieldKind::Labels => (FieldType::Labels, true),
                FieldKind::Dropdown => (FieldType::Option(names()), false),
                FieldKind::Multi => (FieldType::Option(names()), true),
                // Versionen und Meilensteine haben noch keine eigene Wertetabelle: freier Text
                FieldKind::Version | FieldKind::Milestone => (FieldType::Text, false),
            };
            merge_field(&mut out, f.display(lang), ty, multi);
        }
    }
    out
}

/// Das Schema der Abfrage-Leiste für ein Projekt. `lang`: Anzeigenamen (Typen, Status, Felder) in dieser Sprache.
pub fn build_query_schema(conn: &Connection, project_id: i64, lang: Lang) -> Schema {
    let types: Vec<String> = list_types(conn, project_id).unwrap_or_default().iter().map(|t| t.display(lang)).collect();
    let statuses: Vec<String> = list_statuses(conn, project_id).unwrap_or_default().iter().map(|s| s.display(lang)).collect();
    let projects: Vec<String> = list_projects(conn).unwrap_or_default().into_iter().map(|p| p.key).collect();
    let mut s = Schema::new()
        .with_field("key", FieldType::TicketRef)
        .with_field("type", FieldType::Option(types))
        .with_field("status", FieldType::Option(statuses))
        .with_field("title", FieldType::Text)
        .with_field("summary", FieldType::Text)
        .with_field("parent", FieldType::TicketRef)
        .with_field("created", FieldType::DateTime)
        .with_field("updated", FieldType::DateTime)
        .with_field("flagged", FieldType::Bool)
        .with_field("starred", FieldType::Bool)
        .with_field("project", FieldType::Option(projects))
        .with_field("sprint", FieldType::Text)
        .with_field("changedBy", FieldType::Option(CHANGED_BY_VALUES.iter().map(|x| x.to_string()).collect()));
    for (name, ty, multi) in custom_fields(conn, project_id, lang) {
        // Die Pseudo-Felder haben Vorrang vor gleichnamigen eigenen Feldern.
        if s.field(&name).is_some() {
            continue;
        }
        s = if multi { s.with_multi_field(&name, ty) } else { s.with_field(&name, ty) };
    }
    apply_to_schema(conn, s)
}

// ------------------------------------------------------------------ Reiter

impl PlanUi {
    /// Der Reiter „Suche“: Abfrage-Leiste plus Platzhalter für die Ergebnisse.
    pub(super) fn search_tab(&mut self, ui: &mut Ui, cx: &Cx) {
        let lang = cx.lang;
        let Some(proj) = self.project().cloned() else { return };
        if !self.search.history_loaded {
            self.search.history_loaded = true;
            if let Some(h) = cx.db.meta(HISTORY_META) {
                self.search.bar.history = super::query_logic::History::from_json(&h);
            }
        }
        let stale = self.search.stale || self.search.schema.as_ref().is_none_or(|(p, l, _)| *p != proj.id || *l != lang);
        if stale {
            self.search.schema = Some((proj.id, lang, build_query_schema(cx.conn, proj.id, lang)));
            self.search.stale = false;
        }
        hint(ui, lang.t(
            "Schreibe eine Abfrage, z. B. type = Epic AND status != Done ORDER BY priority. Vorschläge erscheinen beim Tippen; das Fragezeichen öffnet die Anleitung.",
            "Write a query, e.g. type = Epic AND status != Done ORDER BY priority. Suggestions appear while you type; the question mark opens the guide.",
        ));
        ui.add_space(S2);
        let Some((_, _, schema)) = self.search.schema.as_ref() else { return };
        let out = query_bar(ui, &mut self.search.bar, &QueryBarCtx::new(schema, lang).with_filters(cx.conn, Some(proj.id), &proj.key));
        if out.help_clicked {
            self.help.open_at(None);
        }
        if out.keywords_clicked {
            self.kw.open();
        }
        if out.history_changed {
            cx.db.set_meta(HISTORY_META, &self.search.bar.history.to_json());
        }
        if let Some(q) = out.submitted {
            self.run_search(&q);
        }
        ui.add_space(S3);
        widgets::card(ui, |ui| {
            ui.label(RichText::new(lang.t("Ergebnisse erscheinen, sobald die Suche angeschlossen ist.", "Results appear here once the search engine is wired.")).color(style::dim(ui)));
            if let Some(q) = &self.search.last {
                ui.add_space(S2);
                ui.label(RichText::new(lang.t("Zuletzt abgeschickt:", "Last submitted:")).small().color(style::dim(ui)));
                ui.label(RichText::new(if q.is_empty() { "—" } else { q }).monospace());
            }
        });
    }

    /// **Anschlusspunkt der Suche:** jede abgeschickte Abfrage (gültig oder leer) landet hier. Später ruft die Ticket-Liste von hier
    /// die Ausführung auf; bis dahin wird die Abfrage nur gemerkt.
    pub(super) fn run_search(&mut self, query: &str) {
        self.search.last = Some(query.to_string());
    }

    /// Den Schlüsselwort-Dialog zeichnen (aus `dialogs`); Änderungen machen das Schema der Leiste ungültig.
    pub(super) fn keywords_dialog_ui(&mut self, ctx: &eframe::egui::Context, cx: &Cx) {
        if super::keywords_ed::keywords_dialog(ctx, &mut self.kw, cx.conn, cx.lang) {
            self.search.stale = true;
            self.search.bar.filters.invalidate();
            self.dirty = true;
        }
    }

    /// Reiter „Suche“ wählen (z. B. nach „In Suche übernehmen“ in der Hilfe).
    pub(super) fn show_search_tab(&mut self) {
        self.tab = Tab::Search;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::views::keywords::{Cond, Definition, NewKeyword, create_keyword};
    use crate::planning::views::testutil::{db, project};
    use crate::planning::workflow::ConditionOp;
    use crate::query::{self, validate};

    fn issues(schema: &Schema, q: &str) -> usize {
        validate(&query::parse(q).expect("parst"), schema).len()
    }

    #[test]
    fn schema_has_the_pseudo_fields_and_keywords() {
        let c = db();
        let p = project(&c, "AA", true);
        let s = build_query_schema(&c, p, Lang::En);
        for f in ["key", "type", "status", "title", "summary", "parent", "created", "updated", "flagged", "starred", "project", "sprint", "changedBy"] {
            assert!(s.field(f).is_some(), "Feld {f} fehlt");
        }
        assert!(s.predicate("blocked").is_some() && s.predicate("favorite").is_some());
        assert!(s.sort_keyword("favorites").is_some() && s.sort_keyword("flagged").is_some());
        assert!(!s.functions.is_empty());
        // Typen, Status und Projekte als Werte
        let FieldType::Option(types) = &s.field("type").unwrap().ty else { panic!("type ist eine Auswahl") };
        assert!(types.iter().any(|t| t == "Epic") && types.iter().any(|t| t == "Defect"), "{types:?}");
        let FieldType::Option(st) = &s.field("status").unwrap().ty else { panic!() };
        assert!(st.iter().any(|t| t == "To Do") && st.iter().any(|t| t == "Done"), "{st:?}");
        let FieldType::Option(pr) = &s.field("project").unwrap().ty else { panic!() };
        assert!(pr.iter().any(|k| k == "AA"));
    }

    #[test]
    fn schema_accepts_typical_queries_and_rejects_typos() {
        let c = db();
        let p = project(&c, "AA", true);
        let s = build_query_schema(&c, p, Lang::En);
        for q in [
            "type = Epic AND status != Done ORDER BY priority",
            "summary ~ \"login\" AND flagged = true",
            "IS favorite AND NOT IS blocked",
            "created > 2026-01-01 ORDER BY updated DESC",
            "project = AA AND parent IS EMPTY",
            "changedBy = agent SORT BY favorites",
            "key IN (AA-1, AA-2)",
        ] {
            assert_eq!(issues(&s, q), 0, "{q}");
        }
        assert!(issues(&s, "stauts = Done") > 0);
        assert!(issues(&s, "type = Nonsense") > 0);
        assert!(issues(&s, "created > notadate") > 0);
    }

    #[test]
    fn custom_fields_are_merged_by_name_and_typed() {
        let c = db();
        let p = project(&c, "AA", true);
        let fields = custom_fields(&c, p, Lang::En);
        let names: Vec<&str> = fields.iter().map(|(n, _, _)| n.as_str()).collect();
        let mut dedup = names.clone();
        dedup.sort_by_key(|n| n.to_lowercase());
        dedup.dedup_by_key(|n| n.to_lowercase());
        assert_eq!(names.len(), dedup.len(), "gleichnamige Felder mehrerer Typen sind zusammengefasst: {names:?}");
        assert!(names.contains(&"Priority"), "{names:?}");
        let prio = fields.iter().find(|(n, _, _)| n == "Priority").unwrap();
        assert!(matches!(&prio.1, FieldType::Option(v) if v.iter().any(|x| x == "High")), "{:?}", prio.1);
        // das Schema kennt sie, ohne Beachtung der Schreibweise
        let s = build_query_schema(&c, p, Lang::En);
        assert!(s.field("priority").is_some());
        assert_eq!(issues(&s, "Priority = High"), 0);
        assert!(issues(&s, "Priority = Banana") > 0);
    }

    #[test]
    fn merge_unites_option_values_and_keeps_the_first_kind() {
        let mut v = Vec::new();
        merge_field(&mut v, "Prio".into(), FieldType::Option(vec!["A".into()]), false);
        merge_field(&mut v, "prio".into(), FieldType::Option(vec!["a".into(), "B".into()]), false);
        merge_field(&mut v, "PRIO".into(), FieldType::Number, false);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].1, FieldType::Option(vec!["A".into(), "B".into()]));
    }

    #[test]
    fn names_follow_the_language() {
        let c = db();
        let p = project(&c, "AA", true);
        let en = build_query_schema(&c, p, Lang::En);
        let de = build_query_schema(&c, p, Lang::De);
        let FieldType::Option(a) = &en.field("status").unwrap().ty else { panic!() };
        let FieldType::Option(b) = &de.field("status").unwrap().ty else { panic!() };
        assert_ne!(a, b, "Standard-Status sind übersetzt");
        assert!(b.iter().any(|x| x == "Erledigt"), "{b:?}");
    }

    #[test]
    fn custom_keywords_reach_the_schema_and_pseudo_fields_win() {
        let c = db();
        let p = project(&c, "AA", true);
        let f = super::super::keywords_ed::all_field_choices(&c, Lang::En);
        let prio = f.iter().find(|x| x.label.ends_with("· Priority")).unwrap();
        create_keyword(&c, &NewKeyword { name: "hot".into(), description: String::new(), definition: Definition::Predicate { condition: Cond::Compare { field_id: prio.id, op: ConditionOp::IsNotEmpty, value: serde_json::Value::Null } } }).unwrap();
        let s = build_query_schema(&c, p, Lang::En);
        assert!(s.predicate("hot").is_some());
        assert_eq!(issues(&s, "IS hot"), 0);
        // ein eigenes Feld „Status“ verdrängt das Pseudo-Feld nicht
        let n = s.fields.iter().filter(|f| f.name.eq_ignore_ascii_case("status")).count();
        assert_eq!(n, 1);
    }

    #[test]
    fn system_project_and_empty_project_work() {
        let c = db();
        let sys = crate::planning::projects::project_by_key(&c, "TODO").unwrap().id;
        let s = build_query_schema(&c, sys, Lang::De);
        assert!(s.field("status").is_some());
        let empty = project(&c, "EE", false);
        let s = build_query_schema(&c, empty, Lang::En);
        let FieldType::Option(t) = &s.field("type").unwrap().ty else { panic!() };
        assert!(t.is_empty());
    }

    // ---- Der ganze Reiter im Headless-Lauf (Kopf, Seite, Hilfe, Dialoge)

    fn open_db() -> crate::db::Db {
        let d = std::env::temp_dir().join(format!("paneldeck-test-searchtab-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        crate::db::open(&d, &d.join("backups")).unwrap()
    }

    fn frame(ctx: &eframe::egui::Context, ui: &mut PlanUi, db: &crate::db::Db, lang: Lang, events: Vec<eframe::egui::Event>) {
        use crate::app::tasks_ui::Env;
        let env = Env { db: Some(db), db_error: None, lang, bg: eframe::egui::Color32::from_rgb(20, 20, 24), undo_steps: 10 };
        let raw = eframe::egui::RawInput { screen_rect: Some(eframe::egui::Rect::from_min_size(eframe::egui::Pos2::ZERO, eframe::egui::vec2(1200.0, 800.0))), events, focused: true, ..Default::default() };
        let _ = ctx.run(raw, |ctx| {
            ui.header(ctx, &env, db);
            ui.page(ctx, &env, db);
        });
    }

    #[test]
    fn search_tab_help_and_dialogs_draw_end_to_end() {
        use eframe::egui::{Event, Key, Modifiers};
        let db = open_db();
        let p = crate::planning::projects::create_project(&db.conn, &{
            let mut n = crate::planning::projects::NewProject::new("QQ", "Query test", "#336699");
            n.with_defaults = true;
            n
        })
        .unwrap();
        db.set_meta("expert_project", &p.to_string());
        let ctx = eframe::egui::Context::default();
        let mut ui = PlanUi::default();
        ui.ensure_loaded(Some(&db));
        ui.set_expert(&db, true);
        for lang in [Lang::En, Lang::De] {
            ui.tab = Tab::Search;
            frame(&ctx, &mut ui, &db, lang, vec![]);
            frame(&ctx, &mut ui, &db, lang, vec![]);
            assert!(ui.search.schema.is_some(), "das Schema der Leiste wird beim ersten Zeichnen gebaut");
            // Abfrage von außen (z. B. aus der Hilfe) und Abschicken
            ui.search.bar.set_query("type = Epic");
            frame(&ctx, &mut ui, &db, lang, vec![]);
            frame(&ctx, &mut ui, &db, lang, vec![Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
            assert_eq!(ui.search.last.as_deref(), Some("type = Epic"), "{lang:?}: die abgeschickte Abfrage kommt an der Anschlussstelle an");
            assert!(db.meta(HISTORY_META).is_some_and(|h| h.contains("type = Epic")), "Verlauf wird gespeichert");
            // Hilfe öffnen, suchen, schließen
            ui.help.open_at(Some("filters"));
            frame(&ctx, &mut ui, &db, lang, vec![]);
            frame(&ctx, &mut ui, &db, lang, vec![]);
            assert!(ui.help.open);
            frame(&ctx, &mut ui, &db, lang, vec![Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
            assert!(!ui.help.open, "Esc schließt die Hilfe");
            // Schlüsselwort-Dialog
            ui.kw.open();
            frame(&ctx, &mut ui, &db, lang, vec![]);
            assert!(ui.kw.open);
            ui.kw.open = false;
            // Reiter „Felder“ mit dem Schlüsselwort-Knopf
            ui.tab = Tab::Fields;
            frame(&ctx, &mut ui, &db, lang, vec![]);
        }
        // Wechsel der Sprache baut das Schema neu
        assert!(ui.search.schema.as_ref().is_some_and(|(_, l, _)| *l == Lang::De));
    }

    #[test]
    fn help_example_can_be_taken_over_into_the_search_field() {
        let db = open_db();
        let ctx = eframe::egui::Context::default();
        let mut ui = PlanUi::default();
        ui.ensure_loaded(Some(&db));
        ui.set_expert(&db, true);
        frame(&ctx, &mut ui, &db, Lang::En, vec![]);
        ui.help.open_at(None);
        frame(&ctx, &mut ui, &db, Lang::En, vec![]);
        // dieselbe Übernahme wie beim Klick auf „In Suche übernehmen“
        let q = crate::planning::query_help::HelpDoc::builtin().examples()[0].jql.clone();
        ui.search.bar.set_query(&q);
        ui.show_search_tab();
        ui.help.open = false;
        frame(&ctx, &mut ui, &db, Lang::En, vec![]);
        frame(&ctx, &mut ui, &db, Lang::En, vec![]);
        assert_eq!(ui.search.bar.text, q);
        assert_eq!(ui.tab, Tab::Search);
    }
}
