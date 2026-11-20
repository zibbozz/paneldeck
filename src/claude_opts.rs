//! Startoptionen für Claude Code (#161): Modell, Effort, Permission-Mode und Agent – dazu (#182) weitere Schalter und
//! Wertoptionen ([`ExtraOpts`]).
//!
//! - Nur geprüfte Werte, keine freien Argumente: Modell/Agent bestehen aus `[A-Za-z0-9._:-]` (höchstens
//!   [`MAX_NAME`] Zeichen, erstes Zeichen Buchstabe oder Ziffer – sonst könnte ein Wert wie eine Option aussehen),
//!   Effort und Permission-Mode stammen aus festen Listen.
//! - `bypassPermissions` gibt es nur, wenn der Nutzer es im Dialog „Neue Sitzung“ ausdrücklich für diese eine Sitzung
//!   wählt: nie als gespeicherter Standard, nie als „letzte Wahl“, nie über MCP, Kontextmenü, `--claude`, IPC, Vorlagen
//!   oder Verlauf ([`resolve_new`] mit `allow_bypass = false`).
//! - `manual` (neuere Claude-Code-Versionen) wird zu `default` (versteht jede Version).
//! - Agenten-Sitzungen ohne Permission-Mode bekommen ausdrücklich `default` ([`ClaudeOpts::for_agent`]); sonst gälte
//!   `permissions.defaultMode` aus `~/.claude/settings.json` des Nutzers, und das kann `bypassPermissions` sein.
//! - Weitere Optionen (#182), feste geprüfte Liste im Code (neue kommen mit Updates; Grundlage ist eine dokumentierte
//!   Tabelle im Test `switch_table`, nie `claude --help` zur Laufzeit):
//!   - Schalter ohne Wert, die auch Agenten über MCP setzen dürfen ([`SWITCHES`], z. B. `--chrome`): ohne Einfluss auf
//!     Rechte, Zugriffe oder die Hooks, auf denen Paneldecks Status- und Freigabe-Erkennung beruht.
//!   - Schalter nur für den Nutzer ([`USER_SWITCHES`]): `--safe-mode` schaltet Hooks und MCP-Server ab – dann erkennt
//!     Paneldeck weder Status noch Freigabe-Fragen der Sitzung (der Schutz `permission_pending` greift nicht).
//!   - Wertoptionen (`--add-dir`, `--plugin-dir`, `--allowed-tools`, `--disallowed-tools`, `--mcp-config` (Liste) mit
//!     `--strict-mcp-config`, `--system-prompt`, `--append-system-prompt`, `--fallback-model`, `--max-budget-usd`,
//!     `--autocompact`; Listen und eigener System-Prompt seit #289): nur der Nutzer in Paneldeck, nie über MCP. Ein
//!     eigener System-Prompt, MCP-Konfigurationen und Plugin-Ordner gelten nie für Agenten-Sitzungen. Die
//!     Rechte erweiternden ([`RISKY`]) und die Nutzer-Schalter gelten nie für Sitzungen, die ein Agent anlegt – auch nicht
//!     als Standard ([`ExtraOpts::agent_safe`]). Pfade müssen lokal sein und existieren (geprüft bei jedem Start, ohne
//!     je eine Netzwerkfreigabe zu berühren, [`check_path`]); Texte enthalten keine Zeichen, die eine Shell auswertet;
//!     gequotet wird je Shell, Pfade passend zur Shell umgerechnet ([`PathStyle`]).
//! - Gespeichert in `settings.json` (Standards, letzte Wahl; nur gültige Werte ohne `bypassPermissions`,
//!   [`ser_user_opts`]) und je Sitzung in `state.json`; beim Lesen werden ungültige Werte still verworfen.
//!
//! Kennt kein egui.

use crate::quote::{quote, Kind};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Component, Path, PathBuf};

/// Höchstlänge von Modell- und Agentenname.
pub const MAX_NAME: usize = 100;
/// Erlaubte Effort-Stufen (`--effort`).
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
/// Permission-Modi, die auch ein Agent setzen darf (`--permission-mode`). `default` = bei allem fragen
/// (heißt in neueren Claude-Code-Versionen `manual`, die `default` weiter annehmen).
pub const PERMISSION_MODES: [&str; 5] = ["acceptEdits", "auto", "plan", "default", "dontAsk"];
/// Neuer Name von `default`; wird beim Lesen und Prüfen zu `default`.
pub const MANUAL: &str = "manual";
/// Nur vom Nutzer selbst wählbar, nie über MCP.
pub const BYPASS: &str = "bypassPermissions";
/// Vorschläge für das Modell in der Oberfläche (Claude-Code-Aliase).
pub const MODEL_ALIASES: [&str; 4] = ["fable", "opus", "sonnet", "haiku"];

/// Schalter ohne Wert, die auch Agenten setzen dürfen (#182): (Name in MCP und JSON, Flag). Ohne Einfluss auf Rechte,
/// Zugriffe oder Hooks. `chrome` und `no-chrome` schließen sich aus.
pub const SWITCHES: [(&str, &str); 7] = [
    ("chrome", "--chrome"),
    ("no-chrome", "--no-chrome"),
    ("ide", "--ide"),
    ("verbose", "--verbose"),
    ("brief", "--brief"),
    ("restricted", "--restricted"),
    ("disable-slash-commands", "--disable-slash-commands"),
];
/// Schalter ohne Wert nur für den Nutzer: nie über MCP, nie für Sitzungen von Agenten (auch nicht als Standard).
/// `--safe-mode` schaltet Hooks und MCP-Server ab (keine Status-/Freigabe-Erkennung durch Paneldeck).
pub const USER_SWITCHES: [(&str, &str); 1] = [("safe-mode", "--safe-mode")];
/// Flags, die über MCP nie erlaubt sind (eigene Fehlermeldung; alles außerhalb von [`SWITCHES`] ist ohnehin abgelehnt):
/// Rechte oder Zugriffe erweitern, Hooks abschalten, Ausgabe/Konversation steuern (das macht Paneldeck),
/// Fernsteuerung oder Wertoptionen des Nutzers. Dazu jedes Flag mit `dangerously` im Namen.
pub const NEVER_VIA_MCP: [&str; 32] = [
    "add-dir",
    "allowed-tools",
    "allowedtools",
    "disallowed-tools",
    "disallowedtools",
    "tools",
    "mcp-config",
    "strict-mcp-config",
    "plugin-dir",
    "settings",
    "setting-sources",
    "agents",
    "permission-prompt-tool",
    "print",
    "p",
    "output-format",
    "input-format",
    "resume",
    "r",
    "continue",
    "c",
    "session-id",
    "fork-session",
    "remote-control",
    "teleport",
    "cloud",
    "append-system-prompt",
    "system-prompt",
    "fallback-model",
    "max-budget-usd",
    "safe-mode",
    "autocompact",
];
/// Parameter von `create_session` für Wertoptionen und gefährliche Flags: angegeben → `bad_args` statt still
/// ignoriert (nur der Nutzer kann sie in Paneldeck setzen). Allgemeine Namen (`args`, `settings` …) bleiben wie
/// bisher unbeachtet.
pub const USER_ONLY_PARAMS: [&str; 17] = [
    "add_dir",
    "add_dirs",
    "plugin_dirs",
    "mcp_configs",
    "allowed_tools",
    "disallowed_tools",
    "mcp_config",
    "strict_mcp_config",
    "plugin_dir",
    "append_system_prompt",
    "system_prompt",
    "fallback_model",
    "max_budget_usd",
    "autocompact",
    "permission_prompt_tool",
    "dangerously_skip_permissions",
    "safe_mode",
];
/// Wertoptionen, die Rechte oder Zugriffe erweitern (Namen wie in [`ExtraOpts::value_names`]): Warnung in der
/// Oberfläche, nie für Sitzungen von Agenten ([`ExtraOpts::agent_safe`]).
pub const RISKY: [&str; 4] = ["add_dir", "plugin_dir", "allowed_tools", "mcp_config"];
/// Höchstzahl der Einträge in `claude_flags`.
pub const MAX_MCP_FLAGS: usize = 16;
/// Höchstlänge eines Pfads (`--add-dir`, `--plugin-dir`, `--mcp-config`).
pub const MAX_PATH_LEN: usize = 400;
/// Höchstzahl der Einträge je Liste (zusätzliche Ordner, Plugin-Ordner, MCP-Konfigurationen).
pub const MAX_DIRS: usize = 20;
/// Höchstlänge der Werkzeuglisten.
pub const MAX_TOOLS_LEN: usize = 1000;
/// Höchstlänge des Zusatzes zum System-Prompt.
pub const MAX_TEXT_LEN: usize = 2000;
/// Höchstlänge des eigenen System-Prompts (`--system-prompt`, mehrzeilig; die Zeile wird in die Shell getippt).
pub const MAX_SYSTEM_PROMPT_LEN: usize = 4000;

/// Modell- oder Agentenname gültig?
pub fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_NAME
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b':' | b'-'))
}

pub fn valid_effort(s: &str) -> bool {
    EFFORTS.contains(&s)
}

/// Permission-Mode gültig? `bypassPermissions` nur mit `allow_bypass` (Wahl des Nutzers).
pub fn valid_permission_mode(s: &str, allow_bypass: bool) -> bool {
    PERMISSION_MODES.contains(&s) || (allow_bypass && s == BYPASS)
}

/// Flag eines Schalters aus [`SWITCHES`] oder [`USER_SWITCHES`].
pub fn switch_flag(name: &str) -> Option<&'static str> {
    SWITCHES.iter().chain(USER_SWITCHES.iter()).find(|(n, _)| *n == name).map(|(_, f)| *f)
}

/// Darf ein Agent diesen Schalter setzen ([`SWITCHES`])?
pub fn agent_switch(name: &str) -> bool {
    SWITCHES.iter().any(|(n, _)| *n == name)
}

/// Keine Zeichen, die eine Shell auswertet oder die sich nicht sicher übergeben lassen: cmd wertet `% ! ^ & | < >` aus
/// (auch wenn PowerShell an eine `.cmd`-Datei weiterreicht), `"` lässt sich in cmd und Windows PowerShell 5.1 nicht
/// sicher übergeben; Steuerzeichen (auch Zeilenumbrüche) nie.
fn shell_safe(s: &str) -> bool {
    !s.chars().any(|c| c.is_control() || matches!(c, '"' | '%' | '!' | '^' | '&' | '|' | '<' | '>'))
}

/// Gemeinsame Regeln für Werte: nicht leer, ohne Rand-Leerzeichen, höchstens `max` Zeichen, kein führendes `-` (sähe
/// wie eine Option aus), Shell-sicher.
fn plain(s: &str, max: usize) -> bool {
    !s.is_empty() && s.chars().count() <= max && s.trim() == s && !s.starts_with('-') && shell_safe(s)
}

/// Absoluter lokaler Pfad (nur die Form): unter Windows `X:\…` bzw. `X:/…` – nie UNC (`\\server\…`, `//server/…`) oder
/// Gerätepfade (`\\?\…`); sonst `/…` (nicht `//…`).
fn absolute_local(p: &str) -> bool {
    let b = p.as_bytes();
    if cfg!(windows) {
        b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/')
    } else {
        b.first() == Some(&b'/') && !p.starts_with("//")
    }
}

/// Pfad für `--add-dir`/`--mcp-config` gültig (nur die Form, ohne Dateisystem; siehe [`check_path`])?
pub fn valid_path(p: &str) -> bool {
    plain(p, MAX_PATH_LEN) && absolute_local(p)
}

/// Linux-Pfad für eine WSL-Sitzung (nur unter Windows; sonst ist `/…` ohnehin ein lokaler Pfad, [`valid_path`]):
/// absolut (`/…`, nicht `//…`), ohne `\`, Shell-sicher. Lässt sich vom Windows-Programm aus nicht prüfen (das könnte
/// die WSL-VM starten); gilt nur in WSL-Sitzungen, sonst wird er beim Start mit Hinweis weggelassen.
pub fn wsl_linux_path(p: &str) -> bool {
    cfg!(windows) && p.starts_with('/') && !p.starts_with("//") && !p.contains('\\') && plain(p, MAX_PATH_LEN)
}

/// Eintrag einer Pfadliste (`--add-dir`, `--plugin-dir`, `--mcp-config`) gültig: lokaler Pfad oder Linux-Pfad für WSL.
pub fn valid_list_path(p: &str) -> bool {
    valid_path(p) || wsl_linux_path(p)
}

/// Eigener System-Prompt (`--system-prompt`) gültig? Wie [`valid_text`], aber mehrzeilig (nur `\n` als Steuerzeichen)
/// und bis [`MAX_SYSTEM_PROMPT_LEN`] Zeichen.
pub fn valid_system_prompt(s: &str) -> bool {
    !s.is_empty()
        && s.chars().count() <= MAX_SYSTEM_PROMPT_LEN
        && s.trim() == s
        && !s.starts_with('-')
        && !s.contains('\\')
        && s.split('\n').all(|l| shell_safe(l))
}

/// Zeilenumbrüche vereinheitlichen (`\r\n`/`\r` → `\n`) und den Rand trimmen (Eingabe im mehrzeiligen Feld).
pub fn normalize_multiline(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n").trim().to_string()
}

/// Mehrzeiligen Text als ein Argument für die Shell. Echte Zeilenumbrüche nur mit `ansi_c` – einer Shell, die sicher
/// bash ist (Git Bash): `$'z1\nz2'`. Sonst werden die Zeilen mit Leerzeichen verbunden: in WSL kann die Login-Shell
/// fish (Syntaxfehler) oder nushell sein (`$'…'` wertet dort `(…)` aus – ein Befehl liefe); ein Zeilenumbruch in einem
/// Argument beendet in cmd die Zeile, und `claude` ist unter Windows oft eine `.cmd`-Datei (npm), die über cmd läuft –
/// der Rest der Zeile (auch `--session-id`) ginge verloren. Einzeilig wie [`quote`]. Der Text ist geprüft
/// ([`valid_system_prompt`]: kein `\`, keine Shell-Zeichen).
fn quote_multiline(kind: Kind, s: &str, ansi_c: bool) -> String {
    if !s.contains('\n') {
        return quote(kind, s);
    }
    if kind == Kind::Posix && ansi_c {
        return format!("$'{}'", s.replace('\'', "\\'").replace('\n', "\\n"));
    }
    quote(kind, &s.split('\n').map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" "))
}

/// Grenze von cmd für eine Befehlszeile (`claude.cmd` läuft über cmd): länger startet Paneldeck Claude nicht.
pub const MAX_COMMAND_LEN: usize = 8191;
/// Ab dieser Länge der Befehlszeile warnt die Oberfläche.
pub const WARN_COMMAND_LEN: usize = 8000;

/// Vergleichsschlüssel eines Pfads gegen Doppelte: Windows-Pfade ohne Groß-/Kleinschreibung und ohne Trenner am Ende,
/// Linux-Pfade nur ohne `/` am Ende.
pub fn path_key(p: &str) -> String {
    let t = p.trim();
    let b = t.as_bytes();
    let windows = (b.len() >= 2 && b[1] == b':') || t.starts_with("\\\\");
    if windows {
        let s = t.replace('/', "\\").to_lowercase();
        let s = s.trim_end_matches('\\');
        if s.len() == 2 { format!("{s}\\") } else { s.to_string() }
    } else {
        let s = t.trim_end_matches('/');
        if s.is_empty() { "/".to_string() } else { s.to_string() }
    }
}

/// Werkzeugliste für `--allowed-tools`/`--disallowed-tools` gültig? Z. B. `Bash(git log:*) Read mcp__github__*`.
pub fn valid_tools(s: &str) -> bool {
    plain(s, MAX_TOOLS_LEN) && !s.ends_with('\\') && s.chars().all(|c| c.is_ascii_alphanumeric() || " _.,:*()/@-~=+[]?#\\".contains(c))
}

/// Zusatz zum System-Prompt gültig? Eine Zeile, Shell-sicher, ohne `\` (fish wertet `\` auch in einfachen Anführungszeichen
/// aus; in Windows-Argumenten schützt `\` ein folgendes Anführungszeichen).
pub fn valid_text(s: &str) -> bool {
    plain(s, MAX_TEXT_LEN) && !s.contains('\\')
}

/// Budget in USD gültig? Positive Zahl mit höchstens 6 Stellen vor und 2 nach dem Punkt.
pub fn valid_budget(s: &str) -> bool {
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    !int.is_empty()
        && int.len() <= 6
        && int.bytes().all(|c| c.is_ascii_digit())
        && frac.len() <= 2
        && frac.bytes().all(|c| c.is_ascii_digit())
        && (s.contains('.') == !frac.is_empty())
        && s.parse::<f64>().is_ok_and(|v| v > 0.0)
}

