//! Oberfläche der Aktionen beim Speichern (#383): Abschnitt in *Einstellungen → Editor* (Hauptschalter, Liste der
//! Regeln zum Ziehen, Export/Import), Regel-Dialog (Gültigkeit, Auslöser, Reihenfolge, Aktionen zum Ziehen, Befehls-
//! Optionen wie in der Prozessliste, *Für Datei testen …*, *Jetzt ausführen*), Test-Ansicht *Regeln für diese Datei*
//! und der Ausgabebereich (letzte 100 Läufe). Laufzeit: `save_actions_ui`.

use super::save_actions_ui::RunStatus;
use super::*;
use crate::procs;
use crate::save_actions::matching::{self, Switches};
use crate::save_actions::{self as sa, vars, Action as RuleAction, ActionKind, Builtin, Rule, SaveOrigin, Scope};
use std::collections::HashSet;

/// Ein Variablen-Eintrag im Regel-Dialog: neuer Wert (leer = gespeicherten behalten) und der gespeicherte Blob.
#[derive(Clone, Default, Debug)]
pub(super) struct EnvRow {
    pub name: String,
    pub value: String,
    pub stored: Option<String>,
}

/// Regel-Dialog.
#[derive(Clone, Debug)]
pub(super) struct RuleEditor {
    pub rule: Rule,
    pub is_new: bool,
    pub patterns: String,
    pub env: Vec<EnvRow>,
    pub error: Option<String>,
    /// Gezogene Aktion (Index).
    drag: Option<usize>,
    pub confirm_delete: bool,
    /// Das Ordnerfeld lässt den Wert nicht übernehmen (#392: ungültiger/fehlender Linux-Ordner, andere Distribution,
    /// Linux-Pfad für eine Windows-Shell) – Speichern gesperrt.
    pub cwd_block: Option<String>,
}

impl RuleEditor {
    pub fn of(rule: &Rule, is_new: bool) -> RuleEditor {
        RuleEditor {
            patterns: rule.patterns.join("\n"),
            env: rule.env.iter().map(|e| EnvRow { name: e.name.clone(), value: String::new(), stored: e.value_protected.clone() }).collect(),
            rule: rule.clone(),
            is_new,
            error: None,
            drag: None,
            confirm_delete: false,
            cwd_block: None,
        }
    }

    /// Prüfen und als Regel liefern.
    pub fn build(&self, lang: Lang) -> Result<Rule, String> {
        let mut r = self.rule.clone();
        r.name = r.name.trim().to_string();
        r.path = r.path.trim().to_string();
        if r.path.is_empty() {
            return Err(match r.scope {
                Scope::File => lang.t("Bitte eine Datei wählen.", "Please choose a file."),
                _ => lang.t("Bitte einen Ordner wählen.", "Please choose a folder."),
            }
            .into());
        }
        if !Path::new(&r.path).is_absolute() {
            return Err(lang.t("Der Pfad muss absolut sein.", "The path must be absolute.").into());
        }
        r.patterns = self.patterns.lines().map(matching::normalize_pattern).filter(|l| !l.is_empty()).collect();
        if r.scope == Scope::Pattern {
            if r.patterns.is_empty() {
                return Err(lang.t("Bitte mindestens ein Muster eingeben (z. B. *.json).", "Please enter at least one pattern (e.g. *.json).").into());
            }
            if let Some((p, e)) = r.patterns.iter().find_map(|p| matching::pattern_error(p).map(|e| (p.clone(), e))) {
                return Err(format!("{} {}: {e}", lang.t("Ungültiges Muster", "Invalid pattern"), lang.q(&p)));
            }
        } else {
            r.patterns.clear();
        }
        if r.actions.is_empty() {
            return Err(lang.t("Bitte mindestens eine Aktion hinzufügen.", "Please add at least one action.").into());
        }
        for a in &r.actions {
            match &a.kind {
                ActionKind::Command { command } if command.trim().is_empty() || command.contains('\n') => return Err(lang.t("Ein Befehl ist leer oder hat mehrere Zeilen.", "A command is empty or has several lines.").into()),
                ActionKind::Copy { target, .. } => {
                    if let Err(e) = sa::copy::check_target(target, &r.folder()) {
                        return Err(e.text(lang).into());
                    }
                }
                ActionKind::SendToSession { session: 0, .. } => return Err(lang.t("Bitte eine Sitzung wählen.", "Please choose a session.").into()),
                ActionKind::SendToSession { text, .. } if text.trim().is_empty() => return Err(lang.t("Bitte den Text für die Sitzung eingeben.", "Please enter the text for the session.").into()),
                ActionKind::RestartProcess { process: 0, group, .. } if group.trim().is_empty() => return Err(lang.t("Bitte einen Prozess oder eine Gruppe wählen.", "Please choose a process or a group.").into()),
                ActionKind::Notify { text } if text.trim().is_empty() => return Err(lang.t("Bitte den Text der Benachrichtigung eingeben.", "Please enter the text of the notification.").into()),
                _ => {}
            }
        }
        let mut env = Vec::new();
        let mut names = HashSet::new();
        for row in &self.env {
            let n = row.name.trim();
            if n.is_empty() && row.value.is_empty() {
                continue;
            }
            if !procs::valid_env_name(n) {
                return Err(format!("{} {}", lang.t("Ungültiger Variablenname:", "Invalid variable name:"), lang.q(n)));
            }
            if !names.insert(n.to_ascii_uppercase()) {
                return Err(format!("{} {}", lang.t("Variable doppelt:", "Duplicate variable:"), lang.q(n)));
            }
            let value_protected = if row.value.is_empty() {
                row.stored.clone()
            } else {
                Some(procs::protect_value(&row.value).ok_or_else(|| lang.t("Der Wert lässt sich hier nicht verschlüsselt speichern.", "The value cannot be stored encrypted here.").to_string())?)
            };
            env.push(procs::EnvVar { name: n.to_string(), value_protected });
        }
        r.env = env;
        r.cwd = r.cwd.trim().to_string();
        r.timeout_secs = r.timeout_secs.clamp(sa::TIMEOUT_MIN, sa::TIMEOUT_MAX);
        Ok(r)
    }
}

/// Test-Ansicht *Regeln für diese Datei* / *Für Datei testen …*.
#[derive(Clone, Debug, Default)]
pub(super) struct TestView {
    pub file: Option<PathBuf>,
    /// Nur diese Regel (aus dem Regel-Dialog: der ungespeicherte Stand).
    pub rule: Option<Box<Rule>>,
    /// Berechneter Plan und Zeilen (Schlüssel: Datei, Regeln, Pause) – nicht jedes Bild neu (Entschlüsseln!).
    cache: Option<(u64, matching::Plan, Vec<(u64, String, Vec<String>)>)>,
}

/// Vorschau beim Import.
#[derive(Clone, Debug, Default)]
pub(super) struct ImportView {
    pub rules: Vec<Rule>,
    pub chosen: Vec<bool>,
}

/// Zustand der Oberfläche.
#[derive(Default)]
pub(super) struct SaUi {
    pub output_open: bool,
    pub output_sel: Option<u64>,
    pub editor: Option<RuleEditor>,
    pub test: Option<TestView>,
    pub import: Option<ImportView>,
    /// Für den Export gewählte Regeln.
    pub selected: HashSet<u64>,
    /// Gezogene Regel (Index in der Liste).
    drag: Option<usize>,
    /// „Ordner fehlt“ je Regel (mit Zeitpunkt der Prüfung).
    missing: std::collections::HashMap<u64, (bool, Instant)>,
    pub msg: Option<String>,
}

/// Element von `from` nach `to` verschieben (Ziel = Index in der Liste vor dem Entfernen; `to` == Länge = ans Ende).
pub(super) fn move_item<T>(v: &mut Vec<T>, from: usize, to: usize) -> bool {
    if from >= v.len() || to > v.len() || to == from || to == from + 1 {
        return false;
    }
    let x = v.remove(from);
    let to = if to > from { to - 1 } else { to };
    v.insert(to, x);
    true
}

/// Einfügestelle aus der Zeigerhöhe und den Zeilen (Mitte der Zeile entscheidet).
pub(super) fn drop_index(rows: &[Rect], y: f32) -> usize {
    rows.iter().position(|r| y < r.center().y).unwrap_or(rows.len())
}

/// Ziehgriff einer Zeile; liefert die Antwort (Ziehen beginnt/läuft).
fn grip(ui: &mut Ui, tip: &str) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(Vec2::new(14.0, 18.0), Sense::drag());
    let c = if resp.hovered() || resp.dragged() { ui.visuals().strong_text_color() } else { style::dim(ui) };
    for dy in [-4.0, 0.0, 4.0] {
        ui.painter().line_segment([Pos2::new(r.left() + 2.0, r.center().y + dy), Pos2::new(r.right() - 2.0, r.center().y + dy)], Stroke::new(1.2_f32, c));
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
    }
    resp.on_hover_text(tip)
}

