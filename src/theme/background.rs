//! Eigener Hintergrund eines Themes (#387): optionales Objekt `background` in `themes/*.json` (öffentliches Format, neu in
//! 3.2.0). Bild, Farbverlauf, Farbe oder Muster, dazu Helligkeit, Unschärfe, Überlagerung und je Bereich (Terminals,
//! Editor, To-do, Chat, Seitenleiste) ein Schalter mit Deckkraft. Kennt kein egui außer `Color32`.
//!
//! Regeln: Jeder Wert ist optional und tolerant – ein ungültiger Wert fällt auf seinen Standard zurück, das Theme bleibt
//! gültig. Ungültige Werte bekannter Felder und unbekannte Felder (neuere Versionen) bleiben roh in [`Background::extra`]
//! und werden beim Schreiben unverändert wieder eingefügt. Bilddateien stehen nur als **bloßer Dateiname** im Ordner
//! `themes/<datei>/` neben `<datei>.json` ([`safe_file_name`]); Pfade, `..`, Laufwerke, UNC und Symlinks werden nie
//! benutzt ([`resolve_file`]).

use super::color::over;
use super::tokens::{resolve, Spec};
use super::Theme;
use eframe::egui::Color32;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// Endungen, die ein Hintergrundbild haben darf (Groß-/Kleinschreibung egal).
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp"];

/// Längster Dateiname eines Bildes.
const NAME_MAX: usize = 120;

/// Bereich der Oberfläche, der den Hintergrund durchscheinen lassen kann.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Area {
    Terminals,
    Editor,
    Todo,
    Chat,
    Sidebar,
}

impl Area {
    pub const ALL: [Area; 5] = [Area::Terminals, Area::Editor, Area::Todo, Area::Chat, Area::Sidebar];

    /// Name in der Datei (öffentlich).
    pub fn key(self) -> &'static str {
        match self {
            Area::Terminals => "terminals",
            Area::Editor => "editor",
            Area::Todo => "todo",
            Area::Chat => "chat",
            Area::Sidebar => "sidebar",
        }
    }

    pub fn label(self, lang: crate::state::Lang) -> &'static str {
        match self {
            Area::Terminals => lang.t("Terminals", "Terminals"),
            Area::Editor => lang.t("Editor", "Editor"),
            Area::Todo => lang.t("To-do und Planung", "To-do and planning"),
            Area::Chat => lang.t("Chat", "Chat"),
            Area::Sidebar => lang.t("Seitenleiste (mit Titelleiste und Reitern)", "Sidebar (with title bar and tabs)"),
        }
    }

    fn index(self) -> usize {
        Area::ALL.iter().position(|a| *a == self).unwrap_or(0)
    }

    /// Standard-Deckkraft der Fläche über dem Hintergrund (Prozent).
    fn default_opacity(self) -> u8 {
        match self {
            Area::Terminals => 80,
            Area::Editor => 85,
            Area::Todo => 85,
            Area::Chat => 85,
            Area::Sidebar => 75,
        }
    }
}

/// Schalter und Deckkraft eines Bereichs. Aus = die Fläche bleibt deckend.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct AreaCfg {
    pub on: bool,
    /// Deckkraft der Fläche über dem Hintergrund (0–100 %).
    pub opacity: u8,
}

/// Art des Hintergrunds.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Kind {
    Image,
    Gradient,
    Color,
    Pattern,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Image, Kind::Gradient, Kind::Color, Kind::Pattern];

    pub fn key(self) -> &'static str {
        match self {
            Kind::Image => "image",
            Kind::Gradient => "gradient",
            Kind::Color => "color",
            Kind::Pattern => "pattern",
        }
    }

    fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.key() == s)
    }

    pub fn label(self, lang: crate::state::Lang) -> &'static str {
        match self {
            Kind::Image => lang.t("Bild", "Image"),
            Kind::Gradient => lang.t("Farbverlauf", "Gradient"),
            Kind::Color => lang.t("Farbe", "Colour"),
            Kind::Pattern => lang.t("Muster", "Pattern"),
        }
    }
}