/// Wert für `--autocompact` gültig? `auto` oder eine Tokenzahl (1–9 Ziffern, ohne führende 0, optional mit `k`).
pub fn valid_autocompact(s: &str) -> bool {
    let n = s.strip_suffix('k').unwrap_or(s);
    s == "auto" || (!n.is_empty() && n.len() <= 9 && n.bytes().all(|c| c.is_ascii_digit()) && !n.starts_with('0'))
}

/// Warum ein Pfad (beim Start bzw. in der Oberfläche) nicht verwendet wird.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathIssue {
    /// Form ungültig (nicht absolut, Netzwerkform, unerlaubte Zeichen, zu lang).
    Invalid,
    /// Netzwerklaufwerk oder Verknüpfung auf eine Netzwerkfreigabe.
    Network,
    /// Existiert nicht (bzw. kein Ordner/keine Datei).
    Missing,
}

/// Pfad prüfen: Form, lokales Laufwerk, jede Verknüpfung (Symlink/Junction) im Pfad zeigt auf ein lokales Ziel (gelesen
/// mit `read_link`, bevor ihr gefolgt wird – eine Netzwerkfreigabe wird nie berührt), existiert als Ordner (`dir`) bzw.
/// Datei.
pub fn check_path(p: &str, dir: bool) -> Result<(), PathIssue> {
    if !valid_path(p) {
        return Err(PathIssue::Invalid);
    }
    if is_remote_drive(p) {
        return Err(PathIssue::Network);
    }
    local_chain(Path::new(p), 8)?;
    let md = std::fs::metadata(p).map_err(|_| PathIssue::Missing)?;
    if (dir && md.is_dir()) || (!dir && md.is_file()) { Ok(()) } else { Err(PathIssue::Missing) }
}

/// Jeden Teil des Pfads einzeln prüfen, ohne Verknüpfungen zu folgen (`symlink_metadata`). Eine Verknüpfung wird nur
/// gelesen (`read_link`); ihr Ziel muss lokal sein und wird ebenso geprüft (höchstens `depth` Ebenen), erst dann geht es
/// dort weiter.
fn local_chain(p: &Path, depth: u8) -> Result<(), PathIssue> {
    let mut cur = PathBuf::new();
    for comp in p.components() {
        cur.push(comp);
        if matches!(comp, Component::Prefix(_) | Component::RootDir) {
            continue;
        }
        let md = std::fs::symlink_metadata(&cur).map_err(|_| PathIssue::Missing)?;
        if !md.file_type().is_symlink() {
            continue;
        }
        let target = std::fs::read_link(&cur).map_err(|_| PathIssue::Missing)?;
        let target = if target.is_relative() { cur.parent().map(|d| d.join(&target)).unwrap_or(target) } else { target };
        let s = target.to_string_lossy().to_string();
        let local = s.strip_prefix(r"\\?\").or_else(|| s.strip_prefix(r"\??\")).unwrap_or(&s).to_string();
        let lower = local.to_ascii_lowercase();
        if crate::pathfmt::is_remote_unc(&s) || lower.starts_with("unc\\") || local.starts_with("\\\\") || local.starts_with("//") {
            return Err(PathIssue::Network);
        }
        if lower.starts_with("volume{") {
            // Laufwerk ohne Buchstaben (Bereitstellungspunkt): lokal; nicht weiter auflösbar.
            continue;
        }
        if depth == 0 || is_remote_drive(&local) {
            return Err(PathIssue::Network);
        }
        local_chain(Path::new(&local), depth - 1)?;
        cur = PathBuf::from(local);
    }
    Ok(())
}

/// Liegt der Pfad auf einem Netzwerk- oder unbekannten Laufwerk? Lokal sind feste, Wechsel-, CD- und RAM-Laufwerke.
/// Ohne Laufwerksbuchstaben (`platform::fs::DRIVE_LETTERS`) nie.
fn is_remote_drive(p: &str) -> bool {
    if !crate::platform::fs::DRIVE_LETTERS {
        return false;
    }
    let b = p.as_bytes();
    if b.len() < 2 || !b[0].is_ascii_alphabetic() || b[1] != b':' {
        return true;
    }
    // 2 = Wechsel-, 3 = festes, 5 = CD-, 6 = RAM-Laufwerk; 4 = Netzwerk, 0/1 = unbekannt/kein Laufwerk.
    !matches!(crate::platform::fs::drive_type(b[0] as char), Some(2 | 3 | 5 | 6))
}

/// Wie Pfade für die Shell geschrieben werden, in der Claude startet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathStyle {
    /// Unverändert (PowerShell, cmd; unter Linux/macOS jede Shell).
    Native,
    /// POSIX-Shell unter Windows (Git Bash, MSYS): Schrägstriche; Claude startet mit `MSYS2_ARG_CONV_EXCL='*'`.
    Slash,
    /// WSL: `/mnt/<lw>/…`.
    Wsl,
}

impl PathStyle {
    /// Art der Shell aus Kennung und Programm: WSL auch bei eigenen Profilen mit `wsl.exe` oder dem `bash.exe` aus
    /// System32; andere POSIX-Shells unter Windows (Git Bash) mit Schrägstrichen.
    pub fn for_shell(shell_id: &str, program: &str, kind: Kind) -> PathStyle {
        if cfg!(windows) && (shell_id.starts_with("wsl") || crate::shells::is_wsl_program(program)) {
            PathStyle::Wsl
        } else if cfg!(windows) && kind == Kind::Posix {
            PathStyle::Slash
        } else {
            PathStyle::Native
        }
    }

    /// Aufruf von Claude: in Git Bash ohne die Pfad-Umschreibung von MSYS (sonst würde aus `Edit(/src/**)` in
    /// `--allowed-tools` ein Windows-Pfad).
    pub fn claude_program(self) -> &'static str {
        match self {
            PathStyle::Slash => "MSYS2_ARG_CONV_EXCL='*' claude",
            _ => "claude",
        }
    }
}

/// Pfad für die Kommandozeile der Shell; Trenner am Ende fallen weg (ein `\` vor dem schließenden Anführungszeichen würde
/// es unter Windows schützen).
fn shell_path(style: PathStyle, p: &str) -> String {
    let p = match style {
        PathStyle::Wsl => crate::pathfmt::convert("wsl", p),
        PathStyle::Slash => p.replace('\\', "/"),
        PathStyle::Native => p.to_string(),
    };
    let t = p.trim_end_matches(['\\', '/']);
    if t.is_empty() {
        "/".to_string()
    } else if t.len() == 2 && t.ends_with(':') {
        format!("{t}/")
    } else {
        t.to_string()
    }
}

/// Weitere Optionen (#182, #289): Schalter und Wertoptionen. Leere Felder setzen nichts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExtraOpts {
    /// Schalter aus [`SWITCHES`] und [`USER_SWITCHES`] (Namen ohne `--`, z. B. `chrome`).
    pub flags: Vec<String>,
    /// `--add-dir`: zusätzliche Ordner (nur Nutzer, nie für Agenten-Sitzungen).
    pub add_dirs: Vec<String>,
    /// `--plugin-dir` (#289): Ordner mit Plugins, je Ordner ein `--plugin-dir` (nur Nutzer, nie für Agenten-Sitzungen).
    pub plugin_dirs: Vec<String>,
    /// `--allowed-tools` (nur Nutzer, nie für Agenten-Sitzungen).
    pub allowed_tools: String,
    /// `--disallowed-tools` (nur Nutzer; schränkt ein und gilt deshalb auch für Agenten-Sitzungen).
    pub disallowed_tools: String,
    /// `--mcp-config <Datei> …` (#289: Liste; nur Nutzer, nie für Agenten-Sitzungen). In JSON `mcp_configs`, dazu
    /// für ältere Versionen der erste Eintrag als `mcp_config` (beim Lesen zusammengeführt).
    pub mcp_configs: Vec<String>,
    /// `--strict-mcp-config` (#289): nur die MCP-Server aus `mcp_configs`; ohne MCP-Konfiguration wirkungslos.
    pub strict_mcp_config: bool,
    /// `--system-prompt <Text>` (#289): ersetzt den System-Prompt von Claude Code; mehrzeilig, nur Nutzer, nie für
    /// Agenten-Sitzungen.
    pub system_prompt: String,
    /// `--append-system-prompt <Text>` (nur Nutzer).
    pub append_system_prompt: String,
    /// `--fallback-model <Name>` (nur Nutzer).
    pub fallback_model: String,
    /// `--max-budget-usd <Zahl>` (nur Nutzer; als Text, wie eingegeben).
    pub max_budget_usd: String,
    /// `--autocompact <auto|Tokens>` (nur Nutzer).
    pub autocompact: String,
}

/// Kompakt (leere Felder fehlen), flach in [`ClaudeOpts`]. `mcp_config` (erster Eintrag) bleibt für ältere Versionen.
impl Serialize for ExtraOpts {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(None)?;
        for (k, v) in [("flags", &self.flags), ("add_dirs", &self.add_dirs), ("plugin_dirs", &self.plugin_dirs)] {
            if !v.is_empty() {
                m.serialize_entry(k, v)?;
            }
        }
        for (k, v) in [("allowed_tools", &self.allowed_tools), ("disallowed_tools", &self.disallowed_tools)] {
            if !v.is_empty() {
                m.serialize_entry(k, v)?;
            }
        }
        if let Some(first) = self.mcp_configs.first() {
            m.serialize_entry("mcp_config", first)?;
            m.serialize_entry("mcp_configs", &self.mcp_configs)?;
        }
        if self.strict_mcp_config {
            m.serialize_entry("strict_mcp_config", &true)?;
        }
        for (k, v) in [
            ("system_prompt", &self.system_prompt),
            ("append_system_prompt", &self.append_system_prompt),
            ("fallback_model", &self.fallback_model),
            ("max_budget_usd", &self.max_budget_usd),
            ("autocompact", &self.autocompact),
        ] {
            if !v.is_empty() {
                m.serialize_entry(k, v)?;
            }
        }
        m.end()
    }
}

/// Pfadliste bereinigen: getrimmt, nur gültige ([`valid_list_path`]), ohne Doppelte, höchstens [`MAX_DIRS`].
fn clean_paths(list: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for d in list.iter().map(|d| d.trim()) {
        if valid_list_path(d) && !out.iter().any(|x| path_key(x) == path_key(d)) && out.len() < MAX_DIRS {
            out.push(d.to_string());
        }
    }
    out
}

/// Ergebnis der Wertoptionen beim Start: Zeile, tatsächlich gesetzte Optionen und weggelassene Pfade.
struct ValueArgs {
    line: String,
    applied: ExtraOpts,
    dropped: Vec<String>,
}

impl ExtraOpts {
    pub fn is_empty(&self) -> bool {
        *self == ExtraOpts::default()
    }

    /// Nur gültige Werte (Form, ohne Dateisystem): Schalter in fester Reihenfolge ohne Doppelte (`chrome` zusammen mit
    /// `no-chrome` fällt ganz weg), Ordner ohne Doppelte und höchstens [`MAX_DIRS`], Ränder getrimmt.
    pub fn sanitized(&self) -> ExtraOpts {
        let mut flags: Vec<String> = SWITCHES.iter().chain(USER_SWITCHES.iter()).filter(|(n, _)| self.flags.iter().any(|f| f.trim() == *n)).map(|(n, _)| n.to_string()).collect();
        if flags.iter().any(|f| f == "chrome") && flags.iter().any(|f| f == "no-chrome") {
            flags.retain(|f| f != "chrome" && f != "no-chrome");
        }
        let keep = |s: &str, ok: fn(&str) -> bool| {
            let s = s.trim();
            if ok(s) { s.to_string() } else { String::new() }
        };
        let mcp_configs = clean_paths(&self.mcp_configs);
        let system_prompt = normalize_multiline(&self.system_prompt);
        ExtraOpts {
            flags,
            add_dirs: clean_paths(&self.add_dirs),
            plugin_dirs: clean_paths(&self.plugin_dirs),
            allowed_tools: keep(&self.allowed_tools, valid_tools),
            disallowed_tools: keep(&self.disallowed_tools, valid_tools),
            strict_mcp_config: self.strict_mcp_config && !mcp_configs.is_empty(),
            mcp_configs,
            system_prompt: if valid_system_prompt(&system_prompt) { system_prompt } else { String::new() },
            append_system_prompt: keep(&self.append_system_prompt, valid_text),
            fallback_model: keep(&self.fallback_model, valid_name),
            max_budget_usd: keep(&self.max_budget_usd, valid_budget),
            autocompact: keep(&self.autocompact, valid_autocompact),
        }
    }

    /// Ränder getrimmt, leere Ordnerzeilen weg, sonst unverändert (gleich [`ExtraOpts::sanitized`] = alles gültig).
    pub fn trimmed(&self) -> ExtraOpts {
        let t = |s: &str| s.trim().to_string();
        let list = |v: &Vec<String>| -> Vec<String> { v.iter().map(|d| t(d)).filter(|d| !d.is_empty()).collect() };
        let mcp_configs = list(&self.mcp_configs);
        ExtraOpts {
            flags: self.flags.iter().map(|f| t(f)).collect(),
            add_dirs: list(&self.add_dirs),
            plugin_dirs: list(&self.plugin_dirs),
            allowed_tools: t(&self.allowed_tools),
            disallowed_tools: t(&self.disallowed_tools),
            strict_mcp_config: self.strict_mcp_config && !mcp_configs.is_empty(),
            mcp_configs,
            system_prompt: normalize_multiline(&self.system_prompt),
            append_system_prompt: t(&self.append_system_prompt),
            fallback_model: t(&self.fallback_model),
            max_budget_usd: t(&self.max_budget_usd),
            autocompact: t(&self.autocompact),
        }
    }

    /// Je Feld: eigener Wert, sonst der aus `fallback`.
    pub fn or(&self, fallback: &ExtraOpts) -> ExtraOpts {
        let pick = |a: &str, b: &str| if a.is_empty() { b.to_string() } else { a.to_string() };
        let pick_v = |a: &Vec<String>, b: &Vec<String>| if a.is_empty() { b.clone() } else { a.clone() };
        // „Nur diese MCP-Server“ gehört zu den MCP-Konfigurationen, aus denen sie stammen.
        let (mcp_configs, strict_mcp_config) = if self.mcp_configs.is_empty() { (fallback.mcp_configs.clone(), fallback.strict_mcp_config) } else { (self.mcp_configs.clone(), self.strict_mcp_config) };
        ExtraOpts {
            flags: pick_v(&self.flags, &fallback.flags),
            add_dirs: pick_v(&self.add_dirs, &fallback.add_dirs),
            plugin_dirs: pick_v(&self.plugin_dirs, &fallback.plugin_dirs),
            allowed_tools: pick(&self.allowed_tools, &fallback.allowed_tools),
            disallowed_tools: pick(&self.disallowed_tools, &fallback.disallowed_tools),
            mcp_configs,
            strict_mcp_config,
            system_prompt: pick(&self.system_prompt, &fallback.system_prompt),
            append_system_prompt: pick(&self.append_system_prompt, &fallback.append_system_prompt),
            fallback_model: pick(&self.fallback_model, &fallback.fallback_model),
            max_budget_usd: pick(&self.max_budget_usd, &fallback.max_budget_usd),
            autocompact: pick(&self.autocompact, &fallback.autocompact),
        }
    }

    /// Ohne die Rechte erweiternden Wertoptionen ([`RISKY`]: Ordner, Plugin-Ordner, erlaubte Werkzeuge,
    /// MCP-Konfigurationen samt `--strict-mcp-config`), ohne eigenen System-Prompt und ohne die Schalter nur für den
    /// Nutzer ([`USER_SWITCHES`]) – für Sitzungen, die ein Agent anlegt.
    pub fn agent_safe(&self) -> ExtraOpts {
        ExtraOpts {
            flags: self.flags.iter().filter(|f| agent_switch(f)).cloned().collect(),
            add_dirs: Vec::new(),
            plugin_dirs: Vec::new(),
            allowed_tools: String::new(),
            mcp_configs: Vec::new(),
            strict_mcp_config: false,
            system_prompt: String::new(),
            ..self.clone()
        }
    }

