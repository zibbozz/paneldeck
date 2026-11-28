//! Editor-Bereich zeichnen (#54): Ordnerbaum links, Teilungen mit eigener Reiterleiste, Hinweisleisten, Textfeld,
//! Statusleiste unten, Rückfragen. Tastenkürzel des Editors.

use super::doc::{DocEntry, StatusInfo};
use super::panes::Drop as PaneDrop;
use super::text::{self, TextCx};
use super::tree::TreeOut;
use super::{CloseAsk, EditorUi, TabDrag};
use crate::app::tutorial::{spot_rect, Spot, TutMark};
use crate::editor::syntax::Mode;
use crate::layout::{Dir, Node};
use crate::state::Lang;
use crate::style::{self, Icon};
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Id, Key, Margin, Modifiers, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub(super) const BAR_H: f32 = 30.0;
const STATUS_H: f32 = 24.0;
const GAP: f32 = 6.0;
const TAB_MIN_W: f32 = 90.0;
const TAB_MAX_W: f32 = 220.0;
const INFO_H: f32 = 30.0;
/// Breite des Ordnerbaums: Grenzen und Vorgabe.
pub(super) const TREE_MIN: f32 = 160.0;
pub(super) const TREE_MAX: f32 = 600.0;
const TREE_DEFAULT: f32 = 240.0;
/// Darunter (Breite oder Höhe des Fensters) wird der Editor nicht gezeichnet – so meldet Windows ein minimiertes Fenster.
const MIN_DRAW: f32 = 120.0;

/// Ergebnis eines Klicks irgendwo im Bereich, nach dem Zeichnen ausgeführt.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Cmd {
    OpenPath(PathBuf, bool),
    OpenFileDialog,
    OpenFolderDialog,
    SetRoot(PathBuf),
    CloseRoot,
    NewFile,
    Activate(u64, usize),
    Close(Vec<(u64, u64)>),
    Split(u64, Dir),
    Save(u64),
    SaveAs(u64),
    Reload(u64),
    KeepMine(u64),
    ShowDiff(u64),
    DismissLarge(u64),
    DismissAgent(u64),
    Drop(TabDrag, PaneDrop),
    CopyPath(PathBuf),
    /// Linux-Pfad einer WSL-Datei kopieren (#392).
    CopyLinuxPath(PathBuf),
    SetEncoding(u64, &'static str),
    SetEol(u64, bool),
    ForgetRecent,
    /// Sprache des Dokuments wählen (`None` = erkennen) – Statusleiste (#312).
    SetLanguage(u64, Option<String>),
    /// Modus des Dokuments wählen (`None` = aus den Einstellungen).
    SetDocMode(u64, Option<Mode>),
    /// Modus für alle Dateien einer Sprache (Einstellung; `None` = Standard). Die eigene Wahl im Dokument fällt weg.
    SetLangMode(u64, String, Option<Mode>),
    /// Standard-Modus für alle übrigen Dateien (Einstellung). Die eigene Wahl im Dokument fällt weg.
    SetDefaultMode(u64, Mode),
    // ---- Menüleiste und Formatieren (#381)
    /// Teilung fokussieren (vor jedem Befehl aus ihrer Menüleiste).
    FocusGroup(u64),
    /// Befehl des Textfelds auf den aktiven Reiter der Teilung (Rückgängig, Zwischenablage, Zeilen, Vorkommen …).
    EditOp(u64, super::doc::Op),
    /// Suchleiste öffnen (`true` = mit Ersetzen).
    OpenFind(bool),
    GotoLineAsk,
    /// Fenster „In Dateien suchen“.
    FindInFiles,
    /// Alle ungespeicherten Dateien mit Pfad speichern.
    SaveAll,
    ToggleWrap,
    ToggleMinimap,
    ToggleTree,
    /// Befehls-Palette öffnen.
    OpenPalette,
    /// Formatieren/Minify/Prüfen/Schlüssel sortieren im aktiven Reiter der Teilung.
    Format(u64, crate::editor::format::Op),
    FormatCancel(u64),
    /// Zur Fehlerstelle springen (Dokument, Zeile, Spalte; 1-basiert).
    FormatGoto(u64, usize, usize),
    FormatDismiss(u64),
    /// Antwort auf die Rückfrage vor *Minify* von JSONC.
    MinifyConfirm(bool),
    /// Einträge der Aktionen beim Speichern im Menü *Datei* (#383).
    OnSaveMenu(super::super::save_actions_dialog::OnSaveMenu),
    // ---- Vergleich und Änderungsmarken (#385)
    /// Dokument mit einem Ziel vergleichen (Ziel links, Dokument rechts).
    Compare(u64, super::diff_tab::Target),
    /// Zwei Dateien vergleichen (links, rechts).
    ComparePaths(PathBuf, PathBuf),
    /// Vergleichs-Reiter schließen (Gruppe, Vergleich).
    CloseDiff(u64, u64),
    SwapDiff(u64),
    /// Revisionsauswahl für diese Datei öffnen; mit Vergleich: dessen linke Seite ersetzen.
    PickRevision(PathBuf, Option<u64>),
    SavePatch(u64),
    /// Dokument im Datei-Reiter zeigen, Cursor auf Zeile (ab 0).
    OpenAt(u64, usize),
    /// Kurzer Hinweis in der Statusleiste.
    Hint(String),
    SetMarkers(bool),
    SetMarkerBase(crate::editor_svc::settings::MarkerBase),
    /// Nächste/vorige Änderung (F7 / Umschalt+F7).
    StepChange(bool),
    /// Smart-Modus (#367): Ordner vertrauen, einen übergeordneten wählen, jetzt nicht.
    Trust(PathBuf),
    TrustPick(PathBuf),
    TrustNotNow(PathBuf),
    /// Nach der Absturz-Regel: Smart für die Sprache wieder an bzw. ihr Protokoll zeigen.
    SmartRetry(String),
    SmartShowLog(String),
    /// Problemliste ein/aus, nächstes/voriges Problem (F8 / Umschalt+F8), Fenster „Language Server“.
    ToggleProblems,
    StepProblem(bool),
    OpenServers,
    /// Vervollständigen in der Gruppe (Strg+Leertaste), Vorschlag übernehmen, Popup schließen.
    Complete(u64),
    AcceptCompletion,
    ClosePopupCompletion,
    /// Marke in dieser Zeile des Dokuments zurücksetzen (ein Rückgängig-Schritt).
    RevertMark(u64, usize),
    /// Marke in dieser Zeile im Vergleich öffnen.
    MarkToDiff(u64, usize),
    ClosePopup,
}

