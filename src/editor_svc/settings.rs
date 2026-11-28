//! *Einstellungen → Editor* (#54): Feld `editor` in `settings.json` (öffentliches Datenformat ab 3.0.0, nur additiv
//! ändern). Fehlt das Feld (ältere Dateien), gelten die Standards; ein falscher Typ eines einzelnen Werts ergibt nur für
//! diesen Wert den Standard, unbekannte Werte werden ignoriert. Die drei Agenten-Schalter sind immer aus, wenn sie nicht
//! ausdrücklich `true` sind. Ab 3.1.0 (#312, Syntax-Modus): `default_mode`, `language_modes`, `language_map`.
//! Ab 3.2.0 (#381): `menu_bar` (Menüleiste je Teilung, Standard an – aus nur bei ausdrücklichem `false`),
//! und der Agenten-Schalter `agent_format` (Standard aus). Formatiert wird nie von allein: nur auf Knopfdruck oder über
//! eine Regel mit der Aktion *Formatieren* (Aktionen beim Speichern, #383); es gibt keine eigene Einstellung dafür.
//! Ab 3.2.0 (#385, Vergleich und Änderungsmarken): `change_markers`, `marker_base`, `diff_context`,
//! `diff_ignore_whitespace`, `diff_layout`.
//! Ab 3.3.0 (#394, mehrere Cursor; alle standardmäßig aus): `multi_cursor` (Kürzel und Befehle), `multi_cursor_alt_click`
//! (Alt+Klick), `multi_cursor_middle` (mittlere Maustaste: Spaltenauswahl und Cursor), `agent_selections`
//! (`editor_read` nennt die Auswahlen).

use crate::editor::syntax::{Mode, UserMap};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

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
/// Höchstzahl der Einträge in `language_modes` bzw. `language_map`, Länge einer Sprach-Id bzw. eines Musters.
pub const MAX_LANGUAGE_ENTRIES: usize = 500;
pub const MAX_LANGUAGE_ID_LEN: usize = 100;
pub const MAX_PATTERN_LEN: usize = 260;
/// Standard und Grenzen der Kontextzeilen im Vergleich (#385).
pub const DEFAULT_DIFF_CONTEXT: u32 = 3;
pub const DIFF_CONTEXT_RANGE: (u32, u32) = (0, 50);

/// Womit die Änderungsmarken vergleichen (#385): `git` = Index und HEAD zugleich (außerhalb eines Repositorys und für
/// Dateien, die git nicht kennt, die Datei auf der Platte), `disk` = immer die gespeicherte Datei.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarkerBase {
    #[default]
    Git,
    Disk,
}

impl MarkerBase {
    pub fn from_id(s: &str) -> Option<MarkerBase> {
        match s.trim().to_ascii_lowercase().as_str() {
            "git" => Some(MarkerBase::Git),
            "disk" => Some(MarkerBase::Disk),
            _ => None,
        }
    }
}

/// Darstellung des Vergleichs (#385): `auto` = nebeneinander, in schmalen Teilungen (unter ~700 px) untereinander.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffLayout {
    #[default]
    Auto,
    Split,
    Unified,
}

