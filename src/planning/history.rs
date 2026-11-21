//! Verlauf eines Tickets: Quelle der Änderung (Oberfläche, Quick-Menü, Agent, Import), Einträge mit altem und neuem Wert
//! und die Anzeigetexte in beiden Sprachen.
//!
//! * Jede schreibende Funktion der Ticket-Schicht bekommt eine [`Source`] und schreibt ihre Einträge mit einer gemeinsamen
//!   `batch`-Nummer: Alles, was zu einer Aktion gehört (z. B. ein Statuswechsel samt gesetzter Werte), hat dieselbe Nummer.
//! * Die Spalte `field` nennt die Art der Änderung: `created`, `title`, `status`, `type`, `parent`, `flagged`, `starred`,
//!   `link`, `comment`, `time`, `moved` oder `field:<Feld-Id>` für Werte eigener Felder ([`HistoryField`]).
//! * `old`/`new` sind Rohwerte: Kennungen (Status, Typ, Eltern) als Zahl-Text, Feldwerte als JSON, Verknüpfungen und
//!   Zeiten als JSON-Objekt. [`describe`] macht daraus einen lesbaren Satz (Namen werden zum Anzeigezeitpunkt aufgelöst).

use super::fields::{get_field, Field, FieldKind};
use super::link_types::get_link_type;
use super::statuses::get_status;
use super::ticket_error::TRes;
use super::types::get_type;
use super::work_kinds::get_work_kind;
use crate::calc::duration::format_duration;
use crate::state::Lang;
use rusqlite::{params, Connection};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// Oberfläche (Fenster).
    Ui,
    /// Quick-Menü (Taste `.`).
    Quick,
    /// Agent über die MCP-Schnittstelle.
    Agent,
    /// Import.
    Import,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Ui => "ui",
            SourceKind::Quick => "quick",
            SourceKind::Agent => "agent",
            SourceKind::Import => "import",
        }
    }

    pub fn parse(s: &str) -> SourceKind {
        match s {
            "quick" => SourceKind::Quick,
            "agent" => SourceKind::Agent,
            "import" => SourceKind::Import,
            _ => SourceKind::Ui,
        }
    }
}

/// Zusatz der Quelle für Änderungen der Wartungsseite.
pub const MAINTENANCE: &str = "maintenance";

/// Wer eine Änderung ausgelöst hat.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub kind: SourceKind,
    /// Zusatz, z. B. der Name der Agenten-Sitzung.
    pub detail: Option<String>,
}

impl Source {
    pub fn ui() -> Self {
        Source { kind: SourceKind::Ui, detail: None }
    }

    pub fn quick() -> Self {
        Source { kind: SourceKind::Quick, detail: None }
    }

    /// Agent mit dem Namen seiner Sitzung (leer = ohne Angabe).
    pub fn agent(session: &str) -> Self {
        let d = session.trim();
        Source { kind: SourceKind::Agent, detail: if d.is_empty() { None } else { Some(d.to_string()) } }
    }

    pub fn import() -> Self {
        Source { kind: SourceKind::Import, detail: None }
    }

    /// Wartungsseite (#308): eine Änderung der Oberfläche mit dem Zusatz `maintenance` (das Schema kennt keine eigene Quelle);
    /// der Verlauf zeigt „geändert durch Wartung“.
    pub fn maintenance() -> Self {
        Source { kind: SourceKind::Ui, detail: Some(MAINTENANCE.to_string()) }
    }

    pub fn is_maintenance(&self) -> bool {
        self.kind == SourceKind::Ui && self.detail.as_deref() == Some(MAINTENANCE)
    }

    /// Anzeigetext: „User interface“, „Quick menu“, „Agent (Sitzung)“, „Import“.
    pub fn label(&self, lang: Lang) -> String {
        match self.kind {
            SourceKind::Ui if self.is_maintenance() => lang.t("Wartung", "Maintenance").to_string(),
            SourceKind::Ui => lang.t("Oberfläche", "User interface").to_string(),
            SourceKind::Quick => lang.t("Quick-Menü", "Quick menu").to_string(),
            SourceKind::Agent => match &self.detail {
                Some(d) => format!("Agent ({d})"),
                None => "Agent".to_string(),
            },
            SourceKind::Import => lang.t("Import", "Import").to_string(),
        }
    }
}

