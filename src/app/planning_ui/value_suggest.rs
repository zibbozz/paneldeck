//! Vorschläge für Label-, Tag- und ähnliche Felder (#239): Beim Tippen erscheinen die schon benutzten Werte mit der Zahl
//! ihrer Verwendungen, z. B. `backend (12)` – ohne Beachtung der Groß-/Kleinschreibung nach dem Getippten gefiltert, Treffer
//! am Wortanfang zuerst, dann nach Häufigkeit und Name. Ein Vorschlag lässt sich übernehmen, neue Werte bleiben erlaubt.
//!
//! * Reine Logik (ohne egui, getestet): [`count_values`] (Zählen und Zusammenfassen), [`filter`], [`token_at`] /
//!   [`replace_token`] / [`list_items`] für kommagetrennte Felder.
//! * [`CountCache`]: Zwischenspeicher der Zählungen mit Stempel (z. B. `Connection::total_changes`), damit nicht jedes Bild
//!   die Datenbank fragt; [`cached_per_open`] rechnet für Felder ohne Datenbank (Sitzungs-Tags) einmal je Öffnen.
//! * [`suggest_edit`]: das Eingabefeld samt Popup, Tastatur wie im Abfrage-Editor (↑/↓ wählen, Tab/Enter übernehmen, Esc
//!   schließt, ↓ oder Strg+Leertaste öffnet). Das Popup meldet sich als egui-Popup an, damit Dialoge Esc nicht selbst nehmen.

use super::query_bar::popup_order;
use super::query_logic::{byte_of, char_of, clamp_boundary};
use crate::state::Lang;
use crate::style;
use eframe::egui::{self, text::CCursor, text::CCursorRange, FontId, Id, Key, Modifiers, Pos2, Rect, Response, RichText, Sense, Ui, Vec2};
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::Hash;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

/// So viele Vorschläge zeigt das Popup höchstens (der Rest ist über das Tippen erreichbar).
pub const MAX_SHOWN: usize = 50;

/// Ein benutzter Wert und wie oft er vorkommt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Counted {
    pub name: String,
    pub count: usize,
}

impl Counted {
    /// Anzeige wie `backend (12)`.
    pub fn label(&self) -> String {
        format!("{} ({})", self.name, self.count)
    }
}

/// Vergleichsschlüssel: ohne Ränder und führendes `#`, klein geschrieben.
pub fn fold(s: &str) -> String {
    s.trim().trim_start_matches('#').trim().to_lowercase()
}

/// Zählt Werte (`(Name, Anzahl)`; für Einzelvorkommen Anzahl 1). Gleiche Namen ohne Beachtung der Groß-/Kleinschreibung werden
/// zusammengezählt; gezeigt wird die häufigste Schreibweise (bei Gleichstand die zuerst gesehene). Leere Namen fallen weg.
/// Ergebnis nach Anzahl absteigend, dann nach Name sortiert.
pub fn count_values<I, S>(values: I) -> Vec<Counted>
where
    I: IntoIterator<Item = (S, usize)>,
    S: AsRef<str>,
{
    // Schlüssel → (Reihenfolge, Schreibweisen mit Anzahl in Reihenfolge des ersten Auftretens)
    let mut map: HashMap<String, (usize, Vec<(String, usize)>)> = HashMap::new();
    for (name, n) in values {
        let name = name.as_ref().trim();
        let key = fold(name);
        if key.is_empty() {
            continue;
        }
        let next = map.len();
        let e = map.entry(key).or_insert((next, Vec::new()));
        match e.1.iter_mut().find(|(s, _)| s == name) {
            Some(v) => v.1 += n,
            None => e.1.push((name.to_string(), n)),
        }
    }
    let mut out: Vec<Counted> = map
        .into_values()
        .map(|(_, variants)| {
            let count = variants.iter().map(|v| v.1).sum();
            let mut best = &variants[0];
            for v in &variants[1..] {
                if v.1 > best.1 {
                    best = v;
                }
            }
            Counted { name: best.0.clone(), count }
        })
        .collect();
    sort(&mut out);
    out
}

/// Nach Anzahl absteigend, dann nach Name (ohne Beachtung der Groß-/Kleinschreibung).
pub fn sort(v: &mut [Counted]) {
    v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())).then_with(|| a.name.cmp(&b.name)));
}

/// Die Vorschläge zum Getippten: Werte, die `typed` enthalten (ohne Beachtung der Groß-/Kleinschreibung, leer = alle), ohne
/// die schon gewählten (`exclude`), höchstens `limit`. Werte, die mit dem Getippten beginnen, kommen zuerst, danach die, die
/// es nur enthalten; innerhalb davon nach Häufigkeit, dann Name (wie [`sort`]).
pub fn filter<'a>(all: &'a [Counted], typed: &str, exclude: &[String], limit: usize) -> Vec<&'a Counted> {
    let q = fold(typed);
    let skip: Vec<String> = exclude.iter().map(|e| fold(e)).collect();
    let mut hits: Vec<(bool, &Counted)> = all
        .iter()
        .filter_map(|c| {
            let k = fold(&c.name);
            (k.contains(&q) && !skip.contains(&k)).then(|| (!k.starts_with(&q), c))
        })
        .collect();
    hits.sort_by(|(pa, a), (pb, b)| pa.cmp(pb).then_with(|| b.count.cmp(&a.count)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())).then_with(|| a.name.cmp(&b.name)));
    hits.into_iter().take(limit).map(|(_, c)| c).collect()
}

