//! Theme-Dateien `themes/*.json` (öffentliches Format): lesen mit klarer Fehlermeldung, schreiben, Größengrenze,
//! eindeutige Namen. Neu in 2.6.0 (#309) ist nur das optionale Objekt `ui` (Oberflächenfarben); fehlt es oder ein
//! Wert, gilt die Ableitung. Unbekannte Felder und Namen werden ignoriert (ältere/neuere Versionen).

use super::color::{parse_hex, to_hex};
use super::tokens::{find_cycle, Spec, SpecError, UI_TOKENS};
use super::Theme;
use eframe::egui::Color32;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Größte Theme-Datei (Import und Laden).
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Meldung in beiden Sprachen.
#[derive(Clone, Debug, PartialEq)]
pub struct Msg {
    pub de: String,
    pub en: String,
}

impl Msg {
    pub fn new(de: impl Into<String>, en: impl Into<String>) -> Msg {
        Msg { de: de.into(), en: en.into() }
    }

    pub fn get(&self, lang: crate::state::Lang) -> &str {
        match lang {
            crate::state::Lang::De => &self.de,
            crate::state::Lang::En => &self.en,
        }
    }
}

/// Eine Theme-Datei, die nicht geladen werden konnte.
#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub path: PathBuf,
    pub msg: Msg,
}

fn field_err(field: &str, value: &str) -> Msg {
    Msg::new(
        format!("Ungültige Farbe in „{field}“: „{value}“ (erwartet #RRGGBB oder #RRGGBBAA)"),
        format!("Invalid colour in “{field}”: “{value}” (expected #RRGGBB or #RRGGBBAA)"),
    )
}

fn color_field(obj: &serde_json::Map<String, Value>, key: &str, required: bool) -> Result<Option<Color32>, Msg> {
    match obj.get(key) {
        None | Some(Value::Null) if !required => Ok(None),
        None | Some(Value::Null) => Err(Msg::new(format!("Pflichtfeld „{key}“ fehlt"), format!("Required field “{key}” is missing"))),
        Some(Value::String(s)) => parse_hex(s).map(Some).ok_or_else(|| field_err(key, s)),
        Some(v) => Err(field_err(key, &v.to_string())),
    }
}

