//! Agenten-Werkzeuge der Chats im Hub (#338): Ausführung der Datei-, Such-, Git- und Web-Werkzeuge im Hintergrund,
//! `update_todos` und `run_command` im eigenen Terminal des Chats.
//!
//! Das Terminal des Chats ist eine normale, sichtbare Terminal-Sitzung im Projektordner (Kind des Chats in der
//! Seitenleiste, wie `create_session`); es wird beim ersten Befehl angelegt und danach wiederverwendet. Der Befehl
//! läuft über denselben Weg wie das Paneldeck-Werkzeug `run_command` (Shell-Integration, Freigabe vorher im Chat).

use super::*;
use crate::chat::agent::{self as chat_agent, AgentEnv};
use crate::chat::store::ToolCall;

/// So lange wartet `run_command` auf die Shell-Integration eines frisch gestarteten Terminals.
const READY_WAIT: Duration = Duration::from_secs(20);

impl App {
    /// Projektordner und Einstellungen der Agenten-Werkzeuge für den Chat `chat`.
    pub(in crate::app) fn agent_env(&self, chat: u64) -> AgentEnv {
        let root = self.entries.get(&chat).map(|e| e.meta.cwd.trim().to_string()).filter(|c| !c.is_empty()).map(std::path::PathBuf::from);
        AgentEnv { root, data_dir: Some(self.dir.clone()), settings: self.settings.agent_tools.clone(), max_bytes: 0 }
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
            "run_command" => self.agent_run(ctx, chat, &call.arguments, &tx),
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
    fn agent_term_alive(&self, id: u64) -> bool {
        self.entries.get(&id).is_some_and(|e| !e.is_chat() && !e.is_claude() && !e.exited() && e.live.as_ref().is_some_and(|s| !s.has_exited()))
    }

    /// `run_command` ohne `session`: im Terminal des Chats (angelegt beim ersten Mal, im Projektordner).
    fn agent_run(&mut self, ctx: &egui::Context, chat: u64, args: &Value, tx: &SyncSender<R>) {
        let r = (|| -> Result<Value, (String, String)> {
            // Wie bei jedem Aufruf eines Chats: keine MCP-Brücke aus einer von einem Agenten ausgelösten Runde.
            let origins = crate::app::taint::ancestors(&self.entries, Some(chat));
            if let Some(msg) = crate::chat::tools::spawn_forbidden("run_command", args, &origins) {
                return Err(er("forbidden", msg));
            }
            let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("").trim();
            if cmd.is_empty() || cmd.contains('\n') || cmd.contains('\r') || cmd.len() > 4096 {
                return Err(er("bad_args", "command must be exactly one line (at most 4096 bytes). Example: {\"command\":\"cargo test\"}"));
            }
            if !self.settings.shell_integration {
                return Err(er("no_shell_integration", "Shell integration is switched off (Settings → General); without it the end of a command cannot be detected. Ask the user to switch it on."));
            }
            let env = self.agent_env(chat);
            let root = chat_agent::paths::root(&env)?;
            let mut a = args.clone();
            if a.get("timeout_secs").is_none_or(Value::is_null) {
                a["timeout_secs"] = json!(self.settings.agent_tools.command_timeout_secs);
            }
            let id = match self.mcp.chat_terms.get(&chat).copied().filter(|id| self.agent_term_alive(*id)) {
                Some(id) => id,
                None => {
                    let chat_name = self.entries.get(&chat).map(|e| e.display_name()).unwrap_or_default();
                    let name = format!("{} – {}", mcp::short(chat_name.trim(), 40), tr("Befehle", "commands"));
                    let cwd = chat_agent::paths::display(&root.canon);
                    let id = match self.mcp_create(ctx, &json!({"cwd": cwd, "name": name}), Some(chat), None) {
                        CreateStep::Done(Ok(v)) => v["id"].as_u64().ok_or_else(|| er("terminal_failed", "The chat's terminal could not be started."))?,
                        CreateStep::Done(Err((code, msg))) => return Err(er(&code, format!("The chat's terminal could not be started: {msg}"))),
                        CreateStep::Check { .. } => return Err(er("terminal_failed", "The chat's terminal could not be started (the default shell needs a folder check). Use run_command with the session of an existing terminal.")),
                    };
                    self.mcp.chat_terms.insert(chat, id);
                    id
                }
            };
            a["session"] = json!(id);
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
    /// das Paneldeck-Werkzeug `run_command` (das meldet dann ggf. „keine Shell-Integration“).
    pub(super) fn agent_ready(&mut self, p: PendingMcp) {
        let ready = self.entries.get(&p.session).and_then(|e| e.live.as_ref()).map(|s| s.has_exited() || s.shell_state.lock().unwrap_or_else(|x| x.into_inner()).integration_seen);
        match ready {
            None => self.mcp_finish(&p.reply, &p.tool, &p.args, p.caller, p.how, Err(er("session_closed", "The chat's terminal was closed before the command ran."))),
            Some(false) if Instant::now() < p.deadline => self.mcp.pending.push(p),
            _ => {
                if let Out::Now(r) = self.mcp_run(&p.args, p.caller, &p.reply, &p.tool, p.how) {
                    self.mcp_finish(&p.reply, &p.tool, &p.args, p.caller, p.how, r);
                }
            }
        }
    }
}
