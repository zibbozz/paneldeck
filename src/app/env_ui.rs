//! Reiter „Umgebung“ (#384): Tabelle der Umgebungsvariablen einer Sitzung im Dialog „Neue Sitzung“, in den
//! Einstellungen der Sitzung und im Vorlagen-Editor; dazu Einfügen von `KEY=VALUE`-Zeilen, Import einer `.env`-Datei mit
//! Vorschau, die Rückfrage *Neu starten / Jetzt setzen / Später* nach dem Speichern und das Kennzeichen in der Kopfzeile.
//!
//! Die Daten und Regeln stehen in `crate::session_env` (kein egui); hier nur Oberfläche und Anbindung an die App.

use super::*;
use crate::session_env::{self as senv, EnvVar, LiveIssue, Mode};

/// Eine Zeile der Tabelle. Bei geheimen Werten steht der gespeicherte Wert nur verschlüsselt in `stored`; `value` ist
/// dann ein neu eingegebener Wert (leer = den gespeicherten behalten).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct EnvRow {
    pub(super) name: String,
    pub(super) value: String,
    pub(super) stored: Option<String>,
    pub(super) secret: bool,
    /// „Geheim“ von Hand umgeschaltet (sonst folgt es dem Namen, `senv::looks_secret`).
    pub(super) secret_touched: bool,
    pub(super) mode: Mode,
    pub(super) remove: bool,
    pub(super) wsl_path: bool,
    pub(super) by_agent: bool,
}

impl EnvRow {
    fn of(e: &EnvVar) -> EnvRow {
        EnvRow {
            name: e.name.clone(),
            value: if e.secret { String::new() } else { e.value.clone() },
            stored: e.value_protected.clone().filter(|_| e.secret),
            secret: e.secret,
            secret_touched: true,
            mode: e.mode,
            remove: e.remove,
            wsl_path: e.wsl_path,
            by_agent: e.by_agent,
        }
    }

    fn blank(&self) -> bool {
        self.name.trim().is_empty() && self.value.is_empty() && self.stored.is_none()
    }
}

/// Vorschau eines `.env`-Imports: je Zeile übernehmen ja/nein.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct ImportPreview {
    pub(super) file: String,
    pub(super) lines: Vec<(senv::ParsedLine, bool)>,
}

/// Zustand des Reiters.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct EnvEditor {
    pub(super) rows: Vec<EnvRow>,
    /// Offenes Feld „KEY=VALUE einfügen“.
    pub(super) paste: Option<String>,
    pub(super) import: Option<ImportPreview>,
    /// Hinweis nach Einfügen/Import (übersprungene Zeilen) bzw. Fehler beim Speichern.
    pub(super) note: Option<String>,
    /// Vorlage aus einer Sitzung (#384): `Some(false)` = die geheimen Werte der Sitzung werden nicht übernommen, bis der
    /// Nutzer zustimmt (`Some(true)`); `None` = keine Rückfrage.
    pub(super) keep_secrets: Option<bool>,
}

/// Was der Reiter über seine Umgebung wissen muss.
#[derive(Clone, Copy)]
pub(super) struct EnvUiOpts<'a> {
    pub(super) lang: Lang,
    pub(super) salt: &'a str,
    /// Die Sitzung startet in WSL (Spalte „Pfad umrechnen“).
    pub(super) wsl: bool,
    /// Claude-Sitzung (Hinweis: Claude und seine Werkzeuge sehen alle Werte).
    pub(super) claude: bool,
    /// Variablen des gewählten Profils (Name, Profilname) – nur zur Anzeige.
    pub(super) profile: Option<(&'a str, &'a str)>,
}

impl EnvEditor {
    pub(super) fn of(vars: &[EnvVar]) -> EnvEditor {
        EnvEditor { rows: vars.iter().map(EnvRow::of).collect(), ..EnvEditor::default() }
    }

    /// Zeilen mit Namen.
    pub(super) fn count(&self) -> usize {
        self.rows.iter().filter(|r| !r.name.trim().is_empty()).count()
    }

    fn full(&self) -> bool {
        self.rows.iter().filter(|r| !r.blank()).count() >= senv::MAX_VARS
    }

    pub(super) fn add_row(&mut self) {
        if !self.full() {
            self.rows.push(EnvRow::default());
        }
    }

    /// Zeilen übernehmen (Einfügen, Import): ein vorhandener Name bekommt den neuen Wert, sonst eine neue Zeile.
    /// Rückgabe: (übernommen, übersprungen).
    pub(super) fn add_lines(&mut self, lines: &[senv::ParsedLine]) -> (usize, Vec<String>) {
        let (mut n, mut skipped) = (0, Vec::new());
        for l in lines {
            if l.problem.is_some() {
                skipped.push(l.name.clone());
                continue;
            }
            let full = self.full();
            match self.rows.iter_mut().find(|r| senv::same_name(r.name.trim(), &l.name)) {
                Some(r) => {
                    r.value = l.value.clone();
                    r.secret |= l.secret;
                    r.remove = false;
                    r.mode = l.mode;
                }
                None if full => {
                    skipped.push(l.name.clone());
                    continue;
                }
                None => {
                    self.rows.retain(|r| !r.blank());
                    self.rows.push(EnvRow { name: l.name.clone(), value: l.value.clone(), secret: l.secret, mode: l.mode, ..EnvRow::default() });
                }
            }
            n += 1;
        }
        (n, skipped)
    }

    /// Eingefügten Text übernehmen (`KEY=VALUE` je Zeile).
    pub(super) fn paste_text(&mut self, text: &str, lang: Lang) {
        let (n, skipped) = self.add_lines(&senv::parse_lines(text));
        self.note = Some(import_note(n, &skipped, lang));
    }

    /// Vorschau für den Inhalt einer `.env`-Datei: jede brauchbare Zeile ist angehakt.
    pub(super) fn start_import(&mut self, file: &str, text: &str) {
        let lines = senv::parse_lines(text).into_iter().map(|l| {
            let ok = l.problem.is_none();
            (l, ok)
        });
        self.import = Some(ImportPreview { file: file.to_string(), lines: lines.collect() });
    }

    /// Angehakte Zeilen der Vorschau übernehmen.
    pub(super) fn finish_import(&mut self, lang: Lang) {
        let Some(p) = self.import.take() else { return };
        let chosen: Vec<senv::ParsedLine> = p.lines.into_iter().filter(|(_, on)| *on).map(|(l, _)| l).collect();
        let (n, skipped) = self.add_lines(&chosen);
        self.note = Some(import_note(n, &skipped, lang));
    }

