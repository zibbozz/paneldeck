//! Tastenkürzel: Standardbelegung, Parsen und Formatieren ("Ctrl+Shift+T").

use eframe::egui::{Key, Modifiers};

/// (Kennung, Standardkürzel)
pub const DEFAULTS: &[(&str, &str)] = &[
    ("new", "Ctrl+Shift+T"),
    ("split_r", "Ctrl+Shift+D"),
    ("split_d", "Ctrl+Shift+E"),
    ("hide", "Ctrl+Shift+W"),
    ("rename", "Ctrl+Shift+R"),
    ("palette", "Ctrl+Shift+P"),
    ("search", "Ctrl+Shift+F"),
    ("broadcast", "Ctrl+Shift+B"),
    ("notes", "Ctrl+Shift+N"),
    ("files", "Ctrl+Shift+O"),
    ("editor", "Ctrl+Shift+K"),
    ("diff", "Ctrl+Shift+H"),
    ("queue", "Ctrl+Shift+Q"),
    ("composer", "Ctrl+Shift+M"),
    ("search_all", "Ctrl+Shift+G"),
    ("prev_cmd", "Ctrl+Shift+Up"),
    ("next_cmd", "Ctrl+Shift+Down"),
    ("next", "Ctrl+Tab"),
    ("prev", "Ctrl+Shift+Tab"),
    // Reiterwechsel der Fensterleiste (nicht Strg+Alt+Zahl: das ist AltGr auf deutschen Tastaturen)
    ("mode_terminals", "Alt+Shift+1"),
    ("mode_tasks", "Alt+Shift+2"),
    // Zweite Belegung ohne Alt+Shift (Windows schaltet damit die Eingabesprache um); Strg+1/2 verbraucht der Hub; Strg+3…0 gehen ans Terminal (win32-input-mode, #183)
    ("mode_terminals_alt", "Ctrl+1"),
    ("mode_tasks_alt", "Ctrl+2"),
    // Alle Sitzungen schließen (#156): standardmäßig ohne Kürzel (leer = keins)
    ("close_all", ""),
    // Sitzung neu laden (#169): standardmäßig ohne Kürzel
    ("reload", ""),
];

/// Kürzel, die in jedem Bereich gelten (Reiterwechsel) – die übrigen nur im Terminal-Bereich.
pub fn is_global(id: &str) -> bool {
    id.starts_with("mode_")
}

pub fn label(id: &str, de: bool) -> &'static str {
    let (d, e) = match id {
        "new" => ("Neue Sitzung", "New session"),
        "split_r" => ("Bereich rechts teilen", "Split pane right"),
        "split_d" => ("Bereich darunter teilen", "Split pane below"),
        "hide" => ("Bereich minimieren", "Minimize pane"),
        "rename" => ("Sitzung umbenennen", "Rename session"),
        "palette" => ("Befehle (Palette)", "Commands (palette)"),
        "search" => ("Im Verlauf suchen", "Search scrollback"),
        "broadcast" => ("Broadcast an/aus", "Toggle broadcast"),
        "notes" => ("Notizen", "Notes"),
        "files" => ("Dateibaum", "File tree"),
        "editor" => ("CLAUDE.md / Claude-Dateien", "CLAUDE.md / Claude files"),
        "diff" => ("Änderungen (Diff)", "Changes (diff)"),
        "queue" => ("Warteschlange der Sitzung", "Session queue"),
        "composer" => ("Prompt-Composer", "Prompt composer"),
        "search_all" => ("Alle Sitzungen durchsuchen", "Search all sessions"),
        "prev_cmd" => ("Zum vorherigen Befehl springen", "Jump to previous command"),
        "next_cmd" => ("Zum nächsten Befehl springen", "Jump to next command"),
        "next" => ("Nächste Sitzung", "Next session"),
        "prev" => ("Vorherige Sitzung", "Previous session"),
        "mode_terminals" => ("Reiter „Terminal Hub“", "“Terminal Hub” tab"),
        "mode_tasks" => ("Reiter „Todo-Liste“", "“Todo list” tab"),
        "mode_terminals_alt" => ("Reiter „Terminal Hub“ (zweite Belegung)", "“Terminal Hub” tab (alternative)"),
        "mode_tasks_alt" => ("Reiter „Todo-Liste“ (zweite Belegung)", "“Todo list” tab (alternative)"),
        "close_all" => ("Alle Sitzungen schließen", "Close all sessions"),
        "reload" => ("Sitzung neu laden", "Reload session"),
        _ => ("?", "?"),
    };
    if de { d } else { e }
}