/// Kurzbeschreibung einer Regel für die Liste.
fn scope_text(r: &Rule, lang: Lang) -> String {
    match r.scope {
        Scope::File => format!("{}: {}", r.scope.label(lang), r.path),
        Scope::Folder => format!("{}: {}", r.scope.label(lang), r.path),
        Scope::Pattern => format!("{} {}  ({})", r.scope.label(lang), r.patterns.join("  "), r.path),
    }
}

/// Neue Aktion einer Art (für das Menü *+ Aktion*).
fn new_action(kind: &str) -> RuleAction {
    RuleAction::new(match kind {
        "command" => ActionKind::Command { command: String::new() },
        "copy" => ActionKind::Copy { target: String::new(), keep_structure: true },
        "send_to_session" => ActionKind::SendToSession { session: 0, name: String::new(), text: String::new() },
        "restart_process" => ActionKind::RestartProcess { process: 0, group: String::new(), name: String::new() },
        "notify" => ActionKind::Notify { text: "${fileBasename}".into() },
        "format" => ActionKind::Builtin(Builtin::Format),
        "trim_whitespace" => ActionKind::Builtin(Builtin::TrimWhitespace),
        "final_newline" => ActionKind::Builtin(Builtin::FinalNewline),
        "line_endings" => ActionKind::Builtin(Builtin::LineEndings(crate::editor::LineEnding::Lf)),
        "encoding" => ActionKind::Builtin(Builtin::Encoding(crate::editor::TextEncoding::Utf8)),
        "tabs_to_spaces" => ActionKind::Builtin(Builtin::TabsToSpaces(0)),
        _ => ActionKind::Builtin(Builtin::SpacesToTabs(0)),
    })
}

const ADD_KINDS: [&str; 12] = ["command", "copy", "send_to_session", "restart_process", "notify", "format", "trim_whitespace", "final_newline", "line_endings", "encoding", "tabs_to_spaces", "spaces_to_tabs"];

/// Farbe eines Ergebnisses.
fn status_color(s: RunStatus) -> Color32 {
    let u = style::ui();
    match s {
        RunStatus::Running => u.info,
        RunStatus::Ok => u.success,
        RunStatus::Failed | RunStatus::TimeLimit => u.danger,
        RunStatus::Cancelled | RunStatus::Skipped => u.warning,
    }
}

/// Was der Regel-Dialog zum Zeichnen braucht (ohne `App`, damit er sich ohne Fenster testen lässt).
pub(super) struct DialogCx<'a> {
    pub lang: Lang,
    /// Erkannte Shells (Kennung, Anzeige).
    pub shells: &'a [(String, String)],
    /// Terminals (Kennung, Name) für *An Sitzung senden*.
    pub sessions: &'a [(u64, String)],
    /// Prozesse und Gruppen der Prozessliste für *Prozess neu starten*.
    pub processes: &'a [(u64, String)],
    pub groups: &'a [String],
    /// *Jetzt ausführen* möglich (gespeicherte Regel, offene Datei).
    pub can_run: bool,
    /// Quoting der gewählten Shell (Prüfung der Befehle) und ob es cmd ist.
    pub quote: crate::quote::Kind,
    pub cmd: bool,
    /// Ziel des Ordnerfelds (#392): nach der Shell der Regel (WSL: Linux-Pfade, Git Bash `/c/x`, `~`).
    pub cwd_target: super::folder_picker::Target,
    /// Zuletzt benutzte WSL-Ordner (Vorschläge des Ordnerfelds).
    pub wsl_recent: &'a std::collections::BTreeMap<String, Vec<String>>,
}

/// Knöpfe des Regel-Dialogs.
#[derive(Default, Debug, PartialEq)]
pub(super) struct DialogOut {
    pub save: bool,
    pub cancel: bool,
    pub test: bool,
    pub run_now: bool,
    pub delete: bool,
}

