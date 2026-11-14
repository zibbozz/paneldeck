//! Übersetzung einer geprüften Abfrage ([`Query`]) in SQL.
//!
//! # Sicherheit
//!
//! Das SQL enthält nur feste Textbausteine dieser Datei. Jeder Wert (Suchtext, Zahl, Datum, Kennung, Feld-Kennung) geht als
//! nummerierter Parameter (`?N`) hinein, auch Feld-Kennungen und Positionen. Feld- und Tabellennamen kommen nie aus der Abfrage:
//! Feldnamen der Abfrage werden über das [`SearchSchema`] auf Kennungen abgebildet. Eine Abfrage ist nur über [`compile`] zu
//! übersetzen, und das prüft sie vorher mit dem Validator.
//!
//! # Semantik (wie in der Hilfe festgelegt, siehe `query_help::eval`)
//!
//! Jede Bedingung wird zuerst als *positive* Aussage gebaut, die nie `NULL` ist (`=`, `~`, `<`, `>`, `<=`, `>=`, `IN`); die
//! verneinten Operatoren sind `NOT (positiv)`. Darum treffen `!=`, `!~` und `NOT IN` auch Tickets mit leerem Feld, die positiven
//! nie. Bei Mehrfachwerten (Labels, Mehrfachauswahl) heißt `=` „enthält“. Text wird ohne Beachtung der Schreibweise verglichen
//! (ASCII über `LIKE`; enthält der Suchtext andere Buchstaben, über `GLOB` mit Zeichenklassen, damit auch Umlaute passen).
//!
//! # Grenzen
//!
//! Höchstens [`MAX_NODES`] Bedingungen, [`MAX_DEPTH`] Ebenen und [`MAX_SQL_LEN`] Zeichen SQL; sonst `TooComplex`/`TooDeep`
//! (statt eines Absturzes oder eines SQLite-Fehlers).

use super::error::{SRes, SearchError};
use super::pending;
use super::schema::{Binding, Class, OptionSet, Pred, SearchSchema, SortKw, Store, ValField, Vals, CHANGED_BY};
use super::{Ctx, Scope};
use crate::planning::fields::{Field, FieldKind};
use crate::planning::tickets::parse_key;
use crate::planning::views::keywords::{Cond, SortDir as KwDir};
use crate::planning::workflow::ConditionOp;
use crate::query::{validate, CmpOp, Expr, Query, SortDir, Value, ValueKind};
use chrono::{NaiveDate, NaiveTime};
use rusqlite::types::Value as Sql;
use std::collections::HashMap;

/// Höchste Zahl von Bedingungen (Knoten des Ausdrucks).
pub const MAX_NODES: usize = 300;
/// Tiefste Verschachtelung (Klammern, `NOT`).
pub const MAX_DEPTH: usize = 100;
/// Längstes erzeugtes SQL in Zeichen.
pub const MAX_SQL_LEN: usize = 400_000;

/// Das Ergebnis der Übersetzung. Die Platzhalter sind nummeriert (`?1` …); `params` gehört zu `where_sql` **und** `order_sql`,
/// `where_params` ist die Zahl der Parameter, die schon `where_sql` braucht (für Zählabfragen ohne Sortierung).
#[derive(Debug, Clone)]
pub struct Compiled {
    /// Vollständige Abfrage nach den Kennungen: `SELECT t.id … WHERE … ORDER BY …` (ohne Seitenbegrenzung).
    pub sql: String,
    /// Parameter von [`Compiled::sql`] in der Reihenfolge der Nummern.
    pub params: Vec<Sql>,
    /// Bedingung (ohne `WHERE`), `1` ohne Bedingung.
    pub where_sql: String,
    /// Sortierung (ohne `ORDER BY`), endet immer mit der Schlüsselreihenfolge.
    pub order_sql: String,
    pub where_params: usize,
}

/// `FROM`-Teil aller Ticket-Abfragen (`t` = Ticket, `p` = Projekt).
pub(super) const FROM: &str = "FROM tickets t JOIN projects p ON p.id = t.project_id";

impl Compiled {
    /// `SELECT <cols> FROM … WHERE … ORDER BY …` mit den Parametern.
    pub(super) fn select(&self, cols: &str) -> String {
        format!("SELECT {cols} {FROM} WHERE {} ORDER BY {}", self.where_sql, self.order_sql)
    }

    /// `SELECT COUNT(*)` und seine Parameter.
    pub(super) fn count(&self) -> (String, &[Sql]) {
        (format!("SELECT COUNT(*) {FROM} WHERE {}", self.where_sql), &self.params[..self.where_params.min(self.params.len())])
    }
}

