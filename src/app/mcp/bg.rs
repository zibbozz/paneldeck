//! Hintergrund-Befehle der Chats im Hub (#355): die App als [`BgHost`] für `chat::bg_commands`. Jeder Befehl bekommt
//! ein eigenes, sichtbares Terminal unter dem Chat (Kind in der Seitenleiste, im Projektordner, mit der gewählten
//! Shell); ein Terminal eines beendeten Befehls mit derselben Shell wird wiederverwendet. Eingegeben wird über
//! denselben Weg wie `run_command` (Shell-Integration, kein Zeitlimit); endet der Befehl, kommt eine Nachricht von
//! Paneldeck in den Chat (`ChatState::push_event`).

use super::agent_tools::ShellPick;
use super::*;
use crate::chat::bg_commands::{self, BgHost, RunRequest};
use crate::chat::store::ChatEvent;

/// „Kein Zeitlimit“ für Hintergrund-Befehle: 30 Tage.
const BG_SECS: u64 = 30 * 24 * 3600;

/// Die App als Hub der Hintergrund-Befehle.
struct BgHub<'a> {
    app: &'a mut App,
    ctx: &'a egui::Context,
}

impl BgHost for BgHub<'_> {
    fn open_terminal(&mut self, chat: u64, req: &RunRequest, title: &str, reuse: &[u64]) -> Result<(u64, String, Option<String>), (String, String)> {
        let pick: ShellPick = self.app.agent_shell(chat, req.shell.as_deref())?;
        let chat_name = self.app.entries.get(&chat).map(|e| e.display_name()).unwrap_or_default();
        let name = format!("{} – {title}", mcp::short(chat_name.trim(), 30));
        // Ein freies Terminal eines beendeten Befehls (noch offen, Shell ruht an ihrem Prompt – der Nutzer hat dort nichts
        // gestartet, `term_idle`): umbenennen und weiter nutzen. Sonst ein neues; nie in ein Programm des Nutzers tippen.
        let free = reuse.iter().copied().find(|s| self.app.term_idle(*s) && self.app.entries.get(s).is_some_and(|e| e.status != ClaudeStatus::Permission));
        let id = match free {
            Some(s) => {
                if let Some(e) = self.app.entries.get_mut(&s) {
                    e.meta.name = name.chars().take(80).collect();
                    e.meta.custom_name = true;
                }
                s
            }
            None => self.app.agent_new_terminal(self.ctx, chat, &pick, &name)?,
        };
        let shell = self.app.entries.get(&id).map(|e| e.eff_shell()).and_then(|sid| self.app.shell_by_id(sid).map(|s| s.label)).unwrap_or_default();
        Ok((id, shell, pick.note))
    }

    fn terminal_ready(&self, session: u64) -> Option<bool> {
        let e = self.app.entries.get(&session)?;
        let s = e.live.as_ref()?;
        Some(s.has_exited() || s.shell_state.lock().unwrap_or_else(|x| x.into_inner()).integration_seen)
    }

    fn run(&mut self, id: &str, chat: u64, session: u64, command: &str) -> Result<std::sync::mpsc::Receiver<R>, (String, String)> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let args = json!({"session": session, "command": command, "timeout_secs": BG_SECS, "max_output_lines": bg_commands::MAX_KEPT_LINES});
        // Anfang der Ausgabe festhalten (wie `run_command`), für `command_output` während des Laufs.
        let pin = self.app.entries.get(&session).and_then(|e| e.live.as_ref()).map(|s| s.pin_abs(s.abs_cursor_line()));
        match self.app.mcp_run_for(&args, Some(chat), &tx, "run_command", "chat", BG_SECS) {
            Out::Later => {
                if let Some(p) = pin {
                    self.app.mcp.bg_live.insert((session, id.to_string()), p);
                }
                Ok(rx)
            }
            Out::Now(Ok(_)) => Err(er("internal", "run_command finished at once.")),
            Out::Now(Err(e)) => {
                self.app.mcp_finish(&tx, "run_command", &args, Some(chat), "chat", Err(e.clone()));
                Err(e)
            }
        }
    }

    fn live_output(&self, id: &str, session: u64, command: &str) -> Option<Vec<String>> {
        let pin = self.app.mcp.bg_live.get(&(session, id.to_string()))?;
        let e = self.app.entries.get(&session)?;
        let s = e.live.as_ref()?;
        let zsh = e.shell_flavor == Some(crate::shellinit::Flavor::Zsh);
        Some(s.run_output(command, pin.get(), None, None, false, zsh).lines)
    }

    fn interrupt(&mut self, session: u64) {
        if let Some(s) = self.app.entries.get(&session).and_then(|e| e.live.as_ref()) {
            s.write(&[0x03]);
        }
    }

    fn close_terminal(&mut self, session: u64) {
        // Erst alles im Job der Sitzung beenden (wie „Alle beenden“): ein Programm, das Strg+C ignoriert, liefe sonst
        // im aufbewahrten Job weiter, obwohl der Befehl als angehalten gilt (#355 Review).
        if let Some(s) = self.app.entries.get(&session).and_then(|e| e.live.as_ref()) {
            s.end_job();
        }
        if self.app.entries.contains_key(&session) {
            self.app.close_session(session);
        }
    }

    fn finished(&mut self, chat: u64, event: ChatEvent, text: String) {
        if let Some(c) = self.app.entries.get_mut(&chat).and_then(|e| e.chat.as_mut()) {
            c.push_event(event, text);
            self.app.save_chat(chat);
        }
        self.ctx.request_repaint();
    }
}

