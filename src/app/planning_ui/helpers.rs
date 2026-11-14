//! Reine Hilfsfunktionen der Projekt-Oberfläche (ohne egui-Zustand, gut testbar): Schlüssel-Vorschlag, Farben,
//! Reihenfolge, Übergangsmatrix, Beschriftungen der Aufzählungen.

use crate::planning::fields::FieldKind;
use crate::planning::link_types::TargetKind;
use crate::planning::statuses::StatusCategory;
use crate::planning::workflow::{ConditionOp, TransitionDef, WorkflowProblem};
use crate::state::Lang;
use eframe::egui::Color32;

/// Farbpalette für Projekte, Status und Typen (für dunkle wie helle Oberflächen brauchbar).
pub const PALETTE: [&str; 14] = [
    "#D97757", "#E5A04B", "#D8B94A", "#7FBF6A", "#4FB8A8", "#58A6E6", "#6F8FE8", "#B77FE0", "#E07FB0", "#E06C75", "#97A3B0", "#8C7B6B", "#5E8F5E", "#4A78B5",
];

/// „#RRGGBB“ → Farbe (nur genau diese Form).
pub fn parse_hex(s: &str) -> Option<Color32> {
    let t = s.trim();
    if t.len() != 7 || !t.starts_with('#') || !t[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let v = u32::from_str_radix(&t[1..], 16).ok()?;
    Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

pub fn color_of(s: &str) -> Color32 {
    parse_hex(s).unwrap_or(Color32::GRAY)
}

/// Erste Farbe der Palette, die noch am seltensten benutzt wird (für neue Projekte/Status/Typen).
pub fn next_palette_color(used: &[String]) -> &'static str {
    PALETTE.iter().min_by_key(|c| used.iter().filter(|u| u.eq_ignore_ascii_case(c)).count()).copied().unwrap_or(PALETTE[0])
}

/// Umlaute und ß in ASCII, übrige Nicht-ASCII-Zeichen entfallen.
fn fold(c: char) -> Option<String> {
    Some(match c {
        'ä' | 'Ä' => "A".into(),
        'ö' | 'Ö' => "O".into(),
        'ü' | 'Ü' => "U".into(),
        'ß' => "S".into(),
        c if c.is_ascii_alphanumeric() => c.to_ascii_uppercase().to_string(),
        _ => return None,
    })
}

/// Vorschlag für den Projektschlüssel aus dem Namen: mehrere Wörter → Anfangsbuchstaben (bis 4), ein Wort → die ersten
/// drei Zeichen. Beginnt mit einem Buchstaben, hat mindestens zwei Zeichen und ist nicht in `taken` (sonst Ziffer anhängen).
pub fn suggest_key(name: &str, taken: &[String]) -> String {
    let words: Vec<String> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(|w| w.chars().filter_map(fold).collect::<String>()).filter(|w| !w.is_empty()).collect();
    // Wörter, die mit einer Ziffer beginnen, zählen nicht (der Schlüssel muss mit einem Buchstaben beginnen).
    let words: Vec<String> = words.into_iter().filter(|w| w.chars().next().is_some_and(|c| c.is_ascii_alphabetic())).collect();
    let mut key = if words.len() >= 2 { words.iter().take(4).filter_map(|w| w.chars().next()).collect::<String>() } else { words.first().map(|w| w.chars().take(3).collect()).unwrap_or_default() };
    if key.is_empty() {
        return String::new();
    }
    if key.len() < 2 {
        key.push('X');
    }
    let base = key.clone();
    let mut n = 2;
    while taken.iter().any(|t| t.eq_ignore_ascii_case(&key)) {
        let suffix = n.to_string();
        let cut = base.len().min(10 - suffix.len());
        key = format!("{}{}", &base[..cut], suffix);
        n += 1;
    }
    key
}

/// Einen Eintrag in der Reihenfolge verschieben (`dir` -1 = nach oben, 1 = nach unten). `false`, wenn nichts zu tun war.
pub fn move_id(order: &mut [i64], id: i64, dir: i32) -> bool {
    let Some(i) = order.iter().position(|x| *x == id) else { return false };
    let j = i as i64 + dir as i64;
    if j < 0 || j >= order.len() as i64 {
        return false;
    }
    order.swap(i, j as usize);
    true
}

/// Komma-getrennte Liste in Einträge (bereinigt, ohne Leere und Doppelte).
pub fn parse_csv(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in s.split(',') {
        let p = p.split_whitespace().collect::<Vec<_>>().join(" ");
        if !p.is_empty() && !out.iter().any(|o| o.eq_ignore_ascii_case(&p)) {
            out.push(p);
        }
    }
    out
}

// ------------------------------------------------------------------ Übergangsmatrix

pub fn has_transition(defs: &[TransitionDef], from: i64, to: i64) -> bool {
    defs.iter().any(|d| d.from_status_id == from && d.to_status_id == to)
}

/// Linearer Ablauf: jeder Status zum nächsten und wieder zurück.
pub fn linear_transitions(statuses: &[i64]) -> Vec<TransitionDef> {
    let mut out = Vec::new();
    for w in statuses.windows(2) {
        out.push(TransitionDef { from_status_id: w[0], to_status_id: w[1], ..Default::default() });
        out.push(TransitionDef { from_status_id: w[1], to_status_id: w[0], ..Default::default() });
    }
    out
}

// ------------------------------------------------------------------ Beschriftungen

pub fn kind_label(k: FieldKind, lang: Lang) -> &'static str {
    match k {
        FieldKind::Text => lang.t("Text", "Text"),
        FieldKind::Rich => lang.t("Rich-Text (Markdown)", "Rich text (Markdown)"),
        FieldKind::Checkbox => lang.t("Checkbox", "Checkbox"),
        FieldKind::Dropdown => lang.t("Dropdown", "Dropdown"),
        FieldKind::Number => lang.t("Zahl", "Number"),
        FieldKind::Ticket => lang.t("Ticket-Verweis", "Ticket link"),
        FieldKind::Terminal => lang.t("Terminal", "Terminal"),
        FieldKind::Version => lang.t("Version", "Version"),
        FieldKind::Milestone => lang.t("Meilenstein", "Milestone"),
        FieldKind::Date => lang.t("Datum", "Date"),
        FieldKind::Datetime => lang.t("Datum + Uhrzeit", "Date + time"),
        FieldKind::Multi => lang.t("Mehrfachauswahl", "Multi-select"),
        FieldKind::Labels => lang.t("Labels", "Labels"),
    }
}

