//! Icon-Namen für Typen (die Zeichnung selbst kommt mit der Oberfläche): der feste Satz eingebauter Icons ([`ICONS`]) und
//! additiv die Lucide-Icons als `lucide:<name>` (Tabelle in [`lucide`], Auswahl nur im Icon-Explorer).

pub mod lucide;

pub const ICONS: &[&str] = &[
    "alert", "bolt", "bookmark", "bug", "check", "checkbox", "clipboard", "clock", "code", "cog", "comment", "compass", "document", "fire", "flag", "flask",
    "folder", "globe", "heart", "key", "layers", "lightbulb", "link", "list", "lock", "mail", "package", "puzzle", "rocket", "shield", "star",
    "subtask", "tag", "target", "terminal", "user", "wrench",
];

/// Vorsatz der gespeicherten Lucide-Namen, z. B. `lucide:alarm-clock`.
pub const LUCIDE_PREFIX: &str = "lucide:";

/// Name hinter `lucide:` (ohne Prüfung, ob es das Icon gibt); `None` für eingebaute oder andere Namen.
pub fn lucide_name(icon: &str) -> Option<&str> {
    icon.strip_prefix(LUCIDE_PREFIX)
}

/// Gespeicherter Wert für ein Lucide-Icon.
pub fn lucide_value(name: &str) -> String {
    format!("{LUCIDE_PREFIX}{name}")
}

/// Formal gültiger Lucide-Name: `kebab-case` aus a–z und 0–9, höchstens 64 Zeichen.
pub fn is_lucide_slug(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Gültiger Icon-Name für neue Eingaben (Oberfläche, MCP): ein eingebautes Icon oder `lucide:<name>` mit einem Icon, das es in
/// der eingebetteten Lucide-Version gibt (auch unter einem alten Namen; gespeichert wird der neue, siehe [`normalize`]).
pub fn is_valid(name: &str) -> bool {
    ICONS.contains(&name) || lucide_name(name).is_some_and(|n| lucide::canonical(n).is_some())
}

/// Gespeicherter Wert in aktueller Form: ein alter Lucide-Name (Alias) wird zum neuen (`lucide:alarm-check` →
/// `lucide:alarm-clock-check`), alles andere bleibt unverändert. Angewandt beim Lesen und Speichern von Typen.
pub fn normalize(name: &str) -> String {
    match lucide_name(name).and_then(lucide::canonical) {
        Some(c) if Some(c) != lucide_name(name) => lucide_value(c),
        _ => name.to_string(),
    }
}

/// Speicherbarer Icon-Name (Datenbank, Sicherung, Vorlagen-Import): wie [`is_valid`], aber ein formal richtiger `lucide:`-Name
/// wird auch angenommen, wenn es das Icon in dieser Version nicht gibt (z. B. aus einer neueren Paneldeck-Version) – er wird
/// dann als Platzhalter gezeichnet, statt den Import abzubrechen.
pub fn is_storable(name: &str) -> bool {
    ICONS.contains(&name) || lucide_name(name).is_some_and(is_lucide_slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_set_is_sane() {
        assert!(ICONS.len() >= 30);
        let mut sorted = ICONS.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ICONS.len(), "keine doppelten Icon-Namen");
        assert!(ICONS.iter().all(|n| n.chars().all(|c| c.is_ascii_lowercase())));
        assert!(is_valid("bug") && !is_valid("Bug") && !is_valid(""));
        // Eingebaute Namen sind nie zugleich Lucide-Werte.
        assert!(ICONS.iter().all(|n| lucide_name(n).is_none()));
    }

    #[test]
    fn lucide_names_are_valid_only_when_known_and_storable_when_well_formed() {
        assert!(is_valid("lucide:alarm-clock") && is_storable("lucide:alarm-clock"));
        assert_eq!(lucide_name("lucide:alarm-clock"), Some("alarm-clock"));
        assert_eq!(lucide_value("alarm-clock"), "lucide:alarm-clock");
        // Unbekannt, aber formal richtig: nicht für neue Eingaben, aber speicherbar (Platzhalter beim Zeichnen).
        assert!(!is_valid("lucide:not-an-icon-yet") && is_storable("lucide:not-an-icon-yet"));
        // Formal falsch: weder noch.
        for bad in ["lucide:", "lucide:Alarm", "lucide:alarm clock", "lucide:-x", "lucide:x-", "lucide:a--b", "lucide:ä", "Lucide:alarm-clock", "lucide:bug/../x"] {
            assert!(!is_valid(bad) && !is_storable(bad), "{bad}");
        }
        let long = format!("lucide:{}", "a".repeat(65));
        assert!(!is_storable(&long));
        // Ein eingebauter Name mit Vorsatz ist kein Lucide-Icon, nur weil er eingebaut existiert (es gibt ihn aber auch bei Lucide).
        assert!(is_valid("lucide:bug") == lucide::get("bug").is_some());
        assert!(!is_storable("nope") && is_storable("bug"));
    }

    #[test]
    fn old_lucide_names_are_valid_and_normalized_to_the_new_name() {
        assert!(is_valid("lucide:alarm-check") && is_storable("lucide:alarm-check"));
        assert_eq!(normalize("lucide:alarm-check"), "lucide:alarm-clock-check");
        assert_eq!(normalize("lucide:alarm-clock"), "lucide:alarm-clock");
        assert_eq!(normalize("lucide:not-an-icon-yet"), "lucide:not-an-icon-yet");
        assert_eq!(normalize("bug"), "bug");
        assert_eq!(normalize("lucide:Bad"), "lucide:Bad");
    }
}