impl DiffLayout {
    pub fn from_id(s: &str) -> Option<DiffLayout> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(DiffLayout::Auto),
            "split" => Some(DiffLayout::Split),
            "unified" => Some(DiffLayout::Unified),
            _ => None,
        }
    }
}

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
    /// Modus einer Datei, für deren Sprache nichts eingestellt ist (#312): `text` (Standard) oder `syntax`.
    pub default_mode: Mode,
    /// Modus je Sprache (Sprach-Id → `text`/`syntax`), geht vor `default_mode`.
    pub language_modes: BTreeMap<String, Mode>,
    /// Eigene Zuordnungen (Muster → Sprach-Id), vor der eingebauten Erkennung: Dateiname (`Jenkinsfile`) oder Endung
    /// (`*.tpl`, `.tpl`).
    pub language_map: BTreeMap<String, String>,
    /// Menüleiste (*Datei · Bearbeiten · Ansicht · Format*) über jeder Teilung (#381). Standard an.
    pub menu_bar: bool,
    /// Agenten dürfen offene JSON/XML-Dokumente formatieren (MCP `format_file` außer `validate`; ungespeichert, als
    /// „vom Agenten“ markiert, rückgängig zu machen). Standard aus.
    pub agent_format: bool,
    /// Änderungsmarken am linken Rand (#385), Standard aus.
    pub change_markers: bool,
    /// Grundlage der Änderungsmarken.
    pub marker_base: MarkerBase,
    /// Kontextzeilen um Änderungen im Vergleich (Rest eingeklappt).
    pub diff_context: u32,
    /// Vorgabe für *Leerraum ignorieren* im Vergleich (Standard aus).
    pub diff_ignore_whitespace: bool,
    /// Vorgabe der Darstellung im Vergleich.
    pub diff_layout: DiffLayout,
    /// Mehrere Cursor (#394): Kürzel und Befehle (nächstes/alle Vorkommen, Cursor/Auswahl zurücknehmen). Standard aus.
    pub multi_cursor: bool,
    /// Alt+Klick setzt einen weiteren Cursor bzw. nimmt einen weg (#394). Standard aus.
    pub multi_cursor_alt_click: bool,
    /// Mittlere Maustaste im Text (#394): Ziehen = Spaltenauswahl, Strg dazu = hinzufügen, Klick = Cursor. Standard aus.
    pub multi_cursor_middle: bool,
    /// `editor_read` liefert zusätzlich die Auswahlen des Nutzers (#394, Feld `selections`). Standard aus.
    pub agent_selections: bool,
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
            default_mode: Mode::Text,
            language_modes: BTreeMap::new(),
            language_map: BTreeMap::new(),
            menu_bar: true,
            agent_format: false,
            change_markers: false,
            marker_base: MarkerBase::Git,
            diff_context: DEFAULT_DIFF_CONTEXT,
            diff_ignore_whitespace: false,
            diff_layout: DiffLayout::Auto,
            multi_cursor: false,
            multi_cursor_alt_click: false,
            multi_cursor_middle: false,
            agent_selections: false,
        }
    }
}

/// Stand der Agenten-Schalter für Stellen ohne Zugriff auf die Einstellungen (Werkzeugliste der Chats), wie
/// `mcp::LEVEL`: Bit 0 öffnen, 1 bearbeiten, 2 speichern, dazu [`DOCS_OPEN`] und [`FORMAT`] (formatieren, #381). Die Oberfläche schreibt ihn jedes Bild
/// ([`publish`]); geprüft wird beim Ausführen trotzdem immer an den Einstellungen selbst.
pub static AGENT_SWITCHES: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Bit in [`AGENT_SWITCHES`]: Im Editor ist mindestens ein Dokument offen (sonst gibt es für Chats nichts zu lesen,
/// zu bearbeiten oder zu speichern, und die Werkzeuge kosteten nur Platz im Kontext kleiner Modelle).
pub const DOCS_OPEN: u8 = 8;

/// Bit in [`AGENT_SWITCHES`]: *Agenten dürfen formatieren* (`agent_format`, #381).
pub const FORMAT: u8 = 16;

/// Schalter und „Dokumente offen“ veröffentlichen (jedes Bild).
pub fn publish(s: &EditorSettings, docs_open: bool) {
    AGENT_SWITCHES.store(switch_bits(s) | if docs_open { DOCS_OPEN } else { 0 }, std::sync::atomic::Ordering::Relaxed);
}

/// Schalter als Bits (siehe [`AGENT_SWITCHES`]).
pub fn switch_bits(s: &EditorSettings) -> u8 {
    (s.agent_open as u8) | ((s.agent_edit as u8) << 1) | ((s.agent_save as u8) << 2) | if s.agent_format { FORMAT } else { 0 }
}

/// Was ein Agent im Editor tun will (je ein Schalter).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AgentRight {
    Open,
    Edit,
    Save,
    /// JSON/XML formatieren (MCP `format_file`, #381).
    Format,
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
            default_mode: o.get("default_mode").and_then(Value::as_str).and_then(Mode::from_id).unwrap_or_default(),
            language_modes: o
                .get("language_modes")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| Some((clean_id(k)?, v.as_str().and_then(Mode::from_id)?)))
                        .take(MAX_LANGUAGE_ENTRIES)
                        .collect()
                })
                .unwrap_or_default(),
            language_map: o
                .get("language_map")
                .and_then(Value::as_object)
                .map(|m| m.iter().filter_map(|(k, v)| Some((clean_pattern(k)?, clean_id(v.as_str()?)?))).take(MAX_LANGUAGE_ENTRIES).collect())
                .unwrap_or_default(),
            // Standard an: aus nur bei ausdrücklichem `false`.
            menu_bar: o.get("menu_bar").and_then(Value::as_bool) != Some(false),
            agent_format: flag("agent_format"),
            change_markers: flag("change_markers"),
            marker_base: o.get("marker_base").and_then(Value::as_str).and_then(MarkerBase::from_id).unwrap_or_default(),
            diff_context: num("diff_context", d.diff_context, DIFF_CONTEXT_RANGE),
            diff_ignore_whitespace: flag("diff_ignore_whitespace"),
            diff_layout: o.get("diff_layout").and_then(Value::as_str).and_then(DiffLayout::from_id).unwrap_or_default(),
            multi_cursor: flag("multi_cursor"),
            multi_cursor_alt_click: flag("multi_cursor_alt_click"),
            multi_cursor_middle: flag("multi_cursor_middle"),
            agent_selections: flag("agent_selections"),
        }
    }

    /// Eigene Zuordnungen für die Erkennung.
    pub fn user_map(&self) -> UserMap {
        UserMap { entries: self.language_map.iter().map(|(k, v)| (k.clone(), v.clone())).collect() }
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
            AgentRight::Format => self.agent_format,
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

/// Sprach-Id bereinigen: getrimmt, nicht leer, begrenzt.
pub fn clean_id(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty() && s.len() <= MAX_LANGUAGE_ID_LEN).then(|| s.to_string())
}

