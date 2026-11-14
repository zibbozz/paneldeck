//! Fester Satz einfacher Icon-Namen für Typen (die Zeichnung selbst kommt mit der Oberfläche).

pub const ICONS: &[&str] = &[
    "bolt", "bookmark", "bug", "check", "checkbox", "clipboard", "clock", "code", "cog", "comment", "compass", "document", "fire", "flag", "flask",
    "folder", "globe", "heart", "key", "layers", "lightbulb", "link", "list", "lock", "mail", "package", "puzzle", "rocket", "shield", "star",
    "subtask", "tag", "target", "terminal", "user", "wrench",
];

pub fn is_valid(name: &str) -> bool {
    ICONS.contains(&name)
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
    }
}
