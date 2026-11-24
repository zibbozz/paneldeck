//! Hintergrund eines Themes berechnen (#387): aus [`Background`] und den Farben des Themes entsteht ein Bild in genau der
//! Größe des Fensters (physische Pixel). Reihenfolge: Grundfarbe → Ebene (Bild, Verlauf, Farbe oder Muster) in der Fläche
//! ohne Abstände → Unschärfe → Kennzahlen ([`Stats`]) → Helligkeit und Überlagerung ([`super::background::post`]) →
//! Deckkraft. Läuft nur im Hintergrund-Faden ([`submit`]); kennt kein egui außer `Color32`.
//!
//! Dazu: Bilder einlesen ([`prepare`], gleiche Grenzen wie Bilder in Chats, höchstens [`MAX_SIDE`] Pixel je Seite in der
//! Kopie des Themes) und ein kleiner Zwischenspeicher der zuletzt gelesenen Quellbilder.

use super::background::{post, spec_color, Anchor, Background, GradKind, Kind, Mode, PatternName};
use super::color::luminance;
use super::Theme;
use eframe::egui::Color32;
use image::RgbaImage;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

/// Längere Seite der Kopie im Ordner des Themes und eines gelesenen Quellbilds (Pixel).
pub const MAX_SIDE: u32 = 3840;

/// Formate eines Hintergrundbilds.
const FORMATS: &[image::ImageFormat] = &[image::ImageFormat::Png, image::ImageFormat::Jpeg, image::ImageFormat::WebP, image::ImageFormat::Bmp];

/// Quelle eines Bildes: Datei im Ordner des Themes (mit Änderungszeit und Größe, damit ein ersetztes Bild neu gelesen
/// wird) oder ein schon gelesenes Bild (gewählt, aber noch nicht gespeichert).
#[derive(Clone, Debug)]
pub enum Src {
    File { path: PathBuf, stamp: (u64, u64) },
    Mem { id: u64, img: Arc<RgbaImage> },
}

impl Src {
    /// Datei mit aktuellem Stempel (`None`, wenn sie nicht lesbar ist).
    pub fn file(path: PathBuf) -> Option<Src> {
        let m = std::fs::metadata(&path).ok()?;
        let t = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0);
        Some(Src::File { path, stamp: (t, m.len()) })
    }
}

impl PartialEq for Src {
    fn eq(&self, o: &Src) -> bool {
        match (self, o) {
            (Src::File { path: a, stamp: s }, Src::File { path: b, stamp: t }) => a == b && s == t,
            (Src::Mem { id: a, .. }, Src::Mem { id: b, .. }) => a == b,
            _ => false,
        }
    }
}

impl Hash for Src {
    fn hash<H: Hasher>(&self, h: &mut H) {
        match self {
            Src::File { path, stamp } => {
                0u8.hash(h);
                path.hash(h);
                stamp.hash(h);
            }
            Src::Mem { id, .. } => {
                1u8.hash(h);
                id.hash(h);
            }
        }
    }
}

/// Die Ebene über der Grundfarbe (Farben deckend aufgelöst als `[r, g, b, a]` ohne Vormultiplikation).
#[derive(Clone, PartialEq, Hash, Debug)]
pub enum Layer {
    None,
    Color([u8; 4]),
    Image(Src),
    Gradient { radial: bool, angle: u16, center: [u8; 2], radius: u16, stops: Vec<(u8, [u8; 4])> },
    Pattern { name: PatternName, tile: Option<Src>, ink: [u8; 4], scale: u16, offset: [i32; 2] },
}

/// Alles, was ein Bild bestimmt (aufgelöst, ohne Verweise auf das Theme). Gleiche Angaben = gleiches Bild.
#[derive(Clone, PartialEq, Hash, Debug)]
pub struct BakeSpec {
    pub w: u32,
    pub h: u32,
    /// Bildpunkte je Punkt × 1000.
    pub ppp_milli: u32,
    pub base: [u8; 4],
    pub layer: Layer,
    pub mode: Mode,
    pub anchor: Anchor,
    pub margin: [u16; 4],
    pub opacity: u8,
    pub brightness: i8,
    pub blur: u8,
    pub overlay: Option<([u8; 4], u8)>,
}

impl BakeSpec {
    /// Kennung des Bildes.
    pub fn key(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.hash(&mut h);
        h.finish()
    }

    /// Kennung ohne Größe (gleiche Einstellungen, anderes Fenster).
    pub fn look_key(&self) -> u64 {
        let mut s = self.clone();
        s.w = 0;
        s.h = 0;
        s.key()
    }

    fn ppp(&self) -> f32 {
        (self.ppp_milli as f32 / 1000.0).max(0.1)
    }
}

/// Kennzahlen des Bildes vor Helligkeit und Überlagerung (über der Grundfarbe, deckend): Mittelwert, Mittel der hellsten
/// und der dunkelsten 10 %. Helligkeit und Überlagerung wirken auf jeden Punkt gleich, darum lassen sich die Werte danach
/// mit [`post`] ausrechnen (Design-Prüfung, Vorschläge).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Stats {
    pub mean: Color32,
    pub bright: Color32,
    pub dark: Color32,
}

impl Stats {
    /// Die drei Werte nach Helligkeit und Überlagerung von `spec`.
    pub fn after(&self, brightness: i8, overlay: Option<(Color32, u8)>) -> Stats {
        Stats { mean: post(self.mean, brightness, overlay), bright: post(self.bright, brightness, overlay), dark: post(self.dark, brightness, overlay) }
    }
}

/// Ergebnis: Bild in Fenstergröße (ohne Vormultiplikation) und Kennzahlen.
pub struct Baked {
    pub img: RgbaImage,
    pub stats: Stats,
}

fn rgba(c: Color32) -> [u8; 4] {
    c.to_srgba_unmultiplied()
}

