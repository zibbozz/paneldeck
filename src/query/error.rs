//! Syntaxfehler mit Position und zweisprachigem Text.

use super::ast::Span;
use super::lexer::is_reserved_word;
use crate::state::Lang;

/// Was an der Fehlerstelle erwartet wurde.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    /// Anfang einer Bedingung (Feld, `NOT`, `IS`, Klammer oder Text in Anführungszeichen)
    Condition,
    /// Operator nach einem Feldnamen
    Operator,
    Value,
    OpenParen,
    CloseParen,
    /// `,` oder `)` in einer IN-Liste bzw. Argumentliste
    ListSeparator,
    /// `IN` nach `feld NOT`
    In,
    /// `EMPTY`/`NULL` (nach `feld IS` bzw. `feld IS NOT`)
    EmptyKeyword,
    /// Prädikatname nach `IS`
    PredicateName,
    /// `BY` nach `ORDER`/`SORT`
    By,
    /// Feld oder Sortier-Schlüsselwort in `ORDER BY`
    OrderField,
    /// nach einer Bedingung: `AND`, `OR`, `ORDER BY` oder Ende
    LogicOrEnd,
    /// nach einem Sortier-Eintrag: `,`, `ASC`, `DESC` oder Ende
    OrderTail,
}

impl Expect {
    fn describe(self, lang: Lang) -> &'static str {
        match self {
            Expect::Condition => lang.t("eine Bedingung (Feld, NOT, IS oder Klammer)", "a condition (field, NOT, IS or parenthesis)"),
            Expect::Operator => lang.t(
                "einen Operator (=, !=, ~, !~, <, >, <=, >=, IN, NOT IN, IS EMPTY)",
                "an operator (=, !=, ~, !~, <, >, <=, >=, IN, NOT IN, IS EMPTY)",
            ),
            Expect::Value => lang.t("einen Wert", "a value"),
            Expect::OpenParen => lang.t("eine öffnende Klammer '('", "an opening parenthesis '('"),
            Expect::CloseParen => lang.t("eine schließende Klammer ')'", "a closing parenthesis ')'"),
            Expect::ListSeparator => lang.t("',' oder ')'", "',' or ')'"),
            Expect::In => "IN",
            Expect::EmptyKeyword => lang.t("EMPTY, NULL oder NOT", "EMPTY, NULL or NOT"),
            Expect::PredicateName => lang.t("einen Prädikatnamen (z. B. IS favorite)", "a predicate name (e.g. IS favorite)"),
            Expect::By => "BY",
            Expect::OrderField => lang.t("ein Feld oder Sortier-Schlüsselwort", "a field or sort keyword"),
            Expect::LogicOrEnd => lang.t("AND, OR, ORDER BY oder das Ende der Abfrage", "AND, OR, ORDER BY or the end of the query"),
            Expect::OrderTail => lang.t("',', ASC, DESC oder das Ende der Abfrage", "',', ASC, DESC or the end of the query"),
        }
    }
}

/// Was stattdessen gefunden wurde.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    End,
    /// der gefundene Text, wie er in der Eingabe steht
    Token(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ErrorCode {
    /// unbekanntes Zeichen
    UnexpectedChar(char),
    /// Anführungszeichen nicht geschlossen
    UnterminatedString,
    /// ungültige Escape-Sequenz (das Zeichen nach dem `\`)
    InvalidEscape(char),
    /// `(` ohne passende `)`; die Position ist die der öffnenden Klammer
    UnclosedParen,
    /// `)` ohne passende `(`
    UnmatchedParen,
    /// zweites `ORDER BY`/`SORT BY`
    DuplicateOrderBy,
    /// `IN ()`
    EmptyList,
    /// Klammern oder `NOT` zu tief verschachtelt
    TooDeep,
    Expected { what: Expect, found: Found },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub span: Span,
    pub code: ErrorCode,
}

impl ParseError {
    /// Fehlertext in der gewünschten Sprache.
    pub fn message(&self, lang: Lang) -> String {
        match &self.code {
            ErrorCode::UnexpectedChar(c) => format!("{} {}", lang.t("Unbekanntes Zeichen", "Unknown character"), lang.q(&c.to_string())),
            ErrorCode::UnterminatedString => lang
                .t("Anführungszeichen nicht geschlossen (schließendes Zeichen fehlt)", "Unterminated string (closing quote is missing)")
                .to_string(),
            ErrorCode::InvalidEscape(c) => {
                format!("{} {}", lang.t("Ungültige Escape-Sequenz", "Invalid escape sequence"), lang.q(&format!("\\{c}")))
            }
            ErrorCode::UnclosedParen => lang.t("Schließende Klammer ')' fehlt", "Missing closing parenthesis ')'").to_string(),
            ErrorCode::UnmatchedParen => lang.t("Schließende Klammer ')' ohne öffnende Klammer", "Closing parenthesis ')' without an opening one").to_string(),
            ErrorCode::DuplicateOrderBy => lang
                .t("ORDER BY bzw. SORT BY darf nur einmal vorkommen", "ORDER BY / SORT BY may only appear once")
                .to_string(),
            ErrorCode::EmptyList => lang.t("Die Liste in IN (…) darf nicht leer sein", "The list in IN (…) must not be empty").to_string(),
            ErrorCode::TooDeep => lang.t("Die Abfrage ist zu tief verschachtelt", "The query is nested too deeply").to_string(),
            ErrorCode::Expected { what, found } => {
                let what_text = what.describe(lang);
                match found {
                    Found::End if *what == Expect::Value => lang.t("Wert fehlt", "Missing value").to_string(),
                    Found::End => format!("{} {what_text}", lang.t("Unerwartetes Ende der Abfrage, erwartet:", "Unexpected end of query, expected:")),
                    Found::Token(t) => {
                        let mut s = format!(
                            "{} {what_text}, {} {}",
                            lang.t("Erwartet:", "Expected:"),
                            lang.t("gefunden:", "found:"),
                            lang.q(t)
                        );
                        let name_slot = matches!(what, Expect::Value | Expect::OrderField | Expect::PredicateName);
                        if name_slot && is_reserved_word(t) {
                            s.push_str(lang.t(
                                " (reserviertes Wort – in Anführungszeichen setzen, wenn es als Name oder Wert gemeint ist)",
                                " (reserved word – put it in quotes if it is meant as a name or value)",
                            ));
                        }
                        s
                    }
                }
            }
        }
    }
}
