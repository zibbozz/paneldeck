//! Ordner-Auswahlfeld mit WSL-Unterstützung (#172), wiederverwendbar (neue Sitzung, Ordner-/Projektvorgaben,
//! Sitzungseinstellungen #170).
//!
//! # Benutzung
//! ```ignore
//! // Anlegen (z. B. beim Öffnen eines Dialogs); `Target` folgt aus der Shell:
//! let mut p = FolderPicker::new("newdlg", &cwd, Target::for_shell(shell.as_ref(), true));
//! // Jedes Bild (Shell kann sich ändern – billig, wenn gleich):
//! p.set_target(Target::for_shell(shell.as_ref(), true));
//! let r = p.show(ui, lang, &settings.wsl_recent);
//! if let Some(d) = r.distro_changed { /* z. B. Shell auf wsl:<d> umstellen */ }
//! // Übernehmen:
//! if p.blocking_error(lang).is_none() {
//!     let cwd = p.value();             // Windows-Pfad oder Linux-Pfad (`/…`, `~`)
//!     p.remember(&mut settings.wsl_recent); // „zuletzt benutzt“ (nur WSL)
//! }
//! ```
//! Für Windows-Shells ist es ein Textfeld mit „…“ (Ordnerdialog) wie bisher. Für WSL-Shells kommen dazu: Umschalter
//! „Windows-Pfad / WSL-Pfad“, Distribution (wenn wählbar), Vervollständigung und Prüfung im Hintergrund
//! (`wsl.exe`, entprellt), zuletzt benutzte Ordner der Distribution; der Ordnerdialog öffnet in
//! `\\wsl.localhost\<Distro>\home\<user>` und liefert den Linux-Pfad. Nichts davon blockiert die Oberfläche
//! (außer dem modalen Windows-Ordnerdialog selbst).

use crate::shells::ShellSpec;
use crate::state::Lang;
use crate::wsl::{self, Backend, PathError};
use eframe::egui::{self, Rect, RichText};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{mpsc, Arc};

/// Entprellzeit der Vervollständigung (Sekunden nach dem letzten Tastendruck).
const DEBOUNCE: f64 = 0.25;
/// So viele Vorschläge werden angezeigt.
const SHOW_MAX: usize = 8;

/// Wofür der Ordner gewählt wird.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Windows-Shell: Windows-Pfad.
    Windows,
    /// WSL-Shell. `distro` = None: Standard-Distribution. `choose_distro`: Auswahl der Distribution anzeigen
    /// (sonst ist sie durch die Shell festgelegt).
    Wsl { distro: Option<String>, choose_distro: bool },
}

impl Target {
    /// Ziel für eine Shell: WSL (`wsl:<Distro>` oder `wsl.exe -d <Distro>`) oder Windows.
    pub fn for_shell(shell: Option<&ShellSpec>, choose_distro: bool) -> Target {
        match shell {
            Some(s) if crate::shells::is_wsl(s) => {
                let distro = s.id.strip_prefix("wsl:").filter(|d| !d.is_empty()).map(str::to_string).or_else(|| crate::shells::wsl_distro(&s.args));
                Target::Wsl { distro, choose_distro }
            }
            _ => Target::Windows,
        }
    }
}

/// Ergebnis der Ordnerprüfung (nur WSL).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    /// Noch nicht geprüft (oder Windows-Pfad).
    Idle,
    /// Prüfung läuft / wartet auf die Entprellung oder das Home-Verzeichnis.
    Pending,
    Exists,
    Missing,
    Invalid(PathError),
}

/// Was sich in diesem Bild geändert hat.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PickerResponse {
    /// Text, Modus oder Distribution geändert.
    pub changed: bool,
    /// Der Nutzer hat eine andere Distribution gewählt (nur mit `choose_distro`, oder über einen
    /// `\\wsl.localhost\<andere>\…`-Pfad).
    pub distro_changed: Option<String>,
}

enum Msg {
    Distros(Vec<String>),
    Home(String, Option<String>),
    Listing { seq: u64, exists: bool, items: Vec<String> },
}

pub struct FolderPicker {
    id: egui::Id,
    target: Target,
    wsl_mode: bool,
    text: String,
    /// Hat der Nutzer (oder der Aufrufer per `set_text`) den Text gesetzt? Sonst gilt beim Wechsel zu WSL `~`.
    touched: bool,
    distro: Option<String>,
    distros: Option<Vec<String>>,
    distros_requested: bool,
    homes: BTreeMap<String, Option<String>>,
    homes_requested: BTreeSet<String>,
    check: Check,
    suggestions: Vec<String>,
    edited_at: Option<f64>,
    seq: u64,
    queried: Option<(String, String)>,
    note: Option<String>,
    win_home: String,
    tx: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
    backend: Arc<dyn Backend>,
    /// Lage der Bedienelemente im letzten Bild (für Tests).
    rects: Rects,
}

