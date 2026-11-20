//! Seitenfenster und Übersichten.

use super::*;

/// Inhalt eines Panels so zeigen, dass er die Größe des Panels nicht verändern kann (#160, #236).
///
/// egui füllt den Rahmen eines `SidePanel`/`TopBottomPanel` nur innerhalb des Panel-Rechtecks (Clip), reserviert aber das
/// ganze Rechteck des Inhalts: Ragt eine Zeile über die Breite (Seitenleiste, Dateibaum) oder der Inhalt über die feste
/// Höhe (Kopf der Todo-Liste), beginnt das Nachbar-Panel erst dahinter. Der Streifen dazwischen wird von keiner Fläche
/// gefüllt und ist im transparenten Fenster durchsichtig – eine Lücke zwischen zwei Bereichen. Darum läuft der Inhalt in
/// einem Kind-Ui, das seine Größe nicht weitergibt; das Panel belegt genau seine Fläche (Überstehendes wird abgeschnitten).
pub(super) fn fixed_body<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let area = ui.max_rect();
    let mut body = ui.new_child(egui::UiBuilder::new().id_salt("fixed_body").max_rect(area).layout(*ui.layout()));
    let r = add(&mut body);
    // Genau die verfügbare Fläche belegen – nicht mehr (Inhalt zu groß) und nicht weniger (Inhalt zu klein); ohne Widget,
    // damit nichts Klicks oder Hover des Inhalts abfängt.
    ui.advance_cursor_after_rect(area);
    r
}

/// Wurzel des Dateibaums (#277): immer der eingestellte Arbeitsordner der Sitzung (Sitzungseinstellungen bzw.
/// Startordner) – nie der Ordner, in den die Shell (oder Claude) inzwischen gewechselt ist.
/// * `cwd` = eingestellter Arbeitsordner (`SessionMeta::cwd`), `start` = erster gemeldeter Ordner der Sitzung,
/// * `wsl` = `Some(Distribution)` bei einer WSL-Sitzung (`Some(None)`: Distribution unbekannt), `home` = Windows-Home.
///
/// Linux-Pfade einer WSL-Sitzung werden zu `\\wsl.localhost\<Distro>\…` (`/mnt/c/…` zu `C:\…`); `~` und eine leere
/// Angabe bei WSL lassen sich ohne Rückfrage bei WSL nicht auflösen: dann der Startordner.
pub(super) fn files_root(cwd: &str, start: &str, wsl: Option<Option<&str>>, home: &str) -> String {
    let c = cwd.trim();
    let fallback = || if start.is_empty() { if wsl.is_some() { "~".to_string() } else { home.to_string() } } else { start.to_string() };
    if c.is_empty() {
        return if wsl.is_some() { fallback() } else { home.to_string() };
    }
    if crate::wsl::looks_linux(c) {
        return match wsl {
            Some(d) if c.starts_with('/') => crate::wsl::linux_to_windows(d.unwrap_or(""), c).unwrap_or_else(fallback),
            _ => fallback(),
        };
    }
    c.to_string()
}

/// Vergleichsschlüssel eines Pfads im Dateibaum: WSL-Pfade (`\\wsl.localhost\…`, `\\wsl$\…`) als Distribution und
/// Linux-Pfad, sonst der Windows-Pfad mit `\`, ohne Groß-/Kleinschreibung und ohne `\` am Ende.
pub(super) fn tree_key(p: &str) -> String {
    if let Some((d, l)) = crate::wsl::unc_to_linux(p) {
        // `/mnt/<lw>/…` ist ein Windows-Laufwerk: so vergleicht der gemeldete Ordner (`\\wsl$\D\mnt\c\x`) mit einer
        // Wurzel `C:\x` (aus `/mnt/c/x` im Arbeitsordner).
        if let Some(w) = crate::wsl::linux_to_windows("", &l) {
            return w.replace('/', "\\").trim_end_matches('\\').to_lowercase();
        }
        return format!("wsl:{}:{}", d.to_lowercase(), l.trim_end_matches('/'));
    }
    p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// Liegt `cur` (aktueller Ordner der Shell) unter `root`: die Schlüssel der Ordner von unterhalb der Wurzel bis `cur`
/// (zum Aufklappen und Markieren, #277). Sonst – auch bei `cur == root` – leer.
pub(super) fn tree_chain(root: &str, cur: &str) -> Vec<String> {
    if root.is_empty() || cur.is_empty() {
        return Vec::new();
    }
    let (r, c) = (tree_key(root), tree_key(cur));
    let sep = if r.starts_with("wsl:") { '/' } else { '\\' };
    let Some(rest) = c.strip_prefix(&r).and_then(|x| x.strip_prefix(sep)) else { return Vec::new() };
    let mut acc = r.clone();
    rest.split(sep)
        .filter(|s| !s.is_empty())
        .map(|part| {
            acc.push(sep);
            acc.push_str(part);
            acc.clone()
        })
        .collect()
}

/// Aktueller Ordner der Shell im Dateibaum (#277): Kette der Ordner bis dorthin (Schlüssel, [`tree_key`]) und ob sie in
/// diesem Bild aufgeklappt werden soll (nur, wenn sich der Ordner geändert hat – danach darf man wieder zuklappen).
#[derive(Default)]
pub(super) struct TreeMark {
    pub(super) chain: Vec<String>,
    pub(super) reveal: bool,
    /// Langsame Wurzel (Netzwerk, WSL): Ordner im Hilfsthread lesen ([`slow_list`]), nur Gelesenes aufklappen.
    pub(super) slow: bool,
}

/// Wurzel, deren Ordner nicht im UI-Thread gelesen werden dürfen (#277 Review): Netzwerkpfade und Netzlaufwerke
/// (`pathfmt::is_remote`) und WSL-Pfade (das Lesen kann die WSL-VM starten und blockieren).
pub(super) fn is_slow_root(root: &str) -> bool {
    crate::wsl::is_wsl_unc(root) || crate::pathfmt::is_remote(root)
}

/// Gelesener Inhalt eines langsamen Ordners (`items`: `None` = noch nie gelesen) und ob gerade gelesen wird.
struct SlowDir {
    items: Option<Vec<TreeItem>>,
    at: Instant,
    loading: bool,
}

/// Inhalte langsamer Ordner je (Ordner, versteckte zeigen) – wie `links::exists_cached` im Hilfsthread gelesen.
static SLOW_DIRS: std::sync::Mutex<Option<HashMap<(PathBuf, bool), SlowDir>>> = std::sync::Mutex::new(None);
/// So lange gilt ein gelesener Inhalt; danach wird im Hintergrund neu gelesen (der alte bleibt sichtbar).
const SLOW_TTL: Duration = Duration::from_secs(30);
const SLOW_PARALLEL: usize = 4;
const SLOW_MAX: usize = 400;

/// Inhalt eines langsamen Ordners ohne den UI-Thread zu blockieren: `None`, solange er zum ersten Mal gelesen wird
/// (danach zeichnet `ctx` neu). Ein abgelaufener Inhalt wird im Hintergrund neu gelesen.
pub(super) fn slow_list(ctx: &egui::Context, dir: &Path, hidden: bool) -> Option<Vec<TreeItem>> {
    let key = (dir.to_path_buf(), hidden);
    let mut g = SLOW_DIRS.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(HashMap::new);
    let stale = match map.get(&key) {
        Some(d) if d.loading => return d.items.clone(),
        Some(d) if d.at.elapsed() < SLOW_TTL => return d.items.clone(),
        Some(_) => true,
        None => false,
    };
    if map.values().filter(|d| d.loading).count() >= SLOW_PARALLEL {
        return map.get(&key).and_then(|d| d.items.clone());
    }
    if !stale && map.len() >= SLOW_MAX {
        map.retain(|_, d| d.loading);
    }
    let old = map.remove(&key).and_then(|d| d.items);
    map.insert(key.clone(), SlowDir { items: old.clone(), at: Instant::now(), loading: true });
    let ctx = ctx.clone();
    let k = key.clone();
    let spawned = std::thread::Builder::new().name("tree-list".into()).spawn(move || {
        let items = list_dir(&k.0, k.1);
        let mut g = SLOW_DIRS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(d) = g.get_or_insert_with(HashMap::new).get_mut(&k) {
            *d = SlowDir { items: Some(items), at: Instant::now(), loading: false };
        }
        drop(g);
        ctx.request_repaint();
    });
    if spawned.is_err() {
        map.remove(&key);
    }
    old
}

/// Ist der Inhalt eines langsamen Ordners schon gelesen (dann darf er ohne Warten aufgeklappt werden)?
pub(super) fn slow_cached(dir: &Path, hidden: bool) -> bool {
    let g = SLOW_DIRS.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().and_then(|m| m.get(&(dir.to_path_buf(), hidden))).is_some_and(|d| d.items.is_some())
}

/// „Aktualisieren“: gelesene Inhalte langsamer Ordner vergessen (laufende Lesevorgänge bleiben).
pub(super) fn slow_clear() {
    let mut g = SLOW_DIRS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(m) = g.as_mut() {
        m.retain(|_, d| d.loading);
    }
}

/// Warum der Dateibaum nichts liest (#277 Review): Wurzel unbekannt (WSL ohne Ordner, noch keine Meldung) oder WSL-Wurzel
/// einer Sitzung, die nicht läuft (das Lesen könnte die WSL-VM starten).
pub(super) fn files_blocked(root: &str, running: bool, lang: Lang) -> Option<&'static str> {
    if !Path::new(root).is_absolute() && !crate::wsl::is_wsl_unc(root) {
        return Some(lang.t(
            "Der Arbeitsordner der Sitzung ist noch nicht bekannt (WSL ohne Ordner): Er erscheint, sobald die Shell ihren Ordner meldet.",
            "The session's working folder is not known yet (WSL without a folder): it appears once the shell reports its folder.",
        ));
    }
    if crate::wsl::is_wsl_unc(root) && !running {
        return Some(lang.t(
            "Die WSL-Sitzung läuft nicht – ihre Dateien werden gezeigt, sobald sie läuft.",
            "The WSL session is not running – its files are shown once it runs.",
        ));
    }
    None
}

