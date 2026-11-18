//! Vorlagen-Bibliothek (#244): eine gemeinsame Liste in `templates.json` für
//! - **Gruppen-Vorlagen** – ganze Arbeitsumgebungen (mehrere Sitzungen, Ordner, Raster), gespeichert per Rechtsklick auf
//!   einen Ordner („Als Vorlage speichern …“), und
//! - **Terminal-Vorlagen** (`Template::terminal`) – genau eine Sitzung mit allen Einstellungen (Name, Profil/Shell,
//!   Startordner, Ordner im Baum, Farbe, Tags, Notizen, Claude mit Startoptionen, Konversation, Start-Prompt), die man
//!   beliebig oft gleich startet.
//!
//! Profile bleiben die Bausteine (Shell, Programm, Umgebung …), auf die eine Vorlage verweist. Alle neuen Felder sind
//! optional und werden nachsichtig gelesen: Ältere Dateien laden unverändert, ein falsch getyptes neues Feld lädt als
//! „nicht gesetzt“ statt die ganze Datei zu verwerfen.

use crate::claude_opts::ClaudeOpts;
use serde::{Deserialize, Deserializer, Serialize};

/// Feld nachsichtig lesen: falscher Typ → Standardwert (nie ein Ladefehler der ganzen Datei).
fn lenient<'de, D: Deserializer<'de>, T: serde::de::DeserializeOwned + Default>(d: D) -> Result<T, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(v).unwrap_or_default())
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Clone, Serialize, Deserialize, Default, PartialEq, Debug)]
#[serde(default)]
pub struct TplItem {
    pub name: String,
    pub shell_id: String,
    /// Startordner (Windows-Pfad, bei WSL-Shells auch ein Linux-Pfad).
    pub cwd: String,
    pub claude: bool,
    /// Alte Zeile mit Claude-Optionen (bis 2.3.x); neue Vorlagen nutzen `claude_opts`.
    pub claude_args: String,
    /// Profil (0 = keins).
    pub profile: u64,
    /// Ordnerpfad im Sitzungsbaum (leer = kein Ordner).
    pub group: String,
    /// Start-Prompt (nur bei Claude-Sitzungen).
    pub prompt: String,
    /// „Konversation fortsetzen“ der Sitzung (#245); fehlt = an (wie bisher), gespeichert wird nur „aus“.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claude_continue: Option<bool>,
    /// Farbe der Sitzung (#244); fehlt = Farbe des Profils bzw. automatisch.
    #[serde(skip_serializing_if = "Option::is_none", deserialize_with = "lenient")]
    pub color: Option<[u8; 3]>,
    /// Tags der Sitzung (#244).
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "lenient")]
    pub tags: Vec<String>,
    /// Notizen der Sitzung (#244).
    #[serde(skip_serializing_if = "String::is_empty", deserialize_with = "lenient")]
    pub notes: String,
    /// Claude-Startoptionen (#244), strukturiert wie in `state.json`; nie `bypassPermissions` (wird beim Speichern
    /// weggelassen). Fehlt = Optionen aus `claude_args` bzw. die Standards des Nutzers.
    #[serde(skip_serializing_if = "ClaudeOpts::is_empty", serialize_with = "crate::claude_opts::ser_user_opts")]
    pub claude_opts: ClaudeOpts,
    /// Beim Start einmalig die letzte Konversation im Ordner fortsetzen (`--continue`, #244); nur Starts des Nutzers.
    #[serde(skip_serializing_if = "is_false", deserialize_with = "lenient")]
    pub continue_last: bool,
}

#[derive(Clone, Serialize, Deserialize, Default, PartialEq, Debug)]
#[serde(default)]
pub struct Template {
    pub name: String,
    pub items: Vec<TplItem>,
    /// Beim Starten als Split-Raster anordnen.
    pub grid: bool,
    /// Terminal-Vorlage (#244): genau eine Sitzung, wiederholte Starts zählen den Namen hoch (`API`, `API #2` …).
    /// Fehlt = Gruppen-Vorlage. Ältere Versionen starten eine Terminal-Vorlage als Gruppe mit einer Sitzung.
    #[serde(skip_serializing_if = "is_false", deserialize_with = "lenient")]
    pub terminal: bool,
}

