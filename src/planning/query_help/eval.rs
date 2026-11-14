//! Referenz-Auswertung: ein kleiner Interpreter über [`crate::query::Query`], der für eine [`DemoSpec`] die Tickets liefert, die eine
//! Abfrage trifft (in der Reihenfolge von `ORDER BY`). Er belegt, dass das Beispielergebnis stimmt, und legt die Regeln fest, an die
//! sich die spätere Übersetzung in SQL halten muss:
//!
//! * Groß-/Kleinschreibung ist bei Text, Namen und Werten egal.
//! * `!=`, `!~` und `NOT IN` treffen auch Tickets, bei denen das Feld leer ist (wie in Jira); `=`, `~`, `<`, `>`, `<=`, `>=`, `IN` nicht.
//! * Bei Mehrfachwerten (Labels) heißt `=`/`IN` „enthält“ und `!=`/`NOT IN` „enthält nicht“.
//! * Freitext (`"login"`) sucht im Titel und in allen Text-Feldern.
//! * Sortieren: Auswahlfelder nach der Reihenfolge ihrer Werte (Prioritäten: wichtigste zuerst), Zahlen/Daten aufsteigend, Text ohne
//!   Beachtung der Schreibweise; leere Werte stehen immer hinten (bei `ASC` und `DESC`); `favorites` stellt markierte Tickets nach vorn;
//!   bei Gleichstand gilt die Reihenfolge der Schlüssel.

use super::demo::{DemoSpec, DemoTicket, DemoValue};
use crate::planning::fields::FieldKind;
use crate::query::{CmpOp, Expr, FieldType, Query, SortDir, Value, ValueKind, parse, validate};
use std::cmp::Ordering;

/// Fester Bezugstag für Prüfungen. Die Beispieldaten nutzen relative Daten, das Ergebnis hängt also nicht vom Tag ab.
pub const REFERENCE_TODAY: &str = "2026-11-16";

impl DemoSpec {
    /// Schlüssel der Tickets, die `jql` trifft, in Ergebnis-Reihenfolge; „heute“ ist [`REFERENCE_TODAY`].
    pub fn expected_keys(&self, jql: &str) -> Result<Vec<String>, String> {
        self.evaluate(jql, REFERENCE_TODAY)
    }

    /// Wie [`DemoSpec::expected_keys`], mit frei wählbarem „heute“ (`JJJJ-MM-TT`).
    pub fn evaluate(&self, jql: &str, today: &str) -> Result<Vec<String>, String> {
        let spec = self.resolved(today);
        let schema = spec.schema();
        let query = parse(jql).map_err(|e| e.iter().map(|x| x.message(crate::state::Lang::En)).collect::<Vec<_>>().join("; "))?;
        let issues = validate(&query, &schema);
        if !issues.is_empty() {
            return Err(issues.iter().map(|x| x.message(crate::state::Lang::En)).collect::<Vec<_>>().join("; "));
        }
        let ctx = Ctx { spec: &spec, schema: &schema, today: today.to_string() };
        ctx.run(&query)
    }
}

struct Ctx<'a> {
    spec: &'a DemoSpec,
    schema: &'a crate::query::Schema,
    today: String,
}

