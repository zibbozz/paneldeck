//! Bekannte Language Server (#367): eingebaute Liste je Sprache (Sprach-Ids des Syntax-Modus) und die Suche im PATH.
//!
//! Regeln (Entscheidung zu #367): ein Server aus dem PATH zählt schon in 3.3.0; gesucht wird im **frischen** PATH von
//! Windows (Registry + Prozess, wie `portable_pty::CommandBuilder` ihn aufbaut), aufgelöst zu einer absoluten Datei – nie
//! aus dem Projektordner (auch nicht relativ: `.`, leere Einträge, Einträge im Projekt). npm-Hüllen (`.cmd`/`.bat`)
//! laufen über `cmd.exe /d /s /c` mit gequotetem Pfad; der Proxy von rustup (eine Kopie von `rustup.exe`) und der
//! Python-Platzhalter des Microsoft Store (`WindowsApps`, 0 Byte) werden erkannt. Vertrauenswürdige Ordner bringen in
//! 3.3.0 keine eigenen Server mit (`node_modules/.bin`, `.venv`).

use std::path::{Path, PathBuf};

/// Ein bekannter Server: Programmname (ohne Endung) und Argumente.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Known {
    /// Kennung (stabil, in `settings.json`: abgewählte Server je Sprache).
    pub id: &'static str,
    pub program: &'static str,
    pub args: &'static [&'static str],
}

const fn k(id: &'static str, program: &'static str, args: &'static [&'static str]) -> Known {
    Known { id, program, args }
}

/// Eingebaute Server je Sprache (erste gefundene gewinnt). PowerShell, NSIS und SQL nur mit eigenem Pfad.
pub fn known(lang: &str) -> &'static [Known] {
    const RUST: &[Known] = &[k("rust-analyzer", "rust-analyzer", &[])];
    const PY: &[Known] = &[k("basedpyright", "basedpyright-langserver", &["--stdio"]), k("pyright", "pyright-langserver", &["--stdio"]), k("pylsp", "pylsp", &[])];
    const TS: &[Known] = &[k("typescript-language-server", "typescript-language-server", &["--stdio"])];
    const C: &[Known] = &[k("clangd", "clangd", &[])];
    const GO: &[Known] = &[k("gopls", "gopls", &[])];
    const LUA: &[Known] = &[k("lua-language-server", "lua-language-server", &[])];
    const CS: &[Known] = &[k("csharp-ls", "csharp-ls", &[])];
    const JSON: &[Known] = &[k("vscode-json-language-server", "vscode-json-language-server", &["--stdio"])];
    const HTML: &[Known] = &[k("vscode-html-language-server", "vscode-html-language-server", &["--stdio"])];
    const CSS: &[Known] = &[k("vscode-css-language-server", "vscode-css-language-server", &["--stdio"])];
    const YAML: &[Known] = &[k("yaml-language-server", "yaml-language-server", &["--stdio"])];
    const TOML: &[Known] = &[k("taplo", "taplo", &["lsp", "stdio"])];
    const SH: &[Known] = &[k("bash-language-server", "bash-language-server", &["start"])];
    const TEX: &[Known] = &[k("texlab", "texlab", &[])];
    const PHP: &[Known] = &[k("phpactor", "phpactor", &["language-server"])];
    const RUBY: &[Known] = &[k("ruby-lsp", "ruby-lsp", &[])];
    const CMAKE: &[Known] = &[k("neocmakelsp", "neocmakelsp", &["--stdio"])];
    const MD: &[Known] = &[k("marksman", "marksman", &["server"])];
    const DOCKER: &[Known] = &[k("docker-langserver", "docker-langserver", &["--stdio"])];
    const JAVA: &[Known] = &[k("jdtls", "jdtls", &[])];
    const KT: &[Known] = &[k("kotlin-language-server", "kotlin-language-server", &[])];
    const PERL: &[Known] = &[k("perlnavigator", "perlnavigator", &["--stdio"])];
    const XML: &[Known] = &[k("lemminx", "lemminx", &[])];
    const SWIFT: &[Known] = &[k("sourcekit-lsp", "sourcekit-lsp", &[])];
    match lang {
        "rust" => RUST,
        "python" => PY,
        "typescript" | "javascript" | "tsx" => TS,
        "c" | "cpp" => C,
        "go" => GO,
        "lua" => LUA,
        "csharp" => CS,
        "json" => JSON,
        "html" => HTML,
        "css" | "scss" | "less" => CSS,
        "yaml" => YAML,
        "toml" => TOML,
        "shell" => SH,
        "latex" | "bibtex" | "tex" => TEX,
        "php" => PHP,
        "ruby" => RUBY,
        "cmake" => CMAKE,
        "markdown" => MD,
        "dockerfile" => DOCKER,
        "java" => JAVA,
        "kotlin" => KT,
        "perl" => PERL,
        "xml" => XML,
        "swift" => SWIFT,
        _ => &[],
    }
}

