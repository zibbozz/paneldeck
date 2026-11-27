//! Pfadarten des Ziels (#392): Windows, WSL (je Distribution), Git Bash/MSYS und `~` – überall, wo ein Pfad
//! eingegeben, gewählt, abgelegt, eingefügt oder angeklickt wird, mit Umrechnung in beide Richtungen.
//!
//! * [`PathKind::of`] erkennt die Pfadart einer Shell (eine Stelle für alles, auch eigene Profile: `wsl.exe`, der
//!   WSL-Starter `bash.exe` aus System32, Git-Bash-/MSYS-`bash.exe`).
//! * [`PathKind::to_target`]: ein Pfad (Windows, UNC, Linux, `~`, Git Bash) so, wie ihn das Ziel versteht –
//!   `C:\x` → `/mnt/c/x` (WSL, Einhängepunkt je Distribution) bzw. `/c/x` (Git Bash); `\\wsl.localhost\<D>\x` →
//!   `/x` für dieselbe Distribution, eine andere Distribution ist ein Fehler ([`PathErr::OtherDistro`]); Linux-Pfade und
//!   `~` bleiben; für Windows-Ziele nur Linux-Pfade über `/mnt/<lw>` bzw. `/<lw>` und `~`.
//! * [`PathKind::to_windows`]: umgekehrt, für alles, was Windows öffnet (Explorer, Editor, Chat-Werkzeuge,
//!   Startordner): `/mnt/c/x` → `C:\x`, `/home/u` → `\\wsl.localhost\<D>\home\u` (mit 9P-Zeichen), `~/x` über das
//!   bekannte Home, Git Bash `/c/x` → `C:\x`.
//! * [`for_insert`]: Pfad zum Einfügen in ein Terminal (umgerechnet und für die laufende Shell gequotet).
//!
//! Nichts hier fragt WSL selbst ab: Home, Einhängepunkt und Standard-Distribution kommen über [`Lookup`] – im Programm
//! aus den Zwischenspeichern von `wsl` ([`Cached`], fehlt etwas, wird es im Hintergrund geholt), in Tests aus einem
//! Ersatz. Die Prüfung gegen Befehlseinschleusung (`wsl::validate_linux_path` vor `--cd`) bleibt beim Start.
//!
//! Wiederverwendbar für weitere Stellen (z. B. `${file}` einer Speichern-Aktion: `PathKind::of(&shell).for_insert`
//! bzw. `to_target`; „Pfad umrechnen“ einer Umgebungsvariable: `to_target`, bei `Err` unverändert lassen und melden).

use crate::shells::ShellSpec;
use crate::state::Lang;

/// Pfadart eines Ziels (Shell bzw. Sitzung).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathKind {
    /// Windows-Programm (PowerShell, cmd, andere). `cmd`: cmd.exe (kennt keinen UNC-Arbeitsordner).
    Windows { cmd: bool },
    /// WSL; `distro` = None: die Standard-Distribution (`wsl.exe` ohne `-d`, bzw. der WSL-Starter `bash.exe`).
    Wsl { distro: Option<String> },
    /// Git Bash / MSYS2: `/c/x` = `C:\x`; Cygwin (Installationsordner `cygwin…`): `/cygdrive/c/x`. `root` =
    /// Windows-Ordner der Installation (für `/usr/…`, `/etc/…`), wenn bekannt. Git Bash/MSYS: `/tmp` = `%TEMP%`,
    /// `/home/<u>` = Windows-Benutzerordner.
    Msys { root: Option<String> },
    /// Linux/macOS: Pfade bleiben, wie sie sind.
    Native,
}

/// Warum sich ein Pfad nicht für das Ziel umrechnen lässt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathErr {
    /// Der Pfad liegt in einer anderen WSL-Distribution (`path`) als die des Ziels (`target`).
    OtherDistro { path: String, target: String },
    /// WSL-Pfad, aber die Distribution des Ziels ist (noch) nicht bekannt.
    UnknownDistro,
    /// `~`, aber das Home-Verzeichnis ist (noch) nicht bekannt.
    HomeUnknown,
    /// Lässt sich für das Ziel nicht ausdrücken (Linux-Pfad für Windows, Netzwerkfreigabe für WSL …).
    NotConvertible,
}

impl PathErr {
    /// Meldung in der Sprache `lang`.
    pub fn message(&self, lang: Lang) -> String {
        let de = lang == Lang::De;
        match self {
            PathErr::OtherDistro { path, target } if de => format!("Der Pfad liegt in der WSL-Distribution {path}, das Ziel nutzt {target}."),
            PathErr::OtherDistro { path, target } => format!("The path is in the WSL distribution {path}, the target uses {target}."),
            PathErr::UnknownDistro => lang.t("Unklar, welche WSL-Distribution das Ziel nutzt.", "It is unclear which WSL distribution the target uses.").to_string(),
            PathErr::HomeUnknown => lang.t("Das Home-Verzeichnis ist noch nicht bekannt – bitte einen absoluten Pfad angeben oder kurz warten.", "The home folder is not known yet – enter an absolute path or wait a moment.").to_string(),
            PathErr::NotConvertible => lang.t("Dieser Pfad lässt sich für das Ziel nicht umrechnen.", "This path cannot be converted for the target.").to_string(),
        }
    }
}

