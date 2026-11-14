//! Gemeinsame Hilfen: Atomarität, Bereinigen/Prüfen von Namen, Farben und Schlüsseln, Reihenfolge, Namenskonflikte.

use super::names;
use super::{PlanError, Res};
use crate::state::Lang;
use rusqlite::{params_from_iter, Connection};

/// Längster Name in Zeichen (Projekte, Status, Typen, Felder, Werte …).
pub const MAX_NAME_CHARS: usize = 80;

/// Führt `f` atomar aus (SAVEPOINT): bei einem Fehler wird alles zurückgenommen. Lässt sich verschachteln und in einer
/// äußeren Transaktion benutzen.
pub fn atomic<T>(conn: &Connection, f: impl FnOnce() -> Res<T>) -> Res<T> {
    conn.execute_batch("SAVEPOINT plan_op")?;
    match f() {
        Ok(v) => {
            conn.execute_batch("RELEASE plan_op")?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO plan_op; RELEASE plan_op");
            Err(e)
        }
    }
}

pub fn bool_i(b: bool) -> i64 {
    b as i64
}

/// Name bereinigen: Leerraum zusammenfassen; leer → `NameEmpty`, zu lang → `NameTooLong`.
pub fn clean_name(s: &str) -> Res<String> {
    let n = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if n.is_empty() {
        return Err(PlanError::NameEmpty);
    }
    if n.chars().count() > MAX_NAME_CHARS {
        return Err(PlanError::NameTooLong { max: MAX_NAME_CHARS });
    }
    Ok(n)
}

/// Namensänderung: leer zurücksetzen (nur möglich, wenn es einen Standard-Schlüssel gibt → `None` = Standardname) oder bereinigen.
pub fn rename_value(key: Option<&str>, new: &str) -> Res<Option<String>> {
    if new.trim().is_empty() {
        return if key.is_some() { Ok(None) } else { Err(PlanError::NameEmpty) };
    }
    Ok(Some(clean_name(new)?))
}

/// Farbe „#RRGGBB“ prüfen und in Großbuchstaben bringen.
pub fn normalize_color(s: &str) -> Res<String> {
    let t = s.trim();
    let ok = t.len() == 7 && t.starts_with('#') && t[1..].chars().all(|c| c.is_ascii_hexdigit());
    if ok { Ok(t.to_ascii_uppercase()) } else { Err(PlanError::ColorInvalid) }
}

/// Projektschlüssel normalisieren (Leerraum entfernen, ASCII-Großbuchstaben). Prüfen mit [`validate_key`].
pub fn normalize_key(s: &str) -> String {
    s.trim().to_ascii_uppercase()
}

/// Streng: 2–10 Zeichen, nur `A–Z` und `0–9`, erstes Zeichen ein Buchstabe.
pub fn validate_key(s: &str) -> Res<()> {
    let n = s.chars().count();
    let ok = (2..=10).contains(&n) && s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) && s.chars().next().is_some_and(|c| c.is_ascii_uppercase());
    if ok { Ok(()) } else { Err(PlanError::KeyInvalid) }
}

/// Standard-Schlüssel (Übersetzung): kleingeschriebene ASCII-Bezeichnung mit `_` und `.`.
pub fn validate_ident(s: &str) -> Res<()> {
    let ok = !s.is_empty() && s.len() <= 40 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.');
    if ok { Ok(()) } else { Err(PlanError::KeyInvalid) }
}

/// Nächste freie Position (ans Ende) in dem durch `where_sql` (Parameter `?1…`) umrissenen Bereich.
pub fn next_position(conn: &Connection, table: &str, where_sql: &str, params: &[i64]) -> Res<i64> {
    let sql = format!("SELECT COALESCE(MAX(position) + 1, 0) FROM {table} WHERE {where_sql}");
    Ok(conn.query_row(&sql, params_from_iter(params.iter()), |r| r.get(0))?)
}

/// Neue Reihenfolge setzen. `ids` muss genau die Einträge des Bereichs enthalten (jeder einmal), sonst `ReorderMismatch`.
pub fn reorder(conn: &Connection, table: &str, where_sql: &str, params: &[i64], ids: &[i64]) -> Res<()> {
    let mut have: Vec<i64> = {
        let mut st = conn.prepare(&format!("SELECT id FROM {table} WHERE {where_sql}"))?;
        st.query_map(params_from_iter(params.iter()), |r| r.get(0))?.collect::<Result<_, _>>()?
    };
    let mut want = ids.to_vec();
    have.sort_unstable();
    want.sort_unstable();
    if have != want || want.windows(2).any(|w| w[0] == w[1]) {
        return Err(PlanError::ReorderMismatch);
    }
    atomic(conn, || {
        for (i, id) in ids.iter().enumerate() {
            conn.execute(&format!("UPDATE {table} SET position = ?1 WHERE id = ?2"), [i as i64, *id])?;
        }
        Ok(())
    })
}

