//! Pfad-Sicherheit der Agenten-Werkzeuge (#338).
//!
//! * Alles bleibt im Projektordner des Chats: Pfade werden lexikalisch aufgelöst (`..`) und dann kanonisiert
//!   (Symlinks, Junctions); das echte Ziel muss im kanonischen Projektordner liegen. Beim Öffnen wird noch einmal
//!   geprüft (Windows: der endgültige Pfad des Handles, sonst Gerät und Inode), damit ein zwischendurch ausgetauschter
//!   Ordner nicht hinausführt.
//! * Abgelehnt: UNC- und Gerätepfade (`\\server\share`, `\\?\…`, `\\.\…`, `CON`, `NUL` …), alternative Datenströme
//!   (`datei:strom`), laufwerksrelative Pfade (`C:datei`, `\datei`), Abschnitte mit Punkt oder Leerzeichen am Ende,
//!   `~` (wird nicht aufgelöst) und ein Laufwerks-Stammordner als Projektordner.
//! * Gesperrt, auch mit Freigabe: der Datenordner von Paneldeck und Geheimnis-Ordner des Benutzers (`~/.ssh`,
//!   `~/.claude`, `~/.claude.json`, `~/.aws`, `AppData` …); Schreiben in `.git`.
//! * Sensibel (Lesen nur mit Freigabe, Liste in den Einstellungen änderbar): `.env`, Schlüssel, Zertifikate …

use super::glob::glob_match;
use super::{err, AgentEnv, ToolErr};
use std::path::{Component, Path, PathBuf};

/// Wozu ein Pfad aufgelöst wird (Schreiben ist strenger: nie in `.git`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

/// Kanonischer Projektordner eines Chats.
#[derive(Clone, Debug)]
pub struct Root {
    pub canon: PathBuf,
}

/// Ein geprüfter Pfad im Projektordner.
#[derive(Clone, Debug)]
pub struct Resolved {
    /// Kanonischer Pfad (vorhandener Teil kanonisiert, der Rest angehängt).
    pub path: PathBuf,
    /// Relativ zum Projektordner, mit `/` getrennt (`.` = der Projektordner selbst).
    pub rel: String,
    pub exists: bool,
    pub is_dir: bool,
}

/// Standardliste sensibler Dateien (Lesen nur mit Freigabe). Muster ohne `/` gelten für den Dateinamen, mit `/` für
/// den Pfad relativ zum Projektordner; Groß-/Kleinschreibung egal.
pub const DEFAULT_SENSITIVE: &[&str] = &[
    ".env",
    ".env.*",
    "*.env",
    "*.pem",
    "*.key",
    "*.p12",
    "*.pfx",
    "*.jks",
    "*.keystore",
    "*.kdbx",
    "*.ppk",
    "id_rsa*",
    "id_dsa*",
    "id_ecdsa*",
    "id_ed25519*",
    "credentials",
    "credentials.*",
    "secrets.*",
    "*.secret",
    ".npmrc",
    ".pypirc",
    ".htpasswd",
    "*.tfstate",
    "*.tfstate.*",
    "*.tfvars",
];

/// Ordnernamen, die überall gesperrt sind (auch mitten im Projekt).
const BLOCKED_NAMES: &[&str] = &[".ssh", ".gnupg"];

/// Dateinamen, die überall gesperrt sind (Zugangsdaten von Claude Code und git, auch außerhalb des Benutzerordners,
/// z. B. in WSL oder einem kopierten Profil; #392).
const BLOCKED_FILES: &[&str] = &[".credentials.json", ".claude.json", ".git-credentials"];

/// Unter dem Benutzerordner gesperrt (relativ zu ihm).
const BLOCKED_HOME: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".claude",
    ".claude.json",
    ".aws",
    ".azure",
    ".kube",
    ".docker",
    ".config/gh",
    ".config/gcloud",
    ".netrc",
    "_netrc",
    ".git-credentials",
    ".password-store",
    ".local/share/keyrings",
    ".mozilla",
    ".config/google-chrome",
    ".config/chromium",
    "AppData",
    "Library/Keychains",
    "Library/Cookies",
    "Library/Application Support",
];

/// Ausnahmen in [`BLOCKED_HOME`]: der Temp-Ordner unter `AppData` (dort liegen auch entpackte Projekte).
const ALLOWED_HOME: &[&str] = &["AppData/Local/Temp"];

/// Benutzerordner (`USERPROFILE` bzw. `HOME`), kanonisch, wenn möglich.
pub fn home_dir() -> Option<PathBuf> {
    let v = if cfg!(windows) { std::env::var_os("USERPROFILE") } else { std::env::var_os("HOME") };
    let p = PathBuf::from(v.filter(|v| !v.is_empty())?);
    Some(std::fs::canonicalize(&p).unwrap_or(p))
}

