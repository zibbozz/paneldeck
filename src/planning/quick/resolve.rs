//! Auflösung der Defaults einer `create_ticket`-Aktion und der Sitzungs-Parameter zu fertigen Werten.
//!
//! **Platzhalter** sind bewusst klein und sicher begrenzt: `{{name}}` mit einem Namen aus einer festen Liste ([`PLACEHOLDERS`]),
//! keine Ausdrücke, keine Verschachtelung, keine Dateizugriffe. Es wird in einem einzigen Durchgang ersetzt – ersetzter
//! Text wird nie erneut gelesen (ein Ticket-Titel mit `{{…}}` bleibt also Text). `{{{{` ergibt ein wörtliches `{{`.
//! Unbekannte Platzhalter bleiben beim Auflösen unverändert stehen; beim Speichern einer Aktion werden sie abgelehnt
//! ([`check_placeholders`]). Jeder ersetzte Wert ist auf [`MAX_SUBSTITUTION_CHARS`] Zeichen begrenzt, das Ergebnis auf
//! [`MAX_EXPANDED_CHARS`].
//!
//! Ausnahme für Ticket-Felder: Der Wert `"{{source}}"` (genau so) bedeutet „das Ausgangsticket“ und wird zu dessen Id.

use super::params::{CreateTicketParams, FieldRef, RelationSpec, StartSessionParams};
use super::{QRes, QuickError};
use crate::planning::fields::{check_value, list_fields, Field, FieldKind};
use crate::state::Lang;
use rusqlite::Connection;
use serde_json::Value;

/// Die erlaubten Platzhalter (klein geschrieben, ohne Klammern).
pub const PLACEHOLDERS: [&str; 10] = [
    "source.id",
    "source.key",
    "source.number",
    "source.title",
    "source.type",
    "source.status",
    "source.description",
    "project.key",
    "project.name",
    "today",
];

/// Der Wert, der in Ticket-Feldern „das Ausgangsticket“ bedeutet.
pub const SOURCE_REF: &str = "{{source}}";

/// Höchstlänge eines einzelnen ersetzten Werts.
pub const MAX_SUBSTITUTION_CHARS: usize = 5_000;
/// Höchstlänge eines aufgelösten Textes.
pub const MAX_EXPANDED_CHARS: usize = 100_000;
/// Höchstlänge eines Ticket-Titels.
pub const MAX_TITLE_CHARS: usize = 255;
/// Höchstlänge eines Labels.
pub const MAX_LABEL_CHARS: usize = 40;
/// Höchstlänge der Beschreibung im Prompt einer Sitzung.
const MAX_PROMPT_DESCRIPTION: usize = 8_000;

/// Das Ticket, in dem die Aktion ausgelöst wird (Ausgangsticket).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SourceContext {
    pub id: i64,
    /// Schlüssel wie `PAN-423`.
    pub key: String,
    pub number: i64,
    pub title: String,
    /// Anzeigename des Typs und des Status in der aktuellen Sprache.
    pub type_name: String,
    pub status_name: String,
    /// Beschreibung als Text (Markdown).
    pub description: String,
}

/// Alles, was Platzhalter brauchen. `today` ist ein ISO-Datum (`JJJJ-MM-TT`), damit die Auflösung ohne Uhr rein bleibt.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolveContext {
    pub source: Option<SourceContext>,
    pub project_key: String,
    pub project_name: String,
    pub today: String,
}

enum Tok<'a> {
    Text(&'a str),
    /// `{{{{` = wörtliches `{{`
    Open,
    Ph { name: String, raw: &'a str },
}

fn valid_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 40 && n.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'.' || c == b'_')
}

