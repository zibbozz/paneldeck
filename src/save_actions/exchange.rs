//! Export und Import ausgewählter Regeln (#383): JSON **ohne geheime Werte** (von Umgebungsvariablen bleiben nur die
//! Namen; die verschlüsselten Werte gelten ohnehin nur für diesen Benutzer auf diesem Rechner). Importierte Regeln
//! kommen ausgeschaltet an und bekommen neue Kennungen; die Oberfläche zeigt vorher alle Befehle.

use super::{ActionKind, Rule};
use serde_json::{json, Value};

/// Kennung des Formats in der Exportdatei.
pub const MARKER: &str = "paneldeck_save_actions";

/// Regeln als JSON-Text (ohne Werte der Umgebungsvariablen).
pub fn export(rules: &[&Rule]) -> String {
    let list: Vec<Value> = rules
        .iter()
        .map(|r| {
            let mut r = (*r).clone();
            for e in &mut r.env {
                e.value_protected = None;
            }
            serde_json::to_value(&r).unwrap_or(Value::Null)
        })
        .collect();
    serde_json::to_string_pretty(&json!({ MARKER: 1, "rules": list })).unwrap_or_default()
}

/// Exportierte Regeln lesen: ausgeschaltet, ohne Werte, Kennungen ab `first_id`. Fehler, wenn es keine Exportdatei ist.
pub fn import(text: &str, first_id: u64) -> Result<Vec<Rule>, String> {
    let v: Value = serde_json::from_str(crate::state::strip_bom(text)).map_err(|e| e.to_string())?;
    let o = v.as_object().ok_or("not an object")?;
    if !o.contains_key(MARKER) && !o.contains_key("version") {
        return Err("not an export of actions on save".into());
    }
    let items = o.get("rules").and_then(|r| r.as_array()).ok_or("no rules")?;
    let mut out = Vec::new();
    let mut id = first_id.max(1);
    for it in items.iter().take(super::MAX_RULES) {
        let Ok(mut r) = serde_json::from_value::<Rule>(it.clone()) else { continue };
        r.id = id;
        id += 1;
        r.enabled = false;
        for e in &mut r.env {
            e.value_protected = None;
        }
        r.actions.truncate(super::MAX_ACTIONS);
        r.timeout_secs = r.timeout_secs.clamp(super::TIMEOUT_MIN, super::TIMEOUT_MAX);
        out.push(r);
    }
    if out.is_empty() {
        return Err("no readable rules".into());
    }
    Ok(out)
}

/// Alles, was eine importierte Regel ausführen würde (für die Vorschau vor dem Import): Befehle, Kopierziele, Texte an
/// Sitzungen.
pub fn commands_of(r: &Rule) -> Vec<String> {
    r.actions
        .iter()
        .filter_map(|a| match &a.kind {
            ActionKind::Command { command } => Some(command.clone()),
            ActionKind::Copy { target, .. } => Some(format!("copy → {target}")),
            ActionKind::SendToSession { text, name, .. } => Some(format!("→ {name}: {text}")),
            ActionKind::RestartProcess { name, .. } => Some(format!("restart {name}")),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procs::EnvVar;
    use crate::save_actions::{Action, Scope};

    #[test]
    fn export_drops_secret_values_and_import_switches_rules_off() {
        let mut r = Rule::new(4, Scope::Folder, "C:\\w");
        r.name = "Upload".into();
        r.env = vec![EnvVar { name: "TOKEN".into(), value_protected: Some("BLOB".into()) }];
        r.actions = vec![Action::new(ActionKind::Command { command: "scp ${file} host:/x".into() })];
        let text = export(&[&r]);
        assert!(!text.contains("BLOB"));
        assert!(text.contains("TOKEN"));
        let back = import(&text, 10).unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].id, 10);
        assert!(!back[0].enabled);
        assert_eq!(back[0].env[0].value_protected, None);
        assert_eq!(commands_of(&back[0]), ["scp ${file} host:/x"]);
        assert!(import("{\"x\":1}", 1).is_err());
        assert!(import("nope", 1).is_err());
    }
}