/// Sprachen, die nur mit eigenem Pfad gehen (kein eingebauter Server).
pub const OWN_PATH_ONLY: &[&str] = &["powershell", "nsis", "sql"];

/// Wie ein gefundenes Programm gestartet wird.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Launch {
    /// Absoluter Pfad des Programms (bei `.cmd`/`.bat`: `cmd.exe` aus System32).
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Gefundene Datei (für die Anzeige; bei Hüllen die `.cmd`).
    pub found: PathBuf,
    /// Proxy von rustup: startet nur, wenn die Komponente installiert ist (sonst Hinweis bei sofortigem Ende).
    pub rustup_proxy: bool,
    /// Server in WSL (#367): Projektwurzel als Linux-Pfad (für `rootUri`); `None` = Windows.
    pub lsp_root: Option<String>,
}

/// Was die Suche braucht (in Tests ersetzbar).
pub struct Env<'a> {
    /// Einträge des PATH in Reihenfolge.
    pub path: Vec<PathBuf>,
    /// Endungen (`.EXE`, `.CMD` …) unter Windows; leer = Unix (ohne Endung, ausführbar).
    pub exts: Vec<String>,
    /// Projektordner: nie daraus starten.
    pub project: Option<&'a Path>,
    /// Ordner von System32 (für `cmd.exe`).
    pub system32: PathBuf,
    /// Größe von `~/.cargo/bin/rustup(.exe)`, falls vorhanden (Erkennung des Proxys).
    pub rustup_size: Option<u64>,
    pub is_file: &'a dyn Fn(&Path) -> Option<u64>,
}

impl Env<'_> {
    /// Frischer PATH dieses Rechners (Registry + Prozess unter Windows), PATHEXT, System32, Größe von rustup.
    #[cfg_attr(test, allow(dead_code))]
    pub fn system(project: Option<&Path>) -> Env<'_> {
        let path_var = portable_pty::CommandBuilder::new("x").get_env("PATH").map(|s| s.to_os_string()).or_else(|| std::env::var_os("PATH")).unwrap_or_default();
        let path: Vec<PathBuf> = std::env::split_paths(&path_var).collect();
        let exts = if cfg!(windows) {
            std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into()).split(';').filter(|e| !e.is_empty()).map(|e| e.to_ascii_lowercase()).filter(|e| [".com", ".exe", ".bat", ".cmd"].contains(&e.as_str())).collect()
        } else {
            Vec::new()
        };
        let system32 = std::env::var_os("SystemRoot").map(|r| PathBuf::from(r).join("System32")).unwrap_or_else(|| PathBuf::from(r"C:\Windows\System32"));
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
        let rustup_size = home.and_then(|h| std::fs::metadata(h.join(".cargo").join("bin").join(if cfg!(windows) { "rustup.exe" } else { "rustup" })).ok()).map(|m| m.len());
        static IS_FILE: fn(&Path) -> Option<u64> = |p: &Path| std::fs::metadata(p).ok().filter(|m| m.is_file()).map(|m| m.len());
        Env { path, exts, project, system32, rustup_size, is_file: &IS_FILE }
    }
}

/// Liegt `dir` im Projekt (oder ist relativ)? Dann wird dort nie gesucht.
fn unsafe_dir(dir: &Path, project: Option<&Path>) -> bool {
    if dir.as_os_str().is_empty() || !dir.is_absolute() {
        return true;
    }
    let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase();
    project.is_some_and(|pr| {
        let (d, p) = (norm(dir), norm(pr));
        d == p || d.starts_with(&format!("{p}\\"))
    })
}

