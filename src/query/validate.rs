//! Validator: prüft eine geparste Abfrage gegen ein [`Schema`] (Felder, Datentypen, Funktionen, Prädikate).

use super::ast::*;
use super::lexer::is_number;
use super::schema::*;
use crate::state::Lang;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssueCode {
    UnknownField { name: String, suggestion: Option<String> },
    /// Operator passt nicht zum Datentyp des Feldes
    OperatorNotAllowed { field: String, op: OpKind, kind: FieldKind },
    /// Wert passt nicht zum Datentyp des Feldes
    ValueTypeMismatch { field: String, expected: FieldKind, found: String },
    /// Das Datum hat die richtige Form, existiert aber nicht (z. B. `2026-13-45`)
    InvalidDate { found: String },
    /// Wert steht nicht in der Werteliste des Auswahlfeldes
    OptionNotAllowed { field: String, value: String, suggestion: Option<String> },
    UnknownFunction { name: String, suggestion: Option<String> },
    FunctionArity { name: String, min: usize, max: usize, got: usize },
    /// Rückgabewert der Funktion passt nicht zum Feldtyp
    FunctionNotAllowed { name: String, field: String, kind: FieldKind },
    UnknownPredicate { name: String, suggestion: Option<String> },
    /// `ORDER BY` mit unbekanntem Feld/Schlüsselwort
    UnknownSortKey { name: String, suggestion: Option<String> },
    /// Nach Mehrfachwert-Feldern lässt sich nicht sortieren
    NotSortable { name: String },
    /// `NULL` mit einem anderen Operator als `=`/`!=`
    NullOperator { op: OpKind },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationIssue {
    pub span: Span,
    pub code: IssueCode,
}

impl ValidationIssue {
    /// Ein Verbesserungsvorschlag („meinten Sie …?“), falls vorhanden.
    pub fn suggestion(&self) -> Option<&str> {
        match &self.code {
            IssueCode::UnknownField { suggestion, .. }
            | IssueCode::OptionNotAllowed { suggestion, .. }
            | IssueCode::UnknownFunction { suggestion, .. }
            | IssueCode::UnknownPredicate { suggestion, .. }
            | IssueCode::UnknownSortKey { suggestion, .. } => suggestion.as_deref(),
            _ => None,
        }
    }

    pub fn message(&self, lang: Lang) -> String {
        let hint = |s: &Option<String>| -> String {
            match s {
                Some(s) => format!(" – {} {}?", lang.t("meinten Sie", "did you mean"), lang.q(s)),
                None => String::new(),
            }
        };
        match &self.code {
            IssueCode::UnknownField { name, suggestion } => format!("{} {}{}", lang.t("Unbekanntes Feld", "Unknown field"), lang.q(name), hint(suggestion)),
            IssueCode::OperatorNotAllowed { field, op, kind } => format!(
                "{} {} {} {} ({}: {})",
                lang.t("Der Operator", "The operator"),
                lang.q(op.symbol()),
                lang.t("passt nicht zum Feld", "does not fit the field"),
                lang.q(field),
                lang.t("Typ", "type"),
                kind.label(lang)
            ),
            IssueCode::ValueTypeMismatch { field, expected, found } => format!(
                "{} {} {} {} ({}: {})",
                lang.t("Der Wert", "The value"),
                lang.q(found),
                lang.t("passt nicht zum Feld", "does not fit the field"),
                lang.q(field),
                lang.t("erwartet", "expected"),
                expected.label(lang)
            ),
            IssueCode::InvalidDate { found } => format!(
                "{} {} {}",
                lang.q(found),
                lang.t("ist kein gültiges Datum", "is not a valid date"),
                lang.t("(Format JJJJ-MM-TT)", "(format YYYY-MM-DD)")
            ),
            IssueCode::OptionNotAllowed { field, value, suggestion } => format!(
                "{} {} {}{}",
                lang.q(value),
                lang.t("ist kein erlaubter Wert für das Feld", "is not an allowed value for the field"),
                lang.q(field),
                hint(suggestion)
            ),
            IssueCode::UnknownFunction { name, suggestion } => {
                format!("{} {}{}", lang.t("Unbekannte Funktion", "Unknown function"), lang.q(&format!("{name}()")), hint(suggestion))
            }
            IssueCode::FunctionArity { name, min, max, got } => {
                let expected = if min == max { min.to_string() } else { format!("{min}–{max}") };
                format!(
                    "{} {} {} {}, {} {}",
                    lang.t("Die Funktion", "The function"),
                    lang.q(&format!("{name}()")),
                    lang.t("erwartet", "expects"),
                    expected,
                    lang.t("übergeben:", "given:"),
                    got
                ) + lang.t(" Argument(e)", " argument(s)")
            }
            IssueCode::FunctionNotAllowed { name, field, kind } => format!(
                "{} {} {} {} ({}: {})",
                lang.t("Die Funktion", "The function"),
                lang.q(&format!("{name}()")),
                lang.t("passt nicht zum Feld", "does not fit the field"),
                lang.q(field),
                lang.t("Typ", "type"),
                kind.label(lang)
            ),
            IssueCode::UnknownPredicate { name, suggestion } => format!("{} {}{}", lang.t("Unbekanntes Prädikat", "Unknown predicate"), lang.q(name), hint(suggestion)),
            IssueCode::UnknownSortKey { name, suggestion } => {
                format!("{} {}{}", lang.t("Unbekanntes Sortierfeld", "Unknown sort field"), lang.q(name), hint(suggestion))
            }
            IssueCode::NotSortable { name } => {
                format!("{} {} {}", lang.t("Nach dem Feld", "The field"), lang.q(name), lang.t("lässt sich nicht sortieren (Mehrfachwert)", "cannot be sorted (multi-valued)"))
            }
            IssueCode::NullOperator { op } => format!(
                "NULL {} {} {}",
                lang.t("lässt sich nur mit = oder != vergleichen, nicht mit", "can only be compared with = or !=, not with"),
                lang.q(op.symbol()),
                lang.t("– nutze IS EMPTY", "– use IS EMPTY")
            ),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Hilfen
// ---------------------------------------------------------------------------------------------

/// Levenshtein-Abstand (über Zeichen, Groß-/Kleinschreibung ignoriert).
pub fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.to_lowercase().chars().collect();
    let b: Vec<char> = b.to_lowercase().chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur.push((prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Der ähnlichste Kandidat zu `name` (Levenshtein, bei Präfix-Treffern bevorzugt), falls einer nahe genug ist.
pub fn closest<'a>(name: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let n = name.chars().count();
    let limit = if n <= 3 { 1 } else if n <= 6 { 2 } else { 3 };
    let lower = name.to_lowercase();
    let mut best: Option<(usize, &str)> = None;
    for c in candidates {
        let mut d = levenshtein(name, c);
        if n >= 3 && c.to_lowercase().starts_with(&lower) {
            d = d.min(1);
        }
        if d > 0 && d <= limit && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, c));
        }
    }
    best.map(|(_, c)| c.to_string())
}

/// `JJJJ-MM-TT` → (Jahr, Monat, Tag), nur wenn das Datum existiert.
pub fn parse_date(s: &str) -> Option<(i32, u32, u32)> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        let t = &s[r];
        if t.bytes().all(|c| c.is_ascii_digit()) { t.parse().ok() } else { None }
    };
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let dim = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => return None,
    };
    (d >= 1 && d <= dim).then_some((y as i32, m, d))
}

