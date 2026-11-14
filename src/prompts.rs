//! Prompt-Bibliothek (prompts.json im Datenordner).

use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Snippet {
    pub name: String,
    pub text: String,
    /// Nach dem Einfügen automatisch Enter senden.
    pub submit: bool,
}

pub fn defaults() -> Vec<Snippet> {
    vec![
        Snippet { name: "Weitermachen".into(), text: "Bitte mach an der Stelle weiter, an der du aufgehört hast.".into(), submit: true },
        Snippet { name: "Tests ausführen & fixen".into(), text: "Führe die Tests aus und behebe alle Fehler.".into(), submit: true },
        Snippet { name: "Zusammenfassung".into(), text: "Fasse den aktuellen Stand und die nächsten Schritte kurz zusammen.".into(), submit: true },
        Snippet { name: "Code-Review".into(), text: "Reviewe die letzten Änderungen auf Fehler, Edge-Cases und Verbesserungen.".into(), submit: false },
    ]
}
