//! Umgebungsvariablen je Sitzung (#384, 3.2.0). Kennt kein egui und keine App.
//!
//! - **Datenformat** ([`EnvVar`]): `state.json` `sessions[].env` und `templates.json` `items[].env` – je Variable Name,
//!   Wert (nur bei normalen Werten im Klartext), `value_protected` (geheime Werte, verschlüsselt wie die Prozessliste:
//!   DPAPI unter Windows, Schlüsselbund/Master-Passwort unter Linux/macOS), `secret`, `mode` (`replace`/`prepend`/
//!   `append`), `remove`, `wsl_path`, `by_agent`. Alle Felder sind optional, unbekannte Werte laden nachsichtig.
//! - **Ebenen:** Profil < Vorlage < Sitzung (gleicher Name → die höhere gewinnt, [`merge`]); die Vorlage wird beim Start
//!   in die Sitzung kopiert. Unter Windows zählt der Name ohne Groß-/Kleinschreibung ([`same_name`]).
//! - **Geschützte Namen** ([`check_name`]): `PANELDECK_*` und die internen Variablen (`terminal::INTERNAL_ENV`) setzt
//!   kein Profil, keine Vorlage und keine Sitzung; Namen der Shell-Integration ([`warn_name`]) gehen, Paneldecks Werte
//!   gewinnen aber (WSLENV wird zusammengeführt).
//! - **Werte sind wörtlich** (kein `%VAR%`/`$VAR`); das gilt auch für das Setzen im laufenden Terminal ([`live_command`]).
//! - **Agenten** setzen Variablen nur mit Schalter und nur in selbst angelegten Sitzungen ([`agent_vars`]: feste
//!   Sperrliste, eigene Muster, höchstens 20 Variablen, Werte bis 1 KB ohne Steuerzeichen, nie geheim) und sehen nur
//!   Namen und Kennzeichen ([`exposed`]), nie Werte.

use serde::{Deserialize, Deserializer, Serialize};

/// Höchstzahl der Variablen einer Sitzung (Editor).
pub const MAX_VARS: usize = 100;
/// Höchstzahl der Variablen, die ein Agent mit `create_session` setzen darf.
pub const AGENT_MAX_VARS: usize = 20;
/// Längster Wert (Bytes), den ein Agent setzen darf.
pub const AGENT_MAX_VALUE: usize = 1024;
/// Längster Wert (Bytes) im Editor.
pub const MAX_VALUE: usize = 32 * 1024;

fn is_false(b: &bool) -> bool {
    !*b
}

/// Wie eine Variable wirkt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Wert ersetzen.
    #[default]
    Replace,
    /// Vor den geerbten Wert setzen (Trennzeichen der Plattform, `;` bzw. `:`) – für PATH und andere Listen.
    Prepend,
    /// Hinter den geerbten Wert setzen.
    Append,
}

impl Mode {
    fn is_replace(&self) -> bool {
        *self == Mode::Replace
    }

    /// Nachsichtig lesen: unbekannte Werte und falsche Typen zählen als „ersetzen“.
    fn lenient<'de, D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        Ok(serde_json::from_value(v).unwrap_or_default())
    }

    pub fn label(self, lang: crate::state::Lang) -> &'static str {
        match self {
            Mode::Replace => lang.t("Ersetzen", "Replace"),
            Mode::Prepend => lang.t("Voranstellen", "Prepend"),
            Mode::Append => lang.t("Anhängen", "Append"),
        }
    }

    pub const ALL: [Mode; 3] = [Mode::Replace, Mode::Prepend, Mode::Append];
}

/// Eine Umgebungsvariable einer Sitzung oder Vorlage.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EnvVar {
    pub name: String,
    /// Wert im Klartext – nur bei normalen Werten (bei `secret` immer leer).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub value: String,
    /// Verschlüsselter Wert (`llm::secret::protect`, Base64) – nur bei `secret`; fehlt = kein Wert gespeichert.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_protected: Option<String>,
    /// Geheimer Wert: verschlüsselt, maskiert, nie getippt, exportiert oder an Agenten gegeben.
    #[serde(skip_serializing_if = "is_false")]
    pub secret: bool,
    #[serde(skip_serializing_if = "Mode::is_replace", deserialize_with = "Mode::lenient")]
    pub mode: Mode,
    /// Geerbte Variable entfernen (statt sie zu setzen).
    #[serde(skip_serializing_if = "is_false")]
    pub remove: bool,
    /// WSL: Wert als Pfad umrechnen (`WSLENV …/p`, bei einer Liste `/l`).
    #[serde(skip_serializing_if = "is_false")]
    pub wsl_path: bool,
    /// Von einem Agenten gesetzt (`create_session` mit `env`).
    #[serde(skip_serializing_if = "is_false")]
    pub by_agent: bool,
}

impl EnvVar {
    /// Normale Variable mit Wert (ersetzen).
    pub fn plain(name: &str, value: &str) -> EnvVar {
        EnvVar { name: name.to_string(), value: value.to_string(), ..EnvVar::default() }
    }

    /// Geheime Variable: Wert verschlüsselt. `Err`, wenn das hier nicht geht (z. B. Master-Passwort fehlt).
    pub fn secret(name: &str, value: &str) -> Result<EnvVar, crate::llm::secret::SecretError> {
        Ok(EnvVar { name: name.to_string(), value_protected: Some(crate::llm::secret::protect(value)?), secret: true, ..EnvVar::default() })
    }

}

// ---------------------------------------------------------------------------------------------------------------
// Namen

/// Gleicher Name? Unter Windows ohne Groß-/Kleinschreibung (wie die Umgebung selbst), sonst genau.
pub fn same_name(a: &str, b: &str) -> bool {
    if cfg!(windows) { a.eq_ignore_ascii_case(b) } else { a == b }
}

/// Warum ein Name nicht geht.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameError {
    Empty,
    /// `=`, Leer- oder Steuerzeichen, zu lang.
    Invalid,
    /// `PANELDECK_*` oder eine interne Variable.
    Protected,
}

impl NameError {
    pub fn text(self, lang: crate::state::Lang) -> &'static str {
        match self {
            NameError::Empty => lang.t("Name fehlt", "Name missing"),
            NameError::Invalid => lang.t("Ungültiger Name (kein =, keine Leer- oder Steuerzeichen)", "Invalid name (no =, spaces or control characters)"),
            NameError::Protected => lang.t("Geschützter Name (PANELDECK_* und interne Variablen setzt nur Paneldeck)", "Protected name (only Paneldeck sets PANELDECK_* and internal variables)"),
        }
    }
}

/// Name prüfen: nicht leer, ohne `=`/Leer-/Steuerzeichen, höchstens 255 Zeichen, nicht `PANELDECK_*` oder intern.
pub fn check_name(name: &str) -> Result<(), NameError> {
    let n = name.trim();
    if n.is_empty() {
        return Err(NameError::Empty);
    }
    // `:` und `/` trennen in WSLENV Einträge bzw. Kennzeichen (`X:PATH/l` reichte sonst den Windows-PATH nach WSL).
    if n.len() > 255 || n.chars().any(|c| matches!(c, '=' | ':' | '/') || !printable(c)) {
        return Err(NameError::Invalid);
    }
    if is_protected(n) {
        return Err(NameError::Protected);
    }
    Ok(())
}

/// Sichtbares Zeichen? Keine Steuer-, Leer- oder unsichtbaren Formatzeichen (Bidi-Steuerung, Nullbreite, BOM).
fn printable(c: char) -> bool {
    !(c.is_control() || c.is_whitespace() || matches!(c, '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}'))
}

/// `PANELDECK_*` oder eine interne Variable (Groß-/Kleinschreibung egal)?
pub fn is_protected(name: &str) -> bool {
    crate::terminal::is_internal_env(name) || name.trim().to_ascii_uppercase().starts_with("PANELDECK_")
}

/// Hinweis zu einem erlaubten Namen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameWarn {
    /// Name der Shell-Integration: Paneldecks Wert gewinnt (WSLENV wird zusammengeführt).
    ShellIntegration,
    /// Claude Code wertet ihn selbst aus (`ANTHROPIC_*`, `CLAUDE_CODE_*`).
    Claude,
}

impl NameWarn {
    pub fn text(self, lang: crate::state::Lang) -> &'static str {
        match self {
            NameWarn::ShellIntegration => lang.t(
                "Die Shell-Integration setzt diese Variable selbst – Paneldecks Wert gewinnt (WSLENV wird zusammengeführt).",
                "Shell integration sets this variable itself – Paneldeck's value wins (WSLENV is merged).",
            ),
            NameWarn::Claude => lang.t(
                "Claude Code wertet diese Variable selbst aus (Anmeldung, Modell, Verhalten).",
                "Claude Code evaluates this variable itself (sign-in, model, behaviour).",
            ),
        }
    }
}

