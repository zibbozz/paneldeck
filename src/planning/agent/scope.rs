//! Sicht des Agenten (#386) und die Werkzeuge der Erweiterung: Ticket-Projekte anlegen, Tickets und eigene Kommentare in
//! den Papierkorb legen, eigene Kommentare ändern.
//!
//! * Sicht: [`Ctx::view`] (`access::AgentView`). Ohne Sicht (Tests, interne Aufrufe) ist alles erreichbar.
//! * `create_project` (Schalter „Agenten dürfen Projekte anlegen“): nur Ticket-Projekte, verknüpft mit dem Heimatprojekt
//!   des Agenten (Steuern nötig), Herkunft „Agent“. Seitenleisten-Projekte legt nur der Nutzer an, Projekte löscht nie ein Agent.
//! * `delete_ticket`/`delete_comment` (Schalter „Agenten dürfen Tickets/Kommentare löschen“): nur in den Papierkorb;
//!   endgültig löscht nur der Nutzer. Kommentare nur mit Herkunft Agent (`update_comment` ohne Schalter, Steuern).

use super::*;
use crate::access::AgentView;
use crate::mcp::McpLevel;
use crate::planning::access::{self as acc, Origin};

fn gate_err(ctx: &Ctx, e: (String, String)) -> ToolError {
    let code = if e.0 == crate::access::LOCKED_CODE { crate::access::LOCKED_CODE } else { "forbidden" };
    let _ = ctx;
    ToolError { code, message: e.1, details: None }
}

impl Ctx<'_> {
    /// Stufe des Agenten im Ticket-Projekt (ohne Sicht: Steuern).
    pub(super) fn level_in(&self, project: i64) -> McpLevel {
        self.view.map(|v| v.ticket_level(project)).unwrap_or(McpLevel::Control)
    }

    pub(super) fn project_visible(&self, project: i64) -> bool {
        self.level_in(project) > McpLevel::Off
    }

    /// Projekt erreichbar mit mindestens `need`? Gesperrt = `project_locked`, zu niedrig = `forbidden`.
    pub(super) fn check_project(&self, project: i64, need: McpLevel) -> Result<(), ToolError> {
        crate::access::gate(self.level_in(project), need).map_err(|e| gate_err(self, e))
    }

    /// Sieht der Agent dieses Ticket (Projekt erreichbar, nicht privat, nicht im Papierkorb)?
    pub(super) fn ticket_visible(&self, t: &Ticket) -> bool {
        self.view.is_none() || (!t.private && !t.trashed && self.project_visible(t.project_id))
    }
}

/// Verknüpfung sichtbar? Ticket-Ziele nur, wenn der Agent das Ziel sieht.
pub(super) fn link_visible(ctx: &Ctx, l: &super::super::links::LinkView) -> bool {
    match &l.target {
        LinkTarget::Ticket(r) => ctx.view.is_none() || get_ticket(ctx.conn, r.id).is_ok_and(|t| ctx.ticket_visible(&t)),
        // Terminal-Verknüpfungen (Review 3.2.0): nur Sitzungen, die der Agent sieht (nie die eines gesperrten Projekts).
        LinkTarget::Terminal(s) => terminal_visible(ctx, s),
        _ => true,
    }
}

/// Sieht der Agent die Sitzung dieser Terminal-Verknüpfung (`session:<id>`)? Ohne Sicht: alles.
pub(super) fn terminal_visible(ctx: &Ctx, target: &str) -> bool {
    ctx.view.is_none_or(|v| v.terminal_visible(super::parse_terminal_ref(target)))
}

/// Satz eines Verlaufseintrags für einen Agenten (Review 3.2.0): Kommentar-Einträge ohne den Text (der Kommentar kann
/// inzwischen im Papierkorb liegen; seinen Text liefern nur `comments`, die den Papierkorb auslassen). Ohne Sicht wie in
/// der Oberfläche.
pub(super) fn history_change(ctx: &Ctx, e: &super::super::history::HistoryEntry) -> String {
    use super::super::history::HistoryField as H;
    if ctx.view.is_some() && e.field == H::Comment {
        let l = ctx.lang;
        return match (&e.old, &e.new) {
            (None, Some(_)) => l.t("Kommentar hinzugefügt", "Comment added"),
            (Some(_), None) => l.t("Kommentar gelöscht", "Comment deleted"),
            _ => l.t("Kommentar geändert", "Comment edited"),
        }
        .to_string();
    }
    super::super::history::describe(ctx.conn, e, ctx.lang)
}

fn id_visible(ctx: &Ctx, s: Option<&str>) -> bool {
    match s.and_then(|x| x.trim().parse::<i64>().ok()) {
        Some(id) => get_ticket(ctx.conn, id).is_ok_and(|t| ctx.ticket_visible(&t)),
        None => true,
    }
}

/// Ticket über den Schlüssel sichtbar? Gibt es das Ticket nicht mehr, entscheidet das Projekt des Schlüssels.
fn key_visible(ctx: &Ctx, key: &str) -> bool {
    match ticket_by_key(ctx.conn, key) {
        Ok(t) => ctx.ticket_visible(&t),
        Err(_) => match key.rsplit_once('-').and_then(|(p, _)| project_by_key(ctx.conn, p).ok()) {
            Some(p) => ctx.project_visible(p.id),
            None => true,
        },
    }
}

