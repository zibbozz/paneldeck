//! Listen-Editor (#289): eine Liste von Ordnern oder Dateien mit **+** und **−** daneben – für die zusätzlichen Ordner
//! (`--add-dir`), Plugin-Ordner (`--plugin-dir`) und MCP-Konfigurationen (`--mcp-config`) der Claude-Optionen.
//!
//! - Scrollbare Liste; ein Eintrag wird mit Klick (oder Tastatur: Tab auf die Liste, Pfeil hoch/runter) gewählt.
//! - **+** öffnet den Ordner- bzw. Dateidialog (JSON); für WSL-Sitzungen startet er in `\\wsl.localhost\<Distro>\` und
//!   liefert den Linux-Pfad ([`pick_entry`]). Doppelte werden nicht erneut eingetragen.
//! - **−** (oder Entf auf einem gewählten Eintrag) fragt in einem kleinen Popup nach („Diesen Ordner aus der Liste
//!   entfernen?“). Das Popup nimmt den Fokus (auf „Abbrechen“, #255), Esc schließt nur das Popup und wird verbraucht –
//!   der Dialog dahinter bleibt offen (er verbraucht Esc nur, solange kein Popup offen ist).
//! - Ungültige Einträge erscheinen rot mit Begründung als Tooltip (Prüfung beim Aufrufer).
//!
//! Der Dialog zum Auswählen wird vom Aufrufer übergeben (`pick`), damit die Tests ohne Fenster laufen.

use super::folder_picker::Target;
use crate::state::Lang;
use eframe::egui::{self, Color32, Id, Key, Modifiers, Popup, PopupCloseBehavior, RichText, Ui};

/// Was die Liste enthält (Texte, Dialog).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ListKind {
    /// Ordner (Ordnerdialog).
    Folders,
    /// JSON-Dateien (Dateidialog mit Filter).
    JsonFiles,
}

impl ListKind {
    fn confirm_text(self, lang: Lang) -> &'static str {
        match self {
            ListKind::Folders => lang.t("Diesen Ordner aus der Liste entfernen?", "Remove this folder from the list?"),
            ListKind::JsonFiles => lang.t("Diese Datei aus der Liste entfernen?", "Remove this file from the list?"),
        }
    }

    fn add_tip(self, lang: Lang) -> &'static str {
        match self {
            ListKind::Folders => lang.t("Ordner hinzufügen …", "Add folder …"),
            ListKind::JsonFiles => lang.t("JSON-Datei hinzufügen …", "Add JSON file …"),
        }
    }

    fn remove_tip(self, lang: Lang) -> &'static str {
        match self {
            ListKind::Folders => lang.t("Gewählten Ordner entfernen", "Remove the selected folder"),
            ListKind::JsonFiles => lang.t("Gewählte Datei entfernen", "Remove the selected file"),
        }
    }

    fn empty_text(self, lang: Lang) -> &'static str {
        match self {
            ListKind::Folders => lang.t("Keine Ordner", "No folders"),
            ListKind::JsonFiles => lang.t("Keine Dateien", "No files"),
        }
    }
}

/// Zustand je Liste (im Speicher von egui).
#[derive(Clone, Default)]
struct State {
    selected: Option<usize>,
    /// Beim nächsten Bild den Fokus auf „Abbrechen“ im Popup legen.
    focus_cancel: bool,
    /// Den Fokus auf den gewählten Eintrag legen (Pfeiltasten).
    focus_row: bool,
    /// Hinweis nach **+** (z. B. schon in der Liste, andere Distribution).
    note: Option<String>,
}

/// Breite der Liste.
const LIST_W: f32 = 236.0;
/// Höchsthöhe der Liste (danach scrollt sie).
const LIST_H: f32 = 84.0;

/// Rechteck eines Elements für die Tests ohne Fenster.
fn mark(ui: &Ui, salt: &str, key: &'static str, r: &egui::Response) {
    #[cfg(test)]
    ui.data_mut(|d| d.insert_temp(Id::new(("list_mark", salt.to_string(), key)), r.rect));
    let _ = (ui, salt, key, r);
}

