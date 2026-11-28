//! Zugriff der Agenten je Projekt (#386, 3.2.0): Stufe je Seitenleisten-Projekt, je Ticket-Projekt und für den
//! Bereich „Allgemein“ (alles ohne Projekt), dazu der Schalter „Für alle“. Kennt weder egui noch die App.
//!
//! Regeln (Entscheidungen in Issue #386):
//! * Die globale Stufe (`settings.json` `mcp_level`, bei Chats die Stufe des Modells) ist die **Obergrenze**: ein Bereich
//!   hat höchstens sie; „wie global“ übernimmt sie; global Aus schaltet alles aus.
//! * Ein Agent in einer Paneldeck-Sitzung arbeitet im Projekt dieser Sitzung (**Heimatprojekt**). Andere Projekte erreicht
//!   er nur, wenn sie „Für alle“ freigegeben sind; externe Agenten (ohne Sitzung) erreichen nur „Für alle“-Projekte.
//! * „Allgemein“ und Ticket-Projekte ohne Verknüpfung gehören keinem Seitenleisten-Projekt: Ihre Stufe gilt für Agenten in
//!   jeder Paneldeck-Sitzung, „Für alle“ nimmt externe Agenten dazu.
//! * Ein mit einem Seitenleisten-Projekt verknüpftes Ticket-Projekt folgt dessen Stufe und Schalter.
//! * Todo-Karten gehören einem Seitenleisten-Projekt oder „Allgemein“; zusätzlich gilt die Stufe des System-Projekts TODO.
//! * Nach dem Update steht alles auf „wie global“ + „Für alle“ (es ändert sich nichts); neue, vom Nutzer angelegte oder
//!   importierte Projekte beginnen **Aus** ([`AgentAccess::LOCKED`]).
//! * Private Karten, Tags und Tickets sieht nie ein Agent ([`AgentView`] blendet sie aus, direkter Zugriff = `not_found`).

use crate::mcp::McpLevel;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Stufe eines Bereichs: „wie global“ oder fest.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AreaLevel {
    /// Übernimmt die globale Stufe (bei Chats: die Stufe des Modells).
    #[default]
    Global,
    Off,
    Read,
    Control,
}

impl AreaLevel {
    pub const ALL: [AreaLevel; 4] = [AreaLevel::Global, AreaLevel::Off, AreaLevel::Read, AreaLevel::Control];

    pub fn as_str(self) -> &'static str {
        match self {
            AreaLevel::Global => "global",
            AreaLevel::Off => "off",
            AreaLevel::Read => "read",
            AreaLevel::Control => "control",
        }
    }

    /// Tolerant: Unbekanntes ergibt „Aus“ (der sichere Fall), nie einen Lesefehler.
    pub fn parse(s: &str) -> AreaLevel {
        match s.trim().to_ascii_lowercase().as_str() {
            "global" | "as_global" => AreaLevel::Global,
            "read" => AreaLevel::Read,
            "control" => AreaLevel::Control,
            _ => AreaLevel::Off,
        }
    }

    /// Wirksame Stufe unter der Obergrenze `global`.
    pub fn effective(self, global: McpLevel) -> McpLevel {
        match self {
            AreaLevel::Global => global,
            AreaLevel::Off => McpLevel::Off,
            AreaLevel::Read => McpLevel::Read.min(global),
            AreaLevel::Control => McpLevel::Control.min(global),
        }
    }

    pub fn label(self, lang: crate::state::Lang) -> &'static str {
        match self {
            AreaLevel::Global => lang.t("Wie global", "As global"),
            AreaLevel::Off => lang.t("Aus", "Off"),
            AreaLevel::Read => lang.t("Nur lesen", "Read only"),
            AreaLevel::Control => lang.t("Steuern", "Control"),
        }
    }
}

impl Serialize for AreaLevel {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for AreaLevel {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(v.as_str().map(AreaLevel::parse).unwrap_or(AreaLevel::Off))
    }
}