/// Verlaufseintrag sichtbar? Einträge, die ein unsichtbares Ticket nennen (Verknüpfung, Eltern, Ticket-Feld), fehlen.
pub(super) fn history_visible(ctx: &Ctx, e: &super::super::history::HistoryEntry) -> bool {
    use super::super::history::HistoryField as H;
    if ctx.view.is_none() {
        return true;
    }
    match &e.field {
        // Verknüpfungen nennen das Ziel mit seinem Schlüssel (`PAN-3`), bei gelöschten Zielen die Kennung.
        H::Link => [&e.old, &e.new].iter().all(|v| {
            let Some(j) = v.as_deref().and_then(|s| serde_json::from_str::<Value>(s).ok()) else { return true };
            // Terminal-Verknüpfungen (Review 3.2.0): nur Sitzungen, die der Agent sieht.
            if j["kind"] == "terminal" {
                return j["target"].as_str().is_some_and(|t| terminal_visible(ctx, t));
            }
            if j["kind"] != "ticket" {
                return true;
            }
            match j["target"].as_str() {
                Some(k) if k.contains('-') => key_visible(ctx, k),
                Some(k) => id_visible(ctx, Some(k)),
                None => id_visible(ctx, j["target"].as_i64().map(|i| i.to_string()).as_deref()),
            }
        }),
        H::Parent => id_visible(ctx, e.old.as_deref()) && id_visible(ctx, e.new.as_deref()),
        H::Custom(fid) => match super::super::fields::get_field(ctx.conn, *fid) {
            Ok(f) if f.kind == FieldKind::Ticket => id_visible(ctx, e.old.as_deref()) && id_visible(ctx, e.new.as_deref()),
            _ => true,
        },
        // Privat-Schalter und Papierkorb sind Sache des Nutzers: Agenten sehen diese Einträge nicht.
        H::Other(s) if s == "private" => false,
        _ => true,
    }
}

/// Karte sichtbar (Bereich erreichbar, nicht privat – auch nicht über einen Tag)?
pub(super) fn card_visible(ctx: &Ctx, it: &tasks::Item, tags: &[tasks::Tag]) -> bool {
    match ctx.view {
        None => true,
        Some(v) => !tasks::is_private(it, tags) && v.card_level(it.project) > McpLevel::Off,
    }
}

fn card_level(v: &AgentView, it: &tasks::Item) -> McpLevel {
    v.card_level(it.project)
}

/// Prüft eine Karte für ein schreibendes Werkzeug: privat/unbekannt = `not_found`, Bereich gesperrt = `project_locked`.
pub(super) fn check_card(ctx: &Ctx, id: i64) -> Result<(), ToolError> {
    let Some(v) = ctx.view else { return Ok(()) };
    let not_found = || ctx.err("not_found", format!("Die Karte TODO-{id} gibt es nicht"), format!("There is no card TODO-{id}"));
    let it = tasks::get_item(ctx.conn, id).map_err(|_| not_found())?;
    let tags = tasks::list_tags(ctx.conn).map_err(|e| task_err(ctx.lang, e))?;
    if tasks::is_private(&it, &tags) {
        return Err(not_found());
    }
    crate::access::gate(card_level(v, &it), ctx.need.get()).map_err(|e| gate_err(ctx, e))
}

/// Projekt, Herkunft und Prüfung für eine neue Karte eines Agenten: Heimatprojekt (externe Agenten: „Allgemein“).
pub(super) fn new_card_meta(ctx: &Ctx) -> Result<tasks::ItemMeta, ToolError> {
    let meta = tasks::ItemMeta { project: ctx.view.and_then(|v| v.home), private: false, origin: Origin::of(&ctx.source), origin_detail: acc::origin_detail(&ctx.source) };
    if let Some(v) = ctx.view {
        crate::access::gate(v.card_level(meta.project), McpLevel::Control).map_err(|e| gate_err(ctx, e))?;
    }
    Ok(meta)
}

// ------------------------------------------------------------------ neue Werkzeuge

fn switch_off(ctx: &Ctx, de: &str, en: &str) -> ToolError {
    ctx.err("forbidden", format!("{de} (Einstellungen → Agenten; nur der Nutzer kann das einschalten)."), format!("{en} (Settings → Agents; only the user can switch it on)."))
}

/// `create_project`: neues Ticket-Projekt, verknüpft mit dem Heimatprojekt des Agenten.
pub(super) fn create_project_tool(ctx: &Ctx, v: &Value) -> R {
    let a = args(ctx, v, &["key", "name", "description", "color"])?;
    if let Some(view) = ctx.view {
        if !view.may_create_projects {
            return Err(switch_off(ctx, "Agenten dürfen keine Projekte anlegen", "Agents may not create projects"));
        }
        if view.home.is_none() {
            return Err(ctx.err(
                "forbidden",
                "Projekte legen nur Agenten in einer Paneldeck-Sitzung an (das neue Projekt gehört zu deren Projekt).",
                "Only agents running in a Paneldeck session can create projects (the new project belongs to that session's project).",
            ));
        }
        crate::access::gate(view.home_level, McpLevel::Control).map_err(|e| gate_err(ctx, e))?;
    }
    let key = a.req_str("key", 10)?;
    let name = a.req_str("name", MAX_NAME_CHARS)?;
    let description = a.opt_str("description", 2_000)?.unwrap_or_default();
    let color = a.opt_str("color", 16)?.unwrap_or_else(|| "#5b8def".to_string());
    let mut np = super::super::projects::NewProject::new(&key, &name, &color);
    np.description = description;
    let id = pl(ctx, super::super::projects::create_project(ctx.conn, &np))?;
    // Verknüpft mit dem Heimatprojekt (folgt dessen Stufe); Herkunft Agent (Badge).
    let home = ctx.view.and_then(|v| v.home);
    tk(ctx, acc::set_project_access(ctx.conn, id, crate::access::AgentAccess::LOCKED, home))?;
    acc::set_project_origin(ctx.conn, id, Origin::of(&ctx.source), acc::origin_detail(&ctx.source).as_deref()).map_err(|e| ticket_err(ctx.lang, TicketError::from(e)))?;
    let p = pl(ctx, super::super::projects::get_project(ctx.conn, id))?;
    wrote(
        json!({"project": project_json(ctx, &p), "linked_to_your_project": home.is_some()}),
        ctx.tx(format!("Projekt {} angelegt: {}", p.key, quoted(ctx, &p.name)), format!("created project {}: {}", p.key, quoted(ctx, &p.name))),
        Some(p.key.clone()),
    )
}

