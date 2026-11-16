//! Grobe Token-Schätzung und Kürzen des Verlaufs auf ein Token-Budget.

use super::{ChatMessage, Role};

/// Vorsichtige Schätzung: ASCII-Zeichen / 3 (aufgerundet) plus ein Token je Nicht-ASCII-Zeichen.
///
/// Warum so vorsichtig: Für englischen Fließtext liegen gängige BPE-Tokenizer (Llama, Qwen, Mistral) bei etwa 4 Zeichen
/// je Token, für JSON, Code, Pfade und Terminalausgabe – genau das, was Werkzeug-Ergebnisse enthalten – eher bei 2,5–3.
/// Umlaute, Emoji und CJK kosten oft ein Token oder mehr je Zeichen. Zu hoch geschätzt kostet nur etwas Verlauf; zu
/// niedrig geschätzt schneidet Ollama still den Anfang ab – samt Systemprompt. Die tatsächliche Zahl des Servers
/// gleicht der Chat zusätzlich über einen Faktor ab (`TurnContext::token_scale`).
pub fn estimate_tokens(s: &str) -> usize {
    let (ascii, other) = s.chars().fold((0usize, 0usize), |(a, o), c| if c.is_ascii() { (a + 1, o) } else { (a, o + 1) });
    ascii.div_ceil(3) + other
}

/// Geschätzte Tokens einer Nachricht (Inhalt, Werkzeugaufrufe, kleiner Aufschlag für Rolle/Formatierung).
pub fn message_tokens(m: &ChatMessage) -> usize {
    let calls: usize = m.tool_calls.iter().map(|c| estimate_tokens(&c.name) + estimate_tokens(&c.arguments.to_string()) + 3).sum();
    4 + estimate_tokens(&m.content) + calls
}

/// Kürzt den Verlauf auf `budget` Tokens: Die erste Systemnachricht bleibt immer, die ältesten Runden fallen als Ganzes weg
/// (eine Runde = Nutzer-Nachricht samt folgenden Assistent- und Werkzeug-Nachrichten; Nachrichten vor der ersten
/// Nutzer-Nachricht bilden eine eigene Runde). Die letzte Runde bleibt immer, auch wenn sie allein zu groß ist.
///
/// Rückgabe: Index der ersten behaltenen Nachricht nach der Systemnachricht. Das Modell sieht
/// `[messages[0] falls System] + messages[start..]`. Nichts entfernt ⇔ `start == first_turn(messages)`.
pub fn trim_to_budget(messages: &[ChatMessage], budget: usize) -> usize {
    let first = first_turn(messages);
    let mut total: usize = messages.iter().map(message_tokens).sum();
    let mut starts: Vec<usize> = (first..messages.len()).filter(|&i| i == first || messages[i].role == Role::User).collect();
    starts.push(messages.len());
    let mut k = 0;
    // starts[k]..starts[k+1] ist eine Runde; die letzte (starts.len()-2) bleibt.
    while total > budget && k + 2 < starts.len() {
        total -= messages[starts[k]..starts[k + 1]].iter().map(message_tokens).sum::<usize>();
        k += 1;
    }
    starts[k].min(messages.len())
}

/// Index der ersten Nachricht nach der (optionalen) Systemnachricht am Anfang.
pub fn first_turn(messages: &[ChatMessage]) -> usize {
    usize::from(messages.first().is_some_and(|m| m.role == Role::System))
}

/// Die Nachrichten, die das Modell nach `trim_to_budget` sieht.
pub fn visible(messages: &[ChatMessage], start: usize) -> Vec<ChatMessage> {
    let first = first_turn(messages);
    messages[..first].iter().chain(messages[start.max(first).min(messages.len())..].iter()).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::ToolCall;

    fn m(role: Role, n: usize) -> ChatMessage {
        ChatMessage { role, content: "x".repeat(n * 3), ..Default::default() }
    }

    #[test]
    fn estimate_is_conservative() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("a"), 1);
        assert_eq!(estimate_tokens("abc"), 1);
        assert_eq!(estimate_tokens("abcd"), 2);
        assert_eq!(estimate_tokens("äöüß"), 4, "Nicht-ASCII: ein Token je Zeichen");
        assert_eq!(estimate_tokens("Grüße"), 1 + 2, "G, r, e = 3 ASCII → 1; ü, ß → 2");
        let c = ChatMessage { role: Role::Assistant, tool_calls: vec![ToolCall { id: "1".into(), name: "abcd".into(), arguments: serde_json::json!({}) }], ..Default::default() };
        assert_eq!(message_tokens(&c), 4 + 0 + 2 + 1 + 3);
    }

    #[test]
    fn drops_oldest_whole_turns() {
        // Jede Nachricht = 4 + 10 = 14 Tokens.
        let msgs = vec![m(Role::System, 10), m(Role::User, 10), m(Role::Assistant, 10), m(Role::Tool, 10), m(Role::Assistant, 10), m(Role::User, 10), m(Role::Assistant, 10), m(Role::User, 10)];
        assert_eq!(trim_to_budget(&msgs, 1000), 1, "nothing dropped");
        // Gesamt 112; ohne erste Runde (4 Nachrichten = 56) bleiben 56.
        assert_eq!(trim_to_budget(&msgs, 100), 5);
        assert_eq!(trim_to_budget(&msgs, 56), 5);
        assert_eq!(trim_to_budget(&msgs, 55), 7);
        // Die letzte Runde bleibt immer.
        assert_eq!(trim_to_budget(&msgs, 0), 7);
        let v = visible(&msgs, 7);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].role, v[1].role), (Role::System, Role::User));
    }

    #[test]
    fn without_system_prompt_and_leading_assistant() {
        let msgs = vec![m(Role::Assistant, 10), m(Role::User, 10), m(Role::Assistant, 10)];
        assert_eq!(trim_to_budget(&msgs, 1000), 0);
        assert_eq!(trim_to_budget(&msgs, 30), 1);
        assert_eq!(visible(&msgs, 1).len(), 2);
        assert_eq!(trim_to_budget(&[], 0), 0);
        let only_sys = vec![m(Role::System, 100)];
        assert_eq!(trim_to_budget(&only_sys, 0), 1);
        assert_eq!(visible(&only_sys, 1).len(), 1);
    }
}
