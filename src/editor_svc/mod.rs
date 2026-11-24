//! Dienste des Editor-Reiters (#54, 3.0.0), ohne egui und ohne App:
//!
//! * [`settings`]: *Einstellungen → Editor* (`settings.json` `editor`, öffentliches Datenformat, nur additiv).
//! * [`agent_path`]: welche Pfade ein Agent öffnen darf (lokal, nie Netzwerk-/UNC-Pfade oder Geräte).
//! * [`tools`]: die MCP-Werkzeuge `editor_list`, `editor_read`, `editor_open`, `editor_edit`, `editor_save` als reine
//!   Logik gegen die Schnittstelle [`tools::EditorHost`] (die Oberfläche setzt sie um, Tests mit einem Ersatz).
//! * [`format_job`]: ein Formatier-Lauf (JSON/XML, #381) im Hintergrund mit Fortschritt und Abbruch; liefert Änderungen
//!   als einen Rückgängig-Schritt (Menüleiste, Kürzel, Palette, Formatieren beim Speichern, MCP `format_file`).
//! * [`find`]: Suche in Dateien des geöffneten Ordners (Hintergrund-Thread, abbrechbar, gedeckelt; `.gitignore`).
//! * [`watch`]: Änderungen geöffneter Dateien auf der Platte erkennen (Abfrage von Größe/Zeitstempel im Hintergrund).
//!
//! Der Kern des Editors (Dokument, Bearbeiten, Speichern, Hot-Exit) liegt in `crate::editor`, die Oberfläche in `app`.

pub mod agent_path;
pub mod find;
pub mod format_job;
pub mod settings;
pub mod tools;
pub mod watch;
