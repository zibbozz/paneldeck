//! Füllstand des Kontexts eines Chats (#339, #341): wie viel der Kontextlänge die nächste Anfrage belegt, geschrieben
//! als `4342/8192 (53%)`. Kennt kein egui (die Anzeige steht in `app/chat_ui.rs`).
//!
//! Die Zahlen kommen aus der Antwort des Servers (`prompt_eval_count`/`eval_count` bei Ollama, `usage` bei
//! OpenAI-kompatiblen Servern; im Chat als [`super::ChatState::usage`]). Solange es keine gibt (vor der ersten Antwort,
//! während einer Anfrage, nach dem Leeren), zeigt der Chat Paneldecks Schätzung mit `~` davor – hochgerechnet mit dem
//! letzten Abgleich `token_scale`. Die Aufteilung (Systemprompt, Werkzeuge, Verlauf, Antwort-Reserve) ist immer
//! geschätzt; sie rechnet `llm_backend::prepare` bzw. [`super::llm_backend::context_estimate`] genau wie die Anfrage.

use crate::state::Lang;

/// Ab so viel Prozent des Platzes vor der Kürzung (Kontext ohne Antwort-Reserve) wechselt die Anzeige auf „fast voll“.
/// Bezugsgröße ist absichtlich nicht der ganze Kontext: Ab `max - reserve` fallen bereits alte Nachrichten heraus.
pub const NEAR_PERCENT: usize = 80;

/// Ein Server-Wert für die Eingabe gilt nur, wenn er mindestens ein Viertel der Schätzung ist: Paneldeck schätzt
/// vorsichtig (höchstens etwa doppelt so viel wie echte Tokenizer), viel kleinere Zahlen meldet ein Server, der nur die
/// nicht zwischengespeicherten Tokens zählt – dann bleibt die Schätzung.
pub const MIN_SERVER_SHARE: usize = 4;

/// Aufteilung einer (geplanten) Anfrage in Tokens, schon mit `token_scale` auf echte Tokens hochgerechnet.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ContextParts {
    /// Systemprompt samt Kontextzeile (Projekt, Ordner).
    pub system: usize,
    /// Werkzeug-Beschreibungen (JSON-Schemas).
    pub tools: usize,
    /// Verlauf, den das Modell sieht (nach der Kürzung).
    pub history: usize,
    /// Kontextlänge der Anfrage (eigene Angabe, sonst automatisch bzw. vom Server erkannt, sonst 8192).
    pub max: u32,
    /// Für die Antwort frei gehaltene Tokens (25 % des Kontexts).
    pub reserve: u32,
    /// Ältere Nachrichten passen schon nicht mehr hinein und fallen weg.
    pub dropped: bool,
    /// Faktor des Abgleichs mit dem Server (1,0 = noch keiner).
    pub scale: f32,
}

impl ContextParts {
    /// Geschätzte Tokens der ganzen Anfrage.
    pub fn total(&self) -> usize {
        self.system + self.tools + self.history
    }

    /// Ab so vielen Tokens fallen die ältesten Nachrichten heraus (Kontext ohne Antwort-Reserve).
    pub fn limit(&self) -> usize {
        self.max.saturating_sub(self.reserve) as usize
    }
}

/// Woher die angezeigte Zahl kommt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Zählung des Servers in der letzten Antwort: Eingabe und – falls gemeldet – Antwort.
    Server { prompt: u32, completion: Option<u32> },
    /// Paneldecks Schätzung (Anzeige mit `~`).
    Estimate,
}

/// Wie nah der Kontext an seiner Grenze ist (bestimmt die Farbe und den Hinweis).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillLevel {
    Normal,
    /// Ab [`NEAR_PERCENT`] des Platzes vor der Kürzung: bald fallen alte Nachrichten heraus.
    Near,
    /// Der Platz ist voll oder ältere Nachrichten fallen schon heraus.
    Full,
}

/// Angezeigter Füllstand.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextFill {
    pub used: usize,
    pub source: Source,
    pub parts: ContextParts,
}

