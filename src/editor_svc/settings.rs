//! *Einstellungen → Editor* (#54): Feld `editor` in `settings.json` (öffentliches Datenformat ab 3.0.0, nur additiv
//! ändern). Fehlt das Feld (ältere Dateien), gelten die Standards; ein falscher Typ eines einzelnen Werts ergibt nur für
//! diesen Wert den Standard, unbekannte Werte werden ignoriert. Die drei Agenten-Schalter sind immer aus, wenn sie nicht
//! ausdrücklich `true` sind.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Standard für [`EditorSettings::large_file_mb`] (Entscheidung im Issue: ab 20 MB).
pub const DEFAULT_LARGE_FILE_MB: u32 = 20;
/// Grenzen der Schwelle für große Dateien (MB).
pub const LARGE_FILE_MB_RANGE: (u32, u32) = (1, 4096);
/// Standard-Verzögerung des automatischen Speicherns (ms).
pub const DEFAULT_AUTOSAVE_MS: u32 = 1000;
/// Grenzen der Verzögerung (ms): 0,2 s bis 10 min.
pub const AUTOSAVE_MS_RANGE: (u32, u32) = (200, 600_000);
/// Höchstzahl eigener Ausschlussmuster und deren Länge.
pub const MAX_EXCLUDES: usize = 100;
pub const MAX_EXCLUDE_LEN: usize = 260;

/// Einstellungen des Editors (`settings.json` → `editor`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorSettings {
    /// Schriftfamilie des Editors; leer = die Terminalschrift (Standard).
    pub font: String,
    /// Ab dieser Dateigröße (MB) gilt eine Datei als groß: aufwendige Funktionen (Minimap, Umbruch, später Syntax) sind
    /// aus, eine Infoleiste sagt es. Auch die Suche in Dateien überspringt größere Dateien.
    pub large_file_mb: u32,
    /// Automatisch speichern (Standard aus) – nach `autosave_delay_ms` ohne weitere Eingabe, nur bestehende Dateien.
    pub autosave: bool,
    pub autosave_delay_ms: u32,
    /// Eigene Ausschlussmuster der Suche in Dateien (gitignore-Syntax, z. B. `node_modules`, `*.min.js`, `dist/`).
    pub search_exclude: Vec<String>,
    /// Standard für den Schalter *Ignorierte einbeziehen* der Suche (aus = `.gitignore` gilt).
    pub search_include_ignored: bool,
    /// Agenten dürfen Dateien öffnen (MCP `editor_open`; nur bei Stufe Steuern, nur lokale Pfade). Standard aus.
    pub agent_open: bool,
    /// Agenten dürfen geöffnete Dateien bearbeiten (MCP `editor_edit`; ungespeichert, als „vom Agenten“ markiert,
    /// rückgängig zu machen). Standard aus.
    pub agent_edit: bool,
    /// Agenten dürfen geöffnete, schon vorhandene Dateien speichern (MCP `editor_save`). Standard aus.
    pub agent_save: bool,
}

impl Default for EditorSettings {
    fn default() -> Self {
        EditorSettings {
            font: String::new(),
            large_file_mb: DEFAULT_LARGE_FILE_MB,
            autosave: false,
            autosave_delay_ms: DEFAULT_AUTOSAVE_MS,
            search_exclude: Vec::new(),
            search_include_ignored: false,
            agent_open: false,
            agent_edit: false,
            agent_save: false,
        }
    }
}

/// Stand der drei Agenten-Schalter für Stellen ohne Zugriff auf die Einstellungen (Werkzeugliste der Chats), wie
/// `mcp::LEVEL`: Bit 0 öffnen, 1 bearbeiten, 2 speichern. Die Oberfläche schreibt ihn jedes Bild ([`publish`]); geprüft
/// wird beim Ausführen trotzdem immer an den Einstellungen selbst.
pub static AGENT_SWITCHES: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Schalter veröffentlichen (jedes Bild).
pub fn publish(s: &EditorSettings) {
    AGENT_SWITCHES.store(switch_bits(s), std::sync::atomic::Ordering::Relaxed);
}

/// Schalter als Bits (siehe [`AGENT_SWITCHES`]).
pub fn switch_bits(s: &EditorSettings) -> u8 {
    (s.agent_open as u8) | ((s.agent_edit as u8) << 1) | ((s.agent_save as u8) << 2)
}

/// Was ein Agent im Editor tun will (je ein Schalter).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentRight {
    Open,
    Edit,
    Save,
}

impl AgentRight {
    /// Name des Feldes in `settings.json` (für Meldungen).
    pub fn field(self) -> &'static str {
        match self {
            AgentRight::Open => "agent_open",
            AgentRight::Edit => "agent_edit",
            AgentRight::Save => "agent_save",
        }
    }
}

