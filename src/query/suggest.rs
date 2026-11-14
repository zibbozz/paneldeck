//! Autovervollständigung: entscheidet am Cursor, was als Nächstes erwartet wird, und liefert passende Vorschläge.
//!
//! Vorgehen: Die Eingabe wird fehlertolerant zerlegt; die Token links vom Cursor laufen durch einen kleinen Zustandsautomaten,
//! der die Grammatik spiegelt (Bedingung → Operator → Wert → AND/OR/ORDER BY …). Ein angefangenes Wort unter dem Cursor
//! ist der Präfix, nach dem gefiltert wird; es wird beim Einfügen komplett ersetzt.

use super::ast::{Span, is_bare_safe, name_literal};
use super::lexer::{Tok, Token, lex};
use super::parser::{Kw, is_reserved_kw, kw_of};
use super::schema::*;
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuggestionKind {
    Field,
    Operator,
    Value,
    /// `AND`, `OR`, `ORDER BY`, `NOT`, `IS`, `BY`, `ASC`, … sowie Satzzeichen
    Keyword,
    Function,
    Predicate,
    /// Sortier-Schlüsselwort (z. B. `favorites`)
    SortKey,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    /// Anzeigetext
    pub label: String,
    /// Text, der anstelle von `replace_range` eingefügt wird (Namen mit Leerzeichen sind bereits in Anführungszeichen)
    pub insert_text: String,
    /// Byte-Bereich der Eingabe, der ersetzt wird (leer = am Cursor einfügen)
    pub replace_range: Range<usize>,
    pub kind: SuggestionKind,
}

#[derive(Clone, Debug)]
enum St {
    /// Anfang einer Bedingung (Start, nach AND/OR/NOT/`(`)
    Cond,
    /// nach `IS` am Anfang einer Bedingung (Prädikat folgt)
    IsStart { negated: bool },
    /// nach einem Feldnamen (Feld, falls bekannt)
    Op(String),
    /// nach `feld NOT`
    FieldNot(String),
    /// nach `feld IS` / `feld IS NOT`
    FieldIs(String, bool),
    /// nach einem Vergleichsoperator
    Value(String),
    /// nach `IN`
    InOpen(String),
    /// in einer IN-Liste nach `(` oder `,`
    ListValue(String),
    /// in einer IN-Liste nach einem Wert
    ListAfter(String),
    FuncArgs,
    /// nach einer vollständigen Bedingung
    AfterCond,
    /// nach `ORDER`/`SORT`
    OrderBy,
    /// nach `BY` oder `,` in der Sortierung
    OrderTerm,
    OrderAfterTerm,
    OrderAfterDir,
    Unknown,
}

#[derive(Clone, Debug)]
enum Paren {
    Group,
    InList,
    /// Funktionsaufruf; Zustand, in den nach der `)` zurückgekehrt wird
    Func(Box<St>),
}

struct Machine {
    st: St,
    stack: Vec<Paren>,
    /// Das vorige Token war ein Wort an Wertposition (dann ist ein folgendes `(` ein Funktionsaufruf).
    last_value_word: bool,
}

impl Machine {
    fn new() -> Self {
        Self { st: St::Cond, stack: Vec::new(), last_value_word: false }
    }

