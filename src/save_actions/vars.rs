//! Variablen in Befehlen und Texten (#383): `${file}`, `${fileDirname}`, `${fileBasename}`,
//! `${fileBasenameNoExtension}`, `${fileExtname}`, `${relativeFile}`, `${ruleFolder}`, `${workspaceFolder}`,
//! `${env:NAME}`.
//!
//! In Befehlen wird jeder Wert für die gewählte Shell gequotet (ein wörtliches Argument – auch bei Namen mit `% & ' $`,
//! Backticks oder Leerzeichen); Anführungszeichen um eine Variable sind also nicht nötig. Unbekannte Namen bleiben
//! stehen (bash und PowerShell kennen selbst `${…}`). Dieselben Werte gibt es außerdem als Umgebungsvariablen
//! (`PANELDECK_FILE` …, [`Ctx::env`]) – der sichere Weg für eigene Skripte.
//!
//! cmd: Die Befehlszeile geht über eine Umgebungsvariable an `cmd /c` (siehe `procs::launch_spec`), deren Inhalt cmd
//! nicht noch einmal ersetzt. Deshalb ersetzt der Aufrufer `%NAME%` der Vorlage **vor** dem Einsetzen der Werte
//! ([`expand_command`] mit `cmd_lookup`) – ein `%` in einem Dateinamen bleibt so wörtlich.

use super::relative;
use crate::procs::Flavor;
use crate::quote::{self, Kind};
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

/// Wofür eingesetzt wird.
#[derive(Clone, Debug)]
pub struct Ctx {
    pub file: PathBuf,
    pub rule_folder: PathBuf,
    /// Geöffneter Ordner des Editors, wenn die Datei darin liegt (sonst gilt der Regelordner).
    pub workspace: Option<PathBuf>,
}

