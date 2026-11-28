//! Variablen in Befehlen und Texten (#383): `${file}`, `${fileDirname}`, `${fileBasename}`,
//! `${fileBasenameNoExtension}`, `${fileExtname}`, `${relativeFile}`, `${ruleFolder}`, `${workspaceFolder}`,
//! `${env:NAME}`.
//!
//! In Befehlen wird jeder Wert für die gewählte Shell gequotet (ein wörtliches Argument – auch bei Namen mit `% & ' $`,
//! Backticks, `;` oder Leerzeichen). Beim Zerlegen der Vorlage wird der Quote-Zustand der Shell mitgeführt: Eine
//! Variable **innerhalb** von Anführungszeichen (`"--out=${file}"`) würde das Quoting aushebeln (Einschleusen über den
//! Dateinamen) und wird abgelehnt – außer die Anführungszeichen umschließen genau die Variable (`"${file}"`), dann fallen
//! sie weg und der Wert wird richtig gequotet. Unbekannte Namen bleiben stehen (bash und PowerShell kennen selbst
//! `${…}`).
//!
//! Dieselben Werte gibt es als Umgebungsvariablen (`PANELDECK_FILE` …, [`Ctx::env`]). Sicher sind sie, wenn ein Skript
//! sie über die Umgebung liest (`$env:PANELDECK_FILE` in PowerShell, `"$PANELDECK_FILE"` in bash). **cmd** setzt
//! `%PANELDECK_FILE%` dagegen ungequotet ein – dort immer `"%PANELDECK_FILE%"` schreiben (der Dialog warnt).
//!
//! cmd: Die Befehlszeile geht über eine Umgebungsvariable an `cmd /c` (siehe `procs::launch_spec`), deren Inhalt cmd
//! nicht noch einmal ersetzt. Deshalb ersetzt der Aufrufer `%NAME%` der Vorlage **vor** dem Einsetzen der Werte
//! ([`expand_command`] mit `cmd_lookup`) – ein `%` in einem Dateinamen bleibt so wörtlich. `PANELDECK_*` und die
//! geheimen Variablen der Regel dürfen dort nicht hinein (geheime Werte nie in eine Befehlszeile); kommen sie als
//! `%NAME%` vor ([`cmd_needs_batch`]), läuft der Befehl über eine Batch-Datei ([`cmd_batch_line`], `super::batch`),
//! in der cmd sie beim Ausführen aus der Umgebung ersetzt.

use super::relative;
use crate::procs::Flavor;
use crate::quote::{self, Kind};
use crate::state::t;
use crate::targetpath::PathKind;
use std::path::{Path, PathBuf};

/// Die Namen der Variablen (ohne `env:`).
pub const NAMES: [&str; 8] = ["file", "fileDirname", "fileBasename", "fileBasenameNoExtension", "fileExtname", "relativeFile", "ruleFolder", "workspaceFolder"];

/// Umgebungsvariable zu einem Namen.
pub fn env_name(name: &str) -> &'static str {
    match name {
        "file" => "PANELDECK_FILE",
        "fileDirname" => "PANELDECK_FILE_DIRNAME",
        "fileBasename" => "PANELDECK_FILE_BASENAME",
        "fileBasenameNoExtension" => "PANELDECK_FILE_BASENAME_NO_EXTENSION",
        "fileExtname" => "PANELDECK_FILE_EXTNAME",
        "relativeFile" => "PANELDECK_RELATIVE_FILE",
        "ruleFolder" => "PANELDECK_RULE_FOLDER",
        _ => "PANELDECK_WORKSPACE_FOLDER",
    }
}

/// Ist der Wert ein Pfad (für WSL/Git Bash umzurechnen)?
fn is_path(name: &str) -> bool {
    matches!(name, "file" | "fileDirname" | "ruleFolder" | "workspaceFolder")
}

/// Ordner ohne abschließenden Trenner (außer der Wurzel wie `C:\` oder `/`).
fn trim_sep(p: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    let t = s.trim_end_matches(['\\', '/']);
    if t.is_empty() || (t.len() == 2 && t.ends_with(':')) || t.ends_with(":\\") { s } else { t.to_string() }
}

/// Wofür eingesetzt wird.
#[derive(Clone, Debug)]
pub struct Ctx {
    pub file: PathBuf,
    pub rule_folder: PathBuf,
    /// Geöffneter Ordner des Editors, wenn die Datei darin liegt (sonst gilt der Regelordner).
    pub workspace: Option<PathBuf>,
}

