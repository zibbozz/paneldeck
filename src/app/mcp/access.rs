//! Zugriff der Agenten je Projekt (#386) in der UI-Schleife: Momentaufnahme bauen, Aufrufer einordnen, Werkzeuge vor
//! der Ausführung prüfen, Listen filtern, den einmaligen Hinweis „Freigeben …“ zeigen. Regeln in `crate::access`.

use super::*;
use crate::access::{self as acc, AccessMap, Area, Caller, ToolScope};
use crate::procs;

/// Kennzeichen des Toast-Knopfs „Freigeben …“ (Bit 62; Bit 61 = Ticket-Projekt, Bit 60 = „Allgemein“, Rest = Kennung).
pub(in crate::app) const ACCESS_TOKEN: u64 = 1 << 62;
const TICKET_BIT: u64 = 1 << 61;
const GENERAL_BIT: u64 = 1 << 60;
/// Kennung des Bereichs: Bits 0–58 (Bit 59 = „Ausgabe zeigen“ der Aktionen beim Speichern, #383).
const ID_MASK: u64 = (1 << 59) - 1;

pub(in crate::app) fn area_token(a: Area) -> u64 {
    match a {
        Area::Project(p) => ACCESS_TOKEN | (p & ID_MASK),
        Area::Tickets(t) => ACCESS_TOKEN | TICKET_BIT | (t as u64 & ID_MASK),
        Area::General => ACCESS_TOKEN | GENERAL_BIT,
    }
}

pub(in crate::app) fn token_area(token: u64) -> Option<Area> {
    if token & ACCESS_TOKEN == 0 {
        return None;
    }
    let id = token & ID_MASK;
    Some(if token & GENERAL_BIT != 0 {
        Area::General
    } else if token & TICKET_BIT != 0 {
        Area::Tickets(id as i64)
    } else {
        Area::Project(id)
    })
}

/// Nur die Kennungen sichtbarer Sitzungen (#386: `agent_chain`, `children`, `parent` nennen keine unsichtbaren).
pub(in crate::app) fn only_visible(ids: Vec<u64>, visible: &HashSet<u64>) -> Vec<u64> {
    ids.into_iter().filter(|i| visible.contains(i)).collect()
}

/// Liegt der Ordner `cwd` in einem der Ordner `locked` (gesperrte Projekte, #386)? Vergleich ohne Groß-/Kleinschreibung,
/// `\` = `/`, nur ganze Ordnernamen.
pub(in crate::app) fn cwd_hidden(cwd: &str, locked: &[String]) -> bool {
    let norm = |s: &str| s.replace('\\', "/").trim_end_matches('/').to_lowercase();
    let c = norm(cwd);
    locked.iter().map(|l| norm(l)).filter(|l| !l.is_empty()).any(|l| c == l || c.starts_with(&format!("{l}/")))
}

/// [`cwd_hidden`] für einen Dateipfad (auch in der Form `\\?\C:\…`).
pub(in crate::app) fn path_hidden(p: &std::path::Path, locked: &[String]) -> bool {
    let s = p.to_string_lossy();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    cwd_hidden(s, locked)
}

/// Dokument-Argumente eines Editor-Werkzeugs (`doc`, `other`: Kennung oder Pfad; `path`), die in einen gesperrten
/// Ordner zeigen (#386, Review 3.2.0)? `docs` = alle offenen Dokumente (Kennung, Pfad). Ein solcher Aufruf bekommt
/// `project_locked`, ohne dass das Werkzeug läuft.
pub(in crate::app) fn editor_args_hidden(args: &Value, docs: &[(u64, Option<std::path::PathBuf>)], locked: &[String]) -> bool {
    if locked.is_empty() {
        return false;
    }
    let by_id = |id: u64| docs.iter().find(|(d, _)| *d == id).and_then(|(_, p)| p.as_deref()).is_some_and(|p| path_hidden(p, locked));
    let refers = |v: &Value| match v {
        Value::Number(n) => n.as_u64().is_some_and(by_id),
        Value::String(s) => match s.trim().parse::<u64>() {
            Ok(id) => by_id(id),
            Err(_) => !s.trim().is_empty() && path_hidden(std::path::Path::new(s.trim()), locked),
        },
        _ => false,
    };
    ["doc", "other", "path"].iter().filter_map(|k| args.get(*k)).any(refers)
}

/// `list_save_actions` ohne die Regeln für Ordner gesperrter Projekte (#386, Review 3.2.0; ihr `path` nennte den Ordner).
pub(in crate::app) fn hide_save_action_rules(mut v: Value, locked: &[String]) -> Value {
    if let Some(rules) = v.get_mut("rules").and_then(Value::as_array_mut) {
        rules.retain(|r| !r["path"].as_str().map(str::trim).is_some_and(|p| !p.is_empty() && cwd_hidden(p, locked)));
    }
    v
}

/// Datenbank vor der Wiederherstellung ohne Zugriffs-Spalten (#386, Schema < 13)? Dann gelten die Standards (wie global, für
/// alle) – der Nutzer bekommt einen Hinweis.
pub(in crate::app) fn restore_resets_access(version: Option<u32>) -> bool {
    version.is_some_and(|v| v < 13)
}

/// Schema-Version einer Datenbankdatei, ohne sie zu verändern.
pub(in crate::app) fn file_schema_version(path: &std::path::Path) -> Option<u32> {
    let c = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    c.pragma_query_value(None, "user_version", |r| r.get(0)).ok()
}

