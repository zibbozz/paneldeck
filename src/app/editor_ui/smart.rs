//! Smart-Modus im Editor (#367, 3.3.0): verbindet die Dokumente mit den Language Servern (`crate::lsp`) – Vertrauen
//! je Projektordner, Server starten, Text senden (sofort, ganzer Text), Diagnosen anzeigen (erst ~500 ms nach der
//! letzten Änderung, veraltete fallen weg), Vervollständigen (Popup, Snippets mit Tab-Stopps), Problemliste und F8.
//!
//! Smart gilt nur für gespeicherte Dateien bis zur Größengrenze in vertrauenswürdigen Ordnern; sonst läuft der
//! Syntax-Modus weiter (die Statusleiste sagt warum). Nicht im Vergleichs-Reiter. WSL-Dateien: der Server läuft in der
//! Distribution (`wsl.exe -d <D> --cd <Wurzel> --exec <Server>`, gefunden im Linux-PATH), Adressen sind Linux-Pfade;
//! Wurzel und Server werden im Hintergrund ermittelt, nie im UI-Thread. Nach `crash_max` Abstürzen in
//! `crash_window_minutes` ist Smart für die Sprache bis zum Neustart aus (Hinweisleiste *Erneut versuchen* /
//! *Protokoll zeigen*); Server ohne offenes Dokument enden nach `idle_minutes`.

use super::doc::DocEntry;
use super::{EditorPrefs, EditorUi};
use crate::editor::syntax::Mode;
use crate::editor::{EditOrigin, Selection, Selections, TextEdit};
use crate::lsp::{self, client, proto, servers, trust::Trust};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Pause nach zwei Wortzeichen, bevor die Vervollständigung von selbst fragt.
pub(super) const AUTO_PAUSE: Duration = Duration::from_millis(100);
/// Zeilen im Popup.
pub(super) const POPUP_ROWS: usize = 12;
/// So lange gilt die Suche nach dem Server einer Sprache (danach neu, z. B. nach einer Installation).
const RESOLVE_TTL: Duration = Duration::from_secs(30);

/// Warum ein Dokument (nicht) im Smart-Modus läuft – für Statusleiste und Hinweisleiste.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum State {
    /// Kein Smart-Modus gewählt.
    Off,
    /// Ordner nicht vertrauenswürdig: Hinweisleiste mit dem vorgeschlagenen Ordner (Projektwurzel).
    NeedsTrust(PathBuf),
    /// „Jetzt nicht“ für diesen Ordner gewählt.
    NotNow(PathBuf),
    NoServer,
    TooLarge,
    /// Unbenannt bzw. Netzwerkpfad (kein Server).
    Unsupported,
    /// WSL: Wurzel und Server werden im Hintergrund ermittelt.
    Resolving,
    /// Zu oft abgestürzt: Smart für diese Sprache bis zum Neustart aus (Hinweisleiste).
    CrashedOff(String),
    Running(lsp::Key),
    Stopped(String),
}

/// Eine angezeigte Diagnose (Zeichenbereich im Dokument).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Shown {
    pub range: Range<usize>,
    pub line: usize,
    pub col: usize,
    pub severity: proto::Severity,
    pub message: String,
    pub unnecessary: bool,
    pub deprecated: bool,
}

struct DocLsp {
    key: lsp::Key,
    uri: String,
    sent_rev: u64,
    edited_at: Instant,
    encoding: proto::Encoding,
}

/// Vervollständigen: offenes Popup.
#[derive(Clone, Debug)]
pub(crate) struct Popup {
    pub doc: u64,
    pub group: u64,
    /// Anfang des Worts, das vervollständigt wird (Zeichen).
    pub start: usize,
    pub items: Vec<proto::Completion>,
    /// Indizes in `items` nach dem Filtern.
    pub shown: Vec<usize>,
    pub selected: usize,
    pub ticket: u64,
    pub waiting: bool,
    /// Liste des Servers unvollständig (`isIncomplete`): beim Weitertippen neu fragen.
    pub incomplete: bool,
}

/// Laufendes Snippet: die übrigen Tab-Stopps (als Auswahlen, sie folgen dem Text).
pub(crate) struct Snippet {
    pub doc: u64,
    pub group: u64,
    pub stops: Vec<Selections>,
}

#[derive(Default)]
pub(crate) struct SmartUi {
    pub servers: lsp::Servers,
    pub trust: Option<Trust>,
    docs: HashMap<u64, DocLsp>,
    pub state: HashMap<u64, State>,
    pub shown: HashMap<u64, Vec<Shown>>,
    /// Projektwurzel je (Datei, Sprache) und Server je (Sprache, Wurzel): im Hintergrund ermittelt (nie im UI-Thread).
    roots: HashMap<(PathBuf, String), Job<Option<Root>>>,
    found: HashMap<(String, PathBuf), Job<Option<(String, servers::Launch)>>>,
    pub not_now: HashSet<PathBuf>,
    pub problems_open: bool,
    pub window_open: bool,
    pub window_sel: Option<lsp::Key>,
    pub popup: Option<Popup>,
    pub snippet: Option<Snippet>,
    /// Von selbst fragen, sobald die Pause um ist: (Dokument, Gruppe, fällig).
    pub auto: Option<(u64, u64, Instant)>,
    ticket: u64,
    /// Zwischenspeicher des Protokolls im Fenster „Language Server“: (Server, Stand, Text).
    pub(super) log_cache: Option<(lsp::Key, u64, String)>,
    /// Ersatz für die Server-Suche (Tests).
    #[cfg(test)]
    pub test_resolve: Option<servers::Launch>,
}

/// Projektwurzel einer Datei: Ordner (Vertrauen, Schlüssel) und unter WSL zusätzlich (Distribution, Linux-Pfad).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Root {
    pub dir: PathBuf,
    pub wsl: Option<(String, String)>,
}

/// Arbeit im Hintergrund mit Ergebnis.
enum Job<T> {
    Pending(std::sync::mpsc::Receiver<T>),
    Done(Instant, T),
}