fn deletes_allowed(ctx: &Ctx) -> Result<(), ToolError> {
    match ctx.view {
        Some(v) if !v.may_delete => Err(switch_off(ctx, "Agenten dürfen nichts löschen", "Agents may not delete anything")),
        _ => Ok(()),
    }
}

/// `delete_ticket`: in den Papierkorb (wiederherstellbar; endgültig löscht nur der Nutzer).
pub(super) fn delete_ticket_tool(ctx: &Ctx, v: &Value) -> R {
    let a = args(ctx, v, &["ticket"])?;
    deletes_allowed(ctx)?;
    let t = ticket_arg(ctx, &a, "ticket")?;
    if t.project_id == super::super::projects::list_projects(ctx.conn).ok().and_then(|ps| ps.into_iter().find(|p| p.system)).map(|p| p.id).unwrap_or(-1) {
        return Err(ctx.err("forbidden", "Das System-Projekt TODO ist schreibgeschützt.", "The system project TODO is read-only."));
    }
    tk(ctx, acc::trash_ticket(ctx.conn, t.id, &ctx.source))?;
    wrote(
        json!({"ticket": t.key(), "trashed": true, "note": ctx.tx("Im Papierkorb: Nur der Nutzer kann es wiederherstellen oder endgültig löschen.", "In the trash: only the user can restore it or delete it permanently.")}),
        ctx.tx(format!("{} in den Papierkorb gelegt", t.key()), format!("moved {} to the trash", t.key())),
        Some(t.key()),
    )
}

/// Kommentar für ein Agenten-Werkzeug: Ticket sichtbar mit Steuern, Herkunft Agent (sonst `forbidden`).
fn agent_comment(ctx: &Ctx, a: &Args) -> Result<(super::super::comments::Comment, Ticket), ToolError> {
    let id = a.opt_int("comment", 1, i64::MAX)?.ok_or_else(|| ctx.bad("comment (Kennung des Kommentars, siehe get_ticket) fehlt", "comment (the comment id, see get_ticket) is missing"))?;
    let not_found = || ctx.err("not_found", format!("Den Kommentar {id} gibt es nicht"), format!("There is no comment {id}"));
    let c = super::super::comments::get_comment(ctx.conn, id).map_err(|_| not_found())?;
    let t = get_ticket(ctx.conn, c.ticket_id).map_err(|_| not_found())?;
    let trashed: bool = ctx.conn.query_row("SELECT trashed_at IS NOT NULL FROM comments WHERE id = ?1", [id], |r| r.get(0)).unwrap_or(true);
    if trashed || (ctx.view.is_some() && (t.private || t.trashed)) {
        return Err(not_found());
    }
    ctx.check_project(t.project_id, McpLevel::Control)?;
    if c.origin != Origin::Agent {
        return Err(ctx.err("forbidden", "Kommentare des Nutzers kann ein Agent weder ändern noch löschen.", "An agent can neither edit nor delete the user's comments."));
    }
    Ok((c, t))
}

/// `update_comment`: Text eines Agenten-Kommentars ändern (steht im Verlauf).
pub(super) fn update_comment_tool(ctx: &Ctx, v: &Value) -> R {
    let a = args(ctx, v, &["comment", "body"])?;
    let (c, t) = agent_comment(ctx, &a)?;
    let body = a.req_str("body", MAX_TEXT_CHARS)?;
    let op = tk(ctx, super::super::comments::edit_comment(ctx.conn, c.id, &body, &ctx.source))?;
    let result = json!({"changed": op.is_some(), "comment_id": c.id, "ticket": t.key()});
    if op.is_none() {
        return read(result);
    }
    wrote(result, ctx.tx(format!("Kommentar an {} geändert", t.key()), format!("edited a comment on {}", t.key())), Some(t.key()))
}

