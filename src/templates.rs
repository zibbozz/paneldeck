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
