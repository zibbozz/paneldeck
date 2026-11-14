//! Fuzz-artige Prüfungen mit festem Startwert: Müll-Eingaben aus Bausteinen (Operatoren, Quotes, Klammern, Umlaute, Emoji …)
//! dürfen den Parser, den Prüfer und die Autovervollständigung nie abstürzen lassen, hängen lassen oder Positionen mitten in ein
//! Mehrbyte-Zeichen setzen. Bei gültigen Abfragen muss die kanonische Ausgabe wieder dasselbe ergeben.

use super::*;
use crate::state::Lang;
use std::time::{Duration, Instant};

/// Kleiner deterministischer Zufallsgenerator (xorshift64*).
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
}

const PIECES: &[&str] = &[
    "(", ")", ",", "=", "!=", "~", "!~", "<", ">", "<=", ">=", "!", "AND", "OR", "NOT", "IN", "IS", "EMPTY", "NULL", "ORDER", "BY", "SORT",
    "ASC", "DESC", "\"", "'", "\\", "\\n", "\\q", "ä", "Ü", "ß", "İ", "😀", "日本語", "é", " ", "  ", "\n", "\t", "\u{a0}", "\u{0}", "status",
    "priority", "due", "created", "estimate", "Größe", "labels", "favorite", "favorites", "sprint", "parent", "shell", "2026-11-14",
    "2026-11-14T10:30", "2026-13-45", "-5", "3.14", "1e5", "PAN-12", "x", "currentDate()", "currentDate(", "activeSprint()", "linkedTerminal(",
    "foo(", "foo(bar(", "\"a b\"", "'a b'", "\"Release version\"", "a.b:c-d", "...", "--", "::", "@", "#", "$", "%", "&", "*", "[", "]", "{", "}",
    "|", "^", "`", "?", ";", "+", "/",
];

fn garbage(rng: &mut Rng) -> String {
    let n = 1 + rng.below(40);
    let mut s = String::new();
    for _ in 0..n {
        s.push_str(PIECES[rng.below(PIECES.len())]);
        if rng.below(3) != 0 {
            s.push(' ');
        }
    }
    s
}

fn schema() -> Schema {
    Schema::new()
        .with_field("priority", FieldType::Option(vec!["Highest".into(), "High".into(), "In Progress".into(), "Ünder".into()]))
        .with_field("status", FieldType::Option(vec!["Open".into(), "Done".into()]))
        .with_field("sprint", FieldType::Option(vec!["Sprint 1".into()]))
        .with_field("labels", FieldType::Labels)
        .with_field("Release version", FieldType::Text)
        .with_field("due", FieldType::Date)
        .with_field("created", FieldType::DateTime)
        .with_field("estimate", FieldType::Number)
        .with_field("Größe", FieldType::Number)
        .with_field("archived", FieldType::Bool)
        .with_field("parent", FieldType::TicketRef)
        .with_field("shell", FieldType::Terminal)
        .with_multi_field("components", FieldType::Text)
        .with_predicate("favorite")
        .with_sort_keyword("favorites")
}

fn on_boundary(input: &str, s: Span) -> bool {
    s.end() <= input.len() && input.is_char_boundary(s.start) && input.is_char_boundary(s.end())
}

/// Alle Prüfungen für eine Eingabe.
fn exercise(input: &str, sc: &Schema, with_suggest: bool) {
    match parse(input) {
        Ok(q) => {
            let printed = q.to_string();
            let again = parse(&printed).unwrap_or_else(|e| panic!("kanonische Ausgabe {printed:?} von {input:?} lässt sich nicht lesen: {e:?}"));
            assert_eq!(q, again, "parse(print(parse(x))) != parse(x) für {input:?} (ausgegeben: {printed:?})");
            for i in validate(&q, sc) {
                assert!(on_boundary(input, i.span), "Prüf-Position {:?} liegt nicht auf Zeichengrenzen in {input:?}", i.span);
                assert!(!i.message(Lang::De).is_empty() && !i.message(Lang::En).is_empty());
            }
        }
        Err(errors) => {
            assert!(!errors.is_empty());
            for e in &errors {
                assert!(on_boundary(input, e.span), "Fehler-Position {:?} liegt nicht auf Zeichengrenzen in {input:?}", e.span);
                assert!(!e.message(Lang::De).is_empty() && !e.message(Lang::En).is_empty());
            }
        }
    }
    if with_suggest {
        let cursors: Vec<usize> = input.char_indices().map(|(i, _)| i).chain(std::iter::once(input.len())).collect();
        for c in cursors {
            for s in suggest(input, c, sc) {
                assert!(s.replace_range.start <= s.replace_range.end && s.replace_range.end <= input.len(), "Bereich {:?} in {input:?}", s.replace_range);
                assert!(input.is_char_boundary(s.replace_range.start) && input.is_char_boundary(s.replace_range.end), "Bereich {:?} in {input:?}", s.replace_range);
            }
        }
    }
}

#[test]
fn random_garbage_never_panics_and_positions_are_char_boundaries() {
    let sc = schema();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let started = Instant::now();
    for _ in 0..30_000 {
        let input = garbage(&mut rng);
        let one = Instant::now();
        exercise(&input, &sc, input.len() < 120);
        assert!(one.elapsed() < Duration::from_secs(2), "Eingabe {input:?} dauerte {:?}", one.elapsed());
    }
    assert!(started.elapsed() < Duration::from_secs(120), "Fuzz-Lauf dauerte {:?}", started.elapsed());
}

/// Wie der Haupt-Thread von Windows: 1 MiB Stapel.
fn on_small_stack(name: &str, f: impl FnOnce() + Send + 'static) {
    let h = std::thread::Builder::new().name(name.into()).stack_size(1 << 20).spawn(f).expect("Thread");
    h.join().unwrap_or_else(|_| panic!("{name}: Absturz (Panic oder Stapelüberlauf)"));
}

#[test]
fn long_and_deep_inputs_do_not_overflow_a_one_mib_stack() {
    // Die längste Abfrage der Anwendung hat 4000 Zeichen; zusätzlich deutlich mehr, falls ein Weg die Grenze umgeht.
    for chars in [4_000usize, 100_000] {
        on_small_stack("deep", move || {
            let sc = schema();
            let cases: Vec<String> = vec![
                "(".repeat(chars),
                format!("{}a = 1{}", "(".repeat(chars / 2), ")".repeat(chars / 2)),
                "NOT ".repeat(chars / 4),
                format!("estimate = {}", "a(".repeat(chars / 2)),
                format!("estimate = {}1{}", "a(".repeat(chars / 4), ")".repeat(chars / 4)),
                format!("estimate IN ({}", "a(".repeat(chars / 2)),
                format!("status IN ({}", "x, ".repeat(chars / 3)),
                "a = 1 AND ".repeat(chars / 10),
                "a = 1 OR ".repeat(chars / 9),
                "\"".repeat(chars),
                "\\".repeat(chars),
                "'a\\".repeat(chars / 3),
                "😀".repeat(chars),
                "ä = ".repeat(chars / 4),
                "ORDER BY a, ".repeat(chars / 12),
                "IS ".repeat(chars / 3),
                ")".repeat(chars),
                "a IN (".repeat(chars / 6),
            ];
            for c in &cases {
                exercise(c, &sc, false);
            }
            // Autovervollständigung nur am Ende (sonst quadratisch)
            for c in &cases {
                let _ = suggest(c, c.len(), &sc);
                let _ = suggest(c, c.len() / 2, &sc);
            }
        });
    }
}
