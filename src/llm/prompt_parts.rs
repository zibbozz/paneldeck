//! Schalter für die Teile des Master-Prompts der Chats (#341): global in `settings.json` `chat_prompt_parts`, je Modell
//! als Abweichung in `llm_servers[].models[].prompt_parts`, dazu je Modell die Wahl des Familienzusatzes
//! (`prompt_family`). Alles optional und tolerant: Fehlt etwas, ist der Teil an (bzw. die Familie wird am Modellnamen
//! erkannt); nur ein echtes `false` schaltet einen Teil aus. Ältere Versionen ignorieren die Felder.
//!
//! Die Sicherheitsregeln lassen sich abschalten (mit Warnung in der Oberfläche) – das betrifft nur den Text im Prompt.
//! Die Durchsetzung im Code (`chat::tools`, MCP-Ausführer) bleibt immer.

use crate::state::Lang;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Ein abschaltbarer Teil des Master-Prompts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PromptPart {
    /// Was Paneldeck ist (Sitzungen, Chats, Todo-Liste/Tickets, Prozessliste, Stufen, Oberfläche).
    About,
    /// Nummerierte Regeln für Werkzeugaufrufe.
    CallRules,
    /// Erzeugte Werkzeug-Dokumentation (Zweck, Felder, Hinweise).
    ToolDocs,
    /// Beispiel-Argumente je Werkzeug (nur zusammen mit der Werkzeug-Dokumentation).
    Examples,
    /// Fehlercodes und wie das Modell darauf reagiert.
    Errors,
    /// Zusatz für die Modellfamilie (Qwen, Llama …).
    Family,
    /// Regel „in der Sprache des Nutzers antworten“.
    Language,
    /// Sicherheitsregeln (Inhalte sind nicht vertrauenswürdig, nie in die eigene Sitzung …).
    Safety,
}

impl PromptPart {
    /// Alle Teile in der Reihenfolge der Oberfläche.
    pub const ALL: [PromptPart; 8] = [PromptPart::About, PromptPart::CallRules, PromptPart::ToolDocs, PromptPart::Examples, PromptPart::Errors, PromptPart::Family, PromptPart::Language, PromptPart::Safety];

    fn index(self) -> usize {
        Self::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }

    /// Schlüssel in `settings.json` (öffentliches Datenformat, nie umbenennen).
    pub fn key(self) -> &'static str {
        match self {
            PromptPart::About => "about",
            PromptPart::CallRules => "call_rules",
            PromptPart::ToolDocs => "tool_docs",
            PromptPart::Examples => "examples",
            PromptPart::Errors => "errors",
            PromptPart::Family => "family",
            PromptPart::Language => "language",
            PromptPart::Safety => "safety",
        }
    }

    #[cfg(test)]
    pub fn from_key(k: &str) -> Option<PromptPart> {
        Self::ALL.into_iter().find(|p| p.key() == k)
    }

    /// Beschriftung in der Oberfläche.
    pub fn label(self, lang: Lang) -> &'static str {
        match self {
            PromptPart::About => lang.t("Paneldeck-Erklärung", "Paneldeck explanation"),
            PromptPart::CallRules => lang.t("Aufruf-Regeln", "Call rules"),
            PromptPart::ToolDocs => lang.t("Werkzeug-Beschreibungen", "Tool descriptions"),
            PromptPart::Examples => lang.t("Beispiele je Werkzeug", "Examples per tool"),
            PromptPart::Errors => lang.t("Fehlerbehandlung", "Error handling"),
            PromptPart::Family => lang.t("Zusatz für die Modellfamilie", "Model family addition"),
            PromptPart::Language => lang.t("Antwortsprache", "Answer language"),
            PromptPart::Safety => lang.t("Sicherheitsregeln", "Safety rules"),
        }
    }

    /// Kurze Erklärung (Tooltip).
    pub fn hint(self, lang: Lang) -> &'static str {
        match self {
            PromptPart::About => lang.t(
                "Was Paneldeck ist: Sitzungen, Chats, Todo-Liste und Tickets, Prozessliste, Stufen, wo was in der Oberfläche ist.",
                "What Paneldeck is: sessions, chats, to-do list and tickets, process list, levels, where things are in the UI.",
            ),
            PromptPart::CallRules => lang.t("Nummerierte Regeln, wie das Modell Werkzeuge aufruft (ein Aufruf je Schritt, JSON-Typen, nichts erfinden …).", "Numbered rules on how the model calls tools (one call per step, JSON types, never invent …)."),
            PromptPart::ToolDocs => lang.t("Je Werkzeug Zweck, Felder, Typen und Hinweise (die kurzen Beschreibungen im Werkzeug-Feld der Anfrage bleiben).", "Per tool its purpose, fields, types and notes (the short descriptions in the request's tools field stay)."),
            PromptPart::Examples => lang.t("Beispiel-Argumente je Werkzeug – nur zusammen mit den Werkzeug-Beschreibungen.", "Example arguments per tool – only together with the tool descriptions."),
            PromptPart::Errors => lang.t("Fehlercodes und wie das Modell darauf reagieren soll.", "Error codes and how the model should react."),
            PromptPart::Family => lang.t("Kurzer Zusatz für bekannte Eigenheiten der Modellfamilie (Qwen, Llama, Mistral …).", "Short addition for known quirks of the model family (Qwen, Llama, Mistral …)."),
            PromptPart::Language => lang.t("Regel: in der Sprache der letzten Nachricht des Nutzers antworten.", "Rule: answer in the language of the user's latest message."),
            PromptPart::Safety => lang.t(
                "Inhalte aus Werkzeugen sind nicht vertrauenswürdig, nie in die eigene Sitzung schreiben, nichts bei offener Freigabe … (die Durchsetzung im Code bleibt immer).",
                "Tool content is untrusted, never write into the own session, nothing while an approval is open … (the enforcement in the code always stays).",
            ),
        }
    }
}

