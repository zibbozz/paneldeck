//! Einstellung „Sitzungen merken“ (#157). Ist sie aus, startet Paneldeck jedes Mal frisch: keine Terminals, keine Chats,
//! keine Projekte/Ordner aus dem letzten Lauf, keine weiteren oder abgedockten Fenster. Was bleibt: die Einstellungen
//! (`settings.json`, also auch Fenstergröße/Breite der Seitenleiste), die Todo-Liste und Tickets (`paneldeck.db`), die
//! Darstellung der Seitenleiste (Baum/Liste) und der Entwurf im Prompt-Composer.
//!
//! Ablauf: Während des Laufs schreibt Paneldeck `state.json` wie gewohnt (mit Sitzungsliste) – stürzt es ab, findet der
//! nächste Start darüber die Dateien der Sitzungen und löscht sie. Beim Beenden ([`App::save_for_exit`]) werden Scrollback
//! und Chatverläufe der offenen Sitzungen gelöscht und ein zugeschnittener Zustand ([`forget`]) geschrieben.

use super::*;
use std::path::Path;

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

/// Dateien dieser Sitzungen löschen: Scrollback (`scrollback/<id>.txt`) und Chatverlauf (`chats/<id>.json` samt
/// Sicherungen beschädigter Dateien). Beiseitegelegte fremde Verläufe (`.orphan-`) bleiben.
pub(super) fn remove_session_files(dir: &Path, ids: impl IntoIterator<Item = u64>) {
    for id in ids {
        let _ = std::fs::remove_file(dir.join("scrollback").join(format!("{id}.txt")));
        chatmod::store::remove(dir, id);
    }
}

impl App {
    /// Speichern vor dem Beenden oder einem Neustart (Update, Sicherung einspielen). Mit „Sitzungen merken“ aus: Scrollback
    /// und Chatverläufe der offenen Sitzungen löschen und einen leeren Zustand schreiben; sonst wie `save_all`.
    pub(super) fn save_for_exit(&self) {
        if self.settings.remember_sessions {
            self.save_all();
            return;
        }
        state::save_json(&self.dir.join("settings.json"), &self.settings);
        state::save_json(&self.dir.join("prompts.json"), &self.prompts);
        state::save_json(&self.dir.join("state.json"), &forget(self.saved_state(), state::persisted_lang(self.settings.language)));
        remove_session_files(&self.dir, self.entries.keys().copied());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: u64, window: u64) -> SessionMeta {
        SessionMeta { id, window, project: 2, group: "a/b".into(), ..SessionMeta::default() }
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
        let f = forget(saved, Lang::En);
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
    }

    #[test]
    fn forgotten_state_roundtrips_through_load_saved() {
        let dir = std::env::temp_dir().join(format!("pd-fresh-{}-{}", std::process::id(), line!()));
        std::fs::create_dir_all(&dir).unwrap();
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
        let dir = std::env::temp_dir().join(format!("pd-fresh-{}-{}", std::process::id(), line!()));
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
}
