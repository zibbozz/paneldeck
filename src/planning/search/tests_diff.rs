//! Vergleichstest mit festem Startwert: zufällig gebaute, gültige Abfragen (AND/OR/NOT, Klammern, alle Operatoren, `ORDER BY` mit
//! mehreren Feldern) laufen einmal durch die Referenz-Auswertung der Hilfe und einmal als SQL auf der echten Datenbank mit denselben
//! Daten; beide müssen genau dieselben Tickets in derselben Reihenfolge liefern.
//!
//! Bewusst nicht verglichen (die SQL-Seite ist dort absichtlich anders als die einfache Referenz): Freitext trifft auch Schlüssel und
//! Labels, `sprint IS EMPTY` heißt „in keinem offenen Sprint“ (siehe `pending.rs`) und `ORDER BY parent` sortiert die Schlüssel
//! numerisch (`WEB-2` vor `WEB-19`), die Referenz als Text.

use super::*;
use crate::planning::query_help::{shop, DemoSpec, REFERENCE_TODAY};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

/// Wert so schreiben, dass er immer als ein Wert gelesen wird.
fn q(s: &str) -> String {
    crate::query::quote_string(s)
}

/// Schreibweise zufällig verändern (Groß-/Kleinschreibung ist egal).
fn case(rng: &mut Rng, s: &str) -> String {
    match rng.below(4) {
        0 => s.to_uppercase(),
        1 => s.to_lowercase(),
        _ => s.to_string(),
    }
}