    fn step(&mut self, t: &Token) {
        let value_word = std::mem::take(&mut self.last_value_word);
        let kw = match &t.tok {
            Tok::Word(w) => kw_of(w),
            _ => None,
        };
        let in_args = matches!(self.stack.last(), Some(Paren::InList | Paren::Func(_)));

        // AND/OR beginnen außerhalb von Listen/Argumenten immer eine neue Bedingung.
        if matches!(kw, Some(Kw::And | Kw::Or)) && !in_args {
            self.st = St::Cond;
            return;
        }
        match &t.tok {
            Tok::Invalid(_) => {
                self.st = St::Unknown;
                return;
            }
            Tok::RParen => {
                self.st = match self.stack.pop() {
                    Some(Paren::Group | Paren::InList) => St::AfterCond,
                    Some(Paren::Func(ret)) => *ret,
                    None => St::Unknown,
                };
                return;
            }
            Tok::LParen => {
                let cur = std::mem::replace(&mut self.st, St::Unknown);
                self.st = match cur {
                    St::AfterCond if value_word => {
                        self.stack.push(Paren::Func(Box::new(St::AfterCond)));
                        St::FuncArgs
                    }
                    St::ListAfter(f) if value_word => {
                        self.stack.push(Paren::Func(Box::new(St::ListAfter(f))));
                        St::FuncArgs
                    }
                    St::InOpen(f) => {
                        self.stack.push(Paren::InList);
                        St::ListValue(f)
                    }
                    St::Cond => {
                        self.stack.push(Paren::Group);
                        St::Cond
                    }
                    St::FuncArgs => {
                        self.stack.push(Paren::Func(Box::new(St::FuncArgs)));
                        St::FuncArgs
                    }
                    _ => {
                        self.stack.push(Paren::Group);
                        St::Unknown
                    }
                };
                return;
            }
            _ => {}
        }

        let cur = std::mem::replace(&mut self.st, St::Unknown);
        self.st = match (cur, &t.tok) {
            (St::Cond, Tok::Word(w)) => match kw {
                Some(Kw::Not) => St::Cond,
                Some(Kw::Is) => St::IsStart { negated: false },
                Some(Kw::Order | Kw::Sort) => St::OrderBy,
                Some(k) if is_reserved_kw(k) => St::Unknown,
                _ => St::Op(w.clone()),
            },
            (St::Cond, Tok::Quoted { text, .. }) => St::Op(text.clone()),
            (St::IsStart { negated }, Tok::Word(_)) => match kw {
                Some(Kw::Not) if !negated => St::IsStart { negated: true },
                Some(k) if is_reserved_kw(k) => St::Unknown,
                _ => St::AfterCond,
            },
            (St::IsStart { .. }, Tok::Quoted { .. }) => St::AfterCond,
            (St::Op(f), Tok::Op(_)) => St::Value(f),
            (St::Op(f), Tok::Word(_)) => match kw {
                Some(Kw::In) => St::InOpen(f),
                Some(Kw::Not) => St::FieldNot(f),
                Some(Kw::Is) => St::FieldIs(f, false),
                Some(Kw::Order | Kw::Sort) => St::OrderBy,
                _ => St::Unknown,
            },
            (St::FieldNot(f), Tok::Word(_)) if kw == Some(Kw::In) => St::InOpen(f),
            (St::FieldIs(f, neg), Tok::Word(_)) => match kw {
                Some(Kw::Not) if !neg => St::FieldIs(f, true),
                Some(Kw::Empty | Kw::Null) => St::AfterCond,
                _ => St::Unknown,
            },
            (St::Value(_), Tok::Word(_)) => match kw {
                Some(Kw::Null | Kw::Empty) => St::AfterCond,
                Some(k) if is_reserved_kw(k) => St::Unknown,
                _ => {
                    self.last_value_word = true;
                    St::AfterCond
                }
            },
            (St::Value(_), Tok::Quoted { .. }) => St::AfterCond,
            (St::ListValue(f), Tok::Word(_)) => match kw {
                Some(Kw::Null | Kw::Empty) => St::ListAfter(f),
                Some(k) if is_reserved_kw(k) => St::Unknown,
                _ => {
                    self.last_value_word = true;
                    St::ListAfter(f)
                }
            },
            (St::ListValue(f), Tok::Quoted { .. }) => St::ListAfter(f),
            (St::ListAfter(f), Tok::Comma) => St::ListValue(f),
            (St::AfterCond, Tok::Word(_)) => match kw {
                Some(Kw::Order | Kw::Sort) => St::OrderBy,
                _ => St::Unknown,
            },
            (St::OrderBy, Tok::Word(_)) if kw == Some(Kw::By) => St::OrderTerm,
            (St::OrderTerm, Tok::Word(_)) => match kw {
                Some(k) if is_reserved_kw(k) => St::Unknown,
                _ => St::OrderAfterTerm,
            },
            (St::OrderTerm, Tok::Quoted { .. }) => St::OrderAfterTerm,
            (St::OrderAfterTerm, Tok::Word(_)) if matches!(kw, Some(Kw::Asc | Kw::Desc)) => St::OrderAfterDir,
            (St::OrderAfterTerm | St::OrderAfterDir, Tok::Comma) => St::OrderTerm,
            (St::FuncArgs, Tok::Word(_) | Tok::Quoted { .. } | Tok::Comma) => St::FuncArgs,
            _ => St::Unknown,
        };
    }
}

