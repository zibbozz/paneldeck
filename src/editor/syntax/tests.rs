//! Tests of the Syntax mode (#312): grammars, detection, highlighting, incremental invalidation.

use super::detect::{UserMap, detect, head_tail, modeline, shebang_interpreter};
use super::engine::{self, STEP, SyntectHighlighter};
use super::languages::{LANGUAGES, all_languages, is_known, language_name};
use super::*;
use crate::editor::{Document, EditOrigin, TextEdit};
use std::path::Path;
use std::time::Duration;

fn later() -> Instant {
    Instant::now() + Duration::from_secs(60)
}

fn det(name: &str, text: &str) -> Option<String> {
    let (h, t) = head_tail(&Rope::from_str(text));
    detect(Some(Path::new(name)), &h, &t, &UserMap::default())
}

/// Toks of all lines of `text` in `lang`.
fn toks(lang: &str, text: &str) -> Vec<Vec<(Range<usize>, Tok)>> {
    let rope = Rope::from_str(text);
    let mut h = highlighter(lang).unwrap_or_else(|| panic!("kein Highlighter für {lang}"));
    assert!(h.prepare(&rope, 0..rope.len_lines(), later()), "{lang}");
    assert!(!h.failed(), "{lang}: Grammatik-Fehler");
    (0..rope.len_lines()).map(|l| h.spans(l).unwrap_or_else(|| panic!("{lang}: Zeile {l} fehlt")).iter().map(|s| (s.range(), s.tok)).collect()).collect()
}

fn tok_at(lines: &[Vec<(Range<usize>, Tok)>], line: usize, col: usize) -> Option<Tok> {
    lines[line].iter().find(|(r, _)| r.contains(&col)).map(|(_, t)| *t)
}

#[test]
fn own_grammars_compile() {
    let set = engine::build_own().expect("eigene Grammatiken");
    let names: Vec<&str> = set.syntaxes().iter().map(|s| s.name.as_str()).collect();
    for (n, _) in engine::OWN_GRAMMARS {
        assert!(names.contains(n), "{n} fehlt in {names:?}");
    }
}

#[test]
fn every_table_language_has_a_grammar_and_a_unique_id() {
    let mut ids: Vec<&str> = LANGUAGES.iter().map(|l| l.id).collect();
    ids.sort();
    let n = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), n, "doppelte Ids");
    for l in LANGUAGES {
        assert!(l.id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()), "{}", l.id);
        if l.id == "plaintext" {
            assert!(!engine::has_grammar(l.id));
        } else {
            assert!(engine::has_grammar(l.id), "{}: Grammatik nicht gefunden", l.id);
        }
        assert!(l.exts.iter().all(|e| *e == e.to_lowercase() && !e.starts_with('.')), "{}", l.id);
    }
    // Weitere Grammatiken aus bat stehen im Wähler, mit ihrem Scope als Id.
    let all = all_languages();
    assert!(all.len() > LANGUAGES.len() + 50, "{}", all.len());
    let hs = all.iter().find(|l| l.id == "source.haskell").expect("Haskell");
    assert!(!hs.builtin && is_known(&hs.id) && engine::has_grammar(&hs.id));
    assert_eq!(language_name("rust"), Some("Rust"));
    assert_eq!(language_name("nope"), None);
}

