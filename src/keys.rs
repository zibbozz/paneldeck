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
    ("mode_editor", "Alt+Shift+3"),
    // Zweite Belegung ohne Alt+Shift (Windows schaltet damit die Eingabesprache um); Strg+1/2 verbraucht der Hub; Strg+3…0 gehen ans Terminal (win32-input-mode, #183)
    ("mode_terminals_alt", "Ctrl+1"),
    ("mode_tasks_alt", "Ctrl+2"),
    // Editor (#54): zweite Belegung standardmäßig leer – Strg+3 gehört dem Terminal (#183).
    ("mode_editor_alt", ""),
    // Reiter „Zeit“ (#164): Alt+Shift+4; zweite Belegung leer (Strg+4 gehört dem Terminal, #183).
    ("mode_time", "Alt+Shift+4"),
    ("mode_time_alt", ""),
    // Alle Sitzungen schließen (#156): standardmäßig ohne Kürzel (leer = keins)
    ("close_all", ""),
    // Sitzung neu laden (#169): standardmäßig ohne Kürzel
    ("reload", ""),
    // Editor: Formatieren (#381). Nicht Strg+Alt+Buchstabe – das ist AltGr auf vielen Tastaturen (AltGr+M = µ auf
    // deutschen); Alt+Umschalt+F wie „Dokument formatieren“ in VS Code. Schlüssel sortieren ohne Standard.
    ("ed_format", "Alt+Shift+F"),
    ("ed_minify", "Alt+Shift+M"),
    ("ed_validate", "Alt+Shift+V"),
    ("ed_sort_keys", ""),
    // Zweite Belegung (wie bei `mode_*`): Alt+Umschalt schaltet bei mehreren Eingabesprachen die Sprache um. Strg+Umschalt+I
    // wie „Dokument formatieren“ in VS Code unter Linux.
    ("ed_format_alt", "Ctrl+Shift+I"),
    ("ed_minify_alt", ""),
    ("ed_validate_alt", ""),
    // Editor: Ohne Aktionen speichern (#383): standardmäßig ohne Kürzel
    ("save_no_actions", ""),
    // Editor (#385): nächste/vorige Änderung im Vergleich und bei den Änderungsmarken – nur im Editor-Reiter.
    ("ed_next_change", "F7"),
    ("ed_prev_change", "Shift+F7"),
    // Editor: mehrere Cursor (#394, 3.3.0) – wie Sublime Text; wirken nur mit dem Schalter *Mehrere Cursor*
    // (Einstellungen → Editor, Standard aus). Strg+Umschalt+D teilt im Terminal Hub weiter rechts (`split_r`), im Editor
    // markiert es mit dem Schalter alle Vorkommen.
    ("ed_next_occurrence", "Ctrl+D"),
    ("ed_all_occurrences", "Ctrl+Shift+D"),
    ("ed_soft_undo", "Ctrl+U"),
    // Kommentar umschalten (#394) mit jedem Cursor; auf deutschen Tastaturen liegt „/“ auf Umschalt+7 (zweite Belegung
    // frei, z. B. Strg+#).
    ("ed_toggle_comment", "Ctrl+Slash"),
    ("ed_toggle_comment_alt", ""),
    // Smart-Modus (#367): Vervollständigen (für IME-Nutzer umbelegbar), Problemliste, nächstes/voriges Problem.
    ("ed_complete", "Ctrl+Space"),
    ("ed_problems", "Ctrl+Shift+M"),
    ("ed_next_problem", "F8"),
    ("ed_prev_problem", "Shift+F8"),
];

/// Kürzel, die im Editor eine andere Belegung des Terminal Hub überdecken dürfen (#394: Strg+Umschalt+D markiert im
/// Editor alle Vorkommen, im Terminal Hub teilt es rechts).
pub const EDITOR_OVERRIDES: &[(&str, &str)] = &[("ed_all_occurrences", "split_r"), ("ed_problems", "composer")];

/// Kürzel, die in jedem Bereich gelten (Reiterwechsel) – die übrigen nur im Terminal-Bereich.
pub fn is_global(id: &str) -> bool {
    id.starts_with("mode_")
}

