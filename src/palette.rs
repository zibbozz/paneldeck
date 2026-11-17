//! Farbpaletten (seit 2.3.0, Issue #173): die vorgeschlagenen Farben für Sitzungen, Ordner, Projekte und Profile
//! (Terminal-Hub), Tags der einfachen Todo-Liste und den Expertenmodus (Projekte, Typen, Status, Auswahlwerte, Diagramme).
//!
//! Standard ist **eine gemeinsame Palette** für alles. Mit `per_area` hat jeder Bereich seine eigene; beide Varianten
//! bleiben gespeichert, ein Umschalten verliert nichts. Gespeichert wird in `settings.json` (Feld `palette`, optional).
//! Jede Liste ist optional: fehlt sie (`None`), gelten die eingebauten Vorgaben – auch künftige Änderungen daran.
//! Elemente speichern ihren Farbwert selbst – ändern oder löschen einer Palettenfarbe wirkt nur auf künftige Auswahlen.
//!
//! **Automatische Vergabe** (neue Sitzungen, Tags, Projekte/Typen/Status, Auswahlwerte, Diagrammsegmente): solange die
//! geltende Liste unverändert (oder leer) ist, nimmt sie die bisherige Vorgabe des Bereichs in der bisherigen Reihenfolge
//! ([`HUB_DEFAULTS`], [`TODO_DEFAULTS`], [`EXPERT_DEFAULTS`]) – nach dem Update ändert sich also nichts. Erst eine selbst
//! bearbeitete Liste wird für die Vergabe benutzt.
//!
//! Die Oberfläche liest die Palette über [`with_current`] (die App veröffentlicht ihre Einstellungen jedes Bild mit
//! [`publish`]), damit nicht jede Auswahl die Einstellungen durchreichen muss. Kein egui hier.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::RwLock;

/// Eingebaute Vorgabe des Terminal-Hubs (bis 2.2 die feste Palette für Sitzungen, Ordner, Projekte, Profile).
pub const HUB_DEFAULTS: [&str; 8] = ["#61AFEF", "#98C379", "#E5C07B", "#E06C75", "#C678DD", "#56B6C2", "#D97757", "#888888"];

/// Eingebaute Vorgabe der Todo-Tags (bis 2.2 `tasks::PALETTE`).
pub const TODO_DEFAULTS: [&str; 10] = ["#E06C75", "#E5A04B", "#D8B94A", "#7FBF6A", "#4FB8A8", "#58A6E6", "#7B8CF0", "#B77FE0", "#E07BB0", "#97A3B0"];

/// Eingebaute Vorgabe des Expertenmodus (bis 2.2 `planning_ui::helpers::PALETTE`).
pub const EXPERT_DEFAULTS: [&str; 14] = [
    "#D97757", "#E5A04B", "#D8B94A", "#7FBF6A", "#4FB8A8", "#58A6E6", "#6F8FE8", "#B77FE0", "#E07FB0", "#E06C75", "#97A3B0", "#8C7B6B", "#5E8F5E", "#4A78B5",
];

/// Eingebaute gemeinsame Palette: Vereinigung der drei obigen ohne Doppelte. Fast gleiche Paare sind zusammengefasst
/// (`#7B8CF0`/`#6F8FE8`, `#E07BB0`/`#E07FB0` – mit bloßem Auge nicht zu unterscheiden). Reihenfolge: zuerst die acht
/// Hub-Farben, dann abwechselnd die übrigen (gut unterscheidbare zuerst), die gedeckten Töne zuletzt.
pub const SHARED_DEFAULTS: [&str; 20] = [
    "#61AFEF", "#98C379", "#E5C07B", "#E06C75", "#C678DD", "#56B6C2", "#D97757", "#888888", "#E5A04B", "#E07FB0", "#4A78B5", "#5E8F5E", "#B77FE0", "#4FB8A8",
    "#D8B94A", "#6F8FE8", "#7FBF6A", "#58A6E6", "#97A3B0", "#8C7B6B",
];

/// Längster Farbname in Zeichen.
pub const MAX_NAME_CHARS: usize = 40;
/// Höchstens so viele Farben je Palette.
pub const MAX_COLORS: usize = 64;