/// Namen der Shell-Integration (Paneldeck setzt sie selbst).
const SHELL_INTEGRATION: [&str; 5] = ["PROMPT", "PROMPT_COMMAND", "PS0", "ZDOTDIR", "WSLENV"];

pub fn warn_name(name: &str) -> Option<NameWarn> {
    let n = name.trim().to_ascii_uppercase();
    if SHELL_INTEGRATION.contains(&n.as_str()) {
        return Some(NameWarn::ShellIntegration);
    }
    if n.starts_with("ANTHROPIC_") || n.starts_with("CLAUDE_CODE_") {
        return Some(NameWarn::Claude);
    }
    None
}

/// Sieht der Name nach einem Geheimnis aus (TOKEN, KEY, SECRET, PASS, PWD, AUTH)? Dann ist „geheim“ vorbelegt.
pub fn looks_secret(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    ["TOKEN", "KEY", "SECRET", "PASS", "PWD", "AUTH"].iter().any(|w| n.contains(w))
}

// ---------------------------------------------------------------------------------------------------------------
// Laden

/// Liste nachsichtig lesen (`state.json`, `templates.json`): kein Array → leer, unlesbare oder geschützte Einträge fallen
/// weg, doppelte Namen → der letzte gilt, höchstens [`MAX_VARS`]. Ein von Hand eingetragener Klartext-Wert einer
/// geheimen Variable wird sofort verschlüsselt (oder verworfen, wenn das nicht geht) – nie im Klartext behalten.
pub fn lenient_list<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<EnvVar>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    let items = v.as_array().cloned().unwrap_or_default();
    let parsed = items.into_iter().filter_map(|x| serde_json::from_value::<EnvVar>(x).ok());
    Ok(clean(parsed))
}

/// Bereinigen wie beim Laden ([`lenient_list`]).
pub fn clean(list: impl IntoIterator<Item = EnvVar>) -> Vec<EnvVar> {
    let mut out: Vec<EnvVar> = Vec::new();
    for mut e in list {
        e.name = e.name.trim().to_string();
        if check_name(&e.name).is_err() {
            continue;
        }
        if e.secret {
            if !e.value.is_empty() {
                e.value_protected = crate::llm::secret::protect(&e.value).ok();
                e.value.clear();
            }
        } else {
            e.value_protected = None;
        }
        if e.remove {
            e.value.clear();
            e.value_protected = None;
            e.mode = Mode::Replace;
        }
        out.retain(|x| !same_name(&x.name, &e.name));
        out.push(e);
    }
    if out.len() > MAX_VARS {
        out.drain(..out.len() - MAX_VARS);
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// Ebenen

/// Variablen eines Profils (`NAME=VALUE` je Zeile) als Liste; geschützte Namen fallen weg (#384: ein Profil kann
/// `PANELDECK_SESSION`/`PANELDECK_SESSION_KEY` nicht mehr überschreiben).
pub fn from_profile(lines: &str) -> Vec<EnvVar> {
    let vars = lines.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| EnvVar::plain(k.trim(), v.trim())).filter(|e| check_name(&e.name).is_ok());
    clean(vars)
}

/// Zwei Ebenen zusammenführen: `upper` gewinnt bei gleichem Namen ([`same_name`]). Reihenfolge: `lower` (ohne die
/// überschriebenen), dann `upper`.
pub fn merge(lower: &[EnvVar], upper: &[EnvVar]) -> Vec<EnvVar> {
    let mut out: Vec<EnvVar> = lower.iter().filter(|l| !upper.iter().any(|u| same_name(&u.name, &l.name))).cloned().collect();
    out.extend(upper.iter().cloned());
    out
}

// ---------------------------------------------------------------------------------------------------------------
// Anwenden

/// Was mit einer Variable beim Start geschieht (Werte entschlüsselt).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpKind {
    Set(String),
    Prepend(String),
    Append(String),
    Remove,
}

/// Eine Variable, fertig zum Anwenden.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Op {
    pub name: String,
    pub kind: OpKind,
    pub secret: bool,
    pub wsl_path: bool,
}

/// Geheime Werte, die sich nicht entschlüsseln ließen: die Sitzung wartet („gesperrt“).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Locked {
    pub names: Vec<String>,
    pub error: crate::llm::secret::SecretError,
}

impl Locked {
    /// Muss der Nutzer die Werte neu eingeben (DPAPI-Fehler, beschädigt)? Sonst genügt Entsperren (Master-Passwort,
    /// Schlüsselbund).
    pub fn needs_reentry(&self) -> bool {
        use crate::llm::secret::SecretError as E;
        matches!(self.error, E::Encoding | E::Utf8 | E::Sys(_) | E::Unsupported | E::WrongPassword)
    }
}

/// Liste in anwendbare Schritte umsetzen; geheime Werte werden mit `unprotect` entschlüsselt. Variablen ohne Wert
/// (und nicht `remove`) und geschützte Namen fallen weg. Lässt sich ein geheimer Wert nicht entschlüsseln: [`Locked`].
pub fn resolve(vars: &[EnvVar], unprotect: impl Fn(&str) -> Result<String, crate::llm::secret::SecretError>) -> Result<Vec<Op>, Locked> {
    let mut ops = Vec::new();
    let mut locked: Option<Locked> = None;
    for e in vars {
        if check_name(&e.name).is_err() {
            continue;
        }
        let value = if e.remove {
            None
        } else if e.secret {
            match e.value_protected.as_deref().filter(|b| !b.trim().is_empty()) {
                None => continue,
                Some(b) => match unprotect(b) {
                    Ok(v) => Some(v),
                    Err(err) => {
                        let l = locked.get_or_insert(Locked { names: Vec::new(), error: err });
                        l.names.push(e.name.clone());
                        continue;
                    }
                },
            }
        } else {
            Some(e.value.clone())
        };
        let kind = match (value, e.mode) {
            (None, _) => OpKind::Remove,
            (Some(v), Mode::Replace) => OpKind::Set(v),
            (Some(v), _) if v.is_empty() => continue,
            (Some(v), Mode::Prepend) => OpKind::Prepend(v),
            (Some(v), Mode::Append) => OpKind::Append(v),
        };
        ops.push(Op { name: e.name.clone(), kind, secret: e.secret, wsl_path: e.wsl_path });
    }
    match locked {
        Some(l) => Err(l),
        None => Ok(ops),
    }
}

/// Trennzeichen für Listen der Plattform (`;` unter Windows, sonst `:`).
pub fn list_sep() -> char {
    if cfg!(windows) { ';' } else { ':' }
}

/// Neuer Wert einer Variable: `base` = geerbter Wert. `None` = entfernen.
pub fn apply_one(kind: &OpKind, base: Option<&str>, sep: char) -> Option<String> {
    let base = base.filter(|b| !b.is_empty());
    match kind {
        OpKind::Set(v) => Some(v.clone()),
        OpKind::Remove => None,
        OpKind::Prepend(v) => Some(match base {
            Some(b) => format!("{v}{sep}{b}"),
            None => v.clone(),
        }),
        OpKind::Append(v) => Some(match base {
            Some(b) => format!("{b}{sep}{v}"),
            None => v.clone(),
        }),
    }
}

/// WSL: Kennzeichen für `WSLENV` (`/p` Pfad, `/l` Pfadliste, sonst leer).
pub fn wslenv_flag(op: &Op) -> &'static str {
    if !op.wsl_path {
        return "";
    }
    match &op.kind {
        OpKind::Set(v) if v.contains(';') => "/l",
        _ => "/p",
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Im laufenden Terminal setzen

/// Syntax der Shell für das Setzen im laufenden Terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveShell {
    Pwsh,
    Cmd,
    /// bash, zsh, sh (auch WSL mit unbekannter Login-Shell)
    Posix,
    Fish,
}

impl LiveShell {
    pub fn of(flavor: crate::shellinit::Flavor) -> LiveShell {
        use crate::shellinit::Flavor;
        match flavor {
            Flavor::Pwsh => LiveShell::Pwsh,
            Flavor::Cmd => LiveShell::Cmd,
            Flavor::Bash | Flavor::Zsh => LiveShell::Posix,
            Flavor::Fish => LiveShell::Fish,
        }
    }

    /// Listen-Trennzeichen in dieser Shell (`wsl` = läuft in WSL).
    pub fn sep(self, wsl: bool) -> char {
        if wsl || !cfg!(windows) {
            return ':';
        }
        match self {
            LiveShell::Pwsh | LiveShell::Cmd => ';',
            // Git Bash: PATH ist in der Shell eine POSIX-Liste.
            LiveShell::Posix | LiveShell::Fish => ':',
        }
    }
}

/// Warum eine Änderung nicht im laufenden Terminal gesetzt werden kann.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveIssue {
    /// Geheime Werte nur per Neustart (nie getippt).
    Secret,
    /// Name ist kein einfacher Bezeichner (A–Z, 0–9, _).
    Name,
    /// Wert mit Steuerzeichen bzw. (cmd) mit `%`, `!` oder `"` – ließe sich nicht wörtlich setzen.
    Value,
    /// Gelöschte Zeile oder geänderte Voranstellen/Anhängen-Variable: der alte Stand ist im Terminal nicht bekannt.
    NeedsRestart,
}