#[test]
fn detection_by_extension_and_name() {
    let cases = [
        ("main.rs", "rust"),
        ("A.PY", "python"),
        ("app.tsx", "tsx"),
        ("x.jsx", "tsx"),
        ("index.mjs", "javascript"),
        ("lib.ts", "typescript"),
        ("page.html", "html"),
        ("site.css", "css"),
        ("data.json", "json"),
        ("Program.cs", "csharp"),
        ("a.c", "c"),
        ("a.hpp", "cpp"),
        ("A.java", "java"),
        ("i.php", "php"),
        ("s.au3", "autoit"),
        ("run.bat", "batch"),
        ("run.CMD", "batch"),
        ("CMakeLists.txt", "cmake"),
        ("x.cmake", "cmake"),
        ("main.go", "go"),
        ("setup.iss", "innosetup"),
        ("a.xml", "xml"),
        ("App.csproj", "xml"),
        ("doc.tex", "latex"),
        ("init.lua", "lua"),
        ("Makefile", "makefile"),
        ("GNUmakefile", "makefile"),
        ("rules.mk", "makefile"),
        ("installer.nsi", "nsis"),
        ("unit1.pas", "pascal"),
        ("x.pl", "perl"),
        ("Gemfile", "ruby"),
        ("x.rb", "ruby"),
        ("build.sh", "shell"),
        (".bashrc", "shell"),
        ("q.sql", "sql"),
        ("M.vb", "vb"),
        ("s.vbs", "vb"),
        ("Module1.bas", "vb"),
        ("c.yml", "yaml"),
        ("c.ini", "ini"),
        ("profile.ps1", "powershell"),
        ("Dockerfile", "dockerfile"),
        ("Dockerfile.dev", "dockerfile"),
        ("README.md", "markdown"),
        ("Cargo.toml", "toml"),
        ("Cargo.lock", "toml"),
        ("notes.txt", "plaintext"),
        ("Types.ps1xml", "xml"),
        ("site.conf", "plaintext"),
    ];
    for (name, want) in cases {
        assert_eq!(det(name, "").as_deref(), Some(want), "{name}");
    }
    // Andere Grammatiken aus bat (Id = Scope) und Namen aus bats Liste.
    assert_eq!(det("Main.hs", "").as_deref(), Some("source.haskell"));
    assert_eq!(det("go.mod", "").as_deref(), Some("source.gomod"));
    // .cls: VBA-Klassenmodul oder LaTeX-Klasse – nach dem Inhalt.
    assert_eq!(det("Class1.cls", "VERSION 1.0 CLASS\nBEGIN\n").as_deref(), Some("vb"));
    assert_eq!(det("article.cls", "\\NeedsTeXFormat{LaTeX2e}\n").as_deref(), Some("tex"));
    // Unbekannt: nichts (reiner Text).
    assert_eq!(det("data.unknownext", "hello"), None);
    assert_eq!(det("build", "x"), None, "kein ganzer Name aus bats Liste ohne Punkt/Großbuchstaben");
    assert_eq!(det("Jenkinsfile", "").as_deref(), Some("source.groovy"));
    assert_eq!(det("Build", "x"), None, "großgeschrieben, aber nicht genau so in bats Liste");
    assert_eq!(det("a.inc", "<?php echo 1;").as_deref(), Some("php"));
    assert_eq!(det("a.inc", "procedure Foo;\nbegin\nend;").as_deref(), Some("pascal"));
    assert_eq!(det("a.inc", "%macro x 0\n").as_deref(), Some("source.asm.x86_64"));
    assert_eq!(det("Prog.fs", "module M\nlet x = 1\n").as_deref(), Some("source.fsharp"));
    assert_eq!(det("shade.fs", "#version 330\nvoid main() {}\n").as_deref(), Some("source.glsl"));
    assert_eq!(det("nginx.conf", "server {\n  listen 80;\n}\n").as_deref(), Some("source.nginx"));
    assert_eq!(det("app.conf", "[main]\nkey=value\n").as_deref(), Some("ini"));
    assert_eq!(det("page", "<!DOCTYPE html>\n<html></html>\n").as_deref(), Some("html"), "nicht Svelte");
    let (h, t) = head_tail(&Rope::from_str("hello"));
    assert_eq!(detect(None, &h, &t, &UserMap::default()), None);
}