fn condition(rng: &mut Rng) -> String {
    let eq_ops = ["=", "!="];
    let cmp_ops = ["=", "!=", "<", ">", "<=", ">="];
    match rng.below(16) {
        0 => {
            let f = rng.pick(&["type", "status", "priority", "sprint", "changedBy", "Release version"]);
            let vals: &[&str] = match f {
                "type" => &["Epic", "User story", "Dev task", "Subtask", "Defect", "Nope"],
                "status" => &["To Do", "In Progress", "Development finished", "Done", "Nope"],
                "priority" => &["Highest", "High", "Medium", "Low", "Lowest"],
                "sprint" => &["Sprint 14", "Sprint 15", "Sprint 16"],
                "changedBy" => &["ui", "agent", "quick", "import"],
                _ => &["1.0.0", "1.1.0", "2.0.0", "2.1.0"],
            };
            let base = rng.pick(vals);
            let v = case(rng, base);
            format!("{} {} {}", q(f), rng.pick(&eq_ops), q(&v))
        }
        1 => {
            let f = rng.pick(&["type", "status", "priority", "sprint", "Release version"]);
            let vals: &[&str] = match f {
                "type" => &["Epic", "User story", "Dev task", "Subtask", "Defect"],
                "status" => &["To Do", "In Progress", "Development finished", "Done"],
                "priority" => &["Highest", "High", "Medium", "Low", "Lowest"],
                "sprint" => &["Sprint 14", "Sprint 15", "Sprint 16"],
                _ => &["1.0.0", "2.0.0", "2.1.0"],
            };
            let n = 1 + rng.below(3);
            let list: Vec<String> = (0..n).map(|_| q(rng.pick(vals))).collect();
            format!("{} {}IN ({})", q(f), if rng.below(2) == 0 { "NOT " } else { "" }, list.join(", "))
        }
        2 => {
            let f = rng.pick(&["type", "status", "priority", "parent", "Story points", "Due", "Terminal", "Description", "Release version", "Labels", "changedBy", "summary"]);
            format!("{} IS {}{}", q(f), if rng.below(2) == 0 { "NOT " } else { "" }, rng.pick(&["EMPTY", "NULL"]))
        }
        3 => {
            let f = rng.pick(&["summary", "Description", "Terminal", "Labels"]);
            let w = rng.pick(&["login", "Cart", "CHECKOUT", "e-mail", "the", "xyz", "api", "ö", "%", "_", "c", "auth", "front", "Write tests for rate limiting", "login-fix"]);
            let op = if f == "Labels" || f == "Terminal" { rng.pick(&["=", "!=", "~", "!~"]) } else { rng.pick(&["=", "!=", "~", "!~"]) };
            let w = case(rng, w);
            format!("{} {} {}", q(f), op, q(&w))
        }
        4 => {
            let v = rng.pick(&["0", "1", "2", "3", "5", "8", "100", "2.5", "-1"]);
            format!("\"Story points\" {} {}", rng.pick(&cmp_ops), v)
        }
        5 => {
            let n = 1 + rng.below(3);
            let list: Vec<&str> = (0..n).map(|_| rng.pick(&["1", "2", "3", "5", "8", "13"])).collect();
            format!("\"Story points\" {}IN ({})", if rng.below(2) == 0 { "NOT " } else { "" }, list.join(", "))
        }
        6 => {
            let v = rng.pick(&["2026-11-16", "2026-11-17", "2026-11-15", "2026-11-01", "2026-12-31", "2026-11-16T00:00", "currentDate()"]);
            if v.starts_with("2026-11-16T") {
                format!("Due {} {}", rng.pick(&eq_ops), "2026-11-16")
            } else {
                format!("Due {} {}", rng.pick(&cmp_ops), v)
            }
        }
        7 => {
            let n = 1 + rng.below(3);
            let list: Vec<&str> = (0..n).map(|_| rng.pick(&["2026-11-16", "2026-11-17", "2026-11-13", "currentDate()"])).collect();
            format!("Due {}IN ({})", if rng.below(2) == 0 { "NOT " } else { "" }, list.join(", "))
        }
        8 => {
            let l = rng.pick(&["frontend", "backend", "api", "auth", "legacy", "payments", "Backend", "nope"]);
            format!("Labels {} {}", rng.pick(&eq_ops), l)
        }
        9 => {
            let n = 1 + rng.below(3);
            let list: Vec<&str> = (0..n).map(|_| rng.pick(&["frontend", "backend", "api", "auth", "legacy", "nope"])).collect();
            format!("Labels {}IN ({})", if rng.below(2) == 0 { "NOT " } else { "" }, list.join(", "))
        }
        10 => {
            let k = rng.pick(&["WEB-1", "WEB-2", "WEB-3", "WEB-4", "web-9", "WEB-99"]);
            format!("parent {} {}", rng.pick(&eq_ops), k)
        }
        11 => {
            let t = rng.pick(&["api-work", "login-fix", "docs", "LOGIN-FIX"]);
            if rng.below(3) == 0 {
                format!("Terminal {} linkedTerminal()", rng.pick(&eq_ops))
            } else {
                format!("Terminal {} {}", rng.pick(&eq_ops), t)
            }
        }
        12 => format!("IS {}{}", if rng.below(2) == 0 { "NOT " } else { "" }, rng.pick(&["favorite", "blocked"])),
        13 => {
            let base = rng.pick(&["login", "coupon", "checkout", "ä", "provider", "the", "password", "%"]);
            q(&case(rng, base))
        }
        14 => format!("sprint {} activeSprint()", rng.pick(&eq_ops)),
        _ => {
            let f = rng.pick(&["type", "status", "priority"]);
            let v = rng.pick(&["Epic", "Done", "High"]);
            if f == "type" { format!("type {} Epic", rng.pick(&eq_ops)) } else if f == "status" { format!("status {} {}", rng.pick(&eq_ops), if v == "Done" { "Done" } else { "\"To Do\"" }) } else { format!("priority {} High", rng.pick(&eq_ops)) }
        }
    }
}

fn expr(rng: &mut Rng, depth: usize) -> String {
    if depth == 0 || rng.below(3) == 0 {
        return condition(rng);
    }
    match rng.below(5) {
        0 => format!("NOT ({})", expr(rng, depth - 1)),
        1 => format!("NOT {}", condition(rng)),
        2 => format!("({} OR {})", expr(rng, depth - 1), expr(rng, depth - 1)),
        3 => format!("{} AND {}", expr(rng, depth - 1), expr(rng, depth - 1)),
        _ => format!("{} OR {} AND {}", expr(rng, depth - 1), condition(rng), condition(rng)),
    }
}