/// Projektordner des Chats prüfen.
pub fn root(env: &AgentEnv) -> Result<Root, ToolErr> {
    let Some(r) = env.root.as_ref().filter(|r| !r.as_os_str().is_empty()) else {
        return Err(err("no_project_folder", "This chat has no project folder, so file, search and git tools are not available. Ask the user to start the chat in a project folder."));
    };
    let canon = std::fs::canonicalize(r).map_err(|e| err("no_project_folder", format!("The chat's project folder {} is not accessible ({e}).", r.display())))?;
    if !canon.is_dir() {
        return Err(err("no_project_folder", format!("The chat's project folder {} is not a folder.", r.display())));
    }
    if canon.parent().is_none() {
        return Err(err("unsafe_project_folder", format!("The chat's project folder {} is a drive root; file tools only work inside a project folder.", r.display())));
    }
    // WSL (#392): Ein Ordner, der einen Laufwerks-Einhängepunkt enthält (`/`, `/mnt`, `/mnt/c` …), öffnete über
    // `/mnt/c/…` die ganze Windows-Platte – wie ein Laufwerks-Stammordner ablehnen.
    if wsl_root_contains_mount(&canon) {
        return Err(err("unsafe_project_folder", format!("The chat's project folder {} contains the Windows drives of WSL (/mnt …); file tools only work inside a project folder.", r.display())));
    }
    if let Some(why) = blocked(&canon, &[], Access::Read, env) {
        return Err(err("blocked", format!("The chat's project folder is off-limits: {why}.")));
    }
    Ok(Root { canon })
}

/// Lexikalische Probleme eines eingegebenen Pfads (vor jedem Dateizugriff). `None` = in Ordnung.
pub fn lexical_problem(input: &str) -> Option<String> {
    if input.contains('\0') {
        return Some("the path contains a NUL character".into());
    }
    if input.starts_with('~') {
        return Some("'~' is not expanded; use a path relative to the project folder, e.g. src/main.rs".into());
    }
    if cfg!(windows) {
        if input.starts_with("\\\\") || input.starts_with("//") || input.starts_with("\\/") || input.starts_with("/\\") {
            return Some("network (UNC) and device paths such as \\\\server\\share or \\\\?\\C:\\ are not allowed; use a path relative to the project folder".into());
        }
        let b = input.as_bytes();
        for (i, c) in input.char_indices() {
            if c == ':' && !(i == 1 && b[0].is_ascii_alphabetic()) {
                return Some("':' is only allowed after a drive letter (alternate data streams like file:stream are not allowed)".into());
            }
        }
        if b.len() >= 2 && b[1] == b':' && !matches!(b.get(2), Some(b'\\') | Some(b'/')) {
            return Some("drive-relative paths like C:file are not allowed; use C:\\full\\path or a path relative to the project folder".into());
        }
        if matches!(b.first(), Some(b'\\') | Some(b'/')) {
            return Some("a path starting with \\ or / has no drive; use a path relative to the project folder, e.g. src/main.rs".into());
        }
        for part in input.split(['\\', '/']) {
            if part.is_empty() || part == "." || part == ".." {
                continue;
            }
            if part.ends_with('.') || part.ends_with(' ') {
                return Some(format!("the path part “{}” ends with a dot or a space (Windows would silently drop it)", crate::mcp::short(part, 60)));
            }
            let base = part.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
            let device = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" | "CLOCK$")
                || ((base.starts_with("COM") || base.starts_with("LPT")) && base.len() == 4 && base.as_bytes()[3].is_ascii_digit() && base.as_bytes()[3] != b'0');
            if device {
                return Some(format!("“{}” is a reserved device name on Windows", crate::mcp::short(part, 60)));
            }
        }
    }
    None
}