pub fn category_label(c: StatusCategory, lang: Lang) -> &'static str {
    match c {
        StatusCategory::Todo => lang.t("Zu erledigen", "To Do"),
        StatusCategory::Progress => lang.t("In Arbeit", "In Progress"),
        StatusCategory::Done => lang.t("Erledigt", "Done"),
    }
}

pub fn target_label(k: TargetKind, lang: Lang) -> &'static str {
    match k {
        TargetKind::Ticket => lang.t("Ticket", "Ticket"),
        TargetKind::Terminal => lang.t("Terminal", "Terminal"),
        TargetKind::Url => lang.t("URL", "URL"),
        TargetKind::Branch => lang.t("Git-Branch", "Git branch"),
    }
}

pub fn op_label(op: ConditionOp, lang: Lang) -> &'static str {
    match op {
        ConditionOp::Eq => lang.t("ist", "is"),
        ConditionOp::Ne => lang.t("ist nicht", "is not"),
        ConditionOp::Gt => lang.t("größer als", "greater than"),
        ConditionOp::Ge => lang.t("mindestens", "at least"),
        ConditionOp::Lt => lang.t("kleiner als", "less than"),
        ConditionOp::Le => lang.t("höchstens", "at most"),
        ConditionOp::In => lang.t("ist einer von", "is one of"),
        ConditionOp::NotIn => lang.t("ist keiner von", "is none of"),
        ConditionOp::IsEmpty => lang.t("ist leer", "is empty"),
        ConditionOp::IsNotEmpty => lang.t("ist nicht leer", "is not empty"),
    }
}