/// Vergleichbare Bezeichnungen eines Eintrags (klein geschrieben): ein eigener Name gilt in beiden Sprachen, sonst beide
/// Übersetzungen des Schlüssels.
pub fn labels(key: Option<&str>, name: Option<&str>) -> Vec<String> {
    if let Some(n) = name {
        if !n.trim().is_empty() {
            return vec![n.trim().to_lowercase()];
        }
    }
    match key {
        Some(k) => {
            let mut v = vec![names::display_name(Some(k), None, Lang::En).to_lowercase(), names::display_name(Some(k), None, Lang::De).to_lowercase()];
            v.dedup();
            v
        }
        None => Vec::new(),
    }
}

/// Heißen die beiden Einträge (in irgendeiner Sprache) gleich?
pub fn same_label(a_key: Option<&str>, a_name: Option<&str>, b_key: Option<&str>, b_name: Option<&str>) -> bool {
    let (a, b) = (labels(a_key, a_name), labels(b_key, b_name));
    a.iter().any(|x| b.contains(x))
}

/// Stellt sicher, dass kein anderer Eintrag des Bereichs (`id <> exclude`) denselben Namen trägt. Tabellen mit Spalten `id, key, name`.
pub fn ensure_name_free(conn: &Connection, table: &str, where_sql: &str, params: &[i64], key: Option<&str>, name: Option<&str>, exclude: Option<i64>) -> Res<()> {
    let mut st = conn.prepare(&format!("SELECT id, key, name FROM {table} WHERE {where_sql}"))?;
    let rows = st.query_map(params_from_iter(params.iter()), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?, r.get::<_, Option<String>>(2)?)))?;
    for row in rows {
        let (id, k, n) = row?;
        if Some(id) == exclude {
            continue;
        }
        if same_label(key, name, k.as_deref(), n.as_deref()) {
            return Err(PlanError::NameTaken);
        }
    }
    Ok(())
}

/// Lokale Zeit als ISO-Text (wie `meta.created_at`).
pub fn now() -> String {
    crate::tasks::now()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_names() {
        assert_eq!(clean_name("  a   b ").unwrap(), "a b");
        assert_eq!(clean_name("  "), Err(PlanError::NameEmpty));
        assert_eq!(clean_name(&"x".repeat(81)), Err(PlanError::NameTooLong { max: 80 }));
        assert!(clean_name(&"x".repeat(80)).is_ok());
        assert_eq!(rename_value(Some("todo"), " ").unwrap(), None);
        assert_eq!(rename_value(None, " "), Err(PlanError::NameEmpty));
        assert_eq!(rename_value(None, " Neu ").unwrap(), Some("Neu".into()));
    }

    #[test]
    fn checks_colors_and_keys() {
        assert_eq!(normalize_color("#aabbcc").unwrap(), "#AABBCC");
        for bad in ["", "aabbcc", "#abc", "#gggggg", "#aabbccdd", "red"] {
            assert_eq!(normalize_color(bad), Err(PlanError::ColorInvalid), "{bad}");
        }
        for ok in ["AB", "PAN", "A1", "ABCDEFGHIJ", "X9Y8"] {
            assert!(validate_key(ok).is_ok(), "{ok}");
        }
        for bad in ["", "A", "ABCDEFGHIJK", "1AB", "pan", "A-B", "A B", "ÄB", "A_B"] {
            assert_eq!(validate_key(bad), Err(PlanError::KeyInvalid), "{bad}");
        }
        assert_eq!(normalize_key(" pan "), "PAN");
    }

    #[test]
    fn labels_cover_both_languages() {
        assert!(same_label(Some("todo"), None, None, Some("to do")));
        assert!(same_label(Some("todo"), None, None, Some("Zu erledigen")));
        assert!(!same_label(Some("todo"), None, None, Some("Backlog")));
        assert!(same_label(None, Some("A"), None, Some("a")));
        assert!(!same_label(None, None, None, None));
    }
}