impl TreeMark {
    /// Rolle eines Ordners: (liegt auf dem Weg zum aktuellen Ordner, ist der aktuelle Ordner).
    pub(super) fn role(&self, path: &Path) -> (bool, bool) {
        if self.chain.is_empty() {
            return (false, false);
        }
        let k = tree_key(&path.to_string_lossy());
        match self.chain.iter().position(|c| *c == k) {
            Some(i) => (true, i + 1 == self.chain.len()),
            None => (false, false),
        }
    }
}

impl App {
    /// Wurzel des Dateibaums für die Sitzung `id` ([`files_root`], #277).
    pub(super) fn files_root_of(&self, id: u64) -> String {
        let Some(e) = self.entries.get(&id) else { return home_dir() };
        let shell = self.shell_by_id(e.eff_shell());
        let wsl = match folder_picker::Target::for_shell(shell.as_ref(), false) {
            folder_picker::Target::Wsl { distro, .. } => Some(distro),
            _ if e.eff_shell().starts_with("wsl") => Some(e.eff_shell().strip_prefix("wsl:").filter(|d| !d.is_empty()).map(str::to_string)),
            _ => None,
        };
        let wsl = if e.is_chat() { None } else { wsl };
        files_root(&e.meta.cwd, &e.start_cwd, wsl.as_ref().map(|d| d.as_deref()), &home_dir())
    }