/// Art der Änderung eines Verlaufseintrags (Spalte `field`).
#[derive(Debug, Clone, PartialEq)]
pub enum HistoryField {
    Created,
    Title,
    Status,
    Type,
    Parent,
    Flagged,
    Starred,
    Link,
    Comment,
    Time,
    Moved,
    /// Wert eines Feldes (Feld-Id; das Feld kann inzwischen gelöscht sein).
    Custom(i64),
    Other(String),
}

impl HistoryField {
    pub fn as_column(&self) -> String {
        match self {
            HistoryField::Created => "created".into(),
            HistoryField::Title => "title".into(),
            HistoryField::Status => "status".into(),
            HistoryField::Type => "type".into(),
            HistoryField::Parent => "parent".into(),
            HistoryField::Flagged => "flagged".into(),
            HistoryField::Starred => "starred".into(),
            HistoryField::Link => "link".into(),
            HistoryField::Comment => "comment".into(),
            HistoryField::Time => "time".into(),
            HistoryField::Moved => "moved".into(),
            HistoryField::Custom(id) => format!("field:{id}"),
            HistoryField::Other(s) => s.clone(),
        }
    }

    pub fn parse(s: &str) -> HistoryField {
        match s {
            "created" => HistoryField::Created,
            "title" => HistoryField::Title,
            "status" => HistoryField::Status,
            "type" => HistoryField::Type,
            "parent" => HistoryField::Parent,
            "flagged" => HistoryField::Flagged,
            "starred" => HistoryField::Starred,
            "link" => HistoryField::Link,
            "comment" => HistoryField::Comment,
            "time" => HistoryField::Time,
            "moved" => HistoryField::Moved,
            other => match other.strip_prefix("field:").and_then(|n| n.parse().ok()) {
                Some(id) => HistoryField::Custom(id),
                None => HistoryField::Other(other.to_string()),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub id: i64,
    pub ticket_id: i64,
    pub at: String,
    pub source: Source,
    pub field: HistoryField,
    pub old: Option<String>,
    pub new: Option<String>,
    /// Gleiche Nummer = gehört zu einer Aktion.
    pub batch: i64,
}

const COLS: &str = "id, ticket_id, at, source, source_detail, field, old, new, batch";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<HistoryEntry> {
    Ok(HistoryEntry {
        id: r.get(0)?,
        ticket_id: r.get(1)?,
        at: r.get(2)?,
        source: Source { kind: SourceKind::parse(&r.get::<_, String>(3)?), detail: r.get(4)? },
        field: HistoryField::parse(&r.get::<_, String>(5)?),
        old: r.get(6)?,
        new: r.get(7)?,
        batch: r.get(8)?,
    })
}

/// Nächste Aktions-Nummer (fortlaufend, über alle Tickets; wird in `meta` mitgezählt, damit sie auch nach dem Löschen
/// von Tickets nie wiederkehrt).
pub(crate) fn next_batch(conn: &Connection) -> TRes<i64> {
    let cur: i64 = conn.query_row("SELECT COALESCE((SELECT value FROM meta WHERE key = 'history_batch'), '0')", [], |r| r.get::<_, String>(0))?.parse().unwrap_or(0);
    let next = cur + 1;
    conn.execute("INSERT INTO meta (key, value) VALUES ('history_batch', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value", [next.to_string()])?;
    Ok(next)
}

/// Schreibt einen Verlaufseintrag.
pub(crate) fn log(conn: &Connection, ticket_id: i64, source: &Source, batch: i64, field: &HistoryField, old: Option<&str>, new: Option<&str>) -> TRes<()> {
    conn.execute(
        "INSERT INTO history (ticket_id, at, source, source_detail, field, old, new, batch) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![ticket_id, super::common::now(), source.kind.as_str(), source.detail, field.as_column(), old, new, batch],
    )?;
    Ok(())
}

/// Der Verlauf eines Tickets, älteste Einträge zuerst (die Oberfläche kehrt ihn zur Anzeige um).
pub fn history_of(conn: &Connection, ticket_id: i64) -> TRes<Vec<HistoryEntry>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM history WHERE ticket_id = ?1 ORDER BY id"))?;
    Ok(st.query_map([ticket_id], from_row)?.collect::<Result<Vec<_>, _>>()?)
}

/// Alle Einträge mit einer Nummer über `after_id` (aufsteigend), optional nur einer Quelle – Grundlage für Hinweise wie
/// „Ein Agent hat ein Ticket geändert“. `after_id = 0` liefert alles.
pub fn history_since(conn: &Connection, after_id: i64, source: Option<SourceKind>) -> TRes<Vec<HistoryEntry>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM history WHERE id > ?1 AND (?2 IS NULL OR source = ?2) ORDER BY id"))?;
    Ok(st.query_map(params![after_id, source.map(|s| s.as_str())], from_row)?.collect::<Result<Vec<_>, _>>()?)
}

// ------------------------------------------------------------------ Anzeigetexte

fn quote(lang: Lang, s: &str) -> String {
    lang.q(s)
}

fn clip(s: &str, max: usize) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() > max { format!("{}…", one.chars().take(max).collect::<String>()) } else { one }
}