/// Ergebnis einer Hintergrund-Arbeit holen bzw. sie starten. `ttl` = so lange gilt ein Ergebnis (`None` = bleibt).
/// `None` zurück, solange es läuft.
fn job<K: std::hash::Hash + Eq + Clone, T: Clone + Send + 'static>(map: &mut HashMap<K, Job<T>>, key: &K, now: Instant, ttl: Option<Duration>, start: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    match map.get(key) {
        Some(Job::Done(at, v)) if ttl.is_none_or(|t| now.duration_since(*at) < t) => return Some(v.clone()),
        Some(Job::Pending(rx)) => match rx.try_recv() {
            Ok(v) => {
                map.insert(key.clone(), Job::Done(now, v.clone()));
                return Some(v);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => return None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {}
        },
        _ => {}
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(start());
    });
    map.insert(key.clone(), Job::Pending(rx));
    None
}

/// Hintergrund: Projektwurzel suchen. Pfade werden vorher bereinigt (`.`/`..` aufgelöst, unter Unix der echte Pfad),
/// damit `C:\vertraut\..\fremd\x.rs` nicht als vertraut gilt. WSL: Markierungen über den UNC-Pfad, nie über die
/// Distribution hinaus; sonst nie über das Benutzerverzeichnis.
pub(crate) fn find_root(file: &Path, lang: &str, home: Option<&Path>) -> Option<Root> {
    let file = lsp::trust::normalize(file);
    let exists = |p: &Path| p.exists();
    if let Some((distro, _)) = crate::wsl::unc_to_linux(&file.to_string_lossy()) {
        let top = PathBuf::from(crate::wsl::linux_to_unc(&distro, "/"));
        let dir = client::project_root(&file, lang, &exists, Some(&top));
        let (_, linux) = crate::wsl::unc_to_linux(&dir.to_string_lossy())?;
        return Some(Root { dir, wsl: Some((distro, linux)) });
    }
    Some(Root { dir: client::project_root(&file, lang, &exists, home), wsl: None })
}

/// Hintergrund: den ersten eingebauten Server im Linux-PATH der Distribution suchen (erst nach dem Vertrauen).
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn wsl_server(distro: &str, root_linux: &str, lang: &str, disabled: &[String]) -> Option<(String, servers::Launch)> {
    servers::known(lang).iter().filter(|k| !disabled.iter().any(|d| d == k.id)).find_map(|k| {
        let prog = crate::wsl::find_program(distro, k.program)?;
        let args = crate::wsl::server_args(distro, root_linux, &prog, k.args)?;
        Some((k.id.to_string(), servers::Launch { program: crate::wsl::exe_path(), args, found: PathBuf::from(&prog), rustup_proxy: false, lsp_root: Some(root_linux.to_string()), raw_tail: None, invalid: None }))
    })
}

impl SmartUi {
    pub fn trust(&mut self, data: &Path) -> &mut Trust {
        self.trust.get_or_insert_with(|| Trust::load(data))
    }

    pub fn state_of(&self, doc: u64) -> State {
        self.state.get(&doc).cloned().unwrap_or(State::Off)
    }

    pub fn counts(&self, doc: u64) -> (usize, usize) {
        self.shown.get(&doc).map(|v| (v.iter().filter(|d| d.severity == proto::Severity::Error).count(), v.iter().filter(|d| d.severity == proto::Severity::Warning).count())).unwrap_or((0, 0))
    }

    /// Alle angezeigten Diagnosen (Problemliste): (Dokument, Diagnose).
    pub fn all_problems(&self) -> Vec<(u64, &Shown)> {
        let mut v: Vec<(u64, &Shown)> = self.shown.iter().flat_map(|(d, l)| l.iter().map(move |s| (*d, s))).collect();
        v.sort_by_key(|(d, s)| (s.severity, *d, s.line, s.col));
        v
    }
}

/// Sprachkennung, Pfad und Projektwurzel eines Dokuments für den Smart-Modus (oder warum nicht).
fn wanted(e: &mut DocEntry, prefs: &EditorPrefs, settings: &lsp::SmartSettings) -> Result<(String, PathBuf), State> {
    if e.doc.is_none() || e.mode(prefs) != Mode::Smart {
        return Err(State::Off);
    }
    let Some(path) = e.path.clone() else { return Err(State::Unsupported) };
    if !super::doc::is_wsl_path(&path) && crate::pathfmt::is_remote_unc(&path.to_string_lossy()) {
        return Err(State::Unsupported);
    }
    if e.doc.as_ref().is_some_and(|d| d.len_bytes() as u64 > settings.max_file_bytes()) {
        return Err(State::TooLarge);
    }
    let lang = e.language(prefs).ok_or(State::NoServer)?;
    Ok((lang, path))
}

