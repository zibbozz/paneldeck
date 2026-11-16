//! Farbpaletten (seit 2.3.0, Issue #173): die vorgeschlagenen Farben für Sitzungen, Ordner, Projekte und Profile
//! (Terminal-Hub), Tags der einfachen Todo-Liste und den Expertenmodus (Projekte, Typen, Status, Auswahlwerte, Diagramme).
//!
//! Standard ist **eine gemeinsame Palette** für alles. Mit `per_area` hat jeder Bereich seine eigene; beide Varianten
//! bleiben gespeichert, ein Umschalten verliert nichts. Gespeichert wird in `settings.json` (Feld `palette`, optional;
//! fehlt es, gelten die eingebauten Vorgaben). Elemente speichern ihren Farbwert selbst – ändern oder löschen einer
//! Palettenfarbe wirkt nur auf künftige Auswahlen.
//!
//! Die Oberfläche liest die Palette über [`current`] (die App veröffentlicht ihre Einstellungen jedes Bild mit
//! [`publish`]), damit nicht jede Auswahl die Einstellungen durchreichen muss. Kein egui hier.

use serde::{Deserialize, Serialize};
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
/// Hub-Farben (neue Sitzungen bekommen also dieselben Farben wie bisher), dann abwechselnd die übrigen, damit die
/// automatische Vergabe zuerst gut unterscheidbare Farben verteilt; die gedeckten Töne kommen zuletzt.
pub const SHARED_DEFAULTS: [&str; 20] = [
    "#61AFEF", "#98C379", "#E5C07B", "#E06C75", "#C678DD", "#56B6C2", "#D97757", "#888888", "#E5A04B", "#E07FB0", "#4A78B5", "#5E8F5E", "#B77FE0", "#4FB8A8",
    "#D8B94A", "#6F8FE8", "#7FBF6A", "#58A6E6", "#97A3B0", "#8C7B6B",
];

/// Längster Farbname in Zeichen.
pub const MAX_NAME_CHARS: usize = 40;
/// Höchstens so viele Farben je Palette (Schutz vor kaputten Dateien; die Oberfläche bietet „Hinzufügen“ dann nicht mehr an).
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
    pub fn defaults(self) -> &'static [&'static str] {
        match self {
            Which::Shared => &SHARED_DEFAULTS,
            Which::Area(a) => a.defaults(),
        }
    }
}

/// Eine Palettenfarbe: Wert `#RRGGBB` (Großbuchstaben) und optionaler Name.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct PaletteColor {
    pub hex: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
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
}

fn list(defaults: &[&str]) -> Vec<PaletteColor> {
    defaults.iter().map(|h| PaletteColor::new(h)).collect()
}

/// Alle Paletten (Feld `palette` in `settings.json`).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Palettes {
    /// `false` (Standard): eine gemeinsame Palette für alles. `true`: je Bereich eine eigene.
    pub per_area: bool,
    pub shared: Vec<PaletteColor>,
    pub hub: Vec<PaletteColor>,
    pub todo: Vec<PaletteColor>,
    pub expert: Vec<PaletteColor>,
}

impl Default for Palettes {
    fn default() -> Self {
        Self { per_area: false, shared: list(&SHARED_DEFAULTS), hub: list(&HUB_DEFAULTS), todo: list(&TODO_DEFAULTS), expert: list(&EXPERT_DEFAULTS) }
    }
}

impl Palettes {
    /// Unverändert gegenüber den Vorgaben (dann schreibt `settings.json` das Feld nicht – spätere Vorgaben gelten).
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Welche Liste für `area` gerade gilt.
    pub fn which(&self, area: Area) -> Which {
        if self.per_area { Which::Area(area) } else { Which::Shared }
    }

    pub fn get(&self, w: Which) -> &Vec<PaletteColor> {
        match w {
            Which::Shared => &self.shared,
            Which::Area(Area::Hub) => &self.hub,
            Which::Area(Area::Todo) => &self.todo,
            Which::Area(Area::Expert) => &self.expert,
        }
    }

    pub fn get_mut(&mut self, w: Which) -> &mut Vec<PaletteColor> {
        match w {
            Which::Shared => &mut self.shared,
            Which::Area(Area::Hub) => &mut self.hub,
            Which::Area(Area::Todo) => &mut self.todo,
            Which::Area(Area::Expert) => &mut self.expert,
        }
    }

    /// Die Farben, die die Auswahlen in `area` anbieten (kann leer sein).
    pub fn colors(&self, area: Area) -> &[PaletteColor] {
        self.get(self.which(area))
    }