impl LiveIssue {
    pub fn text(self, lang: crate::state::Lang) -> &'static str {
        match self {
            LiveIssue::Secret => lang.t("geheimer Wert – nur per Neustart", "secret value – only by restart"),
            LiveIssue::Name => lang.t("Name lässt sich nicht sicher tippen", "name cannot be typed safely"),
            LiveIssue::Value => lang.t("Wert lässt sich in dieser Shell nicht wörtlich setzen", "value cannot be set literally in this shell"),
            LiveIssue::NeedsRestart => lang.t("gilt erst nach einem Neustart", "applies only after a restart"),
        }
    }
}

fn simple_name(n: &str) -> bool {
    let mut c = n.chars();
    matches!(c.next(), Some(f) if f.is_ascii_alphabetic() || f == '_') && c.all(|x| x.is_ascii_alphanumeric() || x == '_')
}

/// Kann diese Änderung in dieser Shell wörtlich gesetzt werden? `wsl` = die Shell läuft in WSL.
pub fn live_issue(shell: LiveShell, wsl: bool, op: &Op) -> Option<LiveIssue> {
    if op.secret {
        return Some(LiveIssue::Secret);
    }
    if !simple_name(&op.name) {
        return Some(LiveIssue::Name);
    }
    // Git Bash: Listen sind in der Shell POSIX-Listen mit `:`, die Werte aber Windows-Pfade – nur per Neustart (dann
    // rechnet MSYS die ganze Windows-Liste um).
    if cfg!(windows) && !wsl && matches!(shell, LiveShell::Posix | LiveShell::Fish) && matches!(op.kind, OpKind::Prepend(_) | OpKind::Append(_)) {
        return Some(LiveIssue::NeedsRestart);
    }
    let v = match &op.kind {
        OpKind::Set(v) | OpKind::Prepend(v) | OpKind::Append(v) => v.as_str(),
        OpKind::Remove => "",
    };
    if v.chars().any(|c| c.is_control()) || (shell == LiveShell::Cmd && v.chars().any(|c| matches!(c, '%' | '!' | '"'))) {
        return Some(LiveIssue::Value);
    }
    None
}

/// Befehl, der die Änderungen in der laufenden Shell setzt – sichtbar, mit führendem Leerzeichen (bleibt so, wo die
/// Shell das kennt, aus dem Verlauf). Werte werden für die Shell gequotet und nie expandiert. `None`, wenn nichts zu
/// setzen ist; Schritte mit [`live_issue`] fallen weg (der Aufrufer meldet sie). Voranstellen/Anhängen prüft in jeder
/// Shell, ob es die Variable schon gibt (sonst nur der Wert, ohne Trennzeichen und ohne wörtliches `%NAME%`).
pub fn live_command(shell: LiveShell, wsl: bool, ops: &[Op]) -> Option<String> {
    use crate::quote::{quote, Kind};
    let sep = shell.sep(wsl);
    let parts: Vec<String> = ops
        .iter()
        .filter(|o| live_issue(shell, wsl, o).is_none())
        .map(|o| {
            let n = &o.name;
            match shell {
                LiveShell::Pwsh => {
                    let q = |v: &str| quote(Kind::Pwsh, v);
                    match &o.kind {
                        OpKind::Set(v) => format!("$env:{n} = {}", q(v)),
                        OpKind::Prepend(v) => format!("$env:{n} = if ($env:{n}) {{ {} + $env:{n} }} else {{ {} }}", q(&format!("{v}{sep}")), q(v)),
                        OpKind::Append(v) => format!("$env:{n} = if ($env:{n}) {{ $env:{n} + {} }} else {{ {} }}", q(&format!("{sep}{v}")), q(v)),
                        OpKind::Remove => format!("Remove-Item Env:{n} -ErrorAction SilentlyContinue"),
                    }
                }
                LiveShell::Cmd => match &o.kind {
                    OpKind::Set(v) => format!("set \"{n}={v}\""),
                    OpKind::Prepend(v) => format!("if defined {n} (set \"{n}={v}{sep}%{n}%\") else (set \"{n}={v}\")"),
                    OpKind::Append(v) => format!("if defined {n} (set \"{n}=%{n}%{sep}{v}\") else (set \"{n}={v}\")"),
                    OpKind::Remove => format!("set \"{n}=\""),
                },
                LiveShell::Posix => {
                    let q = |v: &str| quote(Kind::Posix, v);
                    match &o.kind {
                        OpKind::Set(v) => format!("export {n}={}", q(v)),
                        OpKind::Prepend(v) => format!("export {n}={}\"${{{n}:+{sep}${n}}}\"", q(v)),
                        OpKind::Append(v) => format!("export {n}=\"${{{n}:+${n}{sep}}}\"{}", q(v)),
                        OpKind::Remove => format!("unset {n}"),
                    }
                }
                LiveShell::Fish => {
                    let q = |v: &str| quote(Kind::Fish, v);
                    // fish führt *PATH-Variablen als Liste (beim Exportieren mit `:` verbunden).
                    let list = n.ends_with("PATH");
                    match &o.kind {
                        OpKind::Set(v) => format!("set -gx {n} {}", q(v)),
                        OpKind::Prepend(v) if list => format!("set -gx {n} {} ${n}", q(v)),
                        OpKind::Append(v) if list => format!("set -gx {n} ${n} {}", q(v)),
                        OpKind::Prepend(v) => format!("if set -q {n}; and test -n \"${n}\"; set -gx {n} {}\"${n}\"; else; set -gx {n} {}; end", q(&format!("{v}{sep}")), q(v)),
                        OpKind::Append(v) => format!("if set -q {n}; and test -n \"${n}\"; set -gx {n} \"${n}\"{}; else; set -gx {n} {}; end", q(&format!("{sep}{v}")), q(v)),
                        OpKind::Remove => format!("set -e {n}"),
                    }
                }
            }
        })
        .collect();
    if parts.is_empty() {
        return None;
    }
    let join = if shell == LiveShell::Cmd { " & " } else { "; " };
    Some(format!(" {}", parts.join(join)))
}

/// Taste, die vor „Jetzt setzen“ die Eingabezeile leert (sonst hinge der Befehl an Getipptem): Strg+U in bash/zsh/fish
/// (auch WSL), Esc in PowerShell (PSReadLine) und cmd. Kodiert wird sie wie die Tastatur (`vtkeys::key_bytes`).
pub fn clear_line_key(shell: LiveShell) -> &'static str {
    match shell {
        LiveShell::Posix | LiveShell::Fish => "ctrl+u",
        LiveShell::Pwsh | LiveShell::Cmd => "esc",
    }
}

/// Gleiche Variablen (ohne Rücksicht auf die Reihenfolge)?
pub fn same_set(a: &[EnvVar], b: &[EnvVar]) -> bool {
    a.len() == b.len() && a.iter().all(|x| b.iter().any(|y| y == x))
}

/// Stand der Shell nach „Jetzt setzen“: die gesetzten Variablen aus `new` in `live` übernehmen.
pub fn apply_live(live: &mut Vec<EnvVar>, new: &[EnvVar], applied: &[Op]) {
    for op in applied {
        live.retain(|v| !same_name(&v.name, &op.name));
        if let Some(n) = new.iter().find(|n| same_name(&n.name, &op.name)) {
            live.push(n.clone());
        }
    }
}

/// Was sich zwischen `old` und `new` geändert hat – als Schritte für das Setzen im laufenden Terminal und die
/// Änderungen, die erst nach einem Neustart gelten ([`LiveIssue::NeedsRestart`] oder [`LiveIssue::Secret`]).
/// Geheime Werte werden hier nie entschlüsselt.
pub fn live_changes(old: &[EnvVar], new: &[EnvVar]) -> (Vec<Op>, Vec<(String, LiveIssue)>) {
    let (mut ops, mut later) = (Vec::new(), Vec::new());
    for n in new {
        let before = old.iter().find(|o| same_name(&o.name, &n.name));
        if before == Some(n) {
            continue;
        }
        if n.secret || before.is_some_and(|b| b.secret) {
            later.push((n.name.clone(), LiveIssue::Secret));
            continue;
        }
        let was_list = before.is_some_and(|b| b.mode != Mode::Replace && !b.remove);
        if was_list || (before.is_some() && n.mode != Mode::Replace && !n.remove) {
            // Der alte Zusatz steckt schon im Wert der Shell; ihn zu ersetzen geht nur mit einem Neustart.
            later.push((n.name.clone(), LiveIssue::NeedsRestart));
            continue;
        }
        let kind = if n.remove {
            OpKind::Remove
        } else {
            match n.mode {
                Mode::Replace => OpKind::Set(n.value.clone()),
                Mode::Prepend => OpKind::Prepend(n.value.clone()),
                Mode::Append => OpKind::Append(n.value.clone()),
            }
        };
        ops.push(Op { name: n.name.clone(), kind, secret: false, wsl_path: n.wsl_path });
    }
    for o in old {
        if !new.iter().any(|n| same_name(&n.name, &o.name)) {
            // Gelöschte Zeile: der geerbte Wert ist im Terminal nicht mehr bekannt.
            later.push((o.name.clone(), if o.secret { LiveIssue::Secret } else { LiveIssue::NeedsRestart }));
        }
    }
    (ops, later)
}

