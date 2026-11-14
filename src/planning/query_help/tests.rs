//! Prüfungen der Hilfe-Daten: jedes Beispiel parst, validiert, hat die erwarteten Treffer; beide Sprachen sind vollständig;
//! Referenz und Fehlersuche stimmen mit Parser und Validator überein.

use super::*;
use crate::query::{parse, validate};

fn doc() -> &'static HelpDoc {
    HelpDoc::builtin()
}

/// Alle zweisprachigen Texte des Dokuments samt Ort (für Fehlermeldungen).
fn all_texts(doc: &HelpDoc) -> Vec<(String, Text)> {
    let mut v = Vec::new();
    for s in &doc.sections {
        v.push((format!("{} / title", s.id), s.title));
        for (i, b) in s.blocks.iter().enumerate() {
            let at = format!("{} / block {i}", s.id);
            match b {
                Block::Paragraph(t) | Block::Note(t) => v.push((at, *t)),
                Block::Code(_) => {}
                Block::Example(e) => {
                    v.push((format!("{} / title", e.id), e.title));
                    v.push((format!("{} / explanation", e.id), e.explanation));
                }
                Block::Table { header, rows } => {
                    for (j, t) in header.iter().enumerate() {
                        v.push((format!("{at} header {j}"), *t));
                    }
                    for (r, row) in rows.iter().enumerate() {
                        for (c, t) in row.iter().enumerate() {
                            v.push((format!("{at} row {r} col {c}"), *t));
                        }
                    }
                }
            }
        }
    }
    for l in [Level::Beginner, Level::Intermediate, Level::Advanced] {
        v.push((format!("level {l:?}"), l.label()));
    }
    v
}

/// Texte, die in beiden Sprachen gleich sein dürfen: Code, einzelne Wörter/Namen, Wertelisten der Beispieldaten.
fn language_neutral(s: &str) -> bool {
    const NAMES: [&str; 4] = [
        "Epic, User story, Dev task, Subtask, Defect",
        "Highest, High, Medium, Low, Lowest",
        "frontend, backend, api, auth, payments, legacy",
        "api-work, login-fix, docs",
    ];
    if NAMES.contains(&s) || !s.trim().contains(' ') || (s.starts_with('"') && s.ends_with('"')) {
        return true;
    }
    // ohne Code-Stücke (zwischen Backticks) bleibt kein Buchstabe übrig
    let mut rest = String::new();
    for (i, part) in s.split('`').enumerate() {
        if i % 2 == 0 {
            rest.push_str(part);
        }
    }
    !rest.chars().any(char::is_alphabetic)
}

#[test]
fn builtin_document_has_the_planned_shape() {
    let n = doc().examples().len();
    assert!((20..=30).contains(&n), "{n} Beispiele");
    for id in ["start", "data", "filters", "text", "empty", "dates", "sprints", "terminals", "flags", "logic", "sorting", "advanced", "troubleshooting", "reference"] {
        assert!(doc().section(id).is_some(), "Abschnitt {id}");
    }
    for level in [Level::Beginner, Level::Intermediate, Level::Advanced] {
        assert!(doc().examples().iter().any(|e| e.level == level), "kein Beispiel der Stufe {level:?}");
    }
    // die Beispiele gehen von einfach nach komplex: die Profi-Beispiele stehen am Ende
    let last_advanced = doc().examples().iter().rposition(|e| e.level == Level::Advanced).unwrap();
    assert_eq!(last_advanced, n - 1);
}

#[test]
fn ids_are_unique_and_well_formed() {
    let mut ids: Vec<&str> = doc().examples().iter().map(|e| e.id).collect();
    let total = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), total, "doppelte Beispiel-IDs");
    for id in &ids {
        assert!(!id.is_empty() && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'), "ID {id}");
    }
    let mut secs: Vec<&str> = doc().sections.iter().map(|s| s.id).collect();
    let total = secs.len();
    secs.sort();
    secs.dedup();
    assert_eq!(secs.len(), total, "doppelte Abschnitts-IDs");
}

#[test]
fn every_example_parses_and_validates_against_its_demo_schema() {
    for e in doc().examples() {
        let q = parse(&e.jql).unwrap_or_else(|err| panic!("{}: {:?}", e.id, err.iter().map(|x| x.message(Lang::En)).collect::<Vec<_>>()));
        let issues = validate(&q, &e.demo.schema());
        assert!(issues.is_empty(), "{}: {:?}", e.id, issues.iter().map(|i| i.message(Lang::En)).collect::<Vec<_>>());
    }
}

