//! Einstellung „Sitzungen merken“ (#157). Ist sie aus, startet Paneldeck jedes Mal frisch: keine Terminals, keine Chats,
//! keine Projekte/Ordner aus dem letzten Lauf, keine weiteren oder abgedockten Fenster. Was bleibt: die Einstellungen
//! (`settings.json`, also auch Fenstergröße/Breite der Seitenleiste), die Todo-Liste und Tickets (`paneldeck.db`), die
//! Darstellung der Seitenleiste (Baum/Liste) und der Entwurf im Prompt-Composer.
//!
//! Ablauf: Während des Laufs schreibt Paneldeck `state.json` wie gewohnt (mit Sitzungsliste). Beim Beenden
//! ([`App::save_for_exit`]) werden Scrollback und Chatverläufe der offenen Sitzungen gelöscht und ein zugeschnittener Zustand
//! ([`forget`]) geschrieben. Beim Start mit „aus“ werden alle Scrollback- und Chat-Dateien früherer Läufe gelöscht
//! ([`sweep_session_files`], fängt auch einen Absturz ab).
//!
//! Schutz vor versehentlichem Verlust: Ausschalten fragt nach (Einstellungen), und bevor ein Zustand mit Sitzungen,
//! Projekten oder Ordnern verworfen wird, entsteht eine Sicherung `state.json.bak-<JJJJMMTT-HHMMSS>` im Datenordner
//! ([`backup_state`]; nur die neueste bleibt). Beim Beenden geschieht das nur, wenn die Einstellung beim Start noch an war
//! (der Übergang); beim Start, wenn die Datei noch etwas enthält (z. B. Absturz nach dem Ausschalten).
//!
//! Update-Neustart (Entscheidung des Nutzers zu #157): Ein Update soll laufende Arbeit nicht beenden. Vor dem Neustart wird
//! deshalb ganz normal gespeichert ([`App::save_for_update_restart`]) und die neue Instanz mit
//! `PANELDECK_UPDATE_KEEP_SESSIONS=1` gestartet; sie stellt die Sitzungen **einmal** wieder her ([`restore_once_applies`]).
//! Die Variable wird beim Start sofort entfernt (`update::install::wait_for_previous_instance`) – danach, auch nach einem
//! Absturz, gilt wieder der frische Start.

use super::*;
use std::path::Path;

/// Präfix der Sicherungen von `state.json` (wie `settings.json.bak-<zeit>` bei `--reset-settings`).
const BAK_PREFIX: &str = "state.json.bak-";

/// Stellt dieser Start die Sitzungen trotz „Sitzungen merken“ aus wieder her? Nur nach einem Update-Neustart
/// (`update_restart` = `update::install::restore_once`); mit „an“ gilt ohnehin die normale Wiederherstellung.
pub(super) fn restore_once_applies(remember: bool, update_restart: bool) -> bool {
    !remember && update_restart
}

/// Gespeicherten Zustand für einen frischen Start zuschneiden: keine Sitzungen, Layouts, abgedockten oder weiteren
/// Fenster, keine Projekte und Ordner – nur ein leeres Standardprojekt. Bleibt: `next_id` (Ids werden nie wiederverwendet,
/// verknüpfte Tickets zeigen die alten Sitzungen als „beendet“), der Composer-Entwurf und die Baum-/Listenansicht des
/// zuletzt aktiven Projekts (Darstellung der Seitenleiste). `lang` = gespeicherte Sprache (Name des Standardprojekts).
pub(super) fn forget(saved: SavedState, lang: Lang) -> SavedState {
    let tree_view = saved.projects.iter().find(|p| p.id == saved.active_project).or(saved.projects.first()).is_some_and(|p| p.tree_view);
    let project = Project { id: 1, name: lang.t("Standard", "Default").to_string(), tree_view, ..Project::default() };
    SavedState {
        sessions: Vec::new(),
        layout: None,
        focused: 0,
        next_id: saved.next_id,
        projects: vec![project],
        active_project: 1,
        detached: Vec::new(),
        composer_draft: saved.composer_draft,
        extra_windows: Vec::new(),
    }
}