struct Rects {
    text: Rect,
    /// Umschalter Windows-Pfad / WSL-Pfad
    modes: [Rect; 2],
    suggestions: Vec<Rect>,
}

impl Default for Rects {
    fn default() -> Self {
        Rects { text: Rect::NOTHING, modes: [Rect::NOTHING; 2], suggestions: Vec::new() }
    }
}

impl FolderPicker {
    /// Neues Feld mit Startwert (Windows- oder Linux-Pfad). Abfragen laufen über `wsl.exe`.
    pub fn new(id_salt: impl std::hash::Hash, initial: &str, target: Target) -> Self {
        Self::with_backend(id_salt, initial, target, Arc::new(wsl::WslExe))
    }

    /// Wie `new`, mit eigenem Zugriff auf WSL (Tests).
    pub fn with_backend(id_salt: impl std::hash::Hash, initial: &str, target: Target, backend: Arc<dyn Backend>) -> Self {
        Self::build(id_salt, initial, target, backend, std::env::var("USERPROFILE").unwrap_or_default())
    }

    fn build(id_salt: impl std::hash::Hash, initial: &str, target: Target, backend: Arc<dyn Backend>, win_home: String) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut p = FolderPicker {
            id: egui::Id::new(("folder_picker", id_salt)),
            target: Target::Windows,
            wsl_mode: false,
            text: initial.to_string(),
            touched: false,
            distro: None,
            distros: None,
            distros_requested: false,
            homes: BTreeMap::new(),
            homes_requested: BTreeSet::new(),
            check: Check::Idle,
            suggestions: Vec::new(),
            edited_at: None,
            seq: 0,
            queried: None,
            note: None,
            win_home,
            tx,
            rx,
            backend,
            rects: Rects::default(),
        };
        p.set_target(target);
        p
    }

    /// Ziel ändern (z. B. andere Shell gewählt). Gleiches Ziel: nichts. Windows ↔ WSL rechnet den Pfad um.
    pub fn set_target(&mut self, t: Target) {
        if t == self.target {
            return;
        }
        match &t {
            Target::Windows => {
                if self.wsl_mode {
                    self.leave_wsl();
                }
            }
            Target::Wsl { distro, .. } => {
                if distro.is_some() {
                    self.distro = distro.clone();
                }
                if !self.wsl_mode {
                    self.target = t.clone();
                    self.enter_wsl();
                }
                self.reset_check();
            }
        }
        self.target = t;
    }

    /// Wert von außen setzen (z. B. Startordner eines Profils). Im WSL-Modus wird ein Windows-Pfad umgerechnet.
    pub fn set_text(&mut self, s: &str) {
        self.text = s.to_string();
        self.touched = true;
        if self.wsl_mode {
            self.convert_typed();
        }
        self.reset_check();
    }

    /// Gewählter Ordner zum Speichern: Windows-Pfad, oder im WSL-Modus ein Linux-Pfad (`/…` oder `~`; `~/x` wird
    /// mit dem bekannten Home aufgelöst, abschließender `/` entfernt).
    pub fn value(&self) -> String {
        if self.wsl_mode {
            wsl::normalize(&self.text, self.home())
        } else {
            self.text.trim().to_string()
        }
    }

    /// Wird ein Linux-Pfad gewählt?
    pub fn is_wsl(&self) -> bool {
        self.wsl_mode
    }

    /// Distribution im WSL-Modus (None: noch unbekannt bzw. Windows-Pfad).
    pub fn distro(&self) -> Option<&str> {
        self.distro.as_deref().filter(|_| self.wsl_mode)
    }

    /// Grund, warum der Wert nicht übernommen werden sollte (ungültiger Linux-Pfad, Ordner fehlt), sonst None.
    /// Läuft die Prüfung noch, gilt der Wert als übernehmbar.
    pub fn blocking_error(&self, lang: Lang) -> Option<String> {
        if !self.wsl_mode {
            return None;
        }
        match self.check {
            Check::Invalid(e) => Some(e.message(lang == Lang::De).to_string()),
            Check::Missing => Some(self.missing_text(lang)),
            _ => None,
        }
    }

    /// WSL-Ordner in „zuletzt benutzt“ der Distribution aufnehmen (Windows-Pfade und die Vorgabe `~`: nichts).
    pub fn remember(&self, recent: &mut BTreeMap<String, Vec<String>>) {
        let v = self.value();
        if let Some(d) = self.distro().filter(|_| v != "~") {
            wsl::push_recent(recent, d, &v);
        }
    }

    /// Zeichnet das Feld (ohne Beschriftung, passt in eine Grid-Zelle). `recent` = `Settings::wsl_recent`.
    pub fn show(&mut self, ui: &mut egui::Ui, lang: Lang, recent: &BTreeMap<String, Vec<String>>) -> PickerResponse {
        let mut r = PickerResponse::default();
        let now = ui.input(|i| i.time);
        self.poll(&mut r);
        if self.wsl_mode {
            self.request_background(ui.ctx());
        }
        let de = lang == Lang::De;
        ui.vertical(|ui| {
            if let Target::Wsl { choose_distro, .. } = self.target.clone() {
                ui.horizontal(|ui| {
                    let w = ui.selectable_label(!self.wsl_mode, lang.t("Windows-Pfad", "Windows path"));
                    let l = ui.selectable_label(self.wsl_mode, lang.t("WSL-Pfad", "WSL path"));
                    self.rects.modes = [w.rect, l.rect];
                    if w.clicked() && self.wsl_mode {
                        self.leave_wsl();
                        r.changed = true;
                    }
                    if l.clicked() && !self.wsl_mode {
                        self.enter_wsl();
                        r.changed = true;
                    }
                    if self.wsl_mode && choose_distro {
                        let cur = self.distro.clone().unwrap_or_else(|| lang.t("lädt …", "loading …").to_string());
                        egui::ComboBox::from_id_salt(self.id.with("distro")).selected_text(cur).show_ui(ui, |ui| match &self.distros {
                            None => {
                                ui.weak(lang.t("lädt …", "loading …"));
                            }
                            Some(list) if list.is_empty() => {
                                ui.weak(lang.t("Keine WSL-Distribution gefunden", "No WSL distribution found"));
                            }
                            Some(list) => {
                                for d in list.clone() {
                                    if ui.selectable_label(self.distro.as_deref() == Some(d.as_str()), &d).clicked() && self.distro.as_deref() != Some(d.as_str()) {
                                        self.distro = Some(d.clone());
                                        self.reset_check();
                                        r.changed = true;
                                        r.distro_changed = Some(d);
                                    }
                                }
                            }
                        });
                    }
                });
            }
            let te_id = self.id.with("text");
            ui.horizontal(|ui| {
                let hint = if self.wsl_mode { lang.t("/home/… oder ~", "/home/… or ~") } else { "" };
                let te = ui.add(egui::TextEdit::singleline(&mut self.text).id(te_id).hint_text(hint).desired_width(260.0));
                self.rects.text = te.rect;
                if te.changed() {
                    self.touched = true;
                    if self.wsl_mode {
                        if let Some(d) = self.convert_typed() {
                            r.distro_changed = Some(d);
                        }
                    }
                    self.edited_at = Some(now);
                    self.reset_check();
                    r.changed = true;
                }
                if ui.button("…").on_hover_text(lang.t("Ordner wählen", "Choose folder")).clicked() {
                    if let Some(p) = self.dialog_start().and_then(|s| crate::notify::pick_folder(&s)) {
                        if let Some(d) = self.apply_picked(&p, lang) {
                            r.distro_changed = Some(d);
                        }
                        r.changed = true;
                    }
                }
                let recents: Vec<String> = self.distro().and_then(|d| recent.get(d)).cloned().unwrap_or_default();
                if self.wsl_mode && !recents.is_empty() {
                    ui.menu_button("▾", |ui| {
                        ui.label(RichText::new(lang.t("Zuletzt benutzt", "Recently used")).small().weak());
                        for p in &recents {
                            if ui.button(p).clicked() {
                                self.set_text(p);
                                r.changed = true;
                                ui.close();
                            }
                        }
                    })
                    .response
                    .on_hover_text(lang.t("Zuletzt benutzte WSL-Ordner", "Recently used WSL folders"));
                }
            });
            if self.wsl_mode {
                self.status_line(ui, lang, de);
                self.rects.suggestions.clear();
                let shown: Vec<String> = self.visible_suggestions();
                if !shown.is_empty() {
                    let mut pick = None;
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for (i, s) in shown.iter().enumerate() {
                            let name = s.trim_end_matches('/').rsplit('/').next().unwrap_or(s);
                            let b = ui.small_button(format!("{name}/")).on_hover_text(s.as_str());
                            self.rects.suggestions.push(b.rect);
                            if b.clicked() {
                                pick = Some(i);
                            }
                        }
                    });
                    if let Some(i) = pick {
                        self.text = shown[i].clone();
                        self.touched = true;
                        self.edited_at = None;
                        self.reset_check();
                        r.changed = true;
                        // Weiter tippen können: Fokus zurück, Schreibmarke ans Ende.
                        let ctx = ui.ctx();
                        if let Some(mut st) = egui::text_edit::TextEditState::load(ctx, te_id) {
                            let n = self.text.chars().count();
                            st.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(n))));
                            st.store(ctx, te_id);
                        }
                        ctx.memory_mut(|m| m.request_focus(te_id));
                    }
                }
            }
            if let Some(n) = &self.note {
                ui.label(RichText::new(n).small().color(ui.visuals().warn_fg_color));
            }
        });
        if self.wsl_mode {
            self.maybe_query(ui.ctx(), now);
        }
        r
    }

    fn home(&self) -> Option<&str> {
        self.distro.as_ref().and_then(|d| self.homes.get(d)).and_then(|h| h.as_deref())
    }

    fn missing_text(&self, lang: Lang) -> String {
        let d = self.distro.clone().unwrap_or_default();
        if lang == Lang::De { format!("Ordner nicht gefunden (in {d})") } else { format!("Folder not found (in {d})") }
    }

    fn reset_check(&mut self) {
        // Antworten auf ältere Abfragen verwerfen.
        self.seq += 1;
        self.check = if self.wsl_mode { Check::Pending } else { Check::Idle };
        self.suggestions.clear();
        self.queried = None;
        self.note = None;
    }

    /// Wechsel zu WSL: unveränderte Vorgabe (leer oder Windows-Home) → `~`, Windows-Pfad → `/mnt/<lw>/…`,
    /// `\\wsl.localhost\<D>\…` → Linux-Pfad.
    fn enter_wsl(&mut self) {
        self.wsl_mode = true;
        let t = self.text.trim().to_string();
        let untouched_default = !self.touched && (t.is_empty() || same_windows_path(&t, &self.win_home));
        if t.is_empty() || untouched_default {
            self.text = "~".into();
        } else {
            self.convert_typed();
        }
        self.reset_check();
    }

    /// Wechsel zu Windows: `/mnt/c/x` → `C:\x`, anderer Linux-Pfad → `\\wsl.localhost\<D>\…`, `~` (die Vorgabe)
    /// → Windows-Home, `~/x` → UNC-Pfad im Linux-Home.
    fn leave_wsl(&mut self) {
        let t = self.text.trim().to_string();
        let d = self.distro.clone().unwrap_or_default();
        let full = wsl::expand_home(&t, self.home());
        self.text = match full.as_deref().and_then(|f| wsl::linux_to_windows(&d, f)) {
            _ if t == "~" => self.win_home.clone(),
            Some(w) => w,
            None if t.starts_with('~') => self.win_home.clone(),
            None => t,
        };
        self.wsl_mode = false;
        self.reset_check();
    }

    /// Im WSL-Modus eingegebenen Windows-/UNC-Pfad umrechnen. Liefert eine neue Distribution, wenn ein
    /// `\\wsl.localhost\<andere>\…`-Pfad sie wechselt (nur wenn wählbar).
    fn convert_typed(&mut self) -> Option<String> {
        let t = self.text.trim();
        if wsl::looks_linux(t) {
            return None;
        }
        if let Some((d, l)) = wsl::unc_to_linux(t) {
            self.text = l;
            return self.switch_distro(&d);
        }
        if let Some(l) = wsl::windows_to_linux(t) {
            self.text = l;
        }
        None
    }

    /// Pfad aus dem Ordnerdialog übernehmen.
    fn apply_picked(&mut self, p: &str, lang: Lang) -> Option<String> {
        self.touched = true;
        self.edited_at = None;
        if !self.wsl_mode {
            self.text = p.to_string();
            self.reset_check();
            return None;
        }
        let other = wsl::unc_to_linux(p).map(|(d, _)| d).filter(|d| self.distro.as_deref().is_some_and(|c| !c.eq_ignore_ascii_case(d)));
        if let (Some(d), Target::Wsl { choose_distro: false, .. }) = (&other, &self.target) {
            self.note = Some(if lang == Lang::De { format!("Der Ordner liegt in {d}, nicht in der Distribution dieser Shell.") } else { format!("The folder is in {d}, not in this shell's distribution.") });
            return None;
        }
        self.text = p.to_string();
        let changed = self.convert_typed();
        self.reset_check();
        changed
    }

    fn switch_distro(&mut self, d: &str) -> Option<String> {
        let choose = matches!(self.target, Target::Wsl { choose_distro: true, .. });
        if !choose || self.distro.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(d)) {
            return None;
        }
        let name = self.distros.as_ref().and_then(|l| l.iter().find(|x| x.eq_ignore_ascii_case(d)).cloned()).unwrap_or_else(|| d.to_string());
        self.distro = Some(name.clone());
        Some(name)
    }

    /// Startordner des Ordnerdialogs: im WSL-Modus `\\wsl.localhost\<D>\<ordner oder home>` (sonst `\\wsl$\…`).
    fn dialog_start(&self) -> Option<String> {
        if !self.wsl_mode {
            return Some(self.text.clone());
        }
        let d = self.distro.clone()?;
        let linux = wsl::expand_home(self.text.trim(), self.home())
            .filter(|_| self.check == Check::Exists)
            .or_else(|| self.home().map(str::to_string))
            .unwrap_or_else(|| "/".into());
        let unc = wsl::linux_to_unc(&d, &linux);
        if std::path::Path::new(&unc).is_dir() {
            return Some(unc);
        }
        Some(crate::pathfmt::from_wsl(&format!("wsl:{d}"), &linux))
    }

    fn visible_suggestions(&self) -> Vec<String> {
        // Ist der Text schon genau ein Vorschlag (bzw. der Ordner ohne `/`), nichts mehr anbieten.
        let t = self.text.trim();
        if self.suggestions.len() == 1 && self.suggestions[0].trim_end_matches('/') == t.trim_end_matches('/') {
            return Vec::new();
        }
        self.suggestions.iter().take(SHOW_MAX).cloned().collect()
    }

    fn status_line(&self, ui: &mut egui::Ui, lang: Lang, de: bool) {
        let dim = crate::style::dim(ui);
        let (text, color) = match self.check {
            Check::Idle => return,
            Check::Pending => (lang.t("Prüfe …", "Checking …").to_string(), dim),
            Check::Exists => {
                let mut s = lang.t("Ordner gefunden", "Folder found").to_string();
                if self.text.trim().starts_with('~') {
                    if let Some(h) = self.home() {
                        s += &format!(" · ~ = {h}");
                    }
                }
                (s, dim)
            }
            Check::Missing => (self.missing_text(lang), ui.visuals().warn_fg_color),
            Check::Invalid(e) => (e.message(de).to_string(), ui.visuals().error_fg_color),
        };
        ui.label(RichText::new(text).small().color(color));
    }

    fn poll(&mut self, r: &mut PickerResponse) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Distros(v) => {
                    if self.distro.is_none() {
                        if let Some(first) = v.first() {
                            self.distro = Some(first.clone());
                            self.reset_check();
                            r.changed = true;
                        }
                    }
                    self.distros = Some(v);
                }
                Msg::Home(d, h) => {
                    self.homes.insert(d, h);
                    // Eine wegen des Homes wartende Prüfung neu starten.
                    if self.check == Check::Pending {
                        self.queried = None;
                    }
                }
                Msg::Listing { seq, exists, items } if seq == self.seq => {
                    self.check = if exists { Check::Exists } else { Check::Missing };
                    let base = match self.text.trim() {
                        "~" => "~/".to_string(),
                        t => t.rfind('/').map(|i| t[..=i].to_string()).unwrap_or_default(),
                    };
                    self.suggestions = items.into_iter().map(|n| format!("{base}{n}")).collect();
                }
                Msg::Listing { .. } => {}
            }
        }
    }

    fn request_background(&mut self, ctx: &egui::Context) {
        if !self.distros_requested && (self.distro.is_none() || matches!(self.target, Target::Wsl { choose_distro: true, .. })) {
            self.distros_requested = true;
            let (b, tx, ctx) = (self.backend.clone(), self.tx.clone(), ctx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(Msg::Distros(b.distros()));
                ctx.request_repaint();
            });
        }
        if let Some(d) = self.distro.clone() {
            if self.homes_requested.insert(d.clone()) {
                let (b, tx, ctx) = (self.backend.clone(), self.tx.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let h = b.home(&d);
                    let _ = tx.send(Msg::Home(d, h));
                    ctx.request_repaint();
                });
            }
        }
    }

    /// Vervollständigung und Prüfung im Hintergrund starten, wenn sich Text/Distribution geändert haben und die
    /// Entprellzeit um ist.
    fn maybe_query(&mut self, ctx: &egui::Context, now: f64) {
        let Some(d) = self.distro.clone() else { return };
        let text = self.text.trim().to_string();
        let key = (d.clone(), text.clone());
        if self.queried.as_ref() == Some(&key) {
            return;
        }
        if let Some(t) = self.edited_at {
            let wait = DEBOUNCE - (now - t);
            if wait > 0.0 {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait));
                return;
            }
        }
        let home_known = self.homes.contains_key(&d);
        let home = self.home().map(str::to_string);
        // `~` braucht das Home nur für Vorschläge; `~/x` zum Auflösen.
        let full = match wsl::expand_home(&text, home.as_deref()) {
            Some(f) => f,
            None if text.starts_with('~') && !home_known => {
                self.check = Check::Pending;
                return;
            }
            None if text == "~" => {
                self.queried = Some(key);
                self.check = Check::Exists;
                return;
            }
            None => {
                self.queried = Some(key);
                self.check = Check::Invalid(wsl::validate_linux_path(&text).err().unwrap_or(PathError::NotAbsolute));
                return;
            }
        };
        self.queried = Some(key);
        if let Err(e) = wsl::validate_linux_path(&full) {
            self.check = Check::Invalid(e);
            return;
        }
        let Some((dir, prefix)) = wsl::split_for_completion(&text, home.as_deref()) else {
            self.check = Check::Invalid(PathError::NotAbsolute);
            return;
        };
        self.seq += 1;
        self.check = Check::Pending;
        let (seq, b, tx, ctx) = (self.seq, self.backend.clone(), self.tx.clone(), ctx.clone());
        let is_home = text == "~";
        std::thread::spawn(move || {
            let listed = b.list(&d, &dir);
            let items = listed.as_deref().map(|o| wsl::parse_ls(o, &prefix, 50)).unwrap_or_default();
            let exists = if is_home || prefix.is_empty() { is_home || listed.is_some() } else { b.is_dir(&d, &full) };
            let _ = tx.send(Msg::Listing { seq, exists, items });
            ctx.request_repaint();
        });
    }
}

