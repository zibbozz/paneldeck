//! Farbige Emojis: Bitmaps (PNG) aus der eingebetteten Noto Color Emoji (CBDT) als Texturen.
//! Einzelne Zeichen werden unterstützt; zusammengesetzte Sequenzen (ZWJ, Flaggen) erscheinen
//! als einzelne Emojis hintereinander.

use eframe::egui::{self, ColorImage, TextureHandle, TextureOptions};
use std::cell::RefCell;
use std::collections::HashMap;

static FONT: &[u8] = include_bytes!("../assets/fonts/NotoColorEmoji.ttf");

struct Atlas {
    face: Option<ttf_parser::Face<'static>>,
    textures: HashMap<(u16, u32), Option<TextureHandle>>,
    shaped: HashMap<Vec<char>, Option<ttf_parser::GlyphId>>,
}

thread_local! {
    static ATLAS: RefCell<Atlas> = RefCell::new(Atlas {
        face: ttf_parser::Face::parse(FONT, 0).ok(),
        textures: HashMap::new(),
        shaped: HashMap::new(),
    });
}

/// Soll das Zeichen als Farb-Emoji gezeichnet werden?
pub fn wants_color(c: char, wide: bool, vs16: bool) -> bool {
    let u = c as u32;
    if u < 0x80 {
        return false;
    }
    let in_emoji_block = (0x1F000..=0x1FAFF).contains(&u);
    let has_glyph = ATLAS.with(|a| a.borrow().face.as_ref().is_some_and(|f| f.glyph_index(c).is_some()));
    has_glyph && (in_emoji_block || vs16 || (wide && (0x2190..=0x2BFF).contains(&u)))
}

pub fn is_regional(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

pub fn is_modifier(c: char) -> bool {
    ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
}

type Gid = ttf_parser::GlyphId;

/// Bestmögliche Ligatur (längste) am Anfang von `seq`: (Anzahl Eingabeglyphen, Ergebnisglyph).
fn best_ligature(face: &ttf_parser::Face<'static>, seq: &[Gid]) -> Option<(usize, Gid)> {
    use ttf_parser::gsub::SubstitutionSubtable;
    let gsub = face.tables().gsub.as_ref()?;
    let mut best: Option<(usize, Gid)> = None;
    for lookup in gsub.lookups {
        for st in lookup.subtables.into_iter() {
            let SubstitutionSubtable::Ligature(l) = st else { continue };
            let Some(idx) = l.coverage.get(seq[0]) else { continue };
            let Some(set) = l.ligature_sets.get(idx) else { continue };
            for lig in set {
                let n = lig.components.len() as usize + 1;
                if n <= seq.len() && lig.components.into_iter().zip(seq[1..].iter()).all(|(a, b)| a == *b) && best.is_none_or(|(bn, _)| n > bn) {
                    best = Some((n, lig.glyph));
                }
            }
        }
    }
    best
}

fn ligate(face: &ttf_parser::Face<'static>, mut g: Vec<Gid>) -> Vec<Gid> {
    for _ in 0..8 {
        let (mut out, mut i, mut changed) = (Vec::with_capacity(g.len()), 0, false);
        while i < g.len() {
            match best_ligature(face, &g[i..]) {
                Some((n, glyph)) => {
                    out.push(glyph);
                    i += n;
                    changed = true;
                }
                None => {
                    out.push(g[i]);
                    i += 1;
                }
            }
        }
        g = out;
        if !changed {
            break;
        }
    }
    g
}

/// Folge von Codepunkten (ZWJ-Sequenz, Flagge, Hautfarbe, Keycap) zu einem Emoji-Glyph zusammensetzen.
pub fn shape(cps: &[char]) -> Option<Gid> {
    ATLAS.with(|a| {
        let mut a = a.borrow_mut();
        if let Some(r) = a.shaped.get(cps) {
            return *r;
        }
        let res = (|| {
            let face = a.face.as_ref()?;
            let variants: [Vec<char>; 2] = [cps.to_vec(), cps.iter().copied().filter(|c| !matches!(c, '\u{fe0e}' | '\u{fe0f}')).collect()];
            for v in variants {
                let glyphs: Option<Vec<Gid>> = v.iter().map(|c| face.glyph_index(*c)).collect();
                let Some(glyphs) = glyphs else { continue };
                let out = if glyphs.len() == 1 { glyphs } else { ligate(face, glyphs) };
                if out.len() == 1 {
                    return Some(out[0]);
                }
            }
            None
        })();
        a.shaped.insert(cps.to_vec(), res);
        res
    })
}

/// Textur für das Glyph `gid` in ca. `px` Pixel Kantenlänge (wird gecacht).
pub fn texture_gid(ctx: &egui::Context, gid: Gid, px: u32) -> Option<(TextureHandle, f32)> {
    let px = (px.clamp(12, 136) + 3) / 4 * 4;
    ATLAS.with(|a| {
        let mut a = a.borrow_mut();
        let key = (gid.0, px);
        if let Some(t) = a.textures.get(&key) {
            return t.clone().map(|t| {
                let s = t.size();
                (t, s[0] as f32 / s[1].max(1) as f32)
            });
        }
        let tex = (|| {
            let face = a.face.as_ref()?;
            let img = face.glyph_raster_image(gid, 136)?;
            if img.format != ttf_parser::RasterImageFormat::PNG {
                return None;
            }
            let rgba = image::load_from_memory_with_format(img.data, image::ImageFormat::Png).ok()?.to_rgba8();
            let (w, h) = (rgba.width(), rgba.height());
            let scale = px as f32 / w.max(h) as f32;
            let (nw, nh) = (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1));
            let small = image::imageops::resize(&rgba, nw, nh, image::imageops::FilterType::Lanczos3);
            let ci = ColorImage::from_rgba_unmultiplied([nw as usize, nh as usize], small.as_raw());
            Some(ctx.load_texture(format!("emoji-{}-{px}", gid.0), ci, TextureOptions::LINEAR))
        })();
        a.textures.insert(key, tex.clone());
        tex.map(|t| {
            let s = t.size();
            (t, s[0] as f32 / s[1].max(1) as f32)
        })
    })
}

/// Textur für ein einzelnes Zeichen.
pub fn texture(ctx: &egui::Context, c: char, px: u32) -> Option<(TextureHandle, f32)> {
    let gid = ATLAS.with(|a| a.borrow().face.as_ref().and_then(|f| f.glyph_index(c)))?;
    texture_gid(ctx, gid, px)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies() {
        assert!(!wants_color('a', false, false));
        assert!(!wants_color('é', false, false));
        assert!(!wants_color('界', true, false));
        assert!(wants_color('😀', true, false));
        assert!(wants_color('❤', false, true));
        assert!(wants_color('✅', true, false));
    }

    #[test]
    fn shapes_sequences() {
        // Einzelnes Emoji mit Variationsselektor
        assert!(shape(&['❤', '\u{fe0f}']).is_some());
        // ZWJ-Familie, Flagge, Hautfarbe, Keycap
        assert!(shape(&['👨', '\u{200d}', '👩', '\u{200d}', '👧']).is_some(), "Familie");
        assert!(shape(&['🇩', '🇪']).is_some(), "Flagge");
        assert!(shape(&['👍', '🏽']).is_some(), "Hautfarbe");
        assert!(shape(&['1', '\u{fe0f}', '\u{20e3}']).is_some(), "Keycap");
        // Ergebnis ist ein anderes Glyph als das erste Einzelemoji
        assert_ne!(shape(&['👨', '\u{200d}', '👩', '\u{200d}', '👧']), shape(&['👨']));
    }
}
