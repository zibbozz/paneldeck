//! Hintergrund des Themes im Fenster (#387): Jedes Fenster des Hubs (Hauptfenster und weitere Fenster) bekommt ein eigenes,
//! im Hintergrund-Faden berechnetes Bild in seiner Größe (physische Pixel, höchstens `max_texture_side`). Es liegt ganz
//! unten auf der Hintergrund-Ebene; die Flächen darüber (Terminals, Editor, To-do, Chat, Seitenleiste) bekommen die
//! Deckkraft ihres Bereichs ([`App::surface_alpha`]). Nach einer Größenänderung wird erst nach [`DEBOUNCE_S`] neu
//! gerechnet, bis dahin wird das alte Bild gestreckt; danach genügt ein einzelnes Neuzeichnen (kein Dauer-Neuzeichnen).
//! Dateien werden je Stand der Themes nur einmal aufgelöst ([`Backdrops::src`]).
//!
//! Ohne Hintergrund im Theme, mit „Transparenz reduzieren“ und in den Doku-Screenshots bleibt alles wie vor 3.2.0.

use super::App;
use crate::theme::background::{self, Area};
use crate::theme::bake::{self, BakeSpec, Slot, Src, Stats};
use eframe::egui::{self, Color32, Pos2, Rect};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Wartezeit nach einer Größenänderung, bevor neu gerechnet wird.
const DEBOUNCE_S: f64 = 0.25;
/// Größe der kleinen Bilder für die Kennzahlen anderer Themes (`theme_get`).
const STATS_SIZE: (u32, u32) = (192, 108);
/// Auftraggeber-Kennung der Kennzahlen-Aufträge.
const STATS_WHO: u64 = 0x5747_0001;
/// Zustand eines Fensters, das so lange kein Bild mehr gezeichnet hat (geschlossen), wird freigegeben.
const FORGET_AFTER: Duration = Duration::from_secs(30);

/// Zustand je Fenster.
#[derive(Default)]
struct WinBg {
    /// Gezeigtes Bild: (Kennung, Kennung ohne Größe, Textur).
    tex: Option<(u64, u64, egui::TextureHandle)>,
    /// Laufender Auftrag: (Kennung, Kennung ohne Größe, Theme, Ergebnis).
    pending: Option<(u64, u64, String, Slot)>,
    /// Neues Bild nach einer Größenänderung: (Kennung, seit).
    wait: Option<(u64, f64)>,
    /// Zwischenspeicher der Angaben (Schlüssel, Angaben).
    spec: Option<(SpecKey, Option<BakeSpec>)>,
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
    /// Kennzahlen je Theme-Name (für `theme_get` und den Kontrast im Terminal): (Kennung ohne Größe, Kennzahlen). Bleiben
    /// über ein Neuladen erhalten, solange die Kennung passt.
    stats: HashMap<String, (u64, Stats)>,
    /// Laufender Kennzahlen-Auftrag: (Theme, Kennung ohne Größe, Stand, Ergebnis).
    stats_job: Option<(String, u64, u64, Slot)>,
    /// Noch zu rechnende Kennzahlen (Theme, Angaben) und der Stand, für den die Liste gilt.
    stats_todo: Vec<(String, BakeSpec)>,
    stats_checked: Option<u64>,
    /// Aufgelöste Bilddateien je (Theme-Datei, Name) für den aktuellen Stand.
    srcs: HashMap<(PathBuf, String), Option<Src>>,
    /// Zählt bei jedem Neuladen der Themes hoch (Dateien neu auflösen).
    generation: u64,
}

impl Backdrops {
    /// Themes wurden neu geladen: Dateien neu auflösen, Angaben neu bauen, Kennzahlen neu prüfen (passende bleiben).
    pub(super) fn themes_changed(&mut self) {
        self.generation += 1;
        self.srcs.clear();
        self.stats_job = None;
        self.stats_todo.clear();
        self.stats_checked = None;
        for w in self.wins.values_mut() {
            w.spec = None;
        }
    }

