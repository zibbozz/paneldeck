//! Agenten-Werkzeuge der Chats im Hub (#338): Ausführung der Datei-, Such-, Git- und Web-Werkzeuge im Hintergrund,
//! `update_todos`, `ask_user` (#356) und `run_command` im eigenen Terminal des Chats – im Vordergrund oder (#355) im
//! Hintergrund (`bg.rs`).
//!
//! Das Terminal des Chats ist eine normale, sichtbare Terminal-Sitzung im Projektordner (Kind des Chats in der
//! Seitenleiste, wie `create_session`); es wird beim ersten Befehl angelegt und danach wiederverwendet – je gewählter
//! Shell eines (#355). Der Befehl läuft über denselben Weg wie das Paneldeck-Werkzeug `run_command`
//! (Shell-Integration, Freigabe vorher im Chat).

use super::*;
use crate::chat::agent::{self as chat_agent, AgentEnv};
use crate::chat::bg_commands::{self, ShellCandidate};
use crate::chat::store::ToolCall;

/// So lange wartet `run_command` auf die Shell-Integration eines frisch gestarteten Terminals.
const READY_WAIT: Duration = Duration::from_secs(20);

/// Schlüssel in den Argumenten eines wartenden `run_command` eines Chats: Korrekturen für das Ergebnis (#355).
pub(super) const CORRECTED_KEY: &str = "_paneldeck_corrected";

/// Welche Shell ein Befehl des Chats bekommt (#355).
pub(super) struct ShellPick {
    /// Schlüssel für das Terminal des Chats (`""` = Profil des Projekts).
    pub key: String,
    /// Fest gewähltes Profil für `mcp_create_for_chat` (`Some(0)` = keins; `None` = das des Projekts).
    pub profile: Option<u64>,
    /// Gewählte erkannte Shell.
    pub shell: Option<String>,
    /// Hinweis für das Modell (z. B. „Profil des Projekts nicht nutzbar, Standard-Shell genommen“).
    pub note: Option<String>,
}

impl App {
    /// Projektordner und Einstellungen der Agenten-Werkzeuge für den Chat `chat`.
    pub(in crate::app) fn agent_env(&self, chat: u64) -> AgentEnv {
        let root = self.entries.get(&chat).map(|e| e.meta.cwd.trim().to_string()).filter(|c| !c.is_empty()).map(std::path::PathBuf::from);
        AgentEnv { root, data_dir: Some(self.dir.clone()), settings: self.settings.agent_tools.clone(), max_bytes: 0, background: self.mcp.bg.any(chat) }
    }