// ------------------------------------------------------------------ kommagetrennte Felder

/// Bereich des Eintrags unter dem Cursor in einer Liste „a, b, c“ (zwischen den Kommas, ohne Leerraum am Rand).
pub fn token_at(text: &str, cursor: usize) -> Range<usize> {
    let c = clamp_boundary(text, cursor);
    let start = text[..c].rfind(',').map(|i| i + 1).unwrap_or(0);
    let end = text[c..].find(',').map(|i| c + i).unwrap_or(text.len());
    let seg = &text[start..end];
    let lead = seg.len() - seg.trim_start().len();
    let trail = seg.len() - seg.trim_end().len();
    let (s, e) = (start + lead, end - trail);
    if s > e { c..c } else { s..e }
}

/// Der getippte Teil des Eintrags unter dem Cursor (ohne führendes `#`).
pub fn typed_at(text: &str, cursor: usize) -> String {
    let r = token_at(text, cursor);
    text[r].trim_start_matches('#').trim().to_string()
}

/// Ersetzt den Eintrag unter dem Cursor durch `name`; steht er am Ende, folgt „, “ für den nächsten. Ergebnis: neuer Text
/// und Cursor dahinter.
pub fn replace_token(text: &str, cursor: usize, name: &str) -> (String, usize) {
    let r = token_at(text, cursor);
    let rest = text[r.end..].trim_start();
    let at_end = rest.is_empty();
    let mut out = String::with_capacity(text.len() + name.len() + 2);
    out.push_str(&text[..r.start]);
    if r.start > 0 && !out.ends_with(' ') {
        out.push(' ');
    }
    out.push_str(name);
    if at_end {
        out.push_str(", ");
        let c = out.len();
        return (out, c);
    }
    let c = out.len();
    out.push_str(&text[r.end..]);
    (out, c)
}

/// Die Einträge einer Liste „a, #b, c“ ohne den unter dem Cursor (der wird gerade getippt).
pub fn list_items(text: &str, cursor: Option<usize>) -> Vec<String> {
    let skip = cursor.map(|c| token_at(text, c));
    let mut out = Vec::new();
    let mut start = 0;
    for part in text.split(',') {
        let r = start..start + part.len();
        start = r.end + 1;
        if skip.as_ref().is_some_and(|s| s.start >= r.start && s.end <= r.end) {
            continue;
        }
        let t = part.trim().trim_start_matches('#').trim();
        if !t.is_empty() {
            out.push(t.to_string());
        }
    }
    out
}

// ------------------------------------------------------------------ Zwischenspeicher

/// Zählungen je Schlüssel (z. B. Projekt und Feld), gültig solange der Stempel gleich bleibt. Innen veränderlich, damit
/// Eingaben mit geteiltem Kontext (`&InCx`) ihn nutzen können.
pub struct CountCache<K> {
    map: RefCell<HashMap<K, (u64, Rc<Vec<Counted>>)>>,
}

impl<K> Default for CountCache<K> {
    fn default() -> Self {
        CountCache { map: RefCell::new(HashMap::new()) }
    }
}

impl<K: Hash + Eq> CountCache<K> {
    /// Die Zählung für `key`; `load` läuft nur, wenn es keine gibt oder sich der Stempel geändert hat.
    pub fn get(&self, key: K, stamp: u64, load: impl FnOnce() -> Vec<Counted>) -> Rc<Vec<Counted>> {
        if let Some((s, v)) = self.map.borrow().get(&key) {
            if *s == stamp {
                return v.clone();
            }
        }
        let v = Rc::new(load());
        self.map.borrow_mut().insert(key, (stamp, v.clone()));
        v
    }

    /// Alles verwerfen (Daten haben sich außerhalb geändert).
    pub fn clear(&self) {
        self.map.borrow_mut().clear();
    }
}

/// Für Felder ohne Datenbank: Werte einmal je Öffnen zählen. Neu gezählt wird, wenn das Feld im letzten Bild nicht zu sehen
/// war (Dialog neu geöffnet).
pub fn cached_per_open(ctx: &egui::Context, id: Id, load: impl FnOnce() -> Vec<Counted>) -> Arc<Vec<Counted>> {
    // In egui-Daten (Send + Sync) als geteilte Liste mit der Nummer des Bildes, in dem sie zuletzt gebraucht wurde – so
    // kostet ein Bild nur das Zählen einer Referenz, nicht das Kopieren aller Namen.
    let key = id.with("value_suggest_values");
    let frame = ctx.cumulative_pass_nr();
    let prev: Option<(u64, Arc<Vec<Counted>>)> = ctx.data(|d| d.get_temp::<(u64, Arc<Vec<Counted>>)>(key));
    let v = match prev {
        Some((f, v)) if f + 1 >= frame => v,
        _ => Arc::new(load()),
    };
    ctx.data_mut(|d| d.insert_temp(key, (frame, v.clone())));
    v
}

// ------------------------------------------------------------------ Eingabefeld mit Popup

/// Wie das Feld gefüllt wird.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Das Feld hält genau einen neuen Wert; ein gewählter Vorschlag wird gemeldet ([`SuggestOut::picked`]), der Aufrufer
    /// fügt ihn hinzu (das Feld bleibt unverändert).
    Single,
    /// Das Feld hält eine kommagetrennte Liste; ein gewählter Vorschlag ersetzt den Eintrag unter dem Cursor.
    List,
}

