//! Formatieren im Editor (#381, 3.2.0): JSON/XML formatieren, minifizieren, prüfen, Schlüssel sortieren – aus der
//! Menüleiste, über Tastenkürzel und die Befehls-Palette, beim Speichern und für Agenten (MCP `format_file`).
//!
//! Die Arbeit läuft im Hilfsthread ([`crate::editor_svc::format_job`]); hier liegt, was die Oberfläche dazu braucht:
//! laufende Läufe je Dokument (Fortschrittszeile mit *Abbrechen* in jeder Teilung, die das Dokument zeigt), das
//! Anwenden des Ergebnisses als **ein** Rückgängig-Schritt (nur wenn das Dokument unverändert ist; Cursor bleibt auf
//! seiner Zeile/Spalte), Meldungen (*Gültig*, Fehler mit Zeile/Spalte und *Gehe zu*), die Rückfrage vor *Minify* bei
//! JSONC (Kommentare fallen weg) und das Formatieren beim Speichern (synchron, nur kleine Dateien, nie auf Kosten des
//! Speicherns).

use super::doc::DocEntry;
use super::view::Cmd;
use super::EditorUi;
use crate::editor::format::{FormatError, Kind, Op, Options};
use crate::editor::{EditOrigin, Selections};
use crate::editor_svc::format_job::{self, Done, Job, Request, Runner};
use crate::state::Lang;
use crate::style;
use eframe::egui::{self, Id};
use std::collections::HashMap;
use std::sync::Arc;

/// Ergebnis des Formatierens beim Speichern.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct SaveFormat {
    /// Hinweis, wenn nicht formatiert wurde (zu groß, ungültig).
    pub hint: Option<String>,
    /// Der Text wurde geändert.
    pub formatted: bool,
    /// Der Formatierer lief über das ganze Dokument: `Ok` = gültig (formatiert oder schon so), `Err` = ungültig (Meldung
    /// wie bei der Aktion *Formatieren* einer Regel). `None` = nicht gelaufen (aus, zu groß, schreibgeschützt, keine Art).
    pub ran: Option<Result<(), String>>,
}

/// Ergebnis von [`EditorUi::save_doc`].
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SavedDoc {
    /// Hinweis des Formatierens beim Speichern (für die Statusleiste).
    pub hint: Option<String>,
    /// Je eingebauter Aktion der Regeln: `Ok(None)`, `Ok(Some(hinweis))` (nichts geändert) oder `Err(fehler)`.
    pub builtins: Vec<(crate::save_actions::Builtin, Result<Option<String>, String>)>,
}

/// Ein laufender Lauf des Nutzers.
pub(super) struct UiJob {
    pub job: Job,
    pub doc: u64,
    /// Teilung, aus der er gestartet wurde (deren Cursor zählt).
    pub group: u64,
    /// Auf einer Auswahl gestartet.
    pub selection: bool,
}

/// Meldung zu einem Dokument (Leiste über dem Text, bis zur nächsten Änderung oder *Ausblenden*).
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Note {
    /// Ungültig; Position 1-basiert im Dokument.
    Invalid(Kind, FormatError),
    Valid(Kind),
    /// Sonstiger Hinweis (schon formatiert, abgebrochen, geändert während des Laufs …).
    Info(String),
}

/// Rückfrage vor *Minify* von JSONC.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct MinifyAsk {
    pub doc: u64,
    pub group: u64,
}

/// Zustand des Formatierens im Editor.
#[derive(Default)]
pub(crate) struct FmtState {
    pub(super) jobs: Vec<UiJob>,
    /// Meldung je Dokument mit dem Stand (`revision`), für den sie gilt.
    pub(super) notes: HashMap<u64, (Note, u64)>,
    /// Rückfragen vor *Minify* von JSONC, höchstens eine je Dokument (die erste wird gezeigt).
    pub(super) ask: Vec<MinifyAsk>,
    /// Eigener Formatierer (Tests: langsamer Ersatz für Fortschritt und *Abbrechen*); sonst der eingebaute.
    pub(super) runner: Option<Arc<Runner>>,
}

impl FmtState {
    pub(super) fn job_for(&self, doc: u64) -> Option<&UiJob> {
        self.jobs.iter().find(|j| j.doc == doc)
    }
}

