//! Startoptionen für Claude Code (#161): Modell, Effort, Permission-Mode und Agent.
//!
//! - Nur geprüfte Werte, keine freien Argumente: Modell/Agent bestehen aus `[A-Za-z0-9._:-]` (höchstens
//!   [`MAX_NAME`] Zeichen, erstes Zeichen Buchstabe oder Ziffer – sonst könnte ein Wert wie eine Option aussehen),
//!   Effort und Permission-Mode stammen aus festen Listen.
//! - `bypassPermissions` gibt es nur, wenn der Nutzer es im Dialog „Neue Sitzung“ ausdrücklich für diese eine Sitzung
//!   wählt: nie als gespeicherter Standard, nie als „letzte Wahl“, nie über MCP, Kontextmenü, `--claude`, IPC, Vorlagen
//!   oder Verlauf ([`resolve_new`] mit `allow_bypass = false`).
//! - `manual` (neuere Claude-Code-Versionen) wird zu `default` (versteht jede Version).
//! - Agenten-Sitzungen ohne Permission-Mode bekommen ausdrücklich `default` ([`ClaudeOpts::for_agent`]); sonst gälte
//!   `permissions.defaultMode` aus `~/.claude/settings.json` des Nutzers, und das kann `bypassPermissions` sein.
//! - Gespeichert in `settings.json` (Standards, letzte Wahl; nur gültige Werte ohne `bypassPermissions`,
//!   [`ser_user_opts`]) und je Sitzung in `state.json`; beim Lesen werden ungültige Werte still verworfen.
//!
//! Kennt kein egui.

use crate::quote::{quote, Kind};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Höchstlänge von Modell- und Agentenname.
pub const MAX_NAME: usize = 100;
/// Erlaubte Effort-Stufen (`--effort`).
pub const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
/// Permission-Modi, die auch ein Agent setzen darf (`--permission-mode`). `default` = bei allem fragen
/// (heißt in neueren Claude-Code-Versionen `manual`, die `default` weiter annehmen).
pub const PERMISSION_MODES: [&str; 5] = ["acceptEdits", "auto", "plan", "default", "dontAsk"];
/// Neuer Name von `default`; wird beim Lesen und Prüfen zu `default`.
pub const MANUAL: &str = "manual";
/// Nur vom Nutzer selbst wählbar, nie über MCP.
pub const BYPASS: &str = "bypassPermissions";
/// Vorschläge für das Modell in der Oberfläche (Claude-Code-Aliase).
pub const MODEL_ALIASES: [&str; 4] = ["fable", "opus", "sonnet", "haiku"];

/// Modell- oder Agentenname gültig?
pub fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_NAME
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b':' | b'-'))
}

pub fn valid_effort(s: &str) -> bool {
    EFFORTS.contains(&s)
}

/// Permission-Mode gültig? `bypassPermissions` nur mit `allow_bypass` (Wahl des Nutzers).
pub fn valid_permission_mode(s: &str, allow_bypass: bool) -> bool {
    PERMISSION_MODES.contains(&s) || (allow_bypass && s == BYPASS)
}

/// Die vier Optionen; ein leeres Feld heißt „Standard von Claude Code“ (Flag wird nicht gesetzt).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ClaudeOpts {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub effort: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub permission_mode: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub agent: String,
}

/// Fehler bei der Prüfung von MCP-Argumenten: (Deutsch, Englisch).
pub type OptError = (String, String);

impl ClaudeOpts {
    pub fn is_empty(&self) -> bool {
        self.model.is_empty() && self.effort.is_empty() && self.permission_mode.is_empty() && self.agent.is_empty()
    }

