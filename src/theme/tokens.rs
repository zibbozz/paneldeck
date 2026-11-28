//! Benannte Oberflächenfarben eines Themes (#309). Die Namen sind öffentliche API (`themes/*.json`, Feld `ui`, und
//! `theme_get`) – nur ergänzen, nie umbenennen. Fehlt ein Wert, wird er wie bisher aus den Terminalfarben abgeleitet
//! ([`default_expr`]); ein Wert ist eine Farbe (`#RRGGBB[AA]`) oder ein Verweis auf eine andere Farbe (`@name`,
//! optional mit Deckkraft in Prozent: `@text/20`).

use super::color::{parse_hex, to_hex};
use super::Theme;
use eframe::egui::Color32;

/// Gruppe in der Farbliste des Editors (Reihenfolge = Anzeige).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    Base,
    Text,
    Surfaces,
    Accent,
    Status,
    Sidebar,
    Tickets,
    Chat,
    DiffSyntax,
    Code,
    EditorDiff,
    Terminal,
}

impl Group {
    pub const ALL: [Group; 12] = [Group::Base, Group::Text, Group::Surfaces, Group::Accent, Group::Status, Group::Sidebar, Group::Tickets, Group::Chat, Group::DiffSyntax, Group::Code, Group::EditorDiff, Group::Terminal];

    pub fn label(self, lang: crate::state::Lang) -> &'static str {
        match self {
            Group::Base => lang.t("Grundflächen", "Base"),
            Group::Text => lang.t("Text", "Text"),
            Group::Surfaces => lang.t("Flächen und Rahmen", "Surfaces and borders"),
            Group::Accent => lang.t("Akzent und Auswahl", "Accent and selection"),
            Group::Status => lang.t("Status", "Status"),
            Group::Sidebar => lang.t("Seitenleiste und Tabs", "Sidebar and tabs"),
            Group::Tickets => lang.t("Tickets und Chips", "Tickets and chips"),
            Group::Chat => lang.t("Chat", "Chat"),
            Group::DiffSyntax => lang.t("Diff und Syntax", "Diff and syntax"),
            Group::Code => lang.t("Editor: Syntaxfarben", "Editor: syntax colours"),
            Group::EditorDiff => lang.t("Editor: Vergleich und Änderungsmarken", "Editor: comparison and change markers"),
            Group::Terminal => lang.t("Terminal", "Terminal"),
        }
    }
}

/// Eine benannte Farbe mit Gruppe und Beschreibung (DE, EN).
#[derive(Clone, Copy, Debug)]
pub struct Token {
    pub name: &'static str,
    pub group: Group,
    pub de: &'static str,
    pub en: &'static str,
}