    /// Namen der gesetzten Wertoptionen (für MCP-Auskünfte; die Werte bleiben beim Nutzer).
    pub fn value_names(&self) -> Vec<&'static str> {
        [
            ("add_dir", !self.add_dirs.is_empty()),
            ("plugin_dir", !self.plugin_dirs.is_empty()),
            ("allowed_tools", !self.allowed_tools.is_empty()),
            ("disallowed_tools", !self.disallowed_tools.is_empty()),
            ("mcp_config", !self.mcp_configs.is_empty()),
            ("strict_mcp_config", self.strict_mcp_config && !self.mcp_configs.is_empty()),
            ("system_prompt", !self.system_prompt.is_empty()),
            ("append_system_prompt", !self.append_system_prompt.is_empty()),
            ("fallback_model", !self.fallback_model.is_empty()),
            ("max_budget_usd", !self.max_budget_usd.is_empty()),
            ("autocompact", !self.autocompact.is_empty()),
        ]
        .into_iter()
        .filter(|(_, set)| *set)
        .map(|(n, _)| n)
        .collect()
    }

    /// Sind Rechte erweiternde Wertoptionen ([`RISKY`]) gesetzt?
    pub fn has_risky(&self) -> bool {
        self.value_names().iter().any(|n| RISKY.contains(n))
    }

    /// Gesetzte Schalter als Kommandozeilen-Teil (mit führendem Leerzeichen), z. B. ` --chrome --ide`.
    fn switch_args(&self) -> String {
        self.sanitized().flags.iter().filter_map(|f| switch_flag(f)).map(|f| format!(" {f}")).collect()
    }

    /// Wertoptionen als Kommandozeilen-Teil (mit führendem Leerzeichen), gequotet für `kind`. Pfade, die es (noch) nicht
    /// gibt oder die nicht lokal sind, fallen weg (geprüft bei jedem Start) und werden gemeldet.
    fn value_args(&self, kind: Kind, style: PathStyle, check_fs: bool) -> ValueArgs {
        let mut e = self.sanitized();
        let mut dropped = Vec::new();
        let mut line = String::new();
        // Lokale Pfade müssen existieren; Linux-Pfade gelten nur in WSL-Sitzungen (ungeprüft, siehe `wsl_linux_path`).
        // Ohne `check_fs` (Vorschau) nur die Form, nie ein Zugriff aufs Dateisystem.
        let mut usable = |list: &mut Vec<String>, dir: bool| {
            let (ok, missing): (Vec<String>, Vec<String>) =
                std::mem::take(list).into_iter().partition(|p| if wsl_linux_path(p) { style == PathStyle::Wsl } else { !check_fs || check_path(p, dir).is_ok() });
            dropped.extend(missing);
            *list = ok;
        };
        usable(&mut e.add_dirs, true);
        usable(&mut e.plugin_dirs, true);
        usable(&mut e.mcp_configs, false);
        let paths = |list: &[String]| list.iter().map(|d| quote(kind, &shell_path(style, d))).collect::<Vec<_>>();
        if !e.add_dirs.is_empty() {
            line += &format!(" --add-dir {}", paths(&e.add_dirs).join(" "));
        }
        // Je Plugin-Ordner ein eigenes `--plugin-dir` (wiederholbar).
        for p in paths(&e.plugin_dirs) {
            line += &format!(" --plugin-dir {p}");
        }
        for (flag, v) in [("--allowed-tools", &e.allowed_tools), ("--disallowed-tools", &e.disallowed_tools)] {
            if !v.is_empty() {
                line += &format!(" {flag} {}", quote(kind, v));
            }
        }
        // Mehrere Dateien durch Leerzeichen getrennt nach einem `--mcp-config` (wie in der Hilfe von Claude Code); die
        // nächste Option begrenzt die Liste.
        if !e.mcp_configs.is_empty() {
            line += &format!(" --mcp-config {}", paths(&e.mcp_configs).join(" "));
        }
        e.strict_mcp_config &= !e.mcp_configs.is_empty();
        if e.strict_mcp_config {
            line += " --strict-mcp-config";
        }
        if !e.system_prompt.is_empty() {
            line += &format!(" --system-prompt {}", quote_multiline(kind, &e.system_prompt, style == PathStyle::Slash));
        }
        if !e.append_system_prompt.is_empty() {
            line += &format!(" --append-system-prompt {}", quote(kind, &e.append_system_prompt));
        }
        for (flag, v) in [("--fallback-model", &e.fallback_model), ("--max-budget-usd", &e.max_budget_usd), ("--autocompact", &e.autocompact)] {
            if !v.is_empty() {
                line += &format!(" {flag} {}", quote_value(kind, v));
            }
        }
        ValueArgs { line, applied: e, dropped }
    }
}

/// Die Startoptionen; ein leeres Feld heißt „Standard von Claude Code“ (Flag wird nicht gesetzt).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ClaudeOpts {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub effort: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub permission_mode: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub agent: String,
    /// Weitere Schalter und Wertoptionen (#182), in JSON auf derselben Ebene (neue, optionale Felder).
    #[serde(flatten)]
    pub extra: ExtraOpts,
}

/// Fehler bei der Prüfung von MCP-Argumenten: (Deutsch, Englisch).
pub type OptError = (String, String);

/// Kommandozeilen-Teil eines Starts ([`ClaudeOpts::launch`]).
pub struct LaunchArgs {
    /// Mit führendem Leerzeichen; danach folgt höchstens noch eine Option von Paneldeck (`--resume`, `--continue`), die
    /// die mehrwertigen `--add-dir`/`--allowed-tools`/`--disallowed-tools` begrenzt – sonst endet die Zeile (#302).
    pub line: String,
    /// Tatsächlich gesetzte Optionen (ohne weggelassene Pfade) – für Auskünfte über MCP.
    pub applied: ClaudeOpts,
    /// Weggelassene Pfade (fehlen, nicht lokal) – Hinweis an den Nutzer.
    pub dropped: Vec<String>,
}

impl ClaudeOpts {
    pub fn is_empty(&self) -> bool {
        self.model.is_empty() && self.effort.is_empty() && self.permission_mode.is_empty() && self.agent.is_empty() && self.extra.is_empty()
    }

    /// Nur gültige Werte behalten (ungültige werden zu „Standard“); `manual` wird zu `default`.
    pub fn sanitized(&self, allow_bypass: bool) -> ClaudeOpts {
        let keep = |s: &str, ok: bool| if ok { s.to_string() } else { String::new() };
        let mode = if self.permission_mode == MANUAL { "default" } else { self.permission_mode.as_str() };
        ClaudeOpts {
            model: keep(&self.model, valid_name(&self.model)),
            effort: keep(&self.effort, valid_effort(&self.effort)),
            permission_mode: keep(mode, valid_permission_mode(mode, allow_bypass)),
            agent: keep(&self.agent, valid_name(&self.agent)),
            extra: self.extra.sanitized(),
        }
    }

    /// Ränder der Eingaben getrimmt (Vergleich mit [`ClaudeOpts::sanitized`] in der Oberfläche: gleich = alles gültig).
    pub fn trimmed(&self) -> ClaudeOpts {
        ClaudeOpts {
            model: self.model.trim().to_string(),
            effort: self.effort.trim().to_string(),
            permission_mode: self.permission_mode.trim().to_string(),
            agent: self.agent.trim().to_string(),
            extra: self.extra.trimmed(),
        }
    }

    /// Je Feld: eigener Wert, sonst der aus `fallback`.
    pub fn or(&self, fallback: &ClaudeOpts) -> ClaudeOpts {
        let pick = |a: &str, b: &str| if a.is_empty() { b.to_string() } else { a.to_string() };
        ClaudeOpts {
            model: pick(&self.model, &fallback.model),
            effort: pick(&self.effort, &fallback.effort),
            permission_mode: pick(&self.permission_mode, &fallback.permission_mode),
            agent: pick(&self.agent, &fallback.agent),
            extra: self.extra.or(&fallback.extra),
        }
    }

    /// Optionen einer über MCP (oder per `paneldeck --open` aus einem Hub-Terminal) angelegten Sitzung: Angaben des
    /// Agenten, sonst die Standards des Nutzers – nie `bypassPermissions`. Ohne Permission-Mode ausdrücklich `default`,
    /// damit nicht `permissions.defaultMode` aus `~/.claude/settings.json` gilt. Schalter: die des Nutzers und die des
    /// Agenten; `no-chrome` des Nutzers gewinnt gegen `chrome` des Agenten (MCP lehnt das vorher ab,
    /// [`ClaudeOpts::agent_conflict`]), sonst entscheidet der Agent. Die Rechte erweiternden Wertoptionen ([`RISKY`]) und
    /// die Schalter nur für den Nutzer nie, auch nicht aus den Standards.
    pub fn for_agent(&self, defaults: &ClaudeOpts) -> ClaudeOpts {
        let d = defaults.sanitized(false);
        let mut o = self.or(&d).sanitized(false);
        let chrome = |f: &String| f == "chrome" || f == "no-chrome";
        let user_off = d.extra.flags.iter().any(|f| f == "no-chrome");
        let own: Vec<String> = self.extra.flags.iter().filter(|f| !(user_off && f.as_str() == "chrome")).cloned().collect();
        let own_chrome = own.iter().any(chrome);
        let mut flags = own;
        flags.extend(d.extra.flags.iter().filter(|f| !(own_chrome && chrome(f))).cloned());
        o.extra.flags = flags;
        o.extra = o.extra.agent_safe().sanitized();
        if o.permission_mode.is_empty() {
            o.permission_mode = "default".into();
        }
        o
    }

    /// Widerspricht die Angabe eines Agenten einer Vorgabe des Nutzers? `chrome`, wenn der Nutzer `no-chrome` als
    /// Standard gesetzt hat.
    pub fn agent_conflict(&self, defaults: &ClaudeOpts) -> Result<(), OptError> {
        let off = defaults.sanitized(false).extra.flags.iter().any(|f| f == "no-chrome");
        if off && self.extra.flags.iter().any(|f| f == "chrome") {
            return Err((
                "claude_flags: chrome ist nicht erlaubt – der Nutzer hat Chrome als Standard ausgeschaltet (no-chrome).".into(),
                "claude_flags: chrome is not allowed – the user turned Chrome off by default (no-chrome).".into(),
            ));
        }
        Ok(())
    }

    /// Kommandozeilen-Teil ohne Wertoptionen (mit führendem Leerzeichen, leer ohne Optionen): Modell, Effort,
    /// Permission-Mode, Agent und Schalter. Ungültige Werte werden ausgelassen; gequotet wird für die Shell, in die die
    /// Zeile getippt wird.
    pub fn args(&self, kind: Kind) -> String {
        let o = self.sanitized(true);
        let mut out = String::new();
        for (flag, v) in [("--model", &o.model), ("--effort", &o.effort), ("--permission-mode", &o.permission_mode), ("--agent", &o.agent)] {
            if !v.is_empty() {
                out += &format!(" {flag} {}", quote_value(kind, v));
            }
        }
        out + &o.extra.switch_args()
    }

    /// Kommandozeilen-Teil für einen Start: [`ClaudeOpts::args`], die bereinigten freien Reste alter `claude_args`
    /// (`legacy_rest`) und zuletzt die Wertoptionen – so kann kein freier Rest von einem mehrwertigen `--add-dir`
    /// geschluckt werden; dahinter setzt Paneldeck höchstens noch `--resume`/`--continue`.
    pub fn launch(&self, kind: Kind, style: PathStyle, legacy_rest: &str) -> LaunchArgs {
        let o = self.sanitized(true);
        let mut line = o.args(kind);
        if !legacy_rest.trim().is_empty() {
            line += &format!(" {}", legacy_args(legacy_rest, kind));
        }
        let v = o.extra.value_args(kind, style, true);
        line += &v.line;
        LaunchArgs { line, applied: ClaudeOpts { extra: ExtraOpts { flags: o.extra.flags.clone(), ..v.applied }, ..o }, dropped: v.dropped }
    }

    /// Wie [`ClaudeOpts::launch`] für die Vorschau in der Oberfläche: ohne jeden Zugriff aufs Dateisystem (Pfade nur
    /// nach Form; fehlende würden erst beim Start weggelassen).
    pub fn preview(&self, kind: Kind, style: PathStyle, legacy_rest: &str) -> String {
        let o = self.sanitized(true);
        let mut line = o.args(kind);
        if !legacy_rest.trim().is_empty() {
            line += &format!(" {}", legacy_args(legacy_rest, kind));
        }
        line + &o.extra.value_args(kind, style, false).line
    }

    /// Prüft die optionalen Parameter `model`/`effort`/`permission_mode`/`agent`/`claude_flags` von `create_session`.
    /// Fehlende oder leere Werte bleiben leer; alles andere muss gültig sein (`bypassPermissions` nie). Wertoptionen
    /// (Parameter aus [`USER_ONLY_PARAMS`]) gibt es über MCP nicht.
    pub fn from_mcp(args: &Value) -> Result<ClaudeOpts, OptError> {
        user_only_param(args)?;
        let get = |k: &str| -> Result<String, OptError> {
            match args.get(k) {
                None | Some(Value::Null) => Ok(String::new()),
                Some(Value::String(s)) if k == "permission_mode" && s.trim() == MANUAL => Ok("default".to_string()),
                Some(Value::String(s)) => Ok(s.trim().to_string()),
                Some(_) => Err((format!("{k} muss ein String sein."), format!("{k} must be a string."))),
            }
        };
        let o = ClaudeOpts {
            model: get("model")?,
            effort: get("effort")?,
            permission_mode: get("permission_mode")?,
            agent: get("agent")?,
            extra: ExtraOpts { flags: flags_from_mcp(args.get("claude_flags"))?, ..Default::default() },
        };
        for (k, v) in [("model", &o.model), ("agent", &o.agent)] {
            if !v.is_empty() && !valid_name(v) {
                return Err((
                    format!("{k}: nur Buchstaben, Ziffern, . _ : - (beginnend mit Buchstabe oder Ziffer), höchstens {MAX_NAME} Zeichen."),
                    format!("{k}: only letters, digits, . _ : - (starting with a letter or digit), at most {MAX_NAME} characters."),
                ));
            }
        }
        if !o.effort.is_empty() && !valid_effort(&o.effort) {
            return Err((format!("effort: eins von {}.", EFFORTS.join(", ")), format!("effort: one of {}.", EFFORTS.join(", "))));
        }
        if o.permission_mode == BYPASS {
            return Err((
                "permission_mode: bypassPermissions ist über MCP nie erlaubt – nur der Nutzer kann es selbst in Paneldeck wählen.".to_string(),
                "permission_mode: bypassPermissions is never allowed via MCP – only the user can choose it in Paneldeck.".to_string(),
            ));
        }
        if !o.permission_mode.is_empty() && !valid_permission_mode(&o.permission_mode, false) {
            return Err((
                format!("permission_mode: eins von {}.", PERMISSION_MODES.join(", ")),
                format!("permission_mode: one of {}.", PERMISSION_MODES.join(", ")),
            ));
        }
        Ok(o)
    }

    /// Optionen aus einer alten `claude_args`-Zeile (vor 2.3.0: „--model x --permission-mode y“) herauslösen, auch
    /// Schalter aus [`SWITCHES`]/[`USER_SWITCHES`] und `--autocompact <Wert>`. Rückgabe: erkannte Optionen und die
    /// übrigen Token (unverändert, Reihenfolge erhalten).
    pub fn split_legacy(args: &str) -> (ClaudeOpts, String) {
        let mut o = ClaudeOpts::default();
        let mut rest = Vec::new();
        let toks: Vec<&str> = args.split_whitespace().collect();
        let mut i = 0;
        while i < toks.len() {
            if let Some(name) = toks[i].strip_prefix("--").filter(|n| switch_flag(n).is_some()) {
                if !o.extra.flags.iter().any(|f| f == name) {
                    o.extra.flags.push(name.to_string());
                }
                i += 1;
                continue;
            }
            // Nur Werte übernehmen, die auch hier gültig sind (z. B. bleibt „--model opus[1m]“ als freie Zeile erhalten).
            let (slot, ok): (Option<&mut String>, fn(&str) -> bool) = match toks[i] {
                "--model" => (Some(&mut o.model), valid_name),
                "--effort" => (Some(&mut o.effort), valid_effort),
                "--permission-mode" => (Some(&mut o.permission_mode), |s| s == MANUAL || valid_permission_mode(s, true)),
                "--agent" => (Some(&mut o.agent), valid_name),
                "--autocompact" => (Some(&mut o.extra.autocompact), valid_autocompact),
                _ => (None, |_| false),
            };
            match (slot, toks.get(i + 1)) {
                (Some(slot), Some(v)) if ok(v) && slot.is_empty() => {
                    *slot = v.to_string();
                    i += 2;
                }
                _ => {
                    rest.push(toks[i]);
                    i += 1;
                }
            }
        }
        (o.sanitized(true), rest.join(" "))
    }

    /// Als `claude_args`-Zeile (für Vorlagen, deren Dateiformat nur diese Zeile kennt): Modell, Effort, Permission-Mode,
    /// Agent und Schalter – Wertoptionen nicht (Pfade und Texte gehören nicht in eine weitergebbare Vorlage).
    pub fn to_legacy(&self) -> String {
        self.args(Kind::Cmd).trim().to_string()
    }