    /// Farben für die automatische Vergabe in `area`. Ist die geltende Palette leer, gelten die eingebauten Vorgaben
    /// dieser Liste (gemeinsam bzw. des Bereichs) – automatisch vergeben wird also immer eine Farbe.
    pub fn auto(&self, area: Area) -> Vec<String> {
        let w = self.which(area);
        let l = self.get(w);
        if l.is_empty() { w.defaults().iter().map(|s| s.to_string()).collect() } else { l.iter().map(|c| c.hex.clone()).collect() }
    }

    /// Tolerant bereinigen (nach dem Lesen): ungültige Werte fallen weg, Hex in Großbuchstaben, Doppelte entfallen,
    /// Namen gekürzt, höchstens [`MAX_COLORS`] je Liste.
    pub fn sanitize(&mut self) {
        for w in [Which::Shared, Which::Area(Area::Hub), Which::Area(Area::Todo), Which::Area(Area::Expert)] {
            let l = self.get_mut(w);
            let mut out: Vec<PaletteColor> = Vec::new();
            for c in l.drain(..) {
                let Some(hex) = normalize_hex(&c.hex) else { continue };
                if out.iter().any(|o| o.hex == hex) || out.len() >= MAX_COLORS {
                    continue;
                }
                out.push(PaletteColor { hex, name: clean_name(&c.name) });
            }
            *l = out;
        }
    }

