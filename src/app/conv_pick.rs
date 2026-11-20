//! Auswahl der Konversation vor einem Start (#302, „Fortsetzen nach Neustart“ = „Konversation auswählen“).
//!
//! Vor jedem Neu laden/Neustart (und beim Programmstart, wenn die globale Einstellung es erlaubt) startet eine solche
//! Sitzung nicht gleich: Paneldeck sucht im Hintergrund die Konversationen ihres Arbeitsordners (Windows:
//! `~/.claude/projects/<Ordnername>`, WSL: dasselbe auf der Linux-Seite über `\\wsl.localhost`, nie im UI-Thread) und
//! zeigt sie zur Auswahl, dazu „Neue Konversation“. Bestätigen startet `claude --resume <id>`, Abbrechen oder „Neue
//! Konversation“ einen neuen Start; ohne gefundene Konversation startet Claude ohne Rückfrage neu. Mehrere wartende
//! Sitzungen (Programmstart) kommen nacheinander an die Reihe. Bis dahin hat die Sitzung keinen Prozess
//! (`Entry::pick_pending`). Über MCP gibt es keinen Weg, die Auswahl auszulösen: Starts ohne vorherige Auswahl beginnen
//! in diesem Modus neu (`session_settings::launch_start`).

use super::*;
use crate::history::ConvItem;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, TryRecvError};

/// Höchstens so viele Konversationen in der Auswahl.
const LIMIT: usize = 200;

/// Wo die Konversationen einer Sitzung liegen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Target {
    /// Windows: `projects/<Ordnername>` (`None`: nicht vorherzusagen, z. B. sehr langer Pfad).
    Windows(Option<PathBuf>),
    /// WSL: Distribution (`None` = Standard) und gespeicherter Arbeitsordner; aufgelöst erst im Hintergrund.
    Wsl { distro: Option<String>, cwd: String },
}

/// Eine Sitzung, die vor ihrem Start auf die Auswahl wartet.
struct PickJob {
    id: u64,
    /// Programmstart (Scrollback wiederherstellen) statt Neu laden.
    restore: bool,
    rx: Option<Receiver<Vec<ConvItem>>>,
    items: Option<Vec<ConvItem>>,
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
    /// Wo die Konversationen der Sitzung liegen (wie beim Start: Shell der Sitzung, Arbeitsordner).
    fn pick_target(&self, meta: &SessionMeta) -> Target {
        let shell = self.shell_by_id(&meta.shell_id);
        if let Some(s) = shell.as_ref().filter(|s| shells::is_wsl(s)) {
            let distro = s.id.strip_prefix("wsl:").filter(|d| !d.is_empty()).map(str::to_string).or_else(|| shells::wsl_distro(&s.args));
            return Target::Wsl { distro, cwd: meta.cwd.clone() };
        }
        let cwd = session_settings::windows_cwd(meta, &home_dir());
        Target::Windows(crate::history::projects_dir().zip(crate::history::project_slug(&cwd)).map(|(p, slug)| p.join(slug)))
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
        entry.error = Some(lang.t("Wartet auf die Auswahl der Konversation …", "Waiting for the conversation to be chosen …").to_string());
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
        self.conv_pick.jobs.push(PickJob { id, restore, rx: Some(rx), items: None });
        true
    }

    /// Je Bild (Hauptfenster): Suchergebnisse abholen; ohne Konversation gleich neu starten; sonst die nächste Auswahl
    /// öffnen (in der Reihenfolge der Anfragen).
    pub(super) fn poll_picks(&mut self, ctx: &egui::Context) {
        let entries = &self.entries;
        let waiting = |id: u64| entries.get(&id).is_some_and(|e| e.pick_pending);
        self.conv_pick.jobs.retain(|j| waiting(j.id));
        if self.conv_pick.dlg.as_ref().is_some_and(|d| !waiting(d.id)) {
            self.conv_pick.dlg = None;
        }
        let mut fresh = Vec::new();
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
                Err(TryRecvError::Empty) => {}
            }
        }
        self.conv_pick.jobs.retain(|j| !fresh.iter().any(|(id, _)| *id == j.id));
        for (id, restore) in fresh {
            self.finish_pick(ctx, id, restore, None);
        }
        if self.conv_pick.dlg.is_none() {
            if let Some(pos) = self.conv_pick.jobs.iter().position(|j| j.items.is_some()) {
                let j = self.conv_pick.jobs.remove(pos);
                let items = j.items.unwrap_or_default();
                let Some(e) = self.entries.get(&j.id) else { return };
                // Vorauswahl: die bisherige Konversation der Sitzung, sonst die neueste.
                let sel = items.iter().position(|c| c.session_id == e.meta.claude_session).or(Some(0));
                self.conv_pick.dlg = Some(PickDlg { id: j.id, restore: j.restore, name: e.display_name(), cwd: e.meta.cwd.clone(), items, sel });
                ctx.request_repaint();
            }
        }
    }

    /// Auswahl erledigt: die Sitzung starten – mit `--resume <id>` oder neu (`resume` = `None`).
    fn finish_pick(&mut self, ctx: &egui::Context, id: u64, restore: bool, resume: Option<String>) {
        let Some(mut e) = self.entries.remove(&id) else { return };
        e.error = None;
        // Ohne gewählte ID beginnt der Modus „Konversation auswählen“ in `launch_start` immer neu.
        self.spawn_into(ctx, &mut e, Launch { restore, restart: !restore, continue_last: false, prompt: None, resume_id: resume.as_deref() });
        self.entries.insert(id, e);
    }

    /// Fenster der Auswahl (nur im Hauptfenster gezeichnet). Enter startet, Esc beginnt neu, ↑/↓ wählen.
    pub(super) fn conv_pick_ui(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.conv_pick.dlg.take() else { return };
        let lang = self.settings.language;
        let de = lang == Lang::De;
        let (esc, enter, up, down) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::Escape),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
            )
        });
        let n = d.items.len();
        // Zeile 0 = „Neue Konversation“, danach die Konversationen.
        let mut row = d.sel.map_or(0, |i| i + 1);
        if up {
            row = row.saturating_sub(1);
        }
        if down {
            row = (row + 1).min(n);
        }
        d.sel = row.checked_sub(1);
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
                    let meta = format!("{} · {} · {}", crate::history::age_label(c.modified, de), crate::history::size_label(c.size), crate::mcp::short(&c.session_id, 8));
                    let text = format!("{}\n{meta}", crate::mcp::short(&c.title, 90));
                    let r = ui.selectable_label(d.sel == Some(i), text);
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
            let resume = if go { d.sel.and_then(|i| d.items.get(i)).map(|c| c.session_id.clone()) } else { None };
            self.finish_pick(ctx, d.id, d.restore, resume);
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
}