impl App {
    /// `run_command` mit `background: true` (schon geprüft und freigegeben): Terminal öffnen, sofort die Kennung.
    pub(super) fn agent_bg_start(&mut self, ctx: &egui::Context, chat: u64, req: &RunRequest, fixes: &[String]) -> R {
        let limit = self.settings.agent_tools.background_limit;
        // Nach einem Neustart zählen die Kennungen weiter (bg1 … aus dem Verlauf).
        let seen = self.entries.get(&chat).and_then(|e| e.chat.as_ref()).map_or(0, |c| crate::chat::bg_commands::max_id_in(c.messages()));
        self.mcp.bg.seed(chat, seen);
        let mut bg = std::mem::take(&mut self.mcp.bg);
        let r = bg.start(&mut BgHub { app: self, ctx }, chat, req, limit, fixes, Instant::now());
        // Während `start` kann nichts Neues dazugekommen sein (die Liste lag hier); zurücklegen.
        self.mcp.bg = bg;
        ctx.request_repaint();
        r
    }

    /// `command_status`, `command_output`, `stop_command` des Chats `chat`.
    pub(super) fn agent_bg_tool(&mut self, ctx: &egui::Context, chat: u64, tool: &str, args: &Value, max_bytes: usize) -> R {
        let mut bg = std::mem::take(&mut self.mcp.bg);
        let now = Instant::now();
        let r = match tool {
            "command_status" => bg.status(chat, args, now),
            "command_output" => bg.output(&BgHub { app: self, ctx }, chat, args, max_bytes),
            _ => bg.stop(&mut BgHub { app: self, ctx }, chat, args, now),
        };
        self.mcp.bg = bg;
        r
    }

    /// Jedes Bild (Hauptdurchgang): Hintergrund-Befehle weiterführen.
    pub(super) fn poll_bg(&mut self, ctx: &egui::Context) {
        if !self.mcp.bg.busy() {
            return;
        }
        let mut bg = std::mem::take(&mut self.mcp.bg);
        bg.poll(&mut BgHub { app: self, ctx }, Instant::now());
        self.mcp.bg = bg;
        // Festgehaltene Zeilen beendeter Befehle freigeben.
        let live: std::collections::HashSet<(u64, String)> = self.mcp.bg.running_keys().collect();
        self.mcp.bg_live.retain(|k, _| live.contains(k));
        if self.mcp.bg.busy() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
    }
}
