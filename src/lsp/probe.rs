//! Probe gegen echte Sprachserver (#367): nutzt den echten Weg von Paneldeck – Suche im PATH bzw. eigener Pfad
//! ([`servers::resolve`], `.cmd`-Hüllen über `raw_arg`), [`client::Client::spawn`], `initialize`, `didOpen`,
//! Diagnosen, Vervollständigen, `shutdown`. Läuft nur von Hand (`#[ignore]`), nie in der CI:
//!
//! ```text
//! set LSP_PROBE_DIRS=E:\…\npm\node_modules\.bin;E:\…\srv\clangd\…\bin;…
//! set LSP_PROBE_SAMPLES=E:\…\lsp\samples
//! set LSP_PROBE_OWN=xml=E:\…\lemminx-win32.exe;php=C:\…\php.exe|E:\…\phpactor.phar|language-server
//! cargo test --release -- --ignored lsp::probe --nocapture
//! ```
//!
//! `LSP_PROBE_DIRS` kommt vor den PATH (auch des Prozesses, damit Hüllen ihr `node` finden). Die Beispielprojekte legt
//! die Probe selbst unter `LSP_PROBE_SAMPLES` an (je Sprache ein Ordner mit Markierungsdatei und einer Datei mit
//! absichtlichem Fehler); andere Dateien öffnet sie nie. Ergebnis: Tabelle auf stderr und `probe-report.txt` dort.

use super::client::{Client, ClientLimits, Reply, State};
use super::{proto, servers};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// (Sprache, Server-Kennung, Dateien des Beispiels (Pfad, Inhalt), Datei zum Öffnen, Vervollständigen an (Zeile, Spalte)).
type Sample = (&'static str, &'static str, &'static [(&'static str, &'static str)], &'static str, (u32, u32));