macro_rules! ui_tokens {
    ($( $f:ident : $g:ident, $de:literal, $en:literal; )*) => {
        /// Alle aufgelösten Oberflächenfarben eines Themes.
        #[derive(Clone, Copy, PartialEq, Debug)]
        pub struct UiColors { $( pub $f: Color32, )* }

        /// Die Oberflächenfarben (öffentliche Namen) in Anzeige-Reihenfolge.
        pub const UI_TOKENS: &[Token] = &[ $( Token { name: stringify!($f), group: Group::$g, de: $de, en: $en }, )* ];

        impl UiColors {
            pub fn get(&self, name: &str) -> Option<Color32> {
                match name { $( stringify!($f) => Some(self.$f), )* _ => None }
            }

            fn from_fn(mut f: impl FnMut(&'static str) -> Color32) -> Self {
                UiColors { $( $f: f(stringify!($f)), )* }
            }
        }
    };
}

ui_tokens! {
    window_bg: Base, "Hintergrund von Dialogen und Fenstern", "Background of dialogs and windows";
    surface: Base, "Abgesetzte Fläche (Kopfzeilen, Karten)", "Raised surface (headers, cards)";
    input_bg: Base, "Eingabefelder", "Input fields";
    code_bg: Base, "Code-Blöcke", "Code blocks";
    text: Text, "Normaler Text", "Normal text";
    text_dim: Text, "Gedämpfter Text (Hinweise)", "Dimmed text (hints)";
    text_strong: Text, "Betonter Text (Hover, gedrückt)", "Strong text (hover, pressed)";
    link: Text, "Links", "Links";
    widget_bg: Surfaces, "Knöpfe und Bedienelemente", "Buttons and controls";
    hover_bg: Surfaces, "Fläche beim Überfahren", "Background on hover";
    border: Surfaces, "Rahmen und Trennlinien", "Borders and separators";
    widget_border: Surfaces, "Rahmen von Bedienelementen", "Border of controls";
    hover_border: Surfaces, "Rahmen beim Überfahren", "Border on hover";
    accent: Accent, "Akzentfarbe", "Accent colour";
    active_bg: Accent, "Gedrückte und aktive Elemente", "Pressed and active elements";
    selection_bg: Accent, "Textauswahl", "Text selection";
    focus: Accent, "Fokusrahmen und Textcursor", "Focus ring and text cursor";
    success: Status, "Erfolg, bereit", "Success, ready";
    warning: Status, "Warnung, wartet auf Eingabe", "Warning, waiting for input";
    error: Status, "Fehler", "Error";
    danger: Status, "Kritisch (Freigabe nötig, Fehlermeldung)", "Critical (approval needed, error toast)";
    info: Status, "Information", "Information";
    sidebar_bg: Sidebar, "Seitenleiste und Grundfläche", "Sidebar and base panels";
    sidebar_active: Sidebar, "Aktive Sitzung in der Seitenleiste", "Active session in the sidebar";
    tab_active: Sidebar, "Aktiver Tab", "Active tab";
    progress: Sidebar, "Fortschrittsbalken", "Progress bar";
    chip_active: Tickets, "Aktiver Chip (Projekt, Filter)", "Active chip (project, filter)";
    ticket_flag: Tickets, "Markiertes Ticket (Fahne)", "Flagged ticket";
    ticket_star: Tickets, "Favorit (Stern)", "Favourite (star)";
    danger_button: Tickets, "Knopf für zerstörende Aktionen", "Button for destructive actions";
    chat_user_bg: Chat, "Eigene Nachricht", "Own message";
    chat_approval_bg: Chat, "Freigabe-Frage", "Approval request";
    chat_send: Chat, "Senden-Knopf", "Send button";
    diff_add: DiffSyntax, "Diff: hinzugefügt", "Diff: added";
    diff_remove: DiffSyntax, "Diff: entfernt", "Diff: removed";
    diff_change: DiffSyntax, "Diff: geändert", "Diff: modified";
    diff_rename: DiffSyntax, "Diff: umbenannt", "Diff: renamed";
    syntax_field: DiffSyntax, "Syntax: Feld", "Syntax: field";
    syntax_operator: DiffSyntax, "Syntax: Operator", "Syntax: operator";
    syntax_value: DiffSyntax, "Syntax: Wert", "Syntax: value";
    syntax_keyword: DiffSyntax, "Syntax: Schlüsselwort", "Syntax: keyword";
    syntax_function: DiffSyntax, "Syntax: Funktion", "Syntax: function";
    syntax_predicate: DiffSyntax, "Syntax: Prädikat", "Syntax: predicate";
    syntax_sort: DiffSyntax, "Syntax: Sortierung", "Syntax: sort key";
    code_comment: Code, "Code: Kommentar", "Code: comment";
    code_string: Code, "Code: Zeichenkette", "Code: string";
    code_number: Code, "Code: Zahl", "Code: number";
    code_constant: Code, "Code: Konstante (true, null, Makros)", "Code: constant (true, null, macros)";
    code_keyword: Code, "Code: Schlüsselwort", "Code: keyword";
    code_type: Code, "Code: Typ, Klasse", "Code: type, class";
    code_function: Code, "Code: Funktion", "Code: function";
    code_operator: Code, "Code: Operator", "Code: operator";
    code_tag: Code, "Code: Tag (HTML/XML), Schlüssel (YAML)", "Code: tag (HTML/XML), key (YAML)";
    code_attribute: Code, "Code: Attribut, Schlüssel (JSON, INI)", "Code: attribute, key (JSON, INI)";
    code_preprocessor: Code, "Code: Präprozessor, Direktive", "Code: preprocessor, directive";
    code_escape: Code, "Code: Escape-Sequenz, Platzhalter", "Code: escape sequence, placeholder";
    code_heading: Code, "Code: Überschrift, Abschnitt", "Code: heading, section";
    code_variable: Code, "Code: Variable, Parameter", "Code: variable, parameter";
    code_invalid: Code, "Code: ungültig", "Code: invalid";
    diag_error: Code, "Smart-Modus: Fehler (Wellenlinie)", "Smart mode: error (squiggle)";
    diag_warning: Code, "Smart-Modus: Warnung (Wellenlinie)", "Smart mode: warning (squiggle)";
    diag_info: Code, "Smart-Modus: Information (Wellenlinie)", "Smart mode: information (squiggle)";
    diag_hint: Code, "Smart-Modus: Hinweis (gepunktet)", "Smart mode: hint (dotted)";
    diff_add_bg: EditorDiff, "Vergleich: Hintergrund hinzugefügter Zeilen", "Comparison: background of added lines";
    diff_remove_bg: EditorDiff, "Vergleich: Hintergrund entfernter Zeilen", "Comparison: background of removed lines";
    diff_add_word: EditorDiff, "Vergleich: hinzugefügte Wörter in der Zeile", "Comparison: added words within a line";
    diff_remove_word: EditorDiff, "Vergleich: entfernte Wörter in der Zeile", "Comparison: removed words within a line";
    diff_filler: EditorDiff, "Vergleich: Schraffur der Füllzeilen", "Comparison: hatching of filler lines";
    marker_added: EditorDiff, "Änderungsmarke: hinzugefügt (nicht gestagt, ungespeichert)", "Change marker: added (unstaged, unsaved)";
    marker_modified: EditorDiff, "Änderungsmarke: geändert (nicht gestagt, ungespeichert)", "Change marker: modified (unstaged, unsaved)";
    marker_deleted: EditorDiff, "Änderungsmarke: entfernt (nicht gestagt, ungespeichert)", "Change marker: removed (unstaged, unsaved)";
    marker_staged_added: EditorDiff, "Änderungsmarke: hinzugefügt (gestagt)", "Change marker: added (staged)";
    marker_staged_modified: EditorDiff, "Änderungsmarke: geändert (gestagt)", "Change marker: modified (staged)";
    marker_staged_deleted: EditorDiff, "Änderungsmarke: entfernt (gestagt)", "Change marker: removed (staged)";
}

/// Die Terminalfarben (Felder `fg`, `bg`, `cursor`, `selection` und `ansi` der Datei) als Namen für Verweise und Editor.
pub const TERMINAL_TOKENS: &[Token] = &[
    Token { name: "fg", group: Group::Terminal, de: "Terminal: Schrift", en: "Terminal: text" },
    Token { name: "bg", group: Group::Terminal, de: "Terminal: Hintergrund", en: "Terminal: background" },
    Token { name: "cursor", group: Group::Terminal, de: "Terminal: Cursor", en: "Terminal: cursor" },
    Token { name: "selection", group: Group::Terminal, de: "Terminal: Auswahl", en: "Terminal: selection" },
    Token { name: "ansi0", group: Group::Terminal, de: "ANSI 0 Schwarz", en: "ANSI 0 black" },
    Token { name: "ansi1", group: Group::Terminal, de: "ANSI 1 Rot", en: "ANSI 1 red" },
    Token { name: "ansi2", group: Group::Terminal, de: "ANSI 2 Grün", en: "ANSI 2 green" },
    Token { name: "ansi3", group: Group::Terminal, de: "ANSI 3 Gelb", en: "ANSI 3 yellow" },
    Token { name: "ansi4", group: Group::Terminal, de: "ANSI 4 Blau", en: "ANSI 4 blue" },
    Token { name: "ansi5", group: Group::Terminal, de: "ANSI 5 Magenta", en: "ANSI 5 magenta" },
    Token { name: "ansi6", group: Group::Terminal, de: "ANSI 6 Cyan", en: "ANSI 6 cyan" },
    Token { name: "ansi7", group: Group::Terminal, de: "ANSI 7 Weiß", en: "ANSI 7 white" },
    Token { name: "ansi8", group: Group::Terminal, de: "ANSI 8 helles Schwarz", en: "ANSI 8 bright black" },
    Token { name: "ansi9", group: Group::Terminal, de: "ANSI 9 helles Rot", en: "ANSI 9 bright red" },
    Token { name: "ansi10", group: Group::Terminal, de: "ANSI 10 helles Grün", en: "ANSI 10 bright green" },
    Token { name: "ansi11", group: Group::Terminal, de: "ANSI 11 helles Gelb", en: "ANSI 11 bright yellow" },
    Token { name: "ansi12", group: Group::Terminal, de: "ANSI 12 helles Blau", en: "ANSI 12 bright blue" },
    Token { name: "ansi13", group: Group::Terminal, de: "ANSI 13 helles Magenta", en: "ANSI 13 bright magenta" },
    Token { name: "ansi14", group: Group::Terminal, de: "ANSI 14 helles Cyan", en: "ANSI 14 bright cyan" },
    Token { name: "ansi15", group: Group::Terminal, de: "ANSI 15 helles Weiß", en: "ANSI 15 bright white" },
];

/// Alle Farben (Oberfläche, dann Terminal).
pub fn all_tokens() -> impl Iterator<Item = &'static Token> {
    UI_TOKENS.iter().chain(TERMINAL_TOKENS.iter())
}