/// `not_found` für Prozess-Werkzeuge mit den Prozessen bzw. Gruppen, die der Agent sieht (#386).
fn proc_not_found(visible: &[procs::ProcDef], what: &str, group: bool) -> (String, String) {
    let de = state::lang() == Lang::De;
    let msg = if group {
        let list = procs::groups(visible).join(", ");
        if de { format!("Keine Gruppe „{what}“. Vorhanden: {list}") } else { format!("No group “{what}”. Available: {list}") }
    } else {
        let list = visible.iter().map(|d| format!("{} ({})", d.name, d.id)).collect::<Vec<_>>().join(", ");
        if de { format!("Kein Prozess {what} in der Prozessliste. Vorhanden: {list}") } else { format!("No process {what} in the process list. Available: {list}") }
    };
    ("not_found".to_string(), msg)
}

/// Beginnt die Anleger-Kette von `id` bei einem externen Agenten (oder bei einer geschlossenen, unbekannten Sitzung)?
/// Vom Nutzer angelegte Sitzungen stehen nicht in `created`.
fn external_rooted(created: &HashMap<u64, super::Creator>, id: u64) -> bool {
    let mut cur = id;
    for _ in 0..64 {
        match created.get(&cur) {
            None => return false,
            Some(super::Creator::External) | Some(super::Creator::Closed) => return true,
            Some(super::Creator::Session(p)) if *p == cur => return false,
            Some(super::Creator::Session(p)) => cur = *p,
        }
    }
    true
}

/// Fenster und Projekt einer Sitzung aus `paneldeck --open` (#386): aus einer Sitzung heraus deren Fenster und Projekt.
pub(in crate::app) fn open_home(origin: Option<(u64, u64)>, active: u64) -> (u64, u64) {
    super::super::multiwin::child_home(origin, active)
}

/// Fremder Name (Sitzung eines Agenten) für einen Hinweis: Steuer- und täuschende Zeichen entschärft, eine Zeile,
/// höchstens 40 Zeichen (Review 3.2.0).
pub(in crate::app) fn toast_name(name: &str) -> String {
    mcp::short(&crate::hooks::untrusted_text(name, 80).replace('\n', " "), 40)
}

/// Flächen der offenen Fenster, Dialoge, Popups, Menüs, Hinweise und Tooltips im Hauptfenster (alle egui-Ebenen außer
/// dem Hintergrund mit Seitenleiste, Fensterleiste und Bereichen); `None` = Fläche unbekannt (gilt als ganzes Fenster).
pub(in crate::app) fn overlay_rects(ctx: &egui::Context) -> Vec<Option<egui::Rect>> {
    let layers = ctx.memory(|m| m.areas().visible_layer_ids());
    let mut v: Vec<Option<egui::Rect>> = layers.into_iter().filter(|l| l.order != egui::Order::Background).map(|l| egui::AreaState::load(ctx, l.id).map(|s| s.rect())).collect();
    if v.is_empty() && egui::Popup::is_any_open(ctx) {
        v.push(None);
    }
    v
}

/// Liegt eine dieser Flächen über `r` (unbekannte Flächen immer)?
pub(in crate::app) fn covered(overlays: &[Option<egui::Rect>], r: egui::Rect) -> bool {
    overlays.iter().any(|o| o.is_none_or(|o| o.intersects(r)))
}

/// Ausschnitt einer Sitzung für `screenshot` mit `session` (Review 3.2.0): nur ihr Inhalt, ohne die Kopfzeile (Name,
/// verknüpfte Tickets).
pub(in crate::app) fn shot_body(pane: egui::Rect, header: f32) -> Option<egui::Rect> {
    let body = egui::Rect::from_min_max(egui::pos2(pane.min.x, pane.min.y + header), pane.max);
    (body.width() >= 1.0 && body.height() >= 1.0).then_some(body)
}

fn shot_refused() -> (String, String) {
    (
        "forbidden".to_string(),
        state::t(
            "Ein Screenshot des ganzen Fensters geht nur, wenn alle Paneldeck-Fenster den Terminal Hub zeigen, kein Dialog, Menü oder Hinweis offen ist und keine Verknüpfung ein Ticket zeigt, das du nicht sehen darfst – nimm screenshot mit session (nur der Inhalt dieser Sitzung).",
            "A screenshot of the whole window is only possible while every Paneldeck window shows the Terminal Hub, no dialog, menu or notice is open and no link shows a ticket you may not see – use screenshot with session (only that session's content).",
        )
        .to_string(),
    )
}

fn shot_covered() -> (String, String) {
    (
        "not_visible".to_string(),
        state::t(
            "Über der Sitzung liegt gerade ein Dialog, Menü oder Hinweis – später noch einmal versuchen.",
            "A dialog, menu or notice currently covers the session – try again later.",
        )
        .to_string(),
    )
}