impl super::super::App {
    /// Editor-Bereich (statt Seitenleiste und Terminals).
    pub(crate) fn editor_view(&mut self, ctx: &egui::Context) {
        let lang = self.settings.language;
        let prefs = self.editor_prefs();
        // Deckkraft des Bereichs „Editor“ (#387).
        let theme = self.theme_for(crate::theme::background::Area::Editor);
        let mut cmds: Vec<Cmd> = Vec::new();
        self.editor_keys(ctx, &mut cmds);
        // Dateien aus dem Explorer ablegen: Dateien öffnen, ein Ordner wird die Wurzel.
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.clone()).collect());
        for p in dropped {
            cmds.push(if p.is_dir() { Cmd::SetRoot(p) } else { Cmd::OpenPath(p, false) });
        }
        // Aktionen beim Speichern (#383): Stand für Statusleiste/Reiter, danach Klicks daraus.
        self.sa_publish_view();
        draw(ctx, &mut self.ed, &theme, &prefs, lang, &mut cmds);
        self.editor_dialogs(ctx, &mut cmds);
        for c in cmds {
            self.editor_cmd(ctx, c);
        }
        self.sa_take_clicks();
        if let Some((_, t)) = &self.ed.hint {
            if t.elapsed() > Duration::from_secs(4) {
                self.ed.hint = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(500));
            }
        }
    }

    /// Tastenkürzel des Editors (nur im Editor-Reiter). Teilen nutzt die Belegung des Terminal Hub (`split_r`/`split_d`).
    fn editor_keys(&mut self, ctx: &egui::Context, cmds: &mut Vec<Cmd>) {
        // Auch nicht, solange die Rückfrage „JSONC minifizieren?“ offen ist (#381).
        if self.capture_key.is_some() || self.ed.close_ask.is_some() || !self.ed.fmt.ask.is_empty() {
            return;
        }
        let g = self.ed.panes.focused;
        let doc = self.ed.active_file_doc();
        let diff = self.ed.active_diff();
        let binds: Vec<(&str, Option<(Modifiers, Key)>)> = vec![
            ("split_r", self.binding("split_r")),
            ("split_d", self.binding("split_d")),
            ("ed_next_change", self.binding("ed_next_change")),
            ("ed_prev_change", self.binding("ed_prev_change")),
            ("palette", self.binding("palette")),
        ];
        let mut palette = false;
        // Mehrere Cursor und Kommentar (#394): nur in Text-Reitern; die Cursor-Befehle nur mit dem Schalter. Sie kommen vor
        // `split_r` dran (Strg+Umschalt+D markiert im Editor alle Vorkommen, im Terminal Hub teilt es rechts).
        let prefs = self.editor_prefs();
        let text_tab = doc.is_some() && diff.is_none();
        let comment = match (text_tab, doc) {
            (true, Some(d)) => self.ed.docs.get_mut(&d).and_then(|e| e.line_comment(&prefs)),
            _ => None,
        };
        let mut multi_binds: Vec<(super::doc::Op, Option<(Modifiers, Key)>)> = Vec::new();
        if text_tab && prefs.multi.on {
            multi_binds.push((super::doc::Op::SelectNextOccurrence, self.binding("ed_next_occurrence")));
            multi_binds.push((super::doc::Op::SelectAllOccurrences, self.binding("ed_all_occurrences")));
            multi_binds.push((super::doc::Op::SoftUndo, self.binding("ed_soft_undo")));
        }
        if text_tab {
            for id in ["ed_toggle_comment", "ed_toggle_comment_alt"] {
                match comment {
                    Some(c) => multi_binds.push((super::doc::Op::ToggleComment(c), self.binding(id))),
                    // Ohne Zeilenkommentar: die Taste verschlucken und einen Hinweis zeigen (sonst käme „/“ als Text an).
                    None => multi_binds.push((super::doc::Op::ToggleComment(""), self.binding(id))),
                }
            }
        }
        // Formatieren (#381) und die Befehls-Palette: Belegung aus Einstellungen → Tastenkürzel.
        use crate::editor::format::Op as FmtOp;
        let fmt_binds: Vec<(FmtOp, Option<(Modifiers, Key)>)> =
            [
                (FmtOp::Format, "ed_format"),
                (FmtOp::Minify, "ed_minify"),
                (FmtOp::Validate, "ed_validate"),
                (FmtOp::SortKeys, "ed_sort_keys"),
                // Zweite Belegung (Alt+Umschalt schaltet bei mehreren Eingabesprachen die Sprache um).
                (FmtOp::Format, "ed_format_alt"),
                (FmtOp::Minify, "ed_minify_alt"),
                (FmtOp::Validate, "ed_validate_alt"),
            ]
            .into_iter()
            .map(|(o, id)| (o, self.binding(id)))
            .collect();
        // *Ohne Aktionen speichern* (#383): eigenes Kürzel, standardmäßig keins.
        let no_actions = self.binding("save_no_actions");
        let mut save_plain = false;
        let ctrl = Modifiers::CTRL;
        let cs = Modifiers::CTRL | Modifiers::SHIFT;
        let ca = Modifiers::CTRL | Modifiers::ALT;
        let mut nav = None;
        let mut no_comment = false;
        // Smart-Modus (#367): Popup der Vervollständigung (Pfeile, Tab/Eingabe, Esc), Tab im Snippet, Kürzel.
        let popup = self.ed.smart.popup.as_ref().is_some_and(|p| !p.shown.is_empty() && p.group == g);
        let snippet = self.ed.smart.snippet.as_ref().is_some_and(|s| s.group == g);
        let smart_binds: Vec<(&str, Option<(Modifiers, Key)>)> = if text_tab {
            vec![("ed_complete", self.binding("ed_complete")), ("ed_next_problem", self.binding("ed_next_problem")), ("ed_prev_problem", self.binding("ed_prev_problem"))]
        } else {
            Vec::new()
        };
        let problems_key = self.binding("ed_problems");
        ctx.input_mut(|i| {
            let mut take0 = |m: Modifiers, k: Key| super::super::consume_key_exact(i, m, k, false);
            if popup {
                if take0(Modifiers::NONE, Key::ArrowDown) {
                    self.ed.smart_popup_move(true);
                }
                if take0(Modifiers::NONE, Key::ArrowUp) {
                    self.ed.smart_popup_move(false);
                }
                if take0(Modifiers::NONE, Key::Tab) || take0(Modifiers::NONE, Key::Enter) {
                    cmds.push(Cmd::AcceptCompletion);
                }
                if take0(Modifiers::NONE, Key::Escape) {
                    cmds.push(Cmd::ClosePopupCompletion);
                }
            } else if snippet {
                if take0(Modifiers::NONE, Key::Tab) {
                    self.ed.smart_next_stop();
                }
                if take0(Modifiers::NONE, Key::Escape) {
                    self.ed.smart.snippet = None;
                }
            }
            for (id, b) in &smart_binds {
                if let Some((m, k)) = b
                    && super::super::consume_key_exact(i, *m, *k, false)
                {
                    match *id {
                        "ed_complete" => cmds.push(Cmd::Complete(g)),
                        "ed_next_problem" => cmds.push(Cmd::StepProblem(true)),
                        _ => cmds.push(Cmd::StepProblem(false)),
                    }
                }
            }
            if let Some((m, k)) = problems_key
                && super::super::consume_key_exact(i, m, k, false)
            {
                cmds.push(Cmd::ToggleProblems);
            }
            for (op, b) in &multi_binds {
                if let Some((m, k)) = b
                    && super::super::consume_key_exact(i, *m, *k, false)
                {
                    if *op == super::doc::Op::ToggleComment("") {
                        no_comment = true;
                    } else {
                        cmds.push(Cmd::EditOp(g, op.clone()));
                    }
                }
            }
            for (id, b) in &binds {
                if let Some((m, k)) = b
                    && super::super::consume_key_exact(i, *m, *k, false)
                {
                    match *id {
                        "split_r" => cmds.push(Cmd::Split(g, Dir::Horizontal)),
                        "split_d" => cmds.push(Cmd::Split(g, Dir::Vertical)),
                        "ed_next_change" => cmds.push(Cmd::StepChange(true)),
                        "ed_prev_change" => cmds.push(Cmd::StepChange(false)),
                        _ => palette = true,
                    }
                }
            }
            // Formatieren nur in Datei-Reitern, nicht im Vergleich (#385).
            if doc.is_some() && diff.is_none() {
                for (op, b) in &fmt_binds {
                    if let Some((m, k)) = b
                        && super::super::consume_key_exact(i, *m, *k, false)
                    {
                        cmds.push(Cmd::Format(g, *op));
                        // Alt+Umschalt+Buchstabe darf nicht zusätzlich als Text ankommen.
                        if m.alt && !m.ctrl {
                            drop_alt_text(i, *k);
                        }
                    }
                }
            }
            if doc.is_some()
                && let Some((m, k)) = no_actions
                && super::super::consume_key_exact(i, m, k, false)
            {
                save_plain = true;
            }
            let mut take = |m: Modifiers, k: Key| super::super::consume_key_exact(i, m, k, false);
            if let Some(d) = doc {
                if take(cs, Key::S) {
                    cmds.push(Cmd::SaveAs(d));
                }
                if take(ctrl, Key::S) {
                    cmds.push(Cmd::Save(d));
                }
                if take(ctrl, Key::W) || take(ctrl, Key::F4) {
                    match diff {
                        Some((dg, id)) => cmds.push(Cmd::CloseDiff(dg, id)),
                        None => cmds.push(Cmd::Close(vec![(g, d)])),
                    }
                }
            }
            if take(ctrl, Key::N) {
                cmds.push(Cmd::NewFile);
            }
            if take(ctrl, Key::O) {
                cmds.push(Cmd::OpenFileDialog);
            }
            if take(cs, Key::O) {
                cmds.push(Cmd::OpenFolderDialog);
            }
            if take(ctrl, Key::B) {
                self.ed.tree_open = !self.ed.tree_open;
            }
            if take(Modifiers::ALT, Key::Z) {
                self.ed.wrap = !self.ed.wrap;
            }
            for (k, n) in [(Key::ArrowLeft, super::super::Nav::Left), (Key::ArrowRight, super::super::Nav::Right), (Key::ArrowUp, super::super::Nav::Up), (Key::ArrowDown, super::super::Nav::Down)] {
                if super::super::focus_target(&self.ed.group_rects, g, n).is_some() && take(ca, k) {
                    nav = Some(n);
                }
            }
            if let Some(grp) = self.ed.panes.groups.get(&g) {
                let n = grp.tabs.len();
                if n > 1 {
                    if take(cs, Key::Tab) {
                        cmds.push(Cmd::Activate(g, (grp.active + n - 1) % n));
                    } else if take(ctrl, Key::Tab) {
                        cmds.push(Cmd::Activate(g, (grp.active + 1) % n));
                    }
                }
            }
            if doc.is_some() && diff.is_none() {
                if take(ctrl, Key::F) {
                    self.ed.find.open(false);
                }
                if take(ctrl, Key::H) {
                    self.ed.find.open(true);
                }
                if take(ctrl, Key::G) {
                    self.ed.goto = Some(super::find::GotoLine::default());
                }
            }
        });
        if no_comment {
            let lang = self.settings.language;
            self.ed.set_hint(lang.t("Die Sprache hat keinen Zeilenkommentar (Sprache in der Statusleiste wählen).", "The language has no line comment (choose the language in the status bar).").to_string());
        }
        if save_plain && let Some(d) = doc {
            self.sa_save_without(d);
        }
        if let Some(n) = nav
            && let Some(t) = super::super::focus_target(&self.ed.group_rects, g, n)
        {
            self.ed.panes.focused = t;
            self.ed.want_focus = true;
        }
        // Befehlspalette auch im Editor (#385: Vergleichen, Änderungsmarken, F7).
        if palette && self.palette.is_none() {
            self.palette = Some(super::super::Palette { query: String::new(), sel: 0 });
        }
        // Suchtext aus der Auswahl übernehmen, wenn die Leiste gerade aufgeht.
        if self.ed.find.just_opened()
            && let Some(sel) = self.ed.focused_view_and_doc().and_then(|(v, d)| v.selected_text(d))
            && !sel.contains('\n')
            && !sel.is_empty()
        {
            self.ed.find.query = sel;
        }
    }

    /// Befehl aus dem Bild ausführen.
    pub(super) fn editor_cmd(&mut self, ctx: &egui::Context, c: Cmd) {
        let lang = self.settings.language;
        let prefs = self.editor_prefs();
        let dir = self.dir.clone();
        match c {
            Cmd::OpenPath(p, side) => {
                // WSL-Dateien ungeprüft (#392; sie laden im Hintergrund, ein Fehler erscheint im Reiter).
                if crate::wsl::is_wsl_unc(&p.to_string_lossy()) || p.is_file() {
                    self.ed.open_path(&p, side);
                } else {
                    self.ed.set_hint(format!("{}: {}", lang.t("Datei nicht gefunden", "File not found"), p.display()));
                    let s = p.to_string_lossy().to_string();
                    self.ed.recent_files.retain(|x| *x != s);
                }
            }
            Cmd::OpenFileDialog => {
                let start = self.ed.root.as_ref().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                if let Some(p) = crate::platform::notify::pick_file(&start) {
                    self.ed.open_path(&p, false);
                }
            }
            Cmd::OpenFolderDialog => {
                let start = self.ed.root.as_ref().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                if let Some(p) = crate::platform::notify::pick_folder(&start) {
                    self.ed.set_root(Path::new(&p));
                }
            }
            Cmd::SetRoot(p) => {
                if p.is_dir() {
                    self.ed.set_root(&p);
                } else {
                    let s = p.to_string_lossy().to_string();
                    self.ed.recent_folders.retain(|x| *x != s);
                }
            }
            Cmd::CloseRoot => self.ed.root = None,
            Cmd::NewFile => {
                self.ed.new_untitled(None);
            }
            Cmd::Activate(g, i) => {
                self.ed.panes.focused = g;
                if let Some(grp) = self.ed.panes.groups.get_mut(&g)
                    && i < grp.tabs.len()
                {
                    grp.active = i;
                }
                self.ed.want_focus = true;
            }
            Cmd::Close(tabs) => {
                self.ed.close_ask = Some(CloseAsk::new(tabs));
                self.editor_close_step(&dir);
            }
            Cmd::Split(g, d) => {
                if let Some(ng) = self.ed.panes.split(g, d) {
                    self.ed.fix_split_diff(ng);
                    self.ed.want_focus = true;
                }
            }
            Cmd::Save(id) => {
                let untitled = self.ed.docs.get(&id).is_some_and(|d| d.path.is_none());
                if untitled {
                    self.editor_cmd(ctx, Cmd::SaveAs(id));
                } else if self.ed.docs.contains_key(&id) {
                    // Aktionen beim Speichern (#383): nie auf Kosten des Speicherns.
                    match self.editor_save_with(id, None, crate::save_actions::SaveOrigin::User) {
                        Ok(()) => {
                            // WSL (#392): läuft das Schreiben noch im Hintergrund, folgen Hinweis und Löschen der
                            // Hot-Exit-Kopie erst mit dem Ergebnis (`editor_background_saved`).
                            if let Some(d) = self.ed.docs.get_mut(&id)
                                && !d.disk_busy()
                            {
                                d.drop_hot(&dir);
                                let t = d.title(lang);
                                self.ed.set_hint(format!("{t}: {}", lang.t("gespeichert", "saved")));
                            }
                        }
                        Err(e) => self.notify_text(e),
                    }
                }
            }
            Cmd::SaveAs(id) => self.editor_save_as_dialog(id),
            Cmd::Reload(id) => {
                if let Some(d) = self.ed.docs.get_mut(&id) {
                    let _ = d.reload_from_disk(&prefs);
                }
                self.ed.generation += 1;
            }
            Cmd::KeepMine(id) => {
                if let Some(d) = self.ed.docs.get_mut(&id) {
                    d.keep_mine();
                }
            }
            // „Unterschied anzeigen“ der Hinweisleiste: Vergleichs-Reiter gespeicherte Datei ↔ Puffer (#385).
            Cmd::ShowDiff(id) => self.editor_cmd(ctx, Cmd::Compare(id, super::diff_tab::Target::Disk)),
            Cmd::DismissLarge(id) => {
                if let Some(d) = self.ed.docs.get_mut(&id) {
                    d.large_dismissed = true;
                }
            }
            Cmd::DismissAgent(id) => {
                if let Some(d) = self.ed.docs.get_mut(&id) {
                    d.agent_note_dismissed = true;
                }
            }
            Cmd::Drop(drag, to) => {
                self.ed.panes.move_tab(drag.group, drag.index, to);
                self.ed.want_focus = true;
            }
            Cmd::CopyPath(p) => ctx.copy_text(p.to_string_lossy().to_string()),
            Cmd::CopyLinuxPath(p) => {
                if let Some((_, l)) = crate::wsl::unc_to_linux(&p.to_string_lossy()) {
                    ctx.copy_text(l);
                }
            }
            Cmd::SetEncoding(id, enc) => {
                if let Some(d) = self.ed.docs.get_mut(&id)
                    && let Err(e) = d.set_encoding(enc, lang)
                {
                    self.notify_text(e);
                }
            }
            Cmd::SetEol(id, crlf) => {
                if let Some(d) = self.ed.docs.get_mut(&id) {
                    d.set_eol(crlf);
                }
            }
            Cmd::ForgetRecent => {
                self.ed.recent_files.clear();
                self.ed.recent_folders.clear();
            }
            Cmd::SetLanguage(id, l) => self.ed.set_doc_language(id, l),
            Cmd::SetDocMode(id, m) => self.ed.set_doc_mode(id, m),
            Cmd::SetLangMode(id, l, m) => {
                match m {
                    Some(m) => {
                        self.settings.editor.language_modes.insert(l, m);
                    }
                    None => {
                        self.settings.editor.language_modes.remove(&l);
                    }
                }
                self.ed.set_doc_mode(id, None);
            }
            Cmd::SetDefaultMode(id, m) => {
                self.settings.editor.default_mode = m;
                self.ed.set_doc_mode(id, None);
            }
            Cmd::Compare(doc, target) => {
                use super::diff_tab::Target;
                match target {
                    Target::PickFile => {
                        let start = self.ed.docs.get(&doc).and_then(|d| d.path.as_ref()).and_then(|p| p.parent()).map(|p| p.to_string_lossy().to_string()).or_else(|| self.ed.root.as_ref().map(|p| p.to_string_lossy().to_string())).unwrap_or_default();
                        if let Some(p) = crate::platform::notify::pick_file(&start) {
                            self.editor_cmd(ctx, Cmd::Compare(doc, Target::File(p)));
                        }
                    }
                    t => {
                        if let Err(e) = self.ed.compare(doc, t, &prefs, lang) {
                            self.ed.set_hint(e);
                        }
                    }
                }
            }
            Cmd::ComparePaths(a, b) => {
                if self.ed.compare_paths(&a, &b, &prefs).is_none() {
                    self.ed.set_hint(lang.t("Diese Dateien lassen sich nicht vergleichen.", "These files cannot be compared.").to_string());
                }
            }
            Cmd::CloseDiff(g, id) => self.ed.close_diff(g, id, &dir),
            Cmd::SwapDiff(id) => self.ed.swap_diff(id),
            Cmd::PickRevision(p, diff) => {
                if p.as_os_str().is_empty() {
                    self.ed.set_hint(lang.t("Die Datei ist noch nicht gespeichert.", "The file has not been saved yet.").to_string());
                } else {
                    self.ed.rev_picker = Some(super::diff_menu::RevPicker::new(p, diff));
                }
            }
            Cmd::SavePatch(id) => {
                let Some(dt) = self.ed.diffs.get(&id) else { return };
                match dt.patch(&self.ed.docs, true) {
                    Some(p) if !p.is_empty() => {
                        let name = dt.patch_file_name(&self.ed.docs);
                        let start = dt.folder(&self.ed.docs).or_else(|| self.ed.root.clone());
                        if let Some(path) = crate::platform::notify::save_any(start.as_deref(), &name) {
                            match crate::state::write_atomic(&path, p.as_bytes()) {
                                Ok(()) => self.ed.set_hint(format!("{}: {}", lang.t("Patch gespeichert", "Patch saved"), path.display())),
                                Err(e) => self.notify_text(format!("{}: {e}", lang.t("Patch ließ sich nicht speichern", "Could not save the patch"))),
                            }
                        }
                    }
                    Some(_) => self.ed.set_hint(lang.t("Keine Unterschiede – kein Patch.", "No differences – no patch.").to_string()),
                    None => self.ed.set_hint(lang.t("Für diesen Vergleich gibt es keinen Patch.", "There is no patch for this comparison.").to_string()),
                }
            }
            Cmd::OpenAt(doc, line) => {
                if self.ed.docs.contains_key(&doc) {
                    self.ed.show_doc(doc, false);
                    self.ed.goto_line(line, 0);
                }
            }
            Cmd::Hint(t) => self.ed.set_hint(t),
            Cmd::SetMarkers(on) => {
                self.settings.editor.change_markers = on;
                if !on {
                    self.ed.mark_popup = None;
                }
            }
            Cmd::SetMarkerBase(b) => self.settings.editor.marker_base = b,
            Cmd::StepChange(forward) => {
                if let Some(h) = self.ed.step_change(forward, &prefs, lang) {
                    self.ed.set_hint(h);
                }
            }
            Cmd::Trust(folder) => {
                let dir = self.dir.clone();
                match self.ed.smart_trust(&folder, &dir) {
                    Ok(()) => self.ed.set_hint(format!("{} {}", lang.t("Vertraut:", "Trusted:"), folder.display())),
                    Err(e) => self.ed.set_hint(e),
                }
            }
            Cmd::TrustPick(folder) => {
                if let Some(p) = crate::platform::notify::pick_folder(&folder.to_string_lossy()) {
                    self.editor_cmd(ctx, Cmd::Trust(PathBuf::from(p)));
                }
            }
            Cmd::TrustNotNow(folder) => {
                self.ed.smart.not_now.insert(folder);
            }
            Cmd::SmartRetry(l) => self.ed.smart_retry(&l),
            Cmd::SmartShowLog(l) => self.ed.smart_show_log(&l),
            Cmd::ToggleProblems => self.ed.smart.problems_open = !self.ed.smart.problems_open,
            Cmd::StepProblem(forward) => {
                let g = self.ed.panes.focused;
                if self.ed.smart_step_problem(g, forward).is_none() {
                    self.ed.set_hint(lang.t("Keine Probleme in dieser Datei.", "No problems in this file.").to_string());
                }
            }
            Cmd::OpenServers => self.ed.smart.window_open = true,
            Cmd::Complete(g) => {
                if let Some(d) = self.ed.panes.groups.get(&g).and_then(|grp| grp.active_tab()).filter(|t| t.diff.is_none()).map(|t| t.doc) {
                    if matches!(self.ed.smart.state_of(d), super::smart::State::Running(_)) {
                        self.ed.smart_complete(d, g, None);
                    } else {
                        self.ed.set_hint(lang.t("Vervollständigen gibt es im Smart-Modus (Statusleiste).", "Completion works in smart mode (status bar).").to_string());
                    }
                }
            }
            Cmd::AcceptCompletion => {
                self.ed.smart_accept();
                self.ed.want_focus = true;
            }
            Cmd::ClosePopupCompletion => self.ed.smart.popup = None,
            Cmd::RevertMark(doc, line) => {
                if let Err(e) = self.ed.revert_mark(doc, line, lang) {
                    self.ed.set_hint(e);
                }
            }
            Cmd::MarkToDiff(doc, line) => {
                let target = self.ed.docs.get(&doc).and_then(|d| {
                    let m = d.marks.result.clone()?;
                    let k = m.at(line)?;
                    Some(super::marks::diff_target(&m.hunks[k], d.marks.base.as_deref()))
                });
                self.ed.mark_popup = None;
                if let Some(t) = target {
                    self.editor_cmd(ctx, Cmd::Compare(doc, t));
                }
            }
            Cmd::ClosePopup => {
                self.ed.mark_popup = None;
                self.ed.want_focus = true;
            }
            Cmd::FocusGroup(g) => {
                if self.ed.panes.groups.contains_key(&g) {
                    self.ed.panes.focused = g;
                }
                self.ed.want_focus = true;
            }
            Cmd::EditOp(g, op) => self.ed.menu_edit_op(ctx, g, op),
            Cmd::OpenFind(replace) => {
                if self.ed.active_doc().is_some() {
                    self.ed.find.open(replace);
                }
            }
            Cmd::GotoLineAsk => {
                if self.ed.active_doc().is_some() {
                    self.ed.goto = Some(super::find::GotoLine::default());
                }
            }
            Cmd::FindInFiles => {
                self.editor_svc.search_open = true;
                self.editor_svc.search.focus = true;
            }
            Cmd::SaveAll => self.editor_save_all(),
            Cmd::ToggleWrap => self.ed.wrap = !self.ed.wrap,
            Cmd::ToggleMinimap => self.ed.minimap = !self.ed.minimap,
            Cmd::ToggleTree => self.ed.tree_open = !self.ed.tree_open,
            Cmd::OpenPalette => self.palette = Some(super::super::Palette { query: String::new(), sel: 0 }),
            Cmd::Format(g, op) => {
                let c = ctx.clone();
                self.ed.start_format(g, op, false, move || c.request_repaint(), lang);
            }
            Cmd::FormatCancel(doc) => self.ed.cancel_format(doc),
            Cmd::FormatGoto(doc, line, col) => self.ed.format_goto(doc, line, col),
            Cmd::FormatDismiss(doc) => {
                self.ed.fmt.notes.remove(&doc);
            }
            Cmd::MinifyConfirm(yes) => {
                if self.ed.fmt.ask.is_empty() {
                    return;
                }
                let a = self.ed.fmt.ask.remove(0);
                // In der Teilung der Anfrage, sonst in einem Reiter, der das Dokument zeigt.
                let here = self.ed.panes.groups.get(&a.group).and_then(|g| g.active_tab()).is_some_and(|t| t.doc == a.doc);
                let at = if here { Some(a.group) } else { self.ed.panes.tabs_of(a.doc).first().map(|&(g, i)| {
                    if let Some(grp) = self.ed.panes.groups.get_mut(&g) {
                        grp.active = i;
                    }
                    g
                }) };
                if yes && let Some(g) = at {
                    let c = ctx.clone();
                    self.ed.start_format(g, crate::editor::format::Op::Minify, true, move || c.request_repaint(), lang);
                }
            }
            Cmd::OnSaveMenu(m) => self.on_save_menu_items(m),
        }
    }

    /// *Datei → Alle speichern*: alle ungespeicherten Dateien mit Pfad (Aktionen beim Speichern wie bei
    /// *Speichern*, Auslöser „Alle speichern“; je Datei ein Lauf); unbenannte Reiter brauchen *Speichern unter*.
    fn editor_save_all(&mut self) {
        let lang = self.settings.language;
        let dir = self.dir.clone();
        let ids: Vec<u64> = self.ed.docs.iter().filter(|(_, d)| d.dirty() && d.path.is_some()).map(|(id, _)| *id).collect();
        let mut saved = 0;
        let mut pending = 0;
        for id in ids {
            match self.editor_save_with(id, None, crate::save_actions::SaveOrigin::SaveAll) {
                Ok(_) => match self.ed.docs.get_mut(&id) {
                    // WSL (#392): schreibt noch im Hintergrund – Hot-Exit-Kopie bleibt bis zum Ergebnis.
                    Some(d) if d.disk_busy() => pending += 1,
                    Some(d) => {
                        d.drop_hot(&dir);
                        saved += 1;
                    }
                    None => {}
                },
                Err(e) => {
                    let t = self.ed.docs.get(&id).map(|d| d.title(lang)).unwrap_or_default();
                    self.notify_text(format!("{t}: {e}"));
                }
            }
        }
        let untitled = self.ed.docs.values().filter(|d| d.path.is_none() && d.dirty()).count();
        let mut msg = match lang {
            Lang::De => format!("{saved} Datei(en) gespeichert"),
            Lang::En => format!("{saved} file(s) saved"),
        };
        if pending > 0 {
            msg.push_str(&match lang {
                Lang::De => format!(" – {pending} werden noch gespeichert (WSL)"),
                Lang::En => format!(" – {pending} still being saved (WSL)"),
            });
        }
        if untitled > 0 {
            msg.push_str(&match lang {
                Lang::De => format!(" – {untitled} unbenannte(r) Reiter: „Speichern unter …“"),
                Lang::En => format!(" – {untitled} untitled tab(s): “Save as …”"),
            });
        }
        self.ed.set_hint(msg);
    }

    /// *Speichern unter …* (Dateidialog) für ein Dokument; Auslöser „Speichern unter“ der Aktionen beim Speichern.
    pub(crate) fn editor_save_as_dialog(&mut self, id: u64) {
        let lang = self.settings.language;
        let prefs = self.editor_prefs();
        let dir = self.dir.clone();
        let Some(d) = self.ed.docs.get(&id) else { return };
        let name = d.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| format!("{}.txt", d.title(lang)));
        let start = d.path.as_ref().and_then(|p| p.parent()).map(|p| p.to_path_buf()).or_else(|| self.ed.root.clone());
        if let Some(p) = crate::platform::notify::save_any(start.as_deref(), &name) {
            let other = self.ed.doc_for_path(&p).filter(|o| *o != id);
            match self.editor_save_with(id, Some(p.clone()), crate::save_actions::SaveOrigin::SaveAs) {
                Ok(()) => {
                    super::persist::push_recent(&mut self.ed.recent_files, &p.to_string_lossy());
                    if self.ed.docs.get(&id).is_some_and(|d| d.disk_busy()) {
                        // WSL (#392): Schreiben läuft noch – Hot-Exit-Kopie und der andere Reiter folgen mit dem
                        // Ergebnis (`editor_background_saved`).
                        if let Some(o) = other {
                            self.ed.reload_after_save.insert(id, o);
                        }
                    } else {
                        if let Some(d) = self.ed.docs.get_mut(&id) {
                            d.drop_hot(&dir);
                        }
                        if let Some(o) = other {
                            self.editor_refresh_other(o, &prefs);
                        }
                    }
                }
                Err(e) => self.notify_text(e),
            }
        }
    }

    /// Dieselbe Datei ist noch in einem anderen Reiter offen und wurde eben per *Speichern unter* überschrieben: ohne
    /// eigene Änderungen neu laden, sonst nie überschreiben – die Leiste „auf der Platte geändert“ (*Meine behalten* /
    /// *Von der Platte laden* / *Unterschied*) entscheidet.
    pub(super) fn editor_refresh_other(&mut self, other: u64, prefs: &super::EditorPrefs) {
        let Some(o) = self.ed.docs.get_mut(&other) else { return };
        if o.doc.is_none() {
            // Noch nicht gelesen: liest beim Zeigen ohnehin den neuen Stand.
            return;
        }
        if o.dirty() {
            o.disk_conflict = true;
        } else {
            let _ = o.reload_from_disk(prefs);
        }
        self.ed.generation += 1;
    }

    /// Ein Speichern im Hintergrund (WSL, #392) ist geschrieben: Hot-Exit-Kopie löschen (nur ohne neue Änderungen), den
    /// anderen Reiter derselben Datei auffrischen (*Speichern unter*), eine wartende Schließen-Rückfrage fortsetzen.
    pub(in crate::app) fn editor_background_saved(&mut self, id: u64) {
        let dir = self.dir.clone();
        let prefs = self.editor_prefs();
        if let Some(d) = self.ed.docs.get_mut(&id)
            && !d.dirty()
        {
            d.drop_hot(&dir);
        }
        if let Some(o) = self.ed.reload_after_save.remove(&id) {
            self.editor_refresh_other(o, &prefs);
        }
        let first = self.ed.close_ask.as_ref().and_then(|a| a.tabs.first()).map(|(_, d)| *d);
        if first == Some(id)
            && let Some(ask) = self.ed.close_ask.as_mut()
            && ask.saving
        {
            ask.saving = false;
            ask.error = None;
            // Sauber: weiter schließen; während des Schreibens weiter bearbeitet: noch einmal fragen.
            if self.ed.docs.get(&id).is_some_and(|d| !d.dirty()) {
                self.editor_close_step(&dir);
            }
        }
    }

    /// Ein Speichern im Hintergrund ist gescheitert: Hot-Exit-Kopie bleibt; eine wartende Schließen-Rückfrage fragt
    /// wieder und zeigt den Fehler.
    pub(in crate::app) fn editor_background_save_failed(&mut self, id: u64, msg: &str) {
        self.ed.reload_after_save.remove(&id);
        let first = self.ed.close_ask.as_ref().and_then(|a| a.tabs.first()).map(|(_, d)| *d);
        if first == Some(id)
            && let Some(ask) = self.ed.close_ask.as_mut()
            && ask.saving
        {
            ask.saving = false;
            ask.error = Some(msg.to_string());
        }
    }

    /// Nächster Schritt einer Schließen-Rückfrage: Reiter ohne ungespeicherte Änderungen (oder deren Dokument noch in
    /// einem anderen Reiter offen ist) schließen, beim ersten mit Änderungen anhalten und fragen.
    pub(in crate::app) fn editor_close_step(&mut self, dir: &Path) {
        let Some(mut ask) = self.ed.close_ask.take() else { return };
        while let Some(&(g, doc)) = ask.tabs.first() {
            let elsewhere = self.ed.panes.tabs_of(doc).len() > 1;
            let dirty = self.ed.docs.get(&doc).is_some_and(|d| d.dirty());
            if dirty && !elsewhere {
                self.ed.close_ask = Some(ask);
                return;
            }
            ask.tabs.remove(0);
            // Hinweis und Fehler galten dem eben geschlossenen Reiter.
            ask.saving = false;
            ask.error = None;
            self.editor_close_now(g, doc, dir);
        }
    }

    fn editor_close_now(&mut self, g: u64, doc: u64, dir: &Path) {
        if let Some(i) = self.ed.panes.groups.get(&g).and_then(|grp| grp.find(doc))
            && let Some(gone) = self.ed.panes.close_tab(g, i)
        {
            self.ed.release_doc(gone, dir);
        }
        self.ed.want_focus = true;
    }

    /// Rückfragen und Fenster des Editors: Speichern beim Schließen, Gehe zu Zeile, Unterschied anzeigen.
    fn editor_dialogs(&mut self, ctx: &egui::Context, cmds: &mut Vec<Cmd>) {
        let lang = self.settings.language;
        let dir = self.dir.clone();
        // Schließen mit ungespeicherten Änderungen
        if let Some(ask) = self.ed.close_ask.clone() {
            'ask: {
                let Some(&(g, doc)) = ask.tabs.first() else {
                    self.ed.close_ask = None;
                    return;
                };
                let name = self.ed.docs.get(&doc).map(|d| d.title(lang)).unwrap_or_default();
                let mut choice = None;
                // WSL (#392): „Speichern“ gewählt, das Schreiben läuft noch – Hinweis statt Rückfrage, geschlossen wird mit
                // dem Ergebnis (`editor_background_saved`). Läuft nichts mehr, ohne dass ein Ergebnis kam, gilt wieder die
                // Rückfrage.
                if ask.saving {
                    if self.ed.docs.get(&doc).is_some_and(|d| d.disk_busy()) {
                        let mut stop = false;
                        style::modal(Id::new("ed_close_ask")).show(ctx, |ui| {
                            ui.set_max_width(420.0);
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(format!("{} {}", lang.q(&name), lang.t("wird gespeichert …", "is being saved …")));
                            });
                            if ask.tabs.len() > 1 {
                                ui.weak(format!("{} {}", ask.tabs.len() - 1, lang.t("weitere Reiter werden danach geschlossen.", "more tabs will be closed afterwards.")));
                            }
                            ui.add_space(style::S3);
                            // Hängt WSL: nicht schließen, das Speichern läuft weiter (Ergebnis wie sonst als Hinweis).
                            if ui.button(lang.t("Nicht schließen", "Don't close")).clicked() {
                                stop = true;
                            }
                        });
                        if stop {
                            self.ed.close_ask = None;
                        } else {
                            ctx.request_repaint_after(std::time::Duration::from_millis(100));
                        }
                        break 'ask;
                    }
                    if let Some(a) = self.ed.close_ask.as_mut() {
                        a.saving = false;
                    }
                }
                let r = style::modal(Id::new("ed_close_ask")).show(ctx, |ui| {
                    ui.set_max_width(420.0);
                    ui.heading(lang.t("Änderungen speichern?", "Save changes?"));
                    ui.add_space(style::S2);
                    ui.label(format!("{} {}", lang.q(&name), lang.t("hat ungespeicherte Änderungen.", "has unsaved changes.")));
                    if let Some(e) = &ask.error {
                        ui.add_space(style::S2);
                        ui.colored_label(style::ui().error, e);
                    }
                    if ask.tabs.len() > 1 {
                        ui.weak(format!("{} {}", ask.tabs.len() - 1, lang.t("weitere Reiter werden danach geschlossen.", "more tabs will be closed afterwards.")));
                    }
                    ui.add_space(style::S3);
                    ui.horizontal(|ui| {
                        if ui.button(lang.t("Speichern", "Save")).clicked() || ui.input(|i| i.key_pressed(Key::Enter)) {
                            choice = Some(0);
                        }
                        if ui.button(lang.t("Nicht speichern", "Don't save")).clicked() {
                            choice = Some(1);
                        }
                        if ui.button(lang.t("Abbrechen", "Cancel")).clicked() {
                            choice = Some(2);
                        }
                    });
                });
                if r.should_close() && choice.is_none() {
                    choice = Some(2);
                }
                match choice {
                    Some(0) => {
                        let untitled = self.ed.docs.get(&doc).is_some_and(|d| d.path.is_none());
                        if untitled {
                            self.ed.close_ask = None;
                            self.editor_cmd(ctx, Cmd::SaveAs(doc));
                            if self.ed.docs.get(&doc).is_some_and(|d| d.disk_busy()) {
                                // *Speichern unter* auf einen WSL-Pfad (#392): schließen, wenn das Schreiben fertig ist.
                                self.ed.close_ask = Some(CloseAsk { saving: true, error: None, ..ask });
                            } else if self.ed.docs.get(&doc).is_some_and(|d| !d.dirty()) {
                                self.ed.close_ask = Some(ask);
                                self.editor_close_step(&dir);
                            }
                        } else if self.ed.docs.get(&doc).is_some_and(|d| d.disk_busy()) {
                            // WSL (#392): Es wird gerade schon gespeichert (z. B. automatisch) oder gelesen – abwarten; danach
                            // schließt es (sauber) bzw. fragt erneut.
                            self.ed.close_ask = Some(CloseAsk { saving: true, error: None, ..ask });
                        } else if self.ed.docs.contains_key(&doc) {
                            match self.editor_save_with(doc, None, crate::save_actions::SaveOrigin::Close) {
                                // WSL (#392): Schreiben läuft noch – „wird gespeichert …“, geschlossen wird mit dem Ergebnis.
                                Ok(_) if self.ed.docs.get(&doc).is_some_and(|d| d.disk_busy()) => {
                                    self.ed.close_ask = Some(CloseAsk { saving: true, error: None, ..ask });
                                }
                                Ok(_) => self.editor_close_step(&dir),
                                Err(e) => {
                                    self.ed.close_ask = None;
                                    self.notify_text(e);
                                }
                            }
                        }
                    }
                    Some(1) => {
                        if let Some(d) = self.ed.docs.get_mut(&doc) {
                            d.discard_changes();
                        }
                        let mut ask = ask;
                        ask.tabs.remove(0);
                        ask.saving = false;
                        ask.error = None;
                        self.editor_close_now(g, doc, &dir);
                        self.ed.close_ask = Some(ask);
                        self.editor_close_step(&dir);
                    }
                    Some(_) => self.ed.close_ask = None,
                    None => {}
                }
            }
        }
        // Gehe zu Zeile
        if self.ed.goto.is_some() {
            let max = self.ed.focused_view_and_doc().map(|(_, d)| d.line_count()).unwrap_or(1);
            let mut target = None;
            let mut close = false;
            if let Some(gl) = self.ed.goto.as_mut() {
                let r = style::modal(Id::new("ed_goto")).anchor(Align2::CENTER_TOP, Vec2::new(0.0, 90.0)).show(ctx, |ui| {
                    ui.set_width(300.0);
                    ui.label(format!("{} (1–{max})", lang.t("Gehe zu Zeile", "Go to line")));
                    let resp = ui.add(egui::TextEdit::singleline(&mut gl.text).hint_text(lang.t("Zeile[:Spalte]", "line[:column]")).desired_width(f32::INFINITY));
                    if !gl.focused {
                        resp.request_focus();
                        gl.focused = true;
                    }
                    if ui.input(|i| i.key_pressed(Key::Enter)) {
                        target = super::find::parse_goto(&gl.text, max);
                        close = true;
                    }
                });
                if r.should_close() {
                    close = true;
                }
            }
            if close {
                self.ed.goto = None;
                self.ed.want_focus = true;
            }
            if let Some((line, col)) = target {
                self.ed.goto_line(line, col);
            }
        }
        // Vergleichen mit Revision … (#385)
        if let Some(p) = self.ed.rev_picker.as_mut() {
            match p.show(ctx, lang) {
                Some(super::diff_menu::Picked::Rev(rev)) => {
                    let (path, diff) = (p.path.clone(), p.diff);
                    self.ed.rev_picker = None;
                    self.editor_pick_revision(ctx, path, diff, rev);
                }
                Some(super::diff_menu::Picked::Close) => {
                    self.ed.rev_picker = None;
                    self.ed.want_focus = true;
                }
                None => {}
            }
        }
        let _ = cmds;
    }

    /// Gewählte Revision: im Vergleich die linke Seite ersetzen, sonst einen neuen Vergleich mit dem Dokument öffnen.
    fn editor_pick_revision(&mut self, ctx: &egui::Context, path: PathBuf, diff: Option<u64>, rev: crate::editor_svc::git_base::Rev) {
        use super::diff_tab::{Side, Source, StaticSide, Target};
        use crate::editor_svc::git_base::Rev;
        if let Some(d) = diff.filter(|d| self.ed.diffs.contains_key(d)) {
            let dir = self.dir.clone();
            self.ed.set_diff_left(d, Side::Text(Box::new(StaticSide::new(Source::Git { path, rev }))), &dir);
            return;
        }
        let doc = match self.ed.doc_for_path(&path) {
            Some(d) => d,
            None => self.ed.open_path(&path, false),
        };
        let t = match rev {
            Rev::Head => Target::Head,
            Rev::Index => Target::Index,
            Rev::Commit(c) => Target::Rev(c),
        };
        self.editor_cmd(ctx, Cmd::Compare(doc, t));
    }
}

