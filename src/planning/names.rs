//! Übersetzbare Standardnamen. Ein Eintrag mit gesetztem `key` und leerem `name` wird in der Sprache der Oberfläche
//! angezeigt; wer umbenennt, setzt `name` (gilt dann für beide Sprachen), der `key` bleibt.
//!
//! Die Schlüssel sind ein gemeinsamer Namensraum: Status `todo`/`progress`/`done`, Typen, Felder, Dropdown-Werte
//! (`priority.medium`), Verknüpfungsnamen (`link.blocks.out`/`link.blocks.in`) und Arten der Arbeit.

use crate::state::Lang;

/// (Schlüssel, Deutsch, Englisch)
pub const NAMES: &[(&str, &str, &str)] = &[
    // Status
    ("todo", "Zu erledigen", "To Do"),
    ("progress", "In Arbeit", "In Progress"),
    ("done", "Erledigt", "Done"),
    // Typen
    ("epic", "Epic", "Epic"),
    ("userstory", "Userstory", "User story"),
    ("devtask", "Dev-Aufgabe", "Dev task"),
    ("subtask", "Unteraufgabe", "Subtask"),
    ("defect", "Fehler", "Defect"),
    ("todolist", "Todolist", "Todolist"),
    // Felder
    ("description", "Beschreibung", "Description"),
    ("priority", "Priorität", "Priority"),
    ("story_points", "Story Points", "Story points"),
    ("release_version", "Release-Version", "Release version"),
    ("milestone", "Meilenstein", "Milestone"),
    ("steps", "Schritte zur Reproduktion", "Steps to reproduce"),
    ("parent", "Übergeordnet", "Parent"),
    // Dropdown-Werte der Priorität (Standardwert: Medium bzw. „Normal“)
    ("priority.highest", "Höchste", "Highest"),
    ("priority.high", "Hoch", "High"),
    ("priority.medium", "Normal", "Medium"),
    ("priority.low", "Niedrig", "Low"),
    ("priority.lowest", "Niedrigste", "Lowest"),
    // Verknüpfungstypen (Hin- und Rückrichtung)
    ("link.blocks.out", "blockiert", "blocks"),
    ("link.blocks.in", "ist blockiert durch", "is blocked by"),
    ("link.relates.out", "ist in Beziehung mit", "relates to"),
    ("link.relates.in", "ist in Beziehung mit", "relates to"),
    ("link.duplicates.out", "ist Duplikat von", "duplicates"),
    ("link.duplicates.in", "hat Duplikat", "is duplicated by"),
    ("link.terminal.out", "Terminal", "Terminal"),
    // Arten der Arbeit
    ("development", "Entwicklung", "Development"),
    ("testing", "Test", "Testing"),
    ("planning", "Planung", "Planning"),
    ("refinement", "Schärfung", "Refinement"),
];

/// Übersetzung eines Standard-Schlüssels, falls bekannt.
pub fn translate(key: &str, lang: Lang) -> Option<&'static str> {
    NAMES.iter().find(|(k, _, _)| *k == key).map(|(_, de, en)| match lang {
        Lang::De => *de,
        Lang::En => *en,
    })
}

/// Anzeigename: ein eigener Name (`name`) gilt in beiden Sprachen; sonst die Übersetzung des Schlüssels; ein unbekannter
/// Schlüssel wird unverändert gezeigt; ohne beides ist der Text leer.
pub fn display_name(key: Option<&str>, name: Option<&str>, lang: Lang) -> String {
    if let Some(n) = name {
        if !n.trim().is_empty() {
            return n.to_string();
        }
    }
    match key {
        Some(k) => translate(k, lang).map(str::to_string).unwrap_or_else(|| k.to_string()),
        None => String::new(),
    }
}

/// Namen einer Verknüpfung: (Hinrichtung, Rückrichtung). Ohne Rückrichtung (z. B. `terminal`) ist der zweite Wert `None`;
/// bei symmetrischen Typen gilt die Hinrichtung für beide Seiten.
pub fn link_names(key: Option<&str>, name_out: Option<&str>, name_in: Option<&str>, symmetric: bool, has_reverse: bool, lang: Lang) -> (String, Option<String>) {
    let out_key = key.map(|k| format!("link.{k}.out"));
    let in_key = key.map(|k| format!("link.{k}.in"));
    let out = display_name(out_key.as_deref(), name_out, lang);
    if symmetric {
        return (out.clone(), Some(out));
    }
    if !has_reverse {
        return (out, None);
    }
    (out, Some(display_name(in_key.as_deref(), name_in, lang)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique() {
        let mut keys: Vec<_> = NAMES.iter().map(|n| n.0).collect();
        keys.sort();
        let len = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), len);
    }

    #[test]
    fn translates_in_both_languages_and_renaming_overrides() {
        assert_eq!(display_name(Some("todo"), None, Lang::En), "To Do");
        assert_eq!(display_name(Some("todo"), None, Lang::De), "Zu erledigen");
        assert_eq!(display_name(Some("priority.medium"), None, Lang::De), "Normal");
        assert_eq!(display_name(Some("priority.medium"), None, Lang::En), "Medium");
        assert_eq!(display_name(Some("userstory"), None, Lang::De), "Userstory");
        assert_eq!(display_name(Some("userstory"), None, Lang::En), "User story");
        // eigener Name gilt für beide Sprachen, der Schlüssel bleibt
        assert_eq!(display_name(Some("todo"), Some("Backlog"), Lang::De), "Backlog");
        assert_eq!(display_name(Some("todo"), Some("Backlog"), Lang::En), "Backlog");
        // leerer Name zählt als nicht gesetzt
        assert_eq!(display_name(Some("done"), Some("  "), Lang::En), "Done");
        // ohne Schlüssel nur der Name; unbekannter Schlüssel unverändert; nichts gesetzt = leer
        assert_eq!(display_name(None, Some("Eigen"), Lang::De), "Eigen");
        assert_eq!(display_name(Some("unbekannt"), None, Lang::De), "unbekannt");
        assert_eq!(display_name(None, None, Lang::En), "");
    }

    #[test]
    fn link_names_follow_the_language() {
        assert_eq!(link_names(Some("blocks"), None, None, false, true, Lang::En), ("blocks".into(), Some("is blocked by".into())));
        assert_eq!(link_names(Some("blocks"), None, None, false, true, Lang::De), ("blockiert".into(), Some("ist blockiert durch".into())));
        assert_eq!(link_names(Some("duplicates"), None, None, false, true, Lang::De), ("ist Duplikat von".into(), Some("hat Duplikat".into())));
        assert_eq!(link_names(Some("relates"), None, None, true, true, Lang::De), ("ist in Beziehung mit".into(), Some("ist in Beziehung mit".into())));
        assert_eq!(link_names(Some("relates"), None, None, true, true, Lang::En), ("relates to".into(), Some("relates to".into())));
        // terminal: keine Rückrichtung
        assert_eq!(link_names(Some("terminal"), None, None, false, false, Lang::En), ("Terminal".into(), None));
        // eigener Typ
        assert_eq!(link_names(None, Some("klont"), Some("ist Klon von"), false, true, Lang::En), ("klont".into(), Some("ist Klon von".into())));
    }
}
