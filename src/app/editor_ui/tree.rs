//! Ordnerbaum des Editors (#54): eigener Ordner, unabhängig vom Dateibaum der Terminals. Ordner werden beim Aufklappen
//! gelesen (je Ordner höchstens 500 Einträge, wie im Terminal Hub), langsame Wurzeln (Netzwerk, WSL) im Hilfsthread.

use super::super::{list_dir, panels, TreeItem};
use crate::state::Lang;
use crate::style;
use eframe::egui::{self, Color32, FontId, Pos2, Rect, Sense, Ui, Vec2};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Was ein Klick im Baum auslöst.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeOut {
    Open(PathBuf),
    /// In einer neuen Teilung rechts öffnen.
    OpenSide(PathBuf),
    CopyPath(PathBuf),
    /// Ordner als neue Wurzel.
    SetRoot(PathBuf),
    /// Zwei Dateien vergleichen (#385): links, rechts.
    Compare(PathBuf, PathBuf),
}

#[derive(Default)]
pub struct Tree {
    pub expanded: HashSet<PathBuf>,
    cache: HashMap<PathBuf, Vec<TreeItem>>,
    stamp: Option<Instant>,
    pub hidden: bool,
    /// Zuletzt geöffnete Datei (markiert).
    pub selected: Option<PathBuf>,
    /// *Zum Vergleichen auswählen* (#385): diese Datei ist die linke Seite des nächsten *Vergleichen mit …*.
    pub compare_pick: Option<PathBuf>,
    /// Mit Strg+Klick markierte Dateien (höchstens zwei) – *Vergleichen* nimmt beide.
    pub marked: Vec<PathBuf>,
}

const ROW_H: f32 = 22.0;
const INDENT: f32 = 14.0;

impl Tree {
    /// Gelesene Inhalte vergessen (Knopf „Aktualisieren“, neue Wurzel).
    pub fn refresh(&mut self) {
        self.cache.clear();
        panels::slow_clear();
        self.stamp = Some(Instant::now());
    }

    fn items(&mut self, ctx: &egui::Context, dir: &Path, slow: bool) -> Option<Vec<TreeItem>> {
        if slow {
            return panels::slow_list(ctx, dir, self.hidden);
        }
        let hidden = self.hidden;
        Some(self.cache.entry(dir.to_path_buf()).or_insert_with(|| list_dir(dir, hidden)).clone())
    }

