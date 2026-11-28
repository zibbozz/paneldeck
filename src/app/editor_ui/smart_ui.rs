//! Oberfläche des Smart-Modus (#367): Hinweisleiste „Ordner vertrauen“, Statusleiste (Server, Fehler/Warnungen),
//! Popup der Vervollständigung mit Dokumentation daneben, Problemliste und das Fenster „Language Server“.

use super::smart::State;
use super::view::Cmd;
use super::EditorUi;
use crate::lsp::proto::Severity;
use crate::state::Lang;
use crate::style;
use eframe::egui::{self, Align2, Pos2, Rect, RichText, Sense, Ui, Vec2};
use std::path::Path;

/// Hinweisleiste über dem Text: *Vertrauen* · *Übergeordneten Ordner vertrauen …* · *Jetzt nicht*. Bis dahin läuft der
/// Syntax-Modus.
pub(super) fn trust_bar(ui: &mut Ui, r: Rect, root: &Path, lang: Lang, cmds: &mut Vec<Cmd>) {
    let u = style::ui();
    ui.painter().rect_filled(r, 0.0, u.warning.gamma_multiply(0.12));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink2(Vec2::new(10.0, 2.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
    if child.small_button(lang.t("Jetzt nicht", "Not now")).clicked() {
        cmds.push(Cmd::TrustNotNow(root.to_path_buf()));
    }
    if child.small_button(lang.t("Übergeordneten Ordner vertrauen …", "Trust parent folder …")).clicked() {
        cmds.push(Cmd::TrustPick(root.parent().unwrap_or(root).to_path_buf()));
    }
    if child.small_button(lang.t("Vertrauen", "Trust")).on_hover_text(root.display().to_string()).clicked() {
        cmds.push(Cmd::Trust(root.to_path_buf()));
    }
    child.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
        let t = if lang == Lang::De {
            format!("Smart-Modus: Ein Sprachserver liest und startet Dateien dieses Projekts. Ordner „{}“ vertrauen? Bis dahin: Syntax-Modus.", root.display())
        } else {
            format!("Smart mode: a language server reads and runs files of this project. Trust the folder “{}”? Until then: Syntax mode.", root.display())
        };
        ui.add(egui::Label::new(RichText::new(t).small()).truncate());
    });
}

/// Statusleiste: Zustand des Servers und Zähler der Fehler/Warnungen (Klick: Problemliste), Meldung der Cursorzeile.
pub(super) fn status_chip(ui: &mut Ui, ed: &mut EditorUi, doc: u64, lang: Lang, cmds: &mut Vec<Cmd>) {
    let st = ed.smart.state_of(doc);
    let (txt, tip): (String, String) = match &st {
        State::Off => return,
        State::Running(k) => {
            let s = ed.smart.servers.entries.get(k).map(|e| e.status()).unwrap_or_default();
            (k.0.clone(), format!("{} · {}", k.1.display(), s))
        }
        State::NeedsTrust(_) | State::NotNow(_) => (lang.t("Smart: Ordner nicht vertraut", "Smart: folder not trusted").into(), lang.t("Bis zum Vertrauen läuft der Syntax-Modus.", "Syntax mode runs until you trust the folder.").into()),
        State::NoServer => (lang.t("Smart: kein Server", "Smart: no server").into(), lang.t("Für diese Sprache gibt es keinen Sprachserver im PATH und keinen eigenen Pfad (Einstellungen → Editor → Sprachen).", "There is no language server for this language in PATH and no own path (Settings → Editor → Languages).").into()),
        State::TooLarge => (lang.t("Smart: Datei zu groß", "Smart: file too large").into(), lang.t("Über der Grenze des Smart-Modus läuft der Syntax-Modus.", "Above the smart mode limit Syntax mode runs.").into()),
        State::Unsupported => (lang.t("Smart: nicht für diese Datei", "Smart: not for this file").into(), lang.t("Nur gespeicherte lokale Dateien (WSL-Dateien folgen später).", "Only saved local files (WSL files follow later).").into()),
        State::Stopped(w) => (lang.t("Smart: Server aus", "Smart: server off").into(), w.clone()),
    };
    let (e, w) = ed.smart.counts(doc);
    if matches!(st, State::Running(_)) {
        let c = ui.add(egui::Button::new(RichText::new(format!("✖ {e}  ⚠ {w}")).small()).frame(false)).on_hover_text(lang.t("Problemliste (Strg+Umschalt+M)", "Problem list (Ctrl+Shift+M)"));
        if c.clicked() {
            cmds.push(Cmd::ToggleProblems);
        }
    }
    if ui.add(egui::Button::new(RichText::new(txt).small().color(style::dim(ui))).frame(false)).on_hover_text(tip).clicked() {
        cmds.push(Cmd::OpenServers);
    }
    // Meldung der Cursorzeile, klein
    if let Some(list) = ed.smart.shown.get(&doc)
        && let Some((v, d)) = ed.focused_view_and_doc().filter(|(_, d)| d.doc.is_some())
    {
        let (line, _) = v.cursor_linecol(d);
        if let Some(m) = list.iter().find(|s| s.line == line) {
            let short: String = m.message.lines().next().unwrap_or("").chars().take(80).collect();
            ui.label(RichText::new(short).small().color(style::dim(ui)));
        }
    }
}