#[test]
fn detection_by_shebang_modeline_and_first_line() {
    assert_eq!(shebang_interpreter("#!/usr/bin/env python3").as_deref(), Some("python"));
    assert_eq!(shebang_interpreter("#!/usr/bin/env -S node --flag").as_deref(), Some("node"));
    assert_eq!(shebang_interpreter("#!/bin/bash -e").as_deref(), Some("bash"));
    assert_eq!(shebang_interpreter("#! /usr/bin/python3.11").as_deref(), Some("python"));
    assert_eq!(shebang_interpreter("no shebang"), None);
    assert_eq!(det("script", "#!/usr/bin/env python3\nprint(1)\n").as_deref(), Some("python"));
    assert_eq!(det("tool", "#!/bin/sh\necho hi\n").as_deref(), Some("shell"));
    assert_eq!(det("run", "#!/usr/bin/env pwsh\nWrite-Host hi\n").as_deref(), Some("powershell"));
    assert_eq!(det("srv", "#!/usr/bin/env node\n").as_deref(), Some("javascript"));
    // Die Endung geht vor dem Shebang.
    assert_eq!(det("x.rb", "#!/usr/bin/env python\n").as_deref(), Some("ruby"));

    assert_eq!(modeline("# vim: set ft=python :").as_deref(), Some("python"));
    assert_eq!(modeline("// vim: filetype=rust").as_deref(), Some("rust"));
    assert_eq!(modeline("# -*- mode: ruby; coding: utf-8 -*-").as_deref(), Some("ruby"));
    assert_eq!(modeline("/* -*- c++ -*- */").as_deref(), Some("c++"));
    assert_eq!(modeline("nothing here"), None);
    assert_eq!(det("conf", "# vim: ft=yaml\na: 1\n").as_deref(), Some("yaml"));
    assert_eq!(det("thing", "/* -*- c++ -*- */\nint x;\n").as_deref(), Some("cpp"));
    // Modeline am Dateiende.
    let text = format!("{}# vim: set ft=sh:\n", "x\n".repeat(20));
    assert_eq!(det("noext", &text).as_deref(), Some("shell"));
    // Erste Zeile nach den Mustern der bat-Grammatiken.
    assert_eq!(det("feed", "<?xml version=\"1.0\"?>\n<a/>\n").as_deref(), Some("xml"));
}

#[test]
fn user_mappings_come_first() {
    let user = UserMap { entries: vec![("*.tpl".into(), "html".into()), ("Jenkinsfile".into(), "source.groovy".into()), (".rs".into(), "plaintext".into()), ("*.blade.php".into(), "html".into())] };
    let d = |n: &str| detect(Some(Path::new(n)), "", "", &user);
    assert_eq!(d("page.TPL").as_deref(), Some("html"));
    assert_eq!(d("jenkinsfile").as_deref(), Some("source.groovy"));
    assert_eq!(d("main.rs").as_deref(), Some("plaintext"), "eigene Zuordnung schlägt die eingebaute");
    assert_eq!(d("view.blade.php").as_deref(), Some("html"), "längste Endung gewinnt");
    assert_eq!(d("x.php").as_deref(), Some("php"));
    assert_eq!(user.lookup("tpl"), None, "ein Name ohne Punkt ist kein Treffer für *.tpl");
    let dots = UserMap { entries: vec![(".clang-format".into(), "yaml".into()), ("*.prettierrc".into(), "json".into()), (".envrc".into(), "shell".into())] };
    assert_eq!(dots.lookup(".clang-format"), Some("yaml"), "Punktdatei selbst");
    assert_eq!(dots.lookup(".prettierrc"), Some("json"));
    assert_eq!(dots.lookup(".ENVRC"), Some("shell"));
}