/// Bekannte Angaben über WSL und Windows für die Umrechnung. Darf nie blockieren (wird im UI-Thread benutzt).
pub trait Lookup {
    /// Home-Verzeichnis des Standardnutzers der Distribution, wenn bekannt.
    fn wsl_home(&self, distro: &str) -> Option<String>;
    /// Einhängepunkt der Laufwerke (mit `/` am Ende), wenn bekannt; sonst gilt `/mnt/`.
    fn wsl_mount(&self, distro: &str) -> Option<String>;
    /// Echte Standard-Distribution, wenn bekannt.
    fn default_distro(&self) -> Option<String>;
    /// Windows-Benutzerordner (`USERPROFILE`).
    fn win_home(&self) -> String;
    /// Windows-Temp-Ordner (`TEMP`; Git Bash `/tmp`).
    fn win_temp(&self) -> String {
        std::env::var("TEMP").or_else(|_| std::env::var("TMP")).unwrap_or_default()
    }
}

/// Cygwin-Installation (Ordnername `cygwin…`)? Dann heißen Laufwerke `/cygdrive/c/…`.
fn is_cygwin(root: &Option<String>) -> bool {
    root.as_ref().and_then(|r| r.trim_end_matches('\\').rsplit('\\').next().map(|n| n.to_ascii_lowercase().starts_with("cygwin"))).unwrap_or(false)
}

/// Angaben aus den Zwischenspeichern von [`crate::wsl`]; fehlt etwas, wird es im Hintergrund geholt (nie in Tests).
pub struct Cached;

impl Lookup for Cached {
    fn wsl_home(&self, distro: &str) -> Option<String> {
        let h = crate::wsl::cached_home(distro);
        if h.is_none() {
            crate::wsl::prefetch(Some(distro));
        }
        h
    }
    fn wsl_mount(&self, distro: &str) -> Option<String> {
        let m = crate::wsl::cached_mount(distro);
        if m.is_none() {
            crate::wsl::prefetch(Some(distro));
        }
        m
    }
    fn default_distro(&self) -> Option<String> {
        let d = crate::wsl::cached_default();
        if d.is_none() {
            crate::wsl::prefetch(None);
        }
        d
    }
    fn win_home(&self) -> String {
        if cfg!(windows) { std::env::var("USERPROFILE").unwrap_or_default() } else { std::env::var("HOME").unwrap_or_default() }
    }
}

/// `~`, `~/x` bzw. `~\x` → Rest hinter `~` (`""` für `~` allein); sonst `None`.
fn home_rest(p: &str) -> Option<&str> {
    if p == "~" {
        return Some("");
    }
    p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\"))
}

/// UNC- oder Gerätepfad (`\\…`, `//…`)?
fn is_unc(p: &str) -> bool {
    p.starts_with("\\\\") || p.starts_with("//")
}

/// Windows-Pfad aus Benutzerordner und Rest (`a/b` → `<home>\a\b`).
fn win_join(home: &str, rest: &str) -> String {
    let parts: Vec<&str> = rest.split(['/', '\\']).filter(|c| !c.is_empty()).collect();
    if parts.is_empty() {
        home.to_string()
    } else {
        format!("{}\\{}", home.trim_end_matches(['\\', '/']), parts.join("\\"))
    }
}

/// Git-Bash-Laufwerkspfad (`/c/x`, `/c`, `/cygdrive/c/x`) → Windows-Pfad.
fn msys_drive(p: &str) -> Option<String> {
    let q = p.strip_prefix("/cygdrive").filter(|r| r.starts_with('/')).unwrap_or(p);
    crate::wsl::mounted_drive(q, "/").map(|(d, rest)| format!("{}:\\{}", d.to_ascii_uppercase(), rest.join("\\")))
}

