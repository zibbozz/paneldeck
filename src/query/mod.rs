//! Abfragesprache (JQL-artig): Parser, Prüfer und Autovervollständigung. Reine Logik – kein egui, keine Datenbank.
//!
//! Ablauf: [`parse`] macht aus dem Text einen [`Query`]-Baum (oder eine Liste von [`ParseError`]s mit Positionen),
//! [`validate`] prüft den Baum gegen ein vom Nutzer konfiguriertes [`Schema`] (Felder, Typen, Prädikate, Funktionen),
//! [`suggest`] liefert Vorschläge für die Stelle des Cursors. `Query::to_string()` gibt den Baum kanonisch wieder aus
//! (`parse(print(parse(x))) == parse(x)`). Die Übersetzung in SQL ist nicht Teil dieses Moduls.
//!
//! # Grammatik (EBNF)
//!
//! Schlüsselwörter sind englisch und Groß-/Kleinschreibung ist egal. `{ x }` = beliebig oft, `[ x ]` = optional.
//!
//! ```text
//! query       = [ or_expr ] [ order_by ] ;
//! order_by    = ( "ORDER" | "SORT" ) "BY" order_term { "," order_term } ;
//! order_term  = name [ "ASC" | "DESC" ] ;               (* Feld oder Sortier-Schlüsselwort, z. B. favorites *)
//!
//! or_expr     = and_expr { "OR" and_expr } ;
//! and_expr    = not_expr { "AND" not_expr } ;
//! not_expr    = "NOT" not_expr | primary ;               (* Vorrang: NOT vor AND vor OR *)
//! primary     = "(" or_expr ")" | condition ;
//!
//! condition   = name cmp_op value                        (* priority = High *)
//!             | name [ "NOT" ] "IN" "(" value { "," value } ")"
//!             | name "IS" [ "NOT" ] ( "EMPTY" | "NULL" )
//!             | "IS" [ "NOT" ] name                      (* Prädikat, z. B. IS favorite *)
//!             | string ;                                 (* Freitext, z. B. "login bug" *)
//! cmp_op      = "=" | "!=" | "~" | "!~" | "<" | ">" | "<=" | ">=" ;
//!
//! value       = number | date | datetime | word | string | "NULL" | "EMPTY" | call ;
//! call        = word "(" [ value { "," value } ] ")" ;     (* activeSprint(), currentDate(), linkedTerminal() *)
//! name        = word | string ;                          (* "Release version" mit Leerzeichen: in Anführungszeichen *)
//!
//! number      = [ "-" ] digit { digit } [ "." digit { digit } ] ;
//! date        = digit4 "-" digit2 "-" digit2 ;           (* 2026-11-14 *)
//! datetime    = date "T" digit2 ":" digit2 [ ":" digit2 ] ;
//! word        = wordchar { wordchar } ;                  (* Done, 2.0.0, PAN-12 – kein reserviertes Wort *)
//! wordchar    = Buchstabe | Ziffer | "_" | "-" | "." | ":" ;   (* Unicode: Umlaute sind erlaubt *)
//! string      = '"' { zeichen | escape } '"' | "'" { zeichen | escape } "'" ;
//! escape      = "\\" | "\"" | "\'" | "\n" | "\t" | "\r" ;
//! ```
//!
//! Reservierte Wörter (müssen als Name oder Wert in Anführungszeichen stehen): `AND OR NOT IN IS EMPTY NULL ORDER SORT`.
//! `BY`, `ASC`, `DESC` gelten nur innerhalb von `ORDER BY`. `SORT BY` ist ein Synonym für `ORDER BY`; `ORDER BY` darf nur einmal
//! vorkommen. Ein Text in Anführungszeichen ohne Operator dahinter ist ein Freitext, mit Operator ein Feldname.
//!
//! Beispiel: `type = Epic AND "Release version" = 2.0.0 AND status IN (Done, "Development finished") ORDER BY priority`
//!
//! # Positionen
//!
//! Alle Positionen sind Byte-Offsets in die Eingabe samt Länge in Bytes ([`Span`]); sie liegen immer auf Zeichengrenzen. Fehler am
//! Ende der Eingabe haben die Länge 0. Beim Vergleich zweier Bäume (`==`) werden Positionen ignoriert.

// Das Modul wird erst von der Suchleiste und der SQL-Übersetzung benutzt; bis dahin ist vieles „ungenutzt“.
#![allow(dead_code, unused_imports)]

mod ast;
mod error;
mod lexer;
mod parser;
mod schema;
mod suggest;
mod validate;

#[cfg(test)]
mod fuzz_tests;
#[cfg(test)]
mod tests;

pub use ast::{CmpOp, Expr, Loc, Name, OrderTerm, Query, SortDir, Span, Value, ValueKind, name_literal, quote_string};
pub use error::{ErrorCode, Expect, Found, ParseError};
pub use parser::parse;
pub use schema::{FieldDef, FieldKind, FieldType, FuncDef, OpKind, Schema, builtin_functions};
pub use suggest::{Suggestion, SuggestionKind, suggest};
pub use validate::{IssueCode, ValidationIssue, closest, levenshtein, validate};