/// Was das Feld in diesem Bild gemeldet hat.
pub struct SuggestOut {
    pub response: Response,
    /// Übernommener Vorschlag.
    pub picked: Option<String>,
}

#[derive(Clone, Default)]
struct PopState {
    open: bool,
    sel: Option<usize>,
    rect: Option<Rect>,
    scroll: bool,
    cursor: usize,
    /// Das Popup war im letzten Bild zu sehen.
    shown: bool,
}

/// Würde das Feld `id` ein Enter in diesem Bild selbst nehmen (Popup offen und ein Vorschlag gewählt)? Für Aufrufer, die
/// Enter vor dem Feld auswerten.
pub fn wants_enter(ctx: &egui::Context, id: Id) -> bool {
    ctx.data(|d| d.get_temp::<PopState>(id.with("value_suggest_state"))).is_some_and(|s| s.shown && s.open && s.sel.is_some())
}

fn popup_id(id: Id) -> Id {
    id.with("value_suggest_popup")
}

/// Cursor (Byte-Offset) des Textfelds.
fn cursor_of(ctx: &egui::Context, id: Id, text: &str) -> Option<usize> {
    egui::TextEdit::load_state(ctx, id).and_then(|s| s.cursor.char_range()).map(|r| byte_of(text, r.primary.index))
}

fn set_cursor(ctx: &egui::Context, id: Id, text: &str, byte: usize) {
    let mut s = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    s.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(char_of(text, byte)))));
    s.store(ctx, id);
}

/// Der getippte Eintrag (im Listenmodus der unter dem Cursor), ohne Ränder und `#`.
fn typed_now(text: &str, cursor: usize, mode: Mode) -> String {
    match mode {
        Mode::Single => text.trim().trim_start_matches('#').trim().to_string(),
        Mode::List => typed_at(text, cursor),
    }
}

/// Die Vorschläge zum aktuellen Stand.
fn candidates(values: &[Counted], text: &str, cursor: usize, exclude: &[String], mode: Mode) -> Vec<Counted> {
    let typed = typed_now(text, cursor, mode);
    let mut skip = match mode {
        Mode::Single => Vec::new(),
        Mode::List => list_items(text, Some(cursor)),
    };
    skip.extend(exclude.iter().cloned());
    filter(values, &typed, &skip, MAX_SHOWN).into_iter().cloned().collect()
}

/// Nimmt die Taste `key`, aber nur ohne jede Zusatztaste. (`consume_key(Modifiers::NONE, …)` vergleicht „logisch“ und nähme
/// auch Umschalt+Tab oder Umschalt+Enter.)
fn take_plain(i: &mut egui::InputState, key: Key) -> bool {
    let before = i.events.len();
    i.events.retain(|e| !matches!(e, egui::Event::Key { key: k, pressed: true, modifiers, .. } if *k == key && modifiers.is_none()));
    i.events.len() != before
}