    /// Agenten-Werkzeug des Chats `chat` ausführen (nach Prüfung und Freigabe in `ChatState::tool_step`). `max_bytes` =
    /// Platz für das Ergebnis. Das Ergebnis kommt über den Kanal (Platte, git und Netz laufen im Hintergrund).
    pub(in crate::app) fn agent_call(&mut self, ctx: &egui::Context, chat: u64, call: &ToolCall, max_bytes: usize) -> std::sync::mpsc::Receiver<R> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        match call.name.as_str() {
            "update_todos" => {
                let r = chat_agent::parse_todos(&call.arguments).map(|(list, fixes)| {
                    let v = chat_agent::todos_result(&list, &fixes);
                    if let Some(c) = self.entries.get_mut(&chat).and_then(|e| e.chat.as_mut()) {
                        c.set_todos(list);
                    }
                    v
                });
                self.mcp_finish(&tx, &call.name, &json!({}), Some(chat), "chat", r);
            }
            // Fragen an den Nutzer (#356): das Formular wartet im Chat; das Ergebnis schickt die Oberfläche beim Absenden
            // bzw. Abbrechen über diesen Kanal. Kein Zeitlimit.
            "ask_user" => match crate::chat::ask::parse(&call.arguments) {
                Ok((questions, fixes)) => {
                    let form = crate::chat::ask::AskForm::new(call.id.clone(), questions, fixes, Some(tx.clone()));
                    match self.entries.get_mut(&chat).and_then(|e| e.chat.as_mut()) {
                        Some(c) => c.open_ask(form),
                        None => self.mcp_finish(&tx, &call.name, &json!({}), Some(chat), "chat", Err(er("not_found", "The chat is gone."))),
                    }
                    ctx.request_repaint();
                }
                Err(e) => self.mcp_finish(&tx, &call.name, &json!({}), Some(chat), "chat", Err(e)),
            },
            "run_command" => self.agent_run(ctx, chat, &call.arguments, &tx),
            "command_status" | "command_output" | "stop_command" => {
                let r = self.agent_bg_tool(ctx, chat, &call.name, &call.arguments, max_bytes);
                self.mcp_finish(&tx, &call.name, &call.arguments, Some(chat), "chat", r);
            }
            _ => {
                let mut env = self.agent_env(chat);
                env.max_bytes = max_bytes;
                let (name, args) = (call.name.clone(), call.arguments.clone());
                let info = chat_agent::log_info(call);
                let (log_how, model) = self.log_origin("chat", Some(chat));
                let dir = self.dir.clone();
                let ctx2 = ctx.clone();
                std::thread::spawn(move || {
                    let r = chat_agent::execute(&name, &args, &env);
                    let info = match &r {
                        Err((code, msg)) => format!("{code}: {}", mcp::short(msg, 80)),
                        Ok(_) => info,
                    };
                    let entry = LogEntry { ts: usage::today_secs(), tool: name, session: None, caller: Some(chat), ok: r.is_ok(), info, how: log_how, model };
                    mcp::append_log(&dir, &entry);
                    let _ = tx.try_send(r);
                    ctx2.request_repaint();
                });
            }
        }
        rx
    }

    /// Lebt das Terminal `id` noch als Befehlszeile (keine Claude-Sitzung, kein Chat, nicht beendet)?
    pub(super) fn agent_term_alive(&self, id: u64) -> bool {
        self.entries.get(&id).is_some_and(|e| !e.is_chat() && !e.is_claude() && !e.exited() && e.live.as_ref().is_some_and(|s| !s.has_exited()))
    }

    /// Profile und erkannte Shells, die ein Befehl eines Chats nennen kann (#355; gesperrte mit Grund).
    pub(super) fn shell_candidates(&self) -> Vec<ShellCandidate> {
        let mut v: Vec<ShellCandidate> = self
            .settings
            .profiles
            .iter()
            .filter(|p| !p.name.trim().is_empty())
            .map(|p| ShellCandidate::profile(p.id, &p.name, p.admin, crate::templates::agent_profile_issue(&p.program, &p.args, &p.startup, false).is_some()))
            .collect();
        v.extend(self.shells.iter().map(|s| ShellCandidate::shell(&s.id, &s.label)));
        v
    }

    /// Shell für einen Befehl des Chats `chat` (#355): `name` = Profil oder Shell des Nutzers; ohne Namen das Profil des
    /// Projekts – außer es startet als Administrator oder hat ein eigenes Programm, eigene Argumente oder einen
    /// Startbefehl (dann die Standard-Shell, mit Hinweis).
    pub(super) fn agent_shell(&self, chat: u64, name: Option<&str>) -> Result<ShellPick, (String, String)> {
        let cands = self.shell_candidates();
        if let Some(n) = name {
            let c = &cands[bg_commands::resolve_shell(n, &cands)?];
            return Ok(match c.key.strip_prefix("profile:").and_then(|p| p.parse::<u64>().ok()) {
                Some(pid) => ShellPick { key: c.key.clone(), profile: Some(pid), shell: None, note: None },
                None => ShellPick { key: c.key.clone(), profile: Some(0), shell: Some(c.key.clone()), note: None },
            });
        }
        let project = self.entries.get(&chat).map(|e| e.meta.project).unwrap_or(self.active_project);
        let pid = self.projects.iter().find(|p| p.id == project).map(|p| p.profile).unwrap_or(0);
        match self.profile(pid) {
            Some(p) if p.admin || crate::templates::agent_profile_issue(&p.program, &p.args, &p.startup, false).is_some() => {
                let shell = self.shell_by_id(&self.settings.default_shell).map(|s| s.label).unwrap_or_else(|| self.settings.default_shell.clone());
                Ok(ShellPick {
                    key: String::new(),
                    profile: Some(0),
                    shell: None,
                    note: Some(format!(
                        "The project's profile “{}” starts as administrator or has its own program, which agents may never use – the default shell “{shell}” was used instead.",
                        mcp::short(p.name.trim(), 40)
                    )),
                })
            }
            _ => Ok(ShellPick { key: String::new(), profile: None, shell: None, note: None }),
        }
    }

    /// Neues Terminal für Befehle des Chats `chat` im Projektordner mit der Shell `pick` (Kind des Chats).
    pub(super) fn agent_new_terminal(&mut self, ctx: &egui::Context, chat: u64, pick: &ShellPick, name: &str) -> Result<u64, (String, String)> {
        let env = self.agent_env(chat);
        let root = chat_agent::paths::root(&env)?;
        let cwd = chat_agent::paths::display(&root.canon);
        let mut args = json!({"cwd": cwd, "name": name});
        if let Some(s) = &pick.shell {
            args["shell"] = json!(s);
        }
        match self.mcp_create_for_chat(ctx, &args, chat, pick.profile) {
            Ok(v) => v["id"].as_u64().ok_or_else(|| er("terminal_failed", "The chat's terminal could not be started.")),
            Err((code, msg)) if code == "terminal_failed" => Err((code, msg)),
            Err((code, msg)) => Err(er(&code, format!("The chat's terminal could not be started: {msg}"))),
        }
    }

    /// `run_command` ohne `session`: im Vordergrund im Terminal des Chats (angelegt beim ersten Mal, im Projektordner,
    /// je Shell eines), mit `background` in einem eigenen Terminal (#355, `bg.rs`).
    fn agent_run(&mut self, ctx: &egui::Context, chat: u64, args: &Value, tx: &SyncSender<R>) {
        let parsed = (|| -> Result<(bg_commands::RunRequest, Vec<String>), (String, String)> {
            // Wie bei jedem Aufruf eines Chats: keine MCP-Brücke aus einer von einem Agenten ausgelösten Runde.
            let origins = crate::app::taint::ancestors(&self.entries, Some(chat));
            if let Some(msg) = crate::chat::tools::spawn_forbidden("run_command", args, &origins) {
                return Err(er("forbidden", msg));
            }
            let (req, fixes) = bg_commands::parse_run(args, &self.settings.agent_tools)?;
            if args.get("session").is_some_and(|s| !s.is_null()) {
                return Err(er("bad_args", "background and shell only work in the chat's own terminals – leave out session (or leave out background and shell to run in that session)."));
            }
            if !self.settings.shell_integration {
                return Err(er("no_shell_integration", "Shell integration is switched off (Settings → General); without it the end of a command cannot be detected. Ask the user to switch it on."));
            }
            // Ohne Projektordner kein Terminal (wie bisher).
            chat_agent::paths::root(&self.agent_env(chat))?;
            Ok((req, fixes))
        })();
        let (req, mut fixes) = match parsed {
            Ok(x) => x,
            Err(e) => return self.mcp_finish(tx, "run_command", args, Some(chat), "chat", Err(e)),
        };
        if req.background {
            let r = self.agent_bg_start(ctx, chat, &req, &fixes);
            return self.mcp_finish(tx, "run_command", args, Some(chat), "chat", r);
        }
        let r = (|| -> Result<Value, (String, String)> {
            let pick = self.agent_shell(chat, req.shell.as_deref())?;
            fixes.extend(pick.note.clone());
            let key = (chat, pick.key.clone());
            let id = match self.mcp.chat_terms.get(&key).copied().filter(|id| self.agent_term_alive(*id)) {
                Some(id) => id,
                None => {
                    let chat_name = self.entries.get(&chat).map(|e| e.display_name()).unwrap_or_default();
                    let mut name = format!("{} – {}", mcp::short(chat_name.trim(), 40), tr("Befehle", "commands"));
                    if let Some(n) = req.shell.as_deref() {
                        name = format!("{name} ({})", mcp::short(n, 20));
                    }
                    let id = self.agent_new_terminal(ctx, chat, &pick, &name)?;
                    self.mcp.chat_terms.insert(key, id);
                    id
                }
            };
            let mut a = json!({"session": id, "command": req.command, "timeout_secs": req.timeout_secs, "max_output_lines": req.max_output_lines});
            if !fixes.is_empty() {
                a[CORRECTED_KEY] = json!(fixes);
            }
            Ok(a)
        })();
        let args = match r {
            Ok(a) => a,
            Err(e) => return self.mcp_finish(tx, "run_command", args, Some(chat), "chat", Err(e)),
        };
        let id = args["session"].as_u64().unwrap_or(0);
        // Frisch gestartet: auf die Shell-Integration warten (`poll_mcp` → `agent_ready`).
        self.mcp.pending.push(PendingMcp { reply: tx.clone(), tool: "run_command".into(), args, caller: Some(chat), how: "chat", session: id, deadline: Instant::now() + READY_WAIT, kind: Pend::AgentRun });
        ctx.request_repaint();
    }

    /// Wartendes `run_command` im Terminal eines Chats: ist die Shell bereit (oder die Frist um), läuft der Befehl wie
    /// das Paneldeck-Werkzeug `run_command` (das meldet dann ggf. „keine Shell-Integration“) – mit dem Höchstwert für
    /// `timeout_secs` aus den Einstellungen der Chats (#355).
    pub(super) fn agent_ready(&mut self, p: PendingMcp) {
        let ready = self.entries.get(&p.session).and_then(|e| e.live.as_ref()).map(|s| s.has_exited() || s.shell_state.lock().unwrap_or_else(|x| x.into_inner()).integration_seen);
        match ready {
            None => self.mcp_finish(&p.reply, &p.tool, &p.args, p.caller, p.how, Err(er("session_closed", "The chat's terminal was closed before the command ran."))),
            Some(false) if Instant::now() < p.deadline => self.mcp.pending.push(p),
            _ => {
                let max = self.settings.agent_tools.command_timeout_max_secs;
                if let Out::Now(r) = self.mcp_run_for(&p.args, p.caller, &p.reply, &p.tool, p.how, max) {
                    self.mcp_finish(&p.reply, &p.tool, &p.args, p.caller, p.how, r);
                }
            }
        }
    }
}