/// Pfad prüfen und auflösen. `input` ist relativ zum Projektordner oder absolut (dann muss er darin liegen).
pub fn resolve(root: &Root, env: &AgentEnv, input: &str, access: Access) -> Result<Resolved, ToolErr> {
    let trimmed = input.trim();
    let trimmed = if trimmed.is_empty() { "." } else { trimmed };
    if let Some(p) = lexical_problem(trimmed) {
        return Err(err("bad_path", format!("Invalid path “{}”: {p}.", crate::mcp::short(trimmed, 200))));
    }
    let p = Path::new(trimmed);
    // Lexikalisch auflösen: Abschnitte einzeln anhängen (ein kanonischer Windows-Pfad mit `\\?\` nimmt `/` sonst wörtlich).
    let mut cand = if p.is_absolute() { PathBuf::new() } else { root.canon.clone() };
    for c in p.components() {
        match c {
            Component::Prefix(pre) => cand.push(pre.as_os_str()),
            Component::RootDir => cand.push(std::path::MAIN_SEPARATOR_STR),
            Component::CurDir => {}
            Component::ParentDir => {
                if !cand.pop() {
                    return Err(outside(trimmed, root));
                }
            }
            Component::Normal(n) => cand.push(n),
        }
    }
    // WSL (#392): Linux-Verknüpfungen löst Windows über 9P nicht verlässlich auf (absolute Ziele zeigen in Linux) –
    // in WSL-Projektordnern wird keiner gefolgt.
    if let Some(link) = wsl_symlink_below(&root.canon, &cand) {
        return Err(err("blocked", format!("“{}” goes through the symbolic link {}; links are not followed in WSL project folders.", crate::mcp::short(trimmed, 200), display(&link))));
    }
    let (real, exists) = real_path(&cand).map_err(|e| err("bad_path", format!("Cannot resolve “{}”: {e}.", crate::mcp::short(trimmed, 200))))?;
    let Ok(rel_path) = real.strip_prefix(&root.canon) else { return Err(outside(trimmed, root)) };
    let comps: Vec<String> = rel_path.components().filter_map(|c| if let Component::Normal(n) = c { Some(n.to_string_lossy().into_owned()) } else { None }).collect();
    if let Some(why) = blocked(&real, &comps, access, env) {
        return Err(err("blocked", format!("Access to “{}” is not allowed: {why}.", crate::mcp::short(trimmed, 200))));
    }
    let is_dir = exists && real.is_dir();
    let rel = if comps.is_empty() { ".".to_string() } else { comps.join("/") };
    Ok(Resolved { path: real, rel, exists, is_dir })
}

fn outside(input: &str, root: &Root) -> ToolErr {
    err("outside_project", format!("“{}” is outside the project folder {}. Only paths inside it are allowed; use a path relative to it, e.g. src/main.rs.", crate::mcp::short(input, 200), display(&root.canon)))
}

/// Pfad für Meldungen (ohne `\\?\`).
pub fn display(p: &Path) -> String {
    let s = p.display().to_string();
    s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
}

/// Längster vorhandener Anfang (auch ein Symlink selbst zählt als vorhanden) kanonisiert, der Rest angehängt; (Pfad,
/// ganz vorhanden). Ein Symlink, dessen Ziel fehlt, ist ein Fehler (sonst schriebe man durch ihn hindurch).
fn real_path(p: &Path) -> std::io::Result<(PathBuf, bool)> {
    let mut existing = p.to_path_buf();
    let mut rest: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if std::fs::symlink_metadata(&existing).is_ok() {
            break;
        }
        match (existing.file_name().map(|n| n.to_os_string()), existing.parent().map(Path::to_path_buf)) {
            (Some(n), Some(parent)) => {
                rest.push(n);
                existing = parent;
            }
            _ => return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no existing parent folder")),
        }
    }
    let mut real = std::fs::canonicalize(&existing).map_err(|e| std::io::Error::new(e.kind(), format!("{e} (a link whose target is missing?)")))?;
    let exists = rest.is_empty();
    for n in rest.into_iter().rev() {
        real.push(n);
    }
    Ok((real, exists))
}

/// Warum dieser (kanonische) Pfad gesperrt ist; `comps` = Abschnitte relativ zum Projektordner.
pub fn blocked(real: &Path, comps: &[String], access: Access, env: &AgentEnv) -> Option<String> {
    if let Some(why) = windows_blocked(real, env) {
        return Some(why);
    }
    // Projektordner in WSL (#392): dieselben Geheimnis-Ordner im Linux-Home (`/home/<u>/.ssh`, `/root/.aws` …) und –
    // über die Laufwerks-Einhängepunkte (`/mnt/c/…`, `/c/…`) – dieselben Windows-Ordner wie direkt.
    if let Some(why) = wsl_blocked(real) {
        return Some(why);
    }
    for w in wsl_drive_paths(real) {
        if let Some(why) = windows_blocked(&w, env) {
            return Some(format!("{why} (reached through WSL)"));
        }
    }
    let all: Vec<String> = real.components().filter_map(|c| if let Component::Normal(n) = c { Some(n.to_string_lossy().to_lowercase()) } else { None }).collect();
    if let Some(b) = all.iter().find(|c| BLOCKED_NAMES.contains(&c.as_str())) {
        return Some(format!("{b} folders hold keys"));
    }
    if let Some(b) = all.last().filter(|n| BLOCKED_FILES.contains(&n.as_str())) {
        return Some(format!("{b} holds credentials"));
    }
    if access == Access::Write && comps.iter().any(|c| c.eq_ignore_ascii_case(".git")) {
        return Some("writing inside .git is not allowed (git operations go through run_command with the user's approval)".into());
    }
    None
}