impl EditorUi {
    /// Je Bild (Hauptdurchgang): Server abfragen, Dokumente öffnen/ändern/schließen, Diagnosen übernehmen, Antworten
    /// der Vervollständigung einsortieren. Liefert, ob bald neu gezeichnet werden soll.
    pub(crate) fn smart_tick(&mut self, prefs: &EditorPrefs, settings: &lsp::SmartSettings, data: &Path) -> bool {
        let now = Instant::now();
        let mut busy = self.smart.servers.tick();
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from);
        let ids: Vec<u64> = self.docs.keys().copied().collect();
        let mut alive: HashSet<u64> = HashSet::new();
        for id in ids {
            let Some(e) = self.docs.get_mut(&id) else { continue };
            let (lang, path) = match wanted(e, prefs, settings) {
                Ok(x) => x,
                Err(s) => {
                    self.smart.state.insert(id, s);
                    continue;
                }
            };
            if self.smart.servers.off.contains(&lang) {
                self.smart.state.insert(id, State::CrashedOff(lang));
                continue;
            }
            let disabled: Vec<String> = settings.disabled.get(&lang).cloned().unwrap_or_default();
            // 1. Projektwurzel im Hintergrund (Plattenzugriffe nie im UI-Thread, auch nicht für Netzlaufwerke).
            let rk = (path.clone(), lang.clone());
            let (p2, l2, h2) = (path.clone(), lang.clone(), home.clone());
            let Some(root_info) = job(&mut self.smart.roots, &rk, now, Some(RESOLVE_TTL), move || find_root(&p2, &l2, h2.as_deref())) else {
                self.smart.state.insert(id, State::Resolving);
                busy = true;
                continue;
            };
            let Some(root_info) = root_info else {
                self.smart.state.insert(id, State::Unsupported);
                continue;
            };
            let root = root_info.dir.clone();
            // 2. Vertrauen (vor jeder Suche nach dem Server).
            if !self.smart.trust.get_or_insert_with(|| Trust::load(data)).trusted(&root) {
                let s = if self.smart.not_now.contains(&root) { State::NotNow(root) } else { State::NeedsTrust(root) };
                self.smart.state.insert(id, s);
                continue;
            }
            // 3. Server im Hintergrund suchen; solange einer für Sprache und Wurzel läuft, nicht neu.
            let running = self.smart.servers.entries.values().any(|e| e.language == lang && e.root == root && e.client.as_ref().is_some_and(|c| !matches!(c.state, client::State::Exited(_))));
            let fk = (lang.clone(), root.clone());
            let ttl = if running { None } else { Some(RESOLVE_TTL) };
            #[cfg(test)]
            let test_launch = self.smart.test_resolve.clone();
            let (own, own_args) = (settings.own_paths.get(&lang).cloned(), settings.own_args.get(&lang).cloned().unwrap_or_default());
            let (l3, r3, info3) = (lang.clone(), root.clone(), root_info.clone());
            let Some(found) = job(&mut self.smart.found, &fk, now, ttl, move || {
                #[cfg(test)]
                {
                    let _ = (&own, &own_args, &r3, &info3, &disabled, &l3);
                    test_launch.map(|l| ("test".to_string(), l))
                }
                #[cfg(not(test))]
                match &info3.wsl {
                    Some((distro, linux)) => wsl_server(distro, linux, &l3, &disabled),
                    None => {
                        let env = servers::Env::system(Some(&r3));
                        servers::resolve(&l3, own.as_deref(), &own_args, &disabled, &env)
                    }
                }
            }) else {
                self.smart.state.insert(id, State::Resolving);
                busy = true;
                continue;
            };
            let uri_path = match &root_info.wsl {
                Some(_) => crate::wsl::unc_to_linux(&lsp::trust::normalize(&path).to_string_lossy()).map(|(_, l)| l).unwrap_or_default(),
                None => lsp::trust::normalize(&path).to_string_lossy().to_string(),
            };
            let r = self.smart.servers.ensure(&lang, &root, settings, &|_| found.clone());
            let key = match r {
                lsp::Ensure::CrashedOff => {
                    self.smart.state.insert(id, State::CrashedOff(lang));
                    continue;
                }
                lsp::Ensure::Running(k) => k,
                lsp::Ensure::NoServer => {
                    self.smart.state.insert(id, State::NoServer);
                    continue;
                }
                lsp::Ensure::Stopped(w) | lsp::Ensure::Failed(w) => {
                    self.smart.state.insert(id, State::Stopped(w));
                    continue;
                }
            };
            self.smart.state.insert(id, State::Running(key.clone()));
            alive.insert(id);
            let Some(e) = self.docs.get(&id) else { continue };
            let Some(doc) = e.doc.as_ref() else { continue };
            let rev = doc.revision();
            let uri = proto::path_to_uri(&uri_path);
            let Some(c) = self.smart.servers.client(&key) else { continue };
            let entry = self.smart.docs.get(&id);
            if entry.is_none_or(|d| d.key != key) {
                if let Some(old) = self.smart.docs.remove(&id)
                    && let Some(oc) = self.smart.servers.client(&old.key)
                {
                    oc.close(&old.uri);
                }
                let Some(c) = self.smart.servers.client(&key) else { continue };
                c.open(&uri, &lang, &doc.text());
                let encoding = c.caps.encoding;
                self.smart.docs.insert(id, DocLsp { key: key.clone(), uri, sent_rev: rev, edited_at: now - settings.typing_pause(), encoding });
            } else if entry.is_some_and(|d| d.sent_rev != rev) {
                c.change(&uri, &doc.text());
                if let Some(d) = self.smart.docs.get_mut(&id) {
                    d.sent_rev = rev;
                    d.edited_at = now;
                    d.encoding = c.caps.encoding;
                }
                // Veraltete Diagnosen fallen weg, bis der Server zum neuen Stand meldet.
                self.smart.shown.remove(&id);
            }
        }
        // Server ohne offenes Dokument enden nach der Leerlaufzeit.
        self.smart.servers.end_idle(settings.idle());
        // Dokumente, die nicht mehr im Smart-Modus laufen (geschlossen, anderer Modus): beim Server schließen.
        let gone: Vec<u64> = self.smart.docs.keys().filter(|d| !alive.contains(d)).copied().collect();
        for id in gone {
            if let Some(old) = self.smart.docs.remove(&id)
                && let Some(c) = self.smart.servers.client(&old.key)
            {
                c.close(&old.uri);
            }
            self.smart.shown.remove(&id);
        }
        self.smart.state.retain(|id, _| self.docs.contains_key(id));
        // Diagnosen übernehmen (nach der Pause, nur zum aktuellen Stand).
        let ids: Vec<u64> = self.smart.docs.keys().copied().collect();
        for id in ids {
            let (key, uri, edited_at, enc) = {
                let d = &self.smart.docs[&id];
                (d.key.clone(), d.uri.clone(), d.edited_at, d.encoding)
            };
            if now.duration_since(edited_at) < settings.typing_pause() {
                busy = true;
                continue;
            }
            let Some(c) = self.smart.servers.client(&key) else { continue };
            let cur = c.version(&uri);
            let Some(fd) = c.diagnostics.get(&uri) else {
                self.smart.shown.remove(&id);
                continue;
            };
            if fd.version.is_some_and(|v| Some(v) != cur) {
                continue;
            }
            let Some(doc) = self.docs.get(&id).and_then(|e| e.doc.as_ref()) else { continue };
            self.smart.shown.insert(id, to_shown(doc, &fd.items, enc));
        }
        // Antworten der Vervollständigung
        let mut replies: Vec<client::Reply> = Vec::new();
        for e in self.smart.servers.entries.values_mut() {
            if let Some(c) = e.client.as_mut() {
                replies.extend(c.take_replies());
            }
        }
        for r in replies {
            match r {
                client::Reply::Completions { ticket, items, incomplete } => {
                    if let Some(p) = self.smart.popup.as_mut()
                        && p.ticket == ticket
                    {
                        p.incomplete = incomplete;
                        p.items = items;
                        p.waiting = false;
                        self.smart_refilter();
                    }
                }
                client::Reply::Failed { ticket, .. } => {
                    if self.smart.popup.as_ref().is_some_and(|p| p.ticket == ticket) {
                        self.smart.popup = None;
                    }
                }
            }
        }
        // Von selbst fragen, wenn die Pause um ist
        if let Some((doc, g, due)) = self.smart.auto {
            if now >= due {
                self.smart.auto = None;
                self.smart_complete(doc, g, None);
            } else {
                busy = true;
            }
        }
        busy || self.smart.popup.as_ref().is_some_and(|p| p.waiting)
    }

    /// Ordner vertrauen und speichern; der Smart-Modus startet im nächsten Bild.
    pub(crate) fn smart_trust(&mut self, folder: &Path, data: &Path) -> Result<(), String> {
        Trust::check_folder(folder).map_err(str::to_string)?;
        let t = self.smart.trust(data);
        t.add(folder);
        t.save(data).map_err(|e| e.to_string())?;
        self.smart.not_now.remove(folder);
        Ok(())
    }

    /// Nach dem Speichern: `didSave` an den Server des Dokuments.
    pub(crate) fn smart_saved(&mut self, doc: u64) {
        let Some(d) = self.smart.docs.get(&doc) else { return };
        let (key, uri) = (d.key.clone(), d.uri.clone());
        let text = self.docs.get(&doc).and_then(|e| e.text()).unwrap_or_default();
        if let Some(c) = self.smart.servers.client(&key) {
            c.saved(&uri, &text);
        }
    }

    /// *Erneut versuchen* nach der Absturz-Regel: Smart für die Sprache wieder an.
    pub(crate) fn smart_retry(&mut self, lang: &str) {
        self.smart.servers.retry(lang);
        if let Some(k) = self.smart.servers.key_of_language(lang) {
            self.smart.servers.restart(&k);
        }
    }

    /// *Protokoll zeigen*: Fenster „Language Server“ mit dem Server der Sprache.
    pub(crate) fn smart_show_log(&mut self, lang: &str) {
        self.smart.window_sel = self.smart.servers.key_of_language(lang);
        self.smart.window_open = true;
    }

    /// Vertrauen entziehen: die Server dort enden sofort.
    pub(crate) fn smart_revoke(&mut self, folder: &Path, data: &Path) -> Result<(), String> {
        let t = self.smart.trust(data);
        t.remove(folder);
        t.save(data).map_err(|e| e.to_string())?;
        let t = self.smart.trust.clone().unwrap_or_default();
        self.smart.servers.stop_under(folder, &t);
        Ok(())
    }

    /// Cursor (Zeile, Spalte in der Kodierung des Servers) und Adresse eines Dokuments der Gruppe `g`.
    fn smart_pos(&mut self, doc: u64, g: u64) -> Option<(lsp::Key, String, proto::Pos, usize)> {
        let d = self.smart.docs.get(&doc)?;
        let (key, uri, enc) = (d.key.clone(), d.uri.clone(), d.encoding);
        let idx = self.panes.groups.get(&g)?.active;
        let (Some(view), Some(e)) = self.view_doc_mut(g, idx) else { return None };
        let docm = e.doc.as_ref()?;
        let mut sels = view.sels.clone()?;
        docm.sync(&mut sels);
        let head = sels.primary().head.min(docm.len_chars());
        let lc = crate::editor::text::pos_to_linecol(docm.rope(), head);
        let line = docm.line_string(lc.line);
        let line = line.trim_end_matches(['\n', '\r']);
        Some((key, uri, proto::Pos { line: lc.line as u32, character: enc.column_of(line, lc.col) }, head))
    }

    /// Vervollständigung anfragen (Strg+Leertaste bzw. nach Auslösezeichen/Pause). `trigger` = Auslösezeichen.
    pub(crate) fn smart_complete(&mut self, doc: u64, g: u64, trigger: Option<char>) {
        let Some((key, uri, pos, head)) = self.smart_pos(doc, g) else { return };
        let start = self.docs.get(&doc).and_then(|e| e.doc.as_ref()).map(|d| word_start(d, head)).unwrap_or(head);
        self.smart.ticket += 1;
        let ticket = self.smart.ticket;
        let Some(c) = self.smart.servers.client(&key) else { return };
        c.cancel_completions();
        if c.complete(&uri, pos, trigger, ticket, 500) {
            let items = self.smart.popup.as_ref().filter(|p| p.doc == doc && p.start == start).map(|p| p.items.clone()).unwrap_or_default();
            self.smart.popup = Some(Popup { doc, group: g, start, items, shown: Vec::new(), selected: 0, ticket, waiting: true, incomplete: false });
            self.smart_refilter();
        }
    }

    /// Nach einer Eingabe im Textfeld: Popup filtern bzw. schließen, Auslösezeichen und zwei Wortzeichen erkennen.
    pub(crate) fn smart_after_edit(&mut self, doc: u64, g: u64, auto: bool) {
        if !matches!(self.smart.state_of(doc), State::Running(_)) {
            self.smart.popup = None;
            return;
        }
        let Some(e) = self.docs.get(&doc) else { return };
        let Some(d) = e.doc.as_ref() else { return };
        let Some(view) = self.panes.groups.get(&g).and_then(|grp| grp.active_tab()).map(|t| &t.view) else { return };
        let Some(mut sels) = view.sels.clone() else { return };
        d.sync(&mut sels);
        let head = sels.primary().head.min(d.len_chars());
        let prev = (head > 0).then(|| d.char_at(head - 1)).flatten();
        let triggers: Vec<char> = self.smart.docs.get(&doc).and_then(|x| self.smart.servers.entries.get(&x.key)).and_then(|en| en.client.as_ref()).map(|c| c.caps.triggers.iter().filter_map(|t| t.chars().next()).collect()).unwrap_or_default();
        if let Some(c) = prev
            && triggers.contains(&c)
        {
            self.smart_complete(doc, g, Some(c));
            return;
        }
        let ws = word_start(d, head);
        if let Some(p) = self.smart.popup.as_ref()
            && p.doc == doc
        {
            if head < p.start || ws != p.start || prev.is_none_or(|c| !crate::editor::text::is_word_char(c)) && head != p.start {
                self.smart.popup = None;
            } else if p.incomplete && !p.waiting {
                // Unvollständige Liste: mit dem längeren Wort neu fragen (das Popup bleibt bis zur Antwort).
                self.smart_complete(doc, g, None);
                return;
            } else {
                self.smart_refilter();
                return;
            }
        }
        if auto && head >= ws + 2 && prev.is_some_and(crate::editor::text::is_word_char) {
            self.smart.auto = Some((doc, g, Instant::now() + AUTO_PAUSE));
        }
    }

    /// Popup nach dem getippten Wortanfang filtern (unscharf: erst Anfang, dann Teilfolge).
    pub(crate) fn smart_refilter(&mut self) {
        let Some(p) = self.smart.popup.as_ref() else { return };
        let typed = self.docs.get(&p.doc).and_then(|e| e.doc.as_ref()).map(|d| {
            let idx = self.panes.groups.get(&p.group).map(|g| g.active).unwrap_or(0);
            let head = self.panes.groups.get(&p.group).and_then(|g| g.tabs.get(idx)).and_then(|t| t.view.sels.clone()).map(|mut s| {
                d.sync(&mut s);
                s.primary().head
            });
            head.filter(|h| *h >= p.start).map(|h| d.slice_string(p.start..h.min(d.len_chars()))).unwrap_or_default()
        });
        let typed = typed.unwrap_or_default();
        let p = self.smart.popup.as_mut().expect("Popup");
        p.shown = filter(&p.items, &typed);
        p.selected = p.shown.iter().position(|i| p.items[*i].preselect).unwrap_or(0);
        if !p.waiting && p.shown.is_empty() {
            self.smart.popup = None;
        }
    }

    /// Auswahl im Popup bewegen.
    pub(crate) fn smart_popup_move(&mut self, down: bool) {
        if let Some(p) = self.smart.popup.as_mut()
            && !p.shown.is_empty()
        {
            let n = p.shown.len();
            p.selected = if down { (p.selected + 1) % n } else { (p.selected + n - 1) % n };
        }
    }

    /// Gewählten Vorschlag übernehmen (ein Rückgängig-Schritt; Snippets mit Tab-Stopps).
    pub(crate) fn smart_accept(&mut self) -> bool {
        let Some(p) = self.smart.popup.take() else { return false };
        let Some(&i) = p.shown.get(p.selected) else { return false };
        let item = p.items[i].clone();
        let enc = self.smart.docs.get(&p.doc).map(|d| d.encoding).unwrap_or_default();
        let Some(idx) = self.panes.groups.get(&p.group).map(|g| g.active) else { return false };
        let (Some(view), Some(e)) = self.view_doc_mut(p.group, idx) else { return false };
        let Some(doc) = e.doc.as_mut() else { return false };
        view.sync(doc);
        let Some(sels) = view.sels.as_mut() else { return false };
        let head = sels.primary().head.min(doc.len_chars());
        let (range, raw) = match &item.text_edit {
            // Bereich des Servers nur in der Zeile des Cursors und mit Anfang vor ihm – sonst ab dem Wortanfang.
            Some((span, text)) => (accept_range(doc, span_to_chars(doc, *span, enc), head, p.start), text.clone()),
            None => (p.start.min(head)..head, item.insert.clone()),
        };
        let (text, stops) = if item.snippet { expand_snippet(&raw) } else { (raw.clone(), Vec::new()) };
        let mut edits = vec![TextEdit::new(range.clone(), text.clone())];
        for (span, t) in &item.additional {
            let r = span_to_chars(doc, *span, enc);
            if r.end <= range.start || r.start >= range.end {
                edits.push(TextEdit::new(r, t.clone()));
            }
        }
        edits.sort_by_key(|t| t.range.start);
        // Vor dem Hauptbereich eingefügte Zusatztexte verschieben ihn.
        let shift: isize = edits.iter().filter(|t| t.range.end <= range.start && t.range != range).map(|t| t.text.chars().count() as isize - t.range.len() as isize).sum();
        doc.break_undo_group();
        let ok = doc.apply_edits(&mut sels.clone(), edits, EditOrigin::User).unwrap_or(false);
        doc.break_undo_group();
        if !ok {
            return false;
        }
        let base = (range.start as isize + shift) as usize;
        let len = text.chars().count();
        let rev = doc.revision();
        let mk = |r: Range<usize>| {
            let mut s = Selections::single(Selection::new(base + r.start, base + r.end));
            s.rev = Some(rev);
            s
        };
        let mut snippet = None;
        match stops.split_first() {
            Some((first, rest)) => {
                *sels = mk(first.clone());
                if !rest.is_empty() {
                    snippet = Some(Snippet { doc: p.doc, group: p.group, stops: rest.iter().cloned().map(mk).collect() });
                }
            }
            None => *sels = mk(len..len),
        }
        view.reveal = true;
        e.on_user_edit();
        self.smart.snippet = snippet;
        true
    }

    /// Tab im Snippet: zum nächsten Stopp. Liefert, ob es einen gab.
    pub(crate) fn smart_next_stop(&mut self) -> bool {
        let Some(sn) = self.smart.snippet.as_mut() else { return false };
        let (doc_id, g) = (sn.doc, sn.group);
        if sn.stops.is_empty() {
            self.smart.snippet = None;
            return false;
        }
        let mut next = sn.stops.remove(0);
        if sn.stops.is_empty() {
            self.smart.snippet = None;
        }
        let Some(idx) = self.panes.groups.get(&g).map(|x| x.active) else { return false };
        let (Some(view), Some(e)) = self.view_doc_mut(g, idx) else { return false };
        let Some(doc) = e.doc.as_ref() else { return false };
        let _ = doc_id;
        doc.sync(&mut next);
        view.sels = Some(next);
        view.reveal = true;
        true
    }

    /// F8 / Umschalt+F8: zum nächsten/vorigen Problem im Dokument (ringsum). Liefert (Zeile, Spalte).
    pub(crate) fn smart_step_problem(&mut self, g: u64, forward: bool) -> Option<(usize, usize)> {
        let grp = self.panes.groups.get(&g)?;
        let t = grp.active_tab()?;
        let doc = t.doc;
        let list = self.smart.shown.get(&doc)?;
        if list.is_empty() {
            return None;
        }
        let d = self.docs.get(&doc)?;
        let cur = t.view.cursor_linecol(d);
        let mut v: Vec<(usize, usize)> = list.iter().map(|s| (s.line, s.col)).collect();
        v.sort();
        v.dedup();
        let target = if forward { v.iter().find(|p| **p > cur).or(v.first()) } else { v.iter().rev().find(|p| **p < cur).or(v.last()) }.copied()?;
        let idx = grp.active;
        let (Some(view), Some(e)) = self.view_doc_mut(g, idx) else { return None };
        view.goto(e, target.0, target.1);
        Some(target)
    }
}