/// Enthält der Zustand etwas, das beim Verwerfen verloren ginge (Sitzungen, weitere Fenster, mehrere Projekte, Ordner,
/// Vorgaben eines Projekts)? Ein bereits zugeschnittener Zustand ([`forget`]) nicht.
pub(super) fn worth_keeping(s: &SavedState) -> bool {
    !s.sessions.is_empty() || !s.extra_windows.is_empty() || s.projects.len() > 1 || s.projects.iter().any(|p| !p.folders.is_empty() || !p.folder.is_empty() || p.profile != 0)
}

/// Sicherung `state.json.bak-<JJJJMMTT-HHMMSS>` (bei Namensgleichheit mit Zähler) mit diesem Inhalt schreiben; ältere
/// Sicherungen dieser Art werden gelöscht (nur die neueste bleibt). Gibt den Pfad zurück.
pub(super) fn backup_state(dir: &Path, content: &[u8]) -> Option<PathBuf> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let mut target = dir.join(format!("{BAK_PREFIX}{stamp}"));
    let mut n = 2;
    while target.exists() {
        target = dir.join(format!("{BAK_PREFIX}{stamp}-{n}"));
        n += 1;
    }
    state::write_atomic(&target, content).ok()?;
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p != target && e.file_name().to_string_lossy().starts_with(BAK_PREFIX) {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    Some(target)
}

/// Dateien dieser Sitzungen löschen: Scrollback (`scrollback/<id>.txt`) und Chatverlauf (`chats/<id>.json` samt
/// Sicherungen beschädigter Dateien). Beiseitegelegte fremde Verläufe (`.orphan-`) bleiben.
pub(super) fn remove_session_files(dir: &Path, ids: impl IntoIterator<Item = u64>) {
    for id in ids {
        let _ = std::fs::remove_file(dir.join("scrollback").join(format!("{id}.txt")));
        chatmod::store::remove(dir, id);
    }
}

/// Start mit „aus“: alle Scrollback- und Chat-Dateien früherer Läufe in diesem Datenordner löschen (`<id>.…` in
/// `scrollback/` und `chats/`), auch von Sitzungen, die nach der letzten Speicherung vor einem Absturz entstanden und
/// deshalb in keiner `state.json` stehen. Beiseitegelegte Verläufe (`.orphan-`) und andere Dateien bleiben. Gibt die
/// höchste gefundene Id zurück (damit sie nicht neu vergeben wird).
pub(super) fn sweep_session_files(dir: &Path) -> u64 {
    let mut max = 0;
    for sub in ["chats", "scrollback"] {
        let Ok(rd) = std::fs::read_dir(dir.join(sub)) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let digits: String = name.chars().take_while(|c| c.is_ascii_digit()).collect();
            let rest = &name[digits.len()..];
            let Ok(id) = digits.parse::<u64>() else { continue };
            if id == 0 || !rest.starts_with('.') || rest.contains(".orphan-") || !e.file_type().is_ok_and(|t| t.is_file()) {
                continue;
            }
            max = max.max(id);
            let _ = std::fs::remove_file(e.path());
        }
    }
    max
}

/// Beenden mit „aus“: bei Bedarf sichern (`backup` und es gibt etwas zu verlieren), zugeschnittenen Zustand schreiben und
/// die Dateien der offenen Sitzungen löschen. Gibt die Sicherung zurück.
pub(super) fn trim_for_exit(dir: &Path, current: SavedState, open: impl IntoIterator<Item = u64>, lang: Lang, backup: bool) -> Option<PathBuf> {
    let bak = if backup && worth_keeping(&current) { serde_json::to_vec_pretty(&current).ok().and_then(|b| backup_state(dir, &b)) } else { None };
    state::save_json(&dir.join("state.json"), &forget(current, lang));
    remove_session_files(dir, open);
    bak
}

