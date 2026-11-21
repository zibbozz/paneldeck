//! Aufräumen der Integration außerhalb des Datenordners (#307): Autostart-Werte und Explorer-Kontextmenü (Registry unter
//! HKCU) sowie `paneldeck*`-Einträge in `~/.claude.json` – jeweils nur, was auf **diese** paneldeck.exe zeigt. Vor dem
//! Ändern wird gesichert: Registry-Werte als `.reg`-Datei (Doppelklick stellt sie wieder her), `~/.claude.json` als Kopie,
//! beides im Sicherungsordner (beim Zurücksetzen neben der ZIP-Sicherung, `Ctx::safety_dir` – der Sicherungsordner im
//! Datenordner wird dabei gelöscht).
//!
//! Die Registry liest und schreibt `platform::registry`; außerhalb von Windows meldet sie nichts, dann sind die Einträge
//! leer. Die Gegenstücke unter Linux/macOS (XDG-Autostart `.desktop`, Dateimanager-Aktion, LaunchAgent) kommen hier als
//! weitere Felder von [`State`] dazu, sobald die Plattform-Schicht sie anlegt.

use super::{Ctx, Item, ItemReport};
use crate::integration::{CONTEXT_KEYS, CONTEXT_PROBE, EDIT_CONTEXT_KEY, EDIT_CONTEXT_PROBE, RUN_KEY};
use crate::launch;
use crate::platform::registry::{self, RegOp};
use std::path::{Path, PathBuf};

/// Was von dieser .exe außerhalb des Datenordners eingetragen ist.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// Run-Werte (Name, Befehl), die auf diese .exe zeigen – Standardinstanz und Profile/Datenordner.
    pub run_values: Vec<(String, String)>,
    /// Kontextmenü zeigt auf diese .exe: gespeicherte Werte (Schlüssel, Wertname, Inhalt) für die Sicherung.
    pub context: Option<Vec<(String, Option<String>, String)>>,
    /// Explorer-Eintrag „In Paneldeck-Editor öffnen“ (#54) zeigt auf diese .exe: gespeicherte Werte für die Sicherung.
    /// Gehört zum selben Fenster-Eintrag „Explorer-Kontextmenü“ wie die Ordner-Einträge.
    pub edit_context: Option<Vec<(String, Option<String>, String)>>,
    /// `~/.claude.json` (bzw. `$CLAUDE_CONFIG_DIR/.claude.json`).
    pub claude_json: Option<PathBuf>,
    /// `mcpServers`-Namen darin, die auf diese .exe zeigen.
    pub claude_json_servers: Vec<String>,
}

/// Run-Werte `Paneldeck` / `Paneldeck (…)`, deren Programm diese .exe ist.
pub fn our_run_values(values: &[(String, String)], exe: &Path) -> Vec<(String, String)> {
    let exe = exe.to_string_lossy();
    values
        .iter()
        .filter(|(name, data)| (name == "Paneldeck" || launch::is_instance_run_value(name)) && launch::same_path(launch::program_of(data), &exe))
        .cloned()
        .collect()
}

/// Run-Werte eines Profils (`--profile <name>` im Befehl) dieser .exe.
pub fn profile_run_values(values: &[(String, String)], exe: &Path, profile: &str) -> Vec<(String, String)> {
    our_run_values(values, exe)
        .into_iter()
        .filter(|(_, data)| {
            let args = crate::cli::split_command_line(data);
            launch::Instance::from_args(args.get(1..).unwrap_or_default()).profile.is_some_and(|p| p.eq_ignore_ascii_case(profile))
        })
        .collect()
}