/// Zugriff eines Bereichs (`state.json` je Projekt `agent_access`, `settings.json` `agent_general`, Spalten der Ticket-Projekte).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentAccess {
    pub level: AreaLevel,
    /// „Für alle“: auch Agenten anderer Projekte und externe Agenten bekommen diese Stufe.
    pub for_all: bool,
}

/// Fehlt der Eintrag (Daten von vor 3.2.0): wie global und für alle – es ändert sich nichts.
impl Default for AgentAccess {
    fn default() -> Self {
        AgentAccess::AS_BEFORE
    }
}

impl AgentAccess {
    /// Stand nach dem Update (und für das erste Projekt, TODO und „Allgemein“).
    pub const AS_BEFORE: AgentAccess = AgentAccess { level: AreaLevel::Global, for_all: true };
    /// Neue, vom Nutzer angelegte oder importierte Projekte.
    pub const LOCKED: AgentAccess = AgentAccess { level: AreaLevel::Off, for_all: false };
}

/// Wer ruft auf: ein Agent in einer Sitzung eines Seitenleisten-Projekts (Heimatprojekt) oder ein externer Agent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Caller {
    Home(u64),
    External,
}

/// Zugriff eines Ticket-Projekts (aus der Datenbank).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TicketAccess {
    /// Verknüpftes Seitenleisten-Projekt: dann gilt dessen Zugriff.
    pub link: Option<u64>,
    pub own: AgentAccess,
    /// System-Projekt TODO (einfache Todo-Liste).
    pub system: bool,
}

/// Momentaufnahme aller Zugriffe (wird je Aufruf bzw. höchstens einmal je Sekunde gebaut).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AccessMap {
    /// Obergrenze: globale Stufe (externe Agenten) bzw. Stufe des Modells (Chats).
    pub global: McpLevel,
    /// Seitenleisten-Projekte.
    pub projects: HashMap<u64, AgentAccess>,
    /// Bereich „Allgemein“.
    pub general: AgentAccess,
    /// Ticket-Projekte (Kennung der Datenbank).
    pub tickets: HashMap<i64, TicketAccess>,
}

/// Wo etwas liegt.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Area {
    /// Seitenleisten-Projekt.
    Project(u64),
    /// Ticket-Projekt.
    Tickets(i64),
    General,
}

impl AccessMap {
    fn reach(&self, a: AgentAccess, home: bool) -> McpLevel {
        if home || a.for_all { a.level.effective(self.global) } else { McpLevel::Off }
    }

    /// Stufe für ein Seitenleisten-Projekt. Ein unbekanntes Projekt ist gesperrt.
    pub fn sidebar(&self, who: Caller, p: u64) -> McpLevel {
        let Some(a) = self.projects.get(&p) else { return McpLevel::Off };
        self.reach(*a, who == Caller::Home(p))
    }

    /// Stufe für „Allgemein“: Agenten in einer Sitzung; externe nur mit „Für alle“.
    pub fn general(&self, who: Caller) -> McpLevel {
        self.reach(self.general, matches!(who, Caller::Home(_)))
    }

    /// Stufe für ein Ticket-Projekt: verknüpft = wie das Seitenleisten-Projekt, sonst eigene Stufe mit der Regel von „Allgemein“.
    pub fn ticket_project(&self, who: Caller, id: i64) -> McpLevel {
        match self.tickets.get(&id) {
            None => McpLevel::Off,
            // Verknüpft mit einem unbekannten (gelöschten) Seitenleisten-Projekt: gesperrt, nie die eigene Stufe.
            Some(t) => match t.link {
                Some(p) => self.sidebar(who, p),
                None => self.reach(t.own, matches!(who, Caller::Home(_))),
            },
        }
    }

    /// Stufe des System-Projekts TODO (gilt zusätzlich für jede Todo-Karte); ohne System-Projekt keine Grenze.
    fn todo_gate(&self, who: Caller) -> McpLevel {
        match self.tickets.iter().find(|(_, t)| t.system) {
            Some((id, _)) => self.ticket_project(who, *id),
            None => self.global,
        }
    }

    /// Stufe für eine Todo-Karte im Projekt `project` (`None` = „Allgemein“; ein unbekanntes Projekt ist gesperrt).
    pub fn card(&self, who: Caller, project: Option<u64>) -> McpLevel {
        let area = match project {
            Some(p) => self.sidebar(who, p),
            None => self.general(who),
        };
        area.min(self.todo_gate(who))
    }

