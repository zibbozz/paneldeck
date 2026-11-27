//! Welche Regeln für eine gespeicherte Datei gelten (#383).
//!
//! - Datei-Regel: genau diese Datei; Ordner-Regel: alles im Ordner und darunter; Muster-Regel: Dateien im Ordner, die
//!   zu den Mustern passen (Syntax wie `.gitignore`, relativ zum Regelordner, `!` nimmt wieder aus; unter Windows ohne
//!   Groß/Klein).
//! - Alle passenden Regeln laufen in Listenreihenfolge. „Regeln übergeordneter Ordner nicht anwenden“ einer passenden
//!   Regel lässt Regeln aus, deren Ordner echt über ihrem liegt; nach einer gelaufenen Regel mit „Keine weiteren Regeln
//!   danach“ kommt keine weitere.
//! - Auslöser: ausdrückliches Speichern immer, automatisches Speichern und Agenten nur, wenn die Regel es erlaubt
//!   (gilt auch für die Text-Aktionen).
//! - Gleiche eingebaute Aktionen mehrerer Regeln laufen nur einmal; sie laufen vor dem Schreiben, alle übrigen
//!   Aktionen danach in Listenreihenfolge.

use super::{is_inside, Action, ActionKind, Builtin, Config, Rule, SaveOrigin, Scope};
use crate::state::Lang;
use std::path::Path;

/// Warum eine passende Regel nicht läuft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// Regel ausgeschaltet.
    Disabled,
    /// Auslöser nicht erlaubt (automatisches Speichern bzw. Agent).
    Trigger(SaveOrigin),
    /// Eine frühere Regel hat „Keine weiteren Regeln danach“ (ihre Kennung).
    Stopped(u64),
    /// Eine Regel in einem Unterordner hat „Regeln übergeordneter Ordner nicht anwenden“ (ihre Kennung).
    Parent(u64),
    /// Hauptschalter aus.
    MasterOff,
    /// Bis zum Neustart pausiert.
    Paused,
    /// *Ohne Aktionen speichern*.
    WithoutActions,
}

impl SkipReason {
    pub fn code(&self) -> &'static str {
        match self {
            SkipReason::Disabled => "disabled",
            SkipReason::Trigger(SaveOrigin::Autosave) => "autosave_not_allowed",
            SkipReason::Trigger(_) => "agent_not_allowed",
            SkipReason::Stopped(_) => "stopped_by_earlier_rule",
            SkipReason::Parent(_) => "parent_rules_off",
            SkipReason::MasterOff => "switched_off",
            SkipReason::Paused => "paused",
            SkipReason::WithoutActions => "saved_without_actions",
        }
    }

    pub fn text(&self, lang: Lang, name_of: impl Fn(u64) -> String) -> String {
        match self {
            SkipReason::Disabled => lang.t("Regel ist aus", "rule is off").into(),
            SkipReason::Trigger(SaveOrigin::Autosave) => lang.t("nicht beim automatischen Speichern", "not on autosave").into(),
            SkipReason::Trigger(_) => lang.t("nicht beim Speichern durch Agenten", "not on agent saves").into(),
            SkipReason::Stopped(id) => format!("{} {}", lang.t("keine weiteren Regeln nach", "no further rules after"), lang.q(&name_of(*id))),
            SkipReason::Parent(id) => format!("{} {}", lang.t("übergeordnete Regeln aus wegen", "parent-folder rules off because of"), lang.q(&name_of(*id))),
            SkipReason::MasterOff => lang.t("Aktionen beim Speichern sind ausgeschaltet", "actions on save are switched off").into(),
            SkipReason::Paused => lang.t("pausiert bis zum Neustart", "paused until restart").into(),
            SkipReason::WithoutActions => lang.t("ohne Aktionen gespeichert", "saved without actions").into(),
        }
    }
}

/// Eine laufende Regel mit ihren Aktionen nach dem Schreiben.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedRule {
    pub rule: u64,
    pub name: String,
    /// Aktionen nach dem Schreiben (Befehl, Kopieren, Paneldeck), in Listenreihenfolge.
    pub post: Vec<Action>,
    /// Die Text-Aktionen dieser Regel mit „Bei Fehler weiter“ (für Meldung und Fehler; ausgeführt werden sie gesammelt
    /// in [`Plan::builtins`]).
    pub builtins: Vec<(Builtin, bool)>,
    /// Stelle der Regel in der Liste (Reihenfolge beim Zusammenführen wartender Läufe).
    pub pos: usize,
}

