//! Die Abschnitte „Fehlersuche“ und „Referenz“. Die Meldungen der Fehlersuche und die Operator-Tabelle je Feldtyp werden aus dem
//! echten Parser/Validator erzeugt (siehe `errors.rs`), damit sie nicht von der Software abweichen können.

use super::content::{code, note, p, same, t, table};
use super::errors::{error_cases, first_message, help_schema};
use super::{Block, Section, Text};
use crate::query::{FieldKind, FieldType, OpKind};
use crate::state::Lang;

/// Text, der in beiden Sprachen gleich ist und als Code gesetzt wird (zur Laufzeit gebaut).
fn code_cell(s: &str) -> Text {
    let c = format!("`{s}`");
    Text::leak(c.clone(), c)
}

pub(super) fn troubleshooting() -> Section {
    let schema = help_schema();
    let header = [t("You typed", "Du hast getippt"), t("Message", "Meldung"), t("What it means and what to do", "Was es bedeutet und was zu tun ist"), t("Works", "So geht es")];
    let cases = error_cases();
    let split = cases.iter().position(|c| c.id == "unknown-field").unwrap_or(cases.len());
    let rows = |slice: &[super::ErrorCase]| -> Vec<Vec<Text>> {
        slice
            .iter()
            .map(|c| {
                let message = |lang| first_message(c.bad, &schema, lang).unwrap_or_default();
                vec![code_cell(c.bad), Text::leak(message(Lang::En), message(Lang::De)), c.cause, code_cell(c.good)]
            })
            .collect()
    };
    Section {
        id: "troubleshooting",
        title: t("Troubleshooting", "Fehlersuche"),
        blocks: vec![
            p(
                "While you type, the search field underlines the problem and shows a message in your language, often with a suggestion (“did you mean …?”). These are the mistakes people make most often; the messages below are exactly what the search shows.",
                "Beim Tippen unterstreicht das Suchfeld das Problem und zeigt eine Meldung in deiner Sprache, oft mit einem Vorschlag („meinten Sie …?“). Das sind die häufigsten Fehler; die Meldungen unten sind genau das, was die Suche anzeigt.",
            ),
            p(
                "**The query is not understood** (the structure is wrong):",
                "**Die Abfrage wird nicht verstanden** (der Aufbau stimmt nicht):",
            ),
            table(&header, rows(&cases[..split])),
            p(
                "**The query is understood, but does not fit your data** (a name, value or operator is not valid):",
                "**Die Abfrage wird verstanden, passt aber nicht zu deinen Daten** (ein Name, Wert oder Operator ist ungültig):",
            ),
            table(&header, rows(&cases[split..])),
            p(
                "**No error, but not the result you expected?** Check these first:",
                "**Kein Fehler, aber nicht das erwartete Ergebnis?** Prüfe zuerst:",
            ),
            table(
                &[t("Symptom", "Beobachtung"), t("Likely reason", "Wahrscheinliche Ursache")],
                vec![
                    vec![
                        t("Too many tickets with `!=`", "Zu viele Tickets bei `!=`"),
                        t("`!=`, `!~` and `NOT IN` also match tickets whose field is empty. Add `AND field IS NOT EMPTY`.", "`!=`, `!~` und `NOT IN` treffen auch Tickets mit leerem Feld. Ergänze `AND feld IS NOT EMPTY`."),
                    ],
                    vec![
                        t("Missing tickets with `<` or `>`", "Fehlende Tickets bei `<` oder `>`"),
                        t("A ticket with an empty date or number never matches a comparison. Use `IS EMPTY` to find those.", "Ein Ticket mit leerem Datum oder leerer Zahl trifft nie einen Vergleich. Mit `IS EMPTY` findest du sie."),
                    ],
                    vec![
                        t("`=` on a title finds nothing", "`=` bei einem Titel findet nichts"),
                        t("`=` needs the whole text to be equal. Use `~` (contains) or a free text search in quotes.", "`=` verlangt, dass der ganze Text gleich ist. Nimm `~` (enthält) oder eine Freitext-Suche in Anführungszeichen."),
                    ],
                    vec![
                        t("A different result than with `OR` and `AND` mixed", "Anderes Ergebnis bei gemischtem `OR` und `AND`"),
                        t("`AND` binds stronger than `OR`. Put parentheses around the part that belongs together.", "`AND` bindet stärker als `OR`. Setze Klammern um das, was zusammengehört."),
                    ],
                    vec![
                        t("Nothing for `sprint = activeSprint()`", "Nichts bei `sprint = activeSprint()`"),
                        t("The project has no active sprint right now (or does not use sprints).", "Das Projekt hat gerade keinen aktiven Sprint (oder nutzt keine Sprints)."),
                    ],
                ],
            ),
        ],
    }
}