/// Gleicher Windows-Pfad (Groß-/Kleinschreibung und Schrägstriche egal)?
fn same_windows_path(a: &str, b: &str) -> bool {
    let n = |s: &str| s.trim().trim_end_matches(['\\', '/']).replace('/', "\\").to_lowercase();
    !b.is_empty() && n(a) == n(b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, PointerButton, Pos2, RawInput, Vec2};
    use std::sync::Mutex;

    /// Schein-WSL: Ubuntu (Home /home/u) mit festen Ordnern; zählt die Aufrufe.
    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<String>>,
    }

    impl Backend for Fake {
        fn distros(&self) -> Vec<String> {
            self.calls.lock().unwrap().push("distros".into());
            vec!["Ubuntu-24.04".into(), "Debian".into()]
        }
        fn home(&self, d: &str) -> Option<String> {
            self.calls.lock().unwrap().push(format!("home {d}"));
            Some(if d == "Debian" { "/home/deb".into() } else { "/home/u".into() })
        }
        fn list(&self, d: &str, dir: &str) -> Option<Vec<u8>> {
            self.calls.lock().unwrap().push(format!("list {d} {dir}"));
            match dir {
                "/home/u/" => Some(b"project/\nproto/\nnotes.txt\n.cache/\n".to_vec()),
                "/home/u/project/" => Some(b"src/\n".to_vec()),
                "/" => Some(b"home/\nmnt/\n".to_vec()),
                _ => None,
            }
        }
        fn is_dir(&self, d: &str, p: &str) -> bool {
            self.calls.lock().unwrap().push(format!("isdir {d} {p}"));
            matches!(p, "/home/u" | "/home/u/project" | "/home/u/proto" | "/" | "/home/u/project/src")
        }
    }

    struct H {
        ctx: egui::Context,
        t: f64,
        p: FolderPicker,
        fake: Arc<Fake>,
        last: PickerResponse,
    }

    impl H {
        fn new(initial: &str, target: Target) -> H {
            let fake = Arc::new(Fake::default());
            let p = FolderPicker::build("t", initial, target, fake.clone(), r"C:\Users\u".into());
            H { ctx: egui::Context::default(), t: 0.0, p, fake, last: PickerResponse::default() }
        }

        fn frame(&mut self, events: Vec<Event>) -> PickerResponse {
            self.t += 0.05;
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(700.0, 400.0))), time: Some(self.t), events, ..Default::default() };
            let mut r = PickerResponse::default();
            let recent: BTreeMap<String, Vec<String>> = BTreeMap::new();
            let p = &mut self.p;
            let _ = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    r = p.show(ui, Lang::En, &recent);
                });
            });
            if r.changed {
                self.last = r.clone();
            }
            r
        }

        /// Bilder zeichnen (Zeit läuft weiter, Hintergrund-Threads arbeiten), bis `f` gilt.
        fn until(&mut self, what: &str, f: impl Fn(&FolderPicker) -> bool) {
            for _ in 0..200 {
                self.frame(vec![]);
                if f(&self.p) {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("timeout: {what} (text {:?}, check {:?}, suggestions {:?})", self.p.text, self.p.check, self.p.suggestions);
        }

        fn click(&mut self, at: Pos2) {
            self.frame(vec![Event::PointerMoved(at)]);
            self.frame(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: true, modifiers: Default::default() }]);
            self.frame(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed: false, modifiers: Default::default() }]);
        }
    }

    fn ubuntu() -> Target {
        Target::Wsl { distro: Some("Ubuntu-24.04".into()), choose_distro: false }
    }

    #[test]
    fn target_from_shell() {
        let s = |id: &str, prog: &str, args: &[&str]| ShellSpec { id: id.into(), label: id.into(), program: prog.into(), args: args.iter().map(|a| a.to_string()).collect() };
        assert_eq!(Target::for_shell(Some(&s("wsl:Ubuntu", "wsl.exe", &["-d", "Ubuntu"])), true), Target::Wsl { distro: Some("Ubuntu".into()), choose_distro: true });
        assert_eq!(Target::for_shell(Some(&s("custom:3", "C:\\Windows\\System32\\wsl.exe", &["-d", "Arch"])), false), Target::Wsl { distro: Some("Arch".into()), choose_distro: false });
        assert_eq!(Target::for_shell(Some(&s("wsl:", "wsl.exe", &[])), false), Target::Wsl { distro: None, choose_distro: false });
        assert_eq!(Target::for_shell(Some(&s("pwsh", "pwsh.exe", &[])), true), Target::Windows);
        assert_eq!(Target::for_shell(None, true), Target::Windows);
    }

    #[test]
    fn default_is_linux_home_and_windows_paths_convert() {
        // Unveränderte Vorgabe (Windows-Home) → ~ für WSL.
        let p = FolderPicker::build("a", r"C:\Users\u", ubuntu(), Arc::new(Fake::default()), r"C:\Users\u".into());
        assert!(p.is_wsl());
        assert_eq!(p.value(), "~");
        assert_eq!(p.distro(), Some("Ubuntu-24.04"));
        // Zurück zu einer Windows-Shell: ~ wird wieder das Windows-Home; ~ landet nicht in „zuletzt benutzt“.
        let mut m = BTreeMap::new();
        p.remember(&mut m);
        assert!(m.is_empty());
        let mut p = p;
        p.set_target(Target::Windows);
        assert_eq!(p.value(), r"C:\Users\u");
        // Ein anderer Windows-Pfad wird umgerechnet, zurück zu Windows wieder hergestellt.
        let mut p = FolderPicker::build("b", r"D:\Dev\my app", Target::Windows, Arc::new(Fake::default()), r"C:\Users\u".into());
        assert!(!p.is_wsl());
        assert_eq!(p.value(), r"D:\Dev\my app");
        p.set_target(ubuntu());
        assert_eq!(p.value(), "/mnt/d/Dev/my app");
        p.set_target(Target::Windows);
        assert_eq!(p.value(), r"D:\Dev\my app");
        assert_eq!(p.distro(), None, "Windows-Pfad: keine Distribution");
        // Linux-Pfad zu Windows: UNC über wsl.localhost
        let mut p = FolderPicker::with_backend("c", "/home/u/p", ubuntu(), Arc::new(Fake::default()));
        assert_eq!(p.value(), "/home/u/p");
        p.set_target(Target::Windows);
        assert_eq!(p.value(), r"\\wsl.localhost\Ubuntu-24.04\home\u\p");
        // Eingefügter UNC-Pfad im WSL-Modus
        let mut p = FolderPicker::with_backend("d", "", ubuntu(), Arc::new(Fake::default()));
        assert_eq!(p.value(), "~");
        p.set_text(r"\\wsl$\Ubuntu-24.04\srv\x\");
        assert_eq!(p.value(), "/srv/x");
        p.set_text(r"E:\a");
        assert_eq!(p.value(), "/mnt/e/a");
    }

    #[test]
    fn picked_folder_from_the_dialog_becomes_a_linux_path() {
        let mut p = FolderPicker::with_backend("e", "~", ubuntu(), Arc::new(Fake::default()));
        assert_eq!(p.apply_picked(r"\\wsl.localhost\Ubuntu-24.04\home\u\project", Lang::En), None);
        assert_eq!(p.value(), "/home/u/project");
        assert_eq!(p.apply_picked(r"C:\Users\u\x y", Lang::En), None);
        assert_eq!(p.value(), "/mnt/c/Users/u/x y");
        // Andere Distribution bei fester Shell: abgelehnt mit Hinweis
        assert_eq!(p.apply_picked(r"\\wsl.localhost\Debian\home\deb", Lang::En), None);
        assert_eq!(p.value(), "/mnt/c/Users/u/x y");
        assert!(p.note.as_deref().is_some_and(|n| n.contains("Debian")));
        // Wählbare Distribution: wechselt
        let mut p = FolderPicker::with_backend("f", "~", Target::Wsl { distro: Some("Ubuntu-24.04".into()), choose_distro: true }, Arc::new(Fake::default()));
        assert_eq!(p.apply_picked(r"\\wsl$\debian\srv", Lang::En).as_deref(), Some("debian"));
        assert_eq!((p.distro(), p.value().as_str()), (Some("debian"), "/srv"));
        // Windows-Ziel: Pfad unverändert
        let mut p = FolderPicker::with_backend("g", "", Target::Windows, Arc::new(Fake::default()));
        p.apply_picked(r"C:\x", Lang::En);
        assert_eq!(p.value(), r"C:\x");
    }

    #[test]
    fn completion_and_check_run_in_the_background() {
        let mut h = H::new("/home/u/pr", ubuntu());
        h.until("Vorschläge", |p| p.check == Check::Missing && p.suggestions.len() == 2);
        assert_eq!(h.p.suggestions, vec!["/home/u/project/".to_string(), "/home/u/proto/".to_string()]);
        assert!(h.p.blocking_error(Lang::En).is_some_and(|e| e.contains("not found")));
        // Klick auf den ersten Vorschlag übernimmt ihn; danach wird der Ordner selbst geprüft und aufgelistet.
        let b = h.p.rects.suggestions[0].center();
        h.click(b);
        assert_eq!(h.p.text, "/home/u/project/");
        assert!(h.last.changed);
        h.until("Unterordner", |p| p.check == Check::Exists && p.suggestions == vec!["/home/u/project/src/".to_string()]);
        assert_eq!(h.p.value(), "/home/u/project");
        assert!(h.p.blocking_error(Lang::En).is_none());
        // Der Fokus ist zurück im Textfeld: weiter tippen hängt an.
        h.frame(vec![Event::Text("s".into())]);
        assert_eq!(h.p.text, "/home/u/project/s");
        h.until("Vorschlag src", |p| p.suggestions == vec!["/home/u/project/src/".to_string()] && p.check == Check::Missing);
        // Ungültiger Pfad: sofort ungültig, kein wsl.exe-Aufruf
        let n = h.fake.calls.lock().unwrap().len();
        h.p.set_text("/a\u{7}b");
        h.until("ungültig", |p| p.check == Check::Invalid(PathError::Control));
        assert_eq!(h.fake.calls.lock().unwrap().len(), n);
        assert!(h.p.blocking_error(Lang::De).is_some_and(|e| e.contains("Steuerzeichen")));
    }

    #[test]
    fn typing_is_debounced_and_home_is_resolved() {
        let mut h = H::new("", ubuntu());
        assert_eq!(h.p.text, "~");
        h.until("Home", |p| p.home() == Some("/home/u") && p.check == Check::Exists && p.suggestions.len() == 2);
        assert_eq!(h.p.suggestions, vec!["~/project/".to_string(), "~/proto/".to_string()]);
        // Tippen: Text ins Feld (Fokus per Klick), mehrere Zeichen schnell hintereinander → eine Abfrage.
        let te = h.p.rects.text.center();
        h.click(te);
        h.frame(vec![Event::Key { key: egui::Key::End, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }]);
        let lists_before = h.fake.calls.lock().unwrap().iter().filter(|c| c.starts_with("list")).count();
        for c in ["/", "p", "r", "o", "j"] {
            h.frame(vec![Event::Text(c.into())]);
        }
        assert_eq!(h.p.text, "~/proj");
        assert_eq!(h.p.check, Check::Pending, "Entprellung läuft");
        h.until("Treffer", |p| p.check == Check::Missing && p.suggestions == vec!["~/project/".to_string()]);
        let lists_after = h.fake.calls.lock().unwrap().iter().filter(|c| c.starts_with("list")).count();
        assert_eq!(lists_after - lists_before, 1, "nur eine Abfrage nach dem Tippen");
        // `~/x` wird beim Übernehmen aufgelöst (wsl.exe --cd kennt nur ~ allein).
        h.p.set_text("~/project");
        assert_eq!(h.p.value(), "/home/u/project");
        // Das Home wird je Distribution nur einmal geholt.
        assert_eq!(h.fake.calls.lock().unwrap().iter().filter(|c| c.starts_with("home")).count(), 1);
    }

    #[test]
    fn distro_choice_lists_all_distros_default_first() {
        let mut h = H::new("", Target::Wsl { distro: None, choose_distro: true });
        h.until("Distributionen", |p| p.distros.is_some() && p.distro.is_some());
        assert_eq!(h.p.distro(), Some("Ubuntu-24.04"), "Standard-Distribution zuerst und vorausgewählt");
        assert!(h.last.changed);
        h.until("Home", |p| p.check == Check::Exists);
        assert!(h.p.is_wsl());
        assert_eq!(h.fake.calls.lock().unwrap().iter().filter(|c| *c == "distros").count(), 1);
    }

    #[test]
    fn mode_switch_buttons_convert_the_path() {
        let mut h = H::new(r"C:\Work\a b", Target::Wsl { distro: Some("Ubuntu-24.04".into()), choose_distro: false });
        h.p.touched = true;
        h.frame(vec![]);
        assert_eq!(h.p.value(), "/mnt/c/Work/a b");
        let win_btn = h.p.rects.modes[0].center();
        h.click(win_btn);
        assert!(!h.p.is_wsl());
        assert_eq!(h.p.value(), r"C:\Work\a b");
        assert!(h.last.changed);
        let wsl_btn = h.p.rects.modes[1].center();
        h.click(wsl_btn);
        assert!(h.p.is_wsl());
        assert_eq!(h.p.value(), "/mnt/c/Work/a b");
    }

    #[test]
    fn recents_are_remembered_per_distro_only_for_wsl() {
        let mut m = BTreeMap::new();
        let mut p = FolderPicker::with_backend("r", "/home/u/p/", ubuntu(), Arc::new(Fake::default()));
        p.remember(&mut m);
        assert_eq!(m["Ubuntu-24.04"], vec!["/home/u/p".to_string()]);
        p.set_target(Target::Windows);
        p.remember(&mut m);
        assert_eq!(m.len(), 1);
        assert_eq!(m["Ubuntu-24.04"].len(), 1);
    }

    #[test]
    fn windows_target_is_a_plain_field() {
        let mut h = H::new(r"C:\x", Target::Windows);
        h.frame(vec![]);
        h.frame(vec![]);
        assert_eq!(h.p.check, Check::Idle);
        assert!(h.fake.calls.lock().unwrap().is_empty(), "keine wsl.exe-Aufrufe für Windows-Shells");
        assert!(h.p.blocking_error(Lang::En).is_none());
        assert!(same_windows_path(r"c:/users/U/", r"C:\Users\u"));
        assert!(!same_windows_path(r"C:\x", ""));
    }
}
