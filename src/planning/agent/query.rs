//! `search_tickets` mit Abfrage (`query`): JQL-artige Abfragen der Abfragesprache, nur lesend, über `planning::search::run_query`.
//!
//! * Die Abfrage ersetzt die einfachen Filter (`text`, `type`, `status`, `starred`, `flagged`, `parent`, `no_parent`, `order`); das
//!   Sortieren übernimmt `ORDER BY` in der Abfrage. `project` (Bereich) sowie `limit` und `offset` gelten weiter.
//! * Grenzen: Abfragetext höchstens [`search::MAX_QUERY_CHARS`] Zeichen, eine Seite höchstens `MAX_LIMIT` Tickets, Zeitlimit
//!   [`QUERY_TIMEOUT`] (die Abfrage läuft in der Oberfläche; ein Abbruch meldet `timeout`).
//! * Fehler: `invalid_query` mit `details.problems` (je Problem `position` = Spalte ab 1, `start`/`length` in Bytes, `message`),
//!   `too_large` (zu langer Text), `timeout`.
//! * `linkedTerminal()` kennt hier kein Terminal und trifft nichts; `currentDate()` ist das heutige Datum.

use super::*;
use crate::planning::search::{self, Page, Scope, SearchError};
use std::time::Duration;

/// Zeitlimit einer Abfrage des Agenten (kurz, weil die Anwendung sie in der Oberfläche ausführt).
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

/// Die Parameter der einfachen Filter, die sich nicht mit `query` verbinden lassen.
pub(super) const CONFLICTING: [&str; 8] = ["text", "type", "status", "starred", "flagged", "parent", "no_parent", "order"];

/// Fehler der Abfrage-Ausführung in einen Werkzeug-Fehler mit festem Code übersetzen.
pub fn search_err(lang: Lang, e: SearchError) -> ToolError {
    let message = e.message(lang);
    let problems = e.problems(lang);
    let code = match &e {
        SearchError::Parse { .. } | SearchError::Invalid { .. } | SearchError::TooDeep { .. } | SearchError::TooComplex { .. } | SearchError::NotAvailableYet(_) => "invalid_query",
        SearchError::TooLong { .. } => "too_large",
        SearchError::Timeout => "timeout",
        SearchError::NotFound => "not_found",
        SearchError::Db(_) | SearchError::Internal(_) => "db_error",
    };
    let details = (!problems.is_empty()).then(|| {
        json!({"problems": problems.iter().map(|p| json!({"position": p.column, "start": p.start, "length": p.len, "message": p.message})).collect::<Vec<_>>()})
    });
    ToolError { code, message, details }
}