    /// Fehler und Hinweise je Zeile (Index, Text); ohne zu verschlüsseln.
    pub(super) fn problems(&self, lang: Lang) -> (Vec<(usize, String)>, Vec<(usize, String)>) {
        let (mut err, mut warn) = (Vec::new(), Vec::new());
        for (i, r) in self.rows.iter().enumerate() {
            if r.blank() {
                continue;
            }
            let name = r.name.trim();
            if let Err(e) = senv::check_name(name) {
                err.push((i, e.text(lang).to_string()));
                continue;
            }
            if self.rows[..i].iter().any(|o| senv::same_name(o.name.trim(), name)) {
                err.push((i, format!("{} {}", lang.t("Doppelter Name:", "Duplicate name:"), name)));
            }
            if r.value.chars().any(|c| c.is_control()) {
                err.push((i, format!("{name}: {}", lang.t("Der Wert enthält Steuerzeichen (z. B. Zeilenumbrüche).", "The value contains control characters (e.g. line breaks)."))));
            }
            if r.value.len() > senv::MAX_VALUE {
                err.push((i, format!("{name}: {}", lang.t("Der Wert ist zu lang (höchstens 32 KB).", "The value is too long (at most 32 KB)."))));
            }
            if let Some(w) = senv::warn_name(name) {
                warn.push((i, format!("{name}: {}", w.text(lang))));
            }
            if r.secret && !r.remove && r.value.is_empty() && r.stored.is_none() {
                warn.push((i, format!("{name}: {}", lang.t("Kein Wert – die Variable fehlt beim Start.", "No value – the variable is left out at start."))));
            }
        }
        if self.rows.iter().filter(|r| !r.blank()).count() > senv::MAX_VARS {
            err.push((0, format!("{} {}", lang.t("Höchstens so viele Variablen:", "At most this many variables:"), senv::MAX_VARS)));
        }
        (err, warn)
    }

    /// Beschriftung des Reiters: Zahl der Variablen, rot bei Fehlern, orange bei Hinweisen.
    pub(super) fn tab_info(&self, lang: Lang) -> super::opts_tabs::TabInfo {
        use super::opts_tabs::{Mark, Tab, TabInfo};
        let (err, warn) = self.problems(lang);
        TabInfo { tab: Tab::Environment, count: self.count(), mark: if !err.is_empty() { Mark::Error } else if !warn.is_empty() { Mark::Warn } else { Mark::None } }
    }

    pub(super) fn ok(&self, lang: Lang) -> bool {
        self.problems(lang).0.is_empty()
    }

    /// Liste zum Speichern: neue geheime Werte werden verschlüsselt (Fehler, wenn das hier nicht geht – z. B. ohne
    /// Master-Passwort), ein von „geheim“ auf „normal“ gestellter gespeicherter Wert wird entschlüsselt. Leere Zeilen
    /// fallen weg.
    pub(super) fn to_vars(&self, lang: Lang) -> Result<Vec<EnvVar>, String> {
        let (err, _) = self.problems(lang);
        if let Some((_, e)) = err.first() {
            return Err(e.clone());
        }
        let mut out = Vec::new();
        for r in self.rows.iter().filter(|r| !r.blank()) {
            let mut e = EnvVar { name: r.name.trim().to_string(), mode: r.mode, remove: r.remove, wsl_path: r.wsl_path, by_agent: r.by_agent, secret: r.secret, ..EnvVar::default() };
            if !r.remove {
                if r.secret {
                    e.value_protected = if !r.value.is_empty() {
                        Some(crate::llm::secret::protect(&r.value).map_err(|x| format!("{}: {}", e.name, x.text(lang)))?)
                    } else if self.keep_secrets == Some(false) {
                        None
                    } else {
                        r.stored.clone()
                    };
                } else if r.value.is_empty() && r.stored.is_some() {
                    // Von „geheim“ auf „normal“: der gespeicherte Wert wird lesbar (wie gewünscht) – geht das nicht, leer.
                    e.value = r.stored.as_deref().and_then(|b| crate::llm::secret::unprotect(b).ok()).unwrap_or_default();
                } else {
                    e.value = r.value.clone();
                }
            }
            out.push(e);
        }
        Ok(senv::clean(out))
    }
}

fn import_note(n: usize, skipped: &[String], lang: Lang) -> String {
    let mut s = if lang == Lang::De { format!("{n} Variable(n) übernommen.") } else { format!("{n} variable(s) added.") };
    if !skipped.is_empty() {
        s += &format!(" {} {}", lang.t("Übersprungen (Name ungültig, geschützt oder zu viele):", "Skipped (invalid, protected or too many):"), skipped.join(", "));
    }
    s
}

/// Rechteck eines Elements für die Tests ohne Fenster.
fn mark(ui: &Ui, key: &'static str, n: usize, r: &egui::Response) {
    #[cfg(test)]
    ui.data_mut(|d| d.insert_temp(Id::new(("env_mark", key, n)), r.rect));
    let _ = (ui, key, n, r);
}

const NAME_W: f32 = 140.0;
const VALUE_W: f32 = 170.0;

