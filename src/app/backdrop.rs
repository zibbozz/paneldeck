//! Hintergrund des Themes im Fenster (#387): Jedes Fenster des Hubs (Hauptfenster und weitere Fenster) bekommt ein eigenes,
//! im Hintergrund-Faden berechnetes Bild in seiner Größe (physische Pixel, höchstens `max_texture_side`). Es liegt ganz
//! unten auf der Hintergrund-Ebene; die Flächen darüber (Terminals, Editor, To-do, Chat, Seitenleiste) bekommen die
//! Deckkraft ihres Bereichs ([`App::surface_alpha`]). Nach einer Größenänderung wird erst nach [`DEBOUNCE_S`] neu
//! gerechnet, bis dahin wird das alte Bild gestreckt. Kein zusätzliches Neuzeichnen: Ein fertiges Bild erscheint mit dem
//! nächsten ohnehin fälligen Bild (die App zeichnet spätestens alle 500 ms).
//!
//! Ohne Hintergrund im Theme, mit „Transparenz reduzieren“ und in den Doku-Screenshots bleibt alles wie vor 3.2.0.

use super::App;
use crate::theme::background::{self, Area};
use crate::theme::bake::{self, Slot, Src, Stats};
use eframe::egui::{self, Color32, Pos2, Rect};
use std::collections::HashMap;
use std::time::Instant;

/// Wartezeit nach einer Größenänderung, bevor neu gerechnet wird.
const DEBOUNCE_S: f64 = 0.25;
/// Größe der kleinen Bilder für die Kennzahlen anderer Themes (`theme_get`).
const STATS_SIZE: (u32, u32) = (192, 108);
/// Auftraggeber-Kennung der Kennzahlen-Aufträge.
const STATS_WHO: u64 = 0x5747_0001;

/// Zustand je Fenster.
#[derive(Default)]
struct WinBg {
    /// Gezeigtes Bild: (Kennung, Kennung ohne Größe, Textur).
    tex: Option<(u64, u64, egui::TextureHandle)>,
    /// Laufender Auftrag: (Kennung, Kennung ohne Größe, Kennzahlen gehören zu Theme, Ergebnis).
    pending: Option<(u64, u64, String, Slot)>,
    /// Neues Bild nach einer Größenänderung: (Kennung, seit).
    wait: Option<(u64, f64)>,
    /// Zwischenspeicher der Angaben (Schlüssel, Angaben).
    spec: Option<(SpecKey, Option<bake::BakeSpec>)>,
    last_used: Option<Instant>,
}

#[derive(Clone, PartialEq)]
struct SpecKey {
    theme: String,
    generation: u64,
    w: u32,
    h: u32,
    ppp: u32,
}

/// Hintergründe aller Fenster und Kennzahlen je Theme.
#[derive(Default)]
pub(super) struct Backdrops {
    wins: HashMap<egui::ViewportId, WinBg>,
    /// Kennzahlen je Theme-Name (für `theme_get` und den Kontrast im Terminal): (Kennung ohne Größe, Kennzahlen).
    stats: HashMap<String, (u64, Stats)>,
    stats_job: Option<(String, u64, Slot)>,
    /// Zählt bei jedem Neuladen der Themes hoch (Dateien neu auflösen).
    generation: u64,
}

impl Backdrops {
    /// Themes wurden neu geladen: Dateien und Kennzahlen neu bestimmen.
    pub(super) fn themes_changed(&mut self) {
        self.generation += 1;
        self.stats.clear();
        for w in self.wins.values_mut() {
            w.spec = None;
        }
    }

    /// Kennzahlen eines Themes (sofern schon berechnet).
    pub(super) fn stats_of(&self, name: &str) -> Option<Stats> {
        self.stats.get(name).map(|(_, s)| *s)
    }
}

/// Quelle einer Bilddatei des Themes mit der Datei `source` (nur sichere Namen im Ordner des Themes).
pub(super) fn file_src(source: Option<&std::path::Path>, name: &str) -> Option<Src> {
    source.and_then(|p| background::resolve_file(p, name)).and_then(Src::file)
}