impl App {
    /// Speichern vor dem Beenden oder einem Neustart (Update, Sicherung einspielen). Mit „Sitzungen merken“ aus: Scrollback
    /// und Chatverläufe der offenen Sitzungen löschen und einen leeren Zustand schreiben (vorher sichern, wenn die
    /// Einstellung in diesem Lauf ausgeschaltet wurde); sonst wie `save_all`.
    pub(super) fn save_for_exit(&self) {
        if self.settings.remember_sessions {
            self.save_all();
            return;
        }
        state::save_json(&self.dir.join("settings.json"), &self.settings);
        state::save_json(&self.dir.join("prompts.json"), &self.prompts);
        trim_for_exit(&self.dir, self.saved_state(), self.entries.keys().copied(), state::persisted_lang(self.settings.language), self.remember_at_start);
    }

    /// Speichern vor einem Update-Neustart (#157): immer vollständig wie `save_all` – auch mit „Sitzungen merken“ aus, denn die
    /// neue Instanz stellt die Sitzungen dann einmal wieder her. Wurde die Einstellung in diesem Lauf ausgeschaltet, entsteht
    /// die Sicherung des Übergangs schon jetzt (die neue Instanz weiß nichts mehr davon und verwirft beim Beenden ohne
    /// Sicherung). Liefert, ob die neue Instanz die Sitzungen einmal wiederherstellen soll.
    pub(super) fn save_for_update_restart(&self) -> bool {
        self.save_all();
        if self.settings.remember_sessions {
            return false;
        }
        if self.remember_at_start {
            let s = self.saved_state();
            if worth_keeping(&s) {
                if let Ok(b) = serde_json::to_vec_pretty(&s) {
                    backup_state(&self.dir, &b);
                }
            }
        }
        true
    }