/// Text eines Befehls für Menü und Palette.
pub(crate) fn op_label(op: Op, kind: Option<Kind>, selection: bool, lang: Lang) -> String {
    let what = kind.map(format_job::kind_name).map(|k| if k == "JSONC" { "JSON" } else { k });
    match (op, what) {
        (Op::Format, Some(k)) if selection => match lang {
            Lang::De => format!("Auswahl als {k} formatieren"),
            Lang::En => format!("Format selection as {k}"),
        },
        (Op::Format, Some(k)) => match lang {
            Lang::De => format!("{k} formatieren"),
            Lang::En => format!("Format {k}"),
        },
        (Op::Format, None) => lang.t("Formatieren", "Format").to_string(),
        (Op::Minify, _) => lang.t("Minifizieren", "Minify").to_string(),
        (Op::Validate, Some(k)) => match lang {
            Lang::De => format!("{k} prüfen"),
            Lang::En => format!("Validate {k}"),
        },
        (Op::Validate, None) => lang.t("Prüfen", "Validate").to_string(),
        (Op::SortKeys, _) => lang.t("JSON-Schlüssel sortieren", "Sort JSON keys").to_string(),
    }
}

/// Fortschrittstext („JSON formatieren … 42 %“).
fn progress_text(op: Op, p: f32, lang: Lang) -> String {
    let what = match op {
        Op::Format => lang.t("Formatiere", "Formatting"),
        Op::Minify => lang.t("Minifiziere", "Minifying"),
        Op::Validate => lang.t("Prüfe", "Validating"),
        Op::SortKeys => lang.t("Sortiere Schlüssel", "Sorting keys"),
    };
    format!("{what} … {} %", (p.clamp(0.0, 1.0) * 100.0).round() as u32)
}

/// Text einer Fehlermeldung (Meldung des Parsers bleibt englisch, Rahmen übersetzt).
pub(super) fn invalid_text(kind: Kind, e: &FormatError, lang: Lang) -> String {
    let k = format_job::kind_name(kind);
    match lang {
        Lang::De => format!("Ungültiges {k} in Zeile {}, Spalte {}: {} – nichts wurde geändert.", e.line, e.column, e.message),
        Lang::En => format!("Invalid {k} at line {}, column {}: {} – nothing was changed.", e.line, e.column, e.message),
    }
}

impl DocEntry {
    /// Art des Dokuments für Menüs (billig: Pfad, Textanfang, gewählte Sprache). `None` = weder JSON noch XML oder noch
    /// nicht gelesen.
    pub(super) fn format_kind(&self) -> Option<Kind> {
        let doc = self.doc.as_ref()?;
        format_job::quick_kind(self.path.as_deref(), doc.rope(), self.syntax.lang.as_deref())
    }

    /// Auftrag für einen Lauf über das ganze Dokument bzw. `range` (Zeichen).
    fn format_request(&self, op: Op, range: Option<std::ops::Range<usize>>, confirmed: bool) -> Option<(Request, u64)> {
        let doc = self.doc.as_ref()?;
        let rope = doc.rope().clone();
        let final_newline = rope.len_chars() > 0 && rope.char(rope.len_chars() - 1) == '\n';
        let opts = Options { indent: format_job::indent_of(&doc.indent_settings()), final_newline };
        let lang = self.syntax.lang.clone();
        Some((Request { op, path: self.path.clone(), lang, rope, range, opts, confirmed, diff_max: format_job::DIFF_MAX_BYTES }, doc.revision()))
    }

