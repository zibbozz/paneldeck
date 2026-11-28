//! Waagerechte Bildlaufleiste des Textfelds (#389): erscheint unter dem Text, wenn der Umbruch aus ist und Zeilen breiter
//! sind als das Feld; Griff ziehen, Klick auf die Bahn blättert eine Seite nach links/rechts. Das rechte Ende ist die
//! längste Zeile (plus [`MARGIN_COLS`]), weiter lässt sich nicht rollen.
//!
//! Die Breite der längsten Zeile wird je Stand des Dokuments (Revision, Tabbreite) einmal gemessen – kleine Dokumente
//! sofort, größere (ab [`SYNC_BYTES`], also auch der Modus für große Dateien) in einem Hilfsthread auf einem Klon des
//! Ropes; bis das Ergebnis da ist, gilt der letzte Stand. Die Messung liegt am Dokument ([`DocWidth`] in `DocEntry`), alle
//! Teilungen mit derselben Datei teilen sie; je Ansicht bleibt nur der Zustand der Leiste ([`HScroll`]). Gemessen wird wie gezeichnet: Tabs bis zum nächsten Tabstopp,
//! breite Zeichen (Ostasiatisch, Emoji) doppelt, sehr lange Zeilen ([`super::text::LONG_LINE`]) ein Zeichen = eine Spalte.
//!
//! Nur Pixel und Spalten, kein Dokument-Zustand: [`bar`] ist auch für andere Textansichten gedacht (z. B. den Vergleich).

use super::text::LONG_LINE;
use crate::editor::Document;
use crate::editor::text as et;
use crate::style;
use eframe::egui::{self, Id, Pos2, Rect, Sense, Ui, Vec2};
use ropey::{Rope, RopeSlice};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Höhe der Leiste (wie die Breite der senkrechten).
pub(super) const BAR_H: f32 = 10.0;
/// Rand rechts hinter der längsten Zeile (Spalten): Platz für den Cursor am Zeilenende (`reveal_cursor` hält drei
/// Spalten Abstand vom rechten Rand).
pub(super) const MARGIN_COLS: f32 = 4.0;
/// Bis zu dieser Größe (Bytes) wird sofort gemessen, darüber im Hilfsthread.
pub(super) const SYNC_BYTES: usize = 256 * 1024;
/// Mindestabstand zwischen zwei Messungen im Hilfsthread (Tippen in einer großen Datei misst nicht pausenlos).
const JOB_GAP: Duration = Duration::from_millis(400);
/// So oft nachsehen, ob die Messung fertig ist.
const POLL: Duration = Duration::from_millis(30);
/// Kleinste Breite des Griffs.
const MIN_THUMB: f32 = 24.0;

/// Stand eines Dokuments, zu dem eine Messung gehört.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    rev: u64,
    tab: usize,
    bytes: usize,
    lines: usize,
}

impl Key {
    fn of(doc: &Document, tab: usize) -> Self {
        Key { rev: doc.revision(), tab, bytes: doc.len_bytes(), lines: doc.len_lines() }
    }
}

struct Job {
    key: Key,
    slot: Arc<Mutex<Option<usize>>>,
}

/// Breite der längsten Zeile eines Dokuments (liegt am Dokument, alle Ansichten teilen sich Messung und Ergebnis).
#[derive(Default)]
pub(crate) struct DocWidth {
    /// Breite (visuelle Spalten) und der Stand, zu dem sie gehört.
    cols: usize,
    measured: Option<Key>,
    job: Option<Job>,
    job_started: Option<Instant>,
    /// Zahl der Messungen (sofort oder im Hintergrund gestartet).
    #[cfg(test)]
    pub(super) scans: usize,
}

impl DocWidth {
    /// Breite der längsten Zeile in Spalten: gemessen für den aktuellen Stand, sonst (solange im Hintergrund gemessen
    /// wird) der letzte Stand. Durchläuft das Dokument höchstens einmal je Stand.
    pub(super) fn longest_cols(&mut self, doc: &Document, tab: usize, ctx: &egui::Context) -> usize {
        let key = Key::of(doc, tab.max(1));
        if self.measured == Some(key) {
            return self.cols;
        }
        // Fertige Messung abholen
        if let Some(job) = &self.job {
            let done = job.slot.lock().ok().and_then(|mut s| s.take());
            match done {
                Some(cols) => {
                    self.cols = cols;
                    self.measured = Some(job.key);
                    self.job = None;
                    if self.measured == Some(key) {
                        return self.cols;
                    }
                }
                None => {
                    ctx.request_repaint_after(POLL);
                    return self.cols;
                }
            }
        }
        if doc.len_bytes() <= SYNC_BYTES {
            self.count_scan();
            self.cols = longest_line_cols(doc.rope(), key.tab);
            self.measured = Some(key);
            return self.cols;
        }
        if let Some(t) = self.job_started
            && t.elapsed() < JOB_GAP
        {
            ctx.request_repaint_after(JOB_GAP - t.elapsed());
            return self.cols;
        }
        self.count_scan();
        let slot = Arc::new(Mutex::new(None));
        let (rope, out, tab) = (doc.rope().clone(), slot.clone(), key.tab);
        let spawned = std::thread::Builder::new()
            .name("pd-ed-width".into())
            .spawn(move || {
                let cols = longest_line_cols(&rope, tab);
                if let Ok(mut s) = out.lock() {
                    *s = Some(cols);
                }
            })
            .is_ok();
        if spawned {
            self.job = Some(Job { key, slot });
            self.job_started = Some(Instant::now());
            ctx.request_repaint_after(POLL);
        } else {
            // Kein Thread: beim letzten Stand bleiben, nicht im Bild messen.
            self.measured = Some(key);
        }
        self.cols
    }