/// Inhalt des Reiters „Umgebung“.
pub(super) fn env_tab_ui(ui: &mut Ui, ed: &mut EnvEditor, o: EnvUiOpts) {
    let lang = o.lang;
    let dim = style::dim(ui);
    ui.label(RichText::new(lang.t(
        "Umgebungsvariablen nur für diese Sitzung. Sie gelten nach den Variablen des Profils (gleicher Name: diese gewinnt). Werte sind wörtlich – kein %VAR% oder $VAR.",
        "Environment variables for this session only. They apply after the profile's variables (same name: this one wins). Values are literal – no %VAR% or $VAR.",
    )).small().color(dim));
    if let Some((names, prof)) = o.profile.filter(|(n, _)| !n.is_empty()) {
        ui.label(RichText::new(format!("{} {}: {names}", lang.t("Aus dem Profil", "From the profile"), lang.q(prof))).small().color(dim));
    }
    if o.claude {
        ui.label(RichText::new(lang.t(
            "Claude-Sitzung: Claude Code und alle Werkzeuge, die es startet, sehen sämtliche Werte (auch geheime).",
            "Claude session: Claude Code and every tool it starts see all values (secret ones too).",
        )).small().color(crate::style::ui().warning));
    }
    if let Some(keep) = ed.keep_secrets.as_mut() {
        let r = ui.checkbox(keep, lang.t("Geheime Werte der Sitzung in die Vorlage übernehmen (verschlüsselt, nur für dich auf diesem Rechner lesbar)", "Include the session's secret values in the template (encrypted, readable only for you on this computer)"));
        mark(ui, "keep_secrets", 0, &r);
    }
    ui.add_space(4.0);
    if let Some(p) = ed.import.as_mut() {
        import_ui(ui, p, lang);
        let n = p.lines.iter().filter(|(_, on)| *on).count();
        let (mut ok, mut cancel) = (false, false);
        ui.horizontal(|ui| {
            let b = ui.add_enabled(n > 0, egui::Button::new(if lang == Lang::De { format!("{n} übernehmen") } else { format!("Import {n}") }));
            mark(ui, "import_ok", 0, &b);
            ok = b.clicked();
            cancel = ui.button(lang.t("Abbrechen", "Cancel")).clicked();
        });
        if ok {
            ed.finish_import(lang);
        } else if cancel {
            ed.import = None;
        }
        return;
    }
    let (err, warn) = ed.problems(lang);
    let mut delete: Option<usize> = None;
    egui::Grid::new((o.salt, "env_grid")).num_columns(if o.wsl { 7 } else { 6 }).spacing([6.0, 4.0]).show(ui, |ui| {
        ui.label(RichText::new(lang.t("Name", "Name")).small().strong());
        ui.label(RichText::new(lang.t("Wert", "Value")).small().strong());
        ui.label(RichText::new(lang.t("Geheim", "Secret")).small().strong()).on_hover_text(lang.t("Verschlüsselt gespeichert, maskiert, nie getippt, exportiert oder an Agenten gegeben.", "Stored encrypted, masked, never typed, exported or given to agents."));
        ui.label(RichText::new(lang.t("Modus", "Mode")).small().strong()).on_hover_text(lang.t("Ersetzen, vor den geerbten Wert setzen oder anhängen (Trennzeichen ; bzw. :) – für PATH und andere Listen.", "Replace, prepend to or append to the inherited value (separator ; or :) – for PATH and other lists."));
        ui.label(RichText::new(lang.t("Entfernen", "Remove")).small().strong()).on_hover_text(lang.t("Entfernt eine geerbte Variable (statt sie zu setzen).", "Removes an inherited variable (instead of setting it)."));
        if o.wsl {
            ui.label(RichText::new(lang.t("Pfad", "Path")).small().strong()).on_hover_text(lang.t("WSL: Wert als Windows-Pfad nach Linux umrechnen (WSLENV /p, Listen /l).", "WSL: convert the value from a Windows path to Linux (WSLENV /p, lists /l)."));
        }
        ui.label("");
        ui.end_row();
        for (i, r) in ed.rows.iter_mut().enumerate() {
            let bad = err.iter().any(|(k, _)| *k == i);
            let hint = warn.iter().chain(err.iter()).filter(|(k, _)| *k == i).map(|(_, t)| t.as_str()).collect::<Vec<_>>().join("\n");
            ui.horizontal(|ui| {
                let mut te = egui::TextEdit::singleline(&mut r.name).desired_width(NAME_W - 18.0).min_size(egui::vec2(NAME_W - 18.0, 0.0)).hint_text("NAME").font(egui::TextStyle::Monospace);
                // `min_size`: Spalten ohne gemerkte Breite gibt das Raster nur `interact_size.x` (40 px); das Feld passte
                // sich daran an und die Spalte blieb für immer so schmal (erstes Öffnen des Reiters „Umgebung“).
                if bad {
                    te = te.text_color(crate::style::ui().error);
                }
                let resp = ui.add(te);
                mark(ui, "name", i, &resp);
                if resp.changed() && !r.secret_touched {
                    r.secret = senv::looks_secret(&r.name);
                }
                if !hint.is_empty() {
                    let c = if bad { crate::style::ui().error } else { crate::style::ui().warning };
                    ui.label(RichText::new("⚠").color(c)).on_hover_text(hint);
                }
            });
            let hint_v = if r.secret && r.stored.is_some() { lang.t("•••• (gespeichert)", "•••• (stored)") } else if r.remove { "" } else { lang.t("Wert", "value") };
            let resp = ui.add_enabled(!r.remove, egui::TextEdit::singleline(&mut r.value).desired_width(VALUE_W).min_size(egui::vec2(VALUE_W, 0.0)).password(r.secret).hint_text(hint_v).font(egui::TextStyle::Monospace));
            mark(ui, "value", i, &resp);
            let sc = ui.checkbox(&mut r.secret, "");
            if sc.changed() {
                r.secret_touched = true;
            }
            mark(ui, "secret", i, &sc);
            ui.add_enabled_ui(!r.remove, |ui| {
                egui::ComboBox::from_id_salt((o.salt, "env_mode", i)).width(86.0).selected_text(r.mode.label(lang)).show_ui(ui, |ui| {
                    for m in Mode::ALL {
                        ui.selectable_value(&mut r.mode, m, m.label(lang));
                    }
                });
            });
            let rm = ui.checkbox(&mut r.remove, "");
            mark(ui, "remove", i, &rm);
            if o.wsl {
                // Umrechnen gibt es nur für gesetzte Werte (WSLENV); Voranstellen/Anhängen/Entfernen wirken auf Linux.
                let can = !r.remove && r.mode == Mode::Replace;
                ui.add_enabled(can, egui::Checkbox::without_text(&mut r.wsl_path)).on_disabled_hover_text(lang.t("Nur bei „Ersetzen“: Voranstellen, Anhängen und Entfernen wirken in WSL auf die Linux-Werte.", "Only with “Replace”: in WSL, prepend, append and remove act on the Linux values."));
            }
            let d = style::icon_btn_small(ui, SIcon::Close, lang.t("Zeile löschen", "Delete row"));
            mark(ui, "delete", i, &d);
            if d.clicked() {
                delete = Some(i);
            }
            ui.end_row();
        }
    });
    if let Some(i) = delete {
        ed.rows.remove(i);
    }
    if ed.rows.is_empty() {
        ui.label(RichText::new(lang.t("Noch keine Variablen.", "No variables yet.")).small().color(dim));
    }
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        let full = ed.full();
        let add = ui.add_enabled(!full, egui::Button::new(lang.t("+ Variable", "+ Variable")));
        mark(ui, "add", 0, &add);
        if add.clicked() {
            ed.add_row();
        }
        let p = ui.add_enabled(ed.paste.is_none(), egui::Button::new(lang.t("KEY=VALUE einfügen …", "Paste KEY=VALUE …")));
        mark(ui, "paste", 0, &p);
        if p.clicked() {
            ed.paste = Some(String::new());
        }
        let imp = ui.button(lang.t("Aus .env importieren …", "Import from .env …")).on_hover_text(lang.t("Einmalig übernehmen (keine Verknüpfung): Vorschau mit Haken je Zeile; verdächtige Namen werden geheim.", "Taken over once (not linked): preview with a checkbox per line; suspicious names become secret."));
        mark(ui, "import", 0, &imp);
        if imp.clicked() {
            if let Some(path) = notify::pick_file("") {
                match read_small(&path) {
                    Ok(text) => ed.start_import(&path.display().to_string(), &text),
                    Err(e) => ed.note = Some(format!("{} {e}", lang.t("Datei nicht lesbar:", "File not readable:"))),
                }
            }
        }
        ui.label(RichText::new(format!("{}/{}", ed.count(), senv::MAX_VARS)).small().color(dim));
    });
    if let Some(text) = ed.paste.as_mut() {
        let r = ui.add(egui::TextEdit::multiline(text).desired_rows(3).desired_width(f32::INFINITY).font(egui::TextStyle::Monospace).hint_text("API_URL=http://localhost:3000\nNODE_ENV=development"));
        super::opts_tabs::multiline(ui, o.salt, &r);
        mark(ui, "paste_text", 0, &r);
        let (mut add, mut cancel) = (false, false);
        ui.horizontal(|ui| {
            let b = ui.button(lang.t("Übernehmen", "Add"));
            mark(ui, "paste_ok", 0, &b);
            add = b.clicked();
            cancel = ui.button(lang.t("Abbrechen", "Cancel")).clicked();
        });
        if add {
            let t = std::mem::take(text);
            ed.paste = None;
            ed.paste_text(&t, lang);
        } else if cancel {
            ed.paste = None;
        }
    }
    if let Some(n) = &ed.note {
        ui.label(RichText::new(n).small().color(dim));
    }
    for (_, e) in &err {
        ui.label(RichText::new(e).small().color(crate::style::ui().error));
    }
}