    /// Änderungen eines Laufs als einen Rückgängig-Schritt anwenden. `sels` = Cursor der Ansicht (sonst die eigenen);
    /// `whole` / ganzes Dokument: der Cursor bleibt auf seiner Zeile/Spalte, eine Auswahl umfasst danach den neuen Text.
    pub(super) fn apply_format(&mut self, sels: Option<&mut Selections>, edits: Vec<crate::editor::TextEdit>, selection: bool, origin: EditOrigin) -> Result<bool, String> {
        let DocEntry { doc, scratch, .. } = self;
        let Some(doc) = doc.as_mut() else { return Err("not loaded".into()) };
        let sels: &mut Selections = match sels {
            Some(s) => s,
            None => scratch,
        };
        doc.sync(sels);
        let head = crate::editor::text::pos_to_linecol(doc.rope(), sels.primary().head);
        let sel_range = edits.first().map(|e| (e.range.start, e.text.chars().count()));
        doc.break_undo_group();
        let changed = doc.apply_edits(sels, edits, origin).map_err(|e| e.to_string())?;
        doc.break_undo_group();
        if changed {
            if selection && let Some((start, n)) = sel_range {
                sels.set_primary(crate::editor::Selection::new(start, start + n));
                sels.collapse_to_primary();
            } else {
                let pos = crate::editor::text::linecol_to_pos(doc.rope(), head);
                *sels = doc.new_selections(pos);
            }
        }
        Ok(changed)
    }
}

impl EditorUi {
    /// Befehl für die aktive Ansicht der Teilung `g` starten (Menü, Kürzel, Palette). `confirmed`: *Minify* von JSONC ist
    /// bestätigt. `wake` weckt die Oberfläche (Fortschritt, Ende).
    pub(crate) fn start_format(&mut self, g: u64, op: Op, confirmed: bool, wake: impl Fn() + Send + Sync + 'static, lang: Lang) {
        let Some((doc_id, mut view_sels)) = self.panes.groups.get(&g).and_then(|grp| grp.active_tab()).map(|t| (t.doc, t.view.sels.clone())) else { return };
        if self.fmt.job_for(doc_id).is_some() {
            self.set_hint(lang.t("Das Formatieren läuft schon.", "Formatting is already running.").to_string());
            return;
        }
        let Some(d) = self.docs.get(&doc_id) else { return };
        let Some(doc) = d.doc.as_ref() else {
            self.set_hint(lang.t("Die Datei wird noch geladen.", "The file is still loading.").to_string());
            return;
        };
        // Eine Auswahl (nur eine) formatiert nur ihren Text; *Prüfen* prüft immer die ganze Datei.
        let range = view_sels.as_mut().filter(|_| op != Op::Validate).and_then(|s| {
            doc.sync(s);
            (!s.is_multi() && !s.primary().is_empty()).then(|| s.primary().range())
        });
        let selection = range.is_some();
        let Some((req, rev)) = d.format_request(op, range, confirmed) else { return };
        let runner = self.fmt.runner.clone().unwrap_or_else(|| Arc::new(format_job::builtin) as Arc<Runner>);
        let job = Job::start_with(req, rev, wake, runner);
        self.fmt.notes.remove(&doc_id);
        self.fmt.jobs.push(UiJob { job, doc: doc_id, group: g, selection });
    }

    /// Lauf eines Dokuments abbrechen (das Ergebnis kommt als „abgebrochen“).
    pub(super) fn cancel_format(&mut self, doc: u64) {
        if let Some(j) = self.fmt.job_for(doc) {
            j.job.cancel();
        }
    }

    /// Fertige Läufe abholen und anwenden (jedes Bild). Meldungen veralten mit der nächsten Änderung.
    pub(super) fn poll_formats(&mut self, lang: Lang) {
        let revs: HashMap<u64, u64> = self.docs.iter().filter_map(|(id, d)| d.doc.as_ref().map(|x| (*id, x.revision()))).collect();
        self.fmt.notes.retain(|id, (_, rev)| revs.get(id) == Some(rev));
        if self.fmt.jobs.is_empty() {
            return;
        }
        let jobs = std::mem::take(&mut self.fmt.jobs);
        for j in jobs {
            if !self.docs.contains_key(&j.doc) {
                j.job.cancel();
                continue;
            }
            match j.job.poll() {
                None => self.fmt.jobs.push(j),
                Some(done) => self.finish_format(j, done, lang),
            }
        }
    }

    fn note(&mut self, doc: u64, n: Note) {
        let rev = self.docs.get(&doc).and_then(|d| d.doc.as_ref()).map(|d| d.revision()).unwrap_or(0);
        self.fmt.notes.insert(doc, (n, rev));
    }