    #[cfg(test)]
    fn count_scan(&mut self) {
        self.scans += 1;
    }

    #[cfg(not(test))]
    fn count_scan(&mut self) {}

    /// Gehört die Messung zum aktuellen Stand von `doc`?
    #[cfg(test)]
    pub(super) fn is_current(&self, doc: &Document, tab: usize) -> bool {
        self.measured == Some(Key::of(doc, tab.max(1)))
    }

    /// Läuft gerade eine Messung im Hintergrund?
    #[cfg(test)]
    pub(super) fn measuring(&self) -> bool {
        self.job.is_some()
    }
}

/// Zustand der Leiste in einer Ansicht: Ziehen des Griffs, Fläche im letzten Bild, Breite der Zeilen unter den Cursorn.
#[derive(Clone, Debug, Default)]
pub(crate) struct HScroll {
    /// Versatz des Zeigers im Griff beim Ziehen.
    pub(super) drag: Option<f32>,
    /// Fläche der Leiste im letzten Bild (`None` = ausgeblendet).
    pub(super) last_bar: Option<Rect>,
    /// Breitste Zeile unter den Cursorn und der Stand dazu (Revision, Tabbreite, Zahl und Summe der Cursor).
    heads: Option<((u64, usize, usize, usize), usize)>,
}

impl HScroll {
    /// Breite der breitesten Zeile, in der ein Cursor steht (sie zählt sicher, auch während die Messung des ganzen
    /// Dokuments im Hintergrund läuft: Tippen/Einfügen mit einem oder mehreren Cursorn). Je Stand einmal gerechnet.
    pub(super) fn head_cols(&mut self, doc: &Document, sels: &crate::editor::Selections, tab: usize) -> usize {
        let key = (doc.revision(), tab, sels.len(), sels.iter().fold(0usize, |a, s| a.wrapping_add(s.head)));
        if let Some((k, cols)) = self.heads
            && k == key
        {
            return cols;
        }
        let mut cols = 0;
        let mut last = None;
        for s in sels.iter() {
            let line = doc.char_to_line(s.head);
            if last == Some(line) {
                continue;
            }
            last = Some(line);
            cols = cols.max(line_cols(doc.line(line), tab));
        }
        self.heads = Some((key, cols));
        cols
    }
}

/// Breite (visuelle Spalten) der längsten Zeile, so wie das Textfeld zeichnet.
pub(super) fn longest_line_cols(rope: &Rope, tab: usize) -> usize {
    let tab = tab.max(1);
    let mut best = 0usize;
    for line in rope.lines() {
        // Obergrenze: ein Byte ist höchstens eine Tabbreite breit (breite Zeichen haben mindestens drei Bytes).
        if line.len_bytes().saturating_mul(tab) <= best {
            continue;
        }
        best = best.max(line_cols(line, tab));
    }
    best
}

/// Breite einer Zeile (mit oder ohne abschließendes `\n`) in visuellen Spalten.
pub(super) fn line_cols(line: RopeSlice<'_>, tab: usize) -> usize {
    let n = line.len_chars();
    let len = if n > 0 && line.char(n - 1) == '\n' { n - 1 } else { n };
    if len >= LONG_LINE {
        return len;
    }
    let mut v = 0usize;
    for chunk in line.chunks() {
        let chunk = chunk.strip_suffix('\n').unwrap_or(chunk);
        // Druckbares ASCII ist je Byte eine Spalte.
        if chunk.bytes().all(|b| (0x20..0x7f).contains(&b)) {
            v += chunk.len();
            continue;
        }
        for c in chunk.chars() {
            v += et::char_width(c, v, tab);
        }
    }
    v
}

/// Breite des Inhalts in Pixeln: längste Zeile plus Rand.
pub(super) fn content_width(cols: usize, cw: f32) -> f32 {
    (cols as f32 + MARGIN_COLS) * cw
}