/// Übersetzt die Abfrage. Prüft sie zuerst gegen das Schema (Fehler: `SearchError::Invalid`).
pub fn compile(q: &Query, schema: &SearchSchema, ctx: &Ctx) -> SRes<Compiled> {
    let issues = validate(q, &schema.schema);
    if !issues.is_empty() {
        return Err(SearchError::Invalid { input: q.to_string(), issues });
    }
    let mut cx = Cx::new(schema, ctx);
    let mut parts: Vec<String> = Vec::new();
    if let Scope::Project(id) = schema.scope {
        let p = cx.int(id);
        parts.push(format!("t.project_id = {p}"));
    }
    if let Some(e) = &q.filter {
        parts.push(format!("({})", cx.expr(e, 0)?));
    }
    let where_sql = if parts.is_empty() { "1".to_string() } else { parts.join(" AND ") };
    let where_params = cx.params.len();
    let order_sql = cx.order(q)?;
    let sql = format!("SELECT t.id {FROM} WHERE {where_sql} ORDER BY {order_sql}");
    if sql.len() > MAX_SQL_LEN {
        return Err(SearchError::TooComplex { max: MAX_NODES });
    }
    Ok(Compiled { sql, params: cx.params, where_sql, order_sql, where_params })
}

/// Die Zustände der Übersetzung: Parameter und Zähler.
pub(crate) struct Cx<'a> {
    pub(super) s: &'a SearchSchema,
    pub(super) ctx: &'a Ctx,
    pub(crate) params: Vec<Sql>,
    nodes: usize,
    ids_cache: HashMap<Vec<i64>, String>,
}

/// Betriebsart eines Textvergleichs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Eq,
    Contains,
}

/// Ein Sortier-Ausdruck.
struct OExpr {
    sql: String,
    nocase: bool,
    nullable: bool,
}

/// Verbindet die Teile ausgewogen (`((a OR b) OR (c OR d))`), damit der Ausdrucksbaum in SQLite flach bleibt (Grenze: Tiefe 1000).
fn balanced(mut parts: Vec<String>, sep: &str) -> String {
    match parts.len() {
        0 => String::new(),
        1 => parts.pop().unwrap_or_default(),
        n => {
            let right = parts.split_off(n / 2);
            format!("({} {sep} {})", balanced(parts, sep), balanced(right, sep))
        }
    }
}

fn or_join(parts: Vec<String>) -> String {
    if parts.is_empty() { "0".to_string() } else { balanced(parts, "OR") }
}

fn not(s: String) -> String {
    format!("NOT ({s})")
}

fn internal<T>(m: &str) -> SRes<T> {
    Err(SearchError::Internal(m.to_string()))
}

/// Datum `JJJJ-MM-TT` (nur der Tag, auch aus einem längeren Text).
fn norm_date(s: &str) -> Option<String> {
    let d = s.trim().get(..10)?;
    NaiveDate::parse_from_str(d, "%Y-%m-%d").ok().map(|d| d.format("%Y-%m-%d").to_string())
}