/// Skalierung des Bildes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Mode {
    /// Seitenverhältnis bleibt, Überstand wird abgeschnitten (Standard).
    Fill,
    /// Seitenverhältnis bleibt, Rest in der Grundfarbe.
    Fit,
    /// Auf die Fläche gezogen.
    Stretch,
    /// 1:1 (ein Bildpunkt = ein Bildschirmpunkt) am Anker.
    Original,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Fill, Mode::Fit, Mode::Stretch, Mode::Original];

    pub fn key(self) -> &'static str {
        match self {
            Mode::Fill => "fill",
            Mode::Fit => "fit",
            Mode::Stretch => "stretch",
            Mode::Original => "original",
        }
    }

    pub fn label(self, lang: crate::state::Lang) -> &'static str {
        match self {
            Mode::Fill => lang.t("Füllen", "Fill"),
            Mode::Fit => lang.t("Einpassen", "Fit"),
            Mode::Stretch => lang.t("Strecken", "Stretch"),
            Mode::Original => lang.t("Original", "Original"),
        }
    }
}

/// Einer von 9 Ankerpunkten (wie `backgroundImageAlignment` in Windows Terminal).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Anchor {
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Anchor {
    pub const ALL: [Anchor; 9] = [Anchor::TopLeft, Anchor::Top, Anchor::TopRight, Anchor::Left, Anchor::Center, Anchor::Right, Anchor::BottomLeft, Anchor::Bottom, Anchor::BottomRight];

    pub fn key(self) -> &'static str {
        match self {
            Anchor::TopLeft => "top_left",
            Anchor::Top => "top",
            Anchor::TopRight => "top_right",
            Anchor::Left => "left",
            Anchor::Center => "center",
            Anchor::Right => "right",
            Anchor::BottomLeft => "bottom_left",
            Anchor::Bottom => "bottom",
            Anchor::BottomRight => "bottom_right",
        }
    }

    /// Lage in x und y (0 = links/oben, 1 = Mitte, 2 = rechts/unten).
    pub fn grid(self) -> (u8, u8) {
        let i = Anchor::ALL.iter().position(|a| *a == self).unwrap_or(4) as u8;
        (i % 3, i / 3)
    }
}

/// Art des Farbverlaufs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum GradKind {
    Linear,
    Radial,
}

/// Ein Stopp des Verlaufs: Lage 0–100 % und Farbe (Hex oder Verweis wie `@accent/40`).
#[derive(Clone, PartialEq, Debug)]
pub struct Stop {
    pub pos: u8,
    pub color: Spec,
}

/// Farbverlauf mit 2–4 Stopps.
#[derive(Clone, PartialEq, Debug)]
pub struct Gradient {
    pub kind: GradKind,
    /// Winkel (linear) in Grad, 0–359; 0 = von oben nach unten, 90 = von links nach rechts.
    pub angle: u16,
    /// Mittelpunkt (radial) in Prozent der Fläche.
    pub center: [u8; 2],
    /// Radius (radial) in Prozent der halben Diagonale, 1–200.
    pub radius: u16,
    pub stops: Vec<Stop>,
}

impl Default for Gradient {
    fn default() -> Self {
        Gradient {
            kind: GradKind::Linear,
            angle: 135,
            center: [50, 50],
            radius: 100,
            stops: vec![Stop { pos: 0, color: Spec::Link { to: "bg".into(), alpha: None } }, Stop { pos: 100, color: Spec::Link { to: "accent".into(), alpha: Some(40) } }],
        }
    }
}

/// Eingebautes Muster (in den Theme-Farben erzeugt, keine Bilddatei).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum PatternName {
    Dots,
    Lines,
    Grid,
    Checks,
    Diagonal,
    Noise,
}

impl PatternName {
    pub const ALL: [PatternName; 6] = [PatternName::Dots, PatternName::Lines, PatternName::Grid, PatternName::Checks, PatternName::Diagonal, PatternName::Noise];

    pub fn key(self) -> &'static str {
        match self {
            PatternName::Dots => "dots",
            PatternName::Lines => "lines",
            PatternName::Grid => "grid",
            PatternName::Checks => "checks",
            PatternName::Diagonal => "diagonal",
            PatternName::Noise => "noise",
        }
    }

    pub fn label(self, lang: crate::state::Lang) -> &'static str {
        match self {
            PatternName::Dots => lang.t("Punkte", "Dots"),
            PatternName::Lines => lang.t("Linien", "Lines"),
            PatternName::Grid => lang.t("Raster", "Grid"),
            PatternName::Checks => lang.t("Karo", "Checks"),
            PatternName::Diagonal => lang.t("Schräge Linien", "Diagonal lines"),
            PatternName::Noise => lang.t("Rauschen", "Noise"),
        }
    }
}