impl Ctx<'_> {
    fn run(&self, q: &Query) -> Result<Vec<String>, String> {
        let mut hits: Vec<&DemoTicket> = Vec::new();
        for t in &self.spec.tickets {
            if match &q.filter {
                Some(e) => self.matches(e, t)?,
                None => true,
            } {
                hits.push(t);
            }
        }
        // Stabile Sortierung: bei Gleichstand bleibt die Reihenfolge der Schlüssel.
        let mut err = None;
        hits.sort_by(|a, b| {
            for term in &q.order {
                let dir = term.dir.unwrap_or(SortDir::Asc);
                match self.compare_by(&term.name.text, a, b, dir) {
                    Ok(Ordering::Equal) => {}
                    Ok(o) => return o,
                    Err(e) => {
                        err = Some(e);
                        return Ordering::Equal;
                    }
                }
            }
            Ordering::Equal
        });
        match err {
            Some(e) => Err(e),
            None => Ok(hits.iter().map(|t| t.key.clone()).collect()),
        }
    }

    // -----------------------------------------------------------------------------------------
    // Filter
    // -----------------------------------------------------------------------------------------

    fn matches(&self, e: &Expr, t: &DemoTicket) -> Result<bool, String> {
        Ok(match e {
            Expr::And(v) => {
                for x in v {
                    if !self.matches(x, t)? {
                        return Ok(false);
                    }
                }
                true
            }
            Expr::Or(v) => {
                for x in v {
                    if self.matches(x, t)? {
                        return Ok(true);
                    }
                }
                false
            }
            Expr::Not(x) => !self.matches(x, t)?,
            Expr::Text(s, _) => {
                let needle = s.to_lowercase();
                let mut hay = vec![t.summary.to_lowercase()];
                for f in self.spec.fields.iter().filter(|f| matches!(f.kind, FieldKind::Text | FieldKind::Rich)) {
                    hay.extend(self.values_of(t, &f.name).into_iter().map(|v| v.to_lowercase()));
                }
                hay.iter().any(|h| h.contains(&needle))
            }
            Expr::Predicate { name, negated } => {
                let on = match name.text.to_lowercase().as_str() {
                    "favorite" => t.favorite,
                    "blocked" => t.blocked,
                    other => return Err(format!("unknown predicate {other}")),
                };
                on != *negated
            }
            Expr::IsEmpty { field, negated, .. } => self.values_of(t, &field.text).is_empty() != *negated,
            Expr::Compare { field, op, value, .. } => {
                let vals = self.values_of(t, &field.text);
                if matches!(value.kind, ValueKind::Null) {
                    return Ok(vals.is_empty() == matches!(op, CmpOp::Eq));
                }
                let rhs = self.rhs(value)?;
                self.compare(&field.text, &vals, *op, &rhs)?
            }
            Expr::In { field, negated, values, .. } => {
                let vals = self.values_of(t, &field.text);
                let mut any = false;
                for v in values {
                    let rhs = self.rhs(v)?;
                    if self.compare(&field.text, &vals, CmpOp::Eq, &rhs)? {
                        any = true;
                        break;
                    }
                }
                any != *negated
            }
        })
    }

    /// Wert der rechten Seite als Text (Funktionen werden ausgewertet).
    fn rhs(&self, v: &Value) -> Result<String, String> {
        match &v.kind {
            ValueKind::Func { name, .. } => match name.text.to_lowercase().as_str() {
                "activesprint" => {
                    self.spec.active_sprint().map(|s| s.name.clone()).ok_or_else(|| "activeSprint() without an active sprint".to_string())
                }
                "currentdate" => Ok(self.today.clone()),
                "linkedterminal" => {
                    self.spec.current_terminal.clone().ok_or_else(|| "linkedTerminal() without a current terminal".to_string())
                }
                other => Err(format!("unknown function {other}")),
            },
            ValueKind::Null => Ok(String::new()),
            _ => Ok(v.literal_text().unwrap_or_default().to_string()),
        }
    }

    /// Alle Werte des Feldes bei diesem Ticket (leer = Feld ist leer; Mehrfachwerte liefern mehrere).
    fn values_of(&self, t: &DemoTicket, field: &str) -> Vec<String> {
        let one = |s: &Option<String>| s.iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>();
        match field.to_lowercase().as_str() {
            "type" => vec![t.ty.clone()],
            "status" => one(&t.status),
            "priority" => one(&t.priority),
            "summary" => vec![t.summary.clone()],
            "parent" => one(&t.parent),
            "sprint" => one(&t.sprint),
            "changedby" => one(&t.changed_by),
            _ => match t.values.iter().find(|(k, _)| k.eq_ignore_ascii_case(field)).map(|(_, v)| v) {
                None => Vec::new(),
                Some(DemoValue::Text(s)) if s.is_empty() => Vec::new(),
                Some(DemoValue::Text(s)) => vec![s.clone()],
                Some(DemoValue::Number(n)) => vec![fmt_num(*n)],
                Some(DemoValue::Bool(b)) => vec![b.to_string()],
                Some(DemoValue::List(l)) => l.clone(),
            },
        }
    }

    fn compare(&self, field: &str, vals: &[String], op: CmpOp, rhs: &str) -> Result<bool, String> {
        let def = self.schema.field(field).ok_or_else(|| format!("unknown field {field}"))?;
        let ci_eq = |a: &str, b: &str| a.to_lowercase() == b.to_lowercase();
        Ok(match &def.ty {
            FieldType::Number => {
                let r: f64 = rhs.parse().map_err(|_| format!("{rhs:?} is not a number"))?;
                let nums: Vec<f64> = vals.iter().filter_map(|v| v.parse().ok()).collect();
                match op {
                    CmpOp::Eq => nums.iter().any(|n| *n == r),
                    CmpOp::Ne => !nums.iter().any(|n| *n == r),
                    CmpOp::Lt => nums.iter().any(|n| *n < r),
                    CmpOp::Gt => nums.iter().any(|n| *n > r),
                    CmpOp::Le => nums.iter().any(|n| *n <= r),
                    CmpOp::Ge => nums.iter().any(|n| *n >= r),
                    CmpOp::Contains | CmpOp::NotContains => return Err("~ on a number".into()),
                }
            }
            FieldType::Date | FieldType::DateTime => {
                let r = norm_datetime(rhs);
                let ds: Vec<String> = vals.iter().map(|v| norm_datetime(v)).collect();
                match op {
                    CmpOp::Eq => ds.iter().any(|d| *d == r),
                    CmpOp::Ne => !ds.iter().any(|d| *d == r),
                    CmpOp::Lt => ds.iter().any(|d| *d < r),
                    CmpOp::Gt => ds.iter().any(|d| *d > r),
                    CmpOp::Le => ds.iter().any(|d| *d <= r),
                    CmpOp::Ge => ds.iter().any(|d| *d >= r),
                    CmpOp::Contains | CmpOp::NotContains => return Err("~ on a date".into()),
                }
            }
            FieldType::Bool => {
                let norm = |s: &str| matches!(s.to_lowercase().as_str(), "true" | "yes" | "1");
                let any = vals.iter().any(|v| norm(v) == norm(rhs));
                match op {
                    CmpOp::Eq => any,
                    CmpOp::Ne => !any,
                    _ => return Err("only = and != on a checkbox".into()),
                }
            }
            _ => match op {
                CmpOp::Eq => vals.iter().any(|v| ci_eq(v, rhs)),
                CmpOp::Ne => !vals.iter().any(|v| ci_eq(v, rhs)),
                CmpOp::Contains => vals.iter().any(|v| v.to_lowercase().contains(&rhs.to_lowercase())),
                CmpOp::NotContains => !vals.iter().any(|v| v.to_lowercase().contains(&rhs.to_lowercase())),
                _ => return Err(format!("{} on a {:?} field", op.symbol(), def.ty.kind())),
            },
        })
    }

    // -----------------------------------------------------------------------------------------
    // Sortierung
    // -----------------------------------------------------------------------------------------

    fn compare_by(&self, key: &str, a: &DemoTicket, b: &DemoTicket, dir: SortDir) -> Result<Ordering, String> {
        let Some(def) = self.schema.field(key) else {
            // Sortier-Schlüsselwort
            return match key.to_lowercase().as_str() {
                "favorites" => {
                    let o = b.favorite.cmp(&a.favorite); // markierte zuerst
                    Ok(if dir == SortDir::Desc { o.reverse() } else { o })
                }
                other => Err(format!("unknown sort keyword {other}")),
            };
        };
        let (va, vb) = (self.values_of(a, key).into_iter().next(), self.values_of(b, key).into_iter().next());
        let o = match (va, vb) {
            (None, None) => return Ok(Ordering::Equal),
            (None, Some(_)) => return Ok(Ordering::Greater), // leere Werte immer hinten
            (Some(_), None) => return Ok(Ordering::Less),
            (Some(x), Some(y)) => match &def.ty {
                FieldType::Option(list) => {
                    let pos = |v: &str| list.iter().position(|o| o.eq_ignore_ascii_case(v)).unwrap_or(usize::MAX);
                    pos(&x).cmp(&pos(&y))
                }
                FieldType::Number => x.parse::<f64>().unwrap_or(0.0).partial_cmp(&y.parse::<f64>().unwrap_or(0.0)).unwrap_or(Ordering::Equal),
                FieldType::Date | FieldType::DateTime => norm_datetime(&x).cmp(&norm_datetime(&y)),
                _ => x.to_lowercase().cmp(&y.to_lowercase()),
            },
        };
        Ok(if dir == SortDir::Desc { o.reverse() } else { o })
    }
}

fn fmt_num(n: f64) -> String {
    if n.fract() == 0.0 { format!("{}", n as i64) } else { n.to_string() }
}

/// Datum → `JJJJ-MM-TTTHH:MM:SS` (reines Datum = 00:00:00), damit Texte sich richtig vergleichen lassen.
fn norm_datetime(s: &str) -> String {
    let (d, t) = match s.split_once(['T', ' ']) {
        Some((d, t)) => (d, t),
        None => (s, ""),
    };
    let mut parts: Vec<&str> = t.split(':').filter(|p| !p.is_empty()).collect();
    while parts.len() < 3 {
        parts.push("00");
    }
    format!("{d}T{}", parts.join(":"))
}