/// Bereich, den ein Vorschlag ersetzt: der des Servers nur in der Zeile des Cursors und mit Anfang vor ihm (bis
/// mindestens zum Cursor), sonst vom Wortanfang `start` bis zum Cursor.
pub(crate) fn accept_range(doc: &crate::editor::Document, r: Range<usize>, head: usize, start: usize) -> Range<usize> {
    let line = doc.char_to_line(head);
    if r.start <= head && doc.char_to_line(r.start) == line && doc.char_to_line(r.end) == line {
        r.start..r.end.max(head)
    } else {
        start.min(head)..head
    }
}

/// Anfang des Worts vor `head`.
fn word_start(d: &crate::editor::Document, head: usize) -> usize {
    let mut s = head;
    while s > 0 && d.char_at(s - 1).is_some_and(crate::editor::text::is_word_char) {
        s -= 1;
    }
    s
}

/// LSP-Bereich → Zeichenbereich im Dokument (in seinen Grenzen).
fn span_to_chars(doc: &crate::editor::Document, span: proto::Span, enc: proto::Encoding) -> Range<usize> {
    let at = |p: proto::Pos| {
        let n = doc.len_lines();
        let line = (p.line as usize).min(n.saturating_sub(1));
        if p.line as usize >= n {
            return doc.len_chars();
        }
        let s = doc.line_string(line);
        let s = s.trim_end_matches(['\n', '\r']);
        doc.line_to_char(line) + enc.char_of(s, p.character)
    };
    let (a, b) = (at(span.start), at(span.end));
    a.min(b)..a.max(b)
}