/// Kleine Probe je Sprache der Tabelle.
fn sample(id: &str) -> &'static str {
    match id {
        "autoit" => "#include <File.au3>\n; comment\nLocal $x = 5\nFunc Foo($a)\n  Return \"s\" & @CRLF\nEndFunc\n#cs\nblock\n#ce\n",
        "batch" => "@echo off\nREM comment\nset X=1\necho %X%\n:label\ngoto label\n",
        "bibtex" => "@article{key,\n  title = {T},\n  year = 2020\n}\n",
        "c" => "#include <stdio.h>\n// c\nint main(void) { return 0; }\n",
        "csharp" => "using System;\n// c\nclass A { int x = 1; string s = \"a\"; }\n",
        "cpp" => "#include <vector>\n// c\nint main() { std::vector<int> v; return 0; }\n",
        "cmake" => "# c\ncmake_minimum_required(VERSION 3.10)\nproject(Foo)\nset(X \"a\")\n",
        "css" => "/* c */\nbody { color: red; margin: 0px; }\n",
        "diff" => "--- a\n+++ b\n@@ -1 +1 @@\n-old\n+new\n",
        "dockerfile" => "# c\nFROM ubuntu:22.04\nRUN echo \"hi\"\n",
        "go" => "package main\n// c\nfunc main() { x := 1; _ = \"s\" }\n",
        "html" => "<!DOCTYPE html>\n<html><body class=\"a\">Hi</body></html>\n",
        "ini" => "; c\n[section]\nkey=value\n",
        "innosetup" => "[Setup]\nAppName=Foo\n; comment\n[Files]\nSource: \"a.exe\"; DestDir: \"{app}\"; Flags: ignoreversion\n#define X 1\n[Code]\nfunction Foo: Boolean;\nbegin\n  Result := True; // c\nend;\n",
        "java" => "// c\npublic class A { int x = 1; String s = \"a\"; }\n",
        "javascript" => "// c\nconst x = 1;\nfunction f() { return \"s\"; }\n",
        "json" => "{\"a\": 1, \"b\": \"s\", \"c\": true}\n",
        "kotlin" => "// c\nfun main() { val x = 1 }\n",
        "latex" => "\\documentclass{article}\n% c\n\\begin{document}\nHi $x^2$\n\\end{document}\n",
        "less" => "// c\n@x: 1px;\n.a { margin: @x; }\n",
        "lua" => "-- c\nlocal x = 1\nfunction f() return \"s\" end\n",
        "makefile" => "# c\nCC = gcc\nall: main.o\n\t$(CC) -o app main.o\n",
        "markdown" => "# Title\n\nSome *text* and `code`.\n",
        "nsis" => "; c\nName \"Foo\"\nSection \"Main\"\n  SetOutPath $INSTDIR\nSectionEnd\n",
        "pascal" => "program P;\n{ c }\nvar x: Integer;\nbegin\n  x := 1;\nend.\n",
        "perl" => "# c\nmy $x = 1;\nprint \"s\";\n",
        "php" => "<?php\n// c\n$x = 1;\necho \"s\";\n",
        "powershell" => "# c\n$x = 5\nGet-ChildItem -Path \"C:\\\" | Where-Object { $_.Length -gt 1 }\nfunction Foo { param([string]$a) }\n<# block\ncomment #>\n",
        "python" => "# c\ndef f(x):\n    return \"s\" + str(1)\n",
        "ruby" => "# c\ndef f\n  x = 1\n  \"s\"\nend\n",
        "rust" => "// c\nfn main() { let x = 1; let s = \"a\"; }\n",
        "scss" => "// c\n$x: 1px;\n.a { margin: $x; }\n",
        "shell" => "#!/bin/bash\n# c\nX=1\necho \"$X\"\nif [ -f a ]; then echo y; fi\n",
        "sql" => "-- c\nSELECT a, 1 FROM t WHERE b = 'x';\n",
        "swift" => "// c\nlet x = 1\nfunc f() -> String { return \"s\" }\n",
        "tex" => "% c\n\\def\\foo{bar}\n",
        "toml" => "# c\n[package]\nname = \"x\"\nversion = 1\n",
        "typescript" => "// c\nconst x: number = 1;\nfunction f(): string { return \"s\"; }\n",
        "tsx" => "// c\nconst A = () => <div className=\"a\">Hi</div>;\n",
        "vb" => "' c\nModule M\n  Sub Main()\n    Dim x As Integer = 1\n    Console.WriteLine(\"s\")\n  End Sub\nEnd Module\n",
        "xml" => "<?xml version=\"1.0\"?>\n<!-- c -->\n<a b=\"c\">t</a>\n",
        "yaml" => "# c\nkey: value\nnum: 1\n",
        other => panic!("keine Probe für {other}"),
    }
}