/// Die Liste mit **+**/**−** zeichnen. `issue(eintrag)` liefert eine Begründung, wenn der Eintrag ungültig ist;
/// `pick` öffnet den Dialog und liefert den neuen Eintrag samt optionalem Hinweis (`Ok(None)` = abgebrochen, `Err` =
/// Hinweis statt Eintrag). Doppelte zählen ohne Groß-/Kleinschreibung und Trenner am Ende (Windows-Pfade). `max` =
/// Höchstzahl der Einträge. Rückgabe: Liste geändert.
pub(super) fn list_editor(
    ui: &mut Ui,
    lang: Lang,
    salt: &str,
    items: &mut Vec<String>,
    kind: ListKind,
    max: usize,
    issue: &dyn Fn(&str) -> Option<String>,
    pick: &mut dyn FnMut() -> Result<Option<(String, Option<String>)>, String>,
) -> bool {
    let id = Id::new(("list_edit", salt.to_string()));
    let mut st: State = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    if st.selected.is_some_and(|i| i >= items.len()) {
        st.selected = None;
    }
    let mut changed = false;
    let red = Color32::from_rgb(0xe0, 0x6c, 0x75);
    ui.vertical(|ui| {
        ui.horizontal(|ui| {
            // Liste in einem Rahmen, damit sie auch leer als Feld erkennbar ist.
            let frame = egui::Frame::group(ui.style()).inner_margin(egui::Margin::same(3));
            let list = frame.show(ui, |ui| {
                ui.set_width(LIST_W);
                egui::ScrollArea::vertical().id_salt(("list_edit_scroll", salt)).max_height(LIST_H).auto_shrink([false, true]).show(ui, |ui| {
                    if items.is_empty() {
                        ui.label(RichText::new(kind.empty_text(lang)).small().color(crate::style::dim(ui)));
                    }
                    let mut remove_key = false;
                    let mut step: i32 = 0;
                    for (i, it) in items.iter().enumerate() {
                        let bad = issue(it);
                        let text = if bad.is_some() { RichText::new(it).color(red) } else { RichText::new(it) };
                        let sel = st.selected == Some(i);
                        let r = ui.add(egui::Button::selectable(sel, text).truncate());
                        let r = match &bad {
                            Some(why) => r.on_hover_text(why),
                            None => r.on_hover_text(it),
                        };
                        if i == 0 {
                            mark(ui, salt, "row0", &r);
                        }
                        if r.clicked() {
                            // Gewählt und mit Fokus: Pfeiltasten und Entf wirken gleich danach.
                            st.selected = Some(i);
                            st.focus_row = true;
                        }
                        if sel && st.focus_row {
                            r.request_focus();
                            r.scroll_to_me(None);
                            st.focus_row = false;
                        }
                        if r.has_focus() {
                            if st.selected != Some(i) && r.gained_focus() {
                                st.selected = Some(i);
                            }
                            ui.input_mut(|inp| {
                                if inp.consume_key(Modifiers::NONE, Key::ArrowDown) {
                                    step = 1;
                                }
                                if inp.consume_key(Modifiers::NONE, Key::ArrowUp) {
                                    step = -1;
                                }
                                if inp.consume_key(Modifiers::NONE, Key::Delete) {
                                    remove_key = true;
                                }
                            });
                        }
                    }
                    if step != 0 && !items.is_empty() {
                        let cur = st.selected.unwrap_or(0) as i32;
                        st.selected = Some((cur + step).clamp(0, items.len() as i32 - 1) as usize);
                        st.focus_row = true;
                    }
                    remove_key
                })
                .inner
            });
            let remove_key = list.inner;
            mark(ui, salt, "list", &list.response);
            ui.vertical(|ui| {
                let full = items.len() >= max;
                let add = ui.add_enabled(!full, egui::Button::new("+").min_size(egui::vec2(24.0, 0.0))).on_hover_text(kind.add_tip(lang)).on_disabled_hover_text(lang.t("Die Liste ist voll.", "The list is full."));
                mark(ui, salt, "add", &add);
                if add.clicked() {
                    st.note = None;
                    match pick() {
                        Ok(Some((p, note))) => {
                            let p = p.trim().to_string();
                            st.note = note;
                            let key = crate::claude_opts::path_key(&p);
                            match items.iter().position(|x| crate::claude_opts::path_key(x) == key) {
                                Some(i) => {
                                    st.selected = Some(i);
                                    st.note = Some(lang.t("Schon in der Liste.", "Already in the list.").to_string());
                                }
                                None if !p.is_empty() => {
                                    items.push(p);
                                    st.selected = Some(items.len() - 1);
                                    st.focus_row = true;
                                    changed = true;
                                }
                                None => {}
                            }
                        }
                        Ok(None) => {}
                        Err(note) => st.note = Some(note),
                    }
                }
                let minus = ui.add_enabled(st.selected.is_some(), egui::Button::new("−").min_size(egui::vec2(24.0, 0.0))).on_hover_text(kind.remove_tip(lang)).on_disabled_hover_text(lang.t("Erst einen Eintrag wählen.", "Select an entry first."));
                mark(ui, salt, "remove", &minus);
                let pid = Popup::default_response_id(&minus);
                if (minus.clicked() || remove_key) && st.selected.is_some() {
                    Popup::open_id(ui.ctx(), pid);
                    st.focus_cancel = true;
                }
                let sel_text = st.selected.and_then(|i| items.get(i)).cloned().unwrap_or_default();
                let mut remove = false;
                if Popup::is_id_open(ui.ctx(), pid) {
                    let mut close = false;
                    Popup::from_response(&minus).id(pid).open_memory(None).close_behavior(PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
                        ui.set_max_width(320.0);
                        ui.label(kind.confirm_text(lang));
                        ui.add(egui::Label::new(RichText::new(&sel_text).monospace().small()).wrap());
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            let rm = ui.button(lang.t("Entfernen", "Remove"));
                            mark(ui, salt, "confirm_remove", &rm);
                            if rm.clicked() {
                                remove = true;
                                close = true;
                            }
                            let cancel = ui.button(lang.t("Abbrechen", "Cancel"));
                            mark(ui, salt, "confirm_cancel", &cancel);
                            if st.focus_cancel {
                                cancel.request_focus();
                                st.focus_cancel = false;
                            }
                            if cancel.clicked() {
                                close = true;
                            }
                        });
                        // Esc schließt nur die Rückfrage und erreicht weder den Dialog noch das Terminal dahinter.
                        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                            close = true;
                        }
                    });
                    if close {
                        Popup::close_id(ui.ctx(), pid);
                        st.focus_row = !remove;
                    }
                }
                if remove {
                    if let Some(i) = st.selected.filter(|i| *i < items.len()) {
                        items.remove(i);
                        changed = true;
                        st.selected = if items.is_empty() { None } else { Some(i.min(items.len() - 1)) };
                        st.focus_row = st.selected.is_some();
                    }
                }
            });
        });
        if let Some(n) = &st.note {
            ui.label(RichText::new(n).small().color(crate::style::dim(ui)));
        }
    });
    ui.data_mut(|d| d.insert_temp(id, st));
    changed
}