    pub fn area(&self, who: Caller, a: Area) -> McpLevel {
        match a {
            Area::Project(p) => self.sidebar(who, p),
            Area::Tickets(t) => self.ticket_project(who, t),
            Area::General => self.general(who),
        }
    }

    /// Bekommen Agenten mehr Zugriff, wenn Sitzungen (und Karten, Prozesse) von `from` nach `to` ziehen? Für die Agenten der
    /// verschobenen Sitzungen (Heimatprojekt vorher/nachher) und für alle übrigen („Für alle“).
    pub fn move_gains(&self, from: u64, to: u64) -> bool {
        self.sidebar(Caller::Home(to), to) > self.sidebar(Caller::Home(from), from) || self.sidebar(Caller::External, to) > self.sidebar(Caller::External, from)
    }

    /// Höchste Stufe, die irgendein Agent irgendwo hat (für `tools/list`, das der Hub ohne Aufrufer beantwortet).
    pub fn highest(&self) -> McpLevel {
        let mut best = self.general.level.effective(self.global);
        for a in self.projects.values() {
            best = best.max(a.level.effective(self.global));
        }
        for t in self.tickets.values() {
            if t.link.is_none() {
                best = best.max(t.own.level.effective(self.global));
            }
        }
        best
    }

    /// Ist für diesen Agenten irgendein Bereich gesperrt (dann kein Screenshot des ganzen Fensters)?
    pub fn any_locked(&self, who: Caller) -> bool {
        self.general(who) == McpLevel::Off
            || self.projects.keys().any(|p| self.sidebar(who, *p) == McpLevel::Off)
            || self.tickets.keys().any(|t| self.ticket_project(who, *t) == McpLevel::Off)
    }

    /// Die Sicht eines Agenten für die Todo-/Ticket-Werkzeuge.
    pub fn view(&self, who: Caller, may_create_projects: bool, may_delete: bool) -> AgentView {
        let projects = self.tickets.keys().map(|id| (*id, self.ticket_project(who, *id))).filter(|(_, l)| *l > McpLevel::Off).collect();
        let cards = self.projects.keys().map(|p| (*p, self.card(who, Some(*p)))).collect();
        AgentView {
            projects,
            cards,
            card_general: self.card(who, None),
            general: self.general(who),
            home: match who {
                Caller::Home(p) => Some(p),
                Caller::External => None,
            },
            home_level: match who {
                Caller::Home(p) => self.sidebar(who, p),
                Caller::External => McpLevel::Off,
            },
            may_create_projects,
            may_delete,
            sessions: None,
        }
    }
}

/// Was ein Agent von der Todo-Liste und den Tickets sieht (an `planning::agent::Ctx` übergeben). Private Einträge sind
/// immer unsichtbar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentView {
    /// Erreichbare Ticket-Projekte mit Stufe (Lesen/Steuern); fehlt = unsichtbar.
    pub projects: HashMap<i64, McpLevel>,
    /// Stufe für Karten je Seitenleisten-Projekt (fehlt = wie „Allgemein“).
    pub cards: HashMap<u64, McpLevel>,
    /// Stufe für Karten ohne Projekt.
    pub card_general: McpLevel,
    /// Stufe im Bereich „Allgemein“ (Termine ohne Projekt, Wecker …).
    pub general: McpLevel,
    /// Heimatprojekt (Seitenleiste) des Agenten; `None` = externer Agent.
    pub home: Option<u64>,
    /// Stufe im Heimatprojekt.
    pub home_level: McpLevel,
    /// Schalter „Projekte anlegen“.
    pub may_create_projects: bool,
    /// Schalter „Löschen“ (in den Papierkorb).
    pub may_delete: bool,
    /// Sitzungen, die der Agent sieht (Review 3.2.0: Terminal-Verknüpfungen nennen nur diese); `None` = keine Grenze
    /// (Tests, interne Aufrufe). Die App setzt sie immer.
    pub sessions: Option<std::collections::HashSet<u64>>,
}