fn to_shown(doc: &crate::editor::Document, items: &[proto::Diagnostic], enc: proto::Encoding) -> Vec<Shown> {
    items
        .iter()
        .map(|d| {
            let r = span_to_chars(doc, d.range, enc);
            let lc = crate::editor::text::pos_to_linecol(doc.rope(), r.start);
            Shown { range: r, line: lc.line, col: lc.col, severity: d.severity, message: d.message.clone(), unnecessary: d.unnecessary, deprecated: d.deprecated }
        })
        .collect()
}

/// Vorschläge filtern und sortieren: Anfang (ohne Groß/Klein) vor Teilfolge, dann `sortText`.
pub(crate) fn filter(items: &[proto::Completion], typed: &str) -> Vec<usize> {
    let t = typed.to_lowercase();
    let mut v: Vec<(u8, &str, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(i, it)| {
            let f = it.filter.to_lowercase();
            let rank = if t.is_empty() || f.starts_with(&t) {
                0
            } else if subsequence(&f, &t) {
                1
            } else {
                return None;
            };
            Some((rank, it.sort.as_str(), i))
        })
        .collect();
    v.sort();
    v.into_iter().map(|(_, _, i)| i).collect()
}

fn subsequence(hay: &str, needle: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|c| it.any(|h| h == c))
}