/// Ein Eingabefeld mit Vorschlägen. `edit` zeichnet das Textfeld (Größe, Hinweis, Rand nach Wahl des Aufrufers) und muss
/// ihm die Kennung `id` geben – z. B. `|ui, te| ui.add(te.desired_width(200.0))`. `exclude`: schon gewählte Werte.
///
/// Tab übernimmt nur, wenn etwas getippt oder ein Vorschlag mit ↑/↓ gewählt ist; sonst (und bei Umschalt+Tab) wandert der
/// Fokus wie gewohnt weiter. Enter übernimmt nur einen gewählten Vorschlag, Umschalt+Enter nie.
#[allow(clippy::too_many_arguments)]
pub fn suggest_edit(ui: &mut Ui, id: Id, text: &mut String, values: &[Counted], exclude: &[String], mode: Mode, lang: Lang, edit: impl FnOnce(&mut Ui, egui::TextEdit<'_>) -> Response) -> SuggestOut {
    let ctx = ui.ctx().clone();
    let skey = id.with("value_suggest_state");
    let mut st: PopState = ctx.data(|d| d.get_temp(skey)).unwrap_or_default();
    let focused_before = ctx.memory(|m| m.has_focus(id));
    let cur0 = cursor_of(&ctx, id, text).unwrap_or(st.cursor).min(text.len());

    // ---- Tasten, bevor das Textfeld sie sieht (Vorschläge nur rechnen, wenn das Feld den Fokus hat)
    let mut accept: Option<usize> = None;
    let mut cands0: Vec<Counted> = Vec::new();
    if focused_before {
        cands0 = candidates(values, text, cur0, exclude, mode);
        let n = cands0.len();
        let visible = st.open && n > 0;
        let typed_empty = typed_now(text, cur0, mode).is_empty();
        let mut focus_move = None;
        ctx.input_mut(|i| {
            if visible {
                if take_plain(i, Key::ArrowDown) {
                    st.sel = Some(st.sel.map_or(0, |s| (s + 1) % n));
                    st.scroll = true;
                }
                if take_plain(i, Key::ArrowUp) {
                    st.sel = Some(st.sel.map_or(n - 1, |s| if s == 0 { n - 1 } else { s - 1 }));
                    st.scroll = true;
                }
                // Tab übernimmt nur Getipptes oder eine ausdrückliche Wahl; bei leerem Eintrag geht der Fokus weiter.
                if (st.sel.is_some() || !typed_empty) && take_plain(i, Key::Tab) {
                    accept = Some(st.sel.unwrap_or(0));
                }
                if take_plain(i, Key::Escape) {
                    st.open = false;
                    st.sel = None;
                }
                if st.sel.is_some() && take_plain(i, Key::Enter) {
                    accept = st.sel;
                }
            } else if n > 0 && take_plain(i, Key::ArrowDown) {
                st.open = true;
                st.sel = Some(0);
            }
            if i.consume_key(Modifiers::CTRL, Key::Space) {
                st.open = true;
            }
            // Das Feld sperrt Tab für egui (Sperre unten); ein nicht übernommenes Tab bewegt den Fokus deshalb selbst.
            if accept.is_none() {
                focus_move = i.events.iter().find_map(|e| match e {
                    egui::Event::Key { key: Key::Tab, pressed: true, modifiers, .. } if !modifiers.ctrl && !modifiers.alt && !modifiers.command => {
                        Some(if modifiers.shift { egui::FocusDirection::Previous } else { egui::FocusDirection::Next })
                    }
                    _ => None,
                });
            }
        });
        if accept.is_some() {
            // falls egui das Tab schon als Fokuswechsel vorgemerkt hat (Sperre noch nicht aktiv): verwerfen
            ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
        } else if let Some(dir) = focus_move {
            ctx.memory_mut(|m| m.move_focus(dir));
            st.open = false;
            st.sel = None;
        }
    }
    let mut picked: Option<String> = None;
    if let Some(c) = accept.and_then(|i| cands0.get(i)) {
        picked = Some(c.name.clone());
        st.sel = None;
        // nach dem Übernehmen zu: ein weiteres Tab auf dem leeren Eintrag wandert weiter, statt erneut zu übernehmen
        st.open = false;
        if mode == Mode::List {
            let (t, cur) = replace_token(text, cur0, &c.name);
            *text = t;
            set_cursor(&ctx, id, text, cur);
            st.cursor = cur;
        }
    }

    // ---- Das Textfeld
    let response = edit(ui, egui::TextEdit::singleline(text).id(id));
    let focused = response.has_focus();
    let cursor = cursor_of(&ctx, id, text).unwrap_or(text.len()).min(text.len());
    if response.changed() {
        // leer getippt (alles gelöscht, im Listenmodus ein neuer Eintrag nach dem Komma): nicht alle Werte aufdrängen
        st.open = !typed_now(text, cursor, mode).is_empty();
        st.sel = None;
    } else if cursor != st.cursor && accept.is_none() {
        // Cursor ohne Tippen bewegt (Klick, Pfeile): Vorschläge nicht aufdrängen.
        st.open = false;
    }
    st.cursor = cursor;
    // Ohne Fokus bleibt das Popup nur, solange die Maus darauf gedrückt wird (der Klick auf einen Vorschlag nimmt dem Feld
    // den Fokus erst beim Loslassen): so bleibt es nie hängen und hält auch kein Esc eines Dialogs auf.
    let pressing_popup = st.rect.is_some_and(|r| ctx.input(|i| (i.pointer.any_down() || i.pointer.any_released()) && i.pointer.interact_pos().is_some_and(|p| r.contains(p))));
    if !focused && !pressing_popup {
        st.open = false;
    }
    if ctx.input(|i| i.pointer.any_pressed()) {
        let pos = ctx.input(|i| i.pointer.interact_pos());
        if pos.is_some_and(|p| !response.rect.contains(p) && !st.rect.is_some_and(|r| r.contains(p))) {
            st.open = false;
        }
    }

    // ---- Popup
    let live = st.open && (focused || pressing_popup);
    let cands = if live { candidates(values, text, cursor, exclude, mode) } else { Vec::new() };
    if st.sel.is_some_and(|i| i >= cands.len()) {
        st.sel = None;
    }
    let pid = popup_id(id);
    let mut clicked: Option<usize> = None;
    let shown = live && !cands.is_empty();
    if focused {
        // Tab gehört dem Feld (übernehmen oder Fokus oben selbst weitergeben), Esc nur bei offenem Popup: dann schließt es
        // nur das Popup und nimmt dem Feld nicht den Fokus.
        ctx.memory_mut(|m| m.set_focus_lock_filter(id, egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: shown }));
    }
    if shown {
        // jedes Bild neu anmelden (hält es für egui offen, `Popup::is_any_open` sieht es)
        egui::Popup::open_id(&ctx, pid);
        let pos = response.rect.left_bottom() + Vec2::new(0.0, 2.0);
        let width = response.rect.width().max(220.0);
        st.rect = Some(popup(&ctx, pid, pos, popup_order(ui), width, &cands, &mut st, lang, &mut clicked));
        st.shown = true;
    } else {
        if egui::Popup::is_id_open(&ctx, pid) {
            egui::Popup::close_id(&ctx, pid);
        }
        st.rect = None;
        st.shown = false;
    }
    if let Some(c) = clicked.and_then(|i| cands.get(i)) {
        picked = Some(c.name.clone());
        st.sel = None;
        st.open = false;
        if mode == Mode::List {
            let (t, cur) = replace_token(text, cursor, &c.name);
            *text = t;
            set_cursor(&ctx, id, text, cur);
            st.cursor = cur;
        }
        ctx.memory_mut(|m| m.request_focus(id));
        ctx.request_repaint();
    }
    ctx.data_mut(|d| d.insert_temp(skey, st));
    SuggestOut { response, picked }
}

