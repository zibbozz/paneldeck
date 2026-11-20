//! Auswahl der Konversation vor einem Start (#302, „Fortsetzen nach Neustart“ = „Konversation auswählen“).
//!
//! Vor jedem Neu laden/Neustart (und beim Programmstart, wenn die globale Einstellung es erlaubt) startet eine solche
//! Sitzung nicht gleich: Paneldeck sucht im Hintergrund die Konversationen ihres Arbeitsordners (Windows:
//! `~/.claude/projects/<Ordnername>`, WSL: dasselbe auf der Linux-Seite über `\\wsl.localhost`, nie im UI-Thread) und
//! zeigt sie zur Auswahl, dazu „Neue Konversation“. Bestätigen startet `claude --resume <id>`, Abbrechen oder „Neue
//! Konversation“ einen neuen Start; ohne gefundene Konversation startet Claude ohne Rückfrage neu. Mehrere wartende
//! Sitzungen (Programmstart) kommen nacheinander an die Reihe. Bis dahin hat die Sitzung keinen Prozess
//! (`Entry::pick_pending`; im Bereich: „Auswahl zeigen“, „Ohne Auswahl neu starten“; die Suche darf 15 s dauern). Über MCP gibt es keinen Weg, die Auswahl auszulösen: Starts ohne vorherige Auswahl beginnen
//! in diesem Modus neu (`session_settings::launch_start`).

use super::*;
use crate::history::ConvItem;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// Höchstens so viele Konversationen in der Auswahl.
const LIMIT: usize = 200;

/// So lange darf die Suche dauern (WSL kann hängen); danach startet die Sitzung neu, mit Hinweis.
const SCAN_TIMEOUT: Duration = Duration::from_secs(15);

/// Wo die Konversationen einer Sitzung liegen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Target {
    /// Windows: `projects/<Ordnername>` (`None`: nicht vorherzusagen, z. B. sehr langer Pfad).
    Windows(Option<PathBuf>),
    /// WSL: Distribution (`None` = Standard) und gespeicherter Arbeitsordner; aufgelöst erst im Hintergrund.
    Wsl { distro: Option<String>, cwd: String },
}

/// Stand der Sitzung, für den gesucht wurde (Shell, Arbeitsordner): Ändert er sich bis zur Wahl, gehört die gewählte
/// Konversation nicht mehr zum Ordner, und die Sitzung startet neu.
type Origin = (String, String);

fn origin_of(meta: &SessionMeta) -> Origin {
    (meta.shell_id.clone(), meta.cwd.clone())
}

/// Eine Sitzung, die vor ihrem Start auf die Auswahl wartet.
struct PickJob {
    id: u64,
    /// Programmstart (Scrollback wiederherstellen) statt Neu laden.
    restore: bool,
    rx: Option<Receiver<Vec<ConvItem>>>,
    items: Option<Vec<ConvItem>>,
    started: Instant,
    origin: Origin,
}

/// Die gerade gezeigte Auswahl.
struct PickDlg {
    id: u64,
    restore: bool,
    name: String,
    cwd: String,
    items: Vec<ConvItem>,
    /// Gewählte Zeile; `None` = „Neue Konversation“.
    sel: Option<usize>,
    origin: Origin,
}

/// Wartende Sitzungen und die offene Auswahl (eine zur Zeit).
#[derive(Default)]
pub(super) struct ConvPick {
    jobs: Vec<PickJob>,
    dlg: Option<PickDlg>,
}

impl ConvPick {
    /// Wartet die Sitzung (Suche läuft, Auswahl steht an oder ist offen)?
    fn has(&self, id: u64) -> bool {
        self.jobs.iter().any(|j| j.id == id) || self.dlg.as_ref().is_some_and(|d| d.id == id)
    }

    /// Ist die Auswahl gerade offen (dann gehören Enter/Esc ihr, nicht dem Terminal)?
    pub(super) fn open(&self) -> bool {
        self.dlg.is_some()
    }

    /// Programmstart oder Neu laden – für den Start ohne Auswahl.
    fn restore_of(&self, id: u64) -> Option<bool> {
        self.jobs.iter().find(|j| j.id == id).map(|j| j.restore).or_else(|| self.dlg.as_ref().filter(|d| d.id == id).map(|d| d.restore))
    }

