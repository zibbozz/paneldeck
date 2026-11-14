//! Aktionsarten der Quick-Actions und ihre Parameter (serde-JSON mit Format-Version).
//!
//! * `create_ticket`: legt ein Ticket eines Ziel-Typs an, optional verknüpft mit dem Ausgangsticket (`relation`) und mit
//!   Standardwerten (`defaults`, Titel oder Felder des Ziel-Typs; Texte dürfen Platzhalter enthalten, siehe `resolve.rs`).
//! * `change_status`: setzt den Status des Tickets (die Übergangsregeln des Workflows gelten und werden später von der
//!   Ticket-Schicht geprüft) und schreibt optional Feldwerte (`set_values`).
//! * `start_session`: startet eine Terminal-/Claude-Sitzung, verknüpft mit dem Ticket.
//!
//! Gespeichert wird als JSON-Objekt der Parameter plus `"version": 1`. Fehlende Felder haben Standardwerte; eine neuere
//! Version als [`PARAMS_VERSION`] wird beim Lesen abgelehnt, nie überschrieben.

use super::{QRes, QuickError};
use crate::planning::workflow::SetValue;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{json, Value};

/// Aktuelle Format-Version der gespeicherten Parameter.
pub const PARAMS_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    CreateTicket,
    ChangeStatus,
    StartSession,
}

impl ActionKind {
    pub const ALL: [ActionKind; 3] = [ActionKind::CreateTicket, ActionKind::ChangeStatus, ActionKind::StartSession];

    pub fn as_str(self) -> &'static str {
        match self {
            ActionKind::CreateTicket => "create_ticket",
            ActionKind::ChangeStatus => "change_status",
            ActionKind::StartSession => "start_session",
        }
    }

    pub fn parse(s: &str) -> Option<ActionKind> {
        ActionKind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// Bezeichnung der Aktionsart für die Oberfläche.
    pub fn label(self, lang: crate::state::Lang) -> &'static str {
        match self {
            ActionKind::CreateTicket => lang.t("Ticket anlegen", "Create ticket"),
            ActionKind::ChangeStatus => lang.t("Status ändern", "Change status"),
            ActionKind::StartSession => lang.t("Sitzung starten", "Start session"),
        }
    }
}

/// Richtung der Verknüpfung, gesehen vom Ausgangsticket (dem Ticket, in dem der Knopf gedrückt wird).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkDirection {
    /// Ausgangsticket –Hinrichtung→ neues Ticket (z. B. „Userstory blockiert Devtask“).
    Out,
    /// Neues Ticket –Hinrichtung→ Ausgangsticket (z. B. „Devtask blockiert Userstory“).
    In,
}

/// Verknüpfung, die beim Anlegen zwischen Ausgangsticket und neuem Ticket gesetzt wird.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelationSpec {
    pub link_type_id: i64,
    pub direction: LinkDirection,
}

/// Ziel eines Standardwerts: der Titel des Tickets oder ein Feld des Ziel-Typs. Im JSON `"title"` bzw. die Feld-Id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldRef {
    Title,
    Field(i64),
}

impl Serialize for FieldRef {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            FieldRef::Title => s.serialize_str("title"),
            FieldRef::Field(id) => s.serialize_i64(*id),
        }
    }
}

impl<'de> Deserialize<'de> for FieldRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        match &v {
            Value::String(s) if s == "title" => Ok(FieldRef::Title),
            Value::Number(n) if n.is_i64() => Ok(FieldRef::Field(n.as_i64().unwrap_or(0))),
            _ => Err(serde::de::Error::custom("field_id must be \"title\" or a field id")),
        }
    }
}