/// Theme aus dem Text einer Datei lesen und prüfen. Gibt eine Meldung mit dem Grund zurück, wenn es ungültig ist.
pub fn parse_str(text: &str) -> Result<Theme, Msg> {
    if text.len() as u64 > MAX_FILE_BYTES {
        return Err(too_large());
    }
    let v: Value = serde_json::from_str(crate::state::strip_bom(text)).map_err(|e| Msg::new(format!("Kein gültiges JSON: {e}"), format!("Not valid JSON: {e}")))?;
    let Value::Object(obj) = v else {
        return Err(Msg::new("Die Datei muss ein JSON-Objekt sein", "The file must contain a JSON object"));
    };
    let name = match obj.get("name") {
        Some(Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
        Some(Value::String(_)) => return Err(Msg::new("„name“ ist leer", "“name” is empty")),
        Some(_) => return Err(Msg::new("„name“ muss ein Text sein", "“name” must be a string")),
        None => return Err(Msg::new("Pflichtfeld „name“ fehlt", "Required field “name” is missing")),
    };
    if name.chars().count() > 80 {
        return Err(Msg::new("„name“ ist länger als 80 Zeichen", "“name” is longer than 80 characters"));
    }
    let fg = color_field(&obj, "fg", true)?.unwrap_or(Color32::WHITE);
    let bg = color_field(&obj, "bg", true)?.unwrap_or(Color32::BLACK);
    // Terminalfarben ohne Transparenz (wie bisher); der Hintergrund bekommt die Deckkraft aus den Einstellungen.
    let solid = |c: Color32| {
        let [r, g, b, _] = c.to_srgba_unmultiplied();
        Color32::from_rgb(r, g, b)
    };
    let (fg, bg) = (solid(fg), solid(bg));
    let cursor = color_field(&obj, "cursor", false)?.map(solid).unwrap_or(fg);
    let selection = color_field(&obj, "selection", false)?.map(solid).unwrap_or_else(|| crate::style::mix(bg, fg, 0.2));
    let dark = match obj.get("dark") {
        None | Some(Value::Null) => super::luma(bg) < 128.0,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(Msg::new("„dark“ muss true oder false sein", "“dark” must be true or false")),
    };
    let arr = match obj.get("ansi") {
        Some(Value::Array(a)) => a,
        Some(_) => return Err(Msg::new("„ansi“ muss eine Liste mit 16 Farben sein", "“ansi” must be a list of 16 colours")),
        None => return Err(Msg::new("Pflichtfeld „ansi“ fehlt", "Required field “ansi” is missing")),
    };
    if arr.len() < 16 {
        return Err(Msg::new(format!("„ansi“ braucht 16 Farben (gefunden: {})", arr.len()), format!("“ansi” needs 16 colours (found: {})", arr.len())));
    }
    let mut ansi = [Color32::BLACK; 16];
    for (i, v) in arr.iter().take(16).enumerate() {
        let key = format!("ansi[{i}]");
        ansi[i] = match v {
            Value::String(s) => solid(parse_hex(s).ok_or_else(|| field_err(&key, s))?),
            other => return Err(field_err(&key, &other.to_string())),
        };
    }
    let mut ui = BTreeMap::new();
    match obj.get("ui") {
        None | Some(Value::Null) => {}
        Some(Value::Object(m)) => {
            for (k, v) in m {
                // Unbekannte Namen (neuere Versionen) ignorieren.
                if !UI_TOKENS.iter().any(|t| t.name == k) {
                    continue;
                }
                let Value::String(s) = v else {
                    return Err(Msg::new(format!("„ui.{k}“ muss ein Text sein"), format!("“ui.{k}” must be a string")));
                };
                match Spec::parse(s) {
                    Ok(spec) => {
                        ui.insert(k.clone(), spec);
                    }
                    // Verweis auf eine Farbe, die diese Version nicht kennt: wie abgeleitet.
                    Err(SpecError::Unknown(_)) => {}
                    Err(SpecError::Alpha) => {
                        return Err(Msg::new(
                            format!("Ungültige Deckkraft in „ui.{k}“: „{s}“ (erwartet 0–100, z. B. @text/20)"),
                            format!("Invalid opacity in “ui.{k}”: “{s}” (expected 0–100, e.g. @text/20)"),
                        ))
                    }
                    Err(SpecError::Syntax) => {
                        return Err(Msg::new(
                            format!("Ungültiger Wert für „ui.{k}“: „{s}“ (erwartet #RRGGBB, #RRGGBBAA oder @name bzw. @name/20)"),
                            format!("Invalid value for “ui.{k}”: “{s}” (expected #RRGGBB, #RRGGBBAA or @name or @name/20)"),
                        ))
                    }
                }
            }
        }
        Some(_) => return Err(Msg::new("„ui“ muss ein Objekt sein", "“ui” must be an object")),
    }
    let t = Theme { name, dark, fg, bg, cursor, selection, ansi, ui, builtin: false, source: None };
    if let Some(c) = find_cycle(&t) {
        let chain = c.join(" → ");
        return Err(Msg::new(format!("Verweise im Kreis: {chain}"), format!("Links form a loop: {chain}")));
    }
    Ok(t)
}

fn too_large() -> Msg {
    Msg::new("Die Datei ist größer als 1 MB", "The file is larger than 1 MB")
}

/// Datei lesen (höchstens [`MAX_FILE_BYTES`]) und prüfen.
pub fn read_file(path: &Path) -> Result<Theme, Msg> {
    let meta = std::fs::metadata(path).map_err(|e| Msg::new(format!("Datei nicht lesbar: {e}"), format!("Cannot read the file: {e}")))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(too_large());
    }
    let bytes = std::fs::read(path).map_err(|e| Msg::new(format!("Datei nicht lesbar: {e}"), format!("Cannot read the file: {e}")))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(too_large());
    }
    let text = String::from_utf8(bytes).map_err(|_| Msg::new("Die Datei ist kein UTF-8-Text", "The file is not UTF-8 text"))?;
    let mut t = parse_str(&text)?;
    t.source = Some(path.to_path_buf());
    Ok(t)
}

/// Eigene Themes aus `<dir>/*.json` laden. Ungültige Dateien und Namen, die schon vergeben sind (eingebaute Themes
/// oder eine frühere Datei), landen mit Grund in der zweiten Liste und sind nicht wählbar.
pub fn load_dir(dir: &Path, builtin: &[Theme]) -> (Vec<Theme>, Vec<Issue>) {
    let (mut out, mut issues) = (Vec::new(), Vec::new());
    let Ok(rd) = std::fs::read_dir(dir) else { return (out, issues) };
    let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")) && p.is_file()).collect();
    files.sort();
    for p in files {
        match read_file(&p) {
            Ok(t) => {
                if builtin.iter().chain(out.iter()).any(|o: &Theme| o.name.eq_ignore_ascii_case(&t.name)) {
                    issues.push(Issue {
                        msg: Msg::new(format!("Der Name „{}“ ist schon vergeben", t.name), format!("The name “{}” is already used", t.name)),
                        path: p,
                    });
                } else {
                    out.push(t);
                }
            }
            Err(msg) => issues.push(Issue { path: p, msg }),
        }
    }
    (out, issues)
}