impl AgentView {
    /// Darf der Agent eine Terminal-Verknüpfung (`session:<id>`) sehen? Nur sichtbare Sitzungen; ein Ziel, das keine
    /// Sitzung nennt, nur ohne Grenze.
    pub fn terminal_visible(&self, session: Option<u64>) -> bool {
        match &self.sessions {
            None => true,
            Some(v) => session.is_some_and(|s| v.contains(&s)),
        }
    }

    pub fn ticket_level(&self, project: i64) -> McpLevel {
        self.projects.get(&project).copied().unwrap_or(McpLevel::Off)
    }

    pub fn card_level(&self, project: Option<u64>) -> McpLevel {
        // Unbekanntes (z. B. gelöschtes) Projekt: gesperrt (so auch `AccessMap::card`).
        match project {
            Some(p) => self.cards.get(&p).copied().unwrap_or(McpLevel::Off),
            None => self.card_general,
        }
    }

    /// Sichtbare Ticket-Projekte, aufsteigend (für SQL-Filter).
    pub fn project_ids(&self) -> Vec<i64> {
        let mut v: Vec<i64> = self.projects.keys().copied().collect();
        v.sort_unstable();
        v
    }
}

/// Prüft die Stufe `have` gegen den Bedarf `need`: gesperrt (`Off`) → `project_locked`, zu niedrig → `forbidden`.
pub fn gate(have: McpLevel, need: McpLevel) -> Result<(), (String, String)> {
    if have == McpLevel::Off {
        return Err(locked_error());
    }
    if have < need {
        return Err(too_low_error());
    }
    Ok(())
}

/// Fehlercode für einen gesperrten Bereich. Nennt nie das Projekt (es ist für den Agenten unsichtbar).
pub const LOCKED_CODE: &str = "project_locked";

pub fn locked_error() -> (String, String) {
    (
        LOCKED_CODE.to_string(),
        crate::state::t(
            "Dieses Projekt ist für Agenten gesperrt (Einstellungen → Agenten → Zugriff → Zugriff je Projekt). Nur der Nutzer kann es freigeben.",
            "This project is locked for agents (Settings → Agents → Access → Access per project). Only the user can release it.",
        )
        .to_string(),
    )
}

pub fn too_low_error() -> (String, String) {
    (
        "forbidden".to_string(),
        crate::state::t(
            "Die Stufe dieses Projekts erlaubt das nicht (nur lesen) – der Nutzer kann sie in Paneldeck ändern (Einstellungen → Agenten → Zugriff).",
            "The level of this project does not allow this (read only) – the user can change it in Paneldeck (Settings → Agents → Access).",
        )
        .to_string(),
    )
}

/// Schalter der Einstellungen (#386) für die Chats, je Bild vom UI-Thread veröffentlicht (wie die Editor-Schalter):
/// Bit [`SW_CREATE`] = „Projekte anlegen“, Bit [`SW_DELETE`] = „… Tickets/Kommentare löschen“.
pub static AGENT_SWITCHES: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
pub const SW_CREATE: u8 = 1;
pub const SW_DELETE: u8 = 2;

pub fn switch_bits(create_projects: bool, delete: bool) -> u8 {
    (if create_projects { SW_CREATE } else { 0 }) | if delete { SW_DELETE } else { 0 }
}

/// Darf ein Chat dieses Werkzeug aufrufen? Wie bei externen Agenten: `create_project` und die Papierkorb-Werkzeuge nur mit
/// ihrem Schalter, `update_comment` mit Steuern allein. Für Chats sind alle vier außerdem standardmäßig aus und je Modell
/// einzuschalten (`chat::tools::CHAT_DEFAULT_OFF`, #393).
pub fn allowed_for_chats(name: &str, bits: u8) -> bool {
    name == "update_comment" || offered_to_chats(name, bits)
}