/// Ein Standardwert für das neue Ticket.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DefaultValue {
    pub field_id: FieldRef,
    pub value: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CreateTicketParams {
    pub ticket_type_id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation: Option<RelationSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub defaults: Vec<DefaultValue>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChangeStatusParams {
    pub to_status_id: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub set_values: Vec<SetValue>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StartSessionParams {
    /// Kennung der Shell (z. B. `pwsh`); `None` = Standard-Shell. Nie ein frei wählbares Programm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// Claude in der Sitzung starten.
    #[serde(default)]
    pub claude: bool,
    /// Titel und Beschreibung des Tickets als Prompt an Claude übergeben (nur mit `claude`).
    #[serde(default)]
    pub prompt_from_ticket: bool,
    /// Arbeitsordner des Projekts als Startordner verwenden.
    #[serde(default)]
    pub cwd_from_project: bool,
}

/// Parameter einer Aktion; die Art ergibt sich aus der Variante.
#[derive(Debug, Clone, PartialEq)]
pub enum ActionParams {
    CreateTicket(CreateTicketParams),
    ChangeStatus(ChangeStatusParams),
    StartSession(StartSessionParams),
}

impl ActionParams {
    pub fn kind(&self) -> ActionKind {
        match self {
            ActionParams::CreateTicket(_) => ActionKind::CreateTicket,
            ActionParams::ChangeStatus(_) => ActionKind::ChangeStatus,
            ActionParams::StartSession(_) => ActionKind::StartSession,
        }
    }

    /// Leere Parameter der Art (Ausgangspunkt für „Aktion hinzufügen“ in der Oberfläche).
    pub fn empty(kind: ActionKind) -> ActionParams {
        match kind {
            ActionKind::CreateTicket => ActionParams::CreateTicket(CreateTicketParams::default()),
            ActionKind::ChangeStatus => ActionParams::ChangeStatus(ChangeStatusParams::default()),
            ActionKind::StartSession => ActionParams::StartSession(StartSessionParams::default()),
        }
    }

    /// JSON für die Datenbank (mit `version`).
    pub fn to_json(&self) -> String {
        let mut v = match self {
            ActionParams::CreateTicket(p) => serde_json::to_value(p),
            ActionParams::ChangeStatus(p) => serde_json::to_value(p),
            ActionParams::StartSession(p) => serde_json::to_value(p),
        }
        .unwrap_or_else(|_| json!({}));
        if let Some(o) = v.as_object_mut() {
            o.insert("version".into(), json!(PARAMS_VERSION));
        }
        v.to_string()
    }

    /// Liest die gespeicherten Parameter einer Aktionsart; neuere Version als bekannt oder kaputtes JSON → Fehler.
    pub fn from_json(kind: ActionKind, text: &str) -> QRes<ActionParams> {
        let text = text.trim_start_matches('\u{feff}');
        let mut v: Value = serde_json::from_str(text).map_err(|e| QuickError::Params(e.to_string()))?;
        let found = v.get("version").and_then(Value::as_u64).unwrap_or(1) as u32;
        if found > PARAMS_VERSION {
            return Err(QuickError::ParamsVersion { found, supported: PARAMS_VERSION });
        }
        if let Some(o) = v.as_object_mut() {
            o.remove("version");
        } else {
            return Err(QuickError::Params("not an object".into()));
        }
        let bad = |e: serde_json::Error| QuickError::Params(e.to_string());
        Ok(match kind {
            ActionKind::CreateTicket => ActionParams::CreateTicket(serde_json::from_value(v).map_err(bad)?),
            ActionKind::ChangeStatus => ActionParams::ChangeStatus(serde_json::from_value(v).map_err(bad)?),
            ActionKind::StartSession => ActionParams::StartSession(serde_json::from_value(v).map_err(bad)?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_roundtrip_through_text() {
        for k in ActionKind::ALL {
            assert_eq!(ActionKind::parse(k.as_str()), Some(k));
            assert_eq!(serde_json::to_string(&k).unwrap(), format!("\"{}\"", k.as_str()));
            assert_eq!(ActionParams::empty(k).kind(), k);
            assert_ne!(k.label(crate::state::Lang::De), k.label(crate::state::Lang::En));
        }
        assert_eq!(ActionKind::parse("nope"), None);
    }

    #[test]
    fn create_ticket_params_roundtrip_with_version() {
        let p = ActionParams::CreateTicket(CreateTicketParams {
            ticket_type_id: 7,
            relation: Some(RelationSpec { link_type_id: 2, direction: LinkDirection::In }),
            defaults: vec![
                DefaultValue { field_id: FieldRef::Title, value: json!("Task for {{source.key}}") },
                DefaultValue { field_id: FieldRef::Field(12), value: json!(3) },
            ],
        });
        let text = p.to_json();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["version"], json!(1));
        assert_eq!(v["defaults"][0]["field_id"], json!("title"));
        assert_eq!(v["defaults"][1]["field_id"], json!(12));
        assert_eq!(v["relation"]["direction"], json!("in"));
        assert_eq!(ActionParams::from_json(ActionKind::CreateTicket, &text).unwrap(), p);
    }

    #[test]
    fn change_status_and_session_params_roundtrip() {
        let c = ActionParams::ChangeStatus(ChangeStatusParams { to_status_id: 5, set_values: vec![SetValue { field_id: 9, value: json!("x") }] });
        assert_eq!(ActionParams::from_json(ActionKind::ChangeStatus, &c.to_json()).unwrap(), c);
        let s = ActionParams::StartSession(StartSessionParams { shell: Some("pwsh".into()), claude: true, prompt_from_ticket: true, cwd_from_project: true });
        assert_eq!(ActionParams::from_json(ActionKind::StartSession, &s.to_json()).unwrap(), s);
    }

    #[test]
    fn missing_fields_use_defaults() {
        let p = ActionParams::from_json(ActionKind::StartSession, "{}").unwrap();
        assert_eq!(p, ActionParams::empty(ActionKind::StartSession));
        let p = ActionParams::from_json(ActionKind::CreateTicket, r#"{"ticket_type_id": 4}"#).unwrap();
        assert_eq!(p, ActionParams::CreateTicket(CreateTicketParams { ticket_type_id: 4, ..Default::default() }));
    }

    #[test]
    fn newer_version_and_garbage_are_rejected() {
        assert_eq!(ActionParams::from_json(ActionKind::StartSession, r#"{"version": 2}"#), Err(QuickError::ParamsVersion { found: 2, supported: 1 }));
        assert!(matches!(ActionParams::from_json(ActionKind::StartSession, "nope"), Err(QuickError::Params(_))));
        assert!(matches!(ActionParams::from_json(ActionKind::StartSession, "[1]"), Err(QuickError::Params(_))));
        assert!(matches!(ActionParams::from_json(ActionKind::ChangeStatus, r#"{"to_status_id": "x"}"#), Err(QuickError::Params(_))));
        // ein Feld-Verweis ist „title“ oder eine Zahl, sonst nichts
        let bad = r#"{"ticket_type_id": 1, "defaults": [{"field_id": "summary", "value": "x"}]}"#;
        assert!(matches!(ActionParams::from_json(ActionKind::CreateTicket, bad), Err(QuickError::Params(_))));
        let bad = r#"{"ticket_type_id": 1, "defaults": [{"field_id": 1.5, "value": "x"}]}"#;
        assert!(matches!(ActionParams::from_json(ActionKind::CreateTicket, bad), Err(QuickError::Params(_))));
    }

    #[test]
    fn byte_order_mark_is_tolerated() {
        let text = format!("\u{feff}{}", ActionParams::empty(ActionKind::StartSession).to_json());
        assert!(ActionParams::from_json(ActionKind::StartSession, &text).is_ok());
    }
}