impl Ctx {
    /// Wert einer Variable (Pfade so, wie Windows bzw. das System sie schreibt; Ordner ohne Trenner am Ende).
    pub fn value(&self, name: &str) -> Option<String> {
        let f = &self.file;
        Some(match name {
            "file" => f.to_string_lossy().into_owned(),
            "fileDirname" => f.parent().map(trim_sep).unwrap_or_default(),
            "fileBasename" => f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            "fileBasenameNoExtension" => f.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            "fileExtname" => f.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default(),
            "relativeFile" => relative(f, &self.rule_folder).map(|r| r.to_string_lossy().into_owned()).unwrap_or_else(|| f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
            "ruleFolder" => trim_sep(&self.rule_folder),
            "workspaceFolder" => trim_sep(self.workspace()),
            _ => return None,
        })
    }

    fn workspace(&self) -> &Path {
        match &self.workspace {
            Some(w) if super::is_inside(&self.file, w) => w,
            _ => &self.rule_folder,
        }
    }

    /// Die Umgebungsvariablen `PANELDECK_FILE` … (native Pfade).
    pub fn env(&self) -> Vec<(String, String)> {
        NAMES.iter().map(|n| (env_name(n).to_string(), self.value(n).unwrap_or_default())).collect()
    }

    /// WSL: `WSLENV`, damit die Variablen in die Distribution mitgehen (Pfade mit `/p` übersetzt) – auch die eigenen
    /// Variablen der Regel (`own`, ohne Übersetzung).
    pub fn wslenv(&self, existing: Option<&str>, own: &[String]) -> String {
        let mut parts: Vec<String> = existing.map(|e| e.split(':').filter(|s| !s.is_empty()).map(str::to_string).collect()).unwrap_or_default();
        let has = |parts: &[String], e: &str| parts.iter().any(|p| p.split('/').next().is_some_and(|k| k.eq_ignore_ascii_case(e)));
        for n in NAMES {
            let e = env_name(n);
            if !has(&parts, e) {
                parts.push(if is_path(n) { format!("{e}/p") } else { e.to_string() });
            }
        }
        for e in own {
            if !e.is_empty() && !e.contains([':', '/']) && !has(&parts, e) {
                parts.push(e.clone());
            }
        }
        parts.join(":")
    }
}

/// Für welche Shell eingesetzt wird.
#[derive(Clone, Debug)]
pub struct Target {
    pub flavor: Flavor,
    /// Quoting (Git Bash/bash/zsh/fish nach der Shell; WSL läuft über `bash -lc`).
    pub quote: Kind,
    /// WSL: die Distribution der Shell (`None` = Standard bzw. keine WSL-Shell).
    pub distro: Option<String>,
    /// Pfadart der Shell (#392, `targetpath::PathKind::of`): Git Bash/MSYS/Cygwin samt Installationsordner usw.
    pub kind: PathKind,
}

impl Target {
    pub fn of(shell: &crate::shells::ShellSpec) -> Target {
        let flavor = crate::procs::flavor_of(shell);
        let quote = match flavor {
            Flavor::Pwsh => Kind::Pwsh,
            Flavor::Cmd => Kind::Cmd,
            Flavor::Wsl => Kind::Posix,
            Flavor::Bash => crate::shells::quote_kind(shell),
        };
        let distro = (flavor == Flavor::Wsl).then(|| crate::shells::wsl_distro(&shell.args).or_else(|| shell.id.strip_prefix("wsl:").map(str::to_string))).flatten();
        Target { flavor, quote, distro, kind: PathKind::of(shell) }
    }

    /// Ohne bekannte Shell: PowerShell (wie `procs::resolve_shell` ohne Angabe).
    pub fn pwsh() -> Target {
        Target { flavor: Flavor::Pwsh, quote: Kind::Pwsh, distro: None, kind: PathKind::Windows { cmd: false } }
    }

    #[cfg(test)]
    pub fn test(flavor: Flavor) -> Target {
        let (quote, id) = match flavor {
            Flavor::Pwsh => (Kind::Pwsh, "powershell"),
            Flavor::Cmd => (Kind::Cmd, "cmd"),
            Flavor::Bash => (Kind::Bash, "gitbash"),
            Flavor::Wsl => (Kind::Posix, "wsl:Ubuntu"),
        };
        Target { flavor, quote, distro: (flavor == Flavor::Wsl).then(|| "Ubuntu".to_string()), kind: PathKind::of_id(id) }
    }

    fn posix(&self) -> bool {
        matches!(self.flavor, Flavor::Bash | Flavor::Wsl)
    }