#[test]
fn every_example_round_trips_through_to_string() {
    for e in doc().examples() {
        let q = parse(&e.jql).unwrap();
        let printed = q.to_string();
        let q2 = parse(&printed).unwrap_or_else(|_| panic!("{}: `{printed}` lässt sich nicht parsen", e.id));
        assert_eq!(q, q2, "{}: parse(print(parse(x))) != parse(x)", e.id);
        assert_eq!(printed, q2.to_string(), "{}: Ausgabe nicht stabil", e.id);
    }
}

#[test]
fn every_demo_is_consistent() {
    for e in doc().examples() {
        assert_eq!(e.demo.check(), Vec::<String>::new(), "{}", e.id);
        assert!(!e.demo.tickets.is_empty(), "{}", e.id);
    }
}

#[test]
fn expected_keys_match_the_hand_written_results_exactly() {
    for e in doc().examples() {
        let got = e.demo.expected_keys(&e.jql).unwrap_or_else(|err| panic!("{}: {err}", e.id));
        assert_eq!(got, e.expect, "{}: {}", e.id, e.jql);
        assert_eq!(e.expected_keys(), got, "{}", e.id);
    }
}

#[test]
fn examples_have_hits_unless_marked_otherwise() {
    for e in doc().examples() {
        if e.expect_none {
            assert!(e.expected_keys().is_empty(), "{} ist als „keine Treffer“ markiert, hat aber welche", e.id);
        } else {
            assert!(!e.expected_keys().is_empty(), "{} hat keine Treffer", e.id);
        }
        // ein gutes Beispiel trifft nicht alles: es gibt auch Nicht-Treffer
        assert!(e.expected_keys().len() < e.demo.tickets.len(), "{} trifft jedes Ticket", e.id);
    }
}

#[test]
fn results_do_not_depend_on_the_day() {
    for e in doc().examples() {
        let base = e.demo.evaluate(&e.jql, "2026-11-16").unwrap();
        for today in ["2024-02-29", "2026-12-31", "2031-07-04"] {
            assert_eq!(e.demo.evaluate(&e.jql, today).unwrap(), base, "{} am {today}", e.id);
        }
    }
}

#[test]
fn demos_survive_json_and_resolve_to_absolute_dates() {
    for e in doc().examples() {
        let json = serde_json::to_string(&e.demo).unwrap();
        let back: DemoSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(back, e.demo, "{}", e.id);
        let abs = e.demo.resolved("2026-11-16");
        assert!(!serde_json::to_string(&abs).unwrap().contains('@'), "{}: relatives Datum übrig", e.id);
        assert_eq!(abs.check(), Vec::<String>::new(), "{}", e.id);
    }
    let d = shop().resolved("2026-11-16");
    assert_eq!(d.ticket("WEB-14").unwrap().values["Due"], DemoValue::Text("2026-11-15".into()));
    assert_eq!(d.sprints[1].end, "2026-11-23");
    assert_eq!(resolve_day("@+2T14:30", chrono::NaiveDate::from_ymd_opt(2026, 11, 30).unwrap()), "2026-12-02T14:30");
    assert_eq!(resolve_day("2026-01-01", chrono::NaiveDate::from_ymd_opt(2026, 11, 30).unwrap()), "2026-01-01");
}

#[test]
fn demo_check_finds_problems() {
    let mut d = shop();
    assert!(d.check().is_empty());
    d.add(DemoTicket::new("Bug", "x").status("Nope").priority("Meh").sprint("Sprint 99").parent("WEB-999").num("Release version", 1.0));
    let problems = d.check().join("\n");
    for needle in ["unknown type", "unknown status", "unknown priority", "unknown sprint", "unknown parent", "does not exist on type", "wrong shape"] {
        assert!(problems.contains(needle), "{needle}\n{problems}");
    }
}