/// Was `mcp_access_gate` prüft (rein): Bereich und nötige Stufe des Aufrufs, `None` = das Werkzeug prüft selbst. Die
/// Stufe kommt aus den Argumenten (Review 3.2.0: `format_file` liest nur mit `op` = `validate`, alles andere braucht
/// Steuern – auch in „Allgemein“ und für Chats; unbekannt = Steuern). `session_area` = Bereich einer Sitzung.
pub(in crate::app) fn gate_target(m: &AccessMap, who: Caller, tool: &str, args: &Value, session_area: &dyn Fn(&Value) -> Option<Area>) -> Result<Option<(Area, McpLevel)>, (String, String)> {
    let need = mcp::call_level(tool, args).unwrap_or(McpLevel::Control);
    Ok(match acc::tool_scope(tool, args) {
        ToolScope::Session => args.get("session").and_then(session_area).map(|a| (a, need)),
        ToolScope::General => Some((Area::General, need)),
        ToolScope::Own => match tool {
            "screenshot" if args.get("session").is_some_and(|s| !s.is_null()) => args.get("session").and_then(session_area).map(|a| (a, McpLevel::Read)),
            "screenshot" => {
                if m.any_locked(who) {
                    return Err((
                        acc::LOCKED_CODE.to_string(),
                        state::t(
                            "Ein Screenshot des ganzen Fensters geht nur, wenn kein Projekt für dich gesperrt ist – nimm screenshot mit session (eine Sitzung, deren Projekt du lesen darfst).",
                            "A screenshot of the whole window is only possible when no project is locked for you – use screenshot with session (a session whose project you may read).",
                        )
                        .to_string(),
                    ));
                }
                // Was gerade zu sehen ist (Reiter, Dialoge, Verknüpfungen), prüft `App::shot_scope` mit dem egui-Kontext.
                None
            }
            // Verknüpfen braucht Lesen in der Sitzung (das Ticket prüft die Sicht der Tickets).
            "link_terminal" => args.get("session").and_then(session_area).map(|a| (a, McpLevel::Read)),
            _ => None,
        },
    })
}

/// Stufe des Projekts `project` für einen Chat, eingeordnet als `who` (Heimat oder extern).
pub(in crate::app) fn chat_cap(m: &AccessMap, who: Caller, project: u64) -> McpLevel {
    m.sidebar(who, project)
}

pub(in crate::app) fn level_name(l: McpLevel) -> &'static str {
    match l {
        McpLevel::Off => "off",
        McpLevel::Read => "read",
        McpLevel::Control => "control",
    }
}

impl App {
    /// Momentaufnahme aller Zugriffe mit der Obergrenze `global`.
    pub(in crate::app) fn access_map(&self, global: McpLevel) -> AccessMap {
        AccessMap {
            global,
            projects: self.projects.iter().map(|p| (p.id, p.agent_access)).collect(),
            general: self.settings.agent_general,
            tickets: self.db.as_ref().and_then(|d| crate::planning::access::ticket_access(&d.conn).ok()).unwrap_or_default(),
        }
    }

    /// Wer ruft auf: eine Sitzung (auch ein Chat) in ihrem Projekt, sonst ein externer Agent. Eine Sitzung, deren Kette
    /// der Anleger bei einem externen Agenten beginnt (oder bei einer geschlossenen Sitzung, die man nicht mehr kennt), gilt
    /// als extern – sonst bekäme ein externer Agent über ein selbst angelegtes Kind dessen Heimatprojekt (#386).
    pub(in crate::app) fn access_who(&self, caller: Option<u64>) -> Caller {
        match caller.and_then(|c| self.entries.get(&c).map(|e| (c, e))) {
            Some((c, _)) if external_rooted(&self.mcp.created, c) => Caller::External,
            Some((_, e)) => Caller::Home(e.meta.project),
            None => Caller::External,
        }
    }

    /// Obergrenze dieses Aufrufs: bei Chats die Stufe des Modells („wie global“ = wie das Modell), sonst die globale Stufe.
    fn access_global(&self, caller: Option<u64>, how: &str) -> McpLevel {
        match caller.filter(|_| how == "chat") {
            Some(chat) => self.chat_policy(chat).cfg.level,
            None => self.settings.mcp_level,
        }
    }

    pub(in crate::app) fn access_for(&self, caller: Option<u64>, how: &str) -> (AccessMap, Caller) {
        (self.access_map(self.access_global(caller, how)), self.access_who(caller))
    }

    /// Sicht für Todo/Tickets/Kalender.
    pub(in crate::app) fn agent_view(&self, caller: Option<u64>, how: &str) -> crate::access::AgentView {
        let (m, who) = self.access_for(caller, how);
        let mut v = m.view(who, self.settings.agent_create_projects, self.settings.agent_delete);
        // Terminal-Verknüpfungen nennen nur Sitzungen, die der Agent sieht (Review 3.2.0).
        v.sessions = Some(self.visible_sessions(caller, how));
        v
    }

    /// Obergrenze des Projekts eines Chats für seine Paneldeck-Werkzeuge (#386): wie `chat_project_cap`, aber mit der
    /// Einordnung des Aufrufers wie bei jedem Werkzeug (`access_who`) – ein Chat, den ein externer Agent angelegt hat,
    /// bekommt die strengere Stufe „extern“. Obergrenze „Steuern“: die Stufe des Modells begrenzt `ToolPolicy` selbst.
    pub(in crate::app) fn chat_cap_of(&self, chat: u64) -> McpLevel {
        let Some(project) = self.entries.get(&chat).map(|e| e.meta.project) else { return McpLevel::Off };
        let m = self.access_map(McpLevel::Control);
        chat_cap(&m, self.access_who(Some(chat)), project)
    }

    /// Stufe des Bereichs „Allgemein“ für einen Chat (Review 3.2.0), eingeordnet wie bei jedem Werkzeug; Obergrenze
    /// „Steuern“ (die Stufe des Modells begrenzt `ToolPolicy` selbst).
    pub(in crate::app) fn chat_general_cap(&self, chat: u64) -> McpLevel {
        if !self.entries.contains_key(&chat) {
            return McpLevel::Off;
        }
        self.access_map(McpLevel::Control).general(self.access_who(Some(chat)))
    }