pub(super) fn reference() -> Section {
    // Welche Operatoren erlaubt der jeweilige Feldtyp? – direkt aus `FieldType::allows`.
    let kinds: [(FieldType, FieldKind); 9] = [
        (FieldType::Text, FieldKind::Text),
        (FieldType::Number, FieldKind::Number),
        (FieldType::Date, FieldKind::Date),
        (FieldType::DateTime, FieldKind::DateTime),
        (FieldType::Bool, FieldKind::Bool),
        (FieldType::Option(Vec::new()), FieldKind::Option),
        (FieldType::Labels, FieldKind::Labels),
        (FieldType::TicketRef, FieldKind::TicketRef),
        (FieldType::Terminal, FieldKind::Terminal),
    ];
    let mut header = vec![t("Field type", "Feldtyp")];
    for op in OpKind::ALL {
        header.push(code_cell(op.symbol()));
    }
    let matrix: Vec<Vec<Text>> = kinds
        .iter()
        .map(|(ty, kind)| {
            let mut row = vec![Text::leak(kind.label(Lang::En).to_string(), kind.label(Lang::De).to_string())];
            for op in OpKind::ALL {
                row.push(same(if ty.allows(op) { "✓" } else { "–" }));
            }
            row
        })
        .collect();

    Section {
        id: "reference",
        title: t("Reference", "Referenz"),
        blocks: vec![
            p("**Operators**", "**Operatoren**"),
            table(
                &[t("Operator", "Operator"), t("Meaning", "Bedeutung"), t("Example", "Beispiel")],
                vec![
                    vec![same("`=`"), t("equals (ignores upper/lower case)", "ist gleich (Groß-/Kleinschreibung egal)"), same("`type = Defect`")],
                    vec![same("`!=`"), t("is different (also matches empty fields)", "ist ungleich (trifft auch leere Felder)"), same("`status != Done`")],
                    vec![same("`~`"), t("contains", "enthält"), same("`summary ~ \"login\"`")],
                    vec![same("`!~`"), t("does not contain (also matches empty fields)", "enthält nicht (trifft auch leere Felder)"), same("`summary !~ \"login\"`")],
                    vec![same("`<`  `>`  `<=`  `>=`"), t("smaller, greater, at most, at least (numbers, dates)", "kleiner, größer, höchstens, mindestens (Zahlen, Daten)"), same("`due < currentDate()`")],
                    vec![same("`IN (a, b)`"), t("equals one of the values", "ist einer der Werte"), same("`priority IN (High, Highest)`")],
                    vec![same("`NOT IN (a, b)`"), t("equals none of the values", "ist keiner der Werte"), same("`type NOT IN (Epic, Subtask)`")],
                    vec![same("`IS EMPTY`"), t("has no value (also `IS NULL`)", "hat keinen Wert (auch `IS NULL`)"), same("`parent IS EMPTY`")],
                    vec![same("`IS NOT EMPTY`"), t("has a value (also `IS NOT NULL`)", "hat einen Wert (auch `IS NOT NULL`)"), same("`terminal IS NOT EMPTY`")],
                    vec![same("`IS name`  `IS NOT name`"), t("a predicate, see below", "ein Prädikat, siehe unten"), same("`IS NOT blocked`")],
                    vec![same("`\"text\"`"), t("free text in the title and all text fields", "Freitext in Titel und allen Textfeldern"), same("`\"coupon\"`")],
                ],
            ),
            p("**Combining and sorting**", "**Verbinden und Sortieren**"),
            table(
                &[t("Keyword", "Schlüsselwort"), t("Meaning", "Bedeutung")],
                vec![
                    vec![same("`NOT`"), t("turns a condition around; binds strongest", "dreht eine Bedingung um; bindet am stärksten")],
                    vec![same("`AND`"), t("both conditions must be true; binds stronger than `OR`", "beide Bedingungen müssen zutreffen; bindet stärker als `OR`")],
                    vec![same("`OR`"), t("at least one condition must be true", "mindestens eine Bedingung muss zutreffen")],
                    vec![same("`( … )`"), t("groups conditions", "gruppiert Bedingungen")],
                    vec![same("`ORDER BY` / `SORT BY`"), t("sorts the result; once, at the end; several fields separated by commas", "sortiert das Ergebnis; einmal, am Ende; mehrere Felder durch Kommas getrennt")],
                    vec![same("`ASC` / `DESC`"), t("ascending (default) / descending, after a sort field", "aufsteigend (Standard) / absteigend, hinter einem Sortierfeld")],
                ],
            ),
            p("**Functions** (used as a value)", "**Funktionen** (als Wert benutzt)"),
            table(
                &[t("Function", "Funktion"), t("Result", "Ergebnis"), t("Fits fields of type", "Passt zu Feldern vom Typ"), t("Example", "Beispiel")],
                vec![
                    vec![same("`activeSprint()`"), t("name of the project's active sprint", "Name des aktiven Sprints des Projekts"), t("dropdown, text", "Auswahl, Text"), same("`sprint = activeSprint()`")],
                    vec![same("`currentDate()`"), t("today's date", "das heutige Datum"), t("date, date and time", "Datum, Datum mit Uhrzeit"), same("`due < currentDate()`")],
                    vec![same("`linkedTerminal()`"), t("the terminal the search was started from", "das Terminal, aus dem die Suche gestartet wurde"), t("terminal, text", "Terminal, Text"), same("`terminal = linkedTerminal()`")],
                ],
            ),
            p("**Predicates and sort keywords**", "**Prädikate und Sortier-Schlüsselwörter**"),
            table(
                &[t("Keyword", "Schlüsselwort"), t("Use", "Verwendung"), t("Meaning", "Bedeutung")],
                vec![
                    vec![same("`favorite`"), same("`IS favorite`  `IS NOT favorite`"), t("starred tickets", "Tickets mit Stern")],
                    vec![same("`blocked`"), same("`IS blocked`  `IS NOT blocked`"), t("tickets marked as blocked", "als blockiert markierte Tickets")],
                    vec![same("`favorites`"), same("`SORT BY favorites`"), t("starred tickets first", "Tickets mit Stern zuerst")],
                ],
            ),
            note(
                "You can define more predicates and sort keywords in the keyword editor; the built-in ones cannot be deleted.",
                "Weitere Prädikate und Sortier-Schlüsselwörter definierst du im Schlüsselwort-Editor; die eingebauten lassen sich nicht löschen.",
            ),
            p("**Which operators fit which field type**", "**Welche Operatoren zu welchem Feldtyp passen**"),
            Block::Table { header, rows: matrix },
            note(
                "Fields that can hold several values (labels, multiple choice): `=` and `IN` mean “contains”, `!=` and `NOT IN` “does not contain”. You cannot sort by such a field.",
                "Felder mit mehreren Werten (Labels, Mehrfachauswahl): `=` und `IN` heißen „enthält“, `!=` und `NOT IN` „enthält nicht“. Nach solchen Feldern lässt sich nicht sortieren.",
            ),
            p("**Values**", "**Werte**"),
            table(
                &[t("Kind", "Art"), t("Written as", "Schreibweise"), t("Examples", "Beispiele")],
                vec![
                    vec![t("Number", "Zahl"), t("digits, optional minus and decimal point", "Ziffern, optional Minus und Dezimalpunkt"), same("`5`  `-2`  `3.14`")],
                    vec![t("Date", "Datum"), t("`YYYY-MM-DD`", "`JJJJ-MM-TT`"), same("`2026-11-14`")],
                    vec![t("Date and time", "Datum mit Uhrzeit"), t("`YYYY-MM-DDTHH:MM`", "`JJJJ-MM-TTTHH:MM`"), same("`2026-11-14T10:30`")],
                    vec![t("Word", "Wort"), t("letters, digits, `_` `-` `.` `:` – no quotes needed", "Buchstaben, Ziffern, `_` `-` `.` `:` – keine Anführungszeichen nötig"), same("`Done`  `2.0.0`  `WEB-12`")],
                    vec![t("Text", "Text"), t("in double or single quotes; `\\\"`, `\\\\`, `\\n`, `\\t` are escapes", "in doppelten oder einfachen Anführungszeichen; `\\\"`, `\\\\`, `\\n`, `\\t` sind Escape-Folgen"), same("`\"In Progress\"`")],
                    vec![t("Empty", "Leer"), t("`EMPTY` or `NULL`, only with `=` or `!=`", "`EMPTY` oder `NULL`, nur mit `=` oder `!=`"), same("`due = EMPTY`")],
                    vec![t("Function", "Funktion"), t("name and parentheses", "Name und Klammern"), same("`currentDate()`")],
                ],
            ),
            p("**Reserved words**", "**Reservierte Wörter**"),
            p(
                "These words have a meaning in the language. If one of them is meant as a field name or a value, put it in quotes (`labels = \"order\"`). `BY`, `ASC` and `DESC` are only keywords inside `ORDER BY` and not reserved.",
                "Diese Wörter haben in der Sprache eine Bedeutung. Ist eines davon als Feldname oder Wert gemeint, setze es in Anführungszeichen (`labels = \"order\"`). `BY`, `ASC` und `DESC` sind nur innerhalb von `ORDER BY` Schlüsselwörter und nicht reserviert.",
            ),
            code("AND  OR  NOT  IN  IS  EMPTY  NULL  ORDER  SORT"),
            p("**Grammar**", "**Grammatik**"),
            code(GRAMMAR),
        ],
    }
}

/// Die Grammatik in Kurzform (Englisch und Deutsch gleich: Code).
pub(super) const GRAMMAR: &str = r#"query      = [ condition-list ] [ order-by ]
order-by   = ( "ORDER" | "SORT" ) "BY" order-term { "," order-term }
order-term = name [ "ASC" | "DESC" ]

condition-list = and-expr { "OR" and-expr }
and-expr       = not-expr { "AND" not-expr }
not-expr       = "NOT" not-expr | "(" condition-list ")" | condition

condition = name ( "=" | "!=" | "~" | "!~" | "<" | ">" | "<=" | ">=" ) value
          | name [ "NOT" ] "IN" "(" value { "," value } ")"
          | name "IS" [ "NOT" ] ( "EMPTY" | "NULL" )
          | "IS" [ "NOT" ] name
          | text

value = number | date | datetime | word | text | "EMPTY" | "NULL" | name "(" [ value { "," value } ] ")"
name  = word | text"#;