/// Was beim Speichern einer Datei passiert.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    /// Text-Aktionen vor dem Schreiben, ohne Doppelte (mit der Regel, aus der sie zuerst kamen, und ob ein Fehler den
    /// Rest dieser Regel nicht aufhält).
    pub builtins: Vec<(Builtin, u64, bool)>,
    pub rules: Vec<PlannedRule>,
    /// Passende Regeln, die nicht laufen, mit Grund.
    pub skipped: Vec<(u64, String, SkipReason)>,
}

#[cfg(test)]
impl Plan {
    pub fn is_empty(&self) -> bool {
        self.builtins.is_empty() && self.rules.iter().all(|r| r.post.is_empty())
    }

    /// Kennungen der laufenden Regeln.
    pub fn rule_ids(&self) -> Vec<u64> {
        self.rules.iter().map(|r| r.rule).collect()
    }
}

/// Passt die Regel zur Datei (ohne An/Aus)?
pub fn rule_matches(rule: &Rule, file: &Path) -> bool {
    let p = rule.path.trim();
    if p.is_empty() {
        return false;
    }
    match rule.scope {
        Scope::File => super::path_key(file) == super::path_key(Path::new(p)),
        Scope::Folder => is_inside(file, Path::new(p)) && super::path_key(file) != super::path_key(Path::new(p)),
        Scope::Pattern => {
            let folder = Path::new(p);
            let Some(rel) = super::relative(file, folder) else { return false };
            if rel.as_os_str().is_empty() {
                return false;
            }
            patterns_match(&rule.patterns, folder, &rel)
        }
    }
}

/// Muster wie `.gitignore` (relativ zum Ordner) gegen einen relativen Pfad prüfen. Ohne Muster passt nichts.
pub fn patterns_match(patterns: &[String], folder: &Path, rel: &Path) -> bool {
    let lines: Vec<String> = patterns.iter().map(|s| normalize_pattern(s)).filter(|s| !s.is_empty() && !s.starts_with('#')).collect();
    if lines.is_empty() {
        return false;
    }
    let mut b = ignore::gitignore::GitignoreBuilder::new(folder);
    b.case_insensitive(cfg!(windows)).ok();
    for l in &lines {
        // Ungültige Muster überspringen (die Oberfläche zeigt sie rot, `pattern_error`).
        let _ = b.add_line(None, l);
    }
    let Ok(gi) = b.build() else { return false };
    gi.matched_path_or_any_parents(folder.join(rel), false).is_ignore()
}

/// Muster bereinigen: Leerraum außen weg; unter Windows `\` als Trenner (wie im Explorer) zu `/` (in `.gitignore` wäre
/// `\` ein Maskierzeichen).
pub fn normalize_pattern(p: &str) -> String {
    let p = p.trim();
    if cfg!(windows) { p.replace('\\', "/") } else { p.to_string() }
}

/// Fehler eines Musters (für die Oberfläche), `None` = in Ordnung.

pub fn pattern_error(pattern: &str) -> Option<String> {
    let p = normalize_pattern(pattern);
    let p = p.as_str();
    if p.is_empty() || p.starts_with('#') {
        return None;
    }
    let mut b = ignore::gitignore::GitignoreBuilder::new(".");
    b.add_line(None, p).err().map(|e| e.to_string())
}

/// Darf die Regel bei diesem Auslöser laufen?
pub fn triggers(rule: &Rule, origin: SaveOrigin) -> bool {
    origin.explicit() || (origin == SaveOrigin::Autosave && rule.on_autosave) || (origin == SaveOrigin::Agent && rule.on_agent_save)
}

/// Schalter für einen Plan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Switches {
    /// Bis zum Neustart pausiert.
    pub paused: bool,
    /// Dieses Speichern ohne Aktionen.
    pub without_actions: bool,
}