/// Muster: eingebaut oder eigenes Kachelbild (`image` geht vor), Größe 25–400 %, Versatz in Punkten.
#[derive(Clone, PartialEq, Debug)]
pub struct Pattern {
    pub name: PatternName,
    pub image: Option<String>,
    /// Farbe des eingebauten Musters (über der Grundfarbe).
    pub color: Spec,
    pub scale: u16,
    pub offset: [i32; 2],
}

impl Default for Pattern {
    fn default() -> Self {
        Pattern { name: PatternName::Dots, image: None, color: Spec::Link { to: "text".into(), alpha: Some(12) }, scale: 100, offset: [0, 0] }
    }
}

/// Überlagerung über dem Bild (unter allen Flächen).
#[derive(Clone, PartialEq, Debug)]
pub struct Overlay {
    pub color: Spec,
    pub opacity: u8,
}

/// Der Hintergrund eines Themes.
#[derive(Clone, PartialEq, Debug)]
pub struct Background {
    pub kind: Kind,
    /// Bilddatei (bloßer Name im Ordner des Themes); `None` oder ungültig = nur die Grundfarbe.
    pub image: Option<String>,
    pub mode: Mode,
    pub anchor: Anchor,
    /// Abstand oben, rechts, unten, links in Punkten.
    pub margin: [u16; 4],
    /// Deckkraft des ganzen Hintergrunds (Prozent); unter 100 scheint der Desktop durch.
    pub opacity: u8,
    /// Farbe der Art „Farbe“; `None` = Grundfarbe des Themes.
    pub color: Option<Spec>,
    pub gradient: Option<Gradient>,
    pub pattern: Option<Pattern>,
    /// −100 (schwarz) … +100 (weiß).
    pub brightness: i8,
    /// Unschärfe in Punkten (0–40).
    pub blur: u8,
    pub overlay: Option<Overlay>,
    pub areas: [AreaCfg; 5],
    /// Unbekannte Felder und ungültige Werte bekannter Felder (roh, werden wieder geschrieben).
    pub extra: Map<String, Value>,
}

/// Felder von `background`, die diese Version kennt.
const KNOWN: &[&str] = &["kind", "image", "mode", "anchor", "margin", "opacity", "color", "gradient", "pattern", "brightness", "blur", "overlay", "areas"];

impl Background {
    /// Neuer Hintergrund der Art `kind` mit Standardwerten.
    pub fn new(kind: Kind) -> Background {
        Background {
            kind,
            image: None,
            mode: Mode::Fill,
            anchor: Anchor::Center,
            margin: [0; 4],
            opacity: 100,
            color: (kind == Kind::Color).then(|| Spec::Link { to: "accent".into(), alpha: Some(25) }),
            gradient: (kind == Kind::Gradient).then(Gradient::default),
            pattern: (kind == Kind::Pattern).then(Pattern::default),
            brightness: 0,
            blur: 0,
            overlay: None,
            areas: Area::ALL.map(|a| AreaCfg { on: true, opacity: a.default_opacity() }),
            extra: Map::new(),
        }
    }

    pub fn area(&self, a: Area) -> AreaCfg {
        self.areas[a.index()]
    }

    pub fn area_mut(&mut self, a: Area) -> &mut AreaCfg {
        &mut self.areas[a.index()]
    }

    /// Ungültige Werte bekannter Felder verwerfen (nach einer Änderung im Editor gilt der neue Wert).
    pub fn clear_invalid(&mut self) {
        self.extra.retain(|k, _| !KNOWN.contains(&k.as_str()));
    }

    /// Bilddateien, auf die der Hintergrund verweist (Bild und Kachel des Musters).
    pub fn files(&self) -> Vec<&str> {
        let mut v: Vec<&str> = Vec::new();
        if let Some(i) = self.image.as_deref() {
            v.push(i);
        }
        if let Some(i) = self.pattern.as_ref().and_then(|p| p.image.as_deref()) {
            if !v.contains(&i) {
                v.push(i);
            }
        }
        v
    }