    fn forget(&mut self, id: u64) {
        self.jobs.retain(|j| j.id != id);
        if self.dlg.as_ref().is_some_and(|d| d.id == id) {
            self.dlg = None;
        }
    }
}

/// Zeile der Auswahl: 0 = „Neue Konversation“, `i + 1` = Konversation `i`.
fn row_of(sel: Option<usize>) -> usize {
    sel.map_or(0, |i| i + 1)
}

/// Nächste wählbare Zeile in Richtung `down` (Konversationen, die in einer anderen Sitzung laufen, `blocked`, werden
/// übersprungen; „Neue Konversation“ ist immer wählbar). Ohne weitere wählbare Zeile bleibt die Auswahl.
pub(super) fn step(sel: Option<usize>, blocked: &[bool], down: bool) -> Option<usize> {
    let mut row = row_of(sel);
    loop {
        if down {
            if row >= blocked.len() {
                return sel;
            }
            row += 1;
        } else {
            if row == 0 {
                return sel;
            }
            row -= 1;
        }
        if row == 0 {
            return None;
        }
        if !blocked[row - 1] {
            return Some(row - 1);
        }
    }
}

/// Vorauswahl: die bisherige Konversation der Sitzung, sonst die neueste – nur, wenn sie nicht anderswo läuft.
pub(super) fn initial(items: &[ConvItem], blocked: &[bool], own: &str) -> Option<usize> {
    let free = |i: &usize| !blocked.get(*i).copied().unwrap_or(true);
    items.iter().position(|c| c.session_id == own).filter(free).or_else(|| (0..items.len()).find(free))
}

/// Linux-Arbeitsordner einer WSL-Sitzung, wie beim Start (`wsl.exe --cd`): leer/`~` = Home, `~/x` mit dem Home,
/// `C:\x` = `/mnt/c/x`, `\\wsl.localhost\<D>\x` = `/x`.
pub(super) fn linux_cwd(cwd: &str, home: &str) -> Option<String> {
    let c = cwd.trim();
    if c.is_empty() || c == "~" {
        let h = home.trim_end_matches('/');
        return Some(if h.is_empty() { "/".to_string() } else { h.to_string() });
    }
    if let Some(l) = crate::wsl::windows_to_linux(c) {
        return Some(l);
    }
    crate::wsl::expand_home(c, Some(home)).map(|p| crate::wsl::normalize(&p, Some(home)))
}

/// Ordner der Konversationen auf der Linux-Seite (als `\\wsl.localhost\…`-Pfad).
pub(super) fn wsl_projects_dir(distro: &str, home: &str, cwd: &str) -> Option<PathBuf> {
    let linux = linux_cwd(cwd, home)?;
    let projects = format!("{}/.claude/projects", home.trim_end_matches('/'));
    Some(PathBuf::from(crate::wsl::linux_to_unc(distro, &projects)).join(crate::history::project_slug(&linux)?))
}

/// Sucht die Konversationen (blockiert: nur im Hintergrund).
fn scan(t: Target) -> Vec<ConvItem> {
    match t {
        Target::Windows(Some(dir)) => crate::history::folder_conversations(&dir, LIMIT),
        Target::Windows(None) => Vec::new(),
        Target::Wsl { distro, cwd } => {
            use crate::wsl::Backend;
            let be = crate::wsl::WslExe;
            let Some(distro) = distro.or_else(|| be.default_distro()) else { return Vec::new() };
            let Some(home) = be.home(&distro) else { return Vec::new() };
            wsl_projects_dir(&distro, &home, &cwd).map(|d| crate::history::folder_conversations(&d, LIMIT)).unwrap_or_default()
        }
    }
}