    /// Rückfrage beim Ausschalten von „Sitzungen merken“ (im Fenster, in dem die Einstellungen offen sind).
    pub(super) fn remember_off_ui(&mut self, ctx: &egui::Context) {
        if self.remember_off_ask != Some(self.wins.cur) {
            return;
        }
        let lang = self.settings.language;
        let (mut go, mut cancel) = (false, ctx.input(|i| i.key_pressed(Key::Escape)));
        egui::Window::new(lang.t("„Sitzungen merken“ ausschalten?", "Turn off “Remember sessions”?"))
            .id(Id::new("remember_off_ask"))
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_max_width(440.0);
                ui.label(lang.t("Beim nächsten Beenden verwirft Paneldeck:", "On the next exit Paneldeck discards:"));
                for l in [
                    lang.t("alle Sitzungen (Terminals und Chats)", "all sessions (terminals and chats)"),
                    lang.t("die Chatverläufe", "the chat histories"),
                    lang.t("alle Projekte und Ordner im Terminal Hub", "all projects and folders in the Terminal Hub"),
                ] {
                    ui.label(format!("• {l}"));
                }
                ui.add_space(S1);
                ui.label(lang.t(
                    "Danach startet Paneldeck jedes Mal leer. Einstellungen, Todo-Liste und Tickets bleiben erhalten.",
                    "After that every start is empty. Settings, the to-do list and tickets are kept.",
                ));
                ui.label(RichText::new(lang.t(
                    "Vorher wird der aktuelle Stand als state.json.bak-<Datum> im Datenordner gesichert (nur die neueste Sicherung bleibt).",
                    "The current state is backed up first as state.json.bak-<date> in the data folder (only the newest backup is kept).",
                )).small().color(style::dim(ui)));
                ui.add_space(S2);
                ui.horizontal(|ui| {
                    go = ui.button(lang.t("Ausschalten", "Turn off")).clicked();
                    cancel |= ui.button(lang.t("Abbrechen", "Cancel")).clicked();
                });
            });
        if go {
            self.settings.remember_sessions = false;
        }
        if go || cancel {
            self.remember_off_ask = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: u64, window: u64) -> SessionMeta {
        SessionMeta { id, window, project: 2, group: "a/b".into(), ..SessionMeta::default() }
    }

    fn tmp(tag: u32) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pd-fresh-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn forget_keeps_only_preferences() {
        let folder = state::Folder { path: "a/b".into(), ..Default::default() };
        let saved = SavedState {
            sessions: vec![meta(3, 0), meta(4, 1)],
            layout: Some(Node::Leaf(3)),
            focused: 3,
            next_id: 9,
            projects: vec![
                Project { id: 1, name: "One".into(), tree_view: false, ..Project::default() },
                Project { id: 2, name: "Two".into(), folder: "C:\\x".into(), tree_view: true, folders: vec![folder], layout: Some(Node::Leaf(3)), focused: 3, ..Project::default() },
            ],
            active_project: 2,
            detached: vec![3],
            composer_draft: "draft".into(),
            extra_windows: vec![state::SavedWindow { id: 1, project: 2, ..Default::default() }],
        };
        assert!(worth_keeping(&saved));
        let f = forget(saved, Lang::En);
        assert!(!worth_keeping(&f), "zugeschnitten ist nichts mehr zu verlieren");
        assert!(f.sessions.is_empty() && f.layout.is_none() && f.focused == 0);
        assert!(f.detached.is_empty() && f.extra_windows.is_empty());
        assert_eq!(f.next_id, 9, "Ids werden nie wiederverwendet");
        assert_eq!(f.composer_draft, "draft");
        assert_eq!(f.active_project, 1);
        assert_eq!(f.projects.len(), 1);
        let p = &f.projects[0];
        assert_eq!((p.id, p.name.as_str(), p.folder.as_str()), (1, "Default", ""));
        assert!(p.folders.is_empty() && p.layout.is_none());
        assert!(p.tree_view, "Baumansicht des aktiven Projekts bleibt (Darstellung)");
        // Deutsch und ohne Projekte (alter Stand)
        let f = forget(SavedState { next_id: 2, ..Default::default() }, Lang::De);
        assert_eq!(f.projects[0].name, "Standard");
        assert!(!f.projects[0].tree_view);
        // Ein einzelnes Projekt mit Ordner ist etwas wert, ein leerer Zustand nicht.
        assert!(!worth_keeping(&SavedState::default()));
        let one = SavedState { projects: vec![Project { id: 1, folders: vec![state::Folder { path: "x".into(), ..Default::default() }], ..Project::default() }], ..Default::default() };
        assert!(worth_keeping(&one));
    }

    #[test]
    fn forgotten_state_roundtrips_through_load_saved() {
        let dir = tmp(line!());
        let path = dir.join("state.json");
        let saved = SavedState { sessions: vec![meta(5, 0)], next_id: 6, extra_windows: vec![state::SavedWindow { id: 1, ..Default::default() }], ..Default::default() };
        state::save_json(&path, &forget(saved, Lang::En));
        let (back, note) = state::load_saved(&path);
        assert!(note.is_none());
        assert!(back.sessions.is_empty() && back.extra_windows.is_empty());
        assert_eq!(back.next_id, 6);
        // Eine ältere Version liest dieselbe Datei (kein `extra_windows`, keine Sitzungen).
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(v.get("extra_windows").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn removes_scrollback_and_chat_files_of_the_sessions_only() {
        let dir = tmp(line!());
        let (sb, chats) = (dir.join("scrollback"), chatmod::store::dir(&dir));
        std::fs::create_dir_all(&sb).unwrap();
        std::fs::create_dir_all(&chats).unwrap();
        for f in [sb.join("1.txt"), sb.join("2.txt"), sb.join("7.txt")] {
            std::fs::write(f, "x").unwrap();
        }
        for f in ["1.json", "1.defekt-5.json", "2.json", "2.orphan-9.json", "7.json"] {
            std::fs::write(chats.join(f), "{}").unwrap();
        }
        remove_session_files(&dir, [1, 2]);
        assert!(!sb.join("1.txt").exists() && !sb.join("2.txt").exists());
        assert!(!chats.join("1.json").exists() && !chats.join("1.defekt-5.json").exists() && !chats.join("2.json").exists());
        assert!(sb.join("7.txt").exists() && chats.join("7.json").exists(), "fremde Sitzungen bleiben");
        assert!(chats.join("2.orphan-9.json").exists(), "beiseitegelegte Verläufe bleiben");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sweep_removes_all_session_files_but_keeps_orphans_and_others() {
        let dir = tmp(line!());
        let (sb, chats) = (dir.join("scrollback"), chatmod::store::dir(&dir));
        std::fs::create_dir_all(&sb).unwrap();
        std::fs::create_dir_all(&chats).unwrap();
        // 3 steht in state.json, 12 entstand nach der letzten Speicherung vor einem Absturz (höher als next_id).
        for f in [sb.join("3.txt"), sb.join("12.txt"), sb.join("notes.txt")] {
            std::fs::write(f, "x").unwrap();
        }
        for f in ["3.json", "12.json", "12.defekt-1.json", "4.orphan-7.json", "0.json", "readme.json"] {
            std::fs::write(chats.join(f), "{}").unwrap();
        }
        std::fs::create_dir_all(chats.join("5.dir")).unwrap();
        assert_eq!(sweep_session_files(&dir), 12);
        for gone in [sb.join("3.txt"), sb.join("12.txt"), chats.join("3.json"), chats.join("12.json"), chats.join("12.defekt-1.json")] {
            assert!(!gone.exists(), "{}", gone.display());
        }
        for kept in [sb.join("notes.txt"), chats.join("4.orphan-7.json"), chats.join("0.json"), chats.join("readme.json"), chats.join("5.dir")] {
            assert!(kept.exists(), "{}", kept.display());
        }
        // Ohne Ordner: nichts zu tun.
        assert_eq!(sweep_session_files(&dir.join("missing")), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_restart_restores_only_once_and_only_when_off() {
        assert!(restore_once_applies(false, true), "aus + Update-Neustart: einmal wiederherstellen");
        assert!(!restore_once_applies(false, false), "aus + normaler Start (auch nach Absturz): frisch");
        assert!(!restore_once_applies(true, true), "an: normale Wiederherstellung, kein Sonderfall");
        assert!(!restore_once_applies(true, false));
    }

    #[test]
    fn backup_keeps_only_the_newest() {
        let dir = tmp(line!());
        std::fs::write(dir.join("state.json.bak-20200101-000000"), "old").unwrap();
        std::fs::write(dir.join("settings.json.bak-20200101-000000"), "other").unwrap();
        let a = backup_state(&dir, b"one").unwrap();
        let b = backup_state(&dir, b"two").unwrap();
        assert_ne!(a, b, "gleiche Sekunde: Zähler im Namen");
        assert!(b.file_name().unwrap().to_string_lossy().starts_with("state.json.bak-"));
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "two");
        let baks: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.starts_with("state.json.bak-")).collect();
        assert_eq!(baks.len(), 1, "nur die neueste: {baks:?}");
        assert!(dir.join("settings.json.bak-20200101-000000").exists(), "andere Sicherungen bleiben");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn trim_for_exit_writes_backup_trims_state_and_deletes_files() {
        let dir = tmp(line!());
        let (sb, chats) = (dir.join("scrollback"), chatmod::store::dir(&dir));
        std::fs::create_dir_all(&sb).unwrap();
        std::fs::create_dir_all(&chats).unwrap();
        std::fs::write(sb.join("1.txt"), "x").unwrap();
        std::fs::write(chats.join("2.json"), "{}").unwrap();
        let folder = state::Folder { path: "f".into(), ..Default::default() };
        let current = SavedState { sessions: vec![meta(1, 0), meta(2, 0)], next_id: 3, projects: vec![Project { id: 2, folders: vec![folder], ..Project::default() }], active_project: 2, ..Default::default() };
        // Einstellung in diesem Lauf ausgeschaltet: Sicherung mit dem vollen Stand.
        let bak = trim_for_exit(&dir, current, [1, 2], Lang::En, true).expect("Sicherung");
        let full: SavedState = serde_json::from_str(&std::fs::read_to_string(&bak).unwrap()).unwrap();
        assert_eq!(full.sessions.len(), 2);
        assert_eq!(full.projects[0].folders.len(), 1);
        let (st, _) = state::load_saved(&dir.join("state.json"));
        assert!(st.sessions.is_empty() && st.projects.len() == 1 && st.projects[0].folders.is_empty());
        assert_eq!(st.next_id, 3);
        assert!(!sb.join("1.txt").exists() && !chats.join("2.json").exists());
        // Schon beim Start aus: keine neue Sicherung (die vom Übergang bleibt).
        let again = SavedState { sessions: vec![meta(4, 0)], next_id: 5, ..Default::default() };
        assert!(trim_for_exit(&dir, again, [4], Lang::En, false).is_none());
        assert!(bak.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