impl EditorSettings {
    /// Tolerant aus einem JSON-Wert lesen (siehe Moduldoku); Zahlen werden auf ihre Grenzen gesetzt.
    pub fn from_value(v: &Value) -> EditorSettings {
        let d = EditorSettings::default();
        let Some(o) = v.as_object() else { return d };
        let num = |k: &str, def: u32, (lo, hi): (u32, u32)| o.get(k).and_then(Value::as_u64).map(|n| n.clamp(lo as u64, hi as u64) as u32).unwrap_or(def);
        let flag = |k: &str| o.get(k).and_then(Value::as_bool) == Some(true);
        EditorSettings {
            font: o.get("font").and_then(Value::as_str).map(|s| s.trim().chars().take(200).collect()).unwrap_or_default(),
            large_file_mb: num("large_file_mb", d.large_file_mb, LARGE_FILE_MB_RANGE),
            autosave: flag("autosave"),
            autosave_delay_ms: num("autosave_delay_ms", d.autosave_delay_ms, AUTOSAVE_MS_RANGE),
            search_exclude: o.get("search_exclude").and_then(Value::as_array).map(|a| clean_excludes(a.iter().filter_map(Value::as_str))).unwrap_or_default(),
            search_include_ignored: flag("search_include_ignored"),
            agent_open: flag("agent_open"),
            agent_edit: flag("agent_edit"),
            agent_save: flag("agent_save"),
        }
    }

    /// `deserialize_with` für `Settings::editor`.
    pub fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<EditorSettings, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(EditorSettings::from_value(&v))
    }

    /// Schwelle für große Dateien in Bytes.
    pub fn large_file_bytes(&self) -> u64 {
        self.large_file_mb.clamp(LARGE_FILE_MB_RANGE.0, LARGE_FILE_MB_RANGE.1) as u64 * 1024 * 1024
    }

    /// Steht der Schalter für `right` auf an? (Die Stufe prüft der Aufrufer: die Schalter wirken nur bei „Steuern“.)
    pub fn allows(&self, right: AgentRight) -> bool {
        match right {
            AgentRight::Open => self.agent_open,
            AgentRight::Edit => self.agent_edit,
            AgentRight::Save => self.agent_save,
        }
    }

    /// Eigene Ausschlussmuster aus einem Textfeld (eine Zeile oder Komma je Muster) übernehmen.
    pub fn set_excludes_from_text(&mut self, text: &str) {
        self.search_exclude = clean_excludes(text.split(['\n', ',']));
    }

    /// Ausschlussmuster als Text für ein mehrzeiliges Feld.
    pub fn excludes_text(&self) -> String {
        self.search_exclude.join("\n")
    }
}

/// Muster bereinigen: getrimmt, ohne leere und doppelte, gekürzt und begrenzt.
pub fn clean_excludes<'a>(it: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in it {
        let s = s.trim();
        if s.is_empty() || s.len() > MAX_EXCLUDE_LEN || out.iter().any(|o| o == s) {
            continue;
        }
        out.push(s.to_string());
        if out.len() == MAX_EXCLUDES {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_are_safe_and_documented() {
        let d = EditorSettings::default();
        assert_eq!(d.font, "", "leer = Terminalschrift");
        assert_eq!(d.large_file_mb, 20);
        assert!(!d.autosave && !d.search_include_ignored);
        assert!(!d.agent_open && !d.agent_edit && !d.agent_save, "Agenten-Schalter sind standardmäßig aus");
        assert_eq!(d.large_file_bytes(), 20 * 1024 * 1024);
        for r in [AgentRight::Open, AgentRight::Edit, AgentRight::Save] {
            assert!(!d.allows(r));
        }
        // Serialisiert mit genau den dokumentierten Feldnamen (öffentliches Datenformat).
        let v = serde_json::to_value(&d).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["agent_edit", "agent_open", "agent_save", "autosave", "autosave_delay_ms", "font", "large_file_mb", "search_exclude", "search_include_ignored"]);
    }

    #[test]
    fn reading_is_lenient_per_field() {
        let s = EditorSettings::from_value(&json!({
            "font": "  Fira Code ",
            "large_file_mb": 999999,
            "autosave": "yes",
            "autosave_delay_ms": 5,
            "search_exclude": ["node_modules", 3, " dist/ ", "", "node_modules"],
            "agent_open": true,
            "agent_edit": 1,
            "agent_save": "true",
            "future_field": {"x": 1}
        }));
        assert_eq!(s.font, "Fira Code");
        assert_eq!(s.large_file_mb, LARGE_FILE_MB_RANGE.1);
        assert!(!s.autosave, "nur true schaltet ein");
        assert_eq!(s.autosave_delay_ms, AUTOSAVE_MS_RANGE.0);
        assert_eq!(s.search_exclude, ["node_modules", "dist/"]);
        assert!(s.agent_open && !s.agent_edit && !s.agent_save);
        assert_eq!(EditorSettings::from_value(&json!("kaputt")), EditorSettings::default());
        assert_eq!(EditorSettings::from_value(&json!({"large_file_mb": -3})).large_file_mb, DEFAULT_LARGE_FILE_MB);
        // Rundweg
        let back = EditorSettings::from_value(&serde_json::to_value(&s).unwrap());
        assert_eq!(back, s);
    }

    #[test]
    fn exclude_text_round_trip() {
        let mut s = EditorSettings::default();
        s.set_excludes_from_text("node_modules\n*.min.js, build/\n\n  \nnode_modules");
        assert_eq!(s.search_exclude, ["node_modules", "*.min.js", "build/"]);
        assert_eq!(s.excludes_text(), "node_modules\n*.min.js\nbuild/");
        let many: String = (0..300).map(|i| format!("p{i}\n")).collect();
        s.set_excludes_from_text(&many);
        assert_eq!(s.search_exclude.len(), MAX_EXCLUDES);
    }
}
