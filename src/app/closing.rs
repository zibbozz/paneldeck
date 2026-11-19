//! Sitzungen schließen mit Rückfrage: eine Sitzung (Schließen-Knopf, Kontextmenü, Mittelklick auf die Zeile in der
//! Seitenleiste, #155) und viele auf einmal (#156: Papierkorb am Ordner, Papierkorb rechts neben dem Filterfeld für alle
//! Sitzungen, Projekt-Kontextmenü, Befehls-Palette, Kürzel „close_all“). Die Rückfrage nennt, was beendet wird; nur beim
//! Schließen vieler Sitzungen gibt sie „Alle schließen“ erst nach 3 Sekunden frei (wie das Löschen von Tickets und Karten,
//! Entscheidung des Nutzers zu #156 – eine einzelne Sitzung fragt ohne Wartezeit wie in 2.2, [`wait_secs`]).
//!
//! * Eine Sitzung: Rückfrage nur bei laufendem Prozess bzw. Chat mit Verlauf und eingeschalteter Einstellung
//!   „Schließen bestätigen“, ohne Wartezeit; Sitzungen mit Git-Worktree gehen wie bisher über die Worktree-Rückfrage.
//! * Viele: Rückfrage immer (wie beim Schließen eines weiteren Fensters, da viele Sitzungen auf einmal enden). Beendet
//!   werden genau die beim Öffnen aufgelisteten Sitzungen; Git-Worktrees und Branches bleiben dabei bestehen.
//! * Hintergrundprozesse (#168): Laufen im Job Object einer Sitzung noch Prozesse, die das Schließen überleben würden
//!   (losgelöst gestartet, GUI-Programme – nicht die Shell und nichts an ihrer Konsole), nennt dieselbe Rückfrage sie und
//!   bietet „Weiterlaufen lassen“ (nur die Sitzung schließen, wie bisher) oder „Alle beenden“ (`TerminateJobObject`).
//!   Eine Sitzung: immer mit dieser Frage, ohne Wartezeit; viele und das Beenden von Paneldeck: eine gemeinsame Frage
//!   mit Wartezeit. Neu laden (#169) fragt genauso (`session_settings::reload_ask_ui`). MCP `close_session` fragt nie
//!   (lässt weiterlaufen und meldet `remaining_processes`).

use super::planning_ui::{countdown_secs, danger_button};
use super::*;
use crate::platform::process::JobProc;

/// Welche Sitzungen „alle schließen“ beendet.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum CloseScope {
    /// Ordner des aktiven Projekts in diesem Fenster, mit Unterordnern (auch minimierte Sitzungen und Chats).
    Folder(String),
    /// Alle Sitzungen dieses Projekts in diesem Fenster.
    Project(u64),
    /// Wirklich alle Sitzungen: alle Projekte, alle Fenster, auch abgedockte.
    All,
    /// Ordner löschen (Papierkorb an der Ordnerzeile, #276): Sitzungen wie bei `Folder` (Projekt, Pfad), danach
    /// verschwindet der Ordner samt Unterordnern und ihren Vorgaben.
    DeleteFolder(u64, String),
}

pub(super) enum CloseTarget {
    One(u64),
    Many(CloseScope, Vec<u64>),
    /// Paneldeck beenden, obwohl Sitzungen Hintergrundprozesse haben (#168).
    Quit,
    /// Nur den Ordner löschen (Projekt, Pfad); Sitzungen und Unterordner rücken eine Ebene hoch (#276).
    FolderLift(u64, String),
    /// Projekt löschen; seine Sitzungen wandern in ein anderes Projekt (#276, früher „zweimal klicken“).
    DeleteProject(u64),
    /// Sitzungs-Vorlage löschen (Index und Name; gelöscht wird nur, wenn beide noch passen – #244/#276).
    DeleteTemplate(usize, String),
}

/// Offene Rückfrage (je Fenster, siehe `WinView::confirm_close`).
pub(super) struct CloseAsk {
    pub(super) target: CloseTarget,
    /// Beginn der Wartezeit (3 s, `countdown_secs`; nur bei vielen Sitzungen, siehe [`wait_secs`]).
    pub(super) since: Instant,
    /// Hintergrundprozesse der betroffenen Sitzungen beim Öffnen der Rückfrage (#168).
    pub(super) lingering: Vec<Lingering>,
}

/// Restliche Wartezeit der Rückfrage in Sekunden: beim Schließen vieler Sitzungen (Ordner-Papierkorb, „alle schließen“)
/// und beim Beenden mit Hintergrundprozessen 3 Sekunden; eine einzelne Sitzung (Schließen-Knopf, Mittelklick,
/// Kontextmenü) wartet nicht – außer sie läuft noch und hat Hintergrundprozesse (`running_with_jobs`, #168).
pub(super) fn wait_secs(target: &CloseTarget, running_with_jobs: bool, since: Instant, now: Instant) -> u32 {
    match target {
        CloseTarget::One(_) if !running_with_jobs => 0,
        // Löschen (Ordner, Projekt, Vorlage) wartet wie „Alle schließen“ (#276).
        CloseTarget::One(_) | CloseTarget::Many(..) | CloseTarget::Quit | CloseTarget::FolderLift(..) | CloseTarget::DeleteProject(_) | CloseTarget::DeleteTemplate(..) => countdown_secs(since, now),
    }
}

/// Was beim Löschen eines Ordners (Inhalt bleibt) passiert: „3 Sitzungen und 1 Unterordner rücken nach „a“ hoch.“
pub(super) fn lift_summary(sessions: usize, subfolders: usize, parent: &str, lang: Lang) -> String {
    let what = match (sessions, subfolders) {
        (0, 0) => return lang.t("Der Ordner ist leer.", "The folder is empty.").to_string(),
        (s, 0) => n_of(s, lang, ("Sitzung", "Sitzungen"), ("session", "sessions")),
        (0, f) => n_of(f, lang, ("Unterordner", "Unterordner"), ("subfolder", "subfolders")),
        (s, f) => format!(
            "{} {} {}",
            n_of(s, lang, ("Sitzung", "Sitzungen"), ("session", "sessions")),
            lang.t("und", "and"),
            n_of(f, lang, ("Unterordner", "Unterordner"), ("subfolder", "subfolders"))
        ),
    };
    let one = sessions + subfolders == 1;
    match (lang, parent.is_empty()) {
        (Lang::De, true) => format!("{what} {} auf die oberste Ebene.", if one { "rückt" } else { "rücken" }),
        (Lang::De, false) => format!("{what} {} nach {} hoch.", if one { "rückt" } else { "rücken" }, lang.q(parent)),
        (Lang::En, true) => format!("{what} {} to the top level.", if one { "moves" } else { "move" }),
        (Lang::En, false) => format!("{what} {} up to {}.", if one { "moves" } else { "move" }, lang.q(parent)),
    }
}

/// Darf ein Ordner ohne Rückfrage verschwinden? Nur ganz leer: keine Sitzungen in irgendeinem Fenster (`content.0`,
/// [`App::folder_content`]) und keine Unterordner (`content.1`) – auch wenn dieses Fenster keine Sitzungen darin zeigt
/// (#276 Review).
pub(super) fn empty_folder(content: (usize, usize)) -> bool {
    content == (0, 0)
}

/// Sitzungen des Ordners in anderen Fenstern (werden nicht geschlossen, rücken hoch, #276).
pub(super) fn elsewhere_line(n: usize, parent: &str, lang: Lang) -> String {
    let what = n_of(n, lang, ("Sitzung", "Sitzungen"), ("session", "sessions"));
    match (lang, parent.is_empty()) {
        (Lang::De, true) => format!("{what} in anderen Fenstern {} auf die oberste Ebene (bleiben offen).", if n == 1 { "rückt" } else { "rücken" }),
        (Lang::De, false) => format!("{what} in anderen Fenstern {} nach {} (bleiben offen).", if n == 1 { "rückt" } else { "rücken" }, lang.q(parent)),
        (Lang::En, true) => format!("{what} in other windows {} to the top level (they stay open).", if n == 1 { "moves" } else { "move" }),
        (Lang::En, false) => format!("{what} in other windows {} to {} (they stay open).", if n == 1 { "moves" } else { "move" }, lang.q(parent)),
    }
}

/// Unterordner, die beim Hochrücken umbenannt werden, weil ihr Name im übergeordneten Ordner schon vergeben ist.
pub(super) fn rename_lines(plan: &[(String, String)], deleted: &str, lang: Lang) -> Vec<String> {
    plan.iter()
        .filter(|(old, new)| *new != state::lift_path(old, deleted))
        .map(|(old, new)| match lang {
            Lang::De => format!("{} heißt danach {} (der Name ist dort schon vergeben).", lang.q(state::folder_leaf(old)), lang.q(state::folder_leaf(new))),
            Lang::En => format!("{} becomes {} (the name is already taken there).", lang.q(state::folder_leaf(old)), lang.q(state::folder_leaf(new))),
        })
        .collect()
}