/// Plan für das Speichern von `file` mit Auslöser `origin`.
pub fn plan(cfg: &Config, file: &Path, origin: SaveOrigin, sw: Switches) -> Plan {
    let matching: Vec<&Rule> = cfg.rules.iter().filter(|r| rule_matches(r, file)).collect();
    let mut out = Plan::default();
    let global = if !cfg.enabled {
        Some(SkipReason::MasterOff)
    } else if sw.without_actions {
        Some(SkipReason::WithoutActions)
    } else if sw.paused {
        Some(SkipReason::Paused)
    } else {
        None
    };
    if let Some(g) = global {
        out.skipped = matching.iter().map(|r| (r.id, r.display_name(), g.clone())).collect();
        return out;
    }
    // Regeln mit „übergeordnete nicht anwenden“ (nur eingeschaltete) und ihre Ordner.
    let blockers: Vec<(u64, std::path::PathBuf)> = matching.iter().filter(|r| r.enabled && r.no_parent_rules).map(|r| (r.id, r.folder())).collect();
    let mut stopped_by: Option<u64> = None;
    for r in matching {
        let pos = cfg.rules.iter().position(|x| x.id == r.id).unwrap_or(usize::MAX);
        let name = r.display_name();
        if !r.enabled {
            out.skipped.push((r.id, name, SkipReason::Disabled));
            continue;
        }
        let folder = r.folder();
        if let Some((b, _)) = blockers.iter().find(|(id, f)| *id != r.id && is_inside(f, &folder) && super::path_key(f) != super::path_key(&folder)) {
            out.skipped.push((r.id, name, SkipReason::Parent(*b)));
            continue;
        }
        if let Some(s) = stopped_by {
            out.skipped.push((r.id, name, SkipReason::Stopped(s)));
            continue;
        }
        if !triggers(r, origin) {
            out.skipped.push((r.id, name, SkipReason::Trigger(origin)));
            continue;
        }
        let mut pr = PlannedRule { rule: r.id, name, post: Vec::new(), builtins: Vec::new(), pos };
        for a in &r.actions {
            match &a.kind {
                ActionKind::Builtin(b) => {
                    pr.builtins.push((b.clone(), a.continue_on_error));
                    if !out.builtins.iter().any(|(x, ..)| x == b) {
                        out.builtins.push((b.clone(), r.id, a.continue_on_error));
                    }
                }
                ActionKind::Unknown(_) => {}
                _ => pr.post.push(a.clone()),
            }
        }
        out.rules.push(pr);
        if r.stop_after {
            stopped_by = Some(r.id);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save_actions::{Action, ActionKind};
    use std::path::PathBuf;

    fn base() -> PathBuf {
        PathBuf::from(if cfg!(windows) { r"C:\work\app" } else { "/work/app" })
    }

    fn rule(id: u64, scope: Scope, path: &Path, patterns: &[&str], cmd: &str) -> Rule {
        let mut r = Rule::new(id, scope, &path.to_string_lossy());
        r.patterns = patterns.iter().map(|s| s.to_string()).collect();
        r.actions = vec![Action::new(ActionKind::Command { command: cmd.into() })];
        r
    }

    fn cfg(rules: Vec<Rule>) -> Config {
        Config { rules, ..Default::default() }
    }

    #[test]
    fn file_folder_and_pattern_rules_match() {
        let b = base();
        let ts = b.join("src").join("ui").join("a.ts");
        let dist = b.join("dist").join("a.ts");
        assert!(rule_matches(&rule(1, Scope::File, &ts, &[], "x"), &ts));
        assert!(!rule_matches(&rule(1, Scope::File, &ts, &[], "x"), &dist));
        assert!(rule_matches(&rule(1, Scope::Folder, &b, &[], "x"), &dist));
        assert!(!rule_matches(&rule(1, Scope::Folder, &b.join("src"), &[], "x"), &dist));
        let p = rule(1, Scope::Pattern, &b, &["src/**/*.ts"], "x");
        assert!(rule_matches(&p, &ts));
        assert!(!rule_matches(&p, &dist));
        let any = rule(1, Scope::Pattern, &b, &["*.ts", "!dist/**"], "x");
        assert!(rule_matches(&any, &ts), "*.ts auf jeder Tiefe");
        assert!(!rule_matches(&any, &dist), "! nimmt wieder aus");
        assert!(!rule_matches(&rule(1, Scope::Pattern, &b, &[], "x"), &ts), "ohne Muster passt nichts");
        assert!(!rule_matches(&rule(1, Scope::Pattern, &b, &["*.ts"], "x"), &PathBuf::from(if cfg!(windows) { r"C:\other\a.ts" } else { "/other/a.ts" })));
        if cfg!(windows) {
            assert!(rule_matches(&rule(1, Scope::Pattern, &b, &["*.JSON"], "x"), &b.join("Package.json")), "Windows: ohne Groß/Klein");
            assert!(rule_matches(&rule(1, Scope::Pattern, &b, &[r"src\**\*.ts"], "x"), &ts), "Windows: Backslash als Trenner");
            assert_eq!(normalize_pattern(r" src\a "), "src/a");
        }
        assert!(pattern_error("src/{a").is_some(), "offene Alternative");
        assert!(pattern_error("*.ts").is_none());
    }

    #[test]
    fn all_matching_rules_run_in_order_with_stop_and_parent_flags() {
        let b = base();
        let f = b.join("src").join("a.json");
        let mut top = rule(1, Scope::Folder, &b, &[], "top");
        let sub = rule(2, Scope::Folder, &b.join("src"), &[], "sub");
        let pat = rule(3, Scope::Pattern, &b, &["*.json"], "pat");
        let p = plan(&cfg(vec![top.clone(), sub.clone(), pat.clone()]), &f, SaveOrigin::User, Switches::default());
        assert_eq!(p.rule_ids(), [1, 2, 3]);
        // Stopp nach Regel 2.
        let mut sub_stop = sub.clone();
        sub_stop.stop_after = true;
        let p = plan(&cfg(vec![top.clone(), sub_stop, pat.clone()]), &f, SaveOrigin::User, Switches::default());
        assert_eq!(p.rule_ids(), [1, 2]);
        assert_eq!(p.skipped, vec![(3, pat.display_name(), SkipReason::Stopped(2))]);
        // Unterordner-Regel schaltet die Regeln darüber ab (auch spätere in der Liste), nicht die im selben Ordner.
        let mut sub_np = sub.clone();
        sub_np.no_parent_rules = true;
        let p = plan(&cfg(vec![sub_np, top.clone(), pat.clone()]), &f, SaveOrigin::User, Switches::default());
        assert_eq!(p.rule_ids(), [2]);
        assert!(p.skipped.iter().all(|(_, _, r)| *r == SkipReason::Parent(2)));
        // Ausgeschaltet.
        top.enabled = false;
        let p = plan(&cfg(vec![top, sub]), &f, SaveOrigin::User, Switches::default());
        assert_eq!(p.rule_ids(), [2]);
        assert_eq!(p.skipped[0].2, SkipReason::Disabled);
    }

    #[test]
    fn triggers_per_origin() {
        let b = base();
        let f = b.join("a.txt");
        let mut r = rule(1, Scope::Folder, &b, &[], "x");
        for o in [SaveOrigin::User, SaveOrigin::SaveAs, SaveOrigin::SaveAll, SaveOrigin::Close] {
            assert_eq!(plan(&cfg(vec![r.clone()]), &f, o, Switches::default()).rule_ids(), [1], "{o:?}");
        }
        for o in [SaveOrigin::Autosave, SaveOrigin::Agent] {
            let p = plan(&cfg(vec![r.clone()]), &f, o, Switches::default());
            assert!(p.rules.is_empty(), "{o:?}");
            assert_eq!(p.skipped[0].2, SkipReason::Trigger(o));
        }
        r.on_autosave = true;
        assert_eq!(plan(&cfg(vec![r.clone()]), &f, SaveOrigin::Autosave, Switches::default()).rule_ids(), [1]);
        assert!(plan(&cfg(vec![r.clone()]), &f, SaveOrigin::Agent, Switches::default()).rules.is_empty());
        r.on_agent_save = true;
        assert_eq!(plan(&cfg(vec![r.clone()]), &f, SaveOrigin::Agent, Switches::default()).rule_ids(), [1]);
        // Auch Text-Aktionen folgen dem Auslöser.
        let mut t = Rule::new(2, Scope::Folder, &b.to_string_lossy());
        t.actions = vec![Action::new(ActionKind::Builtin(Builtin::TrimWhitespace))];
        assert!(plan(&cfg(vec![t.clone()]), &f, SaveOrigin::Autosave, Switches::default()).builtins.is_empty());
        assert_eq!(plan(&cfg(vec![t]), &f, SaveOrigin::User, Switches::default()).builtins.len(), 1);
    }

    #[test]
    fn switches_and_duplicate_builtins() {
        let b = base();
        let f = b.join("a.txt");
        let mut r1 = Rule::new(1, Scope::Folder, &b.to_string_lossy());
        r1.actions = vec![Action::new(ActionKind::Builtin(Builtin::TrimWhitespace)), Action::new(ActionKind::Command { command: "a".into() })];
        let mut r2 = Rule::new(2, Scope::Pattern, &b.to_string_lossy());
        r2.patterns = vec!["*.txt".into()];
        r2.actions = vec![Action::new(ActionKind::Builtin(Builtin::TrimWhitespace)), Action::new(ActionKind::Builtin(Builtin::FinalNewline))];
        let c = cfg(vec![r1, r2]);
        let p = plan(&c, &f, SaveOrigin::User, Switches::default());
        assert_eq!(p.builtins.iter().map(|(b, ..)| b.clone()).collect::<Vec<_>>(), [Builtin::TrimWhitespace, Builtin::FinalNewline]);
        assert_eq!(p.rules[0].post.len(), 1);
        assert!(p.rules[1].post.is_empty());
        for (sw, why) in [(Switches { paused: true, without_actions: false }, SkipReason::Paused), (Switches { paused: false, without_actions: true }, SkipReason::WithoutActions)] {
            let p = plan(&c, &f, SaveOrigin::User, sw);
            assert!(p.is_empty() && p.rules.is_empty());
            assert!(p.skipped.iter().all(|s| s.2 == why));
        }
        let mut off = c.clone();
        off.enabled = false;
        assert!(plan(&off, &f, SaveOrigin::User, Switches::default()).skipped.iter().all(|s| s.2 == SkipReason::MasterOff));
    }
}
