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

/// Standard-Prompts beim ersten Start, in der gewählten Sprache (Englisch ist der Standard).
pub fn defaults(lang: crate::state::Lang) -> Vec<Snippet> {
    let t = |de: &'static str, en: &'static str| lang.t(de, en).to_string();
    vec![
        Snippet { name: t("Weitermachen", "Continue"), text: t("Bitte mach an der Stelle weiter, an der du aufgehört hast.", "Please continue where you left off."), submit: true },
        Snippet { name: t("Tests ausführen & fixen", "Run tests & fix"), text: t("Führe die Tests aus und behebe alle Fehler.", "Run the tests and fix all failures."), submit: true },
        Snippet { name: t("Zusammenfassung", "Summary"), text: t("Fasse den aktuellen Stand und die nächsten Schritte kurz zusammen.", "Briefly summarize the current state and the next steps."), submit: true },
        Snippet { name: t("Code-Review", "Code review"), text: t("Reviewe die letzten Änderungen auf Fehler, Edge-Cases und Verbesserungen.", "Review the latest changes for bugs, edge cases and improvements."), submit: false },
    ]
}