    /// Tickets, die die Verknüpfungs-Chips (Seitenleiste, Terminalköpfe) gerade zeigen und die der Agent nicht sehen darf
    /// (privat, im Papierkorb, gesperrtes Projekt, unbekannt)?
    fn chips_hide_something(&self, view: &crate::access::AgentView) -> bool {
        let ids = self.tlinks.shown_ticket_ids();
        if ids.is_empty() {
            return false;
        }
        let Some(db) = self.db.as_ref() else { return true };
        ids.into_iter().any(|id| !crate::planning::tickets::get_ticket(&db.conn, id).is_ok_and(|t| !t.private && !t.trashed && view.ticket_level(t.project_id) > McpLevel::Off))
    }

    /// Was `screenshot` jetzt aufnehmen darf (Review 3.2.0), beim Anfordern und noch einmal beim Abschluss geprüft:
    /// * ohne `session` das ganze Fenster – nur, wenn alle Paneldeck-Fenster den Terminal Hub zeigen, kein Fenster,
    ///   Dialog, Popup, Menü, Hinweis oder Tooltip offen ist und kein Verknüpfungs-Chip ein für den Agenten unsichtbares
    ///   Ticket zeigt; sonst `forbidden` mit dem Hinweis auf `session` (gesperrte Bereiche prüft `mcp_access_gate`);
    /// * mit `session` nur der Inhalt dieser Sitzung (ohne Kopfzeile), und nur, wenn nichts darüber liegt.
    ///
    /// Liefert den Zuschnitt (`None` = ganzes Fenster).
    pub(in crate::app) fn shot_scope(&self, ctx: &egui::Context, session: Option<u64>, caller: Option<u64>, how: &str) -> Result<Option<egui::Rect>, (String, String)> {
        let overlays = overlay_rects(ctx);
        let hub = self.mode == super::super::titlebar::Mode::Terminals;
        match session {
            None => {
                if !hub || !self.wins.all_show_terminals() || !overlays.is_empty() || self.chips_hide_something(&self.agent_view(caller, how)) {
                    return Err(shot_refused());
                }
                Ok(None)
            }
            Some(id) => {
                let not_visible = || {
                    (
                        "not_visible".to_string(),
                        state::t(
                            "Die Sitzung ist im Fenster nicht zu sehen (anderes Projekt, anderer Reiter oder ausgeblendet) – erst focus_session aufrufen.",
                            "The session is not visible in the window (other project, other tab or hidden) – call focus_session first.",
                        )
                        .to_string(),
                    )
                };
                let pane = self.pane_rects.get(&id).copied().filter(|_| hub).ok_or_else(not_visible)?;
                let body = shot_body(pane, super::super::HEADER_H).ok_or_else(not_visible)?;
                if covered(&overlays, body) {
                    return Err(shot_covered());
                }
                Ok(Some(body))
            }
        }
    }

    /// Prüfung vor der Ausführung (#386): Ziel-Sitzung, „Allgemein“, Screenshot des ganzen Fensters. Listen, Todo, Tickets,
    /// Kalender, Prozesse und `create_session` prüfen selbst. Bei `project_locked` erscheint einmal je Agent und Bereich ein
    /// Hinweis mit „Freigeben …“.
    pub(in crate::app) fn mcp_access_gate(&mut self, tool: &str, args: &Value, caller: Option<u64>, how: &str) -> Result<(), (String, String)> {
        let (m, who) = self.access_for(caller, how);
        let session_area = |s: &Value| s.as_u64().and_then(|id| self.entries.get(&id)).map(|e| Area::Project(e.meta.project));
        let Some((area, need)) = gate_target(&m, who, tool, args, &session_area)? else { return Ok(()) };
        let r = acc::gate(m.area(who, area), need);
        if let Err((code, _)) = &r {
            if code == acc::LOCKED_CODE {
                self.access_hint(caller, how, area);
            }
        }
        r
    }

    /// Einmal je Agent und Bereich: Hinweis „Ein Agent wollte … – Freigeben …“ (öffnet den Dialog „Zugriff für Agenten“).
    pub(in crate::app) fn access_hint(&mut self, caller: Option<u64>, how: &str, area: Area) {
        // Der Sitzungsname stammt u. U. vom Agenten selbst (`rename_session`, OSC-Titel): entschärft und gekürzt (Review 3.2.0).
        let who = match caller.and_then(|c| self.entries.get(&c)) {
            Some(e) => toast_name(&e.display_name()),
            None => if how == "chat" { "chat".into() } else { state::t("externer Agent", "external agent").to_string() },
        };
        if !self.mcp.access_hinted.insert((who.clone(), area)) {
            return;
        }
        let lang = self.settings.language;
        let name = self.area_name(area);
        let text = if lang == Lang::De {
            format!("Agent „{who}“ wollte auf „{name}“ zugreifen – das ist für Agenten gesperrt.")
        } else {
            format!("Agent “{who}” tried to access “{name}” – it is locked for agents.")
        };
        let action = super::toasts::ToastAction { label: lang.t("Freigeben …", "Release …").to_string(), token: area_token(area), window: self.wins.cur };
        self.push_toast_action(text, super::toasts::ToastKind::Info, None, Some(action));
    }

    /// Bereich eines Ticket-Werkzeugs aus `ticket` (`PAN-12`) bzw. `project` (`PAN`): verknüpft = Seitenleisten-Projekt.
    pub(in crate::app) fn ticket_area_of(&self, args: &Value) -> Option<Area> {
        let key = args.get("ticket").and_then(|t| t.as_str()).and_then(|t| t.rsplit_once('-')).map(|(k, _)| k.to_string()).or_else(|| args.get("project").and_then(|p| p.as_str()).map(str::to_string))?;
        let db = self.db.as_ref()?;
        let p = crate::planning::projects::project_by_key(&db.conn, &key).ok()?;
        let (_, link) = crate::planning::access::project_access(&db.conn, p.id).ok()?;
        Some(match link.filter(|l| self.projects.iter().any(|x| x.id == *l)) {
            Some(l) => Area::Project(l),
            None => Area::Tickets(p.id),
        })
    }