/// Programm `name` im PATH finden und den Start beschreiben (`args` = Argumente des Servers).
pub fn find(name: &str, args: &[&str], env: &Env) -> Option<Launch> {
    for dir in &env.path {
        if unsafe_dir(dir, env.project) {
            continue;
        }
        let candidates: Vec<(PathBuf, String)> = if env.exts.is_empty() { vec![(dir.join(name), String::new())] } else { env.exts.iter().map(|e| (dir.join(format!("{name}{e}")), e.clone())).collect() };
        for (p, ext) in candidates {
            let Some(size) = (env.is_file)(&p) else { continue };
            // Platzhalter des Microsoft Store (App-Ausführungsalias, 0 Byte unter WindowsApps): kein echtes Programm.
            if size == 0 && p.to_string_lossy().to_ascii_lowercase().contains("\\windowsapps\\") {
                continue;
            }
            let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            return Some(launch_for(&p, &ext, args, env, size));
        }
    }
    None
}

/// Start für eine gefundene bzw. selbst eingetragene Datei.
pub fn launch_for(p: &Path, ext: &str, args: Vec<String>, env: &Env, size: u64) -> Launch {
    let rustup_proxy = env.rustup_size.is_some_and(|r| r == size) && p.file_stem().is_some_and(|s| s != "rustup");
    if ext == ".cmd" || ext == ".bat" {
        // `cmd /d /s /c ""<hülle>" args"`: /d ohne AutoRun, /s nimmt nur die äußeren Anführungszeichen weg.
        let mut line = format!("\"\"{}\"", p.display());
        for a in &args {
            line.push(' ');
            line.push_str(&cmd_quote(a));
        }
        line.push('"');
        return Launch { program: env.system32.join("cmd.exe"), args: vec!["/d".into(), "/s".into(), "/c".into(), line], found: p.to_path_buf(), rustup_proxy, lsp_root: None };
    }
    Launch { program: p.to_path_buf(), args, found: p.to_path_buf(), rustup_proxy, lsp_root: None }
}

/// Argument für die Zeile von `cmd /c`: nur einfache Zeichen bleiben ungequotet; Sonderzeichen von cmd werden mit `^`
/// entschärft, damit kein Argument einen zweiten Befehl startet.
fn cmd_quote(a: &str) -> String {
    if !a.is_empty() && a.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=".contains(c)) {
        return a.to_string();
    }
    let mut s = String::from("\"");
    for c in a.chars() {
        match c {
            '"' => s.push_str("\"\""),
            '^' | '&' | '|' | '<' | '>' | '%' | '!' | '(' | ')' => {
                s.push('^');
                s.push(c);
            }
            _ => s.push(c),
        }
    }
    s.push('"');
    s
}