    fn finish_format(&mut self, j: UiJob, done: Done, lang: Lang) {
        let UiJob { job, doc, group, selection } = j;
        match done {
            Done::Changed { kind, edits, whole: _ } => {
                let rev = self.docs.get(&doc).and_then(|d| d.doc.as_ref()).map(|d| d.revision());
                if rev != Some(job.revision) {
                    self.note(doc, Note::Info(lang.t("Das Dokument wurde während des Formatierens geändert – nichts wurde übernommen. Bitte noch einmal.", "The document changed while formatting – nothing was applied. Please try again.").to_string()));
                    return;
                }
                // Cursor der Ansicht, aus der der Lauf kam (sonst irgendeine, die das Dokument zeigt).
                let at = self.panes.groups.get(&group).and_then(|g| g.find(doc)).map(|i| (group, i)).or_else(|| self.panes.tabs_of(doc).first().copied());
                let view = at.and_then(|(g, i)| self.panes.groups.get_mut(&g).and_then(|grp| grp.tabs.get_mut(i))).map(|t| &mut t.view);
                let Some(d) = self.docs.get_mut(&doc) else { return };
                let sels = view.and_then(|v| {
                    v.reveal = true;
                    v.sels.as_mut()
                });
                match d.apply_format(sels, edits, selection, EditOrigin::User) {
                    Ok(true) => {
                        d.on_user_edit();
                        let name = format_job::kind_name(kind);
                        let msg = match (job.op, lang) {
                            (Op::Minify, Lang::De) => format!("{name} minifiziert – Strg+Z macht es rückgängig."),
                            (Op::Minify, Lang::En) => format!("{name} minified – Ctrl+Z undoes it."),
                            (Op::SortKeys, Lang::De) => "Schlüssel sortiert – Strg+Z macht es rückgängig.".to_string(),
                            (Op::SortKeys, Lang::En) => "Keys sorted – Ctrl+Z undoes it.".to_string(),
                            (_, Lang::De) => format!("{name} formatiert – Strg+Z macht es rückgängig."),
                            (_, Lang::En) => format!("{name} formatted – Ctrl+Z undoes it."),
                        };
                        self.set_hint(msg);
                    }
                    Ok(false) => self.set_hint(lang.t("Schon formatiert.", "Already formatted.").to_string()),
                    Err(e) => self.note(doc, Note::Info(format!("{}: {e}", lang.t("Formatieren fehlgeschlagen", "Formatting failed")))),
                }
            }
            Done::Unchanged(_) => self.set_hint(lang.t("Schon so formatiert – nichts geändert.", "Already formatted – nothing changed.").to_string()),
            Done::Valid(k) => self.note(doc, Note::Valid(k)),
            Done::Invalid(k, e) => self.note(doc, Note::Invalid(k, e)),
            Done::NotSupported => self.note(doc, Note::Info(lang.t("Weder JSON noch XML.", "Not JSON or XML.").to_string())),
            Done::Unsupported(_) => self.note(doc, Note::Info(lang.t("Schlüssel sortieren gibt es nur für JSON.", "Sorting keys only works for JSON.").to_string())),
            Done::NeedsConfirm(_) => {
                if !self.fmt.ask.iter().any(|a| a.doc == doc) {
                    self.fmt.ask.push(MinifyAsk { doc, group });
                }
            }
            Done::Cancelled => self.set_hint(lang.t("Formatieren abgebrochen – nichts geändert.", "Formatting cancelled – nothing changed.").to_string()),
        }
    }

    /// Zur Fehlerstelle springen (1-basiert) – in der Teilung, die das Dokument zeigt (bevorzugt die fokussierte).
    pub(super) fn format_goto(&mut self, doc: u64, line: usize, col: usize) {
        let here = self.panes.groups.get(&self.panes.focused).and_then(|g| g.find(doc)).map(|i| (self.panes.focused, i));
        let Some((g, i)) = here.or_else(|| self.panes.tabs_of(doc).first().copied()) else { return };
        self.panes.focused = g;
        if let Some(grp) = self.panes.groups.get_mut(&g) {
            grp.active = i;
        }
        self.goto_line(line.saturating_sub(1), col.saturating_sub(1));
    }