impl EditorUi {
    /// Ansicht und Dokument des aktiven Reiters der fokussierten Gruppe.
    pub(super) fn focused_view_and_doc(&self) -> Option<(&text::View, &DocEntry)> {
        let t = self.panes.groups.get(&self.panes.focused)?.active_tab()?;
        Some((&t.view, self.docs.get(&t.doc)?))
    }

    /// Cursor des aktiven Reiters auf Zeile/Spalte (ab 0) setzen und dorthin rollen.
    pub(super) fn goto_line(&mut self, line: usize, col: usize) {
        let g = self.panes.focused;
        let Some(t) = self.panes.groups.get_mut(&g).and_then(|grp| grp.active_tab_mut()) else { return };
        if let Some(d) = self.docs.get(&t.doc) {
            t.view.goto(d, line, col);
        }
        self.want_focus = true;
    }
}

// -------------------------------------------------------------------------------------------------------- Bausteine

/// Alt+Umschalt+Buchstabe als Kürzel verbraucht: einen Text desselben Buchstabens im selben Bild verwerfen (manche
/// Tastaturtreiber liefern ihn zusätzlich).
fn drop_alt_text(i: &mut egui::InputState, k: Key) {
    let name = k.name();
    if name.len() == 1
        && let Some(pos) = i.events.iter().position(|e| matches!(e, egui::Event::Text(t) if t.eq_ignore_ascii_case(name)))
    {
        i.events.remove(pos);
    }
}