/// Muster einer eigenen Zuordnung bereinigen (getrimmt, nicht leer, ohne Pfadtrenner, begrenzt).
pub fn clean_pattern(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty() && s.len() <= MAX_PATTERN_LEN && !s.contains(['/', '\\']) && s != "*" && s != "." && s != "*.").then(|| s.to_string())
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
        assert!(!d.agent_open && !d.agent_edit && !d.agent_save && !d.agent_format, "Agenten-Schalter sind standardmäßig aus");
        assert!(d.menu_bar, "Menüleiste standardmäßig an (#381)");
        assert_eq!(d.large_file_bytes(), 20 * 1024 * 1024);
        for r in [AgentRight::Open, AgentRight::Edit, AgentRight::Save, AgentRight::Format] {
            assert!(!d.allows(r));
        }
        // Serialisiert mit genau den dokumentierten Feldnamen (öffentliches Datenformat).
        let v = serde_json::to_value(&d).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "agent_edit",
                "agent_format",
                "agent_open",
                "agent_save",
                "agent_selections",
                "autosave",
                "autosave_delay_ms",
                "change_markers",
                "default_mode",
                "diff_context",
                "diff_ignore_whitespace",
                "diff_layout",
                "font",
                "language_map",
                "language_modes",
                "large_file_mb",
                "marker_base",
                "menu_bar",
                "multi_cursor",
                "multi_cursor_alt_click",
                "multi_cursor_middle",
                "search_exclude",
                "search_include_ignored"
            ]
        );
        assert!(!d.multi_cursor && !d.multi_cursor_alt_click && !d.multi_cursor_middle && !d.agent_selections, "mehrere Cursor standardmäßig aus (#394)");
        let on = EditorSettings::from_value(&json!({"multi_cursor": true, "multi_cursor_alt_click": true, "multi_cursor_middle": "yes", "agent_selections": 1}));
        assert!(on.multi_cursor && on.multi_cursor_alt_click && !on.multi_cursor_middle && !on.agent_selections);
        assert!(!d.change_markers, "Änderungsmarken sind standardmäßig aus (#385)");
        assert_eq!((v["marker_base"].as_str(), v["diff_layout"].as_str(), d.diff_context), (Some("git"), Some("auto"), 3));
        assert_eq!(v["default_mode"], "text", "neue Dateien: Text");
        assert!(d.language_modes.is_empty() && d.language_map.is_empty());
    }

    #[test]
    fn language_settings_round_trip_and_old_files_load() {
        // Datei von 3.0.0 ohne die neuen Felder.
        let old = EditorSettings::from_value(&json!({"font": "", "large_file_mb": 20, "agent_open": false}));
        assert_eq!(old.default_mode, Mode::Text);
        assert!(old.language_modes.is_empty() && old.language_map.is_empty());
        let s = EditorSettings::from_value(&json!({
            "default_mode": "SYNTAX",
            "language_modes": {"rust": "text", "python": "syntax", "go": "smart", "": "syntax", "lua": 3},
            "language_map": {"*.tpl": "html", " Jenkinsfile ": "source.groovy", "a/b": "rust", "*": "rust", "x": 5}
        }));
        assert_eq!(s.default_mode, Mode::Syntax);
        assert_eq!(s.language_modes.len(), 2, "{:?}", s.language_modes);
        let p = crate::app::editor_ui::SyntaxPrefs::from_settings(&s);
        assert_eq!(p.mode_for(Some("rust")), Mode::Text);
        assert_eq!(p.mode_for(Some("python")), Mode::Syntax);
        assert_eq!(p.mode_for(Some("go")), Mode::Syntax, "unbekannter Modus (smart) → Standard");
        assert_eq!(p.mode_for(None), Mode::Syntax);
        assert_eq!(s.language_map.len(), 2, "{:?}", s.language_map);
        assert_eq!(s.language_map.get("Jenkinsfile").map(String::as_str), Some("source.groovy"));
        let um = s.user_map();
        assert_eq!(um.lookup("page.tpl"), Some("html"));
        // Rundweg über JSON, auch als ganzes settings.json.
        let back = EditorSettings::from_value(&serde_json::to_value(&s).unwrap());
        assert_eq!(back, s);
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["language_modes"]["python"], "syntax");
        assert_eq!(v["language_map"]["*.tpl"], "html");
    }

    #[test]
    fn format_settings_of_3_2_0_round_trip_and_old_files_load() {
        // Datei von 3.1.0 ohne die neuen Felder: Menüleiste an, alles andere aus.
        let old = EditorSettings::from_value(&json!({"font": "", "agent_edit": true}));
        assert!(old.menu_bar && !old.agent_format);
        assert_eq!(switch_bits(&old) & FORMAT, 0);
        let s = EditorSettings::from_value(&json!({"menu_bar": false, "format_on_save_json": true, "format_on_save_xml": "yes", "agent_format": true}));
        // Die Schlüssel `format_on_save_*` gab es nur in Vorabständen von 3.2.0: sie werden ignoriert.
        assert!(!s.menu_bar && s.agent_format);
        assert!(!serde_json::to_value(&s).unwrap().as_object().unwrap().keys().any(|k| k.starts_with("format_on_save")));
        assert!(s.allows(AgentRight::Format) && !s.allows(AgentRight::Edit));
        assert_eq!(switch_bits(&s), FORMAT);
        // Falscher Typ: Menüleiste bleibt an (Standard).
        assert!(EditorSettings::from_value(&json!({"menu_bar": 0})).menu_bar);
        let back = EditorSettings::from_value(&serde_json::to_value(&s).unwrap());
        assert_eq!(back, s);
    }

    #[test]
    fn diff_settings_are_lenient_and_old_files_load() {
        let old = EditorSettings::from_value(&json!({"font": "", "default_mode": "text"}));
        assert!(!old.change_markers && !old.diff_ignore_whitespace);
        assert_eq!((old.marker_base, old.diff_layout, old.diff_context), (MarkerBase::Git, DiffLayout::Auto, DEFAULT_DIFF_CONTEXT));
        let s = EditorSettings::from_value(&json!({"change_markers": true, "marker_base": "DISK", "diff_context": 999, "diff_ignore_whitespace": true, "diff_layout": "unified"}));
        assert!(s.change_markers && s.diff_ignore_whitespace);
        assert_eq!((s.marker_base, s.diff_layout, s.diff_context), (MarkerBase::Disk, DiffLayout::Unified, DIFF_CONTEXT_RANGE.1));
        let bad = EditorSettings::from_value(&json!({"change_markers": "yes", "marker_base": "head", "diff_context": "x", "diff_layout": 3}));
        assert_eq!(bad, EditorSettings::default());
        assert_eq!(EditorSettings::from_value(&serde_json::to_value(&s).unwrap()), s);
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