    /// Antwortfelder für MCP (`null`, wenn Paneldeck die Option nicht setzt): dazu `claude_flags` (gesetzte Schalter)
    /// und `claude_user_options` (Namen der vom Nutzer gesetzten Wertoptionen, ohne Werte).
    pub fn to_json(&self) -> Value {
        let v = |s: &str| if s.is_empty() { Value::Null } else { Value::String(s.to_string()) };
        let e = self.extra.sanitized();
        serde_json::json!({
            "model": v(&self.model),
            "effort": v(&self.effort),
            "permission_mode": v(&self.permission_mode),
            "agent": v(&self.agent),
            "claude_flags": e.flags,
            "claude_user_options": e.value_names(),
        })
    }

    /// Kurzzeile für Tooltips, z. B. „opus · high · plan · Agent reviewer · --chrome --ide · add_dir“ (leer ohne Optionen).
    pub fn summary(&self, agent_label: &str) -> String {
        let mut parts: Vec<String> = [&self.model, &self.effort, &self.permission_mode].iter().filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
        if !self.agent.is_empty() {
            parts.push(format!("{agent_label} {}", self.agent));
        }
        let e = self.extra.sanitized();
        let switches = e.switch_args();
        if !switches.is_empty() {
            parts.push(switches.trim().to_string());
        }
        let names = e.value_names();
        if !names.is_empty() {
            parts.push(names.join(", "));
        }
        parts.join(" · ")
    }
}

/// Ein Parameter aus [`USER_ONLY_PARAMS`] angegeben (nicht `null`)? Dann `bad_args`: diese Optionen setzt nur der Nutzer.
pub fn user_only_param(args: &Value) -> Result<(), OptError> {
    match USER_ONLY_PARAMS.iter().find(|k| args.get(**k).is_some_and(|v| !v.is_null())) {
        Some(k) => Err((
            format!("{k}: über MCP nicht erlaubt – nur der Nutzer kann diese Option in Paneldeck setzen (Agenten nur die Schalter in claude_flags)."),
            format!("{k}: not allowed via MCP – only the user can set this option in Paneldeck (agents only the switches in claude_flags)."),
        )),
        None => Ok(()),
    }
}

/// `claude_flags` von `create_session`: Liste von Namen aus [`SWITCHES`] (führende `-` sind erlaubt, Groß-/
/// Kleinschreibung egal, Doppelte zählen einmal). Alles andere ist `bad_args` – Flags aus [`NEVER_VIA_MCP`] und
/// `--dangerously-*` mit eigener Meldung; `chrome` und `no-chrome` zusammen ebenfalls.
fn flags_from_mcp(v: Option<&Value>) -> Result<Vec<String>, OptError> {
    let list = match v {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(a)) => a,
        Some(_) => return Err(("claude_flags muss eine Liste von Strings sein, z. B. [\"chrome\", \"ide\"].".into(), "claude_flags must be an array of strings, e.g. [\"chrome\", \"ide\"].".into())),
    };
    if list.len() > MAX_MCP_FLAGS {
        return Err((format!("claude_flags: höchstens {MAX_MCP_FLAGS} Einträge."), format!("claude_flags: at most {MAX_MCP_FLAGS} entries.")));
    }
    let allowed = SWITCHES.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ");
    let mut out: Vec<String> = Vec::new();
    for x in list {
        let Some(raw) = x.as_str() else {
            return Err(("claude_flags: nur Strings.".into(), "claude_flags: strings only.".into()));
        };
        let name = raw.trim().trim_start_matches('-').to_ascii_lowercase();
        let shown: String = raw.chars().filter(|c| !c.is_control()).take(60).collect();
        if agent_switch(&name) {
            if !out.contains(&name) {
                out.push(name);
            }
            continue;
        }
        let base = name.split(['=', ' ']).next().unwrap_or("");
        if NEVER_VIA_MCP.contains(&base) || name.contains("dangerously") {
            return Err((
                format!("claude_flags: „{shown}“ ist über MCP nie erlaubt (Rechte/Zugriffe, Hooks oder von Paneldeck gesteuert). Erlaubt: {allowed}."),
                format!("claude_flags: “{shown}” is never allowed via MCP (rights/access, hooks or controlled by Paneldeck). Allowed: {allowed}."),
            ));
        }
        return Err((format!("claude_flags: unbekannter Schalter „{shown}“. Erlaubt: {allowed}."), format!("claude_flags: unknown switch “{shown}”. Allowed: {allowed}.")));
    }
    if out.iter().any(|f| f == "chrome") && out.iter().any(|f| f == "no-chrome") {
        return Err(("claude_flags: chrome und no-chrome schließen sich aus.".into(), "claude_flags: chrome and no-chrome are mutually exclusive.".into()));
    }
    Ok(out)
}

/// Freie Reste alter `claude_args` (nach [`clean_legacy`]) für die Zeile: Flags wie sie sind (nur `-`, Buchstaben,
/// Ziffern), Werte gequotet wie die Startoptionen, damit `@`/`[]` in PowerShell bzw. POSIX-Shells nichts auslösen.
pub fn legacy_args(rest: &str, kind: Kind) -> String {
    rest.split_whitespace().map(|t| if t.starts_with('-') { t.to_string() } else { quote_value(kind, t) }).collect::<Vec<_>>().join(" ")
}

/// Alte `claude_args` bereinigen: nur harmlose Token ([`crate::quote::sanitize_claude_args`]), nie ein
/// `--dangerously-*`/`--allow-dangerously-*`-Flag (würde Freigaben abschalten) und keine Flags, die Paneldeck selbst
/// steuert oder die nur über MCP verboten sind ([`NEVER_VIA_MCP`] außer `--safe-mode`/`--autocompact`, die als
/// Optionen übernommen werden) – samt ihrem Wert (#245: ein gespeichertes `--resume <id>`, `--continue` oder
/// `--session-id` setzte sonst ungefragt eine Konversation fort oder brach den Start ab).
#[cfg(test)]
pub fn clean_legacy(args: &str) -> String {
    clean_legacy_report(args, false).line
}

/// Ergebnis von [`clean_legacy_report`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LegacyClean {
    /// Bereinigte Zeile (wie [`clean_legacy`]).
    pub line: String,
    /// Aus der Zeile übernommene Wertoptionen (geprüft wie in der Oberfläche).
    pub migrated: ExtraOpts,
    /// Weggelassene Flags (mit `--`, ohne Werte, jedes einmal), z. B. `--resume`.
    pub dropped: Vec<String>,
}

/// Flags aus [`NEVER_VIA_MCP`], die in alten Zeilen mehrere Werte nehmen (bis zum nächsten Flag).
fn legacy_many(name: &str) -> bool {
    matches!(name, "add-dir" | "allowed-tools" | "allowedtools" | "disallowed-tools" | "disallowedtools" | "tools" | "mcp-config" | "plugin-dir")
}

/// Wertoption einer alten Zeile in `e` übernehmen, wenn [`ExtraOpts`] sie kennt und der Wert gültig ist (dieselben
/// Prüfungen wie in der Oberfläche). Rechte erweiternde ([`RISKY`]) nur mit `allow_risky`.
fn migrate_legacy_value(e: &mut ExtraOpts, name: &str, vals: &[&str], allow_risky: bool) -> bool {
    let one = || if vals.len() == 1 { Some(vals[0]) } else { None };
    let set = |slot: &mut String, v: Option<&str>, ok: fn(&str) -> bool| match v {
        Some(v) if ok(v) && slot.is_empty() => {
            *slot = v.to_string();
            true
        }
        _ => false,
    };
    let joined = vals.join(" ");
    let tools = Some(joined.as_str()).filter(|s| !s.is_empty());
    match name {
        "add-dir" if allow_risky => {
            if vals.is_empty() || vals.len() > MAX_DIRS || !e.add_dirs.is_empty() || !vals.iter().all(|v| valid_path(v)) {
                return false;
            }
            for v in vals {
                if !e.add_dirs.iter().any(|d| d == v) {
                    e.add_dirs.push(v.to_string());
                }
            }
            true
        }
        "allowed-tools" | "allowedtools" if allow_risky => set(&mut e.allowed_tools, tools, valid_tools),
        "mcp-config" if allow_risky => {
            if vals.is_empty() || vals.len() > MAX_DIRS || !e.mcp_configs.is_empty() || !vals.iter().all(|v| valid_path(v)) {
                return false;
            }
            e.mcp_configs = clean_paths(&vals.iter().map(|v| v.to_string()).collect::<Vec<_>>());
            true
        }
        "plugin-dir" if allow_risky => {
            if vals.len() != 1 || !valid_path(vals[0]) || e.plugin_dirs.len() >= MAX_DIRS {
                return false;
            }
            if !e.plugin_dirs.iter().any(|d| d == vals[0]) {
                e.plugin_dirs.push(vals[0].to_string());
            }
            true
        }
        "strict-mcp-config" if allow_risky => {
            e.strict_mcp_config = vals.is_empty();
            vals.is_empty()
        }
        "disallowed-tools" | "disallowedtools" => set(&mut e.disallowed_tools, tools, valid_tools),
        "append-system-prompt" => set(&mut e.append_system_prompt, one(), valid_text),
        "fallback-model" => set(&mut e.fallback_model, one(), valid_name),
        "max-budget-usd" => set(&mut e.max_budget_usd, one(), valid_budget),
        _ => false,
    }
}

/// Wie [`clean_legacy`], dazu: Wertoptionen, die [`ExtraOpts`] kennt, werden übernommen statt weggeworfen
/// (`--add-dir`, `--allowed-tools`, `--mcp-config` nur mit `allow_risky`: aus der eigenen Sitzung, nie aus einer
/// weitergebbaren Vorlage), alles andere Weggelassene wird aufgezählt (für einen Hinweis an den Nutzer).
pub fn clean_legacy_report(args: &str, allow_risky: bool) -> LegacyClean {
    let clean = crate::quote::sanitize_claude_args(args);
    let toks: Vec<&str> = clean.split_whitespace().collect();
    let mut r = LegacyClean::default();
    let mut out: Vec<&str> = Vec::new();
    let note = |r: &mut LegacyClean, f: String| {
        if !r.dropped.contains(&f) {
            r.dropped.push(f);
        }
    };
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        i += 1;
        let lower = t.to_ascii_lowercase();
        if lower.contains("dangerously") {
            note(&mut r, lower);
            continue;
        }
        let name = lower.trim_start_matches('-');
        if !(t.starts_with('-') && NEVER_VIA_MCP.contains(&name) && !matches!(name, "safe-mode" | "autocompact")) {
            out.push(t);
            continue;
        }
        let mut vals: Vec<&str> = Vec::new();
        while i < toks.len() && !toks[i].starts_with('-') && name != "strict-mcp-config" && (vals.is_empty() || legacy_many(name)) {
            vals.push(toks[i]);
            i += 1;
        }
        if !migrate_legacy_value(&mut r.migrated, name, &vals, allow_risky) {
            note(&mut r, if name.len() == 1 { format!("-{name}") } else { format!("--{name}") });
        }
    }
    r.line = out.join(" ");
    r
}

/// Optionen einer neuen Sitzung (alle Wege laufen hier durch): `explicit` (Dialog, MCP, Vorlage, Duplikat) oder sonst
/// die Standards des Nutzers, ergänzt um Optionen aus alten `claude_args`. `bypassPermissions` bleibt nur mit
/// `allow_bypass` (ausdrückliche Wahl im Dialog „Neue Sitzung“). Rückgabe: Optionen und bereinigte freie Reste.
#[cfg(test)]
pub fn resolve_new(explicit: Option<&ClaudeOpts>, allow_bypass: bool, defaults: &ClaudeOpts, legacy_args: &str) -> (ClaudeOpts, String) {
    let (o, rest, _) = resolve_new_report(explicit, allow_bypass, defaults, legacy_args);
    (o, rest)
}

/// Wie [`resolve_new`], dazu die aus der alten Zeile weggelassenen Flags ([`clean_legacy_report`], ohne die Rechte
/// erweiternden Wertoptionen: die Zeile kann aus einer weitergegebenen Vorlage stammen).
pub fn resolve_new_report(explicit: Option<&ClaudeOpts>, allow_bypass: bool, defaults: &ClaudeOpts, legacy_args: &str) -> (ClaudeOpts, String, Vec<String>) {
    let c = clean_legacy_report(legacy_args, false);
    let (mut legacy, rest) = ClaudeOpts::split_legacy(&c.line);
    legacy.extra = legacy.extra.or(&c.migrated);
    // Ausdrückliche Angabe vor alter Zeile; eine alte Zeile (Vorlage) vor den Standards des Nutzers.
    let o = match explicit {
        Some(e) => e.or(&legacy),
        None => legacy.or(&defaults.sanitized(false)),
    };
    (o.sanitized(allow_bypass), rest, c.dropped)
}

/// Gespeicherte Standards/letzte Wahl in `settings.json`: nur gültige Werte, nie `bypassPermissions`.
pub fn ser_user_opts<S: serde::Serializer>(o: &ClaudeOpts, s: S) -> Result<S::Ok, S::Error> {
    o.sanitized(false).serialize(s)
}

/// Wie [`ser_user_opts`] für `Option`.
pub fn ser_user_opts_opt<S: serde::Serializer>(o: &Option<ClaudeOpts>, s: S) -> Result<S::Ok, S::Error> {
    o.as_ref().map(|o| o.sanitized(false)).serialize(s)
}

/// Werte sind geprüft, PowerShell wertet aber zahlähnliche Wörter (`1e5`, `0x10`) als Zahl aus: dort immer in
/// einfache Anführungszeichen (wörtlich). POSIX-Shells ebenfalls; cmd braucht für diesen Zeichenvorrat keine.
fn quote_value(kind: Kind, v: &str) -> String {
    match kind {
        Kind::Cmd => v.to_string(),
        Kind::Pwsh | Kind::Posix => quote(kind, v),
    }
}