/// Angaben für ein Bild der Größe `w × h` (physische Pixel, `ppp` Bildpunkte je Punkt). `src`: Quelle zu einem
/// Dateinamen des Hintergrunds (`None` = fehlt/ungültig → nur die Grundfarbe). `None` ohne Hintergrund.
pub fn spec_for(t: &Theme, w: u32, h: u32, ppp: f32, src: &dyn Fn(&str) -> Option<Src>) -> Option<BakeSpec> {
    let b: &Background = t.background.as_ref()?;
    let base = rgba(t.bg.to_opaque());
    let layer = match b.kind {
        Kind::Image => b.image.as_deref().and_then(src).map(Layer::Image).unwrap_or(Layer::None),
        Kind::Color => b.color.as_ref().map(|c| Layer::Color(rgba(spec_color(t, c)))).unwrap_or(Layer::None),
        Kind::Gradient => match &b.gradient {
            Some(g) => Layer::Gradient {
                radial: g.kind == GradKind::Radial,
                angle: g.angle % 360,
                center: g.center,
                radius: g.radius.max(1),
                stops: g.stops.iter().map(|s| (s.pos.min(100), rgba(spec_color(t, &s.color)))).collect(),
            },
            None => Layer::None,
        },
        Kind::Pattern => match &b.pattern {
            Some(p) => Layer::Pattern { name: p.name, tile: p.image.as_deref().and_then(src), ink: rgba(spec_color(t, &p.color)), scale: p.scale.clamp(25, 400), offset: p.offset },
            None => Layer::None,
        },
    };
    Some(BakeSpec {
        w: w.max(1),
        h: h.max(1),
        ppp_milli: (ppp * 1000.0).round().max(100.0) as u32,
        base,
        layer,
        mode: b.mode,
        anchor: b.anchor,
        margin: b.margin,
        opacity: b.opacity.min(100),
        brightness: b.brightness,
        blur: b.blur.min(40),
        overlay: b.overlay.as_ref().map(|o| (rgba(spec_color(t, &o.color)), o.opacity.min(100))),
    })
}

/// Überlagerung als Farbe mit Deckkraft (für [`post`]).
pub fn overlay_of(t: &Theme) -> Option<(Color32, u8)> {
    let o = t.background.as_ref()?.overlay.as_ref()?;
    Some((spec_color(t, &o.color), o.opacity))
}

// ---------------------------------------------------------------------------------------------------------------
// Bilder lesen

fn t(de: &'static str, en: &'static str) -> &'static str {
    crate::state::t(de, en)
}

/// Bild dekodieren (PNG, JPEG, WebP, BMP; animiert = erstes Bild) mit den Grenzen der Chat-Bilder.
fn decode(bytes: &[u8]) -> Result<(image::DynamicImage, image::ImageFormat, bool), String> {
    crate::chat::images::decode_formats(bytes, FORMATS, t("Kein unterstütztes Bild (PNG, JPEG, WebP oder BMP)", "Not a supported image (PNG, JPEG, WebP or BMP)"))
}

fn shrink(img: image::DynamicImage) -> image::DynamicImage {
    let (w, h) = (img.width(), img.height());
    let (nw, nh) = crate::chat::images::fit(w, h, MAX_SIDE);
    if (nw, nh) == (w, h) {
        return img;
    }
    let rough = if w.max(h) > 2 * MAX_SIDE { img.thumbnail(2 * MAX_SIDE, 2 * MAX_SIDE) } else { img };
    rough.resize_exact(nw, nh, image::imageops::FilterType::Triangle)
}

/// Ein für das Theme aufbereitetes Bild: Bytes der Kopie, Endung und das Bild selbst.
pub struct Prepared {
    pub bytes: Vec<u8>,
    pub ext: &'static str,
    pub img: Arc<RgbaImage>,
}

/// Bilddatei für das Theme aufbereiten: höchstens [`MAX_SIDE`] Pixel je Seite; PNG und JPEG, die schon passen, bleiben
/// unverändert, sonst PNG (mit Transparenz) bzw. JPEG mit Qualität 90. Das Original wird nie verändert.
pub fn prepare(bytes: Vec<u8>) -> Result<Prepared, String> {
    crate::chat::images::one_at_a_time(|| {
        let (img, fmt, turned) = decode(&bytes)?;
        let big = img.width().max(img.height()) > MAX_SIDE;
        let img = shrink(img);
        let rgba = img.to_rgba8();
        let alpha = rgba.pixels().any(|p| p.0[3] < 255);
        let (out, ext) = match fmt {
            image::ImageFormat::Png if !big && !turned => (bytes, "png"),
            image::ImageFormat::Jpeg if !big && !turned => (bytes, "jpg"),
            _ if alpha => (crate::chat::images::encode_png(&img)?, "png"),
            _ => (crate::chat::images::encode_jpeg(&crate::chat::images::flatten(&img), 90)?, "jpg"),
        };
        Ok(Prepared { bytes: out, ext, img: Arc::new(rgba) })
    })
}

/// Bilddatei lesen und aufbereiten (höchstens 64 MB).
pub fn prepare_file(path: &Path) -> Result<Prepared, String> {
    prepare(crate::chat::images::read_limited(path)?)
}

/// Freier Dateiname für ein Bild aus `original` (Name der gewählten Datei) mit Endung `ext`: nur Buchstaben, Ziffern,
/// `-` und `_`, höchstens 40 Zeichen, nicht in `taken` (ohne Groß-/Kleinschreibung) – sonst „name-2.ext“ usw.
pub fn file_name_for(original: &str, ext: &str, taken: &[String]) -> String {
    let stem = Path::new(original).file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let mut s = String::new();
    for c in stem.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            s.push(c.to_ascii_lowercase());
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
        if s.len() >= 40 {
            break;
        }
    }
    let s = s.trim_matches('-').to_string();
    let s = if s.is_empty() || !super::background::safe_file_name(&format!("{s}.{ext}")) { "background".to_string() } else { s };
    let used = |n: &str| taken.iter().any(|x| x.eq_ignore_ascii_case(n));
    let first = format!("{s}.{ext}");
    if !used(&first) {
        return first;
    }
    (2..).map(|i| format!("{s}-{i}.{ext}")).find(|n| !used(n)).unwrap_or(first)
}