/// Kürzel nur des Editor-Reiters (Formatieren #381, *Ohne Aktionen speichern* #383, …) – dort ausgewertet; der
/// Terminal-Bereich lässt diese Tasten den Terminals.
pub fn is_editor(id: &str) -> bool {
    id.starts_with("ed_") || id == "save_no_actions"
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
        "mode_editor" => ("Reiter „Editor“", "“Editor” tab"),
        "mode_editor_alt" => ("Reiter „Editor“ (zweite Belegung)", "“Editor” tab (alternative)"),
        "mode_time" => ("Reiter „Zeit“", "“Time” tab"),
        "mode_time_alt" => ("Reiter „Zeit“ (zweite Belegung)", "“Time” tab (alternative)"),
        "close_all" => ("Alle Sitzungen schließen", "Close all sessions"),
        "reload" => ("Sitzung neu laden", "Reload session"),
        "ed_format" => ("Editor: JSON/XML formatieren", "Editor: format JSON/XML"),
        "ed_minify" => ("Editor: JSON/XML minifizieren", "Editor: minify JSON/XML"),
        "ed_validate" => ("Editor: JSON/XML prüfen", "Editor: validate JSON/XML"),
        "ed_sort_keys" => ("Editor: JSON-Schlüssel sortieren", "Editor: sort JSON keys"),
        "ed_format_alt" => ("Editor: JSON/XML formatieren (zweite Belegung)", "Editor: format JSON/XML (alternative)"),
        "ed_minify_alt" => ("Editor: JSON/XML minifizieren (zweite Belegung)", "Editor: minify JSON/XML (alternative)"),
        "ed_validate_alt" => ("Editor: JSON/XML prüfen (zweite Belegung)", "Editor: validate JSON/XML (alternative)"),
        "save_no_actions" => ("Editor: Ohne Aktionen speichern", "Editor: Save without actions"),
        "ed_next_change" => ("Editor: nächste Änderung", "Editor: next change"),
        "ed_prev_change" => ("Editor: vorige Änderung", "Editor: previous change"),
        "ed_next_occurrence" => ("Editor: nächstes Vorkommen dazu (mehrere Cursor)", "Editor: add next occurrence (multiple cursors)"),
        "ed_all_occurrences" => ("Editor: alle Vorkommen markieren (mehrere Cursor)", "Editor: select all occurrences (multiple cursors)"),
        "ed_soft_undo" => ("Editor: Cursor/Auswahl zurücknehmen", "Editor: undo cursor/selection change"),
        "ed_toggle_comment" => ("Editor: Kommentar umschalten", "Editor: toggle comment"),
        "ed_toggle_comment_alt" => ("Editor: Kommentar umschalten (zweite Belegung)", "Editor: toggle comment (alternative)"),
        "ed_complete" => ("Editor: Vervollständigen (Smart-Modus)", "Editor: complete (smart mode)"),
        "ed_problems" => ("Editor: Problemliste ein/aus", "Editor: toggle problem list"),
        "ed_next_problem" => ("Editor: nächstes Problem", "Editor: next problem"),
        "ed_prev_problem" => ("Editor: voriges Problem", "Editor: previous problem"),
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

/// Zeichen-Taste ohne Umschalt, die viele Belegungen nur mit Umschalt liefern (`!`, `?`, `:`, `|`, `{`, `}`, `+` auf
/// US-Tastaturen)? Bis 2.3.1 löste ein Kürzel auch mit zusätzlichem Umschalt aus, seit #254 nur mit genau diesen
/// Modifikatoren – eine von Hand eingetragene Belegung wie `Ctrl+?` wirkt dann nur, wenn die Tastatur das Zeichen ohne
/// Umschalt liefert (Hinweis in den Einstellungen).
pub fn may_need_shift(m: Modifiers, k: Key) -> bool {
    !m.shift && matches!(k, Key::Exclamationmark | Key::Questionmark | Key::Colon | Key::Pipe | Key::OpenCurlyBracket | Key::CloseCurlyBracket | Key::Plus)
}

fn is_f_key(k: Key) -> bool {
    let n = k.name();
    n.len() >= 2 && n.starts_with('F') && n[1..].chars().all(|c| c.is_ascii_digit())
}

/// Anzeige im Menü/Tooltip (deutsch: Strg/Umschalt); Satzzeichen-Tasten als Zeichen („Strg+/“ statt „Strg+Slash“).
pub fn pretty(s: &str, de: bool) -> String {
    let s = if de { s.replace("Ctrl", "Strg").replace("Shift", "Umschalt") } else { s.to_string() };
    s.split('+')
        .map(|t| match t {
            "Slash" => "/",
            "Backslash" => "\\",
            "Minus" => "-",
            "Equals" => "=",
            "Comma" => ",",
            "Period" => ".",
            "Semicolon" => ";",
            other => other,
        })
        .collect::<Vec<_>>()
        .join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// *Ohne Aktionen speichern* gilt nur im Editor-Reiter (im Terminal-Bereich gehören die Tasten dem Terminal).
    #[test]
    fn editor_only_shortcuts() {
        assert!(is_editor("save_no_actions") && !is_global("save_no_actions"));
        assert!(!is_editor("new") && label("save_no_actions", false) != "?");
    }

    /// Menü und Tastenkürzel-Seite zeigen „Strg+/“, nicht den egui-Namen „Slash“ (Klicktest 3.3.0).
    #[test]
    fn pretty_shows_punctuation_keys_as_characters() {
        assert_eq!(pretty("Ctrl+Slash", true), "Strg+/");
        assert_eq!(pretty("Ctrl+Shift+Minus", false), "Ctrl+Shift+-");
        assert_eq!(pretty("Alt+Up", true), "Alt+Up");
        assert_eq!(pretty("Ctrl+Shift+D", true), "Strg+Umschalt+D");
        assert_eq!(pretty("", true), "");
    }

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
            // Erlaubte Überdeckung im Editor (#394): Strg+Umschalt+D.
            if EDITOR_OVERRIDES.iter().any(|(e, _)| e == id) {
                continue;
            }
            assert!(d.is_empty() || seen.insert(*d), "Kürzel {d} doppelt");
            assert_ne!(label(id, true), "?", "{id} ohne Beschriftung");
            assert_ne!(label(id, true), label(id, false), "{id}: Texte gleich in beiden Sprachen");
        }
        assert!(is_global("mode_tasks") && is_global("mode_terminals") && !is_global("new"));
        assert!(is_editor("ed_format") && !is_editor("editor") && !is_global("ed_format"));
    }

    /// #381: Die Editor-Kürzel kollidieren nicht mit festen Kürzeln des Editors und nicht mit AltGr (Strg+Alt).
    #[test]
    fn editor_format_keys_avoid_altgr_and_fixed_editor_keys() {
        // Strg+D und Strg+Umschalt+L sind seit #394 keine festen Kürzel mehr (Strg+D ist `ed_next_occurrence`).
        let fixed = ["Ctrl+N", "Ctrl+O", "Ctrl+Shift+O", "Ctrl+S", "Ctrl+Shift+S", "Ctrl+W", "Ctrl+F4", "Ctrl+B", "Alt+Z", "Ctrl+F", "Ctrl+H", "Ctrl+G", "Ctrl+Shift+F", "Ctrl+Z", "Ctrl+Y", "Ctrl+A", "Ctrl+L", "Ctrl+Shift+K", "Alt+Up", "Alt+Down"];
        for (id, d) in DEFAULTS.iter().filter(|(id, d)| is_editor(id) && !d.is_empty()) {
            let (m, _) = parse(d).unwrap();
            assert!(!(m.ctrl && m.alt), "{id}: Strg+Alt ist AltGr");
            assert!(!fixed.contains(d), "{id}: {d} gehört schon dem Editor");
        }
        assert_eq!(DEFAULTS.iter().find(|(id, _)| *id == "ed_format").map(|(_, d)| *d), Some("Alt+Shift+F"));
        // Zweite Belegung ohne Alt+Umschalt (Umschalten der Eingabesprache, Review #381).
        let alt = DEFAULTS.iter().find(|(id, _)| *id == "ed_format_alt").map(|(_, d)| *d).unwrap();
        let (m, _) = parse(alt).unwrap();
        assert!(!(m.alt && m.shift) && is_editor("ed_format_alt"));
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

    /// Review zu #254: von Hand eingetragene Zeichen-Kürzel, die nur mit dem alten zusätzlichen Umschalt auslösten.
    #[test]
    fn symbol_keys_without_shift_get_a_hint() {
        for s in ["Ctrl+?", "Ctrl+Questionmark", "Alt+!", "Ctrl+Plus", "Ctrl+:", "Ctrl+|"] {
            let (m, k) = parse(s).unwrap_or_else(|| panic!("parse {s}"));
            assert!(may_need_shift(m, k), "{s}");
        }
        for s in ["Ctrl+Shift+?", "Ctrl+Shift+Plus", "Ctrl+Minus", "Ctrl+A", "Ctrl+Shift+T", "Alt+1"] {
            let (m, k) = parse(s).unwrap_or_else(|| panic!("parse {s}"));
            assert!(!may_need_shift(m, k), "{s}");
        }
        for (_, d) in DEFAULTS.iter().filter(|(_, d)| !d.is_empty()) {
            let (m, k) = parse(d).unwrap();
            assert!(!may_need_shift(m, k), "Standard {d} ohne Hinweis");
        }
    }

    /// #394: die Kürzel der mehreren Cursor sind belegbar, Strg+Umschalt+D überdeckt im Editor nur `split_r`.
    #[test]
    fn multi_cursor_keys_are_rebindable_editor_keys() {
        for id in ["ed_next_occurrence", "ed_all_occurrences", "ed_soft_undo", "ed_toggle_comment", "ed_toggle_comment_alt"] {
            assert!(is_editor(id) && DEFAULTS.iter().any(|(i, _)| *i == id), "{id}");
        }
        let get = |id: &str| DEFAULTS.iter().find(|(i, _)| *i == id).map(|(_, d)| *d).unwrap();
        assert_eq!(get("ed_all_occurrences"), get("split_r"));
        assert_eq!(parse(get("ed_toggle_comment")), Some((Modifiers::CTRL, Key::Slash)));
        assert!(DEFAULTS.iter().all(|(_, d)| !d.contains("Alt+Shift+Up") && !d.contains("Alt+Shift+Down")), "kein Cursor darüber/darunter");
    }

    #[test]
    fn close_all_has_no_default_shortcut() {
        assert_eq!(DEFAULTS.iter().find(|(id, _)| *id == "close_all").map(|(_, d)| *d), Some(""));
        assert!(parse("").is_none(), "leeres Kürzel löst nie aus");
        assert!(!is_global("close_all"));
    }
}