/// Statusleiste, Ordnerbaum und Teilungen zeichnen (ohne Dialoge); sammelt die Befehle in `cmds`.
pub(super) fn draw(ctx: &egui::Context, ed: &mut EditorUi, theme: &crate::theme::Theme, prefs: &super::EditorPrefs, lang: Lang, cmds: &mut Vec<Cmd>) {
    // Fertige Formatier-Läufe übernehmen (#381) – vor dem Zeichnen, damit das Bild den neuen Text zeigt.
    ed.poll_formats(lang);
    super::format::dialogs(ctx, ed, lang, cmds);
    // Ein Agent hat Reiter oder Teilung gewechselt: Tasten des Nutzers dürfen nicht in dessen Datei landen (#54).
    if std::mem::take(&mut ed.blur)
        && let Some(f) = ctx.memory(|m| m.focused())
        && ed.panes.groups.keys().any(|g| text_id(*g) == f)
    {
        ctx.memory_mut(|m| m.surrender_focus(f));
    }
    // Minimiert (Windows meldet dann 0×0) nichts zeichnen: egui passte sonst die Breite des Ordnerbaums an den fehlenden
    // Platz an und merkte sie sich, und das nicht gezeichnete Textfeld verlöre die Tastatur.
    if !keep_focus_across_deactivation(ctx, ed) {
        return;
    }
    // Fenster bekommt den Fokus: Vergleiche und Änderungsmarken lesen die Stände in git/auf der Platte neu (#385).
    let active = ctx.input(|i| i.focused);
    if active && !ed.was_active {
        ed.generation += 1;
    }
    ed.was_active = active;
    egui::TopBottomPanel::bottom(style::salted(Id::new("ed_status")))
        .exact_height(STATUS_H)
        .frame(egui::Frame::NONE.fill(ctx.style().visuals.panel_fill).inner_margin(Margin::symmetric(10, 0)))
        .show(ctx, |ui| status_bar(ui, ed, lang, prefs, cmds))
        .response
        .spot(Spot::EditorStatus);
    // Problemliste des Smart-Modus (#367) unten im Editor-Reiter.
    if ed.smart.problems_open {
        egui::TopBottomPanel::bottom(style::salted(Id::new("ed_problems"))).resizable(true).default_height(150.0).show(ctx, |ui| super::smart_ui::problems_panel(ui, ed, lang, cmds));
    }
    if ed.smart.window_open {
        super::smart_ui::servers_window(ctx, ed, lang);
    }
    if ed.tree_open {
        use egui::containers::panel::PanelState;
        let id = style::salted(Id::new("ed_tree"));
        let avail = ctx.available_rect().width();
        ed.tree_width = if ed.tree_width.is_finite() { ed.tree_width.clamp(TREE_MIN, TREE_MAX) } else { TREE_DEFAULT };
        // egui merkt sich die Breite des letzten Bilds, auch eine an zu wenig Platz angepasste. Weicht sie von der
        // eigenen ab (nur so möglich, ein Ziehen übernimmt die neue Breite sofort), gilt wieder die eigene.
        if PanelState::load(ctx, id).is_some_and(|s| (s.rect.width() - ed.tree_width).abs() > 0.5) {
            ctx.data_mut(|d| d.remove::<PanelState>(id));
        }
        let r = egui::SidePanel::left(id)
            .resizable(true)
            .default_width(ed.tree_width)
            .width_range(TREE_MIN..=TREE_MAX)
            .frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill).inner_margin(Margin::symmetric(8, 8)))
            .show(ctx, |ui| super::super::panels::fixed_body(ui, |ui| tree_panel(ui, ed, lang, cmds)));
        ed.tree_width = tree_width_seen(ed.tree_width, r.response.rect.width(), avail);
        let _ = r.response.spot(Spot::EditorTree);
    }
    egui::CentralPanel::default().frame(egui::Frame::NONE.fill(theme.bg)).show(ctx, |ui| {
        let rect = ui.available_rect_before_wrap();
        ed.group_rects.clear();
        ed.bar_rects.clear();
        let Some(mut layout) = ed.panes.layout.take() else {
            welcome(ui, rect, ed, lang, cmds);
            return;
        };
        if ed.panes.groups.values().all(|g| g.tabs.is_empty()) {
            ed.panes.layout = Some(layout);
            welcome(ui, rect, ed, lang, cmds);
            return;
        }
        let cx = GroupCx { theme, prefs, lang };
        show_node(ui, &mut layout, rect, ed, &cx, cmds);
        ed.panes.layout = Some(layout);
        drag_overlay(ui, ed, lang, cmds);
    });
}

struct GroupCx<'a> {
    theme: &'a crate::theme::Theme,
    prefs: &'a super::EditorPrefs,
    lang: Lang,
}

fn show_node(ui: &mut Ui, node: &mut Node, rect: Rect, ed: &mut EditorUi, cx: &GroupCx, cmds: &mut Vec<Cmd>) {
    match node {
        Node::Leaf(g) => show_group(ui, *g, rect, ed, cx, cmds),
        Node::Split { dir, ratio, a, b } => {
            let min = 120.0;
            let (ra, rsep, rb) = match dir {
                Dir::Horizontal => {
                    let w = rect.width() - GAP;
                    let wa = super::super::split_px(w, *ratio, min, min);
                    (
                        Rect::from_min_size(rect.min, Vec2::new(wa, rect.height())),
                        Rect::from_min_size(Pos2::new(rect.min.x + wa, rect.min.y), Vec2::new(GAP, rect.height())),
                        Rect::from_min_max(Pos2::new(rect.min.x + wa + GAP, rect.min.y), rect.max),
                    )
                }
                Dir::Vertical => {
                    let h = rect.height() - GAP;
                    let ha = super::super::split_px(h, *ratio, min, min);
                    (
                        Rect::from_min_size(rect.min, Vec2::new(rect.width(), ha)),
                        Rect::from_min_size(Pos2::new(rect.min.x, rect.min.y + ha), Vec2::new(rect.width(), GAP)),
                        Rect::from_min_max(Pos2::new(rect.min.x, rect.min.y + ha + GAP), rect.max),
                    )
                }
            };
            let sep_id = style::salted(Id::new(("ed_sep", a.leaf_ids()[0], b.leaf_ids()[0])));
            let resp = ui.interact(rsep, sep_id, Sense::drag());
            if resp.hovered() || resp.dragged() {
                ui.ctx().set_cursor_icon(match dir {
                    Dir::Horizontal => egui::CursorIcon::ResizeHorizontal,
                    Dir::Vertical => egui::CursorIcon::ResizeVertical,
                });
            }
            if resp.dragged() {
                let d = resp.drag_delta();
                let (delta, len, cur) = match dir {
                    Dir::Horizontal => (d.x, rect.width() - GAP, ra.width()),
                    Dir::Vertical => (d.y, rect.height() - GAP, ra.height()),
                };
                *ratio = super::super::drag_ratio(cur / len.max(1.0), delta, len, min, min);
            }
            let hv = style::anim(ui.ctx(), sep_id.with("h"), resp.hovered() || resp.dragged(), style::D_FAST);
            ui.painter().rect_filled(rsep, 0.0, ui.visuals().panel_fill);
            let line = match dir {
                Dir::Horizontal => Rect::from_center_size(rsep.center(), Vec2::new(1.0 + 2.0 * hv, rsep.height())),
                Dir::Vertical => Rect::from_center_size(rsep.center(), Vec2::new(rsep.width(), 1.0 + 2.0 * hv)),
            };
            ui.painter().rect_filled(line, 0.0, style::mix(style::ui().border, style::brand(), hv));
            show_node(ui, a, ra, ed, cx, cmds);
            show_node(ui, b, rb, ed, cx, cmds);
        }
    }
}

