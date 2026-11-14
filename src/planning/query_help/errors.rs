//! Typische Fehler beim Schreiben von Abfragen: falsche Eingabe, richtige Eingabe und Erklärung. Die Meldungen in der Hilfe sind
//! NICHT von Hand geschrieben, sondern kommen von Parser und Validator selbst (gegen das Schema der Beispieldaten) – so stimmt der
//! Text der Hilfe immer mit dem überein, was die Suche wirklich anzeigt. Die Prüfungen stellen sicher, dass jede falsche Eingabe
//! wirklich einen Fehler liefert und jede korrigierte keinen.

use super::Text;
use super::dataset::shop;
use crate::query::{Schema, parse, validate};
use crate::state::Lang;

#[derive(Clone, Debug, PartialEq)]
pub struct ErrorCase {
    pub id: &'static str,
    /// So wird es oft falsch geschrieben
    pub bad: &'static str,
    /// So geht es
    pub good: &'static str,
    /// Warum es nicht geht und was man tut
    pub cause: Text,
}

pub fn error_cases() -> Vec<ErrorCase> {
    let c = |id, bad, good, cause| ErrorCase { id, bad, good, cause };
    vec![
        c(
            "space-in-field",
            "Release version = 2.0.0",
            "\"Release version\" = 2.0.0",
            Text::new(
                "A field name with a space must be put in quotes, otherwise the search reads `Release` as the field and `version` as the operator.",
                "Ein Feldname mit Leerzeichen muss in Anführungszeichen stehen, sonst liest die Suche `Release` als Feld und `version` als Operator.",
            ),
        ),
        c(
            "space-in-value",
            "status = In Progress",
            "status = \"In Progress\"",
            Text::new(
                "A value with a space needs quotes too. Without them the value ends after `In` – and `In` is a reserved word.",
                "Auch ein Wert mit Leerzeichen braucht Anführungszeichen. Ohne sie endet der Wert nach `In` – und `In` ist ein reserviertes Wort.",
            ),
        ),
        c(
            "reserved-word",
            "labels = order",
            "labels = \"order\"",
            Text::new(
                "`AND OR NOT IN IS EMPTY NULL ORDER SORT` are reserved. If one of them is meant as a name or value, put it in quotes.",
                "`AND OR NOT IN IS EMPTY NULL ORDER SORT` sind reserviert. Ist eines davon als Name oder Wert gemeint, setze es in Anführungszeichen.",
            ),
        ),
        c(
            "double-equals",
            "type == Defect",
            "type = Defect",
            Text::new("Comparing needs a single `=`.", "Zum Vergleichen genügt ein einzelnes `=`."),
        ),
        c(
            "missing-operator",
            "status Done",
            "status = Done",
            Text::new("After the field name an operator is missing (`=`, `!=`, `IN`, `IS EMPTY`, …).", "Nach dem Feldnamen fehlt ein Operator (`=`, `!=`, `IN`, `IS EMPTY`, …)."),
        ),
        c(
            "dangling-and",
            "type = Defect AND",
            "type = Defect",
            Text::new("`AND` and `OR` need a condition on both sides. Remove the last `AND` or add the missing condition.", "`AND` und `OR` brauchen auf beiden Seiten eine Bedingung. Entferne das letzte `AND` oder ergänze die fehlende Bedingung."),
        ),
        c(
            "unclosed-paren",
            "(type = Defect AND priority = High",
            "(type = Defect AND priority = High)",
            Text::new("Every `(` needs a matching `)`.", "Zu jeder `(` gehört eine `)`."),
        ),
        c(
            "unclosed-quote",
            "summary ~ \"login",
            "summary ~ \"login\"",
            Text::new("A quote is not closed. Text in quotes needs the closing `\"` (or `'`) as well.", "Ein Anführungszeichen ist nicht geschlossen. Text in Anführungszeichen braucht auch das schließende `\"` (oder `'`)."),
        ),
        c(
            "empty-list",
            "status IN ()",
            "status IN (Done, \"In Progress\")",
            Text::new("A list after `IN` needs at least one value, separated by commas.", "Die Liste nach `IN` braucht mindestens einen Wert, mehrere trennst du mit Kommas."),
        ),
        c(
            "unknown-character",
            "type = Defect;",
            "type = Defect",
            Text::new("There is no `;` (or other special character) in the language. Remove it.", "Ein `;` (oder ein anderes Sonderzeichen) gibt es in der Sprache nicht. Entferne es."),
        ),
        c(
            "double-order-by",
            "type = Defect ORDER BY priority ORDER BY due",
            "type = Defect ORDER BY priority, due",
            Text::new("`ORDER BY` (or `SORT BY`) may appear only once. List several sort fields after it, separated by commas.", "`ORDER BY` (oder `SORT BY`) darf nur einmal vorkommen. Mehrere Sortierfelder stehen dahinter, durch Kommas getrennt."),
        ),
        c(
            "unknown-field",
            "staus = Done",
            "status = Done",
            Text::new("The field does not exist (typo?). The message suggests the closest field name.", "Das Feld gibt es nicht (Tippfehler?). Die Meldung schlägt den ähnlichsten Feldnamen vor."),
        ),
        c(
            "wrong-operator",
            "priority ~ High",
            "priority = High",
            Text::new("`~` (contains) only works on text and labels. Dropdown fields use `=`, `!=`, `IN`, `NOT IN`.", "`~` (enthält) funktioniert nur bei Text und Labels. Auswahlfelder nutzen `=`, `!=`, `IN`, `NOT IN`."),
        ),
        c(
            "unknown-option",
            "status = Dne",
            "status = Done",
            Text::new("This value is not in the field's list of values. The message suggests the closest one.", "Diesen Wert gibt es in der Werteliste des Feldes nicht. Die Meldung schlägt den ähnlichsten vor."),
        ),
        c(
            "invalid-date",
            "due < 2026-13-45",
            "due < 2026-12-31",
            Text::new("Dates are written `YYYY-MM-DD` and must exist.", "Daten werden `JJJJ-MM-TT` geschrieben und müssen existieren."),
        ),
        c(
            "wrong-value-type",
            "\"Story points\" > many",
            "\"Story points\" > 5",
            Text::new("A number field wants a number as the value.", "Ein Zahlenfeld erwartet eine Zahl als Wert."),
        ),
        c(
            "unknown-function",
            "due < currentdat()",
            "due < currentDate()",
            Text::new("This function does not exist. Available: `activeSprint()`, `currentDate()`, `linkedTerminal()`.", "Diese Funktion gibt es nicht. Verfügbar sind `activeSprint()`, `currentDate()` und `linkedTerminal()`."),
        ),
        c(
            "wrong-function",
            "priority = currentDate()",
            "due = currentDate()",
            Text::new("The function returns a type that does not fit the field: `currentDate()` is a date, so it belongs to date fields.", "Die Funktion liefert einen Typ, der nicht zum Feld passt: `currentDate()` ist ein Datum und gehört zu Datumsfeldern."),
        ),
        c(
            "null-operator",
            "due < EMPTY",
            "due IS EMPTY",
            Text::new("`EMPTY`/`NULL` can only be compared with `=` or `!=`. Better use `IS EMPTY` / `IS NOT EMPTY`.", "`EMPTY`/`NULL` lässt sich nur mit `=` oder `!=` vergleichen. Besser: `IS EMPTY` / `IS NOT EMPTY`."),
        ),
        c(
            "unknown-predicate",
            "IS favourite",
            "IS favorite",
            Text::new("There is no such predicate (spelling?). The built-in ones are `favorite` and `blocked`.", "Dieses Prädikat gibt es nicht (Schreibweise?). Eingebaut sind `favorite` und `blocked`."),
        ),
        c(
            "unknown-sort-key",
            "type = Defect ORDER BY prio",
            "type = Defect ORDER BY priority",
            Text::new("`ORDER BY` accepts fields and sort keywords like `favorites`. The message suggests the closest name.", "`ORDER BY` kennt Felder und Sortier-Schlüsselwörter wie `favorites`. Die Meldung schlägt den ähnlichsten Namen vor."),
        ),
        c(
            "unsortable",
            "type = Defect ORDER BY labels",
            "type = Defect ORDER BY priority",
            Text::new("A field that can hold several values (like labels) cannot be sorted. Sort by a single-valued field.", "Nach einem Feld mit mehreren Werten (wie Labels) lässt sich nicht sortieren. Sortiere nach einem Feld mit einem Wert."),
        ),
    ]
}

/// Die erste Meldung von Parser oder Validator (Syntaxfehler zuerst), `None` wenn die Abfrage in Ordnung ist.
pub fn first_message(jql: &str, schema: &Schema, lang: Lang) -> Option<String> {
    match parse(jql) {
        Err(errors) => errors.first().map(|e| e.message(lang)),
        Ok(q) => validate(&q, schema).first().map(|i| i.message(lang)),
    }
}

/// Das Schema der gemeinsamen Beispieldaten (für die Fehlersuche).
pub fn help_schema() -> Schema {
    shop().schema()
}