    /// Nur gültige Werte behalten (ungültige werden zu „Standard“); `manual` wird zu `default`.
    pub fn sanitized(&self, allow_bypass: bool) -> ClaudeOpts {
        let keep = |s: &str, ok: bool| if ok { s.to_string() } else { String::new() };
        let mode = if self.permission_mode == MANUAL { "default" } else { self.permission_mode.as_str() };
        ClaudeOpts {
            model: keep(&self.model, valid_name(&self.model)),
            effort: keep(&self.effort, valid_effort(&self.effort)),
            permission_mode: keep(mode, valid_permission_mode(mode, allow_bypass)),
            agent: keep(&self.agent, valid_name(&self.agent)),
        }
    }

    /// Je Feld: eigener Wert, sonst der aus `fallback`.
    pub fn or(&self, fallback: &ClaudeOpts) -> ClaudeOpts {
        let pick = |a: &str, b: &str| if a.is_empty() { b.to_string() } else { a.to_string() };
        ClaudeOpts {
            model: pick(&self.model, &fallback.model),
            effort: pick(&self.effort, &fallback.effort),
            permission_mode: pick(&self.permission_mode, &fallback.permission_mode),
            agent: pick(&self.agent, &fallback.agent),
        }
    }

    /// Optionen einer über MCP angelegten Sitzung: Angaben des Agenten, sonst die Standards des Nutzers – nie
    /// `bypassPermissions`. Ohne Permission-Mode ausdrücklich `default`, damit nicht `permissions.defaultMode` aus
    /// `~/.claude/settings.json` gilt (das könnte `bypassPermissions` sein).
    pub fn for_agent(&self, defaults: &ClaudeOpts) -> ClaudeOpts {
        let mut o = self.or(&defaults.sanitized(false)).sanitized(false);
        if o.permission_mode.is_empty() {
            o.permission_mode = "default".into();
        }
        o
    }

    /// Kommandozeilen-Teil (mit führendem Leerzeichen, leer ohne Optionen). Ungültige Werte werden ausgelassen;
    /// gequotet wird für die Shell, in die die Zeile getippt wird.
    pub fn args(&self, kind: Kind) -> String {
        let o = self.sanitized(true);
        let mut out = String::new();
        for (flag, v) in [("--model", &o.model), ("--effort", &o.effort), ("--permission-mode", &o.permission_mode), ("--agent", &o.agent)] {
            if !v.is_empty() {
                out += &format!(" {flag} {}", quote_value(kind, v));
            }
        }
        out
    }

    /// Prüft die optionalen Parameter `model`/`effort`/`permission_mode`/`agent` von `create_session`.
    /// Fehlende oder leere Werte bleiben leer; alles andere muss gültig sein (`bypassPermissions` nie).
    pub fn from_mcp(args: &Value) -> Result<ClaudeOpts, OptError> {
        let get = |k: &str| -> Result<String, OptError> {
            match args.get(k) {
                None | Some(Value::Null) => Ok(String::new()),
                Some(Value::String(s)) if k == "permission_mode" && s.trim() == MANUAL => Ok("default".to_string()),
                Some(Value::String(s)) => Ok(s.trim().to_string()),
                Some(_) => Err((format!("{k} muss ein String sein."), format!("{k} must be a string."))),
            }
        };
        let o = ClaudeOpts { model: get("model")?, effort: get("effort")?, permission_mode: get("permission_mode")?, agent: get("agent")? };
        for (k, v) in [("model", &o.model), ("agent", &o.agent)] {
            if !v.is_empty() && !valid_name(v) {
                return Err((
                    format!("{k}: nur Buchstaben, Ziffern, . _ : - (beginnend mit Buchstabe oder Ziffer), höchstens {MAX_NAME} Zeichen."),
                    format!("{k}: only letters, digits, . _ : - (starting with a letter or digit), at most {MAX_NAME} characters."),
                ));
            }
        }
        if !o.effort.is_empty() && !valid_effort(&o.effort) {
            return Err((format!("effort: eins von {}.", EFFORTS.join(", ")), format!("effort: one of {}.", EFFORTS.join(", "))));
        }
        if o.permission_mode == BYPASS {
            return Err((
                "permission_mode: bypassPermissions ist über MCP nie erlaubt – nur der Nutzer kann es selbst in Paneldeck wählen.".to_string(),
                "permission_mode: bypassPermissions is never allowed via MCP – only the user can choose it in Paneldeck.".to_string(),
            ));
        }
        if !o.permission_mode.is_empty() && !valid_permission_mode(&o.permission_mode, false) {
            return Err((
                format!("permission_mode: eins von {}.", PERMISSION_MODES.join(", ")),
                format!("permission_mode: one of {}.", PERMISSION_MODES.join(", ")),
            ));
        }
        Ok(o)
    }

