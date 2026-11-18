//! Sitzungs-Vorlagen: ganze Arbeitsumgebungen (Sitzungen, Ordner, Raster) speichern und mit einem Klick starten.

use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TplItem {
    pub name: String,
    pub shell_id: String,
    pub cwd: String,
    pub claude: bool,
    pub claude_args: String,
    pub profile: u64,
    /// Ordnerpfad im Sitzungsbaum (leer = kein Ordner).
    pub group: String,
    /// Start-Prompt (nur bei Claude-Sitzungen).
    pub prompt: String,
    /// „Konversation fortsetzen“ der Sitzung (#245); fehlt = an (wie bisher), gespeichert wird nur „aus“.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_continue: Option<bool>,
}

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Template {
    pub name: String,
    pub items: Vec<TplItem>,
    /// Beim Starten als Split-Raster anordnen.
    pub grid: bool,
}

pub fn load(dir: &std::path::Path) -> Vec<Template> {
    crate::state::load_json::<Vec<Template>>(&dir.join("templates.json")).unwrap_or_default()
}

pub fn save(dir: &std::path::Path, t: &[Template]) {
    crate::state::save_json(&dir.join("templates.json"), &t.to_vec());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_templates_load_and_continue_round_trips() {
        // Vorlage von 2.3.1 ohne `claude_continue`
        let old: Vec<Template> = serde_json::from_str(r#"[{"name":"A","grid":true,"items":[{"name":"x","shell_id":"pwsh","cwd":"C:\\a","claude":true,"claude_args":"--model opus","profile":0,"group":"","prompt":""}]}]"#).unwrap();
        assert_eq!(old[0].items[0].claude_continue, None);
        let s = serde_json::to_string(&old).unwrap();
        assert!(!s.contains("claude_continue"), "{s}");
        let it = TplItem { claude_continue: Some(false), ..TplItem::default() };
        let s = serde_json::to_string(&it).unwrap();
        assert!(s.contains(r#""claude_continue":false"#), "{s}");
        let back: TplItem = serde_json::from_str(&s).unwrap();
        assert_eq!(back.claude_continue, Some(false));
    }
}