/// Vorschlagskandidat vor dem Filtern.
struct Cand {
    label: String,
    insert: String,
    kind: SuggestionKind,
    /// Texte, gegen die der Präfix geprüft wird (klein geschrieben)
    matches: Vec<String>,
    /// Satzzeichen, das ohne Leerzeichen an das Vorige anschließt
    glue: bool,
}

impl Cand {
    fn new(label: &str, insert: String, kind: SuggestionKind) -> Self {
        Self { label: label.to_string(), insert, kind, matches: vec![label.to_lowercase()], glue: false }
    }

    fn word(label: &str, kind: SuggestionKind) -> Self {
        Self::new(label, label.to_string(), kind)
    }

    fn glued(label: &str, kind: SuggestionKind) -> Self {
        Self { glue: true, ..Self::new(label, label.to_string(), kind) }
    }
}

/// Wert so schreiben, dass er wieder als einzelner Wert gelesen wird (mit Anführungszeichen, wenn nötig).
fn value_literal(s: &str) -> String {
    if is_bare_safe(s) { s.to_string() } else { super::ast::quote_string(s) }
}

fn field_cands(schema: &Schema, only_sortable: bool) -> Vec<Cand> {
    let mut names: Vec<&FieldDef> = schema.fields.iter().filter(|f| !only_sortable || !f.multi).collect();
    names.sort_by_key(|f| f.name.to_lowercase());
    names
        .into_iter()
        .map(|f| Cand { matches: vec![f.name.to_lowercase()], ..Cand::new(&f.name, name_literal(&f.name), SuggestionKind::Field) })
        .collect()
}

fn function_cands(schema: &Schema, kind: Option<FieldKind>) -> Vec<Cand> {
    let mut fs: Vec<&FuncDef> = schema.functions.iter().filter(|f| f.min_args == 0 && kind.is_some_and(|k| f.accepts.contains(&k))).collect();
    fs.sort_by_key(|f| f.name.to_lowercase());
    fs.into_iter()
        .map(|f| Cand { matches: vec![f.name.to_lowercase()], ..Cand::new(&format!("{}()", f.name), format!("{}()", f.name), SuggestionKind::Function) })
        .collect()
}

fn value_cands(schema: &Schema, field: &str) -> Vec<Cand> {
    let Some(def) = schema.field(field) else { return Vec::new() };
    let mut out = Vec::new();
    match &def.ty {
        FieldType::Option(values) => {
            for v in values {
                out.push(Cand::new(v, value_literal(v), SuggestionKind::Value));
            }
        }
        FieldType::Bool => {
            out.push(Cand::word("true", SuggestionKind::Value));
            out.push(Cand::word("false", SuggestionKind::Value));
        }
        _ => {}
    }
    out.extend(function_cands(schema, Some(def.ty.kind())));
    out
}

fn op_cands(schema: &Schema, field: &str) -> Vec<Cand> {
    let def = schema.field(field);
    OpKind::ALL
        .iter()
        .filter(|op| def.is_none_or(|d| d.ty.allows(**op)))
        .map(|op| Cand::word(op.symbol(), SuggestionKind::Operator))
        .collect()
}