/// Bekommt ein Chat dieses Werkzeug angeboten? Die Werkzeuge der Erweiterung nur mit ihrem Schalter (ohne ihn lieferten
/// sie immer `forbidden`); `update_comment` ist auch ohne Schalter erlaubt ([`allowed_for_chats`]) und wird darum immer
/// angeboten (seit dem Nachladen von Werkzeugen, #393). Das Häkchen beim Modell (`chat::tools::CHAT_DEFAULT_OFF`) gilt
/// zusätzlich.
pub fn offered_to_chats(name: &str, bits: u8) -> bool {
    match name {
        "create_project" => bits & SW_CREATE != 0,
        "delete_ticket" | "delete_comment" => bits & SW_DELETE != 0,
        _ => true,
    }
}

/// Wohin ein MCP-Werkzeug greift, soweit der Hub es ohne Datenbank bestimmen kann.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToolScope {
    /// Ziel ist die Sitzung `session` (Projekt der Sitzung).
    Session,
    /// Bereich „Allgemein“ (Wecker, Editor, Themes, Modelle, Verlauf …).
    General,
    /// Filtert selbst (Listen, Todo, Tickets, Kalender, Prozesse, `create_session`, `screenshot` …).
    Own,
}

/// Werkzeuge, die ihre Ziele selbst prüfen und Listen selbst filtern ([`ToolScope::Own`]): ausdrücklich aufgezählt –
/// ein neues, nicht eingeteiltes Werkzeug gilt als „Allgemein“ (sicherer Fall, nie „prüft selbst“).
const OWN_TOOLS: &[&str] = &["whoami", "list_sessions", "screenshot", "create_session", "list_processes", "read_process_output", "start_process", "stop_process", "restart_process"];

/// Werkzeuge mit einer Ziel-Sitzung `session` ([`ToolScope::Session`]).
const SESSION_TOOLS: &[&str] = &["read_terminal", "send_text", "send_keys", "run_command", "wait_for_status", "queue_prompt", "rename_session", "focus_session", "close_session", "read_chat", "send_chat_message"];