/// Server für eine Sprache: eigener Pfad mit eigenen Argumenten (geht vor), sonst der erste gefundene eingebaute, der
/// nicht abgewählt ist.
pub fn resolve(lang: &str, own: Option<&Path>, own_args: &[String], disabled: &[String], env: &Env) -> Option<(String, Launch)> {
    if let Some(p) = own {
        let size = (env.is_file)(p)?;
        let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy().to_ascii_lowercase())).unwrap_or_default();
        return Some(("own".into(), launch_for(p, &ext, own_args.to_vec(), env, size)));
    }
    known(lang).iter().filter(|k| !disabled.iter().any(|d| d == k.id)).find_map(|k| find(k.program, k.args, env).map(|l| (k.id.to_string(), l)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env<'a>(files: &'a HashMap<PathBuf, u64>, project: Option<&'a Path>, f: &'a dyn Fn(&Path) -> Option<u64>) -> Env<'a> {
        let _ = files;
        Env {
            path: vec![PathBuf::from("."), PathBuf::from(r"C:\proj\node_modules\.bin"), PathBuf::from(r"C:\Users\u\AppData\Local\Microsoft\WindowsApps"), PathBuf::from(r"C:\Users\u\AppData\Roaming\npm"), PathBuf::from(r"C:\Users\u\.cargo\bin"), PathBuf::from(r"C:\tools")],
            exts: vec![".com".into(), ".exe".into(), ".bat".into(), ".cmd".into()],
            project,
            system32: PathBuf::from(r"C:\Windows\System32"),
            rustup_size: Some(9_000_000),
            is_file: f,
        }
    }

    #[test]
    fn path_lookup_skips_the_project_relative_entries_and_store_stubs() {
        let mut files: HashMap<PathBuf, u64> = HashMap::new();
        files.insert(PathBuf::from(r"C:\proj\node_modules\.bin\typescript-language-server.cmd"), 100);
        files.insert(PathBuf::from(r".\clangd.exe"), 100);
        files.insert(PathBuf::from(r"C:\Users\u\AppData\Local\Microsoft\WindowsApps\pylsp.exe"), 0);
        files.insert(PathBuf::from(r"C:\Users\u\AppData\Roaming\npm\typescript-language-server.cmd"), 300);
        files.insert(PathBuf::from(r"C:\Users\u\.cargo\bin\rust-analyzer.exe"), 9_000_000);
        files.insert(PathBuf::from(r"C:\tools\clangd.exe"), 5);
        files.insert(PathBuf::from(r"C:\tools\pylsp.exe"), 7);
        let f = |p: &Path| files.get(p).copied();
        let e = env(&files, Some(Path::new(r"C:\proj")), &f);
        // npm-Hülle über cmd.exe, nicht die im Projekt
        let (id, l) = resolve("typescript", None, &[], &[], &e).unwrap();
        assert_eq!(id, "typescript-language-server");
        assert_eq!(l.found, PathBuf::from(r"C:\Users\u\AppData\Roaming\npm\typescript-language-server.cmd"));
        assert_eq!(l.program, PathBuf::from(r"C:\Windows\System32\cmd.exe"));
        assert_eq!(l.args, vec!["/d", "/s", "/c", "\"\"C:\\Users\\u\\AppData\\Roaming\\npm\\typescript-language-server.cmd\" --stdio\""]);
        // relativer Eintrag „.“ zählt nicht
        assert_eq!(resolve("c", None, &[], &[], &e).unwrap().1.program, PathBuf::from(r"C:\tools\clangd.exe"));
        // Store-Platzhalter übersprungen
        let (id, l) = resolve("python", None, &[], &[], &e).unwrap();
        assert_eq!((id.as_str(), l.program.clone()), ("pylsp", PathBuf::from(r"C:\tools\pylsp.exe")));
        // rustup-Proxy erkannt
        assert!(resolve("rust", None, &[], &[], &e).unwrap().1.rustup_proxy);
        // abgewählt → nichts; eigener Pfad geht vor; unbekannte Sprache
        assert!(resolve("python", None, &[], &["pylsp".into()], &e).is_none());
        assert_eq!(resolve("powershell", Some(Path::new(r"C:\tools\clangd.exe")), &["-x".into()], &[], &e).map(|x| x.0), Some("own".into()));
        assert!(resolve("powershell", None, &[], &[], &e).is_none());
        assert!(OWN_PATH_ONLY.iter().all(|l| known(l).is_empty()));
    }

    #[test]
    fn cmd_arguments_cannot_start_a_second_command() {
        assert_eq!(cmd_quote("--stdio"), "--stdio");
        assert_eq!(cmd_quote("a&b"), "\"a^&b\"");
        assert_eq!(cmd_quote("x\"|y"), "\"x\"\"^|y\"");
        assert_eq!(cmd_quote("%PATH%"), "\"^%PATH^%\"");
    }

    #[test]
    fn every_language_with_a_server_is_a_known_language_id() {
        for l in ["rust", "python", "typescript", "javascript", "tsx", "c", "cpp", "go", "lua", "csharp", "json", "html", "css", "scss", "less", "yaml", "toml", "shell", "latex", "php", "ruby", "cmake", "markdown", "dockerfile", "java", "kotlin", "perl", "xml", "swift"] {
            assert!(crate::editor::syntax::languages::language(l).is_some(), "{l}");
            assert!(!known(l).is_empty(), "{l}");
        }
    }
}