const SAMPLES: &[Sample] = &[
    ("rust", "rust-analyzer", &[("Cargo.toml", "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n"), ("src/main.rs", "fn main() {\n    let x: i32 = \"text\";\n    let v: Vec<i32> = Vec::new();\n    v.\n}\n")], "src/main.rs", (3, 6)),
    ("python", "basedpyright", &[("pyproject.toml", "[project]\nname = \"probe\"\n"), ("main.py", "import os\nx: int = \"text\"\nundefined_name\nos.\n")], "main.py", (3, 3)),
    ("python", "pyright", &[("pyproject.toml", "[project]\nname = \"probe\"\n"), ("main.py", "import os\nx: int = \"text\"\nundefined_name\nos.\n")], "main.py", (3, 3)),
    ("python", "pylsp", &[("pyproject.toml", "[project]\nname = \"probe\"\n"), ("main.py", "import os\nx: int = \"text\"\ndef f(:\n    pass\nos.\n")], "main.py", (4, 3)),
    ("typescript", "typescript-language-server", &[("package.json", "{\"name\": \"probe\"}\n"), ("tsconfig.json", "{\"compilerOptions\": {\"strict\": true}}\n"), ("index.ts", "const n: number = \"text\";\nconst a = [1, 2];\na.\n")], "index.ts", (2, 2)),
    ("typescript", "tsgo", &[("package.json", "{\"name\": \"probe\"}\n"), ("tsconfig.json", "{\"compilerOptions\": {\"strict\": true}}\n"), ("index.ts", "const n: number = \"text\";\nconst a = [1, 2];\na.\n")], "index.ts", (2, 2)),
    ("c", "clangd", &[(".clangd", "CompileFlags:\n  Add: [-xc]\n"), ("main.c", "#include <stdio.h>\nint main(void) {\n    int x = undefined_var;\n    struct { int alpha; } s;\n    s.\n    return 0;\n}\n")], "main.c", (4, 6)),
    ("go", "gopls", &[("go.mod", "module probe\n\ngo 1.21\n"), ("main.go", "package main\n\nimport \"fmt\"\n\nfunc main() {\n\tx := undefined\n\tfmt.\n}\n")], "main.go", (6, 5)),
    ("lua", "lua-language-server", &[(".luarc.json", "{}\n"), ("main.lua", "local t = { alpha = 1 }\nlocal x = = 1\nt.\n")], "main.lua", (2, 2)),
    ("csharp", "csharp-ls", &[("probe.csproj", "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup><OutputType>Exe</OutputType><TargetFramework>net8.0</TargetFramework></PropertyGroup>\n</Project>\n"), ("Program.cs", "class P {\n    static void Main() {\n        int x = \"text\";\n        System.Console.\n    }\n}\n")], "Program.cs", (3, 23)),
    ("json", "vscode-json-language-server", &[("package.json", "{\"name\": \"probe\"}\n"), ("data.json", "{\n  \"a\": 1,,\n  \"b\": \n}\n")], "data.json", (2, 7)),
    ("html", "vscode-html-language-server", &[("index.html", "<!DOCTYPE html>\n<html>\n<body>\n<di\n</body>\n</html>\n")], "index.html", (3, 3)),
    ("css", "vscode-css-language-server", &[("style.css", "a { colr: red; }\nb { col\n")], "style.css", (1, 7)),
    ("yaml", "yaml-language-server", &[("a.yaml", "a: [1, 2\nb: : x\n")], "a.yaml", (0, 0)),
    ("toml", "taplo", &[("test.toml", "[a]\nb = \nc = \"x\n")], "test.toml", (0, 0)),
    ("shell", "bash-language-server", &[("script.sh", "#!/bin/bash\nif [ 1 ]; then\n  ech\n")], "script.sh", (2, 5)),
    ("latex", "texlab", &[("main.tex", "\\documentclass{article}\n\\begin{document}\n\\sec\n")], "main.tex", (2, 4)),
    ("php", "own", &[("composer.json", "{\"name\": \"probe/probe\"}\n"), ("index.php", "<?php\n$x = ;\nstr\n")], "index.php", (2, 3)),
    ("ruby", "ruby-lsp", &[("Gemfile", "source \"https://rubygems.org\"\n"), ("main.rb", "def f(\nputs\n")], "main.rb", (1, 4)),
    ("cmake", "neocmakelsp", &[("CMakeLists.txt", "cmake_minimum_required(VERSION 3.10)\nproject(probe\nadd_exe\n")], "CMakeLists.txt", (2, 7)),
    ("markdown", "marksman", &[(".marksman.toml", ""), ("README.md", "# Probe\n\n## Alpha\n\n[[nonexistent]]\n[x][undefined-ref]\n[a](#\n")], "README.md", (6, 5)),
    ("dockerfile", "docker-langserver", &[("Dockerfile", "FROM\nRUN\nCOP\n")], "Dockerfile", (2, 3)),
    ("java", "jdtls", &[("pom.xml", "<project></project>\n"), ("Main.java", "class Main { void f() { int x = \"s\"; } }\n")], "Main.java", (0, 0)),
    ("kotlin", "kotlin-language-server", &[("build.gradle.kts", ""), ("Main.kt", "fun main() { val x: Int = \"s\" }\n")], "Main.kt", (0, 0)),
    ("perl", "perlnavigator", &[("main.pl", "use strict;\nmy $x = ;\n")], "main.pl", (0, 0)),
    ("xml", "own", &[("a.xml", "<?xml version=\"1.0\"?>\n<a><b></a>\n")], "a.xml", (1, 4)),
    ("swift", "sourcekit-lsp", &[("Package.swift", "// swift-tools-version:5.5\n"), ("main.swift", "let x: Int = \"s\"\n")], "main.swift", (0, 0)),
];

struct Row {
    lang: &'static str,
    server: String,
    found: String,
    init: String,
    diags: String,
    completion: String,
    shutdown: String,
    notes: String,
}