    /// Baum unter `root` zeigen; liefert die ausgelösten Aktionen.
    pub fn show(&mut self, ui: &mut Ui, root: &Path, lang: Lang) -> Vec<TreeOut> {
        // Alle 10 s neu lesen (neue Dateien von Agenten oder Builds erscheinen von selbst), wie der Baum des Terminal Hub.
        if self.stamp.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
            self.cache.clear();
            self.stamp = Some(Instant::now());
        }
        let slow = panels::is_slow_root(&root.to_string_lossy());
        let mut out = Vec::new();
        let mut rows: Vec<(usize, TreeItem)> = Vec::new();
        self.collect(ui.ctx(), root, 0, slow, &mut rows);
        if rows.is_empty() {
            ui.weak(if slow && !panels::slow_cached(root, self.hidden) { lang.t("wird geladen …", "loading …") } else { lang.t("(leer)", "(empty)") });
            return out;
        }
        for (depth, it) in rows {
            self.row(ui, depth, &it, lang, &mut out);
        }
        out
    }

    /// Sichtbare Zeilen in Reihenfolge sammeln (aufgeklappte Ordner mit Inhalt).
    fn collect(&mut self, ctx: &egui::Context, dir: &Path, depth: usize, slow: bool, rows: &mut Vec<(usize, TreeItem)>) {
        if depth > 40 {
            return;
        }
        let Some(items) = self.items(ctx, dir, slow) else { return };
        for it in items {
            let open = it.is_dir && self.expanded.contains(&it.path);
            let path = it.path.clone();
            rows.push((depth, it));
            if open {
                self.collect(ctx, &path, depth + 1, slow, rows);
            }
        }
    }

    fn row(&mut self, ui: &mut Ui, depth: usize, it: &TreeItem, lang: Lang, out: &mut Vec<TreeOut>) {
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, ROW_H), Sense::click());
        let selected = !it.is_dir && self.selected.as_deref() == Some(it.path.as_path());
        let marked = !it.is_dir && (self.marked.contains(&it.path) || self.compare_pick.as_deref() == Some(it.path.as_path()));
        let p = ui.painter();
        if selected {
            p.rect_filled(rect, 4.0, style::ui().sidebar_active);
        } else if marked {
            p.rect_filled(rect, 4.0, style::ui().selection_bg.gamma_multiply(0.6));
        } else if resp.hovered() {
            p.rect_filled(rect, 4.0, style::ui().hover_bg);
        }
        let x = rect.left() + 4.0 + depth as f32 * INDENT;
        let cy = rect.center().y;
        let text_c = if selected { ui.visuals().strong_text_color() } else { ui.visuals().text_color() };
        if it.is_dir {
            let open = self.expanded.contains(&it.path);
            chevron(p, Pos2::new(x + 5.0, cy), open, style::dim(ui));
            style::paint_icon(p, style::Icon::Folder, Pos2::new(x + 19.0, cy), style::dim(ui));
        } else {
            style::paint_icon(p, style::Icon::Doc, Pos2::new(x + 19.0, cy), style::dim(ui));
        }
        let font = FontId::proportional(13.0);
        let room = (rect.right() - (x + 30.0) - 4.0).max(0.0);
        let g = p.layout(it.name.clone(), font, text_c, f32::INFINITY);
        let clip = Rect::from_min_max(Pos2::new(x + 30.0, rect.top()), Pos2::new(x + 30.0 + room, rect.bottom()));
        p.with_clip_rect(clip).galley(Pos2::new(x + 30.0, cy - g.size().y / 2.0), g, text_c);
        let ctrl = ui.input(|i| i.modifiers.ctrl || i.modifiers.command);
        if resp.clicked() {
            if it.is_dir {
                if !self.expanded.remove(&it.path) {
                    self.expanded.insert(it.path.clone());
                }
            } else if ctrl {
                // Strg+Klick markiert für *Vergleichen* (höchstens zwei, die älteste fällt heraus).
                if let Some(i) = self.marked.iter().position(|x| *x == it.path) {
                    self.marked.remove(i);
                } else {
                    self.marked.push(it.path.clone());
                    if self.marked.len() > 2 {
                        self.marked.remove(0);
                    }
                }
            } else {
                self.selected = Some(it.path.clone());
                self.marked.clear();
                out.push(TreeOut::Open(it.path.clone()));
            }
        }
        let path = it.path.clone();
        let is_dir = it.is_dir;
        let tip = if is_dir { path.to_string_lossy().to_string() } else { format!("{}\n{}", path.to_string_lossy(), lang.t("Strg+Klick: zum Vergleichen markieren", "Ctrl+click: mark for comparing")) };
        resp.on_hover_text(tip).context_menu(|ui| {
            if !is_dir {
                if ui.button(lang.t("Öffnen", "Open")).clicked() {
                    out.push(TreeOut::Open(path.clone()));
                    ui.close();
                }
                if ui.button(lang.t("Rechts daneben öffnen", "Open to the side")).clicked() {
                    out.push(TreeOut::OpenSide(path.clone()));
                    ui.close();
                }
                ui.separator();
                if self.marked.len() == 2 && self.marked.contains(&path) {
                    if ui.button(lang.t("Markierte vergleichen", "Compare marked files")).clicked() {
                        out.push(TreeOut::Compare(self.marked[0].clone(), self.marked[1].clone()));
                        self.marked.clear();
                        ui.close();
                    }
                }
                match self.compare_pick.clone() {
                    Some(pick) if pick != path => {
                        let name = pick.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        if ui.button(format!("{} {}", lang.t("Vergleichen mit", "Compare with"), lang.q(&name))).clicked() {
                            out.push(TreeOut::Compare(pick, path.clone()));
                            self.compare_pick = None;
                            ui.close();
                        }
                    }
                    Some(_) => {
                        if ui.button(lang.t("Auswahl zum Vergleichen aufheben", "Clear selection for compare")).clicked() {
                            self.compare_pick = None;
                            ui.close();
                        }
                    }
                    None => {}
                }
                if self.compare_pick.as_deref() != Some(path.as_path()) && ui.button(lang.t("Zum Vergleichen auswählen", "Select for compare")).clicked() {
                    self.compare_pick = Some(path.clone());
                    ui.close();
                }
                ui.separator();
            } else if ui.button(lang.t("Als Wurzel öffnen", "Open as root folder")).clicked() {
                out.push(TreeOut::SetRoot(path.clone()));
                ui.close();
            }
            if ui.button(lang.t("Pfad kopieren", "Copy path")).clicked() {
                out.push(TreeOut::CopyPath(path.clone()));
                ui.close();
            }
            if crate::platform::open::FILE_MANAGER && ui.button(if cfg!(windows) { lang.t("Im Explorer anzeigen", "Show in Explorer") } else { lang.t("Im Dateimanager anzeigen", "Show in file manager") }).clicked() {
                crate::platform::open::show_in_file_manager(&path.to_string_lossy(), is_dir);
                ui.close();
            }
        });
    }

    /// Ordner auf dem Weg zu `file` aufklappen (Datei im Baum zeigen).
    pub fn reveal(&mut self, root: &Path, file: &Path) {
        let mut cur = file.parent();
        while let Some(d) = cur {
            if !d.starts_with(root) || d == root {
                break;
            }
            self.expanded.insert(d.to_path_buf());
            cur = d.parent();
        }
        self.selected = Some(file.to_path_buf());
    }
}

/// Kleiner Pfeil (zu: nach rechts, offen: nach unten).
fn chevron(p: &egui::Painter, c: Pos2, open: bool, col: Color32) {
    let st = egui::Stroke::new(1.4_f32, col);
    let pts = if open { [c + Vec2::new(-3.5, -1.5), c + Vec2::new(0.0, 2.0), c + Vec2::new(3.5, -1.5)] } else { [c + Vec2::new(-1.5, -3.5), c + Vec2::new(2.0, 0.0), c + Vec2::new(-1.5, 3.5)] };
    p.line_segment([pts[0], pts[1]], st);
    p.line_segment([pts[1], pts[2]], st);
}
