//! Konfigurations-Prüfungen der Wartung: Felder (aktiv / lange nicht gesetzt / wahrscheinlich ungenutzt / ungenutzt),
//! ungenutzte Auswahlwerte, Typen ohne Tickets, ungenutzte und unerreichbare Status, fast gleiche Labels, ungenutzte
//! Verknüpfungstypen und Arten der Arbeit, kaputte Schnellaktionen, Dashboards, gespeicherte Filter und Boards. Nur lesend.
//!
//! Entfernen lässt sich nur, was **ungenutzt** ist ([`Fix::Remove`]); Benutztes nur zusammenführen (Labels).

use super::{Check, Finding, Fix, MRes, RunOpts, Settings, Target};
use crate::planning::fields::{list_fields, FieldKind};
use crate::planning::integrity;
use crate::planning::projects::Project;
use crate::planning::search::{self, Scope};
use crate::state::Lang;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::json;
use std::collections::BTreeMap;

/// Anteil der Tickets eines Typs, unter dem ein lange nicht gesetztes Feld als „wahrscheinlich ungenutzt“ gilt.
pub const LIKELY_UNUSED_SHARE: f64 = 0.05;

fn finding(check: Check, id: String, target: Target, message: String, fixes: Vec<Fix>, detail: serde_json::Value) -> Finding {
    Finding { id, check, severity: check.severity(), ticket: None, other: None, target, message, fixes, detail }
}