/// Hinweis beim Löschen eines Ordners mit seinen Sitzungen: was außer den Sitzungen verschwindet.
pub(super) fn folder_delete_note(subfolders: usize, lang: Lang) -> String {
    if subfolders == 0 {
        lang.t("Danach wird der Ordner mit seinen Vorgaben für neue Sitzungen gelöscht.", "Afterwards the folder is deleted with its defaults for new sessions.").to_string()
    } else {
        // Dativ im Deutschen: „mit 1 Unterordner“, „mit 2 Unterordnern“.
        let n = n_of(subfolders, lang, ("Unterordner", "Unterordnern"), ("subfolder", "subfolders"));
        match lang {
            Lang::De => format!("Danach wird der Ordner mit {n} und den Vorgaben für neue Sitzungen gelöscht."),
            Lang::En => format!("Afterwards the folder is deleted with {n} and the defaults for new sessions."),
        }
    }
}

impl CloseAsk {
    pub(super) fn new(target: CloseTarget) -> Self {
        CloseAsk { target, since: Instant::now(), lingering: Vec::new() }
    }

    pub(super) fn with(mut self, lingering: Vec<Lingering>) -> Self {
        self.lingering = lingering;
        self
    }
}

/// Prozesse einer Sitzung, die ihr Schließen überleben würden (#168, `terminal::Session::lingering`).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Lingering {
    pub(super) id: u64,
    /// Job Object der Sitzung beim Öffnen der Rückfrage (`terminal::Session::job_key`). „Alle beenden“ gilt nur diesem
    /// Job: Wurde die Sitzung inzwischen neu gestartet, hat sie einen neuen (#213).
    pub(super) job: u64,
    pub(super) name: String,
    pub(super) procs: Vec<JobProc>,
}

/// Job Object einer geschlossenen oder neu gestarteten Sitzung, in dem noch Prozesse laufen (#218, #213). Der Hub
/// behält das Handle, bis der Job leer ist: Solange ordnet er übrig gebliebene Prozesse dieser Sitzung zu (als
/// Aufrufer mit ihrer Kette, `mcp::attribute`, `taint::set_retired`) und prüft nach [`RECHECK`] einmal, ob dort noch
/// etwas läuft, das die Rückfrage nicht genannt hat (Hilfsprozesse eines Vordergrundprogramms, die es überleben).
pub(super) struct Retired {
    pub(super) job: crate::platform::process::SessionJob,
    pub(super) session: u64,
    name: String,
    since: Instant,
    /// Prozesse, die die Rückfrage beim Schließen genannt hätte (der Nutzer kennt sie): nicht noch einmal melden.
    shown: Vec<u32>,
    checked: bool,
    /// In der Meldung genannte Prozesse (PID, Erzeugungszeit): nur sie beendet „Alle beenden“ der Meldung.
    listed: Vec<(u32, u64)>,
}

/// Kennzeichen im Token der Toast-Aktion „Alle beenden“ eines aufbewahrten Jobs (Rest = Job-Kennung).
pub(super) const RETIRED_TOKEN: u64 = 1 << 63;
/// Nach so langer Zeit wird ein aufbewahrter Job einmal auf übrige Prozesse geprüft – länger als die Frist, die Windows
/// Konsolenprogrammen nach CTRL_CLOSE_EVENT zum Beenden gibt (5 s), damit langsam endende keine Meldung auslösen.
const RECHECK: Duration = Duration::from_secs(6);

/// Prozesse für die Meldung „läuft noch“: ohne Konsolen-Hosts und ohne die schon in der Rückfrage genannten
/// (`shown`), dazu die Zeilen „Programmname (PID)“ – höchstens sechs, dann „…“. Die PIDs sind alle gemeldeten (auch die
/// hinter „…“); nur sie beendet „Alle beenden“ der Meldung.
pub(super) fn leftover_lines(pids: &[u32], shown: &[u32], name_of: impl Fn(u32) -> String) -> (Vec<u32>, Vec<String>) {
    let procs: Vec<(u32, String)> = pids
        .iter()
        .filter(|p| !shown.contains(p))
        .map(|p| (*p, name_of(*p)))
        .filter(|(_, n)| {
            let l = n.to_ascii_lowercase();
            l != "conhost.exe" && l != "openconsole.exe"
        })
        .collect();
    let mut v: Vec<String> = procs.iter().map(|(p, n)| format!("{} (PID {p})", if n.is_empty() { "?" } else { n.as_str() })).collect();
    if v.len() > 6 {
        v.truncate(6);
        v.push("…".into());
    }
    (procs.into_iter().map(|(p, _)| p).collect(), v)
}

/// Was „Alle beenden“ beendet: (Sitzung, Job beim Öffnen der Rückfrage) je Sitzung mit Hintergrundprozessen.
pub(super) fn job_refs(list: &[Lingering]) -> Vec<(u64, u64)> {
    list.iter().map(|l| (l.id, l.job)).collect()
}

/// Sitzungen aus `refs`, deren Job noch derselbe ist wie beim Öffnen der Rückfrage (`current` = Job jetzt, `None` ohne
/// laufende Sitzung). Eine inzwischen neu gestartete Sitzung bleibt unberührt (#213).
pub(super) fn jobs_to_end(refs: &[(u64, u64)], current: impl Fn(u64) -> Option<u64>) -> Vec<u64> {
    refs.iter().filter(|(id, job)| current(*id) == Some(*job)).map(|(id, _)| *id).collect()
}

/// Eine Zeile je Prozess: „notepad.exe (PID 42) · Fenster offen – Titel“.
pub(super) fn proc_line(p: &JobProc, lang: Lang) -> String {
    let mut s = format!("{} (PID {})", p.name, p.pid);
    if p.window {
        s.push_str(" · ");
        s.push_str(lang.t("Fenster offen", "window open"));
    }
    if !p.title.trim().is_empty() {
        s.push_str(" – ");
        s.push_str(p.title.trim());
    }
    s
}

/// Warnung unter der Liste: „Alle beenden“ beendet sofort, ohne zu speichern (#168).
pub(super) fn end_all_warning(list: &[Lingering], lang: Lang) -> String {
    let windows = list.iter().flat_map(|l| &l.procs).any(|p| p.window);
    let base = lang.t(
        "„Alle beenden“ beendet sie sofort – ohne zu speichern.",
        "“End all” terminates them immediately – without saving.",
    );
    if windows {
        format!(
            "{base} {}",
            lang.t("Programme mit offenem Fenster (z. B. Editor, Browser) besser vorher selbst speichern und schließen.", "Better save and close programs with an open window (e.g. an editor or browser) yourself first.")
        )
    } else {
        base.to_string()
    }
}

/// Einleitung der Prozessliste: wie viele Prozesse in wie vielen Sitzungen weiterlaufen würden.
pub(super) fn lingering_intro(list: &[Lingering], lang: Lang) -> String {
    let procs: usize = list.iter().map(|l| l.procs.len()).sum();
    let sessions = list.iter().filter(|l| !l.procs.is_empty()).count();
    let p = n_of(procs, lang, ("Hintergrundprozess", "Hintergrundprozesse"), ("background process", "background processes"));
    match (lang == Lang::De, procs == 1, sessions > 1) {
        (true, true, _) => format!("{p} läuft noch und bleibt nach dem Schließen bestehen, wenn du ihn nicht beendest:"),
        (true, false, false) => format!("{p} laufen noch und bleiben nach dem Schließen bestehen, wenn du sie nicht beendest:"),
        (true, false, true) => format!("{p} in {sessions} Sitzungen laufen noch und bleiben nach dem Schließen bestehen, wenn du sie nicht beendest:"),
        (false, true, _) => format!("{p} is still running and stays after closing unless you end it:"),
        (false, false, false) => format!("{p} are still running and stay after closing unless you end them:"),
        (false, false, true) => format!("{p} in {sessions} sessions are still running and stay after closing unless you end them:"),
    }
}

/// Abschnitt „Hintergrundprozesse“ einer Rückfrage; mit `per_session` steht der Name der Sitzung über ihren Prozessen.
pub(super) fn lingering_section(ui: &mut Ui, list: &[Lingering], lang: Lang, per_session: bool) {
    if list.iter().all(|l| l.procs.is_empty()) {
        return;
    }
    ui.add_space(S1);
    ui.label(lingering_intro(list, lang));
    egui::Frame::new().fill(ui.visuals().extreme_bg_color).corner_radius(CornerRadius::same(style::RADIUS)).inner_margin(Margin::symmetric(8, 6)).show(ui, |ui| {
        egui::ScrollArea::vertical().id_salt("lingering").max_height(160.0).auto_shrink([false, true]).show(ui, |ui| {
            for l in list.iter().filter(|l| !l.procs.is_empty()) {
                if per_session {
                    ui.label(RichText::new(&l.name).strong());
                }
                for p in &l.procs {
                    let line = RichText::new(proc_line(p, lang)).small();
                    ui.label(if p.window { line.strong() } else { line });
                }
            }
        });
    });
    ui.label(RichText::new(end_all_warning(list, lang)).small().color(ui.visuals().warn_fg_color));
}

/// Knöpfe der Rückfrage bei Hintergrundprozessen.
pub(super) fn keep_label(lang: Lang) -> &'static str {
    lang.t("Weiterlaufen lassen", "Keep running")
}

pub(super) fn end_all_label(lang: Lang) -> &'static str {
    lang.t("Alle beenden", "End all")
}