/// Inhalt des Regel-Dialogs.
pub(super) fn rule_editor_body(ui: &mut Ui, ed: &mut RuleEditor, cx: &DialogCx) -> DialogOut {
    let lang = cx.lang;
    let mut out = DialogOut::default();
    egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
        egui::Grid::new("sa_rule_grid").num_columns(2).spacing(Vec2::new(10.0, 6.0)).show(ui, |ui| {
            ui.label(lang.t("Name", "Name"));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut ed.rule.name).hint_text(lang.t("z. B. Lint", "e.g. Lint")).desired_width(300.0));
                ui.checkbox(&mut ed.rule.enabled, lang.t("an", "on"));
            });
            ui.end_row();
            ui.label(lang.t("Gilt für", "Applies to"));
            ui.horizontal(|ui| {
                for s in [Scope::File, Scope::Folder, Scope::Pattern] {
                    ui.radio_value(&mut ed.rule.scope, s, s.label(lang));
                }
            });
            ui.end_row();
            ui.label(if ed.rule.scope == Scope::File { lang.t("Datei", "File") } else { lang.t("Ordner", "Folder") });
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut ed.rule.path).desired_width(360.0));
                if ui.button("…").on_hover_text(lang.t("Wählen", "Choose")).clicked() {
                    if ed.rule.scope == Scope::File {
                        if let Some(p) = crate::platform::notify::pick_file(&ed.rule.path) {
                            ed.rule.path = p.to_string_lossy().to_string();
                        }
                    } else if let Some(p) = crate::platform::notify::pick_folder(&ed.rule.path) {
                        ed.rule.path = p;
                    }
                }
            });
            ui.end_row();
            if ed.rule.scope == Scope::Pattern {
                ui.label(lang.t("Muster", "Patterns"));
                ui.vertical(|ui| {
                    ui.add(egui::TextEdit::multiline(&mut ed.patterns).desired_rows(3).desired_width(380.0).font(egui::TextStyle::Monospace).hint_text("*.json\nsrc/**/*.ts\n!dist/**"));
                    ui.label(RichText::new(lang.t("Eines je Zeile, wie .gitignore, relativ zum Ordner; ! nimmt wieder aus.", "One per line, like .gitignore, relative to the folder; ! excludes again.")).small().color(style::dim(ui)));
                    for l in ed.patterns.lines() {
                        if let Some(e) = matching::pattern_error(l) {
                            ui.colored_label(style::danger_soft(), format!("{l}: {e}"));
                        }
                    }
                });
                ui.end_row();
            }
            ui.label(lang.t("Auslöser", "Triggers"));
            ui.vertical(|ui| {
                ui.label(RichText::new(lang.t("Speichern, Speichern unter, Alle speichern und Speichern beim Schließen lösen immer aus.", "Save, Save as, Save all and saving on close always trigger.")).small().color(style::dim(ui)));
                ui.checkbox(&mut ed.rule.on_autosave, lang.t("Auch beim automatischen Speichern", "Also on autosave"));
                ui.checkbox(&mut ed.rule.on_agent_save, lang.t("Auch wenn ein Agent speichert", "Also when an agent saves"));
            });
            ui.end_row();
            ui.label(lang.t("Reihenfolge", "Order"));
            ui.vertical(|ui| {
                ui.checkbox(&mut ed.rule.stop_after, lang.t("Keine weiteren Regeln danach", "No further rules afterwards"));
                ui.checkbox(&mut ed.rule.no_parent_rules, lang.t("Regeln übergeordneter Ordner nicht anwenden", "Don't apply parent-folder rules"));
            });
            ui.end_row();
        });
        ui.add_space(S2);
        ui.label(RichText::new(lang.t("Aktionen", "Actions")).strong());
        ui.label(RichText::new(lang.t("Ändernde Aktionen laufen vor dem Schreiben, die übrigen danach in dieser Reihenfolge. Ein Fehler überspringt den Rest der Regel (außer mit „Bei Fehler weiter“). Variablen: ${file} ${fileDirname} ${fileBasename} ${fileBasenameNoExtension} ${fileExtname} ${relativeFile} ${ruleFolder} ${workspaceFolder} ${env:NAME} – werden selbst gequotet, auch als PANELDECK_FILE … in der Umgebung.", "Modifying actions run before writing, the others afterwards in this order. An error skips the rest of the rule (unless “Continue on error”). Variables: ${file} ${fileDirname} ${fileBasename} ${fileBasenameNoExtension} ${fileExtname} ${relativeFile} ${ruleFolder} ${workspaceFolder} ${env:NAME} – quoted automatically, also as PANELDECK_FILE … in the environment.")).small().color(style::dim(ui)));
        let mut rows = Vec::new();
        let mut remove = None;
        let mut drag_start = None;
        let n = ed.rule.actions.len();
        for i in 0..n {
            let a = &mut ed.rule.actions[i];
            let resp = ui.horizontal(|ui| {
                let g = grip(ui, lang.t("Ziehen zum Sortieren", "Drag to sort"));
                if g.drag_started() {
                    drag_start = Some(i);
                }
                ui.label(RichText::new(a.kind.label(lang)).strong());
                match &mut a.kind {
                    ActionKind::Command { command } => {
                        ui.add(egui::TextEdit::singleline(command).font(egui::TextStyle::Monospace).hint_text("npx eslint --fix ${file}").desired_width(300.0));
                    }
                    ActionKind::Copy { target, keep_structure } => {
                        ui.add(egui::TextEdit::singleline(target).hint_text(lang.t("Zielordner (lokal)", "target folder (local)")).desired_width(200.0));
                        if ui.small_button("…").clicked()
                            && let Some(p) = crate::platform::notify::pick_folder(target)
                        {
                            *target = p;
                        }
                        ui.checkbox(keep_structure, lang.t("Ordnerstruktur", "Keep structure")).on_hover_text(lang.t("Pfad relativ zum Regelordner beibehalten", "Keep the path relative to the rule folder"));
                    }
                    ActionKind::SendToSession { session, name, text } => {
                        let shown = cx.sessions.iter().find(|(id, _)| id == session).map(|(_, n)| n.clone()).unwrap_or_else(|| if name.is_empty() { lang.t("Sitzung wählen", "Choose a session").to_string() } else { format!("{name} ({})", lang.t("nicht offen", "not open")) });
                        egui::ComboBox::from_id_salt(("sa_sess", i)).selected_text(shown).width(150.0).show_ui(ui, |ui| {
                            for (id, n) in cx.sessions {
                                if ui.selectable_label(*session == *id, n).clicked() {
                                    *session = *id;
                                    *name = n.clone();
                                }
                            }
                        });
                        ui.add(egui::TextEdit::singleline(text).font(egui::TextStyle::Monospace).hint_text("make test").desired_width(160.0));
                    }
                    ActionKind::RestartProcess { process, group, name } => {
                        let shown = if name.is_empty() { lang.t("Prozess wählen", "Choose a process").to_string() } else { name.clone() };
                        egui::ComboBox::from_id_salt(("sa_proc", i)).selected_text(shown).width(200.0).show_ui(ui, |ui| {
                            for (id, n) in cx.processes {
                                if ui.selectable_label(*process == *id && group.is_empty(), n).clicked() {
                                    *process = *id;
                                    group.clear();
                                    *name = n.clone();
                                }
                            }
                            if !cx.groups.is_empty() {
                                ui.separator();
                            }
                            for g in cx.groups {
                                if ui.selectable_label(group == g, format!("{} {g}", lang.t("Gruppe", "Group"))).clicked() {
                                    *group = g.clone();
                                    *process = 0;
                                    *name = g.clone();
                                }
                            }
                        });
                    }
                    ActionKind::Notify { text } => {
                        ui.add(egui::TextEdit::singleline(text).desired_width(300.0));
                    }
                    ActionKind::Builtin(Builtin::LineEndings(e)) => {
                        ui.radio_value(e, crate::editor::LineEnding::Lf, "LF");
                        ui.radio_value(e, crate::editor::LineEnding::CrLf, "CRLF");
                    }
                    ActionKind::Builtin(Builtin::Encoding(e)) => {
                        egui::ComboBox::from_id_salt(("sa_enc", i)).selected_text(e.id()).show_ui(ui, |ui| {
                            for (k, l) in super::editor_ui::ENCODING_CHOICES {
                                if let Some(x) = crate::editor::TextEncoding::from_id(k) {
                                    ui.selectable_value(e, x, *l);
                                }
                            }
                        });
                    }
                    ActionKind::Builtin(Builtin::TabsToSpaces(w)) | ActionKind::Builtin(Builtin::SpacesToTabs(w)) => {
                        ui.add(egui::DragValue::new(w).range(0..=16)).on_hover_text(lang.t("Breite; 0 = Tabbreite der Datei", "Width; 0 = the file's tab width"));
                    }
                    _ => {}
                }
                if !a.kind.modifies() {
                    ui.checkbox(&mut a.continue_on_error, lang.t("Bei Fehler weiter", "Continue on error"));
                }
                if style::icon_btn_small(ui, SIcon::Close, lang.t("Aktion entfernen", "Remove the action")).clicked() {
                    remove = Some(i);
                }
            });
            rows.push(resp.response.rect);
            match &a.kind {
                ActionKind::Command { command } => {
                    if let Err(e) = vars::check_command(command, cx.quote) {
                        ui.label(RichText::new(e).small().color(style::ui().danger));
                    }
                    if cx.cmd {
                        let bare = vars::cmd_unquoted_env(command);
                        if !bare.is_empty() {
                            ui.label(RichText::new(format!("{} {}", lang.t("cmd setzt diese Variablen ungequotet ein – als \"%NAME%\" schreiben:", "cmd inserts these variables unquoted – write them as \"%NAME%\":"), bare.join(", "))).small().color(style::ui().warning));
                        }
                    }
                }
                ActionKind::SendToSession { text, .. } => {
                    let names: Vec<String> = ed.env.iter().map(|e| e.name.trim().to_string()).collect();
                    if let Some(n) = vars::secret_used(text, &|v| names.iter().any(|s| s.eq_ignore_ascii_case(v))) {
                        ui.label(RichText::new(format!("{} {n}", lang.t("Geheime Variablen gehen nicht an Sitzungen:", "Secret variables are not sent to sessions:"))).small().color(style::ui().danger));
                    }
                }
                _ => {}
            }
        }
        if let Some(i) = drag_start {
            ed.drag = Some(i);
        }
        if let Some(from) = ed.drag {
            let ptr = ui.ctx().pointer_interact_pos();
            if ui.input(|i| i.pointer.any_released()) {
                if let Some(p) = ptr {
                    move_item(&mut ed.rule.actions, from, drop_index(&rows, p.y));
                }
                ed.drag = None;
            }
        }
        if let Some(i) = remove {
            ed.rule.actions.remove(i);
        }
        ui.menu_button(lang.t("+ Aktion", "+ Action"), |ui| {
            for (k, sep) in ADD_KINDS.iter().zip([false, false, false, false, true, false, false, false, false, false, false, false]) {
                let a = new_action(k);
                if ui.button(a.kind.label(lang)).clicked() {
                    ed.rule.actions.push(a);
                    ui.close();
                }
                if sep {
                    ui.separator();
                    ui.label(RichText::new(lang.t("Vor dem Schreiben", "Before writing")).small().color(style::dim(ui)));
                }
            }
        });
        ui.add_space(S2);
        egui::CollapsingHeader::new(RichText::new(lang.t("Befehls-Optionen", "Command options")).strong()).id_salt("sa_cmd_opts").default_open(ed.rule.has_commands()).show(ui, |ui| {
            egui::Grid::new("sa_cmd_grid").num_columns(2).spacing(Vec2::new(10.0, 6.0)).show(ui, |ui| {
                ui.label(lang.t("Shell", "Shell"));
                let current = if ed.rule.shell.is_empty() { lang.t("PowerShell (Standard)", "PowerShell (default)").to_string() } else { cx.shells.iter().find(|(id, _)| *id == ed.rule.shell).map(|(_, l)| l.clone()).unwrap_or_else(|| ed.rule.shell.clone()) };
                egui::ComboBox::from_id_salt("sa_shell").selected_text(current).width(320.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut ed.rule.shell, String::new(), lang.t("PowerShell (Standard)", "PowerShell (default)"));
                    for (id, label) in cx.shells {
                        ui.selectable_value(&mut ed.rule.shell, id.clone(), label);
                    }
                });
                ui.end_row();
                ui.label(lang.t("Arbeitsordner", "Working folder"));
                ui.vertical(|ui| {
                    // Gemeinsames Ordnerfeld (#392, wie Vorlagen und Prozessliste): nach der Shell der Regel.
                    ed.cwd_block = super::folder_picker::field(ui, "sa_rule_cwd", &mut ed.rule.cwd, cx.cwd_target.clone(), false, lang, cx.wsl_recent).blocking;
                    if ed.rule.cwd.trim().is_empty() {
                        ui.label(RichText::new(lang.t("Leer = Regelordner (Datei-Regel: Ordner der Datei)", "Empty = rule folder (file rule: the file's folder)")).small().color(style::dim(ui)));
                    }
                });
                ui.end_row();
                ui.label(lang.t("Zeitlimit", "Time limit"));
                ui.add(egui::DragValue::new(&mut ed.rule.timeout_secs).range(sa::TIMEOUT_MIN..=sa::TIMEOUT_MAX).suffix(" s")).on_hover_text(lang.t("Danach Strg+C, nach 5 s der ganze Prozessbaum", "Then Ctrl+C, after 5 s the whole process tree"));
                ui.end_row();
                ui.label(lang.t("Anzeige", "Display"));
                ui.vertical(|ui| {
                    ui.checkbox(&mut ed.rule.terminal, lang.t("In einem Terminal zeigen (z. B. für Passwortabfragen)", "Show in a terminal (e.g. for password prompts)"));
                    ui.checkbox(&mut ed.rule.notify_success, lang.t("Auch bei Erfolg benachrichtigen", "Notify on success"));
                    ui.checkbox(&mut ed.rule.log_file, lang.t("Ausgabe in eine Protokolldatei (save-action-logs, rotiert)", "Output to a log file (save-action-logs, rotated)"));
                });
                ui.end_row();
            });
            ui.label(RichText::new(lang.t("Umgebungsvariablen", "Environment variables")).strong());
            ui.label(RichText::new(lang.t("Werte werden verschlüsselt gespeichert, nie angezeigt und nie exportiert. Leer lassen = gespeicherten Wert behalten.", "Values are stored encrypted, never shown and never exported. Leave empty = keep the stored value.")).small().color(style::dim(ui)));
            let mut rm = None;
            for (i, r) in ed.env.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut r.name).hint_text("NAME").font(egui::TextStyle::Monospace).desired_width(160.0));
                    let hint = if r.stored.is_some() { lang.t("(gespeichert – unverändert)", "(stored – unchanged)") } else { lang.t("Wert", "value") };
                    ui.add(egui::TextEdit::singleline(&mut r.value).password(true).hint_text(hint).desired_width(240.0));
                    if style::icon_btn_small(ui, SIcon::Close, lang.t("Variable entfernen", "Remove the variable")).clicked() {
                        rm = Some(i);
                    }
                });
            }
            if let Some(i) = rm {
                ed.env.remove(i);
            }
            if ui.button(lang.t("+ Variable", "+ Variable")).clicked() {
                ed.env.push(EnvRow::default());
            }
        });
        if let Some(e) = &ed.error {
            ui.add_space(S1);
            ui.colored_label(style::danger_soft(), e);
        }
        ui.add_space(S2);
        ui.horizontal(|ui| {
            let blocked = ed.cwd_block.clone();
            let sb = ui.add_enabled(blocked.is_none(), egui::Button::new(RichText::new(lang.t("Speichern", "Save")).strong()));
            let sb = match &blocked {
                Some(why) => sb.on_disabled_hover_text(why.as_str()),
                None => sb,
            };
            out.save = sb.clicked();
            #[cfg(test)]
            ui.data_mut(|d| d.insert_temp(Id::new("sa_test_save_btn"), sb.rect));
            out.cancel = ui.button(lang.t("Abbrechen", "Cancel")).clicked();
            ui.add_space(S3);
            out.test = ui.button(lang.t("Für Datei testen …", "Test for file …")).clicked();
            let can_run = cx.can_run;
            let b = ui.add_enabled(can_run, egui::Button::new(lang.t("Jetzt ausführen", "Run now")));
            let b = if can_run { b.on_hover_text(lang.t("Aktionen nach dem Schreiben für die aktive Datei ausführen (gespeicherter Stand der Regel)", "Run the after-write actions for the active file (saved state of the rule)")) } else { b.on_disabled_hover_text(lang.t("Braucht eine gespeicherte Regel und eine offene Datei im Editor.", "Needs a saved rule and an open file in the editor.")) };
            out.run_now = b.clicked();
            if !ed.is_new {
                ui.add_space(S3);
                let label = if ed.confirm_delete { lang.t("Wirklich löschen?", "Really delete?") } else { lang.t("Löschen …", "Delete …") };
                if ui.button(RichText::new(label).color(style::danger_soft())).clicked() {
                    if ed.confirm_delete {
                        out.delete = true;
                    } else {
                        ed.confirm_delete = true;
                    }
                }
            }
        });
    });
    out
}