impl TplItem {
    /// Startoptionen der Vorlage: die gespeicherten, sonst die aus der alten Zeile `claude_args` (ohne die Rechte
    /// erweiternden Wertoptionen – die Zeile kann aus einer weitergegebenen Vorlage stammen). Nie `bypassPermissions`.
    pub fn opts(&self) -> ClaudeOpts {
        if !self.claude_opts.is_empty() {
            return self.claude_opts.sanitized(false);
        }
        let c = crate::claude_opts::clean_legacy_report(&self.claude_args, false);
        let (mut o, _) = ClaudeOpts::split_legacy(&c.line);
        o.extra = o.extra.or(&c.migrated);
        o.sanitized(false)
    }

    /// Optionen einer Sitzung, die ein **Agent** aus dieser Vorlage anlegt (MCP `create_session` mit `template`):
    /// Angaben des Agenten vor denen der Vorlage, danach wie jede Agenten-Sitzung ([`ClaudeOpts::for_agent`]) – nie
    /// `bypassPermissions`, nie die Rechte erweiternden Wertoptionen (zusätzliche Ordner, erlaubte Werkzeuge,
    /// MCP-Konfiguration), nie `--safe-mode`, ohne Permission-Mode ausdrücklich `default`.
    /// Schalter (`claude_flags`) werden wie bei den Standards ergänzt, nicht ersetzt; `chrome`/`no-chrome` des Agenten
    /// gehen vor. Ein `--append-system-prompt` der Vorlage gilt für Agenten ebenfalls nie (Text aus einer womöglich
    /// weitergegebenen Datei, den niemand bestätigt hat).
    pub fn agent_opts(&self, asked: &ClaudeOpts, defaults: &ClaudeOpts) -> ClaudeOpts {
        let mut tpl = self.opts();
        tpl.extra.append_system_prompt.clear();
        let mut merged = asked.or(&tpl);
        let chrome = |f: &String| f == "chrome" || f == "no-chrome";
        let own_chrome = asked.extra.flags.iter().any(chrome);
        let mut flags = asked.extra.flags.clone();
        for f in tpl.extra.flags.iter().filter(|f| !(own_chrome && chrome(f))) {
            if !flags.contains(f) {
                flags.push(f.clone());
            }
        }
        merged.extra.flags = flags;
        merged.for_agent(defaults)
    }

    /// Optionen dieser Vorlage, die für einen Agenten weggelassen werden (MCP-Feld `template_ignored`): Namen wie in
    /// den MCP-Auskünften (`add_dir`, `allowed_tools`, `mcp_config`, `append_system_prompt`, `safe-mode`,
    /// `bypassPermissions`, `continue_last`). Ohne Claude leer (dann gibt es keine Startoptionen).
    pub fn agent_ignored(&self) -> Vec<&'static str> {
        if !self.claude {
            return Vec::new();
        }
        let o = self.opts();
        let legacy_bypass = self.claude_opts.is_empty() && self.claude_args.to_ascii_lowercase().contains("bypasspermissions");
        [
            ("add_dir", !o.extra.add_dirs.is_empty()),
            ("allowed_tools", !o.extra.allowed_tools.is_empty()),
            ("mcp_config", !o.extra.mcp_config.is_empty()),
            ("append_system_prompt", !o.extra.append_system_prompt.is_empty()),
            ("safe-mode", o.extra.flags.iter().any(|f| !crate::claude_opts::agent_switch(f))),
            ("bypassPermissions", self.claude_opts.permission_mode.trim() == crate::claude_opts::BYPASS || legacy_bypass),
            ("continue_last", self.continue_last),
        ]
        .into_iter()
        .filter(|(_, on)| *on)
        .map(|(n, _)| n)
        .collect()
    }

    /// Optionen der Vorlage, die erweitern, worauf Claude zugreifen darf oder wie es läuft – vor einem Start durch den
    /// Nutzer wird nachgefragt (#244, `templates.json` kann weitergegeben werden). Ohne Claude leer.
    pub fn risky(&self) -> Vec<(Risk, String)> {
        if !self.claude {
            return Vec::new();
        }
        let o = self.opts();
        let mut v: Vec<(Risk, String)> = o.extra.add_dirs.iter().map(|d| (Risk::AddDir, d.clone())).collect();
        if !o.extra.allowed_tools.is_empty() {
            v.push((Risk::AllowedTools, o.extra.allowed_tools.clone()));
        }
        if !o.extra.mcp_config.is_empty() {
            v.push((Risk::McpConfig, o.extra.mcp_config.clone()));
        }
        if o.extra.flags.iter().any(|f| !crate::claude_opts::agent_switch(f)) {
            v.push((Risk::SafeMode, "--safe-mode".into()));
        }
        if !o.extra.append_system_prompt.is_empty() {
            v.push((Risk::AppendSystemPrompt, o.extra.append_system_prompt.clone()));
        }
        v
    }

    /// Startoptionen und bereinigte freie Reste der alten Zeile `claude_args` (frühere Vorlagen können dort Argumente
    /// haben, die keine Option kennt; sie bleiben erhalten).
    pub fn opts_and_rest(&self) -> (ClaudeOpts, String) {
        (self.opts(), legacy_rest(&self.claude_args))
    }

    /// Tags bereinigt (Ränder, leere und doppelte weg).
    pub fn clean_tags(tags: &[String]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for t in tags.iter().map(|t| t.trim().trim_start_matches('#').trim()).filter(|t| !t.is_empty()) {
            if !out.iter().any(|x| x.eq_ignore_ascii_case(t)) {
                out.push(t.to_string());
            }
        }
        out
    }
}