#[test]
fn every_language_highlights_its_sample_with_several_colours() {
    for l in LANGUAGES.iter().filter(|l| l.id != "plaintext") {
        let text = sample(l.id);
        let lines = toks(l.id, text);
        let rope = Rope::from_str(text);
        let mut kinds: Vec<Tok> = lines.iter().flatten().map(|(_, t)| *t).collect();
        kinds.sort_by_key(|t| t.index());
        kinds.dedup();
        assert!(kinds.len() > 1, "{}: nur {kinds:?}", l.id);
        for (i, spans) in lines.iter().enumerate() {
            let len = crate::editor::text::line_len(&rope, i);
            for (r, _) in spans {
                assert!(r.start < r.end && r.end <= len, "{}: Zeile {i} {r:?} (Länge {len})", l.id);
            }
            assert!(spans.windows(2).all(|w| w[0].0.end <= w[1].0.start), "{}: überlappend", l.id);
        }
    }
    // Ein paar Stichproben.
    let rs = toks("rust", sample("rust"));
    assert_eq!(tok_at(&rs, 0, 1), Some(Tok::Comment));
    assert_eq!(tok_at(&rs, 1, 0), Some(Tok::Keyword), "fn");
    let ps = toks("powershell", sample("powershell"));
    assert_eq!(tok_at(&ps, 1, 0), Some(Tok::Variable), "$x");
    assert_eq!(tok_at(&ps, 2, 0), Some(Tok::Function), "Get-ChildItem");
    assert_eq!(tok_at(&ps, 5, 3), Some(Tok::Comment), "Blockkommentar über zwei Zeilen");
    let au = toks("autoit", sample("autoit"));
    assert_eq!(tok_at(&au, 0, 0), Some(Tok::Preprocessor));
    assert_eq!(tok_at(&au, 7, 0), Some(Tok::Comment), "#cs … #ce");
    let iss = toks("innosetup", sample("innosetup"));
    assert_eq!(tok_at(&iss, 0, 1), Some(Tok::Heading), "[Setup]");
    assert_eq!(tok_at(&iss, 2, 0), Some(Tok::Comment));
    assert_eq!(tok_at(&iss, 8, 0), Some(Tok::Keyword), "begin im [Code]-Teil");
    assert_eq!(tok_at(&iss, 9, 12), Some(Tok::Constant), "True");
    assert_eq!(tok_at(&iss, 4, 30), Some(Tok::Constant), "{{app}}");
    let vb = toks("vb", sample("vb"));
    assert_eq!(tok_at(&vb, 0, 0), Some(Tok::Comment));
    assert_eq!(tok_at(&vb, 2, 6), Some(Tok::Function), "Sub Main");
    let json = toks("json", sample("json"));
    assert_eq!(tok_at(&json, 0, 2), Some(Tok::Attribute), "Schlüssel");
    assert_eq!(tok_at(&json, 0, 15), Some(Tok::String), "Wert");
}

#[test]
fn all_other_bundled_grammars_survive_a_sample() {
    let text = "a = 1 // x\n\"s\" 'c' # y\n<b c=\"d\">e</b>\nfunction f() { return 2; }\n";
    let rope = Rope::from_str(text);
    for l in all_languages().iter().filter(|l| !l.builtin) {
        let mut h = highlighter(&l.id).unwrap_or_else(|| panic!("{}", l.id));
        assert!(h.prepare(&rope, 0..rope.len_lines(), later()), "{}", l.id);
    }
}