    /// Anzeigename eines Bereichs (nur für den Nutzer).
    pub(in crate::app) fn area_name(&self, area: Area) -> String {
        match area {
            Area::Project(p) => self.projects.iter().find(|x| x.id == p).map(|x| x.name.clone()).unwrap_or_default(),
            Area::Tickets(t) => self.db.as_ref().and_then(|d| crate::planning::projects::get_project(&d.conn, t).ok()).map(|p| p.key).unwrap_or_default(),
            Area::General => state::t("Allgemein", "General").to_string(),
        }
    }

    /// Sitzungen, die dieser Agent sieht (Projekt mindestens Lesen).
    pub(in crate::app) fn visible_sessions(&self, caller: Option<u64>, how: &str) -> HashSet<u64> {
        let (m, who) = self.access_for(caller, how);
        self.entries.iter().filter(|(_, e)| m.sidebar(who, e.meta.project) > McpLevel::Off).map(|(id, _)| *id).collect()
    }

    /// Ordner der für diesen Agenten gesperrten Projekte (Standardordner und Ordner ihrer Sitzungen) – für `search_history`.
    pub(in crate::app) fn locked_folders(&self, caller: Option<u64>, how: &str) -> Vec<String> {
        let (m, who) = self.access_for(caller, how);
        let locked: HashSet<u64> = self.projects.iter().filter(|p| m.sidebar(who, p.id) == McpLevel::Off).map(|p| p.id).collect();
        let mut out: Vec<String> = self.projects.iter().filter(|p| locked.contains(&p.id)).map(|p| p.folder.clone()).collect();
        out.extend(self.entries.values().filter(|e| locked.contains(&e.meta.project)).map(|e| e.cwd_now()));
        out.retain(|f| !f.trim().is_empty());
        out
    }

    /// `whoami`-Teil: erreichbare Projekte mit Stufe (`home` = Projekt der eigenen Sitzung) und „Allgemein“.
    pub(in crate::app) fn access_json(&self, caller: Option<u64>, how: &str) -> Value {
        let (m, who) = self.access_for(caller, how);
        let projects: Vec<Value> = self
            .projects
            .iter()
            .filter_map(|p| {
                let l = m.sidebar(who, p.id);
                (l > McpLevel::Off).then(|| json!({"id": p.id, "name": p.name, "level": level_name(l), "home": who == Caller::Home(p.id)}))
            })
            .collect();
        json!({"projects": projects, "general": level_name(m.general(who))})
    }

    /// Projekt für `create_session` (#386): `project` (Name oder Kennung) braucht Steuern; ohne: das Heimatprojekt, bei
    /// externen Agenten das aktive Projekt – jeweils nur mit Steuern.
    pub(in crate::app) fn create_project_target(&mut self, args: &Value, caller: Option<u64>, how: &str) -> Result<u64, (String, String)> {
        let (m, who) = self.access_for(caller, how);
        let target = match args.get("project").filter(|v| !v.is_null()) {
            Some(v) => {
                let found = match v {
                    Value::Number(n) => n.as_u64().and_then(|id| self.projects.iter().find(|p| p.id == id)),
                    Value::String(s) => self.projects.iter().find(|p| p.name.eq_ignore_ascii_case(s.trim()) || p.id.to_string() == s.trim()),
                    _ => None,
                };
                match found {
                    Some(p) => p.id,
                    None => {
                        // Nur erreichbare Projekte nennen.
                        let names: Vec<String> = self.projects.iter().filter(|p| m.sidebar(who, p.id) > McpLevel::Off).map(|p| p.name.clone()).collect();
                        return Err(er("bad_args", if de() { format!("Unbekanntes Projekt (verfügbar: {})", names.join(", ")) } else { format!("Unknown project (available: {})", names.join(", ")) }));
                    }
                }
            }
            None => match who {
                // Kinder bleiben im Projekt ihres Anlegers.
                Caller::Home(_) => super::multiwin::child_home(caller.and_then(|c| self.entries.get(&c)).map(|e| (e.meta.window, e.meta.project)), self.active_project).1,
                Caller::External => self.active_project,
            },
        };
        let r = acc::gate(m.sidebar(who, target), McpLevel::Control);
        if let Err((code, _)) = &r {
            if code == acc::LOCKED_CODE {
                self.access_hint(caller, how, Area::Project(target));
            }
        }
        r.map(|_| target)
    }

    /// `list_processes` u. a.: Prozesse dieses Agenten (Projekt 0 = „Allgemein“).
    pub(in crate::app) fn process_level(&self, m: &AccessMap, who: Caller, project: u64) -> McpLevel {
        // Projekt 0 = „Allgemein“; ein unbekanntes (gelöschtes) Projekt ist gesperrt (#386).
        if project == 0 { m.general(who) } else { m.sidebar(who, project) }
    }