/// Zuletzt gelesene Quellbilder (Datei, Stempel → Bild); höchstens drei.
fn source_cache() -> &'static Mutex<Vec<(PathBuf, (u64, u64), Arc<RgbaImage>)>> {
    static C: OnceLock<Mutex<Vec<(PathBuf, (u64, u64), Arc<RgbaImage>)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(Vec::new()))
}

/// Quellbild lesen (aus dem Zwischenspeicher, sonst von der Platte; höchstens [`MAX_SIDE`] je Seite).
pub fn load(src: &Src) -> Result<Arc<RgbaImage>, String> {
    match src {
        Src::Mem { img, .. } => Ok(img.clone()),
        Src::File { path, stamp } => {
            if let Some((_, _, img)) = source_cache().lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(p, s, _)| p == path && s == stamp) {
                return Ok(img.clone());
            }
            if !super::background::real_file(path) {
                return Err(t("Bild nicht gefunden", "Image not found").into());
            }
            let bytes = crate::chat::images::read_limited(path)?;
            let img = crate::chat::images::one_at_a_time(|| decode(&bytes).map(|(i, _, _)| Arc::new(shrink(i).to_rgba8())))?;
            let mut c = source_cache().lock().unwrap_or_else(|e| e.into_inner());
            c.insert(0, (path.clone(), *stamp, img.clone()));
            c.truncate(3);
            Ok(img)
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Berechnen

/// Geordnetes Rauschen (4 × 4) gegen Streifen in Verläufen: −0,47 … +0,47 Stufen.
const BAYER: [[f32; 4]; 4] = [[0.0, 8.0, 2.0, 10.0], [12.0, 4.0, 14.0, 6.0], [3.0, 11.0, 1.0, 9.0], [15.0, 7.0, 13.0, 5.0]];

fn dither(x: u32, y: u32) -> f32 {
    (BAYER[(y & 3) as usize][(x & 3) as usize] - 7.5) / 16.0
}

/// `c` (ohne Vormultiplikation) über den deckenden Punkt `p`.
fn blend(p: &mut [u8; 4], c: [u8; 4]) {
    let a = c[3] as u32;
    if a == 0 {
        return;
    }
    for i in 0..3 {
        p[i] = ((c[i] as u32 * a + p[i] as u32 * (255 - a) + 127) / 255) as u8;
    }
    p[3] = 255;
}

/// Rechteck in Pixeln (x0, y0, x1, y1), halboffen.
type R = (i64, i64, i64, i64);

fn content_rect(s: &BakeSpec) -> R {
    let ppp = s.ppp();
    let m = |v: u16| (v as f32 * ppp).round() as i64;
    let (w, h) = (s.w as i64, s.h as i64);
    let (x0, y0) = (m(s.margin[3]).min(w), m(s.margin[0]).min(h));
    let (x1, y1) = ((w - m(s.margin[1])).max(x0), (h - m(s.margin[2])).max(y0));
    (x0, y0, x1, y1)
}

/// Lage und Größe des Bildes `iw × ih` in der Fläche `r` (Pixel; kann über `r` hinausragen).
pub fn place(mode: Mode, anchor: Anchor, r: R, iw: u32, ih: u32, ppp: f32) -> (f64, f64, f64, f64) {
    let (cw, ch) = ((r.2 - r.0) as f64, (r.3 - r.1) as f64);
    let (iw, ih) = (iw.max(1) as f64, ih.max(1) as f64);
    let (dw, dh) = match mode {
        Mode::Fill => {
            let s = (cw / iw).max(ch / ih);
            (iw * s, ih * s)
        }
        Mode::Fit => {
            let s = (cw / iw).min(ch / ih);
            (iw * s, ih * s)
        }
        Mode::Stretch => (cw, ch),
        Mode::Original => (iw * ppp as f64, ih * ppp as f64),
    };
    let (gx, gy) = anchor.grid();
    let x = r.0 as f64 + (cw - dw) * gx as f64 / 2.0;
    let y = r.1 as f64 + (ch - dh) * gy as f64 / 2.0;
    (x, y, dw, dh)
}

fn draw_image(out: &mut RgbaImage, s: &BakeSpec, r: R, src: &RgbaImage) {
    let (x, y, dw, dh) = place(s.mode, s.anchor, r, src.width(), src.height(), s.ppp());
    if dw < 1.0 || dh < 1.0 {
        return;
    }
    // Sichtbarer Teil (in der Fläche) und der passende Ausschnitt des Quellbilds.
    let vx0 = (x.max(r.0 as f64)).floor() as i64;
    let vy0 = (y.max(r.1 as f64)).floor() as i64;
    let vx1 = ((x + dw).min(r.2 as f64)).ceil() as i64;
    let vy1 = ((y + dh).min(r.3 as f64)).ceil() as i64;
    if vx1 <= vx0 || vy1 <= vy0 {
        return;
    }
    let (sx, sy) = (src.width() as f64 / dw, src.height() as f64 / dh);
    let cx0 = (((vx0 as f64 - x) * sx).floor().max(0.0) as u32).min(src.width() - 1);
    let cy0 = (((vy0 as f64 - y) * sy).floor().max(0.0) as u32).min(src.height() - 1);
    let cx1 = (((vx1 as f64 - x) * sx).ceil() as u32).clamp(cx0 + 1, src.width());
    let cy1 = (((vy1 as f64 - y) * sy).ceil() as u32).clamp(cy0 + 1, src.height());
    let crop = image::imageops::crop_imm(src, cx0, cy0, cx1 - cx0, cy1 - cy0).to_image();
    let (vw, vh) = ((vx1 - vx0) as u32, (vy1 - vy0) as u32);
    let scaled = if (crop.width(), crop.height()) == (vw, vh) { crop } else { image::imageops::resize(&crop, vw, vh, image::imageops::FilterType::Triangle) };
    for (px, py, p) in scaled.enumerate_pixels() {
        let (ox, oy) = (vx0 + px as i64, vy0 + py as i64);
        if ox < 0 || oy < 0 || ox >= out.width() as i64 || oy >= out.height() as i64 {
            continue;
        }
        blend(&mut out.get_pixel_mut(ox as u32, oy as u32).0, p.0);
    }
}

fn lerp_stops(stops: &[(u8, [u8; 4])], t: f32) -> [f32; 4] {
    let pick = |c: [u8; 4]| [c[0] as f32, c[1] as f32, c[2] as f32, c[3] as f32];
    let Some(first) = stops.first() else { return [0.0; 4] };
    if t <= first.0 as f32 / 100.0 {
        return pick(first.1);
    }
    for w in stops.windows(2) {
        let (a, b) = (w[0].0 as f32 / 100.0, w[1].0 as f32 / 100.0);
        if t <= b {
            let k = if b > a { (t - a) / (b - a) } else { 1.0 };
            let (ca, cb) = (pick(w[0].1), pick(w[1].1));
            return [0, 1, 2, 3].map(|i| ca[i] + (cb[i] - ca[i]) * k);
        }
    }
    pick(stops.last().map(|s| s.1).unwrap_or([0; 4]))
}

fn draw_gradient(out: &mut RgbaImage, r: R, radial: bool, angle: u16, center: [u8; 2], radius: u16, stops: &[(u8, [u8; 4])]) {
    let (cw, ch) = ((r.2 - r.0) as f32, (r.3 - r.1) as f32);
    if cw <= 0.0 || ch <= 0.0 || stops.is_empty() {
        return;
    }
    let th = (angle as f32).to_radians();
    let (dx, dy) = (th.sin(), th.cos());
    let half = (cw / 2.0 * dx.abs() + ch / 2.0 * dy.abs()).max(1.0);
    let (ccx, ccy) = (r.0 as f32 + cw * center[0] as f32 / 100.0, r.1 as f32 + ch * center[1] as f32 / 100.0);
    let rad = ((cw * cw + ch * ch).sqrt() / 2.0 * radius as f32 / 100.0).max(1.0);
    let (mx, my) = (r.0 as f32 + cw / 2.0, r.1 as f32 + ch / 2.0);
    for y in r.1.max(0)..r.3.min(out.height() as i64) {
        for x in r.0.max(0)..r.2.min(out.width() as i64) {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let t = if radial {
                ((fx - ccx).powi(2) + (fy - ccy).powi(2)).sqrt() / rad
            } else {
                (((fx - mx) * dx + (fy - my) * dy) / half + 1.0) / 2.0
            };
            let c = lerp_stops(stops, t.clamp(0.0, 1.0));
            let d = dither(x as u32, y as u32);
            let q = |v: f32| (v + d).round().clamp(0.0, 255.0) as u8;
            let col = [q(c[0]), q(c[1]), q(c[2]), c[3].round().clamp(0.0, 255.0) as u8];
            blend(&mut out.get_pixel_mut(x as u32, y as u32).0, col);
        }
    }
}

/// Einfacher Hash für das Rauschen (gleiches Bild bei gleichen Angaben).
fn noise(x: u32, y: u32) -> u32 {
    let mut h = x.wrapping_mul(0x9e37_79b1) ^ y.wrapping_mul(0x85eb_ca77);
    h ^= h >> 15;
    h = h.wrapping_mul(0xc2b2_ae3d);
    h ^ (h >> 13)
}

/// Kachel eines eingebauten Musters: Deckkraft der Musterfarbe je Pixel (0–1). `u` = Pixel je Musterpunkt.
fn builtin_tile(name: PatternName, u: f32) -> (u32, u32, Vec<f32>) {
    let px = |v: f32| (v * u).round().max(2.0) as u32;
    let (tw, th) = match name {
        PatternName::Dots => (px(16.0), px(16.0)),
        PatternName::Lines => (px(8.0), px(8.0)),
        PatternName::Grid => (px(24.0), px(24.0)),
        PatternName::Checks => (px(32.0), px(32.0)),
        PatternName::Diagonal => (px(12.0), px(12.0)),
        PatternName::Noise => (128, 128),
    };
    let line = (u).max(1.0);
    let mut a = vec![0.0f32; (tw * th) as usize];
    for y in 0..th {
        for x in 0..tw {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let v = match name {
                PatternName::Dots => {
                    let (cx, cy) = (tw as f32 / 2.0, th as f32 / 2.0);
                    let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
                    (1.6 * u - d + 0.5).clamp(0.0, 1.0)
                }
                PatternName::Lines => (line - fy).clamp(0.0, 1.0),
                PatternName::Grid => (line - fx).clamp(0.0, 1.0).max((line - fy).clamp(0.0, 1.0)),
                PatternName::Checks => {
                    if (x < tw / 2) == (y < th / 2) {
                        1.0
                    } else {
                        0.0
                    }
                }
                PatternName::Diagonal => {
                    let d = (fx + fy).rem_euclid(tw as f32);
                    let dist = d.min(tw as f32 - d) / std::f32::consts::SQRT_2;
                    (line * 0.75 - dist + 0.5).clamp(0.0, 1.0)
                }
                PatternName::Noise => (noise(x, y) & 0xff) as f32 / 255.0,
            };
            a[(y * tw + x) as usize] = v;
        }
    }
    (tw, th, a)
}

fn draw_pattern(out: &mut RgbaImage, s: &BakeSpec, r: R, name: PatternName, tile: Option<&RgbaImage>, ink: [u8; 4], scale: u16, offset: [i32; 2]) {
    let ppp = s.ppp();
    let k = scale as f32 / 100.0;
    let (ox, oy) = ((offset[0] as f32 * ppp).round() as i64, (offset[1] as f32 * ppp).round() as i64);
    match tile {
        Some(img) => {
            let (tw, th) = (((img.width() as f32 * k * ppp).round() as u32).clamp(1, 4096), ((img.height() as f32 * k * ppp).round() as u32).clamp(1, 4096));
            let t = if (tw, th) == (img.width(), img.height()) { img.clone() } else { image::imageops::resize(img, tw, th, image::imageops::FilterType::Triangle) };
            for y in r.1.max(0)..r.3.min(out.height() as i64) {
                for x in r.0.max(0)..r.2.min(out.width() as i64) {
                    let (tx, ty) = ((x - r.0 - ox).rem_euclid(tw as i64) as u32, (y - r.1 - oy).rem_euclid(th as i64) as u32);
                    blend(&mut out.get_pixel_mut(x as u32, y as u32).0, t.get_pixel(tx, ty).0);
                }
            }
        }
        None => {
            let (tw, th, a) = builtin_tile(name, k * ppp);
            for y in r.1.max(0)..r.3.min(out.height() as i64) {
                for x in r.0.max(0)..r.2.min(out.width() as i64) {
                    let (tx, ty) = ((x - r.0 - ox).rem_euclid(tw as i64) as u32, (y - r.1 - oy).rem_euclid(th as i64) as u32);
                    let v = a[(ty * tw + tx) as usize];
                    if v > 0.0 {
                        let c = [ink[0], ink[1], ink[2], (ink[3] as f32 * v).round() as u8];
                        blend(&mut out.get_pixel_mut(x as u32, y as u32).0, c);
                    }
                }
            }
        }
    }
}

/// Kastenunschärfe in einer Richtung (gleitende Summe, Rand wiederholt).
fn box_pass(src: &[u8], dst: &mut [u8], w: usize, h: usize, rad: usize, horizontal: bool) {
    let (n, lines) = if horizontal { (w, h) } else { (h, w) };
    let idx = |line: usize, i: usize| if horizontal { (line * w + i) * 4 } else { (i * w + line) * 4 };
    let div = (2 * rad + 1) as u32;
    for line in 0..lines {
        for c in 0..3 {
            let at = |i: isize| src[idx(line, i.clamp(0, n as isize - 1) as usize) + c] as u32;
            let mut sum: u32 = (-(rad as isize)..=rad as isize).map(at).sum();
            for i in 0..n {
                dst[idx(line, i) + c] = ((sum + div / 2) / div) as u8;
                sum = sum + at(i as isize + rad as isize + 1) - at(i as isize - rad as isize);
            }
        }
        for i in 0..n {
            dst[idx(line, i) + 3] = src[idx(line, i) + 3];
        }
    }
}

/// Unschärfe mit Radius `r` Pixel (drei Kastenläufe ≈ Gauß).
pub fn blur(img: &mut RgbaImage, r: f32) {
    if r < 0.5 {
        return;
    }
    let (w, h) = (img.width() as usize, img.height() as usize);
    let rad = ((r / 3.0).round() as usize).max(1);
    let mut tmp = vec![0u8; w * h * 4];
    for _ in 0..3 {
        box_pass(img.as_raw(), &mut tmp, w, h, rad, true);
        let buf: &mut [u8] = img;
        box_pass(&tmp, buf, w, h, rad, false);
    }
}

/// Kennzahlen eines deckenden Bildes (Stichprobe von höchstens etwa 65 000 Punkten).
pub fn stats_of(img: &RgbaImage) -> Stats {
    let (w, h) = (img.width().max(1), img.height().max(1));
    let step = ((w as u64 * h as u64 / 65_000) as f64).sqrt().floor().max(1.0) as u32;
    let mut px: Vec<(f64, [u8; 3])> = Vec::new();
    let mut y = 0;
    while y < h {
        let mut x = 0;
        while x < w {
            let p = img.get_pixel(x, y).0;
            px.push((luminance(Color32::from_rgb(p[0], p[1], p[2])), [p[0], p[1], p[2]]));
            x += step;
        }
        y += step;
    }
    let avg = |s: &[(f64, [u8; 3])]| {
        let n = s.len().max(1) as f64;
        let m = |i: usize| (s.iter().map(|p| p.1[i] as f64).sum::<f64>() / n).round().clamp(0.0, 255.0) as u8;
        Color32::from_rgb(m(0), m(1), m(2))
    };
    let mean = avg(&px);
    px.sort_by(|a, b| a.0.total_cmp(&b.0));
    let tenth = (px.len() / 10).max(1).min(px.len());
    Stats { mean, bright: avg(&px[px.len() - tenth..]), dark: avg(&px[..tenth]) }
}

/// Bild berechnen.
pub fn bake(s: &BakeSpec) -> Result<Baked, String> {
    let (w, h) = (s.w.clamp(1, 16384), s.h.clamp(1, 16384));
    let mut out = RgbaImage::from_pixel(w, h, image::Rgba([s.base[0], s.base[1], s.base[2], 255]));
    let r = content_rect(s);
    // Fehlt ein Bild oder ist es nicht lesbar, bleibt die Grundfarbe (mit Filtern).
    if r.2 > r.0 && r.3 > r.1 {
        match &s.layer {
            Layer::None => {}
            Layer::Color(c) => {
                for y in r.1..r.3 {
                    for x in r.0..r.2 {
                        blend(&mut out.get_pixel_mut(x as u32, y as u32).0, *c);
                    }
                }
            }
            Layer::Image(src) => {
                if let Ok(img) = load(src) {
                    draw_image(&mut out, s, r, &img);
                }
            }
            Layer::Gradient { radial, angle, center, radius, stops } => draw_gradient(&mut out, r, *radial, *angle, *center, *radius, stops),
            Layer::Pattern { name, tile, ink, scale, offset } => {
                if let Ok(t) = tile.as_ref().map(load).transpose() {
                    draw_pattern(&mut out, s, r, *name, t.as_deref(), *ink, *scale, *offset);
                }
            }
        }
    }
    blur(&mut out, s.blur as f32 * s.ppp());
    let stats = stats_of(&out);
    // Helligkeit und Überlagerung je Kanalwert über eine Tabelle (dieselbe Rechnung wie `post`).
    let overlay = s.overlay.map(|(c, op)| (Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]), op));
    if s.brightness != 0 || overlay.is_some() {
        let mut lut = [[0u8; 256]; 3];
        for v in 0..=255u8 {
            let c = post(Color32::from_rgb(v, v, v), s.brightness, overlay);
            let (cr, cg, cb) = (c.r(), c.g(), c.b());
            // `post` mischt je Kanal unabhängig: der Wert eines Kanals hängt nur von seinem Eingang ab.
            lut[0][v as usize] = cr;
            lut[1][v as usize] = cg;
            lut[2][v as usize] = cb;
        }
        for p in out.pixels_mut() {
            for i in 0..3 {
                p.0[i] = lut[i][p.0[i] as usize];
            }
        }
    }
    let a = (s.opacity as f32 * 2.55).round() as u8;
    if a < 255 {
        for p in out.pixels_mut() {
            p.0[3] = a;
        }
    }
    Ok(Baked { img: out, stats })
}