/// Füllstand aus der Aufteilung (`parts`, `None` = unbekannt, z. B. Server gelöscht) und den Zahlen des Servers
/// (`usage` = Eingabe, Antwort der letzten Antwort; `None` vor der ersten Antwort und während einer Anfrage).
/// `context_start`: Die letzte Anfrage ließ schon ältere Nachrichten weg.
pub fn fill(parts: Option<&ContextParts>, usage: Option<(Option<u32>, Option<u32>)>, context_start: bool) -> Option<ContextFill> {
    let mut parts = *parts?;
    parts.dropped |= context_start;
    let server = usage.and_then(|(p, c)| p.filter(|p| *p > 0 && (*p as usize).saturating_mul(MIN_SERVER_SHARE) >= parts.total()).map(|p| (p, c)));
    Some(match server {
        Some((prompt, completion)) => ContextFill { used: prompt as usize + completion.unwrap_or(0) as usize, source: Source::Server { prompt, completion }, parts },
        None => ContextFill { used: parts.total(), source: Source::Estimate, parts },
    })
}

impl ContextFill {
    pub fn estimated(&self) -> bool {
        self.source == Source::Estimate
    }

    /// Anteil am ganzen Kontext in Prozent (gerundet; über 100, wenn mehr belegt ist als der Kontext fasst).
    pub fn percent(&self) -> usize {
        let max = self.parts.max.max(1) as usize;
        (self.used.saturating_mul(100) + max / 2) / max
    }

    /// `4342/8192 (53%)`, geschätzt `~4342/8192 (53%)`.
    pub fn text(&self) -> String {
        format!("{}{}/{} ({}%)", if self.estimated() { "~" } else { "" }, self.used, self.parts.max, self.percent())
    }

    pub fn level(&self) -> FillLevel {
        let limit = self.parts.limit();
        if self.parts.dropped || self.used >= limit {
            FillLevel::Full
        } else if self.used.saturating_mul(100) >= limit.saturating_mul(NEAR_PERCENT) {
            FillLevel::Near
        } else {
            FillLevel::Normal
        }
    }

