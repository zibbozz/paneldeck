//! Syntaxbaum der Abfragesprache und Pretty-Printer (`Display`).
//!
//! Der Baum ist bewusst „positionsblind“ beim Vergleich: [`Loc`] vergleicht immer als gleich, damit
//! `parse(print(parse(x))) == parse(x)` gilt, obwohl sich die Byte-Positionen durch das Ausgeben ändern.
//! Wer Positionen prüfen will, liest sie über `span()` aus (dort wird normal verglichen).

use super::lexer::{classify_word, is_reserved_word, is_word_char};
use std::fmt;
use std::ops::Range;

/// Bereich im Eingabetext: Byte-Offset und Länge in Bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Span {
    pub start: usize,
    pub len: usize,
}

impl Span {
    pub fn new(start: usize, len: usize) -> Self {
        Self { start, len }
    }

    pub fn end(&self) -> usize {
        self.start + self.len
    }

    pub fn range(&self) -> Range<usize> {
        self.start..self.end()
    }

    /// Kleinster Bereich, der beide umfasst.
    pub fn union(self, other: Span) -> Span {
        let start = self.start.min(other.start);
        let end = self.end().max(other.end());
        Span { start, len: end - start }
    }
}

/// Position eines Baumknotens. Vergleicht immer als gleich (siehe Moduldoku).
#[derive(Clone, Copy, Debug, Default)]
pub struct Loc(pub Span);

impl PartialEq for Loc {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for Loc {}

/// Name eines Feldes, einer Funktion, eines Prädikats oder Sortier-Schlüsselworts (wie geschrieben, ohne Anführungszeichen).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Name {
    pub text: String,
    pub loc: Loc,
}

impl Name {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into(), loc: Loc::default() }
    }

    pub fn at(text: impl Into<String>, span: Span) -> Self {
        Self { text: text.into(), loc: Loc(span) }
    }

    pub fn span(&self) -> Span {
        self.loc.0
    }
}

/// Vergleichsoperator mit einem einzelnen Wert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Contains,
    NotContains,
    Lt,
    Gt,
    Le,
    Ge,
}

impl CmpOp {
    pub fn symbol(self) -> &'static str {
        match self {
            CmpOp::Eq => "=",
            CmpOp::Ne => "!=",
            CmpOp::Contains => "~",
            CmpOp::NotContains => "!~",
            CmpOp::Lt => "<",
            CmpOp::Gt => ">",
            CmpOp::Le => "<=",
            CmpOp::Ge => ">=",
        }
    }
}

/// Wert auf der rechten Seite eines Vergleichs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Value {
    pub kind: ValueKind,
    pub loc: Loc,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValueKind {
    /// Zahl, im Originaltext (`-5`, `3.14`)
    Number(String),
    /// Datum `JJJJ-MM-TT` (nur syntaktisch erkannt; ob es existiert, prüft der Validator)
    Date(String),
    /// Datum mit Uhrzeit `JJJJ-MM-TTTHH:MM[:SS]`
    DateTime(String),
    /// unquotiertes Wort (`Done`, `2.0.0`, `PAN-12`)
    Word(String),
    /// Text in Anführungszeichen (bereits entschlüsselt)
    Text(String),
    /// `NULL` / `EMPTY`
    Null,
    /// Funktionsaufruf `name(arg, …)`
    Func { name: Name, args: Vec<Value> },
}

impl Value {
    pub fn new(kind: ValueKind) -> Self {
        Self { kind, loc: Loc::default() }
    }

    pub fn span(&self) -> Span {
        self.loc.0
    }

    /// Der Wert als Text, soweit er ein Literal ist (Zahl, Datum, Wort, Text); sonst `None`.
    pub fn literal_text(&self) -> Option<&str> {
        match &self.kind {
            ValueKind::Number(s) | ValueKind::Date(s) | ValueKind::DateTime(s) | ValueKind::Word(s) | ValueKind::Text(s) => Some(s),
            ValueKind::Null | ValueKind::Func { .. } => None,
        }
    }
}

/// Bedingung bzw. logischer Ausdruck.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expr {
    /// `a AND b AND …` (mindestens zwei Operanden)
    And(Vec<Expr>),
    /// `a OR b OR …` (mindestens zwei Operanden)
    Or(Vec<Expr>),
    Not(Box<Expr>),
    /// `feld <op> wert`
    Compare { field: Name, op: CmpOp, op_loc: Loc, value: Value },
    /// `feld [NOT] IN (werte)`
    In { field: Name, negated: bool, op_loc: Loc, values: Vec<Value> },
    /// `feld IS [NOT] EMPTY`
    IsEmpty { field: Name, negated: bool, op_loc: Loc },
    /// `IS [NOT] name` (benanntes Prädikat ohne Vergleichswert)
    Predicate { name: Name, negated: bool },
    /// Freitext in Anführungszeichen ohne Feld: `"login bug"`
    Text(String, Loc),
}