pub fn run(conn: &Connection, project: &Project, s: &Settings, o: &RunOpts) -> MRes<Vec<Finding>> {
    let lang = o.lang;
    let mut out = Vec::new();
    let types = crate::planning::types::list_types(conn, project.id)?;
    // ---- Felder und Auswahlwerte
    for ty in types.iter().filter(|t| !t.system) {
        let tickets_of_type: i64 = conn.query_row("SELECT COUNT(*) FROM tickets WHERE type_id = ?1 AND archived = 0", [ty.id], |r| r.get(0))?;
        for f in list_fields(conn, ty.id)? {
            if f.is_fixed() {
                continue;
            }
            let name = format!("{} · {}", ty.display(lang), f.display(lang));
            let with_value: i64 = conn.query_row("SELECT COUNT(DISTINCT ticket_id) FROM ticket_values WHERE field_id = ?1", [f.id], |r| r.get(0))?;
            let last: Option<String> = conn.query_row("SELECT MAX(at) FROM history WHERE field = ?1", [format!("field:{}", f.id)], |r| r.get(0)).optional()?.flatten();
            if with_value == 0 {
                let message = match lang {
                    Lang::De => format!("Feld {} hat in keinem Ticket einen Wert – kann entfernt werden.", lang.q(&name)),
                    Lang::En => format!("Field {} has no value in any ticket – it can be removed.", lang.q(&name)),
                };
                out.push(finding(Check::FieldUnused, format!("field_unused:f{}", f.id), Target::Field(f.id), message, vec![Fix::Remove], json!({"field": name, "class": "unused"})));
            } else {
                let days = last.as_deref().and_then(|l| super::days_since(l, o.now));
                // ohne Verlauf (z. B. importiert): gilt als lange nicht gesetzt
                if days.is_none_or(|d| d >= s.field_days) {
                    let likely = tickets_of_type > 0 && (with_value as f64) < (tickets_of_type as f64) * LIKELY_UNUSED_SHARE;
                    let class = if likely { "likely_unused" } else { "stale" };
                    let since = match days {
                        Some(d) => match lang {
                            Lang::De => format!("seit {d} Tagen nicht gesetzt"),
                            Lang::En => format!("not set for {d} days"),
                        },
                        None => lang.t("nie über die Oberfläche gesetzt", "never set through the interface").to_string(),
                    };
                    let message = match (lang, likely) {
                        (Lang::De, true) => format!("Feld {} ist {since} und hat nur in {with_value} von {tickets_of_type} Tickets einen Wert – wahrscheinlich ungenutzt.", lang.q(&name)),
                        (Lang::En, true) => format!("Field {} is {since} and has a value in only {with_value} of {tickets_of_type} tickets – likely unused.", lang.q(&name)),
                        (Lang::De, false) => format!("Feld {} ist {since} (Schwelle {} Tage).", lang.q(&name), s.field_days),
                        (Lang::En, false) => format!("Field {} is {since} (threshold {} days).", lang.q(&name), s.field_days),
                    };
                    out.push(finding(Check::FieldStale, format!("field_stale:f{}", f.id), Target::Field(f.id), message, vec![], json!({"field": name, "class": class, "days": days, "tickets_with_value": with_value})));
                }
            }
            if matches!(f.kind, FieldKind::Dropdown | FieldKind::Multi) && with_value > 0 {
                for opt in &f.options.values {
                    if integrity::usage_count_option(conn, f.id, opt.id)? == 0 {
                        let oname = opt.display(lang);
                        let message = match lang {
                            Lang::De => format!("Auswahlwert {} im Feld {} wird von keinem Ticket benutzt.", lang.q(&oname), lang.q(&name)),
                            Lang::En => format!("Option {} of field {} is used by no ticket.", lang.q(&oname), lang.q(&name)),
                        };
                        out.push(finding(Check::OptionUnused, format!("option_unused:f{}:o{}", f.id, opt.id), Target::FieldOption { field: f.id, option: opt.id }, message, vec![Fix::Remove], json!({"field": name, "option": oname})));
                    }
                }
            }
        }
        // ---- Typen ohne Tickets (auch archivierte zählen als Benutzung)
        if integrity::usage_count_type(conn, ty.id)? == 0 {
            let message = match lang {
                Lang::De => format!("Typ {} hat keine Tickets – kann entfernt werden.", lang.q(&ty.display(lang))),
                Lang::En => format!("Type {} has no tickets – it can be removed.", lang.q(&ty.display(lang))),
            };
            out.push(finding(Check::TypeUnused, format!("type_unused:t{}", ty.id), Target::Type(ty.id), message, vec![Fix::Remove], json!({"type": ty.display(lang)})));
        }
        // ---- unerreichbare Status im Workflow
        for p in crate::planning::workflow::workflow_problems(conn, ty.id)? {
            if let crate::planning::workflow::WorkflowProblem::Unreachable(sid) = p {
                let sname = crate::planning::statuses::get_status(conn, sid).map(|s| s.display(lang)).unwrap_or_default();
                let message = match lang {
                    Lang::De => format!("Im Workflow von {} führt kein Übergang in den Status {}.", lang.q(&ty.display(lang)), lang.q(&sname)),
                    Lang::En => format!("In the workflow of {} no transition leads into the status {}.", lang.q(&ty.display(lang)), lang.q(&sname)),
                };
                out.push(finding(Check::StatusUnreachable, format!("status_unreachable:t{}:s{sid}", ty.id), Target::Status(sid), message, vec![], json!({"type": ty.display(lang), "status": sname})));
            }
        }
    }
    // ---- Status ohne Tickets und in keinem Workflow
    for st in crate::planning::statuses::list_statuses(conn, project.id)? {
        let in_wf: i64 = conn.query_row("SELECT COUNT(*) FROM type_statuses WHERE status_id = ?1", [st.id], |r| r.get(0))?;
        if in_wf == 0 && integrity::usage_count_status(conn, st.id)? == 0 {
            let message = match lang {
                Lang::De => format!("Status {} hat keine Tickets und steht in keinem Workflow – kann entfernt werden.", lang.q(&st.display(lang))),
                Lang::En => format!("Status {} has no tickets and is in no workflow – it can be removed.", lang.q(&st.display(lang))),
            };
            out.push(finding(Check::StatusUnused, format!("status_unused:s{}", st.id), Target::Status(st.id), message, vec![Fix::Remove], json!({"status": st.display(lang)})));
        }
    }
    labels(conn, project, lang, &mut out)?;
    // ---- Verknüpfungstypen (global) und Arten der Arbeit
    for lt in crate::planning::link_types::list_link_types(conn)?.into_iter().filter(|l| !l.system) {
        if integrity::usage_count_link_type(conn, lt.id)? == 0 {
            let n = lt.names(lang).0;
            let message = match lang {
                Lang::De => format!("Verknüpfungstyp {} wird nirgends benutzt (gilt für alle Projekte) – kann entfernt werden.", lang.q(&n)),
                Lang::En => format!("Link type {} is used nowhere (shared by all projects) – it can be removed.", lang.q(&n)),
            };
            out.push(finding(Check::LinkTypeUnused, format!("link_type_unused:l{}", lt.id), Target::LinkType(lt.id), message, vec![Fix::Remove], json!({"link_type": n})));
        }
    }
    for wk in crate::planning::work_kinds::list_work_kinds(conn, project.id)? {
        if integrity::usage_count_work_kind(conn, wk.id)? == 0 {
            let message = match lang {
                Lang::De => format!("Art der Arbeit {} hat keine Zeiteinträge – kann entfernt werden.", lang.q(&wk.display(lang))),
                Lang::En => format!("Kind of work {} has no time entries – it can be removed.", lang.q(&wk.display(lang))),
            };
            out.push(finding(Check::WorkKindUnused, format!("work_kind_unused:w{}", wk.id), Target::WorkKind(wk.id), message, vec![Fix::Remove], json!({"work_kind": wk.display(lang)})));
        }
    }
    views(conn, project, lang, &mut out)?;
    Ok(out)
}