/// `.env`-Datei lesen (höchstens 256 KB).
fn read_small(path: &std::path::Path) -> Result<String, String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > 256 * 1024 {
        return Err("> 256 KB".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    Ok(state::strip_bom(&String::from_utf8_lossy(&bytes)).to_string())
}

/// Vorschau des Imports: je Zeile Haken, Name, Wert (geheime maskiert) und „geheim“.
fn import_ui(ui: &mut Ui, p: &mut ImportPreview, lang: Lang) {
    ui.label(RichText::new(format!("{} {}", lang.t("Import aus", "Import from"), p.file)).strong());
    ui.label(RichText::new(lang.t("Angehakte Zeilen werden einmalig übernommen; vorhandene Namen bekommen den neuen Wert.", "Ticked lines are taken over once; existing names get the new value.")).small().color(style::dim(ui)));
    egui::Grid::new("env_import_grid").num_columns(4).spacing([6.0, 3.0]).show(ui, |ui| {
        for (i, (l, on)) in p.lines.iter_mut().enumerate() {
            let ok = l.problem.is_none();
            let c = ui.add_enabled(ok, egui::Checkbox::without_text(on));
            mark(ui, "import_line", i, &c);
            let name = RichText::new(&l.name).monospace();
            match l.problem {
                Some(e) => ui.label(name.color(crate::style::ui().error)).on_hover_text(e.text(lang)),
                None => ui.label(name),
            };
            let shown = if l.secret { "••••".to_string() } else if l.value.chars().count() > 40 { format!("{}…", l.value.chars().take(40).collect::<String>()) } else { l.value.clone() };
            ui.label(RichText::new(shown).monospace().small());
            ui.add_enabled(ok, egui::Checkbox::new(&mut l.secret, lang.t("geheim", "secret")));
            ui.end_row();
        }
    });
    if p.lines.is_empty() {
        ui.label(RichText::new(lang.t("Keine KEY=VALUE-Zeilen gefunden.", "No KEY=VALUE lines found.")).small().color(style::dim(ui)));
    }
}

/// Text einer Sitzung mit gesperrten geheimen Variablen (#384).
pub(super) fn env_locked_text(l: &senv::Locked, lang: Lang) -> String {
    if l.needs_reentry() {
        format!(
            "{} {}",
            lang.t("Geheime Variablen lassen sich nicht entschlüsseln (anderer Benutzer oder Rechner?) – bitte die Werte neu eingeben:", "Secret variables cannot be decrypted (different user or computer?) – please enter the values again:"),
            l.names.join(", ")
        )
    } else {
        format!("{} ({})", lang.t("Geheime Variablen gesperrt – entsperren und starten", "Secret variables locked – unlock and start"), l.names.join(", "))
    }
}

/// Rückfrage nach dem Speichern geänderter Variablen einer laufenden Sitzung: was sich jetzt setzen ließe.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct EnvLive {
    /// Befehl für „Jetzt setzen“ (`None` = nicht möglich, Grund in `why`).
    pub(super) command: Option<String>,
    pub(super) why: Option<String>,
    /// Änderungen, die erst nach einem Neustart gelten.
    pub(super) later: Vec<(String, LiveIssue)>,
    /// Schritte, die der Befehl setzt (danach Stand der Shell, `Entry::env_live`).
    pub(super) applied: Vec<senv::Op>,
    /// Hinweise (#392): „Pfad umrechnen“ ging nicht – der Wert wird unverändert gesetzt.
    pub(super) notes: Vec<String>,
}

/// „Pfad umrechnen“ (WSL, `wsl_path`) für das Setzen im laufenden Terminal (#392): ein Pfad bzw. eine Liste mit `;`
/// (wird eine Liste mit `:`) über `targetpath` in die Pfadart der Sitzung. `Err`: ein Eintrag ließ sich nicht umrechnen
/// (andere Distribution, unbekannte Distribution …) – dann bleibt der ganze Wert, wie er ist.
pub(super) fn live_path_value(kind: &crate::targetpath::PathKind, v: &str, lang: Lang) -> Result<String, String> {
    let parts: Vec<&str> = if v.contains(';') { v.split(';').filter(|p| !p.trim().is_empty()).collect() } else { vec![v] };
    let mut out = Vec::with_capacity(parts.len());
    for p in parts {
        out.push(kind.to_target(p).map_err(|e| e.message(lang))?);
    }
    Ok(out.join(":"))
}

/// Kann jetzt gesetzt werden? Bekannte Shell (Shell-Integration), wartet am Prompt (nichts getippt, nichts läuft, kein
/// Vollbild). Sonst der Grund.
pub(super) fn live_ready(e: &Entry, lang: Lang) -> Result<(senv::LiveShell, bool), String> {
    let Some(s) = e.live.as_ref().filter(|s| !s.has_exited()) else { return Err(lang.t("Die Sitzung läuft nicht.", "The session is not running.").into()) };
    let Some(flavor) = e.shell_flavor else {
        return Err(lang.t("Shell ohne Shell-Integration – die Werte gelten nach einem Neustart.", "Shell without shell integration – the values apply after a restart.").into());
    };
    let alt = s.alt_screen();
    let st = s.shell_state.lock().unwrap_or_else(|x| x.into_inner());
    if !st.integration_seen || alt || st.enters_since_prompt > 0 || st.nested(alt) || e.is_claude() {
        return Err(lang.t("Die Shell wartet gerade nicht an ihrem Prompt (es läuft ein Programm).", "The shell is not waiting at its prompt right now (a program is running).").into());
    }
    Ok((senv::LiveShell::of(flavor), e.eff_shell().starts_with("wsl")))
}