/// Größe des Bildes für ein Fenster: physische Pixel, höchstens `max_side` je Seite (dann wird es gestreckt).
pub(super) fn bake_size(size_pt: egui::Vec2, ppp: f32, max_side: usize) -> (u32, u32) {
    let (w, h) = ((size_pt.x * ppp).round().max(1.0), (size_pt.y * ppp).round().max(1.0));
    let s = (max_side.max(64) as f32 / w.max(h)).min(1.0);
    (((w * s).round() as u32).max(1), ((h * s).round() as u32).max(1))
}

/// Was ein Fenster in diesem Bild tun soll.
#[derive(Debug, PartialEq)]
pub(super) enum Step {
    /// Das gezeigte Bild passt.
    Keep,
    /// Auftrag läuft schon.
    Wait,
    /// Neu rechnen.
    Submit,
    /// Nur die Größe ist anders: bis `DEBOUNCE_S` warten (altes Bild gestreckt).
    Debounce,
}

/// Entscheidung je Bild (getrennt testbar): `shown`/`pending` = Kennungen (mit, ohne Größe) des gezeigten bzw.
/// laufenden Bildes, `wait` = seit wann auf die Größe `key` gewartet wird.
pub(super) fn decide(key: u64, look: u64, shown: Option<(u64, u64)>, pending: Option<u64>, wait: Option<(u64, f64)>, now: f64) -> Step {
    if shown.is_some_and(|s| s.0 == key) {
        return Step::Keep;
    }
    if pending == Some(key) {
        return Step::Wait;
    }
    // Andere Einstellungen (oder noch kein Bild): sofort. Nur eine andere Größe: entprellt.
    if !shown.is_some_and(|s| s.1 == look) {
        return Step::Submit;
    }
    match wait {
        Some((k, since)) if k == key && now - since >= DEBOUNCE_S => Step::Submit,
        _ => Step::Debounce,
    }
}

impl App {
    /// Zeigt das aktuelle Theme einen eigenen Hintergrund? Nicht mit „Transparenz reduzieren“ und nie in den
    /// Doku-Screenshots (die Bilder bleiben ohne Hintergrundbild).
    pub(super) fn backdrop_active(&self) -> bool {
        #[cfg(feature = "readme-shots")]
        if super::shot_start::spec().is_some() {
            return false;
        }
        !self.settings.reduce_transparency && self.current_theme().background.is_some()
    }

    /// Das gewählte Theme ohne Änderungen (deckender Hintergrund).
    pub(super) fn current_theme(&self) -> &crate::theme::Theme {
        self.themes.iter().find(|t| t.name == self.settings.theme).unwrap_or(&self.themes[0])
    }

    /// Deckkraft (0–1) der Flächen eines Bereichs: „Transparenz reduzieren“ = deckend; mit Hintergrund globale
    /// Deckkraft × Deckkraft des Bereichs (Bereich aus = deckend); sonst wie bisher die globale Deckkraft.
    pub(super) fn surface_alpha(&self, area: Area) -> f32 {
        if self.settings.reduce_transparency {
            return 1.0;
        }
        let g = self.settings.opacity.clamp(0.3, 1.0);
        match self.current_theme().background.as_ref() {
            Some(b) if self.backdrop_active() => b.surface_alpha(area, g),
            _ => g,
        }
    }

    /// Theme mit der Deckkraft des Bereichs `area` im Hintergrund.
    pub(super) fn theme_for(&self, area: Area) -> crate::theme::Theme {
        let mut t = self.current_theme().clone();
        let a = (self.surface_alpha(area) * 255.0).round() as u8;
        t.bg = Color32::from_rgba_unmultiplied(t.bg.r(), t.bg.g(), t.bg.b(), a);
        t
    }