#[test]
fn texts_exist_in_both_languages_and_differ() {
    for (at, t) in all_texts(doc()) {
        assert!(!t.en.trim().is_empty(), "{at}: Englisch fehlt");
        assert!(!t.de.trim().is_empty(), "{at}: Deutsch fehlt");
        if t.en == t.de {
            assert!(language_neutral(t.en), "{at}: Deutsch ist gleich Englisch: {:?}", t.en);
        }
        assert_eq!(t.get(Lang::En), t.en);
        assert_eq!(t.get(Lang::De), t.de);
        assert_eq!(t.en.matches('`').count() % 2, 0, "{at}: Backticks in {:?}", t.en);
        assert_eq!(t.de.matches('`').count() % 2, 0, "{at}: Backticks in {:?}", t.de);
        assert_eq!(t.en.matches("**").count() % 2, 0, "{at}: ** in {:?}", t.en);
        assert_eq!(t.de.matches("**").count() % 2, 0, "{at}: ** in {:?}", t.de);
    }
}

#[test]
fn both_languages_mention_the_same_code_and_keys() {
    // Code in Backticks und Ticket-Schlüssel müssen in beiden Sprachen vorkommen (Übersetzungen ändern Code nicht).
    let spans = |s: &str| -> Vec<String> { s.split('`').enumerate().filter(|(i, _)| i % 2 == 1).map(|(_, p)| p.to_string()).collect() };
    for e in doc().examples() {
        let (mut en, mut de) = (spans(e.explanation.en), spans(e.explanation.de));
        en.sort();
        de.sort();
        assert_eq!(en, de, "{}: Code-Stücke der Erklärung weichen zwischen EN und DE ab", e.id);
        let keys = |s: &str| -> Vec<String> { s.split(|c: char| !(c.is_alphanumeric() || c == '-')).filter(|w| w.starts_with("WEB-")).map(String::from).collect() };
        assert_eq!(keys(e.explanation.en), keys(e.explanation.de), "{}", e.id);
        for k in keys(e.explanation.en) {
            assert!(e.demo.ticket(&k).is_some(), "{}: nennt {k}, das es in den Beispieldaten nicht gibt", e.id);
        }
    }
}

#[test]
fn tables_are_rectangular() {
    for s in &doc().sections {
        for b in &s.blocks {
            if let Block::Table { header, rows } = b {
                assert!(!rows.is_empty(), "{}", s.id);
                for r in rows {
                    assert_eq!(r.len(), header.len(), "{}: {:?}", s.id, r.first().map(|t| t.en));
                }
            }
        }
    }
}

#[test]
fn examples_together_cover_the_whole_language() {
    let all: String = doc().examples().iter().map(|e| e.jql.to_lowercase() + "\n").collect();
    for needle in [
        " = ", " != ", " ~ ", " !~ ", " < ", " > ", " <= ", " >= ", " in (", " not in (", " is empty", " is not empty", " and ", " or ", "not (", "(", "order by", "sort by",
        " asc", " desc", "activesprint()", "currentdate()", "linkedterminal()", "is favorite", "is not blocked", "sort by favorites", "changedby = agent", "\"release version\"",
        "status in (done, \"development finished\")", "type = epic and",
    ] {
        assert!(all.contains(needle), "kein Beispiel nutzt {needle:?}");
    }
    // Freitext ohne Feld
    assert!(doc().examples().iter().any(|e| e.jql.starts_with('"') && e.jql.ends_with('"') && !e.jql.contains('=')));
}