/// Plan für „Jetzt setzen“ aus altem und neuem Stand.
/// `old` = Stand der laufenden Shell (`Entry::env_live`: beim Start festgehalten, dann um „Jetzt setzen“ ergänzt).
pub(super) fn live_plan(e: &Entry, old: &[EnvVar], new: &[EnvVar], lang: Lang) -> EnvLive {
    let (ops, mut later) = senv::live_changes(old, new);
    match live_ready(e, lang) {
        Err(why) => EnvLive { command: None, why: Some(why), later, applied: Vec::new(), notes: Vec::new() },
        Ok((shell, wsl)) => {
            let mut applied = Vec::new();
            let mut notes = Vec::new();
            // WSL: „Pfad umrechnen“ gilt auch beim Setzen im laufenden Terminal (beim Start rechnet WSLENV `/p` um).
            let kind = e.live.as_ref().filter(|_| wsl).map(|s| crate::targetpath::PathKind::of(&s.spec));
            for mut o in ops {
                if let (Some(k), true, senv::OpKind::Set(v)) = (&kind, o.wsl_path, o.kind.clone()) {
                    match live_path_value(k, &v, lang) {
                        Ok(c) => o.kind = senv::OpKind::Set(c),
                        Err(err) => notes.push(format!("{}: {} – {err}", o.name, lang.t("Pfad nicht umgerechnet, Wert unverändert", "path not converted, value unchanged"))),
                    }
                }
                match senv::live_issue(shell, wsl, &o) {
                    Some(i) => later.push((o.name.clone(), i)),
                    None => applied.push(o),
                }
            }
            let command = senv::live_command(shell, wsl, &applied);
            let why = command.is_none().then(|| lang.t("Nichts davon lässt sich im laufenden Terminal setzen.", "None of it can be set in the running terminal.").to_string());
            EnvLive { command, why, later, applied, notes }
        }
    }
}

/// Tasten für „Jetzt setzen“: erst die Eingabezeile leeren (`senv::clear_line_key`, kodiert wie die Tastatur), danach
/// kommt der Befehl als Text.
pub(super) fn clear_line_bytes(shell: senv::LiveShell, mode: crate::vtkeys::KeyMode) -> Vec<u8> {
    crate::vtkeys::key_bytes(senv::clear_line_key(shell), mode).unwrap_or_default()
}

/// Ergebnis der Rückfrage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LiveOut {
    Open,
    Restart,
    SetNow,
    Later,
}

/// Fenster *Neu starten / Jetzt setzen / Später* (ohne App, headless testbar).
pub(super) fn env_ask_ui(ctx: &egui::Context, name: &str, live: &EnvLive, lang: Lang) -> LiveOut {
    let mut out = LiveOut::Open;
    let esc = ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape));
    style::movable(ctx, Id::new("env_apply_ask"), egui::Window::new(lang.t("Umgebungsvariablen übernehmen?", "Apply the environment variables?"))).collapsible(false).resizable(false).show(ctx, |ui| {
        ui.set_max_width(480.0);
        ui.label(RichText::new(name).strong());
        ui.add_space(S1);
        ui.label(lang.t(
            "Die Shell dieser Sitzung läuft schon. Neue Werte gelten ab einem Neustart – normale Werte lassen sich auch jetzt im Terminal setzen (als sichtbarer Befehl).",
            "This session's shell is already running. New values apply from a restart – normal values can also be set in the terminal now (as a visible command).",
        ));
        if let Some(c) = &live.command {
            ui.add_space(S1);
            ui.add(egui::Label::new(RichText::new(c.trim_start()).monospace().small()).wrap());
        }
        if !live.later.is_empty() {
            ui.add_space(S1);
            let names: Vec<String> = live.later.iter().map(|(n, i)| format!("{n} ({})", i.text(lang))).collect();
            ui.label(RichText::new(format!("{} {}", lang.t("Nur nach einem Neustart:", "Only after a restart:"), names.join(", "))).small().color(style::dim(ui)));
        }
        for n in &live.notes {
            ui.label(RichText::new(format!("⚠ {n}")).small().color(style::ui().warning));
        }
        ui.add_space(S2);
        ui.horizontal(|ui| {
            let r = ui.button(lang.t("Neu starten", "Restart"));
            mark(ui, "ask_restart", 0, &r);
            if r.clicked() {
                out = LiveOut::Restart;
            }
            let mut s = ui.add_enabled(live.command.is_some(), egui::Button::new(lang.t("Jetzt setzen", "Set now")));
            if let Some(w) = &live.why {
                s = s.on_disabled_hover_text(w.as_str());
            }
            mark(ui, "ask_set", 0, &s);
            if s.clicked() {
                out = LiveOut::SetNow;
            }
            let l = ui.button(lang.t("Später", "Later"));
            mark(ui, "ask_later", 0, &l);
            l.request_focus();
            if l.clicked() {
                out = LiveOut::Later;
            }
        });
        if let (None, Some(w)) = (&live.command, &live.why) {
            ui.label(RichText::new(w).small().color(style::dim(ui)));
        }
    });
    if esc && out == LiveOut::Open {
        out = LiveOut::Later;
    }
    out
}

/// Kennzeichen in der Kopfzeile: `$` mit der Zahl der Variablen, Tooltip mit den Namen (normale mit Wert, geheime als
/// ••••), oranger Punkt, solange eine Änderung erst nach einem Neustart gilt. Gibt die Breite zurück (0 = nichts).
pub(super) fn header_badge(ui: &mut Ui, id: u64, vars: &[EnvVar], pending: bool, right: Pos2, lang: Lang) -> f32 {
    if vars.is_empty() && !pending {
        return 0.0;
    }
    let weak = ui.visuals().weak_text_color();
    let text = format!("$ {}", vars.len());
    let g = ui.painter().layout_no_wrap(text, FontId::monospace(11.0), weak);
    let w = g.size().x + if pending { 10.0 } else { 4.0 };
    let rect = Rect::from_min_size(Pos2::new(right.x - w, right.y - 9.0), Vec2::new(w, 18.0));
    ui.painter().galley(Pos2::new(rect.min.x + 2.0, right.y - g.size().y / 2.0), g, weak);
    if pending {
        ui.painter().circle_filled(Pos2::new(rect.max.x - 3.0, rect.min.y + 4.0), 3.0, Color32::from_rgb(0xe5, 0xa0, 0x4b));
    }
    let mut tip = senv::tooltip(vars, lang);
    if pending {
        tip = format!("{}\n\n{tip}", lang.t("● Eine Änderung gilt erst nach einem Neustart.", "● A change applies only after a restart."));
    }
    let tip = format!("{}\n{tip}", lang.t("Umgebungsvariablen der Sitzung", "Session environment variables"));
    ui.interact(rect, Id::new(("env_badge", id)), Sense::hover()).on_hover_text(tip);
    #[cfg(test)]
    ui.data_mut(|d| d.insert_temp(Id::new(("env_badge_rect", id)), rect));
    w
}