    /// Stand der Themes (zählt bei jedem Neuladen hoch).
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    /// Kennzahlen eines Themes (sofern schon berechnet).
    pub(super) fn stats_of(&self, name: &str) -> Option<Stats> {
        self.stats.get(name).map(|(_, s)| *s)
    }

    /// Quelle einer Bilddatei (aufgelöst einmal je Stand der Themes).
    fn src(&mut self, source: Option<&Path>, name: &str) -> Option<Src> {
        let source = source?;
        self.srcs.entry((source.to_path_buf(), name.to_string())).or_insert_with(|| file_src(Some(source), name)).clone()
    }

    /// Angaben eines Themes mit den zwischengespeicherten Dateien.
    fn spec(&mut self, t: &crate::theme::Theme, w: u32, h: u32, ppp: f32) -> Option<BakeSpec> {
        let files: Vec<(String, Option<Src>)> = t.background.as_ref()?.files().into_iter().map(|n| (n.to_string(), self.src(t.source.as_deref(), n))).collect();
        bake::spec_for(t, w, h, ppp, &|n| files.iter().find(|(k, _)| k == n).and_then(|(_, s)| s.clone()))
    }
}

/// Quelle einer Bilddatei des Themes mit der Datei `source` (nur sichere Namen im Ordner des Themes).
pub(super) fn file_src(source: Option<&Path>, name: &str) -> Option<Src> {
    source.and_then(|p| background::resolve_file(p, name)).and_then(Src::file)
}