#[test]
fn the_users_epic_example_is_there() {
    let e = doc().example("epics-release").unwrap();
    assert_eq!(e.jql, r#"type = Epic AND "Release version" = 2.0.0 AND status IN (Done, "Development finished") ORDER BY priority"#);
    assert_eq!(e.level, Level::Advanced);
    // „Development finished“ ist ein benutzerdefinierter Status der Beispieldaten, kein Standard-Status
    assert!(e.demo.statuses.iter().any(|s| s.name == "Development finished"));
    assert_eq!(e.expected_keys(), ["WEB-2", "WEB-1"]);
}

// -------------------------------------------------------------------------------------------
// Aussagen der Erklärungen (damit der Text nichts Falsches behauptet)
// -------------------------------------------------------------------------------------------

fn keys(jql: &str) -> Vec<String> {
    shop().expected_keys(jql).unwrap_or_else(|e| panic!("{jql}: {e}"))
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort_by_key(|k| k.trim_start_matches("WEB-").parse::<u32>().unwrap());
    v
}

#[test]
fn claims_made_in_the_explanations_hold() {
    // Groß-/Kleinschreibung ist egal
    assert_eq!(keys("Type = defect"), keys("type = Defect"));
    assert_eq!(keys("type = defect and STATUS = done"), keys("type = Defect AND status = Done"));
    // IN ist die Kurzform von OR
    assert_eq!(keys("priority IN (High, Highest)"), sorted(keys("priority = High OR priority = Highest")));
    // NOT (a OR b) = NOT IN
    assert_eq!(
        keys(r#"type = "User story" AND NOT (status = Done OR status = "Development finished")"#),
        keys(r#"type = "User story" AND status NOT IN (Done, "Development finished")"#)
    );
    // Vorrang: AND vor OR
    assert_eq!(keys("type = Defect AND priority = High OR priority = Highest"), sorted(keys("(type = Defect AND priority = High) OR priority = Highest")));
    assert_ne!(keys("type = Defect AND priority = High OR priority = Highest"), keys("type = Defect AND (priority = High OR priority = Highest)"));
    // != trifft auch leere Felder, = und < nie
    assert!(keys(r#""Release version" != 2.0.0"#).contains(&"WEB-4".to_string()));
    assert!(keys("due != 2030-01-01").contains(&"WEB-1".to_string()));
    assert!(!keys("due < 2030-01-01").contains(&"WEB-1".to_string()));
    // Label-Felder: = „enthält“, != „enthält nicht“; beide zusammen ergeben alle Tickets
    let with = keys("labels = auth");
    let without = keys("labels != auth");
    assert_eq!(with.len() + without.len(), shop().tickets.len());
    assert!(with.iter().all(|k| !without.contains(k)));
    // IS NULL ist IS EMPTY
    assert_eq!(keys("parent IS NULL"), keys("parent IS EMPTY"));
    assert_eq!(keys("parent = EMPTY"), keys("parent IS EMPTY"));
    assert_eq!(keys("parent != NULL"), keys("parent IS NOT EMPTY"));
    // = bei Text verlangt den ganzen Text, ~ nicht
    assert!(keys(r#"summary = "login""#).is_empty());
    assert!(!keys(r#"summary ~ "LOGIN""#).is_empty());
    // SORT BY = ORDER BY, DESC kehrt um, leere Werte immer hinten
    assert_eq!(keys("type = Defect SORT BY priority"), keys("type = Defect ORDER BY priority"));
    assert_eq!(keys("type = Defect ORDER BY priority ASC"), ["WEB-14", "WEB-15", "WEB-18", "WEB-17", "WEB-16"]);
    assert_eq!(keys("type = Defect ORDER BY priority DESC"), ["WEB-16", "WEB-17", "WEB-15", "WEB-18", "WEB-14"]);
    assert_eq!(keys("type = Defect ORDER BY due ASC").last().unwrap(), "WEB-16");
    assert_eq!(keys("type = Defect ORDER BY due DESC").last().unwrap(), "WEB-16");
    // SORT BY favorites stellt markierte Tickets nach vorn
    assert_eq!(&keys("type = Defect SORT BY favorites")[..1], ["WEB-14"]);
    // Prädikate
    assert_eq!(keys("IS blocked"), ["WEB-10", "WEB-15"]);
    assert_eq!(keys("IS NOT blocked").len(), shop().tickets.len() - 2);
}

#[test]
fn reference_evaluation_reports_what_it_cannot_do() {
    let d = shop();
    assert!(d.expected_keys("staus = Done").unwrap_err().contains("Unknown field"));
    assert!(d.expected_keys("type =").is_err());
    let mut no_sprint = d.clone();
    no_sprint.sprints.iter_mut().for_each(|s| s.state = SprintState::Closed);
    assert!(no_sprint.expected_keys("sprint = activeSprint()").unwrap_err().contains("active sprint"));
    let mut no_term = d.clone();
    no_term.current_terminal = None;
    assert!(no_term.expected_keys("terminal = linkedTerminal()").is_err());
    // Projekt ohne Sprints: das Feld gibt es nicht
    let mut plain = d.clone();
    plain.sprints.clear();
    plain.tickets.iter_mut().for_each(|t| t.sprint = None);
    assert!(plain.expected_keys("sprint = activeSprint()").is_err());
    // leere Abfrage = alle
    assert_eq!(keys("").len(), 22);
    assert_eq!(keys("ORDER BY priority").len(), 22);
}

// -------------------------------------------------------------------------------------------
// Fehlersuche und Referenz
// -------------------------------------------------------------------------------------------

#[test]
fn every_error_case_really_fails_and_its_fix_works() {
    let schema = errors::help_schema();
    let cases = error_cases();
    assert!(cases.len() >= 20);
    let mut ids: Vec<&str> = cases.iter().map(|c| c.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), cases.len(), "doppelte Fehlerfall-IDs");
    for c in &cases {
        for lang in [Lang::En, Lang::De] {
            let msg = errors::first_message(c.bad, &schema, lang).unwrap_or_else(|| panic!("{}: `{}` liefert keinen Fehler", c.id, c.bad));
            assert!(!msg.is_empty(), "{}", c.id);
            assert_eq!(errors::first_message(c.good, &schema, lang), None, "{}: die Lösung `{}` ist selbst fehlerhaft", c.id, c.good);
        }
        let (en, de) = (errors::first_message(c.bad, &schema, Lang::En).unwrap(), errors::first_message(c.bad, &schema, Lang::De).unwrap());
        assert_ne!(en, de, "{}", c.id);
        assert!(!c.cause.en.is_empty() && !c.cause.de.is_empty() && c.cause.en != c.cause.de);
    }
    // alle Fehlerarten von Parser und Validator kommen vor
    let joined: String = cases.iter().map(|c| errors::first_message(c.bad, &schema, Lang::En).unwrap() + "\n").collect();
    for needle in [
        "Expected", "Unexpected end", "Missing closing", "Unterminated", "must not be empty", "Unknown character", "only appear once", "Unknown field", "does not fit the field",
        "not an allowed value", "not a valid date", "Unknown function", "NULL can only", "Unknown predicate", "Unknown sort field", "cannot be sorted", "reserved word",
    ] {
        assert!(joined.contains(needle), "kein Fehlerfall für {needle:?}");
    }
}

#[test]
fn the_troubleshooting_tables_show_the_real_messages() {
    let schema = errors::help_schema();
    let s = doc().section("troubleshooting").unwrap();
    let rows: Vec<&Vec<Text>> = s
        .blocks
        .iter()
        .filter_map(|b| if let Block::Table { rows, .. } = b { Some(rows) } else { None })
        .flatten()
        .filter(|r| r.len() == 4)
        .collect();
    assert_eq!(rows.len(), error_cases().len());
    for (row, case) in rows.iter().zip(error_cases()) {
        assert_eq!(row[0].en, format!("`{}`", case.bad));
        assert_eq!(row[1].en, errors::first_message(case.bad, &schema, Lang::En).unwrap());
        assert_eq!(row[1].de, errors::first_message(case.bad, &schema, Lang::De).unwrap());
        assert_eq!(row[3].en, format!("`{}`", case.good));
    }
}

fn tables(s: &Section) -> Vec<(&Vec<Text>, &Vec<Vec<Text>>)> {
    s.blocks.iter().filter_map(|b| if let Block::Table { header, rows } = b { Some((header, rows)) } else { None }).collect()
}

fn table_with<'a>(s: &'a Section, first_header: &str, columns: usize) -> (&'a Vec<Text>, &'a Vec<Vec<Text>>) {
    tables(s)
        .into_iter()
        .find(|(h, _)| h[0].en == first_header && h.len() == columns)
        .unwrap_or_else(|| panic!("keine Tabelle {first_header} mit {columns} Spalten"))
}

/// Die Stücke in Backticks einer Zelle.
fn code_spans(cell: &Text) -> Vec<String> {
    cell.en.split('`').enumerate().filter(|(i, _)| i % 2 == 1).map(|(_, p)| p.to_string()).collect()
}

#[test]
fn queries_shown_in_the_reference_work() {
    let r = doc().section("reference").unwrap();
    let schema = shop().schema();
    let check = |q: &str| {
        let parsed = parse(q).unwrap_or_else(|e| panic!("Referenz `{q}`: {:?}", e.iter().map(|x| x.message(Lang::En)).collect::<Vec<_>>()));
        let issues = validate(&parsed, &schema);
        assert!(issues.is_empty(), "Referenz `{q}`: {:?}", issues.iter().map(|i| i.message(Lang::En)).collect::<Vec<_>>());
    };
    let (_, ops) = table_with(r, "Operator", 3);
    assert!(ops.len() >= 10);
    for row in ops {
        for q in code_spans(&row[2]) {
            check(&q);
        }
    }
    let (_, funcs) = table_with(r, "Function", 4);
    assert_eq!(funcs.len(), 3);
    for row in funcs {
        for q in code_spans(&row[3]) {
            check(&q);
        }
    }
    let (_, preds) = table_with(r, "Keyword", 3);
    assert_eq!(preds.len(), 3);
    for row in preds {
        for q in code_spans(&row[1]) {
            check(&q);
        }
    }
}

#[test]
fn the_function_table_matches_the_builtin_functions() {
    let r = doc().section("reference").unwrap();
    let (_, funcs) = table_with(r, "Function", 4);
    let mut names: Vec<String> = funcs.iter().map(|f| f[0].en.trim_matches('`').trim_end_matches("()").to_string()).collect();
    names.sort();
    let mut builtin: Vec<String> = crate::query::builtin_functions().into_iter().map(|f| f.name).collect();
    builtin.sort();
    assert_eq!(names, builtin);
}

#[test]
fn reserved_words_are_exactly_those_the_parser_rejects() {
    let r = doc().section("reference").unwrap();
    let words: Vec<String> = r
        .blocks
        .iter()
        .find_map(|b| match b {
            Block::Code(c) if c.starts_with("AND") => Some(c.split_whitespace().map(String::from).collect()),
            _ => None,
        })
        .expect("Code-Block mit den reservierten Wörtern");
    assert_eq!(words.len(), 9);
    for w in &words {
        // als Feldname oder Sortierfeld nicht ohne Anführungszeichen benutzbar, in Anführungszeichen schon
        assert!(parse(&format!("{w} = Done")).is_err(), "{w} sollte als Feldname reserviert sein");
        assert!(parse(&format!("{} = Done", w.to_lowercase())).is_err(), "{w} klein geschrieben");
        assert!(parse(&format!("\"{w}\" = Done")).is_ok(), "{w} in Anführungszeichen");
        assert!(parse(&format!("type = Epic ORDER BY {w}")).is_err(), "{w} als Sortierfeld");
        assert!(parse(&format!("type = Epic ORDER BY \"{w}\"")).is_ok(), "{w} als Sortierfeld in Anführungszeichen");
    }
    // als Wert: EMPTY und NULL bedeuten „leer“, alle anderen sind Fehler
    for w in &words {
        let as_value = parse(&format!("labels = {w}"));
        assert_eq!(as_value.is_ok(), w == "EMPTY" || w == "NULL", "{w} als Wert");
    }
    for w in ["BY", "ASC", "DESC", "by", "asc", "desc"] {
        assert!(parse(&format!("labels = {w}")).is_ok(), "{w} ist nicht reserviert");
        assert!(parse(&format!("{w} = Done")).is_ok(), "{w} als Feldname");
    }
}

#[test]
fn the_operator_matrix_matches_the_validator() {
    let r = doc().section("reference").unwrap();
    let (header, rows) = table_with(r, "Field type", 13);
    assert_eq!(rows.len(), 9);
    // Stichproben gegen echte Validierung: Beispiel-Abfrage je Feldtyp und Operator
    let schema = shop().schema();
    let probe = |q: &str| validate(&parse(q).unwrap(), &schema).is_empty();
    let col = |sym: &str| header.iter().position(|h| h.en == format!("`{sym}`")).unwrap_or_else(|| panic!("Spalte {sym}"));
    let row = |kind: &str| rows.iter().find(|r| r[0].en == kind).unwrap_or_else(|| panic!("Zeile {kind}"));
    let yes = |kind: &str, sym: &str| row(kind)[col(sym)].en == "✓";
    // Text: summary
    assert_eq!(yes("text", "~"), probe(r#"summary ~ "x""#));
    assert_eq!(yes("text", "<"), probe(r#"summary < "x""#));
    // Zahl
    assert_eq!(yes("number", "<="), probe(r#""Story points" <= 3"#));
    assert_eq!(yes("number", "~"), probe(r#""Story points" ~ 3"#));
    // Datum
    assert_eq!(yes("date", ">="), probe("due >= 2026-01-01"));
    assert_eq!(yes("date", "~"), probe("due ~ 2026"));
    // Auswahl
    assert_eq!(yes("dropdown", "IN"), probe("priority IN (High)"));
    assert_eq!(yes("dropdown", "<"), probe("priority < High"));
    // Labels
    assert_eq!(yes("labels", "~"), probe("labels ~ auth"));
    assert_eq!(yes("labels", "<"), probe("labels < auth"));
    // Schlüssel, Terminal
    assert_eq!(yes("key (e.g. PAN-12)", "NOT IN"), probe("parent NOT IN (WEB-1)"));
    assert_eq!(yes("terminal", "~"), probe("terminal ~ x"));
    // IS EMPTY gilt für alle Typen
    assert!(rows.iter().all(|r| r[col("IS EMPTY")].en == "✓" && r[col("IS NOT EMPTY")].en == "✓"));
}

// -------------------------------------------------------------------------------------------
// Markdown und Suche
// -------------------------------------------------------------------------------------------

#[test]
fn markdown_contains_everything_in_both_languages() {
    for lang in [Lang::En, Lang::De] {
        let md = to_markdown(doc(), lang);
        assert!(md.starts_with(&format!("# {}", HELP_TITLE.get(lang))));
        for s in &doc().sections {
            assert!(md.contains(&format!("## {}\n", s.title.get(lang))), "{}", s.id);
        }
        for e in doc().examples() {
            assert!(md.contains(&format!("### {} ({})", e.title.get(lang), e.level.label().get(lang))), "{}", e.id);
            assert!(md.contains(&e.jql), "{}", e.id);
            assert!(md.contains(e.explanation.get(lang)), "{}", e.id);
            assert!(md.contains(&e.expected_keys().join(", ")), "{}", e.id);
        }
        assert_eq!(md.matches("```").count() % 2, 0, "Code-Zäune müssen paarweise stehen");
        assert_eq!(md.lines().filter(|l| l.starts_with("### ")).count(), doc().examples().len());
        assert!(!md.contains("\n\n\n"));
    }
    // Sprachen unterscheiden sich
    assert_ne!(to_markdown(doc(), Lang::En), to_markdown(doc(), Lang::De));
    assert!(to_markdown(doc(), Lang::De).contains("Ergebnis mit den Beispieldaten:"));
    assert!(to_markdown(doc(), Lang::En).contains("Result with the example data:"));
}

#[test]
fn search_finds_examples_sections_and_reference() {
    let d = doc();
    // Treffer über die Abfrage
    let hits = search(d, Lang::En, "activeSprint");
    assert!(hits.iter().any(|h| h.example_id == Some("sprint-active")));
    assert!(hits.windows(2).all(|w| w[0].score >= w[1].score), "sortiert nach Güte");
    // Titel eines Abschnitts steht vor Fließtext
    let hits = search(d, Lang::En, "sprints");
    assert_eq!(hits[0].block, None);
    assert_eq!(hits[0].section_id, "sprints");
    // mehrere Wörter: alle müssen vorkommen, Reihenfolge egal
    let hits = search(d, Lang::En, "coupon description");
    assert!(hits.iter().any(|h| h.example_id == Some("text-free")));
    assert!(search(d, Lang::En, "coupon zzzzqqqq").is_empty());
    // Groß-/Kleinschreibung und Sprache
    assert_eq!(search(d, Lang::En, "OVERDUE"), search(d, Lang::En, "overdue"));
    let de = search(d, Lang::De, "überfällig");
    assert!(de.iter().any(|h| h.example_id == Some("dates-overdue")), "{de:?}");
    assert!(search(d, Lang::En, "überfällig").is_empty());
    // Referenz-Tabellen werden gefunden, Markup stört nicht
    assert!(search(d, Lang::En, "reserved words").iter().any(|h| h.section_id == "reference"));
    assert!(search(d, Lang::En, "IS NOT EMPTY").iter().any(|h| h.section_id == "reference"));
    // leere Anfrage
    assert!(search(d, Lang::En, "").is_empty());
    assert!(search(d, Lang::En, "   ").is_empty());
    // Ausschnitte brechen keine Zeichen (Umlaute, Sonderzeichen)
    for q in ["ä", "a", "e", "ö", "ü", "=", "(", "~", "ß", "„"] {
        for lang in [Lang::En, Lang::De] {
            for h in search(d, lang, q) {
                assert!(!h.section_id.is_empty() && !h.snippet.is_empty());
            }
        }
    }
}

#[test]
fn screenshot_file_names_are_unique_and_safe() {
    let mut names: Vec<String> = doc().examples().iter().map(|e| e.screenshot_file()).collect();
    names.sort();
    let n = names.len();
    names.dedup();
    assert_eq!(names.len(), n);
    assert!(names.iter().all(|f| f.starts_with("query-help-") && f.ends_with(".png")));
}

#[test]
fn the_first_example_is_the_simplest_one() {
    let first = doc().examples()[0];
    assert_eq!(first.jql, "type = Defect");
    assert_eq!(first.level, Level::Beginner);
}