/// Führt die Abfrage aus und liefert die Seite im Format von `search_tickets`.
pub(super) fn run(ctx: &Ctx, project: Option<&Project>, query: &str, limit: i64, offset: i64) -> R {
    let scope = match project {
        Some(p) => Scope::Project(p.id),
        None => Scope::All,
    };
    let sctx = search::Ctx::new(&ctx.now.format("%Y-%m-%d").to_string(), ctx.lang).with_timeout(QUERY_TIMEOUT);
    let page = search::run_query(ctx.conn, &sctx, scope, query, Page::new(limit, offset)).map_err(|e| search_err(ctx.lang, e))?;
    let mut tickets = Vec::with_capacity(page.rows.len());
    for r in &page.rows {
        let t = tk(ctx, get_ticket(ctx.conn, r.id))?;
        tickets.push(ticket_json(ctx, &t));
    }
    read(json!({
        "tickets": tickets,
        "total": page.total,
        "limit": limit,
        "offset": offset,
        "note": untrusted(ctx.lang),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::projects::{update_project, UpdateProject};
    use crate::planning::tickets::fixture::Fx;
    use chrono::NaiveDate;

    fn ctx(f: &Fx, lang: Lang) -> Ctx<'_> {
        Ctx { conn: &f.c, lang, source: Source::agent("Builder"), now: NaiveDate::from_ymd_opt(2026, 11, 14).unwrap().and_hms_opt(12, 0, 0).unwrap() }
    }

    fn keys(d: &Done) -> Vec<String> {
        d.result["tickets"].as_array().unwrap().iter().map(|t| t["key"].as_str().unwrap().to_string()).collect()
    }

    fn setup() -> Fx {
        let f = Fx::new();
        f.mk(f.epic, "Checkout");
        f.mk(f.task, "Login fix");
        f.mk(f.task, "Docs");
        f.mk(f.defect, "Login crash");
        f
    }

    #[test]
    fn a_query_finds_tickets_in_one_or_all_projects() {
        let f = setup();
        let d = call(&ctx(&f, Lang::En), "search_tickets", &json!({"project": "PAN", "query": "summary ~ login ORDER BY key DESC"}), false).unwrap();
        assert_eq!(keys(&d), vec!["PAN-4", "PAN-2"]);
        assert_eq!(d.result["total"], 2);
        assert!(d.change.is_none(), "nur lesend");
        assert!(d.result["note"].as_str().unwrap().contains("untrusted"));
        let d = call(&ctx(&f, Lang::En), "search_tickets", &json!({"query": "type = Epic"}), false).unwrap();
        assert_eq!(keys(&d), vec!["PAN-1"]);
        // Sortierung der Abfrage, Seiten mit total
        let d = call(&ctx(&f, Lang::En), "search_tickets", &json!({"query": "ORDER BY summary", "limit": 2, "offset": 1}), false).unwrap();
        assert_eq!(keys(&d), vec!["PAN-3", "PAN-4"]);
        assert_eq!((d.result["total"].clone(), d.result["limit"].clone(), d.result["offset"].clone()), (json!(4), json!(2), json!(1)));
        // eine leere Abfrage trifft alle
        assert_eq!(call(&ctx(&f, Lang::En), "search_tickets", &json!({"query": ""}), false).unwrap().result["total"], 4);
    }

    #[test]
    fn page_sizes_are_limited_like_the_simple_search() {
        let f = setup();
        let d = call(&ctx(&f, Lang::En), "search_tickets", &json!({"query": "", "limit": 100000}), false).unwrap();
        assert_eq!(d.result["limit"], MAX_LIMIT);
    }

    #[test]
    fn query_errors_carry_positions_in_both_languages() {
        let f = setup();
        for lang in [Lang::En, Lang::De] {
            let e = call(&ctx(&f, lang), "search_tickets", &json!({"project": "PAN", "query": "type = Epic AND nonsense = 1"}), false).unwrap_err();
            assert_eq!(e.code, "invalid_query");
            let p = &e.details.as_ref().unwrap()["problems"][0];
            assert_eq!(p["position"], 17);
            assert!(p["message"].as_str().unwrap().contains("nonsense"));
            assert!(e.message.contains("17") && e.message.contains("nonsense"), "{}", e.message);
        }
        let en = call(&ctx(&f, Lang::En), "search_tickets", &json!({"query": "type = "}), false).unwrap_err();
        let de = call(&ctx(&f, Lang::De), "search_tickets", &json!({"query": "type = "}), false).unwrap_err();
        assert_eq!((en.code, de.code), ("invalid_query", "invalid_query"));
        assert_ne!(en.message, de.message);
    }

    #[test]
    fn too_long_queries_and_conflicting_parameters_are_refused() {
        let f = setup();
        let e = call(&ctx(&f, Lang::En), "search_tickets", &json!({"query": "x".repeat(search::MAX_QUERY_CHARS + 1)}), false).unwrap_err();
        assert_eq!(e.code, "too_large");
        let e = call(&ctx(&f, Lang::En), "search_tickets", &json!({"query": "type = Epic", "text": "a"}), false).unwrap_err();
        assert_eq!(e.code, "bad_args");
        assert!(e.message.contains("text"), "{}", e.message);
        for k in CONFLICTING {
            let v = if k == "starred" || k == "flagged" || k == "no_parent" { json!(true) } else { json!("x") };
            let e = call(&ctx(&f, Lang::En), "search_tickets", &json!({"query": "", k: v}), false).unwrap_err();
            assert_eq!(e.code, "bad_args", "{k}");
        }
        // null zählt als nicht angegeben
        assert!(call(&ctx(&f, Lang::En), "search_tickets", &json!({"query": "", "text": null}), false).is_ok());
        assert_eq!(call(&ctx(&f, Lang::En), "search_tickets", &json!({"query": 5}), false).unwrap_err().code, "bad_args");
        assert_eq!(call(&ctx(&f, Lang::En), "search_tickets", &json!({"project": "NOPE", "query": ""}), false).unwrap_err().code, "not_found");
    }

    #[test]
    fn every_search_error_has_a_code_and_a_message_in_both_languages() {
        use crate::planning::search::Feature;
        let cases = [
            (SearchError::Timeout, "timeout"),
            (SearchError::TooLong { len: 5000, max: 4000 }, "too_large"),
            (SearchError::TooDeep { max: 100 }, "invalid_query"),
            (SearchError::TooComplex { max: 300 }, "invalid_query"),
            (SearchError::NotAvailableYet(Feature::Sprints), "invalid_query"),
            (SearchError::NotFound, "not_found"),
            (SearchError::Db("x".into()), "db_error"),
            (SearchError::Internal("x".into()), "db_error"),
        ];
        for (e, code) in cases {
            let (en, de) = (search_err(Lang::En, e.clone()), search_err(Lang::De, e.clone()));
            assert_eq!((en.code, de.code), (code, code), "{e:?}");
            assert!(!en.message.is_empty() && en.message != de.message, "{e:?}");
            assert!(en.details.is_none(), "keine Positionen ohne Stelle im Text");
        }
    }

    #[test]
    fn queries_cannot_change_anything_and_ignore_injection() {
        let f = setup();
        let before: i64 = f.c.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get(0)).unwrap();
        for q in ["summary = \"x'; DROP TABLE tickets; --\"", "summary ~ \"%\" OR 1=1", "DELETE FROM tickets", "type = Epic; DELETE FROM tickets"] {
            let _ = call(&ctx(&f, Lang::En), "search_tickets", &json!({"query": q}), false);
        }
        let after: i64 = f.c.query_row("SELECT COUNT(*) FROM tickets", [], |r| r.get(0)).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn sprint_queries_work_when_the_project_uses_sprints() {
        use crate::calc::calendar::WorkCalendar;
        use crate::planning::sprints::sprint::{create_sprint, move_tickets_to_sprint, start_sprint, NewSprint};
        let f = setup();
        update_project(&f.c, f.project, &UpdateProject { use_sprints: Some(true), ..Default::default() }).unwrap();
        let cal = WorkCalendar::default();
        let s = create_sprint(&f.c, &NewSprint::new(f.project, "Sprint 1"), &cal).unwrap();
        move_tickets_to_sprint(&f.c, s, &[2, 3]).unwrap();
        start_sprint(&f.c, s, &cal).unwrap();
        let d = call(&ctx(&f, Lang::En), "search_tickets", &json!({"project": "PAN", "query": "sprint = activeSprint() ORDER BY key"}), false).unwrap();
        assert_eq!(keys(&d), vec!["PAN-2", "PAN-3"]);
        let d = call(&ctx(&f, Lang::En), "search_tickets", &json!({"project": "PAN", "query": "sprint IS EMPTY"}), false).unwrap();
        assert_eq!(keys(&d), vec!["PAN-1", "PAN-4"]);
    }
}