fn probe_one(s: &Sample, root_base: &Path, env: &servers::Env, own: &[(String, Vec<String>)]) -> Row {
    let (lang, id, files, open, pos) = *s;
    let mut row = Row { lang, server: id.to_string(), found: "–".into(), init: "–".into(), diags: "–".into(), completion: "–".into(), shutdown: "–".into(), notes: String::new() };
    let dir = root_base.join(format!("{lang}-{id}"));
    let _ = std::fs::remove_dir_all(&dir);
    for (p, text) in files {
        let f = dir.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, text).unwrap();
    }
    let file = dir.join(open);
    let exists = |p: &Path| p.exists();
    let root = super::client::project_root(&file, lang, &exists, Some(root_base));
    // Nur genau diesen Server: die anderen der Sprache abwählen; eigener Pfad nur für „own“.
    let disabled: Vec<String> = servers::known(lang).iter().filter(|k| k.id != id).map(|k| k.id.to_string()).collect();
    let own_entry = own.iter().find(|(l, _)| l == lang).map(|(_, v)| v.clone()).filter(|_| id == "own");
    let (own_path, own_args) = match &own_entry {
        Some(v) if !v.is_empty() => (Some(PathBuf::from(&v[0])), v[1..].to_vec()),
        _ => (None, Vec::new()),
    };
    let env2 = servers::Env { path: env.path.clone(), exts: env.exts.clone(), project: Some(&root), system32: env.system32.clone(), rustup_size: env.rustup_size, is_file: env.is_file };
    let Some((sid, launch)) = servers::resolve(lang, own_path.as_deref(), &own_args, &disabled, &env2) else {
        row.found = "no".into();
        row.notes = "not installed / not in PATH".into();
        return row;
    };
    row.server = if sid == "own" { format!("own ({})", own_path.as_ref().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).unwrap_or_default()) } else { sid };
    row.found = launch.found.display().to_string();
    if let Some(w) = &launch.invalid {
        row.notes = format!("invalid launch: {w}");
        return row;
    }
    let limit = if lang == "rust" { Duration::from_secs(180) } else { Duration::from_secs(90) };
    let t0 = Instant::now();
    let limits = ClientLimits { init_timeout: limit, trace: std::env::var("LSP_PROBE_TRACE").is_ok(), ..Default::default() };
    let mut c = match Client::spawn(&launch.program, &launch.args, launch.raw_tail.as_deref(), &root, &root, limits, None) {
        Ok(c) => c,
        Err(e) => {
            row.init = format!("spawn failed: {e}");
            return row;
        }
    };
    let uri = proto::path_to_uri(&file.to_string_lossy());
    let text = std::fs::read_to_string(&file).unwrap();
    c.open(&uri, lang, &text);
    // initialize
    while c.state == State::Starting && t0.elapsed() < limit {
        c.poll();
        std::thread::sleep(Duration::from_millis(20));
    }
    match &c.state {
        State::Ready => row.init = format!("ok ({:?}, {:.1}s)", c.caps.encoding, t0.elapsed().as_secs_f32()),
        State::Exited(w) => {
            row.init = format!("exited: {w}");
            row.notes = last_log(&c);
            return row;
        }
        State::Starting => {
            row.init = "timeout".into();
            row.notes = last_log(&c);
            c.stop();
            return row;
        }
    }
    // Diagnosen: bis zu 60 s (rust-analyzer: bis zur Zeitgrenze, cargo check braucht)
    let until = Instant::now() + if lang == "rust" { Duration::from_secs(150) } else { Duration::from_secs(45) };
    // Speichern auslösen (manche Server prüfen erst dann).
    c.saved(&uri, &text);
    while Instant::now() < until && !c.diagnostics.get(&uri).is_some_and(|d| !d.items.is_empty()) && !matches!(c.state, State::Exited(_)) {
        c.poll();
        std::thread::sleep(Duration::from_millis(50));
    }
    // Noch nichts (Server lädt erst das Projekt, z. B. csharp-ls): wie ein Tippen den Text erneut senden.
    if !c.diagnostics.get(&uri).is_some_and(|d| !d.items.is_empty()) && c.state == State::Ready {
        c.change(&uri, &text);
        let until = Instant::now() + Duration::from_secs(20);
        while Instant::now() < until && !c.diagnostics.get(&uri).is_some_and(|d| !d.items.is_empty()) {
            c.poll();
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    row.diags = match c.diagnostics.get(&uri) {
        Some(d) if !d.items.is_empty() => format!("{} – {}", d.items.len(), d.items[0].message.lines().next().unwrap_or("").chars().take(60).collect::<String>()),
        Some(_) => "0 (empty publish)".into(),
        None => "none".into(),
    };
    // Vervollständigen
    if c.caps.completion && c.state == State::Ready {
        let p = proto::Pos { line: pos.0, character: pos.1 };
        let mut got = None;
        for attempt in 0..3 {
            if !c.complete(&uri, p, None, attempt + 1, 500) {
                break;
            }
            let t = Instant::now();
            while t.elapsed() < Duration::from_secs(20) {
                c.poll();
                let r = c.take_replies();
                if let Some(x) = r.into_iter().next() {
                    got = Some(x);
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if matches!(&got, Some(Reply::Completions { items, .. }) if !items.is_empty()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(1500));
        }
        row.completion = match got {
            Some(Reply::Completions { items, incomplete, .. }) => format!("{}{}{}", items.len(), if incomplete { " (incomplete)" } else { "" }, items.first().map(|i| format!(" e.g. {}", i.label.chars().take(20).collect::<String>())).unwrap_or_default()),
            Some(Reply::Failed { message, .. }) => format!("failed: {}", message.chars().take(50).collect::<String>()),
            None => "no reply".into(),
        };
    } else {
        row.completion = "not offered".into();
    }
    // Beenden
    let pid = c.pid;
    let ts = Instant::now();
    c.stop();
    std::thread::sleep(Duration::from_millis(1200));
    let alive = pid.is_some_and(crate::platform::process::is_alive);
    row.shutdown = format!("{} ({:.1}s total)", if alive { "still running!" } else { "ok" }, t0.elapsed().as_secs_f32() - ts.elapsed().as_secs_f32() + 1.2);
    if row.notes.is_empty() && (row.diags == "none" || row.diags.starts_with('0')) {
        row.notes = last_log(&c);
    }
    if std::env::var("LSP_PROBE_TRACE").is_ok() {
        let _ = std::fs::write(dir.join("probe-log.txt"), c.log.text());
    }
    row
}

fn last_log(c: &Client) -> String {
    c.log.lines().collect::<Vec<_>>().into_iter().rev().find(|l| l.contains("stderr") || l.contains("closed") || l.contains("failed") || l.contains("exit")).map(|l| l.chars().take(100).collect()).unwrap_or_default()
}

#[test]
#[ignore = "probe against real language servers; run by hand (see module docs)"]
fn real_servers() {
    let Ok(base) = std::env::var("LSP_PROBE_SAMPLES") else {
        eprintln!("LSP_PROBE_SAMPLES not set – skipped");
        return;
    };
    let base = PathBuf::from(base);
    std::fs::create_dir_all(&base).unwrap();
    let dirs: Vec<PathBuf> = std::env::var("LSP_PROBE_DIRS").unwrap_or_default().split(';').filter(|s| !s.is_empty()).map(PathBuf::from).collect();
    // Für Hüllen (node) und Server, die selbst Programme suchen: auch der PATH des Prozesses.
    let path = std::env::join_paths(dirs.iter().cloned().chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()))).unwrap();
    // SAFETY: nur in dieser von Hand gestarteten Probe, bevor Server-Prozesse entstehen.
    unsafe { std::env::set_var("PATH", &path) };
    let sys = servers::Env::system(None);
    let mut env_path = dirs.clone();
    env_path.extend(sys.path.iter().cloned());
    let env = servers::Env { path: env_path, exts: sys.exts.clone(), project: None, system32: sys.system32.clone(), rustup_size: sys.rustup_size, is_file: sys.is_file };
    let own: Vec<(String, Vec<String>)> = std::env::var("LSP_PROBE_OWN").unwrap_or_default().split(';').filter_map(|e| e.split_once('=')).map(|(l, v)| (l.to_string(), v.split('|').map(str::to_string).collect())).collect();
    let only = std::env::var("LSP_PROBE_ONLY").ok();
    let mut rows = Vec::new();
    for s in SAMPLES.iter().filter(|s| only.as_deref().is_none_or(|o| o.split(',').any(|x| x == s.0 || x == s.1))) {
        eprintln!("probing {} / {} …", s.0, s.1);
        let r = probe_one(s, &base, &env, &own);
        eprintln!("  {} | {} | {} | {} | {} | {} | {} | {}", r.lang, r.server, r.found, r.init, r.diags, r.completion, r.shutdown, r.notes);
        rows.push(r);
    }
    let mut out = String::from("language | server | found | init | diagnostics | completion | shutdown | notes\n");
    for r in &rows {
        out.push_str(&format!("{} | {} | {} | {} | {} | {} | {} | {}\n", r.lang, r.server, r.found, r.init, r.diags, r.completion, r.shutdown, r.notes.replace('|', "/")));
    }
    std::fs::write(base.join("probe-report.txt"), &out).unwrap();
    eprintln!("{out}");
}