// ---------------------------------------------------------------------------------------------------------------
// Einfügen, .env, Kopieren

/// Eine Zeile aus eingefügtem Text bzw. einer `.env`-Datei.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedLine {
    pub name: String,
    pub value: String,
    /// Vorschlag „geheim“ ([`looks_secret`]).
    pub secret: bool,
    /// Problem des Namens (dann nicht übernehmbar).
    pub problem: Option<NameError>,
    /// Modus aus einer Kommentarzeile `# paneldeck: prepend|append` davor ([`to_dotenv`]); sonst ersetzen.
    pub mode: Mode,
}

/// `KEY=VALUE`-Zeilen bzw. eine `.env`-Datei lesen: Leerzeilen und `#`-Kommentare fallen weg, `export ` davor wird
/// ignoriert, Werte in `'…'` bleiben wörtlich, in `"…"` werden `\"`, `\\` und `\n`/`\t` entschlüsselt (Steuerzeichen
/// bleiben im Wert und werden vom Editor gemeldet); ohne Anführungszeichen endet der Wert vor ` #`. Eine Zeile
/// `# paneldeck: prepend` bzw. `append` gibt der nächsten Variable ihren Modus (so schreibt sie [`to_dotenv`]).
pub fn parse_lines(text: &str) -> Vec<ParsedLine> {
    let mut out = Vec::new();
    let mut mode = Mode::Replace;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(c) = line.strip_prefix('#') {
            match c.trim().strip_prefix("paneldeck:").map(str::trim) {
                Some("prepend") => mode = Mode::Prepend,
                Some("append") => mode = Mode::Append,
                _ => {}
            }
            continue;
        }
        if line.is_empty() {
            continue;
        }
        let line = line.strip_prefix("export ").map(str::trim_start).unwrap_or(line);
        let Some((k, v)) = line.split_once('=') else { continue };
        let name = k.trim().to_string();
        let v = v.trim();
        let value = if let Some(inner) = v.strip_prefix('\'') {
            inner.find('\'').map(|j| inner[..j].to_string()).unwrap_or_else(|| inner.to_string())
        } else if let Some(inner) = v.strip_prefix('"') {
            let mut s = String::new();
            let mut it = inner.chars();
            while let Some(c) = it.next() {
                match c {
                    '"' => break,
                    '\\' => match it.next() {
                        Some('n') => s.push('\n'),
                        Some('r') => s.push('\r'),
                        Some('t') => s.push('\t'),
                        Some(o) => s.push(o),
                        None => s.push('\\'),
                    },
                    _ => s.push(c),
                }
            }
            s
        } else {
            match v.find(" #") {
                Some(j) => v[..j].trim_end().to_string(),
                None => v.to_string(),
            }
        };
        out.push(ParsedLine { secret: looks_secret(&name), problem: check_name(&name).err(), name, value, mode });
        mode = Mode::Replace;
    }
    out
}

/// Als `.env` (*Kopieren als .env*): geheime Werte bleiben leer, `remove`-Zeilen fallen weg; Voranstellen/Anhängen
/// steht als Kommentarzeile `# paneldeck: prepend|append` vor der Variable (der Wert ist nur der Zusatz – andere
/// `.env`-Leser übergehen den Kommentar, [`parse_lines`] übernimmt den Modus wieder). Werte mit Leer-, Steuerzeichen,
/// `#`, `'`, `"` oder `\` stehen in `"…"` mit `\\`, `\"`, `\n`, `\r`, `\t`.
pub fn to_dotenv(vars: &[EnvVar]) -> String {
    let mut s = String::new();
    for e in vars.iter().filter(|e| !e.remove) {
        let v = if e.secret { String::new() } else { e.value.clone() };
        let needs = v.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '#' | '\'' | '"' | '\\'));
        let v = if needs { format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t")) } else { v };
        match e.mode {
            Mode::Replace => {}
            Mode::Prepend => s.push_str("# paneldeck: prepend\n"),
            Mode::Append => s.push_str("# paneldeck: append\n"),
        }
        s.push_str(&format!("{}={v}\n", e.name));
    }
    s
}

// ---------------------------------------------------------------------------------------------------------------
// Anzeige und Agenten