    /// Prozess-Werkzeuge (#386): Ziele prüfen, Listen filtern.
    pub(in crate::app) fn mcp_procs_scoped(&mut self, ctx: &egui::Context, tool: &str, args: &Value, caller: Option<u64>, how: &str) -> Result<Value, (String, String)> {
        let (m, who) = self.access_for(caller, how);
        let need = mcp::tool_level(tool).unwrap_or(McpLevel::Control);
        // Nur sichtbare Prozesse (#386): unsichtbare gelten als unbekannt, Fehlerlisten nennen nur sichtbare.
        let visible_defs: Vec<procs::ProcDef> = self.procs.defs.iter().filter(|d| self.process_level(&m, who, d.project) > McpLevel::Off).cloned().collect();
        let level_of = |id: u64| self.procs.defs.iter().find(|d| d.id == id).map(|d| self.process_level(&m, who, d.project)).unwrap_or(McpLevel::Off);
        if tool != "list_processes" {
            if let Some(k) = args.get("process").filter(|v| !v.is_null()) {
                let Some(d) = procs::find(&visible_defs, k) else { return Err(proc_not_found(&visible_defs, &k.to_string(), false)) };
                acc::gate(level_of(d.id), need)?;
            }
            if let Some(g) = args.get("group").and_then(|v| v.as_str()).map(str::trim).filter(|g| !g.is_empty()) {
                let members = procs::group_members(&visible_defs, g);
                if members.is_empty() {
                    return Err(proc_not_found(&visible_defs, g, true));
                }
                for id in &members {
                    acc::gate(level_of(*id), need)?;
                }
                // Gehören der Gruppe auch unsichtbare Prozesse an, nur die sichtbaren einzeln steuern.
                if members.len() != procs::group_members(&self.procs.defs, g).len() {
                    let mut out: Vec<Value> = Vec::new();
                    let mut note = Value::Null;
                    for id in members {
                        let v = self.mcp_procs(ctx, tool, &json!({"process": id}))?;
                        out.extend(v["processes"].as_array().cloned().unwrap_or_default());
                        note = v["note"].clone();
                    }
                    return Ok(json!({"processes": out, "note": note}));
                }
            }
        }
        let mut v = self.mcp_procs(ctx, tool, args).map_err(|(code, msg)| if code == "not_found" { proc_not_found(&visible_defs, "", false) } else { (code, msg) })?;
        if tool == "list_processes" {
            let visible: HashSet<u64> = self.procs.defs.iter().filter(|d| self.process_level(&m, who, d.project) > McpLevel::Off).map(|d| d.id).collect();
            if let Some(list) = v.get_mut("processes").and_then(|l| l.as_array_mut()) {
                list.retain(|p| p["id"].as_u64().is_some_and(|id| visible.contains(&id)));
            }
            let defs: Vec<procs::ProcDef> = self.procs.defs.iter().filter(|d| visible.contains(&d.id)).cloned().collect();
            v["groups"] = json!(procs::groups(&defs));
        }
        Ok(v)
    }

    /// Höchste erreichbare Stufe für `tools/list` (der Hub antwortet ohne Aufrufer); höchstens alle 2 s neu berechnet.
    pub(in crate::app) fn publish_reach(&mut self) {
        let now = Instant::now();
        if self.mcp.reach_at.is_some_and(|t| now.duration_since(t) < Duration::from_secs(2)) {
            return;
        }
        self.mcp.reach_at = Some(now);
        let reach = if self.settings.mcp_level == McpLevel::Off { McpLevel::Off } else { self.access_map(self.settings.mcp_level).highest() };
        mcp::REACH.store(reach.as_u8(), std::sync::atomic::Ordering::Relaxed);
    }

    /// Projekt eines Aufrufs für `mcp-log.jsonl` (#386): Projekt der Ziel-Sitzung, Schlüssel des Tickets/Projekts.
    pub(in crate::app) fn log_project(&self, tool: &str, args: &Value) -> Option<String> {
        if let Some(e) = args.get("session").and_then(|s| s.as_u64()).and_then(|id| self.entries.get(&id)) {
            return self.projects.iter().find(|p| p.id == e.meta.project).map(|p| p.name.clone());
        }
        if let Some(t) = args.get("ticket").and_then(|t| t.as_str()) {
            return t.rsplit_once('-').map(|(k, _)| k.to_uppercase());
        }
        if let Some(p) = args.get("project").filter(|_| tool != "create_session") {
            return p.as_str().map(|s| s.to_uppercase());
        }
        if let Some(p) = args.get("project") {
            return p.as_str().map(str::to_string).or_else(|| p.as_u64().and_then(|id| self.projects.iter().find(|x| x.id == id)).map(|x| x.name.clone()));
        }
        None
    }