/// Bereich, dessen Auswahlen eine Palette nutzen.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Area {
    /// Terminal-Hub: Sitzungen, Ordner, Projekte, Profile.
    Hub,
    /// Einfache Todo-Liste: Tag-Farben.
    Todo,
    /// Expertenmodus: Projekte, Typen, Status, Auswahlwerte, Diagramme.
    Expert,
}

impl Area {
    pub const ALL: [Area; 3] = [Area::Hub, Area::Todo, Area::Expert];

    pub fn label(self, lang: crate::state::Lang) -> &'static str {
        match self {
            Area::Hub => "Terminal Hub",
            Area::Todo => lang.t("Todo-Liste", "Todo list"),
            Area::Expert => lang.t("Expertenmodus", "Expert mode"),
        }
    }

    /// Die bisherige Vorgabe des Bereichs (auch Grundlage der automatischen Vergabe, solange nichts bearbeitet ist).
    pub fn defaults(self) -> &'static [&'static str] {
        match self {
            Area::Hub => &HUB_DEFAULTS,
            Area::Todo => &TODO_DEFAULTS,
            Area::Expert => &EXPERT_DEFAULTS,
        }
    }
}

/// Welche der gespeicherten Listen (die gemeinsame oder die eines Bereichs).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Which {
    Shared,
    Area(Area),
}

impl Which {
    pub const ALL: [Which; 4] = [Which::Shared, Which::Area(Area::Hub), Which::Area(Area::Todo), Which::Area(Area::Expert)];

    pub fn defaults(self) -> &'static [&'static str] {
        match self {
            Which::Shared => &SHARED_DEFAULTS,
            Which::Area(a) => a.defaults(),
        }
    }

    fn key(self) -> &'static str {
        match self {
            Which::Shared => "shared",
            Which::Area(Area::Hub) => "hub",
            Which::Area(Area::Todo) => "todo",
            Which::Area(Area::Expert) => "expert",
        }
    }
}

/// Eine Palettenfarbe: Wert `#RRGGBB` (Großbuchstaben) und optionaler Name.
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct PaletteColor {
    pub hex: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
}

impl PaletteColor {
    pub fn new(hex: &str) -> Self {
        Self { hex: normalize_hex(hex).unwrap_or_else(|| hex.to_string()), name: String::new() }
    }

    #[cfg(test)]
    pub fn named(hex: &str, name: &str) -> Self {
        Self { name: name.to_string(), ..Self::new(hex) }
    }

    /// Text des Tooltips: `#2288FF` ohne Namen, `Cooles Blau (#2288FF)` mit Namen.
    pub fn tooltip(&self) -> String {
        let n = self.name.trim();
        if n.is_empty() { self.hex.clone() } else { format!("{n} ({})", self.hex) }
    }

    /// Tolerant aus JSON: Objekt mit `hex` (Pflicht, gültig) und `name` (Text, sonst leer) oder nur der Hex-Text.
    fn from_value(v: &Value) -> Option<Self> {
        let (hex, name) = match v {
            Value::String(s) => (s.as_str(), ""),
            Value::Object(o) => (o.get("hex")?.as_str()?, o.get("name").and_then(Value::as_str).unwrap_or("")),
            _ => return None,
        };
        Some(Self { hex: normalize_hex(hex)?, name: clean_name(name) })
    }
}

/// Liste aus Vorgaben.
pub fn list_of(defaults: &[&str]) -> Vec<PaletteColor> {
    defaults.iter().map(|h| PaletteColor::new(h)).collect()
}

/// Alle Paletten (Feld `palette` in `settings.json`). `None` = eingebaute Vorgaben (nicht gespeichert).
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize)]
pub struct Palettes {
    /// `false` (Standard): eine gemeinsame Palette für alles. `true`: je Bereich eine eigene.
    pub per_area: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared: Option<Vec<PaletteColor>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hub: Option<Vec<PaletteColor>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub todo: Option<Vec<PaletteColor>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expert: Option<Vec<PaletteColor>>,
}

impl Palettes {
    /// Nichts eingestellt (dann schreibt `settings.json` das Feld nicht).
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Welche Liste für `area` gerade gilt.
    pub fn which(&self, area: Area) -> Which {
        if self.per_area { Which::Area(area) } else { Which::Shared }
    }