/// Eine Gruppe: Menüleiste (#381), Reiterleiste, Hinweisleisten, Fortschritt/Meldung des Formatierens, ggf.
/// Suchleiste, Textfeld.
fn show_group(ui: &mut Ui, g: u64, rect: Rect, ed: &mut EditorUi, cx: &GroupCx, cmds: &mut Vec<Cmd>) {
    ed.group_rects.insert(g, rect);
    let focused = ed.panes.focused == g;
    let mut top = rect.top();
    if cx.prefs.menu_bar {
        let mr = Rect::from_min_size(rect.min, Vec2::new(rect.width(), super::menu::MENU_H));
        let narrow = super::menu::menu_bar(ui, mr, g, ed, cx.prefs, cx.lang, cmds);
        ed.menu_rects.insert(g, (mr, narrow));
        if focused {
            spot_rect(ui, Spot::EditorMenuBar, mr);
        }
        top = mr.bottom();
    } else {
        ed.menu_rects.remove(&g);
    }
    let bar = Rect::from_min_size(Pos2::new(rect.left(), top), Vec2::new(rect.width(), BAR_H));
    tab_bar(ui, g, bar, ed, cx.lang, cmds);
    let mut body = Rect::from_min_max(Pos2::new(rect.left(), bar.bottom()), rect.max);
    let Some((doc_id, idx, diff)) = ed.panes.groups.get(&g).and_then(|grp| grp.active_tab().map(|t| (t.doc, grp.active, t.diff))) else {
        ui.painter().text(body.center(), Align2::CENTER_CENTER, cx.lang.t("Keine Datei offen", "No file open"), FontId::proportional(13.0), style::dim(ui));
        return;
    };
    // Vergleichs-Reiter (#385)
    if let Some(diff) = diff {
        let want = focused && std::mem::take(&mut ed.want_focus);
        let Some(mut dt) = ed.diffs.remove(&diff) else { return };
        let dcx = super::diff_tab::DiffCx { id: diff_id(g), font: editor_font(ui.ctx(), cx.prefs.font_size), theme: cx.theme, prefs: cx.prefs, lang: cx.lang, wrap: ed.wrap, request_focus: want, diff };
        let out = super::diff_tab::show(ui, body, &mut dt, &mut ed.docs, ed.generation, &dcx, cmds);
        ed.diffs.insert(diff, dt);
        if out.focused || out.clicked {
            ed.panes.focused = g;
        }
        if focused && ed.panes.groups.len() > 1 {
            ui.painter().rect_stroke(rect.shrink(0.5), 0.0, Stroke::new(1.0_f32, style::accent().gamma_multiply(0.5)), StrokeKind::Inside);
        }
        return;
    }
    // Datei erst jetzt lesen (zuerst sichtbar).
    if let Some(d) = ed.docs.get_mut(&doc_id) {
        d.ensure_loaded(cx.prefs);
    }
    // Hinweisleisten
    for bar_kind in info_bars(ed.docs.get(&doc_id), cx.prefs) {
        let r = Rect::from_min_size(body.min, Vec2::new(body.width(), INFO_H));
        info_bar(ui, r, doc_id, bar_kind, cx.lang, cmds);
        body.min.y += INFO_H;
    }
    // Formatieren (#381): Fortschritt mit *Abbrechen*, danach die Meldung (gültig, Fehler mit *Gehe zu*).
    if let Some(j) = ed.fmt.job_for(doc_id) {
        let r = Rect::from_min_size(body.min, Vec2::new(body.width(), super::format::LINE_H));
        let large = ed.docs.get(&doc_id).is_some_and(|d| d.is_large(cx.prefs));
        super::format::progress_line(ui, r, doc_id, j, large, cx.lang, cmds);
        body.min.y += super::format::LINE_H;
    }
    if let Some((n, _)) = ed.fmt.notes.get(&doc_id).cloned() {
        let r = Rect::from_min_size(body.min, Vec2::new(body.width(), super::format::LINE_H));
        super::format::note_line(ui, r, doc_id, &n, cx.lang, cmds);
        body.min.y += super::format::LINE_H;
    }
    // Smart-Modus (#367): Ordner noch nicht vertrauenswürdig – Hinweisleiste (nicht modal), bis dahin Syntax-Modus.
    match ed.smart.state_of(doc_id) {
        super::smart::State::NeedsTrust(root) => {
            let r = Rect::from_min_size(body.min, Vec2::new(body.width(), INFO_H));
            super::smart_ui::trust_bar(ui, r, &root, cx.lang, cmds);
            body.min.y += INFO_H;
        }
        super::smart::State::CrashedOff(lang) => {
            let r = Rect::from_min_size(body.min, Vec2::new(body.width(), INFO_H));
            super::smart_ui::crash_bar(ui, r, &lang, cx.lang, cmds);
            body.min.y += INFO_H;
        }
        _ => {}
    }
    // Suchleiste (nur in der fokussierten Gruppe)
    if focused && ed.find.visible {
        let h = ed.find.height();
        let r = Rect::from_min_size(body.min, Vec2::new(body.width(), h));
        let mut f = std::mem::take(&mut ed.find);
        if let (Some(view), Some(d)) = ed.view_doc_mut(g, idx) {
            super::find::show_bar(ui, r, &mut f, view, d, cx.lang);
        }
        ed.find = f;
        spot_rect(ui, Spot::EditorFind, r);
        if !ed.find.visible {
            ed.want_focus = true;
        }
        body.min.y += h;
    }
    if body.height() < 10.0 {
        return;
    }
    let want = focused && std::mem::take(&mut ed.want_focus);
    let wrap = ed.wrap;
    let minimap = ed.minimap;
    let find_hl = ed.find.highlight();
    let generation = ed.generation;
    let popup_line = ed.mark_popup.as_ref().filter(|p| p.group == g && p.doc == doc_id).map(|p| p.line);
    let diags = ed.smart.shown.get(&doc_id).cloned();
    let Some(d) = ed.docs.get_mut(&doc_id) else { return };
    // Änderungsmarken (#385): im Hintergrund lesen und vergleichen.
    if super::marks::tick(d, cx.prefs, generation, std::time::Instant::now()) {
        ui.ctx().request_repaint_after(Duration::from_millis(60));
    }
    let marks = d.marks.result.clone().filter(|_| cx.prefs.markers);
    let base_git = d.marks.base.as_ref().is_some_and(|b| b.is_git());
    let Some(view) = ed.panes.groups.get_mut(&g).and_then(|grp| grp.tabs.get_mut(idx)).map(|t| &mut t.view) else { return };
    let tcx = TextCx { id: text_id(g), font: editor_font(ui.ctx(), cx.prefs.font_size), theme: cx.theme, prefs: cx.prefs, wrap, minimap, request_focus: want, find: find_hl.as_ref(), lang: cx.lang, marks: marks.as_deref(), popup_line, diags: diags.as_deref() };
    let out = text::show(ui, body, d, view, &tcx);
    // Smart-Modus (#367): Vervollständigen nach der Eingabe, Popup am Cursor.
    if out.edited {
        ed.smart_after_edit(doc_id, g, cx.prefs.smart_auto);
    }
    if let Some(at) = out.caret
        && ed.smart.popup.as_ref().is_some_and(|p| p.group == g && p.doc == doc_id)
    {
        super::smart_ui::completion_popup(ui, at, ed, cx.lang, cmds);
    }
    if let Some(h) = out.hint.clone() {
        cmds.push(Cmd::Hint(h));
    }
    if let Some(line) = out.strip_click {
        let same = ed.mark_popup.as_ref().is_some_and(|p| p.group == g && p.doc == doc_id && p.line == line);
        ed.mark_popup = (!same).then_some(super::marks::MarkPopup { group: g, doc: doc_id, line });
    }
    if let (Some((anchor, width)), Some(m)) = (out.popup_anchor, marks.as_deref())
        && let Some(line) = popup_line
    {
        mark_popup(ui, anchor, width.min(680.0), g, doc_id, line, m, base_git, cx, cmds);
    }
    if focused {
        spot_rect(ui, Spot::EditorText, body);
    }
    if out.focused && ed.panes.focused != g {
        ed.panes.focused = g;
    }
    if out.clicked {
        ed.panes.focused = g;
    }
    // Rahmen der fokussierten Gruppe (bei mehreren)
    if focused && ed.panes.groups.len() > 1 {
        ui.painter().rect_stroke(rect.shrink(0.5), 0.0, Stroke::new(1.0_f32, style::accent().gamma_multiply(0.5)), StrokeKind::Inside);
    }
}

/// Id des Textfelds einer Gruppe (Tastaturfokus).
pub(super) fn text_id(g: u64) -> Id {
    style::salted(Id::new(("ed_text", g)))
}

/// Id der Fläche eines Vergleichs-Reiters einer Gruppe (Tastaturfokus, #385).
pub(super) fn diff_id(g: u64) -> Id {
    style::salted(Id::new(("ed_diff", g)))
}

/// Fenster einer Änderungsmarke unter ihrer Zeile (#385): die alten Zeilen, *Zurücksetzen*, vorige/nächste Änderung,
/// *Im Vergleich öffnen*. Klick daneben oder Esc schließt es.
#[allow(clippy::too_many_arguments)]
fn mark_popup(ui: &mut Ui, anchor: Pos2, width: f32, g: u64, doc: u64, line: usize, m: &crate::editor::linemarks::LineMarks, git: bool, cx: &GroupCx, cmds: &mut Vec<Cmd>) {
    let Some(k) = m.at(line) else {
        cmds.push(Cmd::ClosePopup);
        return;
    };
    let h = &m.hunks[k];
    let lang = cx.lang;
    let u = style::ui();
    let id = style::salted(Id::new(("ed_mark_popup", g)));
    let area = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(anchor).constrain(true).show(ui.ctx(), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_width(width.max(260.0));
            let what = match h.kind {
                crate::editor::compare::HunkKind::Added => lang.t("Hinzugefügt", "Added"),
                crate::editor::compare::HunkKind::Modified => lang.t("Geändert", "Modified"),
                crate::editor::compare::HunkKind::Deleted => lang.t("Entfernt", "Removed"),
            };
            let state = match (git, h.staged) {
                (true, true) => lang.t("gestagt (Index ↔ HEAD)", "staged (index ↔ HEAD)"),
                (true, false) => lang.t("nicht gestagt (Datei ↔ Index)", "unstaged (file ↔ index)"),
                _ => lang.t("ungespeichert (Datei ↔ Platte)", "unsaved (file ↔ disk)"),
            };
            ui.horizontal(|ui| {
                ui.painter().rect_filled(Rect::from_min_size(ui.cursor().min + Vec2::new(0.0, 3.0), Vec2::new(4.0, 12.0)), 1.0, super::marks::color(h, &u));
                ui.add_space(8.0);
                ui.label(RichText::new(format!("{what} · {state}")).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✕").on_hover_text(lang.t("Schließen (Esc)", "Close (Esc)")).clicked() {
                        cmds.push(Cmd::ClosePopup);
                    }
                });
            });
            ui.add_space(2.0);
            let mono = editor_font(ui.ctx(), (cx.prefs.font_size - 1.0).max(9.0));
            if h.old.is_empty() {
                ui.label(RichText::new(lang.t("Neue Zeilen – vorher stand hier nichts.", "New lines – there was nothing here before.")).small().color(style::dim(ui)));
            } else {
                egui::ScrollArea::vertical().max_height(260.0).auto_shrink([false, true]).show(ui, |ui| {
                    for l in h.old.iter().take(2000) {
                        let text = RichText::new(format!("− {l}")).font(mono.clone()).color(cx.theme.fg).background_color(u.diff_remove_bg);
                        ui.add(egui::Label::new(text).extend());
                    }
                });
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let revert = ui.add_enabled(h.revertable, egui::Button::new(lang.t("Zurücksetzen", "Revert")));
                let revert = if h.revertable { revert.on_hover_text(lang.t("Die alten Zeilen wiederherstellen (ein Schritt zum Rückgängigmachen)", "Restore the old lines (one undo step)")) } else { revert.on_disabled_hover_text(lang.t("Die gestagte Änderung wurde danach noch einmal geändert.", "The staged change was changed again afterwards.")) };
                if revert.clicked() {
                    cmds.push(Cmd::RevertMark(doc, line));
                }
                if style::icon_btn_small(ui, Icon::Up, lang.t("Vorige Änderung (Umschalt+F7)", "Previous change (Shift+F7)")).clicked() {
                    cmds.push(Cmd::StepChange(false));
                }
                if style::icon_btn_small(ui, Icon::Down, lang.t("Nächste Änderung (F7)", "Next change (F7)")).clicked() {
                    cmds.push(Cmd::StepChange(true));
                }
                if ui.button(lang.t("Im Vergleich öffnen", "Open in diff")).clicked() {
                    cmds.push(Cmd::MarkToDiff(doc, line));
                }
            });
        });
    });
    let r = area.response.rect;
    let (pressed, pos, esc) = ui.input(|i| (i.pointer.any_pressed(), i.pointer.interact_pos(), i.key_pressed(Key::Escape)));
    if esc || (pressed && pos.is_some_and(|p| !r.contains(p) && p.x > anchor.x - 40.0 && !(p.y > anchor.y - 30.0 && p.y < anchor.y && p.x < anchor.x))) {
        cmds.push(Cmd::ClosePopup);
    }
}