/// Zerlegt in Text, Escape und Platzhalter. Die Suche nach `}}` ist auf ein kurzes Fenster begrenzt (kein quadratischer Aufwand).
fn tokenize(s: &str) -> Vec<Tok<'_>> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let (mut i, mut start) = (0usize, 0usize);
    while i + 1 < b.len() {
        if b[i] == b'{' && b[i + 1] == b'{' {
            if s[i..].starts_with("{{{{") {
                if start < i {
                    out.push(Tok::Text(&s[start..i]));
                }
                out.push(Tok::Open);
                i += 4;
                start = i;
                continue;
            }
            let from = i + 2;
            let to = b.len().min(from + 48);
            let close = (from..to.saturating_sub(1)).find(|&j| b[j] == b'}' && b[j + 1] == b'}');
            if let Some(j) = close {
                let name = s[from..j].trim().to_ascii_lowercase();
                if valid_name(&name) {
                    if start < i {
                        out.push(Tok::Text(&s[start..i]));
                    }
                    out.push(Tok::Ph { name, raw: &s[i..j + 2] });
                    i = j + 2;
                    start = i;
                    continue;
                }
            }
        }
        i += 1;
    }
    if start < s.len() {
        out.push(Tok::Text(&s[start..]));
    }
    out
}

/// Namen aller Platzhalter im Text (in Reihenfolge, auch Unbekannte).
pub fn placeholders_in(text: &str) -> Vec<String> {
    tokenize(text)
        .into_iter()
        .filter_map(|t| match t {
            Tok::Ph { name, .. } => Some(name),
            _ => None,
        })
        .collect()
}

/// Lehnt unbekannte Platzhalter ab (beim Speichern einer Aktion).
pub fn check_placeholders(text: &str) -> QRes<()> {
    for n in placeholders_in(text) {
        if !PLACEHOLDERS.contains(&n.as_str()) {
            return Err(QuickError::Placeholder(n));
        }
    }
    Ok(())
}

fn clamp(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn lookup(name: &str, ctx: &ResolveContext) -> Option<String> {
    let src = |f: &dyn Fn(&SourceContext) -> String| Some(ctx.source.as_ref().map(f).unwrap_or_default());
    match name {
        "source.id" => src(&|s| s.id.to_string()),
        "source.key" => src(&|s| s.key.clone()),
        "source.number" => src(&|s| s.number.to_string()),
        "source.title" => src(&|s| s.title.clone()),
        "source.type" => src(&|s| s.type_name.clone()),
        "source.status" => src(&|s| s.status_name.clone()),
        "source.description" => src(&|s| s.description.clone()),
        "project.key" => Some(ctx.project_key.clone()),
        "project.name" => Some(ctx.project_name.clone()),
        "today" => Some(ctx.today.clone()),
        _ => None,
    }
}

/// Ersetzt die Platzhalter (siehe Moduldoku). Reine Funktion.
pub fn expand(text: &str, ctx: &ResolveContext) -> String {
    let mut out = String::new();
    for t in tokenize(text) {
        match t {
            Tok::Text(s) => out.push_str(s),
            Tok::Open => out.push_str("{{"),
            Tok::Ph { name, raw } => match lookup(&name, ctx) {
                Some(v) => out.push_str(&clamp(&v, MAX_SUBSTITUTION_CHARS)),
                None => out.push_str(raw),
            },
        }
        if out.len() > MAX_EXPANDED_CHARS * 4 {
            break;
        }
    }
    clamp(&out, MAX_EXPANDED_CHARS)
}

/// Einzeiliger Text: Steuerzeichen und Zeilenumbrüche werden zu Leerzeichen, Leerraum zusammengefasst, auf `max` gekürzt.
pub fn single_line(s: &str, max: usize) -> String {
    let flat: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    clamp(flat.split_whitespace().collect::<Vec<_>>().join(" ").as_str(), max)
}

/// Mehrzeiliger Text: `\r\n`/`\r` werden `\n`, andere Steuerzeichen (außer Tab) entfallen, auf `max` gekürzt.
pub fn multi_line(s: &str, max: usize) -> String {
    let norm = s.replace("\r\n", "\n").replace('\r', "\n");
    clamp(&norm.chars().filter(|c| !c.is_control() || *c == '\n' || *c == '\t').collect::<String>(), max)
}

/// Ergebnis der Auflösung einer `create_ticket`-Aktion.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedCreate {
    pub ticket_type_id: i64,
    /// Titel (leer, wenn die Aktion keinen vorgibt – die Oberfläche fragt dann).
    pub title: String,
    /// (Feld-Id, Wert) in der Reihenfolge der Defaults.
    pub values: Vec<(i64, Value)>,
    /// Verknüpfung mit dem Ausgangsticket, falls gewünscht (zum Durchreichen an die Ticket-Schicht).
    pub relation: Option<RelationSpec>,
    /// Felder, die nicht gesetzt werden konnten (Feld gelöscht, Wert passt nicht mehr, kein Ausgangsticket).
    pub skipped_fields: Vec<i64>,
}