    /// Optionen aus einer alten `claude_args`-Zeile (vor 2.3.0: „--model x --permission-mode y“) herauslösen.
    /// Rückgabe: erkannte Optionen und die übrigen Token (unverändert, Reihenfolge erhalten).
    pub fn split_legacy(args: &str) -> (ClaudeOpts, String) {
        let mut o = ClaudeOpts::default();
        let mut rest = Vec::new();
        let toks: Vec<&str> = args.split_whitespace().collect();
        let mut i = 0;
        while i < toks.len() {
            // Nur Werte übernehmen, die auch hier gültig sind (z. B. bleibt „--model opus[1m]“ als freie Zeile erhalten).
            let (slot, ok): (Option<&mut String>, fn(&str) -> bool) = match toks[i] {
                "--model" => (Some(&mut o.model), valid_name),
                "--effort" => (Some(&mut o.effort), valid_effort),
                "--permission-mode" => (Some(&mut o.permission_mode), |s| s == MANUAL || valid_permission_mode(s, true)),
                "--agent" => (Some(&mut o.agent), valid_name),
                _ => (None, |_| false),
            };
            match (slot, toks.get(i + 1)) {
                (Some(slot), Some(v)) if ok(v) && slot.is_empty() => {
                    *slot = v.to_string();
                    i += 2;
                }
                _ => {
                    rest.push(toks[i]);
                    i += 1;
                }
            }
        }
        (o.sanitized(true), rest.join(" "))
    }

    /// Als `claude_args`-Zeile (für Vorlagen, deren Dateiformat nur diese Zeile kennt).
    pub fn to_legacy(&self) -> String {
        self.args(Kind::Cmd).trim().to_string()
    }

    /// Antwortfelder für MCP (`null`, wenn Paneldeck die Option nicht setzt).
    pub fn to_json(&self) -> Value {
        let v = |s: &str| if s.is_empty() { Value::Null } else { Value::String(s.to_string()) };
        serde_json::json!({"model": v(&self.model), "effort": v(&self.effort), "permission_mode": v(&self.permission_mode), "agent": v(&self.agent)})
    }

    /// Kurzzeile für Tooltips, z. B. „opus · high · plan · Agent reviewer“ (leer ohne Optionen).
    pub fn summary(&self, agent_label: &str) -> String {
        let mut parts: Vec<String> = [&self.model, &self.effort, &self.permission_mode].iter().filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
        if !self.agent.is_empty() {
            parts.push(format!("{agent_label} {}", self.agent));
        }
        parts.join(" · ")
    }
}

/// Freie Reste alter `claude_args` (nach [`clean_legacy`]) für die Zeile: Flags wie sie sind (nur `-`, Buchstaben,
/// Ziffern), Werte gequotet wie die Startoptionen, damit `@`/`[]` in PowerShell bzw. POSIX-Shells nichts auslösen.
pub fn legacy_args(rest: &str, kind: Kind) -> String {
    rest.split_whitespace().map(|t| if t.starts_with('-') { t.to_string() } else { quote_value(kind, t) }).collect::<Vec<_>>().join(" ")
}