    pub fn slot(&self, w: Which) -> &Option<Vec<PaletteColor>> {
        match w {
            Which::Shared => &self.shared,
            Which::Area(Area::Hub) => &self.hub,
            Which::Area(Area::Todo) => &self.todo,
            Which::Area(Area::Expert) => &self.expert,
        }
    }

    pub fn slot_mut(&mut self, w: Which) -> &mut Option<Vec<PaletteColor>> {
        match w {
            Which::Shared => &mut self.shared,
            Which::Area(Area::Hub) => &mut self.hub,
            Which::Area(Area::Todo) => &mut self.todo,
            Which::Area(Area::Expert) => &mut self.expert,
        }
    }

    /// Die Liste `w` wie sie gilt (gespeichert oder die Vorgaben).
    pub fn list(&self, w: Which) -> Vec<PaletteColor> {
        self.slot(w).clone().unwrap_or_else(|| list_of(w.defaults()))
    }

    /// Liste `w` setzen; gleicht sie den Vorgaben, wird sie nicht gespeichert (folgt dann künftigen Vorgaben).
    pub fn set(&mut self, w: Which, l: Vec<PaletteColor>) {
        *self.slot_mut(w) = if l == list_of(w.defaults()) { None } else { Some(l) };
    }

    /// Die Farben, die die Auswahlen in `area` anbieten (kann leer sein).
    pub fn colors(&self, area: Area) -> Vec<PaletteColor> {
        self.list(self.which(area))
    }

    /// Farben für die automatische Vergabe in `area` (nie leer): die selbst bearbeitete geltende Liste; ist sie
    /// unverändert oder leer, die bisherige Vorgabe des Bereichs in der bisherigen Reihenfolge.
    pub fn auto(&self, area: Area) -> Vec<String> {
        match self.slot(self.which(area)) {
            Some(l) if !l.is_empty() => l.iter().map(|c| c.hex.clone()).collect(),
            _ => area.defaults().iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Tolerant aus JSON, je Feld und je Eintrag: ungültige Einträge fallen weg (die übrigen bleiben), eine unlesbare
    /// Liste – auch eine, in der kein Eintrag gültig ist – ergibt die Vorgaben (nur eine wirklich leere Liste bleibt leer),
    /// Doppelte entfallen, höchstens [`MAX_COLORS`] je Liste.
    pub fn from_value(v: &Value) -> Self {
        let mut p = Palettes { per_area: v.get("per_area").and_then(Value::as_bool).unwrap_or(false), ..Default::default() };
        for w in Which::ALL {
            let Some(Value::Array(items)) = v.get(w.key()) else { continue };
            let mut out: Vec<PaletteColor> = Vec::new();
            for c in items.iter().filter_map(PaletteColor::from_value) {
                if out.len() < MAX_COLORS && !out.iter().any(|o| o.hex == c.hex) {
                    out.push(c);
                }
            }
            if out.is_empty() && !items.is_empty() {
                // nur ungültige Einträge: Vorgaben statt einer leeren Liste, die sonst ohne Sicherung gespeichert würde (#221)
                continue;
            }
            *p.slot_mut(w) = Some(out);
        }
        p
    }

    /// Für `settings.json` (`deserialize_with`).
    pub fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Self::from_value(&Value::deserialize(d)?))
    }
}

/// Name bereinigen: Leerraum am Rand weg, Zeilenumbrüche zu Leerzeichen, höchstens [`MAX_NAME_CHARS`] Zeichen.
pub fn clean_name(s: &str) -> String {
    s.trim().replace(['\r', '\n', '\t'], " ").chars().take(MAX_NAME_CHARS).collect()
}

/// `#RRGGBB` (mit oder ohne `#`, beliebige Groß-/Kleinschreibung) → `#RRGGBB` in Großbuchstaben; sonst `None`.
pub fn normalize_hex(s: &str) -> Option<String> {
    let t = s.trim();
    let t = t.strip_prefix('#').unwrap_or(t);
    (t.len() == 6 && t.chars().all(|c| c.is_ascii_hexdigit())).then(|| format!("#{}", t.to_ascii_uppercase()))
}

