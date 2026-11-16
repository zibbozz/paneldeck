//! Lucide-Icons (https://lucide.dev, ISC): Tabelle Name → Zeichen der eingebetteten Schrift, Kategorien und Stichwörter,
//! dazu die Suche des Icon-Explorers. Kein egui – gezeichnet wird in `app::planning_ui::icon_explorer` über die Schrift
//! `assets/fonts/lucide.ttf`. Tabelle und Schrift erzeugt `tools/update-lucide.ps1` (feste Version, siehe [`VERSION`]).

use crate::state::Lang;
use std::sync::OnceLock;

/// Version des npm-Pakets `lucide-static`, aus dem Schrift und Tabelle stammen.
pub const VERSION: &str = "1.66.0";

static TSV: &str = include_str!("../../../assets/lucide/icons.tsv");

/// Ein Lucide-Icon: kanonischer Name (`kebab-case`), Zeichen in der Schrift, Kategorie-Schlüssel und Stichwörter (englisch).
#[derive(Debug)]
pub struct LucideIcon {
    pub name: &'static str,
    pub ch: char,
    pub categories: Vec<&'static str>,
    pub tags: Vec<&'static str>,
}

/// Eine Kategorie: Schlüssel (z. B. `arrows`) und englischer Titel aus dem Lucide-Projekt.
#[derive(Debug)]
pub struct Category {
    pub key: &'static str,
    pub title_en: &'static str,
}

pub struct Table {
    /// Nach Name sortiert (binäre Suche in [`get`]).
    pub icons: Vec<LucideIcon>,
    /// In der Reihenfolge der Datei (nach Schlüssel sortiert).
    pub categories: Vec<Category>,
}

fn parse(text: &'static str) -> Table {
    let mut icons = Vec::new();
    let mut categories = Vec::new();
    let split = |s: &'static str| -> Vec<&'static str> { s.split(',').filter(|x| !x.is_empty()).collect() };
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&'static str> = line.split('\t').collect();
        if parts[0] == "@" {
            if let (Some(key), Some(title)) = (parts.get(1), parts.get(2)) {
                categories.push(Category { key, title_en: title });
            }
            continue;
        }
        let Some(ch) = parts.get(1).and_then(|h| u32::from_str_radix(h, 16).ok()).and_then(char::from_u32) else {
            continue;
        };
        icons.push(LucideIcon { name: parts[0], ch, categories: parts.get(2).map(|s| split(s)).unwrap_or_default(), tags: parts.get(3).map(|s| split(s)).unwrap_or_default() });
    }
    icons.sort_by(|a, b| a.name.cmp(b.name));
    Table { icons, categories }
}

/// Die eingebettete Tabelle (beim ersten Zugriff gelesen).
pub fn table() -> &'static Table {
    static T: OnceLock<Table> = OnceLock::new();
    T.get_or_init(|| parse(TSV))
}

/// Icon zum kanonischen Namen (ohne `lucide:`).
pub fn get(name: &str) -> Option<&'static LucideIcon> {
    let t = table();
    t.icons.binary_search_by(|i| i.name.cmp(name)).ok().map(|i| &t.icons[i])
}

/// Deutsche Titel der Kategorien (die englischen kommen aus der Tabelle).
const CATEGORY_DE: &[(&str, &str)] = &[
    ("accessibility", "Barrierefreiheit"),
    ("account", "Konten & Zugang"),
    ("animals", "Tiere"),
    ("arrows", "Pfeile"),
    ("buildings", "Gebäude"),
    ("charts", "Diagramme"),
    ("communication", "Kommunikation"),
    ("connectivity", "Verbindungen"),
    ("cursors", "Mauszeiger"),
    ("design", "Design"),
    ("development", "Programmierung & Entwicklung"),
    ("devices", "Geräte"),
    ("emoji", "Emoji"),
    ("files", "Dateien"),
    ("finance", "Finanzen"),
    ("food-beverage", "Essen & Trinken"),
    ("gaming", "Spiele"),
    ("home", "Zuhause"),
    ("layout", "Layout"),
    ("letters-digits", "Buchstaben & Ziffern"),
    ("mail", "E-Mail"),
    ("math", "Mathematik"),
    ("medical", "Medizin"),
    ("multimedia", "Multimedia"),
    ("nature", "Natur"),
    ("navigation", "Navigation & Orte"),
    ("notifications", "Benachrichtigungen"),
    ("people", "Personen"),
    ("photography", "Fotografie"),
    ("science", "Wissenschaft"),
    ("seasons", "Jahreszeiten"),
    ("security", "Sicherheit"),
    ("shapes", "Formen"),
    ("shopping", "Einkaufen"),
    ("social", "Soziales"),
    ("sports", "Sport"),
    ("sustainability", "Nachhaltigkeit"),
    ("text", "Textformatierung"),
    ("time", "Zeit & Kalender"),
    ("tools", "Werkzeuge"),
    ("transportation", "Verkehr"),
    ("travel", "Reisen"),
    ("weather", "Wetter"),
];