    /// Lauf für einen Agenten (MCP `format_file`) starten: ganzes Dokument, keine Ansicht, nichts in der Oberfläche.
    /// Liefert den Lauf; angewendet wird mit [`EditorUi::apply_agent_format`].
    pub(crate) fn start_agent_format(&mut self, doc: u64, op: Op, confirmed: bool, prefs: &super::EditorPrefs, wake: impl Fn() + Send + Sync + 'static) -> Result<Job, String> {
        let d = self.docs.get_mut(&doc).ok_or_else(|| "no such document".to_string())?;
        d.ensure_loaded_now(prefs);
        if let Some(e) = &d.load_error {
            return Err(e.clone());
        }
        let (req, rev) = d.format_request(op, None, confirmed).ok_or_else(|| "not loaded".to_string())?;
        let runner = self.fmt.runner.clone().unwrap_or_else(|| Arc::new(format_job::builtin) as Arc<Runner>);
        Ok(Job::start_with(req, rev, wake, runner))
    }

    /// Ergebnis eines Agenten-Laufs anwenden (Herkunft Agent: markiert, ungespeichert, ein Rückgängig-Schritt). Ein
    /// inzwischen geändertes Dokument bleibt unverändert (`Err` mit Code `conflict`).
    pub(crate) fn apply_agent_format(&mut self, doc: u64, revision: u64, edits: Vec<crate::editor::TextEdit>) -> Result<(), (&'static str, &'static str)> {
        let d = self.docs.get_mut(&doc).ok_or(("not_found", "gone"))?;
        if d.doc.as_ref().map(|x| x.revision()) != Some(revision) {
            return Err(("conflict", "changed"));
        }
        let chars: Vec<(std::ops::Range<usize>, String)> = edits.into_iter().map(|e| (e.range, e.text)).collect();
        d.apply_agent_edit(chars, Lang::En).map_err(|_| ("internal", "edit"))
    }

    /// Reiter, dessen Cursor beim Formatieren beim Speichern zählen (fokussierte Teilung, sonst irgendeiner).
    fn save_tab(&self, id: u64) -> Option<(u64, usize)> {
        self.panes.groups.get(&self.panes.focused).and_then(|g| g.find(id)).map(|i| (self.panes.focused, i)).or_else(|| self.panes.tabs_of(id).first().copied())
    }