    pub(super) fn files_ui(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let lang = self.settings.language;
        // Wurzel = Arbeitsordner der Sitzung, nicht ihr aktueller Ordner (#277); der wird darin aufgeklappt und markiert.
        let root = self.files_root_of(self.focused);
        let cur = self.entries.get(&self.focused).map(|e| e.live_cwd.clone()).unwrap_or_default();
        if root != self.tree_root || self.tree_stamp.elapsed() > Duration::from_secs(10) {
            self.tree_cache.clear();
            self.tree_root = root.clone();
            self.tree_stamp = Instant::now();
        }
        let hidden = self.files_hidden;
        let running = self.entries.get(&self.focused).and_then(|e| e.live.as_ref()).is_some_and(|s| !s.has_exited());
        let open = self.files_open;
        egui::SidePanel::right(style::salted(Id::new("files"))).resizable(true).default_width(270.0).width_range(180.0..=520.0).show_animated(ctx, open, |ui| fixed_body(ui, |ui| {
            let blocked = files_blocked(&root, running, lang);
            let slow = blocked.is_none() && is_slow_root(&root);
            let chain = tree_chain(&root, &cur);
            // Aufklappen nur, wenn sich Wurzel oder aktueller Ordner geändert haben (gemerkt je Panel, also je Fenster).
            let seen_id = ui.id().with("files_cur");
            let now = format!("{root}\n{cur}");
            let reveal = ui.data(|d| d.get_temp::<String>(seen_id)).as_deref() != Some(now.as_str());
            if reveal {
                ui.data_mut(|d| d.insert_temp(seen_id, now));
            }
            let outside = !cur.is_empty() && chain.is_empty() && tree_key(&cur) != tree_key(&root);
            let mark = TreeMark { chain, reveal, slow };
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.strong(lang.t("Dateien", "Files"));
                if ui.small_button(lang.t("Aktualisieren", "Refresh")).clicked() {
                    self.tree_cache.clear();
                    slow_clear();
                }
                if ui.checkbox(&mut self.files_hidden, lang.t("versteckte", "hidden")).changed() {
                    self.tree_cache.clear();
                }
            });
            ui.add(egui::Label::new(RichText::new(&root).weak().small()).truncate()).on_hover_text(lang.t("Arbeitsordner der Sitzung", "Working folder of the session"));
            if outside {
                // Die Shell ist außerhalb des Arbeitsordners: die Wurzel bleibt, der Ordner steht als Hinweis da.
                ui.add(egui::Label::new(RichText::new(format!("{} {cur}", lang.t("Shell ist in:", "Shell is in:"))).weak().small()).truncate());
            }
            ui.weak(lang.t("Doppelklick fügt den Pfad ins Terminal ein · Rechtsklick für mehr", "Double-click inserts the path · right-click for more"));
            ui.separator();
            if let Some(why) = blocked {
                ui.label(RichText::new(why).small().color(style::dim(ui)));
                return;
            }
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                tree_children(ui, Path::new(&root), hidden, &mut self.tree_cache, actions, lang, &mark);
            });
        }));
    }

    pub(super) fn queue_ui(&mut self, ctx: &egui::Context) {
        let lang = self.settings.language;
        let Some(id) = self.queue_open else { return };
        if !self.entries.contains_key(&id) {
            self.queue_open = None;
            return;
        }
        let mut open = true;
        let title = format!("{} – {}", lang.t("Warteschlange", "Queue"), self.entries[&id].display_name());
        let mut add: Option<String> = None;
        style::movable(ctx, Id::new("queue_win"), egui::Window::new(title)).open(&mut open).default_size([520.0, 420.0]).show(ctx, |ui| {
            let Some(e) = self.entries.get_mut(&id) else { return };
            ui.label(RichText::new(lang.t(
                "Die Prompts werden nacheinander gesendet: der nächste, sobald Claude „bereit“ meldet. Bei Freigabe-Fragen wartet die Warteschlange – ebenso nach einer Unterbrechung (Esc) oder einem Fehler, bis die nächste Antwort fertig ist oder du „Nächsten jetzt senden“ wählst.",
                "Prompts are sent one after another: the next as soon as Claude reports “ready”. It waits during approval requests – and after an interruption (Esc) or an error, until the next answer is finished or you choose “Send next now”.",
            )).small().color(style::dim(ui)));
            ui.add_space(S2);
            ui.horizontal(|ui| {
                let label = if e.meta.queue_paused { lang.t("Fortsetzen", "Resume") } else { lang.t("Pausieren", "Pause") };
                if ui.button(label).clicked() {
                    e.meta.queue_paused = !e.meta.queue_paused;
                    if !e.meta.queue_paused && e.status == ClaudeStatus::Idle {
                        e.send_next_queued();
                    }
                }
                if ui.add_enabled(!e.meta.queue.is_empty() && e.status == ClaudeStatus::Idle, egui::Button::new(lang.t("Nächsten jetzt senden", "Send next now"))).clicked() {
                    e.send_next_queued();
                }
                if ui.add_enabled(!e.meta.queue.is_empty(), egui::Button::new(lang.t("Alle löschen", "Clear all"))).clicked() {
                    e.meta.queue.clear();
                }
                if !e.is_claude() {
                    ui.colored_label(Color32::from_rgb(0xe5, 0xc0, 0x7b), lang.t("Keine Claude-Sitzung – die Warteschlange läuft nur bei Claude.", "Not a Claude session – the queue only runs with Claude."));
                }
            });
            ui.separator();
            egui::ScrollArea::vertical().max_height(230.0).auto_shrink([false, true]).show(ui, |ui| {
                let n = e.meta.queue.len();
                let mut remove: Option<usize> = None;
                let mut swap: Option<(usize, usize)> = None;
                for i in 0..n {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{}", i + 1)).color(style::dim(ui)));
                        ui.add(egui::TextEdit::multiline(&mut e.meta.queue[i]).desired_rows(1).desired_width(330.0));
                        if i > 0 && style::icon_btn_small(ui, SIcon::Up, lang.t("Nach oben", "Move up")).clicked() {
                            swap = Some((i, i - 1));
                        }
                        if i + 1 < n && style::icon_btn_small(ui, SIcon::Down, lang.t("Nach unten", "Move down")).clicked() {
                            swap = Some((i, i + 1));
                        }
                        if style::icon_btn_small(ui, SIcon::Close, lang.t("Entfernen", "Remove")).clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if n == 0 {
                    ui.label(RichText::new(lang.t("Die Warteschlange ist leer.", "The queue is empty.")).color(style::dim(ui)));
                }
                if let Some((a, b)) = swap {
                    e.meta.queue.swap(a, b);
                }
                if let Some(i) = remove {
                    e.meta.queue.remove(i);
                }
            });
            ui.separator();
            ui.label(lang.t("Neuer Prompt", "New prompt"));
            ui.add(egui::TextEdit::multiline(&mut self.queue_draft).desired_rows(3).desired_width(f32::INFINITY).hint_text(lang.t("Prompt eingeben …", "Enter prompt …")));
            ui.horizontal(|ui| {
                if ui.add_enabled(!self.queue_draft.trim().is_empty(), egui::Button::new(lang.t("Zur Warteschlange hinzufügen", "Add to queue"))).clicked() {
                    add = Some(std::mem::take(&mut self.queue_draft));
                }
            });
        });
        if let Some(t) = add {
            self.pending_actions.push(Action::QueueAdd(id, t));
        }
        if !open {
            self.queue_open = None;
        }
    }

    /// Karten für Sitzungen, die auf eine Freigabe warten (Seitenleiste).
    pub(super) fn inbox_ui(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let lang = self.settings.language;
        let waiting: Vec<(u64, String, String, bool)> = self
            .entries
            .iter()
            .filter(|(_, e)| e.needs_you() && e.is_claude())
            .map(|(id, e)| (*id, e.display_name(), e.status_msg.clone(), e.status == ClaudeStatus::Permission))
            .collect();
        if waiting.is_empty() {
            return;
        }
        egui::Frame::new()
            .fill(style::ACCENT.gamma_multiply(0.12))
            .stroke(Stroke::new(1.0_f32, style::ACCENT.gamma_multiply(0.5)))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::same(8))
            .show(ui, |ui| {
                ui.label(RichText::new(format!("{} ({})", lang.t("Wartet auf dich", "Waiting for you"), waiting.len())).strong().small());
                for (id, name, msg, permission) in waiting {
                    ui.add_space(S1);
                    ui.label(RichText::new(name).strong());
                    if !msg.is_empty() {
                        ui.add(egui::Label::new(RichText::new(msg).small().color(style::dim(ui))).wrap());
                    }
                    ui.horizontal(|ui| {
                        if permission {
                            if ui.add(egui::Button::new(RichText::new(lang.t("Ja", "Yes")).strong()).fill(Color32::from_rgb(0x2f, 0x7d, 0x4a))).on_hover_text(lang.t("Sendet Enter (Standardauswahl „Ja“)", "Sends Enter (default choice “Yes”)")).clicked() {
                                actions.push(Action::Approve(id, true));
                            }
                            if ui.button(lang.t("Nein", "No")).on_hover_text(lang.t("Sendet Esc", "Sends Esc")).clicked() {
                                actions.push(Action::Approve(id, false));
                            }
                        }
                        if ui.button(lang.t("Öffnen", "Open")).clicked() {
                            actions.push(Action::Activate(id));
                        }
                    });
                }
            });
        ui.add_space(S2);
    }

    pub(super) fn diff_ui(&mut self, ctx: &egui::Context) {
        let lang = self.settings.language;
        if !self.diff_open {
            self.diff.set_target(None, None);
            return;
        }
        let dir = self.entries.get(&self.focused).map(|e| e.cwd_now()).unwrap_or_else(home_dir);
        let snap = self.diff.snapshot();
        let sel_change: Option<(Change, bool)> = self
            .diff_sel
            .as_ref()
            .and_then(|(p, st)| snap.changes.iter().find(|c| &c.path == p).map(|c| (c.clone(), *st)));
        self.diff.set_target(Some(PathBuf::from(&dir)), sel_change.clone());
        let esc_close = ctx.input(|i| i.key_pressed(Key::Escape));
        let mut open = true;
        let mut toggle: Option<(Change, bool)> = None; // (Datei, jetzt stagen?)
        let mut do_stage_all = false;
        let mut do_commit = false;
        let mut select: Option<(String, bool)> = None;
        style::movable(ctx, Id::new("diff_win"), egui::Window::new(lang.t("Änderungen", "Changes"))).open(&mut open).default_size([960.0, 540.0]).show(ctx, |ui| {
            if let Some(err) = &snap.error {
                ui.colored_label(Color32::from_rgb(0xe5, 0xc0, 0x7b), err);
                ui.label(RichText::new(&dir).small().color(style::dim(ui)));
                return;
            }
            let staged_n = snap.changes.iter().filter(|c| c.staged()).count();
            ui.horizontal(|ui| {
                ui.label(RichText::new(snap.repo.as_ref().map(|r| r.display().to_string()).unwrap_or_default()).small().color(style::dim(ui)));
                ui.label(RichText::new(format!("git: {}", snap.branch)).strong());
                ui.label(RichText::new(format!("{} {} · {} {}", snap.changes.len(), lang.t("geändert", "changed"), staged_n, lang.t("vorgemerkt", "staged"))).small().color(style::dim(ui)));
            });
            ui.separator();
            ui.scope(|ui| {
            ui.set_max_height(340.0);
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(300.0);
                    egui::ScrollArea::vertical().max_height(320.0).id_salt("difflist").show(ui, |ui| {
                        if snap.changes.is_empty() {
                            ui.label(RichText::new(lang.t("Keine Änderungen.", "No changes.")).color(style::dim(ui)));
                        }
                        for c in &snap.changes {
                            ui.horizontal(|ui| {
                                let mut staged = c.staged() && !c.unstaged();
                                if ui.checkbox(&mut staged, "").on_hover_text(lang.t("Vormerken (Stage) / zurücknehmen", "Stage / unstage")).changed() {
                                    toggle = Some((c.clone(), staged));
                                }
                                let (letter, col) = match (c.x, c.y) {
                                    ('?', _) => ('?', Color32::from_rgb(0x98, 0xc3, 0x79)),
                                    ('A', _) | (_, 'A') => ('A', Color32::from_rgb(0x98, 0xc3, 0x79)),
                                    ('D', _) | (_, 'D') => ('D', Color32::from_rgb(0xe0, 0x6c, 0x75)),
                                    ('R', _) => ('R', Color32::from_rgb(0x61, 0xaf, 0xef)),
                                    _ => ('M', Color32::from_rgb(0xe5, 0xc0, 0x7b)),
                                };
                                ui.label(RichText::new(letter.to_string()).monospace().color(col));
                                let is_sel = self.diff_sel.as_ref().is_some_and(|(p, _)| *p == c.path);
                                let both = c.staged() && c.unstaged();
                                let r = ui.selectable_label(is_sel, RichText::new(&c.path).small());
                                if r.clicked() {
                                    select = Some((c.path.clone(), c.staged() && !c.unstaged()));
                                }
                                if both {
                                    ui.label(RichText::new(lang.t("teils", "part")).small().color(style::dim(ui)));
                                }
                            });
                        }
                    });
                });
                ui.separator();
                ui.vertical(|ui| {
                    if let Some((c, staged_view)) = &sel_change {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&c.path).strong());
                            if c.staged() && c.unstaged() {
                                if afford::choice(ui, !*staged_view, lang.t("Nicht vorgemerkt", "Unstaged"), 0.0).clicked() {
                                    select = Some((c.path.clone(), false));
                                }
                                if afford::choice(ui, *staged_view, lang.t("Vorgemerkt", "Staged"), 0.0).clicked() {
                                    select = Some((c.path.clone(), true));
                                }
                            }
                        });
                    }
                    let lines: Vec<&str> = snap.diff.lines().collect();
                    let row_h = 15.0;
                    egui::ScrollArea::both().max_height(320.0).auto_shrink([false, false]).id_salt("diffview").show_rows(ui, row_h, lines.len(), |ui, range| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for l in &lines[range] {
                            let col = if l.starts_with('+') && !l.starts_with("+++") {
                                Color32::from_rgb(0x98, 0xc3, 0x79)
                            } else if l.starts_with('-') && !l.starts_with("---") {
                                Color32::from_rgb(0xe0, 0x6c, 0x75)
                            } else if l.starts_with("@@") {
                                Color32::from_rgb(0x61, 0xaf, 0xef)
                            } else {
                                style::dim(ui)
                            };
                            ui.add(egui::Label::new(RichText::new(*l).monospace().size(12.0).color(col)).extend());
                        }
                    });
                    if sel_change.is_none() {
                        ui.label(RichText::new(lang.t("Datei links auswählen, um den Diff zu sehen.", "Select a file on the left to see its diff.")).color(style::dim(ui)));
                    }
                });
            });
            });
            ui.separator();
            ui.add(egui::TextEdit::multiline(&mut self.commit_msg).desired_rows(2).desired_width(f32::INFINITY).hint_text(lang.t("Commit-Nachricht …", "Commit message …")));
            ui.horizontal(|ui| {
                if ui.add_enabled(!snap.changes.is_empty(), egui::Button::new(lang.t("Alle vormerken", "Stage all"))).clicked() {
                    do_stage_all = true;
                }
                if ui.add_enabled(staged_n > 0 && !self.commit_msg.trim().is_empty(), egui::Button::new(RichText::new(format!("{} ({staged_n})", lang.t("Committen", "Commit"))).strong())).clicked() {
                    do_commit = true;
                }
                if ui.button(lang.t("Aktualisieren", "Refresh")).clicked() {
                    self.diff.refresh();
                }
                ui.label(RichText::new(lang.t("Kein Push – das übernimmst du im Terminal.", "No push – do that in the terminal.")).small().color(style::dim(ui)));
            });
        });
        if let Some(s) = select {
            self.diff_sel = Some(s);
            self.diff.refresh();
        }
        let repo = snap.repo.clone();
        if let (Some(repo), Some((c, stage_it))) = (repo.clone(), toggle) {
            let p = c.path.clone();
            self.git_bg(ctx, "", move || if stage_it { gitops::stage(&repo, &p).map(|_| String::new()) } else { gitops::unstage(&repo, &p).map(|_| String::new()) });
        }
        if let (Some(repo), true) = (repo.clone(), do_stage_all) {
            self.git_bg(ctx, lang.t("Alles vorgemerkt", "All staged"), move || gitops::stage_all(&repo).map(|_| String::new()));
        }
        if let (Some(repo), true) = (repo, do_commit) {
            // Nachricht erst nach erfolgreichem Commit leeren (Hook abgelehnt, nichts vorgemerkt … → bleibt stehen).
            let msg = self.commit_msg.clone();
            let done = self.commit_ok.clone();
            self.git_bg(ctx, lang.t("Commit erstellt", "Commit created"), move || {
                let r = gitops::commit(&repo, msg.trim());
                if r.is_ok() {
                    done.store(true, std::sync::atomic::Ordering::SeqCst);
                }
                r
            });
        }
        if !open || (esc_close && !ctx.wants_keyboard_input()) {
            self.diff_open = false;
        }
    }

    pub(super) fn costs_ui(&mut self, ctx: &egui::Context) {
        if !self.cost_open {
            return;
        }
        let lang = self.settings.language;
        let mut open = true;
        // Daten sammeln
        let mut per_session: Vec<(u64, String, f64, u64)> = Vec::new();
        let mut day_cost: std::collections::BTreeMap<String, f64> = Default::default();
        for (id, e) in &self.entries {
            if let Some(u) = self.usage.get(*id) {
                let tok = u.total.input + u.total.output + u.total.cache_read + u.total.cache_write;
                per_session.push((*id, e.display_name(), u.cost(), tok));
                for (d, c) in u.cost_by_day() {
                    *day_cost.entry(d).or_default() += c;
                }
            }
        }
        let now = usage::today_secs();
        let today = usage::utc_date(now);
        let days: Vec<(String, f64)> = (0..14).rev().map(|i| { let d = usage::utc_date(now.saturating_sub(i * 86400)); let c = day_cost.get(&d).copied().unwrap_or(0.0); (d, c) }).collect();
        let week: f64 = days.iter().rev().take(7).map(|d| d.1).sum::<f64>() + 0.0;
        let total: f64 = per_session.iter().map(|s| s.2).sum::<f64>() + 0.0;
        let today_c = days.last().map(|d| d.1).unwrap_or(0.0);
        let budget = self.settings.daily_budget;
        style::movable(ctx, Id::new("cost_win"), egui::Window::new(lang.t("Kosten-Übersicht", "Cost overview"))).open(&mut open).default_size([560.0, 560.0]).show(ctx, |ui| {
            ui.label(RichText::new(lang.t("Geschätzt aus den lokalen Transkripten der Sitzungen in Paneldeck (nur Sitzungen, die hier geöffnet sind). Tage in UTC.", "Estimated from the local transcripts of sessions in Paneldeck (only sessions open here). Days in UTC.")).small().color(style::dim(ui)));
            ui.add_space(S1);
            ui.horizontal(|ui| {
                for (label, v) in [(lang.t("Heute", "Today"), today_c), (lang.t("7 Tage", "7 days"), week), (lang.t("Gesamt", "Total"), total)] {
                    ui.vertical(|ui| {
                        ui.label(RichText::new(format!("≈ ${v:.2}")).heading());
                        ui.label(RichText::new(label).small().color(style::dim(ui)));
                    });
                    ui.add_space(S3);
                }
            });
            if budget > 0.0 {
                let frac = (today_c / budget).clamp(0.0, 1.0) as f32;
                ui.add(egui::ProgressBar::new(frac).text(format!("{} ${:.2} / ${:.2}", lang.t("Tagesbudget", "Daily budget"), today_c, budget)).fill(if today_c > budget { Color32::from_rgb(0xe0, 0x4f, 0x5f) } else { style::ACCENT }));
            }
            ui.add_space(S2);
            ui.label(RichText::new(lang.t("Letzte 14 Tage", "Last 14 days")).strong());
            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 110.0), Sense::hover());
            let p = ui.painter();
            let max = days.iter().map(|d| d.1).fold(0.0001_f64, f64::max);
            let bw = rect.width() / days.len() as f32;
            let weak = style::dim(ui);
            for (i, (d, c)) in days.iter().enumerate() {
                let h = ((*c / max) as f32 * (rect.height() - 34.0)).max(if *c > 0.0 { 2.0 } else { 0.0 });
                let x0 = rect.min.x + i as f32 * bw;
                let bar = Rect::from_min_max(Pos2::new(x0 + bw * 0.18, rect.max.y - 16.0 - h), Pos2::new(x0 + bw * 0.82, rect.max.y - 16.0));
                let col = if *d == today { style::ACCENT } else { style::ACCENT.gamma_multiply(0.55) };
                p.rect_filled(bar, 2.0, col);
                if *c > 0.0 {
                    p.text(Pos2::new(bar.center().x, bar.min.y - 2.0), Align2::CENTER_BOTTOM, format!("{c:.2}"), FontId::proportional(9.5), weak);
                }
                p.text(Pos2::new(bar.center().x, rect.max.y - 2.0), Align2::CENTER_BOTTOM, &d[5..], FontId::proportional(9.5), weak);
            }
            ui.add_space(S2);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                // Nach Projekt
                ui.label(RichText::new(lang.t("Nach Projekt", "By project")).strong());
                for pr in &self.projects {
                    let c: f64 = per_session.iter().filter(|s| self.entries.get(&s.0).is_some_and(|e| e.meta.project == pr.id)).map(|s| s.2).sum::<f64>() + 0.0;
                    ui.horizontal(|ui| {
                        ui.label(&pr.name);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(format!("≈ ${c:.2}")));
                    });
                }
                // Nach Ordner (aktives Projekt)
                if let Some(pr) = self.projects.iter().find(|p| p.id == self.active_project) {
                    if !pr.folders.is_empty() {
                        ui.add_space(S2);
                        ui.label(RichText::new(format!("{} {}", lang.t("Nach Ordner in", "By folder in"), lang.q(&pr.name))).strong());
                        let mut fs: Vec<&state::Folder> = pr.folders.iter().collect();
                        fs.sort_by_key(|f| f.path.to_lowercase());
                        for f in fs {
                            let c: f64 = per_session.iter().filter(|s| self.entries.get(&s.0).is_some_and(|e| e.meta.project == pr.id && !e.meta.group.is_empty() && state::folder_within(&e.meta.group, &f.path))).map(|s| s.2).sum::<f64>() + 0.0;
                            ui.horizontal(|ui| {
                                ui.add_space(f.path.matches('/').count() as f32 * 14.0);
                                ui.label(state::folder_leaf(&f.path));
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(format!("≈ ${c:.2}")));
                            });
                        }
                    }
                }
                // Nach Sitzung
                ui.add_space(S2);
                ui.label(RichText::new(lang.t("Teuerste Sitzungen", "Most expensive sessions")).strong());
                per_session.sort_by(|a, b| b.2.total_cmp(&a.2));
                for (_, name, c, tok) in per_session.iter().take(15) {
                    ui.horizontal(|ui| {
                        ui.add(egui::Label::new(name).truncate());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(format!("≈ ${c:.2} · {} {}", fmt_tokens(*tok), lang.t("Token", "tokens")));
                        });
                    });
                }
            });
        });
        self.cost_open = open && !ctx.input(|i| i.key_pressed(Key::Escape));
    }

    pub(super) fn history_ui(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        if !self.history_open {
            return;
        }
        let lang = self.settings.language;
        let mut open = true;
        style::movable(ctx, Id::new("history_win"), egui::Window::new(lang.t("Claude-Verlauf", "Claude history")))
            .open(&mut open)
            .default_size([680.0, 460.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let te = ui.add(egui::TextEdit::singleline(&mut self.history_filter).hint_text(lang.t("Filtern (Prompt oder Ordner) …", "Filter (prompt or folder) …")).desired_width(300.0));
                    let enter = te.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    let can = self.history_filter.trim().chars().count() >= 2 && self.history_hits_rx.is_none();
                    if ui.add_enabled(can, egui::Button::new(lang.t("Volltext", "Full text"))).on_hover_text(lang.t("Durchsucht alle Konversationen (Enter)", "Searches all conversations (Enter)")).clicked() || (enter && can) {
                        self.history_hits_rx = Some(history::search_async(ctx.clone(), self.history_filter.clone()));
                        self.history_hits = None;
                    }
                    if (self.history_hits.is_some() || self.history_hits_rx.is_some()) && ui.button("✕").on_hover_text(lang.t("Suche beenden", "Clear search")).clicked() {
                        self.history_hits = None;
                        self.history_hits_rx = None;
                    }
                    if ui.button(lang.t("Neu laden", "Reload")).clicked() {
                        self.history_rx = Some(history::scan_async(ctx.clone()));
                    }
                    if self.history_rx.is_some() || self.history_hits_rx.is_some() {
                        ui.spinner();
                    }
                });
                ui.separator();
                if let Some(hits) = &self.history_hits {
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        ui.weak(format!("{} {}", hits.len(), lang.t("Treffer im Volltext", "full-text matches")));
                        if hits.is_empty() {
                            ui.weak(lang.t("Nichts gefunden.", "Nothing found."));
                        }
                        for (h, sn) in hits {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.button(lang.t("Fortsetzen", "Resume")).clicked() {
                                    actions.push(Action::ResumeItem(h.clone()));
                                }
                                ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                                    ui.add(egui::Label::new(RichText::new(&h.title).strong()).truncate());
                                    ui.add(egui::Label::new(RichText::new(sn).small()).truncate());
                                    ui.weak(format!("{} · {}", h.cwd, history::age_label(h.modified, lang == Lang::De)));
                                });
                            });
                            ui.separator();
                        }
                    });
                    return;
                }
                let f = self.history_filter.to_lowercase();
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    if self.history.is_empty() && self.history_rx.is_none() {
                        ui.weak(lang.t("Keine Konversationen gefunden (~/.claude/projects).", "No conversations found (~/.claude/projects)."));
                    }
                    for (i, h) in self.history.iter().enumerate() {
                        if !f.is_empty() && !h.title.to_lowercase().contains(&f) && !h.cwd.to_lowercase().contains(&f) {
                            continue;
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button(lang.t("Fortsetzen", "Resume")).clicked() {
                                actions.push(Action::ResumeHistory(i));
                            }
                            ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                                ui.add(egui::Label::new(RichText::new(&h.title).strong()).truncate());
                                ui.weak(format!("{} · {}", h.cwd, history::age_label(h.modified, lang == Lang::De)));
                            });
                        });
                        ui.separator();
                    }
                });
            });
        self.history_open = open;
    }

    pub(super) fn gsearch_ui(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let lang = self.settings.language;
        let Some(mut g) = self.gsearch.take() else { return };
        let now = ctx.input(|i| i.time);
        let mut open = !ctx.input(|i| i.key_pressed(Key::Escape));
        let mut jump: Option<(u64, i32)> = None;
        style::movable(ctx, Id::new("gsearch_win"), egui::Window::new(lang.t("Alle Sitzungen durchsuchen", "Search all sessions")))
            .open(&mut open)
            .default_size([640.0, 440.0])
            .show(ctx, |ui| {
                let te = ui.add(egui::TextEdit::singleline(&mut g.query).desired_width(f32::INFINITY).hint_text(lang.t("Suchbegriff (mind. 2 Zeichen) …", "Search term (min. 2 characters) …")));
                if !g.opened {
                    te.request_focus();
                    g.opened = true;
                }
                if te.changed() {
                    g.last_edit = now;
                    ctx.request_repaint_after(Duration::from_millis(350));
                }
                // Entprellt suchen
                if g.query != g.done_for && (now - g.last_edit > 0.3 || ctx.input(|i| i.key_pressed(Key::Enter))) {
                    g.done_for = g.query.clone();
                    g.hits.clear();
                    if g.query.trim().chars().count() >= 2 {
                        for (id, e) in &self.entries {
                            let Some(s) = e.live.as_ref() else { continue };
                            let t = s.term.lock().unwrap_or_else(|e| e.into_inner());
                            for m in search::find_all(&t, g.query.trim()).into_iter().rev().take(40) {
                                g.hits.push(GsHit { session: *id, line: m.line, text: search::line_text(&t, m.line) });
                            }
                        }
                    }
                }
                ui.add_space(S1);
                ui.label(RichText::new(format!("{} {}", g.hits.len(), lang.t("Treffer", "hits"))).small().color(style::dim(ui)));
                ui.separator();
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    let mut last = 0u64;
                    for h in &g.hits {
                        if h.session != last {
                            last = h.session;
                            let name = self.entries.get(&h.session).map(|e| e.display_name()).unwrap_or_default();
                            ui.add_space(S1);
                            ui.label(RichText::new(name).strong());
                        }
                        let r = ui.add(egui::Button::new(RichText::new(h.text.trim()).monospace().small()).frame(false).wrap_mode(egui::TextWrapMode::Truncate));
                        if r.clicked() {
                            jump = Some((h.session, h.line));
                        }
                    }
                });
            });
        if let Some((sid, line)) = jump {
            actions.push(Action::GlobalJump(sid, line, g.query.trim().to_string()));
        }
        if open {
            self.gsearch = Some(g);
        }
    }

    pub(super) fn palette_items(&self) -> Vec<(String, String, Action)> {
        let lang = self.settings.language;
        let f = self.focused;
        let mut v: Vec<(String, String, Action)> = vec![
            (lang.t("Neue Sitzung (Standard-Shell)", "New session (default shell)").into(), self.binding_text("new"), Action::NewQuick),
            (lang.t("Neue Sitzung …", "New session …").into(), String::new(), Action::NewDialog(false)),
        ];
        if self.settings.claude_mode {
            v.push((lang.t("Neue Claude-Sitzung …", "New Claude session …").into(), String::new(), Action::NewDialog(true)));
        }
        v.push((lang.t("Neuer Chat", "New chat").into(), String::new(), Action::NewChat));
        v.push((lang.t("Vorlagen …", "Templates …").into(), String::new(), Action::OpenTemplates));
        for (i, t) in self.templates.iter().enumerate() {
            v.push((format!("{} {}", lang.t("Vorlage starten:", "Start template:"), t.name), String::new(), Action::TemplateStart(i)));
        }
        if self.settings.claude_mode {
            v.push((lang.t("Kosten-Übersicht", "Cost overview").into(), String::new(), Action::OpenCosts));
        }
        v.extend([
            (lang.t("Bereich rechts teilen", "Split pane right").into(), self.binding_text("split_r"), Action::Split(f, Dir::Horizontal)),
            (lang.t("Bereich darunter teilen", "Split pane below").into(), self.binding_text("split_d"), Action::Split(f, Dir::Vertical)),
            (lang.t("Bereich minimieren", "Minimize pane").into(), self.binding_text("hide"), Action::MinimizePane(f)),
            (lang.t("Sitzung umbenennen", "Rename session").into(), self.binding_text("rename"), Action::StartRename(f)),
            (lang.t("Einstellungen der Sitzung …", "Session settings …").into(), String::new(), Action::OpenSessionSettings(f)),
            (lang.t("Sitzung schließen", "Close session").into(), String::new(), Action::RequestClose(f)),
            (lang.t("Alle Sitzungen des Projekts schließen …", "Close all sessions of the project …").into(), String::new(), Action::CloseAll(closing::CloseScope::Project(self.active_project))),
            (lang.t("Alle Sitzungen schließen … (alle Projekte und Fenster)", "Close all sessions … (all projects and windows)").into(), self.binding_text("close_all"), Action::CloseAll(closing::CloseScope::All)),
            (lang.t("Alle Sitzungen durchsuchen", "Search all sessions").into(), self.binding_text("search_all"), Action::OpenGlobalSearch),
            (lang.t("Verlauf als Datei speichern", "Save history to file").into(), String::new(), Action::ExportHistory(f)),
            (lang.t("Zum vorherigen Befehl springen", "Jump to previous command").into(), self.binding_text("prev_cmd"), Action::JumpCmd(-1)),
            (lang.t("Zum nächsten Befehl springen", "Jump to next command").into(), self.binding_text("next_cmd"), Action::JumpCmd(1)),
            (lang.t("Im Verlauf suchen", "Search scrollback").into(), self.binding_text("search"), Action::OpenSearch),
            (lang.t("Broadcast an/aus", "Toggle broadcast").into(), self.binding_text("broadcast"), Action::ToggleBroadcast),
            (lang.t("Prompt-Composer", "Prompt composer").into(), self.binding_text("composer"), Action::ToggleComposer),
            (lang.t("Warteschlange der Sitzung", "Session queue").into(), self.binding_text("queue"), Action::OpenQueue(f)),
            (lang.t("Änderungen (Diff) ein/aus", "Toggle changes (diff)").into(), self.binding_text("diff"), Action::ToggleDiff),
            (lang.t("Notizen der Sitzung", "Session notes").into(), self.binding_text("notes"), Action::OpenNotes(f)),
            (lang.t("Prompt-Bibliothek", "Prompt library").into(), String::new(), Action::OpenPrompts),
            (lang.t("Sitzung in eigenem Fenster öffnen", "Open session in own window").into(), String::new(), Action::Detach(f)),
            (lang.t("CLAUDE.md / Claude-Dateien bearbeiten", "Edit CLAUDE.md / Claude files").into(), self.binding_text("editor"), Action::OpenEditor),
            (lang.t("Dateibaum ein/aus", "Toggle file tree").into(), self.binding_text("files"), Action::ToggleFiles),
            (lang.t("Claude-Verlauf (Konversationen fortsetzen)", "Claude history (resume conversations)").into(), String::new(), Action::OpenHistory),
            (lang.t("Einstellungen", "Settings").into(), String::new(), Action::OpenSettings),
            (lang.t("Aufräumen …", "Clean up …").into(), String::new(), Action::OpenCleanup),
        ]);
        // „Neu laden“ nur für Terminals (ein Chat hat keinen Prozess).
        if self.entries.get(&f).is_some_and(|e| !e.is_chat()) {
            v.push((lang.t("Sitzung neu laden …", "Reload session …").into(), String::new(), Action::RequestReload(f)));
        }
        v.push((lang.t("Neues Projekt …", "New project …").into(), String::new(), Action::NewProjectPrompt));
        for (i, p) in self.projects.iter().enumerate() {
            if p.id != self.active_project {
                // Feste Kürzel gibt es nur für die ersten neun Projekte (#254)
                let hint = if i < 9 { keys::pretty(&format!("Ctrl+Shift+{}", i + 1), lang == Lang::De) } else { String::new() };
                v.push((format!("{} {}", lang.t("Projekt wechseln:", "Switch project:"), p.name), hint, Action::SwitchProject(p.id)));
            }
        }
        for (id, e) in &self.entries {
            // Minimierte Sitzungen: „Wechseln zu“ öffnet sie allein, die sichtbaren Bereiche werden minimiert (`App::activate`).
            let hint = if e.meta.minimized {
                let m = lang.t("minimiert", "minimized");
                if e.meta.group.is_empty() { m.to_string() } else { format!("{} · {m}", e.meta.group) }
            } else {
                e.meta.group.clone()
            };
            v.push((format!("{} {}", lang.t("Wechseln zu:", "Switch to:"), e.display_name()), hint, Action::Activate(*id)));
        }
        for (i, s) in self.prompts.iter().enumerate() {
            v.push((format!("{} {}", lang.t("Prompt senden:", "Send prompt:"), s.name), String::new(), Action::SendSnippet(i)));
        }
        v
    }

    pub(super) fn palette_ui(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(mut p) = self.palette.take() else { return };
        let lang = self.settings.language;
        let (down, up, enter, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        let mut items: Vec<(i32, String, String, Action)> = self
            .palette_items()
            .into_iter()
            .filter_map(|(l, h, a)| if p.query.trim().is_empty() { Some((0, l, h, a)) } else { fuzzy(&p.query, &l).map(|s| (s, l, h, a)) })
            .collect();
        if !p.query.trim().is_empty() {
            items.sort_by(|a, b| b.0.cmp(&a.0));
        }
        items.truncate(14);
        // Ticket-Treffer (Experten-Projekte): bei einer Ticketnummer zuerst, sonst nach den Befehlen
        let tickets = self.tlinks_palette_hits(&p.query);
        if !tickets.is_empty() {
            let rows = tickets.into_iter().map(|(l, h, a)| (0, l, h, a));
            if terminal_links::normalize_key_query(&p.query).is_some() {
                items.splice(0..0, rows);
            } else {
                items.extend(rows);
            }
        }
        if down && !items.is_empty() {
            p.sel = (p.sel + 1) % items.len();
        }
        if up && !items.is_empty() {
            p.sel = (p.sel + items.len() - 1) % items.len();
        }
        p.sel = p.sel.min(items.len().saturating_sub(1));
        let mut chosen: Option<Action> = None;
        let mut close = esc;
        if enter {
            chosen = items.get(p.sel).map(|i| i.3.clone());
        }
        // Auf kleinen Fenstern (ab 400 pt Höhe) bleibt die Palette im Bild: Treffer scrollen, Abstand oben schrumpft (#241).
        let screen = ctx.content_rect();
        let top = (screen.height() * 0.1).clamp(8.0, 70.0);
        let list_h = (screen.height() - top - 90.0).max(80.0);
        let width = (screen.width() - 32.0).clamp(240.0, 560.0);
        egui::Window::new("palette")
            .title_bar(false)
            .resizable(false)
            .anchor(Align2::CENTER_TOP, Vec2::new(0.0, top))
            .fixed_size([width, 0.0])
            .show(ctx, |ui| {
                let before = p.query.clone();
                let te = ui.add(
                    egui::TextEdit::singleline(&mut p.query)
                        .desired_width(f32::INFINITY)
                        .hint_text(if self.tlinks.enabled {
                            lang.t("Befehl, Sitzung oder Ticket suchen …", "Search command, session or ticket …")
                        } else {
                            lang.t("Befehl oder Sitzung suchen …", "Search command or session …")
                        }),
                );
                te.request_focus();
                if p.query != before {
                    p.sel = 0;
                }
                ui.add_space(4.0);
                egui::ScrollArea::vertical().id_salt("palette_list").max_height(list_h).min_scrolled_height(list_h).show(ui, |ui| {
                    for (i, (_, label, hint, act)) in items.iter().enumerate() {
                        let r = ui.horizontal(|ui| {
                            let resp = ui.selectable_label(i == p.sel, label);
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.weak(hint);
                            });
                            resp
                        });
                        if i == p.sel && (down || up) {
                            r.inner.scroll_to_me(None);
                        }
                        if r.inner.clicked() {
                            chosen = Some(act.clone());
                        }
                    }
                });
                if items.is_empty() {
                    ui.weak(lang.t("Keine Treffer", "No matches"));
                }
            });
        if chosen.is_some() {
            close = true;
        }
        if let Some(a) = chosen {
            actions.push(a);
        }
        if close {
            self.want_focus = Some(self.focused);
        } else {
            self.palette = Some(p);
        }
    }
}