impl App {
    /// Zeilen `NAME=VALUE` aus Profil und Sitzung (nur normale gesetzte Werte) – für die Prüfung, ob Claude eigene Hook-
    /// Einstellungen braucht (`CLAUDE_CONFIG_DIR`, `hooks::session_needs_settings`).
    pub(super) fn env_lines_of(&self, meta: &SessionMeta) -> String {
        let profile = self.profile(meta.profile).map(|p| senv::from_profile(&p.env)).unwrap_or_default();
        senv::merge(&profile, &meta.env).iter().filter(|e| !e.secret && !e.remove && e.mode == Mode::Replace).map(|e| format!("{}={}", e.name, e.value)).collect::<Vec<_>>().join("\n")
    }

    /// Namen der Profilvariablen (Anzeige im Reiter).
    pub(super) fn profile_env_names(&self, profile: u64) -> Option<(String, String)> {
        let p = self.profile(profile)?;
        let names: Vec<String> = senv::from_profile(&p.env).into_iter().map(|e| e.name).collect();
        (!names.is_empty()).then(|| (names.join(", "), p.name.clone()))
    }

    /// Variablen einer Sitzung geändert (Einstellungen, Einfügen): läuft sie, fragen *Neu starten / Jetzt setzen /
    /// Später*; sonst gilt die Änderung beim nächsten Start (oranger Punkt).
    pub(super) fn env_changed(&mut self, id: u64, old: Vec<EnvVar>) {
        let lang = self.settings.language;
        let Some(e) = self.entries.get_mut(&id) else { return };
        if e.meta.env == old {
            return;
        }
        if e.env_locked.is_some() && e.live.is_none() {
            // Gesperrt (#384 Review): Werte neu eingegeben – Sperre aufheben und gleich starten.
            e.env_locked = None;
            e.error = None;
            self.pending_actions.push(Action::Restart(id));
            return;
        }
        e.env_pending = !senv::same_set(&e.env_live, &e.meta.env);
        if e.exited() || e.is_chat() || !e.env_pending {
            return;
        }
        let live = live_plan(e, &e.env_live, &e.meta.env, lang);
        self.env_ask = Some((id, live));
    }

    /// Rückfrage zeigen (Hauptfenster bzw. das Fenster, in dem gespeichert wurde).
    pub(super) fn env_ask_dialog(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some((id, live)) = self.env_ask.take() else { return };
        let lang = self.settings.language;
        let Some(e) = self.entries.get(&id) else { return };
        let name = e.display_name();
        match env_ask_ui(ctx, &name, &live, lang) {
            LiveOut::Open => self.env_ask = Some((id, live)),
            LiveOut::Restart => {
                let lingering = self.lingering_of(&[id]);
                if lingering.is_empty() {
                    actions.push(Action::Restart(id));
                } else {
                    self.reload_ask = Some(session_settings::ReloadAsk { id, after_settings: true, detached: false, lingering });
                }
            }
            LiveOut::SetNow => {
                // Erneut prüfen: in der Zwischenzeit kann ein Programm gestartet worden sein.
                let ready = self.entries.get(&id).map(|e| live_ready(e, lang));
                match (ready, &live.command) {
                    (Some(Ok((shell, _))), Some(cmd)) => {
                        if let Some(e) = self.entries.get_mut(&id) {
                            if let Some(s) = &e.live {
                                // Erst die Eingabezeile leeren, damit der Befehl nicht an Getipptem hängt.
                                s.write_raw(&clear_line_bytes(shell, s.key_mode()));
                                s.send_text_agent(cmd, true);
                            }
                            // Stand der Shell fortschreiben; der Punkt bleibt, solange etwas noch nicht gilt.
                            let new = e.meta.env.clone();
                            senv::apply_live(&mut e.env_live, &new, &live.applied);
                            e.env_pending = !senv::same_set(&e.env_live, &e.meta.env);
                        }
                        self.want_focus = Some(id);
                    }
                    (Some(Err(why)), _) => self.notify_text(why),
                    _ => {}
                }
            }
            LiveOut::Later => self.want_focus = Some(self.focused),
        }
    }

    /// Kontextmenü: Variablen kopieren (bleibt in Paneldeck, geheime Werte nie in der Zwischenablage).
    pub(super) fn env_copy(&mut self, id: u64) {
        if let Some(e) = self.entries.get(&id) {
            self.env_clip = Some(e.meta.env.clone());
            let lang = self.settings.language;
            self.notify_text(if lang == Lang::De { format!("{} Variable(n) kopiert – „Variablen einfügen“ in einer anderen Sitzung.", e.meta.env.len()) } else { format!("{} variable(s) copied – “Paste variables” in another session.", e.meta.env.len()) });
        }
    }

    /// Kontextmenü: kopierte Variablen einfügen (gleiche Namen bekommen den kopierten Wert; danach unabhängig).
    pub(super) fn env_paste(&mut self, id: u64) {
        let Some(clip) = self.env_clip.clone() else { return };
        let Some(e) = self.entries.get_mut(&id).filter(|e| !e.is_chat()) else { return };
        let old = e.meta.env.clone();
        e.meta.env = senv::clean(senv::merge(&old, &clip));
        self.env_changed(id, old);
    }