impl Expr {
    /// Bereich des gesamten Ausdrucks (bei zusammengesetzten Ausdrücken von der ersten bis zur letzten Teilposition).
    pub fn span(&self) -> Span {
        match self {
            Expr::And(v) | Expr::Or(v) => v.iter().map(Expr::span).reduce(Span::union).unwrap_or_default(),
            Expr::Not(e) => e.span(),
            Expr::Compare { field, value, .. } => field.span().union(value.span()),
            Expr::In { field, op_loc, values, .. } => values.iter().fold(field.span().union(op_loc.0), |s, v| s.union(v.span())),
            Expr::IsEmpty { field, op_loc, .. } => field.span().union(op_loc.0),
            Expr::Predicate { name, .. } => name.span(),
            Expr::Text(_, loc) => loc.0,
        }
    }
}

/// Sortierrichtung.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

/// Ein Eintrag von `ORDER BY`: Feld oder Sortier-Schlüsselwort (das entscheidet erst der Validator).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderTerm {
    pub name: Name,
    /// `None` = nicht angegeben (Standardrichtung)
    pub dir: Option<SortDir>,
}

/// Eine vollständige Abfrage: optionaler Filter, dann optionale Sortierung.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Query {
    pub filter: Option<Expr>,
    pub order: Vec<OrderTerm>,
}

// ---------------------------------------------------------------------------------------------
// Pretty-Printer
// ---------------------------------------------------------------------------------------------

/// Text in `"…"` mit Escapes (`\\`, `\"`, `\n`, `\t`, `\r`).
pub fn quote_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Kann `s` ohne Anführungszeichen als einzelnes Wort geschrieben werden (nur Wortzeichen, kein reserviertes Wort)?
pub fn is_bare_safe(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_word_char) && !is_reserved_word(s)
}

/// `s` als Name (Feld, Prädikat, Sortier-Schlüsselwort): ohne Anführungszeichen, wenn möglich.
pub fn name_literal(s: &str) -> String {
    if is_bare_safe(s) { s.to_string() } else { quote_string(s) }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&name_literal(&self.text))
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ValueKind::Number(s) | ValueKind::Date(s) | ValueKind::DateTime(s) => f.write_str(s),
            // Ein von Hand gebautes Wort, das nicht als Wort gelesen würde, wird zu Text.
            ValueKind::Word(s) => {
                if is_bare_safe(s) && matches!(classify_word(s), ValueKind::Word(_)) {
                    f.write_str(s)
                } else {
                    f.write_str(&quote_string(s))
                }
            }
            ValueKind::Text(s) => f.write_str(&quote_string(s)),
            ValueKind::Null => f.write_str("NULL"),
            ValueKind::Func { name, args } => {
                write!(f, "{}(", name.text)?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{a}")?;
                }
                f.write_str(")")
            }
        }
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Expr::And(items) => write_joined(f, items, " AND ", |e| matches!(e, Expr::And(_) | Expr::Or(_))),
            // In einem OR bindet AND stärker; nur verschachteltes OR braucht Klammern (damit die Struktur erhalten bleibt).
            Expr::Or(items) => write_joined(f, items, " OR ", |e| matches!(e, Expr::Or(_))),
            Expr::Not(e) => {
                f.write_str("NOT ")?;
                if matches!(**e, Expr::And(_) | Expr::Or(_)) { write!(f, "({e})") } else { write!(f, "{e}") }
            }
            Expr::Compare { field, op, value, .. } => write!(f, "{field} {} {value}", op.symbol()),
            Expr::In { field, negated, values, .. } => {
                write!(f, "{field} {}IN (", if *negated { "NOT " } else { "" })?;
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{v}")?;
                }
                f.write_str(")")
            }
            Expr::IsEmpty { field, negated, .. } => write!(f, "{field} IS {}EMPTY", if *negated { "NOT " } else { "" }),
            Expr::Predicate { name, negated } => write!(f, "IS {}{name}", if *negated { "NOT " } else { "" }),
            Expr::Text(s, _) => f.write_str(&quote_string(s)),
        }
    }
}

fn write_joined(f: &mut fmt::Formatter<'_>, items: &[Expr], sep: &str, paren: fn(&Expr) -> bool) -> fmt::Result {
    for (i, e) in items.iter().enumerate() {
        if i > 0 {
            f.write_str(sep)?;
        }
        if paren(e) { write!(f, "({e})")? } else { write!(f, "{e}")? }
    }
    Ok(())
}

impl fmt::Display for OrderTerm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)?;
        match self.dir {
            Some(SortDir::Asc) => f.write_str(" ASC"),
            Some(SortDir::Desc) => f.write_str(" DESC"),
            None => Ok(()),
        }
    }
}

impl fmt::Display for Query {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(e) = &self.filter {
            write!(f, "{e}")?;
        }
        if !self.order.is_empty() {
            if self.filter.is_some() {
                f.write_str(" ")?;
            }
            f.write_str("ORDER BY ")?;
            for (i, t) in self.order.iter().enumerate() {
                if i > 0 {
                    f.write_str(", ")?;
                }
                write!(f, "{t}")?;
            }
        }
        Ok(())
    }
}