/// Popup der Vervollständigung unter dem Cursor: bis zu 12 Zeilen (Art, Name, Detail), Dokumentation daneben.
pub(super) fn completion_popup(ui: &mut Ui, at: Pos2, ed: &mut EditorUi, lang: Lang, cmds: &mut Vec<Cmd>) {
    let Some(p) = ed.smart.popup.clone() else { return };
    let id = style::salted(egui::Id::new(("ed_completion", p.group)));
    let mut click: Option<usize> = None;
    egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(at).constrain(true).show(ui.ctx(), |ui| {
        ui.horizontal_top(|ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_min_width(260.0);
                ui.set_max_width(460.0);
                if p.shown.is_empty() {
                    ui.label(RichText::new(if p.waiting { lang.t("Vorschläge werden geladen …", "Loading suggestions …") } else { lang.t("Keine Vorschläge", "No suggestions") }).small().color(style::dim(ui)));
                    return;
                }
                let first = p.selected.saturating_sub(super::smart::POPUP_ROWS - 1);
                for (row, &i) in p.shown.iter().enumerate().skip(first).take(super::smart::POPUP_ROWS) {
                    let it = &p.items[i];
                    let sel = row == p.selected;
                    let r = ui.horizontal(|ui| {
                        ui.label(RichText::new(it.kind.glyph()).monospace().color(style::accent()));
                        ui.label(RichText::new(&it.label).monospace().strong());
                        if let Some(d) = &it.detail {
                            let d: String = d.chars().take(60).collect();
                            ui.label(RichText::new(d).small().color(style::dim(ui)));
                        }
                    });
                    let resp = ui.interact(r.response.rect, id.with(("row", i)), Sense::click());
                    if sel {
                        ui.painter().rect_filled(r.response.rect.expand(1.0), 2.0, style::ui().selection_bg.gamma_multiply(0.6));
                    }
                    if resp.clicked() {
                        click = Some(row);
                    }
                }
                if p.shown.len() > super::smart::POPUP_ROWS {
                    ui.label(RichText::new(format!("{} / {}", p.selected + 1, p.shown.len())).small().color(style::dim(ui)));
                }
            });
            if let Some(doc) = p.shown.get(p.selected).and_then(|i| p.items[*i].documentation.clone()).filter(|d| !d.trim().is_empty()) {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(380.0);
                    let d: String = doc.chars().take(2000).collect();
                    ui.add(egui::Label::new(RichText::new(d).small()).wrap());
                });
            }
        });
    });
    if let Some(row) = click
        && let Some(pp) = ed.smart.popup.as_mut()
    {
        pp.selected = row;
        cmds.push(Cmd::AcceptCompletion);
    }
}