impl Ctx {
    /// Wert einer Variable (Pfade so, wie Windows bzw. das System sie schreibt).
    pub fn value(&self, name: &str) -> Option<String> {
        let f = &self.file;
        Some(match name {
            "file" => f.to_string_lossy().into_owned(),
            "fileDirname" => f.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            "fileBasename" => f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            "fileBasenameNoExtension" => f.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            "fileExtname" => f.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default(),
            "relativeFile" => relative(f, &self.rule_folder).map(|r| r.to_string_lossy().into_owned()).unwrap_or_else(|| f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()),
            "ruleFolder" => self.rule_folder.to_string_lossy().into_owned(),
            "workspaceFolder" => self.workspace().to_string_lossy().into_owned(),
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

    /// WSL: `WSLENV`, damit die Variablen in die Distribution mitgehen (Pfade mit `/p` übersetzt).
    pub fn wslenv(&self, existing: Option<&str>) -> String {
        let mut parts: Vec<String> = existing.map(|e| e.split(':').filter(|s| !s.is_empty()).map(str::to_string).collect()).unwrap_or_default();
        for n in NAMES {
            let e = env_name(n);
            if !parts.iter().any(|p| p.split('/').next().is_some_and(|k| k.eq_ignore_ascii_case(e))) {
                parts.push(if is_path(n) { format!("{e}/p") } else { e.to_string() });
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
    /// Kennung der Shell (`gitbash`, `wsl:<Distro>` … – für die Umrechnung der Pfade).
    pub shell_id: String,
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
        Target { flavor, quote, shell_id: shell.id.clone() }
    }

    #[cfg(test)]
    pub fn test(flavor: Flavor) -> Target {
        let (quote, id) = match flavor {
            Flavor::Pwsh => (Kind::Pwsh, "powershell"),
            Flavor::Cmd => (Kind::Cmd, "cmd"),
            Flavor::Bash => (Kind::Bash, "gitbash"),
            Flavor::Wsl => (Kind::Posix, "wsl:Ubuntu"),
        };
        Target { flavor, quote, shell_id: id.into() }
    }

    fn posix(&self) -> bool {
        matches!(self.flavor, Flavor::Bash | Flavor::Wsl)
    }
}

/// Einen Wert als ein wörtliches Argument für die Shell.
pub fn quote_for(t: &Target, value: &str) -> String {
    match t.quote {
        // cmd: in "…" sind & | < > ^ wörtlich; ein " kann in Windows-Namen nicht vorkommen (in anderen Werten fällt es weg).
        Kind::Cmd => format!("\"{}\"", value.replace('"', "")),
        k => quote::quote(k, value),
    }
}

/// Teil einer Vorlage.
enum Piece<'a> {
    Text(&'a str),
    Var(&'a str),
    Env(&'a str),
}

/// Vorlage in Text und Variablen zerlegen. Unbekannte `${…}` bleiben Text.
fn pieces(template: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(i) = rest.find("${") {
        let after = &rest[i + 2..];
        let Some(j) = after.find('}') else { break };
        let name = &after[..j];
        let known = NAMES.contains(&name);
        let env = name.strip_prefix("env:").filter(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '(' || c == ')'));
        if known || env.is_some() {
            out.push(Piece::Text(&rest[..i]));
            out.push(if let Some(e) = env { Piece::Env(e) } else { Piece::Var(name) });
        } else {
            out.push(Piece::Text(&rest[..i + 2 + j + 1]));
        }
        rest = &after[j + 1..];
    }
    out.push(Piece::Text(rest));
    out
}

/// Werte für die Anzeige: geheime Werte (eigene Umgebungsvariablen der Regel) erscheinen als `•••`.
pub const HIDDEN: &str = "•••";

/// Befehl für die Shell `t` aus der Vorlage bauen. `cmd_lookup`: nur cmd – `%NAME%` der Vorlage vorher ersetzen (aus
/// der Umgebung des Laufs). `env_lookup`: Wert für `${env:NAME}`. `display`: für Anzeige/Verlauf – Werte, für die
/// `secret` wahr ist, erscheinen als [`HIDDEN`].
pub fn expand_command(template: &str, ctx: &Ctx, t: &Target, cmd_lookup: Option<&dyn Fn(&str) -> Option<String>>, env_lookup: &dyn Fn(&str) -> Option<String>, secret: &dyn Fn(&str) -> bool, display: bool) -> String {
    let pre;
    let template = match (t.flavor, cmd_lookup) {
        (Flavor::Cmd, Some(l)) if !display => {
            pre = crate::procs::expand_cmd_vars(template, l);
            pre.as_str()
        }
        _ => template,
    };
    let mut out = String::with_capacity(template.len() + 32);
    for p in pieces(template) {
        match p {
            Piece::Text(s) => out.push_str(s),
            Piece::Var(n) => {
                let mut v = ctx.value(n).unwrap_or_default();
                if t.posix() {
                    v = if is_path(n) { crate::pathfmt::convert(&t.shell_id, &v) } else if n == "relativeFile" { v.replace('\\', "/") } else { v };
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
    out
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

/// Hinweise zur Vorlage: Variablen direkt in Anführungszeichen (sie werden ohnehin gequotet).
pub fn quoted_vars(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    for q in ['"', '\''] {
        for n in NAMES {
            let needle = format!("{q}${{{n}}}{q}");
            if template.contains(&needle) && !out.contains(&n.to_string()) {
                out.push(n.to_string());
            }
        }
    }
    out
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
        let w = c.wslenv(Some("FOO/u"));
        assert!(w.starts_with("FOO/u:") && w.contains("PANELDECK_FILE/p") && w.contains("PANELDECK_FILE_BASENAME:"));
    }

    /// Unangenehme Namen bleiben in jeder Shell ein wörtliches Argument.
    #[test]
    fn quoting_per_shell_with_nasty_names() {
        let nasty = "100% & it's $HOME `x` (1).ts";
        let c = ctx(nasty);
        let t = "lint ${fileBasename}";
        let pw = expand_command(t, &c, &Target::test(Flavor::Pwsh), None, &none, &no, false);
        assert_eq!(pw, "lint '100% & it''s $HOME `x` (1).ts'");
        let bash = expand_command(t, &c, &Target::test(Flavor::Bash), None, &none, &no, false);
        assert_eq!(bash, r"lint '100% & it'\''s $HOME `x` (1).ts'");
        let cmd = expand_command(t, &c, &Target::test(Flavor::Cmd), Some(&|n: &str| (n == "X").then(|| "1".to_string())), &none, &no, false);
        assert_eq!(cmd, "lint \"100% & it's $HOME `x` (1).ts\"");
        // cmd: `%X%` der Vorlage wird ersetzt, ein `%` im Wert nicht.
        let cmd2 = expand_command("echo %X% ${fileBasename}", &c, &Target::test(Flavor::Cmd), Some(&|n: &str| (n == "X").then(|| "1".to_string())), &none, &no, false);
        assert_eq!(cmd2, "echo 1 \"100% & it's $HOME `x` (1).ts\"");
        // Typografische Apostrophe in PowerShell verdoppelt.
        let c2 = ctx("o’brien.txt");
        assert_eq!(expand_command("${fileBasename}", &c2, &Target::test(Flavor::Pwsh), None, &none, &no, false), "'o’’brien.txt'");
    }

    #[test]
    fn paths_are_converted_for_posix_shells_and_unknown_names_stay() {
        let c = ctx("a.ts");
        let wsl = expand_command("cat ${file} ${relativeFile}", &c, &Target::test(Flavor::Wsl), None, &none, &no, false);
        if cfg!(windows) {
            assert_eq!(wsl, "cat '/mnt/c/work/my app/src/a.ts' 'src/a.ts'");
            let gb = expand_command("cat ${file}", &c, &Target::test(Flavor::Bash), None, &none, &no, false);
            assert_eq!(gb, "cat '/c/work/my app/src/a.ts'");
        }
        assert_eq!(expand_command("echo ${HOME} ${foo", &c, &Target::test(Flavor::Bash), None, &none, &no, false), "echo ${HOME} ${foo");
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
        let t = "up --host ${env:HOST} --token ${env:TOKEN} ${env:MISSING}";
        let run = expand_command(t, &c, &Target::test(Flavor::Pwsh), None, &look, &secret, false);
        assert_eq!(run, "up --host 'example.org' --token 's3cr3t' ''");
        let shown = expand_command(t, &c, &Target::test(Flavor::Pwsh), None, &look, &secret, true);
        assert!(!shown.contains("s3cr3t") && shown.contains(HIDDEN));
        assert_eq!(expand_text("Saved ${fileBasename} with ${env:TOKEN}", &c, &look, &secret), format!("Saved a.ts with {HIDDEN}"));
    }

    #[test]
    fn quoted_variables_are_hinted() {
        assert_eq!(quoted_vars("eslint \"${file}\" '${relativeFile}' ${fileDirname}"), ["file", "relativeFile"]);
        assert!(quoted_vars("eslint ${file}").is_empty());
    }
}