/// `#RRGGBB` als RGB; ungültiger Text ergibt Grau.
pub fn rgb(hex: &str) -> [u8; 3] {
    match normalize_hex(hex).and_then(|h| u32::from_str_radix(&h[1..], 16).ok()) {
        Some(v) => [(v >> 16) as u8, (v >> 8) as u8, v as u8],
        None => [0x88, 0x88, 0x88],
    }
}

pub fn hex_of(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

/// Die Farbe aus `pool`, die in `used` am seltensten vorkommt (bei Gleichstand die vordere) – so rotiert die Vergabe.
/// Groß-/Kleinschreibung zählt nicht. Leerer `pool` ergibt `None`.
pub fn least_used<'a>(pool: &[String], used: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let used: Vec<String> = used.into_iter().filter_map(normalize_hex).collect();
    pool.iter().min_by_key(|c| used.iter().filter(|u| u.eq_ignore_ascii_case(c)).count()).cloned()
}

/// Wie viele Vorgaben in `list` fehlen.
pub fn missing_defaults(list: &[PaletteColor], defaults: &[&str]) -> usize {
    defaults.iter().filter(|d| !list.iter().any(|c| c.hex.eq_ignore_ascii_case(d))).count()
}

/// Gelöschte Vorgaben wieder anhängen (Reihenfolge der Vorgaben; vorhandene Farben und ihre Namen bleiben), höchstens
/// bis [`MAX_COLORS`]. Liefert die Zahl der wieder hinzugefügten Farben.
pub fn restore_defaults(list: &mut Vec<PaletteColor>, defaults: &[&str]) -> usize {
    let mut n = 0;
    for d in defaults {
        if list.len() >= MAX_COLORS {
            break;
        }
        let h = normalize_hex(d).unwrap_or_default();
        if !list.iter().any(|c| c.hex.eq_ignore_ascii_case(&h)) {
            list.push(PaletteColor::new(&h));
            n += 1;
        }
    }
    n
}

/// Farbe für „Hinzufügen“: die erste fehlende Vorgabe, sonst ein Farbton, der noch nicht vorkommt.
pub fn suggest_new(list: &[PaletteColor], defaults: &[&str]) -> String {
    let has = |h: &str| list.iter().any(|c| c.hex.eq_ignore_ascii_case(h));
    if let Some(d) = defaults.iter().find(|d| !has(d)) {
        return d.to_string();
    }
    for step in 0..360u32 {
        let hue = (step * 37) % 360;
        let h = hex_of(hsv(hue as f32, 0.55, 0.85));
        if !has(&h) {
            return h;
        }
    }
    "#808080".into()
}