/// Größtes Snippet, das aufgelöst wird (länger: als Text eingefügt), und größte Schachtelungstiefe.
pub(crate) const SNIPPET_MAX: usize = 64 * 1024;
const SNIPPET_DEPTH: u32 = 32;

/// Snippet auflösen: `$1`, `${1}`, `${1:text}`, `${1|a,b|}` (erste Wahl), `$0`, `\$` → Text und die Tab-Stopps in der
/// Reihenfolge 1, 2, … und zuletzt 0 (Zeichenbereiche im Text). Variablen (`$TM_FILENAME`) bleiben leer. Über
/// [`SNIPPET_MAX`] Zeichen wird nichts aufgelöst; tiefer als 32 Ebenen bleibt der Rest wörtlich.
pub(crate) fn expand_snippet(s: &str) -> (String, Vec<Range<usize>>) {
    if s.len() > SNIPPET_MAX {
        let n = s.chars().count();
        return (s.to_string(), vec![n..n]);
    }
    struct P<'a> {
        c: &'a [char],
        i: usize,
        out: String,
        n: usize,
        stops: Vec<(u32, Range<usize>)>,
    }
    impl P<'_> {
        fn push(&mut self, ch: char) {
            self.out.push(ch);
            self.n += 1;
        }
        fn push_str(&mut self, s: &str) {
            for ch in s.chars() {
                self.push(ch);
            }
        }
        fn num(&mut self) -> Option<u32> {
            let start = self.i;
            let mut v = 0u32;
            while self.i < self.c.len() && self.c[self.i].is_ascii_digit() {
                v = v.saturating_mul(10).saturating_add(self.c[self.i].to_digit(10).unwrap_or(0));
                self.i += 1;
            }
            (self.i > start).then_some(v)
        }
        fn parse(&mut self, depth: u32) {
            while self.i < self.c.len() {
                let ch = self.c[self.i];
                if ch == '\\' && self.i + 1 < self.c.len() && matches!(self.c[self.i + 1], '$' | '}' | '\\') {
                    let x = self.c[self.i + 1];
                    self.push(x);
                    self.i += 2;
                    continue;
                }
                if ch == '}' && depth > 0 {
                    return;
                }
                if ch == '$' && self.i + 1 < self.c.len() && depth < SNIPPET_DEPTH {
                    let nx = self.c[self.i + 1];
                    if nx.is_ascii_digit() {
                        self.i += 1;
                        let num = self.num().unwrap_or(0);
                        let at = self.n;
                        self.stops.push((num, at..at));
                        continue;
                    }
                    if nx == '{' {
                        let save = self.i;
                        self.i += 2;
                        if let Some(num) = self.num() {
                            let at = self.n;
                            match self.c.get(self.i) {
                                Some(':') => {
                                    self.i += 1;
                                    self.parse(depth + 1);
                                    let end = self.n;
                                    self.stops.push((num, at..end));
                                    self.i += 1; // '}'
                                    continue;
                                }
                                Some('|') => {
                                    let close = (self.i + 1..self.c.len()).find(|k| self.c[*k] == '|').unwrap_or(self.c.len());
                                    let choice: String = self.c[self.i + 1..close].iter().collect();
                                    let first = choice.split(',').next().unwrap_or("").to_string();
                                    self.push_str(&first);
                                    self.stops.push((num, at..self.n));
                                    self.i = (close + 2).min(self.c.len());
                                    continue;
                                }
                                _ => {
                                    self.stops.push((num, at..at));
                                    self.i = (self.i + 1).min(self.c.len());
                                    continue;
                                }
                            }
                        }
                        // ${VAR} / ${VAR:default}: Standardtext übernehmen
                        let close = (self.i..self.c.len()).find(|k| self.c[*k] == '}').unwrap_or(self.c.len());
                        let inner: String = self.c[self.i..close].iter().collect();
                        if let Some((_, def)) = inner.split_once(':') {
                            let def = def.to_string();
                            self.push_str(&def);
                        }
                        self.i = (close + 1).min(self.c.len());
                        let _ = save;
                        continue;
                    }
                    if nx.is_ascii_alphabetic() || nx == '_' {
                        self.i += 1;
                        while self.i < self.c.len() && (self.c[self.i].is_ascii_alphanumeric() || self.c[self.i] == '_') {
                            self.i += 1;
                        }
                        continue;
                    }
                }
                self.push(ch);
                self.i += 1;
            }
        }
    }
    let chars: Vec<char> = s.chars().collect();
    let mut p = P { c: &chars, i: 0, out: String::with_capacity(s.len()), n: 0, stops: Vec::new() };
    p.parse(0);
    let (out, n, mut stops) = (p.out, p.n, p.stops);
    // Je Nummer der erste Stopp; Reihenfolge 1, 2, …, dann 0 (Ende).
    stops.sort_by_key(|(k, _)| if *k == 0 { u32::MAX } else { *k });
    let mut seen = HashSet::new();
    let mut v: Vec<Range<usize>> = stops.into_iter().filter(|(k, _)| seen.insert(*k)).map(|(_, r)| r).collect();
    if v.is_empty() {
        v.push(n..n);
    }
    (out, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_expand_with_tab_stops_in_order() {
        assert_eq!(expand_snippet("println!(\"$1\")$0"), ("println!(\"\")".to_string(), vec![10..10, 12..12]));
        assert_eq!(expand_snippet("fn ${1:name}(${2:args}) {\n\t$0\n}"), ("fn name(args) {\n\t\n}".to_string(), vec![3..7, 8..12, 17..17]));
        assert_eq!(expand_snippet("${1|a,b|} \\$x $TM_FILENAME ${X:def}"), ("a $x  def".to_string(), vec![0..1]));
        assert_eq!(expand_snippet("plain"), ("plain".to_string(), vec![5..5]));
        assert_eq!(expand_snippet("${1:a${2:b}c}"), ("abc".to_string(), vec![0..3, 1..2]));
        // Grenzen: zu lang = Text; sehr tief geschachtelt: kein Absturz, Rest wörtlich
        let long = "$1".repeat(SNIPPET_MAX);
        assert_eq!(expand_snippet(&long).0, long);
        let deep = format!("{}x{}", "${1:".repeat(100), "}".repeat(100));
        let (t, _) = expand_snippet(&deep);
        assert!(t.contains('x') && t.len() < deep.len());
    }

    /// Ende zu Ende mit dem Ersatz-Server (`examples/fake_lsp.rs`): Vertrauen, Diagnosen nach der Pause, Vervollständigen
    /// mit Snippet als ein Rückgängig-Schritt, Schließen. Ohne den Ersatz-Server übersprungen.
    #[test]
    fn trust_diagnostics_and_completion_with_the_fake_server() {
        let Some(exe) = crate::lsp::client::tests::fake_server_exe() else {
            eprintln!("fake_lsp fehlt – übersprungen");
            return;
        };
        let dir = std::env::temp_dir().join(format!("pd-smart-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let proj = dir.join("proj");
        std::fs::create_dir_all(proj.join(".git")).unwrap();
        std::fs::write(proj.join("Cargo.toml"), "[package]\n").unwrap();
        let file = proj.join("main.rs");
        std::fs::write(&file, "fn main() {\n    pr\n}\n").unwrap();
        let data = dir.join("data");
        std::fs::create_dir_all(&data).unwrap();
        let prefs = EditorPrefs::default();
        let settings = lsp::SmartSettings::default();
        let mut ed = EditorUi::default();
        ed.smart.test_resolve = Some(servers::Launch { program: exe.clone(), args: vec![], found: exe, rustup_proxy: false, lsp_root: None, raw_tail: None, invalid: None });
        let id = ed.open_path(&file, false);
        ed.docs.get_mut(&id).unwrap().ensure_loaded_now(&prefs);
        ed.smart_tick(&prefs, &settings, &data);
        assert_eq!(ed.smart.state_of(id), State::Off, "Syntax- bzw. Text-Modus: kein Server");
        ed.set_doc_mode(id, Some(Mode::Smart));
        // Wurzel wird im Hintergrund ermittelt
        let t0 = Instant::now();
        while ed.smart.state_of(id) != State::NeedsTrust(proj.clone()) {
            ed.smart_tick(&prefs, &settings, &data);
            assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", ed.smart.state_of(id));
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(ed.smart.servers.running(), 0, "ohne Vertrauen kein Server");
        ed.smart_trust(&proj, &data).unwrap();
        assert!(Trust::load(&data).trusted(&file), "in trust.json gemerkt");
        let t0 = Instant::now();
        loop {
            ed.smart_tick(&prefs, &settings, &data);
            if ed.smart.shown.get(&id).is_some_and(|v| !v.is_empty()) {
                break;
            }
            assert!(t0.elapsed() < Duration::from_secs(10), "keine Diagnose: {:?}", ed.smart.state_of(id));
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(matches!(ed.smart.state_of(id), State::Running(_)));
        assert_eq!(ed.smart.counts(id), (1, 0));
        assert_eq!(ed.smart.shown[&id][0].message, "fake error");
        // Vervollständigen am Ende von „pr“ (Zeile 2)
        let g = ed.panes.focused;
        let pos = ed.docs[&id].doc.as_ref().unwrap().line_to_char(1) + 6;
        {
            let idx = ed.panes.groups[&g].active;
            let (Some(v), Some(e)) = ed.view_doc_mut(g, idx) else { panic!("Ansicht") };
            let d = e.doc.as_ref().unwrap();
            v.sync(d);
            v.sels = Some(d.new_selections(pos));
        }
        ed.smart_complete(id, g, None);
        let t0 = Instant::now();
        while ed.smart.popup.as_ref().is_none_or(|p| p.waiting) {
            ed.smart_tick(&prefs, &settings, &data);
            assert!(t0.elapsed() < Duration::from_secs(10), "keine Vorschläge");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(ed.smart.popup.as_ref().unwrap().shown.len(), 2, "beide beginnen mit „pr“");
        // „print“ steht zuerst (sortText); „println“ (Snippet) wählen wie mit Pfeil runter
        ed.smart_popup_move(true);
        assert_eq!(ed.smart.popup.as_ref().map(|p| p.items[p.shown[p.selected]].label.clone()).as_deref(), Some("println"));
        assert!(ed.smart_accept());
        assert_eq!(ed.docs[&id].text().unwrap(), "fn main() {\n    println!(\"\")\n}\n");
        let head = ed.panes.groups[&g].tabs[0].view.sels.as_ref().unwrap().primary().head;
        assert_eq!(head, pos - 2 + "println!(\"".len(), "Cursor im ersten Tab-Stopp");
        assert!(ed.smart.snippet.is_some(), "noch $0");
        assert!(ed.smart_next_stop());
        assert!(ed.docs.get_mut(&id).unwrap().undo_user());
        assert_eq!(ed.docs[&id].text().unwrap(), "fn main() {\n    pr\n}\n", "ein Rückgängig-Schritt");
        // Zurück in den Syntax-Modus: beim Server geschlossen, keine Diagnosen mehr
        ed.set_doc_mode(id, Some(Mode::Syntax));
        ed.smart_tick(&prefs, &settings, &data);
        assert!(!ed.smart.shown.contains_key(&id));
        ed.smart_revoke(&proj, &data).unwrap();
        assert_eq!(ed.smart.servers.running(), 0, "Entziehen beendet die Server");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn server_ranges_are_only_taken_on_the_cursor_line_and_before_it() {
        let d = crate::editor::Document::from_text("let ab = 1;\nfoo.ba\n");
        let head = d.line_to_char(1) + 6; // nach „ba“
        let start = d.line_to_char(1) + 4;
        assert_eq!(accept_range(&d, d.line_to_char(1) + 4..d.line_to_char(1) + 6, head, start), start..head, "passt");
        assert_eq!(accept_range(&d, 0..3, head, start), start..head, "andere Zeile: ab Wortanfang");
        assert_eq!(accept_range(&d, head + 1..head + 1, head, start), start..head, "hinter dem Cursor");
        assert_eq!(accept_range(&d, d.line_to_char(1)..d.line_to_char(1) + 7, head, start), start..head, "bis in die nächste Zeile");
    }

    /// `C:\\vertraut\\..\\fremd\\…` gilt nicht als vertraut, und so geöffnete Dateien bekommen ihren echten Pfad.
    #[cfg(windows)]
    #[test]
    fn dot_dot_paths_are_resolved_before_trust_and_on_open() {
        let r = find_root(Path::new(r"C:\pd-none\trusted\..\evil\src\main.rs"), "rust", None).unwrap();
        assert_eq!(r.dir, PathBuf::from(r"C:\pd-none\evil\src"));
        let mut t = Trust::default();
        assert!(t.add(Path::new(r"C:\pd-none\trusted")));
        assert!(!t.trusted(&r.dir));
        let mut ed = EditorUi::default();
        let id = ed.open_path(Path::new(r"C:\pd-none\a\..\b\x.txt"), false);
        assert_eq!(ed.docs[&id].path.as_deref(), Some(Path::new(r"C:\pd-none\b\x.txt")));
    }

    #[test]
    fn completions_filter_by_prefix_then_subsequence() {
        let it = |l: &str| proto::Completion { label: l.into(), filter: l.into(), sort: l.into(), insert: l.into(), ..Default::default() };
        let items = vec![it("print"), it("println"), it("sprint"), it("len")];
        assert_eq!(filter(&items, "pr"), vec![0, 1, 2]);
        assert_eq!(filter(&items, "PRL"), vec![1], "Teilfolge, ohne Groß/Klein");
        assert_eq!(filter(&items, ""), vec![3, 0, 1, 2]);
        assert!(filter(&items, "zz").is_empty());
    }
}
