//! Gemerkter Zustand des Editors (#54) in `editor-state.json` im Datenordner: offener Ordner, zuletzt geöffnete Dateien und
//! Ordner, Teilungen mit ihren Reitern, Cursor und Bildlauf je Reiter. Die Datei ist **intern** (nicht Teil der öffentlichen
//! Datenformate); sie wird trotzdem tolerant gelesen (fehlende Felder → Standard, kaputtes Layout → keins) und nur additiv
//! erweitert. Ungespeicherte Inhalte stehen nicht hier, sondern im Hot-Exit-Speicher des Editor-Kerns (`hot` = dessen Schlüssel).

use crate::layout::Node;
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const FILE: &str = "editor-state.json";
pub const VERSION: u32 = 1;
/// Höchstzahl gemerkter zuletzt geöffneter Dateien bzw. Ordner.
pub const RECENT_MAX: usize = 15;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(default)]
pub struct Saved {
    pub version: u32,
    /// Geöffneter Ordner (Wurzel des Baums).
    pub root: Option<String>,
    /// Aufgeklappte Ordner im Baum.
    pub expanded: Vec<String>,
    pub recent_folders: Vec<String>,
    pub recent_files: Vec<String>,
    #[serde(deserialize_with = "crate::layout::lenient")]
    pub layout: Option<Node>,
    pub focused: u64,
    pub groups: Vec<SavedGroup>,
    pub docs: Vec<SavedDoc>,
    pub tree_open: bool,
    pub tree_width: f32,
    /// Weicher Zeilenumbruch (Ansicht, gilt für alle Reiter; ab der Großdatei-Grenze immer aus).
    pub wrap: bool,
    /// Minimap zeigen (ab der Großdatei-Grenze immer aus).
    pub minimap: bool,
    /// Nächste Nummer für „Unbenannt-N“.
    pub next_untitled: u32,
    /// Vergleichs-Reiter (#385); Reiter verweisen über `diff` darauf.
    pub diffs: Vec<SavedDiff>,
}

/// Ein Vergleich (#385). Seiten mit der Zwischenablage werden nicht gemerkt (ihr Inhalt gehört nicht auf die Platte).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct SavedDiff {
    pub id: u64,
    pub left: SavedSide,
    pub right: SavedSide,
    /// `auto`, `split`, `unified`.
    pub layout: String,
    pub ignore_ws: bool,
    pub context: u32,
    pub show_all: bool,
}

/// Eine Seite: ein Dokument (`doc`), die Datei auf der Platte (`disk`) oder ein Stand in git (`path` + `rev`:
/// `HEAD`, `:index` oder eine Revision).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct SavedSide {
    pub doc: Option<u64>,
    pub disk: Option<String>,
    pub path: Option<String>,
    pub rev: Option<String>,
}

impl Default for Saved {
    fn default() -> Self {
        Saved { version: VERSION, root: None, expanded: Vec::new(), recent_folders: Vec::new(), recent_files: Vec::new(), layout: None, focused: 0, groups: Vec::new(), docs: Vec::new(), tree_open: true, tree_width: 240.0, wrap: false, minimap: true, next_untitled: 1, diffs: Vec::new() }
    }
}