/// Fast gleiche Labels (`bug` / `Bug `): gleich bis auf Groß-/Kleinschreibung und Leerraum. Behalten wird die häufigste Schreibweise.
fn labels(conn: &Connection, project: &Project, lang: Lang, out: &mut Vec<Finding>) -> MRes<()> {
    let mut st = conn.prepare(
        "SELECT v.text, COUNT(*) FROM ticket_values v JOIN fields f ON f.id = v.field_id JOIN tickets t ON t.id = v.ticket_id
         WHERE t.project_id = ?1 AND f.kind = 'labels' AND v.text IS NOT NULL GROUP BY v.text",
    )?;
    let mut groups: BTreeMap<String, Vec<(String, i64)>> = BTreeMap::new();
    for r in st.query_map([project.id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (text, n) = r?;
        groups.entry(label_key(&text)).or_default().push((text, n));
    }
    for (key, mut variants) in groups {
        if variants.len() < 2 || key.is_empty() {
            continue;
        }
        variants.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let keep = variants[0].0.clone();
        let merge: Vec<String> = variants[1..].iter().map(|v| v.0.clone()).collect();
        let shown: Vec<String> = variants.iter().map(|(t, n)| format!("{} ({n})", lang.q(t))).collect();
        let message = match lang {
            Lang::De => format!("Fast gleiche Labels: {} – zusammenführen zu {}?", shown.join(", "), lang.q(&keep)),
            Lang::En => format!("Near-identical labels: {} – merge into {}?", shown.join(", "), lang.q(&keep)),
        };
        out.push(finding(Check::LabelsSimilar, format!("labels_similar:{key}"), Target::Labels { keep: keep.clone(), merge: merge.clone() }, message, vec![Fix::MergeLabels], json!({"keep": keep, "merge": merge})));
    }
    Ok(())
}

/// Vergleichsschlüssel eines Labels: klein, Leerraum zusammengefasst und an den Enden entfernt.
pub fn label_key(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Schnellaktionen, Dashboards, gespeicherte Filter und Boards, die auf Gelöschtes verweisen oder kaputte Abfragen haben.
fn views(conn: &Connection, project: &Project, lang: Lang, out: &mut Vec<Finding>) -> MRes<()> {
    let qa = crate::planning::quick::actions::list_project_quick_actions(conn, project.id).map_err(|e| super::MaintError::Db(format!("{e:?}")))?;
    for a in qa.into_iter().filter(|a| a.invalid) {
        let message = match lang {
            Lang::De => format!("Schnellaktion {} verweist auf Gelöschtes (Feld, Status oder Typ) und ist abgeschaltet.", lang.q(&a.display(lang))),
            Lang::En => format!("Quick action {} refers to something deleted (field, status or type) and is disabled.", lang.q(&a.display(lang))),
        };
        out.push(finding(Check::QuickActionBroken, format!("quick_action_broken:q{}", a.id), Target::QuickAction(a.id), message, vec![], json!({"quick_action": a.display(lang)})));
    }
    let ctx = search::Ctx::new(&chrono::Local::now().format("%Y-%m-%d").to_string(), lang);
    let check = |scope: Scope, jql: &str| -> Option<String> {
        if jql.trim().is_empty() {
            return None;
        }
        search::prepare(conn, &ctx, scope, jql).err().map(|e| e.message(lang))
    };
    use crate::planning::views::dashboards::{list_dashboards, TileKind};
    let mut dashes = list_dashboards(conn, Some(project.id)).map_err(|e| super::MaintError::Db(e.message(lang)))?;
    dashes.extend(list_dashboards(conn, None).map_err(|e| super::MaintError::Db(e.message(lang)))?);
    for d in dashes {
        let scope = d.project_id.map(Scope::Project).unwrap_or(Scope::All);
        let mut problems = Vec::new();
        for t in &d.layout.tiles {
            if let TileKind::Search { jql, .. } | TileKind::Counts { jql, .. } = &t.kind {
                if let Some(e) = check(scope, jql) {
                    problems.push(e);
                }
            }
        }
        if !problems.is_empty() {
            let message = match lang {
                Lang::De => format!("Dashboard {}: {} Kachel(n) mit kaputter Abfrage – {}", lang.q(&d.name), problems.len(), problems[0]),
                Lang::En => format!("Dashboard {}: {} tile(s) with a broken query – {}", lang.q(&d.name), problems.len(), problems[0]),
            };
            out.push(finding(Check::DashboardBroken, format!("dashboard_broken:d{}", d.id), Target::Dashboard(d.id), message, vec![], json!({"dashboard": d.name, "problems": problems})));
        }
    }
    for f in crate::planning::views::filters::list_filters_for_project(conn, project.id).map_err(|e| super::MaintError::Db(e.message(lang)))? {
        let scope = f.project_id.map(Scope::Project).unwrap_or(Scope::All);
        if let Some(e) = check(scope, &f.jql) {
            let message = match lang {
                Lang::De => format!("Gespeicherter Filter {}: {e}", lang.q(&f.name)),
                Lang::En => format!("Saved filter {}: {e}", lang.q(&f.name)),
            };
            out.push(finding(Check::FilterBroken, format!("filter_broken:q{}", f.id), Target::Filter(f.id), message, vec![], json!({"filter": f.name})));
        }
    }
    for b in crate::planning::views::boards::list_boards(conn, project.id).map_err(|e| super::MaintError::Db(e.message(lang)))? {
        if let Err(e) = crate::planning::views::boards::check_config(conn, project.id, &b.config) {
            let message = match lang {
                Lang::De => format!("Board {}: {}", lang.q(&b.name), e.message(lang)),
                Lang::En => format!("Board {}: {}", lang.q(&b.name), e.message(lang)),
            };
            out.push(finding(Check::BoardBroken, format!("board_broken:b{}", b.id), Target::Board(b.id), message, vec![], json!({"board": b.name})));
        }
    }
    let _ = params![0];
    Ok(())
}