    /// Kontextmenü: *Als .env kopieren* (geheime Werte leer).
    pub(super) fn env_copy_dotenv(&mut self, ctx: &egui::Context, id: u64) {
        if let Some(e) = self.entries.get(&id) {
            ctx.copy_text(senv::to_dotenv(&e.meta.env));
            let lang = self.settings.language;
            self.notify_text(lang.t("Als .env kopiert (geheime Werte leer).", "Copied as .env (secret values empty).").to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, PointerButton, RawInput};

    fn run(ctx: &egui::Context, ed: &mut EnvEditor, wsl: bool, events: Vec<Event>) {
        let input = RawInput { events, screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 900.0))), ..Default::default() };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                env_tab_ui(ui, ed, EnvUiOpts { lang: Lang::En, salt: "t", wsl, claude: true, profile: Some(("A, B", "Dev")) });
            });
        });
    }

    fn rect(ctx: &egui::Context, key: &'static str, n: usize) -> Option<Rect> {
        ctx.data(|d| d.get_temp::<Rect>(Id::new(("env_mark", key, n))))
    }

    fn click(ctx: &egui::Context, ed: &mut EnvEditor, key: &'static str, n: usize) {
        // erst ein Bild ohne Eingabe: Nach einer neuen Zeile übernimmt das Raster die Spaltenbreiten erst am Ende des
        // Bildes (das Fenster verwirft so ein Bild und zeichnet neu, der Test nicht) – geklickt wird ins fertige Layout.
        run(ctx, ed, false, vec![]);
        let p = rect(ctx, key, n).unwrap_or_else(|| panic!("{key} {n} fehlt")).center();
        run(ctx, ed, false, vec![Event::PointerMoved(p), Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
        run(ctx, ed, false, vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
    }

    /// Klick mit vorherigem Bild nur mit dem Zeiger darüber (nach Eingaben in ein mehrzeiliges Feld nötig).
    fn click_hover(ctx: &egui::Context, ed: &mut EnvEditor, key: &'static str, n: usize) {
        let p = rect(ctx, key, n).unwrap_or_else(|| panic!("{key} {n} fehlt")).center();
        run(ctx, ed, false, vec![Event::PointerMoved(p)]);
        run(ctx, ed, false, vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
        run(ctx, ed, false, vec![Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
    }

    fn type_into(ctx: &egui::Context, ed: &mut EnvEditor, key: &'static str, n: usize, text: &str) {
        click(ctx, ed, key, n);
        run(ctx, ed, false, vec![Event::Text(text.into())]);
    }

    #[test]
    fn the_tab_adds_rows_flags_secrets_and_deletes() {
        let ctx = egui::Context::default();
        let mut ed = EnvEditor::default();
        run(&ctx, &mut ed, false, vec![]);
        run(&ctx, &mut ed, false, vec![]);
        click(&ctx, &mut ed, "add", 0);
        assert_eq!(ed.rows.len(), 1);
        run(&ctx, &mut ed, false, vec![]);
        type_into(&ctx, &mut ed, "name", 0, "GITHUB_TOKEN");
        assert_eq!(ed.rows[0].name, "GITHUB_TOKEN");
        assert!(ed.rows[0].secret, "Name nach Geheimnis: geheim vorbelegt");
        type_into(&ctx, &mut ed, "value", 0, "ghp_123");
        assert_eq!(ed.rows[0].value, "ghp_123");
        // Von Hand umgeschaltet: der Name ändert es nicht mehr.
        click(&ctx, &mut ed, "secret", 0);
        assert!(!ed.rows[0].secret && ed.rows[0].secret_touched);
        type_into(&ctx, &mut ed, "name", 0, "X");
        assert!(!ed.rows[0].secret);
        // Geschützter Name: Fehler im Reiter (rot), Speichern verweigert.
        click(&ctx, &mut ed, "add", 0);
        run(&ctx, &mut ed, false, vec![]);
        type_into(&ctx, &mut ed, "name", 1, "PANELDECK_SESSION");
        assert_eq!(ed.tab_info(Lang::En).mark, super::super::opts_tabs::Mark::Error);
        assert!(ed.to_vars(Lang::En).is_err());
        click(&ctx, &mut ed, "delete", 1);
        assert_eq!(ed.rows.len(), 1);
        assert!(ed.ok(Lang::En));
        // Entfernen-Zeile
        click(&ctx, &mut ed, "remove", 0);
        assert!(ed.rows[0].remove);
        let v = ed.to_vars(Lang::En).unwrap();
        assert!(v[0].remove && v[0].value.is_empty());
        // Hinweis bei Namen der Shell-Integration (orange, kein Fehler)
        let w = EnvEditor::of(&[EnvVar::plain("PROMPT_COMMAND", "x")]);
        assert_eq!(w.tab_info(Lang::En).mark, super::super::opts_tabs::Mark::Warn);
        assert!(w.ok(Lang::En));
    }

    #[test]
    fn paste_and_import_preview() {
        let ctx = egui::Context::default();
        let mut ed = EnvEditor::of(&[EnvVar::plain("API_URL", "old")]);
        run(&ctx, &mut ed, false, vec![]);
        run(&ctx, &mut ed, false, vec![]);
        click(&ctx, &mut ed, "paste", 0);
        assert!(ed.paste.is_some());
        run(&ctx, &mut ed, false, vec![]);
        type_into(&ctx, &mut ed, "paste_text", 0, "API_URL=http://localhost:3000\nNODE_ENV=dev\nPANELDECK_SESSION=1\nDB_PASSWORD=x");
        click_hover(&ctx, &mut ed, "paste_ok", 0);
        assert!(ed.paste.is_none());
        let names: Vec<&str> = ed.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["API_URL", "NODE_ENV", "DB_PASSWORD"]);
        assert_eq!(ed.rows[0].value, "http://localhost:3000", "vorhandener Name bekommt den neuen Wert");
        assert!(ed.rows[2].secret);
        assert!(ed.note.as_deref().is_some_and(|n| n.contains("PANELDECK_SESSION")), "{:?}", ed.note);
        // .env-Import mit Vorschau: Zeilen abwählbar, geschützte nicht wählbar.
        ed.start_import("x.env", "# c\nA=1\nB='two'\nPANELDECK_X=1\nAPI_KEY=k\n");
        run(&ctx, &mut ed, false, vec![]);
        run(&ctx, &mut ed, false, vec![]);
        let p = ed.import.as_ref().unwrap();
        assert_eq!(p.lines.iter().map(|(l, on)| (l.name.as_str(), *on)).collect::<Vec<_>>(), [("A", true), ("B", true), ("PANELDECK_X", false), ("API_KEY", true)]);
        click(&ctx, &mut ed, "import_line", 1);
        assert!(!ed.import.as_ref().unwrap().lines[1].1, "B abgewählt");
        click(&ctx, &mut ed, "import_ok", 0);
        assert!(ed.import.is_none());
        assert!(ed.rows.iter().any(|r| r.name == "A" && r.value == "1"));
        assert!(!ed.rows.iter().any(|r| r.name == "B"));
        assert!(ed.rows.iter().any(|r| r.name == "API_KEY" && r.secret));
    }

    #[test]
    fn limits_and_wsl_column() {
        let mut ed = EnvEditor::default();
        let text: String = (0..120).map(|i| format!("V{i}=x\n")).collect();
        ed.paste_text(&text, Lang::En);
        assert_eq!(ed.count(), senv::MAX_VARS);
        assert!(ed.note.as_deref().unwrap().contains("V100"));
        ed.add_row();
        assert_eq!(ed.rows.len(), senv::MAX_VARS, "kein Platz mehr");
        let ctx = egui::Context::default();
        let mut w = EnvEditor::of(&[EnvVar::plain("P", r"C:\x")]);
        let mut frame = |wsl: bool| {
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 900.0))), ..Default::default() };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| env_tab_ui(ui, &mut w, EnvUiOpts { lang: Lang::De, salt: "w", wsl, claude: false, profile: None }));
            });
        };
        frame(true);
        frame(true);
    }

    #[cfg(windows)]
    #[test]
    fn secrets_are_encrypted_on_save_and_kept_when_untouched() {
        let mut ed = EnvEditor::default();
        ed.rows.push(EnvRow { name: "TOKEN".into(), value: "s3cr3t".into(), secret: true, ..EnvRow::default() });
        let v = ed.to_vars(Lang::En).unwrap();
        assert!(v[0].value.is_empty() && v[0].value_protected.is_some());
        let json = serde_json::to_string(&v).unwrap();
        assert!(!json.contains("s3cr3t"));
        // Wieder öffnen: der Wert steht nur verschlüsselt im Editor und bleibt beim Speichern erhalten.
        let again = EnvEditor::of(&v);
        assert!(again.rows[0].value.is_empty() && again.rows[0].stored == v[0].value_protected);
        assert_eq!(again.to_vars(Lang::En).unwrap()[0].value_protected, v[0].value_protected);
        // Vorlage aus einer Sitzung: ohne Zustimmung keine geheimen Werte.
        let mut tpl = EnvEditor { keep_secrets: Some(false), ..EnvEditor::of(&v) };
        assert_eq!(tpl.to_vars(Lang::En).unwrap()[0].value_protected, None);
        tpl.keep_secrets = Some(true);
        assert!(tpl.to_vars(Lang::En).unwrap()[0].value_protected.is_some());
        // Auf „normal“ gestellt: lesbar.
        let mut plain = EnvEditor::of(&v);
        plain.rows[0].secret = false;
        assert_eq!(plain.to_vars(Lang::En).unwrap()[0].value, "s3cr3t");
    }

    #[test]
    fn the_apply_question_offers_set_now_only_with_a_command() {
        let ctx = egui::Context::default();
        let frame = |live: &EnvLive, events: Vec<Event>| {
            let mut out = LiveOut::Open;
            let input = RawInput { events, screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 900.0))), ..Default::default() };
            let _ = ctx.run(input, |ctx| out = env_ask_ui(ctx, "S", live, Lang::En));
            out
        };
        let click_at = |p: Pos2| vec![Event::PointerMoved(p), Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }, Event::PointerButton { pos: p, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }];
        let ok = EnvLive { command: Some(" export A='1'".into()), why: None, later: vec![("TOKEN".into(), LiveIssue::Secret)], applied: vec![], notes: vec![] };
        frame(&ok, vec![]);
        frame(&ok, vec![]);
        let set = rect(&ctx, "ask_set", 0).unwrap();
        assert_eq!(frame(&ok, click_at(set.center())), LiveOut::SetNow);
        let no = EnvLive { command: None, why: Some("busy".into()), later: vec![], applied: vec![], notes: vec![] };
        frame(&no, vec![]);
        frame(&no, vec![]);
        let set = rect(&ctx, "ask_set", 0).unwrap();
        assert_eq!(frame(&no, click_at(set.center())), LiveOut::Open, "gesperrt");
        let r = rect(&ctx, "ask_restart", 0).unwrap();
        assert_eq!(frame(&no, click_at(r.center())), LiveOut::Restart);
        frame(&no, vec![]);
        let esc = Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
        assert_eq!(frame(&no, vec![esc]), LiveOut::Later);
    }

    /// #392: „Pfad umrechnen“ beim Setzen im laufenden WSL-Terminal über `targetpath`; geht es nicht, bleibt der Wert.
    #[test]
    fn live_wsl_paths_are_converted_or_kept() {
        use crate::targetpath::PathKind;
        let k = PathKind::Wsl { distro: Some("Ubuntu".into()) };
        assert_eq!(live_path_value(&k, r"C:\work\app", Lang::En).as_deref(), Ok("/mnt/c/work/app"));
        assert_eq!(live_path_value(&k, r"\\wsl.localhost\Ubuntu\home\me", Lang::En).as_deref(), Ok("/home/me"));
        assert_eq!(live_path_value(&k, r"C:\a;C:\b", Lang::En).as_deref(), Ok("/mnt/c/a:/mnt/c/b"));
        assert!(live_path_value(&k, r"\\wsl.localhost\Debian\home\me", Lang::En).is_err(), "andere Distribution");
        assert!(live_path_value(&k, r"C:\a;\\wsl.localhost\Debian\x", Lang::En).is_err(), "ein Eintrag geht nicht: ganze Liste bleibt");
    }

    #[test]
    fn set_now_clears_the_input_line_first() {
        use crate::vtkeys::KeyMode;
        let plain = KeyMode::default();
        assert_eq!(clear_line_bytes(senv::LiveShell::Posix, plain), vec![0x15], "Strg+U");
        assert_eq!(clear_line_bytes(senv::LiveShell::Pwsh, plain), vec![0x1b], "Esc");
        // Unter ConPTY (win32-input-mode) als echtes Tastenereignis kodiert, nicht als nacktes Byte.
        let win = KeyMode { win32: true, ..KeyMode::default() };
        let esc = clear_line_bytes(senv::LiveShell::Cmd, win);
        assert!(esc.starts_with(b"\x1b[") && esc.ends_with(b"_"), "{esc:?}");
        assert_eq!(esc, crate::vtkeys::key_bytes("esc", win).unwrap());
        // Import übernimmt den Modus aus `# paneldeck: prepend`.
        let mut ed = EnvEditor::default();
        ed.paste_text("# paneldeck: prepend\nPATH=C:\\tools\n", Lang::En);
        assert_eq!(ed.rows[0].mode, Mode::Prepend);
    }

    #[test]
    fn locked_texts_are_bilingual() {
        use crate::llm::secret::SecretError;
        for err in [SecretError::Sys(13), SecretError::NeedPassword] {
            let l = senv::Locked { names: vec!["TOKEN".into()], error: err };
            let (de, en) = (env_locked_text(&l, Lang::De), env_locked_text(&l, Lang::En));
            assert!(de != en && de.contains("TOKEN") && en.contains("TOKEN"));
        }
    }
}