/// Tolerant lesen: fehlende, falsch getypte oder ungültige Felder ergeben „Standard“, nie einen Ladefehler.
/// `bypassPermissions` bleibt erhalten (nur der Nutzer selbst kann es gesetzt haben – Agenten nie, siehe `from_mcp`).
impl<'de> Deserialize<'de> for ClaudeOpts {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
        let list = |k: &str| -> Vec<String> { v.get(k).and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_str()).map(|x| x.trim().to_string()).collect()).unwrap_or_default() };
        // Zahlen auch als Zahl annehmen (von Hand bearbeitete settings.json).
        let num = |k: &str| match v.get(k) {
            Some(Value::Number(n)) => n.to_string(),
            _ => s(k),
        };
        let extra = ExtraOpts {
            flags: list("flags"),
            add_dirs: list("add_dirs"),
            plugin_dirs: list("plugin_dirs"),
            allowed_tools: s("allowed_tools"),
            disallowed_tools: s("disallowed_tools"),
            // Liste (#289); der einzelne Wert älterer Versionen nur, wenn die Liste fehlt (sonst ist er nur ihr gespiegelter
            // erster Eintrag – ein von Hand entfernter Eintrag käme sonst zurück).
            mcp_configs: if v.get("mcp_configs").is_some() {
                list("mcp_configs")
            } else {
                Some(s("mcp_config")).filter(|x| !x.is_empty()).into_iter().collect()
            },
            strict_mcp_config: v.get("strict_mcp_config").and_then(|x| x.as_bool()).unwrap_or(false),
            system_prompt: v.get("system_prompt").and_then(|x| x.as_str()).map(normalize_multiline).unwrap_or_default(),
            append_system_prompt: s("append_system_prompt"),
            fallback_model: s("fallback_model"),
            max_budget_usd: num("max_budget_usd"),
            autocompact: num("autocompact"),
        };
        Ok(ClaudeOpts { model: s("model"), effort: s("effort"), permission_mode: s("permission_mode"), agent: s("agent"), extra }.sanitized(true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Wie ein Flag Werte nimmt.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Arity {
        /// Schalter ohne Wert.
        None,
        /// Genau ein Wert (Pflicht).
        One,
        /// Ein oder mehrere Werte (mehrwertig, wird von der nächsten Option begrenzt).
        Many,
    }

    /// Wirkung eines Flags, soweit sie für Paneldeck zählt.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Effect {
        /// Kein Einfluss auf Rechte, Zugriffe oder Hooks.
        Harmless,
        /// Schränkt nur ein.
        Restricts,
        /// Erweitert Rechte oder Zugriffe.
        Widens,
        /// Schaltet Hooks ab (Paneldecks Status-/Freigabe-Erkennung fällt aus).
        DisablesHooks,
    }

    /// Geprüfte Tabelle der Flags, die Paneldeck setzen kann (Stand der Hilfe von Claude Code laut #182 und Review;
    /// `claude --help` wird nie ausgeführt). Neue Flags erst hier eintragen, dann in die Listen übernehmen.
    const FLAG_TABLE: [(&str, Arity, Effect); 20] = [
        ("--chrome", Arity::None, Effect::Harmless),
        ("--no-chrome", Arity::None, Effect::Restricts),
        ("--ide", Arity::None, Effect::Harmless),
        ("--verbose", Arity::None, Effect::Harmless),
        ("--brief", Arity::None, Effect::Harmless),
        ("--restricted", Arity::None, Effect::Restricts),
        ("--disable-slash-commands", Arity::None, Effect::Restricts),
        ("--safe-mode", Arity::None, Effect::DisablesHooks),
        ("--autocompact", Arity::One, Effect::Harmless),
        ("--add-dir", Arity::Many, Effect::Widens),
        ("--allowed-tools", Arity::Many, Effect::Widens),
        ("--disallowed-tools", Arity::Many, Effect::Restricts),
        ("--mcp-config", Arity::Many, Effect::Widens),
        ("--plugin-dir", Arity::One, Effect::Widens),
        ("--strict-mcp-config", Arity::None, Effect::Restricts),
        ("--system-prompt", Arity::One, Effect::Harmless),
        ("--append-system-prompt", Arity::One, Effect::Harmless),
        ("--fallback-model", Arity::One, Effect::Harmless),
        ("--max-budget-usd", Arity::One, Effect::Harmless),
        ("--dangerously-skip-permissions", Arity::None, Effect::Widens),
    ];

    fn table(flag: &str) -> (Arity, Effect) {
        FLAG_TABLE.iter().find(|(f, _, _)| *f == flag).map(|(_, a, e)| (*a, *e)).unwrap_or_else(|| panic!("{flag} fehlt in FLAG_TABLE"))
    }

    fn four(model: &str, effort: &str, permission_mode: &str, agent: &str) -> ClaudeOpts {
        ClaudeOpts { model: model.into(), effort: effort.into(), permission_mode: permission_mode.into(), agent: agent.into(), ..Default::default() }
    }

    fn with_flags(flags: &[&str]) -> ClaudeOpts {
        ClaudeOpts { extra: ExtraOpts { flags: flags.iter().map(|s| s.to_string()).collect(), ..Default::default() }, ..Default::default() }
    }

    impl ClaudeOpts {
        /// Kommandozeilen-Teil ohne freie Reste (für die Tests).
        fn args_for(&self, kind: Kind, style: PathStyle) -> String {
            self.launch(kind, style, "").line
        }
    }

    /// Ein vorhandener lokaler Ordner und eine Datei darin (eigener Temp-Ordner je Test).
    fn temp_dir(tag: &str) -> (std::path::PathBuf, String, String) {
        let d = std::env::temp_dir().join(format!("pd-flags-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("mcp.json");
        std::fs::write(&f, "{}").unwrap();
        (d.clone(), d.to_string_lossy().to_string(), f.to_string_lossy().to_string())
    }

    #[test]
    fn switch_table() {
        // Jeder Schalter (Agent und Nutzer) ist laut Tabelle wertlos; Agenten-Schalter berühren weder Rechte noch Hooks.
        for (n, f) in SWITCHES {
            assert_eq!(f, format!("--{n}"));
            let (arity, effect) = table(f);
            assert_eq!(arity, Arity::None, "{f} nimmt einen Wert");
            assert!(matches!(effect, Effect::Harmless | Effect::Restricts), "{f}: {effect:?}");
            assert!(!NEVER_VIA_MCP.contains(&n), "{n}");
        }
        for (n, f) in USER_SWITCHES {
            assert_eq!(f, format!("--{n}"));
            assert_eq!(table(f).0, Arity::None, "{f}");
            assert!(NEVER_VIA_MCP.contains(&n), "{n}: nie über MCP");
            assert!(!agent_switch(n));
        }
        assert_eq!(table("--safe-mode").1, Effect::DisablesHooks);
        // Wertoptionen nehmen laut Tabelle Werte; was Rechte erweitert, steht in RISKY.
        for f in ["--add-dir", "--plugin-dir", "--allowed-tools", "--disallowed-tools", "--mcp-config", "--system-prompt", "--append-system-prompt", "--fallback-model", "--max-budget-usd", "--autocompact"] {
            assert_ne!(table(f).0, Arity::None, "{f}");
            let name = f.trim_start_matches("--").replace('-', "_");
            assert_eq!(table(f).1 == Effect::Widens, RISKY.contains(&name.as_str()), "{f}");
            assert!(switch_flag(f.trim_start_matches("--")).is_none(), "{f} ist kein Schalter");
        }
        // Jedes Flag, das in einer Zeile auftauchen kann, steht in der Tabelle (mit Werten nur als Wertoption).
        let (d, dir, file) = temp_dir("table");
        let all = ClaudeOpts {
            extra: ExtraOpts {
                flags: SWITCHES.iter().chain(USER_SWITCHES.iter()).map(|(n, _)| n.to_string()).filter(|n| n != "no-chrome").collect(),
                add_dirs: vec![dir.clone()],
                plugin_dirs: vec![dir],
                allowed_tools: "Read".into(),
                disallowed_tools: "WebFetch".into(),
                mcp_configs: vec![file],
                strict_mcp_config: true,
                system_prompt: "sys".into(),
                append_system_prompt: "x".into(),
                fallback_model: "sonnet".into(),
                max_budget_usd: "1".into(),
                autocompact: "auto".into(),
            },
            ..Default::default()
        };
        let line = all.args_for(Kind::Cmd, PathStyle::Native);
        let toks: Vec<&str> = line.split_whitespace().collect();
        for (i, t) in toks.iter().enumerate().filter(|(_, t)| t.starts_with("--")) {
            let (arity, _) = table(t);
            let next_is_value = toks.get(i + 1).is_some_and(|n| !n.starts_with("--"));
            assert_eq!(arity != Arity::None, next_is_value, "{t} in {line}");
        }
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn names_are_validated() {
        for ok in ["opus", "fable", "claude-opus-4-5", "claude-sonnet-4-5-20250929", "us.anthropic.claude:v1", "my_agent", "a"] {
            assert!(valid_name(ok), "{ok}");
        }
        let long = "a".repeat(MAX_NAME + 1);
        for bad in ["", "-x", ".hidden", "_x", "a b", "a;b", "opus[1m]", "$(id)", "a`b", "a'b", "a\"b", "a%PATH%", "a&b", "a|b", "ä", "a/b", long.as_str()] {
            assert!(!valid_name(bad), "{bad}");
        }
        assert!(valid_name(&"a".repeat(MAX_NAME)));
    }

    #[test]
    fn manual_maps_to_default() {
        assert_eq!(ClaudeOpts::from_mcp(&json!({"permission_mode": "manual"})).unwrap().permission_mode, "default");
        assert_eq!(ClaudeOpts::from_mcp(&json!({"permission_mode": "default"})).unwrap().permission_mode, "default");
        let o: ClaudeOpts = serde_json::from_value(json!({"permission_mode": "manual"})).unwrap();
        assert_eq!(o.permission_mode, "default");
        assert_eq!(ClaudeOpts::split_legacy("--permission-mode manual").0.permission_mode, "default");
        assert_eq!(ClaudeOpts { permission_mode: "manual".into(), ..Default::default() }.args(Kind::Cmd), " --permission-mode default");
    }

    #[test]
    fn agent_sessions_get_explicit_default_mode() {
        assert_eq!(ClaudeOpts::default().for_agent(&ClaudeOpts::default()).permission_mode, "default");
        let d = ClaudeOpts { permission_mode: "plan".into(), ..Default::default() };
        assert_eq!(ClaudeOpts::default().for_agent(&d).permission_mode, "plan", "Standard des Nutzers ohne bypass gilt");
        assert_eq!(ClaudeOpts { permission_mode: "auto".into(), ..Default::default() }.for_agent(&d).permission_mode, "auto");
    }

    #[test]
    fn bypass_only_from_the_explicit_dialog_choice() {
        let bypass = ClaudeOpts { model: "opus".into(), permission_mode: BYPASS.into(), ..Default::default() };
        // Dialog „Neue Sitzung“, ausdrücklich gewählt: bleibt.
        assert_eq!(resolve_new(Some(&bypass), true, &ClaudeOpts::default(), "").0.permission_mode, BYPASS);
        // Standard der Einstellungen (Kontextmenü, --claude, IPC open, Verlauf, Ticket-Schnellstart, alte Vorlagen): nie.
        let (o, _) = resolve_new(None, false, &bypass, "");
        assert_eq!((o.model.as_str(), o.permission_mode.as_str()), ("opus", ""));
        // MCP, Vorlage, Duplikat: ausdrückliche Angabe, aber ohne Erlaubnis.
        assert_eq!(resolve_new(Some(&bypass), false, &ClaudeOpts::default(), "").0.permission_mode, "");
        // Alte claude_args (Vorlage, Duplikat einer alten Sitzung): weder bypass noch --dangerously-*.
        let (o, rest) = resolve_new(None, false, &ClaudeOpts::default(), "--permission-mode bypassPermissions --dangerously-skip-permissions --allow-dangerously-skip-permissions --verbose --x");
        assert_eq!((o.permission_mode.as_str(), rest.as_str()), ("", "--x"));
        assert_eq!(o.extra.flags, ["verbose"], "Schalter aus alten claude_args werden erkannt");
        // Alte Vorlage: ihre Optionen vor den Standards, fehlende aus den Standards; ohne Optionen nur die Standards.
        let d = ClaudeOpts { model: "sonnet".into(), effort: "low".into(), ..Default::default() };
        let (o, rest) = resolve_new(None, false, &d, "--model opus");
        assert_eq!((o.model.as_str(), o.effort.as_str(), rest.as_str()), ("opus", "low", ""));
        assert_eq!(resolve_new(None, false, &d, "").0, d);
        // Duplikat: Optionen der Quelle vor ihrer alten Zeile.
        let src = ClaudeOpts { model: "haiku".into(), ..Default::default() };
        assert_eq!(resolve_new(Some(&src), false, &d, "--model opus --effort max").0, ClaudeOpts { model: "haiku".into(), effort: "max".into(), ..Default::default() });
        // Agenten-Weg
        assert_eq!(resolve_new(Some(&ClaudeOpts::default().for_agent(&bypass)), false, &bypass, "").0.permission_mode, "default");
    }

    #[test]
    fn user_opts_never_persist_bypass_or_invalid_names() {
        let o = four("opus; calc", "high", BYPASS, "ok");
        let mut buf = Vec::new();
        ser_user_opts(&o, &mut serde_json::Serializer::new(&mut buf)).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), r#"{"effort":"high","agent":"ok"}"#);
    }

    #[test]
    fn legacy_leftovers_are_quoted_per_shell() {
        assert_eq!(legacy_args("--model opus[1m] --x @a", Kind::Pwsh), "--model 'opus[1m]' --x '@a'");
        assert_eq!(legacy_args("--model opus[1m]", Kind::Posix), "--model 'opus[1m]'");
        assert_eq!(legacy_args("--model opus[1m]", Kind::Cmd), "--model opus[1m]");
        assert_eq!(clean_legacy("--dangerously-skip-permissions --model x;y --ok"), "--model --ok");
    }

    /// #245: gespeicherte `claude_args` (state.json, templates.json, Duplikat) setzen nie selbst eine Konversation fort.
    #[test]
    fn legacy_args_never_resume_or_continue() {
        let id = "5d28a95e-dbaf-4f27-b2f3-6c0c42fab338";
        assert_eq!(clean_legacy(&format!("--model opus --resume {id} --continue --verbose")), "--model opus --verbose");
        assert_eq!(clean_legacy(&format!("-r {id} -c --session-id {id} --fork-session -p hi --print --settings x.json --ide")), "--ide");
        assert_eq!(clean_legacy("--add-dir /tmp --mcp-config m.json --output-format json --chrome"), "--chrome");
        // Als Optionen übernommen, nicht weggeworfen
        assert_eq!(clean_legacy("--safe-mode --autocompact auto"), "--safe-mode --autocompact auto");
        for line in [format!("--resume {id}"), "--continue".to_string(), format!("--session-id {id}"), format!("-r {id}")] {
            let (o, r) = resolve_new(None, false, &ClaudeOpts::default(), &line);
            let l = o.launch(Kind::Posix, PathStyle::Native, &r);
            assert!(!l.line.contains("--resume") && !l.line.contains("--continue") && !l.line.contains("--session-id") && !l.line.contains(id), "{line}: {}", l.line);
            let meta = crate::state::SessionMeta { claude: true, claude_args: line.clone(), ..crate::state::SessionMeta::default() };
            let (o, r) = meta.claude_opts_eff();
            assert!(!o.launch(Kind::Pwsh, PathStyle::Native, &r).line.contains(id), "{line}");
        }
    }

    /// Review zu #245: Wertoptionen aus alten `claude_args` werden übernommen statt still weggeworfen, der Rest gemeldet.
    #[test]
    fn legacy_value_options_are_migrated_and_drops_reported() {
        let dir = if cfg!(windows) { "C:/work/lib" } else { "/work/lib" };
        let dir2 = if cfg!(windows) { "D:/data" } else { "/data" };
        let line = format!("--model opus --add-dir {dir} {dir2} --allowed-tools Read Grep --fallback-model sonnet --max-budget-usd 2.5 --append-system-prompt kurz --disallowed-tools Bash --resume x --print hi --verbose --dangerously-skip-permissions");
        let r = clean_legacy_report(&line, true);
        assert_eq!(r.line, "--model opus --verbose");
        assert_eq!(r.migrated.add_dirs, vec![dir.to_string(), dir2.to_string()]);
        assert_eq!((r.migrated.allowed_tools.as_str(), r.migrated.disallowed_tools.as_str()), ("Read Grep", "Bash"));
        assert_eq!((r.migrated.fallback_model.as_str(), r.migrated.max_budget_usd.as_str(), r.migrated.append_system_prompt.as_str()), ("sonnet", "2.5", "kurz"));
        assert_eq!(r.dropped, vec!["--resume", "--print", "--dangerously-skip-permissions"]);
        // Ungültige Werte (wie in der Oberfläche) werden nicht übernommen, sondern gemeldet; Werte nie als Prompt übrig
        let r = clean_legacy_report("--add-dir relative/x more --max-budget-usd 0 --fallback-model --mcp-config a b --verbose", true);
        assert_eq!(r.line, "--verbose");
        assert!(r.migrated.is_empty(), "{:?}", r.migrated);
        assert_eq!(r.dropped, vec!["--add-dir", "--max-budget-usd", "--fallback-model", "--mcp-config"]);
        // Aus einer Vorlage (weitergebbar) nie Rechte erweiternde Optionen
        let r = clean_legacy_report(&format!("--add-dir {dir} --allowed-tools Read --mcp-config {dir}/m.json --fallback-model haiku"), false);
        assert_eq!((r.migrated.add_dirs.len(), r.migrated.allowed_tools.as_str(), r.migrated.mcp_configs.len(), r.migrated.fallback_model.as_str()), (0, "", 0, "haiku"));
        assert_eq!(r.dropped, vec!["--add-dir", "--allowed-tools", "--mcp-config"]);
        // #289: mehrere MCP-Konfigurationen, Plugin-Ordner (je einer) und --strict-mcp-config (ohne Wert) aus alten Zeilen
        let r = clean_legacy_report(&format!("--mcp-config {dir}/a.json {dir}/b.json --strict-mcp-config --plugin-dir {dir} --plugin-dir {dir2} --system-prompt x --verbose"), true);
        assert_eq!(r.migrated.mcp_configs, vec![format!("{dir}/a.json"), format!("{dir}/b.json")]);
        assert!(r.migrated.strict_mcp_config);
        assert_eq!(r.migrated.plugin_dirs, vec![dir.to_string(), dir2.to_string()]);
        assert_eq!(r.line, "--verbose", "der Schalter nimmt keinen Wert");
        assert_eq!(r.dropped, vec!["--system-prompt"], "eigener System-Prompt nie aus alten Zeilen");
        let r = clean_legacy_report(&format!("--plugin-dir {dir} --strict-mcp-config"), false);
        assert!(r.migrated.is_empty() && r.dropped == vec!["--plugin-dir", "--strict-mcp-config"], "{r:?}");
        let (o, rest, dropped) = resolve_new_report(None, false, &ClaudeOpts::default(), &format!("--add-dir {dir} --fallback-model haiku -c"));
        assert_eq!((o.extra.fallback_model.as_str(), o.extra.add_dirs.len(), rest.as_str()), ("haiku", 0, ""));
        assert_eq!(dropped, vec!["--add-dir", "-c"]);
        // Sitzung aus state.json: einmalig umgestellt, danach nichts mehr zu melden; ein gesetzter Wert gewinnt
        let mut meta = crate::state::SessionMeta { claude: true, claude_args: format!("--add-dir {dir} --fallback-model haiku --continue --verbose"), ..crate::state::SessionMeta::default() };
        meta.claude_opts.extra.fallback_model = "opus".into();
        let before = meta.claude_opts_eff().0;
        assert_eq!(meta.migrate_legacy_args(), vec!["--continue"]);
        assert_eq!(meta.claude_args, "--verbose");
        assert_eq!((meta.claude_opts.extra.add_dirs.clone(), meta.claude_opts.extra.fallback_model.as_str()), (vec![dir.to_string()], "opus"));
        assert_eq!(meta.claude_opts_eff().0, before, "wirksame Optionen bleiben gleich");
        assert!(meta.migrate_legacy_args().is_empty(), "nur einmal");
        let mut plain = crate::state::SessionMeta { claude: true, claude_args: "--model opus".into(), ..crate::state::SessionMeta::default() };
        assert!(plain.migrate_legacy_args().is_empty());
        assert_eq!(plain.claude_args, "--model opus");
    }

    #[test]
    fn efforts_and_modes() {
        for e in EFFORTS {
            assert!(valid_effort(e));
        }
        assert!(!valid_effort("High") && !valid_effort("ultra") && !valid_effort(""));
        for m in PERMISSION_MODES {
            assert!(valid_permission_mode(m, false));
        }
        assert!(!valid_permission_mode(BYPASS, false) && valid_permission_mode(BYPASS, true));
        assert!(!valid_permission_mode("manual", true) && !valid_permission_mode("acceptedits", true));
    }

    #[test]
    fn mcp_accepts_valid_values() {
        let o = ClaudeOpts::from_mcp(&json!({"model": " opus ", "effort": "xhigh", "permission_mode": "plan", "agent": "code-reviewer"})).unwrap();
        assert_eq!(o, four("opus", "xhigh", "plan", "code-reviewer"));
        assert!(ClaudeOpts::from_mcp(&json!({})).unwrap().is_empty());
        assert!(ClaudeOpts::from_mcp(&json!({"model": "", "effort": null, "claude_flags": null})).unwrap().is_empty());
        assert!(ClaudeOpts::from_mcp(&json!({"claude_flags": []})).unwrap().is_empty());
        for m in PERMISSION_MODES {
            assert_eq!(ClaudeOpts::from_mcp(&json!({"permission_mode": m})).unwrap().permission_mode, m);
        }
    }

    #[test]
    fn mcp_rejects_invalid_values() {
        for bad in [
            json!({"model": "opus; rm -rf /"}),
            json!({"model": "--dangerously-skip-permissions"}),
            json!({"model": 5}),
            json!({"agent": "a b"}),
            json!({"agent": "x".repeat(101)}),
            json!({"effort": "ultra"}),
            json!({"permission_mode": "ask"}),
            json!({"permission_mode": "bypassPermissions"}),
        ] {
            assert!(ClaudeOpts::from_mcp(&bad).is_err(), "{bad}");
        }
        let (de, en) = ClaudeOpts::from_mcp(&json!({"permission_mode": "bypassPermissions"})).unwrap_err();
        assert!(en.contains("never allowed via MCP") && de.contains("nie erlaubt"));
    }

    #[test]
    fn mcp_accepts_every_agent_switch() {
        for (n, f) in SWITCHES {
            assert_eq!(ClaudeOpts::from_mcp(&json!({"claude_flags": [n]})).unwrap().extra.flags, [n], "{n}");
            assert_eq!(ClaudeOpts::from_mcp(&json!({"claude_flags": [f]})).unwrap().extra.flags, [n], "mit --: {f}");
        }
        let o = ClaudeOpts::from_mcp(&json!({"claude_flags": [" IDE ", "chrome", "ide", "--verbose"]})).unwrap();
        assert_eq!(o.extra.flags, ["ide", "chrome", "verbose"], "getrimmt, klein, Doppelte zählen einmal");
        assert_eq!(o.extra.sanitized().flags, ["chrome", "ide", "verbose"], "feste Reihenfolge");
    }

    #[test]
    fn mcp_refuses_forbidden_flags() {
        // Rechte/Zugriffe, Hooks, Ausgabe/Konversation (steuert Paneldeck), Fernsteuerung, Wertoptionen – eigene Meldung.
        for f in [
            "add-dir", "--add-dir", "allowed-tools", "--allowedTools", "disallowed-tools", "tools", "mcp-config", "strict-mcp-config", "plugin-dir", "settings",
            "agents", "permission-prompt-tool", "print", "-p", "output-format", "resume", "session-id", "remote-control", "teleport", "cloud",
            "dangerously-skip-permissions", "--dangerously-skip-permissions", "allow-dangerously-skip-permissions", "--add-dir=C:\\", "append-system-prompt",
            "fallback-model", "max-budget-usd", "continue", "safe-mode", "--safe-mode", "autocompact", "--autocompact=auto",
        ] {
            let (de, en) = ClaudeOpts::from_mcp(&json!({"claude_flags": [f]})).unwrap_err();
            assert!(en.contains("never allowed via MCP") && de.contains("nie erlaubt"), "{f}: {en}");
        }
        // Unbekannte Schalter, Werte, Feindseliges, falsche Typen.
        for bad in [
            json!({"claude_flags": ["chrom"]}),
            json!({"claude_flags": ["chrome=true"]}),
            json!({"claude_flags": ["chrome; calc"]}),
            json!({"claude_flags": ["$(id)"]}),
            json!({"claude_flags": ["chrome\n--add-dir"]}),
            json!({"claude_flags": ["--model"]}),
            json!({"claude_flags": [""]}),
            json!({"claude_flags": "chrome"}),
            json!({"claude_flags": {"chrome": true}}),
            json!({"claude_flags": [true]}),
            json!({"claude_flags": ["chrome", "no-chrome"]}),
            json!({"claude_flags": vec!["ide"; MAX_MCP_FLAGS + 1]}),
        ] {
            assert!(ClaudeOpts::from_mcp(&bad).is_err(), "{bad}");
        }
        assert!(ClaudeOpts::from_mcp(&json!({"claude_flags": ["chrome", "no-chrome"]})).unwrap_err().1.contains("mutually exclusive"));
        assert!(ClaudeOpts::from_mcp(&json!({"claude_flags": ["chrom"]})).unwrap_err().1.contains("unknown switch"));
    }

    #[test]
    fn mcp_refuses_user_only_params() {
        for k in USER_ONLY_PARAMS {
            let (de, en) = ClaudeOpts::from_mcp(&json!({ k: "x" })).unwrap_err();
            assert!(en.contains("not allowed via MCP") && de.contains("nicht erlaubt"), "{k}");
            assert!(ClaudeOpts::from_mcp(&json!({ k: null })).is_ok(), "null zählt als nicht angegeben: {k}");
        }
        assert!(ClaudeOpts::from_mcp(&json!({"add_dir": ["C:\\"]})).is_err());
        // Allgemeine Namen, die schon vorher unbeachtet blieben, bleiben es (kein neuer Fehler).
        for k in ["args", "tools", "settings", "resume", "session_id", "output_format", "agents", "claude_args"] {
            assert!(ClaudeOpts::from_mcp(&json!({ k: "x" })).unwrap().is_empty(), "{k}");
        }
    }

    #[test]
    fn agents_never_get_bypass_even_as_default() {
        let defaults = four("sonnet", "high", BYPASS, "");
        let o = ClaudeOpts::default().for_agent(&defaults);
        assert_eq!(o, four("sonnet", "high", "default", ""));
        // Angaben des Agenten haben Vorrang vor den Standards.
        let o = ClaudeOpts { model: "haiku".into(), permission_mode: "plan".into(), ..Default::default() }.for_agent(&defaults);
        assert_eq!((o.model.as_str(), o.effort.as_str(), o.permission_mode.as_str()), ("haiku", "high", "plan"));
    }

    #[test]
    fn risky_defaults_never_reach_agent_sessions() {
        let defaults = ClaudeOpts {
            extra: ExtraOpts {
                flags: vec!["verbose".into(), "chrome".into(), "safe-mode".into()],
                add_dirs: vec!["C:\\work".into()],
                allowed_tools: "Bash(git:*) Edit".into(),
                disallowed_tools: "WebFetch".into(),
                mcp_configs: vec!["C:\\cfg\\mcp.json".into()],
                plugin_dirs: vec!["C:\\plug".into()],
                strict_mcp_config: true,
                system_prompt: "Replace.".into(),
                append_system_prompt: "Be brief.".into(),
                fallback_model: "sonnet".into(),
                max_budget_usd: "5".into(),
                autocompact: "auto".into(),
            },
            ..Default::default()
        };
        let o = ClaudeOpts::default().for_agent(&defaults);
        assert!(o.extra.add_dirs.is_empty() && o.extra.allowed_tools.is_empty() && o.extra.mcp_configs.is_empty(), "{o:?}");
        assert!(!o.extra.has_risky());
        // Einschränkendes und Harmloses aus den Standards gilt weiter; Agenten-Schalter des Nutzers auch, safe-mode nie.
        assert_eq!(o.extra.disallowed_tools, "WebFetch");
        assert_eq!((o.extra.append_system_prompt.as_str(), o.extra.fallback_model.as_str(), o.extra.max_budget_usd.as_str(), o.extra.autocompact.as_str()), ("Be brief.", "sonnet", "5", "auto"));
        assert_eq!(o.extra.flags, ["chrome", "verbose"]);
        // Schalter des Agenten kommen dazu; bei chrome/no-chrome entscheidet sonst der Agent.
        let o = with_flags(&["no-chrome", "ide"]).for_agent(&defaults);
        assert_eq!(o.extra.flags, ["no-chrome", "ide", "verbose"]);
        // Selbst wenn eine Angabe (nie aus from_mcp) Rechte erweiternde Werte oder safe-mode trüge: weg.
        let o = defaults.for_agent(&ClaudeOpts::default());
        assert!(o.extra.add_dirs.is_empty() && o.extra.allowed_tools.is_empty() && o.extra.mcp_configs.is_empty());
        assert!(!o.extra.flags.iter().any(|f| f == "safe-mode"));
        let line = o.args_for(Kind::Pwsh, PathStyle::Native);
        assert!(!line.contains("--add-dir") && !line.contains("--allowed-tools") && !line.contains("--mcp-config") && !line.contains("--safe-mode"), "{line}");
        // Für Sitzungen des Nutzers (Kontextmenü, Vorlagen) gelten die Standards vollständig.
        let user = resolve_new(None, false, &defaults, "").0;
        assert_eq!(user.extra.allowed_tools, "Bash(git:*) Edit");
        assert!(user.extra.flags.iter().any(|f| f == "safe-mode"));
    }

    #[test]
    fn users_no_chrome_wins_over_agents_chrome() {
        let defaults = with_flags(&["no-chrome", "verbose"]);
        let agent = ClaudeOpts::from_mcp(&json!({"claude_flags": ["chrome", "ide"]})).unwrap();
        let (de, en) = agent.agent_conflict(&defaults).unwrap_err();
        assert!(en.contains("turned Chrome off") && de.contains("ausgeschaltet"));
        // Auch ohne die Prüfung: chrome des Agenten wird nicht übernommen.
        assert_eq!(agent.for_agent(&defaults).extra.flags, ["no-chrome", "ide", "verbose"]);
        // Umgekehrt (Nutzer chrome, Agent no-chrome) darf der Agent ausschalten.
        let agent = with_flags(&["no-chrome"]);
        assert!(agent.agent_conflict(&with_flags(&["chrome"])).is_ok());
        assert_eq!(agent.for_agent(&with_flags(&["chrome"])).extra.flags, ["no-chrome"]);
        assert!(with_flags(&["ide"]).agent_conflict(&defaults).is_ok());
    }

    #[test]
    fn command_line_per_shell() {
        let o = four("opus", "high", "acceptEdits", "reviewer");
        assert_eq!(o.args(Kind::Pwsh), " --model 'opus' --effort 'high' --permission-mode 'acceptEdits' --agent 'reviewer'");
        assert_eq!(o.args(Kind::Posix), " --model 'opus' --effort 'high' --permission-mode 'acceptEdits' --agent 'reviewer'");
        assert_eq!(o.args(Kind::Cmd), " --model opus --effort high --permission-mode acceptEdits --agent reviewer");
        // PowerShell würde „1e5“ ungequotet als Zahl 100000 übergeben.
        assert_eq!(ClaudeOpts { model: "1e5".into(), ..Default::default() }.args(Kind::Pwsh), " --model '1e5'");
        assert_eq!(ClaudeOpts { model: "claude-opus-4-5:beta".into(), ..Default::default() }.args(Kind::Posix), " --model 'claude-opus-4-5:beta'");
        assert_eq!(ClaudeOpts::default().args(Kind::Pwsh), "");
        // Nur einzelne Felder; bypassPermissions (Wahl des Nutzers) wird übernommen.
        assert_eq!(ClaudeOpts { permission_mode: BYPASS.into(), ..Default::default() }.args(Kind::Cmd), " --permission-mode bypassPermissions");
        // Schalter: feste Flags in fester Reihenfolge, ungequotet (keine Werte).
        let s = with_flags(&["verbose", "chrome", "ide", "safe-mode"]);
        for k in [Kind::Pwsh, Kind::Posix, Kind::Cmd] {
            assert_eq!(s.args(k), " --chrome --ide --verbose --safe-mode", "{k:?}");
            assert_eq!(s.args_for(k, PathStyle::Native), " --chrome --ide --verbose --safe-mode", "{k:?}");
        }
    }

    #[test]
    fn legacy_rest_comes_before_variadic_options() {
        let (d, dir, _) = temp_dir("order");
        let o = ClaudeOpts { model: "opus".into(), extra: ExtraOpts { add_dirs: vec![dir.clone()], disallowed_tools: "WebFetch".into(), ..Default::default() }, ..Default::default() };
        let l = o.launch(Kind::Pwsh, PathStyle::Native, "--x val");
        assert_eq!(l.line, format!(" --model 'opus' --x 'val' --add-dir '{dir}' --disallowed-tools 'WebFetch'"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn command_line_drops_invalid_values() {
        // Direkt gesetzte (nicht über Deserialize geprüfte) Werte kommen nie ungeprüft in die Zeile.
        let mut o = four("x' ; calc; '", "ultra", "ask", "$(id)");
        o.extra = ExtraOpts {
            flags: vec!["add-dir".into(), "dangerously-skip-permissions".into(), "chrome; calc".into(), "autocompact".into()],
            add_dirs: vec!["$(calc)".into(), "\\\\server\\share".into()],
            allowed_tools: "Bash(x)'; calc; '".into(),
            disallowed_tools: "\"a\"".into(),
            mcp_configs: vec!["relative.json".into()],
            plugin_dirs: vec!["relative".into(), "\\\\server\\plug".into()],
            strict_mcp_config: true,
            system_prompt: "a & b\nc".into(),
            append_system_prompt: "a\nb".into(),
            fallback_model: "a b".into(),
            max_budget_usd: "1e3".into(),
            autocompact: "auto; calc".into(),
        };
        for k in [Kind::Pwsh, Kind::Posix, Kind::Cmd] {
            assert_eq!(o.args_for(k, PathStyle::Native), "", "{k:?}");
        }
    }

    #[test]
    fn switches_conflict_and_order() {
        assert_eq!(with_flags(&["chrome", "no-chrome", "ide"]).sanitized(false).extra.flags, ["ide"]);
        assert_eq!(with_flags(&["brief", "brief", "nope", "autocompact"]).sanitized(false).extra.flags, ["brief"]);
    }

    #[test]
    fn paths_are_validated() {
        let long = format!("C:\\{}", "a".repeat(MAX_PATH_LEN));
        for bad in [
            "",
            " C:\\x",
            "relative\\dir",
            "\\\\server\\share",
            "//server/share",
            "\\\\?\\C:\\x",
            "\\\\?\\UNC\\server\\share",
            "\\\\.\\pipe\\x",
            "-C:\\x",
            "C:\\a%PATH%",
            "C:\\a\"b",
            "C:\\a&calc",
            "C:\\a|b",
            "C:\\a^b",
            "C:\\a!b",
            "C:\\a<b",
            "C:\\a\nb",
            "C:\\a\u{7}",
            long.as_str(),
        ] {
            assert!(!valid_path(bad), "{bad:?}");
        }
        #[cfg(windows)]
        for ok in ["C:\\", "C:\\Users\\me\\proj", "d:/x y/z", "C:\\it's $(x) `y`", "C:\\Ünïcode\\ordner"] {
            assert!(valid_path(ok), "{ok}");
        }
        #[cfg(not(windows))]
        for ok in ["/", "/home/me/proj", "/x y/it's"] {
            assert!(valid_path(ok), "{ok}");
        }
    }

    #[test]
    fn existing_local_paths_only() {
        let (d, dir, file) = temp_dir("exist");
        assert_eq!(check_path(&dir, true), Ok(()));
        assert_eq!(check_path(&file, false), Ok(()));
        assert_eq!(check_path(&file, true), Err(PathIssue::Missing), "eine Datei ist kein Ordner");
        assert_eq!(check_path(&dir, false), Err(PathIssue::Missing));
        assert_eq!(check_path(&format!("{dir}{}nope", std::path::MAIN_SEPARATOR), true), Err(PathIssue::Missing));
        assert_eq!(check_path("\\\\server\\share", true), Err(PathIssue::Invalid));
        assert_eq!(check_path("relative", true), Err(PathIssue::Invalid));
        let _ = std::fs::remove_dir_all(d);
    }

    /// Eine Junction (ohne Adminrechte anlegbar) auf eine Netzwerkfreigabe wird erkannt, ohne ihr zu folgen; eine auf
    /// einen lokalen Ordner ist in Ordnung. Angelegt mit `mklink /J` (cmd), das das Ziel nicht prüft.
    #[cfg(windows)]
    #[test]
    fn links_to_network_shares_are_never_followed() {
        let (d, dir, _) = temp_dir("junction");
        let mk_kind = |kind: &str, link: &str, target: &str| std::process::Command::new("cmd").args(["/C", "mklink", kind, link, target]).output().map(|o| o.status.success()).unwrap_or(false);
        // Junction (lokal) bzw. – für eine Freigabe, falls Symlinks erlaubt sind (Entwicklermodus) – ein Ordner-Symlink.
        let mk = |link: &str, target: &str| mk_kind("/J", link, target) || mk_kind("/D", link, target);
        let local = format!("{dir}\\to-local");
        let target = format!("{dir}\\target");
        std::fs::create_dir_all(&target).unwrap();
        if !mk(&local, &target) {
            eprintln!("mklink /J nicht verfügbar – übersprungen");
            let _ = std::fs::remove_dir_all(d);
            return;
        }
        assert_eq!(check_path(&local, true), Ok(()));
        assert_eq!(check_path(&format!("{local}\\"), true), Ok(()));
        // Ziel auf einer Freigabe (der Rechner selbst über einen ungültigen Freigabenamen – wird nie aufgerufen).
        let net = format!("{dir}\\to-net");
        if mk(&net, "\\\\127.0.0.1\\pd-flags-never-touched$\\x") {
            let t0 = std::time::Instant::now();
            assert_eq!(check_path(&net, true), Err(PathIssue::Network));
            assert_eq!(check_path(&format!("{net}\\sub"), true), Err(PathIssue::Network), "auch als Zwischenteil");
            assert!(t0.elapsed() < std::time::Duration::from_secs(2), "kein Netzwerkzugriff (sonst Wartezeit)");
        }
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn tools_text_budget_autocompact() {
        for ok in ["Bash(git log:*) Read", "Edit,Write", "mcp__github__*", "WebFetch(domain:example.com)", "Read(~/.zshrc)", "Edit(/src/**/*.ts)", "Bash(npm run test:*)"] {
            assert!(valid_tools(ok), "{ok}");
        }
        let long_tools = "a".repeat(MAX_TOOLS_LEN + 1);
        for bad in ["", "-x", "Bash(rm -rf /);calc", "a'b", "a\"b", "$(id)", "a`b", "a%b", "a&b", "a|b", "a\nb", "Read\\", long_tools.as_str()] {
            assert!(!valid_tools(bad), "{bad:?}");
        }
        for ok in ["Answer in German.", "Use 'single' quotes, $HOME and `ticks`; (ok) [x] {y}", "Ünïcode ✓"] {
            assert!(valid_text(ok), "{ok}");
        }
        let long_text = "a".repeat(MAX_TEXT_LEN + 1);
        for bad in ["", "-x", "two\nlines", "a\rb", "tab\tx", "a\"b", "50%", "a&b", "a|b", "a<b", "a>b", "a^b", "wow!", "ends\\", "a\\b", "it\\'s", long_text.as_str()] {
            assert!(!valid_text(bad), "{bad:?}");
        }
        for ok in ["1", "5", "0.5", "12.25", "999999.99"] {
            assert!(valid_budget(ok), "{ok}");
        }
        for bad in ["", "0", "0.00", "-1", "1.", ".5", "1.234", "1e3", "1,5", "1234567", " 1", "NaN", "inf", "0x10"] {
            assert!(!valid_budget(bad), "{bad:?}");
        }
        for ok in ["auto", "100000", "200k", "1"] {
            assert!(valid_autocompact(ok), "{ok}");
        }
        for bad in ["", "Auto", "0", "0k", "k", "1kk", "1.5k", "-5", "1234567890", "auto;calc", " auto"] {
            assert!(!valid_autocompact(bad), "{bad:?}");
        }
    }

    #[test]
    fn path_style_follows_the_program() {
        if cfg!(windows) {
            assert_eq!(PathStyle::for_shell("wsl:Ubuntu", "C:\\Windows\\System32\\wsl.exe", Kind::Posix), PathStyle::Wsl);
            assert_eq!(PathStyle::for_shell("custom:3", "C:\\Windows\\System32\\wsl.exe", Kind::Posix), PathStyle::Wsl, "eigenes Profil mit wsl.exe");
            assert_eq!(PathStyle::for_shell("custom:4", "wsl", Kind::Posix), PathStyle::Wsl);
            assert_eq!(PathStyle::for_shell("custom:5", "C:/Windows/System32/bash.exe", Kind::Posix), PathStyle::Wsl, "bash.exe aus System32 ist WSL");
            assert_eq!(PathStyle::for_shell("gitbash", "C:\\Program Files\\Git\\bin\\bash.exe", Kind::Posix), PathStyle::Slash);
            assert_eq!(PathStyle::for_shell("custom:6", "C:\\Program Files\\Git\\bin\\bash.exe", Kind::Posix), PathStyle::Slash);
            assert_eq!(PathStyle::for_shell("powershell", "powershell.exe", Kind::Pwsh), PathStyle::Native);
            assert_eq!(PathStyle::for_shell("cmd", "cmd.exe", Kind::Cmd), PathStyle::Native);
        } else {
            assert_eq!(PathStyle::for_shell("bash", "/bin/bash", Kind::Posix), PathStyle::Native);
        }
        assert_eq!(PathStyle::Slash.claude_program(), "MSYS2_ARG_CONV_EXCL='*' claude");
        assert_eq!(PathStyle::Native.claude_program(), "claude");
        assert_eq!(PathStyle::Wsl.claude_program(), "claude");
    }

    #[test]
    fn value_options_quoted_per_shell() {
        let (d, dir, file) = temp_dir("quote");
        let o = ClaudeOpts {
            extra: ExtraOpts {
                flags: vec!["ide".into()],
                add_dirs: vec![dir.clone(), format!("{dir}{}missing", std::path::MAIN_SEPARATOR)],
                allowed_tools: "Bash(git log:*) Read".into(),
                disallowed_tools: "WebFetch".into(),
                mcp_configs: vec![file.clone()],
                plugin_dirs: Vec::new(),
                strict_mcp_config: false,
                system_prompt: String::new(),
                append_system_prompt: "It's $HOME `x` (y)".into(),
                fallback_model: "sonnet".into(),
                max_budget_usd: "1e3".into(),
                autocompact: "200k".into(),
            },
            ..Default::default()
        };
        let pw = o.launch(Kind::Pwsh, PathStyle::Native, "");
        assert_eq!(
            pw.line,
            format!(
                " --ide --add-dir '{dir}' --allowed-tools 'Bash(git log:*) Read' --disallowed-tools 'WebFetch' --mcp-config '{file}' --append-system-prompt 'It''s $HOME `x` (y)' --fallback-model 'sonnet' --autocompact '200k'"
            ),
            "fehlender Ordner und ungültiges Budget fallen weg"
        );
        assert_eq!(pw.dropped, [format!("{dir}{}missing", std::path::MAIN_SEPARATOR)], "weggelassene Pfade werden gemeldet");
        assert_eq!(pw.applied.extra.add_dirs, [dir.clone()], "nur, was wirklich gesetzt ist");
        let cmd = o.args_for(Kind::Cmd, PathStyle::Native);
        assert_eq!(
            cmd,
            format!(" --ide --add-dir \"{dir}\" --allowed-tools \"Bash(git log:*) Read\" --disallowed-tools \"WebFetch\" --mcp-config \"{file}\" --append-system-prompt \"It's $HOME `x` (y)\" --fallback-model sonnet --autocompact 200k")
        );
        let posix = o.args_for(Kind::Posix, PathStyle::Native);
        assert!(posix.contains("--append-system-prompt 'It'\\''s $HOME `x` (y)'"), "{posix}");
        #[cfg(windows)]
        {
            let fwd = dir.replace('\\', "/");
            let gb = o.args_for(Kind::Posix, PathStyle::Slash);
            assert!(gb.contains(&format!(" --add-dir '{fwd}'")), "Git Bash: Schrägstriche: {gb}");
            // WSL: /mnt/<laufwerk>/…
            let wsl = o.args_for(Kind::Posix, PathStyle::Wsl);
            let drive = dir.as_bytes()[0].to_ascii_lowercase() as char;
            assert!(wsl.contains(&format!(" --add-dir '/mnt/{drive}/")), "{wsl}");
            assert!(wsl.contains(&format!("--mcp-config '/mnt/{drive}/")), "{wsl}");
        }
        // Fehlende MCP-Konfiguration: weggelassen und gemeldet.
        let m = ClaudeOpts { extra: ExtraOpts { mcp_configs: vec![format!("{dir}{}nope.json", std::path::MAIN_SEPARATOR)], ..Default::default() }, ..Default::default() };
        let l = m.launch(Kind::Pwsh, PathStyle::Native, "");
        assert_eq!((l.line.as_str(), l.dropped.len()), ("", 1));
        assert!(l.applied.extra.mcp_configs.is_empty() && l.applied.to_json()["claude_user_options"] == json!([]));
        // Budget gültig: als Wert.
        let b = ClaudeOpts { extra: ExtraOpts { max_budget_usd: "2.50".into(), ..Default::default() }, ..Default::default() };
        assert_eq!(b.args_for(Kind::Pwsh, PathStyle::Native), " --max-budget-usd '2.50'");
        assert_eq!(b.args_for(Kind::Cmd, PathStyle::Native), " --max-budget-usd 2.50");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn trailing_separators_never_escape_the_quote() {
        assert_eq!(shell_path(PathStyle::Native, "C:\\x\\"), "C:\\x");
        assert_eq!(shell_path(PathStyle::Native, "C:\\"), "C:/");
        assert_eq!(shell_path(PathStyle::Wsl, "C:\\"), "/mnt/c");
        assert_eq!(shell_path(PathStyle::Wsl, "D:\\a b\\"), "/mnt/d/a b");
        assert_eq!(shell_path(PathStyle::Slash, "C:\\a\\b\\"), "C:/a/b");
        assert_eq!(shell_path(PathStyle::Native, "/"), "/");
    }

    #[test]
    fn hostile_values_never_reach_the_line() {
        let hostile = [
            "x\" & calc & \"",
            "x' ; calc ; '",
            "$(calc)",
            "`calc`",
            "%COMSPEC%",
            "a\r\nclaude --dangerously-skip-permissions",
            "-\u{2013}dangerously-skip-permissions",
            "--dangerously-skip-permissions",
            "x | calc",
            "x > C:\\out.txt",
            "!x!",
            "^&calc",
            "C:\\x\" --dangerously-skip-permissions \"",
            "x\\' ; calc",
        ];
        for h in hostile {
            let o = ClaudeOpts {
                extra: ExtraOpts {
                    flags: vec![h.into()],
                    add_dirs: vec![h.into(), format!("C:\\{h}")],
                    allowed_tools: h.into(),
                    disallowed_tools: h.into(),
                    mcp_configs: vec![format!("C:\\{h}")],
                    plugin_dirs: vec![h.into(), format!("C:\\{h}")],
                    strict_mcp_config: true,
                    system_prompt: String::new(),
                    append_system_prompt: h.into(),
                    fallback_model: h.into(),
                    max_budget_usd: h.into(),
                    autocompact: h.into(),
                },
                ..Default::default()
            };
            // Pfade, Schalter, Modell, Budget, autocompact: immer verworfen (nicht existierend bzw. ungültig).
            let s = o.sanitized(false).extra;
            assert!(s.flags.is_empty() && s.fallback_model.is_empty() && s.max_budget_usd.is_empty() && s.autocompact.is_empty(), "{h:?}: {s:?}");
            for k in [Kind::Pwsh, Kind::Posix, Kind::Cmd] {
                let line = o.args_for(k, PathStyle::Native);
                assert!(!line.contains("--add-dir") && !line.contains("--mcp-config") && !line.contains(" --dangerously"), "{h:?} {k:?}: {line}");
                assert!(!line.contains(['\r', '\n', '%', '!', '^', '&', '|', '<', '>']), "{h:?} {k:?}: {line}");
                // Was übrig bleibt, ist genau der geprüfte Text als EIN wörtliches Argument.
                let mut want = String::new();
                if valid_tools(h) {
                    want += &format!(" --allowed-tools {q} --disallowed-tools {q}", q = quote(k, h));
                }
                if valid_text(h) {
                    want += &format!(" --append-system-prompt {}", quote(k, h));
                }
                assert_eq!(line, want, "{h:?} {k:?}");
            }
        }
        // `$(calc)`/`` `calc` `` sind als Text erlaubt, aber immer wörtlich gequotet (PowerShell/POSIX: einfache Quotes).
        let t = ClaudeOpts { extra: ExtraOpts { append_system_prompt: "$(calc) `calc`".into(), ..Default::default() }, ..Default::default() };
        assert_eq!(t.args_for(Kind::Pwsh, PathStyle::Native), " --append-system-prompt '$(calc) `calc`'");
        assert_eq!(t.args_for(Kind::Posix, PathStyle::Native), " --append-system-prompt '$(calc) `calc`'");
        // PowerShell: typografische Apostrophe werden verdoppelt.
        let t = ClaudeOpts { extra: ExtraOpts { append_system_prompt: "it\u{2019}s; calc".into(), ..Default::default() }, ..Default::default() };
        assert_eq!(t.args_for(Kind::Pwsh, PathStyle::Native), " --append-system-prompt 'it\u{2019}\u{2019}s; calc'");
    }

    #[test]
    fn serde_is_lenient_and_compact() {
        assert_eq!(serde_json::to_value(ClaudeOpts::default()).unwrap(), json!({}));
        let o = ClaudeOpts { model: "opus".into(), ..Default::default() };
        assert_eq!(serde_json::to_value(&o).unwrap(), json!({"model": "opus"}));
        let back: ClaudeOpts = serde_json::from_value(json!({"model": "opus", "effort": 7, "permission_mode": "bypassPermissions", "agent": "a b", "x": 1})).unwrap();
        assert_eq!(back, ClaudeOpts { model: "opus".into(), permission_mode: BYPASS.into(), ..Default::default() });
        let back: ClaudeOpts = serde_json::from_value(json!("kaputt")).unwrap();
        assert!(back.is_empty());
    }

    #[test]
    fn serde_extra_roundtrip_and_back_compat() {
        // Alte Dateien (vor #182) haben keine weiteren Felder: alles leer.
        let old: ClaudeOpts = serde_json::from_value(json!({"model": "opus", "effort": "high"})).unwrap();
        assert!(old.extra.is_empty());
        let o = ClaudeOpts {
            model: "opus".into(),
            extra: ExtraOpts {
                flags: vec!["chrome".into(), "verbose".into(), "safe-mode".into()],
                add_dirs: vec!["C:\\a".into(), "C:\\b c".into()],
                allowed_tools: "Read".into(),
                disallowed_tools: "WebFetch".into(),
                mcp_configs: vec!["C:\\m.json".into(), "C:\\n.json".into()],
                strict_mcp_config: true,
                plugin_dirs: vec!["C:\\plug".into()],
                system_prompt: "Line 1\nLine 2".into(),
                append_system_prompt: "Be brief.".into(),
                fallback_model: "sonnet".into(),
                max_budget_usd: "2.5".into(),
                autocompact: "200k".into(),
            },
            ..Default::default()
        };
        let j = serde_json::to_value(&o).unwrap();
        // #289: Listen und neue Felder; der erste Eintrag zusätzlich als `mcp_config` für ältere Versionen.
        assert_eq!(j["mcp_configs"], json!(["C:\\m.json", "C:\\n.json"]));
        assert_eq!(j["mcp_config"], "C:\\m.json");
        assert_eq!(j["strict_mcp_config"], true);
        assert_eq!(j["plugin_dirs"], json!(["C:\\plug"]));
        assert_eq!(j["system_prompt"], "Line 1\nLine 2");
        assert!(serde_json::to_value(ClaudeOpts::default()).unwrap().as_object().unwrap().is_empty(), "kompakt");
        assert_eq!(j["flags"], json!(["chrome", "verbose", "safe-mode"]));
        assert_eq!(j["add_dirs"], json!(["C:\\a", "C:\\b c"]));
        assert_eq!(j["max_budget_usd"], "2.5");
        assert_eq!(j["autocompact"], "200k");
        assert!(j.get("extra").is_none(), "flach auf derselben Ebene");
        if cfg!(windows) {
            let back: ClaudeOpts = serde_json::from_value(j).unwrap();
            assert_eq!(back, o);
        }
        // Tolerant: falsche Typen, ungültige Werte und unbekannte Schalter fallen still weg; Zahlen auch als Zahl.
        let back: ClaudeOpts = serde_json::from_value(json!({
            "flags": ["chrome", "add-dir", 5, "dangerously-skip-permissions", "autocompact"],
            "add_dirs": "C:\\x",
            "allowed_tools": ["Read"],
            "append_system_prompt": "a\nb",
            "fallback_model": "a b",
            "max_budget_usd": 3,
            "autocompact": 100000
        }))
        .unwrap();
        assert_eq!(back.extra, ExtraOpts { flags: vec!["chrome".into()], max_budget_usd: "3".into(), autocompact: "100000".into(), ..Default::default() });
        let back: ClaudeOpts = serde_json::from_value(json!({"max_budget_usd": 2.5})).unwrap();
        assert_eq!(back.extra.max_budget_usd, "2.5");
    }

    #[test]
    fn old_single_mcp_config_still_loads_and_merges() {
        let (a, b) = if cfg!(windows) { ("C:\\a.json", "C:\\b.json") } else { ("/a.json", "/b.json") };
        // Datei einer älteren Version: nur der einzelne Wert.
        let old: ClaudeOpts = serde_json::from_value(json!({"mcp_config": a})).unwrap();
        assert_eq!(old.extra.mcp_configs, vec![a.to_string()]);
        // Neue Datei: Liste, erster Eintrag doppelt als mcp_config – keine Doppelten.
        let new: ClaudeOpts = serde_json::from_value(json!({"mcp_config": a, "mcp_configs": [a, b]})).unwrap();
        assert_eq!(new.extra.mcp_configs, vec![a.to_string(), b.to_string()]);
        // Mit Liste zählt nur die Liste: ein von Hand entfernter Eintrag kommt über den Spiegel nicht zurück.
        let mixed: ClaudeOpts = serde_json::from_value(json!({"mcp_config": b, "mcp_configs": [a]})).unwrap();
        assert_eq!(mixed.extra.mcp_configs, vec![a.to_string()]);
        let emptied: ClaudeOpts = serde_json::from_value(json!({"mcp_config": a, "mcp_configs": []})).unwrap();
        assert!(emptied.extra.mcp_configs.is_empty());
        // Rundweg
        let back: ClaudeOpts = serde_json::from_value(serde_json::to_value(&new).unwrap()).unwrap();
        assert_eq!(back, new);
        // strict ohne MCP-Konfiguration fällt weg; falscher Typ ebenso.
        let s: ClaudeOpts = serde_json::from_value(json!({"strict_mcp_config": true})).unwrap();
        assert!(!s.extra.strict_mcp_config);
        let s: ClaudeOpts = serde_json::from_value(json!({"strict_mcp_config": "yes", "mcp_configs": [a]})).unwrap();
        assert!(!s.extra.strict_mcp_config);
        let s: ClaudeOpts = serde_json::from_value(json!({"strict_mcp_config": true, "mcp_configs": [a], "plugin_dirs": [a, "relative", 3], "system_prompt": "x\r\ny"})).unwrap();
        assert!(s.extra.strict_mcp_config);
        assert_eq!(s.extra.plugin_dirs, vec![a.to_string()]);
        assert_eq!(s.extra.system_prompt, "x\ny", "Zeilenumbrüche vereinheitlicht");
    }

    #[test]
    fn system_prompt_validation_and_quoting() {
        for ok in ["Du bist ein Reviewer.", "Line 1\nLine 2\n\n- point $HOME `x` 'q'", "a"] {
            assert!(valid_system_prompt(ok), "{ok:?}");
        }
        let long = "a".repeat(MAX_SYSTEM_PROMPT_LEN + 1);
        for bad in ["", "-x", " x", "x\n", "a\\b", "a\"b", "50%", "a & b", "a\tb", "a\rb", "x\n-y|z", long.as_str()] {
            assert!(!valid_system_prompt(bad), "{bad:?}");
        }
        assert!(valid_system_prompt(&"a".repeat(MAX_SYSTEM_PROMPT_LEN)));
        let o = |s: &str| ClaudeOpts { extra: ExtraOpts { system_prompt: s.into(), append_system_prompt: "Then this.".into(), ..Default::default() }, ..Default::default() };
        // Einzeilig: wie jeder Text gequotet.
        assert_eq!(o("It's me").args_for(Kind::Pwsh, PathStyle::Native), " --system-prompt 'It''s me' --append-system-prompt 'Then this.'");
        // Mehrzeilig je Shell: POSIX-Shells $'…' mit Zeilenumbrüchen, PowerShell und cmd mit Leerzeichen (claude.cmd).
        let m = o("Line 1\nIt's 2\n\n$x");
        assert_eq!(m.args_for(Kind::Pwsh, PathStyle::Native), " --system-prompt 'Line 1 It''s 2 $x' --append-system-prompt 'Then this.'");
        assert_eq!(m.args_for(Kind::Posix, PathStyle::Slash), " --system-prompt $'Line 1\\nIt\\'s 2\\n\\n$x' --append-system-prompt 'Then this.'", "Git Bash: echte Zeilenumbrüche");
        // WSL (Login-Shell unbekannt: fish, nushell …) und andere POSIX-Shells: kein $'…', Zeilen mit Leerzeichen.
        for style in [PathStyle::Wsl, PathStyle::Native] {
            assert_eq!(m.args_for(Kind::Posix, style), " --system-prompt 'Line 1 It'\\''s 2 $x' --append-system-prompt 'Then this.'", "{style:?}");
        }
        assert_eq!(m.args_for(Kind::Cmd, PathStyle::Native), " --system-prompt \"Line 1 It's 2 $x\" --append-system-prompt \"Then this.\"");
        // Die Zeile enthält nie einen echten Zeilenumbruch (sie wird in die Shell getippt).
        for k in [Kind::Pwsh, Kind::Posix, Kind::Cmd] {
            assert!(!m.args_for(k, PathStyle::Native).contains('\n'));
        }
        // Ungültig: nicht übergeben
        assert_eq!(o("a & calc").args_for(Kind::Pwsh, PathStyle::Native), " --append-system-prompt 'Then this.'");
        assert_eq!(o("x").to_json()["claude_user_options"], json!(["system_prompt", "append_system_prompt"]));
    }

    #[test]
    fn duplicates_ignore_case_and_trailing_separators_and_preview_never_touches_the_disk() {
        assert_eq!(path_key("C:\\Work\\"), path_key("c:/work"));
        assert_eq!(path_key("D:\\"), path_key("d:"));
        assert_ne!(path_key("/home/U"), path_key("/home/u"), "Linux-Pfade unterscheiden Groß-/Kleinschreibung");
        assert_eq!(path_key("/srv/x/"), path_key("/srv/x"));
        if cfg!(windows) {
            let e = ExtraOpts { add_dirs: vec!["C:\\Work".into(), "c:\\work\\".into(), "C:/WORK".into()], ..Default::default() };
            assert_eq!(e.sanitized().add_dirs, vec!["C:\\Work".to_string()]);
            // Vorschau: fehlende Pfade bleiben (nur Form), Start: weggelassen.
            let o = ClaudeOpts { extra: ExtraOpts { add_dirs: vec!["C:\\gibt-es-nicht\\x".into()], ..Default::default() }, ..Default::default() };
            assert!(o.preview(Kind::Pwsh, PathStyle::Native, "").contains("--add-dir 'C:\\gibt-es-nicht\\x'"));
            assert!(!o.launch(Kind::Pwsh, PathStyle::Native, "").line.contains("--add-dir"));
            assert!(o.preview(Kind::Pwsh, PathStyle::Native, "--x val").contains("--x"), "mit freien Resten");
        }
    }

    #[test]
    fn plugin_dirs_multiple_mcp_configs_and_strict() {
        let (d, dir, file) = temp_dir("plug");
        let f2 = d.join("second.json");
        std::fs::write(&f2, "{}").unwrap();
        let file2 = f2.to_string_lossy().to_string();
        let sep = std::path::MAIN_SEPARATOR;
        let o = ClaudeOpts {
            extra: ExtraOpts {
                plugin_dirs: vec![dir.clone(), format!("{dir}{sep}missing"), dir.clone()],
                mcp_configs: vec![file.clone(), format!("{dir}{sep}nope.json"), file2.clone()],
                strict_mcp_config: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let l = o.launch(Kind::Pwsh, PathStyle::Native, "");
        let q = |p: &str| quote(Kind::Pwsh, p.trim_end_matches(['\\', '/']));
        assert_eq!(l.line, format!(" --plugin-dir {} --mcp-config {} {} --strict-mcp-config", q(&dir), q(&file), q(&file2)));
        assert_eq!(l.dropped, vec![format!("{dir}{sep}missing"), format!("{dir}{sep}nope.json")]);
        assert_eq!(l.applied.to_json()["claude_user_options"], json!(["plugin_dir", "mcp_config", "strict_mcp_config"]));
        // Je Plugin-Ordner ein eigenes --plugin-dir.
        let two = ClaudeOpts { extra: ExtraOpts { plugin_dirs: vec![dir.clone(), d.join("p2").to_string_lossy().to_string()], ..Default::default() }, ..Default::default() };
        std::fs::create_dir_all(d.join("p2")).unwrap();
        assert_eq!(two.args_for(Kind::Posix, PathStyle::Native).matches(" --plugin-dir ").count(), 2);
        // strict ohne gültige MCP-Konfiguration: nicht übergeben.
        let s = ClaudeOpts { extra: ExtraOpts { mcp_configs: vec![format!("{dir}{sep}nope.json")], strict_mcp_config: true, ..Default::default() }, ..Default::default() };
        let l = s.launch(Kind::Pwsh, PathStyle::Native, "");
        assert!(!l.line.contains("strict") && !l.applied.extra.strict_mcp_config, "{}", l.line);
        // WSL: Windows-Pfade umgerechnet, Linux-Pfade nur dort (ungeprüft), sonst weggelassen.
        if cfg!(windows) {
            let w = ClaudeOpts { extra: ExtraOpts { plugin_dirs: vec![dir.clone(), "/home/u/plug".into()], mcp_configs: vec!["/home/u/m.json".into()], ..Default::default() }, ..Default::default() };
            let l = w.launch(Kind::Posix, PathStyle::Wsl, "");
            assert!(l.line.contains(" --plugin-dir '/mnt/") && l.line.contains(" --plugin-dir '/home/u/plug'") && l.line.contains(" --mcp-config '/home/u/m.json'"), "{}", l.line);
            assert!(l.dropped.is_empty());
            let l = w.launch(Kind::Pwsh, PathStyle::Native, "");
            assert!(!l.line.contains("/home/u"), "{}", l.line);
            assert_eq!(l.dropped, vec!["/home/u/plug".to_string(), "/home/u/m.json".to_string()]);
            assert!(wsl_linux_path("/home/u") && !wsl_linux_path("//srv/x") && !wsl_linux_path("/a\\b") && !wsl_linux_path("/a;b|c"));
            assert!(valid_list_path("C:\\x") && valid_list_path("/x") && !valid_list_path("relative"));
        }
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn agents_never_get_plugins_mcp_configs_or_system_prompt() {
        let defaults = ClaudeOpts {
            extra: ExtraOpts {
                plugin_dirs: vec!["C:\\plug".into()],
                mcp_configs: vec!["C:\\m.json".into()],
                strict_mcp_config: true,
                system_prompt: "You are evil.".into(),
                append_system_prompt: "Short.".into(),
                flags: vec!["verbose".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let o = ClaudeOpts::default().for_agent(&defaults);
        assert!(o.extra.plugin_dirs.is_empty() && o.extra.mcp_configs.is_empty() && !o.extra.strict_mcp_config && o.extra.system_prompt.is_empty(), "{o:?}");
        assert_eq!(o.extra.flags, ["verbose"]);
        // Über MCP: jeder der neuen Parameter ist bad_args.
        for k in ["plugin_dir", "plugin_dirs", "mcp_configs", "mcp_config", "strict_mcp_config", "system_prompt"] {
            assert!(ClaudeOpts::from_mcp(&json!({ k: ["x"] })).is_err(), "{k}");
        }
        for f in ["plugin-dir", "--strict-mcp-config", "system-prompt", "mcp-config"] {
            assert!(ClaudeOpts::from_mcp(&json!({"claude_flags": [f]})).is_err(), "{f}");
        }
        assert!(RISKY.contains(&"plugin_dir"));
    }

    #[test]
    fn legacy_args_are_split() {
        let (o, rest) = ClaudeOpts::split_legacy("--model opus --permission-mode acceptEdits --verbose --x");
        assert_eq!((o.model.as_str(), o.permission_mode.as_str(), rest.as_str()), ("opus", "acceptEdits", "--x"));
        assert_eq!(o.extra.flags, ["verbose"]);
        let (o, rest) = ClaudeOpts::split_legacy("--model --ok");
        assert!(o.is_empty());
        assert_eq!(rest, "--model --ok");
        // --autocompact nimmt seinen Wert mit; ohne gültigen Wert bleibt es in der freien Zeile.
        let (o, rest) = ClaudeOpts::split_legacy("--autocompact 200k --verbose");
        assert_eq!((o.extra.autocompact.as_str(), rest.as_str()), ("200k", ""));
        let (o, rest) = ClaudeOpts::split_legacy("--autocompact --verbose");
        assert_eq!((o.extra.autocompact.as_str(), rest.as_str(), o.extra.flags.clone()), ("", "--autocompact", vec!["verbose".to_string()]));
        // Hier ungültige Werte bleiben in der freien Zeile (alte Sitzungen verlieren nichts).
        let (o, rest) = ClaudeOpts::split_legacy("--model opus[1m] --permission-mode bypassPermissions");
        assert_eq!((o.model.as_str(), o.permission_mode.as_str(), rest.as_str()), ("", BYPASS, "--model opus[1m]"));
        let mut o = four("opus", "max", "plan", "a");
        o.extra.flags = vec!["chrome".into(), "ide".into(), "safe-mode".into()];
        assert_eq!(o.to_legacy(), "--model opus --effort max --permission-mode plan --agent a --chrome --ide --safe-mode");
        assert_eq!(ClaudeOpts::split_legacy(&o.to_legacy()), (o.clone(), String::new()));
        // Wertoptionen kommen nicht in die Vorlagenzeile.
        o.extra.append_system_prompt = "secret".into();
        o.extra.add_dirs = vec!["C:\\x".into()];
        assert!(!o.to_legacy().contains("secret") && !o.to_legacy().contains("add-dir"));
    }

    #[test]
    fn summary_and_json() {
        let o = four("opus", "high", "", "rev");
        assert_eq!(o.summary("Agent"), "opus · high · Agent rev");
        assert_eq!(o.to_json(), json!({"model": "opus", "effort": "high", "permission_mode": null, "agent": "rev", "claude_flags": [], "claude_user_options": []}));
        assert_eq!(ClaudeOpts::default().summary("Agent"), "");
        let o = ClaudeOpts {
            extra: ExtraOpts { flags: vec!["ide".into(), "chrome".into()], add_dirs: vec!["C:\\x".into()], max_budget_usd: "3".into(), append_system_prompt: "private text".into(), ..Default::default() },
            ..Default::default()
        };
        let j = o.to_json();
        assert_eq!(j["claude_flags"], json!(["chrome", "ide"]));
        if cfg!(windows) {
            assert_eq!(j["claude_user_options"], json!(["add_dir", "append_system_prompt", "max_budget_usd"]));
            assert_eq!(o.summary("Agent"), "--chrome --ide · add_dir, append_system_prompt, max_budget_usd");
        }
        assert!(!j.to_string().contains("private text"), "Werte bleiben beim Nutzer");
    }
}