/// Löst die Defaults der Aktion auf. Die Felder des Ziel-Typs werden aus der Datenbank gelesen.
pub fn resolve_create_defaults(conn: &Connection, action: &super::actions::QuickAction, ctx: &ResolveContext) -> QRes<ResolvedCreate> {
    let super::params::ActionParams::CreateTicket(p) = &action.params else {
        return Err(QuickError::WrongKind);
    };
    if action.invalid {
        return Err(QuickError::ActionInvalid);
    }
    let fields = list_fields(conn, p.ticket_type_id)?;
    Ok(resolve_with_fields(&fields, p, ctx))
}

/// Wie [`resolve_create_defaults`], aber ohne Datenbank (die Felder des Ziel-Typs werden übergeben).
pub fn resolve_with_fields(fields: &[Field], p: &CreateTicketParams, ctx: &ResolveContext) -> ResolvedCreate {
    let mut out = ResolvedCreate { ticket_type_id: p.ticket_type_id, relation: p.relation.clone(), ..Default::default() };
    for d in &p.defaults {
        match d.field_id {
            FieldRef::Title => {
                if let Some(s) = d.value.as_str() {
                    out.title = single_line(&expand(s, ctx), MAX_TITLE_CHARS);
                }
            }
            FieldRef::Field(id) => {
                let Some(f) = fields.iter().find(|f| f.id == id) else {
                    out.skipped_fields.push(id);
                    continue;
                };
                match resolve_value(f, &d.value, ctx) {
                    Some(v) => out.values.push((id, v)),
                    None => out.skipped_fields.push(id),
                }
            }
        }
    }
    out
}

fn resolve_value(f: &Field, v: &Value, ctx: &ResolveContext) -> Option<Value> {
    let resolved = match f.kind {
        FieldKind::Text => {
            let max = f.options.max_length.map(|m| m as usize).unwrap_or(MAX_EXPANDED_CHARS);
            Value::String(single_line(&expand(v.as_str()?, ctx), max))
        }
        FieldKind::Rich => Value::String(multi_line(&expand(v.as_str()?, ctx), MAX_EXPANDED_CHARS)),
        FieldKind::Labels => {
            let mut seen: Vec<String> = Vec::new();
            let mut list = Vec::new();
            for x in v.as_array()? {
                let t = single_line(&expand(x.as_str()?, ctx), MAX_LABEL_CHARS);
                if !t.is_empty() && !seen.contains(&t.to_lowercase()) {
                    seen.push(t.to_lowercase());
                    list.push(Value::String(t));
                }
            }
            Value::Array(list)
        }
        FieldKind::Ticket => match v.as_str() {
            Some(SOURCE_REF) => Value::from(ctx.source.as_ref()?.id),
            _ => v.clone(),
        },
        _ => v.clone(),
    };
    check_value(f.kind, &f.options, &resolved, true).ok()?;
    Some(resolved)
}