/// Eine Sitzung, wie sie in der Rückfrage steht.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct CloseItem {
    pub(super) id: u64,
    pub(super) name: String,
    pub(super) project: String,
    pub(super) chat: bool,
    /// Terminal mit laufendem Prozess bzw. Chat mit laufender Antwort.
    pub(super) running: bool,
    /// Claude-Sitzung (nur mit Claude-Modus).
    pub(super) claude: bool,
    /// Chat mit Verlauf (wird gelöscht).
    pub(super) history: bool,
    pub(super) minimized: bool,
    pub(super) worktree: bool,
    pub(super) detached: bool,
}

/// Zahl mit passender Einzahl/Mehrzahl.
fn n_of(n: usize, lang: Lang, de: (&str, &str), en: (&str, &str)) -> String {
    let (one, many) = if lang == Lang::De { de } else { en };
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Zusammenfassung der Rückfrage (eine Zeile je Art; leere Arten fehlen). Zuerst die Gesamtzahl.
pub(super) fn summary_lines(items: &[CloseItem], lang: Lang) -> Vec<String> {
    let count = |f: &dyn Fn(&CloseItem) -> bool| items.iter().filter(|i| f(i)).count();
    let mut v = vec![format!(
        "{} {}",
        n_of(items.len(), lang, ("Sitzung", "Sitzungen"), ("session", "sessions")),
        if items.len() == 1 { lang.t("wird beendet.", "will be ended.") } else { lang.t("werden beendet.", "will be ended.") }
    )];
    let terms = count(&|i| !i.chat && i.running && !i.claude);
    let claude = count(&|i| !i.chat && i.running && i.claude);
    let chats = count(&|i| i.chat && i.history);
    let busy_chats = count(&|i| i.chat && i.running);
    let minimized = count(&|i| i.minimized);
    let worktrees = count(&|i| i.worktree);
    if terms > 0 {
        v.push(n_of(terms, lang, ("laufendes Terminal (Prozess wird beendet)", "laufende Terminals (Prozesse werden beendet)"), ("running terminal (the process ends)", "running terminals (the processes end)")));
    }
    if claude > 0 {
        v.push(n_of(claude, lang, ("Claude-Sitzung", "Claude-Sitzungen"), ("Claude session", "Claude sessions")));
    }
    if chats > 0 {
        v.push(n_of(chats, lang, ("Chat – der Verlauf wird gelöscht", "Chats – die Verläufe werden gelöscht"), ("chat – its history is deleted", "chats – their histories are deleted")));
    }
    if busy_chats > 0 {
        v.push(n_of(busy_chats, lang, ("Chat arbeitet noch (wird abgebrochen)", "Chats arbeiten noch (werden abgebrochen)"), ("chat is still working (it is stopped)", "chats are still working (they are stopped)")));
    }
    if minimized > 0 {
        v.push(n_of(minimized, lang, ("minimierte Sitzung", "minimierte Sitzungen"), ("minimized session", "minimized sessions")));
    }
    if worktrees > 0 {
        v.push(n_of(worktrees, lang, ("Sitzung mit Git-Worktree (Worktree und Branch bleiben)", "Sitzungen mit Git-Worktree (Worktrees und Branches bleiben)"), ("session with a Git worktree (worktree and branch stay)", "sessions with a Git worktree (worktrees and branches stay)")));
    }
    v
}

/// Kurze Merkmale einer Sitzung in der Liste der Rückfrage (z. B. „läuft · minimiert“).
pub(super) fn item_tags(i: &CloseItem, lang: Lang, with_project: bool) -> String {
    let mut t: Vec<String> = Vec::new();
    if with_project && !i.project.is_empty() {
        t.push(i.project.clone());
    }
    if i.chat {
        t.push(lang.t("Chat", "chat").into());
    } else if i.claude {
        t.push("Claude".into());
    }
    if i.running {
        t.push(if i.chat { lang.t("arbeitet", "working") } else { lang.t("läuft", "running") }.into());
    }
    if i.history {
        t.push(lang.t("Verlauf wird gelöscht", "history is deleted").into());
    }
    if i.minimized {
        t.push(lang.t("minimiert", "minimized").into());
    }
    if i.detached {
        t.push(lang.t("eigenes Fenster", "own window").into());
    }
    if i.worktree {
        t.push(lang.t("Worktree bleibt", "worktree stays").into());
    }
    t.join(" · ")
}

/// Aktion eines Klicks auf eine Sitzungszeile der Seitenleiste: Doppelklick benennt um, Klick zeigt die Sitzung,
/// Mittelklick schließt sie wie der Schließen-Knopf (#155, mit derselben Rückfrage).
pub(super) fn session_row_click(resp: &egui::Response, id: u64) -> Option<Action> {
    if resp.middle_clicked() {
        Some(Action::RequestClose(id))
    } else if resp.double_clicked() {
        Some(Action::StartRename(id))
    } else if resp.clicked() {
        Some(Action::Activate(id))
    } else {
        None
    }
}

/// Lage des Papierkorbs am rechten Rand einer Ordnerzeile.
pub(super) fn folder_button_rect(row: Rect) -> Rect {
    Rect::from_center_size(Pos2::new(row.max.x - 15.0, row.center().y), Vec2::splat(22.0))
}

/// Gehört eine Sitzung zum Bereich? `project`/`window` = aktives Projekt und gezeichnetes Fenster (Ordner und Projekt
/// gelten nur in diesem Fenster, wie die Seitenleiste sie zeigt; „alle“ umfasst alles, auch abgedockte und minimierte).
pub(super) fn in_scope(scope: &CloseScope, meta: &SessionMeta, project: u64, window: u64) -> bool {
    match scope {
        CloseScope::Folder(path) => meta.project == project && meta.window == window && !meta.group.is_empty() && state::folder_within(&meta.group, path),
        CloseScope::DeleteFolder(pid, path) => meta.project == *pid && meta.window == window && !meta.group.is_empty() && state::folder_within(&meta.group, path),
        CloseScope::Project(pid) => meta.project == *pid && meta.window == window,
        CloseScope::All => true,
    }
}

/// Von den bestätigten Sitzungen nur die, die es noch gibt (inzwischen geschlossene fallen weg; neu angelegte kommen
/// nie dazu, weil die Liste beim Öffnen der Rückfrage feststeht).
pub(super) fn still_open(ids: &[u64], exists: impl Fn(u64) -> bool) -> Vec<u64> {
    ids.iter().copied().filter(|id| exists(*id)).collect()
}

/// Ergebnis eines Bildes der Rückfrage.
#[derive(Debug, PartialEq, Clone, Copy)]
pub(super) enum Outcome {
    Open,
    Go,
    /// Zweiter, harmloserer Knopf (bei Hintergrundprozessen „Weiterlaufen lassen“, #168).
    Keep,
    Cancel,
}

/// Darf bestätigt werden? Erst wenn die Wartezeit abgelaufen ist (Knopf oder Enter).
pub(super) fn confirmed(secs: u32, clicked: bool, enter: bool) -> bool {
    secs == 0 && (clicked || enter)
}

/// Rückfrage-Fenster mit Wartezeit. Esc und Enter werden hier verbraucht (kein Terminal bekommt sie): Esc bricht ab,
/// Enter bestätigt erst nach der Wartezeit. Der Fokus liegt auf „Abbrechen“.
/// Optional ein zweiter Knopf `keep` (vor dem gefährlichen, ebenfalls erst nach der Wartezeit frei, #168); mit ihm
/// bestätigt Enter diesen harmloseren Knopf, nicht den gefährlichen.
#[allow(clippy::too_many_arguments)]
pub(super) fn confirm_window_keep(ctx: &egui::Context, id: Id, title: &str, button: &str, keep: Option<&str>, secs: u32, lang: Lang, width: f32, body: impl FnOnce(&mut Ui)) -> Outcome {
    let (esc, enter) = ctx.input_mut(|i| (i.consume_key(Modifiers::NONE, Key::Escape), i.consume_key(Modifiers::NONE, Key::Enter)));
    let (mut clicked, mut kept, mut cancel) = (false, false, esc);
    egui::Window::new(title).id(id).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(ctx, |ui| {
        ui.set_max_width(width);
        body(ui);
        ui.add_space(S2);
        ui.horizontal(|ui| {
            if let Some(k) = keep {
                kept = ui.add_enabled(secs == 0, egui::Button::new(k)).clicked();
            }
            clicked = danger_button(ui, button, secs).clicked();
            let c = ui.button(lang.t("Abbrechen", "Cancel"));
            c.request_focus();
            cancel |= c.clicked();
        });
    });
    let (enter_go, enter_keep) = if keep.is_some() { (false, enter) } else { (enter, false) };
    if confirmed(secs, clicked, enter_go) {
        Outcome::Go
    } else if confirmed(secs, kept, enter_keep) {
        Outcome::Keep
    } else if cancel {
        Outcome::Cancel
    } else {
        Outcome::Open
    }
}

impl App {
    /// Sitzungen eines Bereichs (nach Id sortiert).
    pub(super) fn scope_ids(&self, scope: &CloseScope) -> Vec<u64> {
        self.entries.iter().filter(|(_, e)| in_scope(scope, &e.meta, self.active_project, self.wins.cur)).map(|(i, _)| *i).collect()
    }

    pub(super) fn close_item(&self, id: u64) -> Option<CloseItem> {
        let e = self.entries.get(&id)?;
        Some(CloseItem {
            id,
            name: e.display_name(),
            project: self.projects.iter().find(|p| p.id == e.meta.project).map(|p| p.name.clone()).unwrap_or_default(),
            chat: e.is_chat(),
            running: e.running(),
            claude: e.is_claude() && self.settings.claude_mode,
            history: e.chat_has_history(),
            minimized: e.meta.minimized,
            worktree: e.meta.worktree.is_some(),
            detached: self.detached.contains(&id),
        })
    }

    /// „Alle schließen“: Rückfrage mit der Liste öffnen (ohne Sitzungen nur ein Hinweis).
    pub(super) fn request_close_many(&mut self, scope: CloseScope) {
        let ids = self.scope_ids(&scope);
        if let CloseScope::DeleteFolder(pid, path) = &scope {
            // Ganz leerer Ordner (keine Sitzungen in irgendeinem Fenster, keine Unterordner): gleich löschen (#276).
            // Sonst immer die Rückfrage – auch ohne Sitzungen in diesem Fenster (sie nennt Unterordner und Sitzungen
            // anderer Fenster).
            if empty_folder(self.folder_content(*pid, path)) {
                let (pid, path) = (*pid, path.clone());
                self.remove_folder_tree(pid, &path);
                let lang = self.settings.language;
                self.notify_text(format!("{} {}", lang.t("Ordner gelöscht:", "Folder deleted:"), path));
                return;
            }
            let lingering = self.lingering_of(&ids);
            self.confirm_close = Some(CloseAsk::new(CloseTarget::Many(scope, ids)).with(lingering));
            return;
        }
        if ids.is_empty() {
            let lang = self.settings.language;
            self.notify_text(lang.t("Keine Sitzungen zum Schließen.", "No sessions to close.").to_string());
            return;
        }
        let lingering = self.lingering_of(&ids);
        self.confirm_close = Some(CloseAsk::new(CloseTarget::Many(scope, ids)).with(lingering));
    }

    /// Sitzungen des Ordners `path` (Projekt `pid`) in anderen Fenstern als diesem (rücken beim Löschen hoch, #276).
    pub(super) fn folder_elsewhere(&self, pid: u64, path: &str) -> usize {
        self.entries.values().filter(|e| e.meta.project == pid && e.meta.window != self.wins.cur && !e.meta.group.is_empty() && state::folder_within(&e.meta.group, path)).count()
    }

    /// Sitzungen (alle Fenster) und Unterordner im Ordner `path` des Projekts `pid`.
    pub(super) fn folder_content(&self, pid: u64, path: &str) -> (usize, usize) {
        let sessions = self.entries.values().filter(|e| e.meta.project == pid && !e.meta.group.is_empty() && state::folder_within(&e.meta.group, path)).count();
        let subfolders = self.projects.iter().find(|p| p.id == pid).map(|p| p.folders.iter().filter(|f| f.path != path && state::folder_within(&f.path, path)).count()).unwrap_or(0);
        (sessions, subfolders)
    }

    /// Ordner samt Unterordnern (und ihren Vorgaben) entfernen. Sitzungen, die noch darin liegen (andere Fenster,
    /// inzwischen neu angelegte), rücken in den übergeordneten Ordner – sonst legte `sync_folders` ihn wieder an.
    pub(super) fn remove_folder_tree(&mut self, pid: u64, path: &str) {
        let parent = state::folder_parent(path).to_string();
        for e in self.entries.values_mut() {
            if e.meta.project == pid && !e.meta.group.is_empty() && state::folder_within(&e.meta.group, path) {
                e.meta.group = parent.clone();
            }
        }
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == pid) {
            p.remove_folder(path);
        }
    }

    /// Nur den Ordner löschen: Sitzungen und Unterordner rücken eine Ebene hoch (`Project::delete_folder_lift`).
    pub(super) fn lift_folder(&mut self, pid: u64, path: &str) {
        let Some(p) = self.projects.iter_mut().find(|p| p.id == pid) else { return };
        // Plan zuerst (Namenskonflikte im übergeordneten Ordner: freier Name), dann Ordner und Sitzungen gleich abbilden.
        let plan = p.delete_folder_lift(path);
        for e in self.entries.values_mut() {
            if e.meta.project == pid && !e.meta.group.is_empty() && state::folder_within(&e.meta.group, path) {
                e.meta.group = state::lift_mapped(&e.meta.group, path, &plan);
            }
        }
    }

    /// „Ordner löschen (Inhalt bleibt)“ aus dem Kontextmenü: Rückfrage mit Wartezeit; ein leerer Ordner (keine
    /// Sitzungen, keine Unterordner) verschwindet gleich (#276).
    pub(super) fn ask_folder_lift(&mut self, path: String) {
        let pid = self.active_project;
        if empty_folder(self.folder_content(pid, &path)) {
            self.lift_folder(pid, &path);
            let lang = self.settings.language;
            self.notify_text(format!("{} {}", lang.t("Ordner gelöscht:", "Folder deleted:"), path));
        } else {
            self.confirm_close = Some(CloseAsk::new(CloseTarget::FolderLift(pid, path)));
        }
    }

    /// Hintergrundprozesse dieser Sitzungen, die ihr Schließen überleben würden (#168). Nur Sitzungen mit solchen.
    pub(super) fn lingering_of(&self, ids: &[u64]) -> Vec<Lingering> {
        // Prozessnamen und Fenster einmal je Aufruf (nicht je Sitzung), und nur, wenn eine Sitzung mehr als die Shell hat.
        let info = crate::platform::process::ProcInfo::default();
        ids.iter()
            .filter_map(|id| {
                let e = self.entries.get(id)?;
                let live = e.live.as_ref()?;
                let procs = live.lingering(&info);
                (!procs.is_empty()).then(|| Lingering { id: *id, job: live.job_key().unwrap_or_default(), name: e.display_name(), procs })
            })
            .collect()
    }

    /// (Job, Sitzung) der aufbewahrten Jobs für die Zuordnung von Aufrufern (`mcp::attribute`).
    pub(super) fn retired_refs(&self) -> Vec<(u64, u64)> {
        self.retired.iter().map(|r| (r.job.key(), r.session)).collect()
    }

    /// Die Sitzung `id` endet (Schließen) oder startet neu: Ihren Job behalten, wenn noch Prozesse darin laufen
    /// ([`Retired`]). Vorher festhalten, was die Rückfrage genannt hätte (die Konsole lebt da noch).
    pub(super) fn retire_job(&mut self, id: u64, name: String, s: &mut crate::terminal::Session) {
        let shown: Vec<u32> = s.lingering(&crate::platform::process::ProcInfo::default()).into_iter().map(|p| p.pid).collect();
        let Some(job) = s.take_job() else { return };
        if job.pids().is_some_and(|p| p.is_empty()) {
            return;
        }
        self.retired.push(Retired { job, session: id, name, since: Instant::now(), shown, checked: false, listed: Vec::new() });
    }

    /// Aufbewahrte Jobs pflegen (Hauptfenster, höchstens einmal je Sekunde): leere freigeben (und die Kette einer
    /// geschlossenen Sitzung vergessen), nach [`RECHECK`] übrige Prozesse melden – mit „Alle beenden“.
    pub(super) fn poll_retired(&mut self, ctx: &egui::Context) {
        if self.retired.is_empty() {
            return;
        }
        ctx.request_repaint_after(Duration::from_secs(1));
        if self.retired_poll.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
            return;
        }
        self.retired_poll = Some(Instant::now());
        let before: Vec<u64> = self.retired.iter().map(|r| r.session).collect();
        // Nur sicher leere Jobs freigeben (scheitert die Abfrage, bleibt der Job – und mit ihm die Zuordnung).
        self.retired.retain(|r| !r.job.pids().is_some_and(|p| p.is_empty()));
        for s in before {
            if !self.retired.iter().any(|r| r.session == s) && !self.entries.contains_key(&s) {
                taint::remove_retired(s);
            }
        }
        let lang = self.settings.language;
        let mut names: Option<std::collections::HashMap<u32, String>> = None;
        let mut out = Vec::new();
        for r in self.retired.iter_mut().filter(|r| !r.checked && r.since.elapsed() >= RECHECK) {
            r.checked = true;
            let names = names.get_or_insert_with(crate::platform::process::process_names);
            let (pids, lines) = leftover_lines(&r.job.pids().unwrap_or_default(), &r.shown, |p| names.get(&p).cloned().unwrap_or_default());
            r.listed = pids.into_iter().filter_map(|p| crate::platform::process::process_born(p).map(|t| (p, t))).collect();
            if !lines.is_empty() && !r.listed.is_empty() {
                out.push((r.name.clone(), r.job.key(), lines.join(", ")));
            }
        }
        for (name, key, list) in out {
            let text = match lang {
                Lang::De => format!("Aus {} laufen noch Prozesse: {list}", lang.q(&name)),
                Lang::En => format!("Processes from {} are still running: {list}", lang.q(&name)),
            };
            let action = super::toasts::ToastAction { label: lang.t("Alle beenden", "End all").to_string(), token: RETIRED_TOKEN | key, window: self.wins.cur };
            self.push_toast_action(text, super::toasts::ToastKind::Warning, None, Some(action));
        }
    }

    /// „Alle beenden“ aus der Meldung: nur die dort genannten Prozesse beenden (noch im Job, gleiche Erzeugungszeit) –
    /// nicht den ganzen Job, in dem auch Prozesse liegen können, die der Nutzer behalten wollte. Ist der Job schon
    /// freigegeben: nichts.
    pub(super) fn end_retired(&mut self, key: u64) {
        if let Some(r) = self.retired.iter().find(|r| r.job.key() == key) {
            r.job.terminate_listed(&r.listed);
        }
    }

    /// „Alle beenden“: alle Prozesse in den Jobs dieser Sitzungen beenden (#168) – nur, solange es noch der Job von der
    /// Rückfrage ist (`refs` = (Sitzung, Job), [`job_refs`]); eine inzwischen neu gestartete Sitzung bleibt (#213).
    pub(super) fn end_jobs(&mut self, refs: &[(u64, u64)]) {
        let live = |id: u64| self.entries.get(&id).and_then(|e| e.live.as_ref());
        for id in jobs_to_end(refs, |id| live(id).and_then(|s| s.job_key())) {
            if let Some(s) = live(id) {
                s.end_job();
            }
        }
    }

    /// Paneldeck soll beendet werden (Hauptfenster schließen, „Beenden“ im Tray). Haben Sitzungen Hintergrundprozesse,
    /// wird das Schließen angehalten und gefragt (#168). Aufruf in jedem Bild des Hauptfensters nach `handle_tray`.
    pub(super) fn quit_guard(&mut self, ctx: &egui::Context) {
        if self.quit_ok || !ctx.input(|i| i.viewport().close_requested()) {
            return;
        }
        if self.tray.is_some() && !self.quitting && self.settings.close_to_tray {
            return; // geht in den Tray, beendet nichts
        }
        if matches!(self.confirm_close.as_ref().map(|a| &a.target), Some(CloseTarget::Quit)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            return;
        }
        let ids: Vec<u64> = self.entries.keys().copied().collect();
        let lingering = self.lingering_of(&ids);
        if lingering.is_empty() {
            return;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        if self.in_tray {
            self.from_tray(ctx);
        }
        super::models_ui::release_focus(ctx);
        self.confirm_close = Some(CloseAsk::new(CloseTarget::Quit).with(lingering));
    }

    /// Bestätigte Sitzungen beenden (nur noch vorhandene).
    pub(super) fn close_many(&mut self, ids: &[u64]) {
        for id in still_open(ids, |id| self.entries.contains_key(&id)) {
            self.close_session(id);
        }
    }

    /// Rückfrage zum Schließen: eine Sitzung ohne Wartezeit, viele mit 3-Sekunden-Wartezeit ([`wait_secs`]).
    pub(super) fn close_ask_ui(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let Some(ask) = self.confirm_close.take() else { return };
        let lang = self.settings.language;
        let jobs = !ask.lingering.is_empty();
        // Eine laufende Sitzung mit Hintergrundprozessen wartet 3 s: „Weiterlaufen lassen“ schließt sie trotzdem (ConPTY
        // beendet ihren Vordergrund, z. B. claude), Enter soll das nicht sofort tun (#168).
        let running_one = match &ask.target {
            CloseTarget::One(id) => self.entries.get(id).is_some_and(|e| e.running()),
            _ => false,
        };
        let secs = wait_secs(&ask.target, jobs && running_one, ask.since, Instant::now());
        if secs > 0 {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        let keep = jobs.then(|| keep_label(lang));
        let outcome = match &ask.target {
            CloseTarget::One(id) => {
                let Some(e) = self.entries.get(id) else { return }; // inzwischen geschlossen (z. B. von einem Agenten)
                let (name, chat, running) = (e.display_name(), e.is_chat(), e.running());
                let button = if jobs { end_all_label(lang) } else { lang.t("Schließen", "Close") };
                let o = confirm_window_keep(ctx, Id::new("close_one_ask"), lang.t("Sitzung schließen?", "Close session?"), button, keep, secs, lang, 420.0, |ui| {
                    if chat {
                        ui.label(format!(
                            "{}: {}",
                            lang.q(&name),
                            lang.t("Der Chatverlauf wird gelöscht (eine laufende Antwort, ein laufendes Werkzeug oder eine offene Freigabe wird abgebrochen).", "The chat history will be deleted (a running answer, a running tool or a pending approval is stopped).")
                        ));
                    } else if running {
                        ui.label(format!("{} {}", lang.q(&name), lang.t("läuft noch. Beim Schließen wird der Prozess beendet.", "is still running. Closing will terminate the process.")));
                    } else {
                        ui.label(RichText::new(&name).strong());
                    }
                    lingering_section(ui, &ask.lingering, lang, false);
                });
                match o {
                    Outcome::Go if jobs => actions.extend([Action::EndJobs(job_refs(&ask.lingering)), Action::Close(*id)]),
                    Outcome::Go | Outcome::Keep => actions.push(Action::Close(*id)),
                    _ => {}
                }
                o
            }
            CloseTarget::Quit => {
                let o = confirm_window_keep(ctx, Id::new("quit_ask"), lang.t("Paneldeck beenden?", "Quit Paneldeck?"), end_all_label(lang), keep, secs, lang, 460.0, |ui| {
                    ui.label(lang.t("Beim Beenden enden alle Sitzungen.", "Quitting ends all sessions."));
                    lingering_section(ui, &ask.lingering, lang, true);
                });
                match o {
                    Outcome::Go | Outcome::Keep => {
                        if o == Outcome::Go {
                            self.end_jobs(&job_refs(&ask.lingering));
                        }
                        self.quit_ok = true;
                        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
                    }
                    Outcome::Cancel => self.quitting = false,
                    Outcome::Open => {}
                }
                o
            }
            CloseTarget::Many(scope, ids) => {
                let items: Vec<CloseItem> = ids.iter().filter_map(|id| self.close_item(*id)).collect();
                if items.is_empty() && !matches!(scope, CloseScope::DeleteFolder(..)) {
                    return;
                }
                let (title, what) = match scope {
                    CloseScope::Folder(path) => (lang.t("Alle Sitzungen im Ordner schließen?", "Close all sessions in the folder?"), path.clone()),
                    CloseScope::Project(pid) => (
                        lang.t("Alle Sitzungen des Projekts schließen?", "Close all sessions of the project?"),
                        self.projects.iter().find(|p| p.id == *pid).map(|p| p.name.clone()).unwrap_or_default(),
                    ),
                    CloseScope::All => (lang.t("Alle Sitzungen schließen?", "Close all sessions?"), lang.t("Alle Projekte und Fenster", "All projects and windows").to_string()),
                    CloseScope::DeleteFolder(_, path) => (lang.t("Ordner mit seinen Sitzungen löschen?", "Delete the folder with its sessions?"), path.clone()),
                };
                let delete = match scope {
                    CloseScope::DeleteFolder(pid, path) => {
                        if self.projects.iter().find(|p| p.id == *pid).is_none_or(|p| p.folder(path).is_none()) {
                            return; // inzwischen umbenannt oder gelöscht
                        }
                        Some((*pid, path.clone(), self.folder_content(*pid, path).1, self.folder_elsewhere(*pid, path)))
                    }
                    _ => None,
                };
                let title = if delete.is_some() && items.is_empty() { lang.t("Ordner löschen?", "Delete folder?") } else { title };
                let with_project = matches!(scope, CloseScope::All) && items.iter().any(|i| i.project != items[0].project);
                let button = match (jobs, delete.is_some(), items.is_empty()) {
                    (true, ..) => end_all_label(lang),
                    (false, true, true) => lang.t("Ordner löschen", "Delete folder"),
                    (false, true, false) => lang.t("Schließen und löschen", "Close and delete"),
                    (false, false, _) => lang.t("Alle schließen", "Close all"),
                };
                let o = confirm_window_keep(ctx, Id::new("close_many_ask"), title, button, keep, secs, lang, 460.0, |ui| {
                    ui.label(RichText::new(what).strong());
                    ui.add_space(S1);
                    for (n, line) in summary_lines(&items, lang).into_iter().enumerate().filter(|_| !items.is_empty()) {
                        ui.label(if n == 0 { line } else { format!("• {line}") });
                    }
                    ui.add_space(S1);
                    if !items.is_empty() {
                        egui::Frame::new().fill(ui.visuals().extreme_bg_color).corner_radius(CornerRadius::same(style::RADIUS)).inner_margin(Margin::symmetric(8, 6)).show(ui, |ui| {
                            egui::ScrollArea::vertical().max_height(180.0).auto_shrink([false, true]).show(ui, |ui| {
                                for i in &items {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(&i.name);
                                        let tags = item_tags(i, lang, with_project);
                                        if !tags.is_empty() {
                                            ui.label(RichText::new(tags).small().color(style::dim(ui)));
                                        }
                                    });
                                }
                            });
                        });
                    }
                    lingering_section(ui, &ask.lingering, lang, true);
                    if let Some((_, path, subfolders, elsewhere)) = &delete {
                        ui.add_space(S1);
                        if *elsewhere > 0 {
                            ui.label(elsewhere_line(*elsewhere, state::folder_parent(path), lang));
                        }
                        ui.label(folder_delete_note(*subfolders, lang));
                    }
                    ui.add_space(S1);
                    ui.label(RichText::new(lang.t("Das lässt sich nicht rückgängig machen.", "This cannot be undone.")).small().color(style::dim(ui)));
                });
                let ids: Vec<u64> = items.iter().map(|i| i.id).collect();
                match o {
                    Outcome::Go if jobs => actions.extend([Action::EndJobs(job_refs(&ask.lingering)), Action::CloseMany(ids)]),
                    Outcome::Go | Outcome::Keep => actions.push(Action::CloseMany(ids)),
                    _ => {}
                }
                // Erst schließen, dann den Ordner entfernen (übrige Sitzungen rücken hoch, `remove_folder_tree`).
                if let (Outcome::Go | Outcome::Keep, Some((pid, path, ..))) = (o, delete) {
                    actions.push(Action::FolderRemove(pid, path));
                }
                o
            }
            CloseTarget::FolderLift(pid, path) => {
                let Some(proj) = self.projects.iter().find(|p| p.id == *pid) else { return };
                if proj.folder(path).is_none() {
                    return; // inzwischen umbenannt oder gelöscht
                }
                let (sessions, subfolders) = self.folder_content(*pid, path);
                let parent = state::folder_parent(path).to_string();
                let renamed = rename_lines(&proj.lift_plan(path), path, lang);
                let o = confirm_window_keep(ctx, Id::new("folder_lift_ask"), lang.t("Ordner löschen?", "Delete folder?"), lang.t("Ordner löschen", "Delete folder"), None, secs, lang, 420.0, |ui| {
                    ui.label(RichText::new(path.as_str()).strong());
                    ui.add_space(S1);
                    ui.label(lift_summary(sessions, subfolders, &parent, lang));
                    for r in &renamed {
                        ui.label(RichText::new(r).color(ui.visuals().warn_fg_color));
                    }
                    ui.label(lang.t("Es wird nichts geschlossen. Die Vorgaben des Ordners für neue Sitzungen gehen an seine Unterordner.", "Nothing is closed. The folder's defaults for new sessions pass to its subfolders."));
                });
                if o == Outcome::Go {
                    actions.push(Action::FolderDelete(*pid, path.clone()));
                }
                o
            }
            CloseTarget::DeleteProject(pid) => {
                let (Some(p), Some(other)) = (self.projects.iter().find(|p| p.id == *pid), self.projects.iter().find(|p| p.id != *pid)) else { return };
                let n = self.entries.values().filter(|e| e.meta.project == *pid).count();
                let (name, other) = (p.name.clone(), other.name.clone());
                let o = confirm_window_keep(ctx, Id::new("project_delete_ask"), lang.t("Projekt löschen?", "Delete project?"), lang.t("Projekt löschen", "Delete project"), None, secs, lang, 420.0, |ui| {
                    ui.label(RichText::new(&name).strong());
                    ui.add_space(S1);
                    let moved = n_of(n, lang, ("Sitzung", "Sitzungen"), ("session", "sessions"));
                    ui.label(match lang {
                        Lang::De => format!("Nichts wird geschlossen: {moved} {} ins Projekt {}.", if n == 1 { "wechselt" } else { "wechseln" }, lang.q(&other)),
                        Lang::En => format!("Nothing is closed: {moved} {} to the project {}.", if n == 1 { "moves" } else { "move" }, lang.q(&other)),
                    });
                });
                if o == Outcome::Go {
                    actions.push(Action::DeleteProject(*pid));
                }
                o
            }
            CloseTarget::DeleteTemplate(i, name) => {
                // Hat sich die Liste inzwischen verschoben (Löschen, Duplizieren, #244), schließt die Rückfrage einfach.
                let Some(t) = self.templates.get(*i).filter(|t| t.name == *name) else { return };
                let items = t.items.iter().map(|it| it.name.as_str()).collect::<Vec<_>>().join(", ");
                let o = confirm_window_keep(ctx, Id::new("template_delete_ask"), lang.t("Vorlage löschen?", "Delete template?"), lang.t("Löschen", "Delete"), None, secs, lang, 420.0, |ui| {
                    ui.label(RichText::new(name.as_str()).strong());
                    if !items.is_empty() {
                        ui.label(RichText::new(items).small().color(style::dim(ui)));
                    }
                    ui.add_space(S1);
                    ui.label(RichText::new(lang.t("Laufende Sitzungen bleiben. Das lässt sich nicht rückgängig machen.", "Running sessions stay. This cannot be undone.")).small().color(style::dim(ui)));
                });
                if o == Outcome::Go {
                    actions.push(Action::TemplateDelete(*i, name.clone()));
                }
                o
            }
        };
        if outcome == Outcome::Open {
            self.confirm_close = Some(ask);
        } else {
            self.want_focus = Some(self.focused); // Fokus zurück ins Terminal (nach dem Schließen ggf. das nächste)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, PointerButton, RawInput};

    fn item(id: u64) -> CloseItem {
        CloseItem { id, name: format!("s{id}"), project: "P".into(), ..Default::default() }
    }

    #[test]
    fn leftover_lines_skip_hosts_and_known_processes() {
        let name = |p: u32| match p {
            1 => "node.exe".to_string(),
            2 => "conhost.exe".to_string(),
            3 => "notepad.exe".to_string(),
            4 => "OpenConsole.exe".to_string(),
            _ => String::new(),
        };
        assert_eq!(leftover_lines(&[1, 2, 3, 4, 5], &[3], name), (vec![1, 5], vec!["node.exe (PID 1)".to_string(), "? (PID 5)".to_string()]));
        assert!(leftover_lines(&[2, 3, 4], &[3], name).0.is_empty());
        let many: Vec<u32> = (10..20).collect();
        let (pids, l) = leftover_lines(&many, &[], |_| "x.exe".into());
        assert_eq!((l.len(), l.last().map(String::as_str), pids.len()), (7, Some("…"), 10));
    }

    #[test]
    fn end_all_only_hits_the_job_from_the_prompt() {
        let list = [Lingering { id: 1, job: 10, ..Default::default() }, Lingering { id: 2, job: 20, ..Default::default() }, Lingering { id: 3, job: 30, ..Default::default() }];
        assert_eq!(job_refs(&list), [(1, 10), (2, 20), (3, 30)]);
        // 1 unverändert, 2 inzwischen neu gestartet (neuer Job), 3 läuft nicht mehr bzw. ist geschlossen
        let now = |id: u64| match id {
            1 => Some(10),
            2 => Some(21),
            _ => None,
        };
        assert_eq!(jobs_to_end(&job_refs(&list), now), [1]);
    }

    #[test]
    fn summary_counts_each_kind_in_both_languages() {
        let items = vec![
            CloseItem { running: true, ..item(1) },
            CloseItem { running: true, ..item(2) },
            CloseItem { running: true, claude: true, minimized: true, ..item(3) },
            CloseItem { chat: true, history: true, ..item(4) },
            CloseItem { chat: true, history: true, running: true, ..item(5) },
            CloseItem { worktree: true, ..item(6) },
            item(7), // beendetes Terminal: nur in der Gesamtzahl
        ];
        assert_eq!(
            summary_lines(&items, Lang::En),
            vec![
                "7 sessions will be ended.",
                "2 running terminals (the processes end)",
                "1 Claude session",
                "2 chats – their histories are deleted",
                "1 chat is still working (it is stopped)",
                "1 minimized session",
                "1 session with a Git worktree (worktree and branch stay)",
            ]
        );
        let de = summary_lines(&items[..1], Lang::De);
        assert_eq!(de, vec!["1 Sitzung wird beendet.", "1 laufendes Terminal (Prozess wird beendet)"]);
        assert_eq!(summary_lines(&[item(1)], Lang::En), vec!["1 session will be ended."]);
    }

    #[test]
    fn tags_name_what_happens() {
        let i = CloseItem { chat: true, running: true, history: true, minimized: true, ..item(1) };
        assert_eq!(item_tags(&i, Lang::En, false), "chat · working · history is deleted · minimized");
        assert_eq!(item_tags(&i, Lang::De, true), "P · Chat · arbeitet · Verlauf wird gelöscht · minimiert");
        let t = CloseItem { claude: true, running: true, worktree: true, detached: true, ..item(2) };
        assert_eq!(item_tags(&t, Lang::En, false), "Claude · running · own window · worktree stays");
        assert_eq!(item_tags(&item(3), Lang::En, false), "");
    }

    /// Ein Bild mit einer Sitzungszeile (wie in der Seitenleiste: Klick und Ziehen) und dem Papierkorb einer Ordnerzeile
    /// darüber; liefert die Aktion der Zeile und ob der Papierkorb geklickt wurde.
    fn frame(ctx: &egui::Context, t: f64, events: Vec<Event>) -> (Option<Action>, bool, bool) {
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))), time: Some(t), events, ..Default::default() };
        let (mut act, mut trash, mut folder_clicked) = (None, false, false);
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let (_, resp) = ui.allocate_exact_size(Vec2::new(300.0, 46.0), Sense::click_and_drag());
                act = session_row_click(&resp, 7);
                let (row, fresp) = ui.allocate_exact_size(Vec2::new(300.0, 28.0), Sense::click_and_drag());
                trash = style::danger_icon_at(ui, folder_button_rect(row), Id::new("t"), "x", true).clicked();
                folder_clicked = fresp.clicked();
            });
        });
        (act, trash, folder_clicked)
    }

    fn click(ctx: &egui::Context, t: &mut f64, p: Pos2, button: PointerButton) -> (Option<Action>, bool, bool) {
        let ev = |pressed| Event::PointerButton { pos: p, button, pressed, modifiers: Default::default() };
        *t += 0.5;
        frame(ctx, *t, vec![Event::PointerMoved(p)]);
        *t += 0.05;
        frame(ctx, *t, vec![ev(true)]);
        *t += 0.05;
        frame(ctx, *t, vec![ev(false)])
    }

    #[test]
    fn middle_click_on_a_session_row_requests_close_and_trash_beats_the_folder_row() {
        let ctx = egui::Context::default();
        let mut t = 0.0;
        frame(&ctx, t, vec![]);
        // Zeile beginnt bei y ≈ 8 (Rand des Panels); Mitte der Sitzungszeile ≈ y 31, Ordnerzeile ≈ y 70.
        let row = Pos2::new(100.0, 31.0);
        let (a, ..) = click(&ctx, &mut t, row, PointerButton::Middle);
        assert!(matches!(a, Some(Action::RequestClose(7))), "Mittelklick schließt");
        let (a, ..) = click(&ctx, &mut t, row, PointerButton::Primary);
        assert!(matches!(a, Some(Action::Activate(7))), "Linksklick zeigt die Sitzung");
        let (a, ..) = click(&ctx, &mut t, row, PointerButton::Secondary);
        assert!(a.is_none(), "Rechtsklick (Kontextmenü) schließt nicht");
        // Papierkorb am rechten Rand der Ordnerzeile: gewinnt gegen die Zeile darunter (die würde den Ordner aufklappen).
        let mut trash_center = Pos2::ZERO;
        let _ = ctx.run(RawInput { time: Some(t + 0.01), ..Default::default() }, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.allocate_exact_size(Vec2::new(300.0, 46.0), Sense::click_and_drag());
                let (row, _) = ui.allocate_exact_size(Vec2::new(300.0, 28.0), Sense::click_and_drag());
                trash_center = folder_button_rect(row).center();
            });
        });
        t += 0.01;
        let (_, trash, folder) = click(&ctx, &mut t, trash_center, PointerButton::Primary);
        assert!(trash && !folder, "Papierkorb geklickt, Ordner nicht umgeschaltet");
        let (_, trash, folder) = click(&ctx, &mut t, trash_center - Vec2::new(150.0, 0.0), PointerButton::Primary);
        assert!(!trash && folder, "Rest der Zeile bleibt der Ordner");
        // Druck auf den Papierkorb und Ziehen: die Ordnerzeile darunter beginnt kein Ziehen (der Knopf nimmt es).
        let mut dragged = false;
        for ev in [
            Event::PointerMoved(trash_center),
            Event::PointerButton { pos: trash_center, button: PointerButton::Primary, pressed: true, modifiers: Default::default() },
            Event::PointerMoved(trash_center - Vec2::new(40.0, 20.0)),
            Event::PointerMoved(trash_center - Vec2::new(80.0, 40.0)),
            Event::PointerButton { pos: trash_center - Vec2::new(80.0, 40.0), button: PointerButton::Primary, pressed: false, modifiers: Default::default() },
        ] {
            t += 0.05;
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0))), time: Some(t), events: vec![ev], ..Default::default() };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.allocate_exact_size(Vec2::new(300.0, 46.0), Sense::click_and_drag());
                    let (row, fresp) = ui.allocate_exact_size(Vec2::new(300.0, 28.0), Sense::click_and_drag());
                    let _ = style::danger_icon_at(ui, folder_button_rect(row), Id::new("t"), "x", true);
                    dragged |= fresp.drag_started() || fresp.dragged();
                });
            });
        }
        assert!(!dragged, "kein Ordner-Ziehen aus dem Papierkorb heraus");
    }

    fn meta(id: u64, project: u64, window: u64, group: &str) -> SessionMeta {
        SessionMeta { id, project, window, group: group.into(), ..SessionMeta::default() }
    }

    #[test]
    fn scope_covers_subfolders_and_respects_window_and_project() {
        let mut minimized = meta(4, 1, 0, "a/b");
        minimized.minimized = true;
        let sessions = [
            meta(1, 1, 0, "a"),       // Ordner selbst
            meta(2, 1, 0, "a/b/c"),   // Unterordner
            meta(3, 1, 0, "ab"),      // nur gleicher Anfang, nicht darunter
            minimized,                // minimiert, im Unterordner
            meta(5, 1, 1, "a"),       // anderes Fenster
            meta(6, 2, 0, "a"),       // anderes Projekt
            meta(7, 1, 0, ""),        // ohne Ordner
        ];
        let ids = |scope: &CloseScope| sessions.iter().filter(|m| in_scope(scope, m, 1, 0)).map(|m| m.id).collect::<Vec<_>>();
        assert_eq!(ids(&CloseScope::Folder("a".into())), vec![1, 2, 4]);
        assert_eq!(ids(&CloseScope::Folder("a/b".into())), vec![2, 4]);
        assert_eq!(ids(&CloseScope::Project(1)), vec![1, 2, 3, 4, 7]);
        assert_eq!(ids(&CloseScope::Project(2)), vec![6]);
        assert_eq!(ids(&CloseScope::All), vec![1, 2, 3, 4, 5, 6, 7], "alle: auch andere Fenster und Projekte");
        // Aus Sicht des zweiten Fensters
        assert_eq!(sessions.iter().filter(|m| in_scope(&CloseScope::Folder("a".into()), m, 1, 1)).map(|m| m.id).collect::<Vec<_>>(), vec![5]);
        // Ordner löschen (#276): dieselben Sitzungen wie „im Ordner schließen“, Projekt aus der Rückfrage
        assert_eq!(ids(&CloseScope::DeleteFolder(1, "a".into())), vec![1, 2, 4]);
        assert_eq!(sessions.iter().filter(|m| in_scope(&CloseScope::DeleteFolder(2, "a".into()), m, 1, 0)).map(|m| m.id).collect::<Vec<_>>(), vec![6]);
    }

    #[test]
    fn deleting_waits_like_close_all() {
        // #276: Löschen von Ordner, Projekt und Vorlage fragt mit 3-Sekunden-Wartezeit statt „zweimal klicken“.
        let t0 = Instant::now();
        for target in [CloseTarget::FolderLift(1, "a".into()), CloseTarget::DeleteProject(2), CloseTarget::DeleteTemplate(0, "T".into()), CloseTarget::Many(CloseScope::DeleteFolder(1, "a".into()), vec![1])] {
            assert_eq!(wait_secs(&target, false, t0, t0), 3);
            assert_eq!(wait_secs(&target, false, t0, t0 + Duration::from_secs(4)), 0);
        }
    }

    #[test]
    fn only_a_completely_empty_folder_is_deleted_without_asking() {
        assert!(empty_folder((0, 0)));
        assert!(!empty_folder((2, 0)), "Sitzungen (z. B. nur in einem anderen Fenster)");
        assert!(!empty_folder((0, 1)), "leere Unterordner (mit Vorgaben)");
    }

    #[test]
    fn folder_delete_names_other_windows_and_renames() {
        assert_eq!(elsewhere_line(2, "a", Lang::En), "2 sessions in other windows move to “a” (they stay open).");
        assert_eq!(elsewhere_line(1, "", Lang::De), "1 Sitzung in anderen Fenstern rückt auf die oberste Ebene (bleiben offen).");
        let plan = vec![("a/b/c".to_string(), "a/c (2)".to_string()), ("a/b/e".to_string(), "a/e".to_string())];
        assert_eq!(rename_lines(&plan, "a/b", Lang::En), vec!["“c” becomes “c (2)” (the name is already taken there).".to_string()]);
        assert_eq!(rename_lines(&plan, "a/b", Lang::De), vec!["„c“ heißt danach „c (2)“ (der Name ist dort schon vergeben).".to_string()]);
    }

    #[test]
    fn folder_delete_texts_in_both_languages() {
        assert_eq!(lift_summary(3, 1, "a", Lang::En), "3 sessions and 1 subfolder move up to “a”.");
        assert_eq!(lift_summary(1, 0, "", Lang::En), "1 session moves to the top level.");
        assert_eq!(lift_summary(0, 2, "a/b", Lang::De), "2 Unterordner rücken nach „a/b“ hoch.");
        assert_eq!(lift_summary(1, 0, "", Lang::De), "1 Sitzung rückt auf die oberste Ebene.");
        assert_eq!(lift_summary(0, 0, "", Lang::En), "The folder is empty.");
        assert_eq!(folder_delete_note(0, Lang::En), "Afterwards the folder is deleted with its defaults for new sessions.");
        assert_eq!(folder_delete_note(2, Lang::De), "Danach wird der Ordner mit 2 Unterordnern und den Vorgaben für neue Sitzungen gelöscht.");
        assert_eq!(folder_delete_note(1, Lang::En), "Afterwards the folder is deleted with 1 subfolder and the defaults for new sessions.");
    }

    #[test]
    fn close_many_skips_missing_and_never_adds_new_ids() {
        let now: std::collections::HashSet<u64> = [1, 3, 9].into_iter().collect(); // 2 inzwischen geschlossen, 9 neu
        assert_eq!(still_open(&[1, 2, 3], |id| now.contains(&id)), vec![1, 3]);
        assert!(still_open(&[], |_| true).is_empty());
    }

    #[test]
    fn only_closing_many_waits() {
        let t0 = Instant::now();
        assert_eq!(wait_secs(&CloseTarget::One(1), false, t0, t0), 0, "eine Sitzung: ohne Wartezeit (wie 2.2)");
        assert_eq!(wait_secs(&CloseTarget::Many(CloseScope::All, vec![1, 2]), false, t0, t0), 3);
        assert_eq!(wait_secs(&CloseTarget::Many(CloseScope::Folder("a".into()), vec![1]), false, t0, t0 + Duration::from_millis(1500)), 2);
        assert_eq!(wait_secs(&CloseTarget::Many(CloseScope::Project(1), vec![1]), false, t0, t0 + Duration::from_secs(4)), 0);
        // #168: eine laufende Sitzung mit Hintergrundprozessen wartet; Beenden von Paneldeck mit Hintergrundprozessen auch
        assert_eq!(wait_secs(&CloseTarget::One(1), true, t0, t0), 3);
        assert_eq!(wait_secs(&CloseTarget::One(1), true, t0, t0 + Duration::from_secs(4)), 0);
        assert_eq!(wait_secs(&CloseTarget::Quit, false, t0, t0), 3);
    }

    #[test]
    fn countdown_gates_confirmation() {
        assert!(!confirmed(3, true, false));
        assert!(!confirmed(1, false, true), "Enter während der Wartezeit tut nichts");
        assert!(confirmed(0, true, false));
        assert!(confirmed(0, false, true));
        assert!(!confirmed(0, false, false));
    }

    /// Ein Bild der Rückfrage mit diesen Tasten; liefert das Ergebnis und die Tastenereignisse, die danach noch übrig
    /// sind (die bekäme ein Terminal, das nach dem Dialog Eingaben liest).
    fn ask_frame(ctx: &egui::Context, t: f64, secs: u32, keys: &[Key]) -> (Outcome, Vec<Key>) {
        let events = keys.iter().map(|k| Event::Key { key: *k, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }).collect();
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), time: Some(t), events, ..Default::default() };
        let (mut out, mut left) = (Outcome::Open, Vec::new());
        let _ = ctx.run(input, |ctx| {
            out = confirm_window_keep(ctx, Id::new("ask"), "Close?", "Close", None, secs, Lang::En, 300.0, |ui| {
                ui.label("x");
            });
            left = ctx.input(|i| i.events.iter().filter_map(|e| if let Event::Key { key, pressed: true, .. } = e { Some(*key) } else { None }).collect());
        });
        (out, left)
    }

    /// Wie `ask_frame`, mit dem Knopf „Weiterlaufen lassen“ (#168).
    fn keep_frame(ctx: &egui::Context, t: f64, secs: u32, keys: &[Key]) -> (Outcome, Vec<Key>) {
        let events = keys.iter().map(|k| Event::Key { key: *k, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }).collect();
        let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0))), time: Some(t), events, ..Default::default() };
        let (mut out, mut left) = (Outcome::Open, Vec::new());
        let _ = ctx.run(input, |ctx| {
            out = confirm_window_keep(ctx, Id::new("keep"), "Close?", end_all_label(Lang::En), Some(keep_label(Lang::En)), secs, Lang::En, 300.0, |ui| {
                ui.label("x");
            });
            left = ctx.input(|i| i.events.iter().filter_map(|e| if let Event::Key { key, pressed: true, .. } = e { Some(*key) } else { None }).collect());
        });
        (out, left)
    }

    #[test]
    fn with_background_processes_enter_keeps_them_running_and_never_ends_all() {
        let ctx = egui::Context::default();
        assert_eq!(keep_frame(&ctx, 0.0, 0, &[]).0, Outcome::Open);
        let (o, left) = keep_frame(&ctx, 0.1, 0, &[Key::Enter]);
        assert_eq!(o, Outcome::Keep, "Enter = der harmlose Knopf, nie „Alle beenden“");
        assert!(left.is_empty(), "Enter erreicht kein Terminal");
        let (o, left) = keep_frame(&ctx, 0.2, 2, &[Key::Enter]);
        assert_eq!(o, Outcome::Open, "viele Sitzungen: auch „Weiterlaufen lassen“ erst nach der Wartezeit");
        assert!(left.is_empty());
        let (o, left) = keep_frame(&ctx, 0.3, 0, &[Key::Escape]);
        assert_eq!(o, Outcome::Cancel);
        assert!(left.is_empty(), "Esc erreicht kein Terminal");
        // Ohne zweiten Knopf bleibt Enter = bestätigen (bisheriges Verhalten)
        assert_eq!(ask_frame(&ctx, 0.4, 0, &[Key::Enter]).0, Outcome::Go);
    }

    fn jp(pid: u32, name: &str, title: &str) -> JobProc {
        JobProc { pid, name: name.into(), title: title.into(), window: !title.is_empty() }
    }

    #[test]
    fn background_process_texts_in_both_languages() {
        assert_eq!(proc_line(&jp(42, "notepad.exe", " Unbenannt - Editor "), Lang::De), "notepad.exe (PID 42) · Fenster offen – Unbenannt - Editor");
        assert_eq!(proc_line(&jp(42, "notepad.exe", "Editor"), Lang::En), "notepad.exe (PID 42) · window open – Editor");
        assert_eq!(proc_line(&jp(7, "node.exe", ""), Lang::En), "node.exe (PID 7)");
        // Warnung: ohne Speichern; mit Fenstern zusätzlich der Rat, vorher selbst zu schließen
        let quiet = [Lingering { id: 1, job: 1, name: "A".into(), procs: vec![jp(1, "node.exe", "")] }];
        let win = [Lingering { id: 1, job: 1, name: "A".into(), procs: vec![jp(1, "code.exe", "proj")] }];
        assert!(end_all_warning(&quiet, Lang::En).contains("without saving") && !end_all_warning(&quiet, Lang::En).contains("window"));
        assert!(end_all_warning(&win, Lang::En).contains("open window"));
        assert!(end_all_warning(&win, Lang::De).contains("ohne zu speichern") && end_all_warning(&win, Lang::De).contains("offenem Fenster"));
        let one = [Lingering { id: 1, job: 1, name: "A".into(), procs: vec![jp(1, "a.exe", "")] }];
        assert_eq!(lingering_intro(&one, Lang::En), "1 background process is still running and stays after closing unless you end it:");
        assert_eq!(lingering_intro(&one, Lang::De), "1 Hintergrundprozess läuft noch und bleibt nach dem Schließen bestehen, wenn du ihn nicht beendest:");
        let two = [Lingering { id: 1, job: 1, name: "A".into(), procs: vec![jp(1, "a.exe", ""), jp(2, "b.exe", "")] }, Lingering { id: 2, job: 2, name: "B".into(), procs: vec![jp(3, "c.exe", "")] }];
        assert_eq!(lingering_intro(&two, Lang::En), "3 background processes in 2 sessions are still running and stay after closing unless you end them:");
        assert!(lingering_intro(&two[..1], Lang::De).starts_with("2 Hintergrundprozesse laufen noch"));
        assert_ne!(keep_label(Lang::De), keep_label(Lang::En));
        assert_ne!(end_all_label(Lang::De), end_all_label(Lang::En));
        // Abschnitt zeichnet sich ohne Fehler (headless) und nichts ohne Prozesse
        let ctx = egui::Context::default();
        let _ = ctx.run(RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                lingering_section(ui, &two, Lang::En, true);
                lingering_section(ui, &[], Lang::En, true);
            });
        });
    }

    #[test]
    fn dialog_keys_enter_waits_for_the_countdown_and_esc_cancels_without_leaking() {
        let ctx = egui::Context::default();
        assert_eq!(ask_frame(&ctx, 0.0, 3, &[]).0, Outcome::Open);
        let (o, left) = ask_frame(&ctx, 0.1, 3, &[Key::Enter]);
        assert_eq!(o, Outcome::Open, "Enter während der Wartezeit");
        assert!(left.is_empty(), "Enter erreicht kein Terminal: {left:?}");
        let (o, left) = ask_frame(&ctx, 0.2, 0, &[Key::Enter]);
        assert_eq!(o, Outcome::Go, "Enter nach der Wartezeit bestätigt");
        assert!(left.is_empty());
        let (o, left) = ask_frame(&ctx, 0.3, 2, &[Key::Escape]);
        assert_eq!(o, Outcome::Cancel);
        assert!(left.is_empty(), "Esc erreicht kein Terminal: {left:?}");
        // Andere Tasten bleiben unberührt (der Dialog verbraucht nur Enter und Esc).
        let (o, left) = ask_frame(&ctx, 0.4, 2, &[Key::A]);
        assert_eq!((o, left), (Outcome::Open, vec![Key::A]));
        // Der Fokus liegt auf „Abbrechen“ (kein Terminal hält ihn).
        assert!(ctx.memory(|m| m.focused()).is_some());
    }
}