/// Tooltip der Kopfzeile: je Zeile `NAME=wert`, geheim als `••••`, entfernte als „NAME (entfernt)“.
pub fn tooltip(vars: &[EnvVar], lang: crate::state::Lang) -> String {
    vars.iter()
        .map(|e| {
            if e.remove {
                format!("{} ({})", e.name, lang.t("entfernt", "removed"))
            } else if e.secret {
                format!("{}=••••", e.name)
            } else {
                let v = if e.value.chars().count() > 60 { format!("{}…", e.value.chars().take(60).collect::<String>()) } else { e.value.clone() };
                let m = match e.mode {
                    Mode::Replace => "",
                    Mode::Prepend => lang.t(" (vorangestellt)", " (prepended)"),
                    Mode::Append => lang.t(" (angehängt)", " (appended)"),
                };
                format!("{}={v}{m}", e.name)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Was Agenten sehen (`list_sessions`, `whoami`): Namen und Kennzeichen, nie Werte.
pub fn exposed(vars: &[EnvVar]) -> serde_json::Value {
    serde_json::Value::Array(
        vars.iter()
            .map(|e| {
                let mut o = serde_json::json!({"name": e.name, "secret": e.secret, "set_by_agent": e.by_agent});
                if e.remove {
                    o["remove"] = serde_json::json!(true);
                }
                if e.mode != Mode::Replace {
                    o["mode"] = serde_json::json!(if e.mode == Mode::Prepend { "prepend" } else { "append" });
                }
                o
            })
            .collect(),
    )
}

/// Namen, die ein Agent nie setzen darf (Pfade, Start- und Ladevariablen, Proxys, Git, Claude, Paneldeck). Einträge mit
/// `*` am Ende sind Präfixe. Groß-/Kleinschreibung egal.
pub const AGENT_BLOCKED: &[&str] = &[
    // Pfade und Suchpfade
    "PATH", "PATHEXT", "LIBPATH", "CDPATH", "MANPATH", "FPATH", "INFOPATH",
    // Shells: Start und Prompt
    "BASH_ENV", "ENV", "PROMPT_COMMAND", "PROMPT", "PS0", "PS1", "PS2", "PS3", "PS4", "ZDOTDIR", "IFS", "SHELL", "SHELLOPTS", "BASHOPTS",
    "COMSPEC", "PSMODULEPATH", "PSEXECUTIONPOLICYPREFERENCE", "WSLENV", "MSYS*", "MSYSTEM*", "CHERE_INVOKING", "HISTFILE", "INPUTRC",
    // Lader und Laufzeiten
    "LD_*", "DYLD_*", "NODE_OPTIONS", "NODE_PATH", "NODE_EXTRA_CA_CERTS", "NODE_TLS_REJECT_UNAUTHORIZED", "NPM_CONFIG_*", "YARN_*",
    "PYTHONPATH", "PYTHONSTARTUP", "PYTHONHOME", "PYTHONINSPECT", "PYTHONUSERBASE", "PIP_*", "VIRTUAL_ENV", "CONDA_*",
    "PERL5LIB", "PERL5OPT", "PERLLIB", "RUBYOPT", "RUBYLIB", "GEM_*", "BUNDLE_*", "JAVA_TOOL_OPTIONS", "_JAVA_OPTIONS",
    "JDK_JAVA_OPTIONS", "JAVA_OPTIONS", "CLASSPATH", "DOTNET_STARTUP_HOOKS", "DOTNET_*", "COR_*", "CORECLR_*", "COMPLUS_*",
    "CARGO_*", "RUSTC*", "RUSTUP_*", "RUSTFLAGS", "RUSTDOCFLAGS", "GOFLAGS", "GOPATH", "GOROOT", "GOPROXY", "GONOSUMDB", "GOPRIVATE",
    "LUA_PATH", "LUA_CPATH", "PHPRC", "PHP_INI_SCAN_DIR",
    // Werkzeuge, die Programme starten
    "EDITOR", "VISUAL", "PAGER", "MANPAGER", "LESSOPEN", "LESSCLOSE", "BROWSER", "SSH_ASKPASS", "SSH_AUTH_SOCK", "SUDO_ASKPASS",
    "GIT_*", "GNUPGHOME", "GPG_*",
    // Netz
    "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY", "FTP_PROXY", "SOCKS_PROXY", "SSL_CERT_FILE", "SSL_CERT_DIR", "REQUESTS_CA_BUNDLE", "CURL_CA_BUNDLE",
    // Benutzer- und Systemordner
    "HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "PROGRAMDATA", "TEMP", "TMP", "TMPDIR", "SYSTEMROOT", "WINDIR", "SYSTEMDRIVE",
    "XDG_*", "TERM", "COLORTERM", "TERM_PROGRAM", "TERM_PROGRAM_VERSION", "WT_SESSION",
    // Claude und Paneldeck
    "ANTHROPIC_*", "CLAUDE_*", "PANELDECK_*",
    // Review #384: ganze Familien von Start-, Lade- und Konfigurationsvariablen
    "BASH_*", "NODE_*", "PYTHON*", "BUN_*", "JAVA_*", "GRADLE_*", "MAVEN_*", "*_OPTS", "SSH_*",
    "HOMEDRIVE", "HOMEPATH", "HOMESHARE", "KUBECONFIG", "AWS_CONFIG_FILE", "AWS_SHARED_CREDENTIALS_FILE", "DOCKER_HOST", "DOCKER_CONFIG", "GCONV_PATH",
];

/// Passt `name` zum Muster (`*` als Platzhalter, auch mehrfach; Groß-/Kleinschreibung egal)? Iterativ mit zwei Zeigern
/// (letzter `*` als Rücksprungpunkt) – linear in der Länge, kein exponentielles Zurückgehen.
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let (p, n) = (pattern.trim().as_bytes(), name.as_bytes());
    let (mut i, mut j) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while j < n.len() {
        if i < p.len() && p[i] == b'*' {
            star = Some((i, j));
            i += 1;
        } else if i < p.len() && p[i].eq_ignore_ascii_case(&n[j]) {
            i += 1;
            j += 1;
        } else if let Some((si, sj)) = star {
            i = si + 1;
            j = sj + 1;
            star = Some((si, sj + 1));
        } else {
            return false;
        }
    }
    p[i..].iter().all(|c| *c == b'*')
}

/// Name, den ein Agent setzen darf: nur ASCII `[A-Za-z_][A-Za-z0-9_]*` (keine Sonderformen wie `BASH_FUNC_x%%`).
pub fn agent_name_ok(name: &str) -> bool {
    simple_name(name)
}

/// Variablen einer Vorlage für eine Sitzung, die ein **Agent** anlegt: ohne die, die ein Agent nie setzen dürfte
/// ([`agent_blocked`]), und ohne geheime (deren Werte könnte der Agent in seiner Sitzung lesen). Rückgabe: (übernommen,
/// weggelassene Namen für `template_ignored`).
pub fn agent_template_env(vars: &[EnvVar]) -> (Vec<EnvVar>, Vec<String>) {
    let (keep, drop): (Vec<EnvVar>, Vec<EnvVar>) = vars.iter().cloned().partition(|v| !v.secret && !agent_blocked(&v.name));
    (keep, drop.into_iter().map(|v| v.name).collect())
}

/// Darf ein Agent diesen Namen nie setzen?
pub fn agent_blocked(name: &str) -> bool {
    AGENT_BLOCKED.iter().any(|p| glob_match(p, name)) || is_protected(name)
}

/// Variablen aus dem MCP-Parameter `env` (Objekt `{NAME: "wert"}`) prüfen: feste Sperrliste ([`AGENT_BLOCKED`]),
/// eigene Erlaubnis-Muster des Nutzers (`allow`; leer = alles außer der Sperrliste), höchstens [`AGENT_MAX_VARS`],
/// Werte höchstens [`AGENT_MAX_VALUE`] Bytes ohne Steuerzeichen, nie geheim (auch keine Namen, die nach Geheimnis
/// aussehen). Fehler als (Deutsch, Englisch) – der Aufrufer macht daraus `bad_args`; Werte stehen nie im Text.
pub fn agent_vars(env: &serde_json::Value, allow: &[String]) -> Result<Vec<EnvVar>, (String, String)> {
    let Some(map) = env.as_object() else {
        return Err(("env: ein Objekt {\"NAME\": \"Wert\"}".into(), "env: an object {\"NAME\": \"value\"}".into()));
    };
    if map.len() > AGENT_MAX_VARS {
        return Err((format!("env: höchstens {AGENT_MAX_VARS} Variablen"), format!("env: at most {AGENT_MAX_VARS} variables")));
    }
    let mut out: Vec<EnvVar> = Vec::new();
    for (k, v) in map {
        let name = k.trim();
        if let Err(e) = check_name(name) {
            let shown: String = name.chars().filter(|c| c.is_ascii_graphic()).take(60).collect();
            return Err((format!("env: {} – {shown}", e.text(crate::state::Lang::De)), format!("env: {} – {shown}", e.text(crate::state::Lang::En))));
        }
        if !agent_name_ok(name) {
            let shown: String = name.chars().filter(|c| c.is_ascii_graphic()).take(60).collect();
            return Err((format!("env: Namen von Agenten nur aus A–Z, a–z, 0–9 und _ (nicht mit einer Ziffer am Anfang) – {shown}"), format!("env: names from agents only from A–Z, a–z, 0–9 and _ (not starting with a digit) – {shown}")));
        }
        if agent_blocked(name) {
            return Err((format!("env: {name} dürfen Agenten nicht setzen (Pfad-, Start-, Lade-, Proxy-, Git-, Claude- oder Paneldeck-Variable)"), format!("env: agents may not set {name} (path, startup, loader, proxy, Git, Claude or Paneldeck variable)")));
        }
        let allowed = allow.iter().map(|p| p.trim()).filter(|p| !p.is_empty()).collect::<Vec<_>>();
        if !allowed.is_empty() && !allowed.iter().any(|p| glob_match(p, name)) {
            return Err((format!("env: {name} passt zu keinem erlaubten Muster ({})", allowed.join(", ")), format!("env: {name} matches no allowed pattern ({})", allowed.join(", "))));
        }
        if looks_secret(name) {
            return Err((format!("env: {name} sieht nach einem Geheimnis aus – geheime Werte setzt nur der Nutzer"), format!("env: {name} looks like a secret – only the user sets secret values")));
        }
        let Some(value) = v.as_str() else {
            return Err((format!("env: der Wert von {name} muss ein Text sein"), format!("env: the value of {name} must be a string")));
        };
        if value.len() > AGENT_MAX_VALUE {
            return Err((format!("env: der Wert von {name} ist länger als {AGENT_MAX_VALUE} Bytes"), format!("env: the value of {name} is longer than {AGENT_MAX_VALUE} bytes")));
        }
        if value.chars().any(|c| c.is_control()) {
            return Err((format!("env: der Wert von {name} enthält Steuerzeichen"), format!("env: the value of {name} contains control characters")));
        }
        if out.iter().any(|e| same_name(&e.name, name)) {
            return Err((format!("env: {name} steht doppelt"), format!("env: {name} appears twice")));
        }
        out.push(EnvVar { by_agent: true, ..EnvVar::plain(name, value) });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::secret::SecretError;
    use crate::state::Lang;

    fn fake_protect(v: &str) -> String {
        format!("ENC:{}", v.chars().rev().collect::<String>())
    }
    fn fake_unprotect(b: &str) -> Result<String, SecretError> {
        b.strip_prefix("ENC:").map(|s| s.chars().rev().collect()).ok_or(SecretError::Sys(13))
    }
    fn secret(name: &str, v: &str) -> EnvVar {
        EnvVar { name: name.into(), value_protected: Some(fake_protect(v)), secret: true, ..EnvVar::default() }
    }

    #[test]
    fn precedence_profile_template_session() {
        let profile = from_profile("A=profile\nB=profile\nPANELDECK_SESSION=99\n PANELDECK_SESSION_KEY = x\nbad line\nC = c ");
        assert_eq!(profile.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["A", "B", "C"], "geschützte Namen fallen weg");
        assert_eq!(profile[2].value, "c");
        let template = vec![EnvVar::plain("B", "template"), EnvVar::plain("D", "template")];
        let session = vec![EnvVar::plain("d", "session"), EnvVar::plain("E", "session")];
        let eff = merge(&merge(&profile, &template), &session);
        let get = |n: &str| eff.iter().filter(|e| e.name == n).map(|e| e.value.clone()).collect::<Vec<_>>();
        assert_eq!(get("A"), ["profile"]);
        assert_eq!(get("B"), ["template"], "Vorlage vor Profil");
        if cfg!(windows) {
            assert!(get("D").is_empty() && get("d") == ["session"], "Sitzung vor Vorlage, ohne Groß-/Kleinschreibung");
        } else {
            assert_eq!(get("D"), ["template"]);
        }
        assert_eq!(get("E"), ["session"]);
    }

    #[test]
    fn modes_and_remove() {
        let vars = vec![
            EnvVar { mode: Mode::Prepend, ..EnvVar::plain("PATH", r"C:\tools") },
            EnvVar { mode: Mode::Append, ..EnvVar::plain("LIST", "z") },
            EnvVar { remove: true, ..EnvVar::plain("GONE", "") },
            EnvVar::plain("X", "$HOME %USERPROFILE% `x`"),
            EnvVar { mode: Mode::Prepend, ..EnvVar::plain("EMPTY", "") },
        ];
        let ops = resolve(&vars, fake_unprotect).unwrap();
        assert_eq!(ops.len(), 4, "leeres Voranstellen fällt weg");
        assert_eq!(apply_one(&ops[0].kind, Some(r"C:\Windows"), ';').as_deref(), Some(r"C:\tools;C:\Windows"));
        assert_eq!(apply_one(&ops[0].kind, None, ';').as_deref(), Some(r"C:\tools"));
        assert_eq!(apply_one(&ops[1].kind, Some("a:b"), ':').as_deref(), Some("a:b:z"));
        assert_eq!(apply_one(&ops[2].kind, Some("x"), ';'), None, "entfernen");
        assert_eq!(apply_one(&ops[3].kind, Some("old"), ';').as_deref(), Some("$HOME %USERPROFILE% `x`"), "wörtlich");
    }

    #[test]
    fn protected_names_are_never_applied() {
        for n in ["PANELDECK_SESSION", "paneldeck_session_key", "PANELDECK_FOO", "PANELDECK_HUB_DATA", "PANELDECK_DEMO_X"] {
            assert_eq!(check_name(n), Err(NameError::Protected), "{n}");
        }
        assert_eq!(check_name(""), Err(NameError::Empty));
        assert_eq!(check_name("A B"), Err(NameError::Invalid));
        assert_eq!(check_name("A=B"), Err(NameError::Invalid));
        assert!(check_name("API_URL").is_ok());
        let vars = vec![EnvVar::plain("PANELDECK_SESSION", "1"), EnvVar::plain("OK", "1")];
        let ops = resolve(&vars, fake_unprotect).unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(warn_name("prompt_command"), Some(NameWarn::ShellIntegration));
        assert_eq!(warn_name("WSLENV"), Some(NameWarn::ShellIntegration));
        assert_eq!(warn_name("ANTHROPIC_API_KEY"), Some(NameWarn::Claude));
        assert_eq!(warn_name("CLAUDE_CODE_USE_BEDROCK"), Some(NameWarn::Claude));
        assert_eq!(warn_name("API_URL"), None);
        for t in [NameWarn::ShellIntegration, NameWarn::Claude] {
            assert_ne!(t.text(Lang::De), t.text(Lang::En));
        }
    }

    #[test]
    fn secret_flag_by_name() {
        for n in ["GITHUB_TOKEN", "api_key", "DB_PASSWORD", "MY_SECRET", "PGPASS", "AUTH_HEADER", "OLDPWD"] {
            assert!(looks_secret(n), "{n}");
        }
        for n in ["API_URL", "NODE_ENV", "PORT"] {
            assert!(!looks_secret(n), "{n}");
        }
    }

    #[test]
    fn secrets_lock_when_they_cannot_be_decrypted() {
        let vars = vec![secret("TOKEN", "s3cr3t"), EnvVar::plain("A", "1")];
        let ops = resolve(&vars, fake_unprotect).unwrap();
        assert_eq!(ops[0].kind, OpKind::Set("s3cr3t".into()));
        assert!(ops[0].secret);
        let broken = vec![EnvVar { value_protected: Some("garbage".into()), ..secret("TOKEN", "") }, secret("OTHER", "x"), EnvVar { value_protected: Some("??".into()), ..secret("KEY2", "") }];
        let l = resolve(&broken, fake_unprotect).unwrap_err();
        assert_eq!(l.names, ["TOKEN", "KEY2"]);
        assert!(l.needs_reentry(), "DPAPI-Fehler: neu eingeben");
        assert!(!Locked { names: vec![], error: SecretError::NeedPassword }.needs_reentry(), "Master-Passwort: entsperren");
        // Geheim ohne Wert: kein Sperren, die Variable fehlt einfach.
        let none = vec![EnvVar { name: "T".into(), secret: true, ..EnvVar::default() }];
        assert!(resolve(&none, fake_unprotect).unwrap().is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn encryption_round_trip_never_stores_plain_text() {
        let v = EnvVar::secret("TOKEN", "geheim-123").unwrap();
        let json = serde_json::to_string(&v).unwrap();
        assert!(!json.contains("geheim-123") && json.contains("value_protected") && json.contains("\"secret\":true"), "{json}");
        let back: Vec<EnvVar> = serde_json::from_str::<Wrap>(&format!("{{\"env\":[{json}]}}")).unwrap().env;
        let ops = resolve(&back, crate::llm::secret::unprotect).unwrap();
        assert_eq!(ops[0].kind, OpKind::Set("geheim-123".into()));
        // Von Hand eingetragener Klartext einer geheimen Variable wird beim Laden verschlüsselt.
        let hand: Wrap = serde_json::from_str(r#"{"env":[{"name":"PASS","value":"plain!","secret":true}]}"#).unwrap();
        assert!(hand.env[0].value.is_empty() && hand.env[0].value_protected.is_some());
        assert!(!serde_json::to_string(&hand.env).unwrap().contains("plain!"));
        // Fremder Blob (anderer Benutzer/Rechner): gesperrt, Werte neu eingeben.
        let foreign = vec![EnvVar { value_protected: Some("AAAA".into()), ..secret("TOKEN", "") }];
        assert!(resolve(&foreign, crate::llm::secret::unprotect).unwrap_err().needs_reentry());
    }

    #[derive(Deserialize)]
    struct Wrap {
        #[serde(default, deserialize_with = "lenient_list")]
        env: Vec<EnvVar>,
    }

    #[test]
    fn old_and_broken_files_load() {
        let w: Wrap = serde_json::from_str("{}").unwrap();
        assert!(w.env.is_empty());
        let w: Wrap = serde_json::from_str(r#"{"env":"kaputt"}"#).unwrap();
        assert!(w.env.is_empty());
        let w: Wrap = serde_json::from_str(r#"{"env":[{"name":"A","value":"1","mode":"sideways"},{"name":"PANELDECK_X","value":"1"},5,{"name":"a","value":"2","future":true},{"name":"R","value":"x","remove":true}]}"#).unwrap();
        let names: Vec<&str> = w.env.iter().map(|e| e.name.as_str()).collect();
        if cfg!(windows) {
            assert_eq!(names, ["a", "R"], "doppelte Namen: der letzte gilt");
        } else {
            assert_eq!(names, ["A", "a", "R"]);
        }
        assert_eq!(w.env[0].mode, Mode::Replace);
        assert!(w.env.last().unwrap().value.is_empty(), "entfernen hat keinen Wert");
        // Schreiben: nur gesetzte Felder.
        assert_eq!(serde_json::to_string(&EnvVar::plain("A", "1")).unwrap(), r#"{"name":"A","value":"1"}"#);
        let full = EnvVar { mode: Mode::Prepend, wsl_path: true, by_agent: true, ..EnvVar::plain("P", "/x") };
        assert_eq!(serde_json::to_string(&full).unwrap(), r#"{"name":"P","value":"/x","mode":"prepend","wsl_path":true,"by_agent":true}"#);
    }

    #[test]
    fn old_state_and_templates_load_and_new_fields_round_trip() {
        use crate::state::SessionMeta;
        use crate::templates::TplItem;
        let m: SessionMeta = serde_json::from_str(r#"{"id":3,"name":"x","shell_id":"pwsh"}"#).unwrap();
        assert!(m.env.is_empty());
        assert!(!serde_json::to_string(&m).unwrap().contains("\"env\""), "leer = nicht geschrieben");
        let m2 = SessionMeta { env: vec![EnvVar::plain("A", "1"), EnvVar { remove: true, ..EnvVar::plain("B", "") }], ..m };
        let back: SessionMeta = serde_json::from_str(&serde_json::to_string(&m2).unwrap()).unwrap();
        assert_eq!(back.env, m2.env);
        let broken: SessionMeta = serde_json::from_str(r#"{"id":4,"env":{"A":"1"}}"#).unwrap();
        assert!(broken.env.is_empty() && broken.id == 4, "falscher Typ verwirft die Sitzung nicht");
        let t: TplItem = serde_json::from_str(r#"{"name":"x"}"#).unwrap();
        assert!(t.env.is_empty());
        let t2 = TplItem { env: vec![EnvVar { mode: Mode::Append, ..EnvVar::plain("PATH", "/x") }], ..t };
        let back: TplItem = serde_json::from_str(&serde_json::to_string(&t2).unwrap()).unwrap();
        assert_eq!(back.env, t2.env);
    }

    #[test]
    fn wslenv_flags() {
        let op = |v: &str, p: bool| Op { name: "X".into(), kind: OpKind::Set(v.into()), secret: false, wsl_path: p };
        assert_eq!(wslenv_flag(&op(r"C:\x", false)), "");
        assert_eq!(wslenv_flag(&op(r"C:\x", true)), "/p");
        assert_eq!(wslenv_flag(&op(r"C:\x;D:\y", true)), "/l");
    }

    #[test]
    fn live_commands_quote_per_shell() {
        let ops = vec![
            Op { name: "A".into(), kind: OpKind::Set("it's $HOME `x`".into()), secret: false, wsl_path: false },
            Op { name: "PATH".into(), kind: OpKind::Prepend("/opt/bin".into()), secret: false, wsl_path: false },
            Op { name: "GONE".into(), kind: OpKind::Remove, secret: false, wsl_path: false },
            Op { name: "TOKEN".into(), kind: OpKind::Set("never".into()), secret: true, wsl_path: false },
        ];
        let p = live_command(LiveShell::Pwsh, false, &ops).unwrap();
        let sep = if cfg!(windows) { ';' } else { ':' };
        assert_eq!(p, format!(" $env:A = 'it''s $HOME `x`'; $env:PATH = if ($env:PATH) {{ '/opt/bin{sep}' + $env:PATH }} else {{ '/opt/bin' }}; Remove-Item Env:GONE -ErrorAction SilentlyContinue"));
        assert!(!p.contains("never"), "geheime Werte nie getippt");
        let b = live_command(LiveShell::Posix, true, &ops).unwrap();
        assert_eq!(b, r#" export A='it'\''s $HOME `x`'; export PATH='/opt/bin'"${PATH:+:$PATH}"; unset GONE"#);
        let f = live_command(LiveShell::Fish, true, &ops).unwrap();
        assert_eq!(f, r" set -gx A 'it\'s $HOME `x`'; set -gx PATH '/opt/bin' $PATH; set -e GONE");
        // cmd: Voranstellen nur, wenn es die Variable gibt (sonst stünde `%PATH%` wörtlich im Wert).
        let c = live_command(LiveShell::Cmd, false, &ops[1..]).unwrap();
        assert_eq!(c, format!(" if defined PATH (set \"PATH=/opt/bin{sep}%PATH%\") else (set \"PATH=/opt/bin\") & set \"GONE=\""));
        // cmd: `%`, `!` und `"` ließen sich nicht wörtlich setzen.
        assert_eq!(live_issue(LiveShell::Cmd, false, &ops[0]), None);
        let pct = Op { name: "A".into(), kind: OpKind::Set("100%".into()), secret: false, wsl_path: false };
        assert_eq!(live_issue(LiveShell::Cmd, false, &pct), Some(LiveIssue::Value));
        assert_eq!(live_issue(LiveShell::Pwsh, false, &pct), None);
        assert_eq!(live_issue(LiveShell::Posix, true, &Op { name: "A-B".into(), ..pct.clone() }), Some(LiveIssue::Name));
        assert_eq!(live_issue(LiveShell::Posix, true, &Op { kind: OpKind::Set("a\nb".into()), ..pct.clone() }), Some(LiveIssue::Value));
        assert_eq!(live_issue(LiveShell::Posix, true, &ops[3]), Some(LiveIssue::Secret));
        assert_eq!(live_command(LiveShell::Posix, false, &ops[3..]), None);
        // Git Bash (Windows, nicht WSL): Voranstellen/Anhängen nur per Neustart (Windows-Pfade in einer `:`-Liste).
        let gb = live_issue(LiveShell::Posix, false, &ops[1]);
        assert_eq!(gb, if cfg!(windows) { Some(LiveIssue::NeedsRestart) } else { None });
        assert_eq!(live_issue(LiveShell::Posix, false, &ops[0]), None, "Ersetzen geht auch in Git Bash");
        // Append in allen Shells – auch, wenn es die Variable noch nicht gibt (kein Trennzeichen davor)
        let app = [Op { name: "L".into(), kind: OpKind::Append("z".into()), secret: false, wsl_path: false }];
        assert_eq!(live_command(LiveShell::Posix, true, &app).unwrap(), r#" export L="${L:+$L:}"'z'"#);
        assert_eq!(live_command(LiveShell::Fish, true, &app).unwrap(), r#" if set -q L; and test -n "$L"; set -gx L "$L"':z'; else; set -gx L 'z'; end"#);
        assert_eq!(live_command(LiveShell::Pwsh, false, &app).unwrap(), format!(" $env:L = if ($env:L) {{ $env:L + '{sep}z' }} else {{ 'z' }}"));
        assert_eq!(live_command(LiveShell::Cmd, false, &app).unwrap(), format!(" if defined L (set \"L=%L%{sep}z\") else (set \"L=z\")"));
        let pre = [Op { name: "L".into(), kind: OpKind::Prepend("a".into()), secret: false, wsl_path: false }];
        assert_eq!(live_command(LiveShell::Fish, true, &pre).unwrap(), r#" if set -q L; and test -n "$L"; set -gx L 'a:'"$L"; else; set -gx L 'a'; end"#);
        // Eingabezeile vorher leeren: Strg+U bzw. Esc, so kodiert wie die Tastatur.
        let mode = crate::vtkeys::KeyMode::default();
        assert_eq!(crate::vtkeys::key_bytes(clear_line_key(LiveShell::Posix), mode).unwrap(), vec![0x15]);
        assert_eq!(crate::vtkeys::key_bytes(clear_line_key(LiveShell::Fish), mode).unwrap(), vec![0x15]);
        assert_eq!(crate::vtkeys::key_bytes(clear_line_key(LiveShell::Pwsh), mode).unwrap(), vec![0x1b]);
        assert_eq!(crate::vtkeys::key_bytes(clear_line_key(LiveShell::Cmd), mode).unwrap(), vec![0x1b]);
        for i in [LiveIssue::Secret, LiveIssue::Name, LiveIssue::Value, LiveIssue::NeedsRestart] {
            assert_ne!(i.text(Lang::De), i.text(Lang::En));
        }
    }

    #[test]
    fn names_that_break_wslenv_are_refused_and_glob_is_linear() {
        for n in ["X:PATH", "A/l", "A=B", "A\u{200B}B", "A\u{202E}B", "\u{FEFF}A", "A\tB"] {
            assert_eq!(check_name(n), Err(NameError::Invalid), "{n:?}");
        }
        assert!(check_name("my-var.1").is_ok(), "Bindestrich und Punkt gehen für den Nutzer");
        // Agenten: nur ASCII-Bezeichner, große Sperrlisten-Familien
        assert!(agent_name_ok("APP_MODE") && agent_name_ok("_x1") && !agent_name_ok("1A") && !agent_name_ok("A-B") && !agent_name_ok("BASH_FUNC_x%%") && !agent_name_ok("ÄPP"));
        for n in ["BASH_FUNC_ls", "NODE_ENV", "PYTHONWARNINGS", "BUN_INSTALL", "JAVA_HOME", "GRADLE_USER_HOME", "MAVEN_OPTS", "SBT_OPTS", "HOMEDRIVE", "HOMEPATH", "HOMESHARE", "KUBECONFIG", "AWS_CONFIG_FILE", "AWS_SHARED_CREDENTIALS_FILE", "DOCKER_HOST", "DOCKER_CONFIG", "GCONV_PATH", "GIT_DIR", "GIT_SSH", "SSH_AUTH_SOCK", "ssh_foo"] {
            assert!(agent_blocked(n), "{n}");
            assert!(agent_vars(&serde_json::json!({ n: "x" }), &[]).is_err(), "{n}");
        }
        let e = agent_vars(&serde_json::json!({"BASH_FUNC_x%%": "() { id; }"}), &[]).unwrap_err().1;
        assert!(!e.contains("id;"), "{e}");
        assert!(!agent_blocked("APP_MODE") && !agent_blocked("API_URL"));
        // Glob: zwei Zeiger, viele `*` und ein langer Name bleiben schnell
        let long = "A".repeat(255);
        let many = format!("{}B", "*A".repeat(60));
        let t = std::time::Instant::now();
        assert!(!glob_match(&many, &long));
        assert!(glob_match(&"*".repeat(100), &long) && glob_match("A*A", &long) && !glob_match("A*B", &long));
        assert!(t.elapsed() < std::time::Duration::from_millis(200), "{:?}", t.elapsed());
        assert!(glob_match("*_OPTS", "MAVEN_OPTS") && glob_match("a*b*c", "AxbyC") && !glob_match("a*b*c", "AxbyCd") && glob_match("", "") && !glob_match("", "A"));
    }

    #[test]
    fn agent_started_templates_never_get_secret_or_blocked_variables() {
        let vars = vec![EnvVar::plain("APP_MODE", "dev"), secret("DB_PASSWORD", "s3cr3t"), EnvVar::plain("NODE_OPTIONS", "--require x"), EnvVar { name: "PLAIN_BUT_SECRET".into(), secret: true, ..EnvVar::default() }];
        let (keep, dropped) = agent_template_env(&vars);
        assert_eq!(keep.iter().map(|v| v.name.as_str()).collect::<Vec<_>>(), ["APP_MODE"]);
        assert_eq!(dropped, ["DB_PASSWORD", "NODE_OPTIONS", "PLAIN_BUT_SECRET"]);
    }

    #[test]
    fn live_state_follows_set_now() {
        let spawned = vec![EnvVar::plain("A", "1"), EnvVar::plain("B", "1")];
        let new = vec![EnvVar::plain("B", "1"), EnvVar::plain("A", "2"), EnvVar::plain("C", "3"), secret("TOKEN", "x")];
        let (ops, later) = live_changes(&spawned, &new);
        let mut live = spawned.clone();
        apply_live(&mut live, &new, &ops);
        assert!(!same_set(&live, &new), "geheimer Wert fehlt noch in der Shell");
        assert_eq!(later, [("TOKEN".to_string(), LiveIssue::Secret)]);
        // Nur noch das Geheimnis ist offen: ohne es stimmen Shell und Einstellung überein.
        assert!(same_set(&live, &new[..3]));
        assert!(same_set(&[EnvVar::plain("A", "1"), EnvVar::plain("B", "2")], &[EnvVar::plain("B", "2"), EnvVar::plain("A", "1")]));
    }

    #[test]
    fn live_changes_split_what_can_be_set_now() {
        let old = vec![EnvVar::plain("A", "1"), EnvVar { mode: Mode::Prepend, ..EnvVar::plain("PATH", "/x") }, EnvVar::plain("DEL", "1"), secret("TOKEN", "a")];
        let new = vec![
            EnvVar::plain("A", "2"),
            EnvVar { mode: Mode::Prepend, ..EnvVar::plain("PATH", "/y") },
            secret("TOKEN", "b"),
            EnvVar::plain("NEW", "n"),
            EnvVar { mode: Mode::Append, ..EnvVar::plain("LIST", "z") },
            EnvVar { remove: true, ..EnvVar::plain("RM", "") },
        ];
        let (ops, later) = live_changes(&old, &new);
        let names: Vec<&str> = ops.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["A", "NEW", "LIST", "RM"]);
        assert_eq!(ops[3].kind, OpKind::Remove);
        assert_eq!(later, vec![("PATH".to_string(), LiveIssue::NeedsRestart), ("TOKEN".to_string(), LiveIssue::Secret), ("DEL".to_string(), LiveIssue::NeedsRestart)]);
        let (ops, later) = live_changes(&new, &new);
        assert!(ops.is_empty() && later.is_empty());
    }

    #[test]
    fn paste_and_dotenv() {
        let text = "# comment\n\nexport API_URL=https://x.test/a?b=1 # note\nGITHUB_TOKEN='ghp_x y'\nMSG=\"a \\\"q\\\" \\\\ b\"\nPANELDECK_SESSION=1\nnot a line\n EMPTY=\nML=\"a\\nb\"\n";
        let p = parse_lines(text);
        let names: Vec<&str> = p.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["API_URL", "GITHUB_TOKEN", "MSG", "PANELDECK_SESSION", "EMPTY", "ML"]);
        assert_eq!(p[0].value, "https://x.test/a?b=1");
        assert!(!p[0].secret && p[1].secret, "verdächtige Namen werden geheim");
        assert_eq!(p[1].value, "ghp_x y");
        assert_eq!(p[2].value, r#"a "q" \ b"#);
        assert_eq!(p[3].problem, Some(NameError::Protected));
        assert_eq!(p[4].value, "");
        assert_eq!(p[5].value, "a\nb");
        let vars = vec![EnvVar::plain("A", "x y"), secret("TOKEN", "s"), EnvVar { remove: true, ..EnvVar::plain("R", "") }, EnvVar::plain("Q", r#"a"b\c"#)];
        let env = to_dotenv(&vars);
        assert_eq!(env, "A=\"x y\"\nTOKEN=\nQ=\"a\\\"b\\\\c\"\n", "geheime Werte leer, entfernte weg");
        let back = parse_lines(&env);
        assert_eq!(back[0].value, "x y");
        assert_eq!(back[2].value, r#"a"b\c"#);
        // Modus als Kommentarzeile, `\r` maskiert – und beides kommt beim Einlesen zurück.
        let modes = vec![EnvVar { mode: Mode::Prepend, ..EnvVar::plain("PATH", r"C:\tools") }, EnvVar { mode: Mode::Append, ..EnvVar::plain("L", "z") }, EnvVar::plain("CR", "a\rb")];
        let env = to_dotenv(&modes);
        assert_eq!(env, "# paneldeck: prepend\nPATH=\"C:\\\\tools\"\n# paneldeck: append\nL=z\nCR=\"a\\rb\"\n");
        assert!(!env.contains('\r'));
        let back = parse_lines(&env);
        assert_eq!(back.iter().map(|l| (l.name.as_str(), l.value.as_str(), l.mode)).collect::<Vec<_>>(), [("PATH", r"C:\tools", Mode::Prepend), ("L", "z", Mode::Append), ("CR", "a\rb", Mode::Replace)]);
    }

    #[test]
    fn agents_see_names_only_and_set_only_allowed_values() {
        let vars = vec![EnvVar::plain("API_URL", "https://secret-host"), secret("TOKEN", "s3cr3t"), EnvVar { by_agent: true, ..EnvVar::plain("APP_MODE", "dev") }];
        let x = exposed(&vars).to_string();
        assert!(!x.contains("secret-host") && !x.contains("s3cr3t") && !x.contains("dev") && !x.contains("ENC:"), "{x}");
        assert!(x.contains("\"name\":\"TOKEN\"") && x.contains("\"secret\":true") && x.contains("\"set_by_agent\":true"));
        let ok = agent_vars(&serde_json::json!({"APP_MODE": "dev", "API_URL": "http://localhost:3000"}), &[]).unwrap();
        assert!(ok.iter().all(|e| e.by_agent && !e.secret));
        let bad = |v: serde_json::Value, allow: &[&str]| agent_vars(&v, &allow.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap_err().1;
        for n in ["PATH", "path", "BASH_ENV", "PROMPT_COMMAND", "NODE_OPTIONS", "LD_PRELOAD", "DYLD_INSERT_LIBRARIES", "ZDOTDIR", "GIT_SSH_COMMAND", "HTTPS_PROXY", "ANTHROPIC_BASE_URL", "CLAUDE_CONFIG_DIR", "PANELDECK_SESSION", "PYTHONPATH", "COMSPEC"] {
            let e = bad(serde_json::json!({ n: "x" }), &[]);
            assert!(e.contains(n) && !e.contains("\"x\""), "{n}: {e}");
        }
        assert!(bad(serde_json::json!({"API_TOKEN": "x"}), &[]).contains("secret"));
        assert!(bad(serde_json::json!({"A": "a\u{1b}b"}), &[]).contains("control"));
        assert!(bad(serde_json::json!({"A": "x".repeat(1025)}), &[]).contains("1024"));
        assert!(agent_vars(&serde_json::json!({"A": "x".repeat(1024)}), &[]).is_ok());
        assert!(bad(serde_json::json!({"A": 5}), &[]).contains("string"));
        assert!(bad(serde_json::json!(["A=1"]), &[]).contains("object"));
        let many: serde_json::Map<String, serde_json::Value> = (0..21).map(|i| (format!("V{i}"), serde_json::json!("1"))).collect();
        assert!(bad(serde_json::Value::Object(many), &[]).contains("20"));
        // Eigene Muster: nur passende Namen; die Sperrliste gilt trotzdem.
        assert!(agent_vars(&serde_json::json!({"APP_X": "1"}), &["APP_*".into()]).is_ok());
        assert!(bad(serde_json::json!({"OTHER": "1"}), &["APP_*"]).contains("APP_*"));
        assert!(bad(serde_json::json!({"PATH": "1"}), &["*"]).contains("PATH"));
        assert!(glob_match("app_*", "APP_MODE") && glob_match("*_URL", "API_URL") && !glob_match("APP_*", "XAPP_A") && glob_match("A*B*C", "AxxBxC"));
    }

    #[test]
    fn tooltip_masks_secrets() {
        let vars = vec![EnvVar::plain("A", "1"), secret("TOKEN", "s3cr3t"), EnvVar { remove: true, ..EnvVar::plain("R", "") }, EnvVar { mode: Mode::Prepend, ..EnvVar::plain("PATH", "/x") }];
        let t = tooltip(&vars, Lang::En);
        assert_eq!(t, "A=1\nTOKEN=••••\nR (removed)\nPATH=/x (prepended)");
        assert!(!tooltip(&vars, Lang::De).contains("s3cr3t"));
    }
}