/// Titel einer Kategorie in der Sprache der Oberfläche (unbekannter Schlüssel: der Schlüssel selbst).
pub fn category_title(key: &str, lang: Lang) -> &str {
    let en = table().categories.iter().find(|c| c.key == key).map(|c| c.title_en);
    match lang {
        Lang::De => CATEGORY_DE.iter().find(|(k, _)| *k == key).map(|(_, d)| *d).or(en).unwrap_or(key),
        Lang::En => en.unwrap_or(key),
    }
}

/// Rang eines Suchworts für ein Icon (kleiner = besser): Name gleich 0, Name beginnt damit 1, ein Namensteil beginnt damit 2,
/// Name enthält es 3, Stichwort gleich 4, Stichwort beginnt damit 5, Stichwort enthält es 6; `None` = kein Treffer.
fn word_rank(icon: &LucideIcon, w: &str) -> Option<u8> {
    let n = icon.name;
    if n == w {
        return Some(0);
    }
    if n.starts_with(w) {
        return Some(1);
    }
    if n.split('-').any(|p| p.starts_with(w)) {
        return Some(2);
    }
    if n.contains(w) {
        return Some(3);
    }
    let mut best: Option<u8> = None;
    for t in &icon.tags {
        let r = if *t == w {
            4
        } else if t.starts_with(w) || t.split(' ').any(|p| p.starts_with(w)) {
            5
        } else if t.contains(w) {
            6
        } else {
            continue;
        };
        best = Some(best.map_or(r, |b: u8| b.min(r)));
        if r == 4 {
            break;
        }
    }
    best
}