/// Größe des Bildes für ein Fenster: physische Pixel, höchstens `max_side` je Seite (dann wird es gestreckt). Dazu die
/// Bildpunkte je Punkt des verkleinerten Bildes (für Abstände, Muster und Unschärfe).
pub(super) fn bake_size(size_pt: egui::Vec2, ppp: f32, max_side: usize) -> (u32, u32, f32) {
    let (w, h) = ((size_pt.x * ppp).round().max(1.0), (size_pt.y * ppp).round().max(1.0));
    let s = (max_side.max(64) as f32 / w.max(h)).min(1.0);
    (((w * s).round() as u32).max(1), ((h * s).round() as u32).max(1), ppp * s)
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

/// Mittlere Farbe des gefilterten Hintergrunds eines Themes (für den Mindestkontrast der Terminalschrift).
fn filtered_mean(t: &crate::theme::Theme, s: &Stats) -> Color32 {
    s.after(t.background.as_ref().map(|b| b.brightness).unwrap_or(0), bake::overlay_of(t)).mean
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
    /// Deckkraft × Deckkraft des Bereichs (Bereich aus = deckend); sonst wie bisher die globale Deckkraft. Derselbe Wert
    /// gilt für Stil, Titelleiste und Design-Prüfung.
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
        crate::termview::set_backdrop(None);
        if !on {
            self.backdrops.wins.remove(&vid);
            return;
        }
        let wb = self.backdrops.wins.entry(vid).or_default();
        wb.last_used = Some(Instant::now());
        let rect = ctx.content_rect();
        if rect.width() < 1.0 || rect.height() < 1.0 {
            return; // minimiert: das Bild bleibt für das Wiederherstellen
        }
        let ppp = ctx.pixels_per_point();
        let now = ctx.input(|i| i.time);
        let (w, h, ppp_img) = bake_size(rect.size(), ppp, ctx.input(|i| i.max_texture_side));
        let theme = self.current_theme().clone();
        let skey = SpecKey { theme: theme.name.clone(), generation: self.backdrops.generation, w, h, ppp: (ppp_img * 1000.0).round() as u32 };
        if self.backdrops.wins.get(&vid).and_then(|w| w.spec.as_ref()).is_none_or(|(k, _)| *k != skey) {
            let spec = self.backdrops.spec(&theme, w, h, ppp_img);
            if let Some(wb) = self.backdrops.wins.get_mut(&vid) {
                wb.spec = Some((skey, spec));
            }
        }
        let Backdrops { wins, stats, .. } = &mut self.backdrops;
        let Some(wb) = wins.get_mut(&vid) else { return };
        let Some(spec) = wb.spec.as_ref().and_then(|(_, s)| s.clone()) else { return };
        // Fertiges Bild abholen.
        if let Some(r) = wb.pending.as_ref().and_then(|p| bake::take(&p.3)) {
            if let Some((k, l, name, _)) = wb.pending.take() {
                if let Ok(b) = r {
                    let img = egui::ColorImage::from_rgba_unmultiplied([b.img.width() as usize, b.img.height() as usize], b.img.as_raw());
                    let tex = ctx.load_texture(format!("pd-backdrop-{vid:?}"), img, egui::TextureOptions::LINEAR);
                    wb.tex = Some((k, l, tex));
                    stats.insert(name, (l, b.stats));
                }
            }
        }
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
                    // Ein einzelnes Bild nach der Wartezeit, damit das scharfe Bild auch ohne weitere Eingabe kommt.
                    ctx.request_repaint_after(Duration::from_secs_f64(DEBOUNCE_S + 0.02));
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
        // Mindestkontrast der Terminalschrift: das gefilterte Bild (Mittel); die Fläche darüber mischt termview mit dem
        // Theme, das das Terminal wirklich nutzt (auch Profil-Themes).
        let mean = stats.get(&theme.name).map(|(_, s)| filtered_mean(&theme, s));
        crate::termview::set_backdrop(mean);
    }

    /// Im Hauptfenster je Bild: Zustände geschlossener Fenster freigeben, Quellbilder freigeben, wenn nichts sie braucht,
    /// und Kennzahlen der Themes mit Hintergrund nacheinander berechnen lassen (kleines Bild, für `theme_get`).
    pub(super) fn backdrop_tick(&mut self, editor_open: bool) {
        self.backdrops.wins.retain(|_, w| w.last_used.is_some_and(|t| t.elapsed() < FORGET_AFTER));
        if !self.backdrop_active() && !editor_open {
            bake::clear_sources();
        }
        let generation = self.backdrops.generation;
        if self.backdrops.stats_checked != Some(generation) {
            // Nach einem Neuladen: nur Themes, deren Angaben sich geändert haben (oder neue), neu rechnen.
            let themes: Vec<crate::theme::Theme> = self.themes.iter().filter(|t| t.background.is_some()).cloned().collect();
            let mut todo = Vec::new();
            for t in &themes {
                let Some(spec) = self.backdrops.spec(t, STATS_SIZE.0, STATS_SIZE.1, STATS_SIZE.0 as f32 / 1280.0) else { continue };
                if self.backdrops.stats.get(&t.name).map(|x| x.0) != Some(spec.look_key()) {
                    self.backdrops.stats.remove(&t.name);
                    todo.push((t.name.clone(), spec));
                }
            }
            self.backdrops.stats.retain(|n, _| themes.iter().any(|t| t.name == *n));
            self.backdrops.stats_todo = todo;
            self.backdrops.stats_checked = Some(generation);
        }
        if let Some((name, look, job_gen, slot)) = &self.backdrops.stats_job {
            let Some(r) = bake::take(slot) else { return };
            // Nur für den Stand, für den der Auftrag galt (sonst ist das Ergebnis veraltet).
            if *job_gen == generation {
                if let Ok(b) = r {
                    self.backdrops.stats.insert(name.clone(), (*look, b.stats));
                }
            }
            self.backdrops.stats_job = None;
        }
        while let Some((name, spec)) = self.backdrops.stats_todo.pop() {
            // Ein Fenster-Bild hat die Kennzahlen vielleicht schon geliefert.
            if self.backdrops.stats.contains_key(&name) {
                continue;
            }
            let look = spec.look_key();
            self.backdrops.stats_job = Some((name, look, generation, bake::submit(STATS_WHO, spec)));
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bake_size_is_physical_and_capped() {
        assert_eq!(bake_size(egui::Vec2::new(1000.0, 500.0), 1.5, 8192), (1500, 750, 1.5));
        // Begrenzt: die Bildpunkte je Punkt schrumpfen mit (Abstände und Muster bleiben gleich groß im Fenster).
        let (w, h, p) = bake_size(egui::Vec2::new(4000.0, 2000.0), 2.0, 2048);
        assert_eq!((w, h), (2048, 1024));
        assert!((p - 0.512).abs() < 1e-4, "{p}");
        assert_eq!(bake_size(egui::Vec2::new(0.0, 0.0), 1.0, 2048), (1, 1, 1.0));
    }

    #[test]
    fn resizing_is_debounced_but_new_settings_are_not() {
        assert_eq!(decide(1, 10, None, None, None, 0.0), Step::Submit);
        assert_eq!(decide(1, 10, None, Some(1), None, 0.0), Step::Wait);
        assert_eq!(decide(1, 10, Some((1, 10)), None, None, 0.0), Step::Keep);
        assert_eq!(decide(2, 10, Some((1, 10)), None, None, 5.0), Step::Debounce);
        assert_eq!(decide(2, 10, Some((1, 10)), None, Some((2, 5.0)), 5.1), Step::Debounce);
        assert_eq!(decide(2, 10, Some((1, 10)), None, Some((2, 5.0)), 5.3), Step::Submit);
        assert_eq!(decide(3, 10, Some((1, 10)), None, Some((2, 5.0)), 5.3), Step::Debounce);
        assert_eq!(decide(4, 11, Some((1, 10)), None, None, 5.0), Step::Submit);
    }

    #[test]
    fn every_window_keeps_its_own_image_and_stats_survive_reloads() {
        let mut b = Backdrops::default();
        let (a, c) = (egui::ViewportId::ROOT, egui::ViewportId::from_hash_of("pd-window-2"));
        b.wins.entry(a).or_default().wait = Some((1, 0.0));
        b.wins.entry(c).or_default().wait = Some((2, 0.0));
        assert_eq!(b.wins.len(), 2);
        assert_ne!(b.wins[&a].wait, b.wins[&c].wait);
        b.stats.insert("T".into(), (1, Stats { mean: Color32::BLACK, bright: Color32::WHITE, dark: Color32::BLACK }));
        b.stats_job = Some(("T".into(), 1, 0, Default::default()));
        b.srcs.insert((PathBuf::from("x.json"), "a.png".into()), None);
        b.themes_changed();
        assert!(b.wins.values().all(|w| w.spec.is_none()) && b.generation() == 1);
        assert!(b.stats_of("T").is_some(), "Kennzahlen bleiben, bis sich die Angaben ändern");
        assert!(b.stats_job.is_none() && b.srcs.is_empty(), "laufender Auftrag und Dateien verworfen");
    }

    #[test]
    fn files_are_resolved_once_per_generation() {
        let dir = std::env::temp_dir().join(format!("paneldeck-test-bdsrc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("t")).unwrap();
        let src = dir.join("t.json");
        std::fs::write(&src, "{}").unwrap();
        std::fs::write(dir.join("t").join("a.png"), b"x").unwrap();
        let mut b = Backdrops::default();
        assert!(b.src(Some(&src), "a.png").is_some());
        std::fs::remove_file(dir.join("t").join("a.png")).unwrap();
        assert!(b.src(Some(&src), "a.png").is_some(), "aus dem Zwischenspeicher");
        b.themes_changed();
        assert!(b.src(Some(&src), "a.png").is_none(), "nach dem Neuladen neu aufgelöst");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