/// Befehle aus dem Menü *Datei* des Editors ([`App::on_save_menu_items`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OnSaveMenu {
    /// *Aktionen beim Speichern …* für das aktive Dokument (ohne: die Einstellungen).
    Rules(Option<u64>),
    /// *Ohne Aktionen speichern*.
    SaveWithout(u64),
    /// *Beim Speichern formatieren …* (Menü *Format*): neue Regel für die Datei mit der Aktion *Formatieren* vorbelegt
    /// (ohne Datei: die Einstellungen).
    FormatRule(Option<u64>),
    /// *Pausieren* bzw. *Fortsetzen*.
    TogglePause,
}

/// Befehle der Befehls-Palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SaPaletteCmd {
    /// Aktives Dokument des Editors ohne Aktionen speichern.
    SaveWithout,
    /// Pausieren bzw. fortsetzen.
    TogglePause,
    /// *Einstellungen → Editor* öffnen.
    Settings,
    /// Ausgabebereich.
    Output,
}

impl App {
    /// Einträge der Befehls-Palette.
    pub(super) fn sa_palette_items(&self) -> Vec<(String, String, Action)> {
        let lang = self.settings.language;
        let mut v = vec![
            (lang.t("Editor: Aktionen beim Speichern …", "Editor: Actions on save …").to_string(), String::new(), Action::SaveActions(SaPaletteCmd::Settings)),
            (lang.t("Editor: Ausgabe der Aktionen beim Speichern", "Editor: Output of actions on save").to_string(), String::new(), Action::SaveActions(SaPaletteCmd::Output)),
            (
                if self.save_actions.paused { lang.t("Editor: Aktionen beim Speichern fortsetzen", "Editor: Resume actions on save") } else { lang.t("Editor: Aktionen beim Speichern pausieren", "Editor: Pause actions on save") }.to_string(),
                String::new(),
                Action::SaveActions(SaPaletteCmd::TogglePause),
            ),
        ];
        if self.ed.active_doc().is_some() {
            v.push((lang.t("Editor: Ohne Aktionen speichern", "Editor: Save without actions").to_string(), self.binding_text("save_no_actions"), Action::SaveActions(SaPaletteCmd::SaveWithout)));
        }
        v
    }

    pub(super) fn sa_palette(&mut self, c: SaPaletteCmd) {
        match c {
            SaPaletteCmd::SaveWithout => {
                if let Some(d) = self.ed.active_doc() {
                    self.sa_save_without(d);
                }
            }
            SaPaletteCmd::TogglePause => self.save_actions.paused = !self.save_actions.paused,
            SaPaletteCmd::Settings => self.sa_open_settings(),
            SaPaletteCmd::Output => self.save_actions.ui.output_open = true,
        }
    }

    /// Regeln speichern (Fehler als Hinweis).
    fn sa_save(&mut self) {
        if let Err(e) = self.save_actions.save(&self.dir) {
            self.save_actions.ui.msg = Some(e);
        }
    }

    /// Regel-Dialog für eine neue Regel öffnen (Datei oder Ordner vorbelegt; Einstellungen → Editor).
    pub(super) fn sa_new_rule_for(&mut self, path: &Path, is_dir: bool) {
        let id = self.save_actions.cfg.next_id();
        let r = Rule::new(id, if is_dir { Scope::Folder } else { Scope::File }, &path.to_string_lossy());
        self.save_actions.ui.editor = Some(RuleEditor::of(&r, true));
    }

    /// Regel-Dialog für eine neue Regel öffnen, die die Datei beim Speichern formatiert: Muster `*.endung` im Ordner der
    /// Datei (ohne Endung: genau diese Datei) mit der Aktion *Formatieren*. Nichts wird gespeichert, bevor der Nutzer
    /// den Dialog bestätigt.
    pub(super) fn sa_format_rule_for(&mut self, file: &Path) {
        let r = Rule::format_for(self.save_actions.cfg.next_id(), file);
        self.save_actions.ui.editor = Some(RuleEditor::of(&r, true));
    }

    /// Test-Ansicht öffnen (Datei vorbelegt, sonst die aktive Datei des Editors).
    pub(super) fn sa_open_test(&mut self, file: Option<PathBuf>) {
        let file = file.or_else(|| self.ed.active_doc().and_then(|id| self.ed.docs.get(&id)).and_then(|d| d.path.clone()));
        self.save_actions.ui.test = Some(TestView { file, rule: None, cache: None });
    }

    /// Die Einträge der Aktionen beim Speichern im Menü *Datei* der Menüleiste des Editors (Modell in
    /// `editor_ui::menu::on_save_entries`): *Aktionen beim Speichern …* (Test-Ansicht der Datei, ohne Datei die
    /// Einstellungen), *Ohne Aktionen speichern*, *Pausieren*/*Fortsetzen*. Die Menüleiste zeichnet nur mit dem Editor-Zustand;
    /// ein Klick kommt als Befehl hierher.
    pub(crate) fn on_save_menu_items(&mut self, m: OnSaveMenu) {
        match m {
            OnSaveMenu::Rules(doc) => match doc.and_then(|d| self.ed.docs.get(&d)).and_then(|d| d.path.clone()) {
                Some(p) => self.sa_open_test(Some(p)),
                None => self.sa_open_settings(),
            },
            OnSaveMenu::SaveWithout(d) => self.sa_save_without(d),
            OnSaveMenu::FormatRule(doc) => match doc.and_then(|d| self.ed.docs.get(&d)).and_then(|d| d.path.clone()) {
                Some(p) => self.sa_format_rule_for(&p),
                None => self.sa_open_settings(),
            },
            OnSaveMenu::TogglePause => self.save_actions.paused = !self.save_actions.paused,
        }
    }