    /// Pfadart für die Umrechnung: WSL immer mit der Distribution der Regel-Shell (`distro`, `None` = Standard), sonst
    /// die erkannte Pfadart.
    pub fn path_kind(&self) -> PathKind {
        match self.flavor {
            Flavor::Wsl => PathKind::Wsl { distro: self.distro.clone() },
            _ => self.kind.clone(),
        }
    }
}

/// Einen Wert als ein wörtliches Argument für die Shell. Ein `\` am Ende wird für cmd/PowerShell verdoppelt (sonst
/// läse das Programm `\"` als Anführungszeichen im Argument).
pub fn quote_for(t: &Target, value: &str) -> String {
    let v = if matches!(t.quote, Kind::Cmd | Kind::Pwsh) && value.ends_with('\\') { format!("{value}\\") } else { value.to_string() };
    match t.quote {
        // cmd: in "…" sind & | < > ^ wörtlich; ein " kann in Windows-Namen nicht vorkommen (in anderen Werten fällt es weg).
        Kind::Cmd => format!("\"{}\"", v.replace('"', "")),
        k => quote::quote(k, &v),
    }
}

/// Teil einer Vorlage.
#[derive(Debug, PartialEq)]
enum Piece<'a> {
    Text(&'a str),
    Var(&'a str),
    Env(&'a str),
}

/// Name einer bekannten Variable bei `s` (beginnt mit `${`): (Länge bis einschließlich `}`, Teil).
fn var_at(s: &str) -> Option<(usize, Piece<'_>)> {
    let after = s.strip_prefix("${")?;
    let j = after.find('}')?;
    let name = &after[..j];
    if NAMES.contains(&name) {
        return Some((j + 3, Piece::Var(name)));
    }
    let env = name.strip_prefix("env:").filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '(' || c == ')'))?;
    Some((j + 3, Piece::Env(env)))
}

/// Vorlage ohne Shell zerlegen (Benachrichtigungen). Unbekannte `${…}` bleiben Text.
fn pieces(template: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < template.len() {
        if let Some((len, p)) = var_at(&template[i..]) {
            out.push(Piece::Text(&template[start..i]));
            out.push(p);
            i += len;
            start = i;
        } else {
            i += template[i..].chars().next().map_or(1, char::len_utf8);
        }
    }
    out.push(Piece::Text(&template[start..]));
    out
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Q {
    None,
    Single,
    Double,
}

/// Öffnet/schließt `c` in dieser Shell einfache bzw. doppelte Anführungszeichen?
fn quote_class(kind: Kind, c: char) -> Option<Q> {
    match (kind, c) {
        (Kind::Cmd, '"') => Some(Q::Double),
        (Kind::Cmd, _) => None,
        (_, '\'') => Some(Q::Single),
        (_, '"') => Some(Q::Double),
        // PowerShell wertet auch typografische Anführungszeichen als Quotes.
        (Kind::Pwsh, '\u{2018}'..='\u{201B}') => Some(Q::Single),
        (Kind::Pwsh, '\u{201C}'..='\u{201E}') => Some(Q::Double),
        _ => None,
    }
}

/// Maskiert `c` im Zustand `q` das nächste Zeichen?
fn escapes(kind: Kind, q: Q, c: char) -> bool {
    match kind {
        Kind::Pwsh => c == '`' && q != Q::Single,
        Kind::Cmd => c == '^' && q == Q::None,
        Kind::Fish => c == '\\',
        Kind::Posix | Kind::Bash => c == '\\' && q != Q::Single,
    }
}

fn var_label(p: &Piece) -> String {
    match p {
        Piece::Var(n) => format!("${{{n}}}"),
        Piece::Env(n) => format!("${{env:{n}}}"),
        Piece::Text(_) => String::new(),
    }
}

/// Vorlage für eine Shell zerlegen, mit Quote-Zustand. Fehler: Variable innerhalb von Anführungszeichen, die nicht
/// genau diese Variable umschließen (sonst könnte ein Dateiname das Quoting verlassen).
fn shell_pieces(template: &str, kind: Kind) -> Result<Vec<Piece<'_>>, String> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut q = Q::None;
    // Position und Länge des öffnenden Anführungszeichens.
    let mut open = (0usize, 0usize);
    let mut i = 0;
    while i < template.len() {
        if let Some((len, p)) = var_at(&template[i..]) {
            if q == Q::None {
                out.push(Piece::Text(&template[start..i]));
                out.push(p);
                i += len;
                start = i;
                continue;
            }
            let rest = &template[i + len..];
            let closing = rest.chars().next().filter(|c| quote_class(kind, *c) == Some(q));
            match closing {
                Some(c) if open.0 + open.1 == i && open.0 >= start => {
                    out.push(Piece::Text(&template[start..open.0]));
                    out.push(p);
                    i += len + c.len_utf8();
                    start = i;
                    q = Q::None;
                    continue;
                }
                _ => {
                    return Err(format!(
                        "{} {} {}",
                        t("Die Variable", "The variable"),
                        var_label(&p),
                        t(
                            "steht innerhalb von Anführungszeichen – das würde das Quoting aushebeln. Anführungszeichen entfernen (Paneldeck quotet den Wert selbst) oder nur die Variable allein in Anführungszeichen setzen.",
                            "is inside quotes – that would defeat the quoting. Remove the quotes (Paneldeck quotes the value itself) or put only the variable alone in quotes.",
                        )
                    ));
                }
            }
        }
        let c = template[i..].chars().next().unwrap_or(' ');
        let cl = c.len_utf8();
        if escapes(kind, q, c) {
            i += cl;
            i += template[i..].chars().next().map_or(0, char::len_utf8);
            continue;
        }
        match (q, quote_class(kind, c)) {
            (Q::None, Some(n)) => {
                q = n;
                open = (i, cl);
            }
            (cur, Some(n)) if cur == n => q = Q::None,
            _ => {}
        }
        i += cl;
    }
    out.push(Piece::Text(&template[start..]));
    Ok(out)
}

/// Prüfung der Vorlage für eine Shell (Dialog, *Für Datei testen*): `Err` = würde nicht ausgeführt.
pub fn check_command(template: &str, quote: Kind) -> Result<(), String> {
    shell_pieces(template, quote).map(|_| ())
}

/// Werte für die Anzeige: geheime Werte (eigene Umgebungsvariablen der Regel) erscheinen als `•••`.
pub const HIDDEN: &str = "•••";

/// Pfad der Variable `name` für die Shell `t` (#392, `targetpath`): Git Bash `C:\x` → `/c/x`, UNC →
/// `//wsl.localhost/…`; WSL `C:\x` → `/mnt/c/x` (Einhängepunkt der Distribution), `\\wsl.localhost\<D>\x` → `/x` nur in
/// derselben Distribution. Eine andere Distribution, eine unbekannte eigene Distribution (Standard-Distribution noch
/// nicht bekannt) oder ein Ergebnis mit Steuerzeichen ist ein Fehler der Regel – nie ein Lauf mit falschem Pfad.
fn shell_path(t: &Target, name: &str, v: &str) -> Result<String, String> {
    shell_path_with(t, name, v, &crate::targetpath::Cached)
}

fn shell_path_with(t: &Target, name: &str, v: &str, lk: &dyn crate::targetpath::Lookup) -> Result<String, String> {
    if !t.posix() {
        return Ok(v.to_string());
    }
    crate::targetpath::for_insert_unquoted_with(&t.path_kind(), v, lk).map_err(|e| format!("${{{name}}}: {}", e.message(crate::state::lang())))
}

/// Befehl für die Shell `t` aus der Vorlage bauen. `cmd_lookup`: nur cmd – `%NAME%` der Vorlage vorher ersetzen (aus
/// der Umgebung des Laufs; `PANELDECK_*` und geheime Namen bleiben stehen). `env_lookup`: Wert für `${env:NAME}`.
/// `display`: für Anzeige/Verlauf – Werte, für die `secret` wahr ist, erscheinen als [`HIDDEN`]. Fehler: Variable in
/// Anführungszeichen, Pfad einer anderen WSL-Distribution.
pub fn expand_command(template: &str, ctx: &Ctx, t: &Target, cmd_lookup: Option<&dyn Fn(&str) -> Option<String>>, env_lookup: &dyn Fn(&str) -> Option<String>, secret: &dyn Fn(&str) -> bool, display: bool) -> Result<String, String> {
    let pre;
    let template = match (t.flavor, cmd_lookup) {
        (Flavor::Cmd, Some(l)) if !display => {
            let safe = |n: &str| if n.to_ascii_uppercase().starts_with("PANELDECK_") || secret(n) { None } else { l(n) };
            pre = crate::procs::expand_cmd_vars(template, safe);
            pre.as_str()
        }
        _ => template,
    };
    let mut out = String::with_capacity(template.len() + 32);
    for p in shell_pieces(template, t.quote)? {
        match p {
            Piece::Text(s) => out.push_str(s),
            Piece::Var(n) => {
                let mut v = ctx.value(n).unwrap_or_default();
                if t.posix() {
                    v = if is_path(n) { shell_path(t, n, &v)? } else if n == "relativeFile" { v.replace('\\', "/") } else { v };
                }
                out.push_str(&quote_for(t, &v));
            }
            Piece::Env(n) => {
                if display && secret(n) {
                    out.push_str(HIDDEN);
                } else {
                    out.push_str(&quote_for(t, &env_lookup(n).unwrap_or_default()));
                }
            }
        }
    }
    Ok(out)
}

/// Kommt eine geheime Variable der Regel als `${env:…}` vor? (Text an eine Sitzung landet sichtbar im Terminal.)
pub fn secret_used(template: &str, secret: &dyn Fn(&str) -> bool) -> Option<String> {
    pieces(template).into_iter().find_map(|p| match p {
        Piece::Env(n) if secret(n) => Some(n.to_string()),
        _ => None,
    })
}

/// Text ohne Quoting (Benachrichtigungen): Werte so, wie sie sind; geheime Werte nie.
pub fn expand_text(template: &str, ctx: &Ctx, env_lookup: &dyn Fn(&str) -> Option<String>, secret: &dyn Fn(&str) -> bool) -> String {
    let mut out = String::new();
    for p in pieces(template) {
        match p {
            Piece::Text(s) => out.push_str(s),
            Piece::Var(n) => out.push_str(&ctx.value(n).unwrap_or_default()),
            Piece::Env(n) if secret(n) => out.push_str(HIDDEN),
            Piece::Env(n) => out.push_str(&env_lookup(n).unwrap_or_default()),
        }
    }
    out
}

/// cmd: `%PANELDECK_…%` ohne direkt umschließende Anführungszeichen (cmd setzt den Wert ungequotet ein).
pub fn cmd_unquoted_env(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = template;
    let mut offset = 0;
    while let Some(i) = rest.find('%') {
        let after = &rest[i + 1..];
        let Some(j) = after.find('%') else { break };
        let name = &after[..j];
        let abs = offset + i;
        if name.to_ascii_uppercase().starts_with("PANELDECK_") {
            let before = template[..abs].ends_with('"');
            let behind = template[abs + j + 2..].starts_with('"');
            if !(before && behind) && !out.contains(&name.to_string()) {
                out.push(name.to_string());
            }
            rest = &after[j + 1..];
            offset = abs + j + 2;
        } else {
            rest = after;
            offset = abs + 1;
        }
    }
    out
}

/// `%NAME%`-Kandidaten einer cmd-Vorlage so durchgehen wie [`crate::procs::expand_cmd_vars`]: `keep` entscheidet, ob
/// ein Name ersetzt wird (sonst bleibt `%NAME` wörtlich und die Suche geht am schließenden `%` weiter). `f` bekommt je
/// Stück `Some(name)` für eine ersetzte Variable bzw. `(Text, None)` für wörtlichen Text.
fn cmd_scan(template: &str, keep: &dyn Fn(&str) -> bool, mut f: impl FnMut(&str, Option<&str>)) {
    let mut rest = template;
    while let Some(i) = rest.find('%') {
        f(&rest[..i], None);
        let after = &rest[i + 1..];
        let Some(j) = after.find('%') else {
            f("%", None);
            rest = after;
            continue;
        };
        let name = &after[..j];
        if !name.is_empty() && !name.contains(':') && !name.chars().any(|c| c.is_control()) && keep(name) {
            f("", Some(name));
            rest = &after[j + 1..];
        } else {
            f("%", None);
            f(name, None);
            rest = &after[j..];
        }
    }
    f(rest, None);
}

/// cmd: Steht in der Vorlage `%PANELDECK_…%` oder eine geheime Variable der Regel als `%NAME%`? Die ersetzt cmd erst
/// zur Laufzeit aus der Umgebung – dafür läuft der Befehl über eine Batch-Datei ([`cmd_batch_line`]), denn den Inhalt
/// der Variable, über die eine Befehlszeile an `cmd /c` geht, ersetzt cmd kein zweites Mal.
/// `defined`: Variable ist in der Umgebung des Laufs gesetzt (wie bei [`cmd_batch_line`]).
pub fn cmd_needs_batch(template: &str, defined: &dyn Fn(&str) -> bool, secret: &dyn Fn(&str) -> bool) -> bool {
    let runtime = |n: &str| n.to_ascii_uppercase().starts_with("PANELDECK_") || secret(n);
    let mut found = false;
    cmd_scan(template, &|n| runtime(n) || defined(n), |_, n| {
        if let Some(n) = n {
            found |= runtime(n);
        }
    });
    found
}

/// cmd über eine Batch-Datei: Zeile für die Datei. Sie verhält sich wie die getippte Befehlszeile (#383):
/// * `%NAME%` einer gesetzten Variable (`defined`), `%PANELDECK_…%` und geheime Variablen bleiben stehen – cmd ersetzt
///   sie beim Ausführen aus der Umgebung des Laufs, **einmal** (ein `%` oder `&` im Wert wird nicht noch einmal
///   ausgewertet; geheime Werte stehen so nie in der Datei oder in der Befehlszeile).
/// * jedes andere `%` (unbekannte Variable, einzelnes `%`, `for %f`, `%1`) wird zu `%%`, also wörtlich wie auf der
///   Befehlszeile – ebenso jedes `%` in eingesetzten Werten (`${file}` mit `%` im Namen).
/// * `${env:NAME}` einer geheimen Variable wird `"%NAME%"` (Wert erst zur Laufzeit), andere wie bei [`expand_command`].
pub fn cmd_batch_line(template: &str, ctx: &Ctx, t: &Target, defined: &dyn Fn(&str) -> bool, env_lookup: &dyn Fn(&str) -> Option<String>, secret: &dyn Fn(&str) -> bool) -> Result<String, String> {
    let mut pre = String::with_capacity(template.len() + 16);
    let keep = |n: &str| n.to_ascii_uppercase().starts_with("PANELDECK_") || secret(n) || defined(n);
    cmd_scan(template, &keep, |text, name| match name {
        Some(n) => {
            pre.push('%');
            pre.push_str(n);
            pre.push('%');
        }
        None => pre.push_str(&text.replace('%', "%%")),
    });
    let mut out = String::with_capacity(pre.len() + 32);
    for p in shell_pieces(&pre, t.quote)? {
        match p {
            Piece::Text(s) => out.push_str(s),
            Piece::Var(n) => out.push_str(&quote_for(t, &ctx.value(n).unwrap_or_default()).replace('%', "%%")),
            Piece::Env(n) if secret(n) => out.push_str(&format!("\"%{n}%\"")),
            Piece::Env(n) => out.push_str(&quote_for(t, &env_lookup(n).unwrap_or_default()).replace('%', "%%")),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(name: &str) -> Ctx {
        let (root, ws) = if cfg!(windows) { (r"C:\work\my app", r"C:\work") } else { ("/work/my app", "/work") };
        Ctx { file: PathBuf::from(root).join("src").join(name), rule_folder: PathBuf::from(root), workspace: Some(PathBuf::from(ws)) }
    }

    fn none(_: &str) -> Option<String> {
        None
    }
    fn no(_: &str) -> bool {
        false
    }

    fn run(tpl: &str, c: &Ctx, f: Flavor) -> Result<String, String> {
        expand_command(tpl, c, &Target::test(f), None, &none, &no, false)
    }

    #[test]
    fn values_like_vs_code() {
        let c = ctx("a.test.ts");
        assert_eq!(c.value("fileBasename").unwrap(), "a.test.ts");
        assert_eq!(c.value("fileBasenameNoExtension").unwrap(), "a.test");
        assert_eq!(c.value("fileExtname").unwrap(), ".ts");
        assert_eq!(c.value("relativeFile").unwrap(), Path::new("src").join("a.test.ts").to_string_lossy());
        assert!(c.value("workspaceFolder").unwrap().ends_with("work"));
        assert_eq!(c.env().len(), NAMES.len());
        assert!(c.env().iter().any(|(k, v)| k == "PANELDECK_FILE" && v.ends_with("a.test.ts")));
        let w = c.wslenv(Some("FOO/u"), &["TOKEN".to_string()]);
        assert!(w.starts_with("FOO/u:") && w.contains("PANELDECK_FILE/p") && w.contains("PANELDECK_FILE_BASENAME:") && w.ends_with(":TOKEN"), "{w}");
        // Ordner ohne Trenner am Ende (sonst `\"` vor dem schließenden Anführungszeichen).
        let mut c2 = ctx("x");
        c2.rule_folder = PathBuf::from(if cfg!(windows) { r"C:\My Dir\" } else { "/my dir/" });
        assert!(!c2.value("ruleFolder").unwrap().ends_with(['\\', '/']));
        if cfg!(windows) {
            c2.rule_folder = PathBuf::from(r"C:\");
            assert_eq!(c2.value("ruleFolder").unwrap(), r"C:\");
            // Wurzel: `\` am Ende verdoppelt für cmd/PowerShell.
            assert_eq!(quote_for(&Target::test(Flavor::Cmd), r"C:\"), r#""C:\\""#);
            assert_eq!(quote_for(&Target::test(Flavor::Pwsh), r"C:\"), r"'C:\\'");
        }
    }

    /// Unangenehme Namen bleiben in jeder Shell ein wörtliches Argument.
    #[test]
    fn quoting_per_shell_with_nasty_names() {
        let nasty = "100% & it's $HOME `x` (1).ts";
        let c = ctx(nasty);
        let tpl = "lint ${fileBasename}";
        assert_eq!(run(tpl, &c, Flavor::Pwsh).unwrap(), "lint '100% & it''s $HOME `x` (1).ts'");
        assert_eq!(run(tpl, &c, Flavor::Bash).unwrap(), r"lint '100% & it'\''s $HOME `x` (1).ts'");
        let x = |n: &str| (n == "X").then(|| "1".to_string());
        let cmd = expand_command(tpl, &c, &Target::test(Flavor::Cmd), Some(&x), &none, &no, false).unwrap();
        assert_eq!(cmd, "lint \"100% & it's $HOME `x` (1).ts\"");
        // cmd: `%X%` der Vorlage wird ersetzt, ein `%` im Wert nicht.
        let cmd2 = expand_command("echo %X% ${fileBasename}", &c, &Target::test(Flavor::Cmd), Some(&x), &none, &no, false).unwrap();
        assert_eq!(cmd2, "echo 1 \"100% & it's $HOME `x` (1).ts\"");
        // Typografische Apostrophe in PowerShell verdoppelt.
        let c2 = ctx("o’brien.txt");
        assert_eq!(run("${fileBasename}", &c2, Flavor::Pwsh).unwrap(), "'o’’brien.txt'");
    }

    /// Variablen in Anführungszeichen: genau umschlossen → Anführungszeichen fallen weg; sonst abgelehnt – je Shell mit
    /// bösartigen Namen.
    #[test]
    fn variables_inside_quotes_are_refused_or_unwrapped() {
        let names = ["x$(calc).ts", "a & calc.txt", "b`calc`.txt", "c;calc.txt", "d'e.txt", "f\"g.txt", "h%PATH%.txt", "i!x!.txt"];
        for f in [Flavor::Pwsh, Flavor::Cmd, Flavor::Bash, Flavor::Wsl] {
            for n in names {
                let c = ctx(n);
                let plain = run("lint ${fileBasename}", &c, f).unwrap();
                // Genau umschlossen: dasselbe Ergebnis wie ohne Anführungszeichen.
                assert_eq!(run("lint \"${fileBasename}\"", &c, f).unwrap(), plain, "{f:?} {n}");
                if f != Flavor::Cmd {
                    assert_eq!(run("lint '${fileBasename}'", &c, f).unwrap(), plain, "{f:?} {n}");
                }
                // Innerhalb einer größeren Zeichenkette: abgelehnt.
                for tpl in ["lint \"--out=${fileBasename}\"", "lint \"${fileBasename} x\"", "echo \"${env:HOME}/x\""] {
                    assert!(run(tpl, &c, f).is_err(), "{f:?} {n}: {tpl}");
                }
                if f != Flavor::Cmd {
                    assert!(run("lint '--out=${fileBasename}'", &c, f).is_err(), "{f:?} {n}");
                }
                // Der eingesetzte Wert ist ein einziges gequotetes Wort.
                let q = &plain["lint ".len()..];
                match f {
                    Flavor::Cmd => assert!(q.starts_with('"') && q.ends_with('"') && !q[1..q.len() - 1].contains('"'), "{q}"),
                    _ => assert!(q.starts_with('\'') && q.ends_with('\''), "{q}"),
                }
            }
        }
        // Maskierte Anführungszeichen zählen nicht: PowerShell `\" (Backtick), bash \".
        assert!(run("echo `\"${fileBasename}", &ctx("a.txt"), Flavor::Pwsh).is_ok());
        assert!(run("echo \\\"${fileBasename}", &ctx("a.txt"), Flavor::Bash).is_ok());
        // Typografische Anführungszeichen in PowerShell.
        assert!(run("echo “--x=${fileBasename}”", &ctx("a.txt"), Flavor::Pwsh).is_err());
        assert!(check_command("a \"b\" ${file}", Kind::Pwsh).is_ok());
    }

    #[test]
    fn cmd_keeps_paneldeck_and_secret_variables_out_of_the_pre_expansion() {
        let c = ctx("a & calc.txt");
        let look = |n: &str| match n.to_ascii_uppercase().as_str() {
            "PANELDECK_FILE" => Some("C:\\x\\a & calc.txt".to_string()),
            "TOKEN" => Some("s3".to_string()),
            "OTHER" => Some("o".to_string()),
            _ => None,
        };
        let secret = |n: &str| n == "TOKEN";
        let line = expand_command("type %PANELDECK_FILE% %TOKEN% %OTHER%", &c, &Target::test(Flavor::Cmd), Some(&look), &none, &secret, false).unwrap();
        assert_eq!(line, "type %PANELDECK_FILE% %TOKEN% o");
        assert_eq!(cmd_unquoted_env("type %PANELDECK_FILE% \"%PANELDECK_RULE_FOLDER%\" %OTHER%"), ["PANELDECK_FILE"]);
        assert!(cmd_unquoted_env("type \"%PANELDECK_FILE%\"").is_empty());
    }

    /// cmd mit `%PANELDECK_…%`/geheimen `%NAME%` läuft über eine Batch-Datei: Diese Namen und gesetzte Variablen bleiben
    /// als `%NAME%` (cmd ersetzt sie zur Laufzeit einmal), jedes andere `%` – auch in eingesetzten Dateinamen – wird
    /// `%%` (wörtlich wie auf der Befehlszeile); geheime Werte stehen nie in der Zeile.
    #[test]
    fn cmd_batch_line_keeps_runtime_variables_and_escapes_every_other_percent() {
        let c = ctx("100% & %OS%.txt");
        let defined = |n: &str| ["OTHER", "PANELDECK_FILE", "TOKEN"].iter().any(|x| x.eq_ignore_ascii_case(n));
        let secret = |n: &str| n.eq_ignore_ascii_case("TOKEN");
        let env = |n: &str| match n {
            "TOKEN" => Some("geheim".to_string()),
            "PLAIN" => Some("p%q".to_string()),
            _ => None,
        };
        let t = Target::test(Flavor::Cmd);
        assert!(cmd_needs_batch("type \"%PANELDECK_FILE%\"", &defined, &secret));
        assert!(cmd_needs_batch("curl -H \"%token%\"", &defined, &secret), "geheim, ohne Groß-/Kleinschreibung");
        assert!(!cmd_needs_batch("echo %OTHER% 100% ${file}", &defined, &secret), "sonst wie bisher über die Befehlszeile");
        assert!(!cmd_needs_batch("echo %OTHER:~0,1%PANELDECK_FILE", &defined, &secret));
        let line = cmd_batch_line("type \"%PANELDECK_FILE%\" && echo %OTHER% %NOPE% 50% %% for %f in (*) do @echo %f ${fileBasename} \"%TOKEN%\" ${env:TOKEN} ${env:PLAIN}", &c, &t, &defined, &env, &secret).unwrap();
        assert_eq!(line, "type \"%PANELDECK_FILE%\" && echo %OTHER% %%NOPE%% 50%% %%%% for %%f in (*) do @echo %%f \"100%% & %%OS%%.txt\" \"%TOKEN%\" \"%TOKEN%\" \"p%%q\"");
        assert!(!line.contains("geheim"));
        // Wie `expand_cmd_vars`: eine unbekannte Variable gibt ihr schließendes `%` an die nächste weiter
        assert_eq!(cmd_batch_line("echo %NOPE%OTHER% %PANELDECK_FILE%", &c, &t, &defined, &none, &secret).unwrap(), "echo %%NOPE%OTHER% %PANELDECK_FILE%");
        // Variable in Anführungszeichen bleibt ein Fehler der Regel
        assert!(cmd_batch_line("echo \"x${file}\" %PANELDECK_FILE%", &c, &t, &defined, &none, &secret).is_err());
    }

    #[test]
    fn paths_are_converted_for_posix_shells_and_unknown_names_stay() {
        let c = ctx("a.ts");
        let wsl = run("cat ${file} ${relativeFile}", &c, Flavor::Wsl).unwrap();
        if cfg!(windows) {
            assert_eq!(wsl, "cat '/mnt/c/work/my app/src/a.ts' 'src/a.ts'");
            assert_eq!(run("cat ${file}", &c, Flavor::Bash).unwrap(), "cat '/c/work/my app/src/a.ts'");
            // Datei in WSL: in der passenden Distribution der Linux-Pfad, in einer anderen ein Fehler.
            let w = Ctx { file: PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me\a.ts"), rule_folder: PathBuf::from(r"\\wsl.localhost\Ubuntu\home\me"), workspace: None };
            assert_eq!(run("cat ${file}", &w, Flavor::Wsl).unwrap(), "cat '/home/me/a.ts'");
            let mut other = Target::test(Flavor::Wsl);
            other.distro = Some("Debian".into());
            assert!(expand_command("cat ${file}", &w, &other, None, &none, &no, false).is_err());
            // Git Bash: eine WSL-Datei als `//wsl.localhost/…` (#392)
            assert_eq!(run("cat ${file}", &w, Flavor::Bash).unwrap(), "cat '//wsl.localhost/Ubuntu/home/me/a.ts'");
        }
        assert_eq!(run("echo ${HOME} ${foo", &c, Flavor::Bash).unwrap(), "echo ${HOME} ${foo");
    }

    /// #392: Pfade über `targetpath` – andere Distribution, unbekannte eigene Distribution und Steuerzeichen sind Fehler
    /// der Regel (mit dem Namen der Variable), nie ein Lauf mit falschem Pfad.
    #[test]
    fn path_errors_are_rule_errors() {
        struct Lk(Option<&'static str>);
        impl crate::targetpath::Lookup for Lk {
            fn wsl_home(&self, _d: &str) -> Option<String> {
                Some("/home/me".into())
            }
            fn wsl_mount(&self, _d: &str) -> Option<String> {
                None
            }
            fn default_distro(&self) -> Option<String> {
                self.0.map(str::to_string)
            }
            fn win_home(&self) -> String {
                r"C:\Users\me".into()
            }
        }
        let unc = r"\\wsl.localhost\Ubuntu\home\me\a.ts";
        let mut wsl = Target::test(Flavor::Wsl);
        assert_eq!(shell_path_with(&wsl, "file", unc, &Lk(None)).as_deref(), Ok("/home/me/a.ts"));
        wsl.distro = Some("Debian".into());
        let e = shell_path_with(&wsl, "file", unc, &Lk(None)).unwrap_err();
        assert!(e.starts_with("${file}:") && e.contains("Ubuntu") && e.contains("Debian"), "{e}");
        // `wsl.exe` ohne `-d`: Standard-Distribution unbekannt → Fehler; bekannt und gleich → Linux-Pfad
        wsl.distro = None;
        assert!(shell_path_with(&wsl, "file", unc, &Lk(None)).is_err());
        assert_eq!(shell_path_with(&wsl, "file", unc, &Lk(Some("Ubuntu"))).as_deref(), Ok("/home/me/a.ts"));
        assert!(shell_path_with(&wsl, "file", unc, &Lk(Some("Debian"))).is_err());
        // 9P-Zeichen werden in WSL zu Steuerzeichen: nie in einen Befehl
        let ctl = "\\\\wsl.localhost\\Ubuntu\\home\\me\\a\u{F001}b.ts";
        assert!(shell_path_with(&Target::test(Flavor::Wsl), "file", ctl, &Lk(None)).is_err());
        // Windows-Shells bekommen den Pfad unverändert
        assert_eq!(shell_path_with(&Target::test(Flavor::Pwsh), "file", unc, &Lk(None)).as_deref(), Ok(unc));
    }

    #[test]
    fn env_values_and_secrets() {
        let c = ctx("a.ts");
        let look = |n: &str| match n {
            "TOKEN" => Some("s3cr3t".to_string()),
            "HOST" => Some("example.org".to_string()),
            _ => None,
        };
        let secret = |n: &str| n == "TOKEN";
        let tpl = "up --host ${env:HOST} --token ${env:TOKEN} ${env:MISSING}";
        let r = expand_command(tpl, &c, &Target::test(Flavor::Pwsh), None, &look, &secret, false).unwrap();
        assert_eq!(r, "up --host 'example.org' --token 's3cr3t' ''");
        let shown = expand_command(tpl, &c, &Target::test(Flavor::Pwsh), None, &look, &secret, true).unwrap();
        assert!(!shown.contains("s3cr3t") && shown.contains(HIDDEN));
        assert_eq!(expand_text("Saved ${fileBasename} with ${env:TOKEN}", &c, &look, &secret), format!("Saved a.ts with {HIDDEN}"));
        assert_eq!(secret_used("make ${env:TOKEN}", &secret).as_deref(), Some("TOKEN"));
        assert!(secret_used("make ${env:HOST}", &secret).is_none());
    }
}