/// Vergleichsarten, die zum Feldtyp passen (die Datenschicht prüft dasselbe noch einmal).
pub fn ops_for(kind: FieldKind) -> Vec<ConditionOp> {
    use ConditionOp::*;
    match kind {
        FieldKind::Number | FieldKind::Date | FieldKind::Datetime => vec![Eq, Ne, Gt, Ge, Lt, Le, IsEmpty, IsNotEmpty],
        FieldKind::Dropdown | FieldKind::Text => vec![Eq, Ne, In, NotIn, IsEmpty, IsNotEmpty],
        FieldKind::Checkbox | FieldKind::Rich => vec![Eq, Ne, IsEmpty, IsNotEmpty],
        FieldKind::Multi | FieldKind::Labels => vec![Eq, Ne, In, NotIn, IsEmpty, IsNotEmpty],
        _ => vec![IsEmpty, IsNotEmpty],
    }
}

/// Vergleichsarten ohne Wert.
pub fn op_needs_value(op: ConditionOp) -> bool {
    !matches!(op, ConditionOp::IsEmpty | ConditionOp::IsNotEmpty)
}

/// Hinweistext zu einem unvollständigen Workflow (`status_name` für „nicht erreichbar“).
pub fn problem_text(p: &WorkflowProblem, status_name: &dyn Fn(i64) -> String, lang: Lang) -> String {
    match p {
        WorkflowProblem::NoStatuses => lang.t("Es ist noch kein Status gewählt.", "No status is selected yet.").to_string(),
        WorkflowProblem::NoTodoStatus => lang.t("Kein Status der Kategorie „To Do“ – neue Tickets hätten keinen sinnvollen Anfangsstatus.", "No status in the “To Do” category – new tickets would have no sensible starting status.").to_string(),
        WorkflowProblem::NoDoneStatus => lang.t("Kein Status der Kategorie „Erledigt“ – nichts gilt je als erledigt.", "No status in the “Done” category – nothing would ever count as done.").to_string(),
        WorkflowProblem::Unreachable(id) => match lang {
            Lang::De => format!("In den Status {} führt kein Übergang hinein.", lang.q(&status_name(*id))),
            Lang::En => format!("No transition leads into the status {}.", lang.q(&status_name(*id))),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_suggestions() {
        let none: Vec<String> = Vec::new();
        assert_eq!(suggest_key("Paneldeck", &none), "PAN");
        assert_eq!(suggest_key("Paneldeck Hub", &none), "PH");
        assert_eq!(suggest_key("my cool side project x", &none), "MCSP");
        assert_eq!(suggest_key("Übergabe", &none), "UBE", "Umlaut wird zum Grundbuchstaben");
        assert_eq!(suggest_key("  ", &none), "");
        assert_eq!(suggest_key("!!!", &none), "");
        assert_eq!(suggest_key("X", &none), "XX", "mindestens zwei Zeichen");
        assert_eq!(suggest_key("123 Projekt", &none), "PRO", "Wörter mit führender Ziffer zählen nicht");
        assert_eq!(suggest_key("a b", &none), "AB");
    }

    #[test]
    fn key_suggestion_is_valid_and_unique() {
        let taken = vec!["PAN".to_string(), "PAN2".to_string()];
        let k = suggest_key("Paneldeck", &taken);
        assert_eq!(k, "PAN3");
        assert!(crate::planning::common::validate_key(&k).is_ok());
        // sehr langer Name, viele Kollisionen: bleibt gültig (höchstens 10 Zeichen)
        let many: Vec<String> = std::iter::once("ABC".to_string()).chain((2..30).map(|n| format!("ABC{n}"))).collect();
        let k = suggest_key("Abcdefghijklmnop", &many);
        assert!(crate::planning::common::validate_key(&k).is_ok(), "{k}");
        // Groß-/Kleinschreibung beim Vergleich egal
        assert_eq!(suggest_key("Foo", &["foo".to_string()]), "FOO2");
    }

    #[test]
    fn hex_colors() {
        assert_eq!(parse_hex("#D97757"), Some(Color32::from_rgb(0xd9, 0x77, 0x57)));
        assert_eq!(parse_hex(" #d97757 "), Some(Color32::from_rgb(0xd9, 0x77, 0x57)));
        for bad in ["", "d97757", "#d9775", "#d977577", "#gggggg", "red"] {
            assert_eq!(parse_hex(bad), None, "{bad}");
        }
        for c in PALETTE {
            assert!(crate::planning::common::normalize_color(c).is_ok(), "{c}");
        }
    }

    #[test]
    fn palette_rotation_prefers_unused() {
        assert_eq!(next_palette_color(&[]), PALETTE[0]);
        let used = vec![PALETTE[0].to_lowercase(), PALETTE[1].to_string()];
        assert_eq!(next_palette_color(&used), PALETTE[2]);
        let all: Vec<String> = PALETTE.iter().map(|s| s.to_string()).collect();
        assert_eq!(next_palette_color(&all), PALETTE[0], "alle gleich oft → von vorn");
    }

    #[test]
    fn moves_ids() {
        let mut v = vec![1, 2, 3];
        assert!(move_id(&mut v, 2, -1));
        assert_eq!(v, vec![2, 1, 3]);
        assert!(!move_id(&mut v, 2, -1), "schon ganz oben");
        assert!(move_id(&mut v, 2, 1));
        assert!(!move_id(&mut v, 3, 1), "schon ganz unten");
        assert!(!move_id(&mut v, 9, 1), "unbekannte Kennung");
        assert_eq!(v, vec![1, 2, 3]);
    }

    #[test]
    fn csv_lists() {
        assert_eq!(parse_csv(" a ,b,,  c  d , A"), vec!["a", "b", "c d"]);
        assert!(parse_csv(" , ").is_empty());
    }

    #[test]
    fn matrix_helpers() {
        let lin = linear_transitions(&[1, 2, 3]);
        assert_eq!(lin.len(), 4);
        assert!(has_transition(&lin, 1, 2) && has_transition(&lin, 3, 2) && !has_transition(&lin, 1, 3));
        assert!(linear_transitions(&[1]).is_empty());
    }

    #[test]
    fn ops_fit_kinds() {
        assert!(ops_for(FieldKind::Number).contains(&ConditionOp::Gt));
        assert!(!ops_for(FieldKind::Text).contains(&ConditionOp::Gt));
        assert_eq!(ops_for(FieldKind::Ticket), vec![ConditionOp::IsEmpty, ConditionOp::IsNotEmpty]);
        assert!(!op_needs_value(ConditionOp::IsEmpty) && op_needs_value(ConditionOp::Eq));
    }

    #[test]
    fn every_label_exists_in_both_languages() {
        for k in FieldKind::ALL {
            assert!(!kind_label(k, Lang::De).is_empty());
            assert!(!kind_label(k, Lang::En).is_empty());
        }
        for c in [StatusCategory::Todo, StatusCategory::Progress, StatusCategory::Done] {
            assert!(!category_label(c, Lang::De).is_empty() && !category_label(c, Lang::En).is_empty());
        }
        let name = |_: i64| "X".to_string();
        for p in [WorkflowProblem::NoStatuses, WorkflowProblem::NoTodoStatus, WorkflowProblem::NoDoneStatus, WorkflowProblem::Unreachable(1)] {
            assert_ne!(problem_text(&p, &name, Lang::De), problem_text(&p, &name, Lang::En));
        }
    }
}