    /// Formatieren beim Speichern (Einstellung je Sprache): synchron, nur bis [`format_job::SAVE_MAX_BYTES`], ab
    /// [`format_job::SAVE_DIFF_MAX_BYTES`] ohne Zeilenvergleich (ein einziger Ersatz) – so bleibt die Zeit im UI-Thread
    /// klein. `path` = Ziel von *Speichern unter* (die Art richtet sich nach ihm). Nichts bei schreibgeschützten
    /// Dateien. Liefert einen Hinweis, wenn nicht formatiert wurde (zu groß, ungültig), ob formatiert wurde und ob der
    /// Formatierer über das ganze Dokument lief ([`SaveFormat::ran`]).
    pub(super) fn format_on_save(&mut self, id: u64, path: Option<&std::path::Path>, prefs: &super::EditorPrefs, lang: Lang) -> SaveFormat {
        let none = SaveFormat::default();
        if !prefs.format_on_save_json && !prefs.format_on_save_xml || self.fmt.job_for(id).is_some() {
            return none;
        }
        let Some(d) = self.docs.get(&id) else { return none };
        let Some(doc) = d.doc.as_ref() else { return none };
        let target = path.map(std::path::Path::to_path_buf).or_else(|| d.path.clone());
        // Schreibgeschützt: Speichern scheitert ohnehin – das Dokument nicht vorher verändern.
        if target.as_ref().and_then(|p| std::fs::metadata(p).ok()).is_some_and(|m| m.permissions().readonly()) {
            return none;
        }
        let Some(kind) = format_job::quick_kind(target.as_deref(), doc.rope(), d.syntax.lang.as_deref()) else { return none };
        let on = match kind {
            Kind::Json | Kind::Jsonc => prefs.format_on_save_json,
            Kind::Xml => prefs.format_on_save_xml,
        };
        if !on {
            return none;
        }
        let name = format_job::kind_name(kind);
        if doc.len_bytes() > format_job::SAVE_MAX_BYTES {
            let h = match lang {
                Lang::De => format!("Nicht formatiert gespeichert: {name} über 1 MB wird beim Speichern nicht formatiert (Format → {name} formatieren)."),
                Lang::En => format!("Saved without formatting: {name} over 1 MB is not formatted on save (Format → Format {name})."),
            };
            return SaveFormat { hint: Some(h), ..none };
        }
        let Some((mut req, _)) = d.format_request(Op::Format, None, false) else { return none };
        req.path = target;
        req.diff_max = format_job::SAVE_DIFF_MAX_BYTES;
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let done = match &self.fmt.runner {
            Some(r) => format_job::compute_with(&req, &cancel, &|_| {}, &**r),
            None => format_job::compute(&req, &cancel, &|_| {}),
        };
        match done {
            Done::Changed { edits, .. } => {
                let at = self.save_tab(id);
                let EditorUi { panes, docs, .. } = self;
                let sels = at.and_then(|(g, i)| panes.groups.get_mut(&g).and_then(|grp| grp.tabs.get_mut(i))).and_then(|t| t.view.sels.as_mut());
                let Some(d) = docs.get_mut(&id) else { return none };
                let formatted = d.apply_format(sels, edits, false, EditOrigin::User).unwrap_or(false);
                SaveFormat { hint: None, formatted, ran: Some(Ok(())) }
            }
            Done::Unchanged(_) => SaveFormat { ran: Some(Ok(())), ..none },
            Done::Invalid(k, e) => SaveFormat {
                hint: Some(match lang {
                    Lang::De => format!("Unverändert gespeichert: ungültiges {} in Zeile {}, Spalte {}.", format_job::kind_name(k), e.line, e.column),
                    Lang::En => format!("Saved unchanged: invalid {} at line {}, column {}.", format_job::kind_name(k), e.line, e.column),
                }),
                formatted: false,
                // Wie die Aktion *Formatieren* einer Regel ([`crate::save_actions::builtin::apply_text`]) es meldet.
                ran: Some(Err(format!("{} {}:{} {}", lang.t("Ungültiger Inhalt, nicht formatiert:", "Invalid content, not formatted:"), e.line, e.column, e.message))),
            },
            _ => none,
        }
    }

    /// Wie [`EditorUi::format_on_save`], nur Hinweis und ob formatiert wurde (Tests).
    #[cfg(test)]
    pub(super) fn format_before_save(&mut self, id: u64, path: Option<&std::path::Path>, prefs: &super::EditorPrefs, lang: Lang) -> (Option<String>, bool) {
        let r = self.format_on_save(id, path, prefs, lang);
        (r.hint, r.formatted)
    }

