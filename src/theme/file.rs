//! Theme-Dateien `themes/*.json` (öffentliches Format): lesen mit klarer Fehlermeldung, schreiben, Größengrenze,
//! eindeutige Namen. Neu in 2.6.0 (#309) ist nur das optionale Objekt `ui` (Oberflächenfarben); fehlt es oder ein
//! Wert, gilt die Ableitung. Unbekannte Felder und Namen (neuere Versionen) werden für die Darstellung ignoriert,
//! aber in [`Extra`] aufbewahrt und beim Schreiben wieder eingefügt – Speichern, Umbenennen, Duplizieren und
//! Importieren verlieren nichts.

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

/// Inhalte einer Theme-Datei, die diese Version nicht versteht (neuere Versionen). Sie wirken nicht auf die
/// Darstellung und werden von [`to_json`] unverändert wieder geschrieben.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Extra {
    /// Felder der obersten Ebene, die diese Version nicht kennt.
    pub top: serde_json::Map<String, Value>,
    /// Einträge in `ui` mit unbekanntem Namen oder mit einem Verweis auf eine unbekannte Farbe. Ein gesetzter Wert
    /// desselben Namens in [`Theme::ui`] geht beim Schreiben vor.
    pub ui: serde_json::Map<String, Value>,
}

/// Felder der obersten Ebene, die diese Version liest.
const KNOWN_FIELDS: &[&str] = &["name", "dark", "fg", "bg", "cursor", "selection", "ansi", "ui"];

/// Längster Ausschnitt eines Werts in Meldungen (eine 1-MB-Datei soll keine riesigen Texte erzeugen).
const SHOW_MAX: usize = 40;

/// Wert für eine Meldung kürzen (höchstens [`SHOW_MAX`] Zeichen, dann „…“).
pub fn short(s: &str) -> String {
    if s.chars().count() <= SHOW_MAX {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(SHOW_MAX).collect::<String>())
    }
}

fn field_err(field: &str, value: &str) -> Msg {
    let value = short(value);
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
        // Unverändert (ohne Kürzen der Leerzeichen und ohne Längengrenze, wie vor 2.6.0): Auswahl und Profile
        // verweisen auf den genauen Namen. Die 80-Zeichen-Grenze gilt nur für neu vergebene Namen im Editor.
        Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
        Some(Value::String(_)) => return Err(Msg::new("„name“ ist leer", "“name” is empty")),
        Some(_) => return Err(Msg::new("„name“ muss ein Text sein", "“name” must be a string")),
        None => return Err(Msg::new("Pflichtfeld „name“ fehlt", "Required field “name” is missing")),
    };
    let fg = color_field(&obj, "fg", true)?.unwrap_or(Color32::WHITE);
    let bg = color_field(&obj, "bg", true)?.unwrap_or(Color32::BLACK);
    // Terminalfarben ohne Transparenz (wie bisher); der Hintergrund bekommt die Deckkraft aus den Einstellungen.
    let solid = |c: Color32| {
        let [r, g, b, _] = c.to_srgba_unmultiplied();
        Color32::from_rgb(r, g, b)
    };
    let (fg, bg) = (solid(fg), solid(bg));
    // Optionale Felder: ein ungültiger Wert fällt wie vor 2.6.0 auf die Ableitung zurück (die Datei bleibt gültig).
    let optional = |key: &str| color_field(&obj, key, false).ok().flatten().map(solid);
    let cursor = optional("cursor").unwrap_or(fg);
    let selection = optional("selection").unwrap_or_else(|| crate::style::mix(bg, fg, 0.2));
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
    let mut extra = Extra::default();
    for (k, v) in &obj {
        if !KNOWN_FIELDS.contains(&k.as_str()) {
            extra.top.insert(k.clone(), v.clone());
        }
    }
    match obj.get("ui") {
        None | Some(Value::Null) => {}
        Some(Value::Object(m)) => {
            for (k, v) in m {
                // Unbekannte Namen (neuere Versionen): nicht anzeigen, aber aufbewahren.
                if !UI_TOKENS.iter().any(|t| t.name == k) {
                    extra.ui.insert(k.clone(), v.clone());
                    continue;
                }
                let Value::String(s) = v else {
                    return Err(Msg::new(format!("„ui.{k}“ muss ein Text sein"), format!("“ui.{k}” must be a string")));
                };
                let shown = short(s);
                match Spec::parse(s) {
                    Ok(spec) => {
                        ui.insert(k.clone(), spec);
                    }
                    // Verweis auf eine Farbe, die diese Version nicht kennt: wie abgeleitet, aber aufbewahrt.
                    Err(SpecError::Unknown(_)) => {
                        extra.ui.insert(k.clone(), v.clone());
                    }
                    Err(SpecError::Alpha) => {
                        return Err(Msg::new(
                            format!("Ungültige Deckkraft in „ui.{k}“: „{shown}“ (erwartet 0–100, z. B. @text/20)"),
                            format!("Invalid opacity in “ui.{k}”: “{shown}” (expected 0–100, e.g. @text/20)"),
                        ))
                    }
                    Err(SpecError::Syntax) => {
                        return Err(Msg::new(
                            format!("Ungültiger Wert für „ui.{k}“: „{shown}“ (erwartet #RRGGBB, #RRGGBBAA oder @name bzw. @name/20)"),
                            format!("Invalid value for “ui.{k}”: “{shown}” (expected #RRGGBB, #RRGGBBAA or @name or @name/20)"),
                        ))
                    }
                }
            }
        }
        Some(_) => return Err(Msg::new("„ui“ muss ein Objekt sein", "“ui” must be an object")),
    }
    let t = Theme { name, dark, fg, bg, cursor, selection, ansi, ui, builtin: false, source: None, extra };
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