/// Neuen Eintrag über den Dialog wählen: Ordner bzw. JSON-Datei. Für WSL-Sitzungen startet der Dialog in
/// `\\wsl.localhost\<Distro>\` (ungeprüft, siehe `folder_picker`); ein Pfad dort wird zum Linux-Pfad. `linux_ok`: auch
/// ohne WSL-Shell Linux-Pfade annehmen (Standards, Vorlagen – sie gelten dann nur in WSL-Sitzungen). Windows-Pfade
/// bleiben, wie sie sind (sie werden beim Start für WSL umgerechnet).
pub(super) fn pick_entry(kind: ListKind, target: &Target, lang: Lang, linux_ok: bool) -> Result<Option<(String, Option<String>)>, String> {
    let start = match target {
        Target::Wsl { distro, .. } => format!(r"\\wsl.localhost\{}", distro.as_deref().map(|d| format!("{d}\\")).unwrap_or_default()),
        Target::Windows => String::new(),
    };
    let picked = match (kind, target) {
        (ListKind::Folders, Target::Wsl { .. }) => crate::platform::notify::pick_folder_unchecked(&start),
        (ListKind::Folders, Target::Windows) => crate::platform::notify::pick_folder(""),
        (ListKind::JsonFiles, _) => crate::platform::notify::pick_json(&start),
    };
    match picked {
        Some(p) => picked_to_entry(&p, target, lang, linux_ok).map(Some),
        None => Ok(None),
    }
}