    /// Das Schreiben eines Dokuments – der einzige Weg dorthin ist [`crate::app::App::editor_save_with`] (alle
    /// Speicherwege des Editors). Vor dem Schreiben, zusammen **ein** Rückgängig-Schritt:
    /// 1. Formatieren beim Speichern (#381, nur mit `format` = ausdrückliches Speichern durch den Nutzer; synchron, bis
    ///    1 MB, nichts bei schreibgeschützten Dateien, die Art nach `path` bei *Speichern unter*),
    /// 2. die eingebauten Aktionen der Regeln beim Speichern (#383, `builtins` in Laufreihenfolge). *Formatieren* einer
    ///    Regel läuft nicht noch einmal, wenn das Formatieren beim Speichern schon über das Dokument lief (es bekommt
    ///    dessen Ergebnis).
    ///
    /// Scheitert das Schreiben (schreibgeschützt, Zeichen passt nicht in die Kodierung, Platte), werden beide
    /// zurückgenommen – Text, Zeilenenden und Kodierung bleiben, wie sie waren. `Ok` mit Hinweis des Formatierens und
    /// dem Ergebnis je eingebauter Aktion.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn save_doc(&mut self, id: u64, path: Option<std::path::PathBuf>, format: bool, builtins: &[crate::save_actions::Builtin], prefs: &super::EditorPrefs, lang: Lang, keep_lines: &[usize]) -> Result<SavedDoc, String> {
        use crate::save_actions::Builtin;
        let Some(d) = self.docs.get(&id) else { return Ok(SavedDoc::default()) };
        let Some(before) = d.doc.as_ref().map(|x| (x.state_id(), x.format_state())) else { return Ok(SavedDoc::default()) };
        let fos = if format { self.format_on_save(id, path.as_deref(), prefs, lang) } else { SaveFormat::default() };
        let mut out = SavedDoc { hint: fos.hint, builtins: Vec::new() };
        if !builtins.is_empty() {
            // *Formatieren* nur einmal (Formatieren beim Speichern und eine Regel).
            let (dup, list): (Vec<Builtin>, Vec<Builtin>) = builtins.iter().cloned().partition(|b| *b == Builtin::Format && fos.ran.is_some());
            let target = path.clone().or_else(|| self.docs.get(&id).and_then(|d| d.path.clone()));
            let Some(d) = self.docs.get_mut(&id) else { return Ok(out) };
            let mut res = d.apply_save_builtins(&list, target.as_deref(), prefs, lang, keep_lines);
            if let Some(r) = fos.ran.as_ref().filter(|_| !dup.is_empty()) {
                res.push((Builtin::Format, r.clone().map(|()| None)));
            }
            // In der Reihenfolge der Regeln melden.
            out.builtins = builtins.iter().filter_map(|b| res.iter().position(|(x, _)| x == b).map(|i| res.swap_remove(i))).collect();
        }
        let Some(d) = self.docs.get_mut(&id) else { return Ok(out) };
        let changed = d.doc.as_ref().is_some_and(|x| x.state_id() != before.0);
        if changed && let Some(x) = d.doc.as_mut() {
            x.join_undo_since(before.0);
        }
        match d.save(path, lang) {
            Ok(()) => Ok(out),
            Err(e) => {
                if changed {
                    self.undo_format(id);
                }
                if let Some(x) = self.docs.get_mut(&id).and_then(|d| d.doc.as_mut()) {
                    x.restore_format_state(before.1);
                }
                Err(e)
            }
        }
    }

    /// Speichern nur mit dem Formatieren beim Speichern (Tests des Formatierens).
    #[cfg(test)]
    pub(super) fn save_with_format(&mut self, id: u64, path: Option<std::path::PathBuf>, prefs: &super::EditorPrefs, lang: Lang) -> Result<Option<String>, String> {
        self.save_doc(id, path, true, &[], prefs, lang, &[]).map(|s| s.hint)
    }

    /// Den letzten Schritt (das Formatieren beim Speichern) zurücknehmen.
    fn undo_format(&mut self, id: u64) {
        let at = self.save_tab(id);
        let EditorUi { panes, docs, .. } = self;
        let view = at.and_then(|(g, i)| panes.groups.get_mut(&g).and_then(|grp| grp.tabs.get_mut(i))).and_then(|t| t.view.sels.as_mut());
        let Some(d) = docs.get_mut(&id) else { return };
        let DocEntry { doc, scratch, .. } = d;
        let Some(doc) = doc.as_mut() else { return };
        let sels: &mut Selections = match view {
            Some(s) => s,
            None => scratch,
        };
        doc.undo(sels);
    }
}

// ------------------------------------------------------------------------------------------------ Zeichnen

/// Höhe der Fortschritts- und Meldungsleisten.
pub(super) const LINE_H: f32 = 30.0;

/// Fortschrittszeile eines laufenden Laufs: Text, Balken, *Abbrechen*.
/// `large`: große Datei – das Übernehmen am Ende hält den Editor kurz an (gemessen: etwa 1,5 s bei 200 MB).
pub(super) fn progress_line(ui: &mut egui::Ui, r: egui::Rect, doc: u64, j: &UiJob, large: bool, lang: Lang, cmds: &mut Vec<Cmd>) {
    let u = style::ui();
    let p = j.job.progress();
    ui.painter().rect_filled(r, 0.0, u.info.gamma_multiply(0.12));
    let bar = egui::Rect::from_min_max(egui::pos2(r.left(), r.bottom() - 3.0), egui::pos2(r.left() + r.width() * p.clamp(0.0, 1.0), r.bottom()));
    ui.painter().rect_filled(bar, 0.0, style::accent());
    let inner = r.shrink2(egui::vec2(10.0, 3.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::right_to_left(egui::Align::Center)));
    let ui = &mut child;
    let cancelling = j.job.cancelled();
    if ui.add_enabled(!cancelling, egui::Button::new(lang.t("Abbrechen", "Cancel")).small()).clicked() {
        cmds.push(Cmd::FormatCancel(doc));
    }
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.spinner();
        let mut text = if cancelling { lang.t("Wird abgebrochen …", "Cancelling …").to_string() } else { progress_text(j.job.op, p, lang) };
        if large && !cancelling && j.job.op != Op::Validate {
            text.push_str(lang.t(" – große Datei: das Übernehmen hält den Editor am Ende kurz an", " – large file: applying the result pauses the editor briefly at the end"));
        }
        ui.add(egui::Label::new(text).truncate());
    });
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
}