/// Tastatur über einen Fensterwechsel retten (#54): Wird das Fenster inaktiv oder minimiert, merkt es sich (je Fenster),
/// welches Textfeld des Editors die Tastatur hatte. Ist es wieder aktiv und hat inzwischen nichts anderes die Tastatur
/// bekommen (kein Dialog, kein anderes Feld, kein Klick in diesem Bild, kein Reiterwechsel durch einen Agenten), bekommt
/// dieses Textfeld sie zurück – wie bei den Terminals. Ein Agent bekommt so nie die Tastatur: zurück geht sie nur an das
/// Feld, das sie vorher hatte. Liefert, ob der Editor gezeichnet werden soll (nicht minimiert, nicht winzig).
pub(super) fn keep_focus_across_deactivation(ctx: &egui::Context, ed: &mut EditorUi) -> bool {
    let key = Id::new("pd_ed_away");
    let (active, minimized, pressed) = ctx.input(|i| (i.focused, i.viewport().minimized == Some(true), i.pointer.any_pressed()));
    let size = ctx.content_rect().size();
    let visible = !minimized && size.x >= MIN_DRAW && size.y >= MIN_DRAW;
    let focused = ctx.memory(|m| m.focused());
    let mine = focused.and_then(|f| ed.panes.groups.keys().copied().find(|g| text_id(*g) == f));
    let away: Option<(Option<u64>, u64)> = ctx.data(|d| d.get_temp(key));
    if !active || !visible {
        if away.is_none() {
            ctx.data_mut(|d| d.insert_temp(key, (mine, ed.agent_switches)));
        }
    } else if let Some((back, switches)) = away {
        ctx.data_mut(|d| d.remove::<(Option<u64>, u64)>(key));
        if let Some(g) = back
            && focused.is_none()
            && !pressed
            && switches == ed.agent_switches
            && ed.close_ask.is_none()
            && ed.goto.is_none()
            && ed.panes.groups.contains_key(&g)
        {
            ed.panes.focused = g;
            ed.want_focus = true;
        }
    }
    visible
}

/// Breite des Ordnerbaums nach einem Bild: Hat egui sie an zu wenig Platz angepasst (minimiert, sehr schmales Fenster –
/// der Baum nimmt dann den ganzen Platz), bleibt die bisherige; sonst gilt die gezeichnete (auch nach dem Ziehen).
pub(super) fn tree_width_seen(prev: f32, drawn: f32, avail: f32) -> f32 {
    if !drawn.is_finite() || avail < TREE_MIN + 1.0 || drawn + 1.0 >= avail { prev } else { drawn.clamp(TREE_MIN, TREE_MAX) }
}

/// Schrift des Textfelds: die eigene Familie „editor“ (Einstellungen → Editor; gleich der Terminalschrift, solange dort
/// nichts gewählt ist), wenn sie angemeldet ist, sonst die Monospace-Schrift des Terminals.
fn editor_font(ctx: &egui::Context, size: f32) -> FontId {
    let fam = egui::FontFamily::Name("editor".into());
    if ctx.fonts(|f| f.families().contains(&fam)) { FontId::new(size, fam) } else { FontId::monospace(size) }
}

impl EditorUi {
    /// Ansicht des Reiters `idx` der Gruppe `g` und das Dokument dazu (getrennt ausgeliehen).
    pub(super) fn view_doc_mut(&mut self, g: u64, idx: usize) -> (Option<&mut text::View>, Option<&mut DocEntry>) {
        let Some(t) = self.panes.groups.get_mut(&g).and_then(|grp| grp.tabs.get_mut(idx)) else { return (None, None) };
        let doc = t.doc;
        (Some(&mut t.view), self.docs.get_mut(&doc))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum InfoKind {
    LoadError,
    DiskConflict,
    Deleted,
    Large,
    Agent,
}

fn info_bars(d: Option<&DocEntry>, prefs: &super::EditorPrefs) -> Vec<InfoKind> {
    let Some(d) = d else { return Vec::new() };
    let mut v = Vec::new();
    if d.load_error.is_some() {
        v.push(InfoKind::LoadError);
    }
    if d.disk_conflict {
        v.push(InfoKind::DiskConflict);
    }
    if d.disk_deleted {
        v.push(InfoKind::Deleted);
    }
    if d.is_large(prefs) && !d.large_dismissed {
        v.push(InfoKind::Large);
    }
    if d.agent_edited && !d.agent_note_dismissed {
        v.push(InfoKind::Agent);
    }
    v
}

fn info_bar(ui: &mut Ui, r: Rect, doc: u64, kind: InfoKind, lang: Lang, cmds: &mut Vec<Cmd>) {
    let u = style::ui();
    let col = match kind {
        InfoKind::LoadError => u.error,
        InfoKind::DiskConflict | InfoKind::Deleted => u.warning,
        InfoKind::Large => u.info,
        InfoKind::Agent => u.info,
    };
    ui.painter().rect_filled(r, 0.0, col.gamma_multiply(0.16));
    ui.painter().line_segment([r.left_bottom(), r.right_bottom()], Stroke::new(1.0_f32, col.gamma_multiply(0.5)));
    ui.painter().circle_filled(Pos2::new(r.left() + 14.0, r.center().y), 3.5, col);
    let (text, buttons) = info_content(kind, doc, lang);
    // Knöpfe rechts zuerst, der Text nimmt den Rest und wird gekürzt (ganzer Text beim Überfahren). In schmalen Teilungen
    // tragen die Knöpfe Kurzformen, damit sie nicht abgeschnitten werden.
    let inner = r.shrink2(Vec2::new(10.0, 3.0));
    let font = egui::TextStyle::Button.resolve(ui.style());
    let pad = ui.spacing().button_padding.x * 2.0 + ui.spacing().item_spacing.x;
    let full_w: f32 = buttons.iter().map(|b| ui.painter().layout_no_wrap(b.full.to_string(), font.clone(), Color32::PLACEHOLDER).size().x + pad).sum();
    let narrow = info_narrow(inner.width(), full_w);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::right_to_left(egui::Align::Center)));
    let ui = &mut child;
    for b in buttons.iter().rev() {
        let resp = ui.small_button(if narrow { b.short } else { b.full });
        let resp = if narrow { resp.on_hover_text(b.full) } else { resp };
        if resp.clicked() {
            cmds.push(b.cmd.clone());
        }
    }
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.add_space(14.0);
        ui.add(egui::Label::new(text).truncate());
    });
}

/// Knopf einer Hinweisleiste: voller Text, Kurzform für schmale Teilungen, Befehl.
struct InfoButton {
    full: &'static str,
    short: &'static str,
    cmd: Cmd,
}

/// Text und Knöpfe einer Hinweisleiste.
fn info_content(kind: InfoKind, doc: u64, lang: Lang) -> (&'static str, Vec<InfoButton>) {
    let b = |full, short, cmd| InfoButton { full, short, cmd };
    match kind {
        InfoKind::LoadError => (lang.t("Datei konnte nicht gelesen werden.", "The file could not be read."), Vec::new()),
        InfoKind::DiskConflict => (
            lang.t("Die Datei wurde auf der Platte geändert, hier gibt es ungespeicherte Änderungen.", "The file changed on disk and has unsaved changes here."),
            vec![
                b(lang.t("Meine behalten", "Keep mine"), lang.t("Meine", "Mine"), Cmd::KeepMine(doc)),
                b(lang.t("Von der Platte laden", "Load from disk"), lang.t("Laden", "Load"), Cmd::Reload(doc)),
                b(lang.t("Unterschied anzeigen", "Show difference"), lang.t("Vergleich", "Diff"), Cmd::ShowDiff(doc)),
            ],
        ),
        InfoKind::Deleted => (
            lang.t("Die Datei wurde auf der Platte gelöscht. Speichern legt sie neu an.", "The file was deleted on disk. Saving creates it again."),
            vec![b(lang.t("Speichern", "Save"), lang.t("Speichern", "Save"), Cmd::Save(doc))],
        ),
        InfoKind::Large => (
            lang.t("Große Datei: Minimap und Zeilenumbruch sind aus.", "Large file: minimap and word wrap are off."),
            vec![b(lang.t("Ausblenden", "Dismiss"), lang.t("Ausblenden", "Dismiss"), Cmd::DismissLarge(doc))],
        ),
        InfoKind::Agent => (
            lang.t("Von einem Agenten geändert – noch nicht gespeichert. Strg+Z macht es rückgängig.", "Changed by an agent – not saved yet. Ctrl+Z undoes it."),
            vec![b(lang.t("Ausblenden", "Dismiss"), lang.t("Ausblenden", "Dismiss"), Cmd::DismissAgent(doc))],
        ),
    }
}

/// Kurzformen der Knöpfe, wenn neben den vollen Knöpfen weniger als 200 px für den Text bleiben.
pub(super) fn info_narrow(width: f32, buttons_w: f32) -> bool {
    width - buttons_w < 200.0
}