    /// Deckkraft der Fläche `a` (0–1): Bereich an = globale Deckkraft × Deckkraft des Bereichs, aus = deckend.
    pub fn surface_alpha(&self, a: Area, global: f32) -> f32 {
        let c = self.area(a);
        if c.on {
            (global.clamp(0.0, 1.0) * c.opacity.min(100) as f32 / 100.0).clamp(0.0, 1.0)
        } else {
            1.0
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Dateinamen und Pfade

/// Gerätename unter Windows (auch mit Endung, „con.png“).
fn reserved(stem: &str) -> bool {
    let s = stem.to_ascii_lowercase();
    let b = s.as_bytes();
    matches!(s.as_str(), "con" | "prn" | "aux" | "nul") || (b.len() == 4 && (s.starts_with("com") || s.starts_with("lpt")) && (b'1'..=b'9').contains(&b[3]))
}

/// Ist `s` ein bloßer, sicherer Dateiname eines Bildes? Keine Trenner (`/`, `\`), kein `:` (Laufwerk, Datenstrom),
/// kein `..`, keine Steuerzeichen, kein Leerzeichen oder Punkt am Rand, kein Windows-Gerätename, Bild-Endung.
pub fn safe_file_name(s: &str) -> bool {
    if s.is_empty() || s.chars().count() > NAME_MAX || s != s.trim() || s.starts_with('.') || s.ends_with('.') {
        return false;
    }
    if s.chars().any(|c| c == '/' || c == '\\' || c == ':' || c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*')) {
        return false;
    }
    if s.contains("..") {
        return false;
    }
    let p = Path::new(s);
    let (Some(stem), Some(ext)) = (p.file_stem().and_then(|x| x.to_str()), p.extension().and_then(|x| x.to_str())) else { return false };
    !stem.is_empty() && !reserved(stem.split('.').next().unwrap_or(stem)) && IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
}

/// Ordner der Bilder eines Themes: `themes/<datei>/` neben `<datei>.json`. `None` ohne Datei (eingebautes Theme).
pub fn folder_of(source: &Path) -> Option<PathBuf> {
    let stem = source.file_stem()?.to_str()?;
    if stem.is_empty() {
        return None;
    }
    Some(source.parent()?.join(stem))
}

/// Ist `p` ein echter Ordner (kein Symlink, keine Verknüpfung/Junction)?
pub fn real_dir(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
}

/// Ist `p` eine echte Datei (kein Symlink)?
pub fn real_file(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink())
}

/// Pfad einer Bilddatei des Themes `source`, nur wenn der Name sicher ist, Ordner und Datei echt sind (keine Symlinks)
/// und die Datei wirklich im Ordner liegt. Sonst `None` (der Hintergrund zeigt dann nur die Grundfarbe).
pub fn resolve_file(source: &Path, name: &str) -> Option<PathBuf> {
    if !safe_file_name(name) {
        return None;
    }
    let dir = folder_of(source)?;
    if !real_dir(&dir) {
        return None;
    }
    let p = dir.join(name);
    if !real_file(&p) {
        return None;
    }
    let (cd, cp) = (std::fs::canonicalize(&dir).ok()?, std::fs::canonicalize(&p).ok()?);
    (cp.parent() == Some(cd.as_path())).then_some(p)
}

// ---------------------------------------------------------------------------------------------------------------
// Farben

/// Farbe eines Werts im Theme `t` (Verweise aufgelöst, Deckkraft angewandt).
pub fn spec_color(t: &Theme, s: &Spec) -> Color32 {
    match s {
        Spec::Hex(c) => *c,
        Spec::Link { to, alpha } => {
            let c = resolve(t, to);
            match alpha {
                Some(a) => c.gamma_multiply(*a as f32 / 100.0),
                None => c,
            }
        }
    }
}

/// Helligkeit und Überlagerung auf eine (deckende) Farbe anwenden – genau wie beim Berechnen des Bildes.
/// `overlay`: Farbe (mit eigener Deckkraft) und Deckkraft der Überlagerung (0–100).
pub fn post(c: Color32, brightness: i8, overlay: Option<(Color32, u8)>) -> Color32 {
    let b = brightness.clamp(-100, 100) as f32 / 100.0;
    let target = if b < 0.0 { 0.0 } else { 255.0 };
    let k = b.abs();
    let m = |v: u8| (v as f32 + (target - v as f32) * k).round().clamp(0.0, 255.0) as u8;
    let mut out = Color32::from_rgb(m(c.r()), m(c.g()), m(c.b()));
    if let Some((oc, op)) = overlay {
        let [r, g, bb, a] = oc.to_srgba_unmultiplied();
        let a = (a as f32 * op.min(100) as f32 / 100.0).round() as u8;
        out = over(Color32::from_rgba_unmultiplied(r, g, bb, a), out);
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// Lesen und Schreiben

fn spec_of(v: &Value) -> Option<Spec> {
    Spec::parse(v.as_str()?).ok()
}

fn pct(v: &Value, max: i64) -> Option<i64> {
    let n = v.as_i64().or_else(|| v.as_f64().filter(|f| f.is_finite()).map(|f| f.round() as i64))?;
    (0..=max).contains(&n).then_some(n)
}

fn parse_gradient(v: &Value) -> Option<Gradient> {
    let o = v.as_object()?;
    let mut g = Gradient::default();
    if let Some(k) = o.get("type") {
        g.kind = match k.as_str()? {
            "linear" => GradKind::Linear,
            "radial" => GradKind::Radial,
            _ => return None,
        };
    }
    if let Some(a) = o.get("angle") {
        g.angle = pct(a, 359)? as u16;
    }
    if let Some(c) = o.get("center") {
        let a = c.as_array()?;
        if a.len() != 2 {
            return None;
        }
        g.center = [pct(&a[0], 100)? as u8, pct(&a[1], 100)? as u8];
    }
    if let Some(r) = o.get("radius") {
        g.radius = pct(r, 200)?.max(1) as u16;
    }
    if let Some(s) = o.get("stops") {
        let a = s.as_array()?;
        if !(2..=4).contains(&a.len()) {
            return None;
        }
        let mut stops = Vec::new();
        for st in a {
            let so = st.as_object()?;
            stops.push(Stop { pos: pct(so.get("pos")?, 100)? as u8, color: spec_of(so.get("color")?)? });
        }
        stops.sort_by_key(|s| s.pos);
        g.stops = stops;
    }
    Some(g)
}

fn parse_pattern(v: &Value) -> Option<Pattern> {
    let o = v.as_object()?;
    let mut p = Pattern::default();
    if let Some(n) = o.get("name") {
        p.name = PatternName::ALL.into_iter().find(|x| Some(x.key()) == n.as_str())?;
    }
    if let Some(i) = o.get("image") {
        match i {
            Value::Null => {}
            Value::String(s) if safe_file_name(s) => p.image = Some(s.clone()),
            _ => return None,
        }
    }
    if let Some(c) = o.get("color") {
        p.color = spec_of(c)?;
    }
    if let Some(s) = o.get("scale") {
        let s = pct(s, 400)?;
        if s < 25 {
            return None;
        }
        p.scale = s as u16;
    }
    if let Some(off) = o.get("offset") {
        let a = off.as_array()?;
        if a.len() != 2 {
            return None;
        }
        let n = |v: &Value| v.as_i64().filter(|n| n.abs() <= 10_000).map(|n| n as i32);
        p.offset = [n(&a[0])?, n(&a[1])?];
    }
    Some(p)
}

/// `background` aus der Datei lesen. Immer erfolgreich, solange es ein Objekt ist; jeder ungültige Wert fällt auf
/// seinen Standard zurück und bleibt roh in `extra`. `None`, wenn `v` kein Objekt ist.
pub fn parse(v: &Value) -> Option<Background> {
    let o = v.as_object()?;
    let kind = o.get("kind").and_then(|k| k.as_str()).and_then(Kind::parse);
    let mut b = Background::new(kind.unwrap_or(Kind::Color));
    b.color = None;
    let mut extra = Map::new();
    let mut bad: Vec<String> = Vec::new();
    if kind.is_none() && o.contains_key("kind") {
        bad.push("kind".into());
    }
    for (k, raw) in o {
        let ok = match k.as_str() {
            "kind" => true,
            "image" => match raw {
                Value::Null => true,
                Value::String(s) if safe_file_name(s) => {
                    b.image = Some(s.clone());
                    true
                }
                _ => false,
            },
            "mode" => match Mode::ALL.into_iter().find(|m| Some(m.key()) == raw.as_str()) {
                Some(m) => {
                    b.mode = m;
                    true
                }
                None => false,
            },
            "anchor" => match Anchor::ALL.into_iter().find(|a| Some(a.key()) == raw.as_str()) {
                Some(a) => {
                    b.anchor = a;
                    true
                }
                None => false,
            },
            "margin" => match raw.as_array().filter(|a| a.len() == 4).and_then(|a| a.iter().map(|x| pct(x, 2000).map(|n| n as u16)).collect::<Option<Vec<u16>>>()) {
                Some(m) => {
                    b.margin = [m[0], m[1], m[2], m[3]];
                    true
                }
                None => false,
            },
            "opacity" => match pct(raw, 100) {
                Some(n) => {
                    b.opacity = n as u8;
                    true
                }
                None => false,
            },
            "color" => match spec_of(raw) {
                Some(s) => {
                    b.color = Some(s);
                    true
                }
                None => raw.is_null(),
            },
            "gradient" => match parse_gradient(raw) {
                Some(g) => {
                    b.gradient = Some(g);
                    true
                }
                None => raw.is_null(),
            },
            "pattern" => match parse_pattern(raw) {
                Some(p) => {
                    b.pattern = Some(p);
                    true
                }
                None => raw.is_null(),
            },
            "brightness" => match raw.as_i64().filter(|n| (-100..=100).contains(n)) {
                Some(n) => {
                    b.brightness = n as i8;
                    true
                }
                None => false,
            },
            "blur" => match pct(raw, 40) {
                Some(n) => {
                    b.blur = n as u8;
                    true
                }
                None => false,
            },
            "overlay" => match raw {
                Value::Null => true,
                Value::Object(oo) => match (oo.get("color").and_then(spec_of), oo.get("opacity").and_then(|x| pct(x, 100))) {
                    (Some(c), Some(op)) => {
                        b.overlay = Some(Overlay { color: c, opacity: op as u8 });
                        true
                    }
                    _ => false,
                },
                _ => false,
            },
            "areas" => match raw.as_object() {
                Some(m) => {
                    let mut all_ok = true;
                    for a in Area::ALL {
                        let Some(x) = m.get(a.key()) else { continue };
                        let Some(xo) = x.as_object() else {
                            all_ok = false;
                            continue;
                        };
                        let cur = b.area_mut(a);
                        match xo.get("on") {
                            None => {}
                            Some(Value::Bool(on)) => cur.on = *on,
                            Some(_) => all_ok = false,
                        }
                        match xo.get("opacity").map(|v| pct(v, 100)) {
                            None => {}
                            Some(Some(n)) => cur.opacity = n as u8,
                            Some(None) => all_ok = false,
                        }
                    }
                    // Unbekannte Bereiche (neuere Versionen) bleiben über `extra` erhalten.
                    all_ok && m.keys().all(|k| Area::ALL.iter().any(|a| a.key() == k))
                }
                None => false,
            },
            _ => {
                extra.insert(k.clone(), raw.clone());
                true
            }
        };
        if !ok {
            bad.push(k.clone());
        }
    }
    for k in bad {
        if let Some(raw) = o.get(&k) {
            extra.insert(k, raw.clone());
        }
    }
    // Die Art „Farbe“ ohne gültige Farbe zeigt die Grundfarbe; ein Verlauf/Muster ohne Angaben die Standardwerte.
    if b.kind == Kind::Gradient && b.gradient.is_none() && !extra.contains_key("gradient") {
        b.gradient = Some(Gradient::default());
    }
    if b.kind == Kind::Pattern && b.pattern.is_none() && !extra.contains_key("pattern") {
        b.pattern = Some(Pattern::default());
    }
    b.extra = extra;
    Some(b)
}

fn spec_json(s: &Spec) -> Value {
    json!(s.to_text())
}

/// `background` als JSON-Wert (feste Reihenfolge der bekannten Felder, dann die aufbewahrten unbekannten). Ein ungültiger
/// Wert eines bekannten Felds aus der Datei wird statt des Standards wieder geschrieben.
pub fn to_value(b: &Background) -> Value {
    let mut m = Map::new();
    let mut put = |k: &str, v: Value| {
        let v = b.extra.get(k).cloned().unwrap_or(v);
        m.insert(k.to_string(), v);
    };
    put("kind", json!(b.kind.key()));
    if b.image.is_some() || b.extra.contains_key("image") {
        put("image", b.image.as_ref().map(|s| json!(s)).unwrap_or(Value::Null));
    }
    put("mode", json!(b.mode.key()));
    put("anchor", json!(b.anchor.key()));
    put("margin", json!(b.margin));
    put("opacity", json!(b.opacity));
    if b.color.is_some() || b.extra.contains_key("color") {
        put("color", b.color.as_ref().map(spec_json).unwrap_or(Value::Null));
    }
    if b.gradient.is_some() || b.extra.contains_key("gradient") {
        put(
            "gradient",
            b.gradient
                .as_ref()
                .map(|g| {
                    json!({
                        "type": if g.kind == GradKind::Linear { "linear" } else { "radial" },
                        "angle": g.angle,
                        "center": g.center,
                        "radius": g.radius,
                        "stops": g.stops.iter().map(|s| json!({"pos": s.pos, "color": s.color.to_text()})).collect::<Vec<_>>(),
                    })
                })
                .unwrap_or(Value::Null),
        );
    }
    if b.pattern.is_some() || b.extra.contains_key("pattern") {
        put(
            "pattern",
            b.pattern
                .as_ref()
                .map(|p| {
                    let mut o = json!({"name": p.name.key(), "color": p.color.to_text(), "scale": p.scale, "offset": p.offset});
                    if let Some(i) = &p.image {
                        o["image"] = json!(i);
                    }
                    o
                })
                .unwrap_or(Value::Null),
        );
    }
    put("brightness", json!(b.brightness));
    put("blur", json!(b.blur));
    if b.overlay.is_some() || b.extra.contains_key("overlay") {
        put("overlay", b.overlay.as_ref().map(|o| json!({"color": o.color.to_text(), "opacity": o.opacity})).unwrap_or(Value::Null));
    }
    let mut areas = Map::new();
    for a in Area::ALL {
        let c = b.area(a);
        areas.insert(a.key().into(), json!({"on": c.on, "opacity": c.opacity}));
    }
    put("areas", Value::Object(areas));
    for (k, v) in &b.extra {
        if !KNOWN.contains(&k.as_str()) {
            m.insert(k.clone(), v.clone());
        }
    }
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_must_be_bare_and_safe() {
        for ok in ["forest.jpg", "a.PNG", "my bg-2.webp", "x.y.bmp", "ä.jpeg"] {
            assert!(safe_file_name(ok), "{ok}");
        }
        for bad in [
            "", "..", "../x.png", "..\\x.png", "a/b.png", "a\\b.png", "C:x.png", "C:\\x.png", "\\\\server\\share\\x.png", "/etc/x.png", "x.png:stream", " x.png", "x.png ", ".hidden.png", "x.", "x", "x.gif", "x.svg",
            "con.png", "NUL.jpg", "com1.png", "lpt9.webp", "a..b.png", "x\u{0}.png", "x?.png", "x*.png",
        ] {
            assert!(!safe_file_name(bad), "{bad:?}");
        }
        assert!(!safe_file_name(&format!("{}.png", "a".repeat(200))));
    }

    #[test]
    fn invalid_values_fall_back_and_survive_saving() {
        let v = json!({
            "kind": "image", "image": "../secret.png", "mode": "zoom", "anchor": "top_right", "margin": [1, 2, 3],
            "opacity": 150, "brightness": -30, "blur": 99, "overlay": {"color": "@bg", "opacity": 40},
            "areas": {"terminals": {"on": false, "opacity": 50}, "chat": {"opacity": "x"}, "future_area": {"on": true}},
            "future": {"a": 1}
        });
        let b = parse(&v).unwrap();
        assert_eq!(b.kind, Kind::Image);
        assert_eq!(b.image, None, "Pfad abgelehnt");
        assert_eq!((b.mode, b.anchor, b.margin, b.opacity), (Mode::Fill, Anchor::TopRight, [0; 4], 100));
        assert_eq!((b.brightness, b.blur), (-30, 0));
        assert_eq!(b.overlay.as_ref().map(|o| o.opacity), Some(40));
        assert_eq!(b.area(Area::Terminals), AreaCfg { on: false, opacity: 50 });
        assert_eq!(b.area(Area::Editor).opacity, 85, "Standard");
        // Rohwerte kommen unverändert zurück.
        let out = to_value(&b);
        for k in ["image", "mode", "margin", "opacity", "blur", "areas", "future"] {
            assert_eq!(out[k], v[k], "{k}: {out}");
        }
        assert_eq!(out["anchor"], "top_right");
        // Nach einer Änderung im Editor gelten die neuen Werte.
        let mut b2 = b.clone();
        b2.clear_invalid();
        b2.blur = 5;
        let out = to_value(&b2);
        assert_eq!(out["blur"], 5);
        assert_eq!(out["future"], json!({"a": 1}), "unbekannte Felder bleiben");
        assert!(out.get("image").is_none());
        // Kein Objekt: kein Hintergrund.
        assert!(parse(&json!("forest.jpg")).is_none());
        // Unbekannte Art (neuere Version): nur die Grundfarbe, roh erhalten.
        let b = parse(&json!({"kind": "video", "video": "a.mp4"})).unwrap();
        assert_eq!((b.kind, b.color.as_ref()), (Kind::Color, None));
        let out = to_value(&b);
        assert_eq!((out["kind"].as_str(), out["video"].as_str()), (Some("video"), Some("a.mp4")));
    }

    #[test]
    fn round_trip_of_all_kinds() {
        let mut b = Background::new(Kind::Gradient);
        b.gradient.as_mut().unwrap().stops.push(Stop { pos: 50, color: Spec::Hex(Color32::from_rgb(1, 2, 3)) });
        b.gradient.as_mut().unwrap().stops.sort_by_key(|s| s.pos);
        b.pattern = Some(Pattern { name: PatternName::Grid, image: Some("tile.png".into()), color: Spec::Link { to: "accent".into(), alpha: Some(30) }, scale: 200, offset: [3, -4] });
        b.image = Some("bild.webp".into());
        b.overlay = Some(Overlay { color: Spec::Hex(Color32::BLACK), opacity: 35 });
        b.margin = [1, 2, 3, 4];
        b.brightness = 20;
        b.blur = 12;
        b.mode = Mode::Original;
        b.anchor = Anchor::BottomLeft;
        b.area_mut(Area::Sidebar).on = false;
        let back = parse(&to_value(&b)).unwrap();
        assert_eq!(back, b);
        assert_eq!(b.files(), ["bild.webp", "tile.png"]);
        for k in Kind::ALL {
            let b = Background::new(k);
            assert_eq!(parse(&to_value(&b)).unwrap(), b, "{k:?}");
        }
        // Radial mit Mittelpunkt und Radius; Stopps werden nach Lage sortiert; falsche Anzahl = Standard.
        let g = parse_gradient(&json!({"type": "radial", "center": [20, 80], "radius": 150, "stops": [{"pos": 90, "color": "#ffffff"}, {"pos": 10, "color": "@bg/50"}]})).unwrap();
        assert_eq!((g.kind, g.center, g.radius, g.stops[0].pos), (GradKind::Radial, [20, 80], 150, 10));
        assert!(parse_gradient(&json!({"stops": [{"pos": 0, "color": "#000000"}]})).is_none());
        assert!(parse_pattern(&json!({"scale": 10})).is_none() && parse_pattern(&json!({"image": "../x.png"})).is_none());
    }

    #[test]
    fn surface_alpha_multiplies_global_and_area() {
        let mut b = Background::new(Kind::Image);
        b.area_mut(Area::Terminals).opacity = 50;
        assert!((b.surface_alpha(Area::Terminals, 0.8) - 0.4).abs() < 1e-6);
        b.area_mut(Area::Terminals).on = false;
        assert_eq!(b.surface_alpha(Area::Terminals, 0.5), 1.0, "aus = deckend");
    }

    #[test]
    fn post_matches_brightness_and_overlay() {
        let c = Color32::from_rgb(100, 100, 100);
        assert_eq!(post(c, -100, None), Color32::BLACK);
        assert_eq!(post(c, 100, None), Color32::WHITE);
        assert_eq!(post(c, 0, None), c);
        assert_eq!(post(c, -50, None), Color32::from_rgb(50, 50, 50));
        assert_eq!(post(c, 0, Some((Color32::BLACK, 100))), Color32::BLACK);
        assert_eq!(post(c, 0, Some((Color32::from_rgb(200, 200, 200), 50))), Color32::from_rgb(150, 150, 150));
    }

    #[test]
    fn files_resolve_only_inside_the_theme_folder() {
        let dir = std::env::temp_dir().join(format!("paneldeck-test-bgfiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("mine")).unwrap();
        std::fs::write(dir.join("mine.json"), "{}").unwrap();
        std::fs::write(dir.join("mine").join("a.png"), b"x").unwrap();
        std::fs::write(dir.join("other.png"), b"x").unwrap();
        let src = dir.join("mine.json");
        assert_eq!(folder_of(&src), Some(dir.join("mine")));
        assert_eq!(resolve_file(&src, "a.png"), Some(dir.join("mine").join("a.png")));
        assert_eq!(resolve_file(&src, "../other.png"), None);
        assert_eq!(resolve_file(&src, "missing.png"), None);
        std::fs::create_dir_all(dir.join("mine").join("sub.png")).unwrap();
        assert_eq!(resolve_file(&src, "sub.png"), None, "Ordner statt Datei");
        // Symlinks (wo das Anlegen erlaubt ist) werden nie benutzt.
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.join("other.png"), dir.join("mine").join("link.png")).unwrap();
            assert_eq!(resolve_file(&src, "link.png"), None);
        }
        #[cfg(windows)]
        if std::os::windows::fs::symlink_file(dir.join("other.png"), dir.join("mine").join("link.png")).is_ok() {
            assert_eq!(resolve_file(&src, "link.png"), None);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