// ---------------------------------------------------------------------------------------------------------------
// Hintergrund-Faden

/// Ergebnis eines Auftrags (holt der Auftraggeber ab).
pub type Slot = Arc<Mutex<Option<Result<Arc<Baked>, String>>>>;

struct Queue {
    jobs: Mutex<Vec<(u64, BakeSpec, Slot)>>,
    cv: Condvar,
}

fn queue() -> &'static Queue {
    static Q: OnceLock<Queue> = OnceLock::new();
    Q.get_or_init(|| {
        let _ = std::thread::Builder::new().name("paneldeck-bg-bake".into()).spawn(worker);
        Queue { jobs: Mutex::new(Vec::new()), cv: Condvar::new() }
    })
}

fn worker() {
    let q = queue();
    loop {
        let job = {
            let mut jobs = q.jobs.lock().unwrap_or_else(|e| e.into_inner());
            while jobs.is_empty() {
                jobs = q.cv.wait(jobs).unwrap_or_else(|e| e.into_inner());
            }
            jobs.remove(0)
        };
        let (_, spec, slot) = job;
        let r = std::panic::catch_unwind(|| bake(&spec)).unwrap_or_else(|_| Err("bake failed".into()));
        *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(r.map(Arc::new));
    }
}

/// Auftrag an den Hintergrund-Faden. Ein noch wartender Auftrag desselben Auftraggebers (`who`) wird ersetzt – beim
/// Ziehen eines Reglers rechnet nur der letzte Stand.
pub fn submit(who: u64, spec: BakeSpec) -> Slot {
    let slot: Slot = Arc::new(Mutex::new(None));
    let q = queue();
    let mut jobs = q.jobs.lock().unwrap_or_else(|e| e.into_inner());
    jobs.retain(|(w, _, _)| *w != who);
    jobs.push((who, spec, slot.clone()));
    q.cv.notify_one();
    slot
}