    /// Hinweis, wenn ein Bereich durch Verschieben für Agenten erreichbar wird (#386, Entscheidung 18).
    pub(in crate::app) fn access_gain_hint(&mut self, from: Area, to: Area) {
        let m = self.access_map(self.settings.mcp_level);
        let best = |a: Area| [Caller::External].into_iter().chain(self.projects.iter().map(|p| Caller::Home(p.id))).map(|w| m.area(w, a)).max().unwrap_or(McpLevel::Off);
        if best(to) > best(from) {
            let lang = self.settings.language;
            let text = if lang == Lang::De {
                format!("Hinweis: In „{}“ haben Agenten mehr Zugriff als vorher – das gilt sofort.", self.area_name(to))
            } else {
                format!("Note: agents have more access in “{}” than before – this applies immediately.", self.area_name(to))
            };
            self.push_toast(text, super::toasts::ToastKind::Info, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_sessions_and_folders_are_not_named() {
        let visible: HashSet<u64> = [1, 3].into_iter().collect();
        assert_eq!(only_visible(vec![1, 2, 3, 4], &visible), vec![1, 3]);
        let locked = vec!["C:\\Work\\Secret".to_string(), String::new()];
        assert!(cwd_hidden("c:/work/secret", &locked));
        assert!(cwd_hidden("C:\\Work\\Secret\\sub", &locked));
        assert!(!cwd_hidden("C:\\Work\\SecretSauce", &locked), "nur ganze Ordnernamen");
        assert!(!cwd_hidden("C:\\Other", &locked));
        assert!(!cwd_hidden("C:\\", &[String::new()]), "leere Vorgabe sperrt nichts");
    }

    #[test]
    fn restoring_an_old_database_resets_access_with_a_notice() {
        assert!(restore_resets_access(Some(12)) && restore_resets_access(Some(3)));
        assert!(!restore_resets_access(Some(13)) && !restore_resets_access(None));
    }

    #[test]
    fn process_errors_list_only_visible_processes() {
        let visible = vec![procs::ProcDef { group: "dev".into(), ..procs::ProcDef::new(1, "web", "npm run dev") }];
        let (code, msg) = proc_not_found(&visible, "\"secret-db\"", false);
        assert_eq!(code, "not_found");
        assert!(msg.contains("web (1)") && !msg.contains("hidden"), "{msg}");
        let (_, msg) = proc_not_found(&visible, "ops", true);
        assert!(msg.contains("dev"), "{msg}");
    }

    #[test]
    fn children_of_external_agents_stay_external() {
        use super::super::Creator;
        let mut c: HashMap<u64, Creator> = HashMap::new();
        assert!(!external_rooted(&c, 1), "vom Nutzer angelegt");
        c.insert(2, Creator::External);
        c.insert(3, Creator::Session(2));
        c.insert(4, Creator::Session(3));
        c.insert(5, Creator::Session(1));
        c.insert(6, Creator::Closed);
        assert!(external_rooted(&c, 2) && external_rooted(&c, 3) && external_rooted(&c, 4), "Kette eines externen Agenten");
        assert!(!external_rooted(&c, 5), "Kind eines Agenten in einer Nutzer-Sitzung");
        assert!(external_rooted(&c, 6), "unbekannter Anleger: sicher extern");
        c.insert(7, Creator::Session(8));
        c.insert(8, Creator::Session(7));
        assert!(external_rooted(&c, 7), "Kreis: sicher extern");
    }

    #[test]
    fn paneldeck_open_from_a_session_uses_its_window_and_project() {
        assert_eq!(open_home(Some((2, 7)), 1), (2, 7));
        assert_eq!(open_home(None, 1), (0, 1));
    }

    fn map_general(general: crate::access::AreaLevel, global: McpLevel) -> AccessMap {
        let mut m = AccessMap { global, general: crate::access::AgentAccess { level: general, for_all: true }, ..Default::default() };
        m.projects.insert(1, crate::access::AgentAccess { level: crate::access::AreaLevel::Control, for_all: false });
        m
    }

    /// Review 3.2.0 (Blocker): `format_file` schreibt nur mit Steuern in „Allgemein“ – auch wenn die Stufe des Werkzeugs
    /// (Lesen, wegen `op` = `validate`) niedriger ist. Gilt für externe Agenten und Chats (gleiche Prüfung im Hub).
    #[test]
    fn format_needs_control_in_general_even_when_the_tool_level_is_read() {
        use crate::access::AreaLevel;
        let none = |_: &Value| None;
        for who in [Caller::External, Caller::Home(1)] {
            let m = map_general(AreaLevel::Read, McpLevel::Control);
            for op in ["format", "minify", "sort_keys"] {
                let (area, need) = gate_target(&m, who, "format_file", &json!({"op": op}), &none).unwrap().unwrap();
                assert_eq!((area, need), (Area::General, McpLevel::Control), "{op}");
                assert_eq!(acc::gate(m.area(who, area), need).unwrap_err().0, "forbidden", "{who:?} {op}: General = Read");
            }
            let (area, need) = gate_target(&m, who, "format_file", &json!({"op": "validate"}), &none).unwrap().unwrap();
            assert!(acc::gate(m.area(who, area), need).is_ok(), "validate liest nur");
            // „Allgemein“ gesperrt: auch validate nicht
            let off = map_general(AreaLevel::Off, McpLevel::Control);
            let (area, need) = gate_target(&off, who, "format_file", &json!({"op": "validate"}), &none).unwrap().unwrap();
            assert_eq!(acc::gate(off.area(who, area), need).unwrap_err().0, acc::LOCKED_CODE);
            // Steuern in „Allgemein“: formatieren erlaubt
            let ctl = map_general(AreaLevel::Control, McpLevel::Control);
            let (area, need) = gate_target(&ctl, who, "format_file", &json!({"op": "format"}), &none).unwrap().unwrap();
            assert!(acc::gate(ctl.area(who, area), need).is_ok());
        }
        // Ein Chat mit Modell-Stufe Lesen (Obergrenze) kommt auch mit „Allgemein“ = Steuern nicht zum Formatieren.
        let chat = map_general(AreaLevel::Control, McpLevel::Read);
        let (area, need) = gate_target(&chat, Caller::Home(1), "format_file", &json!({"op": "format"}), &none).unwrap().unwrap();
        assert_eq!(acc::gate(chat.area(Caller::Home(1), area), need).unwrap_err().0, "forbidden");
        // Unbekanntes Werkzeug: „Allgemein“ mit Steuern (nie ohne Prüfung)
        assert_eq!(gate_target(&chat, Caller::Home(1), "no_such_tool", &json!({}), &none).unwrap(), Some((Area::General, McpLevel::Control)));
    }

    /// Review 3.2.0: Die Obergrenze eines Chats folgt derselben Einordnung wie seine Werkzeugaufrufe – ein von einem
    /// externen Agenten angelegter Chat gilt als extern (ein Projekt ohne „Für alle“ ist dann gesperrt).
    #[test]
    fn chat_caps_use_the_stricter_external_view() {
        use crate::access::{AgentAccess, AreaLevel};
        let mut m = AccessMap { global: McpLevel::Control, ..Default::default() };
        m.projects.insert(1, AgentAccess { level: AreaLevel::Control, for_all: false });
        m.projects.insert(2, AgentAccess { level: AreaLevel::Read, for_all: true });
        assert_eq!(chat_cap(&m, Caller::Home(1), 1), McpLevel::Control);
        assert_eq!(chat_cap(&m, Caller::External, 1), McpLevel::Off, "extern: nur „Für alle“");
        assert_eq!(chat_cap(&m, Caller::External, 2), McpLevel::Read);
        assert_eq!(chat_cap(&m, Caller::Home(1), 99), McpLevel::Off, "unbekanntes Projekt");
    }

    #[test]
    fn session_screenshots_show_only_the_terminal_body() {
        let pane = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(400.0, 300.0));
        let body = shot_body(pane, 30.0).unwrap();
        assert_eq!(body, egui::Rect::from_min_max(egui::pos2(10.0, 50.0), egui::pos2(410.0, 320.0)), "ohne Kopfzeile (Name, Ticket-Chips)");
        assert_eq!(shot_body(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(100.0, 30.0)), 30.0), None, "nur Kopfzeile");
        // Etwas darüber (Dialog, Menü, Hinweis) – oder eine Fläche unbekannter Lage – verhindert das Bild.
        assert!(!covered(&[], body));
        assert!(covered(&[Some(egui::Rect::from_min_size(egui::pos2(300.0, 300.0), egui::vec2(50.0, 50.0)))], body));
        assert!(!covered(&[Some(egui::Rect::from_min_size(egui::pos2(500.0, 0.0), egui::vec2(50.0, 50.0)))], body));
        assert!(covered(&[None], body));
    }

    #[test]
    fn open_windows_popups_and_toasts_count_as_overlays() {
        let ctx = egui::Context::default();
        let frame = |window: bool, toast: bool| {
            let mut out = Vec::new();
            for _ in 0..2 {
                let _ = ctx.run(egui::RawInput::default(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| ui.label("terminal"));
                    if window {
                        egui::Window::new("Tickets").show(ctx, |ui| ui.label("PAN-1 secret"));
                    }
                    if toast {
                        egui::Area::new(egui::Id::new("toast")).order(egui::Order::Tooltip).show(ctx, |ui| ui.label("t"));
                    }
                    out = overlay_rects(ctx);
                });
            }
            out
        };
        assert!(frame(false, false).is_empty(), "nur Hintergrund (Bereiche, Seitenleiste)");
        assert!(!frame(true, false).is_empty(), "Fenster");
        assert!(!frame(false, true).is_empty(), "Hinweis/Tooltip");
    }