/// `JJJJ-MM-TT` plus optional `T`/Leerzeichen und `HH:MM[:SS]`.
pub fn parse_datetime(s: &str) -> Option<()> {
    if s.len() == 10 {
        return parse_date(s).map(|_| ());
    }
    let sep = s.get(10..11)?;
    if sep != "T" && sep != " " {
        return None;
    }
    parse_date(s.get(..10)?)?;
    let parts: Vec<&str> = s.get(11..)?.split(':').collect();
    if !(parts.len() == 2 || parts.len() == 3) || parts.iter().any(|p| p.len() != 2 || !p.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let h: u32 = parts[0].parse().ok()?;
    let m: u32 = parts[1].parse().ok()?;
    let sec: u32 = parts.get(2).map_or(Some(0), |p| p.parse().ok())?;
    (h < 24 && m < 60 && sec < 60).then_some(())
}

/// Schlüssel wie `PAN-12`: Buchstabe, dann Buchstaben/Ziffern/`_`, ein `-`, dann Ziffern.
pub fn is_ticket_key(s: &str) -> bool {
    let Some((prefix, num)) = s.rsplit_once('-') else { return false };
    let mut chars = prefix.chars();
    chars.next().is_some_and(char::is_alphabetic)
        && chars.all(|c| c.is_alphanumeric() || c == '_')
        && !num.is_empty()
        && num.bytes().all(|b| b.is_ascii_digit())
}

fn is_bool_word(s: &str) -> bool {
    ["true", "false", "yes", "no", "1", "0"].iter().any(|w| eq_ci(w, s))
}

// ---------------------------------------------------------------------------------------------
// Prüfung
// ---------------------------------------------------------------------------------------------

pub fn validate(query: &Query, schema: &Schema) -> Vec<ValidationIssue> {
    let mut v = Validator { schema, issues: Vec::new() };
    if let Some(e) = &query.filter {
        v.expr(e);
    }
    for t in &query.order {
        v.order_term(t);
    }
    v.issues.sort_by_key(|i| i.span.start);
    v.issues
}

struct Validator<'a> {
    schema: &'a Schema,
    issues: Vec<ValidationIssue>,
}