impl App {
    /// Wo die Konversationen der Sitzung liegen (wie beim Start: Shell der Sitzung, Arbeitsordner; WSL nach derselben
    /// Regel wie `claude_command`).
    fn pick_target(&self, meta: &SessionMeta) -> Target {
        let shell = self.shell_by_id(&meta.shell_id);
        if shell_is_wsl(shell.as_ref(), &meta.shell_id) {
            return Target::Wsl { distro: shell.as_ref().and_then(wsl_distro_of), cwd: meta.cwd.clone() };
        }
        let cwd = session_settings::windows_cwd(meta, &home_dir());
        Target::Windows(crate::history::projects_dir().zip(crate::history::project_slug(&cwd)).map(|(p, slug)| p.join(slug)))
    }

    /// Name der Sitzung (außer `except`), in der die Konversation `conv` gerade läuft (wie beim Fortsetzen aus dem Verlauf).
    fn running_in(&self, conv: &str, except: u64) -> Option<String> {
        self.entries.iter().find(|(id, e)| **id != except && e.launched_claude && !e.exited() && e.meta.claude_session == conv).map(|(_, e)| e.display_name())
    }

    /// Ist das Hauptfenster (mit der Auswahl) gerade nicht zu sehen?
    fn main_hidden(&self, ctx: &egui::Context) -> bool {
        self.in_tray || ctx.input(|i| i.viewport().minimized == Some(true))
    }