/// `JJJJ-MM-TTTHH:MM:SS` (ein reines Datum gilt als 00:00:00).
fn norm_datetime(s: &str) -> Option<String> {
    let s = s.trim();
    let (d, t) = match s.split_once(['T', ' ']) {
        Some((d, t)) => (d, t),
        None => (s, ""),
    };
    let date = NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()?;
    let mut nums: Vec<u32> = Vec::new();
    if !t.is_empty() {
        for part in t.split(':') {
            if part.len() != 2 || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            nums.push(part.parse().ok()?);
        }
    }
    if nums.len() > 3 {
        return None;
    }
    nums.resize(3, 0);
    NaiveTime::from_hms_opt(nums[0], nums[1], nums[2])?;
    Some(format!("{}T{:02}:{:02}:{:02}", date.format("%Y-%m-%d"), nums[0], nums[1], nums[2]))
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.to_lowercase().as_str() {
        "true" | "yes" | "1" => Some(true),
        "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

/// Muster für `LIKE`: `%`, `_`, `\` werden maskiert.
fn like_pattern(needle: &str, mode: Mode) -> String {
    let mut out = String::with_capacity(needle.len() + 2);
    if mode == Mode::Contains {
        out.push('%');
    }
    for c in needle.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    if mode == Mode::Contains {
        out.push('%');
    }
    out
}

/// Muster für `GLOB` ohne Beachtung der Schreibweise: jeder Buchstabe wird zur Zeichenklasse (`[uU]`), Sonderzeichen werden maskiert.
fn glob_pattern(needle: &str, mode: Mode) -> String {
    let mut out = String::with_capacity(needle.len() * 4 + 2);
    if mode == Mode::Contains {
        out.push('*');
    }
    for c in needle.chars() {
        match c {
            '*' | '?' | '[' => {
                out.push('[');
                out.push(c);
                out.push(']');
            }
            ']' => out.push_str("[]]"),
            c => {
                let (mut lo, mut up) = (c.to_lowercase(), c.to_uppercase());
                match (lo.next(), lo.next(), up.next(), up.next()) {
                    (Some(l), None, Some(u), None) if l != u => {
                        out.push('[');
                        out.push(l);
                        out.push(u);
                        out.push(']');
                    }
                    _ => out.push(c),
                }
            }
        }
    }
    if mode == Mode::Contains {
        out.push('*');
    }
    out
}

impl<'a> Cx<'a> {
    pub(crate) fn new(s: &'a SearchSchema, ctx: &'a Ctx) -> Self {
        Cx { s, ctx, params: Vec::new(), nodes: 0, ids_cache: HashMap::new() }
    }

    /// Setzt auf bereits vorhandenen Parametern auf (für Zusatzbedingungen am Board).
    pub(crate) fn continuing(s: &'a SearchSchema, ctx: &'a Ctx, params: Vec<Sql>) -> Self {
        Cx { s, ctx, params, nodes: 0, ids_cache: HashMap::new() }
    }

    /// Neuer Parameter; liefert den Platzhalter.
    pub(crate) fn p(&mut self, v: Sql) -> String {
        self.params.push(v);
        format!("?{}", self.params.len())
    }

    pub(crate) fn int(&mut self, i: i64) -> String {
        self.p(Sql::Integer(i))
    }

    pub(crate) fn text(&mut self, s: &str) -> String {
        self.p(Sql::Text(s.to_string()))
    }

    fn real(&mut self, f: f64) -> String {
        self.p(Sql::Real(f))
    }

    /// `col = ?n` bzw. `col IN (?a, ?b)`; ohne Kennungen `0`.
    pub(crate) fn ids_clause(&mut self, col: &str, ids: &[i64]) -> String {
        match ids.len() {
            0 => "0".to_string(),
            1 => {
                let p = self.int(ids[0]);
                format!("{col} = {p}")
            }
            _ => {
                let list = match self.ids_cache.get(ids) {
                    Some(l) => l.clone(),
                    None => {
                        let l = ids.iter().map(|i| self.int(*i)).collect::<Vec<_>>().join(", ");
                        self.ids_cache.insert(ids.to_vec(), l.clone());
                        l
                    }
                };
                format!("{col} IN ({list})")
            }
        }
    }

    // ------------------------------------------------------------------ Ausdruck

    fn expr(&mut self, e: &Expr, depth: usize) -> SRes<String> {
        if depth > MAX_DEPTH {
            return Err(SearchError::TooDeep { max: MAX_DEPTH });
        }
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(SearchError::TooComplex { max: MAX_NODES });
        }
        let s = self.s;
        Ok(match e {
            Expr::And(v) | Expr::Or(v) => {
                let sep = if matches!(e, Expr::And(_)) { "AND" } else { "OR" };
                let mut parts = Vec::with_capacity(v.len());
                for x in v {
                    parts.push(self.expr(x, depth + 1)?);
                }
                balanced(parts, sep)
            }
            Expr::Not(x) => not(self.expr(x, depth + 1)?),
            Expr::Text(text, _) => self.free_text(text),
            Expr::Predicate { name, negated } => {
                let sql = match s.pred(&name.text) {
                    Some(Pred::Blocked) => "t.flagged = 1".to_string(),
                    Some(Pred::Favorite) => "t.starred = 1".to_string(),
                    Some(Pred::Custom(c)) => self.cond(c, 0)?,
                    None => return internal("Prädikat ohne Definition"),
                };
                if *negated { not(sql) } else { sql }
            }
            Expr::IsEmpty { field, negated, .. } => {
                let b = self.binding(&field.text)?;
                let sql = self.is_empty(b)?;
                if *negated { not(sql) } else { sql }
            }
            Expr::Compare { field, op, value, .. } => {
                let b = self.binding(&field.text)?;
                self.compare(b, *op, value)?
            }
            Expr::In { field, negated, values, .. } => {
                let b = self.binding(&field.text)?;
                let mut vals: Vec<String> = Vec::new();
                let mut with_null = false;
                for v in values {
                    if matches!(v.kind, ValueKind::Null) {
                        with_null = true;
                    } else {
                        vals.extend(self.value_texts(v)?);
                    }
                }
                let mut parts = vec![self.pos(b, CmpOp::Eq, &vals)?];
                if with_null {
                    parts.push(self.is_empty(b)?);
                }
                let sql = or_join(parts);
                if *negated { not(sql) } else { sql }
            }
        })
    }

    fn binding(&self, name: &str) -> SRes<&'a Binding> {
        let s: &'a SearchSchema = self.s;
        match s.binding(name) {
            Some(b) => Ok(b),
            None => internal("unbekanntes Feld nach der Prüfung"),
        }
    }

    fn compare(&mut self, b: &Binding, op: CmpOp, value: &Value) -> SRes<String> {
        if matches!(value.kind, ValueKind::Null) {
            let e = self.is_empty(b)?;
            return match op {
                CmpOp::Eq => Ok(e),
                CmpOp::Ne => Ok(not(e)),
                _ => internal("NULL mit anderem Operator"),
            };
        }
        let vals = self.value_texts(value)?;
        let negate = matches!(op, CmpOp::Ne | CmpOp::NotContains);
        let pos_op = match op {
            CmpOp::Ne => CmpOp::Eq,
            CmpOp::NotContains => CmpOp::Contains,
            other => other,
        };
        let sql = self.pos(b, pos_op, &vals)?;
        Ok(if negate { not(sql) } else { sql })
    }

    /// Der Wert der rechten Seite als Texte (Funktionen werden ausgewertet; eine Funktion kann mehrere Werte liefern).
    fn value_texts(&mut self, v: &Value) -> SRes<Vec<String>> {
        match &v.kind {
            ValueKind::Number(s) | ValueKind::Date(s) | ValueKind::DateTime(s) | ValueKind::Word(s) | ValueKind::Text(s) => Ok(vec![s.clone()]),
            ValueKind::Null => Ok(Vec::new()),
            ValueKind::Func { name, .. } => match name.text.to_lowercase().as_str() {
                "currentdate" => Ok(vec![self.ctx.today.clone()]),
                "linkedterminal" => Ok(self.ctx.terminals.clone()),
                "activesprint" => pending::active_sprint_name(self),
                other => Err(SearchError::Internal(format!("unbekannte Funktion {other}"))),
            },
        }
    }

    // ------------------------------------------------------------------ Textvergleiche

    /// Platzhalter und Art (GLOB oder LIKE) für einen Suchtext.
    fn ci_pattern(&mut self, needle: &str, mode: Mode) -> (String, bool) {
        if needle.is_ascii() {
            (self.text(&like_pattern(needle, mode)), false)
        } else {
            (self.text(&glob_pattern(needle, mode)), true)
        }
    }

    fn ci_test(col: &str, ph: &str, glob: bool) -> String {
        if glob { format!("{col} GLOB {ph}") } else { format!("{col} LIKE {ph} ESCAPE '\\'") }
    }

    /// `col` stimmt mit einem der Texte überein (ohne Beachtung der Schreibweise).
    fn ci_any(&mut self, col: &str, vals: &[String], mode: Mode) -> String {
        let mut parts = Vec::new();
        for v in vals {
            let (ph, glob) = self.ci_pattern(v, mode);
            parts.push(Self::ci_test(col, &ph, glob));
        }
        or_join(parts)
    }

    fn free_text(&mut self, text: &str) -> String {
        let (ph, glob) = self.ci_pattern(text, Mode::Contains);
        let title = Self::ci_test("t.title", &ph, glob);
        let key = Self::ci_test("(p.key || '-' || t.number)", &ph, glob);
        let val = Self::ci_test("v.text", &ph, glob);
        format!(
            "({title} OR {key} OR EXISTS (SELECT 1 FROM ticket_values v JOIN fields f ON f.id = v.field_id WHERE v.ticket_id = t.id AND f.kind IN ('text', 'rich', 'labels') AND {val}))"
        )
    }

    // ------------------------------------------------------------------ positive Aussagen

    /// `EXISTS`-Aussage über die Werte der Felder (das `+` vor `v.field_id` hält SQLite davon ab, die Indizes über `field_id` zu wählen und
    /// je Ticket alle Werte des Feldes zu durchsuchen; gewünscht ist der Zugriff über den Schlüssel `(ticket_id, …)`); `cond` = zusätzliche Bedingung an die Zeile `v` (`1` = keine).
    fn exists_v(&mut self, ids: &[i64], cond: &str) -> String {
        if ids.is_empty() {
            return "0".to_string();
        }
        let f = self.ids_clause("+v.field_id", ids);
        if cond == "1" {
            format!("EXISTS (SELECT 1 FROM ticket_values v WHERE v.ticket_id = t.id AND {f})")
        } else {
            format!("EXISTS (SELECT 1 FROM ticket_values v WHERE v.ticket_id = t.id AND {f} AND ({cond}))")
        }
    }

    /// Referenz auf ein Ticket aus dem Schlüssel (`PAN-12`): SQL, das seine Kennung liefert; `None` bei ungültigem Schlüssel.
    fn ticket_ref(&mut self, key: &str) -> Option<String> {
        let (k, n) = parse_key(key)?;
        let (pk, pn) = (self.text(&k), self.int(n));
        Some(format!("(SELECT t2.id FROM tickets t2 JOIN projects p2 ON p2.id = t2.project_id WHERE p2.key = {pk} AND t2.number = {pn})"))
    }

    /// Positive Aussage `feld <op> werte` (`Eq`: eines der Werte; `Contains`; `Lt`/`Gt`/`Le`/`Ge`). Nie `NULL`.
    fn pos(&mut self, b: &Binding, op: CmpOp, vals: &[String]) -> SRes<String> {
        if vals.is_empty() {
            // z. B. `linkedTerminal()` ohne Terminal: nichts trifft
            if let Store::Sprint = b.store {
                return pending::sprint_condition(self, Some(op), vals);
            }
            return Ok("0".to_string());
        }
        let cmp_sql = |op: CmpOp| match op {
            CmpOp::Lt => "<",
            CmpOp::Gt => ">",
            CmpOp::Le => "<=",
            _ => ">=",
        };
        let ordering = matches!(op, CmpOp::Lt | CmpOp::Gt | CmpOp::Le | CmpOp::Ge);
        match &b.store {
            Store::Key => match op {
                CmpOp::Eq => Ok(self.ci_any("(p.key || '-' || t.number)", vals, Mode::Eq)),
                CmpOp::Contains => Ok(self.ci_any("(p.key || '-' || t.number)", vals, Mode::Contains)),
                _ => internal("Operator für key"),
            },
            Store::Title => match op {
                CmpOp::Eq => Ok(self.ci_any("t.title", vals, Mode::Eq)),
                CmpOp::Contains => Ok(self.ci_any("t.title", vals, Mode::Contains)),
                _ => internal("Operator für summary"),
            },
            Store::Parent => {
                let mut parts = Vec::new();
                for v in vals {
                    if let Some(sub) = self.ticket_ref(v) {
                        parts.push(format!("COALESCE(t.parent_id = {sub}, 0)"));
                    }
                }
                Ok(or_join(parts))
            }
            Store::Created | Store::Updated => {
                let col = if matches!(b.store, Store::Created) { "t.created_at" } else { "t.updated_at" };
                let mut parts = Vec::new();
                for v in vals {
                    if let Some(d) = norm_datetime(v) {
                        let p = self.text(&d);
                        let o = if ordering { cmp_sql(op) } else { "=" };
                        parts.push(format!("{col} {o} {p}"));
                    }
                }
                Ok(or_join(parts))
            }
            Store::Flagged | Store::Starred => {
                let col = if matches!(b.store, Store::Flagged) { "t.flagged" } else { "t.starred" };
                let mut want: Vec<i64> = vals.iter().filter_map(|v| parse_bool(v)).map(i64::from).collect();
                want.sort_unstable();
                want.dedup();
                Ok(self.ids_clause(col, &want))
            }
            Store::ChangedBy => {
                let mut want: Vec<String> = vals.iter().map(|v| v.to_lowercase()).filter(|v| CHANGED_BY.contains(&v.as_str())).collect();
                want.sort();
                want.dedup();
                if want.is_empty() {
                    return Ok("0".to_string());
                }
                let list = want.iter().map(|w| self.text(w)).collect::<Vec<_>>().join(", ");
                Ok(format!("COALESCE((SELECT h.source FROM history h WHERE h.ticket_id = t.id ORDER BY h.id DESC LIMIT 1) IN ({list}), 0)"))
            }
            Store::Sprint => pending::sprint_condition(self, Some(op), vals),
            Store::Type(set) => Ok(self.col_in("t.type_id", set, vals)),
            Store::Status(set) => Ok(self.col_in("t.status_id", set, vals)),
            Store::Project(set) => Ok(self.col_in("t.project_id", set, vals)),
            Store::Values(v) => self.values_pos(v, op, vals),
        }
    }

    /// `col IN (Kennungen der passenden Auswahlwerte)`.
    fn col_in(&mut self, col: &str, set: &OptionSet, vals: &[String]) -> String {
        let mut ids: Vec<i64> = Vec::new();
        for v in vals {
            for o in set.matching(v) {
                ids.extend(o.members.iter().map(|(_, id)| *id));
            }
        }
        ids.sort_unstable();
        ids.dedup();
        self.ids_clause(col, &ids)
    }

    fn values_pos(&mut self, v: &Vals, op: CmpOp, vals: &[String]) -> SRes<String> {
        let ids: Vec<i64> = v.fields.iter().map(|f| f.id).collect();
        let ordering = matches!(op, CmpOp::Lt | CmpOp::Gt | CmpOp::Le | CmpOp::Ge);
        let ord_sql = match op {
            CmpOp::Lt => "<",
            CmpOp::Gt => ">",
            CmpOp::Le => "<=",
            _ => ">=",
        };
        match v.class {
            Class::Text | Class::Labels | Class::Terminal => {
                let mode = match op {
                    CmpOp::Eq => Mode::Eq,
                    CmpOp::Contains => Mode::Contains,
                    _ => return internal("Operator für Textfeld"),
                };
                let cond = self.ci_any("v.text", vals, mode);
                let mut parts = vec![self.exists_v(&ids, &cond)];
                if v.links {
                    let c = self.ci_any("l.to_text", vals, mode);
                    parts.push(format!("EXISTS (SELECT 1 FROM links l WHERE l.from_ticket_id = t.id AND l.to_kind = 'terminal' AND {c})"));
                }
                Ok(or_join(parts))
            }
            Class::Number => {
                let nums: Vec<f64> = vals.iter().filter_map(|s| s.trim().parse::<f64>().ok()).filter(|x| x.is_finite()).collect();
                if nums.is_empty() {
                    return Ok("0".to_string());
                }
                let cond = if ordering {
                    let parts: Vec<String> = nums.iter().map(|x| format!("v.num {ord_sql} {}", self.real(*x))).collect();
                    or_join(parts)
                } else {
                    let list = nums.iter().map(|x| self.real(*x)).collect::<Vec<_>>().join(", ");
                    format!("v.num IN ({list})")
                };
                Ok(self.exists_v(&ids, &cond))
            }
            Class::Bool => {
                let mut want: Vec<i64> = vals.iter().filter_map(|v| parse_bool(v)).map(i64::from).collect();
                want.sort_unstable();
                want.dedup();
                if want.is_empty() || ordering {
                    return Ok("0".to_string());
                }
                let cond = self.ids_clause("v.num", &want);
                Ok(self.exists_v(&ids, &cond))
            }
            Class::Date | Class::DateTime => {
                let mut parts = Vec::new();
                for s in vals {
                    let n = if v.class == Class::Date { norm_date(s) } else { norm_datetime(s) };
                    if let Some(d) = n {
                        let p = self.text(&d);
                        parts.push(format!("v.text {} {p}", if ordering { ord_sql } else { "=" }));
                    }
                }
                let cond = or_join(parts);
                if cond == "0" { Ok(cond) } else { Ok(self.exists_v(&ids, &cond)) }
            }
            Class::Dropdown | Class::Multi => {
                // je Feld die passenden Wert-Kennungen
                let mut per_field: Vec<(i64, Vec<i64>)> = Vec::new();
                for val in vals {
                    for o in v.opts.matching(val) {
                        for (owner, id) in &o.members {
                            match per_field.iter_mut().find(|(f, _)| f == owner) {
                                Some((_, l)) => {
                                    if !l.contains(id) {
                                        l.push(*id)
                                    }
                                }
                                None => per_field.push((*owner, vec![*id])),
                            }
                        }
                    }
                }
                let owners: Vec<i64> = per_field.iter().map(|(f, _)| *f).collect();
                let mut parts = Vec::new();
                for (f, wanted) in &per_field {
                    let a = self.int(*f);
                    let b = self.ids_clause("v.num", wanted);
                    parts.push(format!("(v.field_id = {a} AND {b})"));
                }
                let cond = or_join(parts);
                if cond == "0" { Ok(cond) } else { Ok(self.exists_v(&owners, &cond)) }
            }
            Class::Ticket => {
                let mut parts = Vec::new();
                for s in vals {
                    if let Some(sub) = self.ticket_ref(s) {
                        parts.push(format!("v.text = CAST({sub} AS TEXT)"));
                    }
                }
                let cond = or_join(parts);
                if cond == "0" { Ok(cond) } else { Ok(self.exists_v(&ids, &cond)) }
            }
            // EINHÄNGEPUNKT Versionen
            Class::Version | Class::Milestone => pending::version_condition(self, &ids, Some(op), vals),
        }
    }

    // ------------------------------------------------------------------ leer

    /// Aussage „das Feld hat keinen Wert“ (nie `NULL`).
    fn is_empty(&mut self, b: &Binding) -> SRes<String> {
        Ok(match &b.store {
            Store::Key | Store::Title | Store::Created | Store::Updated | Store::Flagged | Store::Starred | Store::Type(_) | Store::Project(_) => "0".to_string(),
            Store::Status(_) => "t.status_id IS NULL".to_string(),
            Store::Parent => "t.parent_id IS NULL".to_string(),
            Store::ChangedBy => "NOT EXISTS (SELECT 1 FROM history h WHERE h.ticket_id = t.id)".to_string(),
            Store::Sprint => pending::sprint_condition(self, None, &[])?,
            Store::Values(v) => {
                let ids: Vec<i64> = v.fields.iter().map(|f| f.id).collect();
                if matches!(v.class, Class::Version | Class::Milestone) {
                    return pending::version_condition(self, &ids, None, &[]);
                }
                // Text-Werte ohne Inhalt zählen als leer
                let cond = if matches!(v.class, Class::Text | Class::Terminal | Class::Labels | Class::Date | Class::DateTime) { "COALESCE(v.text, '') <> ''" } else { "1" };
                let has = self.exists_v(&ids, cond);
                let mut sql = not(has);
                if v.links {
                    sql = format!("({sql} AND NOT EXISTS (SELECT 1 FROM links l WHERE l.from_ticket_id = t.id AND l.to_kind = 'terminal'))");
                }
                sql
            }
        })
    }

    // ------------------------------------------------------------------ Schlüsselwörter (eigene Prädikate)

    fn cond(&mut self, c: &Cond, depth: usize) -> SRes<String> {
        if depth > MAX_DEPTH {
            return Err(SearchError::TooDeep { max: MAX_DEPTH });
        }
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(SearchError::TooComplex { max: MAX_NODES });
        }
        match c {
            Cond::And { items } | Cond::Or { items } => {
                let and = matches!(c, Cond::And { .. });
                let mut parts = Vec::new();
                for i in items {
                    parts.push(self.cond(i, depth + 1)?);
                }
                if parts.is_empty() { Ok(if and { "1".into() } else { "0".into() }) } else { Ok(balanced(parts, if and { "AND" } else { "OR" })) }
            }
            Cond::Compare { field_id, op, value } => {
                let s = self.s;
                let Some(f) = s.kw_fields.get(field_id) else { return Ok("0".to_string()) };
                self.cond_leaf(f, *op, value)
            }
        }
    }

    /// `ticket_values`-Bedingung für einen JSON-Wert (Dropdown/Mehrfachauswahl: Wert-Kennung, Verweise: Kennung).
    fn json_eq(&mut self, f: &Field, v: &serde_json::Value, op: Option<&str>) -> Option<String> {
        let o = op.unwrap_or("=");
        match f.kind {
            FieldKind::Dropdown | FieldKind::Multi | FieldKind::Number | FieldKind::Checkbox => {
                let x = match v {
                    serde_json::Value::Bool(b) => f64::from(u8::from(*b)),
                    other => other.as_f64()?,
                };
                let p = self.real(x);
                Some(format!("v.num {o} {p}"))
            }
            FieldKind::Ticket | FieldKind::Version | FieldKind::Milestone => {
                let p = self.text(&v.as_i64()?.to_string());
                Some(format!("v.text {o} {p}"))
            }
            FieldKind::Text | FieldKind::Rich | FieldKind::Terminal | FieldKind::Labels => {
                if o != "=" {
                    return None;
                }
                let s = v.as_str()?;
                Some(self.ci_any("v.text", &[s.to_string()], Mode::Eq))
            }
            FieldKind::Date | FieldKind::Datetime => {
                let s = v.as_str()?;
                let n = if f.kind == FieldKind::Date { norm_date(s) } else { norm_datetime(s) }?;
                let p = self.text(&n);
                Some(format!("v.text {o} {p}"))
            }
        }
    }

    fn cond_leaf(&mut self, f: &Field, op: ConditionOp, value: &serde_json::Value) -> SRes<String> {
        let ids = [f.id];
        let nonempty = if matches!(f.kind, FieldKind::Text | FieldKind::Rich | FieldKind::Terminal | FieldKind::Labels | FieldKind::Date | FieldKind::Datetime) { "COALESCE(v.text, '') <> ''" } else { "1" };
        Ok(match op {
            ConditionOp::IsEmpty => not(self.exists_v(&ids, nonempty)),
            ConditionOp::IsNotEmpty => self.exists_v(&ids, nonempty),
            ConditionOp::Eq | ConditionOp::Ne | ConditionOp::In | ConditionOp::NotIn => {
                let list: Vec<&serde_json::Value> = match (op, value) {
                    (ConditionOp::In | ConditionOp::NotIn, serde_json::Value::Array(a)) => a.iter().collect(),
                    _ => vec![value],
                };
                let mut parts = Vec::new();
                for v in list {
                    if let Some(c) = self.json_eq(f, v, None) {
                        parts.push(c);
                    }
                }
                let cond = or_join(parts);
                let pos = if cond == "0" { cond } else { self.exists_v(&ids, &cond) };
                if matches!(op, ConditionOp::Ne | ConditionOp::NotIn) { not(pos) } else { pos }
            }
            ConditionOp::Gt | ConditionOp::Ge | ConditionOp::Lt | ConditionOp::Le => {
                let o = match op {
                    ConditionOp::Gt => ">",
                    ConditionOp::Ge => ">=",
                    ConditionOp::Lt => "<",
                    _ => "<=",
                };
                match self.json_eq(f, value, Some(o)) {
                    Some(c) => self.exists_v(&ids, &c),
                    None => "0".to_string(),
                }
            }
        })
    }

    // ------------------------------------------------------------------ Sortierung

    /// `ORDER BY`-Text (ohne `ORDER BY`): die Sortierfelder der Abfrage, danach die Schlüsselreihenfolge.
    fn order(&mut self, q: &Query) -> SRes<String> {
        let mut out: Vec<String> = Vec::new();
        let s = self.s;
        for term in &q.order {
            let mut dir = term.dir.unwrap_or(SortDir::Asc);
            // Sortier-Schlüsselwörter gehen vor gleichnamigen Feldern (z. B. `flagged` = markierte zuerst, nicht das Ja/Nein-Feld)
            let (exprs, flip) = match s.sort_kw(&term.name.text) {
                Some(SortKw::Favorites) => (vec![OExpr { sql: "t.starred".into(), nocase: false, nullable: false }], true),
                Some(SortKw::Flagged) => (vec![OExpr { sql: "t.flagged".into(), nocase: false, nullable: false }], true),
                Some(SortKw::Custom { field_id, dir: kd }) => {
                    if term.dir.is_none() {
                        dir = if *kd == KwDir::Desc { SortDir::Desc } else { SortDir::Asc };
                    }
                    let Some(f) = s.kw_fields.get(field_id) else { return internal("Sortierfeld fehlt") };
                    let vals = single_vals(f);
                    (self.values_order(&vals)?, false)
                }
                None => match s.binding(&term.name.text) {
                    Some(b) => (self.order_exprs(b)?, false),
                    None => return internal("Sortierfeld nach der Prüfung unbekannt"),
                },
            };
            // `favorites`/`flagged`: aufsteigend heißt „markierte zuerst“
            let dir_sql = match (dir, flip) {
                (SortDir::Asc, false) | (SortDir::Desc, true) => "ASC",
                _ => "DESC",
            };
            for e in exprs {
                if e.nullable {
                    out.push(format!("({}) IS NULL", e.sql));
                }
                let nc = if e.nocase { " COLLATE NOCASE" } else { "" };
                out.push(format!("({}){nc} {dir_sql}", e.sql));
            }
        }
        out.push("t.project_id".to_string());
        out.push("t.number".to_string());
        Ok(out.join(", "))
    }

    /// `CASE col WHEN id THEN stelle … END` über die Reihenfolge der Auswahlwerte.
    fn case_over(&mut self, col: &str, set: &OptionSet) -> String {
        let mut sql = format!("CASE {col}");
        for (idx, o) in set.opts.iter().enumerate() {
            for (_, id) in &o.members {
                let (a, b) = (self.int(*id), self.int(idx as i64));
                sql.push_str(&format!(" WHEN {a} THEN {b}"));
            }
        }
        sql.push_str(" END");
        sql
    }

    fn order_exprs(&mut self, b: &Binding) -> SRes<Vec<OExpr>> {
        let plain = |sql: &str, nocase: bool, nullable: bool| OExpr { sql: sql.to_string(), nocase, nullable };
        Ok(match &b.store {
            Store::Key => vec![plain("p.key", true, false), plain("t.number", false, false)],
            Store::Title => vec![plain("t.title", true, false)],
            Store::Parent => vec![plain("(SELECT pp.key || substr('0000000000' || pt.number, -10) FROM tickets pt JOIN projects pp ON pp.id = pt.project_id WHERE pt.id = t.parent_id)", true, true)],
            Store::Created => vec![plain("t.created_at", false, false)],
            Store::Updated => vec![plain("t.updated_at", false, false)],
            Store::Flagged => vec![plain("t.flagged", false, false)],
            Store::Starred => vec![plain("t.starred", false, false)],
            Store::ChangedBy => {
                let mut sql = "SELECT CASE h.source".to_string();
                for (i, name) in CHANGED_BY.iter().enumerate() {
                    let (a, b) = (self.text(name), self.int(i as i64));
                    sql.push_str(&format!(" WHEN {a} THEN {b}"));
                }
                sql.push_str(" END FROM history h WHERE h.ticket_id = t.id ORDER BY h.id DESC LIMIT 1");
                vec![OExpr { sql: format!("({sql})"), nocase: false, nullable: true }]
            }
            Store::Sprint => vec![OExpr { sql: pending::sprint_order(self)?, nocase: false, nullable: true }],
            Store::Type(set) => vec![OExpr { sql: self.case_over("t.type_id", set), nocase: false, nullable: true }],
            Store::Status(set) => vec![OExpr { sql: self.case_over("t.status_id", set), nocase: false, nullable: true }],
            Store::Project(set) => vec![OExpr { sql: self.case_over("t.project_id", set), nocase: false, nullable: true }],
            Store::Values(v) => self.values_order(v)?,
        })
    }

    fn values_order(&mut self, v: &Vals) -> SRes<Vec<OExpr>> {
        let ids: Vec<i64> = v.fields.iter().map(|f| f.id).collect();
        if ids.is_empty() {
            return Ok(vec![OExpr { sql: "NULL".into(), nocase: false, nullable: true }]);
        }
        let f = self.ids_clause("+v.field_id", &ids);
        let agg = |col: &str, f: &str| format!("(SELECT MIN({col}) FROM ticket_values v WHERE v.ticket_id = t.id AND {f})");
        Ok(match v.class {
            Class::Number | Class::Bool => vec![OExpr { sql: agg("v.num", &f), nocase: false, nullable: true }],
            Class::Date | Class::DateTime => vec![OExpr { sql: agg("v.text", &f), nocase: false, nullable: true }],
            Class::Text | Class::Terminal => {
                let mut sql = agg("v.text", &f);
                if v.links {
                    sql = format!("COALESCE({sql}, (SELECT MIN(l.to_text) FROM links l WHERE l.from_ticket_id = t.id AND l.to_kind = 'terminal'))");
                }
                vec![OExpr { sql, nocase: true, nullable: true }]
            }
            Class::Ticket => {
                let inner = agg("v.text", &f);
                vec![OExpr {
                    sql: format!("(SELECT pp.key || substr('0000000000' || pt.number, -10) FROM tickets pt JOIN projects pp ON pp.id = pt.project_id WHERE pt.id = CAST({inner} AS INTEGER))"),
                    nocase: true,
                    nullable: true,
                }]
            }
            Class::Dropdown => {
                // Felder mit gleicher Zuordnung (Wert-Kennung → Stelle) zusammenfassen
                let mut groups: Vec<(Vec<(i64, usize)>, Vec<i64>)> = Vec::new();
                for fld in &v.fields {
                    let mapping: Vec<(i64, usize)> = v.opts.opts.iter().enumerate().flat_map(|(idx, o)| o.members.iter().filter(|(ow, _)| *ow == fld.id).map(move |(_, id)| (*id, idx))).collect();
                    match groups.iter_mut().find(|(m, _)| *m == mapping) {
                        Some((_, fs)) => fs.push(fld.id),
                        None => groups.push((mapping, vec![fld.id])),
                    }
                }
                let mut sql = "CASE".to_string();
                for (mapping, fs) in groups {
                    let cond = self.ids_clause("+v.field_id", &fs);
                    sql.push_str(&format!(" WHEN {cond} THEN CASE v.num"));
                    for (id, idx) in mapping {
                        let (a, b) = (self.int(id), self.int(idx as i64));
                        sql.push_str(&format!(" WHEN {a} THEN {b}"));
                    }
                    sql.push_str(" END");
                }
                sql.push_str(" END");
                vec![OExpr { sql: agg(&sql, &f), nocase: false, nullable: true }]
            }
            Class::Multi | Class::Labels => return internal("Mehrfachfeld ist nicht sortierbar"),
            // EINHÄNGEPUNKT Versionen
            Class::Version | Class::Milestone => return Err(SearchError::NotAvailableYet(super::error::Feature::Versions)),
        })
    }
}

/// Ein einzelnes Feld als Bindung (für Sortier-Schlüsselwörter mit festem Feld).
fn single_vals(f: &Field) -> Vals {
    let mut opts = OptionSet::default();
    for o in &f.options.values {
        // Reihenfolge der Werte im Feld; ein Name ist hier nur ein Platzhalter für die Zusammenlegung (jeder Wert für sich)
        opts.opts.push(super::schema::MOpt { names: vec![format!("#{}", o.id)], members: vec![(f.id, o.id)] });
    }
    Vals { class: super::schema::class_of(f.kind), fields: vec![ValField { id: f.id, kind: f.kind }], opts, links: false }
}
