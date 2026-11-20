//! `maintenance_report` (MCP, Stufe Lesen, nur Expertenmodus, #308): der Wartungsbericht eines Projekts – Befunde je Kategorie
//! (Art, Schwere, Ticket, Erklärung), optional nach Kategorie oder Schwere gefiltert, höchstens
//! [`MCP_MAX_FINDINGS`](crate::planning::maintenance::MCP_MAX_FINDINGS) Befunde mit `truncated` und `total`. Startet die Prüfung
//! bei Bedarf (nur lesend); ignorierte Befunde nur mit `include_ignored`. Es gibt keine reparierenden Aktionen über MCP.

use super::*;
use crate::planning::maintenance::{self as maint, Category, RunOpts, Severity, Target, MCP_MAX_FINDINGS};

fn target_json(t: &Target) -> Value {
    match t {
        Target::None | Target::Ticket => Value::Null,
        Target::Tickets(_) => json!({"kind": "tickets"}),
        Target::Field(id) => json!({"kind": "field", "id": id}),
        Target::FieldOption { field, option } => json!({"kind": "option", "field": field, "id": option}),
        Target::Status(id) => json!({"kind": "status", "id": id}),
        Target::Type(id) => json!({"kind": "type", "id": id}),
        Target::LinkType(id) => json!({"kind": "link_type", "id": id}),
        Target::WorkKind(id) => json!({"kind": "work_kind", "id": id}),
        Target::QuickAction(id) => json!({"kind": "quick_action", "id": id}),
        Target::Dashboard(id) => json!({"kind": "dashboard", "id": id}),
        Target::Filter(id) => json!({"kind": "filter", "id": id}),
        Target::Board(id) => json!({"kind": "board", "id": id}),
        Target::Labels { keep, merge } => json!({"kind": "labels", "keep": keep, "merge": merge}),
        Target::Rule(id) => json!({"kind": "rule", "id": id}),
        Target::Database => json!({"kind": "database"}),
    }
}

pub(super) fn maintenance_report_tool(ctx: &Ctx, v: &Value) -> R {
    let a = args(ctx, v, &["project", "category", "severity", "include_ignored", "limit"])?;
    let project = project_arg(ctx, &a, "project")?;
    if project.system {
        return Err(ctx.err("forbidden", "Das System-Projekt TODO hat keine Wartung.", "The system project TODO has no maintenance."));
    }
    let category = match a.opt_str("category", 32)? {
        None => None,
        Some(c) => Some(Category::parse(&c).ok_or_else(|| ctx.bad("category: tickets, configuration, database oder rules", "category: tickets, configuration, database or rules"))?),
    };
    let severity = match a.opt_str("severity", 32)? {
        None => None,
        Some(s) => Some(Severity::parse(&s).ok_or_else(|| ctx.bad("severity: error, warning oder hint", "severity: error, warning or hint"))?),
    };
    let limit = a.opt_int("limit", 1, MCP_MAX_FINDINGS as i64)?.unwrap_or(MCP_MAX_FINDINGS as i64) as usize;
    let include_ignored = a.opt_bool("include_ignored")?.unwrap_or(false);
    // Das Werkzeug läuft in der Oberfläche: die Integritätsprüfung der Datei (quick_check, liest die ganze Datei) nur mit
    // `category = database`, sonst nur Fremdschlüssel und Zähler; die Duplikat-Suche hat eine Obergrenze (`duplicates_truncated`).
    let db_integrity = category == Some(Category::Database);
    let opts = RunOpts {
        now: ctx.now,
        lang: ctx.lang,
        duplicates: category.is_none_or(|c| c == Category::Tickets),
        database: category.is_none_or(|c| c == Category::Database).then_some(if db_integrity { maint::DbScope::Quick } else { maint::DbScope::Structure }),
        rules: category.is_none_or(|c| c == Category::Rules),
        include_ignored,
        duplicate_budget: Some(maint::DupBudget::MCP),
    };
    let report = maint::run_report(ctx.conn, project.id, &opts).map_err(|e| ctx.err("db_error", e.message(Lang::De), e.message(Lang::En)))?;
    let ignored: std::collections::HashSet<String> = if include_ignored { maint::ignored(ctx.conn, project.id).unwrap_or_default().into_iter().map(|(id, _)| id).collect() } else { Default::default() };
    let matching: Vec<&maint::Finding> = report.findings.iter().filter(|f| category.is_none_or(|c| f.category() == c) && severity.is_none_or(|s| f.severity == s)).collect();
    let total = matching.len();
    let mut counts = Map::new();
    for c in Category::ALL {
        counts.insert(c.as_str().into(), json!(matching.iter().filter(|f| f.category() == c).count()));
    }
    let mut by_sev = Map::new();
    for s in Severity::ALL {
        by_sev.insert(s.as_str().into(), json!(matching.iter().filter(|f| f.severity == s).count()));
    }
    let findings: Vec<Value> = matching
        .iter()
        .take(limit)
        .map(|f| {
            let mut o = json!({
                "id": f.id,
                "category": f.category().as_str(),
                "kind": f.check.as_str(),
                "severity": f.severity.as_str(),
                "ticket": f.ticket.as_ref().map(|t| t.key.clone()),
                "title": f.ticket.as_ref().map(|t| short(&t.title, 200)),
                "other_ticket": f.other.as_ref().map(|t| t.key.clone()),
                "target": target_json(&f.target),
                "message": short(&f.message, 500),
            });
            if include_ignored {
                o["ignored"] = json!(ignored.contains(&f.id));
            }
            o
        })
        .collect();
    read(json!({
        "project": project.key,
        "generated_at": report.generated_at,
        "total": total,
        "returned": findings.len(),
        "truncated": total > findings.len(),
        "counts": counts,
        "by_severity": by_sev,
        "ignored_hidden": if include_ignored { 0 } else { report.ignored },
        "database_integrity_checked": db_integrity,
        "duplicates_truncated": report.duplicates_truncated,
        "findings": findings,
        "note": untrusted(ctx.lang),
    }))
}