/// Das Popup; gibt sein Rechteck zurück, ein Klick landet in `clicked`.
#[allow(clippy::too_many_arguments)]
fn popup(ctx: &egui::Context, pid: Id, pos: Pos2, order: egui::Order, width: f32, items: &[Counted], st: &mut PopState, lang: Lang, clicked: &mut Option<usize>) -> Rect {
    let scroll = std::mem::take(&mut st.scroll);
    let area = egui::Area::new(pid).order(order).fixed_pos(pos).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(4)).show(ui, |ui| {
            ui.set_width(width);
            egui::ScrollArea::vertical().max_height(220.0).auto_shrink([true, true]).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                for (i, c) in items.iter().enumerate() {
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::click());
                    if st.sel == Some(i) {
                        ui.painter().rect_filled(rect, 4.0, style::accent().gamma_multiply(0.35));
                        if scroll {
                            ui.scroll_to_rect(rect, None);
                        }
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, 4.0, ui.visuals().widgets.hovered.bg_fill);
                    }
                    let name = ui.painter().layout_no_wrap(c.name.clone(), FontId::proportional(13.0), ui.visuals().text_color());
                    let x = rect.min.x + 6.0;
                    let name_w = name.size().x;
                    ui.painter().galley(Pos2::new(x, rect.center().y - name.size().y / 2.0), name, ui.visuals().text_color());
                    ui.painter().text(Pos2::new(x + name_w + 4.0, rect.center().y), egui::Align2::LEFT_CENTER, format!("({})", c.count), FontId::proportional(12.0), style::dim(ui));
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, c.label()));
                    if resp.clicked() {
                        *clicked = Some(i);
                    }
                }
            });
            ui.add_space(2.0);
            ui.label(RichText::new(lang.t("↑↓ wählen · Tab/Enter übernehmen · Esc schließen", "↑↓ choose · Tab/Enter accept · Esc close")).size(10.5).color(style::dim(ui)));
        });
    });
    area.response.rect
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(name: &str, count: usize) -> Counted {
        Counted { name: name.into(), count }
    }

    #[test]
    fn counting_merges_case_insensitively_and_sorts_by_count_then_name() {
        let v = count_values([("backend", 1), ("UI", 1), ("Backend", 1), ("backend", 1), ("api", 1), ("", 1), ("  ", 1), ("Zeta", 1), ("ui", 1)]);
        assert_eq!(v, vec![c("backend", 3), c("UI", 2), c("api", 1), c("Zeta", 1)], "häufigste Schreibweise, Gleichstand nach Name");
        // Anzahlen aus einer gruppierten Abfrage werden addiert
        let v = count_values([("x", 4), ("X", 5), ("y", 9)]);
        assert_eq!(v, vec![c("X", 9), c("y", 9)], "Gleichstand der Anzahl: nach Name");
        // Gleichstand der Schreibweisen: die zuerst gesehene
        assert_eq!(count_values([("Ab", 1), ("aB", 1)]), vec![c("Ab", 2)]);
        // „#tag“ und „tag“ sind derselbe Wert, Ränder zählen nicht
        assert_eq!(count_values([("#ops", 1), (" ops ", 2)]), vec![c("ops", 3)]);
        assert!(count_values(Vec::<(String, usize)>::new()).is_empty());
    }

    #[test]
    fn sort_orders_umlauts_and_case_stably() {
        let mut v = vec![c("b", 1), c("Ä", 1), c("a", 1), c("B", 2)];
        sort(&mut v);
        assert_eq!(v, vec![c("B", 2), c("a", 1), c("b", 1), c("Ä", 1)]);
    }

    #[test]
    fn label_shows_the_count_in_parentheses() {
        assert_eq!(c("backend", 12).label(), "backend (12)");
    }

    #[test]
    fn filter_matches_case_insensitively_keeps_order_and_skips_chosen() {
        let all = count_values([("backend", 12), ("Frontend", 3), ("ops", 7), ("Bäckerei", 1)]);
        let names = |v: Vec<&Counted>| v.into_iter().map(|c| c.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(filter(&all, "", &[], 10)), vec!["backend", "ops", "Frontend", "Bäckerei"]);
        assert_eq!(names(filter(&all, "END", &[], 10)), vec!["backend", "Frontend"]);
        assert_eq!(names(filter(&all, "  #end ", &[], 10)), vec!["backend", "Frontend"], "Ränder und # zählen nicht");
        assert_eq!(names(filter(&all, "BÄCK", &[], 10)), vec!["Bäckerei"], "Umlaute");
        assert_eq!(names(filter(&all, "end", &["BACKEND".into()], 10)), vec!["Frontend"], "schon gewählte fehlen");
        assert_eq!(names(filter(&all, "", &[], 2)), vec!["backend", "ops"]);
        assert!(filter(&all, "xyz", &[], 10).is_empty());
    }

    #[test]
    fn tokens_of_a_comma_list() {
        let t = "a, bb ,  #cc";
        assert_eq!(&t[token_at(t, 0)], "a");
        assert_eq!(&t[token_at(t, 4)], "bb");
        assert_eq!(&t[token_at(t, t.len())], "#cc");
        assert_eq!(typed_at(t, t.len()), "cc");
        assert_eq!(token_at("a, ", 3), 3..3);
        assert_eq!(token_at("", 0), 0..0);
        assert_eq!(token_at("ä,ö", 1), 0..2, "Offset mitten im Zeichen");
        assert_eq!(list_items("a, #b,, c ", None), vec!["a", "b", "c"]);
        assert_eq!(list_items("a, b, c", Some(4)), vec!["a", "c"], "der Eintrag unter dem Cursor fehlt");
        assert_eq!(list_items("a, ", Some(3)), vec!["a"]);
    }

    #[test]
    fn replacing_the_token_adds_a_separator_at_the_end() {
        assert_eq!(replace_token("", 0, "backend"), ("backend, ".to_string(), 9));
        assert_eq!(replace_token("ops, ba", 7, "backend"), ("ops, backend, ".to_string(), 14));
        assert_eq!(replace_token("ops,ba", 6, "backend"), ("ops, backend, ".to_string(), 14));
        // in der Mitte: der Rest bleibt, kein zusätzliches Komma
        let (t, c) = replace_token("ops, ba, ui", 7, "backend");
        assert_eq!(t, "ops, backend, ui");
        assert_eq!(&t[..c], "ops, backend");
        // Leerraum hinter dem letzten Eintrag zählt als Ende
        assert_eq!(replace_token("fr  ", 2, "Frontend").0, "Frontend, ");
        assert_eq!(replace_token("#ba", 3, "backend").0, "backend, ", "das # wird mit ersetzt");
    }

    #[test]
    fn candidates_in_both_modes() {
        let all = count_values([("backend", 3), ("ops", 2), ("ui", 1)]);
        let names = |v: Vec<Counted>| v.into_iter().map(|c| c.name).collect::<Vec<_>>();
        assert_eq!(names(candidates(&all, "o", 1, &["ops".into()], Mode::Single)), Vec::<String>::new());
        assert_eq!(names(candidates(&all, "", 0, &[], Mode::Single)), vec!["backend", "ops", "ui"]);
        // Liste: schon eingetragene fehlen, gefiltert wird nach dem Eintrag unter dem Cursor
        assert_eq!(names(candidates(&all, "ops, ", 5, &[], Mode::List)), vec!["backend", "ui"]);
        assert_eq!(names(candidates(&all, "ops, U", 6, &[], Mode::List)), vec!["ui"]);
    }

    #[test]
    fn cache_reloads_only_when_the_stamp_changes() {
        let cache: CountCache<(i64, String)> = CountCache::default();
        let mut loads = 0;
        let get = |stamp: u64, loads: &mut i32| cache.get((1, "labels".into()), stamp, || { *loads += 1; vec![c("a", 1)] });
        assert_eq!(get(5, &mut loads).len(), 1);
        get(5, &mut loads);
        get(5, &mut loads);
        assert_eq!(loads, 1, "gleicher Stempel: aus dem Speicher");
        get(6, &mut loads);
        assert_eq!(loads, 2, "neuer Stempel: neu gezählt");
        cache.clear();
        get(6, &mut loads);
        assert_eq!(loads, 3, "nach clear neu gezählt");
    }

    // ---- Headless-Läufe des Feldes (egui ohne Fenster)

    use eframe::egui::{Event, RawInput};

    struct H {
        ctx: egui::Context,
        text: String,
        values: Vec<Counted>,
        mode: Mode,
        t: f64,
        picked: Vec<String>,
        /// Ein zweites Feld dahinter, damit Tab den Fokus weitergeben kann.
        other: String,
    }

    impl H {
        fn new(mode: Mode) -> H {
            let mut h = H { ctx: egui::Context::default(), text: String::new(), values: count_values([("backend", 12), ("ops", 7), ("Frontend", 3)]), mode, t: 0.0, picked: Vec::new(), other: String::new() };
            h.ctx.memory_mut(|m| m.request_focus(Id::new("vs_field")));
            h.frame(vec![]);
            h.frame(vec![]);
            h
        }

        fn frame(&mut self, events: Vec<Event>) -> bool {
            self.t += 0.05;
            let modifiers = events.iter().find_map(|e| if let Event::Key { modifiers, .. } = e { Some(*modifiers) } else { None }).unwrap_or_default();
            let raw = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), events, focused: true, time: Some(self.t), modifiers, ..Default::default() };
            let (text, values, mode, other) = (&mut self.text, &self.values, self.mode, &mut self.other);
            let mut out = None;
            let _ = self.ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    out = Some(suggest_edit(ui, Id::new("vs_field"), text, values, &[], mode, Lang::En, |ui, te| ui.add(te.desired_width(200.0))));
                    ui.add_space(300.0);
                    ui.add(egui::TextEdit::singleline(other).id(Id::new("other")));
                });
            });
            let out = out.unwrap();
            if let Some(p) = out.picked {
                self.picked.push(p);
            }
            egui::Popup::is_any_open(&self.ctx)
        }

        fn key(&mut self, key: Key) -> bool {
            self.key_mod(key, Modifiers::NONE)
        }

        fn key_mod(&mut self, key: Key, modifiers: Modifiers) -> bool {
            self.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }]);
            self.frame(vec![Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers }])
        }

        fn focused(&self) -> bool {
            self.ctx.memory(|m| m.has_focus(Id::new("vs_field")))
        }

        fn other_focused(&self) -> bool {
            self.ctx.memory(|m| m.has_focus(Id::new("other")))
        }

        fn press(&mut self, pos: Pos2, pressed: bool) -> bool {
            self.frame(vec![Event::PointerMoved(pos), Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE }])
        }

        fn st(&self) -> PopState {
            self.ctx.data(|d| d.get_temp(Id::new("vs_field").with("value_suggest_state"))).unwrap_or_default()
        }
    }

    #[test]
    fn typing_opens_the_popup_and_tab_picks_in_single_mode() {
        let mut h = H::new(Mode::Single);
        h.frame(vec![Event::Text("END".into())]);
        assert!(h.frame(vec![]), "Popup offen und als egui-Popup angemeldet");
        assert!(h.st().open);
        h.key(Key::Tab);
        assert_eq!(h.picked, vec!["backend"], "Tab übernimmt den ersten (häufigsten) Treffer");
        assert_eq!(h.text, "END", "im Einzelmodus ändert das Feld nichts, der Aufrufer fügt hinzu");
        assert!(!h.st().open);
    }

    #[test]
    fn arrows_choose_enter_accepts_and_escape_closes() {
        let mut h = H::new(Mode::Single);
        h.frame(vec![Event::Text("e".into())]);
        // Treffer: backend (12), Frontend (3)
        h.key(Key::ArrowDown);
        h.key(Key::ArrowDown);
        assert_eq!(h.st().sel, Some(1));
        h.key(Key::ArrowUp);
        h.key(Key::ArrowUp);
        assert_eq!(h.st().sel, Some(1), "nach oben über den Anfang hinaus: der letzte");
        h.key(Key::Enter);
        assert_eq!(h.picked, vec!["Frontend"]);
        assert_eq!(h.text, "e", "Enter nach der Auswahl übernimmt nur");
        // neu tippen, Esc schließt nur das Popup
        h.text.clear();
        h.frame(vec![Event::Text("o".into())]);
        assert!(h.st().open);
        let open = h.key(Key::Escape);
        assert!(!h.st().open && !open);
        assert!(h.ctx.memory(|m| m.has_focus(Id::new("vs_field"))), "das Feld behält den Fokus");
        // Pfeil runter öffnet wieder
        h.key(Key::ArrowDown);
        assert!(h.st().open);
        assert_eq!(h.st().sel, Some(0));
    }

    #[test]
    fn list_mode_replaces_the_current_entry_and_keeps_going() {
        let mut h = H::new(Mode::List);
        h.frame(vec![Event::Text("ops, fr".into())]);
        h.key(Key::Tab);
        assert_eq!(h.text, "ops, Frontend, ");
        assert_eq!(h.picked, vec!["Frontend"]);
        // weiter: der nächste Eintrag, gewählte fehlen in den Vorschlägen
        h.frame(vec![Event::Text("b".into())]);
        assert_eq!(h.text, "ops, Frontend, b");
        h.key(Key::Tab);
        assert_eq!(h.text, "ops, Frontend, backend, ");
    }

    #[test]
    fn values_are_counted_once_per_opening() {
        let ctx = egui::Context::default();
        let id = Id::new("tags");
        let mut loads = 0;
        for _ in 0..3 {
            let _ = ctx.run(RawInput::default(), |ctx| {
                cached_per_open(ctx, id, || { loads += 1; vec![c("a", 1)] });
            });
        }
        assert_eq!(loads, 1, "solange das Feld zu sehen ist: einmal gezählt");
        let _ = ctx.run(RawInput::default(), |_| {});
        let _ = ctx.run(RawInput::default(), |ctx| {
            cached_per_open(ctx, id, || { loads += 1; vec![c("a", 1)] });
        });
        assert_eq!(loads, 2, "nach einem Bild ohne Feld (neu geöffnet): neu gezählt");
    }

    #[test]
    fn tab_on_an_empty_entry_moves_focus_instead_of_accepting() {
        // Strg+Leertaste zeigt alle Werte, aber ohne Getipptes und ohne Wahl geht Tab weiter
        let mut h = H::new(Mode::Single);
        h.frame(vec![Event::Key { key: Key::Space, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::CTRL }]);
        assert!(h.frame(vec![]) && h.st().open, "Strg+Leertaste öffnet");
        h.key(Key::Tab);
        assert!(h.picked.is_empty(), "nichts übernommen");
        assert!(h.other_focused(), "der Fokus wandert zum nächsten Feld");
        assert!(!h.st().open);
        // mit ausdrücklicher Wahl übernimmt Tab dagegen
        let mut h = H::new(Mode::Single);
        h.key(Key::ArrowDown);
        assert_eq!(h.st().sel, Some(0));
        h.key(Key::Tab);
        assert_eq!(h.picked, vec!["backend"]);
        assert!(h.focused(), "nach dem Übernehmen bleibt der Fokus im Feld");
    }

    #[test]
    fn tab_without_popup_moves_focus() {
        let mut h = H::new(Mode::Single);
        h.key(Key::Tab);
        assert!(h.other_focused() && h.picked.is_empty());
    }

    #[test]
    fn list_mode_closes_after_accepting_so_tab_moves_on() {
        let mut h = H::new(Mode::List);
        h.frame(vec![Event::Text("ops, fr".into())]);
        h.key(Key::Tab);
        assert_eq!(h.text, "ops, Frontend, ");
        assert!(!h.st().open, "nach dem Übernehmen zu");
        assert!(!h.frame(vec![]), "kein Popup angemeldet");
        h.key(Key::Tab);
        assert_eq!(h.picked, vec!["Frontend"], "kein zweites Übernehmen auf dem leeren Eintrag");
        assert_eq!(h.text, "ops, Frontend, ");
        assert!(h.other_focused(), "Tab gibt den Fokus weiter");
    }

    #[test]
    fn clearing_the_text_does_not_offer_all_values() {
        let mut h = H::new(Mode::Single);
        h.frame(vec![Event::Text("b".into())]);
        assert!(h.st().open);
        h.key(Key::Backspace);
        assert_eq!(h.text, "");
        assert!(!h.st().open && !h.frame(vec![]), "leer getippt: kein Popup");
        h.key(Key::Tab);
        assert!(h.picked.is_empty() && h.other_focused());
        // Listenmodus: ein neuer, leerer Eintrag nach dem Komma öffnet ebenfalls nichts
        let mut h = H::new(Mode::List);
        h.frame(vec![Event::Text("ops,".into())]);
        assert!(!h.st().open);
        h.frame(vec![Event::Text(" ".into())]);
        assert!(!h.st().open);
        h.frame(vec![Event::Text("u".into())]);
        assert!(h.st().open, "erst Getipptes öffnet");
    }

    #[test]
    fn shift_tab_and_shift_enter_never_accept() {
        let mut h = H::new(Mode::Single);
        h.frame(vec![Event::Text("e".into())]);
        h.key(Key::ArrowDown);
        assert_eq!(h.st().sel, Some(0));
        h.key_mod(Key::Enter, Modifiers::SHIFT);
        assert!(h.picked.is_empty(), "Umschalt+Enter übernimmt nicht");
        // Umschalt+Tab: nichts übernommen, das Popup schließt, der Fokus verlässt das Feld
        let mut h = H::new(Mode::Single);
        h.frame(vec![Event::Text("e".into())]);
        h.key(Key::ArrowDown);
        h.key_mod(Key::Tab, Modifiers::SHIFT);
        assert!(h.picked.is_empty(), "Umschalt+Tab übernimmt nicht");
        assert!(!h.st().open);
        assert!(!h.focused(), "der Fokus geht zurück");
        // aus dem zweiten Feld kommt Umschalt+Tab ins Vorschlagsfeld zurück
        h.ctx.memory_mut(|m| m.request_focus(Id::new("other")));
        h.frame(vec![]);
        h.frame(vec![]);
        h.key_mod(Key::Tab, Modifiers::SHIFT);
        h.frame(vec![]);
        assert!(h.focused(), "Umschalt+Tab erreicht das Feld");
    }

    #[test]
    fn popup_without_focus_closes_even_under_the_pointer() {
        let mut h = H::new(Mode::Single);
        h.frame(vec![Event::Text("e".into())]);
        assert!(h.frame(vec![]));
        let r = h.st().rect.expect("Popup gezeichnet");
        h.frame(vec![Event::PointerMoved(r.center())]);
        h.ctx.memory_mut(|m| m.surrender_focus(Id::new("vs_field")));
        let open = h.frame(vec![Event::PointerMoved(r.center())]);
        assert!(!open && !h.st().open, "nur Schweben hält das Popup nicht offen");
        assert!(!h.frame(vec![]), "und es kommt nicht wieder (blockiert kein Esc des Dialogs)");
    }

    #[test]
    fn clicking_a_suggestion_still_picks_it() {
        let mut h = H::new(Mode::List);
        h.frame(vec![Event::Text("ops, fr".into())]);
        assert!(h.frame(vec![]));
        let r = h.st().rect.expect("Popup gezeichnet");
        let row = Pos2::new(r.min.x + 30.0, r.min.y + 4.0 + 11.0);
        h.frame(vec![Event::PointerMoved(row)]);
        h.press(row, true);
        h.press(row, false);
        assert_eq!(h.picked, vec!["Frontend"]);
        assert_eq!(h.text, "ops, Frontend, ");
        h.frame(vec![]);
        assert!(h.focused(), "der Fokus kehrt ins Feld zurück");
        assert!(!h.st().open, "nach dem Übernehmen zu");
    }

    #[test]
    fn prefix_matches_come_before_contains_matches() {
        let all = count_values([("backend", 12), ("ops", 7), ("Endpoint", 1), ("frontend", 3), ("end", 2)]);
        let names = |v: Vec<&Counted>| v.into_iter().map(|c| c.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(filter(&all, "end", &[], 10)), vec!["end", "Endpoint", "backend", "frontend"], "Anfang vor Enthalten, dann Anzahl");
        // auch bei unsortierter Eingabe: nach Anzahl, dann Name
        let raw = vec![c("b", 1), c("Ab", 1), c("ab", 5), c("xab", 9)];
        assert_eq!(names(filter(&raw, "a", &[], 10)), vec!["ab", "Ab", "xab"]);
    }

    #[test]
    fn per_open_values_are_shared_not_copied() {
        let ctx = egui::Context::default();
        let id = Id::new("shared");
        let mut got = Vec::new();
        for _ in 0..2 {
            let _ = ctx.run(RawInput::default(), |ctx| got.push(cached_per_open(ctx, id, || vec![c("a", 1)])));
        }
        assert!(Arc::ptr_eq(&got[0], &got[1]), "dieselbe Liste in jedem Bild");
    }
}