/// Reiterleiste einer Gruppe: Reiter (Name, ungespeichert-Punkt, Schließen beim Überfahren), Ziehen zwischen Leisten und
/// in neue Teilungen, Mittelklick schließt, Kontextmenü; rechts Teilen-Knöpfe.
fn tab_bar(ui: &mut Ui, g: u64, bar: Rect, ed: &mut EditorUi, lang: Lang, cmds: &mut Vec<Cmd>) {
    let u = style::ui();
    ui.painter().rect_filled(bar, 0.0, u.surface);
    ui.painter().line_segment([bar.left_bottom() - Vec2::new(0.0, 0.5), bar.right_bottom() - Vec2::new(0.0, 0.5)], Stroke::new(1.0_f32, u.border));
    let Some(grp) = ed.panes.groups.get(&g) else { return };
    let focused = ed.panes.focused == g;
    // Knöpfe rechts
    let btn_w = 24.0;
    let btn_r = Rect::from_min_max(Pos2::new(bar.right() - btn_w * 2.0 - 6.0, bar.top()), bar.max);
    let tabs_r = Rect::from_min_max(bar.min, Pos2::new(btn_r.left(), bar.bottom()));
    let n = grp.tabs.len();
    let font = FontId::proportional(12.5);
    let titles: Vec<(String, bool, bool, Option<PathBuf>)> = grp
        .tabs
        .iter()
        .map(|t| match (t.diff.and_then(|d| ed.diffs.get(&d)), ed.docs.get(&t.doc)) {
            (Some(dt), _) => (dt.title(&ed.docs, lang), dt.dirty(&ed.docs), false, None),
            (None, Some(d)) => (d.title(lang), d.dirty(), d.agent_edited || d.opened_by_agent, d.path.clone()),
            (None, None) => ("?".into(), false, false, None),
        })
        .collect();
    // Aktionen beim Speichern (#383): roter Punkt, bis ein Lauf für die Datei wieder klappt.
    let failed: Vec<bool> = titles.iter().map(|(.., p)| p.as_ref().is_some_and(|p| ed.on_save.failed.contains(&crate::save_actions::path_key(p)))).collect();
    let widths: Vec<f32> = titles.iter().map(|(t, ..)| (ui.painter().layout_no_wrap(t.clone(), font.clone(), Color32::PLACEHOLDER).size().x + 44.0).clamp(TAB_MIN_W, TAB_MAX_W)).collect();
    let total: f32 = widths.iter().sum();
    // Bildlauf der Leiste, wenn die Reiter nicht passen (Mausrad über der Leiste); der aktive bleibt sichtbar.
    let off_id = style::salted(Id::new(("ed_bar_off", g)));
    let mut off: f32 = ui.data(|d| d.get_temp(off_id)).unwrap_or(0.0);
    let room = tabs_r.width();
    if total > room {
        if ui.rect_contains_pointer(tabs_r) {
            let s = ui.input(|i| i.smooth_scroll_delta);
            off -= s.x + s.y;
        }
        let a0: f32 = widths.iter().take(grp.active).sum();
        let a1 = a0 + widths.get(grp.active).copied().unwrap_or(0.0);
        if ed.panes.focused == g && (a0 < off || a1 > off + room) && !ui.rect_contains_pointer(tabs_r) {
            off = if a0 < off { a0 } else { a1 - room };
        }
        off = off.clamp(0.0, total - room);
    } else {
        off = 0.0;
    }
    ui.data_mut(|d| d.insert_temp(off_id, off));
    let painter = ui.painter().with_clip_rect(tabs_r);
    let mut x = tabs_r.left() - off;
    let mut rects = Vec::with_capacity(n);
    for (i, (title, dirty, agent, path)) in titles.iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(x, bar.top()), Vec2::new(widths[i], BAR_H));
        x += widths[i];
        rects.push(r);
        let vis = r.intersect(tabs_r);
        if vis.width() <= 1.0 {
            continue;
        }
        let id = style::salted(Id::new(("ed_tab", g, i)));
        let resp = ui.interact(vis, id, Sense::click_and_drag());
        let active = i == grp.active;
        let hovered = resp.hovered();
        if active {
            painter.rect_filled(r, 0.0, ui.visuals().extreme_bg_color.gamma_multiply(0.0).linear_multiply(0.0));
            painter.rect_filled(r, 0.0, u.tab_active.gamma_multiply(if focused { 0.55 } else { 0.3 }));
            painter.line_segment([Pos2::new(r.left(), r.top() + 1.0), Pos2::new(r.right(), r.top() + 1.0)], Stroke::new(2.0_f32, if focused { style::accent() } else { u.border }));
        } else if hovered {
            painter.rect_filled(r, 0.0, u.hover_bg);
        }
        painter.line_segment([r.right_top(), r.right_bottom()], Stroke::new(1.0_f32, u.border.gamma_multiply(0.6)));
        let tc = if active { ui.visuals().strong_text_color() } else { ui.visuals().text_color().gamma_multiply(0.8) };
        let tc = if *agent { style::mix(tc, u.info, 0.6) } else { tc };
        let lead = if failed[i] { 10.0 } else { 0.0 };
        if failed[i] {
            painter.circle_filled(Pos2::new(r.left() + 9.0, r.center().y), 3.5, u.danger);
        }
        let text_r = Rect::from_min_max(Pos2::new(r.left() + 10.0 + lead, r.top()), Pos2::new(r.right() - 26.0, r.bottom()));
        let gal = painter.layout_no_wrap(title.clone(), font.clone(), tc);
        painter.with_clip_rect(text_r.intersect(tabs_r)).galley(Pos2::new(text_r.left(), r.center().y - gal.size().y / 2.0), gal, tc);
        // Rechts: Schließen beim Überfahren, sonst Punkt bei ungespeicherten Änderungen
        let close_r = Rect::from_center_size(Pos2::new(r.right() - 14.0, r.center().y), Vec2::splat(18.0));
        let close_hover = ui.rect_contains_pointer(close_r.intersect(tabs_r)) && hovered;
        if hovered || (active && !dirty) {
            if close_hover {
                painter.rect_filled(close_r, 4.0, u.hover_bg);
            }
            let c = close_r.center();
            let col = if close_hover { ui.visuals().strong_text_color() } else { style::dim(ui) };
            let st = Stroke::new(1.3_f32, col);
            painter.line_segment([c + Vec2::new(-3.5, -3.5), c + Vec2::new(3.5, 3.5)], st);
            painter.line_segment([c + Vec2::new(3.5, -3.5), c + Vec2::new(-3.5, 3.5)], st);
        } else if *dirty {
            painter.circle_filled(close_r.center(), 3.5, tc);
        }
        let tip = tab_tooltip(path.as_deref(), *dirty, *agent, lang);
        let tip = if failed[i] { format!("{tip}\n{}", lang.t("Aktionen beim Speichern: letzter Lauf mit Fehler", "Actions on save: last run had an error")) } else { tip };
        resp.clone().on_hover_text(tip);
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let doc = grp.tabs[i].doc;
        let tab_diff = grp.tabs[i].diff;
        let close_cmd = match tab_diff {
            Some(d) => Cmd::CloseDiff(g, d),
            None => Cmd::Close(vec![(g, doc)]),
        };
        if resp.clicked() {
            if close_hover {
                cmds.push(close_cmd.clone());
            } else {
                cmds.push(Cmd::Activate(g, i));
            }
        }
        if resp.middle_clicked() {
            cmds.push(close_cmd.clone());
        }
        if resp.drag_started() && ed.drag.is_none() {
            ed.drag = Some(TabDrag { group: g, index: i, doc });
        }
        // Schließen mehrerer Reiter: Datei-Reiter über die Rückfrage, Vergleiche direkt.
        let closing = |keep: Option<usize>| -> Vec<Cmd> {
            let files: Vec<(u64, u64)> = grp.tabs.iter().enumerate().filter(|(j, t)| Some(*j) != keep && t.diff.is_none()).map(|(_, t)| (g, t.doc)).collect();
            let mut v: Vec<Cmd> = grp.tabs.iter().enumerate().filter(|(j, _)| Some(*j) != keep).filter_map(|(_, t)| t.diff.map(|d| Cmd::CloseDiff(g, d))).collect();
            if !files.is_empty() {
                v.push(Cmd::Close(files));
            }
            v
        };
        let others = closing(Some(i));
        let all = closing(None);
        let path = path.clone();
        resp.context_menu(|ui| {
            if ui.button(lang.t("Schließen", "Close")).clicked() {
                cmds.push(close_cmd.clone());
                ui.close();
            }
            if ui.add_enabled(!others.is_empty(), egui::Button::new(lang.t("Andere schließen", "Close others"))).clicked() {
                cmds.extend(others.iter().cloned());
                ui.close();
            }
            if ui.button(lang.t("Alle schließen", "Close all")).clicked() {
                cmds.extend(all.iter().cloned());
                ui.close();
            }
            match tab_diff {
                Some(d) => {
                    ui.separator();
                    if ui.button(lang.t("Seiten tauschen", "Swap sides")).clicked() {
                        cmds.push(Cmd::SwapDiff(d));
                        ui.close();
                    }
                }
                None => {
                    ui.separator();
                    super::diff_menu::compare_submenu(ui, ed, doc, lang, cmds);
                }
            }
            ui.separator();
            if ui.button(lang.t("Rechts teilen", "Split right")).clicked() {
                cmds.push(Cmd::Activate(g, i));
                cmds.push(Cmd::Split(g, Dir::Horizontal));
                ui.close();
            }
            if ui.button(lang.t("Unten teilen", "Split down")).clicked() {
                cmds.push(Cmd::Activate(g, i));
                cmds.push(Cmd::Split(g, Dir::Vertical));
                ui.close();
            }
            if let Some(p) = &path {
                ui.separator();
                if ui.button(lang.t("Pfad kopieren", "Copy path")).clicked() {
                    cmds.push(Cmd::CopyPath(p.clone()));
                    ui.close();
                }
                if crate::wsl::is_wsl_unc(&p.to_string_lossy()) && ui.button(lang.t("Als Linux-Pfad kopieren", "Copy as Linux path")).clicked() {
                    cmds.push(Cmd::CopyLinuxPath(p.clone()));
                    ui.close();
                }
                if crate::platform::open::FILE_MANAGER && ui.button(if cfg!(windows) { lang.t("Im Explorer anzeigen", "Show in Explorer") } else { lang.t("Im Dateimanager anzeigen", "Show in file manager") }).clicked() {
                    crate::platform::open::show_in_file_manager(&p.to_string_lossy(), false);
                    ui.close();
                }
            }
        });
    }
    ed.bar_rects.insert(g, (tabs_r, rects));
    if focused {
        spot_rect(ui, Spot::EditorTabBar, tabs_r);
    }
    // Teilen-Knöpfe
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(btn_r.shrink2(Vec2::new(2.0, 4.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
    if style::icon_btn_small(&mut child, Icon::SplitDown, lang.t("Unten teilen", "Split down")).clicked() {
        cmds.push(Cmd::Split(g, Dir::Vertical));
    }
    let split = style::icon_btn_small(&mut child, Icon::SplitRight, lang.t("Rechts teilen", "Split right"));
    let split = if focused { split.spot(Spot::EditorSplit) } else { split };
    if split.clicked() {
        cmds.push(Cmd::Split(g, Dir::Horizontal));
    }
}

fn tab_tooltip(path: Option<&Path>, dirty: bool, agent: bool, lang: Lang) -> String {
    let mut s = path.map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| lang.t("Noch nicht gespeichert", "Not saved yet").to_string());
    if dirty {
        s.push('\n');
        s.push_str(lang.t("Ungespeicherte Änderungen", "Unsaved changes"));
    }
    if agent {
        s.push('\n');
        s.push_str(lang.t("Von einem Agenten geöffnet oder geändert", "Opened or changed by an agent"));
    }
    s
}

/// Ziehen eines Reiters: Geist am Zeiger, Ablageziel hervorheben, beim Loslassen ablegen.
fn drag_overlay(ui: &mut Ui, ed: &mut EditorUi, lang: Lang, cmds: &mut Vec<Cmd>) {
    let Some(drag) = ed.drag else { return };
    let (pos, released, down) = ui.input(|i| (i.pointer.interact_pos(), i.pointer.any_released(), i.pointer.any_down()));
    let target = pos.and_then(|p| drop_target(ed, p));
    let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("ed_drag")));
    if let Some((_, r)) = target {
        painter.rect_filled(r, 4.0, style::accent().gamma_multiply(0.18));
        painter.rect_stroke(r, 4.0, Stroke::new(2.0_f32, style::accent()), StrokeKind::Inside);
    }
    if let Some(p) = pos {
        let diff = ed.panes.groups.get(&drag.group).and_then(|grp| grp.tabs.get(drag.index)).and_then(|t| t.diff).and_then(|d| ed.diffs.get(&d));
        let name = match diff {
            Some(dt) => dt.title(&ed.docs, lang),
            None => ed.docs.get(&drag.doc).map(|d| d.title(lang)).unwrap_or_default(),
        };
        let g = painter.layout_no_wrap(name, FontId::proportional(12.5), ui.visuals().strong_text_color());
        let r = Rect::from_min_size(p + Vec2::new(12.0, 8.0), g.size() + Vec2::new(16.0, 8.0));
        painter.rect_filled(r, CornerRadius::same(5), style::ui().window_bg);
        painter.rect_stroke(r, CornerRadius::same(5), Stroke::new(1.0_f32, style::accent()), StrokeKind::Inside);
        painter.galley(r.min + Vec2::new(8.0, 4.0), g, Color32::PLACEHOLDER);
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    }
    if released || !down {
        ed.drag = None;
        if let Some((to, _)) = target {
            cmds.push(Cmd::Drop(drag, to));
        }
    }
    ui.ctx().request_repaint();
}

/// Ablageziel unter dem Zeiger: Reiterleiste (Position zwischen Reitern) oder Rand/Mitte einer Gruppe.
pub(super) fn drop_target(ed: &EditorUi, p: Pos2) -> Option<(PaneDrop, Rect)> {
    for (g, (bar, tabs)) in &ed.bar_rects {
        // Die Menüleiste darüber (#381) zählt wie die Reiterleiste.
        let over_menu = ed.menu_rects.get(g).is_some_and(|(m, _)| m.contains(p));
        if bar.contains(p) || over_menu {
            let idx = tabs.iter().position(|r| p.x < r.center().x).unwrap_or(tabs.len());
            let x = tabs.get(idx).map(|r| r.left()).or_else(|| tabs.last().map(|r| r.right())).unwrap_or(bar.left());
            return Some((PaneDrop::Bar { group: *g, index: idx }, Rect::from_center_size(Pos2::new(x, bar.center().y), Vec2::new(4.0, bar.height()))));
        }
    }
    for (g, r) in &ed.group_rects {
        if !r.contains(p) {
            continue;
        }
        // Unter Menü- und Reiterleiste (die Reiterleiste kennt ihre Lage aus dem letzten Bild).
        let top = ed.bar_rects.get(g).map(|(b, _)| b.bottom()).unwrap_or(r.top() + BAR_H);
        let body = Rect::from_min_max(Pos2::new(r.left(), top), r.max);
        let (fx, fy) = ((p.x - body.left()) / body.width().max(1.0), (p.y - body.top()) / body.height().max(1.0));
        let edge = 0.25;
        let half = |dir: Dir, before: bool| -> Rect {
            match (dir, before) {
                (Dir::Horizontal, true) => Rect::from_min_max(body.min, Pos2::new(body.center().x, body.bottom())),
                (Dir::Horizontal, false) => Rect::from_min_max(Pos2::new(body.center().x, body.top()), body.max),
                (Dir::Vertical, true) => Rect::from_min_max(body.min, Pos2::new(body.right(), body.center().y)),
                (Dir::Vertical, false) => Rect::from_min_max(Pos2::new(body.left(), body.center().y), body.max),
            }
        };
        let split = if fx < edge {
            Some((Dir::Horizontal, true))
        } else if fx > 1.0 - edge {
            Some((Dir::Horizontal, false))
        } else if fy < edge {
            Some((Dir::Vertical, true))
        } else if fy > 1.0 - edge {
            Some((Dir::Vertical, false))
        } else {
            None
        };
        return Some(match split {
            Some((dir, before)) => (PaneDrop::Split { group: *g, dir, before }, half(dir, before)),
            None => {
                let n = ed.panes.groups.get(g).map(|x| x.tabs.len()).unwrap_or(0);
                (PaneDrop::Bar { group: *g, index: n }, body)
            }
        });
    }
    None
}

/// Ordnerbaum mit Kopf; ohne Ordner: Knöpfe und zuletzt geöffnete Ordner.
fn tree_panel(ui: &mut Ui, ed: &mut EditorUi, lang: Lang, cmds: &mut Vec<Cmd>) {
    ui.horizontal(|ui| {
        let name = ed.root.as_ref().and_then(|r| r.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| lang.t("Kein Ordner", "No folder").to_string());
        ui.add(egui::Label::new(RichText::new(name).strong()).truncate()).on_hover_text(ed.root.as_ref().map(|r| r.to_string_lossy().to_string()).unwrap_or_default());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if style::icon_btn_small(ui, Icon::FolderPlus, lang.t("Ordner öffnen … (Strg+Umschalt+O)", "Open folder … (Ctrl+Shift+O)")).clicked() {
                cmds.push(Cmd::OpenFolderDialog);
            }
            if ed.root.is_some() && style::icon_btn_small(ui, Icon::Reload, lang.t("Aktualisieren", "Refresh")).clicked() {
                ed.tree.refresh();
            }
        });
    });
    if ed.root.is_some() {
        ui.horizontal(|ui| {
            if ui.checkbox(&mut ed.tree.hidden, lang.t("versteckte", "hidden")).changed() {
                ed.tree.refresh();
            }
            if ui.small_button(lang.t("Ordner schließen", "Close folder")).clicked() {
                cmds.push(Cmd::CloseRoot);
            }
        });
    }
    ui.add_space(style::S1);
    ui.separator();
    match ed.root.clone() {
        Some(root) => {
            egui::ScrollArea::vertical().id_salt("ed_tree_scroll").auto_shrink([false, false]).show(ui, |ui| {
                for o in ed.tree.show(ui, &root, lang) {
                    cmds.push(match o {
                        TreeOut::Open(p) => Cmd::OpenPath(p, false),
                        TreeOut::OpenSide(p) => Cmd::OpenPath(p, true),
                        TreeOut::CopyPath(p) => Cmd::CopyPath(p),
                        TreeOut::CopyLinuxPath(p) => Cmd::CopyLinuxPath(p),
                        TreeOut::SetRoot(p) => Cmd::SetRoot(p),
                        TreeOut::RulesFor(p) => {
                            ed.on_save.clicked = Some(super::super::save_actions_ui::OnSaveClick::RulesFor(p));
                            continue;
                        }
                        TreeOut::NewRule(p, dir) => {
                            ed.on_save.clicked = Some(super::super::save_actions_ui::OnSaveClick::NewRule(p, dir));
                            continue;
                        }
                        TreeOut::Compare(a, b) => Cmd::ComparePaths(a, b),
                    });
                }
            });
        }
        None => {
            egui::ScrollArea::vertical().id_salt("ed_tree_recent").auto_shrink([false, false]).show(ui, |ui| recent_lists(ui, ed, lang, cmds));
        }
    }
}

/// Zuletzt geöffnete Ordner und Dateien.
fn recent_lists(ui: &mut Ui, ed: &EditorUi, lang: Lang, cmds: &mut Vec<Cmd>) {
    if ui.button(lang.t("Ordner öffnen …", "Open folder …")).clicked() {
        cmds.push(Cmd::OpenFolderDialog);
    }
    if ui.button(lang.t("Datei öffnen …", "Open file …")).clicked() {
        cmds.push(Cmd::OpenFileDialog);
    }
    for (title, list, folders) in [(lang.t("Zuletzt geöffnete Ordner", "Recent folders"), &ed.recent_folders, true), (lang.t("Zuletzt geöffnete Dateien", "Recent files"), &ed.recent_files, false)] {
        if list.is_empty() {
            continue;
        }
        ui.add_space(style::S3);
        ui.weak(title);
        for p in list {
            let name = Path::new(p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.clone());
            let r = ui.add(egui::Button::new(name).frame(false)).on_hover_text(p);
            if r.clicked() {
                cmds.push(if folders { Cmd::SetRoot(PathBuf::from(p)) } else { Cmd::OpenPath(PathBuf::from(p), false) });
            }
        }
    }
    if !ed.recent_files.is_empty() || !ed.recent_folders.is_empty() {
        ui.add_space(style::S2);
        if ui.small_button(lang.t("Liste leeren", "Clear list")).clicked() {
            cmds.push(Cmd::ForgetRecent);
        }
    }
}