/// Datenordner von Paneldeck und Geheimnis-Ordner im Windows-Benutzerordner (ohne Rücksicht auf Groß-/Kleinschreibung
/// und `\\?\`-Präfix).
fn windows_blocked(real: &Path, env: &AgentEnv) -> Option<String> {
    let p = norm_win(real);
    if let Some(d) = env.data_dir.as_ref() {
        let canon = std::fs::canonicalize(d).unwrap_or_else(|_| d.clone());
        if [norm_win(&canon), norm_win(d)].iter().any(|d| under_norm(&p, d)) {
            return Some("this is Paneldeck's own data folder".into());
        }
    }
    if let Some(home) = home_dir() {
        let h = norm_win(&home);
        let under = |rel: &str| under_norm(&p, &format!("{h}{SEP}{}", rel.replace('/', SEP).to_lowercase()));
        if !ALLOWED_HOME.iter().any(|a| under(a))
            && let Some(b) = BLOCKED_HOME.iter().find(|b| under(b))
        {
            return Some(format!("~/{b} holds the user's secrets or settings"));
        }
    }
    None
}

/// Trenner der normalisierten Form.
const SEP: &str = if cfg!(windows) { "\\" } else { "/" };

/// Vergleichsform eines Pfads: unter Windows klein, `/` → `\`, ohne `\\?\` bzw. `\\?\UNC\` (→ `\\`), ohne
/// Trenner am Ende.
fn norm_win(p: &Path) -> String {
    let s = p.to_string_lossy().to_string();
    if !cfg!(windows) {
        return s.trim_end_matches('/').to_string();
    }
    let s = s.replace('/', "\\").to_lowercase();
    let s = if let Some(r) = s.strip_prefix(r"\\?\unc\") { format!(r"\\{r}") } else { s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s) };
    s.trim_end_matches('\\').to_string()
}

/// Liegt `p` in `dir` (oder ist es)? Beide in [`norm_win`]-Form.
fn under_norm(p: &str, dir: &str) -> bool {
    !dir.is_empty() && (p == dir || p.strip_prefix(dir).is_some_and(|r| r.starts_with(SEP)))
}

/// Einhängepunkte der Windows-Laufwerke, die für eine Distribution in Frage kommen (der bekannte, dazu sicherheitshalber
/// `/mnt/` und `/`).
fn wsl_mounts(distro: &str) -> Vec<String> {
    let mut v = vec![crate::wsl::DEFAULT_MOUNT.to_string(), "/".to_string()];
    if let Some(m) = crate::wsl::cached_mount(distro).filter(|m| !v.contains(m)) {
        v.insert(0, m);
    }
    v
}

/// WSL-Pfad (`\\wsl.localhost\<D>\mnt\c\x`) → die Windows-Pfade, die er über einen Laufwerks-Einhängepunkt erreichen
/// kann (`C:\x`).
fn wsl_drive_paths(real: &Path) -> Vec<PathBuf> {
    let Some((d, linux)) = crate::wsl::unc_to_linux(&real.to_string_lossy()) else { return Vec::new() };
    let mut out: Vec<PathBuf> = Vec::new();
    for m in wsl_mounts(&d) {
        if let Some((drive, rest)) = crate::wsl::mounted_drive(&linux, &m) {
            let p = PathBuf::from(format!("{}:\\{}", drive.to_ascii_uppercase(), rest.join("\\")));
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// Enthält der WSL-Ordner einen Laufwerks-Einhängepunkt (ist `/`, ein Einhängepunkt, ein Ordner darüber oder ein
/// eingehängtes Laufwerk selbst wie `/mnt/c`)?
fn wsl_root_contains_mount(canon: &Path) -> bool {
    let Some((d, linux)) = crate::wsl::unc_to_linux(&canon.to_string_lossy()) else { return false };
    let l = linux.trim_end_matches('/');
    if l.is_empty() {
        return true;
    }
    wsl_mounts(&d).iter().any(|m| {
        let m = m.trim_end_matches('/');
        // Einhängepunkt selbst oder ein Ordner darüber
        let contains = !m.is_empty() && (m == l || m.starts_with(&format!("{l}/")));
        // ein eingehängtes Laufwerk selbst (`/mnt/c`, bei `/` als Einhängepunkt `/c`)
        let drive_root = crate::wsl::mounted_drive(l, &format!("{m}/")).is_some_and(|(_, rest)| rest.is_empty());
        contains || drive_root
    })
}

/// Erste Verknüpfung (Symlink/Junction) zwischen WSL-Projektordner `root` und `cand` (nur vorhandene Teile).
fn wsl_symlink_below(root: &Path, cand: &Path) -> Option<PathBuf> {
    if !crate::wsl::is_wsl_unc(&root.to_string_lossy()) {
        return None;
    }
    let rel = cand.strip_prefix(root).ok()?;
    let mut cur = root.to_path_buf();
    for c in rel.components() {
        cur.push(c);
        match std::fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => return Some(cur),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
}

/// Gesperrt, weil der Pfad in einer WSL-Distribution unter einem Linux-Home in [`BLOCKED_HOME`] liegt (`/root/…`,
/// `/home/<u>/…`)?
fn wsl_blocked(real: &Path) -> Option<String> {
    let s = real.to_string_lossy();
    let (_, linux) = crate::wsl::unc_to_linux(&s)?;
    let parts: Vec<&str> = linux.split('/').filter(|c| !c.is_empty()).collect();
    let rest: &[&str] = match parts.as_slice() {
        ["root", rest @ ..] => rest,
        ["home", _, rest @ ..] => rest,
        _ => return None,
    };
    let rel = rest.join("/");
    BLOCKED_HOME.iter().find(|b| rel == **b || rel.starts_with(&format!("{b}/"))).map(|b| format!("~/{b} holds the user's secrets or settings (in WSL)"))
}

/// `starts_with` ohne Rücksicht auf Groß-/Kleinschreibung unter Windows und macOS.
fn starts_with_ci(p: &Path, prefix: &Path) -> bool {
    if !cfg!(any(windows, target_os = "macos")) {
        return p.starts_with(prefix);
    }
    let a: Vec<String> = p.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect();
    let b: Vec<String> = prefix.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect();
    a.len() >= b.len() && a[..b.len()] == b[..]
}

/// Ist die Datei sensibel (Lesen nur mit Freigabe)? `rel` relativ zum Projektordner mit `/`.
pub fn is_sensitive(rel: &str, patterns: &[String]) -> bool {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    patterns.iter().map(|p| p.trim()).filter(|p| !p.is_empty() && !p.starts_with('#')).any(|p| {
        let p = p.trim_start_matches("./");
        if p.contains('/') {
            let p = p.trim_start_matches('/');
            glob_match(p, rel, true) || glob_match(&format!("**/{p}"), rel, true)
        } else {
            glob_match(p, name, true)
        }
    })
}

/// Nach dem Öffnen: liegt die geöffnete Datei wirklich im Projektordner (und ist es die geprüfte)?
pub fn verify_open(f: &std::fs::File, root: &Root, expected: &Path) -> Result<(), ToolErr> {
    let bad = || err("outside_project", "The file changed while it was opened (a link to outside the project folder?); nothing was done.");
    #[cfg(windows)]
    {
        let _ = expected;
        let real = final_path(f).ok_or_else(bad)?;
        if !starts_with_ci(&real, &root.canon) {
            return Err(bad());
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let a = f.metadata().map_err(|_| bad())?;
        let b = std::fs::metadata(expected).map_err(|_| bad())?;
        if a.dev() != b.dev() || a.ino() != b.ino() {
            return Err(bad());
        }
        let real = std::fs::canonicalize(expected).map_err(|_| bad())?;
        if !real.starts_with(&root.canon) {
            return Err(bad());
        }
    }
    Ok(())
}

/// Endgültiger Pfad eines geöffneten Handles (`\\?\C:\…`).
#[cfg(windows)]
pub fn final_path(f: &std::fs::File) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, FILE_NAME_NORMALIZED};
    let mut buf = vec![0u16; 1024];
    loop {
        // SAFETY: gültiges Handle der offenen Datei, Puffer mit der angegebenen Länge.
        let n = unsafe { GetFinalPathNameByHandleW(f.as_raw_handle() as _, buf.as_mut_ptr(), buf.len() as u32, FILE_NAME_NORMALIZED) } as usize;
        if n == 0 {
            return None;
        }
        if n < buf.len() {
            return Some(PathBuf::from(std::ffi::OsString::from_wide(&buf[..n])));
        }
        if buf.len() > 65_536 {
            return None;
        }
        buf.resize(n + 1, 0);
    }
}

/// Datei zum Lesen öffnen und prüfen.
pub fn open_read(root: &Root, r: &Resolved) -> Result<std::fs::File, ToolErr> {
    let f = std::fs::File::open(&r.path).map_err(|e| io_err(&r.rel, &e))?;
    verify_open(&f, root, &r.path)?;
    Ok(f)
}

/// Fehler beim Dateizugriff für das Modell.
pub fn io_err(rel: &str, e: &std::io::Error) -> ToolErr {
    match e.kind() {
        std::io::ErrorKind::NotFound => err("not_found", format!("“{rel}” does not exist.")),
        std::io::ErrorKind::PermissionDenied => err("permission_denied", format!("No permission to access “{rel}” (read-only, in use or protected).")),
        _ => err("io_error", format!("“{rel}”: {e}")),
    }
}

/// Vorschläge für einen nicht gefundenen Pfad: ähnliche Namen im (vorhandenen) Elternordner und gleichnamige Dateien in
/// den ersten Ebenen des Projekts. Höchstens drei, relativ zum Projektordner.
pub fn suggestions(root: &Root, r: &Resolved) -> Vec<String> {
    let want = r.rel.rsplit('/').next().unwrap_or("").to_lowercase();
    if want.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<(usize, String)> = Vec::new();
    // Nächster vorhandener Elternordner.
    let mut dir = r.path.parent().map(Path::to_path_buf);
    while let Some(d) = dir.clone() {
        if d.is_dir() || !d.starts_with(&root.canon) {
            break;
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    let rel_of = |p: &Path| -> Option<String> {
        let rp = p.strip_prefix(&root.canon).ok()?;
        let parts: Vec<String> = rp.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        Some(parts.join("/"))
    };
    if let Some(d) = dir.filter(|d| d.starts_with(&root.canon))
        && let Ok(rd) = std::fs::read_dir(&d)
    {
        for e in rd.flatten().take(2000) {
            let name = e.file_name().to_string_lossy().to_lowercase();
            let dist = edit_distance(&want, &name);
            if dist <= (want.chars().count() / 3).max(2)
                && let Some(rel) = rel_of(&e.path())
            {
                out.push((dist, rel));
            }
        }
    }
    // Gleicher Name woanders (drei Ebenen tief, ohne versteckte Ordner und große Ordner).
    if out.is_empty() {
        let mut stack = vec![(root.canon.clone(), 0usize)];
        let mut seen = 0usize;
        while let Some((d, depth)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                seen += 1;
                if seen > 5000 || out.len() >= 3 {
                    break;
                }
                let name = e.file_name().to_string_lossy().to_lowercase();
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    if depth < 3 && !name.starts_with('.') && !matches!(name.as_str(), "target" | "node_modules" | "dist" | "build") {
                        stack.push((e.path(), depth + 1));
                    }
                } else if name == want
                    && let Some(rel) = rel_of(&e.path())
                {
                    out.push((0, rel));
                }
            }
        }
    }
    out.sort();
    out.dedup_by(|a, b| a.1 == b.1);
    out.into_iter().take(3).map(|(_, s)| s).collect()
}

/// Levenshtein-Abstand (Zeichen), für Vorschläge.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > 8 {
        return usize::MAX / 2;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(ca != cb)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Meldung „nicht gefunden“ mit Vorschlägen.
pub fn not_found(root: &Root, r: &Resolved, what: &str, example: &str) -> ToolErr {
    let s = suggestions(root, r);
    let hint = if s.is_empty() { format!("Use list_dir or glob to find the right path. Example: {example}") } else { format!("Did you mean {}?", s.join(", ")) };
    err("not_found", format!("{what} “{}” does not exist in the project folder. {hint}", r.rel))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Temporärer Projektordner (eindeutig je Test).
    pub fn tmp_root(name: &str) -> (PathBuf, AgentEnv) {
        let d = std::env::temp_dir().join(format!("pd-agent-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let env = AgentEnv { root: Some(d.clone()), ..AgentEnv::default() };
        (d, env)
    }

    #[test]
    fn paths_stay_inside_the_project_folder() {
        let (d, env) = tmp_root("inside");
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::write(d.join("src/main.rs"), "fn main() {}").unwrap();
        let root = root(&env).unwrap();
        let r = resolve(&root, &env, "src/main.rs", Access::Read).unwrap();
        assert!(r.exists && !r.is_dir);
        assert_eq!(r.rel, "src/main.rs");
        assert_eq!(resolve(&root, &env, "./src/../src/main.rs", Access::Read).unwrap().rel, "src/main.rs");
        assert_eq!(resolve(&root, &env, "", Access::Read).unwrap().rel, ".");
        let new = resolve(&root, &env, "src/new/deep.rs", Access::Write).unwrap();
        assert!(!new.exists && new.rel == "src/new/deep.rs");
        // absolut innerhalb: erlaubt
        let abs = d.join("src").join("main.rs");
        assert_eq!(resolve(&root, &env, &abs.to_string_lossy(), Access::Read).unwrap().rel, "src/main.rs");
        // hinaus: abgelehnt
        for bad in ["../x", "src/../../x", "../../../../../../../../etc/passwd"] {
            let e = resolve(&root, &env, bad, Access::Read).unwrap_err();
            assert_eq!(e.0, "outside_project", "{bad}: {e:?}");
            assert!(e.1.contains("relative"), "{}", e.1);
        }
        let outside_abs = std::env::temp_dir().join("pd-agent-elsewhere.txt");
        assert_eq!(resolve(&root, &env, &outside_abs.to_string_lossy(), Access::Read).unwrap_err().0, "outside_project");
        assert_eq!(resolve(&root, &env, "~/x", Access::Read).unwrap_err().0, "bad_path");
        assert_eq!(resolve(&root, &env, "a\0b", Access::Read).unwrap_err().0, "bad_path");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(windows)]
    #[test]
    fn windows_secrets_are_blocked_through_wsl_drive_mounts() {
        let home = home_dir().expect("home");
        let data = std::env::temp_dir().join(format!("pd-paths-data-{}", std::process::id()));
        let env = AgentEnv { data_dir: Some(data.clone()), ..AgentEnv::default() };
        if let Some((d, parts)) = crate::wsl::drive_parts(&home.to_string_lossy()) {
            let rest = parts.join("/");
            for mount in ["/mnt", ""] {
                let unc = |sub: &str| PathBuf::from(crate::wsl::linux_to_unc("Sec-Distro", &format!("{mount}/{d}/{rest}/{sub}")));
                assert!(blocked(&unc("AppData/Roaming/x"), &[], Access::Read, &env).is_some(), "{mount}: AppData");
                assert!(blocked(&unc(".claude/.credentials.json"), &[], Access::Read, &env).is_some(), "{mount}: .claude");
                assert!(blocked(&unc(".ssh/id_rsa"), &[], Access::Read, &env).is_some());
                assert!(blocked(&unc("proj/src/main.rs"), &[], Access::Read, &env).is_none(), "{mount}: normales Projekt");
            }
        }
        // Datenordner (ipc.json mit dem Token) auch über /mnt/<lw>
        if let Some((d, parts)) = crate::wsl::drive_parts(&data.to_string_lossy()) {
            let p = PathBuf::from(crate::wsl::linux_to_unc("Sec-Distro", &format!("/mnt/{d}/{}/ipc.json", parts.join("/"))));
            assert!(blocked(&p, &[], Access::Read, &env).unwrap().contains("data folder"));
        }
        // Zugangsdaten-Dateien überall
        assert!(blocked(Path::new("C:/proj/.claude/.credentials.json"), &[], Access::Read, &env).is_some());
        assert!(blocked(Path::new("C:/proj/.Git-Credentials"), &[], Access::Read, &env).is_some());
        assert!(blocked(Path::new("C:/proj/credentials.md"), &[], Access::Read, &env).is_none());
    }

    #[test]
    fn wsl_roots_with_drive_mounts_are_refused() {
        let r = |l: &str| wsl_root_contains_mount(&PathBuf::from(crate::wsl::linux_to_unc("Mnt-Distro", l)));
        assert!(r("/") && r("/mnt") && r("/mnt/c") && r("/c"), "Einhängepunkte und Laufwerke");
        assert!(!r("/mnt/c/Users/u/proj") && !r("/home/u/proj") && !r("/srv"));
        assert!(wsl_root_contains_mount(Path::new(r"\\?\UNC\wsl.localhost\Mnt-Distro\mnt")));
        assert!(!wsl_root_contains_mount(Path::new(r"C:\Users\u\proj")));
        crate::wsl::set_cached("Mnt-Win-Distro", None, Some("/win/"));
        assert!(wsl_root_contains_mount(&PathBuf::from(crate::wsl::linux_to_unc("Mnt-Win-Distro", "/win"))), "eigener Einhängepunkt");
        assert!(wsl_root_contains_mount(&PathBuf::from(crate::wsl::linux_to_unc("Mnt-Win-Distro", "/win/d"))));
        assert_eq!(wsl_drive_paths(&PathBuf::from(crate::wsl::linux_to_unc("Mnt-Win-Distro", "/win/d/x"))), vec![PathBuf::from(r"D:\x")]);
        // Kein Verknüpfungs-Test für Windows-Ordner
        assert_eq!(wsl_symlink_below(Path::new(r"C:\x"), Path::new(r"C:\x\y")), None);
        assert!(under_norm(&norm_win(Path::new(r"\\?\C:\Users\U\AppData\x")), &norm_win(Path::new(r"c:\users\u\appdata"))) == cfg!(windows));
    }

    #[test]
    fn wsl_secret_folders_are_blocked() {
        // #392: Projektordner über \\wsl.localhost – die Geheimnis-Ordner im Linux-Home bleiben gesperrt.
        assert!(wsl_blocked(Path::new(r"\\?\UNC\wsl.localhost\Ubuntu\home\u\.ssh\id_rsa")).is_some());
        assert!(wsl_blocked(Path::new(r"\\wsl.localhost\Ubuntu\root\.aws")).is_some());
        assert!(wsl_blocked(Path::new(r"\\wsl$\Ubuntu\home\u\.config\gh\hosts.yml")).is_some());
        assert!(wsl_blocked(Path::new(r"\\wsl.localhost\Ubuntu\home\u\proj\src")).is_none());
        assert!(wsl_blocked(Path::new(r"\\wsl.localhost\Ubuntu\home\u\.sshx")).is_none());
        assert!(wsl_blocked(Path::new(r"C:\Users\u\proj")).is_none());
    }

    #[test]
    fn windows_device_unc_and_stream_paths_are_refused() {
        for bad in [r"\\server\share\x", r"\\?\C:\Windows", r"\\.\PhysicalDrive0", "//server/share", "file.txt:stream", "src/a.rs::$DATA", "C:relative.txt", r"\rooted.txt", "CON", "src/nul.txt", "COM1", "lpt9.log", "trailing.", "space ", "dir./x"] {
            assert!(lexical_problem(bad).is_some(), "{bad}");
        }
        for ok in ["C:\\Users\\x", "src/main.rs", "a.b.c", "COM10.txt", "console.rs", "nullable.rs", "..", "./x"] {
            assert!(lexical_problem(ok).is_none(), "{ok}: {:?}", lexical_problem(ok));
        }
    }

    #[test]
    fn links_out_of_the_project_are_refused() {
        let (d, env) = tmp_root("links");
        let outside = std::env::temp_dir().join(format!("pd-agent-links-out-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "x").unwrap();
        let root = root(&env).unwrap();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&outside, d.join("link")).is_ok();
        #[cfg(windows)]
        let made = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(d.join("link")).arg(&outside).output().map(|o| o.status.success()).unwrap_or(false);
        if made {
            let e = resolve(&root, &env, "link/secret.txt", Access::Read).unwrap_err();
            assert_eq!(e.0, "outside_project", "{e:?}");
            assert_eq!(resolve(&root, &env, "link/new.txt", Access::Write).unwrap_err().0, "outside_project");
        }
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn data_folder_git_and_secret_folders_are_blocked() {
        let (d, mut env) = tmp_root("blocked");
        std::fs::create_dir_all(d.join("paneldeck-data")).unwrap();
        std::fs::create_dir_all(d.join(".git")).unwrap();
        std::fs::create_dir_all(d.join("x/.ssh")).unwrap();
        env.data_dir = Some(d.join("paneldeck-data"));
        let root = root(&env).unwrap();
        assert_eq!(resolve(&root, &env, "paneldeck-data/ipc.json", Access::Read).unwrap_err().0, "blocked");
        assert_eq!(resolve(&root, &env, "x/.ssh/id_rsa", Access::Read).unwrap_err().0, "blocked");
        assert!(resolve(&root, &env, ".git/config", Access::Read).is_ok(), "lesen in .git geht");
        let e = resolve(&root, &env, ".git/hooks/pre-commit", Access::Write).unwrap_err();
        assert!(e.0 == "blocked" && e.1.contains("run_command"), "{e:?}");
        // Projektordner fehlt / ist ein Laufwerk
        assert_eq!(super::root(&AgentEnv::default()).unwrap_err().0, "no_project_folder");
        let drive = AgentEnv { root: Some(PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" })), ..AgentEnv::default() };
        assert_eq!(super::root(&drive).unwrap_err().0, "unsafe_project_folder");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn sensitive_patterns_match_names_and_paths() {
        let pats: Vec<String> = DEFAULT_SENSITIVE.iter().map(|s| s.to_string()).collect();
        for s in [".env", "app/.env.local", "config/prod.env", "certs/server.PEM", "id_rsa", "id_ed25519.pub", "secrets.yaml", "infra/main.tfstate", ".npmrc", "credentials"] {
            assert!(is_sensitive(s, &pats), "{s}");
        }
        for s in ["src/main.rs", "environment.md", "src/app/secret_ui.rs", "keys.rs", "README.md", "credentialsmanager.rs"] {
            assert!(!is_sensitive(s, &pats), "{s}");
        }
        let custom = vec!["config/*.json".to_string(), "# comment".into(), " ".into()];
        assert!(is_sensitive("config/a.json", &custom) && is_sensitive("sub/config/a.json", &custom) && !is_sensitive("a.json", &custom));
    }

    #[test]
    fn suggestions_find_similar_and_same_names() {
        let (d, env) = tmp_root("suggest");
        std::fs::create_dir_all(d.join("src/deep")).unwrap();
        std::fs::write(d.join("src/main.rs"), "").unwrap();
        std::fs::write(d.join("src/deep/util.rs"), "").unwrap();
        let root = root(&env).unwrap();
        let r = resolve(&root, &env, "src/mian.rs", Access::Read).unwrap();
        assert_eq!(suggestions(&root, &r), ["src/main.rs"]);
        let r = resolve(&root, &env, "util.rs", Access::Read).unwrap();
        assert_eq!(suggestions(&root, &r), ["src/deep/util.rs"]);
        let e = not_found(&root, &resolve(&root, &env, "src/mian.rs", Access::Read).unwrap(), "File", "{}");
        assert!(e.1.contains("Did you mean src/main.rs?"), "{}", e.1);
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn opened_files_are_checked_again() {
        let (d, env) = tmp_root("open");
        std::fs::write(d.join("a.txt"), "x").unwrap();
        let root = root(&env).unwrap();
        let r = resolve(&root, &env, "a.txt", Access::Read).unwrap();
        assert!(open_read(&root, &r).is_ok());
        let other = Root { canon: std::fs::canonicalize(std::env::temp_dir()).unwrap().join("pd-agent-not-here") };
        assert!(open_read(&other, &r).is_err(), "geöffnete Datei außerhalb des (anderen) Projektordners");
        let _ = std::fs::remove_dir_all(&d);
    }
}