#[cfg(test)]
mod fit_tests {
    //! Lücken zwischen Bereichen (#160, #236): ein Panel, dessen Inhalt größer ist als das Panel, darf den Nachbarn nicht
    //! verschieben – sonst bleibt zwischen den Füllflächen ein durchsichtiger Streifen.
    use super::fixed_body;
    use eframe::egui::{self, Color32, Pos2, Rect, Sense, Vec2};

    /// Ein Durchlauf mit Kopf (feste Höhe 44), rechtem Panel (Breite 300) und Mitte. `big`: Inhalt größer als die Panels.
    /// Liefert (Kopf-Füllung, Panel-Füllung, Mitte).
    fn run(ctx: &egui::Context, helper: bool, big: bool) -> (Rect, Rect, Rect) {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 700.0))), ..Default::default() };
        let (mut head, mut side, mut central) = (Rect::NOTHING, Rect::NOTHING, Rect::NOTHING);
        let content = |ui: &mut egui::Ui, size: Vec2| {
            let s = if big { size * 2.0 } else { size * 0.5 };
            ui.allocate_exact_size(s, Sense::hover());
            ui.clip_rect()
        };
        let _ = ctx.run(input, |ctx| {
            head = egui::TopBottomPanel::top("head")
                .exact_height(44.0)
                .show_separator_line(false)
                .frame(egui::Frame::NONE.fill(Color32::DARK_GRAY))
                .show(ctx, |ui| if helper { fixed_body(ui, |ui| content(ui, Vec2::new(100.0, 44.0))) } else { content(ui, Vec2::new(100.0, 44.0)) })
                .inner;
            side = egui::SidePanel::right("side")
                .resizable(true)
                .default_width(300.0)
                .width_range(200.0..=500.0)
                .frame(egui::Frame::NONE.fill(Color32::DARK_GRAY))
                .show(ctx, |ui| if helper { fixed_body(ui, |ui| content(ui, Vec2::new(300.0, 10.0))) } else { content(ui, Vec2::new(300.0, 10.0)) })
                .inner;
            central = egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| ui.max_rect()).response.rect;
        });
        (head, side, central)
    }

    #[test]
    fn plain_panels_leave_gaps_when_their_content_is_too_big() {
        // Nachweis der Ursache mit egui allein: die Füllung endet an der festen Größe, der Nachbar beginnt dahinter.
        let ctx = egui::Context::default();
        let (head, _, central) = run(&ctx, false, true);
        assert!(central.top() - head.bottom() > 20.0, "erwartet eine Lücke unter dem Kopf: {head:?} / {central:?}");
    }

    #[test]
    fn fixed_panels_touch_their_neighbours() {
        for big in [false, true] {
            let ctx = egui::Context::default();
            for _ in 0..3 {
                let (head, side, central) = run(&ctx, true, big);
                assert!((head.bottom() - 44.0).abs() < 0.01 && (central.top() - head.bottom()).abs() < 0.01, "Kopf {head:?} / Mitte {central:?} (groß: {big})");
                assert!((side.top() - head.bottom()).abs() < 0.01, "Panel {side:?} beginnt nicht unter dem Kopf {head:?}");
                assert!((side.left() - central.right()).abs() < 0.01 && (side.width() - 300.0).abs() < 0.01, "Panel {side:?} / Mitte {central:?} (groß: {big})");
            }
        }
    }
}