/// Eigene Themes aus `<dir>/*.json` laden. Ungültige Dateien und Namen, die schon genau so vergeben sind (eingebaute
/// Themes oder eine frühere Datei), landen mit Grund in der zweiten Liste und sind nicht wählbar. Namen, die sich nur in
/// Groß-/Kleinschreibung unterscheiden („one dark“), bleiben wie vor 2.6.0 wählbar; nur neu vergebene Namen müssen
/// auch ohne Groß-/Kleinschreibung eindeutig sein ([`unique_name`]).
pub fn load_dir(dir: &Path, builtin: &[Theme]) -> (Vec<Theme>, Vec<Issue>) {
    let (mut out, mut issues) = (Vec::new(), Vec::new());
    let Ok(rd) = std::fs::read_dir(dir) else { return (out, issues) };
    let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")) && p.is_file()).collect();
    files.sort();
    for p in files {
        match read_file(&p) {
            Ok(t) => {
                if builtin.iter().chain(out.iter()).any(|o: &Theme| o.name == t.name) {
                    let n = short(&t.name);
                    issues.push(Issue {
                        msg: Msg::new(format!("Der Name „{n}“ ist schon vergeben"), format!("The name “{n}” is already used")),
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

/// JSON-Text eines Themes (feste Reihenfolge, `ui` nur mit gesetzten Werten in der Reihenfolge der Tabelle). Unbekannte
/// Inhalte ([`Extra`]) kommen unverändert dahinter: `ui`-Einträge ans Ende von `ui`, Felder ans Ende der Datei.
pub fn to_json(t: &Theme) -> String {
    let q = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into());
    // Mehrzeilige Werte (Objekte, Listen) passend einrücken.
    let val = |v: &Value, indent: &str| serde_json::to_string_pretty(v).unwrap_or_else(|_| "null".into()).replace('\n', &format!("\n{indent}"));
    let mut top: Vec<String> = Vec::new();
    top.push(format!("  \"name\": {}", q(&t.name)));
    top.push(format!("  \"dark\": {}", t.dark));
    for (k, c) in [("fg", t.fg), ("bg", t.bg), ("cursor", t.cursor), ("selection", t.selection)] {
        let [r, g, b, _] = c.to_srgba_unmultiplied();
        top.push(format!("  \"{k}\": \"{}\"", to_hex(Color32::from_rgb(r, g, b))));
    }
    let rows: Vec<String> = (0..2).map(|row| format!("    {}", (0..8).map(|i| format!("\"{}\"", to_hex(t.ansi[row * 8 + i]))).collect::<Vec<_>>().join(", "))).collect();
    top.push(format!("  \"ansi\": [\n{}\n  ]", rows.join(",\n")));
    let mut ui: Vec<String> = UI_TOKENS.iter().filter_map(|tok| t.ui.get(tok.name).map(|v: &Spec| format!("    \"{}\": \"{}\"", tok.name, v.to_text()))).collect();
    ui.extend(t.extra.ui.iter().filter(|(k, _)| !t.ui.contains_key(k.as_str())).map(|(k, v)| format!("    {}: {}", q(k), val(v, "    "))));
    if !ui.is_empty() {
        top.push(format!("  \"ui\": {{\n{}\n  }}", ui.join(",\n")));
    }
    top.extend(t.extra.top.iter().filter(|(k, _)| !KNOWN_FIELDS.contains(&k.as_str())).map(|(k, v)| format!("  {}: {}", q(k), val(v, "  "))));
    format!("{{\n{}\n}}\n", top.join(",\n"))
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

/// Gerätename unter Windows („con.json“ lässt sich dort weder anlegen noch öffnen).
fn reserved_on_windows(slug: &str) -> bool {
    let b = slug.as_bytes();
    matches!(slug, "con" | "prn" | "aux" | "nul") || (b.len() == 4 && (slug.starts_with("com") || slug.starts_with("lpt")) && (b'1'..=b'9').contains(&b[3]))
}

/// Freier Dateiname in `dir` zu einem Theme-Namen (Kleinbuchstaben, Ziffern, Bindestriche; Windows-Gerätenamen wie
/// „con“ bekommen „-theme“ angehängt).
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
    let slug = if slug.is_empty() {
        "theme".to_string()
    } else if reserved_on_windows(&slug) {
        format!("{slug}-theme")
    } else {
        slug
    };
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
        // Ungültige optionale Farben fallen wie vor 2.6.0 auf die Ableitung zurück.
        let t = parse_str(&file(r##","cursor":"blau","selection":7"##)).unwrap();
        assert_eq!(t.cursor, t.fg);
        assert_eq!(t.selection, crate::style::mix(t.bg, t.fg, 0.2));
        // Namen bleiben unverändert (Leerzeichen, Länge) – Auswahl und Profile verweisen auf den genauen Namen.
        assert_eq!(parse_str(&file("").replace("\"X\"", "\" Foo \"")).unwrap().name, " Foo ");
        let long = "L".repeat(120);
        assert_eq!(parse_str(&file("").replace("\"X\"", &format!("\"{long}\""))).unwrap().name, long);
    }

    #[test]
    fn unknown_content_survives_saving() {
        let src = file(r##","future":{"a":[1,2],"b":"x"},"zzz":true,"ui":{"nope":{"deep":1},"accent":"#0080ff","border":"@future_token/20"}"##);
        let mut t = parse_str(&src).unwrap();
        // Für die Darstellung ignoriert: border bleibt abgeleitet.
        assert_eq!(t.ui.len(), 1);
        let plain = parse_str(&file(r##","ui":{"accent":"#0080ff"}"##)).unwrap();
        assert_eq!(t.ui_colors(), plain.ui_colors());
        assert_eq!(t.extra.top.len(), 2);
        assert_eq!(t.extra.ui.len(), 2);
        // Umbenennen und Speichern (wie Editor, Duplizieren, Import) behält alles.
        t.name = "Renamed".into();
        let json = to_json(&t);
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["future"], serde_json::json!({"a": [1, 2], "b": "x"}), "{json}");
        assert_eq!(v["zzz"], true);
        assert_eq!(v["ui"]["nope"], serde_json::json!({"deep": 1}));
        assert_eq!(v["ui"]["border"], "@future_token/20");
        assert_eq!(v["ui"]["accent"], "#0080ff");
        let back = parse_str(&json).unwrap();
        assert_eq!(back, t);
        // Ein in dieser Version gesetzter Wert geht dem aufbewahrten vor.
        t.ui.insert("border".into(), Spec::Hex(Color32::RED));
        let v: Value = serde_json::from_str(&to_json(&t)).unwrap();
        assert_eq!(v["ui"]["border"], "#ff0000");
        // Ohne ui-Werte, aber mit unbekannten ui-Einträgen entsteht trotzdem ein ui-Objekt.
        let mut t = parse_str(&file(r##","ui":{"nope":"#ffffff"}"##)).unwrap();
        assert!(t.ui.is_empty());
        t.name = "Y".into();
        let v: Value = serde_json::from_str(&to_json(&t)).unwrap();
        assert_eq!(v["ui"]["nope"], "#ffffff");
    }

    #[test]
    fn messages_quote_only_a_short_part_of_the_value() {
        let huge = format!("[{}]", vec!["1"; 50_000].join(","));
        let e = parse_str(&file("").replace("\"#ffffff\",\"bg\"", &format!("{huge},\"bg\""))).unwrap_err();
        assert!(e.en.contains("“fg”") && e.en.contains('…') && e.en.chars().count() < 160, "{}", e.en);
        let e = parse_str(&file(&format!(r##","ui":{{"border":"{}"}}"##, "x".repeat(5000)))).unwrap_err();
        assert!(e.en.contains("“ui.border”") && e.de.chars().count() < 200, "{}", e.de);
        assert_eq!(short("abc"), "abc");
        assert_eq!(short(&"ä".repeat(41)), format!("{}…", "ä".repeat(40)));
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
        // Nur in Groß-/Kleinschreibung anders: wie vor 2.6.0 wählbar.
        std::fs::write(dir.join("lower.json"), file("").replace("\"X\"", "\"one dark\"")).unwrap();
        let (themes, issues) = load_dir(&dir, &crate::theme::all());
        assert_eq!(themes.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["one dark", "X"]);
        assert_eq!(themes[1].source.as_deref(), Some(dir.join("ok.json").as_path()));
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
        // Windows-Gerätenamen.
        for (n, f) in [("CON", "con-theme.json"), ("Nul", "nul-theme.json"), ("com1", "com1-theme.json"), ("LPT9", "lpt9-theme.json"), ("com10", "com10.json"), ("console", "console.json")] {
            assert_eq!(file_for(&dir, n), dir.join(f), "{n}");
        }
        let mut t = crate::theme::all()[0].clone();
        t.builtin = false;
        write(&dir.join("one-dark-2.json"), &t).unwrap();
        assert_eq!(file_for(&dir, "One Dark (2)"), dir.join("one-dark-2-2.json"));
        assert_eq!(read_file(&dir.join("one-dark-2.json")).unwrap().name, "One Dark");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