fn hsv(h: f32, s: f32, v: f32) -> [u8; 3] {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let f = |t: f32| ((t + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    [f(r), f(g), f(b)]
}

/// Eintrag `from` so verschieben, dass er vor dem bisherigen Eintrag `before` steht (`before == len`: ans Ende).
/// Liefert `true`, wenn sich die Reihenfolge änderte.
pub fn move_before<T>(list: &mut Vec<T>, from: usize, before: usize) -> bool {
    if from >= list.len() || before > list.len() || before == from || before == from + 1 {
        return false;
    }
    let item = list.remove(from);
    let to = if before > from { before - 1 } else { before };
    list.insert(to, item);
    true
}

// ------------------------------------------------------------------ geltende Paletten (für die Oberfläche)

static CURRENT: RwLock<Option<Palettes>> = RwLock::new(None);

/// Die Einstellungen der App veröffentlichen (jedes Bild; kopiert nur bei Änderung).
pub fn publish(p: &Palettes) {
    if CURRENT.read().map(|c| c.as_ref() == Some(p)).unwrap_or(false) {
        return;
    }
    if let Ok(mut c) = CURRENT.write() {
        *c = Some(p.clone());
    }
}

/// Mit den geltenden Paletten arbeiten (noch nichts veröffentlicht, z. B. in Tests: die Vorgaben).
pub fn with_current<R>(f: impl FnOnce(&Palettes) -> R) -> R {
    let g = CURRENT.read().ok();
    match g.as_ref().and_then(|g| g.as_ref()) {
        Some(p) => f(p),
        None => f(&Palettes::default()),
    }
}

/// Farben, die die Auswahlen in `area` anbieten.
pub fn colors(area: Area) -> Vec<PaletteColor> {
    with_current(|p| p.colors(area))
}

/// Farben für die automatische Vergabe in `area` (nie leer).
pub fn auto(area: Area) -> Vec<String> {
    with_current(|p| p.auto(area))
}

/// Vorgabefarbe Nummer `i` (läuft im Kreis), z. B. für neue Sitzungen (`id`) oder Diagrammsegmente.
pub fn nth(area: Area, i: usize) -> String {
    let a = auto(area);
    a[i % a.len()].clone()
}

/// Wie [`nth`] als RGB (Terminal-Hub).
pub fn nth_rgb(area: Area, i: usize) -> [u8; 3] {
    rgb(&nth(area, i))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Settings;

    fn hexes(l: &[PaletteColor]) -> Vec<&str> {
        l.iter().map(|c| c.hex.as_str()).collect()
    }

    #[test]
    fn shared_defaults_are_the_deduplicated_union_of_the_three_old_palettes() {
        let mut seen = std::collections::HashSet::new();
        for h in SHARED_DEFAULTS {
            assert_eq!(normalize_hex(h).as_deref(), Some(h), "Großbuchstaben, gültig");
            assert!(seen.insert(h), "keine Doppelten: {h}");
        }
        for h in HUB_DEFAULTS.iter().chain(TODO_DEFAULTS.iter()).chain(EXPERT_DEFAULTS.iter()) {
            let twin = match *h {
                "#7B8CF0" => "#6F8FE8",
                "#E07BB0" => "#E07FB0",
                o => o,
            };
            assert!(SHARED_DEFAULTS.contains(&twin), "{h} fehlt");
        }
        assert_eq!(&SHARED_DEFAULTS[..8], &HUB_DEFAULTS[..]);
    }

    #[test]
    fn old_settings_without_the_field_load_with_the_defaults() {
        let old = r#"{"language":"De","theme":"One Dark","font_size":15.0}"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        assert!(s.palette.is_default());
        assert_eq!(hexes(&s.palette.colors(Area::Todo)), SHARED_DEFAULTS.to_vec());
        let out = serde_json::to_string(&Settings::default()).unwrap();
        assert!(!out.contains("\"palette\""), "{out}");
        // Nur bearbeitete Listen werden geschrieben.
        let mut s = Settings::default();
        s.palette.per_area = true;
        s.palette.set(Which::Area(Area::Todo), vec![PaletteColor::new("#010101")]);
        let v: Value = serde_json::to_value(&s).unwrap();
        assert_eq!(v["palette"], serde_json::json!({"per_area": true, "todo": [{"hex": "#010101"}]}));
    }

    #[test]
    fn palettes_round_trip_and_broken_data_is_kept_per_list_and_item() {
        let mut s = Settings::default();
        s.palette.per_area = true;
        s.palette.shared = Some(vec![PaletteColor::named("#2288ff", "Cooles Blau")]);
        s.palette.todo = Some(vec![]);
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.palette, s.palette);
        assert_eq!(back.palette.shared.as_ref().unwrap()[0].hex, "#2288FF");
        assert_eq!(back.palette.todo, Some(vec![]), "eine geleerte Liste bleibt leer");

        // Ganz kaputtes Feld: Vorgaben, die übrige Datei bleibt lesbar.
        let s: Settings = serde_json::from_str(r#"{"font_size":17.0,"palette":"kaputt"}"#).unwrap();
        assert_eq!(s.font_size, 17.0);
        assert!(s.palette.is_default());
        // Einzelne kaputte Einträge: nur sie fallen weg; ein falscher Name kostet nur den Namen; eine unlesbare Liste
        // ergibt nur für sich die Vorgaben; Doppelte entfallen, Namen werden gekürzt.
        let long = "x".repeat(100);
        let j = format!(
            r##"{{"palette":{{"per_area":"ja","shared":[{{"hex":"#abc"}},{{"hex":"112233","name":"  a\nb "}},{{"hex":"#112233"}},{{"hex":"#445566","name":"{long}"}},{{"hex":"#778899","name":5}},7,"#AABBCC"],"hub":"kaputt","todo":[{{"hex":"#010101"}}]}}}}"##
        );
        let s: Settings = serde_json::from_str(&j).unwrap();
        assert!(!s.palette.per_area, "falscher Typ: aus");
        assert_eq!(
            s.palette.shared.unwrap(),
            vec![PaletteColor::named("#112233", "a b"), PaletteColor::named("#445566", &"x".repeat(MAX_NAME_CHARS)), PaletteColor::new("#778899"), PaletteColor::new("#AABBCC")]
        );
        assert_eq!(s.palette.hub, None, "unlesbare Liste: Vorgaben");
        assert_eq!(s.palette.todo, Some(vec![PaletteColor::new("#010101")]), "die anderen Listen bleiben");
        // Nur ungültige Einträge (#221): Vorgaben, nicht leer; eine wirklich leere Liste bleibt leer.
        let s: Settings = serde_json::from_str(r##"{"palette":{"shared":[7,{"hex":"#xyz"},"kaputt"],"todo":[]}}"##).unwrap();
        assert_eq!(s.palette.shared, None, "nur ungültige Einträge: Vorgaben");
        assert_eq!(s.palette.todo, Some(vec![]));
        // Zu viele Farben: gekappt
        let many: Vec<String> = (0..100).map(|i| format!(r##"{{"hex":"#0000{i:02X}"}}"##)).collect();
        let s: Settings = serde_json::from_str(&format!(r#"{{"palette":{{"shared":[{}]}}}}"#, many.join(","))).unwrap();
        assert_eq!(s.palette.shared.unwrap().len(), MAX_COLORS);
    }

    #[test]
    fn switching_between_shared_and_per_area_keeps_both() {
        let mut p = Palettes::default();
        p.set(Which::Shared, vec![PaletteColor::new("#111111")]);
        assert_eq!(p.colors(Area::Todo), vec![PaletteColor::new("#111111")]);
        p.per_area = true;
        let mut todo = p.list(Which::Area(Area::Todo));
        todo.push(PaletteColor::new("#222222"));
        p.set(Which::Area(Area::Todo), todo);
        p.set(Which::Area(Area::Hub), vec![PaletteColor::new("#333333")]);
        assert_eq!(p.colors(Area::Hub), vec![PaletteColor::new("#333333")]);
        assert_eq!(p.colors(Area::Todo).last().unwrap().hex, "#222222");
        assert_eq!(hexes(&p.colors(Area::Expert)), EXPERT_DEFAULTS.to_vec());
        p.per_area = false;
        assert_eq!(p.colors(Area::Hub), vec![PaletteColor::new("#111111")]);
        assert_eq!(p.hub, Some(vec![PaletteColor::new("#333333")]));
        p.per_area = true;
        assert_eq!(p.colors(Area::Hub), vec![PaletteColor::new("#333333")]);
        assert_eq!(p.shared, Some(vec![PaletteColor::new("#111111")]));
        // Eine Liste gleich den Vorgaben wird nicht gespeichert (folgt künftigen Vorgaben).
        p.set(Which::Area(Area::Hub), list_of(&HUB_DEFAULTS));
        assert_eq!(p.hub, None);
    }

    #[test]
    fn restore_defaults_re_adds_only_deleted_ones_and_stops_when_full() {
        let mut l = vec![PaletteColor::named("#E06C75", "Rot"), PaletteColor::new("#123456")];
        assert_eq!(missing_defaults(&l, &TODO_DEFAULTS), TODO_DEFAULTS.len() - 1);
        let n = restore_defaults(&mut l, &TODO_DEFAULTS);
        assert_eq!(n, TODO_DEFAULTS.len() - 1);
        assert_eq!(l[0], PaletteColor::named("#E06C75", "Rot"));
        assert_eq!(l[1].hex, "#123456");
        assert_eq!(l[2].hex, TODO_DEFAULTS[1]);
        assert_eq!(restore_defaults(&mut l, &TODO_DEFAULTS), 0);
        // fast voll: nur bis zur Grenze
        let mut full: Vec<PaletteColor> = (0..MAX_COLORS - 3).map(|i| PaletteColor::new(&format!("#0000{i:02X}"))).collect();
        assert_eq!(restore_defaults(&mut full, &SHARED_DEFAULTS), 3);
        assert_eq!(full.len(), MAX_COLORS);
        assert_eq!(missing_defaults(&full, &SHARED_DEFAULTS), SHARED_DEFAULTS.len() - 3);
    }

    #[test]
    fn auto_assignment_keeps_the_old_order_until_edited_and_falls_back_when_empty() {
        let mut p = Palettes::default();
        // unverändert: die bisherige Vorgabe des Bereichs – nach dem Update ändert sich nichts
        assert_eq!(p.auto(Area::Hub), HUB_DEFAULTS.map(String::from).to_vec());
        assert_eq!(p.auto(Area::Todo), TODO_DEFAULTS.map(String::from).to_vec());
        assert_eq!(p.auto(Area::Expert), EXPERT_DEFAULTS.map(String::from).to_vec());
        // bearbeitet: die eigene Liste
        p.set(Which::Shared, vec![PaletteColor::new("#010101"), PaletteColor::new("#020202")]);
        let pool = p.auto(Area::Todo);
        assert_eq!(pool, ["#010101", "#020202"]);
        assert_eq!(least_used(&pool, []).unwrap(), "#010101");
        assert_eq!(least_used(&pool, ["#010101"]).unwrap(), "#020202");
        assert_eq!(least_used(&pool, ["#010101", "#020202"]).unwrap(), "#010101");
        assert_eq!(least_used(&pool, ["#ffffff"]).unwrap(), "#010101");
        // leer: die Vorgabe des Bereichs, die Auswahl zeigt die leere Palette
        p.set(Which::Shared, vec![]);
        assert_eq!(p.auto(Area::Expert)[0], EXPERT_DEFAULTS[0]);
        assert!(p.colors(Area::Expert).is_empty());
        p.per_area = true;
        p.set(Which::Area(Area::Todo), vec![]);
        assert_eq!(p.auto(Area::Todo), TODO_DEFAULTS.map(String::from).to_vec());
        assert_eq!(least_used(&[], ["#010101"]), None);
        // ohne Veröffentlichung: die Vorgaben
        assert_eq!(nth(Area::Hub, 0), HUB_DEFAULTS[0]);
        assert_eq!(nth(Area::Hub, HUB_DEFAULTS.len() + 1), HUB_DEFAULTS[1], "läuft im Kreis");
        assert_eq!(nth(Area::Expert, 3), EXPERT_DEFAULTS[3]);
    }

    #[test]
    fn tooltip_shows_the_hex_and_the_name_if_any() {
        assert_eq!(PaletteColor::new("#2288ff").tooltip(), "#2288FF");
        assert_eq!(PaletteColor::named("#2288FF", "Cooles Blau").tooltip(), "Cooles Blau (#2288FF)");
        assert_eq!(PaletteColor::named("#2288FF", "   ").tooltip(), "#2288FF");
    }

    #[test]
    fn hex_helpers_and_moving() {
        assert_eq!(normalize_hex(" 2288ff "), Some("#2288FF".into()));
        assert_eq!(normalize_hex("#22g8ff"), None);
        assert_eq!(rgb("#2288FF"), [0x22, 0x88, 0xFF]);
        assert_eq!(hex_of([0x22, 0x88, 0xFF]), "#2288FF");
        let mut v = vec![1, 2, 3, 4];
        assert!(move_before(&mut v, 0, 4));
        assert_eq!(v, [2, 3, 4, 1]);
        assert!(move_before(&mut v, 3, 0));
        assert_eq!(v, [1, 2, 3, 4]);
        assert!(move_before(&mut v, 1, 3));
        assert_eq!(v, [1, 3, 2, 4]);
        assert!(!move_before(&mut v, 1, 1) && !move_before(&mut v, 1, 2) && !move_before(&mut v, 9, 0));
        let full = list_of(&SHARED_DEFAULTS);
        let s = suggest_new(&full, &SHARED_DEFAULTS);
        assert!(normalize_hex(&s).is_some() && !SHARED_DEFAULTS.contains(&s.as_str()));
        assert_eq!(suggest_new(&full[1..], &SHARED_DEFAULTS), SHARED_DEFAULTS[0]);
    }
}