/// `\\?\UNC\server\x` → `\\server\x` (andere Pfade unverändert).
fn plain_unc(p: &str) -> String {
    let s = p.replace('/', "\\");
    match s.get(..8) {
        Some(pre) if pre.eq_ignore_ascii_case(r"\\?\UNC\") => format!(r"\\{}", &s[8..]),
        _ => s,
    }
}

impl PathKind {
    /// Pfadart einer Shell. WSL auch für eigene Profile mit `wsl.exe` und für den WSL-Starter `bash.exe` aus System32;
    /// Git Bash (`…\Git\bin\bash.exe`) und andere POSIX-Shells unter Windows sind MSYS; cmd.exe eigens.
    pub fn of(shell: &ShellSpec) -> PathKind {
        if crate::shells::is_wsl(shell) {
            let d = crate::shells::wsl_distro_key(shell);
            return PathKind::Wsl { distro: (!d.is_empty()).then_some(d) };
        }
        if shell.id == "gitbash" {
            return PathKind::Msys { root: msys_root(&crate::shells::resolve_program(&shell.program).replace('/', "\\")) };
        }
        if !cfg!(windows) {
            return PathKind::Native;
        }
        // Ein bloßes `bash.exe` (eigenes Profil) über PATH auflösen: der WSL-Starter aus System32 ist oben schon WSL.
        let prog = crate::shells::resolve_program(&shell.program).replace('/', "\\");
        let file = prog.rsplit('\\').next().unwrap_or("").to_ascii_lowercase();
        let stem = file.strip_suffix(".exe").unwrap_or(&file);
        match stem {
            "cmd" => PathKind::Windows { cmd: true },
            "bash" | "sh" | "zsh" | "dash" | "fish" | "mintty" => PathKind::Msys { root: msys_root(&prog) },
            _ if shell.id == "gitbash" => PathKind::Msys { root: msys_root(&prog) },
            _ => PathKind::Windows { cmd: false },
        }
    }

    /// Pfadart nur aus der Kennung einer Shell (wo das Programm nicht bekannt ist): `wsl:<D>`/`wsl:` → WSL, `gitbash`
    /// → MSYS, `cmd` → cmd, sonst Windows (unter Linux/macOS: Native).
    pub fn of_id(shell_id: &str) -> PathKind {
        if let Some(d) = shell_id.strip_prefix("wsl:") {
            return PathKind::Wsl { distro: Some(d.to_string()).filter(|d| crate::wsl::valid_distro(d)) };
        }
        match shell_id {
            "wsl" => PathKind::Wsl { distro: None },
            "gitbash" => PathKind::Msys { root: None },
            _ if !cfg!(windows) => PathKind::Native,
            "cmd" => PathKind::Windows { cmd: true },
            _ => PathKind::Windows { cmd: false },
        }
    }

    /// Versteht das Ziel POSIX-Pfade (WSL, MSYS, Linux/macOS)?
    pub fn is_posix(&self) -> bool {
        !matches!(self, PathKind::Windows { .. })
    }

    /// Distribution einer WSL-Pfadart (die eigene, sonst die bekannte Standard-Distribution).
    pub fn distro_with(&self, lk: &dyn Lookup) -> Option<String> {
        match self {
            PathKind::Wsl { distro } => distro.clone().or_else(|| lk.default_distro()),
            _ => None,
        }
    }

    /// Pfad so, wie ihn das Ziel versteht (ohne Quoting), mit den bekannten Angaben aus den Zwischenspeichern.
    pub fn to_target(&self, p: &str) -> Result<String, PathErr> {
        self.to_target_with(p, &Cached)
    }

    /// Wie [`PathKind::to_target`] mit eigenen Angaben (Tests).
    pub fn to_target_with(&self, p: &str, lk: &dyn Lookup) -> Result<String, PathErr> {
        let p = p.trim();
        match self {
            PathKind::Native => Ok(p.to_string()),
            PathKind::Windows { .. } => {
                if crate::wsl::drive_parts(p).is_some() || is_unc(p) || !(p.starts_with('/') || p.starts_with('~')) {
                    return Ok(p.to_string());
                }
                self.to_windows_with(p, lk)
            }
            PathKind::Wsl { .. } => {
                if let Some((pd, linux)) = crate::wsl::unc_to_linux(p) {
                    return match self.distro_with(lk) {
                        Some(own) if own.eq_ignore_ascii_case(&pd) => Ok(linux),
                        Some(own) => Err(PathErr::OtherDistro { path: pd, target: own }),
                        None => Err(PathErr::UnknownDistro),
                    };
                }
                if crate::wsl::drive_parts(p).is_some() {
                    let mount = self.distro_with(lk).and_then(|d| lk.wsl_mount(&d)).unwrap_or_else(|| crate::wsl::DEFAULT_MOUNT.to_string());
                    return crate::wsl::windows_to_linux_at(p, &mount).ok_or(PathErr::NotConvertible);
                }
                if is_unc(p) {
                    return Err(PathErr::NotConvertible);
                }
                if crate::wsl::looks_linux(p) {
                    return Ok(p.to_string());
                }
                if let Some(rest) = p.strip_prefix("~\\") {
                    return Ok(format!("~/{}", rest.replace('\\', "/")));
                }
                // Relativer Pfad: wie er ist (ein `\` kann in Linux zum Namen gehören).
                Ok(p.to_string())
            }
            PathKind::Msys { root } => {
                if let Some((d, parts)) = crate::wsl::drive_parts(p) {
                    let pre = if is_cygwin(root) { "/cygdrive" } else { "" };
                    return Ok(if parts.is_empty() { format!("{pre}/{d}") } else { format!("{pre}/{d}/{}", parts.join("/")) });
                }
                if is_unc(p) {
                    // MSYS versteht UNC-Pfade mit Schrägstrichen (`//wsl.localhost/Ubuntu/x`, `//server/share`).
                    return Ok(plain_unc(p).replace('\\', "/"));
                }
                if let Some(rest) = p.strip_prefix("~\\") {
                    return Ok(format!("~/{}", rest.replace('\\', "/")));
                }
                Ok(p.to_string())
            }
        }
    }

    /// Pfad für Windows (Explorer, Editor, Startordner, Chat-Werkzeuge), mit den Angaben aus den Zwischenspeichern.
    /// Windows- und UNC-Pfade bleiben, wie sie sind.
    pub fn to_windows(&self, p: &str) -> Result<String, PathErr> {
        self.to_windows_with(p, &Cached)
    }

    /// Wie [`PathKind::to_windows`] mit eigenen Angaben (Tests).
    pub fn to_windows_with(&self, p: &str, lk: &dyn Lookup) -> Result<String, PathErr> {
        let p = p.trim();
        if let PathKind::Native = self {
            return Ok(match home_rest(p) {
                Some(rest) => {
                    let h = lk.win_home();
                    if rest.is_empty() { h } else { format!("{}/{rest}", h.trim_end_matches('/')) }
                }
                None => p.to_string(),
            });
        }
        if crate::wsl::drive_parts(p).is_some() || is_unc(p) {
            return Ok(p.to_string());
        }
        match self {
            PathKind::Native => unreachable!(),
            PathKind::Windows { .. } => {
                if let Some(rest) = home_rest(p) {
                    return Ok(win_join(&lk.win_home(), rest));
                }
                if p.starts_with('/') {
                    // Nur `/mnt/<lw>/…` (WSL-Schreibweise); `/c/…` gilt nur für Git Bash/MSYS/Cygwin.
                    if let Some((d, rest)) = crate::wsl::mounted_drive(p, crate::wsl::DEFAULT_MOUNT) {
                        return Ok(format!("{}:\\{}", d.to_ascii_uppercase(), rest.join("\\")));
                    }
                    return Err(PathErr::NotConvertible);
                }
                Ok(p.to_string())
            }
            PathKind::Msys { root } => {
                if let Some(rest) = home_rest(p) {
                    return Ok(win_join(&lk.win_home(), rest));
                }
                if p.starts_with('/') {
                    let cyg = is_cygwin(root);
                    // Cygwin: Laufwerke nur unter `/cygdrive`; `/c/…` ist dort ein Ordner der Installation.
                    if let Some(w) = msys_drive(p).filter(|_| !cyg || p.starts_with("/cygdrive/")) {
                        return Ok(w);
                    }
                    if !cyg {
                        // Git for Windows/MSYS2: `/tmp` = `%TEMP%`, `/home/<u>` = Windows-Benutzerordner.
                        let parts: Vec<&str> = p.split('/').filter(|c| !c.is_empty()).collect();
                        match parts.as_slice() {
                            ["tmp", rest @ ..] if !lk.win_temp().is_empty() => return Ok(win_join(&lk.win_temp(), &rest.join("/"))),
                            ["home", _, rest @ ..] => return Ok(win_join(&lk.win_home(), &rest.join("/"))),
                            _ => {}
                        }
                    }
                    return match root {
                        Some(r) => Ok(win_join(r, p)),
                        None => Err(PathErr::NotConvertible),
                    };
                }
                Ok(p.to_string())
            }
            PathKind::Wsl { .. } => {
                let d = self.distro_with(lk);
                let full = match home_rest(p) {
                    Some(rest) => {
                        let d = d.as_deref().ok_or(PathErr::UnknownDistro)?;
                        let h = lk.wsl_home(d).ok_or(PathErr::HomeUnknown)?;
                        let rest = rest.replace('\\', "/");
                        if rest.is_empty() { h } else { format!("{}/{rest}", h.trim_end_matches('/')) }
                    }
                    None if p.starts_with('/') => p.to_string(),
                    None => return Err(PathErr::NotConvertible),
                };
                let mount = d.as_deref().and_then(|d| lk.wsl_mount(d)).unwrap_or_else(|| crate::wsl::DEFAULT_MOUNT.to_string());
                if let Some(w) = crate::wsl::linux_to_windows_at("", &full, &mount) {
                    return Ok(w);
                }
                let d = d.ok_or(PathErr::UnknownDistro)?;
                Ok(crate::wsl::linux_to_unc(&d, &full))
            }
        }
    }
}

/// Windows-Ordner einer MSYS-Installation aus dem Programm (`…\Git\bin\bash.exe`, `…\Git\usr\bin\bash.exe`,
/// `…\msys64\usr\bin\bash.exe` → `…\Git` bzw. `…\msys64`).
fn msys_root(program: &str) -> Option<String> {
    let parts: Vec<&str> = program.split('\\').collect();
    if parts.len() < 3 || crate::wsl::drive_parts(program).is_none() {
        return None;
    }
    let dir = &parts[..parts.len() - 1];
    let n = dir.len();
    let cut = if n >= 2 && dir[n - 1].eq_ignore_ascii_case("bin") && dir[n - 2].eq_ignore_ascii_case("usr") {
        n - 2
    } else if dir[n - 1].eq_ignore_ascii_case("bin") {
        n - 1
    } else {
        return None;
    };
    (cut >= 1).then(|| dir[..cut].join("\\"))
}

/// Pfad zum Einfügen in das Terminal der Shell: für das Ziel umgerechnet ([`PathKind::to_target`]) und für die
/// laufende Shell gequotet (`shells::quote_kind`: fish anders als bash, #57). Ein Pfad einer anderen
/// WSL-Distribution wird nicht eingefügt (`Err`).
pub fn for_insert(shell: &ShellSpec, p: &str) -> Result<String, PathErr> {
    for_insert_with(shell, p, &Cached)
}

/// Wie [`for_insert`] mit eigenen Angaben (Tests).
pub fn for_insert_with(shell: &ShellSpec, p: &str, lk: &dyn Lookup) -> Result<String, PathErr> {
    let t = PathKind::of(shell).to_target_with(p, lk)?;
    // 9P-Zeichen (U+F001…) werden zu Steuerzeichen zurückgebildet: so einen Pfad nie ins Terminal tippen (#392).
    if t.chars().any(char::is_control) {
        return Err(PathErr::NotConvertible);
    }
    Ok(crate::quote::quote_if_needed(crate::shells::quote_kind(shell), &t))
}

/// Kurzer Hinweis unter einem Pfadfeld: wohin ein eingegebener Pfad für das Ziel führt (`~`, Git Bash `/c/x`,
/// `/mnt/c/x` für Windows/MSYS-Ziele), bzw. warum er dort nicht geht. `None`: nichts umzurechnen.
pub fn field_hint(kind: &PathKind, text: &str, lk: &dyn Lookup) -> Option<Result<String, PathErr>> {
    let t = text.trim();
    if t.is_empty() || !matches!(kind, PathKind::Windows { .. } | PathKind::Msys { .. }) {
        return None;
    }
    if !(t.starts_with('/') || t.starts_with('~')) {
        return None;
    }
    Some(kind.to_windows_with(t, lk))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Schein-Angaben: Ubuntu (Home /home/u, Einhängepunkt /mnt/), Arch (Home /home/a, Einhängepunkt /),
    /// Standard-Distribution Ubuntu, Windows-Home C:\Users\u.
    struct Fake {
        default: Option<&'static str>,
    }

    impl Lookup for Fake {
        fn wsl_home(&self, d: &str) -> Option<String> {
            match d.to_ascii_lowercase().as_str() {
                "ubuntu" => Some("/home/u".into()),
                "arch" => Some("/home/a".into()),
                _ => None,
            }
        }
        fn wsl_mount(&self, d: &str) -> Option<String> {
            match d.to_ascii_lowercase().as_str() {
                "arch" => Some("/".into()),
                "ubuntu" => Some("/mnt/".into()),
                _ => None,
            }
        }
        fn default_distro(&self) -> Option<String> {
            self.default.map(str::to_string)
        }
        fn win_home(&self) -> String {
            r"C:\Users\u".into()
        }
        fn win_temp(&self) -> String {
            r"C:\Users\u\AppData\Local\Temp".into()
        }
    }

    const LK: Fake = Fake { default: Some("Ubuntu") };
    const NO_DEFAULT: Fake = Fake { default: None };

    fn sp(id: &str, program: &str, args: &[&str]) -> ShellSpec {
        ShellSpec { id: id.into(), label: id.into(), program: program.into(), args: args.iter().map(|s| s.to_string()).collect() }
    }

    fn wsl(d: &str) -> PathKind {
        PathKind::Wsl { distro: Some(d.into()) }
    }

    fn win() -> PathKind {
        PathKind::Windows { cmd: false }
    }

    fn msys() -> PathKind {
        PathKind::Msys { root: Some(r"C:\Program Files\Git".into()) }
    }

    #[test]
    fn kind_of_every_shell() {
        // WSL und Git Bash nach Kennung/Programm auf jeder Plattform; der Rest nur unter Windows.
        assert_eq!(PathKind::of_id("wsl:Debian"), wsl("Debian"));
        assert_eq!(PathKind::of_id("wsl:"), PathKind::Wsl { distro: None }, "leere Distribution = Standard");
        assert_eq!(PathKind::of_id("wsl:-bad"), PathKind::Wsl { distro: None });
        assert_eq!(PathKind::of_id("gitbash"), PathKind::Msys { root: None });
        if !cfg!(windows) {
            assert_eq!(PathKind::of(&sp("bash", "/bin/bash", &[])), PathKind::Native);
            assert_eq!(PathKind::of_id("bash"), PathKind::Native);
            return;
        }
        assert_eq!(PathKind::of(&sp("wsl:Ubuntu", "wsl.exe", &["-d", "Ubuntu"])), wsl("Ubuntu"));
        assert_eq!(PathKind::of(&sp("custom:3", r"C:\Windows\System32\wsl.exe", &["-d", "Arch"])), wsl("Arch"), "eigenes Profil mit wsl.exe");
        assert_eq!(PathKind::of(&sp("custom:4", "wsl", &[])), PathKind::Wsl { distro: None });
        assert_eq!(PathKind::of(&sp("custom:5", "C:/Windows/System32/bash.exe", &[])), PathKind::Wsl { distro: None }, "WSL-Starter bash.exe");
        assert_eq!(PathKind::of(&sp("gitbash", r"C:\Program Files\Git\bin\bash.exe", &["--login", "-i"])), msys());
        assert_eq!(PathKind::of(&sp("custom:6", r"D:\Tools\Git\usr\bin\bash.exe", &[])), PathKind::Msys { root: Some(r"D:\Tools\Git".into()) }, "eigenes Git-Bash-Profil");
        assert_eq!(PathKind::of(&sp("custom:7", r"C:\msys64\usr\bin\zsh.exe", &[])), PathKind::Msys { root: Some(r"C:\msys64".into()) });
        // Bloßes bash.exe: über PATH aufgelöst – WSL-Starter (System32) oder Git Bash, nie eine Windows-Shell
        assert!(matches!(PathKind::of(&sp("custom:8", "bash.exe", &[])), PathKind::Msys { .. } | PathKind::Wsl { .. }));
        assert_eq!(crate::shells::resolve_program("pd-gibt-es-nicht-bash"), "pd-gibt-es-nicht-bash");
        assert_eq!(PathKind::of(&sp("custom:8", "pd-gibt-es-nicht-bash.exe", &[])), win());
        assert_eq!(PathKind::of(&sp("cmd", "cmd.exe", &[])), PathKind::Windows { cmd: true });
        assert_eq!(PathKind::of(&sp("pwsh", r"C:\Program Files\PowerShell\7\pwsh.exe", &[])), win());
        assert_eq!(PathKind::of(&sp("custom:9", "ssh.exe", &[])), win());
        assert_eq!(PathKind::of_id("cmd"), PathKind::Windows { cmd: true });
        assert_eq!(PathKind::of_id("powershell"), win());
        assert!(wsl("x").is_posix() && msys().is_posix() && !win().is_posix());
    }

    /// Die ganze Tabelle: jede Pfadform × jedes Ziel.
    #[test]
    fn conversion_table_to_target() {
        let ub = wsl("Ubuntu");
        let arch = wsl("Arch");
        let def = PathKind::Wsl { distro: None };
        let t = |k: &PathKind, p: &str| k.to_target_with(p, &LK);
        // Windows-Pfad
        assert_eq!(t(&ub, r"C:\Users\Max\a b.txt"), Ok("/mnt/c/Users/Max/a b.txt".into()));
        assert_eq!(t(&arch, r"C:\Users\Max"), Ok("/c/Users/Max".into()), "eigener Einhängepunkt der Distribution");
        assert_eq!(t(&def, r"D:\"), Ok("/mnt/d".into()));
        assert_eq!(t(&msys(), r"D:\Dev\my app\f.rs"), Ok("/d/Dev/my app/f.rs".into()));
        assert_eq!(t(&msys(), "E:"), Ok("/e".into()));
        assert_eq!(t(&win(), r"C:\x"), Ok(r"C:\x".into()));
        assert_eq!(t(&PathKind::Native, "/home/x"), Ok("/home/x".into()));
        // \\?\-Pfade
        assert_eq!(t(&ub, r"\\?\C:\x\y"), Ok("/mnt/c/x/y".into()));
        assert_eq!(t(&msys(), r"\\?\C:\x"), Ok("/c/x".into()));
        // UNC beider Präfixe, \\?\UNC, gleiche und andere Distribution, 9P-Zeichen
        assert_eq!(t(&ub, r"\\wsl.localhost\Ubuntu\home\u\p"), Ok("/home/u/p".into()));
        assert_eq!(t(&ub, r"\\wsl$\ubuntu\srv\x"), Ok("/srv/x".into()), "Groß-/Kleinschreibung der Distribution egal");
        assert_eq!(t(&ub, r"\\?\UNC\wsl.localhost\Ubuntu\tmp"), Ok("/tmp".into()));
        assert_eq!(t(&ub, "\\\\wsl.localhost\\Ubuntu\\a\u{F03A}b"), Ok("/a:b".into()), "PUA zurück");
        assert_eq!(t(&def, r"\\wsl.localhost\Ubuntu\x"), Ok("/x".into()), "Standard-Distribution bekannt");
        assert_eq!(def.to_target_with(r"\\wsl.localhost\Ubuntu\x", &NO_DEFAULT), Err(PathErr::UnknownDistro));
        assert_eq!(t(&ub, r"\\wsl.localhost\Debian\srv"), Err(PathErr::OtherDistro { path: "Debian".into(), target: "Ubuntu".into() }));
        assert_eq!(t(&msys(), r"\\wsl.localhost\Ubuntu\home\u"), Ok("//wsl.localhost/Ubuntu/home/u".into()), "Git Bash: UNC mit /");
        assert_eq!(t(&msys(), r"\\?\UNC\server\share\x"), Ok("//server/share/x".into()));
        assert_eq!(t(&win(), r"\\wsl.localhost\Ubuntu\x"), Ok(r"\\wsl.localhost\Ubuntu\x".into()));
        assert_eq!(t(&ub, r"\\server\share\x"), Err(PathErr::NotConvertible), "Netzwerkfreigabe gibt es in WSL nicht");
        // Linux-Pfade und ~ bleiben für POSIX-Ziele
        for p in ["/home/u/a b", "~", "~/x", "/mnt/c/x"] {
            assert_eq!(t(&ub, p), Ok(p.to_string()), "{p}");
            assert_eq!(t(&msys(), p), Ok(p.to_string()), "{p}");
        }
        assert_eq!(t(&ub, r"~\x\y"), Ok("~/x/y".into()));
        // Windows-Ziel: Linux nur über /mnt/<lw> und /<lw>, ~ = Windows-Home
        assert_eq!(t(&win(), "/mnt/c/Users/u"), Ok(r"C:\Users\u".into()));
        assert_eq!(t(&win(), "/d/x"), Err(PathErr::NotConvertible), "/<lw>/ nur für Git Bash/MSYS/Cygwin");
        // Cygwin: /cygdrive/<lw>
        let cyg = PathKind::Msys { root: Some(r"C:\cygwin64".into()) };
        assert_eq!(t(&cyg, r"C:\Users\u\x"), Ok("/cygdrive/c/Users/u/x".into()));
        assert_eq!(t(&cyg, "E:"), Ok("/cygdrive/e".into()));
        assert_eq!(t(&win(), "~"), Ok(r"C:\Users\u".into()));
        assert_eq!(t(&win(), "~/projects/a"), Ok(r"C:\Users\u\projects\a".into()));
        assert_eq!(t(&win(), "/home/u"), Err(PathErr::NotConvertible));
        assert_eq!(t(&win(), "relative\\x"), Ok("relative\\x".into()));
        // Git-Bash-Pfad an WSL: ist ein gültiger Linux-Pfad und bleibt
        assert_eq!(t(&ub, "/c/x"), Ok("/c/x".into()));
    }

    #[test]
    fn conversion_table_to_windows() {
        let ub = wsl("Ubuntu");
        let w = |k: &PathKind, p: &str| k.to_windows_with(p, &LK);
        assert_eq!(w(&ub, "/mnt/c/Users/Max/a b"), Ok(r"C:\Users\Max\a b".into()));
        assert_eq!(w(&ub, "/mnt/d"), Ok(r"D:\".into()));
        assert_eq!(w(&ub, "/home/u/p q"), Ok(r"\\wsl.localhost\Ubuntu\home\u\p q".into()), "wsl.localhost, nicht wsl$");
        assert_eq!(w(&ub, "/x/a:b"), Ok("\\\\wsl.localhost\\Ubuntu\\x\\a\u{F03A}b".into()), "PUA hin");
        assert_eq!(w(&ub, "~"), Ok(r"\\wsl.localhost\Ubuntu\home\u".into()));
        assert_eq!(w(&ub, "~/src/x.rs"), Ok(r"\\wsl.localhost\Ubuntu\home\u\src\x.rs".into()));
        assert_eq!(w(&wsl("Arch"), "/c/x"), Ok(r"C:\x".into()), "eigener Einhängepunkt");
        assert_eq!(w(&wsl("Arch"), "/mnt/c/x"), Ok(r"\\wsl.localhost\Arch\mnt\c\x".into()));
        assert_eq!(w(&wsl("Nohome"), "~"), Err(PathErr::HomeUnknown));
        // Leere Distribution (`wsl:`): die bekannte Standard-Distribution, sonst nur /mnt/<lw>
        let def = PathKind::Wsl { distro: None };
        assert_eq!(w(&def, "/home/u"), Ok(r"\\wsl.localhost\Ubuntu\home\u".into()));
        assert_eq!(def.to_windows_with("/home/u", &NO_DEFAULT), Err(PathErr::UnknownDistro));
        assert_eq!(def.to_windows_with("/mnt/e/x", &NO_DEFAULT), Ok(r"E:\x".into()));
        assert_eq!(w(&ub, "relative/x"), Err(PathErr::NotConvertible));
        // Windows-/UNC-Pfade bleiben
        for p in [r"C:\x", r"\\wsl.localhost\Debian\srv", r"\\server\s"] {
            assert_eq!(w(&ub, p), Ok(p.to_string()));
            assert_eq!(w(&msys(), p), Ok(p.to_string()));
            assert_eq!(w(&win(), p), Ok(p.to_string()));
        }
        // Git Bash
        assert_eq!(w(&msys(), "/c/Users/u/x"), Ok(r"C:\Users\u\x".into()));
        assert_eq!(w(&msys(), "/c"), Ok(r"C:\".into()));
        assert_eq!(w(&msys(), "/cygdrive/d/a"), Ok(r"D:\a".into()));
        assert_eq!(w(&msys(), "~"), Ok(r"C:\Users\u".into()));
        assert_eq!(w(&msys(), "~/repo"), Ok(r"C:\Users\u\repo".into()));
        assert_eq!(w(&msys(), "/usr/bin"), Ok(r"C:\Program Files\Git\usr\bin".into()));
        assert_eq!(PathKind::Msys { root: None }.to_windows_with("/usr/bin", &LK), Err(PathErr::NotConvertible));
        // Windows-Ziel
        assert_eq!(w(&win(), "~"), Ok(r"C:\Users\u".into()));
        assert_eq!(w(&win(), r"~\a"), Ok(r"C:\Users\u\a".into()));
        assert_eq!(w(&win(), "/mnt/c/x"), Ok(r"C:\x".into()));
        assert_eq!(w(&win(), "/c/x"), Err(PathErr::NotConvertible), "/<lw>/ nur für Git Bash/MSYS/Cygwin");
        // Git Bash: /tmp = %TEMP%, /home/<u> = Windows-Home, /etc über den Installationsordner
        assert_eq!(w(&msys(), "/tmp/a.log"), Ok(r"C:\Users\u\AppData\Local\Temp\a.log".into()));
        assert_eq!(w(&msys(), "/tmp"), Ok(r"C:\Users\u\AppData\Local\Temp".into()));
        assert_eq!(w(&msys(), "/home/u/repo"), Ok(r"C:\Users\u\repo".into()));
        assert_eq!(w(&msys(), "/etc/profile"), Ok(r"C:\Program Files\Git\etc\profile".into()));
        // Cygwin: Laufwerke nur unter /cygdrive, /tmp und /home liegen in der Installation
        let cyg = PathKind::Msys { root: Some(r"C:\cygwin64".into()) };
        assert_eq!(w(&cyg, "/cygdrive/c/x"), Ok(r"C:\x".into()));
        assert_eq!(w(&cyg, "/c/x"), Ok(r"C:\cygwin64\c\x".into()));
        assert_eq!(w(&cyg, "/tmp/a"), Ok(r"C:\cygwin64\tmp\a".into()));
        assert_eq!(w(&cyg, "/home/u"), Ok(r"C:\cygwin64\home\u".into()));
        assert_eq!(w(&win(), "/home/u"), Err(PathErr::NotConvertible));
        // Hin und zurück
        for p in ["/home/u/a b", "/srv/ä-ö", "/x/a:b|c"] {
            let unc = w(&ub, p).unwrap();
            assert_eq!(ub.to_target_with(&unc, &LK), Ok(p.to_string()));
        }
        for p in [r"C:\Users\Max", r"Z:\a b\c"] {
            assert_eq!(w(&ub, &ub.to_target_with(p, &LK).unwrap()), Ok(p.to_string()));
            assert_eq!(w(&msys(), &msys().to_target_with(p, &LK).unwrap()), Ok(p.to_string()));
        }
    }

    /// Eingefügte Pfade können in keiner Shell einen Befehl einschleusen (`"`, `\`, `'`, `;` …).
    #[test]
    fn insert_is_converted_and_quoted_without_injection() {
        let ub = sp("wsl:pf-ins-u", "wsl.exe", &["-d", "pf-ins-u"]);
        let gb = sp("gitbash", r"C:\Program Files\Git\bin\bash.exe", &["--login", "-i"]);
        let pw = sp("pwsh", "pwsh.exe", &[]);
        let cm = sp("cmd", "cmd.exe", &[]);
        struct One;
        impl Lookup for One {
            fn wsl_home(&self, _d: &str) -> Option<String> {
                None
            }
            fn wsl_mount(&self, _d: &str) -> Option<String> {
                None
            }
            fn default_distro(&self) -> Option<String> {
                None
            }
            fn win_home(&self) -> String {
                r"C:\Users\u".into()
            }
        }
        let i = |s: &ShellSpec, p: &str| for_insert_with(s, p, &One);
        assert_eq!(i(&ub, r"C:\Users\Max\a.png"), Ok("/mnt/c/Users/Max/a.png".into()));
        assert_eq!(i(&ub, r"\\wsl.localhost\pf-ins-u\home\u\my file"), Ok("'/home/u/my file'".into()), "UNC derselben Distribution → Linux-Pfad");
        assert!(matches!(i(&ub, r"\\wsl.localhost\Other\x"), Err(PathErr::OtherDistro { .. })), "andere Distribution: nichts einfügen");
        assert_eq!(i(&gb, r"D:\Dev\my app\f.rs"), Ok("'/d/Dev/my app/f.rs'".into()));
        assert_eq!(i(&gb, r"\\wsl.localhost\U\home\x"), Ok("//wsl.localhost/U/home/x".into()));
        assert_eq!(i(&pw, r"C:\Users\Max\a.png"), Ok(r"C:\Users\Max\a.png".into()));
        assert_eq!(i(&cm, r"C:\Program Files\x.txt"), Ok("\"C:\\Program Files\\x.txt\"".into()));
        // Anführungszeichen und Backslash im Namen (unter Windows nur über 9P-Zeichen bzw. in Linux-Namen möglich)
        let evil = "\\\\wsl.localhost\\pf-ins-u\\tmp\\x\u{F022}; touch pwned; #\u{F05C}";
        let q = i(&ub, evil).unwrap();
        assert!(q.starts_with('\'') && q.ends_with('\''), "{q}");
        assert!(q.contains("x\"; touch pwned; #\\"), "{q}");
        let q = i(&gb, r"C:\x\';touch pwned;#").unwrap();
        assert_eq!(q, r"'/c/x/'\'';touch pwned;#'");
        // 9P-Zeichen für Steuerzeichen (U+F00A → Zeilenumbruch, U+F01B → Escape): nie eintippen
        assert_eq!(i(&ub, "\\\\wsl.localhost\\pf-ins-u\\tmp\\a\u{F00A}rm -rf ~"), Err(PathErr::NotConvertible));
        assert_eq!(i(&ub, "\\\\wsl.localhost\\pf-ins-u\\tmp\\a\u{F01B}[2J"), Err(PathErr::NotConvertible));
        assert_eq!(i(&ub, "/tmp/a\nb"), Err(PathErr::NotConvertible));
    }

    #[test]
    fn field_hints_for_windows_and_git_bash_targets() {
        assert_eq!(field_hint(&win(), "~/x", &LK), Some(Ok(r"C:\Users\u\x".into())));
        assert_eq!(field_hint(&msys(), "/c/repo", &LK), Some(Ok(r"C:\repo".into())));
        assert_eq!(field_hint(&win(), "/home/u", &LK), Some(Err(PathErr::NotConvertible)));
        assert_eq!(field_hint(&win(), r"C:\x", &LK), None, "nichts umzurechnen");
        assert_eq!(field_hint(&wsl("Ubuntu"), "~", &LK), None, "WSL hat seine eigene Statuszeile");
        assert!(PathErr::OtherDistro { path: "A".into(), target: "B".into() }.message(Lang::De).contains("A"));
        assert!(PathErr::HomeUnknown.message(Lang::En).contains("home"));
    }

    #[test]
    fn msys_roots() {
        assert_eq!(msys_root(r"C:\Program Files\Git\bin\bash.exe").as_deref(), Some(r"C:\Program Files\Git"));
        assert_eq!(msys_root(r"C:\Program Files\Git\usr\bin\bash.exe").as_deref(), Some(r"C:\Program Files\Git"));
        assert_eq!(msys_root(r"C:\msys64\usr\bin\bash.exe").as_deref(), Some(r"C:\msys64"));
        assert_eq!(msys_root("bash.exe"), None);
        assert_eq!(msys_root(r"C:\tools\bash.exe"), None);
    }
}