    /// Für `settings.json`: unlesbare Daten ergeben die Vorgaben (der Rest der Datei bleibt lesbar).
    pub fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        let mut p: Palettes = serde_json::from_value(v).unwrap_or_default();
        p.sanitize();
        Ok(p)
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

/// Gelöschte Vorgaben wieder anhängen (Reihenfolge der Vorgaben; vorhandene Farben und ihre Namen bleiben).
/// Liefert die Zahl der wieder hinzugefügten Farben.
pub fn restore_defaults(list: &mut Vec<PaletteColor>, defaults: &[&str]) -> usize {
    let mut n = 0;
    for d in defaults {
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
    // Farbkreis in Schritten absuchen (Sättigung/Helligkeit wie die Vorgaben).
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
    with_current(|p| p.colors(area).to_vec())
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

    #[test]
    fn shared_defaults_are_the_deduplicated_union_of_the_three_old_palettes() {
        let mut seen = std::collections::HashSet::new();
        for h in SHARED_DEFAULTS {
            assert_eq!(normalize_hex(h).as_deref(), Some(h), "Großbuchstaben, gültig");
            assert!(seen.insert(h), "keine Doppelten: {h}");
        }
        // Jede alte Farbe ist enthalten – außer den zwei fast gleichen, deren Zwilling drin ist.
        for h in HUB_DEFAULTS.iter().chain(TODO_DEFAULTS.iter()).chain(EXPERT_DEFAULTS.iter()) {
            let twin = match *h {
                "#7B8CF0" => "#6F8FE8",
                "#E07BB0" => "#E07FB0",
                o => o,
            };
            assert!(SHARED_DEFAULTS.contains(&twin), "{h} fehlt");
        }
        assert_eq!(&SHARED_DEFAULTS[..8], &HUB_DEFAULTS[..], "neue Sitzungen bekommen die bisherigen Farben");
    }

    #[test]
    fn old_settings_without_the_field_load_with_the_defaults() {
        let old = r#"{"language":"De","theme":"One Dark","font_size":15.0}"#;
        let s: Settings = serde_json::from_str(old).unwrap();
        assert!(s.palette.is_default());
        assert!(!s.palette.per_area);
        // Unveränderte Paletten werden nicht geschrieben (alte Versionen lesen die Datei weiter, neue Vorgaben gelten).
        let out = serde_json::to_string(&Settings::default()).unwrap();
        assert!(!out.contains("\"palette\""), "{out}");
    }

    #[test]
    fn palettes_round_trip_and_broken_data_falls_back() {
        let mut s = Settings::default();
        s.palette.per_area = true;
        s.palette.shared = vec![PaletteColor::named("#2288ff", "Cooles Blau")];
        s.palette.todo.clear();
        let json = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.palette, s.palette);
        assert_eq!(back.palette.shared[0].hex, "#2288FF");

        // Kaputtes Feld: Vorgaben, die übrige Datei bleibt lesbar.
        let s: Settings = serde_json::from_str(r#"{"font_size":17.0,"palette":"kaputt"}"#).unwrap();
        assert_eq!(s.font_size, 17.0);
        assert!(s.palette.is_default());
        // Einzelne ungültige Einträge und Doppelte fallen weg, Namen werden gekürzt.
        let long = "x".repeat(100);
        let j = format!(r##"{{"palette":{{"shared":[{{"hex":"#abc"}},{{"hex":"112233","name":"  a\nb "}},{{"hex":"#112233"}},{{"hex":"#445566","name":"{long}"}}]}}}}"##);
        let s: Settings = serde_json::from_str(&j).unwrap();
        assert_eq!(s.palette.shared, vec![PaletteColor::named("#112233", "a b"), PaletteColor::named("#445566", &"x".repeat(MAX_NAME_CHARS))]);
        assert_eq!(s.palette.hub.len(), HUB_DEFAULTS.len(), "fehlende Listen: Vorgaben");
    }

    #[test]
    fn switching_between_shared_and_per_area_keeps_both() {
        let mut p = Palettes::default();
        p.shared = vec![PaletteColor::new("#111111")];
        assert_eq!(p.colors(Area::Todo), &[PaletteColor::new("#111111")][..]);
        p.per_area = true;
        p.get_mut(Which::Area(Area::Todo)).push(PaletteColor::new("#222222"));
        p.hub = vec![PaletteColor::new("#333333")];
        assert_eq!(p.colors(Area::Hub), &[PaletteColor::new("#333333")][..]);
        assert_eq!(p.colors(Area::Todo).last().unwrap().hex, "#222222");
        assert_eq!(p.colors(Area::Expert).len(), EXPERT_DEFAULTS.len());
        // zurück zur gemeinsamen: die je Bereich bleiben gespeichert
        p.per_area = false;
        assert_eq!(p.colors(Area::Hub), &[PaletteColor::new("#111111")][..]);
        assert_eq!(p.hub, vec![PaletteColor::new("#333333")]);
        // und gelten wieder
        p.per_area = true;
        assert_eq!(p.colors(Area::Hub), &[PaletteColor::new("#333333")][..]);
        assert_eq!(p.shared, vec![PaletteColor::new("#111111")]);
    }

    #[test]
    fn restore_defaults_re_adds_only_deleted_ones() {
        let mut l = vec![PaletteColor::named("#E06C75", "Rot"), PaletteColor::new("#123456")];
        let n = restore_defaults(&mut l, &TODO_DEFAULTS);
        assert_eq!(n, TODO_DEFAULTS.len() - 1);
        assert_eq!(l[0], PaletteColor::named("#E06C75", "Rot"), "vorhandene behalten Namen und Platz");
        assert_eq!(l[1].hex, "#123456");
        assert_eq!(l[2].hex, TODO_DEFAULTS[1]);
        assert_eq!(restore_defaults(&mut l, &TODO_DEFAULTS), 0, "nichts doppelt");
        let mut empty = Vec::new();
        assert_eq!(restore_defaults(&mut empty, &SHARED_DEFAULTS), SHARED_DEFAULTS.len());
    }

    #[test]
    fn auto_assignment_rotates_and_falls_back_to_the_defaults_when_empty() {
        let mut p = Palettes::default();
        assert_eq!(p.auto(Area::Hub), SHARED_DEFAULTS.map(String::from).to_vec());
        p.shared = vec![PaletteColor::new("#010101"), PaletteColor::new("#020202")];
        let pool = p.auto(Area::Todo);
        assert_eq!(least_used(&pool, []).unwrap(), "#010101");
        assert_eq!(least_used(&pool, ["#010101"]).unwrap(), "#020202");
        assert_eq!(least_used(&pool, ["#010101", "#020202"]).unwrap(), "#010101", "alle gleich oft → von vorn");
        assert_eq!(least_used(&pool, ["#ffffff"]).unwrap(), "#010101", "fremde Farben zählen nicht");
        // leere gemeinsame Palette → eingebaute gemeinsame Vorgaben
        p.shared.clear();
        assert_eq!(p.auto(Area::Expert)[0], SHARED_DEFAULTS[0]);
        assert!(p.colors(Area::Expert).is_empty(), "die Auswahl zeigt die leere Palette");
        // leere Bereichspalette → Vorgaben dieses Bereichs
        p.per_area = true;
        p.todo.clear();
        assert_eq!(p.auto(Area::Todo), TODO_DEFAULTS.map(String::from).to_vec());
        assert_eq!(least_used(&[], ["#010101"]), None);
        // ohne Veröffentlichung: die Vorgaben
        assert_eq!(nth(Area::Hub, 0), SHARED_DEFAULTS[0]);
        assert_eq!(nth(Area::Hub, SHARED_DEFAULTS.len() + 1), SHARED_DEFAULTS[1], "läuft im Kreis");
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
        let full: Vec<PaletteColor> = SHARED_DEFAULTS.iter().map(|h| PaletteColor::new(h)).collect();
        let s = suggest_new(&full, &SHARED_DEFAULTS);
        assert!(normalize_hex(&s).is_some() && !SHARED_DEFAULTS.contains(&s.as_str()));
        assert_eq!(suggest_new(&full[1..], &SHARED_DEFAULTS), SHARED_DEFAULTS[0]);
    }
}