impl Template {
    /// Neue Terminal-Vorlage aus einer Sitzungsbeschreibung.
    pub fn terminal(name: &str, item: TplItem) -> Template {
        Template { name: name.trim().to_string(), items: vec![item], grid: false, terminal: true }
    }

    /// Ist das eine Terminal-Vorlage (genau eine Sitzung)?
    pub fn is_terminal(&self) -> bool {
        self.terminal && self.items.len() == 1
    }

    /// Optionen aller Sitzungen, nach denen vor dem Start gefragt wird ([`TplItem::risky`]): (Sitzungsname, Art, Wert).
    pub fn risky(&self) -> Vec<(String, Risk, String)> {
        self.items.iter().take(24).flat_map(|it| it.risky().into_iter().map(move |(k, v)| (it.name.trim().to_string(), k, v))).collect()
    }

    /// Die Sitzung einer Terminal-Vorlage.
    pub fn item(&self) -> Option<&TplItem> {
        self.is_terminal().then(|| &self.items[0])
    }

    /// Name, mit dem eine Terminal-Vorlage startet (vor dem Hochzählen): der Sitzungsname, sonst der Name der Vorlage.
    pub fn session_name(&self) -> String {
        match self.item().map(|i| i.name.trim()).filter(|n| !n.is_empty()) {
            Some(n) => n.to_string(),
            None => self.name.trim().to_string(),
        }
    }

    /// Passt die Vorlage zur Suche? Ohne Groß-/Kleinschreibung in Name, Sitzungsnamen, Ordnern und Tags; mehrere Wörter
    /// müssen alle vorkommen.
    pub fn matches(&self, query: &str) -> bool {
        let mut hay = self.name.to_lowercase();
        for it in &self.items {
            for s in [&it.name, &it.cwd, &it.group, &it.shell_id] {
                hay.push('\n');
                hay.push_str(&s.to_lowercase());
            }
            for t in &it.tags {
                hay.push_str("\n#");
                hay.push_str(&t.to_lowercase());
            }
        }
        query.split_whitespace().all(|w| hay.contains(&w.to_lowercase()))
    }
}

/// Option einer Vorlage, nach der vor dem Start gefragt wird ([`TplItem::risky`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Risk {
    /// `--add-dir`
    AddDir,
    /// `--allowed-tools`
    AllowedTools,
    /// `--mcp-config`
    McpConfig,
    /// `--safe-mode` (Hooks und MCP-Server aus)
    SafeMode,
    /// `--append-system-prompt`
    AppendSystemPrompt,
}

/// Dieselben Optionen ohne die, nach denen vor dem Start gefragt wird ([`TplItem::risky`]): „Ohne diese Optionen
/// starten“.
pub fn strip_risky(o: &ClaudeOpts) -> ClaudeOpts {
    let mut o = o.clone();
    o.extra = o.extra.agent_safe();
    o.extra.append_system_prompt.clear();
    o
}

/// Bereinigte freie Reste einer alten `claude_args`-Zeile (was keine Startoption ist).
pub fn legacy_rest(claude_args: &str) -> String {
    let c = crate::claude_opts::clean_legacy_report(claude_args, false);
    ClaudeOpts::split_legacy(&c.line).1
}