#[cfg(test)]
mod files_root_tests {
    //! Wurzel des Dateibaums (#277): Arbeitsordner der Sitzung, nicht der aktuelle Ordner der Shell.
    use super::*;

    #[test]
    fn root_is_the_working_folder_not_the_current_one() {
        let home = r"C:\Users\u";
        // Windows-Sitzung: eingestellter Ordner, egal wohin die Shell gewechselt ist; ohne Ordner das Home.
        assert_eq!(files_root(r"C:\Work\proj", r"C:\Work\proj\src", None, home), r"C:\Work\proj");
        assert_eq!(files_root("  ", r"C:\elsewhere", None, home), home);
        assert_eq!(files_root("", "", None, home), home);
        // WSL: Linux-Ordner über \\wsl.localhost, /mnt/c als Windows-Pfad.
        assert_eq!(files_root("/home/u/p", "", Some(Some("Ubuntu")), home), r"\\wsl.localhost\Ubuntu\home\u\p");
        assert_eq!(files_root("/mnt/c/Work", "", Some(Some("Ubuntu")), home), r"C:\Work");
        assert_eq!(files_root(r"C:\Work", "", Some(Some("Ubuntu")), home), r"C:\Work");
        // WSL ohne Ordner oder mit ~: der Startordner der Sitzung (wo die Shell begonnen hat), sonst ~ wie bisher.
        assert_eq!(files_root("", r"\\wsl$\Ubuntu\home\u", Some(Some("Ubuntu")), home), r"\\wsl$\Ubuntu\home\u");
        assert_eq!(files_root("~", r"\\wsl$\Ubuntu\home\u", Some(Some("Ubuntu")), home), r"\\wsl$\Ubuntu\home\u");
        assert_eq!(files_root("~", "", Some(Some("Ubuntu")), home), "~");
        // Distribution unbekannt: nur /mnt/<lw> lässt sich umrechnen.
        assert_eq!(files_root("/mnt/d/x", "", Some(None), home), r"D:\x");
        assert_eq!(files_root("/srv", r"\\wsl$\Arch\srv", Some(None), home), r"\\wsl$\Arch\srv");
    }

