//! Zugriff der Agenten je Projekt (#386) in der UI-Schleife: Momentaufnahme bauen, Aufrufer einordnen, Werkzeuge vor
//! der Ausführung prüfen, Listen filtern, den einmaligen Hinweis „Freigeben …“ zeigen. Regeln in `crate::access`.

use super::*;
use crate::access::{self as acc, AccessMap, Area, Caller, ToolScope};
use crate::procs;

/// Kennzeichen des Toast-Knopfs „Freigeben …“ (Bit 62; Bit 61 = Ticket-Projekt, Bit 60 = „Allgemein“, Rest = Kennung).
pub(in crate::app) const ACCESS_TOKEN: u64 = 1 << 62;
const TICKET_BIT: u64 = 1 << 61;
const GENERAL_BIT: u64 = 1 << 60;

pub(in crate::app) fn area_token(a: Area) -> u64 {
    match a {
        Area::Project(p) => ACCESS_TOKEN | (p & (GENERAL_BIT - 1)),
        Area::Tickets(t) => ACCESS_TOKEN | TICKET_BIT | (t as u64 & (GENERAL_BIT - 1)),
        Area::General => ACCESS_TOKEN | GENERAL_BIT,
    }
}

pub(in crate::app) fn token_area(token: u64) -> Option<Area> {
    if token & ACCESS_TOKEN == 0 {
        return None;
    }
    let id = token & (GENERAL_BIT - 1);
    Some(if token & GENERAL_BIT != 0 {
        Area::General
    } else if token & TICKET_BIT != 0 {
        Area::Tickets(id as i64)
    } else {
        Area::Project(id)
    })
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

    /// Wer ruft auf: eine Sitzung (auch ein Chat) in ihrem Projekt, sonst ein externer Agent.
    pub(in crate::app) fn access_who(&self, caller: Option<u64>) -> Caller {
        caller.and_then(|c| self.entries.get(&c)).map(|e| Caller::Home(e.meta.project)).unwrap_or(Caller::External)
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
        m.view(who, self.settings.agent_create_projects, self.settings.agent_delete)
    }

    /// Prüfung vor der Ausführung (#386): Ziel-Sitzung, „Allgemein“, Screenshot des ganzen Fensters. Listen, Todo, Tickets,
    /// Kalender, Prozesse und `create_session` prüfen selbst. Bei `project_locked` erscheint einmal je Agent und Bereich ein
    /// Hinweis mit „Freigeben …“.
    pub(in crate::app) fn mcp_access_gate(&mut self, tool: &str, args: &Value, caller: Option<u64>, how: &str) -> Result<(), (String, String)> {
        let (m, who) = self.access_for(caller, how);
        let need = mcp::tool_level(tool).unwrap_or(McpLevel::Control);
        let session_area = |s: &Value| s.as_u64().and_then(|id| self.entries.get(&id)).map(|e| Area::Project(e.meta.project));
        let checked: Option<(Area, McpLevel)> = match acc::tool_scope(tool, args) {
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
                    None
                }
                // Verknüpfen braucht Lesen in der Sitzung (das Ticket prüft die Sicht der Tickets).
                "link_terminal" => args.get("session").and_then(session_area).map(|a| (a, McpLevel::Read)),
                _ => None,
            },
        };
        let Some((area, need)) = checked else { return Ok(()) };
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
        let who = match caller.and_then(|c| self.entries.get(&c)) {
            Some(e) => e.display_name().to_string(),
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
        if project == 0 || !self.projects.iter().any(|p| p.id == project) { m.general(who) } else { m.sidebar(who, project) }
    }

    /// Prozess-Werkzeuge (#386): Ziele prüfen, Listen filtern.
    pub(in crate::app) fn mcp_procs_scoped(&mut self, ctx: &egui::Context, tool: &str, args: &Value, caller: Option<u64>, how: &str) -> Result<Value, (String, String)> {
        let (m, who) = self.access_for(caller, how);
        let need = mcp::tool_level(tool).unwrap_or(McpLevel::Control);
        let level_of = |id: u64| self.procs.defs.iter().find(|d| d.id == id).map(|d| self.process_level(&m, who, d.project)).unwrap_or(McpLevel::Off);
        if tool != "list_processes" {
            let mut targets: Vec<u64> = Vec::new();
            if let Some(k) = args.get("process").filter(|v| !v.is_null()) {
                if let Some(d) = procs::find(&self.procs.defs, k) {
                    targets.push(d.id);
                }
            }
            if let Some(g) = args.get("group").and_then(|v| v.as_str()) {
                targets.extend(procs::group_members(&self.procs.defs, g.trim()));
            }
            for id in targets {
                acc::gate(level_of(id), need)?;
            }
        }
        let mut v = self.mcp_procs(ctx, tool, args)?;
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
    fn access_tokens_round_trip() {
        for a in [Area::Project(7), Area::Tickets(42), Area::General] {
            assert_eq!(token_area(area_token(a)), Some(a));
            assert!(area_token(a) & super::super::super::closing::RETIRED_TOKEN == 0, "kein Konflikt mit „Alle beenden“");
        }
        assert_eq!(token_area(5), None);
    }
}