/// Suche im Explorer: `query` (Wörter durch Leerzeichen, Groß/klein egal; Bindestriche wie Leerzeichen) und optional eine
/// Kategorie. Jedes Wort muss im Namen oder in einem Stichwort vorkommen. Treffer im Namen stehen vor Treffern in
/// Stichwörtern (siehe [`word_rank`]), gleich gute nach Name. Leere Suche: alle Icons der Kategorie nach Name.
pub fn search(query: &str, category: Option<&str>) -> Vec<&'static LucideIcon> {
    let q = query.trim().to_lowercase().replace('-', " ");
    let words: Vec<&str> = q.split_whitespace().collect();
    let in_cat = |i: &LucideIcon| category.is_none_or(|c| i.categories.contains(&c));
    // Ganze Eingabe mit Bindestrichen (z. B. „alarm clock“ → „alarm-clock“) zählt als ein Namenstreffer.
    let joined = words.join("-");
    let mut hits: Vec<((u8, u32), &'static LucideIcon)> = Vec::new();
    for icon in table().icons.iter().filter(|i| in_cat(i)) {
        if words.is_empty() {
            hits.push(((0, 0), icon));
            continue;
        }
        let whole = if words.len() > 1 { word_rank(icon, &joined).filter(|r| *r <= 3) } else { None };
        let ranks: Option<Vec<u8>> = words.iter().map(|w| word_rank(icon, w)).collect();
        let key = match (whole, ranks) {
            (Some(r), _) => (r, 0),
            (None, Some(rs)) => (rs.iter().copied().max().unwrap_or(0), rs.iter().map(|r| *r as u32).sum()),
            (None, None) => continue,
        };
        hits.push((key, icon));
    }
    hits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.name.cmp(b.1.name)));
    hits.into_iter().map(|(_, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_loads_with_unique_names_and_characters() {
        let t = table();
        assert!(t.icons.len() > 1500, "{}", t.icons.len());
        assert!(t.categories.len() >= 30);
        let mut names: Vec<&str> = t.icons.iter().map(|i| i.name).collect();
        names.dedup();
        assert_eq!(names.len(), t.icons.len(), "doppelte Namen");
        let mut chars: Vec<char> = t.icons.iter().map(|i| i.ch).collect();
        chars.sort();
        chars.dedup();
        assert_eq!(chars.len(), t.icons.len(), "doppelte Zeichen");
        for i in &t.icons {
            assert!(super::super::is_lucide_slug(i.name), "{}", i.name);
            assert!(('\u{E000}'..='\u{F8FF}').contains(&i.ch), "{} liegt nicht im privaten Bereich", i.name);
            assert!(i.categories.iter().all(|c| t.categories.iter().any(|k| k.key == *c)), "{}: unbekannte Kategorie", i.name);
        }
        let ac = get("alarm-clock").unwrap();
        assert!(ac.categories.contains(&"time") && ac.tags.contains(&"morning"));
        assert!(get("nope-not-there").is_none() && get("").is_none());
    }

    #[test]
    fn the_version_is_the_same_in_table_script_and_licence_list() {
        assert!(TSV.lines().next().unwrap().starts_with(&format!("# Lucide {VERSION} ")));
        let script = include_str!("../../../tools/update-lucide.ps1");
        assert!(script.contains(&format!("$Version = '{VERSION}'")), "tools/update-lucide.ps1");
        let licences = include_str!("../../../tools/gen_licenses.py");
        assert!(licences.contains(&format!("\"{VERSION}\", \"ISC")), "tools/gen_licenses.py");
    }

    #[test]
    fn every_category_has_a_german_title() {
        for c in &table().categories {
            assert!(CATEGORY_DE.iter().any(|(k, _)| *k == c.key), "{} fehlt", c.key);
            assert_eq!(category_title(c.key, Lang::En), c.title_en);
            assert_ne!(category_title(c.key, Lang::De), c.key);
        }
        assert_eq!(category_title("arrows", Lang::De), "Pfeile");
        assert_eq!(category_title("unknown-cat", Lang::De), "unknown-cat");
        // Jede Kategorie hat Icons.
        for c in &table().categories {
            assert!(!search("", Some(c.key)).is_empty(), "{}", c.key);
        }
    }

    #[test]
    fn search_ranks_name_matches_before_tag_matches() {
        let r = search("clock", None);
        let names: Vec<&str> = r.iter().map(|i| i.name).collect();
        assert_eq!(names[0], "clock");
        // „alarm-clock“ (im Namen) steht vor jedem Icon, das „clock“ nur als Stichwort hat.
        let first_tag_only = r.iter().position(|i| !i.name.contains("clock")).expect("Treffer nur über Stichwörter");
        let alarm = names.iter().position(|n| *n == "alarm-clock").unwrap();
        assert!(alarm < first_tag_only, "{alarm} < {first_tag_only}");
        assert!(r[first_tag_only..].iter().all(|i| !i.name.contains("clock")));
        // Stichwort-Treffer: „morning“ steht nur in Stichwörtern.
        assert!(search("morning", None).iter().any(|i| i.name == "alarm-clock"));
        // Groß/klein und mehrere Wörter, Bindestrich wie Leerzeichen.
        assert_eq!(search("Alarm Clock", None)[0].name, "alarm-clock");
        assert_eq!(search("alarm-clock", None)[0].name, "alarm-clock");
        assert!(search("zzzzqqq", None).is_empty());
        // Kategorie filtert.
        let t = search("", Some("weather"));
        assert!(!t.is_empty() && t.iter().all(|i| i.categories.contains(&"weather")));
        assert!(search("clock", Some("weather")).iter().all(|i| i.categories.contains(&"weather")));
        assert_eq!(search("", None).len(), table().icons.len());
    }
}
