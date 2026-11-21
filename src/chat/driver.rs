//! Die Werkzeug-Schleife eines Chats pro Bild weiterführen – unabhängig von der App, damit sich die Verdrahtung mit
//! einem Ersatz-Hub testen lässt. Die App setzt [`ChatToolHost`] um (Ausführung über den MCP-Ausführer mit dem Chat
//! als Aufrufer, nächste Anfrage über das Backend des Hubs).

use super::agent;
use super::session::{ChatState, ToolStep};
use super::store::ToolCall;
use super::tools::{ToolPolicy, ToolResult};
use std::sync::mpsc::Receiver;

/// Was die Werkzeug-Schleife vom Hub braucht.
pub trait ChatToolHost {
    /// Zustand des Chats `id` (`None`, wenn es ihn nicht mehr gibt).
    fn chat(&mut self, id: u64) -> Option<&mut ChatState>;
    /// Was für die Werkzeuge dieses Chats gerade gilt (Einstellungen des Modells, Expertenmodus).
    fn policy(&self, id: u64) -> ToolPolicy;
    /// Werkzeug ausführen, mit dem Chat `chat` als Aufrufer; das Ergebnis kommt sofort oder später über den Kanal.
    fn execute(&mut self, chat: u64, call: &ToolCall) -> Receiver<ToolResult>;
    /// Nächste Anfrage des Chats an das Modell starten (nach [`ToolStep::Request`]).
    fn request(&mut self, id: u64);
    /// Anzeigename einer Sitzung (für die Freigabe-Zeile), `None` wenn unbekannt.
    fn session_name(&self, id: u64) -> Option<String>;
}

/// Höchstens so viele Schritte je Bild (sofort fertige Werkzeuge laufen nacheinander, ohne das Bild lange aufzuhalten).
pub const STEPS_PER_FRAME: usize = 16;

/// Ergebnis von [`drive`].
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Driven {
    /// Verlauf oder Zustand haben sich geändert (speichern, neu zeichnen).
    pub changed: bool,
    /// Es gäbe noch mehr zu tun (Grenze je Bild erreicht): bald neu zeichnen.
    pub more: bool,
}

/// Ziel eines Aufrufs für die Freigabe-Zeile: `session 4 "build"` (bzw. nur die Nummer bei unbekanntem Namen).
pub fn target_label(call: &ToolCall, name: impl Fn(u64) -> Option<String>) -> Option<String> {
    let id = call.arguments.get("session").and_then(|s| s.as_u64())?;
    Some(match name(id) {
        Some(n) if !n.trim().is_empty() => format!("session {id} {:?}", crate::mcp::short(n.trim(), 60)),
        _ => format!("session {id}"),
    })
}