fn none_text(lang: Lang) -> &'static str {
    lang.t("(leer)", "(empty)")
}

fn status_text(conn: &Connection, raw: &Option<String>, lang: Lang) -> String {
    match raw.as_deref().and_then(|s| s.parse::<i64>().ok()) {
        Some(id) => get_status(conn, id).map(|s| s.display(lang)).unwrap_or_else(|_| lang.t("(gelöscht)", "(deleted)").to_string()),
        None => lang.t("(kein Status)", "(no status)").to_string(),
    }
}

fn type_text(conn: &Connection, raw: &Option<String>, lang: Lang) -> String {
    match raw.as_deref().and_then(|s| s.parse::<i64>().ok()) {
        Some(id) => get_type(conn, id).map(|t| t.display(lang)).unwrap_or_else(|_| lang.t("(gelöscht)", "(deleted)").to_string()),
        None => none_text(lang).to_string(),
    }
}

fn ticket_text(conn: &Connection, raw: &str, lang: Lang) -> String {
    match raw.parse::<i64>() {
        Ok(id) => super::tickets::get_ticket(conn, id).map(|t| t.key()).unwrap_or_else(|_| lang.t("(gelöscht)", "(deleted)").to_string()),
        Err(_) => raw.to_string(),
    }
}

/// Wert eines Feldes (JSON-Text) lesbar machen.
fn value_text(conn: &Connection, field: Option<&Field>, raw: &Option<String>, lang: Lang) -> String {
    let Some(raw) = raw else { return none_text(lang).to_string() };
    let v: Value = serde_json::from_str(raw).unwrap_or(Value::String(raw.clone()));
    let one = |v: &Value| -> String {
        match field.map(|f| f.kind) {
            Some(FieldKind::Dropdown) | Some(FieldKind::Multi) => {
                let id = v.as_i64().unwrap_or(0);
                field.and_then(|f| f.options.values.iter().find(|o| o.id == id)).map(|o| o.display(lang)).unwrap_or_else(|| format!("#{id}"))
            }
            Some(FieldKind::Checkbox) => if v.as_bool().unwrap_or(false) { lang.t("Ja", "Yes").to_string() } else { lang.t("Nein", "No").to_string() },
            Some(FieldKind::Ticket) => ticket_text(conn, &v.as_i64().map(|i| i.to_string()).unwrap_or_default(), lang),
            _ => match v {
                Value::String(s) => clip(&s, 80),
                other => other.to_string(),
            },
        }
    };
    match &v {
        Value::Array(a) => a.iter().map(one).collect::<Vec<_>>().join(", "),
        other => one(other),
    }
}

fn link_text(conn: &Connection, raw: &Option<String>, lang: Lang) -> String {
    let Some(v) = raw.as_deref().and_then(|r| serde_json::from_str::<Value>(r).ok()) else { return String::new() };
    let name = v["type"]
        .as_i64()
        .and_then(|id| get_link_type(conn, id).ok())
        .map(|lt| {
            let (out, inn) = lt.names(lang);
            if v["dir"] == "in" { inn.unwrap_or(out) } else { out }
        })
        .unwrap_or_else(|| lang.t("(gelöschter Typ)", "(deleted type)").to_string());
    let target = v["target"].as_str().unwrap_or("");
    let target = if v["kind"] == "branch" { target.replace('\n', " : ") } else { target.to_string() };
    format!("{name} {target}")
}