    /// *Ohne Aktionen speichern* für ein Dokument.
    pub(crate) fn sa_save_without(&mut self, doc: u64) {
        let lang = self.settings.language;
        let dir = self.dir.clone();
        if self.ed.docs.get(&doc).is_some_and(|d| d.path.is_none()) {
            self.save_actions.without_next = true;
            self.editor_save_as_dialog(doc);
            self.save_actions.without_next = false;
            return;
        }
        self.save_actions.without_next = true;
        match self.editor_save_with(doc, None, SaveOrigin::User) {
            Ok(()) => {
                if let Some(d) = self.ed.docs.get_mut(&doc) {
                    d.drop_hot(&dir);
                    let t = d.title(lang);
                    self.ed.set_hint(format!("{t}: {}", lang.t("ohne Aktionen gespeichert", "saved without actions")));
                }
            }
            Err(e) => self.notify_text(e),
        }
        self.save_actions.without_next = false;
    }

    /// *Einstellungen → Editor* öffnen.
    pub(super) fn sa_open_settings(&mut self) {
        self.settings_open = true;
        self.settings_tab = editor_services::SETTINGS_TAB;
    }

    /// Abschnitt *Aktionen beim Speichern* in *Einstellungen → Editor*.
    pub(super) fn save_actions_settings_ui(&mut self, ui: &mut Ui) {
        let lang = self.settings.language;
        let top = ui.cursor().min.y;
        ui.label(RichText::new(lang.t("Aktionen beim Speichern", "Actions on save")).strong());
        ui.label(
            RichText::new(lang.t(
                "Regeln je Datei, Ordner (mit Unterordnern) oder Muster: vor dem Schreiben z. B. Leerraum entfernen oder formatieren, danach Befehle (Lint, Upload mit deinem eigenen scp/rsync …), Kopieren an einen anderen lokalen Ort oder Paneldeck-Aktionen. Die Datei wird immer gespeichert. Gespeichert nur im Datenordner (save-actions.json), nie im Projekt; Agenten können keine Regeln anlegen.",
                "Rules per file, folder (with subfolders) or pattern: before writing e.g. trim whitespace or format, afterwards commands (lint, upload with your own scp/rsync …), copying to another local place or Paneldeck actions. The file is always saved. Stored only in the data folder (save-actions.json), never in the project; agents cannot create rules.",
            ))
            .small()
            .color(style::dim(ui)),
        );
        if let Some(why) = self.save_actions.locked() {
            ui.colored_label(style::ui().warning, why.to_string());
        }
        let mut changed = false;
        ui.horizontal(|ui| {
            changed |= style::toggle(ui, &mut self.save_actions.cfg.enabled).changed();
            ui.label(lang.t("Aktionen beim Speichern ausführen", "Run actions on save"));
            ui.add_space(S3);
            ui.label(lang.t("Parallel (Alle speichern):", "In parallel (Save all):"));
            changed |= ui.add(egui::DragValue::new(&mut self.save_actions.cfg.parallel).range(sa::PARALLEL_MIN..=sa::PARALLEL_MAX)).changed();
        });
        if self.save_actions.paused {
            ui.horizontal(|ui| {
                ui.colored_label(style::ui().warning, lang.t("⏸ Pausiert bis zum Neustart", "⏸ Paused until restart"));
                if ui.small_button(lang.t("Fortsetzen", "Resume")).clicked() {
                    self.save_actions.paused = false;
                }
            });
        }
        ui.add_space(S1);
        // Liste (ziehen zum Sortieren).
        let n = self.save_actions.cfg.rules.len();
        if n == 0 {
            ui.label(RichText::new(lang.t("Noch keine Regeln – ohne Regeln passiert beim Speichern nichts.", "No rules yet – without rules nothing happens on save.")).small().color(style::dim(ui)));
        }
        let mut rows: Vec<Rect> = Vec::with_capacity(n);
        let mut edit = None;
        let mut delete = None;
        let mut dup = None;
        let mut drag_start = None;
        for i in 0..n {
            let r = self.save_actions.cfg.rules[i].clone();
            let missing = self.sa_folder_missing(&r);
            let last = self.save_actions.last.get(&r.id).copied();
            let resp = ui.horizontal(|ui| {
                let g = grip(ui, lang.t("Ziehen zum Sortieren – Regeln laufen in dieser Reihenfolge", "Drag to sort – rules run in this order"));
                if g.drag_started() {
                    drag_start = Some(i);
                }
                let mut on = r.enabled;
                if style::toggle(ui, &mut on).on_hover_text(lang.t("Regel an/aus", "Rule on/off")).changed() {
                    self.save_actions.cfg.rules[i].enabled = on;
                    changed = true;
                }
                let mut sel = self.save_actions.ui.selected.contains(&r.id);
                if ui.checkbox(&mut sel, "").on_hover_text(lang.t("Für den Export wählen", "Select for export")).changed() {
                    if sel {
                        self.save_actions.ui.selected.insert(r.id);
                    } else {
                        self.save_actions.ui.selected.remove(&r.id);
                    }
                }
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        let name = RichText::new(r.display_name()).strong();
                        ui.label(if r.enabled { name } else { name.weak() });
                        if let Some((s, _)) = last {
                            ui.label(RichText::new(s.label(lang)).small().color(status_color(s)));
                        }
                        if missing {
                            ui.label(RichText::new(lang.t("Ordner fehlt", "folder missing")).small().color(style::ui().danger));
                        }
                    });
                    let kinds: Vec<String> = r.actions.iter().map(|a| a.kind.label(lang)).collect();
                    ui.label(RichText::new(format!("{} · {}", scope_text(&r, lang), kinds.join(", "))).small().color(style::dim(ui)));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if style::icon_btn_small(ui, SIcon::Trash, lang.t("Regel löschen", "Delete the rule")).clicked() {
                        delete = Some(r.id);
                    }
                    if ui.small_button(lang.t("Duplizieren", "Duplicate")).clicked() {
                        dup = Some(i);
                    }
                    if ui.small_button(lang.t("Bearbeiten …", "Edit …")).clicked() {
                        edit = Some(i);
                    }
                });
            });
            rows.push(resp.response.rect);
        }
        if let Some(i) = drag_start {
            self.save_actions.ui.drag = Some(i);
        }
        if let Some(from) = self.save_actions.ui.drag {
            let ptr = ui.ctx().pointer_interact_pos();
            if let Some(p) = ptr {
                let to = drop_index(&rows, p.y);
                let y = rows.get(to).map(|r| r.top()).or_else(|| rows.last().map(|r| r.bottom())).unwrap_or(p.y);
                let x = rows.first().map(|r| (r.left(), r.right())).unwrap_or((0.0, 0.0));
                ui.painter().line_segment([Pos2::new(x.0, y), Pos2::new(x.1, y)], Stroke::new(2.0_f32, style::accent()));
            }
            if ui.input(|i| i.pointer.any_released()) {
                if let Some(p) = ptr {
                    let to = drop_index(&rows, p.y);
                    if move_item(&mut self.save_actions.cfg.rules, from, to) {
                        changed = true;
                    }
                }
                self.save_actions.ui.drag = None;
            }
        }
        if let Some(i) = edit {
            let r = self.save_actions.cfg.rules[i].clone();
            self.save_actions.ui.editor = Some(RuleEditor::of(&r, false));
        }
        if let Some(i) = dup {
            let mut r = self.save_actions.cfg.rules[i].clone();
            r.id = self.save_actions.cfg.next_id();
            r.name = format!("{} ({})", r.display_name(), lang.t("Kopie", "copy"));
            self.save_actions.cfg.rules.insert(i + 1, r);
            changed = true;
        }
        if let Some(id) = delete {
            self.save_actions.cfg.rules.retain(|r| r.id != id);
            self.save_actions.ui.selected.remove(&id);
            changed = true;
        }
        ui.add_space(S1);
        ui.horizontal_wrapped(|ui| {
            if ui.button(lang.t("+ Regel", "+ Rule")).clicked() {
                let start = self.editor_root().unwrap_or_default();
                self.sa_new_rule_for(&start, true);
            }
            if ui.button(lang.t("Für Datei testen …", "Test for file …")).clicked() {
                self.sa_open_test(None);
            }
            if ui.button(lang.t("Ausgabe", "Output")).clicked() {
                self.save_actions.ui.output_open = true;
            }
            let sel: Vec<u64> = self.save_actions.cfg.rules.iter().filter(|r| self.save_actions.ui.selected.contains(&r.id)).map(|r| r.id).collect();
            if ui.add_enabled(!sel.is_empty(), egui::Button::new(lang.t("Gewählte exportieren …", "Export selected …"))).clicked() {
                let rules: Vec<&Rule> = self.save_actions.cfg.rules.iter().filter(|r| sel.contains(&r.id)).collect();
                let text = sa::exchange::export(&rules);
                if let Some(p) = crate::platform::notify::save_json("save-actions-export.json") {
                    if let Err(e) = std::fs::write(&p, text) {
                        self.save_actions.ui.msg = Some(e.to_string());
                    }
                }
            }
            if ui.button(lang.t("Importieren …", "Import …")).clicked() {
                if let Some(p) = crate::platform::notify::open_json() {
                    match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|t| sa::exchange::import(&t, self.save_actions.cfg.next_id())) {
                        Ok(rules) => {
                            let chosen = vec![true; rules.len()];
                            self.save_actions.ui.import = Some(ImportView { rules, chosen });
                        }
                        Err(e) => self.save_actions.ui.msg = Some(format!("{}: {e}", lang.t("Import nicht möglich", "Import not possible"))),
                    }
                }
            }
        });
        if let Some(m) = self.save_actions.ui.msg.clone() {
            ui.horizontal(|ui| {
                ui.colored_label(style::danger_soft(), m);
                if ui.small_button("×").clicked() {
                    self.save_actions.ui.msg = None;
                }
            });
        }
        super::tutorial::spot_since(ui, Spot::EditorSaveActions, top);
        if changed {
            self.sa_save();
        }
    }

    /// Fehlt der Ordner einer Regel? (Höchstens alle 5 s geprüft; Netzwerkpfade nur kurz, ohne den UI-Thread zu
    /// blockieren.)
    fn sa_folder_missing(&mut self, r: &Rule) -> bool {
        if let Some((m, at)) = self.save_actions.ui.missing.get(&r.id)
            && at.elapsed() < Duration::from_secs(5)
        {
            return *m;
        }
        let p = r.path.trim();
        let m = !p.is_empty()
            && match r.scope {
                Scope::File => crate::pathfmt::dir_exists_quick(&r.folder().to_string_lossy(), Duration::from_millis(50)) == Some(false),
                _ => crate::pathfmt::dir_exists_quick(p, Duration::from_millis(50)) == Some(false),
            };
        self.save_actions.ui.missing.insert(r.id, (m, Instant::now()));
        m
    }

    /// Dialoge und Fenster (im Hauptfenster, jedes Bild).
    pub(super) fn save_actions_dialogs_ui(&mut self, ctx: &egui::Context) {
        self.sa_rule_editor_ui(ctx);
        self.sa_test_ui(ctx);
        self.sa_import_ui(ctx);
        self.sa_output_ui(ctx);
        self.sa_quit_ui(ctx);
    }

    fn sa_rule_editor_ui(&mut self, ctx: &egui::Context) {
        let Some(mut ed) = self.save_actions.ui.editor.take() else { return };
        let lang = self.settings.language;
        let title = if ed.is_new { lang.t("Neue Regel", "New rule") } else { lang.t("Regel bearbeiten", "Edit rule") };
        let shells: Vec<(String, String)> = self.shells.iter().map(|s| (s.id.clone(), shells::display_label(s, lang))).collect();
        let sessions: Vec<(u64, String)> = self.entries.iter().filter(|(_, e)| !e.is_chat()).map(|(id, e)| (*id, e.display_name())).collect();
        let processes: Vec<(u64, String)> = self.procs.defs.iter().map(|d| (d.id, d.name.clone())).collect();
        let groups = procs::groups(&self.procs.defs);
        let active_file = self.ed.active_doc().and_then(|id| self.ed.docs.get(&id)).and_then(|d| d.path.clone());
        let esc = ctx.input(|i| i.key_pressed(Key::Escape)) && !egui::Popup::is_any_open(ctx);
        let win = style::fit_to_screen(ctx, style::movable(ctx, Id::new("sa_rule_editor"), egui::Window::new(title)), [640.0, 720.0], [480.0, 360.0]).collapsible(false);
        let target = procs::resolve_shell(&ed.rule.shell, &self.shells).map(vars::Target::of);
        let quote = target.as_ref().map(|t| t.quote).unwrap_or(crate::quote::Kind::Pwsh);
        let cmd = target.as_ref().is_some_and(|t| t.flavor == procs::Flavor::Cmd);
        let cwd_target = super::folder_picker::Target::for_shell(procs::resolve_shell(&ed.rule.shell, &self.shells), false);
        let wsl_recent = self.settings.wsl_recent.clone();
        let cx = DialogCx { lang, shells: &shells, sessions: &sessions, processes: &processes, groups: &groups, can_run: active_file.is_some() && !ed.is_new, quote, cmd, cwd_target, wsl_recent: &wsl_recent };
        let mut out = DialogOut::default();
        win.show(ctx, |ui| out = rule_editor_body(ui, &mut ed, &cx));
        let DialogOut { save, cancel, test, run_now, delete } = out;
        if cancel || esc {
            return;
        }
        if delete {
            let id = ed.rule.id;
            self.save_actions.cfg.rules.retain(|r| r.id != id);
            self.sa_save();
            return;
        }
        if test {
            match ed.build(lang) {
                Ok(r) => self.save_actions.ui.test = Some(TestView { file: active_file.clone(), rule: Some(Box::new(r)), cache: None }),
                Err(e) => ed.error = Some(e),
            }
        }
        if run_now && let Some(f) = &active_file {
            let n = self.sa_run_now(f, Some(ed.rule.id));
            if n == 0 {
                ed.error = Some(lang.t("Diese Regel hat keine Aktionen nach dem Schreiben.", "This rule has no actions after writing.").into());
            } else {
                self.save_actions.ui.output_open = true;
            }
        }
        if save {
            match ed.build(lang) {
                Ok(r) => {
                    match self.save_actions.cfg.rules.iter_mut().find(|x| x.id == r.id) {
                        Some(x) => *x = r,
                        None => self.save_actions.cfg.rules.push(r),
                    }
                    self.save_actions.ui.missing.clear();
                    self.sa_save();
                    return;
                }
                Err(e) => ed.error = Some(e),
            }
        }
        self.save_actions.ui.editor = Some(ed);
    }

    /// Test-Ansicht: passende Regeln (mit Gründen für übersprungene), Text-Aktionen und die fertigen Befehlszeilen.
    fn sa_test_ui(&mut self, ctx: &egui::Context) {
        let Some(mut tv) = self.save_actions.ui.test.take() else { return };
        let lang = self.settings.language;
        let mut open = true;
        let mut run = false;
        let mut new_rule = false;
        // Konfiguration für den Test: mit dem ungespeicherten Stand der Regel aus dem Dialog.
        let mut cfg = self.save_actions.cfg.clone();
        if let Some(r) = &tv.rule {
            match cfg.rules.iter_mut().find(|x| x.id == r.id) {
                Some(x) => *x = (**r).clone(),
                None => cfg.rules.push((**r).clone()),
            }
        }
        let key = {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            tv.file.hash(&mut h);
            cfg.to_json().to_string().hash(&mut h);
            self.save_actions.paused.hash(&mut h);
            self.editor_root().hash(&mut h);
            h.finish()
        };
        if tv.cache.as_ref().is_none_or(|c| c.0 != key) {
            let (plan, lines) = self.sa_test_compute(&cfg, tv.file.as_deref());
            tv.cache = Some((key, plan, lines));
        }
        let (plan, lines) = match &tv.cache {
            Some((_, p, l)) if tv.file.is_some() => (Some(p.clone()), l.clone()),
            _ => (None, Vec::new()),
        };
        let name_of = |id: u64| cfg.rule(id).map(|r| r.display_name()).unwrap_or_default();
        style::fit_to_screen(ctx, style::movable(ctx, Id::new("sa_test"), egui::Window::new(lang.t("Regeln für diese Datei", "Rules for this file"))), [620.0, 460.0], [360.0, 240.0]).collapsible(false).open(&mut open).show(ctx, |ui| {
            ui.horizontal(|ui| {
                let shown = tv.file.as_ref().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| lang.t("(keine Datei)", "(no file)").to_string());
                ui.label(RichText::new(shown).monospace());
                if ui.button(lang.t("Datei wählen …", "Choose file …")).clicked() {
                    let start = tv.file.as_ref().and_then(|f| f.parent()).map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                    if let Some(p) = crate::platform::notify::pick_file(&start) {
                        tv.file = Some(p);
                    }
                }
            });
            if tv.rule.is_some() {
                ui.label(RichText::new(lang.t("Mit dem ungespeicherten Stand der Regel aus dem Dialog.", "With the unsaved state of the rule from the dialog.")).small().color(style::dim(ui)));
            }
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink([false, true]).max_height(320.0).show(ui, |ui| {
                let Some(p) = &plan else {
                    ui.label(lang.t("Bitte eine Datei wählen.", "Please choose a file."));
                    return;
                };
                if p.rules.is_empty() && p.skipped.is_empty() {
                    ui.label(lang.t("Keine Regel passt zu dieser Datei.", "No rule matches this file."));
                }
                if !p.builtins.is_empty() {
                    let names: Vec<String> = p.builtins.iter().map(|(b, ..)| b.label(lang)).collect();
                    ui.label(format!("{} {}", lang.t("Vor dem Schreiben:", "Before writing:"), names.join(", ")));
                }
                for (_, name, out) in &lines {
                    ui.label(RichText::new(format!("✓ {name}")).strong().color(style::ui().success));
                    for l in out {
                        ui.label(RichText::new(format!("   {l}")).monospace().small());
                    }
                }
                for (_, name, why) in &p.skipped {
                    ui.label(RichText::new(format!("– {name}: {}", why.text(lang, name_of))).color(style::dim(ui)));
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                let can = tv.file.is_some() && plan.as_ref().is_some_and(|p| p.rules.iter().any(|r| !r.post.is_empty())) && tv.rule.is_none();
                run = ui.add_enabled(can, egui::Button::new(lang.t("Jetzt ausführen", "Run now"))).on_hover_text(lang.t("Aktionen nach dem Schreiben für diese Datei ausführen, ohne zu speichern", "Run the after-write actions for this file without saving")).clicked();
                new_rule = ui.add_enabled(tv.file.is_some(), egui::Button::new(lang.t("Neue Regel für diese Datei …", "New rule for this file …"))).clicked();
            });
        });
        if run && let Some(f) = tv.file.clone() {
            self.sa_run_now(&f, None);
            self.save_actions.ui.output_open = true;
        }
        if new_rule && let Some(f) = tv.file.clone() {
            self.sa_new_rule_for(&f, false);
        }
        if open {
            self.save_actions.ui.test = Some(tv);
        }
    }

    /// Plan und Zeilen der Test-Ansicht (Befehlszeilen mit Hinweisen, Kopierziele).
    fn sa_test_compute(&self, cfg: &sa::Config, file: Option<&Path>) -> (matching::Plan, Vec<(u64, String, Vec<String>)>) {
        let lang = self.settings.language;
        let mut lines: Vec<(u64, String, Vec<String>)> = Vec::new();
        let Some(f) = file else { return (matching::Plan::default(), lines) };
        let owned = f.to_path_buf();
        let f = &owned;
        let p = matching::plan(cfg, f, SaveOrigin::User, Switches { paused: self.save_actions.paused, without_actions: false });
        for pr in &p.rules {
            let Some(def) = cfg.rule(pr.rule) else { continue };
            let vctx = vars::Ctx { file: f.clone(), rule_folder: def.folder(), workspace: self.editor_root() };
            let mut out = Vec::new();
            for a in &pr.post {
                out.push(match &a.kind {
                    ActionKind::Command { command } => match self.sa_prepare(def, command, &vctx) {
                        Ok(p) => {
                            let mut s = format!("{} ({}): {}", lang.t("Befehl", "Command"), p.shell.label, p.display);
                            for n in &p.notes {
                                s.push_str(&format!("\n    ⚠ {n}"));
                            }
                            s
                        }
                        Err(e) => format!("{} ✗ {e}", lang.t("Befehl", "Command")),
                    },
                    ActionKind::Copy { target, keep_structure } => match sa::copy::destination(f, &def.folder(), target, *keep_structure) {
                        Ok(d) => format!("{} → {}", lang.t("Kopieren", "Copy"), d.display()),
                        Err(e) => format!("{} ✗ {}", lang.t("Kopieren", "Copy"), e.text(lang)),
                    },
                    k => k.label(lang),
                });
            }
            lines.push((pr.rule, pr.name.clone(), out));
        }
        (p, lines)
    }

    /// Vorschau beim Import: alle Befehle zeigen; importierte Regeln kommen ausgeschaltet an.
    fn sa_import_ui(&mut self, ctx: &egui::Context) {
        let Some(mut iv) = self.save_actions.ui.import.take() else { return };
        let lang = self.settings.language;
        let (mut ok, mut cancel) = (false, false);
        style::fit_to_screen(ctx, style::movable(ctx, Id::new("sa_import"), egui::Window::new(lang.t("Regeln importieren", "Import rules"))), [600.0, 440.0], [360.0, 240.0]).collapsible(false).show(ctx, |ui| {
            ui.label(RichText::new(lang.t("Bitte prüfen: Diese Befehle würden beim Speichern laufen. Importierte Regeln sind ausgeschaltet, Werte von Umgebungsvariablen fehlen.", "Please check: these commands would run on save. Imported rules are switched off, values of environment variables are missing.")).color(style::ui().warning));
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                for (i, r) in iv.rules.iter().enumerate() {
                    ui.checkbox(&mut iv.chosen[i], RichText::new(r.display_name()).strong());
                    ui.label(RichText::new(scope_text(r, lang)).small().color(style::dim(ui)));
                    let mut trig = Vec::new();
                    if r.on_autosave {
                        trig.push(lang.t("auch beim automatischen Speichern", "also on autosave"));
                    }
                    if r.on_agent_save {
                        trig.push(lang.t("auch wenn ein Agent speichert", "also when an agent saves"));
                    }
                    if !trig.is_empty() {
                        ui.label(RichText::new(trig.join(" · ")).small().color(style::ui().warning));
                    }
                    let unknown: Vec<String> = r.actions.iter().filter(|a| matches!(a.kind, ActionKind::Unknown(_))).map(|a| a.kind.kind()).collect();
                    if !unknown.is_empty() {
                        ui.label(RichText::new(format!("{} {}", lang.t("Unbekannte Aktionen (werden übersprungen):", "Unknown actions (skipped):"), unknown.join(", "))).small().color(style::ui().warning));
                    }
                    for c in sa::exchange::commands_of(r) {
                        ui.label(RichText::new(format!("   {c}")).monospace().small());
                    }
                }
            });
            ui.horizontal(|ui| {
                ok = ui.button(RichText::new(lang.t("Importieren", "Import")).strong()).clicked();
                cancel = ui.button(lang.t("Abbrechen", "Cancel")).clicked();
            });
        });
        if cancel {
            return;
        }
        if ok {
            let mut id = self.save_actions.cfg.next_id();
            for (r, c) in iv.rules.into_iter().zip(iv.chosen) {
                if c {
                    let mut r = r;
                    r.id = id;
                    id += 1;
                    self.save_actions.cfg.rules.push(r);
                }
            }
            self.sa_save();
            return;
        }
        self.save_actions.ui.import = Some(iv);
    }

    /// Ausgabebereich: letzte 100 Läufe (nur im Speicher), Ausgabe des gewählten, *Abbrechen*.
    fn sa_output_ui(&mut self, ctx: &egui::Context) {
        if !self.save_actions.ui.output_open {
            return;
        }
        let lang = self.settings.language;
        let mut open = true;
        let mut cancel = None;
        let mut cancel_all = false;
        let mut clear = false;
        let running = self.save_actions.busy();
        let sel = self.save_actions.ui.output_sel.or_else(|| self.save_actions.history.front().map(|r| r.id));
        let live = sel.and_then(|s| self.save_actions.live_lines(s));
        style::fit_to_screen(ctx, style::movable(ctx, Id::new("sa_output"), egui::Window::new(lang.t("Ausgabe: Aktionen beim Speichern", "Output: actions on save"))), [760.0, 520.0], [420.0, 260.0]).collapsible(false).open(&mut open).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("{} {running}", lang.t("Laufend/wartend:", "Running/waiting:")));
                if ui.add_enabled(running > 0, egui::Button::new(lang.t("Alle abbrechen", "Cancel all"))).clicked() {
                    cancel_all = true;
                }
                if ui.button(lang.t("Verlauf leeren", "Clear history")).clicked() {
                    clear = true;
                }
            });
            ui.separator();
            let h = ui.available_height();
            ui.horizontal_top(|ui| {
                egui::ScrollArea::vertical().id_salt("sa_out_list").max_width(330.0).max_height(h).auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_width(320.0);
                    if self.save_actions.history.is_empty() {
                        ui.label(RichText::new(lang.t("Noch keine Läufe.", "No runs yet.")).color(style::dim(ui)));
                    }
                    for r in &self.save_actions.history {
                        let when = chrono::DateTime::from_timestamp(r.at as i64, 0).map(|d| d.with_timezone(&chrono::Local).format("%H:%M:%S").to_string()).unwrap_or_default();
                        let f = r.file.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
                        let text = format!("{when}  {f} – {}  [{}]", r.rule_name, r.status.label(lang));
                        let resp = ui.selectable_label(sel == Some(r.id), RichText::new(text).small().color(status_color(r.status)));
                        if resp.clicked() {
                            self.save_actions.ui.output_sel = Some(r.id);
                        }
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    let Some(r) = sel.and_then(|s| self.save_actions.history.iter().find(|r| r.id == s)) else { return };
                    ui.label(RichText::new(format!("{} – {}", r.rule_name, r.action)).strong());
                    ui.label(RichText::new(r.file.to_string_lossy()).small().monospace());
                    if !r.command.is_empty() {
                        ui.label(RichText::new(&r.command).small().monospace());
                    }
                    let code = r.exit_code.map(|c| format!(" · {} {c}", lang.t("Exit-Code", "exit code"))).unwrap_or_default();
                    ui.label(RichText::new(format!("{}{code} · {} ms", r.status.label(lang), r.duration_ms)).color(status_color(r.status)));
                    if let Some(m) = &r.message {
                        ui.label(RichText::new(m).small());
                    }
                    if self.save_actions.cancellable(r.id) && ui.button(lang.t("Abbrechen", "Cancel")).clicked() {
                        cancel = Some(r.id);
                    }
                    ui.separator();
                    let lines = live.as_ref().unwrap_or(&r.output);
                    egui::ScrollArea::both().id_salt("sa_out_text").stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                        if lines.is_empty() {
                            ui.label(RichText::new(lang.t("(keine Ausgabe)", "(no output)")).small().color(style::dim(ui)));
                        }
                        for l in lines {
                            ui.label(RichText::new(l).monospace().small());
                        }
                    });
                });
            });
        });
        if let Some(id) = cancel {
            self.sa_cancel_record(id);
        }
        if cancel_all {
            self.sa_cancel_all();
        }
        if clear {
            self.save_actions.history.retain(|r| r.status == RunStatus::Running);
        }
        self.save_actions.ui.output_open = open;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_rules_and_actions() {
        let mut v = vec![1, 2, 3, 4];
        assert!(move_item(&mut v, 0, 2));
        assert_eq!(v, [2, 1, 3, 4]);
        assert!(move_item(&mut v, 3, 0));
        assert_eq!(v, [4, 2, 1, 3]);
        assert!(!move_item(&mut v, 1, 1) && !move_item(&mut v, 1, 2), "an der eigenen Stelle nichts");
        assert!(move_item(&mut v, 0, 4));
        assert_eq!(v, [2, 1, 3, 4]);
        let rows: Vec<Rect> = (0..3).map(|i| Rect::from_min_size(Pos2::new(0.0, i as f32 * 20.0), Vec2::new(100.0, 20.0))).collect();
        assert_eq!(drop_index(&rows, 5.0), 0);
        assert_eq!(drop_index(&rows, 15.0), 1);
        assert_eq!(drop_index(&rows, 99.0), 3);
    }


    fn frame(ctx: &egui::Context, ed: &mut RuleEditor, cx: &DialogCx, events: Vec<egui::Event>, t: &mut f64) -> DialogOut {
        *t += 0.05;
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 1400.0))), time: Some(*t), events, ..Default::default() };
        let mut out = DialogOut::default();
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| out = rule_editor_body(ui, ed, cx));
        });
        out
    }

    /// Der Regel-Dialog zeichnet jede Aktionsart ohne Panik und meldet *Speichern* per Klick (ohne Fenster).
    #[test]
    fn rule_dialog_draws_every_action_kind_and_save_click_is_reported() {
        let base = if cfg!(windows) { r"C:\work" } else { "/work" };
        let mut r = Rule::new(1, Scope::Pattern, base);
        r.patterns = vec!["*.json".into()];
        r.actions = ADD_KINDS.iter().map(|k| new_action(k)).collect();
        r.actions[0] = RuleAction::new(ActionKind::Command { command: "lint \"${file}\"".into() });
        for lang in [Lang::De, Lang::En] {
            let mut ed = RuleEditor::of(&r, false);
            let shells = vec![("pwsh".to_string(), "PowerShell 7".to_string())];
            let sessions = vec![(3u64, "Build".to_string())];
            let processes = vec![(2u64, "web".to_string())];
            let groups = vec!["Backend".to_string()];
            let recent = std::collections::BTreeMap::new();
            let cx = DialogCx { lang, shells: &shells, sessions: &sessions, processes: &processes, groups: &groups, can_run: true, quote: crate::quote::Kind::Pwsh, cmd: false, cwd_target: crate::app::folder_picker::Target::Windows, wsl_recent: &recent };
            let ctx = egui::Context::default();
            let mut t = 0.0;
            for _ in 0..3 {
                assert_eq!(frame(&ctx, &mut ed, &cx, vec![], &mut t), DialogOut::default());
            }
            let btn: Rect = ctx.data(|d| d.get_temp(Id::new("sa_test_save_btn"))).expect("Speichern gezeichnet");
            let p = btn.center();
            frame(&ctx, &mut ed, &cx, vec![egui::Event::PointerMoved(p)], &mut t);
            frame(&ctx, &mut ed, &cx, vec![egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }], &mut t);
            let out = frame(&ctx, &mut ed, &cx, vec![egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }], &mut t);
            assert!(out.save, "{lang:?}: Klick auf Speichern");
            assert!(!out.cancel && !out.delete);
        }
    }
    #[test]
    fn editor_checks_input_and_encrypts_values() {
        let lang = Lang::En;
        let base = if cfg!(windows) { r"C:\work" } else { "/work" };
        let mut ed = RuleEditor::of(&Rule::new(1, Scope::Pattern, base), true);
        assert!(ed.build(lang).is_err(), "Muster fehlt");
        ed.patterns = "*.ts\n\n  src/**  ".into();
        assert!(ed.build(lang).is_err(), "Aktion fehlt");
        ed.rule.actions.push(new_action("command"));
        assert!(ed.build(lang).is_err(), "leerer Befehl");
        ed.rule.actions[0] = RuleAction::new(ActionKind::Command { command: "npm run lint".into() });
        let r = ed.build(lang).unwrap();
        assert_eq!(r.patterns, ["*.ts", "src/**"]);
        ed.rule.actions.push(RuleAction::new(ActionKind::Copy { target: format!("{base}{}dist", std::path::MAIN_SEPARATOR), keep_structure: true }));
        assert!(ed.build(lang).is_err(), "Ziel im Regelordner");
        ed.rule.actions.pop();
        ed.env.push(EnvRow { name: "PANELDECK_X".into(), value: "1".into(), stored: None });
        assert!(ed.build(lang).is_err(), "eigene Variablen dürfen nicht PANELDECK_ heißen");
        ed.env[0].name = "TOKEN".into();
        if let Ok(r) = ed.build(lang) {
            assert!(r.env[0].value_protected.as_deref().is_some_and(|v| v != "1"), "Wert nie im Klartext");
        }
        ed.rule.scope = Scope::Folder;
        ed.rule.path = "relative".into();
        assert!(ed.build(lang).is_err());
    }
}