/// Gewählten Pfad als Eintrag (mit Hinweis): `\\wsl.localhost\<D>\…` → Linux-Pfad – für eine WSL-Sitzung derselben
/// Distribution; ist ihre Distribution die Standard-Distribution (unbekannt, welche), mit Hinweis; ohne WSL-Shell nur mit
/// `linux_ok` und Hinweis. Sonst unverändert.
pub(super) fn picked_to_entry(p: &str, target: &Target, lang: Lang, linux_ok: bool) -> Result<(String, Option<String>), String> {
    let Some((d, linux)) = crate::wsl::unc_to_linux(p) else {
        return Ok((p.to_string(), None));
    };
    if !(crate::claude_opts::wsl_linux_path(&linux) || (!cfg!(windows) && crate::claude_opts::valid_path(&linux))) {
        return Err(format!("{}: {}", lang.q(&linux), lang.t("Pfad mit unerlaubten Zeichen (\" % ! ^ & | < > \\)", "path with characters that are not allowed (\" % ! ^ & | < > \\)")));
    }
    match target {
        Target::Wsl { distro: Some(x), .. } if !x.eq_ignore_ascii_case(&d) => Err(if lang == Lang::De {
            format!("{} liegt in der Distribution {}, die Sitzung nutzt {}.", lang.q(p), lang.q(&d), lang.q(x))
        } else {
            format!("{} is in the distribution {}, the session uses {}.", lang.q(p), lang.q(&d), lang.q(x))
        }),
        Target::Wsl { distro: Some(_), .. } => Ok((linux, None)),
        // Standard-Distribution: welche das ist, lässt sich hier nicht prüfen (wsl.exe könnte die Oberfläche blockieren).
        Target::Wsl { distro: None, .. } => Ok((
            linux,
            Some(if lang == Lang::De {
                format!("Gilt nur, wenn {} deine Standard-Distribution ist.", lang.q(&d))
            } else {
                format!("Applies only if {} is your default WSL distribution.", lang.q(&d))
            }),
        )),
        Target::Windows if linux_ok => Ok((linux, Some(lang.t("Linux-Pfad: gilt nur in WSL-Sitzungen.", "Linux path: applies only to WSL sessions.").to_string()))),
        Target::Windows => Err(lang.t("Ordner in WSL gibt es nur für WSL-Sitzungen.", "WSL folders are only available for WSL sessions.").to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, Pos2, RawInput, Rect};

    struct H {
        ctx: egui::Context,
        items: Vec<String>,
        picks: Vec<Result<Option<(String, Option<String>)>, String>>,
        changed: bool,
        esc_left: bool,
    }

    impl H {
        fn new(items: &[&str]) -> H {
            H { ctx: egui::Context::default(), items: items.iter().map(|s| s.to_string()).collect(), picks: Vec::new(), changed: false, esc_left: false }
        }

        fn frame(&mut self, events: Vec<Event>) {
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))), events, ..Default::default() };
            let (items, picks) = (&mut self.items, &mut self.picks);
            let mut changed = false;
            let mut esc_left = false;
            let _ = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let issue = |p: &str| if p.contains("bad") { Some("bad entry".to_string()) } else { None };
                    let mut pick = || if picks.is_empty() { Ok(None) } else { picks.remove(0) };
                    changed = list_editor(ui, Lang::En, "t", items, ListKind::Folders, 3, &issue, &mut pick);
                });
                esc_left = ctx.input(|i| i.key_pressed(Key::Escape));
            });
            self.changed = changed;
            self.esc_left = esc_left;
        }

        fn rect(&self, key: &'static str) -> Option<Rect> {
            self.ctx.data(|d| d.get_temp::<Rect>(Id::new(("list_mark", "t".to_string(), key))))
        }

        fn click(&mut self, key: &'static str) {
            let at = self.rect(key).unwrap_or_else(|| panic!("{key} fehlt")).center();
            self.frame(vec![Event::PointerMoved(at)]);
            self.frame(vec![Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
            self.frame(vec![Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
            self.frame(vec![]);
        }

        fn key(&mut self, key: Key) {
            self.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
            self.frame(vec![]);
        }

        fn popup_open(&self) -> bool {
            Popup::is_any_open(&self.ctx)
        }
    }

    #[test]
    fn plus_adds_picked_entries_without_duplicates() {
        let mut h = H::new(&[]);
        h.frame(vec![]);
        h.frame(vec![]);
        h.picks = vec![Ok(Some((r"C:\a".into(), None)))];
        h.click("add");
        assert_eq!(h.items, [r"C:\a"]);
        // Abgebrochen: nichts passiert.
        h.click("add");
        assert_eq!(h.items, [r"C:\a"]);
        // Doppelt: nicht erneut, Hinweis.
        h.picks = vec![Ok(Some((r"C:\a".into(), None)))];
        h.click("add");
        assert_eq!(h.items, [r"C:\a"]);
        // Doppelt ohne Groß-/Kleinschreibung und Trenner am Ende.
        h.picks = vec![Ok(Some((r"c:\A\".into(), None)))];
        h.click("add");
        assert_eq!(h.items, [r"C:\a"]);
        // Hinweis statt Eintrag (z. B. andere Distribution).
        h.picks = vec![Err("note".into())];
        h.click("add");
        assert_eq!(h.items.len(), 1);
        // Voll: + gesperrt.
        h.picks = vec![Ok(Some((r"C:\b".into(), None))), Ok(Some((r"C:\c".into(), None))), Ok(Some((r"C:\d".into(), None)))];
        h.click("add");
        h.click("add");
        h.click("add");
        assert_eq!(h.items, [r"C:\a", r"C:\b", r"C:\c"], "höchstens 3");
    }

    #[test]
    fn minus_asks_before_removing() {
        let mut h = H::new(&[r"C:\a", r"C:\bad", r"C:\c"]);
        h.frame(vec![]);
        h.frame(vec![]);
        // Ohne Auswahl ist − gesperrt.
        h.click("remove");
        assert!(!h.popup_open());
        h.click("row0");
        h.click("remove");
        assert!(h.popup_open(), "Rückfrage offen");
        assert!(h.ctx.memory(|m| m.focused()).is_some(), "Fokus in der Rückfrage");
        // Abbrechen: bleibt.
        h.click("confirm_cancel");
        assert!(!h.popup_open());
        assert_eq!(h.items.len(), 3);
        // Entfernen
        h.click("remove");
        assert!(h.popup_open());
        h.click("confirm_remove");
        assert!(!h.popup_open());
        assert_eq!(h.items, [r"C:\bad", r"C:\c"]);
        assert!(h.changed || h.items.len() == 2);
    }

    #[test]
    fn keyboard_select_delete_and_escape() {
        let mut h = H::new(&[r"C:\a", r"C:\b", r"C:\c"]);
        h.frame(vec![]);
        h.frame(vec![]);
        h.click("row0");
        // Pfeil runter wählt den nächsten Eintrag, Entf fragt nach.
        h.key(Key::ArrowDown);
        h.key(Key::Delete);
        assert!(h.popup_open(), "Entf öffnet die Rückfrage");
        // Esc schließt nur die Rückfrage und wird verbraucht.
        h.frame(vec![Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }]);
        assert!(!h.esc_left, "Esc verbraucht");
        h.frame(vec![]);
        assert!(!h.popup_open());
        assert_eq!(h.items.len(), 3);
        // Entf, dann Enter auf dem fokussierten „Abbrechen“? Nein: Tab zu „Entfernen“ ist nicht nötig – Klick.
        h.key(Key::Delete);
        assert!(h.popup_open());
        h.click("confirm_remove");
        assert_eq!(h.items, [r"C:\a", r"C:\c"], "der zweite Eintrag (Pfeil runter) ist weg");
    }

    #[test]
    fn wsl_paths_from_the_dialog() {
        let wsl = Target::Wsl { distro: Some("Ubuntu".into()), choose_distro: false };
        let any = Target::Wsl { distro: None, choose_distro: false };
        let ok = |p: &str| Ok((p.to_string(), None));
        assert_eq!(picked_to_entry(r"C:\x", &wsl, Lang::En, false), ok(r"C:\x"));
        assert_eq!(picked_to_entry(r"C:\x", &Target::Windows, Lang::En, false), ok(r"C:\x"));
        assert_eq!(picked_to_entry(r"\\wsl.localhost\Ubuntu\home\u\p", &wsl, Lang::En, false), ok("/home/u/p"));
        assert_eq!(picked_to_entry(r"\\wsl.localhost\ubuntu\home\u\p", &wsl, Lang::En, false), ok("/home/u/p"), "Groß-/Kleinschreibung egal");
        // Standard-Distribution (unbekannt): angenommen, aber mit Hinweis – nicht still.
        let (p, note) = picked_to_entry(r"\\wsl$\Debian\srv", &any, Lang::En, false).unwrap();
        assert_eq!(p, "/srv");
        assert!(note.is_some_and(|n| n.contains("Debian")));
        assert!(picked_to_entry(r"\\wsl.localhost\Debian\srv", &wsl, Lang::En, false).is_err(), "andere Distribution");
        assert!(picked_to_entry(r"\\wsl.localhost\Ubuntu\srv", &Target::Windows, Lang::En, false).is_err(), "WSL-Pfad nur für WSL-Sitzungen");
        // Standards/Vorlagen: als Linux-Pfad mit Hinweis.
        let (p, note) = picked_to_entry(r"\\wsl.localhost\Ubuntu\srv", &Target::Windows, Lang::En, true).unwrap();
        assert_eq!(p, "/srv");
        assert!(note.is_some());
        assert_ne!(picked_to_entry(r"\\wsl.localhost\Debian\srv", &wsl, Lang::De, false), picked_to_entry(r"\\wsl.localhost\Debian\srv", &wsl, Lang::En, false));
        for k in [ListKind::Folders, ListKind::JsonFiles] {
            assert_ne!(k.confirm_text(Lang::De), k.confirm_text(Lang::En));
            assert_ne!(k.add_tip(Lang::De), k.add_tip(Lang::En));
            assert_ne!(k.remove_tip(Lang::De), k.remove_tip(Lang::En));
            assert_ne!(k.empty_text(Lang::De), k.empty_text(Lang::En));
        }
    }
}