/// Einteilung der Werkzeuge (für die Prüfung vor der Ausführung, siehe `App::mcp_access_gate`). Alles, was weder eine
/// Ziel-Sitzung hat noch in [`OWN_TOOLS`], bei den Todo-/Ticket- oder den Kalender-Werkzeugen steht, gehört zu
/// „Allgemein“ (Editor, Zeit, Themes, Modelle, Verlauf, Aktionen beim Speichern, `get_usage` ohne Sitzung – und jedes
/// künftige Werkzeug, bis es ausdrücklich eingeteilt ist).
pub fn tool_scope(tool: &str, args: &Value) -> ToolScope {
    let has_session = args.get("session").is_some_and(|s| !s.is_null());
    match tool {
        t if SESSION_TOOLS.contains(&t) => ToolScope::Session,
        "get_usage" if has_session => ToolScope::Session,
        t if OWN_TOOLS.contains(&t) || crate::planning::agent::TOOLS.contains(&t) || crate::calendar::agent::TOOLS.contains(&t) => ToolScope::Own,
        _ => ToolScope::General,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use McpLevel::{Control, Off, Read};

    fn acc(level: AreaLevel, for_all: bool) -> AgentAccess {
        AgentAccess { level, for_all }
    }

    fn map(global: McpLevel) -> AccessMap {
        let mut m = AccessMap { global, general: AgentAccess::AS_BEFORE, ..Default::default() };
        m.projects.insert(1, AgentAccess::AS_BEFORE); // erstes Projekt nach dem Update
        m.projects.insert(2, AgentAccess::LOCKED); // neu angelegt
        m.projects.insert(3, acc(AreaLevel::Read, false));
        m.projects.insert(4, acc(AreaLevel::Control, true));
        m.tickets.insert(10, TicketAccess { link: None, own: AgentAccess::AS_BEFORE, system: true }); // TODO
        m.tickets.insert(11, TicketAccess { link: Some(3), own: AgentAccess::LOCKED, system: false });
        m.tickets.insert(12, TicketAccess { link: None, own: AgentAccess::LOCKED, system: false });
        m.tickets.insert(13, TicketAccess { link: Some(99), own: acc(AreaLevel::Read, true), system: false }); // Verknüpfung ins Leere
        m
    }

    #[test]
    fn the_global_level_is_the_upper_bound() {
        assert_eq!(AreaLevel::Control.effective(Read), Read);
        assert_eq!(AreaLevel::Read.effective(Control), Read);
        assert_eq!(AreaLevel::Global.effective(Read), Read);
        for l in AreaLevel::ALL {
            assert_eq!(l.effective(Off), Off, "global Aus schaltet alles aus");
        }
        let m = map(Off);
        assert_eq!(m.highest(), Off);
        assert_eq!(m.sidebar(Caller::Home(4), 4), Off);
        let m = map(Read);
        assert_eq!(m.sidebar(Caller::Home(4), 4), Read);
        assert_eq!(m.highest(), Read);
        assert_eq!(map(Control).highest(), Control);
    }

    #[test]
    fn as_global_and_for_all_keep_the_old_behaviour() {
        let m = map(Control);
        for who in [Caller::Home(1), Caller::Home(2), Caller::External] {
            assert_eq!(m.sidebar(who, 1), Control, "{who:?}");
            assert_eq!(m.general(who), Control, "{who:?}");
            assert_eq!(m.ticket_project(who, 10), Control);
        }
        // fehlende Einträge in alten Dateien = so wie vorher
        let old: AgentAccess = serde_json::from_str("{}").unwrap();
        assert_eq!(old, AgentAccess::AS_BEFORE);
    }

    #[test]
    fn home_project_other_projects_and_external_agents() {
        let m = map(Control);
        // gesperrt: niemand, auch nicht von zu Hause
        assert_eq!(m.sidebar(Caller::Home(2), 2), Off);
        assert_eq!(m.sidebar(Caller::External, 2), Off);
        // nur lesen, nicht für alle: nur im eigenen Projekt
        assert_eq!(m.sidebar(Caller::Home(3), 3), Read);
        assert_eq!(m.sidebar(Caller::Home(1), 3), Off);
        assert_eq!(m.sidebar(Caller::External, 3), Off);
        // für alle: jeder
        assert_eq!(m.sidebar(Caller::Home(3), 4), Control);
        assert_eq!(m.sidebar(Caller::External, 4), Control);
        // unbekanntes Projekt
        assert_eq!(m.sidebar(Caller::Home(77), 77), Off);
    }

    #[test]
    fn general_and_unlinked_ticket_projects_need_for_all_for_external_agents() {
        let mut m = map(Control);
        m.general = acc(AreaLevel::Read, false);
        assert_eq!(m.general(Caller::Home(2)), Read, "jede Sitzung");
        assert_eq!(m.general(Caller::External), Off);
        m.general.for_all = true;
        assert_eq!(m.general(Caller::External), Read);
        m.tickets.insert(14, TicketAccess { link: None, own: acc(AreaLevel::Control, false), system: false });
        assert_eq!(m.ticket_project(Caller::Home(3), 14), Control);
        assert_eq!(m.ticket_project(Caller::External, 14), Off);
        assert_eq!(m.ticket_project(Caller::Home(1), 12), Off, "unverknüpft und gesperrt");
        assert_eq!(m.ticket_project(Caller::Home(1), 999), Off, "unbekannt");
    }

    #[test]
    fn linked_ticket_projects_follow_their_sidebar_project() {
        let m = map(Control);
        assert_eq!(m.ticket_project(Caller::Home(3), 11), Read, "eigene Stufe (Aus) zählt nicht");
        assert_eq!(m.ticket_project(Caller::Home(1), 11), Off);
        assert_eq!(m.ticket_project(Caller::External, 11), Off);
        // Verknüpfung auf ein gelöschtes Projekt: gesperrt (nie die eigene Stufe)
        assert_eq!(m.ticket_project(Caller::External, 13), Off);
        assert_eq!(m.ticket_project(Caller::Home(99), 13), Off);
    }

    #[test]
    fn cards_follow_their_area_and_the_todo_project() {
        let mut m = map(Control);
        assert_eq!(m.card(Caller::Home(1), None), Control);
        assert_eq!(m.card(Caller::Home(1), Some(2)), Off);
        assert_eq!(m.card(Caller::Home(3), Some(3)), Read);
        assert_eq!(m.card(Caller::Home(3), Some(42)), Off, "unbekanntes Projekt: gesperrt, nicht Allgemein");
        m.tickets.get_mut(&10).unwrap().own = acc(AreaLevel::Read, true);
        assert_eq!(m.card(Caller::Home(1), None), Read, "TODO begrenzt jede Karte");
        m.tickets.get_mut(&10).unwrap().own = AgentAccess::LOCKED;
        assert_eq!(m.card(Caller::Home(4), Some(4)), Off);
    }

    #[test]
    fn views_list_only_reachable_projects() {
        let m = map(Control);
        let v = m.view(Caller::Home(3), false, false);
        assert_eq!(v.ticket_level(11), Read);
        assert_eq!(v.ticket_level(12), Off);
        assert!(!v.project_ids().contains(&12));
        assert_eq!(v.home, Some(3));
        assert_eq!(v.home_level, Read);
        assert_eq!(v.card_level(Some(2)), Off);
        assert_eq!(v.card_level(Some(3)), Read);
        assert_eq!(v.card_level(None), Control);
        assert_eq!(v.card_level(Some(4242)), Off, "unbekanntes Projekt");
        let e = m.view(Caller::External, true, true);
        assert_eq!((e.home, e.home_level), (None, Off));
        assert!(e.may_create_projects && e.may_delete);
        // Terminal-Verknüpfungen (Review 3.2.0): ohne Grenze alle, mit Grenze nur sichtbare Sitzungen, nie unbekannte Ziele.
        assert!(e.terminal_visible(Some(9)) && e.terminal_visible(None));
        let limited = AgentView { sessions: Some([3, 4].into_iter().collect()), ..e };
        assert!(limited.terminal_visible(Some(3)) && !limited.terminal_visible(Some(9)) && !limited.terminal_visible(None));
    }

    #[test]
    fn locked_areas_and_the_gate() {
        let m = map(Control);
        assert!(m.any_locked(Caller::Home(1)));
        let mut open = map(Control);
        open.projects.retain(|p, _| *p == 1 || *p == 4);
        open.tickets.retain(|t, _| *t == 10);
        assert!(!open.any_locked(Caller::External));
        assert_eq!(gate(Off, Read).unwrap_err().0, LOCKED_CODE);
        assert_eq!(gate(Read, Control).unwrap_err().0, "forbidden");
        assert!(gate(Read, Read).is_ok() && gate(Control, Read).is_ok());
        // die Meldung nennt kein Projekt
        assert!(!locked_error().1.contains('“'));
    }

    #[test]
    fn levels_are_read_tolerantly() {
        let a: AgentAccess = serde_json::from_str(r#"{"level":"READ","for_all":false}"#).unwrap();
        assert_eq!(a, acc(AreaLevel::Read, false));
        let a: AgentAccess = serde_json::from_str(r#"{"level":"nonsense"}"#).unwrap();
        assert_eq!(a.level, AreaLevel::Off, "Unbekanntes = Aus");
        let a: AgentAccess = serde_json::from_str(r#"{"level":3}"#).unwrap();
        assert_eq!(a.level, AreaLevel::Off);
        assert_eq!(serde_json::to_string(&AgentAccess::LOCKED).unwrap(), r#"{"level":"off","for_all":false}"#);
        for l in AreaLevel::ALL {
            assert_eq!(AreaLevel::parse(l.as_str()), l);
        }
    }

    #[test]
    fn moving_into_a_project_with_more_access_is_noticed() {
        let m = map(Control);
        assert!(m.move_gains(2, 3), "gesperrt → nur lesen im eigenen Projekt (ohne „Für alle“)");
        assert!(m.move_gains(3, 4), "nur lesen → Steuern für alle");
        assert!(!m.move_gains(4, 3) && !m.move_gains(3, 2), "weniger Zugriff: kein Hinweis");
        assert!(!m.move_gains(1, 1));
    }

    #[test]
    fn chats_get_the_extension_tools_only_with_their_switch() {
        for t in ["create_project", "delete_ticket", "delete_comment"] {
            assert!(!offered_to_chats(t, 0), "{t}");
            assert!(offered_to_chats(t, SW_CREATE | SW_DELETE), "{t}");
        }
        // `update_comment` braucht keinen Schalter und wird darum immer angeboten (#393); für alle vier gilt im Chat
        // zusätzlich das Häkchen beim Modell (`chat::tools::CHAT_DEFAULT_OFF`).
        assert!(offered_to_chats("update_comment", 0) && allowed_for_chats("update_comment", 0));
        for t in ["create_project", "delete_ticket", "update_comment", "delete_comment"] {
            assert!(crate::chat::tools::default_off(t), "{t}");
        }
        assert!(offered_to_chats("create_project", SW_CREATE) && !offered_to_chats("delete_ticket", SW_CREATE));
        assert!(offered_to_chats("delete_ticket", SW_DELETE) && !offered_to_chats("create_project", SW_DELETE));
        assert!(offered_to_chats("list_sessions", 0));
        assert!(allowed_for_chats("update_comment", 0), "wie bei externen Agenten: Steuern genügt");
        assert!(!allowed_for_chats("delete_comment", 0) && !allowed_for_chats("create_project", 0));
        assert_eq!(switch_bits(true, true), 3);
    }

    #[test]
    fn tool_scopes() {
        use serde_json::json;
        assert_eq!(tool_scope("read_terminal", &json!({"session": 1})), ToolScope::Session);
        assert_eq!(tool_scope("get_usage", &json!({"session": 1})), ToolScope::Session);
        assert_eq!(tool_scope("get_usage", &json!({})), ToolScope::General);
        assert_eq!(tool_scope("search_history", &json!({})), ToolScope::General);
        assert_eq!(tool_scope("list_alarms", &json!({})), ToolScope::General);
        assert_eq!(tool_scope("editor_list", &json!({})), ToolScope::General);
        assert_eq!(tool_scope("format_file", &json!({"path": "C:\\a.json"})), ToolScope::General);
        assert_eq!(tool_scope("editor_diff", &json!({})), ToolScope::General);
        assert_eq!(tool_scope("list_save_actions", &json!({})), ToolScope::General);
        assert_eq!(tool_scope("list_sessions", &json!({})), ToolScope::Own);
        assert_eq!(tool_scope("screenshot", &json!({})), ToolScope::Own);
        assert_eq!(tool_scope("search_tickets", &json!({})), ToolScope::Own);
        assert_eq!(tool_scope("list_calendar_events", &json!({})), ToolScope::Own);
        assert_eq!(tool_scope("start_process", &json!({})), ToolScope::Own);
    }

    /// Review 3.2.0: jedes Werkzeug ist ausdrücklich eingeteilt; ein unbekanntes (künftiges, vergessenes) Werkzeug gilt
    /// als „Allgemein“ – nie als „prüft selbst“ (das ließe es ohne Prüfung durch).
    #[test]
    fn every_tool_is_classified_and_unknown_tools_fall_into_general() {
        use serde_json::json;
        let general: &[&str] = &["get_usage", "search_history", "list_models", "cleanup_sizes", "theme_list", "theme_get", "list_save_actions"];
        for def in crate::mcp::tools() {
            let n = def.name;
            let explicit = SESSION_TOOLS.contains(&n)
                || OWN_TOOLS.contains(&n)
                || crate::planning::agent::TOOLS.contains(&n)
                || crate::calendar::agent::TOOLS.contains(&n)
                || general.contains(&n)
                || crate::editor_svc::tools::TOOLS.contains(&n)
                || crate::time::agent::TOOLS.contains(&n);
            assert!(explicit, "{n}: nicht eingeteilt");
            let scope = tool_scope(n, &json!({}));
            if general.contains(&n) || crate::editor_svc::tools::TOOLS.contains(&n) || crate::time::agent::TOOLS.contains(&n) {
                assert_eq!(scope, ToolScope::General, "{n}");
            }
        }
        for unknown in ["new_tool_2027", "delete_everything", ""] {
            assert_eq!(tool_scope(unknown, &json!({})), ToolScope::General, "{unknown}");
        }
    }
}