fn claude_json_path() -> Option<PathBuf> {
    crate::mcp::claude_json_path(std::env::var_os("CLAUDE_CONFIG_DIR"), std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")))
}

/// Namen unserer Server in einer `.claude.json` (leer, wenn es sie nicht gibt oder sie unlesbar ist).
pub fn servers_in(path: &Path, exe: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(crate::state::strip_bom(&text)) else { return Vec::new() };
    let mut n = crate::mcp::our_servers(&v, &exe.to_string_lossy());
    n.sort();
    n
}

impl State {
    /// Vom System lesen (nur lesend).
    pub fn read(exe: &Path, _data_dir: &Path) -> State {
        let run_values = our_run_values(&registry::values(RUN_KEY), exe);
        let context = registry::get_str(CONTEXT_PROBE, None).filter(|c| launch::same_path(launch::program_of(c), &exe.to_string_lossy())).map(|_| context_values());
        let edit_context = registry::get_str(EDIT_CONTEXT_PROBE, None).filter(|c| launch::same_path(launch::program_of(c), &exe.to_string_lossy())).map(|_| key_values(&[EDIT_CONTEXT_KEY]));
        let claude_json = claude_json_path().filter(|p| p.is_file());
        let claude_json_servers = claude_json.as_deref().map(|p| servers_in(p, exe)).unwrap_or_default();
        State { run_values, context, edit_context, claude_json, claude_json_servers }
    }

    /// Einträge des Fenster-Eintrags „Explorer-Kontextmenü“: Ordner-Menü und Editor-Eintrag (je einer, wenn vorhanden).
    pub fn context_entries(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.context.is_some() {
            v.push("Directory\\shell\\Paneldeck".to_string());
        }
        if self.edit_context.is_some() {
            v.push("*\\shell\\PaneldeckEdit".to_string());
        }
        v
    }

    fn current(ctx: &Ctx) -> State {
        match &ctx.integ_override {
            Some(s) => s.clone(),
            None if ctx.system_integration => State::read(&ctx.exe, &ctx.data_dir),
            None => State::default(),
        }
    }
}

/// Gespeicherte Werte der Kontextmenü-Schlüssel (Beschriftung, Symbol, Befehl).
fn context_values() -> Vec<(String, Option<String>, String)> {
    key_values(&CONTEXT_KEYS)
}

/// Gespeicherte Werte (Beschriftung, Symbol, Befehl) der angegebenen Schlüssel.
fn key_values(keys: &[&str]) -> Vec<(String, Option<String>, String)> {
    let mut v = Vec::new();
    for &key in keys {
        for (k, name) in [(key.to_string(), None), (key.to_string(), Some("Icon")), (format!("{key}\\command"), None)] {
            if let Some(val) = registry::get_str(&k, name) {
                v.push((k, name.map(str::to_string), val));
            }
        }
    }
    v
}

fn reg_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// `.reg`-Text (Version 5) für Werte unter HKCU, nach Schlüssel gruppiert in der gegebenen Reihenfolge.
pub fn reg_export(values: &[(String, Option<String>, String)]) -> String {
    let mut out = String::from("Windows Registry Editor Version 5.00\r\n");
    let mut last: Option<&str> = None;
    for (key, name, val) in values {
        if last != Some(key.as_str()) {
            out.push_str(&format!("\r\n[HKEY_CURRENT_USER\\{key}]\r\n"));
            last = Some(key);
        }
        let n = match name {
            Some(n) => format!("\"{}\"", reg_escape(n)),
            None => "@".to_string(),
        };
        out.push_str(&format!("{n}=\"{}\"\r\n", reg_escape(val)));
    }
    out
}

/// `.reg`-Datei (UTF-16 LE mit BOM, wie sie regedit schreibt) in den Sicherungsordner.
fn write_reg_backup(dir: &Path, stem: &str, values: &[(String, Option<String>, String)]) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = super::free_name(dir, stem, ".reg");
    let mut bytes = vec![0xff, 0xfe];
    for u in reg_export(values).encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

fn delete_run_values(ctx: &Ctx, vals: &[(String, String)], stem: &str, r: &mut ItemReport) {
    if vals.is_empty() || !ctx.system_integration {
        return;
    }
    let backup: Vec<(String, Option<String>, String)> = vals.iter().map(|(n, d)| (RUN_KEY.to_string(), Some(n.clone()), d.clone())).collect();
    match write_reg_backup(ctx.safety_dir(), stem, &backup) {
        Ok(p) => {
            let p = p.display().to_string();
            r.notes.push((format!("Gesichert: {p}"), format!("Saved: {p}")));
        }
        Err(e) => {
            r.errors.push(e);
            return; // ohne Sicherung nichts ändern
        }
    }
    for (name, _) in vals {
        match registry::apply(&[RegOp::DeleteValue { key: RUN_KEY.into(), name: name.clone() }]) {
            Ok(()) => r.entries += 1,
            Err(e) => r.errors.push(format!("{name}: {e}")),
        }
    }
}

/// Gespeicherte Registry-Werte (Schlüssel, Wertname, Inhalt).
type RegValues = Vec<(String, Option<String>, String)>;
/// Ein Teil des Kontextmenüs: gespeicherte Werte, Name der Sicherung, Ausschalter.
type ContextPart = (Option<RegValues>, &'static str, fn(bool, &Path) -> Result<(), String>);

/// Einen Integrations-Eintrag entfernen (nur im laufenden Betrieb).
pub fn remove(ctx: &Ctx, item: Item, r: &mut ItemReport) {
    let st = State::current(ctx);
    match item {
        Item::Autostart => delete_run_values(ctx, &st.run_values, "autostart", r),
        Item::ContextMenu => {
            if !ctx.system_integration {
                return;
            }
            // Ordner-Einträge und Editor-Eintrag (#54): je eigene Sicherung, dann entfernen.
            let parts: [ContextPart; 2] = [(st.context, "context-menu", crate::integration::set_context), (st.edit_context, "context-menu-editor", crate::integration::set_edit_context)];
            for (vals, stem, off) in parts {
                let Some(vals) = vals else { continue };
                match write_reg_backup(ctx.safety_dir(), stem, &vals) {
                    Ok(p) => {
                        let p = p.display().to_string();
                        r.notes.push((format!("Gesichert: {p}"), format!("Saved: {p}")));
                    }
                    Err(e) => {
                        r.errors.push(e);
                        continue; // ohne Sicherung nichts ändern
                    }
                }
                match off(false, &ctx.exe) {
                    Ok(()) => r.entries += 1,
                    Err(e) => r.errors.push(e),
                }
            }
        }
        Item::ClaudeJson => {
            let Some(path) = st.claude_json.filter(|_| !st.claude_json_servers.is_empty()) else { return };
            remove_claude_json(&path, &ctx.exe, ctx.safety_dir(), r);
        }
        _ => {}
    }
}

/// Unsere Einträge aus einer `.claude.json` entfernen; die Datei wird vorher in `backup_dir` kopiert. Ohne Sicherung
/// bleibt die Datei unverändert. `claude` selbst wird nie aufgerufen.
pub fn remove_claude_json(path: &Path, exe: &Path, backup_dir: &Path, r: &mut ItemReport) {
    if let Err(e) = std::fs::create_dir_all(backup_dir) {
        r.errors.push(format!("{}: {e}", backup_dir.display()));
        return;
    }
    let bak = super::free_name(backup_dir, "claude.json", ".bak");
    if let Err(e) = std::fs::copy(path, &bak) {
        r.errors.push(format!("{}: {e}", bak.display()));
        return;
    }
    let b = bak.display().to_string();
    r.notes.push((format!("Gesichert: {b}"), format!("Saved: {b}")));
    match crate::mcp::remove_our_servers_from(path, &exe.to_string_lossy()) {
        Ok(names) => r.entries += names.len() as u64,
        Err(e) => r.errors.push(e),
    }
}

/// Beim Löschen eines Profils: dessen Autostart-Wert (dieser .exe) mit entfernen.
pub fn remove_profile_autostart(ctx: &Ctx, profile: &str, r: &mut ItemReport) {
    if !ctx.system_integration {
        return;
    }
    let vals = profile_run_values(&registry::values(RUN_KEY), &ctx.exe, profile);
    delete_run_values(ctx, &vals, "autostart", r);
}