#[test]
fn edits_invalidate_from_the_changed_line() {
    let text: String = (0..100).map(|i| format!("int x{i} = {i};\n")).collect();
    let mut doc = Document::from_text(&text);
    let mut hl = DocHighlight::new("c", &doc).unwrap();
    let wait = Duration::from_secs(20);
    assert!(hl.wait(&doc, 0..100, wait));
    assert_eq!(hl.spans(95).unwrap()[0].tok, Tok::Keyword, "int");
    // Block-Kommentar in Zeile 10 öffnen: alles darunter wird Kommentar – auch in einer anderen „Ansicht“.
    let mut sels = doc.new_selections(0);
    let pos = doc.line_to_char(10);
    doc.apply_edits(&mut sels, vec![TextEdit::insert(pos, "/* ")], EditOrigin::User).unwrap();
    assert!(hl.spans(95).is_some(), "noch nicht abgeglichen");
    assert!(!hl.prepare(&doc, 0..100), "neue Farben kommen erst aus dem Hilfsthread");
    assert_eq!(hl.spans(95).map(|s| s[0].tok), Some(Tok::Keyword), "bis dahin die alten Farben (gleiche Zeilenzahl)");
    assert!(hl.wait(&doc, 0..100, wait));
    assert!(hl.spans(95).unwrap().iter().all(|s| s.tok == Tok::Comment), "{:?}", hl.spans(95));
    assert_eq!(hl.spans(5).map(|s| s[0].tok), Some(Tok::Keyword), "davor unverändert");
    // Rückgängig: wieder Code.
    doc.undo(&mut sels);
    assert!(hl.wait(&doc, 90..100, wait));
    assert_eq!(hl.spans(95).unwrap()[0].tok, Tok::Keyword);
}

#[test]
fn budget_limits_work_per_frame_and_checkpoints_resume() {
    let text: String = (0..20_000).map(|i| format!("let v{i} = \"{i}\"; // n\n")).collect();
    let rope = Rope::from_str(&text);
    let mut h = SyntectHighlighter::new("rust").unwrap();
    // Abgelaufene Frist: höchstens eine Zeile je Aufruf, nicht fertig.
    assert!(!h.prepare(&rope, 19_990..20_000, Instant::now()));
    let mut rounds = 0;
    while !h.prepare(&rope, 19_990..20_000, Instant::now() + Duration::from_millis(5)) {
        rounds += 1;
        assert!(rounds < 100_000);
    }
    assert!(h.checkpoint_count() >= 19_990 / STEP, "Zwischenstände unterwegs gemerkt");
    assert!(h.spans(19_995).is_some());
    assert!(h.spans(100).is_none(), "nur die gezeichneten Zeilen (und ein Rand) bleiben");
    // Invalidieren behält die Zwischenstände davor.
    h.invalidate_from(100);
    assert_eq!(h.checkpoint_count(), 100 / STEP + 1);
}

/// Jeder Aufruf kommt voran, auch wenn schon eine Zeile das Budget sprengt (Markdown, Sprung ans Ende): Mit abgelaufener
/// Frist arbeitet jeder Aufruf genau eine Zeile ab und merkt sich, wo er stand.
#[test]
fn every_call_makes_progress_even_with_no_budget() {
    let readme = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md")).unwrap();
    let rope = Rope::from_str(&readme);
    let n = rope.len_lines();
    let mut h = SyntectHighlighter::new("markdown").unwrap();
    let mut calls = 0;
    while !h.prepare(&rope, 0..n.min(60), Instant::now()) {
        calls += 1;
        assert!(calls <= n.min(60) + 1, "kein Fortschritt nach {calls} Aufrufen");
    }
    assert!(h.spans(0).is_some());
    // Ende einer langen Datei ohne Budget: höchstens eine Zeile je Aufruf, aber stetig.
    let text: String = (0..3_000).map(|i| format!("# Kapitel {i}\n\nText *kursiv* `code` {i}\n")).collect();
    let rope = Rope::from_str(&text);
    let n = rope.len_lines();
    let mut h = SyntectHighlighter::new("markdown").unwrap();
    let mut calls = 0;
    while !h.prepare(&rope, n - 40..n, Instant::now()) {
        calls += 1;
        assert!(calls <= n + 1, "kein Fortschritt nach {calls} Aufrufen");
    }
    assert!(h.spans(n - 4).is_some_and(|s| s.iter().any(|x| x.tok == Tok::Heading)), "{:?}", h.spans(n - 4));
}