/// `delete_comment`: Agenten-Kommentar in den Papierkorb.
pub(super) fn delete_comment_tool(ctx: &Ctx, v: &Value) -> R {
    let a = args(ctx, v, &["comment"])?;
    deletes_allowed(ctx)?;
    let (c, t) = agent_comment(ctx, &a)?;
    tk(ctx, acc::trash_comment(ctx.conn, c.id, &ctx.source))?;
    wrote(
        json!({"comment_id": c.id, "ticket": t.key(), "trashed": true}),
        ctx.tx(format!("Kommentar an {} in den Papierkorb gelegt", t.key()), format!("moved a comment on {} to the trash", t.key())),
        Some(t.key()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::{AccessMap, AgentAccess, AreaLevel, Caller, TicketAccess};
    use crate::planning::tickets::fixture::Fx;
    use chrono::NaiveDate;

    fn now() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 11, 14).unwrap().and_hms_opt(12, 0, 0).unwrap()
    }

    fn with<'a>(f: &'a Fx, view: &'a AgentView) -> Ctx<'a> {
        Ctx { conn: &f.c, lang: Lang::En, source: Source::agent("Builder"), now: now(), view: Some(view), need: Default::default() }
    }

    fn todo_id(f: &Fx) -> i64 {
        f.c.query_row("SELECT id FROM projects WHERE system = 1", [], |r| r.get(0)).unwrap()
    }

    /// Fixture: Projekt PAN (verknüpft mit Seitenleisten-Projekt 1), zweites Projekt SEC (gesperrt), TODO.
    fn setup() -> (Fx, i64) {
        let f = Fx::new();
        let mut np = crate::planning::projects::NewProject::new("SEC", "Secret", "#aa0000");
        np.with_defaults = true;
        let sec = crate::planning::projects::create_project(&f.c, &np).unwrap();
        acc::set_project_access(&f.c, f.project, AgentAccess::LOCKED, Some(1)).unwrap();
        (f, sec)
    }

    fn map(f: &Fx, sec: i64, pan_level: AreaLevel) -> AccessMap {
        let mut m = AccessMap { global: McpLevel::Control, general: AgentAccess::AS_BEFORE, ..Default::default() };
        m.projects.insert(1, AgentAccess { level: pan_level, for_all: false });
        m.projects.insert(2, AgentAccess::LOCKED);
        m.tickets.insert(f.project, TicketAccess { link: Some(1), own: AgentAccess::LOCKED, system: false });
        m.tickets.insert(sec, TicketAccess { link: None, own: AgentAccess::LOCKED, system: false });
        m.tickets.insert(todo_id(f), TicketAccess { link: None, own: AgentAccess::AS_BEFORE, system: true });
        m
    }

    fn keys(d: &Done) -> Vec<String> {
        d.result["tickets"].as_array().unwrap().iter().map(|t| t["key"].as_str().unwrap().to_string()).collect()
    }

    fn mk_in(f: &Fx, project: i64, title: &str) -> Ticket {
        let ty = crate::planning::types::list_types(&f.c, project).unwrap().into_iter().find(|t| !t.system).unwrap();
        crate::planning::tickets::create_ticket(&f.c, &crate::planning::tickets::NewTicket::new(project, ty.id, title), &Source::ui()).unwrap().0
    }

    #[test]
    fn locked_and_private_tickets_are_missing_everywhere() {
        let (f, sec) = setup();
        let a = f.mk(f.task, "Visible");
        let p = f.mk(f.task, "Private");
        acc::set_ticket_private(&f.c, p.id, true, &Source::ui()).unwrap();
        let s = mk_in(&f, sec, "Secret");
        let tr = f.mk(f.task, "Trashed");
        acc::trash_ticket(&f.c, tr.id, &Source::ui()).unwrap();
        let view = map(&f, sec, AreaLevel::Control).view(Caller::Home(1), false, false);
        let ctx = with(&f, &view);
        // Listen und Zähler
        let d = call(&ctx, "search_tickets", &json!({}), true).unwrap();
        assert_eq!(keys(&d), vec![a.key()]);
        assert_eq!(d.result["total"], 1);
        let d = call(&ctx, "search_tickets", &json!({"include_archived": true}), true).unwrap();
        assert_eq!(keys(&d), vec![a.key()], "auch nicht mit archivierten (Papierkorb)");
        let d = call(&ctx, "search_tickets", &json!({"query": "ORDER BY key"}), true).unwrap();
        assert_eq!(keys(&d), vec![a.key()], "JQL");
        assert_eq!(call(&ctx, "search_tickets", &json!({"query": "summary ~ Secret"}), true).unwrap().result["total"], 0);
        let d = call(&ctx, "list_projects", &json!({}), true).unwrap();
        let listed: Vec<&str> = d.result["projects"].as_array().unwrap().iter().map(|p| p["key"].as_str().unwrap()).collect();
        assert!(listed.contains(&"PAN") && !listed.contains(&"SEC"), "{listed:?}");
        assert!(!d.result.to_string().contains("\"private\""), "das Feld private sieht ein Agent nie");
        // direkter Zugriff
        assert_eq!(call(&ctx, "get_ticket", &json!({"ticket": p.key()}), true).unwrap_err().code, "not_found");
        assert_eq!(call(&ctx, "get_ticket", &json!({"ticket": tr.key()}), true).unwrap_err().code, "not_found");
        assert_eq!(call(&ctx, "get_ticket", &json!({"ticket": s.key()}), true).unwrap_err().code, crate::access::LOCKED_CODE);
        assert_eq!(call(&ctx, "get_project", &json!({"project": "SEC"}), true).unwrap_err().code, crate::access::LOCKED_CODE);
        let e = call(&ctx, "get_project", &json!({"project": "NOPE"}), true).unwrap_err();
        assert!(!e.message.contains("SEC"), "nie ein unsichtbares Projekt nennen: {}", e.message);
        assert_eq!(call(&ctx, "search_tickets", &json!({"project": "SEC"}), true).unwrap_err().code, crate::access::LOCKED_CODE);
        // Verknüpfungen zu unsichtbaren Tickets fehlen; link_ticket braucht Lesen im Ziel
        crate::planning::links::add_link(&f.c, a.id, crate::planning::link_types::list_link_types(&f.c).unwrap().iter().find(|l| l.target_kind == crate::planning::link_types::TargetKind::Ticket).unwrap().id, crate::planning::links::Target::Ticket(s.id), &Source::ui()).unwrap();
        let d = call(&ctx, "get_ticket", &json!({"ticket": a.key()}), true).unwrap();
        assert_eq!(d.result["links"].as_array().unwrap().len(), 0, "{}", d.result["links"]);
        assert!(!d.result.to_string().contains(&s.key()), "nirgends genannt (auch nicht im Verlauf)");
        assert_eq!(call(&ctx, "link_ticket", &json!({"ticket": a.key(), "to_ticket": s.key()}), true).unwrap_err().code, crate::access::LOCKED_CODE);
        assert_eq!(call(&ctx, "link_ticket", &json!({"ticket": a.key(), "to_ticket": p.key()}), true).unwrap_err().code, "not_found");
    }

    /// Review 3.2.0: Der Verlauf von `get_ticket` nennt Agenten keinen Kommentartext (ein Kommentar im Papierkorb bliebe
    /// sonst lesbar), und Terminal-Verknüpfungen nennen nur Sitzungen, die der Agent sieht.
    #[test]
    fn history_hides_comment_texts_and_links_hide_invisible_sessions() {
        let (f, sec) = setup();
        let a = f.mk(f.task, "Visible");
        let (cid, _) = crate::planning::comments::add_comment(&f.c, a.id, "my secret password is hunter2", &Source::ui()).unwrap();
        acc::trash_comment(&f.c, cid, &Source::ui()).unwrap();
        let term = crate::planning::link_types::list_link_types(&f.c).unwrap().into_iter().find(|l| l.target_kind == crate::planning::link_types::TargetKind::Terminal).unwrap().id;
        crate::planning::links::add_link(&f.c, a.id, term, crate::planning::links::Target::Terminal("session:7".into()), &Source::ui()).unwrap();
        crate::planning::links::add_link(&f.c, a.id, term, crate::planning::links::Target::Terminal("session:8".into()), &Source::ui()).unwrap();
        let mut view = map(&f, sec, AreaLevel::Control).view(Caller::Home(1), false, false);
        view.sessions = Some([7].into_iter().collect());
        let ctx = with(&f, &view);
        let d = call(&ctx, "get_ticket", &json!({"ticket": a.key()}), true).unwrap();
        let text = d.result.to_string();
        assert!(!text.contains("hunter2"), "kein Kommentartext im Verlauf: {}", d.result["history"]);
        assert!(text.contains("Comment added"), "der Eintrag selbst bleibt: {}", d.result["history"]);
        assert_eq!(d.result["comments"].as_array().unwrap().len(), 0, "Papierkorb");
        assert_eq!(d.result["linked_terminals"], json!(["session:7"]), "Sitzung 8 (gesperrtes Projekt) fehlt");
        let terminals: Vec<&Value> = d.result["links"].as_array().unwrap().iter().filter(|l| l["kind"] == "terminal").collect();
        assert_eq!(terminals.len(), 1, "{}", d.result["links"]);
        assert!(!text.contains("session:8"));
        // Ohne Sicht (Oberfläche, interne Aufrufe): wie bisher.
        let ui = Ctx { conn: &f.c, lang: Lang::En, source: Source::ui(), now: now(), view: None, need: Default::default() };
        let d = call(&ui, "get_ticket", &json!({"ticket": a.key()}), true).unwrap();
        assert!(d.result.to_string().contains("hunter2") && d.result["linked_terminals"].as_array().unwrap().len() == 2);
    }

    #[test]
    fn queries_never_name_fields_values_or_projects_of_locked_projects() {
        let (f, sec) = setup();
        let ty = crate::planning::types::list_types(&f.c, sec).unwrap().into_iter().find(|t| !t.system).unwrap();
        crate::planning::fields::create_field(&f.c, &crate::planning::fields::NewField::new(ty.id, "Geheimfeld", FieldKind::Text)).unwrap();
        let view = map(&f, sec, AreaLevel::Control).view(Caller::Home(1), false, false);
        let ctx = with(&f, &view);
        for q in ["Geheimfel = x", "Geheimfeld = x", "project = Nope", "type = Nope", "status = Nope"] {
            let e = call(&ctx, "search_tickets", &json!({"query": q}), true).unwrap_err();
            let all = format!("{} {}", e.message, e.details.map(|d| d.to_string()).unwrap_or_default());
            for secret in ["Secret", "SEC"] {
                assert!(!all.contains(secret), "{q}: {all}");
            }
            if q != "Geheimfeld = x" {
                assert!(!all.contains("Geheimfeld"), "{q}: {all}");
            }
        }
        // Sprints und Versionen gesperrter Projekte stehen auch nicht als Werte im Schema (#386)
        {
            use crate::planning::projects::{update_project, UpdateProject};
            use crate::planning::sprints::releases::{create_release, NewRelease, ReleaseKind};
            for p in [f.project, sec] {
                update_project(&f.c, p, &UpdateProject { use_sprints: Some(true), use_versions: Some(true), ..Default::default() }).unwrap();
            }
            let cal = crate::calc::calendar::WorkCalendar::default();
            crate::planning::sprints::sprint::create_sprint(&f.c, &crate::planning::sprints::sprint::NewSprint::new(f.project, "OpenSprint"), &cal).unwrap();
            crate::planning::sprints::sprint::create_sprint(&f.c, &crate::planning::sprints::sprint::NewSprint::new(sec, "HiddenSprint"), &cal).unwrap();
            create_release(&f.c, ReleaseKind::Version, &NewRelease::new(f.project, "OpenVer")).unwrap();
            create_release(&f.c, ReleaseKind::Version, &NewRelease::new(sec, "HiddenVer")).unwrap();
            create_release(&f.c, ReleaseKind::Milestone, &NewRelease::new(sec, "HiddenMile")).unwrap();
            let vis = view.project_ids();
            let agent = format!("{:?}", crate::planning::search::search_schema_only(&f.c, crate::planning::search::Scope::All, Lang::En, Some(&vis)).unwrap().schema);
            for secret in ["HiddenSprint", "HiddenVer", "HiddenMile", "Geheimfeld", "Secret"] {
                assert!(!agent.contains(secret), "{secret} im Schema des Agenten");
            }
            assert!(agent.contains("OpenSprint"), "sichtbare Sprints bleiben");
            let all = format!("{:?}", crate::planning::search::search_schema(&f.c, crate::planning::search::Scope::All, Lang::En).unwrap().schema);
            assert!(all.contains("HiddenSprint"), "Oberfläche kennt alles");
            let e = call(&ctx, "search_tickets", &json!({"query": "sprint = Nope"}), true).unwrap_err();
            assert!(!format!("{} {:?}", e.message, e.details).contains("HiddenSprint"));
        }
        // ohne Sicht (Oberfläche) kennt die Abfrage das Feld
        let plain = Ctx { conn: &f.c, lang: Lang::En, source: Source::ui(), now: now(), view: None, need: Default::default() };
        assert!(call(&plain, "search_tickets", &json!({"query": "Geheimfeld = x"}), true).is_ok());
    }

    #[test]
    fn sprints_counts_releases_and_maintenance_never_reveal_hidden_tickets() {
        use crate::calc::calendar::WorkCalendar;
        use crate::planning::projects::{update_project, UpdateProject};
        use crate::planning::sprints::sprint::{create_sprint, move_tickets_to_sprint, NewSprint};
        let (f, sec) = setup();
        update_project(&f.c, f.project, &UpdateProject { use_sprints: Some(true), ..Default::default() }).unwrap();
        update_project(&f.c, sec, &UpdateProject { use_sprints: Some(true), ..Default::default() }).unwrap();
        let a = f.mk(f.task, "Login fails on Safari");
        let p = f.mk(f.task, "Login fails in Safari");
        acc::set_ticket_private(&f.c, p.id, true, &Source::ui()).unwrap();
        let cal = WorkCalendar::default();
        let sp = create_sprint(&f.c, &NewSprint::new(f.project, "S1"), &cal).unwrap();
        move_tickets_to_sprint(&f.c, sp, &[a.id, p.id]).unwrap();
        let s = mk_in(&f, sec, "Secret");
        let sp_sec = create_sprint(&f.c, &NewSprint::new(sec, "Hidden"), &cal).unwrap();
        move_tickets_to_sprint(&f.c, sp_sec, &[s.id]).unwrap();
        let view = map(&f, sec, AreaLevel::Control).view(Caller::Home(1), false, false);
        let ctx = with(&f, &view);
        // 1. Sprint über die Kennung: Projekt geprüft; private Tickets fehlen in Liste, Summen und Bericht
        assert_eq!(call(&ctx, "get_sprint", &json!({"sprint": sp_sec}), true).unwrap_err().code, crate::access::LOCKED_CODE);
        let d = call(&ctx, "get_sprint", &json!({"sprint": sp}), true).unwrap();
        let keys: Vec<&str> = d.result["items"].as_array().unwrap().iter().map(|i| i["key"].as_str().unwrap()).collect();
        assert_eq!(keys, vec![a.key().as_str()]);
        assert_eq!(d.result["items_total"], 1);
        assert_eq!(d.result["sprint"]["totals"]["total"]["tickets"], 1, "{}", d.result["sprint"]["totals"]);
        assert!(d.result["report"].is_null());
        let d = call(&ctx, "list_sprints", &json!({"project": "PAN"}), true).unwrap();
        assert_eq!(d.result["sprints"][0]["totals"]["total"]["tickets"], 1);
        // 3. Zähler der Projekte
        let d = call(&ctx, "list_projects", &json!({}), true).unwrap();
        let pan = d.result["projects"].as_array().unwrap().iter().find(|x| x["key"] == "PAN").unwrap().clone();
        let all: i64 = f.c.query_row("SELECT COUNT(*) FROM tickets WHERE project_id = ?1", [f.project], |r| r.get(0)).unwrap();
        assert_eq!(pan["ticket_count"], all - 1, "ohne das private Ticket");
        // 2. Wartung: das Duplikat-Paar nennt das private Ticket – fehlt für den Agenten
        let d = call(&ctx, "maintenance_report", &json!({"project": "PAN"}), true).unwrap();
        assert!(!d.result.to_string().contains(&p.key()), "{}", d.result);
        let plain = Ctx { conn: &f.c, lang: Lang::En, source: Source::ui(), now: now(), view: None, need: Default::default() };
        assert!(call(&plain, "maintenance_report", &json!({"project": "PAN"}), true).unwrap().result.to_string().contains(&p.key()), "Oberfläche sieht es");
    }

    #[test]
    fn private_tags_do_not_exist_for_agents() {
        let (f, sec) = setup();
        tasks::create_item(&f.c, "x", "", None, &["secret".to_string()]).unwrap();
        let tag = tasks::find_tag(&f.c, "secret").unwrap().unwrap();
        tasks::set_tag_private(&f.c, tag.id, true).unwrap();
        let view = map(&f, sec, AreaLevel::Control).view(Caller::Home(1), false, false);
        let d = call(&with(&f, &view), "add_todo", &json!({"title": "New", "tags": ["Secret", "work"]}), false).unwrap();
        assert_eq!(d.result["ignored_tags"], json!(["Secret"]));
        assert_eq!(d.result["todo"]["tags"], json!(["work"]));
        let it = tasks::get_item(&f.c, d.result["todo"]["id"].as_i64().unwrap()).unwrap();
        assert!(!it.tags.contains(&tag.id), "die Karte bekommt den privaten Tag nicht (sie wäre sonst unsichtbar)");
    }

    #[test]
    fn parent_queries_are_no_existence_oracle() {
        let (f, sec) = setup();
        let epic = f.mk(f.epic, "Hidden epic");
        acc::set_ticket_private(&f.c, epic.id, true, &Source::ui()).unwrap();
        let child = crate::planning::tickets::create_child(&f.c, epic.id, f.story, "Visible child", vec![], &Source::ui()).unwrap().0;
        let view = map(&f, sec, AreaLevel::Control).view(Caller::Home(1), false, false);
        let ctx = with(&f, &view);
        // `parent = KEY` des privaten Tickets trifft nichts (wie ein unbekannter Schlüssel)
        assert_eq!(call(&ctx, "search_tickets", &json!({"query": format!("parent = {}", epic.key())}), true).unwrap().result["total"], 0);
        // ein Kind mit unsichtbarem Eltern-Ticket gilt als ohne Eltern
        let d = call(&ctx, "search_tickets", &json!({"no_parent": true}), true).unwrap();
        assert!(keys(&d).contains(&child.key()), "{:?}", keys(&d));
        let d = call(&ctx, "search_tickets", &json!({"query": "parent IS EMPTY"}), true).unwrap();
        assert!(keys(&d).contains(&child.key()), "{:?}", keys(&d));
        let d = call(&ctx, "get_ticket", &json!({"ticket": child.key()}), true).unwrap();
        assert!(d.result["parent"].is_null());
    }

    #[test]
    fn ticket_fields_are_no_existence_oracle() {
        let (f, sec) = setup();
        let fid = crate::planning::fields::create_field(&f.c, &crate::planning::fields::NewField::new(f.task, "Blocks", FieldKind::Ticket)).unwrap();
        let _ = fid;
        let a = f.mk(f.task, "A");
        let b = f.mk(f.task, "B");
        let p = f.mk(f.task, "P");
        acc::set_ticket_private(&f.c, p.id, true, &Source::ui()).unwrap();
        let s = mk_in(&f, sec, "Secret");
        let view = map(&f, sec, AreaLevel::Control).view(Caller::Home(1), false, false);
        let ctx = with(&f, &view);
        assert!(call(&ctx, "update_ticket", &json!({"ticket": a.key(), "fields": {"Blocks": b.key()}}), true).is_ok());
        for target in [json!(s.key()), json!(p.key()), json!(s.id), json!(p.id), json!("NOPE-1"), json!(99999)] {
            let e = call(&ctx, "update_ticket", &json!({"ticket": a.key(), "fields": {"Blocks": target}}), true).unwrap_err();
            assert_eq!(e.code, "invalid_value", "{target}: gleiche Antwort wie für ein unbekanntes Ticket");
        }
    }

    #[test]
    fn read_only_projects_refuse_writes_with_forbidden() {
        let (f, sec) = setup();
        let a = f.mk(f.task, "A");
        let view = map(&f, sec, AreaLevel::Read).view(Caller::Home(1), true, true);
        let ctx = with(&f, &view);
        assert!(call(&ctx, "get_ticket", &json!({"ticket": a.key()}), true).is_ok());
        for (tool, args) in [
            ("update_ticket", json!({"ticket": a.key(), "title": "x"})),
            ("add_comment", json!({"ticket": a.key(), "body": "x"})),
            ("create_ticket", json!({"project": "PAN", "type": "Task", "title": "x"})),
            ("delete_ticket", json!({"ticket": a.key()})),
            ("create_project", json!({"key": "NEW", "name": "New"})),
        ] {
            assert_eq!(call(&ctx, tool, &args, true).unwrap_err().code, "forbidden", "{tool}");
        }
        // anderes Heimatprojekt: PAN ist nicht für alle → gesperrt
        let other = map(&f, sec, AreaLevel::Control).view(Caller::Home(2), true, true);
        assert_eq!(call(&with(&f, &other), "get_ticket", &json!({"ticket": a.key()}), true).unwrap_err().code, crate::access::LOCKED_CODE);
        let ext = map(&f, sec, AreaLevel::Control).view(Caller::External, true, true);
        assert_eq!(call(&with(&f, &ext), "search_tickets", &json!({}), true).unwrap().result["total"], 0);
    }

    #[test]
    fn cards_follow_their_area_privacy_and_tags() {
        let (f, sec) = setup();
        let (open, _) = tasks::create_item(&f.c, "General", "", None, &[]).unwrap();
        let (mine, _) = tasks::create_item_ext(&f.c, "Home", "", None, &[], &tasks::ItemMeta { project: Some(1), ..Default::default() }).unwrap();
        let (locked, _) = tasks::create_item_ext(&f.c, "Locked", "", None, &[], &tasks::ItemMeta { project: Some(2), ..Default::default() }).unwrap();
        let (private, _) = tasks::create_item_ext(&f.c, "Private", "", None, &[], &tasks::ItemMeta { private: true, ..Default::default() }).unwrap();
        let (tagged, _) = tasks::create_item(&f.c, "Tagged", "", None, &["secret".to_string()]).unwrap();
        let tag = tasks::find_tag(&f.c, "secret").unwrap().unwrap();
        tasks::set_tag_private(&f.c, tag.id, true).unwrap();
        let view = map(&f, sec, AreaLevel::Control).view(Caller::Home(1), false, false);
        let ctx = with(&f, &view);
        let d = call(&ctx, "list_todos", &json!({}), false).unwrap();
        let mut ids: Vec<i64> = d.result["todos"].as_array().unwrap().iter().map(|t| t["id"].as_i64().unwrap()).collect();
        ids.sort();
        assert_eq!(ids, vec![open.id, mine.id]);
        assert_eq!(d.result["total"], 2, "Zähler ohne private/gesperrte");
        assert!(!d.result.to_string().contains("secret"), "privater Tag erscheint nicht");
        assert_eq!(call(&ctx, "set_todo_done", &json!({"id": private.id}), false).unwrap_err().code, "not_found");
        assert_eq!(call(&ctx, "set_todo_done", &json!({"id": tagged.id}), false).unwrap_err().code, "not_found");
        assert_eq!(call(&ctx, "set_todo_done", &json!({"id": locked.id}), false).unwrap_err().code, crate::access::LOCKED_CODE);
        assert!(call(&ctx, "set_todo_done", &json!({"id": mine.id}), false).is_ok());
        // neue Karte: Heimatprojekt, Herkunft Agent
        let d = call(&ctx, "add_todo", &json!({"title": "From agent"}), false).unwrap();
        let id = d.result["todo"]["id"].as_i64().unwrap();
        let it = tasks::get_item(&f.c, id).unwrap();
        assert_eq!((it.project, it.origin, it.origin_detail.as_deref(), it.private), (Some(1), Origin::Agent, Some("Builder"), false));
        assert_eq!(d.result["todo"]["origin"], "agent");
        // im gesperrten Heimatprojekt: keine Karte
        let locked_home = map(&f, sec, AreaLevel::Control).view(Caller::Home(2), false, false);
        assert_eq!(call(&with(&f, &locked_home), "add_todo", &json!({"title": "x"}), false).unwrap_err().code, crate::access::LOCKED_CODE);
        // externer Agent: Allgemein
        let ext = map(&f, sec, AreaLevel::Control).view(Caller::External, false, false);
        let d = call(&with(&f, &ext), "add_todo", &json!({"title": "ext"}), false).unwrap();
        assert_eq!(tasks::get_item(&f.c, d.result["todo"]["id"].as_i64().unwrap()).unwrap().project, None);
    }

    #[test]
    fn agents_create_linked_ticket_projects_only_with_the_switch() {
        let (f, sec) = setup();
        let m = map(&f, sec, AreaLevel::Control);
        let off = m.view(Caller::Home(1), false, false);
        assert_eq!(call(&with(&f, &off), "create_project", &json!({"key": "AGT", "name": "Agent"}), true).unwrap_err().code, "forbidden");
        let ext = m.view(Caller::External, true, false);
        assert_eq!(call(&with(&f, &ext), "create_project", &json!({"key": "AGT", "name": "Agent"}), true).unwrap_err().code, "forbidden");
        let on = m.view(Caller::Home(1), true, false);
        let d = call(&with(&f, &on), "create_project", &json!({"key": "AGT", "name": "Agent"}), true).unwrap();
        assert!(d.change.is_some());
        assert_eq!(d.result["project"]["created_by_agent"], true);
        let id: i64 = f.c.query_row("SELECT id FROM projects WHERE key = 'AGT'", [], |r| r.get(0)).unwrap();
        assert_eq!(acc::project_access(&f.c, id).unwrap(), (AgentAccess::LOCKED, Some(1)), "verknüpft mit dem Heimatprojekt");
        assert_eq!(acc::project_origin(&f.c, id).unwrap(), (Origin::Agent, Some("Builder".to_string())));
        // folgt der Stufe des Heimatprojekts
        let mut m2 = m.clone();
        m2.tickets.insert(id, TicketAccess { link: Some(1), own: AgentAccess::LOCKED, system: false });
        assert_eq!(m2.view(Caller::Home(1), false, false).ticket_level(id), McpLevel::Control);
    }

    #[test]
    fn deletes_go_to_the_trash_and_only_agent_comments_can_be_changed() {
        let (f, sec) = setup();
        let a = f.mk(f.task, "A");
        let (user_c, _) = crate::planning::comments::add_comment(&f.c, a.id, "user", &Source::ui()).unwrap();
        let (agent_c, _) = crate::planning::comments::add_comment(&f.c, a.id, "agent", &Source::agent("Other")).unwrap();
        let m = map(&f, sec, AreaLevel::Control);
        let no_del = m.view(Caller::Home(1), false, false);
        let ctx = with(&f, &no_del);
        assert_eq!(call(&ctx, "delete_ticket", &json!({"ticket": a.key()}), true).unwrap_err().code, "forbidden");
        assert_eq!(call(&ctx, "delete_comment", &json!({"comment": agent_c}), true).unwrap_err().code, "forbidden");
        // Ändern geht ohne Schalter, aber nur Agenten-Kommentare (auch die anderer Agenten)
        assert_eq!(call(&ctx, "update_comment", &json!({"comment": user_c, "body": "x"}), true).unwrap_err().code, "forbidden");
        let d = call(&ctx, "update_comment", &json!({"comment": agent_c, "body": "agent edited"}), true).unwrap();
        assert_eq!(d.result["changed"], true);
        let h = crate::planning::history::history_of(&f.c, a.id).unwrap();
        assert!(h.iter().any(|e| e.field == crate::planning::history::HistoryField::Comment && e.new.as_deref() == Some("agent edited")), "Änderung steht im Verlauf");
        let del = m.view(Caller::Home(1), false, true);
        let ctx = with(&f, &del);
        assert_eq!(call(&ctx, "delete_comment", &json!({"comment": user_c}), true).unwrap_err().code, "forbidden");
        assert!(call(&ctx, "delete_comment", &json!({"comment": agent_c}), true).is_ok());
        assert_eq!(call(&ctx, "delete_comment", &json!({"comment": agent_c}), true).unwrap_err().code, "not_found", "schon im Papierkorb");
        assert!(call(&ctx, "delete_ticket", &json!({"ticket": a.key()}), true).is_ok());
        assert!(acc::is_trashed(&f.c, a.id), "nur Papierkorb, nicht endgültig");
        assert_eq!(acc::trash(&f.c).unwrap().len(), 2);
        assert_eq!(call(&ctx, "get_ticket", &json!({"ticket": a.key()}), true).unwrap_err().code, "not_found");
        // System-Projekt TODO bleibt tabu
        let todo_key: String = f.c.query_row("SELECT key FROM projects WHERE system = 1", [], |r| r.get(0)).unwrap();
        let _ = todo_key;
    }
}