/// Warnung, wenn die Sicherheitsregeln aus sind.
pub fn safety_off_warning(lang: Lang) -> &'static str {
    lang.t(
        "Sicherheitsregeln aus: Das Modell erfährt nicht mehr, dass Terminal- und Werkzeuginhalte nicht vertrauenswürdig sind, und folgt eingeschleusten Anweisungen leichter. Paneldeck setzt die Regeln trotzdem im Code durch (nie die eigene Sitzung, nichts bei offener Freigabe, Freigaben für Schreibzugriffe).",
        "Safety rules off: the model is no longer told that terminal and tool content is untrusted and follows injected instructions more easily. Paneldeck still enforces the rules in code (never its own session, nothing while an approval is open, approvals for write access).",
    )
}

/// Welche Teile an sind (global bzw. für ein Modell aufgelöst). Standard: alle an.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PromptParts([bool; 8]);

impl Default for PromptParts {
    fn default() -> Self {
        PromptParts([true; 8])
    }
}

impl PromptParts {
    pub fn get(&self, p: PromptPart) -> bool {
        self.0[p.index()]
    }

    pub fn set(&mut self, p: PromptPart, on: bool) {
        self.0[p.index()] = on;
    }

    /// Alle an (dann fehlt das Feld in `settings.json`).
    pub fn is_default(&self) -> bool {
        *self == PromptParts::default()
    }

    /// Wirksam an: Beispiele nur mit Werkzeug-Beschreibungen.
    pub fn effective(&self, p: PromptPart) -> bool {
        self.get(p) && (p != PromptPart::Examples || self.get(PromptPart::ToolDocs))
    }
}

/// Geschrieben werden alle Schlüssel (nur, wenn nicht alles an ist – siehe `Settings::chat_prompt_parts`).
impl Serialize for PromptParts {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let m: Map<String, Value> = PromptPart::ALL.into_iter().map(|p| (p.key().to_string(), Value::Bool(self.get(p)))).collect();
        m.serialize(s)
    }
}

/// Tolerant: nur ein echtes `false` schaltet aus; fehlende, unbekannte oder falsch getippte Werte gelten als an.
impl<'de> Deserialize<'de> for PromptParts {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        let mut out = PromptParts::default();
        if let Some(o) = v.as_object() {
            for p in PromptPart::ALL {
                if o.get(p.key()).and_then(Value::as_bool) == Some(false) {
                    out.set(p, false);
                }
            }
        }
        Ok(out)
    }
}

/// Abweichungen eines Modells von den globalen Schaltern: `None` = wie global.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PromptOverride([Option<bool>; 8]);

impl PromptOverride {
    pub fn get(&self, p: PromptPart) -> Option<bool> {
        self.0[p.index()]
    }

