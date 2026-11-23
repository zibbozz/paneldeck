//! Eingebaute Formatierer für JSON, JSONC und XML (#381, 3.2.0) – ohne egui, ohne Netzwerk.
//!
//! Gemeinsame Schnittstelle für die Menüleiste des Editors („Format“), Tastenkürzel, Befehls-Palette, Formatieren beim
//! Speichern und das MCP-Werkzeug `format_file`. Die Arbeit selbst läuft in einem Hintergrund-Thread des Aufrufers:
//! [`run`] meldet Fortschritt (0.0–1.0) und bricht ab, sobald `cancel` gesetzt ist.
//!
//! - [`json`]: JSON und JSONC (Kommentare `//`, `/* */` und Komma am Ende bleiben beim Formatieren erhalten).
//! - [`xml`]: behutsames Einrücken; Kommentare, CDATA, Processing Instructions, DOCTYPE, gemischter Inhalt und
//!   `xml:space="preserve"` bleiben unverändert, Attribute bleiben in einer Zeile.
//!
//! Die spätere Formatierung über Sprachserver (#367/#368) ergänzt das, ersetzt es nicht.

pub mod json;
pub mod xml;

use std::path::Path;
use std::sync::atomic::AtomicBool;

/// Art des Inhalts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Json,
    /// JSON mit Kommentaren und Komma am Ende (`.jsonc`, `tsconfig.json`, `.vscode/*.json` …).
    Jsonc,
    Xml,
}

/// Befehl.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// Hübsch einrücken.
    Format,
    /// Leerraum entfernen (JSONC: Kommentare fallen dabei weg – der Aufrufer fragt vorher nach).
    Minify,
    /// Nur prüfen; bei Erfolg kommt der Text unverändert zurück.
    Validate,
    /// Objektschlüssel rekursiv alphabetisch sortieren (nur JSON/JSONC), danach formatiert.
    SortKeys,
}

/// Einrückung (aus `.editorconfig` bzw. den Einstellungen der Datei).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Indent {
    Spaces(u8),
    Tab,
}

/// Optionen für einen Lauf.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    pub indent: Indent,
    /// Zeilenumbruch der Ausgabe ist immer `\n` (das Dokument speichert intern nur `\n`); hier nur, ob am Ende ein
    /// Zeilenumbruch stehen soll (wie im Original).
    pub final_newline: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { indent: Indent::Spaces(2), final_newline: true }
    }
}

/// Fehler mit Position (1-basiert, Spalte in Zeichen) für „Prüfen“ und gescheitertes Formatieren.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatError {
    pub line: usize,
    pub column: usize,
    /// Englische Meldung des Parsers (die Oberfläche setzt einen übersetzten Rahmen darum).
    pub message: String,
}

/// Ergebnis eines Laufs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Neuer Text (bei `Validate` der unveränderte Eingabetext).
    Text(String),
    /// Ungültige Eingabe – nichts wurde verändert.
    Invalid(FormatError),
    /// Abgebrochen über `cancel`.
    Cancelled,
}

/// Art aus Pfad und Inhalt erkennen: `.json` → Json (Jsonc, wenn bekannter Dateiname oder Kommentare vorkommen),
/// `.jsonc`/`.json5`-artige Namen → Jsonc, `.xml`/`.xsd`/`.xsl`/`.xslt`/`.svg`/`.csproj`/`.props`/`.targets`/`.resx`/
/// `.plist`/`.xaml`/`.config` (wenn XML) → Xml; ohne passende Endung anhand des Inhalts (`{`/`[` bzw. `<?xml`/`<`).
/// `None`, wenn es weder JSON noch XML ist.
pub fn detect(path: Option<&Path>, text: &str) -> Option<Kind> {
    let name = path.and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_string()).unwrap_or_default();
    let head = text.trim_start_matches('\u{feff}').trim_start();
    let jsonc_name = name == "tsconfig.json"
        || name == "jsconfig.json"
        || (name.starts_with("tsconfig.") && name.ends_with(".json"))
        || path.and_then(|p| p.parent()).and_then(|d| d.file_name()).is_some_and(|d| d.eq_ignore_ascii_case(".vscode"));
    match ext.as_str() {
        "jsonc" => Some(Kind::Jsonc),
        "json" => Some(if jsonc_name || json::has_comments(text) { Kind::Jsonc } else { Kind::Json }),
        "xml" | "xsd" | "xsl" | "xslt" | "svg" | "csproj" | "vbproj" | "fsproj" | "props" | "targets" | "resx" | "plist" | "xaml" | "nuspec" => Some(Kind::Xml),
        "config" if head.starts_with('<') => Some(Kind::Xml),
        _ if head.starts_with("<?xml") => Some(Kind::Xml),
        _ if head.starts_with('{') || head.starts_with('[') => Some(if json::has_comments(text) { Kind::Jsonc } else { Kind::Json }),
        _ => None,
    }
}

/// Befehl ausführen. `progress` bekommt Werte von 0.0 bis 1.0 (höchstens grob, z. B. je 1 % der Eingabe).
/// Läuft im Hintergrund-Thread des Aufrufers; prüft `cancel` regelmäßig.
pub fn run(kind: Kind, op: Op, text: &str, opts: Options, cancel: &AtomicBool, progress: &dyn Fn(f32)) -> Outcome {
    match kind {
        Kind::Json | Kind::Jsonc => json::run(kind == Kind::Jsonc, op, text, opts, cancel, progress),
        Kind::Xml => xml::run(op, text, opts, cancel, progress),
    }
}

/// Welche Befehle gibt es für diese Art? (Für das Menü: nicht passende Einträge werden ausgegraut.)
pub fn supports(kind: Kind, op: Op) -> bool {
    !(kind == Kind::Xml && op == Op::SortKeys)
}