/// Problemliste: alle Diagnosen der offenen Dokumente, Fehler zuerst; Klick springt hin.
pub(super) fn problems_panel(ui: &mut Ui, ed: &mut EditorUi, lang: Lang, cmds: &mut Vec<Cmd>) {
    let list: Vec<(u64, String, usize, usize, Severity, String)> = ed.smart.all_problems().into_iter().map(|(d, s)| (d, ed.docs.get(&d).map(|e| e.title(lang)).unwrap_or_default(), s.line, s.col, s.severity, s.message.clone())).collect();
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} ({})", lang.t("Probleme", "Problems"), list.len())).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("✕").on_hover_text(lang.t("Schließen (Strg+Umschalt+M)", "Close (Ctrl+Shift+M)")).clicked() {
                cmds.push(Cmd::ToggleProblems);
            }
        });
    });
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if list.is_empty() {
            ui.label(RichText::new(lang.t("Keine Probleme in Dateien im Smart-Modus.", "No problems in files in smart mode.")).small().color(style::dim(ui)));
        }
        let u = style::ui();
        for (doc, title, line, col, sev, msg) in list.iter().take(5000) {
            let (icon, c) = match sev {
                Severity::Error => ("✖", u.diag_error),
                Severity::Warning => ("⚠", u.diag_warning),
                Severity::Information => ("ℹ", u.diag_info),
                Severity::Hint => ("·", u.diag_hint),
            };
            let first: String = msg.lines().next().unwrap_or("").chars().take(200).collect();
            let r = ui.horizontal(|ui| {
                ui.label(RichText::new(icon).color(c));
                ui.label(RichText::new(format!("{title}:{}:{}", line + 1, col + 1)).small().color(style::dim(ui)));
                ui.label(RichText::new(first).small());
            });
            if ui.interact(r.response.rect, egui::Id::new(("ed_problem", *doc, *line, *col)), Sense::click()).on_hover_text(msg).clicked() {
                cmds.push(Cmd::OpenAt(*doc, *line));
            }
        }
    });
}

/// Fenster „Language Server“: Sprache, Ordner, Zustand, PID, *Neu starten*/*Anhalten*; Protokoll des gewählten Servers
/// (nur im Speicher).
pub(super) fn servers_window(ctx: &egui::Context, ed: &mut EditorUi, lang: Lang) {
    let mut open = true;
    let mut action: Option<(crate::lsp::Key, bool)> = None;
    egui::Window::new(lang.t("Language Server", "Language servers")).id(style::salted(egui::Id::new("ed_lsp_window"))).open(&mut open).default_size(Vec2::new(640.0, 420.0)).show(ctx, |ui| {
        if ed.smart.servers.entries.is_empty() {
            ui.label(RichText::new(lang.t("Noch kein Server gestartet. Smart-Modus in der Statusleiste wählen.", "No server started yet. Choose Smart mode in the status bar.")).color(style::dim(ui)));
        }
        egui::Grid::new("ed_lsp_grid").num_columns(5).striped(true).spacing([10.0, 4.0]).show(ui, |ui| {
            for h in [lang.t("Server", "Server"), lang.t("Ordner", "Folder"), lang.t("Zustand", "Status"), "PID", ""] {
                ui.label(RichText::new(h).small().strong());
            }
            ui.end_row();
            let keys: Vec<crate::lsp::Key> = ed.smart.servers.entries.keys().cloned().collect();
            for k in keys {
                let Some(e) = ed.smart.servers.entries.get(&k) else { continue };
                let sel = ed.smart.window_sel.as_ref() == Some(&k);
                if ui.selectable_label(sel, format!("{} ({})", e.server, e.language)).clicked() {
                    ed.smart.window_sel = Some(k.clone());
                }
                ui.label(RichText::new(e.root.display().to_string()).small());
                ui.label(RichText::new(e.status()).small());
                ui.label(RichText::new(e.client.as_ref().and_then(|c| c.pid).map(|p| p.to_string()).unwrap_or_else(|| "–".into())).small());
                ui.horizontal(|ui| {
                    if ui.small_button(lang.t("Neu starten", "Restart")).clicked() {
                        action = Some((k.clone(), true));
                    }
                    if ui.small_button(lang.t("Anhalten", "Stop")).clicked() {
                        action = Some((k.clone(), false));
                    }
                });
                ui.end_row();
            }
        });
        ui.separator();
        ui.label(RichText::new(lang.t("Protokoll (nur im Speicher, höchstens 1 MB je Server; Mitschnitt der Nachrichten: Einstellungen → Editor → Smart-Modus)", "Log (in memory only, at most 1 MB per server; message trace: Settings → Editor → Smart mode)")).small().color(style::dim(ui)));
        let log = ed.smart.window_sel.as_ref().and_then(|k| ed.smart.servers.entries.get(k)).and_then(|e| e.client.as_ref()).map(|c| c.log.text()).unwrap_or_default();
        egui::ScrollArea::vertical().stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
            ui.add(egui::Label::new(RichText::new(log).monospace().small()).wrap());
        });
    });
    if let Some((k, restart)) = action {
        if restart {
            ed.smart.servers.restart(&k);
        } else {
            ed.smart.servers.stop(&k);
        }
    }
    if !open {
        ed.smart.window_open = false;
    }
    let _ = Align2::LEFT_TOP;
}