/// Größter Bildlauf für `content` Pixel Inhalt in `view` Pixel Breite.
pub(super) fn max_scroll(content: f32, view: f32) -> f32 {
    (content - view).max(0.0)
}

/// Fläche der Leiste unter dem Text von `left` bis `right`, wenn `content` breiter ist als `text`; kürzt `text` dann
/// um ihre Höhe (die letzte Zeile bleibt über der Leiste).
pub(super) fn layout(text: &mut Rect, left: f32, right: f32, bottom: f32, content: f32) -> Option<Rect> {
    if content <= text.width() + 0.5 || text.height() < BAR_H * 3.0 {
        return None;
    }
    text.max.y = (bottom - BAR_H).max(text.min.y);
    Some(Rect::from_min_max(Pos2::new(left, bottom - BAR_H), Pos2::new(right.max(left + 1.0), bottom)))
}

/// Leiste in `area` zeichnen und bedienen: Griff ziehen setzt `scroll_x`, Klick neben den Griff blättert eine Seite
/// (`view` Pixel) in diese Richtung. Stil wie die senkrechte Leiste (nur der Griff, keine Bahn).
#[allow(clippy::too_many_arguments)]
pub(super) fn bar(ui: &mut Ui, area: Rect, id: Id, scroll_x: &mut f32, drag: &mut Option<f32>, content: f32, view: f32) {
    let u = style::ui();
    let max = max_scroll(content, view);
    *scroll_x = scroll_x.clamp(0.0, max);
    let track = area.shrink2(Vec2::new(2.0, 2.0));
    let w = (track.width() * (view / content.max(1.0))).clamp(MIN_THUMB.min(track.width()), track.width());
    let free = (track.width() - w).max(1.0);
    let thumb_at = |s: f32| Rect::from_min_size(Pos2::new(track.left() + free * (s / max.max(1.0)).clamp(0.0, 1.0), track.top()), Vec2::new(w, track.height()));
    let thumb = thumb_at(*scroll_x);
    let resp = ui.interact(area, id, Sense::click_and_drag());
    if let Some(p) = resp.interact_pointer_pos() {
        if resp.drag_started() {
            let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
            *drag = Some(if thumb.contains(origin) { origin.x - thumb.left() } else { w / 2.0 });
        }
        if resp.dragged()
            && let Some(grab) = *drag
        {
            *scroll_x = ((p.x - grab - track.left()) / free).clamp(0.0, 1.0) * max;
        } else if resp.clicked() && !thumb.contains(p) {
            // Klick auf die Bahn: eine Seite in Richtung des Klicks
            let page = (view * 0.9).max(1.0);
            *scroll_x = if p.x < thumb.left() { *scroll_x - page } else { *scroll_x + page }.clamp(0.0, max);
        }
    }
    if !resp.dragged() {
        *drag = None;
    }
    let hot = resp.hovered() || resp.dragged();
    ui.painter().rect_filled(thumb_at(*scroll_x), 4.0, if hot { u.text_dim.gamma_multiply(0.7) } else { u.text_dim.gamma_multiply(0.35) });
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Default);
    }
}

/// Ergebnis von [`prepare`].
pub(super) struct Layout {
    /// Fläche der Leiste (`None` = ausgeblendet, alles passt).
    pub area: Option<Rect>,
    /// Breite des Inhalts (Pixel) und größter Bildlauf für die Breite des Textfelds.
    pub content: f32,
    pub max: f32,
    /// Zeichenbreite und Breite des Textfelds.
    cw: f32,
    view: f32,
}

impl Layout {
    /// Eine Zeile mit `cols` Spalten gehört sicher zum Inhalt (z. B. gerade getippt): Inhalt und Grenze wachsen mit.
    pub(super) fn include(&mut self, cols: usize) {
        self.content = self.content.max(content_width(cols, self.cw));
        self.max = max_scroll(self.content, self.view);
    }
}

/// Vor den Eingaben eines Bildes: längste Zeile messen (bzw. den letzten Stand nehmen), bei Bedarf die Leiste unter den
/// Text legen (`text` wird um ihre Höhe kürzer) und den größten Bildlauf liefern. `extra_cols`: Breite einer Zeile, die
/// sicher zählt, auch während die Messung im Hintergrund noch läuft (Zeilen der Cursor).
#[allow(clippy::too_many_arguments)]
pub(super) fn prepare(width: &mut DocWidth, hs: &mut HScroll, ctx: &egui::Context, doc: &Document, tab: usize, cw: f32, extra_cols: usize, text: &mut Rect, left: f32, right: f32, bottom: f32) -> Layout {
    let cols = width.longest_cols(doc, tab, ctx).max(extra_cols);
    let content = content_width(cols, cw);
    let area = layout(text, left, right, bottom, content);
    hs.last_bar = area;
    Layout { area, content, max: max_scroll(content, text.width()), cw, view: text.width() }
}