    /// Review 3.2.0: Editor-Werkzeuge und `list_save_actions` nennen keine Ordner gesperrter Projekte.
    #[test]
    fn editor_and_save_actions_hide_locked_folders() {
        let locked = vec![r"C:\Work\Secret".to_string()];
        let docs = vec![(1, Some(std::path::PathBuf::from(r"C:\Work\Secret\a.rs"))), (2, Some(std::path::PathBuf::from(r"C:\Work\Open\b.rs"))), (3, None)];
        for a in [json!({"doc": 1}), json!({"doc": "1"}), json!({"doc": r"c:/work/secret/a.rs"}), json!({"path": r"C:\Work\Secret\new.txt"}), json!({"doc": 2, "other": 1})] {
            assert!(editor_args_hidden(&a, &docs, &locked), "{a}");
        }
        for a in [json!({"doc": 2}), json!({"doc": 3}), json!({"path": r"C:\Work\SecretSauce\x"}), json!({}), json!({"doc": 99})] {
            assert!(!editor_args_hidden(&a, &docs, &locked), "{a}");
        }
        assert!(!editor_args_hidden(&json!({"doc": 1}), &docs, &[]), "nichts gesperrt");
        assert!(path_hidden(std::path::Path::new(r"\\?\C:\Work\Secret\a.rs"), &locked));
        let v = json!({"rules": [{"id": 1, "path": r"C:\Work\Secret"}, {"id": 2, "path": r"C:\Work\Open"}, {"id": 3, "path": ""}], "enabled": true});
        let v = hide_save_action_rules(v, &locked);
        let ids: Vec<u64> = v["rules"].as_array().unwrap().iter().map(|r| r["id"].as_u64().unwrap()).collect();
        assert_eq!(ids, vec![2, 3]);
        assert!(!v.to_string().contains("Secret"));
    }

    #[test]
    fn toast_names_are_defused() {
        let n = toast_name("evil\u{202e}gnp.exe\nsecond line\u{7}");
        assert!(!n.contains('\u{202e}') && !n.contains('\n') && !n.contains('\u{7}'), "{n:?}");
        assert!(toast_name(&"x".repeat(500)).chars().count() <= 41);
    }

    #[test]
    fn access_tokens_round_trip() {
        for a in [Area::Project(7), Area::Tickets(42), Area::General] {
            assert_eq!(token_area(area_token(a)), Some(a));
            assert!(area_token(a) & super::super::super::closing::RETIRED_TOKEN == 0, "kein Konflikt mit „Alle beenden“");
            assert!(area_token(a) & super::super::super::save_actions_ui::SA_TOKEN == 0, "kein Konflikt mit „Ausgabe zeigen“ (#383)");
        }
        assert_eq!(token_area(5), None);
        assert_eq!(token_area(super::super::super::save_actions_ui::SA_TOKEN | 3), None);
    }
}