/// Nur für die Doku-Screenshots (Feature `readme-shots`, nie im Release): Beispiel-Regeln und der Regel-Dialog.
#[cfg(feature = "readme-shots")]
impl App {
    pub(super) fn shot_save_actions_demo(&mut self, demo: &Path) {
        let lang = self.settings.language;
        let folder = demo.to_string_lossy().to_string();
        let mut fmt = Rule::new(1, Scope::Pattern, &folder);
        fmt.name = "Rust".into();
        fmt.patterns = vec!["src/**/*.rs".into(), "!target/**".into()];
        fmt.actions = vec![
            RuleAction::new(ActionKind::Builtin(Builtin::TrimWhitespace)),
            RuleAction::new(ActionKind::Builtin(Builtin::FinalNewline)),
            RuleAction::new(ActionKind::Command { command: "rustfmt --edition 2021 ${file}".into() }),
            RuleAction { kind: ActionKind::Command { command: "cargo clippy --quiet".into() }, continue_on_error: true },
        ];
        let mut up = Rule::new(2, Scope::Folder, &folder);
        up.name = lang.t("Auf den Testserver hochladen", "Upload to the test server").into();
        up.actions = vec![
            RuleAction::new(ActionKind::Command { command: "scp ${file} deploy@test.example:/srv/acme/${relativeFile}".into() }),
            RuleAction::new(ActionKind::Notify { text: lang.t("${fileBasename} hochgeladen", "${fileBasename} uploaded").into() }),
        ];
        up.enabled = false;
        self.save_actions.cfg.rules = vec![fmt.clone(), up];
        self.save_actions.last.insert(1, (RunStatus::Ok, sa::now_secs()));
        let mut ed = RuleEditor::of(&fmt, false);
        // neutraler Pfad statt des Ordners der Aufnahme (der Dialog wird nicht gespeichert)
        ed.rule.path = if cfg!(windows) { r"C:\Dev\acme-shop".into() } else { "/home/dev/acme-shop".into() };
        ed.env.push(EnvRow { name: "RUSTFLAGS".into(), value: String::new(), stored: Some("x".into()) });
        self.save_actions.ui.editor = Some(ed);
    }
}