    /// Vor einem Start (Programmstart `restore`, Neu laden/Neustart `restart`) im Modus „Konversation auswählen“: statt
    /// zu starten, die Konversationen im Hintergrund suchen und die Auswahl anstellen. `true` = die Sitzung wartet
    /// (nicht starten); `false` = normal starten.
    pub(super) fn begin_pick(&mut self, ctx: &egui::Context, entry: &mut Entry, restore: bool, restart: bool) -> bool {
        if entry.is_chat() || !entry.meta.claude || !self.settings.claude_mode {
            return false;
        }
        if !session_settings::pick_wanted(restore, restart, entry.meta.resume_mode(), self.settings.claude_resume) {
            return false;
        }
        // Administrator-Sitzungen starten in einem eigenen Fenster bzw. beim Programmstart gar nicht: keine Auswahl.
        if self.profile(entry.meta.profile).is_some_and(|p| p.admin) || self.shell_by_id(&entry.meta.shell_id).is_none() {
            return false;
        }
        let lang = self.settings.language;
        entry.live = None;
        entry.pick_pending = true;
        entry.error = Some(lang.t("Wartet auf die Auswahl der Konversation (im Hauptfenster) …", "Waiting for the conversation to be chosen (in the main window) …").to_string());
        // Nichts aus dem vorigen Start gilt noch (Status, Hook-Meldungen übrig gebliebener Prozesse).
        entry.status = ClaudeStatus::Unknown;
        entry.status_msg.clear();
        entry.perm_since = None;
        entry.session_key.clear();
        entry.hook_conv = hooks::ConvBinding::default();
        let id = entry.meta.id;
        if self.conv_pick.has(id) {
            return true;
        }
        let target = self.pick_target(&entry.meta);
        let (tx, rx) = channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(scan(target));
            c.request_repaint();
        });
        self.conv_pick.jobs.push(PickJob { id, restore, rx: Some(rx), items: None, started: Instant::now(), origin: origin_of(&entry.meta) });
        ctx.request_repaint_after(SCAN_TIMEOUT);
        true
    }

    /// Je Bild (Hauptfenster): Suchergebnisse abholen; ohne Konversation (oder nach Zeitüberschreitung) gleich neu
    /// starten; sonst die nächste Auswahl öffnen (in der Reihenfolge der Anfragen).
    pub(super) fn poll_picks(&mut self, ctx: &egui::Context) {
        let entries = &self.entries;
        let waiting = |id: u64| entries.get(&id).is_some_and(|e| e.pick_pending);
        self.conv_pick.jobs.retain(|j| waiting(j.id));
        if self.conv_pick.dlg.as_ref().is_some_and(|d| !waiting(d.id)) {
            self.conv_pick.dlg = None;
        }
        let mut fresh = Vec::new();
        let mut late = Vec::new();
        for j in &mut self.conv_pick.jobs {
            let Some(rx) = &j.rx else { continue };
            match rx.try_recv() {
                Ok(items) => {
                    j.rx = None;
                    if items.is_empty() {
                        fresh.push((j.id, j.restore));
                    } else {
                        j.items = Some(items);
                    }
                }
                Err(TryRecvError::Disconnected) => {
                    j.rx = None;
                    fresh.push((j.id, j.restore));
                }
                Err(TryRecvError::Empty) if j.started.elapsed() >= SCAN_TIMEOUT => {
                    j.rx = None;
                    fresh.push((j.id, j.restore));
                    late.push(j.id);
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        self.conv_pick.jobs.retain(|j| !fresh.iter().any(|(id, _)| *id == j.id));
        let lang = self.settings.language;
        for (id, restore) in fresh {
            if late.contains(&id) {
                let name = self.entries.get(&id).map(|e| e.display_name()).unwrap_or_default();
                self.notify_text(match lang {
                    Lang::De => format!("{name}: Die Konversationen des Arbeitsordners wurden nicht rechtzeitig gefunden – Claude startet mit einer neuen Konversation."),
                    Lang::En => format!("{name}: the conversations of the working folder were not found in time – Claude starts with a new conversation."),
                });
            }
            self.finish_pick(ctx, id, restore, None);
        }
        if self.conv_pick.dlg.is_none() {
            if let Some(pos) = self.conv_pick.jobs.iter().position(|j| j.items.is_some()) {
                let j = self.conv_pick.jobs.remove(pos);
                let items = j.items.unwrap_or_default();
                let Some(e) = self.entries.get(&j.id) else { return };
                let (name, cwd, own) = (e.display_name(), e.meta.cwd.clone(), e.meta.claude_session.clone());
                let blocked: Vec<bool> = items.iter().map(|c| self.running_in(&c.session_id, j.id).is_some()).collect();
                let sel = initial(&items, &blocked, &own);
                if self.main_hidden(ctx) {
                    let msg = match lang {
                        Lang::De => format!("{name}: Paneldeck wartet auf die Auswahl der Konversation (Hauptfenster)."),
                        Lang::En => format!("{name}: Paneldeck is waiting for you to choose the conversation (main window)."),
                    };
                    if self.settings.notify_toast {
                        notify::toast("Paneldeck", &msg, self.settings.notify_sound);
                    }
                    self.notify_text(msg);
                }
                self.conv_pick.dlg = Some(PickDlg { id: j.id, restore: j.restore, name, cwd, items, sel, origin: j.origin });
                ctx.request_repaint();
            }
        }
    }

    /// Auswahl erledigt: die Sitzung starten – mit `--resume <id>` oder neu (`resume` = `None`). Haben sich Shell oder
    /// Arbeitsordner seit der Suche geändert, gehört die gewählte Konversation nicht mehr dazu: dann neu.
    fn finish_pick(&mut self, ctx: &egui::Context, id: u64, restore: bool, resume: Option<String>) {
        self.finish_pick_from(ctx, id, restore, resume, None);
    }

    fn finish_pick_from(&mut self, ctx: &egui::Context, id: u64, restore: bool, resume: Option<String>, origin: Option<&Origin>) {
        let Some(mut e) = self.entries.remove(&id) else { return };
        e.error = None;
        let resume = resume.filter(|_| origin.is_none_or(|o| *o == origin_of(&e.meta)));
        // Ohne gewählte ID beginnt der Modus „Konversation auswählen“ in `launch_start` immer neu.
        self.spawn_into(ctx, &mut e, Launch { restore, restart: !restore, continue_last: false, prompt: None, resume_id: resume.as_deref() });
        self.entries.insert(id, e);
    }

    /// Knopf „Ohne Auswahl neu starten“ im Bereich einer wartenden Sitzung.
    pub(super) fn pick_fresh(&mut self, ctx: &egui::Context, id: u64) {
        if !self.entries.get(&id).is_some_and(|e| e.pick_pending) {
            return;
        }
        let restore = self.conv_pick.restore_of(id).unwrap_or(false);
        self.conv_pick.forget(id);
        self.finish_pick(ctx, id, restore, None);
    }

    /// Knopf „Auswahl zeigen“: Hauptfenster nach vorn, auch aus dem Tray (die Auswahl erscheint nur dort).
    pub(super) fn show_pick_window(&mut self, ctx: &egui::Context) {
        if let (Some(h), true) = (self.hwnd, self.in_tray) {
            crate::platform::window::set_tray_hidden(h, false);
            crate::platform::chrome::apply_corners(h);
        }
        self.in_tray = false;
        for cmd in [egui::ViewportCommand::Minimized(false), egui::ViewportCommand::Visible(true), egui::ViewportCommand::Focus] {
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, cmd);
        }
    }

    /// Fenster der Auswahl (nur im Hauptfenster gezeichnet). Enter startet, Esc beginnt neu, ↑/↓ wählen – aber nur,
    /// wenn das Hauptfenster den Fokus hat und kein anderes Feld die Tastatur hält (das Terminal gibt sie ab, solange die
    /// Auswahl offen ist: `central`). Konversationen, die in einer anderen Sitzung laufen, sind markiert und nicht wählbar.
    pub(super) fn conv_pick_ui(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.conv_pick.dlg.take() else { return };
        let lang = self.settings.language;
        let de = lang == Lang::De;
        let running: Vec<Option<String>> = d.items.iter().map(|c| self.running_in(&c.session_id, d.id)).collect();
        let blocked: Vec<bool> = running.iter().map(Option::is_some).collect();
        if d.sel.is_some_and(|i| blocked.get(i).copied().unwrap_or(true)) {
            d.sel = None;
        }
        let keys_ok = ctx.input(|i| i.viewport().focused.unwrap_or(true)) && ctx.memory(|m| m.focused()).is_none();
        let (esc, enter, up, down) = if keys_ok {
            ctx.input_mut(|i| {
                (
                    i.consume_key(Modifiers::NONE, Key::Escape),
                    i.consume_key(Modifiers::NONE, Key::Enter),
                    i.consume_key(Modifiers::NONE, Key::ArrowUp),
                    i.consume_key(Modifiers::NONE, Key::ArrowDown),
                )
            })
        } else {
            (false, false, false, false)
        };
        if up {
            d.sel = step(d.sel, &blocked, false);
        }
        if down {
            d.sel = step(d.sel, &blocked, true);
        }
        let (mut go, mut cancel) = (enter, esc);
        let title = lang.t("Konversation auswählen", "Choose conversation");
        style::movable(ctx, Id::new("conv_pick"), egui::Window::new(title)).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.set_max_width(560.0);
            ui.label(RichText::new(&d.name).strong());
            if !d.cwd.trim().is_empty() {
                ui.label(RichText::new(&d.cwd).small().color(style::dim(ui)));
            }
            ui.add_space(style::S1);
            ui.label(lang.t("Mit welcher Konversation soll Claude starten?", "Which conversation should Claude start with?"));
            ui.add_space(style::S1);
            egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
                let r = ui.selectable_label(d.sel.is_none(), RichText::new(lang.t("Neue Konversation", "New conversation")).strong());
                if r.clicked() {
                    d.sel = None;
                }
                if r.double_clicked() {
                    go = true;
                }
                if (up || down) && d.sel.is_none() {
                    r.scroll_to_me(None);
                }
                for (i, c) in d.items.iter().enumerate() {
                    let mut meta = format!("{} · {} · {}", crate::history::age_label(c.modified, de), crate::history::size_label(c.size), crate::mcp::short(&c.session_id, 8));
                    if let Some(n) = &running[i] {
                        meta = format!("{meta} · {} {n}", lang.t("Läuft in Sitzung", "Running in session"));
                    }
                    let text = format!("{}\n{meta}", crate::mcp::short(&c.title, 90));
                    let r = ui.add_enabled(running[i].is_none(), egui::Button::selectable(d.sel == Some(i), text));
                    let r = if running[i].is_some() {
                        r.on_disabled_hover_text(lang.t("Diese Konversation läuft gerade in einer anderen Sitzung.", "This conversation is running in another session right now."))
                    } else {
                        r
                    };
                    if r.clicked() {
                        d.sel = Some(i);
                    }
                    if r.double_clicked() {
                        d.sel = Some(i);
                        go = true;
                    }
                    if (up || down) && d.sel == Some(i) {
                        r.scroll_to_me(None);
                    }
                }
            });
            ui.add_space(style::S2);
            ui.horizontal(|ui| {
                go |= ui.button(lang.t("Starten", "Start")).clicked();
                cancel |= ui
                    .button(lang.t("Abbrechen (neue Konversation)", "Cancel (new conversation)"))
                    .on_hover_text(lang.t("Claude startet mit einer neuen Konversation.", "Claude starts with a new conversation."))
                    .clicked();
            });
        });
        if go || cancel {
            // Inzwischen anderswo gestartet: nicht ein zweites Mal öffnen, sondern neu.
            let resume = if go { d.sel.filter(|i| !blocked[*i]).and_then(|i| d.items.get(i)).map(|c| c.session_id.clone()) } else { None };
            let still_free = resume.as_deref().is_none_or(|r| self.running_in(r, d.id).is_none());
            self.finish_pick_from(ctx, d.id, d.restore, resume.filter(|_| still_free), Some(&d.origin));
            ctx.request_repaint();
        } else {
            self.conv_pick.dlg = Some(d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ResumeMode;

    fn wants(mode: ResumeMode, restore: bool, restart: bool, global: bool) -> bool {
        session_settings::pick_wanted(restore, restart, mode, global)
    }

    #[test]
    fn wsl_working_folder_resolves_like_the_start() {
        assert_eq!(linux_cwd("", "/home/u").as_deref(), Some("/home/u"));
        assert_eq!(linux_cwd("~", "/home/u/").as_deref(), Some("/home/u"));
        assert_eq!(linux_cwd("~/app", "/home/u").as_deref(), Some("/home/u/app"));
        assert_eq!(linux_cwd("/srv/x/", "/home/u").as_deref(), Some("/srv/x"));
        assert_eq!(linux_cwd(r"C:\Dev\app", "/home/u").as_deref(), Some("/mnt/c/Dev/app"));
        assert_eq!(linux_cwd(r"\\wsl.localhost\Ubuntu\home\u\p", "/home/u").as_deref(), Some("/home/u/p"));
        let d = wsl_projects_dir("Ubuntu", "/home/u", "~/app").unwrap();
        assert_eq!(d, PathBuf::from(r"\\wsl.localhost\Ubuntu\home\u\.claude\projects").join("-home-u-app"));
    }

    #[test]
    fn the_choice_appears_only_on_reload_restart_and_allowed_restore() {
        assert!(wants(ResumeMode::Pick, false, true, false), "Neu laden/Neustart");
        assert!(wants(ResumeMode::Pick, true, false, true), "Programmstart mit globaler Einstellung");
        assert!(!wants(ResumeMode::Pick, true, false, false), "Programmstart ohne globale Einstellung: neu");
        assert!(!wants(ResumeMode::Pick, false, false, true), "erster Start: neu");
        assert!(!wants(ResumeMode::Continue, false, true, true));
        assert!(!wants(ResumeMode::Off, false, true, true));
    }

    /// Konversationen, die in einer anderen Sitzung laufen, sind weder vorausgewählt noch mit ↑/↓ erreichbar.
    #[test]
    fn running_conversations_are_skipped() {
        let item = |id: &str| ConvItem { session_id: id.into(), title: id.into(), modified: std::time::SystemTime::UNIX_EPOCH, size: 1 };
        let items = vec![item("a"), item("b"), item("c")];
        let blocked = vec![true, false, true];
        assert_eq!(initial(&items, &blocked, "c"), Some(1), "eigene läuft anderswo: neueste freie");
        assert_eq!(initial(&items, &blocked, "b"), Some(1));
        assert_eq!(initial(&items, &[true, true, true], "a"), None, "alles belegt: neue Konversation");
        assert_eq!(step(None, &blocked, true), Some(1), "über die belegte hinweg");
        assert_eq!(step(Some(1), &blocked, true), Some(1), "dahinter nur Belegtes: bleibt");
        assert_eq!(step(Some(1), &blocked, false), None, "nach oben zu „Neue Konversation“");
        assert_eq!(step(None, &blocked, false), None);
        assert_eq!(step(None, &[false, false], true), Some(0));
        assert_eq!(step(Some(0), &[false, false], true), Some(1));
    }
}