pub fn token(name: &str) -> Option<&'static Token> {
    all_tokens().find(|t| t.name == name)
}

pub fn is_ui(name: &str) -> bool {
    UI_TOKENS.iter().any(|t| t.name == name)
}

/// Gesetzter Wert einer Oberflächenfarbe in der Datei.
#[derive(Clone, Debug, PartialEq)]
pub enum Spec {
    /// Feste Farbe.
    Hex(Color32),
    /// Verweis auf eine andere Farbe, optional mit Deckkraft in Prozent (`@text/20`).
    Link { to: String, alpha: Option<u8> },
}

/// Fehler beim Lesen eines Werts (für die Meldung).
#[derive(Clone, Debug, PartialEq)]
pub enum SpecError {
    /// Weder Farbe noch Verweis.
    Syntax,
    /// Deckkraft nicht 0–100.
    Alpha,
    /// Verweis auf eine unbekannte Farbe.
    Unknown(String),
}

impl Spec {
    /// Text eines Werts lesen: `#RRGGBB`, `#RRGGBBAA`, `@name` oder `@name/NN`. Unbekannte Namen sind ein Fehler
    /// ([`SpecError::Unknown`]); beim Laden einer Datei behandelt der Aufrufer sie als „abgeleitet“ (neuere Versionen).
    pub fn parse(s: &str) -> Result<Spec, SpecError> {
        let s = s.trim();
        if let Some(rest) = s.strip_prefix('@') {
            let (name, alpha) = match rest.split_once('/') {
                Some((n, a)) => {
                    let a = a.trim();
                    if a.is_empty() || !a.bytes().all(|b| b.is_ascii_digit()) || a.len() > 3 {
                        return Err(SpecError::Alpha);
                    }
                    let v: u16 = a.parse().map_err(|_| SpecError::Alpha)?;
                    if v > 100 {
                        return Err(SpecError::Alpha);
                    }
                    (n.trim(), Some(v as u8))
                }
                None => (rest.trim(), None),
            };
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                return Err(SpecError::Syntax);
            }
            if token(name).is_none() {
                return Err(SpecError::Unknown(name.to_string()));
            }
            return Ok(Spec::Link { to: name.to_string(), alpha });
        }
        if s.starts_with('#') {
            return parse_hex(s).map(Spec::Hex).ok_or(SpecError::Syntax);
        }
        Err(SpecError::Syntax)
    }

    /// Text für die Datei.
    pub fn to_text(&self) -> String {
        match self {
            Spec::Hex(c) => to_hex(*c),
            Spec::Link { to, alpha: None } => format!("@{to}"),
            Spec::Link { to, alpha: Some(a) } => format!("@{to}/{a}"),
        }
    }
}