fn time_text(conn: &Connection, raw: &Option<String>, lang: Lang) -> String {
    let Some(v) = raw.as_deref().and_then(|r| serde_json::from_str::<Value>(r).ok()) else { return String::new() };
    let minutes = v["minutes"].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32;
    let mut s = format_duration(minutes, 8.0, lang);
    if let Some(k) = v["kind"].as_i64().and_then(|id| get_work_kind(conn, id).ok()) {
        s.push_str(&format!(" ({})", k.display(lang)));
    }
    if let Some(c) = v["comment"].as_str().filter(|c| !c.trim().is_empty()) {
        s.push_str(&format!(" – {}", clip(c, 60)));
    }
    s
}

/// Lesbarer Satz zu einem Eintrag in der gewünschten Sprache, z. B. „Status: To Do → In Progress“.
pub fn describe(conn: &Connection, e: &HistoryEntry, lang: Lang) -> String {
    let arrow = |a: String, b: String| format!("{a} → {b}");
    match &e.field {
        HistoryField::Created => match e.new.as_deref().and_then(|n| n.strip_prefix("clone:")) {
            Some(src) => format!("{} {src}", lang.t("Als Kopie angelegt von", "Created as a copy of")),
            None => match e.new.as_deref() {
                // Import einer Ticket-CSV: der Schlüssel in der Datei steht als Anmerkung (`import:PAN-12`).
                Some(n) if n == "import" || n.starts_with("import:") => match n.strip_prefix("import:").filter(|k| !k.is_empty()) {
                    Some(old_key) => format!("{} {old_key})", lang.t("Importiert (Schlüssel in der Datei:", "Imported (key in the file:")),
                    None => lang.t("Importiert", "Imported").to_string(),
                },
                _ => lang.t("Angelegt", "Created").to_string(),
            },
        },
        HistoryField::Title => format!("{}: {}", lang.t("Titel", "Title"), arrow(quote(lang, e.old.as_deref().unwrap_or("")), quote(lang, e.new.as_deref().unwrap_or("")))),
        HistoryField::Status => format!("Status: {}", arrow(status_text(conn, &e.old, lang), status_text(conn, &e.new, lang))),
        HistoryField::Type => format!("{}: {}", lang.t("Typ", "Type"), arrow(type_text(conn, &e.old, lang), type_text(conn, &e.new, lang))),
        HistoryField::Parent => {
            let t = |r: &Option<String>| r.as_deref().map(|s| ticket_text(conn, s, lang)).unwrap_or_else(|| lang.t("(keins)", "(none)").to_string());
            format!("{}: {}", lang.t("Übergeordnet", "Parent"), arrow(t(&e.old), t(&e.new)))
        }
        HistoryField::Flagged => if e.new.as_deref() == Some("1") { lang.t("Markierung gesetzt", "Flag set") } else { lang.t("Markierung entfernt", "Flag removed") }.to_string(),
        HistoryField::Starred => if e.new.as_deref() == Some("1") { lang.t("Als Favorit markiert", "Marked as favorite") } else { lang.t("Aus den Favoriten entfernt", "Removed from favorites") }.to_string(),
        HistoryField::Link => match (&e.old, &e.new) {
            (None, Some(_)) => format!("{}: {}", lang.t("Verknüpfung hinzugefügt", "Link added"), link_text(conn, &e.new, lang)),
            (Some(_), None) => format!("{}: {}", lang.t("Verknüpfung entfernt", "Link removed"), link_text(conn, &e.old, lang)),
            _ => format!("{}: {}", lang.t("Verknüpfung geändert", "Link changed"), link_text(conn, &e.new, lang)),
        },
        HistoryField::Comment => match (&e.old, &e.new) {
            (None, Some(n)) => format!("{}: {}", lang.t("Kommentar hinzugefügt", "Comment added"), clip(n, 80)),
            (Some(_), None) => lang.t("Kommentar gelöscht", "Comment deleted").to_string(),
            (_, Some(n)) => format!("{}: {}", lang.t("Kommentar geändert", "Comment edited"), clip(n, 80)),
            _ => lang.t("Kommentar geändert", "Comment edited").to_string(),
        },
        HistoryField::Time => match (&e.old, &e.new) {
            (None, Some(_)) => format!("{}: {}", lang.t("Zeit erfasst", "Time logged"), time_text(conn, &e.new, lang)),
            (Some(_), None) => format!("{}: {}", lang.t("Zeiteintrag gelöscht", "Time entry deleted"), time_text(conn, &e.old, lang)),
            _ => format!("{}: {}", lang.t("Zeiteintrag geändert", "Time entry changed"), arrow(time_text(conn, &e.old, lang), time_text(conn, &e.new, lang))),
        },
        HistoryField::Moved => format!("{} {}", lang.t("Verschoben von", "Moved from"), e.old.as_deref().unwrap_or("?")),
        HistoryField::Custom(fid) => {
            let field = get_field(conn, *fid).ok();
            let name = field.as_ref().map(|f| f.display(lang)).unwrap_or_else(|| lang.t("(gelöschtes Feld)", "(deleted field)").to_string());
            if field.as_ref().is_some_and(|f| f.kind == FieldKind::Rich) {
                return format!("{name}: {}", lang.t("geändert", "changed"));
            }
            format!("{name}: {}", arrow(value_text(conn, field.as_ref(), &e.old, lang), value_text(conn, field.as_ref(), &e.new, lang)))
        }
        HistoryField::Other(s) if s == "archived" => {
            if e.new.as_deref() == Some("1") { lang.t("Archiviert", "Archived") } else { lang.t("Aus dem Archiv geholt", "Restored from the archive") }.to_string()
        }
        HistoryField::Other(s) if s == "broken_link" => lang.t("Verknüpfung zu einem gelöschten Ticket entfernt", "Link to a deleted ticket removed").to_string(),
        HistoryField::Other(s) => s.clone(),
    }
}