/// Warum ein Agent eine Vorlage mit diesem Profil nicht starten darf (MCP `create_session` mit `template`): Ein
/// Profil mit eigenem Programm, Argumenten oder Startbefehl ist nur erlaubt, wenn es das Standardprofil des Projekts ist
/// (das bekäme der Agent ohnehin); nie, wenn darin `dangerously` oder `bypassPermissions` vorkommt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentProfileIssue {
    /// Programm, Argumente oder Startbefehl enthalten `dangerously`/`bypassPermissions`.
    Dangerous,
    /// Eigenes Programm, Argumente oder Startbefehl – und nicht das Standardprofil des Projekts.
    Custom,
}

/// Prüfung für [`AgentProfileIssue`].
pub fn agent_profile_issue(program: &str, args: &str, startup: &str, is_project_default: bool) -> Option<AgentProfileIssue> {
    let fields = [program, args, startup];
    if fields.iter().any(|f| {
        let l = f.to_ascii_lowercase();
        l.contains("dangerously") || l.contains("bypasspermissions")
    }) {
        return Some(AgentProfileIssue::Dangerous);
    }
    (!is_project_default && fields.iter().any(|f| !f.trim().is_empty())).then_some(AgentProfileIssue::Custom)
}

/// Gruppen-Vorlage speichern (Ordner → „Als Vorlage speichern …“): eine Gruppen-Vorlage gleichen Namens (ohne
/// Groß-/Kleinschreibung) wird ersetzt; heißt eine Terminal-Vorlage so, bekommt die neue einen freien Namen
/// (`Name #2`), statt sie still zu überschreiben. Gibt den verwendeten Namen zurück.
pub fn save_group(list: &mut Vec<Template>, mut t: Template) -> String {
    let n = t.name.trim().to_string();
    let clash_terminal = list.iter().any(|x| x.is_terminal() && x.name.trim().eq_ignore_ascii_case(&n));
    t.name = if clash_terminal { unique_name(&n, list) } else { n };
    let name = t.name.clone();
    match list.iter().position(|x| !x.is_terminal() && x.name.trim().eq_ignore_ascii_case(&name)) {
        Some(i) => list[i] = t,
        None => list.push(t),
    }
    name
}

/// Name einer neuen Sitzung aus einer Vorlage: `base`, solange keine Sitzung so heißt, sonst `base #2`, `base #3` …
/// (die kleinste freie Nummer; Vergleich ohne Groß-/Kleinschreibung).
pub fn numbered_name(base: &str, taken: &[String]) -> String {
    let base = base.trim();
    let used = |n: &str| taken.iter().any(|t| t.trim().eq_ignore_ascii_case(n));
    if !used(base) {
        return base.to_string();
    }
    (2u32..).map(|i| format!("{base} #{i}")).find(|n| !used(n)).unwrap_or_else(|| base.to_string())
}

/// Vorlage per Name finden: zuerst genau, dann ohne Groß-/Kleinschreibung (Ränder ignoriert).
pub fn find<'a>(list: &'a [Template], name: &str) -> Option<(usize, &'a Template)> {
    let n = name.trim();
    list.iter().enumerate().find(|(_, t)| t.name.trim() == n).or_else(|| list.iter().enumerate().find(|(_, t)| t.name.trim().eq_ignore_ascii_case(n)))
}

/// Freier Name für eine neue Vorlage (Duplizieren): `base`, sonst `base #2` …
pub fn unique_name(base: &str, list: &[Template]) -> String {
    let names: Vec<String> = list.iter().map(|t| t.name.clone()).collect();
    numbered_name(base, &names)
}

pub fn load(dir: &std::path::Path) -> Vec<Template> {
    crate::state::load_json::<Vec<Template>>(&dir.join("templates.json")).unwrap_or_default()
}