    #[test]
    fn current_folder_below_the_root_is_revealed() {
        assert_eq!(tree_chain(r"C:\Work\proj", r"c:/work/proj/src/app/"), vec![r"c:\work\proj\src".to_string(), r"c:\work\proj\src\app".to_string()]);
        assert!(tree_chain(r"C:\Work\proj", r"C:\Work\proj").is_empty(), "Wurzel selbst: nichts aufzuklappen");
        assert!(tree_chain(r"C:\Work\proj", r"C:\Work\project2").is_empty(), "nur gleicher Anfang");
        assert!(tree_chain(r"C:\Work\proj", r"D:\x").is_empty());
        assert!(tree_chain(r"C:\Work", "").is_empty());
        // WSL: Wurzel über wsl.localhost, gemeldeter Ordner über wsl$ (from_wsl) – gleiche Distribution, Linux-Pfad zählt.
        let chain = tree_chain(r"\\wsl.localhost\Ubuntu\home\u\p", r"\\wsl$\ubuntu\home\u\p\src");
        assert_eq!(chain, vec!["wsl:ubuntu:/home/u/p/src".to_string()]);
        assert!(tree_chain(r"\\wsl.localhost\Ubuntu\home\u\p", r"\\wsl$\Ubuntu\home\u\P\src").is_empty(), "Linux unterscheidet Groß-/Kleinschreibung");
        let m = TreeMark { chain, reveal: true, slow: false };
        assert_eq!(m.role(Path::new(r"\\wsl.localhost\Ubuntu\home\u\p\src")), (true, true));
        assert_eq!(m.role(Path::new(r"\\wsl.localhost\Ubuntu\home\u\p\doc")), (false, false));
        let m = TreeMark { chain: tree_chain(r"C:\W", r"C:\W\a\b"), reveal: false, slow: false };
        assert_eq!((m.role(Path::new(r"C:\W\A")), m.role(Path::new(r"C:\W\a\b"))), ((true, false), (true, true)));
    }