/// Alte `claude_args` bereinigen: nur harmlose Token ([`crate::quote::sanitize_claude_args`]) und nie ein
/// `--dangerously-*`/`--allow-dangerously-*`-Flag (würde Freigaben abschalten).
pub fn clean_legacy(args: &str) -> String {
    crate::quote::sanitize_claude_args(args).split_whitespace().filter(|t| !t.to_ascii_lowercase().contains("dangerously")).collect::<Vec<_>>().join(" ")
}

/// Optionen einer neuen Sitzung (alle Wege laufen hier durch): `explicit` (Dialog, MCP, Vorlage, Duplikat) oder sonst
/// die Standards des Nutzers, ergänzt um Optionen aus alten `claude_args`. `bypassPermissions` bleibt nur mit
/// `allow_bypass` (ausdrückliche Wahl im Dialog „Neue Sitzung“). Rückgabe: Optionen und bereinigte freie Reste.
pub fn resolve_new(explicit: Option<&ClaudeOpts>, allow_bypass: bool, defaults: &ClaudeOpts, legacy_args: &str) -> (ClaudeOpts, String) {
    let (legacy, rest) = ClaudeOpts::split_legacy(&clean_legacy(legacy_args));
    // Ausdrückliche Angabe vor alter Zeile; eine alte Zeile (Vorlage) vor den Standards des Nutzers.
    let o = match explicit {
        Some(e) => e.or(&legacy),
        None => legacy.or(&defaults.sanitized(false)),
    };
    (o.sanitized(allow_bypass), rest)
}

/// Gespeicherte Standards/letzte Wahl in `settings.json`: nur gültige Werte, nie `bypassPermissions`.
pub fn ser_user_opts<S: serde::Serializer>(o: &ClaudeOpts, s: S) -> Result<S::Ok, S::Error> {
    o.sanitized(false).serialize(s)
}

/// Wie [`ser_user_opts`] für `Option`.
pub fn ser_user_opts_opt<S: serde::Serializer>(o: &Option<ClaudeOpts>, s: S) -> Result<S::Ok, S::Error> {
    o.as_ref().map(|o| o.sanitized(false)).serialize(s)
}

/// Werte sind geprüft, PowerShell wertet aber zahlähnliche Wörter (`1e5`, `0x10`) als Zahl aus: dort immer in
/// einfache Anführungszeichen (wörtlich). POSIX-Shells ebenfalls; cmd braucht für diesen Zeichenvorrat keine.
fn quote_value(kind: Kind, v: &str) -> String {
    match kind {
        Kind::Cmd => v.to_string(),
        Kind::Pwsh | Kind::Posix => quote(kind, v),
    }
}