    /// Zu Beginn jedes Fensterbilds (vor allen Flächen): Hintergrund dieses Fensters zeichnen bzw. berechnen lassen.
    pub(super) fn backdrop_frame(&mut self, ctx: &egui::Context) {
        let vid = ctx.viewport_id();
        let on = self.backdrop_active();
        crate::style::set_backdrop(on);
        if !on {
            self.backdrops.wins.remove(&vid);
            crate::termview::set_backdrop(None);
            return;
        }
        let rect = ctx.content_rect();
        if rect.width() < 1.0 || rect.height() < 1.0 {
            return; // minimiert
        }
        let ppp = ctx.pixels_per_point();
        let now = ctx.input(|i| i.time);
        let (w, h) = bake_size(rect.size(), ppp, ctx.input(|i| i.max_texture_side));
        let theme = self.current_theme().clone();
        let skey = SpecKey { theme: theme.name.clone(), generation: self.backdrops.generation, w, h, ppp: (ppp * 1000.0).round() as u32 };
        let wb = self.backdrops.wins.entry(vid).or_default();
        wb.last_used = Some(Instant::now());
        if wb.spec.as_ref().is_none_or(|(k, _)| *k != skey) {
            let src = theme.source.clone();
            let spec = bake::spec_for(&theme, w, h, ppp, &|n| file_src(src.as_deref(), n));
            wb.spec = Some((skey, spec));
        }
        let Some(spec) = wb.spec.as_ref().and_then(|(_, s)| s.clone()) else { return };
        // Fertiges Bild abholen.
        if let Some((k, l, name, slot)) = &wb.pending {
            if let Some(r) = bake::take(slot) {
                let (k, l, name) = (*k, *l, name.clone());
                wb.pending = None;
                if let Ok(b) = r {
                    let img = egui::ColorImage::from_rgba_unmultiplied([b.img.width() as usize, b.img.height() as usize], b.img.as_raw());
                    let tex = ctx.load_texture(format!("pd-backdrop-{vid:?}"), img, egui::TextureOptions::LINEAR);
                    wb.tex = Some((k, l, tex));
                    self.backdrops.stats.insert(name, (l, b.stats));
                }
            }
        }
        let wb = self.backdrops.wins.entry(vid).or_default();
        let (key, look) = (spec.key(), spec.look_key());
        match decide(key, look, wb.tex.as_ref().map(|t| (t.0, t.1)), wb.pending.as_ref().map(|p| p.0), wb.wait, now) {
            Step::Keep | Step::Wait => {}
            Step::Submit => {
                let who = {
                    use std::hash::{Hash, Hasher};
                    let mut hs = std::collections::hash_map::DefaultHasher::new();
                    vid.hash(&mut hs);
                    hs.finish()
                };
                wb.pending = Some((key, look, theme.name.clone(), bake::submit(who, spec.clone())));
                wb.wait = None;
            }
            Step::Debounce => {
                if wb.wait.is_none_or(|(k, _)| k != key) {
                    wb.wait = Some((key, now));
                }
            }
        }
        // Zeichnen: ganz unten auf der Hintergrund-Ebene (vor allen Flächen dieses Bilds).
        let painter = ctx.layer_painter(egui::LayerId::background());
        match &wb.tex {
            Some((_, _, tex)) => {
                painter.image(tex.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
            None => {
                // Erstes Bild noch nicht fertig: Grundfarbe statt durchscheinendem Desktop.
                let a = (spec.opacity as f32 * 2.55).round() as u8;
                painter.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(spec.base[0], spec.base[1], spec.base[2], a));
            }
        }
        // Kontrast der Terminalschrift auf dem Standardhintergrund: gegen die Fläche über dem (gefilterten) Bild.
        let behind = self.backdrops.stats.get(&theme.name).map(|(_, s)| s.after(theme.background.as_ref().map(|b| b.brightness).unwrap_or(0), bake::overlay_of(&theme)).mean);
        let term = self.theme_for(Area::Terminals).bg;
        crate::termview::set_backdrop(behind.map(|m| crate::theme::color::over(term, m)));
    }

    /// Im Hauptfenster je Bild: Zustände geschlossener Fenster freigeben und Kennzahlen der übrigen Themes mit Hintergrund
    /// nacheinander berechnen lassen (kleines Bild, für `theme_get`).
    pub(super) fn backdrop_tick(&mut self) {
        self.backdrops.wins.retain(|_, w| w.last_used.is_some_and(|t| t.elapsed().as_secs() < 5));
        if let Some((name, look, slot)) = &self.backdrops.stats_job {
            match bake::take(slot) {
                Some(r) => {
                    if let Ok(b) = r {
                        self.backdrops.stats.insert(name.clone(), (*look, b.stats));
                    }
                    self.backdrops.stats_job = None;
                }
                None => return,
            }
        }
        let todo = self.themes.iter().find(|t| t.background.is_some() && !self.backdrops.stats.contains_key(&t.name));
        if let Some(t) = todo {
            let src = t.source.clone();
            if let Some(spec) = bake::spec_for(t, STATS_SIZE.0, STATS_SIZE.1, STATS_SIZE.0 as f32 / 1280.0, &|n| file_src(src.as_deref(), n)) {
                let look = spec.look_key();
                self.backdrops.stats_job = Some((t.name.clone(), look, bake::submit(STATS_WHO, spec)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bake_size_is_physical_and_capped() {
        assert_eq!(bake_size(egui::Vec2::new(1000.0, 500.0), 1.5, 8192), (1500, 750));
        assert_eq!(bake_size(egui::Vec2::new(4000.0, 2000.0), 2.0, 2048), (2048, 1024));
        assert_eq!(bake_size(egui::Vec2::new(0.0, 0.0), 1.0, 2048), (1, 1));
    }

    #[test]
    fn resizing_is_debounced_but_new_settings_are_not() {
        // Noch kein Bild: sofort.
        assert_eq!(decide(1, 10, None, None, None, 0.0), Step::Submit);
        // Läuft schon.
        assert_eq!(decide(1, 10, None, Some(1), None, 0.0), Step::Wait);
        // Gezeigt.
        assert_eq!(decide(1, 10, Some((1, 10)), None, None, 0.0), Step::Keep);
        // Nur die Größe anders: warten, nach 250 ms rechnen.
        assert_eq!(decide(2, 10, Some((1, 10)), None, None, 5.0), Step::Debounce);
        assert_eq!(decide(2, 10, Some((1, 10)), None, Some((2, 5.0)), 5.1), Step::Debounce);
        assert_eq!(decide(2, 10, Some((1, 10)), None, Some((2, 5.0)), 5.3), Step::Submit);
        // Während des Ziehens wechselt die Größe weiter: die Wartezeit beginnt neu (andere Kennung).
        assert_eq!(decide(3, 10, Some((1, 10)), None, Some((2, 5.0)), 5.3), Step::Debounce);
        // Andere Einstellungen (Theme, Regler): sofort, ohne Warten.
        assert_eq!(decide(4, 11, Some((1, 10)), None, None, 5.0), Step::Submit);
    }

    #[test]
    fn every_window_keeps_its_own_image() {
        let mut b = Backdrops::default();
        let (a, c) = (egui::ViewportId::ROOT, egui::ViewportId::from_hash_of("pd-window-2"));
        b.wins.entry(a).or_default().wait = Some((1, 0.0));
        b.wins.entry(c).or_default().wait = Some((2, 0.0));
        assert_eq!(b.wins.len(), 2);
        assert_ne!(b.wins[&a].wait, b.wins[&c].wait);
        b.stats.insert("T".into(), (1, Stats { mean: Color32::BLACK, bright: Color32::WHITE, dark: Color32::BLACK }));
        b.themes_changed();
        assert!(b.stats.is_empty() && b.wins.values().all(|w| w.spec.is_none()) && b.generation == 1);
    }
}