fn candidates(m: &Machine, schema: &Schema, at_start: bool, has_prefix: bool) -> Vec<Cand> {
    use SuggestionKind::*;
    match &m.st {
        St::Cond => {
            let mut v = field_cands(schema, false);
            v.push(Cand::word("NOT", Keyword));
            let mut preds: Vec<&String> = schema.predicates.iter().collect();
            preds.sort_by_key(|p| p.to_lowercase());
            for p in preds {
                v.push(Cand {
                    matches: vec![format!("is {}", p.to_lowercase()), p.to_lowercase()],
                    ..Cand::new(&format!("IS {p}"), format!("IS {}", name_literal(p)), Predicate)
                });
            }
            if at_start {
                v.push(Cand::word("ORDER BY", Keyword));
                if has_prefix {
                    v.push(Cand::word("SORT BY", Keyword));
                }
            }
            v
        }
        St::IsStart { negated } => {
            let mut v = Vec::new();
            if !negated {
                v.push(Cand::word("NOT", Keyword));
            }
            let mut preds: Vec<&String> = schema.predicates.iter().collect();
            preds.sort_by_key(|p| p.to_lowercase());
            for p in preds {
                v.push(Cand::new(p, name_literal(p), Predicate));
            }
            v
        }
        St::Op(f) => op_cands(schema, f),
        St::FieldNot(_) => vec![Cand::word("IN", Keyword)],
        St::FieldIs(_, neg) => {
            let mut v = Vec::new();
            if !neg {
                v.push(Cand::word("NOT", Keyword));
            }
            v.push(Cand::word("EMPTY", Keyword));
            v.push(Cand::word("NULL", Keyword));
            v
        }
        St::Value(f) | St::ListValue(f) => value_cands(schema, f),
        St::InOpen(_) => vec![Cand::word("(", Keyword)],
        St::ListAfter(_) => vec![Cand::glued(",", Keyword), Cand::glued(")", Keyword)],
        St::AfterCond => {
            let mut v = vec![Cand::word("AND", Keyword), Cand::word("OR", Keyword), Cand::word("ORDER BY", Keyword)];
            if has_prefix {
                v.push(Cand::word("SORT BY", Keyword));
            }
            if matches!(m.stack.last(), Some(Paren::Group)) {
                v.push(Cand::glued(")", Keyword));
            }
            v
        }
        St::OrderBy => vec![Cand::word("BY", Keyword)],
        St::OrderTerm => {
            let mut v = field_cands(schema, true);
            let mut keys: Vec<&String> = schema.sort_keywords.iter().collect();
            keys.sort_by_key(|k| k.to_lowercase());
            for k in keys {
                v.push(Cand::new(k, name_literal(k), SortKey));
            }
            v
        }
        St::OrderAfterTerm => vec![Cand::word("ASC", Keyword), Cand::word("DESC", Keyword), Cand::glued(",", Keyword)],
        St::OrderAfterDir => vec![Cand::glued(",", Keyword)],
        St::FuncArgs | St::Unknown => Vec::new(),
    }
}

/// Vorschläge für die Stelle `cursor` (Byte-Offset) in `input`.
///
/// Reihenfolge: nach Art des Vorschlags in der Reihenfolge, in der die Grammatik sie braucht (Felder, `NOT`, Prädikate; Operatoren in
/// fester logischer Reihenfolge; Werte in der Reihenfolge der Werteliste, danach Funktionen; Schlüsselwörter in fester Reihenfolge),
/// Feldnamen, Prädikate, Sortier-Schlüsselwörter und Funktionen alphabetisch (Groß-/Kleinschreibung ignoriert).
pub fn suggest(input: &str, cursor: usize, schema: &Schema) -> Vec<Suggestion> {
    let mut cursor = cursor.min(input.len());
    while !input.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let (toks, _) = lex(input);

    // Token links vom Cursor und das angefangene Token unter dem Cursor
    let mut before: Vec<&Token> = Vec::new();
    let mut partial: Option<&Token> = None;
    for t in &toks {
        let s = t.span;
        if s.start >= cursor {
            break;
        }
        let inside = match &t.tok {
            Tok::Word(_) => cursor <= s.end(),
            Tok::Quoted { closed, .. } => cursor < s.end() || (!closed && cursor == s.end()),
            _ => cursor < s.end(),
        };
        if inside {
            if matches!(t.tok, Tok::Word(_) | Tok::Quoted { .. }) {
                partial = Some(t);
            }
            break;
        }
        before.push(t);
    }

    let prefix: String = match partial {
        Some(t) => match &t.tok {
            Tok::Quoted { .. } => input[t.span.start + 1..cursor].to_string(),
            _ => input[t.span.start..cursor].to_string(),
        },
        None => String::new(),
    };
    let replace_range: Range<usize> = match partial {
        Some(t) => {
            let Span { start, len } = t.span;
            start..start + len
        }
        None => cursor..cursor,
    };

    let mut m = Machine::new();
    for t in &before {
        m.step(t);
    }

    let lower_prefix = prefix.to_lowercase();
    let prev_char = input[..replace_range.start].chars().next_back();
    let needs_space = partial.is_none() && prev_char.is_some_and(|c| !c.is_whitespace() && c != '(');

    let mut out: Vec<Suggestion> = Vec::new();
    for c in candidates(&m, schema, before.is_empty(), !prefix.is_empty()) {
        if !c.matches.iter().any(|t| t.starts_with(&lower_prefix)) {
            continue;
        }
        if out.iter().any(|s| s.label == c.label && s.kind == c.kind) {
            continue;
        }
        let lead = if needs_space && !c.glue { " " } else { "" };
        out.push(Suggestion { label: c.label, insert_text: format!("{lead}{}", c.insert), replace_range: replace_range.clone(), kind: c.kind });
    }
    out
}