/// Tolerant lesen: fehlende, falsch getypte oder ungültige Felder ergeben „Standard“, nie einen Ladefehler.
/// `bypassPermissions` bleibt erhalten (nur der Nutzer selbst kann es gesetzt haben – Agenten nie, siehe `from_mcp`).
impl<'de> Deserialize<'de> for ClaudeOpts {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
        Ok(ClaudeOpts { model: s("model"), effort: s("effort"), permission_mode: s("permission_mode"), agent: s("agent") }.sanitized(true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn names_are_validated() {
        for ok in ["opus", "fable", "claude-opus-4-5", "claude-sonnet-4-5-20250929", "us.anthropic.claude:v1", "my_agent", "a"] {
            assert!(valid_name(ok), "{ok}");
        }
        let long = "a".repeat(MAX_NAME + 1);
        for bad in ["", "-x", ".hidden", "_x", "a b", "a;b", "opus[1m]", "$(id)", "a`b", "a'b", "a\"b", "a%PATH%", "a&b", "a|b", "ä", "a/b", long.as_str()] {
            assert!(!valid_name(bad), "{bad}");
        }
        assert!(valid_name(&"a".repeat(MAX_NAME)));
    }

    #[test]
    fn manual_maps_to_default() {
        assert_eq!(ClaudeOpts::from_mcp(&json!({"permission_mode": "manual"})).unwrap().permission_mode, "default");
        assert_eq!(ClaudeOpts::from_mcp(&json!({"permission_mode": "default"})).unwrap().permission_mode, "default");
        let o: ClaudeOpts = serde_json::from_value(json!({"permission_mode": "manual"})).unwrap();
        assert_eq!(o.permission_mode, "default");
        assert_eq!(ClaudeOpts::split_legacy("--permission-mode manual").0.permission_mode, "default");
        assert_eq!(ClaudeOpts { permission_mode: "manual".into(), ..Default::default() }.args(Kind::Cmd), " --permission-mode default");
    }

    #[test]
    fn agent_sessions_get_explicit_default_mode() {
        assert_eq!(ClaudeOpts::default().for_agent(&ClaudeOpts::default()).permission_mode, "default");
        let d = ClaudeOpts { permission_mode: "plan".into(), ..Default::default() };
        assert_eq!(ClaudeOpts::default().for_agent(&d).permission_mode, "plan", "Standard des Nutzers ohne bypass gilt");
        assert_eq!(ClaudeOpts { permission_mode: "auto".into(), ..Default::default() }.for_agent(&d).permission_mode, "auto");
    }

    #[test]
    fn bypass_only_from_the_explicit_dialog_choice() {
        let bypass = ClaudeOpts { model: "opus".into(), permission_mode: BYPASS.into(), ..Default::default() };
        // Dialog „Neue Sitzung“, ausdrücklich gewählt: bleibt.
        assert_eq!(resolve_new(Some(&bypass), true, &ClaudeOpts::default(), "").0.permission_mode, BYPASS);
        // Standard der Einstellungen (Kontextmenü, --claude, IPC open, Verlauf, Ticket-Schnellstart, alte Vorlagen): nie.
        let (o, _) = resolve_new(None, false, &bypass, "");
        assert_eq!((o.model.as_str(), o.permission_mode.as_str()), ("opus", ""));
        // MCP, Vorlage, Duplikat: ausdrückliche Angabe, aber ohne Erlaubnis.
        assert_eq!(resolve_new(Some(&bypass), false, &ClaudeOpts::default(), "").0.permission_mode, "");
        // Alte claude_args (Vorlage, Duplikat einer alten Sitzung): weder bypass noch --dangerously-*.
        let (o, rest) = resolve_new(None, false, &ClaudeOpts::default(), "--permission-mode bypassPermissions --dangerously-skip-permissions --allow-dangerously-skip-permissions --verbose");
        assert_eq!((o.permission_mode.as_str(), rest.as_str()), ("", "--verbose"));
        // Alte Vorlage: ihre Optionen vor den Standards, fehlende aus den Standards; ohne Optionen nur die Standards.
        let d = ClaudeOpts { model: "sonnet".into(), effort: "low".into(), ..Default::default() };
        let (o, rest) = resolve_new(None, false, &d, "--model opus");
        assert_eq!((o.model.as_str(), o.effort.as_str(), rest.as_str()), ("opus", "low", ""));
        assert_eq!(resolve_new(None, false, &d, "").0, d);
        // Duplikat: Optionen der Quelle vor ihrer alten Zeile.
        let src = ClaudeOpts { model: "haiku".into(), ..Default::default() };
        assert_eq!(resolve_new(Some(&src), false, &d, "--model opus --effort max").0, ClaudeOpts { model: "haiku".into(), effort: "max".into(), ..Default::default() });
        // Agenten-Weg
        assert_eq!(resolve_new(Some(&ClaudeOpts::default().for_agent(&bypass)), false, &bypass, "").0.permission_mode, "default");
    }

    #[test]
    fn user_opts_never_persist_bypass_or_invalid_names() {
        let o = ClaudeOpts { model: "opus; calc".into(), effort: "high".into(), permission_mode: BYPASS.into(), agent: "ok".into() };
        let mut buf = Vec::new();
        ser_user_opts(&o, &mut serde_json::Serializer::new(&mut buf)).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), r#"{"effort":"high","agent":"ok"}"#);
    }

    #[test]
    fn legacy_leftovers_are_quoted_per_shell() {
        assert_eq!(legacy_args("--model opus[1m] --x @a", Kind::Pwsh), "--model 'opus[1m]' --x '@a'");
        assert_eq!(legacy_args("--model opus[1m]", Kind::Posix), "--model 'opus[1m]'");
        assert_eq!(legacy_args("--model opus[1m]", Kind::Cmd), "--model opus[1m]");
        assert_eq!(clean_legacy("--dangerously-skip-permissions --model x;y --ok"), "--model --ok");
    }

    #[test]
    fn efforts_and_modes() {
        for e in EFFORTS {
            assert!(valid_effort(e));
        }
        assert!(!valid_effort("High") && !valid_effort("ultra") && !valid_effort(""));
        for m in PERMISSION_MODES {
            assert!(valid_permission_mode(m, false));
        }
        assert!(!valid_permission_mode(BYPASS, false) && valid_permission_mode(BYPASS, true));
        assert!(!valid_permission_mode("manual", true) && !valid_permission_mode("acceptedits", true));
    }

    #[test]
    fn mcp_accepts_valid_values() {
        let o = ClaudeOpts::from_mcp(&json!({"model": " opus ", "effort": "xhigh", "permission_mode": "plan", "agent": "code-reviewer"})).unwrap();
        assert_eq!(o, ClaudeOpts { model: "opus".into(), effort: "xhigh".into(), permission_mode: "plan".into(), agent: "code-reviewer".into() });
        assert!(ClaudeOpts::from_mcp(&json!({})).unwrap().is_empty());
        assert!(ClaudeOpts::from_mcp(&json!({"model": "", "effort": null})).unwrap().is_empty());
        for m in PERMISSION_MODES {
            assert_eq!(ClaudeOpts::from_mcp(&json!({"permission_mode": m})).unwrap().permission_mode, m);
        }
    }

    #[test]
    fn mcp_rejects_invalid_values() {
        for bad in [
            json!({"model": "opus; rm -rf /"}),
            json!({"model": "--dangerously-skip-permissions"}),
            json!({"model": 5}),
            json!({"agent": "a b"}),
            json!({"agent": "x".repeat(101)}),
            json!({"effort": "ultra"}),
            json!({"permission_mode": "ask"}),
            json!({"permission_mode": "bypassPermissions"}),
        ] {
            assert!(ClaudeOpts::from_mcp(&bad).is_err(), "{bad}");
        }
        let (de, en) = ClaudeOpts::from_mcp(&json!({"permission_mode": "bypassPermissions"})).unwrap_err();
        assert!(en.contains("never allowed via MCP") && de.contains("nie erlaubt"));
    }

    #[test]
    fn agents_never_get_bypass_even_as_default() {
        let defaults = ClaudeOpts { model: "sonnet".into(), effort: "high".into(), permission_mode: BYPASS.into(), agent: String::new() };
        let o = ClaudeOpts::default().for_agent(&defaults);
        assert_eq!(o, ClaudeOpts { model: "sonnet".into(), effort: "high".into(), permission_mode: "default".into(), ..Default::default() });
        // Angaben des Agenten haben Vorrang vor den Standards.
        let o = ClaudeOpts { model: "haiku".into(), permission_mode: "plan".into(), ..Default::default() }.for_agent(&defaults);
        assert_eq!((o.model.as_str(), o.effort.as_str(), o.permission_mode.as_str()), ("haiku", "high", "plan"));
    }

    #[test]
    fn command_line_per_shell() {
        let o = ClaudeOpts { model: "opus".into(), effort: "high".into(), permission_mode: "acceptEdits".into(), agent: "reviewer".into() };
        assert_eq!(o.args(Kind::Pwsh), " --model 'opus' --effort 'high' --permission-mode 'acceptEdits' --agent 'reviewer'");
        assert_eq!(o.args(Kind::Posix), " --model 'opus' --effort 'high' --permission-mode 'acceptEdits' --agent 'reviewer'");
        assert_eq!(o.args(Kind::Cmd), " --model opus --effort high --permission-mode acceptEdits --agent reviewer");
        // PowerShell würde „1e5“ ungequotet als Zahl 100000 übergeben.
        assert_eq!(ClaudeOpts { model: "1e5".into(), ..Default::default() }.args(Kind::Pwsh), " --model '1e5'");
        assert_eq!(ClaudeOpts { model: "claude-opus-4-5:beta".into(), ..Default::default() }.args(Kind::Posix), " --model 'claude-opus-4-5:beta'");
        assert_eq!(ClaudeOpts::default().args(Kind::Pwsh), "");
        // Nur einzelne Felder; bypassPermissions (Wahl des Nutzers) wird übernommen.
        assert_eq!(ClaudeOpts { permission_mode: BYPASS.into(), ..Default::default() }.args(Kind::Cmd), " --permission-mode bypassPermissions");
    }

    #[test]
    fn command_line_drops_invalid_values() {
        // Direkt gesetzte (nicht über Deserialize geprüfte) Werte kommen nie ungeprüft in die Zeile.
        let o = ClaudeOpts { model: "x' ; calc; '".into(), effort: "ultra".into(), permission_mode: "ask".into(), agent: "$(id)".into() };
        for k in [Kind::Pwsh, Kind::Posix, Kind::Cmd] {
            assert_eq!(o.args(k), "", "{k:?}");
        }
    }

    #[test]
    fn serde_is_lenient_and_compact() {
        assert_eq!(serde_json::to_value(ClaudeOpts::default()).unwrap(), json!({}));
        let o = ClaudeOpts { model: "opus".into(), ..Default::default() };
        assert_eq!(serde_json::to_value(&o).unwrap(), json!({"model": "opus"}));
        let back: ClaudeOpts = serde_json::from_value(json!({"model": "opus", "effort": 7, "permission_mode": "bypassPermissions", "agent": "a b", "x": 1})).unwrap();
        assert_eq!(back, ClaudeOpts { model: "opus".into(), permission_mode: BYPASS.into(), ..Default::default() });
        let back: ClaudeOpts = serde_json::from_value(json!("kaputt")).unwrap();
        assert!(back.is_empty());
    }

    #[test]
    fn legacy_args_are_split() {
        let (o, rest) = ClaudeOpts::split_legacy("--model opus --permission-mode acceptEdits --verbose");
        assert_eq!((o.model.as_str(), o.permission_mode.as_str(), rest.as_str()), ("opus", "acceptEdits", "--verbose"));
        let (o, rest) = ClaudeOpts::split_legacy("--model --ok");
        assert!(o.is_empty());
        assert_eq!(rest, "--model --ok");
        // Hier ungültige Werte bleiben in der freien Zeile (alte Sitzungen verlieren nichts).
        let (o, rest) = ClaudeOpts::split_legacy("--model opus[1m] --permission-mode bypassPermissions");
        assert_eq!((o.model.as_str(), o.permission_mode.as_str(), rest.as_str()), ("", BYPASS, "--model opus[1m]"));
        let o = ClaudeOpts { model: "opus".into(), effort: "max".into(), permission_mode: "plan".into(), agent: "a".into() };
        assert_eq!(ClaudeOpts::split_legacy(&o.to_legacy()), (o, String::new()));
    }

    #[test]
    fn summary_and_json() {
        let o = ClaudeOpts { model: "opus".into(), effort: "high".into(), permission_mode: String::new(), agent: "rev".into() };
        assert_eq!(o.summary("Agent"), "opus · high · Agent rev");
        assert_eq!(o.to_json(), json!({"model": "opus", "effort": "high", "permission_mode": null, "agent": "rev"}));
        assert_eq!(ClaudeOpts::default().summary("Agent"), "");
    }
}