fn order(rng: &mut Rng) -> String {
    let n = rng.below(4);
    if n == 0 {
        return String::new();
    }
    let fields = ["type", "status", "priority", "summary", "sprint", "changedBy", "Story points", "Due", "Terminal", "Description", "Release version", "favorites"];
    let terms: Vec<String> = (0..n)
        .map(|_| {
            let dir = match rng.below(3) {
                0 => " ASC",
                1 => " DESC",
                _ => "",
            };
            format!("{}{dir}", q(rng.pick(&fields)))
        })
        .collect();
    format!(" ORDER BY {}", terms.join(", "))
}

fn seeded() -> (Connection, SeededDemo, DemoSpec) {
    let c = crate::db::open_memory();
    let spec = shop();
    let d = seed_demo(&c, &spec, REFERENCE_TODAY).unwrap();
    (c, d, spec)
}

#[test]
fn random_valid_queries_agree_with_the_reference_evaluation() {
    let (c, d, spec) = seeded();
    let ctx = Ctx { today: d.today.clone(), terminals: d.terminals.clone(), lang: Lang::En, timeout: std::time::Duration::from_secs(10) };
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let mut compared = 0;
    let mut skipped = 0;
    let mut failures: Vec<String> = Vec::new();
    for _ in 0..4000 {
        let jql = format!("{}{}", expr(&mut rng, 3), order(&mut rng));
        let expected = match spec.expected_keys(&jql) {
            Ok(k) => k,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        match run_query(&c, &ctx, Scope::Project(d.project_id), &jql, Page::ALL) {
            Ok(page) => {
                let got: Vec<String> = page.rows.iter().map(|r| r.key.clone()).collect();
                if got != expected && failures.len() < 12 {
                    failures.push(format!("{jql}\n   SQL:       {got:?}\n   Referenz:  {expected:?}"));
                }
                if got == expected {
                    assert_eq!(page.total as usize, expected.len(), "{jql}");
                }
                compared += 1;
            }
            Err(e) => {
                if failures.len() < 12 {
                    failures.push(format!("{jql}\n   Fehler: {}", e.message(Lang::En)));
                }
            }
        }
    }
    assert!(compared > 2000, "zu wenige vergleichbare Abfragen: {compared} (übersprungen: {skipped})");
    assert!(failures.is_empty(), "{} Abweichungen (verglichen: {compared}, übersprungen: {skipped}):\n{}", failures.len(), failures.join("\n"));
}

/// Bausteine für Abfragen, die oft fast gültig sind (echte Feldnamen, Operatoren, seltsame Werte).
const BITS: &[&str] = &[
    "type", "status", "priority", "sprint", "summary", "parent", "changedBy", "\"Story points\"", "Due", "Terminal", "Description", "Labels",
    "\"Release version\"", "key", "created", "updated", "flagged", "starred", "project", "terminal", "=", "!=", "~", "!~", "<", ">", "<=", ">=",
    "IN", "NOT", "IS", "EMPTY", "NULL", "AND", "OR", "(", ")", ",", "ORDER", "BY", "SORT", "ASC", "DESC", "favorite", "blocked", "favorites",
    "Done", "\"To Do\"", "Epic", "High", "1", "-1", "2.5", "0", "2026-11-16", "2026-02-30", "2026-11-16T10:30", "2026-11-16T25:00", "currentDate()",
    "activeSprint()", "linkedTerminal()", "currentDate(1)", "foo()", "WEB-1", "web-99", "\"\"", "\"%\"", "\"_\"", "\"ä\"", "\"[x]\"", "\"*\"", "\"'\"",
    "'a b'", "true", "false", "agent", "ui", "x", "2.0.0", "\"1.0.0\"",
];

#[test]
fn almost_valid_garbage_never_ends_in_a_database_or_internal_error() {
    let (c, d, _) = seeded();
    let ctx = Ctx { today: d.today.clone(), terminals: d.terminals.clone(), lang: Lang::En, timeout: std::time::Duration::from_secs(10) };
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (mut ok, mut bad) = (0, Vec::new());
    for _ in 0..20_000 {
        let n = 1 + rng.below(12);
        let jql = (0..n).map(|_| BITS[rng.below(BITS.len())]).collect::<Vec<_>>().join(" ");
        for scope in [Scope::Project(d.project_id), Scope::All] {
            match run_query(&c, &ctx, scope, &jql, Page::first(5)) {
                Ok(_) => ok += 1,
                Err(SearchError::Parse { .. } | SearchError::Invalid { .. }) => {}
                Err(e) => {
                    if bad.len() < 10 {
                        bad.push(format!("{jql:?}: {e:?}"));
                    }
                }
            }
        }
    }
    assert!(ok > 500, "zu wenige gültige Abfragen: {ok}");
    assert!(bad.is_empty(), "unerwartete Fehler:\n{}", bad.join("\n"));
}

#[test]
fn problem_positions_count_characters_while_start_and_length_are_bytes() {
    let (c, d, _) = seeded();
    let ctx = Ctx::new(&d.today, Lang::En);
    let jql = "summary ~ \"ÖÖ\" AND nonsense = 1 AND ü = 2";
    let e = run_query(&c, &ctx, Scope::Project(d.project_id), jql, Page::ALL).unwrap_err();
    let ps = e.problems(Lang::En);
    assert_eq!(ps.len(), 2, "{ps:?}");
    // „summary ~ "ÖÖ" AND “ sind 19 Zeichen (21 Bytes) vor `nonsense`
    assert_eq!((ps[0].column, ps[0].start, ps[0].len), (20, 21, 8));
    assert_eq!(&jql[ps[0].start..ps[0].start + ps[0].len], "nonsense");
    assert_eq!(&jql[ps[1].start..ps[1].start + ps[1].len], "ü");
    assert_eq!(ps[1].column, jql.chars().position(|ch| ch == 'ü').unwrap() + 1);
    assert!(e.message(Lang::De).contains("Position 20"));
}

#[test]
fn negated_status_conditions_also_match_tickets_of_types_without_a_workflow() {
    use crate::planning::history::Source;
    use crate::planning::tickets::fixture::Fx;
    use crate::planning::transition::transition_ticket;
    let f = Fx::new();
    crate::planning::workflow::set_workflow(&f.c, f.task, false, true).unwrap();
    let no_status = f.mk(f.task, "Has no status");
    let open = f.mk(f.epic, "Open epic");
    let done = f.mk(f.epic, "Done epic");
    transition_ticket(&f.c, done.id, f.done, &[], &Source::ui()).unwrap();
    assert!(get_ticket_status(&f, no_status.id).is_none() && get_ticket_status(&f, open.id).is_some());
    let ctx = Ctx::new("2026-11-16", Lang::En);
    let keys = |jql: &str| -> Vec<String> { run_query(&f.c, &ctx, Scope::Project(f.project), jql, Page::ALL).unwrap().rows.into_iter().map(|r| r.key).collect() };
    assert_eq!(keys("status = Done"), vec![done.key()]);
    for jql in ["status != Done", "status NOT IN (Done)", "NOT status = Done", "NOT (status IN (Done, \"To Do\") AND type = Epic) AND status != Done"] {
        let got = keys(jql);
        assert!(got.contains(&no_status.key()), "{jql}: ein Ticket ohne Status muss bei einer Verneinung dabei sein, war {got:?}");
        assert!(!got.contains(&done.key()), "{jql}: {got:?}");
    }
    assert_eq!(keys("status IS EMPTY"), vec![no_status.key()]);
    assert_eq!(keys("status IS NOT EMPTY ORDER BY key"), vec![open.key(), done.key()]);
}

fn get_ticket_status(f: &crate::planning::tickets::fixture::Fx, id: i64) -> Option<i64> {
    crate::planning::tickets::get_ticket(&f.c, id).unwrap().status_id
}

// ---------------------------------------------------------------------------------------------
// Gesetze der zweiwertigen Logik auf den vielfältigen Daten von `tests::Sx` (alle eingebauten und benutzerdefinierten Feldarten):
// `NOT C` ist genau das Gegenteil von `C` (kein Ticket fällt wegen `NULL` aus beiden Mengen), `AND` ist der Schnitt, `OR` die
// Vereinigung, und die verneinten Operatoren sind `NOT` der positiven.
// ---------------------------------------------------------------------------------------------

use super::tests::Sx;
use std::collections::BTreeSet;

const POOL: &[&str] = &[
    "type = Epic", "type != Epic", "type IN (Epic, \"User story\")", "type NOT IN (Epic)", "type IS EMPTY", "status = \"In Progress\"", "status != Done",
    "status IN (Done, \"To Do\")", "status NOT IN (Done)", "status IS EMPTY", "status IS NOT EMPTY", "priority = High", "priority != High",
    "priority IN (High, Highest)", "priority NOT IN (Low)", "priority IS EMPTY", "priority IS NOT EMPTY", "priority = NULL", "priority != NULL",
    "summary ~ login", "summary !~ login", "summary = Typo", "summary != Typo", "summary IS EMPTY", "key = PAN-3", "key != PAN-3", "key ~ \"PAN-1\"",
    "key !~ \"PAN-1\"", "key IN (PAN-1, PAN-2)", "key NOT IN (PAN-1)", "parent = PAN-1", "parent != PAN-1", "parent IS EMPTY", "parent IS NOT EMPTY",
    "parent IN (PAN-1, PAN-3)", "parent NOT IN (PAN-1)", "parent = PAN-99", "parent != PAN-99", "created >= 2020-01-01", "created < 2020-01-01",
    "created != 2020-01-01", "updated > 2020-01-01", "updated <= 2020-01-01", "flagged = true", "flagged != true", "starred = true", "starred != true",
    "starred = false", "project = PAN", "project != PAN", "changedBy = ui", "changedBy != ui", "changedBy IN (agent, import)", "changedBy NOT IN (ui)",
    "changedBy IS EMPTY", "terminal = t-login", "terminal != t-login", "terminal IS EMPTY", "terminal = linkedTerminal()",
    "terminal != linkedTerminal()", "terminal IN (t-login, t-docs)", "terminal NOT IN (t-docs)", "\"Story points\" > 3", "\"Story points\" <= 3",
    "\"Story points\" = 5", "\"Story points\" != 5", "\"Story points\" IN (2, 8)", "\"Story points\" NOT IN (2, 8)", "\"Story points\" IS EMPTY",
    "Due < currentDate()", "Due >= currentDate()", "Due = 2026-11-10", "Due != 2026-11-10", "Due IN (2026-11-10, 2026-11-20)", "Due NOT IN (2026-11-10)",
    "Due IS EMPTY", "labels = auth", "labels != auth", "labels IN (auth, db)", "labels NOT IN (auth)", "labels ~ au", "labels !~ au", "labels IS EMPTY",
    "components = frontend", "components != frontend", "components IN (backend, db)", "components NOT IN (db)", "components IS EMPTY", "reviewed = true",
    "reviewed != true", "reviewed = false", "reviewed IS EMPTY", "start >= 2026-11-15", "start < 2026-11-15T09:30", "start != 2026-11-15T09:30",
    "start IS EMPTY", "epic = PAN-1", "epic != PAN-1", "epic IS EMPTY", "epic IN (PAN-1, PAN-2)", "IS favorite", "IS NOT favorite", "IS blocked",
    "IS NOT blocked", "\"login\"", "\"%\"", "\"ü\"", "\"t-login\"", "\"auth\"",
];

fn set(s: &Sx, jql: &str) -> BTreeSet<String> {
    s.q(jql).into_iter().collect()
}

#[test]
fn negation_is_the_exact_complement_for_every_field_and_operator() {
    let s = Sx::new();
    let all = set(&s, "");
    assert_eq!(all.len(), 11);
    for c in POOL {
        let (yes, no) = (set(&s, c), set(&s, &format!("NOT ({c})")));
        assert!(yes.is_disjoint(&no), "{c}: Ticket in beiden Mengen: {:?}", yes.intersection(&no).collect::<Vec<_>>());
        let union: BTreeSet<String> = yes.union(&no).cloned().collect();
        assert_eq!(union, all, "{c}: diese Tickets fehlen in `C` und in `NOT C`: {:?}", all.difference(&union).collect::<Vec<_>>());
        // doppelte Verneinung
        assert_eq!(set(&s, &format!("NOT NOT ({c})")), yes, "{c}");
    }
}

#[test]
fn and_is_the_intersection_and_or_the_union() {
    let s = Sx::new();
    let sets: Vec<BTreeSet<String>> = POOL.iter().map(|c| set(&s, c)).collect();
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    for _ in 0..1500 {
        let (i, j) = (rng.below(POOL.len()), rng.below(POOL.len()));
        let (a, b) = (POOL[i], POOL[j]);
        let and: BTreeSet<String> = sets[i].intersection(&sets[j]).cloned().collect();
        let or: BTreeSet<String> = sets[i].union(&sets[j]).cloned().collect();
        assert_eq!(set(&s, &format!("({a}) AND ({b})")), and, "({a}) AND ({b})");
        assert_eq!(set(&s, &format!("({a}) OR ({b})")), or, "({a}) OR ({b})");
        // De Morgan
        assert_eq!(set(&s, &format!("NOT (({a}) OR ({b}))")), set(&s, &format!("NOT ({a}) AND NOT ({b})")), "{a} / {b}");
    }
}

#[test]
fn negated_operators_are_not_of_the_positive_ones() {
    let s = Sx::new();
    let pairs = [
        ("type != Epic", "NOT type = Epic"),
        ("status != Done", "NOT status = Done"),
        ("status NOT IN (Done, \"To Do\")", "NOT status IN (Done, \"To Do\")"),
        ("priority != High", "NOT priority = High"),
        ("summary !~ login", "NOT summary ~ login"),
        ("parent != PAN-1", "NOT parent = PAN-1"),
        ("changedBy != ui", "NOT changedBy = ui"),
        ("terminal != t-login", "NOT terminal = t-login"),
        ("\"Story points\" != 5", "NOT \"Story points\" = 5"),
        ("Due != 2026-11-10", "NOT Due = 2026-11-10"),
        ("labels != auth", "NOT labels = auth"),
        ("labels !~ au", "NOT labels ~ au"),
        ("components NOT IN (db)", "NOT components IN (db)"),
        ("reviewed != true", "NOT reviewed = true"),
        ("start != 2026-11-15T09:30", "NOT start = 2026-11-15T09:30"),
        ("epic != PAN-1", "NOT epic = PAN-1"),
        ("starred != true", "NOT starred = true"),
        ("priority IS NOT EMPTY", "NOT priority IS EMPTY"),
        ("priority != NULL", "priority IS NOT EMPTY"),
        ("priority = NULL", "priority IS EMPTY"),
        ("labels IS NOT NULL", "NOT labels IS NULL"),
    ];
    for (a, b) in pairs {
        assert_eq!(set(&s, a), set(&s, b), "{a} <> {b}");
    }
}

#[test]
fn the_laws_also_hold_in_all_projects_when_field_kinds_clash() {
    use crate::planning::fields::{FieldKind, FieldOptions};
    use crate::planning::history::Source;
    use crate::planning::tickets::{create_ticket, NewTicket};
    use serde_json::json;
    let s = Sx::new();
    let other = s.f.other_project("OTH");
    let types = crate::planning::types::list_types(&s.f.c, other).unwrap();
    let ty = types[0].id;
    // „Due“ ist im Projekt PAN ein Datum, hier ein Text: die Art mit den meisten Feldern (Datum) gewinnt, das Textfeld ist nicht abfragbar
    let due = s.f.add_field(ty, "Due", FieldKind::Text, FieldOptions::default(), false);
    for (title, value) in [("Foreign login", Some("2026-11-10")), ("Plain", None)] {
        let values = value.map(|v| vec![(due.id, json!(v))]).unwrap_or_default();
        create_ticket(&s.f.c, &NewTicket { project_id: other, type_id: ty, title: title.into(), parent_id: None, values }, &Source::ui()).unwrap();
    }
    let schema = crate::planning::search::search_schema(&s.f.c, Scope::All, Lang::En).unwrap();
    assert!(schema.conflicts.iter().any(|c| c.name.eq_ignore_ascii_case("due")), "{:?}", schema.conflicts);
    let run = |jql: &str| -> BTreeSet<String> {
        run_query(&s.f.c, &s.ctx, Scope::All, jql, Page::ALL).unwrap_or_else(|e| panic!("{jql}: {}", e.message(Lang::En))).rows.into_iter().map(|r| r.key).collect()
    };
    let all = run("");
    assert_eq!(all.len(), 13);
    for c in POOL {
        let (yes, no) = (run(c), run(&format!("NOT ({c})")));
        assert!(yes.is_disjoint(&no), "{c}");
        assert_eq!(yes.union(&no).cloned().collect::<BTreeSet<_>>(), all, "{c}");
    }
}