/// Der Satz mit vorangestellter Quelle: „Agent (Sitzung): Status: To Do → Done“.
pub fn describe_with_source(conn: &Connection, e: &HistoryEntry, lang: Lang) -> String {
    format!("{}: {}", e.source.label(lang), describe(conn, e, lang))
}

/// JSON-Text eines Werts für `old`/`new` (`None` = leer).
pub(crate) fn json_text(v: Option<&Value>) -> Option<String> {
    v.map(|v| v.to_string())
}

/// JSON-Objekt einer Verknüpfung für den Verlauf.
pub(crate) fn link_json(link_type_id: i64, dir: &str, kind: &str, target: &str) -> String {
    json!({ "type": link_type_id, "dir": dir, "kind": kind, "target": target }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_kinds_roundtrip_and_have_labels() {
        for k in [SourceKind::Ui, SourceKind::Quick, SourceKind::Agent, SourceKind::Import] {
            assert_eq!(SourceKind::parse(k.as_str()), k);
        }
        assert_eq!(SourceKind::parse("???"), SourceKind::Ui);
        assert_eq!(Source::agent("  Sitzung 1 ").detail.as_deref(), Some("Sitzung 1"));
        assert_eq!(Source::agent("  ").detail, None);
        assert_eq!(Source::agent("S1").label(Lang::En), "Agent (S1)");
        assert_eq!(Source::agent("").label(Lang::De), "Agent");
        for s in [Source::ui(), Source::quick()] {
            assert_ne!(s.label(Lang::De), s.label(Lang::En));
        }
        assert_eq!(Source::import().label(Lang::En), "Import");
    }

    #[test]
    fn history_fields_roundtrip() {
        for f in [
            HistoryField::Created,
            HistoryField::Title,
            HistoryField::Status,
            HistoryField::Type,
            HistoryField::Parent,
            HistoryField::Flagged,
            HistoryField::Starred,
            HistoryField::Link,
            HistoryField::Comment,
            HistoryField::Time,
            HistoryField::Moved,
            HistoryField::Custom(42),
            HistoryField::Other("future".into()),
        ] {
            assert_eq!(HistoryField::parse(&f.as_column()), f);
        }
        assert_eq!(HistoryField::parse("field:x"), HistoryField::Other("field:x".into()));
    }

    #[test]
    fn batch_numbers_count_up() {
        let c = crate::db::open_memory();
        assert_eq!(next_batch(&c).unwrap(), 1);
        assert_eq!(next_batch(&c).unwrap(), 2);
        assert_eq!(next_batch(&c).unwrap(), 3);
    }

    mod describing {
        use super::super::*;
        use crate::planning::fields::{delete_field, FieldKind};
        use crate::planning::integrity::DeleteMode;
        use crate::planning::tickets::fixture::Fx;
        use crate::planning::tickets::*;
        use crate::planning::transition::transition_ticket;
        use serde_json::json;

        fn texts(f: &Fx, ticket: i64, lang: Lang) -> Vec<String> {
            history_of(&f.c, ticket).unwrap().iter().map(|e| describe(&f.c, e, lang)).collect()
        }

        #[test]
        fn describes_every_kind_of_change_in_both_languages() {
            let f = Fx::new();
            let epic = f.mk(f.epic, "E");
            let t = f.mk(f.story, "Alt");
            let sp = f.field(f.story, "story_points");
            let prio = f.field(f.story, "priority");
            let desc = f.field(f.story, "description");
            let epic_field = f.field(f.story, "epic");
            let done = f.add_field(f.story, "Fertig", FieldKind::Checkbox, Default::default(), false);
            let ui = Source::ui();
            update_ticket(&f.c, t.id, &TicketUpdate { title: Some("Neu".into()), values: vec![(sp.id, json!(5)), (prio.id, json!(1)), (desc.id, json!("Langer Text")), (epic_field.id, json!(epic.id)), (done.id, json!(true))] }, &ui).unwrap();
            transition_ticket(&f.c, t.id, f.progress, &[], &ui).unwrap();
            set_flagged(&f.c, t.id, true, &ui).unwrap();
            set_flagged(&f.c, t.id, false, &ui).unwrap();
            set_starred(&f.c, t.id, true, &ui).unwrap();
            set_starred(&f.c, t.id, false, &ui).unwrap();
            set_parent(&f.c, t.id, Some(epic.id), &ui).unwrap();
            set_parent(&f.c, t.id, None, &ui).unwrap();
            let en = texts(&f, t.id, Lang::En);
            let de = texts(&f, t.id, Lang::De);
            assert_eq!(en.len(), de.len());
            for (e, d) in en.iter().zip(&de) {
                assert!(!e.is_empty() && !d.is_empty());
            }
            let has = |v: &Vec<String>, s: &str| assert!(v.iter().any(|x| x == s), "{s} fehlt in {v:?}");
            has(&en, "Created");
            has(&de, "Angelegt");
            has(&en, "Title: “Alt” → “Neu”");
            has(&de, "Titel: „Alt“ → „Neu“");
            has(&en, "Status: To Do → In Progress");
            has(&de, "Status: Zu erledigen → In Arbeit");
            has(&en, "Flag set");
            has(&de, "Markierung gesetzt");
            has(&en, "Flag removed");
            has(&de, "Markierung entfernt");
            has(&en, "Marked as favorite");
            has(&de, "Als Favorit markiert");
            has(&en, "Removed from favorites");
            has(&de, "Aus den Favoriten entfernt");
            has(&en, "Parent: (none) → PAN-1");
            has(&de, "Übergeordnet: (keins) → PAN-1");
            has(&en, "Parent: PAN-1 → (none)");
            has(&en, "Story points: (empty) → 5");
            has(&de, "Story Points: (leer) → 5");
            has(&en, "Priority: Medium → Highest");
            has(&de, "Priorität: Normal → Höchste");
            has(&en, "Description: changed");
            has(&de, "Beschreibung: geändert");
            has(&en, "Epic: (empty) → PAN-1");
            has(&en, "Fertig: (empty) → Yes");
            has(&de, "Fertig: (leer) → Ja");
            // mit Quelle
            let e = history_of(&f.c, t.id).unwrap().into_iter().find(|e| e.field == HistoryField::Title).unwrap();
            assert_eq!(describe_with_source(&f.c, &e, Lang::En), "User interface: Title: “Alt” → “Neu”");
            assert_eq!(describe_with_source(&f.c, &e, Lang::De), "Oberfläche: Titel: „Alt“ → „Neu“");
        }

        #[test]
        fn shows_placeholders_for_deleted_things() {
            let f = Fx::new();
            let t = f.mk(f.story, "S");
            let sp = f.field(f.story, "story_points");
            let ui = Source::ui();
            set_field(&f.c, t.id, sp.id, json!(5), &ui).unwrap();
            let extra = crate::planning::statuses::create_status(&f.c, &crate::planning::statuses::NewStatus { project_id: f.project, key: None, name: Some("Weg".into()), color: "#445566".into(), category: crate::planning::statuses::StatusCategory::Progress }).unwrap();
            crate::planning::workflow::set_workflow_statuses(&f.c, f.story, &[f.todo, f.progress, f.done, extra]).unwrap();
            crate::planning::workflow::replace_transitions(&f.c, f.story, &[]).unwrap();
            transition_ticket(&f.c, t.id, extra, &[], &ui).unwrap();
            transition_ticket(&f.c, t.id, f.done, &[], &ui).unwrap();
            let epic = f.mk(f.epic, "E");
            set_parent(&f.c, t.id, Some(epic.id), &ui).unwrap();
            crate::planning::statuses::delete_status(&f.c, extra, Some(DeleteMode::Discard)).unwrap();
            delete_field(&f.c, sp.id, Some(DeleteMode::Discard)).unwrap();
            delete_ticket(&f.c, epic.id, &ui).unwrap();
            let en = texts(&f, t.id, Lang::En);
            assert!(en.iter().any(|x| x == "Status: To Do → (deleted)"), "{en:?}");
            assert!(en.iter().any(|x| x == "(deleted field): (empty) → 5"), "{en:?}");
            assert!(en.iter().any(|x| x == "Parent: PAN-2 → (none)" || x == "Parent: (deleted) → (none)"), "{en:?}");
            let de = texts(&f, t.id, Lang::De);
            assert!(de.iter().any(|x| x == "(gelöschtes Feld): (leer) → 5"), "{de:?}");
        }

        #[test]
        fn history_since_filters_by_source() {
            let f = Fx::new();
            let a = f.mk(f.task, "A");
            let last = history_of(&f.c, a.id).unwrap().last().unwrap().id;
            set_flagged(&f.c, a.id, true, &Source::agent("S")).unwrap();
            set_starred(&f.c, a.id, true, &Source::ui()).unwrap();
            let all = history_since(&f.c, last, None).unwrap();
            assert_eq!(all.len(), 2);
            let agent = history_since(&f.c, last, Some(SourceKind::Agent)).unwrap();
            assert_eq!(agent.len(), 1);
            assert_eq!(agent[0].source.detail.as_deref(), Some("S"));
            assert!(history_since(&f.c, all[1].id, None).unwrap().is_empty());
            assert!(history_since(&f.c, 0, Some(SourceKind::Import)).unwrap().is_empty());
            assert_eq!(history_since(&f.c, 0, Some(SourceKind::Ui)).unwrap().len(), 2, "Anlegen und Stern");
        }

        #[test]
        fn unknown_columns_survive() {
            let f = Fx::new();
            let a = f.mk(f.task, "A");
            f.c.execute("INSERT INTO history (ticket_id, at, source, field, batch) VALUES (?1, 'x', 'import', 'future_thing', 0)", [a.id]).unwrap();
            let e = history_of(&f.c, a.id).unwrap().pop().unwrap();
            assert_eq!(e.field, HistoryField::Other("future_thing".into()));
            assert_eq!(describe(&f.c, &e, Lang::En), "future_thing");
            assert_eq!(e.source.kind, SourceKind::Import);
        }
    }

    #[test]
    fn clips_long_text() {
        assert_eq!(clip("a  b\nc", 10), "a b c");
        assert_eq!(clip(&"x".repeat(10), 4), "xxxx…");
    }
}