#[test]
fn the_worker_highlights_without_blocking_and_follows_scrolling() {
    let text: String = (0..5_000).map(|i| format!("const x{i}: number = {i}; // c\n")).collect();
    let doc = Document::from_text(&text);
    let mut hl = DocHighlight::new("typescript", &doc).unwrap();
    let t = Instant::now();
    let first = hl.prepare(&doc, 4_900..4_950);
    assert!(t.elapsed() < Duration::from_millis(50), "prepare blockiert nicht ({:?})", t.elapsed());
    assert!(!first, "beim ersten Aufruf noch nichts fertig");
    assert!(hl.wait(&doc, 4_900..4_950, Duration::from_secs(30)));
    assert_eq!(hl.spans(4_920).unwrap()[0].tok, Tok::Keyword, "const");
    assert!(hl.wait(&doc, 0..50, Duration::from_secs(30)), "zurück an den Anfang");
    assert!(!hl.failed());
    assert!(DocHighlight::new("plaintext", &doc).is_none());
}

#[test]
fn very_long_lines_stay_plain_and_keep_the_state() {
    let long = format!("let s = \"{}\";", "x".repeat(MAX_PARSE_LINE + 10));
    let text = format!("// a\n{long}\nfn f() {{}}\n");
    let lines = toks("rust", &text);
    assert!(lines[1].is_empty(), "sehr lange Zeile ohne Farben");
    assert_eq!(tok_at(&lines, 2, 0), Some(Tok::Keyword));
}

#[test]
fn modes_read_leniently() {
    assert_eq!(Mode::from_id("Syntax"), Some(Mode::Syntax));
    assert_eq!(Mode::from_id("text"), Some(Mode::Text));
    assert_eq!(Mode::from_id("smart"), None, "kommt später; ältere Versionen fallen auf den Standard zurück");
    assert_eq!(serde_json::to_string(&Mode::Syntax).unwrap(), "\"syntax\"");
    assert_eq!(Mode::default(), Mode::Text, "neue Dateien: Text");
}

/// Lizenztexte der bat-Grammatiken, die eine Nennung verlangen (für `THIRD_PARTY_LICENSES.txt`).
fn grammar_licence_text() -> String {
    let ack = two_face::acknowledgement::listing();
    let mut md = String::from(
        "Grammars of the editor's syntax colours: the syntax definitions curated by the bat project\n(https://github.com/sharkdp/bat), embedded through the crate two-face. Licences that require an\nacknowledgement, one per grammar folder:\n",
    );
    for l in ack.for_syntaxes().iter().filter(|l| l.needs_acknowledgement()) {
        let path = l.rel_path.to_string_lossy().replace('\\', "/");
        md.push_str(&format!("\n--- {path}\n\n{}\n", l.text.trim().replace("\r\n", "\n")));
    }
    md
}

/// `tools/license-extra/two-face-syntaxes.md` passt zur eingebundenen Version von two-face. Neu schreiben:
/// `WRITE_GRAMMAR_LICENSES=1 cargo test --release grammar_licences`, danach `python tools/gen_licenses.py`.
#[test]
fn grammar_licences_are_listed() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools").join("license-extra").join("two-face-syntaxes.md");
    let want = grammar_licence_text();
    if std::env::var_os("WRITE_GRAMMAR_LICENSES").is_some() {
        std::fs::write(&path, &want).unwrap();
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
    assert!(have == want, "{} ist veraltet: WRITE_GRAMMAR_LICENSES=1 cargo test --release grammar_licences", path.display());
    assert!(want.contains("MIT"), "Lizenztexte enthalten");
}