    /// Headless: Der Baum klappt den Weg zum aktuellen Ordner auf (dessen Inhalt wird gelesen), sonst nur die Wurzel.
    #[test]
    fn tree_opens_the_way_to_the_current_folder() {
        let dir = std::env::temp_dir().join(format!("pd-tree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a").join("b").join("c")).unwrap();
        std::fs::create_dir_all(dir.join("z")).unwrap();
        let root = dir.to_string_lossy().to_string();
        let run = |ctx: &egui::Context, mark: &TreeMark, cache: &mut HashMap<PathBuf, Vec<TreeItem>>| {
            let mut actions = Vec::new();
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    tree_children(ui, Path::new(&root), false, cache, &mut actions, Lang::En, mark);
                });
            });
        };
        // Ohne aktuellen Ordner darunter: nur die Wurzel wird gelesen (alles zugeklappt).
        let mut cache = HashMap::new();
        run(&egui::Context::default(), &TreeMark::default(), &mut cache);
        assert_eq!(cache.len(), 1);
        // Shell in a/b: a und a/b werden aufgeklappt (ihr Inhalt gelesen), c und z bleiben zu.
        let ctx = egui::Context::default();
        let mut cache = HashMap::new();
        let cur = dir.join("a").join("b").to_string_lossy().to_string();
        run(&ctx, &TreeMark { chain: tree_chain(&root, &cur), reveal: true, slow: false }, &mut cache);
        assert!(cache.contains_key(&dir.join("a")) && cache.contains_key(&dir.join("a").join("b")), "{:?}", cache.keys().collect::<Vec<_>>());
        assert!(!cache.contains_key(&dir.join("z")) && !cache.contains_key(&dir.join("a").join("b").join("c")));
        // Im nächsten Bild ohne „reveal“ bleibt es offen (Zustand gemerkt), wird aber nicht mehr erzwungen.
        let mut cache = HashMap::new();
        run(&ctx, &TreeMark { chain: tree_chain(&root, &cur), reveal: false, slow: false }, &mut cache);
        assert!(cache.contains_key(&dir.join("a").join("b")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Langsame Wurzel (Netzwerk, WSL): nichts wird im UI-Thread gelesen; aufgeklappt wird nur schon Gelesenes.
    #[test]
    fn slow_roots_are_listed_in_the_background_and_not_revealed_blindly() {
        let dir = std::env::temp_dir().join(format!("pd-slowtree-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("a").join("b")).unwrap();
        let root = dir.to_string_lossy().to_string();
        let cur = dir.join("a").join("b").to_string_lossy().to_string();
        let ctx = egui::Context::default();
        let mut cache = HashMap::new();
        let mark = TreeMark { chain: tree_chain(&root, &cur), reveal: true, slow: true };
        let frame = |cache: &mut HashMap<PathBuf, Vec<TreeItem>>| {
            let mut actions = Vec::new();
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    tree_children(ui, Path::new(&root), false, cache, &mut actions, Lang::En, &mark);
                });
            });
        };
        frame(&mut cache);
        assert!(cache.is_empty(), "nichts im UI-Thread gelesen");
        let t0 = Instant::now();
        while !slow_cached(&dir, false) && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(slow_cached(&dir, false), "Wurzel im Hilfsthread gelesen");
        assert_eq!(slow_list(&ctx, &dir, false).map(|v| v.len()), Some(1));
        frame(&mut cache);
        // „a“ war noch nicht gelesen: nicht blind aufgeklappt (das hätte ihn gelesen).
        assert!(!slow_cached(&dir.join("a"), false));
        assert!(cache.is_empty());
        slow_clear();
        assert!(!slow_cached(&dir, false), "Aktualisieren vergisst Gelesenes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mnt_drives_of_wsl_compare_with_windows_roots_and_blocked_roots_show_a_hint() {
        // Arbeitsordner /mnt/c/Work → Wurzel C:\Work; die Shell meldet \\wsl$\Ubuntu\mnt\c\Work\src.
        let root = files_root("/mnt/c/Work", "", Some(Some("Ubuntu")), "");
        assert_eq!(tree_chain(&root, r"\\wsl$\Ubuntu\mnt\c\Work\src"), vec![r"c:\work\src".to_string()]);
        assert_eq!(tree_key(r"\\wsl$\Ubuntu\mnt\c\Work"), tree_key(r"C:\Work"));
        assert_eq!(tree_key(r"\\wsl.localhost\Ubuntu\mnt"), "wsl:ubuntu:/mnt");
        // Hinweise statt Lesen
        assert!(files_blocked("~", true, Lang::En).is_some_and(|t| t.contains("not known yet")));
        assert!(files_blocked(r"\\wsl.localhost\Ubuntu\home\u", false, Lang::De).is_some_and(|t| t.contains("läuft nicht")));
        assert!(files_blocked(r"\\wsl.localhost\Ubuntu\home\u", true, Lang::En).is_none());
        // Ein lokaler (absoluter) Ordner der Plattform wird auch ohne laufende Sitzung gezeigt. `files_blocked` prüft mit
        // `Path::is_absolute`: `C:\Work` ist nur unter Windows absolut, unter Linux/macOS ein relativer Name.
        let local = if cfg!(windows) { r"C:\Work" } else { "/srv/work" };
        assert!(files_blocked(local, false, Lang::En).is_none(), "lokaler Ordner auch ohne laufende Sitzung");
        assert!(is_slow_root(r"\\wsl.localhost\Ubuntu\home") && is_slow_root(r"\\server\share\x") && !is_slow_root(r"C:\Work"));
    }
}