/// Ein offenes Dokument; Reiter verweisen über `key` darauf.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct SavedDoc {
    pub key: u64,
    /// Datei auf der Platte (`None` = unbenannt).
    pub path: Option<String>,
    /// Nummer von „Unbenannt-N“.
    pub untitled: u32,
    /// Schlüssel des Hot-Exit-Speichers, wenn es ungespeicherte Änderungen gab.
    pub hot: Option<String>,
    /// In der Statusleiste gewählte Sprache (#312; `None` = erkennen).
    pub language: Option<String>,
    /// In der Statusleiste gewählter Modus (`text`/`syntax`; `None` = aus den Einstellungen).
    pub mode: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct SavedGroup {
    pub id: u64,
    pub active: usize,
    pub tabs: Vec<SavedTab>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct SavedTab {
    pub doc: u64,
    /// Cursor (Zeile, Spalte in Zeichen, ab 0).
    pub line: usize,
    pub col: usize,
    /// Oberste sichtbare Zeile.
    pub top: f32,
    /// Vergleichs-Reiter (#385): Kennung in `Saved::diffs`, und das Dokument der zweiten Seite.
    pub diff: Option<u64>,
    pub extra: Option<u64>,
}

/// Liste der zuletzt geöffneten Einträge: `item` nach vorn, ohne Doppelte (Groß/Klein egal unter Windows), höchstens
/// [`RECENT_MAX`].
pub fn push_recent(list: &mut Vec<String>, item: &str) {
    let key = |s: &str| if cfg!(windows) { s.to_lowercase() } else { s.to_string() };
    let k = key(item);
    list.retain(|x| key(x) != k);
    list.insert(0, item.to_string());
    list.truncate(RECENT_MAX);
}

pub fn load(dir: &Path) -> Saved {
    std::fs::read_to_string(dir.join(FILE)).ok().and_then(|s| serde_json::from_str::<Saved>(crate::state::strip_bom(&s)).ok()).unwrap_or_default()
}

pub fn save(dir: &Path, s: &Saved) {
    if let Ok(b) = serde_json::to_vec_pretty(s) {
        let _ = crate::state::write_atomic(&dir.join(FILE), &b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Dir;

    #[test]
    fn round_trip_and_tolerant_loading() {
        let s = Saved {
            root: Some("C:\\proj".into()),
            recent_files: vec!["C:\\proj\\a.txt".into()],
            layout: Some(Node::Split { dir: Dir::Horizontal, ratio: 0.4, a: Box::new(Node::Leaf(1)), b: Box::new(Node::Leaf(2)) }),
            focused: 2,
            groups: vec![SavedGroup { id: 1, active: 0, tabs: vec![SavedTab { doc: 7, line: 3, col: 4, top: 1.5, ..Default::default() }, SavedTab { doc: 7, diff: Some(3), ..Default::default() }] }, SavedGroup { id: 2, active: 0, tabs: vec![SavedTab { doc: 7, ..Default::default() }] }],
            docs: vec![SavedDoc { key: 7, path: Some("C:\\proj\\a.txt".into()), untitled: 0, hot: Some("h1".into()), language: Some("rust".into()), mode: Some("syntax".into()) }],
            diffs: vec![SavedDiff { id: 3, left: SavedSide { path: Some("a.txt".into()), rev: Some("HEAD".into()), ..Default::default() }, right: SavedSide { doc: Some(7), ..Default::default() }, layout: "split".into(), ignore_ws: true, context: 5, show_all: false }],
            ..Default::default()
        };
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Saved>(&j).unwrap(), s);
        // Leere bzw. unbekannte Felder und ein kaputtes Layout: Standardwerte, der Rest bleibt.
        let t: Saved = serde_json::from_str(r#"{"root":"D:\\x","layout":{"Weird":1},"future":true}"#).unwrap();
        assert_eq!(t.root.as_deref(), Some("D:\\x"));
        assert!(t.layout.is_none());
        assert!(t.tree_open && t.minimap && !t.wrap);
        // Mit BOM (PowerShell 5.1) und kaputte Datei
        let dir = std::env::temp_dir().join(format!("pd-ed-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        save(&dir, &s);
        assert_eq!(load(&dir), s);
        std::fs::write(dir.join(FILE), "\u{feff}{\"root\":\"E:\\\\y\"}").unwrap();
        assert_eq!(load(&dir).root.as_deref(), Some("E:\\y"));
        std::fs::write(dir.join(FILE), "{kaputt").unwrap();
        assert_eq!(load(&dir), Saved::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recent_lists_move_to_front_without_duplicates() {
        let mut l = Vec::new();
        for i in 0..20 {
            push_recent(&mut l, &format!("f{i}"));
        }
        assert_eq!(l.len(), RECENT_MAX);
        assert_eq!(l[0], "f19");
        push_recent(&mut l, "f10");
        assert_eq!(l[0], "f10");
        assert_eq!(l.iter().filter(|x| *x == "f10").count(), 1);
        if cfg!(windows) {
            push_recent(&mut l, "F10");
            assert_eq!(l.iter().filter(|x| x.eq_ignore_ascii_case("f10")).count(), 1);
        }
    }
}