/// Leerer Editor: Einstieg mit Knöpfen, zuletzt geöffneten Einträgen und Tastenkürzeln.
fn welcome(ui: &mut Ui, rect: Rect, ed: &EditorUi, lang: Lang, cmds: &mut Vec<Cmd>) {
    let w = 380.0_f32.min(rect.width() - 32.0).max(100.0);
    let r = Rect::from_center_size(Pos2::new(rect.center().x, rect.top() + (rect.height() * 0.38).max(160.0)), Vec2::new(w, 320.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::top_down(egui::Align::Min)));
    let ui = &mut child;
    ui.heading(lang.t("Editor", "Editor"));
    ui.weak(lang.t("Einfacher Texteditor – unabhängig von den Terminals.", "Plain text editor – independent of the terminals."));
    ui.add_space(style::S3);
    ui.horizontal(|ui| {
        if ui.button(lang.t("Neue Datei", "New file")).clicked() {
            cmds.push(Cmd::NewFile);
        }
        if ui.button(lang.t("Datei öffnen …", "Open file …")).clicked() {
            cmds.push(Cmd::OpenFileDialog);
        }
        if ui.button(lang.t("Ordner öffnen …", "Open folder …")).clicked() {
            cmds.push(Cmd::OpenFolderDialog);
        }
    });
    ui.add_space(style::S3);
    let files: Vec<&String> = ed.recent_files.iter().take(8).collect();
    if !files.is_empty() {
        ui.weak(lang.t("Zuletzt geöffnet", "Recent"));
        for p in files {
            let name = Path::new(p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.clone());
            if ui.add(egui::Button::new(name).frame(false)).on_hover_text(p).clicked() {
                cmds.push(Cmd::OpenPath(PathBuf::from(p), false));
            }
        }
        ui.add_space(style::S3);
    }
    let de = lang == Lang::De;
    for (k, t) in [
        ("Ctrl+N", lang.t("Neue Datei", "New file")),
        ("Ctrl+O", lang.t("Datei öffnen", "Open file")),
        ("Ctrl+Shift+O", lang.t("Ordner öffnen", "Open folder")),
        ("Ctrl+B", lang.t("Ordnerbaum ein/aus", "Toggle folder tree")),
    ] {
        ui.horizontal(|ui| {
            ui.label(RichText::new(crate::keys::pretty(k, de)).monospace().weak());
            ui.weak(t);
        });
    }
}

/// Statusleiste: Zeile:Spalte, Auswahlen, Einrückung, Kodierung und Zeilenende (anklickbar), Modus und Sprache
/// (anklickbar, #312), Hinweise.
fn status_bar(ui: &mut Ui, ed: &mut EditorUi, lang: Lang, prefs: &super::EditorPrefs, cmds: &mut Vec<Cmd>) {
    let syn = ed.active_doc().and_then(|id| ed.docs.get_mut(&id)).filter(|d| d.doc.is_some()).map(|d| d.syntax_status(prefs, lang));
    ui.horizontal_centered(|ui| {
        let doc = ed.active_doc();
        // Mehrere Cursor nennt schon `position_text` („3 Cursor“ / „3 Auswahlen“, #394) – keine zweite Anzeige.
        let info: Option<(StatusInfo, String)> = ed.focused_view_and_doc().and_then(|(v, d)| d.status(prefs, lang).map(|s| (s, v.position_text(d, lang))));
        if let Some((_, pos)) = &info {
            ui.label(RichText::new(pos).small());
        }
        if let Some((t, _)) = &ed.hint {
            ui.add_space(style::S3);
            ui.label(RichText::new(t).small().color(style::ui().info));
        }
        on_save_status(ui, ed, doc, lang);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let (Some(id), Some((s, ..)), Some(syn)) = (doc, info, syn) else { return };
            language_picker(ui, ed, id, &syn, lang, cmds);
            mode_picker(ui, ed, id, &syn, prefs, lang, cmds);
            super::smart_ui::status_chip(ui, ed, id, lang, cmds);
            ui.add_space(style::S2);
            let eol = ui.add(egui::Button::new(RichText::new(if s.crlf { "CRLF" } else { "LF" }).small()).frame(false)).on_hover_text(lang.t("Zeilenende ändern", "Change line endings"));
            egui::Popup::menu(&eol).show(|ui| {
                for (crlf, label) in [(false, "LF"), (true, "CRLF")] {
                    if ui.selectable_label(s.crlf == crlf, label).clicked() {
                        cmds.push(Cmd::SetEol(id, crlf));
                        ui.close();
                    }
                }
            });
            let enc = ui.add(egui::Button::new(RichText::new(&s.encoding).small()).frame(false)).on_hover_text(lang.t("Kodierung ändern (beim nächsten Speichern)", "Change encoding (applies on next save)"));
            egui::Popup::menu(&enc).show(|ui| {
                for (key, label) in super::doc::ENCODINGS {
                    if ui.selectable_label(s.encoding_key == *key, *label).clicked() {
                        cmds.push(Cmd::SetEncoding(id, key));
                        ui.close();
                    }
                }
            });
            ui.label(RichText::new(&s.indent).small()).on_hover_text(lang.t("Einrückung (erkannt bzw. aus .editorconfig)", "Indentation (detected or from .editorconfig)"));
        });
    });
}

/// Aktionen beim Speichern in der Statusleiste (#383): „⟳ 2 Aktionen“ während Läufen, rot bei einem Fehler der aktiven
/// Datei (Klick öffnet die Ausgabe), „✓“ nach einem erfolgreichen Lauf, „⏸“ bei Pause (Klick setzt fort).
fn on_save_status(ui: &mut Ui, ed: &mut EditorUi, doc: Option<u64>, lang: Lang) {
    use super::super::save_actions_ui::OnSaveClick;
    let v = &ed.on_save;
    let key = doc.and_then(|d| ed.docs.get(&d)).and_then(|d| d.path.as_ref()).map(|p| crate::save_actions::path_key(p));
    let last = key.as_ref().and_then(|k| v.last.get(k)).copied();
    let mut click = None;
    let u = style::ui();
    if v.paused {
        ui.add_space(style::S3);
        let b = ui.add(egui::Button::new(RichText::new(lang.t("⏸ Aktionen pausiert", "⏸ actions paused")).small().color(u.warning)).frame(false));
        if b.on_hover_text(lang.t("Aktionen beim Speichern sind bis zum Neustart pausiert – klicken zum Fortsetzen", "Actions on save are paused until restart – click to resume")).clicked() {
            click = Some(OnSaveClick::Resume);
        }
    }
    if v.running > 0 {
        ui.add_space(style::S3);
        let t = format!("⟳ {} {}", v.running, if v.running == 1 { lang.t("Aktion", "action") } else { lang.t("Aktionen", "actions") });
        let b = ui.add(egui::Button::new(RichText::new(t).small().color(u.info)).frame(false));
        if b.on_hover_text(lang.t("Aktionen beim Speichern laufen – klicken für die Ausgabe", "Actions on save are running – click for the output")).clicked() {
            click = Some(OnSaveClick::Output);
        }
    } else if let Some(failed) = last {
        ui.add_space(style::S3);
        let (t, c, tip) = if failed {
            (lang.t("✗ Aktionen beim Speichern", "✗ actions on save"), u.danger, lang.t("Der letzte Lauf hatte einen Fehler – klicken für die Ausgabe", "The last run had an error – click for the output"))
        } else {
            ("✓", style::dim(ui), lang.t("Aktionen beim Speichern erfolgreich – klicken für die Ausgabe", "Actions on save succeeded – click for the output"))
        };
        if ui.add(egui::Button::new(RichText::new(t).small().color(c)).frame(false)).on_hover_text(tip).clicked() {
            click = Some(OnSaveClick::Output);
        }
    }
    if click.is_some() {
        ed.on_save.clicked = click;
    }
}

/// Anzeigename einer Sprache (reiner Text in der Sprache der Oberfläche).
pub(crate) fn lang_label(id: &str, name: &str, lang: Lang) -> String {
    if id == "plaintext" { lang.t("Reiner Text", "Plain text").to_string() } else { name.to_string() }
}

/// Sprachen, die zur Suche passen (Name, Id oder Endung; ohne Groß/Klein), eingebaute zuerst.
pub(super) fn filter_languages(query: &str) -> Vec<&'static crate::editor::syntax::LangInfo> {
    let q = query.trim().trim_start_matches("*.").trim_start_matches('.').to_lowercase();
    crate::editor::syntax::all_languages()
        .iter()
        .filter(|l| q.is_empty() || l.name.to_lowercase().contains(&q) || l.id.to_lowercase().contains(&q) || l.exts.iter().any(|e| e.to_lowercase() == q))
        .collect()
}

/// Sprachwahl in der Statusleiste: Suchfeld (Enter nimmt den ersten Treffer), „Automatisch erkennen“, alle Sprachen.
fn language_picker(ui: &mut Ui, ed: &mut EditorUi, id: u64, syn: &super::doc::SyntaxStatus, lang: Lang, cmds: &mut Vec<Cmd>) {
    let shown = syn.lang.as_deref().map(|l| lang_label(l, &syn.lang_name, lang)).unwrap_or_else(|| syn.lang_name.clone());
    let tip = if syn.lang_chosen { lang.t("Sprache (gewählt) – klicken zum Ändern", "Language (chosen) – click to change") } else { lang.t("Sprache (erkannt) – klicken zum Ändern", "Language (detected) – click to change") };
    let b = ui.add(egui::Button::new(RichText::new(&shown).small()).frame(false)).on_hover_text(tip);
    ed.status_rects.0 = Some(b.rect);
    if b.clicked() {
        ed.lang_search.clear();
        ed.lang_search_focus = true;
    }
    egui::Popup::menu(&b).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        ui.set_min_width(260.0);
        let r = ui.add(egui::TextEdit::singleline(&mut ed.lang_search).hint_text(lang.t("Sprache suchen …", "Search language …")).desired_width(250.0).id(Id::new("ed_lang_search")));
        if std::mem::take(&mut ed.lang_search_focus) {
            r.request_focus();
        }
        let hits = filter_languages(&ed.lang_search);
        if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            if let Some(l) = hits.first() {
                cmds.push(Cmd::SetLanguage(id, Some(l.id.clone())));
            }
            ui.close();
            return;
        }
        if ui.selectable_label(!syn.lang_chosen, lang.t("Automatisch erkennen", "Detect automatically")).clicked() {
            cmds.push(Cmd::SetLanguage(id, None));
            ui.close();
        }
        ui.separator();
        egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
            let mut extra_shown = false;
            for l in &hits {
                if !l.builtin && !extra_shown {
                    extra_shown = true;
                    ui.label(RichText::new(lang.t("Weitere Grammatiken (bat)", "More grammars (bat)")).small().color(style::dim(ui)));
                }
                let current = syn.lang.as_deref() == Some(l.id.as_str());
                let exts = l.exts.iter().take(4).map(|e| format!(".{e}")).collect::<Vec<_>>().join(" ");
                let resp = ui.selectable_label(current, lang_label(&l.id, &l.name, lang));
                let resp = if exts.is_empty() { resp } else { resp.on_hover_text(exts) };
                if resp.clicked() {
                    cmds.push(Cmd::SetLanguage(id, Some(l.id.clone())));
                    ui.close();
                }
            }
            if hits.is_empty() {
                ui.label(RichText::new(lang.t("Keine Sprache gefunden", "No language found")).small().color(style::dim(ui)));
            }
        });
    });
}

/// Moduswahl in der Statusleiste: für diese Datei, für alle Dateien der Sprache, für alle übrigen Dateien.
fn mode_picker(ui: &mut Ui, ed: &mut EditorUi, id: u64, syn: &super::doc::SyntaxStatus, prefs: &super::EditorPrefs, lang: Lang, cmds: &mut Vec<Cmd>) {
    let name = |m: Mode| match m {
        Mode::Text => lang.t("Text", "Text"),
        Mode::Syntax => lang.t("Syntax", "Syntax"),
        Mode::Smart => lang.t("Smart (Sprachserver)", "Smart (language server)"),
    };
    let label = if syn.mode.colours() && syn.large { lang.t("Syntax (aus: große Datei)", "Syntax (off: large file)").to_string() } else { name(syn.mode).to_string() };
    let tip = if syn.mode_chosen { lang.t("Modus (für diese Datei gewählt) – klicken zum Ändern", "Mode (chosen for this file) – click to change") } else { lang.t("Modus (aus den Einstellungen) – klicken zum Ändern", "Mode (from the settings) – click to change") };
    let b = ui.add(egui::Button::new(RichText::new(label).small()).frame(false)).on_hover_text(tip);
    ed.status_rects.1 = Some(b.rect);
    egui::Popup::menu(&b).show(|ui| {
        ui.set_min_width(240.0);
        ui.label(RichText::new(lang.t("Diese Datei", "This file")).small().color(style::dim(ui)));
        for m in Mode::ALL {
            if ui.selectable_label(syn.mode_chosen && syn.mode == m, name(m)).clicked() {
                cmds.push(Cmd::SetDocMode(id, Some(m)));
                ui.close();
            }
        }
        if syn.mode_chosen && ui.selectable_label(false, lang.t("Einstellung verwenden", "Use the setting")).clicked() {
            cmds.push(Cmd::SetDocMode(id, None));
            ui.close();
        }
        if let Some(l) = &syn.lang {
            ui.separator();
            let shown = lang_label(l, &syn.lang_name, lang);
            ui.label(RichText::new(format!("{} {shown}", lang.t("Alle Dateien:", "All files:"))).small().color(style::dim(ui)));
            let cur = prefs.syntax.language_modes.get(l).copied();
            ui.horizontal(|ui| {
                for m in Mode::ALL {
                    if ui.selectable_label(cur == Some(m), name(m)).clicked() {
                        cmds.push(Cmd::SetLangMode(id, l.clone(), Some(m)));
                        ui.close();
                    }
                }
                if ui.selectable_label(cur.is_none(), lang.t("Standard", "Default")).clicked() {
                    cmds.push(Cmd::SetLangMode(id, l.clone(), None));
                    ui.close();
                }
            });
        }
        ui.separator();
        ui.label(RichText::new(lang.t("Standard für alle übrigen Dateien", "Default for all other files")).small().color(style::dim(ui)));
        ui.horizontal(|ui| {
            for m in Mode::ALL {
                if ui.selectable_label(prefs.syntax.default_mode == m, name(m)).clicked() {
                    cmds.push(Cmd::SetDefaultMode(id, m));
                    ui.close();
                }
            }
        });
        if syn.large {
            ui.label(RichText::new(lang.t("Große Datei: Syntaxfarben bleiben aus.", "Large file: syntax colours stay off.")).small().color(style::ui().warning));
        } else if syn.mode.colours() && !syn.active {
            ui.label(RichText::new(lang.t("Für diese Sprache gibt es keine Syntaxfarben.", "There are no syntax colours for this language.")).small().color(style::dim(ui)));
        }
    });
}