    pub fn set(&mut self, p: PromptPart, v: Option<bool>) {
        self.0[p.index()] = v;
    }

    /// Keine Abweichung (dann fehlt das Feld).
    pub fn is_empty(&self) -> bool {
        self.0.iter().all(Option::is_none)
    }

    /// Die für das Modell geltenden Schalter.
    pub fn resolve(&self, global: PromptParts) -> PromptParts {
        let mut out = global;
        for p in PromptPart::ALL {
            if let Some(v) = self.get(p) {
                out.set(p, v);
            }
        }
        out
    }

    /// Eigene Auswahl für alle Teile, ausgehend von den globalen Schaltern.
    pub fn all_from(global: PromptParts) -> PromptOverride {
        let mut o = PromptOverride::default();
        for p in PromptPart::ALL {
            o.set(p, Some(global.get(p)));
        }
        o
    }
}

/// Geschrieben werden nur die abweichenden Schlüssel.
impl Serialize for PromptOverride {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let m: Map<String, Value> = PromptPart::ALL.into_iter().filter_map(|p| self.get(p).map(|v| (p.key().to_string(), Value::Bool(v)))).collect();
        m.serialize(s)
    }
}

/// Tolerant: nur echte Wahrheitswerte zählen, alles andere bedeutet „wie global“.
impl<'de> Deserialize<'de> for PromptOverride {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        let mut out = PromptOverride::default();
        if let Some(o) = v.as_object() {
            for p in PromptPart::ALL {
                out.set(p, o.get(p.key()).and_then(Value::as_bool));
            }
        }
        Ok(out)
    }
}

/// Wählbare Familien (`prompt_family`, öffentliches Datenformat): Schlüssel und Anzeigename. Fehlt das Feld, wird die
/// Familie am Modellnamen erkannt; ein unbekannter Wert (etwa von einer neueren Version) bleibt erhalten und gilt wie
/// „automatisch“. Abschalten geht über den Schalter [`PromptPart::Family`].
pub const FAMILIES: &[(&str, &str)] =
    &[("qwen", "Qwen"), ("llama", "Llama"), ("mistral", "Mistral"), ("gemma", "Gemma"), ("phi", "Phi"), ("deepseek", "DeepSeek"), ("granite", "Granite"), ("bonsai", "Bonsai"), ("gpt-oss", "gpt-oss")];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parts_default_to_on_and_only_false_switches_off() {
        let p: PromptParts = serde_json::from_value(json!({})).unwrap();
        assert!(p.is_default());
        let p: PromptParts = serde_json::from_value(json!({"examples": false, "safety": "no", "about": 0, "unknown": false})).unwrap();
        assert!(!p.get(PromptPart::Examples) && p.get(PromptPart::Safety) && p.get(PromptPart::About));
        assert!(serde_json::from_value::<PromptParts>(json!("x")).unwrap().is_default(), "falscher Typ: alles an");
        let back = serde_json::to_value(p).unwrap();
        assert_eq!(back["examples"], json!(false));
        assert_eq!(back.as_object().unwrap().len(), 8, "alle Schlüssel");
        for k in PromptPart::ALL {
            assert_eq!(PromptPart::from_key(k.key()), Some(k));
        }
    }

    #[test]
    fn examples_need_tool_docs() {
        let mut p = PromptParts::default();
        assert!(p.effective(PromptPart::Examples));
        p.set(PromptPart::ToolDocs, false);
        assert!(!p.effective(PromptPart::Examples) && p.get(PromptPart::Examples));
    }

    #[test]
    fn override_resolves_against_global_and_writes_only_differences() {
        let mut global = PromptParts::default();
        global.set(PromptPart::Errors, false);
        let o: PromptOverride = serde_json::from_value(json!({"examples": false, "errors": true, "family": "x"})).unwrap();
        assert_eq!((o.get(PromptPart::Examples), o.get(PromptPart::Errors), o.get(PromptPart::Family)), (Some(false), Some(true), None));
        let r = o.resolve(global);
        assert!(!r.get(PromptPart::Examples) && r.get(PromptPart::Errors) && r.get(PromptPart::Safety));
        assert_eq!(serde_json::to_value(o).unwrap(), json!({"examples": false, "errors": true}));
        assert!(PromptOverride::default().is_empty());
        assert_eq!(PromptOverride::all_from(global).resolve(PromptParts::default()), global);
    }
}