/// Meldungsleiste (gültig, ungültig mit *Gehe zu*, Hinweis).
pub(super) fn note_line(ui: &mut egui::Ui, r: egui::Rect, doc: u64, n: &Note, lang: Lang, cmds: &mut Vec<Cmd>) {
    let u = style::ui();
    let (col, text) = match n {
        Note::Invalid(k, e) => (u.error, invalid_text(*k, e, lang)),
        Note::Valid(k) => (
            u.success,
            match lang {
                Lang::De => format!("Gültiges {}.", format_job::kind_name(*k)),
                Lang::En => format!("Valid {}.", format_job::kind_name(*k)),
            },
        ),
        Note::Info(s) => (u.info, s.clone()),
    };
    ui.painter().rect_filled(r, 0.0, col.gamma_multiply(0.16));
    ui.painter().line_segment([r.left_bottom(), r.right_bottom()], egui::Stroke::new(1.0_f32, col.gamma_multiply(0.5)));
    ui.painter().circle_filled(egui::pos2(r.left() + 14.0, r.center().y), 3.5, col);
    let inner = r.shrink2(egui::vec2(10.0, 3.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::right_to_left(egui::Align::Center)));
    let ui = &mut child;
    if ui.small_button(lang.t("Ausblenden", "Dismiss")).clicked() {
        cmds.push(Cmd::FormatDismiss(doc));
    }
    if let Note::Invalid(_, e) = n
        && ui.small_button(lang.t("Gehe zu", "Go to")).on_hover_text(format!("{} {}, {} {}", lang.t("Zeile", "Line"), e.line, lang.t("Spalte", "column"), e.column)).clicked()
    {
        cmds.push(Cmd::FormatGoto(doc, e.line, e.column));
    }
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.add_space(14.0);
        ui.add(egui::Label::new(text.as_str()).truncate()).on_hover_text(text.as_str());
    });
}

/// Rückfrage vor *Minify* von JSONC (Kommentare fallen weg).
pub(super) fn dialogs(ctx: &egui::Context, ed: &EditorUi, lang: Lang, cmds: &mut Vec<Cmd>) {
    let Some(ask) = ed.fmt.ask.first().copied() else { return };
    let name = ed.docs.get(&ask.doc).map(|d| d.title(lang)).unwrap_or_default();
    let mut choice = None;
    let r = style::modal(Id::new("ed_minify_ask")).show(ctx, |ui| {
        ui.set_max_width(440.0);
        ui.heading(lang.t("JSONC minifizieren?", "Minify JSONC?"));
        ui.add_space(style::S2);
        ui.label(format!("{} {}", lang.q(&name), lang.t("enthält Kommentare (JSONC). Minifizieren entfernt sie; Strg+Z holt sie zurück.", "contains comments (JSONC). Minifying removes them; Ctrl+Z brings them back.")));
        ui.add_space(style::S3);
        ui.horizontal(|ui| {
            if ui.button(lang.t("Minifizieren", "Minify")).clicked() {
                choice = Some(true);
            }
            if ui.button(lang.t("Abbrechen", "Cancel")).clicked() {
                choice = Some(false);
            }
        });
    });
    if r.should_close() && choice.is_none() {
        choice = Some(false);
    }
    if let Some(c) = choice {
        cmds.push(Cmd::MinifyConfirm(c));
    }
}