/// Ergebnis abholen (`None`, solange es rechnet).
pub fn take(slot: &Slot) -> Option<Result<Arc<Baked>, String>> {
    slot.lock().unwrap_or_else(|e| e.into_inner()).take()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::background::{Area, Overlay, Pattern, Stop};
    use crate::theme::tokens::Spec;

    fn theme_with(b: Background) -> Theme {
        let mut t = crate::theme::all()[0].clone();
        t.background = Some(b);
        t
    }

    fn checker(w: u32, h: u32) -> Arc<RgbaImage> {
        Arc::new(RgbaImage::from_fn(w, h, |x, y| if (x / 10 + y / 10) % 2 == 0 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 255, 255]) }))
    }

    fn mem(img: Arc<RgbaImage>) -> Src {
        Src::Mem { id: 7, img }
    }

    fn px(b: &Baked, x: u32, y: u32) -> [u8; 4] {
        b.img.get_pixel(x, y).0
    }

    fn spec_of(b: Background, w: u32, h: u32, img: Option<Arc<RgbaImage>>) -> BakeSpec {
        let t = theme_with(b);
        spec_for(&t, w, h, 1.0, &|_| img.clone().map(mem)).unwrap()
    }

    #[test]
    fn modes_and_anchors_place_the_image() {
        let r = (0, 0, 200, 100);
        // Füllen: 100×100 → 200×200, mittig abgeschnitten.
        assert_eq!(place(Mode::Fill, Anchor::Center, r, 100, 100, 1.0), (0.0, -50.0, 200.0, 200.0));
        assert_eq!(place(Mode::Fill, Anchor::Top, r, 100, 100, 1.0), (0.0, 0.0, 200.0, 200.0));
        assert_eq!(place(Mode::Fill, Anchor::Bottom, r, 100, 100, 1.0), (0.0, -100.0, 200.0, 200.0));
        // Einpassen: 100×100 → 100×100 mit Rand in der Grundfarbe.
        assert_eq!(place(Mode::Fit, Anchor::Center, r, 100, 100, 1.0), (50.0, 0.0, 100.0, 100.0));
        assert_eq!(place(Mode::Fit, Anchor::Right, r, 100, 100, 1.0), (100.0, 0.0, 100.0, 100.0));
        assert_eq!(place(Mode::Stretch, Anchor::BottomRight, r, 10, 10, 1.0), (0.0, 0.0, 200.0, 100.0));
        // Original: 1:1 je Bildschirmpunkt.
        assert_eq!(place(Mode::Original, Anchor::BottomRight, r, 50, 20, 2.0), (100.0, 60.0, 100.0, 40.0));
        assert_eq!(place(Mode::Original, Anchor::TopLeft, r, 50, 20, 1.0), (0.0, 0.0, 50.0, 20.0));
        // Im Bild: Fit lässt links und rechts die Grundfarbe.
        let mut b = Background::new(Kind::Image);
        b.image = Some("x.png".into());
        b.mode = Mode::Fit;
        let out = bake(&spec_of(b.clone(), 200, 100, Some(checker(100, 100)))).unwrap();
        let base = crate::theme::all()[0].bg.to_opaque().to_srgba_unmultiplied();
        assert_eq!(px(&out, 10, 50), base);
        assert_eq!(px(&out, 190, 50), base);
        assert_ne!(px(&out, 100, 50), base);
        // Original oben links: rechts unten bleibt die Grundfarbe.
        b.mode = Mode::Original;
        b.anchor = Anchor::TopLeft;
        let out = bake(&spec_of(b, 200, 100, Some(checker(40, 40)))).unwrap();
        assert_eq!(px(&out, 5, 5), [255, 0, 0, 255]);
        assert_eq!(px(&out, 15, 5), [0, 0, 255, 255]);
        assert_eq!(px(&out, 150, 80), base);
    }

    #[test]
    fn margins_keep_the_base_colour() {
        let mut b = Background::new(Kind::Color);
        b.color = Some(Spec::Hex(Color32::from_rgb(10, 200, 30)));
        b.margin = [10, 20, 30, 40];
        let out = bake(&spec_of(b, 200, 100, None)).unwrap();
        let base = crate::theme::all()[0].bg.to_opaque().to_srgba_unmultiplied();
        assert_eq!(px(&out, 100, 5), base, "oben");
        assert_eq!(px(&out, 185, 50), base, "rechts");
        assert_eq!(px(&out, 100, 75), base, "unten");
        assert_eq!(px(&out, 35, 50), base, "links");
        assert_eq!(px(&out, 100, 50), [10, 200, 30, 255]);
    }

    #[test]
    fn gradient_runs_from_stop_to_stop_without_banding() {
        let mut b = Background::new(Kind::Gradient);
        let g = b.gradient.as_mut().unwrap();
        g.angle = 90;
        g.stops = vec![Stop { pos: 0, color: Spec::Hex(Color32::BLACK) }, Stop { pos: 100, color: Spec::Hex(Color32::WHITE) }];
        let out = bake(&spec_of(b.clone(), 256, 8, None)).unwrap();
        assert!(px(&out, 0, 0)[0] <= 2 && px(&out, 255, 0)[0] >= 253, "links schwarz, rechts weiß");
        let mid = px(&out, 128, 0)[0];
        assert!((120..=136).contains(&mid), "{mid}");
        // Dithering: benachbarte Zeilen an derselben Stelle nicht alle gleich (keine harten Stufen).
        let col: Vec<u8> = (0..4).map(|y| px(&out, 100, y)[0]).collect();
        assert!(col.iter().any(|v| *v != col[0]) || col[0] % 2 == 0, "{col:?}");
        // Winkel 0 = von oben nach unten.
        b.gradient.as_mut().unwrap().angle = 0;
        let out = bake(&spec_of(b.clone(), 8, 256, None)).unwrap();
        assert!(px(&out, 0, 0)[0] <= 2 && px(&out, 0, 255)[0] >= 253);
        // Radial: Mitte = erster Stopp.
        let g = b.gradient.as_mut().unwrap();
        g.kind = GradKind::Radial;
        g.center = [25, 50];
        let out = bake(&spec_of(b, 200, 100, None)).unwrap();
        assert!(px(&out, 50, 50)[0] <= 3 && px(&out, 199, 0)[0] > 150);
    }

    #[test]
    fn overlay_and_brightness_are_applied_and_stats_predict_them() {
        let mut b = Background::new(Kind::Color);
        b.color = Some(Spec::Hex(Color32::from_rgb(200, 200, 200)));
        b.brightness = -50;
        let out = bake(&spec_of(b.clone(), 20, 20, None)).unwrap();
        assert_eq!(px(&out, 5, 5), [100, 100, 100, 255]);
        assert_eq!(out.stats.mean, Color32::from_rgb(200, 200, 200), "Kennzahlen vor den Filtern");
        assert_eq!(out.stats.after(-50, None).mean, Color32::from_rgb(100, 100, 100));
        b.brightness = 0;
        b.overlay = Some(Overlay { color: Spec::Hex(Color32::BLACK), opacity: 50 });
        let out = bake(&spec_of(b.clone(), 20, 20, None)).unwrap();
        assert_eq!(px(&out, 5, 5), [100, 100, 100, 255]);
        let t = theme_with(b.clone());
        assert_eq!(out.stats.after(0, overlay_of(&t)).mean, Color32::from_rgb(100, 100, 100));
        // Deckkraft des Hintergrunds.
        b.opacity = 40;
        let out = bake(&spec_of(b, 20, 20, None)).unwrap();
        assert_eq!(px(&out, 5, 5)[3], 102);
    }

    #[test]
    fn blur_softens_edges_and_keeps_flat_areas() {
        let mut img = RgbaImage::from_fn(40, 10, |x, _| if x < 20 { image::Rgba([0, 0, 0, 255]) } else { image::Rgba([255, 255, 255, 255]) });
        blur(&mut img, 9.0);
        assert_eq!(img.get_pixel(0, 5).0[0], 0, "flach bleibt");
        assert_eq!(img.get_pixel(39, 5).0[0], 255);
        let e = img.get_pixel(20, 5).0[0];
        assert!((60..200).contains(&e), "Kante weich: {e}");
        let mut b = Background::new(Kind::Image);
        b.image = Some("x.png".into());
        b.blur = 10;
        let out = bake(&spec_of(b, 100, 100, Some(checker(100, 100)))).unwrap();
        let p = px(&out, 50, 50);
        assert!(p[0] > 40 && p[2] > 40, "Rot und Blau gemischt: {p:?}");
    }

    #[test]
    fn builtin_patterns_use_the_theme_colours() {
        for name in PatternName::ALL {
            let mut b = Background::new(Kind::Pattern);
            b.pattern = Some(Pattern { name, image: None, color: Spec::Hex(Color32::WHITE), scale: 100, offset: [0, 0] });
            let out = bake(&spec_of(b, 96, 96, None)).unwrap();
            let base = crate::theme::all()[0].bg.to_opaque().to_srgba_unmultiplied();
            let inked = out.img.pixels().filter(|p| p.0 != base).count();
            assert!(inked > 0 && inked < 96 * 96, "{name:?}: {inked}");
            assert!(out.img.pixels().all(|p| p.0[0] >= base[0]), "{name:?}: nur heller als der Grund");
        }
        // Eigene Kachel mit Größe und Versatz.
        let mut b = Background::new(Kind::Pattern);
        b.pattern = Some(Pattern { name: PatternName::Dots, image: Some("t.png".into()), color: Spec::Hex(Color32::WHITE), scale: 100, offset: [5, 0] });
        let out = bake(&spec_of(b.clone(), 40, 40, Some(checker(20, 20)))).unwrap();
        assert_eq!(px(&out, 5, 0), [255, 0, 0, 255]);
        assert_eq!(px(&out, 0, 0), [0, 0, 255, 255], "Versatz 5 bei 20 px großer Kachel");
        assert_eq!(px(&out, 25, 0), [255, 0, 0, 255], "wiederholt");
        // Größe 200 %: Kachel 40 px, Felder 20 px.
        b.pattern.as_mut().unwrap().scale = 200;
        b.pattern.as_mut().unwrap().offset = [0, 0];
        let out = bake(&spec_of(b, 80, 80, Some(checker(20, 20)))).unwrap();
        assert_eq!((px(&out, 2, 2), px(&out, 30, 2)), ([255, 0, 0, 255], [0, 0, 255, 255]));
    }

    #[test]
    fn stats_find_the_bright_and_dark_tenth() {
        let img = RgbaImage::from_fn(100, 100, |x, _| if x < 10 { image::Rgba([250, 250, 250, 255]) } else { image::Rgba([20, 20, 20, 255]) });
        let s = stats_of(&img);
        assert_eq!(s.bright, Color32::from_rgb(250, 250, 250));
        assert_eq!(s.dark, Color32::from_rgb(20, 20, 20));
        assert_eq!(s.mean, Color32::from_rgb(43, 43, 43));
    }

    #[test]
    fn missing_image_falls_back_to_the_base_colour() {
        let mut b = Background::new(Kind::Image);
        b.image = Some("gone.png".into());
        let t = theme_with(b);
        let s = spec_for(&t, 30, 20, 1.0, &|_| None).unwrap();
        assert_eq!(s.layer, Layer::None);
        let out = bake(&s).unwrap();
        assert!(out.img.pixels().all(|p| p.0 == t.bg.to_opaque().to_srgba_unmultiplied()));
        // Datei weg nach dem Planen: ebenfalls Grundfarbe, kein Fehler.
        let s = BakeSpec { layer: Layer::Image(Src::File { path: std::env::temp_dir().join("paneldeck-nope-bg.png"), stamp: (0, 0) }), ..s };
        assert!(bake(&s).is_ok());
        assert!(spec_for(&crate::theme::all()[0], 10, 10, 1.0, &|_| None).is_none(), "ohne Hintergrund");
    }

    #[test]
    fn keys_change_with_settings_and_size() {
        let b = Background::new(Kind::Gradient);
        let a = spec_of(b.clone(), 100, 50, None);
        let c = spec_of(b.clone(), 120, 50, None);
        assert_ne!(a.key(), c.key());
        assert_eq!(a.look_key(), c.look_key(), "nur die Größe anders");
        let mut b2 = b;
        b2.blur = 3;
        assert_ne!(spec_of(b2, 100, 50, None).look_key(), a.look_key());
        let _ = Area::ALL;
    }

    #[test]
    fn prepare_scales_down_and_keeps_small_files() {
        let small = image::DynamicImage::ImageRgba8(RgbaImage::from_pixel(50, 30, image::Rgba([1, 2, 3, 255])));
        let png = crate::chat::images::encode_png(&small).unwrap();
        let p = prepare(png.clone()).unwrap();
        assert_eq!((p.ext, p.bytes.len(), p.img.width()), ("png", png.len(), 50), "unverändert");
        // Zu groß: auf 3840 verkleinert, ohne Transparenz als JPEG.
        let big = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4000, 1000, image::Rgb([9, 9, 9])));
        let p = prepare(crate::chat::images::encode_png(&big).unwrap()).unwrap();
        assert_eq!((p.ext, p.img.width(), p.img.height()), ("jpg", 3840, 960));
        // Mit Transparenz: PNG. BMP wird kodiert.
        let t = image::DynamicImage::ImageRgba8(RgbaImage::from_pixel(4000, 10, image::Rgba([1, 2, 3, 100])));
        assert_eq!(prepare(crate::chat::images::encode_png(&t).unwrap()).unwrap().ext, "png");
        let mut bmp = Vec::new();
        small.to_rgb8().write_to(&mut std::io::Cursor::new(&mut bmp), image::ImageFormat::Bmp).unwrap();
        assert_eq!(prepare(bmp).unwrap().ext, "jpg");
        // WebP (verlustfrei, vom Kodierer des image-Crates).
        let mut webp = Vec::new();
        small.write_to(&mut std::io::Cursor::new(&mut webp), image::ImageFormat::WebP).unwrap();
        assert_eq!(prepare(webp).unwrap().img.width(), 50);
        // Kein Bild bzw. SVG: abgelehnt.
        assert!(prepare(b"<svg xmlns='http://www.w3.org/2000/svg'/>".to_vec()).is_err());
        assert!(prepare(b"GIF89a....".to_vec()).is_err());
    }

    #[test]
    fn file_names_for_imported_images_are_safe_and_unique() {
        assert_eq!(file_name_for("C:\\Users\\x\\Mein Foto (1).JPG", "jpg", &[]), "mein-foto-1.jpg");
        assert_eq!(file_name_for("con.png", "png", &[]), "background.png");
        assert_eq!(file_name_for("???.png", "png", &[]), "background.png");
        assert_eq!(file_name_for("a.png", "png", &["A.png".into(), "a-2.png".into()]), "a-3.png");
        assert!(crate::theme::background::safe_file_name(&file_name_for(&"x".repeat(300), "webp", &[])));
    }

    #[test]
    fn the_worker_returns_results_and_replaces_waiting_jobs() {
        let mut b = Background::new(Kind::Color);
        b.color = Some(Spec::Hex(Color32::from_rgb(1, 2, 3)));
        let s = spec_of(b, 8, 8, None);
        let slot = submit(9_999, s);
        let t0 = std::time::Instant::now();
        let r = loop {
            if let Some(r) = take(&slot) {
                break r;
            }
            assert!(t0.elapsed() < std::time::Duration::from_secs(20), "Faden antwortet");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(r.unwrap().img.get_pixel(1, 1).0, [1, 2, 3, 255]);
    }
}
