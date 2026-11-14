//! Prüfungen der Werkzeug-Schnittstelle als Ganzes: das veröffentlichte Schema (`mcp::tools()`) und die Parameterprüfung der Werkzeuge
//! stimmen überein, beliebige Argumente (falsche Typen, riesige Zahlen, Umlaute, Emoji) führen nie zu einem Absturz oder einem
//! unbekannten Fehlercode, und lesende Werkzeuge schreiben nie in die Datenbank.

use super::*;
use crate::planning::tickets::fixture::Fx;
use chrono::NaiveDate;

/// Alle Fehlercodes der Schnittstelle (öffentliche API).
const CODES: [&str; 20] = [
    "bad_args",
    "not_found",
    "forbidden",
    "invalid_value",
    "required_missing",
    "transition_needs_fields",
    "transition_not_allowed",
    "conditions_not_met",
    "hierarchy_violation",
    "link_not_allowed",
    "already_exists",
    "too_large",
    "wrong_mode",
    "invalid_query",
    "timeout",
    "feature_disabled",
    "no_workflow",
    "status_not_in_workflow",
    "conflict",
    "db_error",
];

fn ctx(f: &Fx, lang: Lang) -> Ctx<'_> {
    Ctx { conn: &f.c, lang, source: Source::agent("Builder"), now: NaiveDate::from_ymd_opt(2026, 11, 14).unwrap().and_hms_opt(12, 0, 0).unwrap() }
}

/// Werkzeuge dieser Datei (ohne `link_terminal`, das die Anwendung selbst ausführt) mit Stufe und Parameternamen aus dem Schema.
fn planning_tools() -> Vec<(&'static str, crate::mcp::McpLevel, Vec<String>)> {
    crate::mcp::tools()
        .into_iter()
        .filter(|t| TOOLS.contains(&t.name) && t.name != "link_terminal")
        .map(|t| {
            let props: Vec<String> = t.schema["properties"].as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
            (t.name, t.level, props)
        })
        .collect()
}

/// Objekt aus festen Textwerten und einem Parameter mit Beispielwert.
fn with(base: &[(&str, &str)], key: &str, v: Value) -> Value {
    let mut m = Map::new();
    for (k, x) in base {
        m.insert((*k).to_string(), json!(x));
    }
    m.insert(key.to_string(), v);
    Value::Object(m)
}

fn fixture() -> Fx {
    let f = Fx::new();
    f.mk(f.epic, "Checkout");
    f.mk(f.task, "Login fix");
    f
}

/// Beispielwerte für einen Parameter: falsche Typen, Grenzfälle, Mehrbyte-Text.
fn samples() -> Vec<Value> {
    vec![
        Value::Null,
        json!(true),
        json!(-1),
        json!(0),
        json!(1),
        json!(i64::MAX),
        json!(u64::MAX),
        json!(1.5),
        json!(1e300),
        json!(""),
        json!("  "),
        json!("x"),
        json!("PAN"),
        json!("PAN-1"),
        json!("Ünïcödé 😀 日本語"),
        json!("ä".repeat(300)),
        json!("x".repeat(30_000)),
        json!([]),
        json!([1, 2]),
        json!(["a", "PAN-1"]),
        json!({}),
        json!({"a": 1, "nested": {"b": [1, 2, null]}}),
    ]
}

#[test]
fn the_published_schema_names_exactly_the_parameters_the_tools_accept() {
    let f = fixture();
    for (name, _, props) in planning_tools() {
        for lang in [Lang::En, Lang::De] {
            let e = call(&ctx(&f, lang), name, &json!({"__unknown_parameter__": 1}), false).expect_err(name);
            assert_eq!(e.code, "bad_args", "{name}");
            // „(allowed: a, b)“ bzw. „(erlaubt: a, b)“
            let list = e.message.rsplit_once(": ").map(|(_, l)| l.trim_end_matches(')').to_string()).unwrap_or_default();
            let mut allowed: Vec<String> = list.split(", ").map(str::to_string).filter(|s| !s.is_empty()).collect();
            let mut declared = props.clone();
            allowed.sort();
            declared.sort();
            assert_eq!(allowed, declared, "{name} ({lang:?}): Schema und Parameterprüfung weichen ab: {}", e.message);
        }
    }
}

#[test]
fn arbitrary_arguments_never_panic_and_errors_always_carry_a_known_code() {
    for (name, level, props) in planning_tools() {
        let f = fixture();
        for lang in [Lang::En, Lang::De] {
            let mut calls: Vec<Value> = vec![Value::Null, json!(5), json!("x"), json!([]), json!({})];
            for p in &props {
                for s in samples() {
                    calls.push(with(&[], p, s));
                }
            }
            // zwei Parameter zugleich, mit einem gültigen Projekt/Ticket als Rahmen
            for p in &props {
                for s in samples() {
                    calls.push(with(&[("project", "PAN"), ("ticket", "PAN-1")], p, s));
                }
            }
            for args in calls {
                let before = f.c.total_changes();
                let r = call(&ctx(&f, lang), name, &args, false);
                if let Err(e) = &r {
                    assert!(CODES.contains(&e.code), "{name}: unbekannter Fehlercode {:?} bei {}", e.code, short(&args.to_string(), 200));
                    assert!(!e.message.is_empty(), "{name}: leere Meldung");
                }
                if level == crate::mcp::McpLevel::Read {
                    assert_eq!(f.c.total_changes(), before, "{name} (Stufe Lesen) hat geschrieben bei {}", short(&args.to_string(), 200));
                }
            }
        }
    }
}

#[test]
fn read_tools_never_write_even_with_valid_arguments() {
    let f = fixture();
    let valid = [
        ("list_todos", json!({})),
        ("list_projects", json!({})),
        ("get_project", json!({"project": "PAN"})),
        ("search_tickets", json!({"project": "PAN", "query": "type = Epic ORDER BY key"})),
        ("search_tickets", json!({"text": "login", "limit": 5})),
        ("get_ticket", json!({"ticket": "PAN-1"})),
        ("get_time_entries", json!({"ticket": "PAN-1"})),
        ("list_sprints", json!({"project": "PAN"})),
        ("list_versions", json!({"project": "PAN"})),
    ];
    for (tool, args) in valid {
        let before = f.c.total_changes();
        let _ = call(&ctx(&f, Lang::En), tool, &args, true);
        assert_eq!(f.c.total_changes(), before, "{tool} hat geschrieben");
    }
}

#[test]
fn page_parameters_are_clamped_and_reported_back() {
    let f = fixture();
    let d = call(&ctx(&f, Lang::En), "search_tickets", &json!({"limit": 0, "offset": -5}), false).unwrap();
    assert_eq!((d.result["limit"].clone(), d.result["offset"].clone()), (json!(1), json!(0)));
    assert_eq!(d.result["tickets"].as_array().unwrap().len(), 1);
    assert_eq!(d.result["total"], 2);
    let d = call(&ctx(&f, Lang::En), "search_tickets", &json!({"query": "", "offset": 99}), false).unwrap();
    assert_eq!(d.result["tickets"].as_array().unwrap().len(), 0);
    assert_eq!(d.result["total"], 2, "total bleibt die Gesamtzahl, auch hinter dem Ende");
    let e = call(&ctx(&f, Lang::En), "search_tickets", &json!({"limit": 1.5}), false).unwrap_err();
    assert_eq!(e.code, "bad_args");
}