/// JSON-Text eines Themes (feste Reihenfolge, `ui` nur mit gesetzten Werten in der Reihenfolge der Tabelle).
pub fn to_json(t: &Theme) -> String {
    let q = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    let mut s = String::from("{\n");
    s.push_str(&format!("  \"name\": {},\n", q(&t.name)));
    s.push_str(&format!("  \"dark\": {},\n", t.dark));
    for (k, c) in [("fg", t.fg), ("bg", t.bg), ("cursor", t.cursor), ("selection", t.selection)] {
        let [r, g, b, _] = c.to_srgba_unmultiplied();
        s.push_str(&format!("  \"{k}\": \"{}\",\n", to_hex(Color32::from_rgb(r, g, b))));
    }
    s.push_str("  \"ansi\": [\n");
    for row in 0..2 {
        let items: Vec<String> = (0..8).map(|i| format!("\"{}\"", to_hex(t.ansi[row * 8 + i]))).collect();
        s.push_str(&format!("    {}{}\n", items.join(", "), if row == 0 { "," } else { "" }));
    }
    s.push_str("  ]");
    let set: Vec<(&str, &Spec)> = UI_TOKENS.iter().filter_map(|tok| t.ui.get(tok.name).map(|v| (tok.name, v))).collect();
    if set.is_empty() {
        s.push('\n');
    } else {
        s.push_str(",\n  \"ui\": {\n");
        for (i, (k, v)) in set.iter().enumerate() {
            s.push_str(&format!("    \"{k}\": \"{}\"{}\n", v.to_text(), if i + 1 < set.len() { "," } else { "" }));
        }
        s.push_str("  }\n");
    }
    s.push_str("}\n");
    s
}

/// Name ohne angehängte Nummer („One Dark (3)“ → „One Dark“).
fn base_name(name: &str) -> &str {
    let n = name.trim();
    if let Some(open) = n.rfind(" (") {
        let inner = &n[open + 2..];
        if let Some(num) = inner.strip_suffix(')') {
            if !num.is_empty() && num.bytes().all(|b| b.is_ascii_digit()) {
                return &n[..open];
            }
        }
    }
    n
}

/// Freier Name: `name`, sonst „name (2)“, „name (3)“ … (eine vorhandene Nummer wird ersetzt). Vergleich ohne
/// Groß-/Kleinschreibung.
pub fn unique_name(name: &str, taken: &[String]) -> String {
    let used = |n: &str| taken.iter().any(|t| t.eq_ignore_ascii_case(n));
    let n = name.trim();
    if !used(n) {
        return n.to_string();
    }
    let base = base_name(n);
    (2..).map(|i| format!("{base} ({i})")).find(|c| !used(c)).unwrap_or_else(|| n.to_string())
}