/// Werkzeug-Schleife des Chats `id` ein Stück weiterführen (nur im Hauptdurchgang aufrufen): Ergebnisse abholen,
/// nächsten Aufruf prüfen und ausführen, nach dem letzten Ergebnis die nächste Anfrage starten.
pub fn drive(host: &mut dyn ChatToolHost, id: u64) -> Driven {
    let mut out = Driven::default();
    if host.chat(id).is_none_or(|c| c.calls.is_empty() && !c.busy()) {
        return out;
    }
    let policy = host.policy(id);
    for _ in 0..STEPS_PER_FRAME {
        let Some(c) = host.chat(id) else { return out };
        out.changed |= c.poll_tools();
        let before = c.rev();
        let step = c.tool_step(&policy);
        out.changed |= c.rev() != before;
        match step {
            ToolStep::Idle => {
                // Freigabe-Zeile: Ziel mit Namen und – bei edit_file/write_file – die Diff-Vorschau (einmal je wartendem
                // Aufruf; die Vorschau liest die Datei einmal von der Platte).
                let pending = host.chat(id).and_then(|c| {
                    let call = c.pending_approval()?.clone();
                    let need_label = c.approval_target.is_none();
                    let need_preview = c.approval_preview.as_ref().is_none_or(|(cid, _)| *cid != call.id);
                    (need_label || need_preview).then_some((call, need_label, need_preview))
                });
                if let Some((call, need_label, need_preview)) = pending {
                    let label = need_label.then(|| if agent::is_local(&call) { agent::target(&call) } else { target_label(&call, |s| host.session_name(s)) }).flatten();
                    let preview = if need_preview && agent::is_local(&call) { agent::files::preview(&call.name, &call.arguments, &policy.agent).map(|p| (call.id.clone(), p)) } else { None };
                    if let Some(c) = host.chat(id) {
                        if need_label {
                            c.approval_target = label;
                        }
                        if preview.is_some() {
                            c.approval_preview = preview;
                        }
                    }
                }
                return out;
            }
            ToolStep::Execute(call) => {
                let rx = host.execute(id, &call);
                if let Some(c) = host.chat(id) {
                    c.tool_started(rx);
                }
                out.changed = true;
            }
            ToolStep::Request => {
                host.request(id);
                out.changed = true;
                return out;
            }
        }
    }
    out.more = true;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::backend::{ChatBackend, Repaint, TurnContext, TurnEvent, TurnHandle};
    use crate::chat::session::{ChatStatus, Decision};
    use crate::chat::store::{ChatFile, ChatMsg, Role, ToolStatus};
    use crate::llm::config::LlmModel;
    use crate::mcp::McpLevel;
    use crate::state::ChatMeta;
    use std::collections::HashMap;
    use std::sync::mpsc::{sync_channel, SyncSender};
    use std::sync::{Arc, Mutex};

    /// Drehbuch-Backend: je Anfrage die nächste Ereignisliste (die letzte wiederholt sich).
    struct Script(Mutex<Vec<Vec<TurnEvent>>>);

    impl ChatBackend for Script {
        fn start(&self, _: &ChatMeta, _: &[ChatMsg], _: &TurnContext, _: Repaint) -> TurnHandle {
            let (h, tx, _) = TurnHandle::pair();
            let mut v = self.0.lock().unwrap();
            let evs = if v.len() > 1 { v.remove(0) } else { v[0].clone() };
            for ev in evs {
                tx.send(ev).unwrap();
            }
            h
        }
    }

    fn tc(id: &str, name: &str, args: serde_json::Value) -> TurnEvent {
        TurnEvent::ToolCall(ToolCall { id: id.into(), name: name.into(), arguments: args })
    }

    /// Ersatz-Hub: ein Chat, Werkzeuge antworten sofort oder erst auf Abruf („später“, wie `run_command`).
    struct Fake {
        chats: HashMap<u64, ChatState>,
        cfg: LlmModel,
        backend: Script,
        /// (Aufrufer, Werkzeug, Argumente) jedes ausgeführten Aufrufs.
        executed: Vec<(u64, String, serde_json::Value)>,
        /// Werkzeuge, deren Ergebnis erst später kommt; der Sender wartet hier.
        later: Vec<&'static str>,
        waiting: Vec<SyncSender<ToolResult>>,
        agent: crate::chat::agent::AgentEnv,
    }

    impl Fake {
        fn new(cfg: LlmModel, turns: Vec<Vec<TurnEvent>>) -> Fake {
            Fake { chats: HashMap::new(), cfg, backend: Script(Mutex::new(turns)), executed: Vec::new(), later: Vec::new(), waiting: Vec::new(), agent: Default::default() }
        }
        fn send(&mut self, id: u64, text: &str) {
            let c = self.chats.entry(id).or_insert_with(|| ChatState::new(ChatFile::default(), None));
            c.draft = text.into();
            assert!(c.send(&self.backend, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {})));
        }
        /// Ein Bild: Antwort abholen, Schleife weiterführen.
        fn frame(&mut self, id: u64) -> Driven {
            if let Some(c) = self.chats.get_mut(&id) {
                c.poll();
            }
            drive(self, id)
        }
        fn frames(&mut self, id: u64, n: usize) {
            for _ in 0..n {
                self.frame(id);
            }
        }
        fn chat_ref(&self, id: u64) -> &ChatState {
            &self.chats[&id]
        }
    }

    impl ChatToolHost for Fake {
        fn chat(&mut self, id: u64) -> Option<&mut ChatState> {
            self.chats.get_mut(&id)
        }
        fn policy(&self, _: u64) -> ToolPolicy {
            ToolPolicy::new(self.cfg.clone(), None, false).with_agent(self.agent.clone())
        }
        fn execute(&mut self, chat: u64, call: &ToolCall) -> Receiver<ToolResult> {
            self.executed.push((chat, call.name.clone(), call.arguments.clone()));
            let (tx, rx) = sync_channel(1);
            if self.later.contains(&call.name.as_str()) {
                self.waiting.push(tx);
            } else {
                tx.send(Ok(serde_json::json!({"ok": true}))).unwrap();
            }
            rx
        }
        fn request(&mut self, id: u64) {
            let c = self.chats.get_mut(&id).unwrap();
            c.next_request(&self.backend, &ChatMeta::default(), &TurnContext::default(), Arc::new(|| {}));
        }
        fn session_name(&self, id: u64) -> Option<String> {
            (id == 4).then(|| "build".to_string())
        }
    }

    fn cfg(level: McpLevel, max_steps: u32) -> LlmModel {
        LlmModel { level, max_steps, level_chosen: true, ..LlmModel::named("m") }
    }

    #[test]
    fn executes_with_the_chat_as_caller_and_asks_again() {
        let mut h = Fake::new(cfg(McpLevel::Read, 10), vec![vec![tc("a", "list_sessions", serde_json::json!({})), TurnEvent::Done], vec![TurnEvent::Delta("none".into()), TurnEvent::Done]]);
        h.send(7, "q");
        h.frames(7, 5);
        assert_eq!(h.executed, [(7, "list_sessions".to_string(), serde_json::json!({}))], "Aufrufer = der Chat");
        let roles: Vec<Role> = h.chat_ref(7).messages().iter().map(|m| m.role).collect();
        assert_eq!(roles, [Role::User, Role::Assistant, Role::Tool, Role::Assistant]);
        assert_eq!(h.chat_ref(7).status(), ChatStatus::Ready);
    }

    #[test]
    fn later_results_are_picked_up_in_a_later_frame() {
        let mut h = Fake::new(cfg(McpLevel::Control, 10), vec![vec![tc("a", "wait_for_status", serde_json::json!({"session": 2, "status": ["ready"]})), TurnEvent::Done], vec![TurnEvent::Delta("ok".into()), TurnEvent::Done]]);
        h.later = vec!["wait_for_status"];
        h.send(1, "wait");
        h.frames(1, 3);
        assert!(h.chat_ref(1).tool_running(), "wartet ohne Freigabe (NO_APPROVAL)");
        assert_eq!(h.chat_ref(1).status(), ChatStatus::Working);
        h.waiting.pop().unwrap().send(Ok(serde_json::json!({"status": "ready", "matched": true}))).unwrap();
        h.frames(1, 3);
        let m = h.chat_ref(1).messages();
        assert_eq!(m[2].status, Some(ToolStatus::Ok));
        assert!(m[2].content.contains("matched"));
        assert_eq!(m.last().unwrap().content, "ok");
    }

    #[test]
    fn approval_sets_the_status_and_the_target_label() {
        let mut h = Fake::new(cfg(McpLevel::Control, 10), vec![vec![tc("w", "send_text", serde_json::json!({"session": 4, "text": "dir"})), TurnEvent::Done], vec![TurnEvent::Delta("done".into()), TurnEvent::Done]]);
        h.send(3, "type");
        h.frames(3, 3);
        assert!(h.executed.is_empty());
        assert_eq!(h.chat_ref(3).status(), ChatStatus::Approval, "Freigabe → approval (braucht dich)");
        assert_eq!(h.chat_ref(3).approval_target.as_deref(), Some("session 4 \"build\""));
        // Veraltete Entscheidung (andere Kennung) wird ignoriert
        assert!(!h.chats.get_mut(&3).unwrap().decide("other", Decision::Allow));
        assert!(h.chats.get_mut(&3).unwrap().decide("w", Decision::Allow));
        h.frames(3, 4);
        assert_eq!(h.executed.len(), 1);
        assert_eq!(h.executed[0].0, 3);
        assert_eq!(h.chat_ref(3).status(), ChatStatus::Ready);
    }

    #[test]
    fn rejected_calls_count_and_the_round_stops() {
        // Das Modell ruft immer wieder ein unbekanntes Werkzeug auf: früher eine Endlosschleife. Seit #340 hält schon der
        // Schleifen-Schutz an (dreimal derselbe fehlerhafte Aufruf), vor der Höchstzahl 4.
        let mut h = Fake::new(cfg(McpLevel::Read, 4), vec![vec![tc("x", "no_such_tool", serde_json::json!({})), TurnEvent::Done]]);
        h.send(1, "loop");
        h.frames(1, 50);
        let c = h.chat_ref(1);
        assert!(c.paused && c.loop_stop, "angehalten");
        assert_eq!(c.steps, crate::chat::session::LOOP_LIMIT);
        assert!(h.executed.is_empty());
        assert_eq!(c.status(), ChatStatus::Ready);
    }

    #[test]
    fn agent_edits_wait_with_a_diff_preview_and_reads_run_freely() {
        let (d, env) = crate::chat::agent::paths::tests::tmp_root("driver");
        std::fs::write(d.join("a.txt"), "one\ntwo\n").unwrap();
        let mut h = Fake::new(
            cfg(McpLevel::Control, 10),
            vec![
                vec![tc("r", "read_file", serde_json::json!({"path": "a.txt"})), tc("e", "edit_file", serde_json::json!({"path": "a.txt", "old_string": "two", "new_string": "2"})), TurnEvent::Done],
                vec![TurnEvent::Delta("ok".into()), TurnEvent::Done],
            ],
        );
        h.agent = env;
        h.send(5, "edit");
        h.frames(5, 4);
        assert_eq!(h.executed.iter().map(|e| e.1.as_str()).collect::<Vec<_>>(), ["read_file"], "Lesen ohne Freigabe");
        let c = h.chat_ref(5);
        assert_eq!(c.status(), ChatStatus::Approval);
        assert_eq!(c.approval_target.as_deref(), Some("a.txt"));
        let (id, p) = c.approval_preview.clone().expect("Diff-Vorschau");
        assert_eq!(id, "e");
        let diff = p.diff.unwrap();
        assert_eq!((diff.added, diff.removed, p.path.as_str()), (1, 1, "a.txt"));
        assert!(h.chats.get_mut(&5).unwrap().decide("e", Decision::Allow));
        assert!(h.chat_ref(5).approval_preview.is_none(), "nach der Entscheidung weg");
        h.frames(5, 4);
        assert_eq!(h.executed.len(), 2);
        assert_eq!(h.chat_ref(5).status(), ChatStatus::Ready);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn target_label_without_name_or_session() {
        let call = |a: serde_json::Value| ToolCall { id: "c".into(), name: "send_text".into(), arguments: a };
        assert_eq!(target_label(&call(serde_json::json!({"session": 9})), |_| None).as_deref(), Some("session 9"));
        assert_eq!(target_label(&call(serde_json::json!({"text": "x"})), |_| Some("n".into())), None);
    }
}