pub fn parse(s: &str) -> Option<(Modifiers, Key)> {
    let mut m = Modifiers::NONE;
    let mut key = None;
    for part in s.split('+').map(|p| p.trim()) {
        match part.to_lowercase().as_str() {
            "ctrl" | "strg" => m.ctrl = true,
            "shift" | "umschalt" => m.shift = true,
            "alt" => m.alt = true,
            "" => {}
            _ => key = Key::from_name(part),
        }
    }
    key.map(|k| (m, k))
}

pub fn format(m: Modifiers, k: Key) -> String {
    let mut v = Vec::new();
    if m.ctrl {
        v.push("Ctrl");
    }
    if m.alt {
        v.push("Alt");
    }
    if m.shift {
        v.push("Shift");
    }
    v.push(k.name());
    v.join("+")
}

/// Taugt die Kombination als Kürzel? Nur mit Strg oder Alt – oder eine F-Taste (auch allein). Eine einfache Taste
/// (`A`, Umschalt+A, Enter …) finge sonst jede solche Eingabe in allen Terminals ab (#256).
pub fn usable(m: Modifiers, k: Key) -> bool {
    m.ctrl || m.alt || m.command || is_f_key(k)
}

fn is_f_key(k: Key) -> bool {
    let n = k.name();
    n.len() >= 2 && n.starts_with('F') && n[1..].chars().all(|c| c.is_ascii_digit())
}

/// Anzeige im Menü/Tooltip (deutsch: Strg/Umschalt).
pub fn pretty(s: &str, de: bool) -> String {
    if de {
        s.replace("Ctrl", "Strg").replace("Shift", "Umschalt")
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        for (_, d) in DEFAULTS.iter().filter(|(_, d)| !d.is_empty()) {
            let (m, k) = parse(d).unwrap_or_else(|| panic!("parse {d}"));
            assert_eq!(&format(m, k), d);
        }
        assert!(parse("Ctrl+Shift+").is_none());
        // Reihenfolge der Modifikatoren beim Parsen egal, beim Formatieren Ctrl, Alt, Shift
        let (m, k) = parse("Shift+Alt+1").unwrap();
        assert!(m.alt && m.shift && !m.ctrl);
        assert_eq!(format(m, k), "Alt+Shift+1");
        assert_eq!(parse("Alt+Shift+1"), parse("Shift+Alt+1"));
    }

    #[test]
    fn defaults_are_unique_and_labelled() {
        let mut seen = std::collections::HashSet::new();
        for (id, d) in DEFAULTS {
            assert!(d.is_empty() || seen.insert(*d), "Kürzel {d} doppelt");
            assert_ne!(label(id, true), "?", "{id} ohne Beschriftung");
            assert_ne!(label(id, true), label(id, false), "{id}: Texte gleich in beiden Sprachen");
        }
        assert!(is_global("mode_tasks") && is_global("mode_terminals") && !is_global("new"));
    }

    #[test]
    fn plain_keys_are_not_usable_as_shortcuts() {
        for s in ["A", "Shift+A", "Enter", "Shift+Enter", "Space", "1", "Shift+1", "Escape", "Tab"] {
            let (m, k) = parse(s).unwrap_or_else(|| panic!("parse {s}"));
            assert!(!usable(m, k), "{s} darf kein Kürzel sein");
        }
        for s in ["Ctrl+A", "Alt+A", "Ctrl+Shift+T", "F2", "Shift+F5", "F12", "Ctrl+F1", "Alt+Shift+1"] {
            let (m, k) = parse(s).unwrap_or_else(|| panic!("parse {s}"));
            assert!(usable(m, k), "{s} ist ein gültiges Kürzel");
        }
        for (_, d) in DEFAULTS.iter().filter(|(_, d)| !d.is_empty()) {
            let (m, k) = parse(d).unwrap();
            assert!(usable(m, k), "Standard {d} muss gültig sein");
        }
    }

    #[test]
    fn close_all_has_no_default_shortcut() {
        assert_eq!(DEFAULTS.iter().find(|(id, _)| *id == "close_all").map(|(_, d)| *d), Some(""));
        assert!(parse("").is_none(), "leeres Kürzel löst nie aus");
        assert!(!is_global("close_all"));
    }
}