    /// Kurzer Hinweis neben der Zahl (nur nahe der Grenze).
    pub fn hint(&self, lang: Lang) -> Option<&'static str> {
        match self.level() {
            FillLevel::Normal => None,
            FillLevel::Near => Some(lang.t("bald fallen alte Nachrichten heraus", "old messages will drop out soon")),
            FillLevel::Full => Some(lang.t("alte Nachrichten fallen heraus", "old messages drop out")),
        }
    }

    /// Text des Tooltips: Aufteilung, Antwort-Reserve, Grenze der Kürzung, Herkunft der Zahl.
    pub fn tooltip(&self, lang: Lang) -> String {
        let p = &self.parts;
        let de = lang == Lang::De;
        let mut lines = Vec::new();
        lines.push(if de {
            format!("Kontext: {} von {} Tokens belegt ({} %)", self.used, p.max, self.percent())
        } else {
            format!("Context: {} of {} tokens used ({}%)", self.used, p.max, self.percent())
        });
        let fixed = p.system + p.tools;
        lines.push(if de {
            format!("Systemprompt + Werkzeuge: ~{fixed} (Prompt ~{}, Werkzeuge ~{})", p.system, p.tools)
        } else {
            format!("System prompt + tools: ~{fixed} (prompt ~{}, tools ~{})", p.system, p.tools)
        });
        // Mit Zahl des Servers: der Rest nach Systemprompt und Werkzeugen (deren Anteil bleibt geschätzt).
        let history = match self.source {
            Source::Server { .. } => self.used.saturating_sub(fixed),
            Source::Estimate => p.history,
        };
        lines.push(if de { format!("Verlauf: ~{history}") } else { format!("History: ~{history}") });
        let reserve_pct = (p.reserve as usize * 100 + p.max.max(1) as usize / 2) / p.max.max(1) as usize;
        lines.push(if de {
            format!("Reserve für die Antwort: {} ({reserve_pct} %)", p.reserve)
        } else {
            format!("Reserve for the answer: {} ({reserve_pct}%)", p.reserve)
        });
        lines.push(if de {
            format!("Ab {} Tokens fallen die ältesten Nachrichten heraus – das Modell sieht sie dann nicht mehr.", p.limit())
        } else {
            format!("From {} tokens on, the oldest messages drop out – the model no longer sees them.", p.limit())
        });
        lines.push(match (self.source, de) {
            (Source::Server { prompt, completion: Some(c) }, true) => format!("Quelle: Zählung des Servers in der letzten Antwort ({prompt} Eingabe + {c} Antwort)."),
            (Source::Server { prompt, completion: Some(c) }, false) => format!("Source: the server's count in its last answer ({prompt} in + {c} out)."),
            (Source::Server { prompt, completion: None }, true) => format!("Quelle: Zählung des Servers in der letzten Antwort ({prompt} Eingabe)."),
            (Source::Server { prompt, completion: None }, false) => format!("Source: the server's count in its last answer ({prompt} in)."),
            (Source::Estimate, true) if p.scale > 1.0 => "Quelle: Schätzung von Paneldeck (~), abgeglichen mit der letzten Zählung des Servers.".into(),
            (Source::Estimate, false) if p.scale > 1.0 => "Source: Paneldeck's estimate (~), matched to the server's last count.".into(),
            (Source::Estimate, true) => "Quelle: Schätzung von Paneldeck (~) – genaue Zahlen kommen mit der nächsten Antwort des Servers.".into(),
            (Source::Estimate, false) => "Source: Paneldeck's estimate (~) – exact numbers come with the server's next answer.".into(),
        });
        match self.level() {
            FillLevel::Normal => {}
            FillLevel::Near => lines.push(
                lang.t("Der Kontext ist fast voll: Bald fallen die ältesten Nachrichten heraus.", "The context is almost full: the oldest messages will drop out soon.").into(),
            ),
            FillLevel::Full if p.dropped => lines.push(
                lang.t(
                    "Ältere Nachrichten passen nicht mehr in den Kontext und fallen heraus (Kontextlänge in Einstellungen → Modelle → Server & Modelle erhöhen).",
                    "Older messages no longer fit into the context and drop out (raise the context length in Settings → Models → Servers & models).",
                )
                .into(),
            ),
            FillLevel::Full => lines.push(
                lang.t("Der Kontext ist voll: Mit der nächsten Nachricht fallen die ältesten heraus.", "The context is full: with the next message the oldest ones drop out.").into(),
            ),
        }
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(system: usize, tools: usize, history: usize, max: u32) -> ContextParts {
        ContextParts { system, tools, history, max, reserve: max / 4, dropped: false, scale: 1.0 }
    }

    #[test]
    fn text_is_used_slash_max_with_percent_and_a_tilde_for_estimates() {
        let p = parts(3000, 1000, 342, 8192);
        let f = fill(Some(&p), None, false).unwrap();
        assert_eq!((f.used, f.source, f.text()), (4342, Source::Estimate, "~4342/8192 (53%)".to_string()));
        // Zahl des Servers: Eingabe + Antwort, ohne `~`.
        let f = fill(Some(&p), Some((Some(4000), Some(342))), false).unwrap();
        assert_eq!((f.used, f.text()), (4342, "4342/8192 (53%)".to_string()));
        assert_eq!(f.source, Source::Server { prompt: 4000, completion: Some(342) });
        // Nur die Eingabe gemeldet.
        assert_eq!(fill(Some(&p), Some((Some(4000), None)), false).unwrap().text(), "4000/8192 (49%)");
        // Keine Eingabe gemeldet (nur Antwort) oder 0: Schätzung.
        assert!(fill(Some(&p), Some((None, Some(5))), false).unwrap().estimated());
        assert!(fill(Some(&p), Some((Some(0), Some(5))), false).unwrap().estimated());
        // Unbekannte Aufteilung: keine Anzeige.
        assert_eq!(fill(None, Some((Some(1), None)), false), None);
    }

    #[test]
    fn implausibly_small_server_counts_fall_back_to_the_estimate() {
        let p = parts(3000, 1000, 0, 8192);
        assert!(fill(Some(&p), Some((Some(999), Some(10))), false).unwrap().estimated(), "weniger als ein Viertel: zwischengespeichert gezählt");
        assert!(!fill(Some(&p), Some((Some(1000), Some(10))), false).unwrap().estimated());
    }

    #[test]
    fn percent_rounds_and_may_exceed_100() {
        let f = |used: usize, max: u32| ContextFill { used, source: Source::Estimate, parts: parts(used, 0, 0, max) }.percent();
        assert_eq!((f(0, 8192), f(40, 8192), f(41, 8192), f(8192, 8192), f(9000, 8192)), (0, 0, 1, 100, 110));
        assert_eq!(f(5, 0), 500, "max 0 wird nie durch 0 geteilt");
    }

    #[test]
    fn level_and_hint_follow_the_trimming_limit() {
        // 8192, Reserve 2048 → Grenze 6144, „fast voll“ ab 80 % davon = 4915,2.
        let at = |used: usize| fill(Some(&parts(used, 0, 0, 8192)), None, false).unwrap();
        assert_eq!(at(4915).level(), FillLevel::Normal);
        assert_eq!(at(4916).level(), FillLevel::Near);
        assert_eq!(at(6143).level(), FillLevel::Near);
        assert_eq!(at(6144).level(), FillLevel::Full);
        assert_eq!(at(100).hint(Lang::En), None);
        assert_eq!(at(5000).hint(Lang::En), Some("old messages will drop out soon"));
        assert_eq!(at(5000).hint(Lang::De), Some("bald fallen alte Nachrichten heraus"));
        assert_eq!(at(7000).hint(Lang::En), Some("old messages drop out"));
        // Schon gekürzt (Anfrage oder Schätzung): voll, auch bei wenig Belegung.
        assert_eq!(fill(Some(&parts(100, 0, 0, 8192)), None, true).unwrap().level(), FillLevel::Full);
        let mut p = parts(100, 0, 0, 8192);
        p.dropped = true;
        assert_eq!(fill(Some(&p), None, false).unwrap().level(), FillLevel::Full);
    }

    #[test]
    fn tooltip_lists_the_breakdown_reserve_and_source_in_both_languages() {
        let p = parts(2100, 800, 1442, 8192);
        let est = fill(Some(&p), None, false).unwrap().tooltip(Lang::En);
        for want in [
            "Context: 4342 of 8192 tokens used (53%)",
            "System prompt + tools: ~2900 (prompt ~2100, tools ~800)",
            "History: ~1442",
            "Reserve for the answer: 2048 (25%)",
            "From 6144 tokens on, the oldest messages drop out",
            "Source: Paneldeck's estimate (~) – exact numbers come with the server's next answer.",
        ] {
            assert!(est.contains(want), "{want}: {est}");
        }
        let srv = fill(Some(&p), Some((Some(4000), Some(342))), false).unwrap().tooltip(Lang::De);
        for want in ["Kontext: 4342 von 8192 Tokens belegt (53 %)", "Systemprompt + Werkzeuge: ~2900", "Verlauf: ~1442", "Reserve für die Antwort: 2048 (25 %)", "Quelle: Zählung des Servers in der letzten Antwort (4000 Eingabe + 342 Antwort)."] {
            assert!(srv.contains(want), "{want}: {srv}");
        }
        let mut scaled = p;
        scaled.scale = 1.3;
        assert!(fill(Some(&scaled), None, false).unwrap().tooltip(Lang::En).contains("matched to the server's last count"));
        let full = fill(Some(&p), None, true).unwrap().tooltip(Lang::En);
        assert!(full.contains("Older messages no longer fit into the context"), "{full}");
        let near = fill(Some(&parts(5000, 0, 0, 8192)), None, false).unwrap().tooltip(Lang::De);
        assert!(near.contains("Der Kontext ist fast voll"), "{near}");
    }
}