/// Freier Dateiname in `dir` zu einem Theme-Namen (Kleinbuchstaben, Ziffern, Bindestriche).
pub fn file_for(dir: &Path, name: &str) -> PathBuf {
    let mut slug = String::new();
    for ch in name.chars().flat_map(|c| c.to_lowercase()) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
        if slug.len() >= 40 {
            break;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    let slug = if slug.is_empty() { "theme".to_string() } else { slug };
    let mut p = dir.join(format!("{slug}.json"));
    let mut i = 2;
    while p.exists() {
        p = dir.join(format!("{slug}-{i}.json"));
        i += 1;
    }
    p
}

/// Theme atomar schreiben (temporäre Datei, dann umbenennen).
pub fn write(path: &Path, t: &Theme) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, to_json(t))?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANSI: &str = r##""ansi":["#000000","#111111","#222222","#333333","#444444","#555555","#666666","#777777","#888888","#999999","#aaaaaa","#bbbbbb","#cccccc","#dddddd","#eeeeee","#ffffff"]"##;

    fn file(extra: &str) -> String {
        format!(r##"{{"name":"X","fg":"#ffffff","bg":"#000000",{ANSI}{extra}}}"##)
    }

    #[test]
    fn old_files_stay_valid() {
        let t = parse_str(&file("")).unwrap();
        assert!(t.dark && t.ui.is_empty());
        assert_eq!(t.cursor, t.fg, "Cursor abgeleitet");
        assert_eq!(t.ansi[15], Color32::WHITE);
        // Unbekannte Felder (neuere Versionen) und unbekannte ui-Namen werden ignoriert.
        let t = parse_str(&file(r##","future":1,"ui":{"nope":"#ffffff","accent":"#0080ff","border":"@future_token/20"}"##)).unwrap();
        assert_eq!(t.ui.len(), 1);
        assert_eq!(t.ui["accent"], Spec::Hex(Color32::from_rgb(0, 0x80, 0xff)));
        // BOM wie von PowerShell 5.1.
        assert!(parse_str(&format!("\u{feff}{}", file(""))).is_ok());
    }

    #[test]
    fn invalid_files_name_the_problem() {
        let err = |s: &str| parse_str(s).unwrap_err().en;
        assert!(err("{").starts_with("Not valid JSON"));
        assert!(err("[]").contains("JSON object"));
        assert!(err(&file("").replace("\"#ffffff\",\"bg\"", "\"white\",\"bg\"")).contains("“fg”"));
        assert!(err(&file("").replace("\"name\":\"X\",", "")).contains("“name” is missing"));
        assert!(err(&file("").replace(",\"#ffffff\"]", "]")).contains("16 colours (found: 15)"));
        assert!(err(&file(r##","ui":{"border":"@text/150"}"##)).contains("opacity"));
        assert!(err(&file(r##","ui":{"border":"red"}"##)).contains("“ui.border”"));
        assert!(err(&file(r##","ui":{"border":3}"##)).contains("must be a string"));
        assert!(err(&file(r##","ui":[]"##)).contains("must be an object"));
        assert!(err(&file(r##","dark":"yes""##)).contains("true or false"));
        assert!(err(&file(r##","ui":{"text":"@border","border":"@text/20"}"##)).contains("loop"));
        let de = parse_str(&file(r##","ui":{"text":"@border","border":"@text/20"}"##)).unwrap_err().de;
        assert!(de.contains("Kreis"));
    }

    #[test]
    fn size_limit_is_one_megabyte() {
        let big = format!("{}{}", file(""), " ".repeat(MAX_FILE_BYTES as usize));
        assert!(parse_str(&big).unwrap_err().en.contains("1 MB"));
        let dir = std::env::temp_dir().join(format!("paneldeck-test-themesize-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("big.json"), &big).unwrap();
        std::fs::write(dir.join("ok.json"), file("")).unwrap();
        std::fs::write(dir.join("dup.json"), file("").replace("\"X\"", "\"One Dark\"")).unwrap();
        let (themes, issues) = load_dir(&dir, &crate::theme::all());
        assert_eq!(themes.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["X"]);
        assert_eq!(themes[0].source.as_deref(), Some(dir.join("ok.json").as_path()));
        assert_eq!(issues.len(), 2, "{issues:?}");
        assert!(issues.iter().any(|i| i.path.ends_with("big.json") && i.msg.en.contains("1 MB")));
        assert!(issues.iter().any(|i| i.path.ends_with("dup.json") && i.msg.en.contains("already used")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn format_round_trip() {
        for mut t in crate::theme::all() {
            t.ui.insert("accent".into(), Spec::Hex(Color32::from_rgb(1, 2, 3)));
            t.ui.insert("border".into(), Spec::Link { to: "text".into(), alpha: Some(20) });
            t.ui.insert("selection_bg".into(), Spec::Hex(Color32::from_rgba_unmultiplied(10, 20, 30, 128)));
            t.builtin = false;
            let json = to_json(&t);
            let back = parse_str(&json).unwrap_or_else(|e| panic!("{}: {}\n{json}", t.name, e.en));
            assert_eq!(back, t, "{json}");
            let _: Value = serde_json::from_str(&json).unwrap();
        }
        // Ohne ui-Werte kein ui-Objekt (alte Versionen lesen dieselbe Form).
        let plain = to_json(&crate::theme::all()[0]);
        assert!(!plain.contains("\"ui\""), "{plain}");
        assert!(parse_str(&plain).is_ok(), "{plain}");
    }

    #[test]
    fn names_and_files_are_unique() {
        let taken = vec!["One Dark".to_string(), "One Dark (2)".to_string(), "Mine".to_string()];
        assert_eq!(unique_name("Fresh", &taken), "Fresh");
        assert_eq!(unique_name("One Dark", &taken), "One Dark (3)");
        assert_eq!(unique_name("one dark (2)", &taken), "one dark (3)");
        assert_eq!(unique_name("Mine", &taken), "Mine (2)");
        assert_eq!(unique_name("Plan (B)", &["Plan (B)".to_string()]), "Plan (B) (2)");
        let dir = std::env::temp_dir().join(format!("paneldeck-test-themefile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(file_for(&dir, "One Dark (2)"), dir.join("one-dark-2.json"));
        assert_eq!(file_for(&dir, "  ***  "), dir.join("theme.json"));
        let mut t = crate::theme::all()[0].clone();
        t.builtin = false;
        write(&dir.join("one-dark-2.json"), &t).unwrap();
        assert_eq!(file_for(&dir, "One Dark (2)"), dir.join("one-dark-2-2.json"));
        assert_eq!(read_file(&dir.join("one-dark-2.json")).unwrap().name, "One Dark");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