/// Der Prompt, den „Ticketinhalt als Prompt“ an Claude übergibt (zweisprachig; die Beschreibung ist gekürzt).
pub fn ticket_prompt(s: &SourceContext, lang: Lang) -> String {
    let head = match lang {
        Lang::De => format!("Arbeite am Ticket {}: {}", s.key, single_line(&s.title, MAX_TITLE_CHARS)),
        Lang::En => format!("Work on ticket {}: {}", s.key, single_line(&s.title, MAX_TITLE_CHARS)),
    };
    let body = multi_line(s.description.trim(), MAX_PROMPT_DESCRIPTION);
    if body.trim().is_empty() { head } else { format!("{head}\n\n{body}") }
}

/// Fertige Angaben zum Start einer Sitzung.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedSession {
    pub shell: Option<String>,
    pub claude: bool,
    /// Nur mit Claude und wenn `prompt_from_ticket` gesetzt ist und ein Ausgangsticket da ist.
    pub prompt: Option<String>,
    /// Arbeitsordner des Projekts, wenn gewünscht und gesetzt.
    pub cwd: Option<String>,
}

pub fn resolve_session(p: &StartSessionParams, source: Option<&SourceContext>, project_work_dir: &str, lang: Lang) -> ResolvedSession {
    let wd = project_work_dir.trim();
    ResolvedSession {
        shell: p.shell.clone(),
        claude: p.claude,
        prompt: if p.claude && p.prompt_from_ticket { source.map(|s| ticket_prompt(s, lang)) } else { None },
        cwd: if p.cwd_from_project && !wd.is_empty() { Some(wd.to_string()) } else { None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::fields::{FieldOptions, OptionValue};
    use crate::planning::quick::params::DefaultValue;
    use serde_json::json;

    fn ctx() -> ResolveContext {
        ResolveContext {
            source: Some(SourceContext {
                id: 41,
                key: "PAN-12".into(),
                number: 12,
                title: "Login page".into(),
                type_name: "User story".into(),
                status_name: "In Progress".into(),
                description: "Line 1\nLine 2".into(),
            }),
            project_key: "PAN".into(),
            project_name: "Paneldeck".into(),
            today: "2026-11-14".into(),
        }
    }

    fn field(id: i64, kind: FieldKind, options: FieldOptions) -> Field {
        Field { id, type_id: 1, key: None, name: Some(format!("F{id}")), kind, options, required: false, default_value: None, help: String::new(), section: String::new(), position: 0, show_on_card: false, builtin: false }
    }

    #[test]
    fn replaces_known_placeholders() {
        let c = ctx();
        assert_eq!(expand("{{source.key}}: {{source.title}}", &c), "PAN-12: Login page");
        assert_eq!(expand("{{project.key}} / {{project.name}} / {{today}}", &c), "PAN / Paneldeck / 2026-11-14");
        assert_eq!(expand("{{source.id}}-{{source.number}}-{{source.type}}-{{source.status}}", &c), "41-12-User story-In Progress");
        assert_eq!(expand("{{ SOURCE.KEY }}", &c), "PAN-12", "Leerraum und Groß-/Kleinschreibung sind egal");
    }

    #[test]
    fn unknown_placeholders_stay_and_are_rejected_on_save() {
        let c = ctx();
        assert_eq!(expand("a {{nope}} b {{source.key}}", &c), "a {{nope}} b PAN-12");
        assert_eq!(check_placeholders("{{source.key}} {{today}}"), Ok(()));
        assert_eq!(check_placeholders("{{source}}"), Err(QuickError::Placeholder("source".into())));
        assert_eq!(check_placeholders("x {{evil.call()}} y"), Ok(()), "kein gültiger Platzhaltername = einfacher Text");
        assert_eq!(check_placeholders("{{os.env}}"), Err(QuickError::Placeholder("os.env".into())));
    }

    #[test]
    fn substituted_text_is_never_rescanned() {
        let mut c = ctx();
        c.source.as_mut().unwrap().title = "{{project.key}} {{source.description}}".into();
        assert_eq!(expand("{{source.title}}", &c), "{{project.key}} {{source.description}}");
        // auch ein Titel, der wie ein Escape aussieht, bleibt Text
        c.source.as_mut().unwrap().title = "{{{{".into();
        assert_eq!(expand("[{{source.title}}]", &c), "[{{{{]");
    }

    #[test]
    fn escape_gives_literal_braces() {
        let c = ctx();
        assert_eq!(expand("{{{{source.key}}", &c), "{{source.key}}");
        assert_eq!(expand("a{{{{b", &c), "a{{b");
    }

    #[test]
    fn missing_source_gives_empty_text() {
        let c = ResolveContext { project_key: "PAN".into(), ..Default::default() };
        assert_eq!(expand("[{{source.key}}] {{project.key}}", &c), "[] PAN");
    }

    #[test]
    fn evil_inputs_do_not_hang_or_crash() {
        let c = ctx();
        let many_open = "{{a".repeat(50_000);
        let t = std::time::Instant::now();
        let r = expand(&many_open, &c);
        assert!(t.elapsed().as_secs() < 2, "quadratischer Aufwand?");
        assert_eq!(r.chars().count(), MAX_EXPANDED_CHARS, "das Ergebnis ist begrenzt");
        assert!(r.starts_with("{{a{{a{{a"));
        for s in ["{{", "}}", "{", "{{}}", "{{ }}", "{{{", "{{{{{{{{", "{{source.key", "{{é}}", "ä{{source.key}}ö", "{{\0}}", "}}{{source.key}}{{"] {
            let _ = expand(s, &c);
            let _ = placeholders_in(s);
        }
        assert_eq!(expand("ä{{source.key}}ö", &c), "äPAN-12ö");
        assert_eq!(expand("{{é}}", &c), "{{é}}");
    }

    #[test]
    fn substitution_and_result_are_bounded() {
        let mut c = ctx();
        c.source.as_mut().unwrap().description = "x".repeat(MAX_SUBSTITUTION_CHARS * 3);
        assert_eq!(expand("{{source.description}}", &c).chars().count(), MAX_SUBSTITUTION_CHARS);
        let many = "{{source.description}}".repeat(100);
        assert_eq!(expand(&many, &c).chars().count(), MAX_EXPANDED_CHARS);
    }

    #[test]
    fn single_and_multi_line_cleaning() {
        assert_eq!(single_line("  a\r\n b\t\tc\u{0007} ", 50), "a b c");
        assert_eq!(single_line("abcdef", 3), "abc");
        assert_eq!(multi_line("a\r\nb\rc\u{0000}d\te", 50), "a\nb\ncd\te");
        assert_eq!(multi_line("abc", 2), "ab");
    }

    #[test]
    fn resolves_title_and_typed_values() {
        let fields = vec![
            field(1, FieldKind::Text, FieldOptions { max_length: Some(12), ..Default::default() }),
            field(2, FieldKind::Rich, FieldOptions::default()),
            field(3, FieldKind::Dropdown, FieldOptions { values: vec![OptionValue { id: 1, name: Some("A".into()), ..Default::default() }], ..Default::default() }),
            field(4, FieldKind::Ticket, FieldOptions::default()),
            field(5, FieldKind::Labels, FieldOptions::default()),
            field(6, FieldKind::Number, FieldOptions { max: Some(10.0), ..Default::default() }),
        ];
        let p = CreateTicketParams {
            ticket_type_id: 9,
            relation: None,
            defaults: vec![
                DefaultValue { field_id: FieldRef::Title, value: json!("Dev task for {{source.key}}\n{{source.title}}") },
                DefaultValue { field_id: FieldRef::Field(1), value: json!("{{source.title}} and more") },
                DefaultValue { field_id: FieldRef::Field(2), value: json!("From {{source.key}}\r\n\r\n{{source.description}}") },
                DefaultValue { field_id: FieldRef::Field(3), value: json!(1) },
                DefaultValue { field_id: FieldRef::Field(4), value: json!("{{source}}") },
                DefaultValue { field_id: FieldRef::Field(5), value: json!(["{{project.key}}", "pan", "  ", "New"]) },
                DefaultValue { field_id: FieldRef::Field(6), value: json!(11) },
                DefaultValue { field_id: FieldRef::Field(99), value: json!("gone") },
            ],
        };
        let r = resolve_with_fields(&fields, &p, &ctx());
        assert_eq!(r.ticket_type_id, 9);
        assert_eq!(r.title, "Dev task for PAN-12 Login page");
        assert_eq!(
            r.values,
            vec![
                (1, json!("Login page a")),
                (2, json!("From PAN-12\n\nLine 1\nLine 2")),
                (3, json!(1)),
                (4, json!(41)),
                (5, json!(["PAN", "New"])),
            ]
        );
        assert_eq!(r.skipped_fields, vec![6, 99], "Wert außerhalb der Grenzen und gelöschtes Feld werden übersprungen");
    }

    #[test]
    fn source_reference_without_source_is_skipped() {
        let fields = vec![field(4, FieldKind::Ticket, FieldOptions::default())];
        let p = CreateTicketParams { ticket_type_id: 1, relation: None, defaults: vec![DefaultValue { field_id: FieldRef::Field(4), value: json!("{{source}}") }] };
        let r = resolve_with_fields(&fields, &p, &ResolveContext::default());
        assert!(r.values.is_empty());
        assert_eq!(r.skipped_fields, vec![4]);
    }

    #[test]
    fn evil_title_is_cleaned() {
        let mut c = ctx();
        c.source.as_mut().unwrap().title = format!("a\nb\u{0000}{}", "z".repeat(1000));
        let p = CreateTicketParams { ticket_type_id: 1, relation: None, defaults: vec![DefaultValue { field_id: FieldRef::Title, value: json!("{{source.title}}") }] };
        let r = resolve_with_fields(&[], &p, &c);
        assert!(r.title.starts_with("a b "));
        assert_eq!(r.title.chars().count(), MAX_TITLE_CHARS);
        assert!(!r.title.contains('\n') && !r.title.contains('\0'));
    }

    #[test]
    fn prompt_and_session_resolution() {
        let s = ctx().source.unwrap();
        assert_eq!(ticket_prompt(&s, Lang::En), "Work on ticket PAN-12: Login page\n\nLine 1\nLine 2");
        assert_eq!(ticket_prompt(&s, Lang::De), "Arbeite am Ticket PAN-12: Login page\n\nLine 1\nLine 2");
        let mut empty = s.clone();
        empty.description = "  ".into();
        assert_eq!(ticket_prompt(&empty, Lang::En), "Work on ticket PAN-12: Login page");
        let p = StartSessionParams { shell: Some("pwsh".into()), claude: true, prompt_from_ticket: true, cwd_from_project: true };
        let r = resolve_session(&p, Some(&s), " C:\\work\\pan ", Lang::En);
        assert_eq!(r.shell.as_deref(), Some("pwsh"));
        assert!(r.claude);
        assert_eq!(r.cwd.as_deref(), Some("C:\\work\\pan"));
        assert!(r.prompt.unwrap().starts_with("Work on ticket PAN-12"));
        // ohne Claude nie ein Prompt, ohne Arbeitsordner kein Startordner, ohne Ausgangsticket kein Prompt
        let q = StartSessionParams { claude: false, prompt_from_ticket: true, cwd_from_project: true, ..Default::default() };
        let r = resolve_session(&q, Some(&s), "", Lang::En);
        assert_eq!((r.prompt, r.cwd), (None, None));
        let q = StartSessionParams { claude: true, prompt_from_ticket: true, ..Default::default() };
        assert_eq!(resolve_session(&q, None, "x", Lang::En).prompt, None);
    }
}