impl<'a> Validator<'a> {
    fn issue(&mut self, span: Span, code: IssueCode) {
        self.issues.push(ValidationIssue { span, code });
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::And(items) | Expr::Or(items) => items.iter().for_each(|i| self.expr(i)),
            Expr::Not(inner) => self.expr(inner),
            Expr::Text(..) => {}
            Expr::Predicate { name, .. } => {
                if self.schema.predicate(&name.text).is_none() {
                    let suggestion = closest(&name.text, self.schema.predicates.iter().map(String::as_str));
                    self.issue(name.span(), IssueCode::UnknownPredicate { name: name.text.clone(), suggestion });
                }
            }
            Expr::Compare { field, op, op_loc, value } => {
                let Some(def) = self.field(field) else { return };
                let opk = OpKind::from(*op);
                if !def.ty.allows(opk) {
                    self.issue(op_loc.0, IssueCode::OperatorNotAllowed { field: def.name.clone(), op: opk, kind: def.ty.kind() });
                    return;
                }
                if matches!(value.kind, ValueKind::Null) && !matches!(op, CmpOp::Eq | CmpOp::Ne) {
                    self.issue(value.span(), IssueCode::NullOperator { op: opk });
                    return;
                }
                self.value(def, value);
            }
            Expr::In { field, negated, op_loc, values } => {
                let Some(def) = self.field(field) else { return };
                let opk = if *negated { OpKind::NotIn } else { OpKind::In };
                if !def.ty.allows(opk) {
                    self.issue(op_loc.0, IssueCode::OperatorNotAllowed { field: def.name.clone(), op: opk, kind: def.ty.kind() });
                    return;
                }
                for v in values {
                    self.value(def, v);
                }
            }
            Expr::IsEmpty { field, negated, op_loc } => {
                let Some(def) = self.field(field) else { return };
                let opk = if *negated { OpKind::IsNotEmpty } else { OpKind::IsEmpty };
                if !def.ty.allows(opk) {
                    self.issue(op_loc.0, IssueCode::OperatorNotAllowed { field: def.name.clone(), op: opk, kind: def.ty.kind() });
                }
            }
        }
    }

    /// Feld nachschlagen; meldet „unbekanntes Feld“ samt Vorschlag.
    fn field(&mut self, name: &Name) -> Option<&'a FieldDef> {
        let schema: &'a Schema = self.schema;
        match schema.field(&name.text) {
            Some(d) => Some(d),
            None => {
                let suggestion = closest(&name.text, schema.fields.iter().map(|f| f.name.as_str()));
                self.issue(name.span(), IssueCode::UnknownField { name: name.text.clone(), suggestion });
                None
            }
        }
    }

    fn order_term(&mut self, t: &OrderTerm) {
        if let Some(f) = self.schema.field(&t.name.text) {
            if f.multi {
                self.issue(t.name.span(), IssueCode::NotSortable { name: f.name.clone() });
            }
            return;
        }
        if self.schema.sort_keyword(&t.name.text).is_some() {
            return;
        }
        let candidates = self.schema.fields.iter().filter(|f| !f.multi).map(|f| f.name.as_str()).chain(self.schema.sort_keywords.iter().map(String::as_str));
        let suggestion = closest(&t.name.text, candidates);
        self.issue(t.name.span(), IssueCode::UnknownSortKey { name: t.name.text.clone(), suggestion });
    }

    /// Prüft einen Wert gegen den Typ des Feldes.
    fn value(&mut self, def: &FieldDef, v: &Value) {
        let schema = self.schema;
        let kind = def.ty.kind();
        match &v.kind {
            ValueKind::Null => {}
            ValueKind::Func { name, args } => {
                let Some(f) = schema.function(&name.text) else {
                    let suggestion = closest(&name.text, schema.functions.iter().map(|f| f.name.as_str()));
                    self.issue(name.span(), IssueCode::UnknownFunction { name: name.text.clone(), suggestion });
                    return;
                };
                if args.len() < f.min_args || args.len() > f.max_args {
                    self.issue(v.span(), IssueCode::FunctionArity { name: f.name.clone(), min: f.min_args, max: f.max_args, got: args.len() });
                } else if !f.accepts.contains(&kind) {
                    self.issue(v.span(), IssueCode::FunctionNotAllowed { name: f.name.clone(), field: def.name.clone(), kind });
                }
            }
            _ => {
                let text = v.literal_text().unwrap_or_default();
                let mismatch = FieldIssue::Mismatch;
                let verdict = match &def.ty {
                    FieldType::Text | FieldType::Labels | FieldType::Terminal => None,
                    FieldType::Number => (!is_number(text)).then_some(mismatch),
                    FieldType::Bool => (!is_bool_word(text)).then_some(mismatch),
                    FieldType::TicketRef => (!is_ticket_key(text)).then_some(mismatch),
                    FieldType::Date => {
                        if parse_date(text).is_some() {
                            None
                        } else if text.len() == 10 && looks_like_date(text) {
                            Some(FieldIssue::BadDate)
                        } else {
                            Some(mismatch)
                        }
                    }
                    FieldType::DateTime => {
                        if parse_datetime(text).is_some() {
                            None
                        } else if looks_like_date(text) {
                            Some(FieldIssue::BadDate)
                        } else {
                            Some(mismatch)
                        }
                    }
                    FieldType::Option(allowed) => {
                        if allowed.iter().any(|a| eq_ci(a, text)) {
                            None
                        } else {
                            let suggestion = closest(text, allowed.iter().map(String::as_str));
                            self.issue(v.span(), IssueCode::OptionNotAllowed { field: def.name.clone(), value: text.to_string(), suggestion });
                            return;
                        }
                    }
                };
                match verdict {
                    Some(FieldIssue::Mismatch) => {
                        self.issue(v.span(), IssueCode::ValueTypeMismatch { field: def.name.clone(), expected: kind, found: text.to_string() })
                    }
                    Some(FieldIssue::BadDate) => self.issue(v.span(), IssueCode::InvalidDate { found: text.to_string() }),
                    None => {}
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum FieldIssue {
    Mismatch,
    BadDate,
}

/// Beginnt der Text mit der Form `JJJJ-MM-TT` (Ziffern an den richtigen Stellen)?
fn looks_like_date(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 10 && (0..10).all(|i| if i == 4 || i == 7 { b[i] == b'-' } else { b[i].is_ascii_digit() })
}
