//! Glyph-Abdeckung der Oberfläche: Alle Nicht-ASCII-Zeichen in den String-/Zeichen-Literalen der Ticket-, Board-, Sprint-,
//! Dashboard-, Schnellmenü-, Terminal-Verknüpfungs- und Titelleisten-Oberfläche müssen in den geladenen Schriften vorkommen
//! (`build_fonts`), sonst zeichnet egui ein Ersatzkästchen. Der Test liest die Quelltexte zur Laufzeit, braucht kein Fenster
//! und keine Maus. Ausnahmen kommen in `ALLOWED_MISSING` (mit Begründung).

use eframe::egui::{self, FontFamily, FontId};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Zeichen, die bewusst ohne Glyph-Prüfung bleiben dürfen (derzeit keine).
const ALLOWED_MISSING: &[char] = &[];

/// Quelldateien der Oberfläche (relativ zu `src/`): ganze Ordner oder einzelne Dateien.
fn ui_sources() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("app");
    let mut files: Vec<PathBuf> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root.join("planning_ui")) {
        for e in rd.flatten() {
            let p = e.path();
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with(".rs") && !name.ends_with("_tests.rs") {
                files.push(p);
            }
        }
    }
    for f in ["terminal_links.rs", "titlebar.rs", "toasts.rs"] {
        files.push(root.join(f));
    }
    files.sort();
    files
}

/// Nicht-ASCII-Zeichen aus String- und Zeichen-Literalen (Kommentare und der Testteil `#[cfg(test)]` bleiben außen vor).
fn literal_chars(src: &str) -> Vec<char> {
    let src = match src.find("#[cfg(test)]") {
        Some(i) => &src[..i],
        None => src,
    };
    let c: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '/' if c.get(i + 1) == Some(&'/') => {
                while i < c.len() && c[i] != '\n' {
                    i += 1;
                }
            }
            '/' if c.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < c.len() && !(c[i] == '*' && c[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            // Rohe Strings r"..", r#".."#
            'r' if matches!(c.get(i + 1), Some('"') | Some('#')) && (i == 0 || !(c[i - 1].is_alphanumeric() || c[i - 1] == '_')) => {
                let mut j = i + 1;
                let mut hashes = 0;
                while c.get(j) == Some(&'#') {
                    hashes += 1;
                    j += 1;
                }
                if c.get(j) != Some(&'"') {
                    i += 1;
                    continue;
                }
                j += 1;
                while j < c.len() {
                    if c[j] == '"' && (0..hashes).all(|k| c.get(j + 1 + k) == Some(&'#')) {
                        break;
                    }
                    if !c[j].is_ascii() {
                        out.push(c[j]);
                    }
                    j += 1;
                }
                i = j + 1 + hashes;
            }
            '"' => {
                i += 1;
                while i < c.len() && c[i] != '"' {
                    if c[i] == '\\' {
                        i += 1;
                        // \u{XXXX}: das Zeichen selbst auswerten
                        if c.get(i) == Some(&'u') && c.get(i + 1) == Some(&'{') {
                            let mut hex = String::new();
                            let mut j = i + 2;
                            while j < c.len() && c[j] != '}' {
                                hex.push(c[j]);
                                j += 1;
                            }
                            if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                                if !ch.is_ascii() {
                                    out.push(ch);
                                }
                            }
                            i = j;
                        }
                    } else if !c[i].is_ascii() {
                        out.push(c[i]);
                    }
                    i += 1;
                }
                i += 1;
            }
            '\'' => {
                // Zeichen-Literal '…' (Lebenszeiten wie 'a haben kein schließendes ' direkt danach)
                if let (Some(ch), Some('\'')) = (c.get(i + 1), c.get(i + 2)) {
                    if !ch.is_ascii() {
                        out.push(*ch);
                    }
                    i += 3;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    out
}

fn scan() -> BTreeMap<char, Vec<String>> {
    let mut map: BTreeMap<char, Vec<String>> = BTreeMap::new();
    for f in ui_sources() {
        let Ok(text) = std::fs::read_to_string(&f) else { continue };
        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or("?").to_string();
        for ch in literal_chars(&text) {
            let e = map.entry(ch).or_default();
            if !e.contains(&name) {
                e.push(name.clone());
            }
        }
    }
    map
}

#[test]
fn extractor_reads_strings_and_chars_but_not_comments() {
    let src = "// ★ Kommentar\nlet a = \"▲ x\"; /* ⚑ */ let b = '▼'; let c = r#\"⎇ \"raw\"\"#; let d = \"\\u{2715}\"; fn f<'a>(x: &'a str) {}\n#[cfg(test)]\nmod t { const X: &str = \"⌨\"; }";
    let mut got = literal_chars(src);
    got.sort();
    assert_eq!(got, vec!['⎇', '▲', '▼', '✕']);
}

#[test]
fn ui_literals_only_use_characters_the_loaded_fonts_can_draw() {
    // Ohne die Windows-Systemschriften (z. B. Linux-CI) ist die Abdeckung nicht aussagekräftig.
    if !Path::new(r"C:\Windows\Fonts\segoeui.ttf").exists() {
        eprintln!("Segoe UI fehlt: Glyph-Test übersprungen");
        return;
    }
    let chars = scan();
    assert!(chars.len() > 10, "Die Quelldateien wurden nicht gefunden oder gelesen");
    let ctx = egui::Context::default();
    ctx.set_fonts(super::build_fonts(""));
    // Schriften werden erst im ersten Durchlauf geladen.
    for _ in 0..2 {
        let _ = ctx.run(egui::RawInput::default(), |_| {});
    }
    // Gegenprobe: ein Zeichen ohne Glyph wird auch als fehlend erkannt (sonst wäre der Test wertlos).
    assert!(!ctx.fonts_mut(|f| f.has_glyph(&FontId::proportional(14.0), '\u{10FFFE}')));
    eprintln!("geprüfte Zeichen: {}", chars.keys().filter(|c| !c.is_ascii()).map(|c| format!("{c} U+{:04X}", *c as u32)).collect::<Vec<_>>().join(", "));
    let families = [("proportional", FontFamily::Proportional), ("bold", FontFamily::Name("bold".into())), ("monospace", FontFamily::Monospace)];
    let mut missing: Vec<String> = Vec::new();
    for (ch, files) in &chars {
        if ALLOWED_MISSING.contains(ch) {
            continue;
        }
        for (name, fam) in &families {
            let id = FontId::new(14.0, fam.clone());
            if !ctx.fonts_mut(|f| f.has_glyph(&id, *ch)) {
                missing.push(format!("U+{:04X} '{ch}' fehlt in {name} (vorkommend in {})", *ch as u32, files.join(", ")));
            }
        }
    }
    assert!(missing.is_empty(), "Zeichen ohne Glyph in den geladenen Schriften (durch Icon/ASCII ersetzen oder in ALLOWED_MISSING begründen):\n{}", missing.join("\n"));
}