pub fn save(dir: &std::path::Path, t: &[Template]) {
    crate::state::save_json(&dir.join("templates.json"), &t.to_vec());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude_opts::ExtraOpts;

    #[test]
    fn old_templates_load_and_continue_round_trips() {
        // Vorlage von 2.3.1 ohne `claude_continue`
        let old: Vec<Template> = serde_json::from_str(r#"[{"name":"A","grid":true,"items":[{"name":"x","shell_id":"pwsh","cwd":"C:\\a","claude":true,"claude_args":"--model opus","profile":0,"group":"","prompt":""}]}]"#).unwrap();
        assert_eq!(old[0].items[0].claude_continue, None);
        let s = serde_json::to_string(&old).unwrap();
        assert!(!s.contains("claude_continue"), "{s}");
        let it = TplItem { claude_continue: Some(false), ..TplItem::default() };
        let s = serde_json::to_string(&it).unwrap();
        assert!(s.contains(r#""claude_continue":false"#), "{s}");
        let back: TplItem = serde_json::from_str(&s).unwrap();
        assert_eq!(back.claude_continue, Some(false));
    }

    #[test]
    fn old_files_load_as_group_templates_and_save_without_new_fields() {
        let old: Vec<Template> = serde_json::from_str(r#"[{"name":"A","grid":false,"items":[{"name":"x","shell_id":"cmd","cwd":"C:\\a","claude":false,"claude_args":"","profile":3,"group":"g","prompt":""}]}]"#).unwrap();
        assert!(!old[0].terminal && !old[0].is_terminal(), "alte Vorlagen sind Gruppen-Vorlagen");
        let it = &old[0].items[0];
        assert_eq!((it.color, it.tags.len(), it.notes.as_str(), it.claude_opts.is_empty(), it.continue_last), (None, 0, "", true, false));
        let s = serde_json::to_string(&old).unwrap();
        for k in ["terminal", "color", "tags", "notes", "claude_opts", "continue_last", "model"] {
            assert!(!s.contains(k), "{k} nicht geschrieben: {s}");
        }
    }

    #[test]
    fn terminal_templates_round_trip_with_all_fields() {
        let item = TplItem {
            name: "API".into(),
            shell_id: "wsl:Ubuntu".into(),
            cwd: "/home/me/api".into(),
            claude: true,
            profile: 4,
            group: "Work/API".into(),
            prompt: "hi".into(),
            claude_continue: Some(false),
            color: Some([1, 2, 3]),
            tags: vec!["web".into()],
            notes: "note".into(),
            claude_opts: ClaudeOpts { model: "opus".into(), effort: "high".into(), ..ClaudeOpts::default() },
            continue_last: true,
            ..TplItem::default()
        };
        let t = Template::terminal(" API ", item.clone());
        let s = serde_json::to_string(&vec![t.clone()]).unwrap();
        let back: Vec<Template> = serde_json::from_str(&s).unwrap();
        assert_eq!(back[0], Template { name: "API".into(), ..t });
        assert!(back[0].is_terminal());
        assert_eq!(back[0].item(), Some(&item));
        // Startoptionen auf derselben Ebene wie in state.json
        assert!(s.contains(r#""claude_opts":{"model":"opus","effort":"high"}"#), "{s}");
    }

    #[test]
    fn wrong_types_in_new_fields_do_not_lose_the_file() {
        let v: Vec<Template> = serde_json::from_str(r#"[{"name":"T","terminal":"yes","items":[{"name":"x","color":"red","tags":"a","notes":5,"continue_last":"no","claude_opts":7}]},{"name":"B","items":[]}]"#).unwrap();
        assert_eq!(v.len(), 2);
        let it = &v[0].items[0];
        assert!(!v[0].terminal && it.color.is_none() && it.tags.is_empty() && it.notes.is_empty() && !it.continue_last && it.claude_opts.is_empty());
        let v: Vec<Template> = serde_json::from_str(r#"[{"name":"T","items":[{"color":[300,0,0]}]}]"#).unwrap();
        assert_eq!(v[0].items[0].color, None, "Werte außerhalb 0–255");
    }

    #[test]
    fn bypass_is_never_stored_in_a_template() {
        let it = TplItem { claude_opts: ClaudeOpts { permission_mode: "bypassPermissions".into(), model: "opus".into(), ..ClaudeOpts::default() }, ..TplItem::default() };
        let s = serde_json::to_string(&it).unwrap();
        assert!(!s.contains("bypass"), "{s}");
        assert!(s.contains("opus"), "{s}");
        assert_eq!(it.opts().permission_mode, "", "auch beim Start nicht");
    }

    #[test]
    fn counter_names_count_up_from_two() {
        let taken = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(numbered_name("API", &taken(&[])), "API");
        assert_eq!(numbered_name("API", &taken(&["Other"])), "API");
        assert_eq!(numbered_name("API", &taken(&["API"])), "API #2");
        assert_eq!(numbered_name("API", &taken(&["api", "API #2"])), "API #3");
        // Lücken werden gefüllt: nach dem Schließen von „API #2“ kommt es wieder.
        assert_eq!(numbered_name("API", &taken(&["API", "API #3"])), "API #2");
        assert_eq!(numbered_name(" API ", &taken(&["API"])), "API #2");
    }

    #[test]
    fn find_and_unique_names() {
        let list = vec![Template { name: "API".into(), ..Template::default() }, Template { name: "api".into(), ..Template::default() }, Template { name: "Web".into(), ..Template::default() }];
        assert_eq!(find(&list, "api").map(|x| x.0), Some(1), "genauer Treffer vor dem ohne Groß-/Kleinschreibung");
        assert_eq!(find(&list, " WEB ").map(|x| x.0), Some(2));
        assert!(find(&list, "nope").is_none());
        assert_eq!(unique_name("Web", &list), "Web #2");
        assert_eq!(unique_name("New", &list), "New");
    }

    #[test]
    fn session_name_and_search() {
        let mut t = Template::terminal("Backend", TplItem { cwd: r"C:\src\api".into(), tags: vec!["Rust".into()], ..TplItem::default() });
        assert_eq!(t.session_name(), "Backend", "ohne Sitzungsnamen der Name der Vorlage");
        t.items[0].name = "API".into();
        assert_eq!(t.session_name(), "API");
        assert!(t.matches("back"));
        assert!(t.matches("api #rust"));
        assert!(t.matches(r"src\api"));
        assert!(!t.matches("web"));
        assert!(t.matches(""));
        let g = Template { name: "G".into(), items: vec![TplItem::default(), TplItem::default()], terminal: true, ..Template::default() };
        assert!(!g.is_terminal() && g.item().is_none(), "mehr als eine Sitzung ist keine Terminal-Vorlage");
    }

    #[test]
    fn tags_are_cleaned() {
        assert_eq!(TplItem::clean_tags(&[" #web ".into(), "".into(), "Web".into(), "api".into()]), vec!["web".to_string(), "api".to_string()]);
    }

    #[test]
    fn legacy_args_become_options() {
        let it = TplItem { claude_args: "--model opus --permission-mode bypassPermissions --add-dir C:\\x".into(), ..TplItem::default() };
        let o = it.opts();
        assert_eq!(o.model, "opus");
        assert_eq!(o.permission_mode, "", "nie bypassPermissions");
        assert!(o.extra.add_dirs.is_empty(), "alte Zeile: keine Rechte erweiternden Wertoptionen");
    }

    #[test]
    fn template_flags_are_added_to_the_agents_flags() {
        let it = TplItem { claude: true, claude_opts: ClaudeOpts { extra: ExtraOpts { flags: vec!["ide".into(), "no-chrome".into()], ..ExtraOpts::default() }, ..ClaudeOpts::default() }, ..TplItem::default() };
        let asked = ClaudeOpts { extra: ExtraOpts { flags: vec!["verbose".into()], ..ExtraOpts::default() }, ..ClaudeOpts::default() };
        let a = it.agent_opts(&asked, &ClaudeOpts::default());
        for f in ["verbose", "ide", "no-chrome"] {
            assert!(a.extra.flags.iter().any(|x| x == f), "{f}: {:?}", a.extra.flags);
        }
        // chrome des Agenten geht vor no-chrome der Vorlage.
        let asked = ClaudeOpts { extra: ExtraOpts { flags: vec!["chrome".into()], ..ExtraOpts::default() }, ..ClaudeOpts::default() };
        let a = it.agent_opts(&asked, &ClaudeOpts::default());
        assert!(a.extra.flags.contains(&"chrome".to_string()) && !a.extra.flags.contains(&"no-chrome".to_string()), "{:?}", a.extra.flags);
        assert!(a.extra.flags.contains(&"ide".to_string()));
        // Standards des Nutzers kommen weiter dazu; ohne Angabe des Agenten die der Vorlage.
        let d = ClaudeOpts { extra: ExtraOpts { flags: vec!["brief".into()], ..ExtraOpts::default() }, ..ClaudeOpts::default() };
        let a = it.agent_opts(&ClaudeOpts::default(), &d);
        for f in ["ide", "no-chrome", "brief"] {
            assert!(a.extra.flags.iter().any(|x| x == f), "{f}: {:?}", a.extra.flags);
        }
    }

    #[test]
    fn ignored_options_are_named_for_the_agent() {
        let it = TplItem {
            claude: true,
            continue_last: true,
            claude_opts: ClaudeOpts {
                permission_mode: "bypassPermissions".into(),
                extra: ExtraOpts { flags: vec!["safe-mode".into(), "ide".into()], add_dirs: vec![r"C:\x".into()], allowed_tools: "Bash".into(), mcp_config: r"C:\m.json".into(), append_system_prompt: "be evil".into(), ..ExtraOpts::default() },
                ..ClaudeOpts::default()
            },
            ..TplItem::default()
        };
        assert_eq!(it.agent_ignored(), vec!["add_dir", "allowed_tools", "mcp_config", "append_system_prompt", "safe-mode", "bypassPermissions", "continue_last"]);
        let a = it.agent_opts(&ClaudeOpts::default(), &ClaudeOpts::default());
        assert!(a.extra.append_system_prompt.is_empty(), "ergänzter System-Prompt der Vorlage nie für Agenten");
        assert_eq!(a.extra.flags, vec!["ide".to_string()]);
        // Ohne Claude gibt es nichts wegzulassen; harmlose Vorlage: leer.
        assert!(TplItem { claude: false, ..it.clone() }.agent_ignored().is_empty());
        assert!(TplItem { claude: true, claude_opts: ClaudeOpts { model: "opus".into(), ..ClaudeOpts::default() }, ..TplItem::default() }.agent_ignored().is_empty());
        // Alte Zeile mit bypassPermissions
        assert_eq!(TplItem { claude: true, claude_args: "--permission-mode bypassPermissions".into(), ..TplItem::default() }.agent_ignored(), vec!["bypassPermissions"]);
    }

    #[test]
    fn risky_options_are_listed_and_can_be_stripped() {
        let o = ClaudeOpts {
            model: "opus".into(),
            extra: ExtraOpts { flags: vec!["safe-mode".into(), "ide".into()], add_dirs: vec![r"C:\a".into(), r"C:\b".into()], allowed_tools: "Bash".into(), mcp_config: r"C:\m.json".into(), append_system_prompt: "hi".into(), disallowed_tools: "WebFetch".into(), ..ExtraOpts::default() },
            ..ClaudeOpts::default()
        };
        let it = TplItem { name: "S".into(), claude: true, claude_opts: o.clone(), ..TplItem::default() };
        let r = it.risky();
        let kinds: Vec<Risk> = r.iter().map(|x| x.0).collect();
        assert_eq!(kinds, vec![Risk::AddDir, Risk::AddDir, Risk::AllowedTools, Risk::McpConfig, Risk::SafeMode, Risk::AppendSystemPrompt]);
        assert_eq!(r[1].1, r"C:\b");
        assert!(TplItem { claude: false, ..it.clone() }.risky().is_empty(), "ohne Claude keine Rückfrage");
        assert!(TplItem { claude: true, claude_opts: ClaudeOpts { extra: ExtraOpts { disallowed_tools: "X".into(), flags: vec!["ide".into()], ..ExtraOpts::default() }, ..ClaudeOpts::default() }, ..TplItem::default() }.risky().is_empty(), "einschränkende Optionen sind harmlos");
        let s = strip_risky(&o);
        assert_eq!((s.model.as_str(), s.extra.disallowed_tools.as_str()), ("opus", "WebFetch"));
        assert!(s.extra.add_dirs.is_empty() && s.extra.allowed_tools.is_empty() && s.extra.mcp_config.is_empty() && s.extra.append_system_prompt.is_empty());
        assert_eq!(s.extra.flags, vec!["ide".to_string()]);
        // Gruppen-Vorlage: je Sitzung
        let g = Template { name: "G".into(), items: vec![TplItem { name: "a".into(), ..TplItem::default() }, it.clone()], ..Template::default() };
        let gr = g.risky();
        assert_eq!(gr.len(), 6);
        assert!(gr.iter().all(|x| x.0 == "S"));
        // Alte Zeile: --safe-mode und --append-system-prompt kommen aus ihr, Rechte erweiternde Wertoptionen nie.
        let old = TplItem { claude: true, claude_args: r"--safe-mode --append-system-prompt hi --add-dir C:\x".into(), ..TplItem::default() };
        let kinds: Vec<Risk> = old.risky().iter().map(|x| x.0).collect();
        assert_eq!(kinds, vec![Risk::SafeMode, Risk::AppendSystemPrompt]);
    }

    #[test]
    fn free_legacy_arguments_are_kept() {
        let it = TplItem { claude: true, claude_args: "--model opus --foo bar --dangerously-skip-permissions --resume 123".into(), ..TplItem::default() };
        let (o, rest) = it.opts_and_rest();
        assert_eq!(o.model, "opus");
        assert_eq!(rest, "--foo bar", "bereinigt, ohne Optionen");
        assert_eq!(legacy_rest(""), "");
    }

    #[test]
    fn custom_profiles_are_checked_for_agents() {
        assert_eq!(agent_profile_issue("", "", "", false), None);
        assert_eq!(agent_profile_issue(r"C:\x\tool.exe", "", "", false), Some(AgentProfileIssue::Custom));
        assert_eq!(agent_profile_issue("", "-x", "", false), Some(AgentProfileIssue::Custom));
        assert_eq!(agent_profile_issue("", "", "claude", false), Some(AgentProfileIssue::Custom));
        assert_eq!(agent_profile_issue(r"C:\x\tool.exe", "-a", "make", true), None, "Standardprofil des Projekts");
        assert_eq!(agent_profile_issue("", "", "claude --dangerously-skip-permissions", true), Some(AgentProfileIssue::Dangerous));
        assert_eq!(agent_profile_issue("claude.exe", "--permission-mode BypassPermissions", "", true), Some(AgentProfileIssue::Dangerous));
    }

    #[test]
    fn group_templates_replace_groups_but_never_terminal_templates() {
        let term = Template::terminal("API", TplItem::default());
        let group = |n: &str, k: usize| Template { name: n.into(), items: vec![TplItem::default(); k], ..Template::default() };
        let mut list = vec![term.clone(), group("Web", 1)];
        assert_eq!(save_group(&mut list, group("web", 2)), "web", "ohne Groß-/Kleinschreibung ersetzt");
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].items.len(), 2);
        assert_eq!(save_group(&mut list, group("api", 3)), "api #2", "Terminal-Vorlage bleibt");
        assert_eq!(list.len(), 3);
        assert_eq!(list[0], term);
        assert_eq!(save_group(&mut list, group("New", 1)), "New");
        assert_eq!(list.len(), 4);
    }

    #[test]
    fn agents_never_get_risky_options_from_a_template() {
        let risky = ClaudeOpts {
            model: "opus".into(),
            permission_mode: "bypassPermissions".into(),
            extra: ExtraOpts {
                flags: vec!["ide".into(), "safe-mode".into()],
                add_dirs: vec![r"C:\x".into()],
                allowed_tools: "Bash".into(),
                mcp_config: r"C:\m.json".into(),
                disallowed_tools: "WebFetch".into(),
                ..ExtraOpts::default()
            },
            ..ClaudeOpts::default()
        };
        let it = TplItem { claude: true, claude_opts: risky.clone(), ..TplItem::default() };
        // Für den Nutzer bleiben die Wertoptionen (er hat sie selbst in die Vorlage geschrieben), bypassPermissions nie.
        let user = it.opts();
        assert_eq!(user.extra.add_dirs, vec![r"C:\x".to_string()]);
        assert_eq!(user.permission_mode, "");
        assert!(user.extra.flags.contains(&"safe-mode".to_string()));
        // Agent: nichts davon.
        let a = it.agent_opts(&ClaudeOpts::default(), &ClaudeOpts::default());
        assert_eq!(a.model, "opus");
        assert_eq!(a.permission_mode, "default", "ausdrücklich default, nie bypass");
        assert!(a.extra.add_dirs.is_empty() && a.extra.allowed_tools.is_empty() && a.extra.mcp_config.is_empty());
        assert_eq!(a.extra.flags, vec!["ide".to_string()], "kein --safe-mode");
        assert_eq!(a.extra.disallowed_tools, "WebFetch", "einschränkende Optionen bleiben");
        // Angaben des Agenten gehen vor.
        let asked = ClaudeOpts { model: "haiku".into(), permission_mode: "plan".into(), ..ClaudeOpts::default() };
        let a = it.agent_opts(&asked, &ClaudeOpts::default());
        assert_eq!((a.model.as_str(), a.permission_mode.as_str()), ("haiku", "plan"));
        // Riskante Standards des Nutzers gelten für Agenten ebenfalls nicht.
        let defaults = ClaudeOpts { extra: ExtraOpts { add_dirs: vec![r"C:\d".into()], flags: vec!["verbose".into()], ..ExtraOpts::default() }, ..ClaudeOpts::default() };
        let a = TplItem { claude: true, ..TplItem::default() }.agent_opts(&ClaudeOpts::default(), &defaults);
        assert!(a.extra.add_dirs.is_empty());
        assert_eq!(a.extra.flags, vec!["verbose".to_string()]);
    }
}