/// Ausdruck einer abgeleiteten Farbe (Standardwerte).
#[derive(Clone, Copy, Debug)]
pub enum Expr {
    /// Feste Farbe.
    C(Color32),
    /// Andere Farbe unverändert.
    R(&'static str),
    /// Andere Farbe mit Deckkraft (Prozent).
    A(&'static str, u8),
    /// Mischung `a` → `b` zu `t` (0–1).
    M(&'static str, &'static str, f32),
}

const fn rgb(v: u32) -> Expr {
    Expr::C(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// Standardwert (Ableitung) einer Oberflächenfarbe – entspricht der Darstellung vor 2.6.0 (außer den Statusfarben heller Themes: seit 2.6.0 dunkler, damit sie auf hellem Grund lesbar sind). `white`/`black` sind
/// Hilfsnamen, `fg`/`bg`/… die (deckenden) Terminalfarben.
pub fn default_expr(name: &str, dark: bool) -> Expr {
    use Expr::*;
    let d = |a: Expr, b: Expr| if dark { a } else { b };
    match name {
        "window_bg" => d(M("sidebar_bg", "white", 0.05), R("surface")),
        "surface" => d(M("bg", "white", 0.045), M("bg", "white", 0.6)),
        "input_bg" => d(M("sidebar_bg", "black", 0.30), M("sidebar_bg", "white", 0.7)),
        "code_bg" => R("input_bg"),
        "text" => d(M("fg", "white", 0.18), M("fg", "black", 0.15)),
        "text_dim" => A("text", 60),
        "text_strong" => d(rgb(0xffffff), rgb(0x000000)),
        "link" => R("accent"),
        "widget_bg" => d(M("sidebar_bg", "white", 0.10), M("sidebar_bg", "black", 0.08)),
        "hover_bg" => d(M("sidebar_bg", "white", 0.07), M("sidebar_bg", "black", 0.06)),
        "border" => d(M("bg", "white", 0.12), M("bg", "black", 0.14)),
        "widget_border" => M("sidebar_bg", "text", 0.16),
        "hover_border" => M("border", "text", 0.25),
        "accent" => rgb(0xd97757),
        "active_bg" => A("accent", 45),
        "selection_bg" => A("accent", 35),
        "focus" => R("accent"),
        "success" => d(rgb(0x4cc26a), rgb(0x1a7f37)),
        "warning" => d(rgb(0xe5c07b), rgb(0x9a6700)),
        "error" => d(rgb(0xe06c75), rgb(0xcf222e)),
        "danger" => d(rgb(0xe04f5f), rgb(0xa40e26)),
        "info" => R("accent"),
        "sidebar_bg" => d(M("bg", "black", 0.22), M("bg", "black", 0.045)),
        "sidebar_active" => A("accent", 16),
        "tab_active" => A("accent", 25),
        "progress" => R("accent"),
        "chip_active" => A("accent", 28),
        "ticket_flag" => R("error"),
        "ticket_star" => R("warning"),
        "danger_button" => rgb(0xc24141),
        "chat_user_bg" => A("accent", 28),
        "chat_approval_bg" => A("warning", 18),
        "chat_send" => A("accent", 70),
        "diff_add" => rgb(0x98c379),
        "diff_remove" => R("error"),
        "diff_change" => R("warning"),
        "diff_rename" => rgb(0x61afef),
        "syntax_field" => rgb(0x58a6e6),
        "syntax_operator" => rgb(0xe5a04b),
        "syntax_value" => rgb(0x7fbf6a),
        "syntax_keyword" => rgb(0xb77fe0),
        "syntax_function" => rgb(0xe07fb0),
        "syntax_predicate" => rgb(0x4fb8a8),
        "syntax_sort" => rgb(0xd8b94a),
        // Syntaxfarben des Editors (#312): aus der Terminal-Palette, aufgehellt (dunkel) bzw. zur Schrift hin abgedunkelt
        // (hell), damit sie auf dem Hintergrund lesbar bleiben (Kontrast-Prüfung wie bei `syntax_*`).
        "code_comment" => d(M("fg", "bg", 0.3), M("fg", "bg", 0.35)),
        "code_string" => d(M("ansi2", "white", 0.25), M("ansi2", "fg", 0.4)),
        "code_type" => d(M("ansi3", "white", 0.25), M("ansi3", "fg", 0.4)),
        "code_keyword" => d(M("ansi5", "white", 0.25), M("ansi5", "fg", 0.4)),
        "code_function" => d(M("ansi4", "white", 0.25), M("ansi4", "fg", 0.4)),
        "code_operator" => d(M("ansi6", "white", 0.25), M("ansi6", "fg", 0.4)),
        "code_tag" => d(M("ansi1", "white", 0.25), M("ansi1", "fg", 0.4)),
        "code_number" => M("code_type", "code_tag", 0.45),
        "code_constant" => R("code_number"),
        "code_attribute" => R("code_number"),
        "code_preprocessor" => M("code_keyword", "code_function", 0.5),
        "code_escape" => R("code_operator"),
        "code_heading" => R("code_function"),
        "code_variable" => R("code_tag"),
        "code_invalid" => R("error"),
        // Smart-Modus (#367): Diagnosen in den Statusfarben, Hinweise gedämpft.
        "diag_error" => R("error"),
        "diag_warning" => R("warning"),
        "diag_info" => R("info"),
        "diag_hint" => A("text_dim", 80),
        // Vergleich und Änderungsmarken (#385): Zeilen- und Wort-Hintergründe aus den Diff-Farben; Marken nicht gestagt
        // (bzw. ungespeichert) in Grün/Gelb/Rot, gestagt in Türkis/Blau/Violett – unterscheidbar, auch hell lesbar.
        "diff_add_bg" => A("diff_add", 15),
        "diff_remove_bg" => A("diff_remove", 15),
        "diff_add_word" => A("diff_add", 40),
        "diff_remove_word" => A("diff_remove", 40),
        "diff_filler" => A("text_dim", 35),
        "marker_added" => R("success"),
        "marker_modified" => R("warning"),
        "marker_deleted" => R("error"),
        "marker_staged_added" => d(rgb(0x56b6c2), rgb(0x0b7285)),
        "marker_staged_modified" => d(rgb(0x61afef), rgb(0x0969da)),
        "marker_staged_deleted" => d(rgb(0xc678dd), rgb(0x8250df)),
        _ => C(Color32::from_rgb(0xff, 0x00, 0xff)),
    }
}

/// Text eines Standardwerts für die Anzeige („abgeleitet: @accent/35“, „mix(bg, white, 4.5 %)“).
pub fn describe_expr(e: Expr) -> String {
    match e {
        Expr::C(c) => to_hex(c),
        Expr::R(n) => format!("@{n}"),
        Expr::A(n, a) => format!("@{n}/{a}"),
        Expr::M(a, b, t) => format!("mix({a}, {b}, {}%)", (t * 1000.0).round() / 10.0),
    }
}

/// Deckende Fassung des Terminal-Hintergrunds für Ableitungen. Wie `style::apply` vor 2.6.0 aus den vormultiplizierten
/// Kanälen: Bei einer Deckkraft unter 100 % (`App::theme` setzt sie auf `bg`) werden die Flächen dadurch dunkler –
/// genau so sah die App bisher aus.
fn opaque(c: Color32) -> Color32 {
    Color32::from_rgb(c.r(), c.g(), c.b())
}

/// Terminalfarbe nach Name (deckend) oder `None`.
pub fn terminal_color(t: &Theme, name: &str) -> Option<Color32> {
    Some(match name {
        "fg" => t.fg,
        "bg" => opaque(t.bg),
        "cursor" => t.cursor,
        "selection" => t.selection,
        "white" => Color32::WHITE,
        "black" => Color32::BLACK,
        n => {
            let i: usize = n.strip_prefix("ansi")?.parse().ok()?;
            *t.ansi.get(i)?
        }
    })
}

const MAX_DEPTH: usize = 24;

/// Farbe eines Namens auflösen (Oberfläche: gesetzter Wert, sonst Ableitung; Terminal: Feld des Themes).
pub fn resolve(t: &Theme, name: &str) -> Color32 {
    resolve_depth(t, name, 0)
}

fn resolve_depth(t: &Theme, name: &str, depth: usize) -> Color32 {
    if depth > MAX_DEPTH {
        return Color32::from_rgb(0xff, 0x00, 0xff);
    }
    if let Some(c) = terminal_color(t, name) {
        return c;
    }
    match t.ui.get(name) {
        Some(Spec::Hex(c)) => *c,
        Some(Spec::Link { to, alpha }) => {
            let c = resolve_depth(t, to, depth + 1);
            match alpha {
                Some(a) => c.gamma_multiply(*a as f32 / 100.0),
                None => c,
            }
        }
        None => eval(t, default_expr(name, t.dark), depth),
    }
}

fn eval(t: &Theme, e: Expr, depth: usize) -> Color32 {
    match e {
        Expr::C(c) => c,
        Expr::R(n) => resolve_depth(t, n, depth + 1),
        Expr::A(n, a) => resolve_depth(t, n, depth + 1).gamma_multiply(a as f32 / 100.0),
        Expr::M(a, b, k) => crate::style::mix(resolve_depth(t, a, depth + 1), resolve_depth(t, b, depth + 1), k),
    }
}

/// Alle Oberflächenfarben eines Themes.
pub fn ui_colors(t: &Theme) -> UiColors {
    UiColors::from_fn(|n| resolve(t, n))
}

/// Verweis-Ziele eines Namens (gesetzt oder abgeleitet) – für die Kreis-Prüfung.
fn targets(t: &Theme, name: &str) -> Vec<String> {
    if terminal_color(t, name).is_some() {
        return Vec::new();
    }
    match t.ui.get(name) {
        Some(Spec::Hex(_)) => Vec::new(),
        Some(Spec::Link { to, .. }) => vec![to.clone()],
        None => match default_expr(name, t.dark) {
            Expr::C(_) => Vec::new(),
            Expr::R(n) | Expr::A(n, _) => vec![n.to_string()],
            Expr::M(a, b, _) => vec![a.to_string(), b.to_string()],
        },
    }
}

/// Erster Kreis aus Verweisen (`a → b → a`), als Namenskette; `None` ohne Kreis.
pub fn find_cycle(t: &Theme) -> Option<Vec<String>> {
    fn visit(t: &Theme, n: &str, stack: &mut Vec<String>, done: &mut Vec<String>) -> Option<Vec<String>> {
        if let Some(i) = stack.iter().position(|s| s == n) {
            let mut c = stack[i..].to_vec();
            c.push(n.to_string());
            return Some(c);
        }
        if done.iter().any(|d| d == n) {
            return None;
        }
        stack.push(n.to_string());
        for to in targets(t, n) {
            if let Some(c) = visit(t, &to, stack, done) {
                return Some(c);
            }
        }
        stack.pop();
        done.push(n.to_string());
        None
    }
    let mut done = Vec::new();
    for tok in UI_TOKENS {
        if let Some(c) = visit(t, tok.name, &mut Vec::new(), &mut done) {
            return Some(c);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_names_are_unique_and_documented() {
        let mut names: Vec<&str> = all_tokens().map(|t| t.name).collect();
        let n = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), n, "doppelte Namen");
        for t in UI_TOKENS {
            assert!(!t.de.is_empty() && !t.en.is_empty(), "{}", t.name);
            assert!(t.name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_' || b.is_ascii_digit()), "{}", t.name);
            assert!(!matches!(default_expr(t.name, true), Expr::C(c) if c == Color32::from_rgb(0xff, 0, 0xff)), "{} ohne Ableitung", t.name);
        }
        // Jede Gruppe außer Terminal hat Oberflächenfarben.
        for g in Group::ALL {
            assert!(all_tokens().any(|t| t.group == g), "{g:?}");
        }
    }

    #[test]
    fn spec_parsing() {
        assert_eq!(Spec::parse("#d97757"), Ok(Spec::Hex(Color32::from_rgb(0xd9, 0x77, 0x57))));
        assert_eq!(Spec::parse(" @text "), Ok(Spec::Link { to: "text".into(), alpha: None }));
        assert_eq!(Spec::parse("@text/20"), Ok(Spec::Link { to: "text".into(), alpha: Some(20) }));
        assert_eq!(Spec::parse("@ansi4/100"), Ok(Spec::Link { to: "ansi4".into(), alpha: Some(100) }));
        assert_eq!(Spec::parse("@text/101"), Err(SpecError::Alpha));
        assert_eq!(Spec::parse("@text/"), Err(SpecError::Alpha));
        assert_eq!(Spec::parse("@text/-1"), Err(SpecError::Alpha));
        assert_eq!(Spec::parse("@nope"), Err(SpecError::Unknown("nope".into())));
        assert_eq!(Spec::parse("@"), Err(SpecError::Syntax));
        assert_eq!(Spec::parse("red"), Err(SpecError::Syntax));
        assert_eq!(Spec::parse("#12"), Err(SpecError::Syntax));
        for s in ["#d97757", "@text", "@text/20"] {
            assert_eq!(Spec::parse(s).unwrap().to_text(), s);
        }
    }

    #[test]
    fn derived_colours_match_the_old_style() {
        // One Dark: die Werte, die `style::apply` vor 2.6.0 fest berechnet hat.
        let t = &crate::theme::all()[0];
        let u = ui_colors(t);
        let mix = crate::style::mix;
        let bg = t.bg;
        let side = mix(bg, Color32::BLACK, 0.22);
        assert_eq!(u.sidebar_bg, side);
        assert_eq!(u.window_bg, mix(side, Color32::WHITE, 0.05));
        assert_eq!(u.input_bg, mix(side, Color32::BLACK, 0.30));
        assert_eq!(u.text, mix(t.fg, Color32::WHITE, 0.18));
        assert_eq!(u.accent, Color32::from_rgb(0xd9, 0x77, 0x57));
        assert_eq!(u.selection_bg, u.accent.gamma_multiply(0.35));
        assert_eq!(u.error, Color32::from_rgb(0xe0, 0x6c, 0x75));
        assert_eq!(u.ticket_flag, u.error);
        // Durchscheinender Hintergrund (Deckkraft-Einstellung, wie `App::theme`): die alte Darstellung rechnete mit den
        // vormultiplizierten Kanälen von `theme.bg`.
        for a in [77u8, 128, 200] {
            let mut tt = t.clone();
            tt.bg = Color32::from_rgba_unmultiplied(t.bg.r(), t.bg.g(), t.bg.b(), a);
            let old_bg = Color32::from_rgb(tt.bg.r(), tt.bg.g(), tt.bg.b());
            let ut = ui_colors(&tt);
            let side = mix(old_bg, Color32::BLACK, 0.22);
            assert_eq!(ut.sidebar_bg, side, "Deckkraft {a}");
            assert_eq!(ut.window_bg, mix(side, Color32::WHITE, 0.05));
            assert_eq!(ut.input_bg, mix(side, Color32::BLACK, 0.30));
            assert_eq!(ut.border, mix(old_bg, Color32::WHITE, 0.12));
            assert_eq!(ut.hover_bg, mix(side, Color32::WHITE, 0.07));
            assert_eq!(ut.widget_bg, mix(side, Color32::WHITE, 0.10));
            assert_eq!(ut.text, u.text, "Schrift unabhängig von der Deckkraft");
        }
        // Hell: Fenster = abgesetzte Fläche.
        let light = crate::theme::all().into_iter().find(|t| !t.dark).unwrap();
        let ul = ui_colors(&light);
        assert_eq!(ul.window_bg, ul.surface);
        // Hell: dunklere Statusfarben (seit 2.6.0), lesbar auf hellem Grund.
        assert_eq!(ul.warning, Color32::from_rgb(0x9a, 0x67, 0x00));
        assert_eq!(ul.error, Color32::from_rgb(0xcf, 0x22, 0x2e));
        assert_eq!(ul.text_strong, Color32::BLACK);
    }

    #[test]
    fn links_follow_their_target() {
        let mut t = crate::theme::all()[0].clone();
        t.ui.insert("accent".into(), Spec::Hex(Color32::from_rgb(0, 0x80, 0xff)));
        t.ui.insert("border".into(), Spec::Link { to: "text".into(), alpha: Some(20) });
        t.ui.insert("warning".into(), Spec::Link { to: "ansi3".into(), alpha: None });
        let u = ui_colors(&t);
        assert_eq!(u.accent, Color32::from_rgb(0, 0x80, 0xff));
        assert_eq!(u.link, u.accent, "abgeleitet über @accent");
        assert_eq!(u.selection_bg, u.accent.gamma_multiply(0.35));
        assert_eq!(u.border, u.text.gamma_multiply(0.2));
        assert_eq!(u.warning, t.ansi[3]);
        assert_eq!(u.ticket_star, t.ansi[3], "Kette: ticket_star → warning → ansi3");
        assert!(find_cycle(&t).is_none());
        t.ui.insert("text".into(), Spec::Link { to: "border".into(), alpha: None });
        let c = find_cycle(&t).expect("Kreis");
        assert!(c.contains(&"text".to_string()) && c.contains(&"border".to_string()), "{c:?}");
        // Auch mit Kreis hängt die Auflösung nicht.
        let _ = ui_colors(&t);
    }
}
