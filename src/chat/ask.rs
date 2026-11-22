//! Werkzeug `ask_user` der Chats (#356): Das Modell stellt dem Nutzer bis zu [`MAX_QUESTIONS`] Fragen mit je
//! [`MIN_OPTIONS`]–[`MAX_OPTIONS`] Antworten (Einfach- oder Mehrfachauswahl, immer mit „Andere …“ und freiem Text).
//! Antworten, die das Modell als `recommended` markiert, stehen vorn und tragen „(Empfohlen)“. Der Chat zeigt die Fragen
//! als ein kleines Formular mit einem Knopf „Absenden“ (`app/chat_ask_ui.rs`); die Antwort geht als Ergebnis des
//! Aufrufs an das Modell zurück, „Abbrechen“ sagt ihm, dass der Nutzer nicht antworten möchte. Es gibt kein Zeitlimit;
//! solange die Fragen offen sind, steht der Chat auf „wartet auf dich“ (wie bei einer Freigabe).
//!
//! Kennt weder egui noch die App: [`parse`] prüft die Argumente (mit hilfreichen Fehlern im Stil von `repair`),
//! [`AskForm`] hält den Zustand des Formulars und baut das Ergebnis. Der Planungsmodus (#358) nutzt dasselbe.

use super::agent::{err, ToolErr};
use super::tools::ToolResult;
use serde_json::{json, Map, Value};
use std::sync::mpsc::SyncSender;

/// Höchstens so viele Fragen je Aufruf.
pub const MAX_QUESTIONS: usize = 4;
/// So viele Antworten je Frage (ohne „Andere …“, das immer dazukommt).
pub const MIN_OPTIONS: usize = 2;
pub const MAX_OPTIONS: usize = 6;
/// Längen (Zeichen): Frage, kurze Überschrift, Antwort, Beschreibung einer Antwort, freier Text des Nutzers.
pub const MAX_QUESTION_CHARS: usize = 300;
pub const MAX_HEADER_CHARS: usize = 30;
pub const MAX_LABEL_CHARS: usize = 80;
pub const MAX_DESCRIPTION_CHARS: usize = 200;
pub const MAX_OTHER_CHARS: usize = 2000;

/// Beispiel für Fehlermeldungen (gültig gegen das Schema, per Test geprüft).
pub const EXAMPLE: &str = r#"{"questions": [{"question": "Which database should I use?", "options": [{"label": "SQLite", "recommended": true}, {"label": "PostgreSQL"}]}]}"#;

/// Eine Antwortmöglichkeit.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct AskOption {
    pub label: String,
    /// Kurze Erklärung (leer = keine).
    pub description: String,
    /// Vom Modell empfohlen (steht vorn, mit „(Empfohlen)“).
    pub recommended: bool,
}

/// Eine Frage; `options` sind schon sortiert (empfohlene zuerst, sonst in der Reihenfolge des Modells).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct AskQuestion {
    pub question: String,
    /// Kurze Überschrift (z. B. „Datenbank“; leer = keine).
    pub header: String,
    /// Mehrfachauswahl.
    pub multi: bool,
    pub options: Vec<AskOption>,
}

fn bad(msg: impl Into<String>) -> ToolErr {
    err("bad_args", format!("{} Example: {EXAMPLE}", msg.into()))
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

/// Wahrheitswert auch aus Text („true“, „yes“) oder 0/1.
fn truthy(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_i64().map(|i| i != 0),
        Value::String(s) => match s.trim().to_lowercase().as_str() {
            "true" | "yes" | "1" => Some(true),
            "false" | "no" | "0" | "" => Some(false),
            _ => None,
        },
        Value::Null => Some(false),
        _ => None,
    }
}

/// Wäre diese Antwort das „Andere …“, das die Oberfläche ohnehin anbietet?
fn is_other(label: &str) -> bool {
    let l = label.trim().trim_end_matches(['…', '.', ':', ' ']).trim().to_lowercase();
    matches!(l.as_str(), "other" | "others" | "something else" | "andere" | "anderes" | "sonstiges" | "etwas anderes")
}

/// Text eines Feldes (`key` oder ein Alias); falscher Typ → Fehler mit Feldname.
fn text_field(o: &Map<String, Value>, keys: &[&str], path: &str) -> Result<Option<String>, ToolErr> {
    for k in keys {
        match o.get(*k) {
            None | Some(Value::Null) => continue,
            Some(Value::String(s)) => return Ok(Some(s.trim().to_string())),
            Some(Value::Number(n)) => return Ok(Some(n.to_string())),
            Some(_) => return Err(bad(format!("{path}.{k} must be a string."))),
        }
    }
    Ok(None)
}

/// Argumente von `ask_user` prüfen und normalisieren. `Ok((Fragen, Korrekturen))`; Korrekturen sind kurze englische
/// Notizen (z. B. „removed the option “Other” – the form always offers Other …“), die im Ergebnis stehen.
///
/// Nachsichtig bei Formen, die kleine Modelle oft senden: eine einzelne Frage ohne `questions`, `questions` als Objekt
/// statt Liste, Antworten als Text statt Objekt, `multiSelect`/`multiple`, `recommended` als Text. Streng bei allem, was
/// das Formular unklar machte: leere oder zu lange Texte, zu wenige/viele Antworten, doppelte Antworten.
pub fn parse(args: &Value) -> Result<(Vec<AskQuestion>, Vec<String>), ToolErr> {
    let mut fixes = Vec::new();
    let Some(obj) = args.as_object() else { return Err(bad("The arguments must be an object with \"questions\".")) };
    let list: Vec<Value> = match obj.get("questions") {
        Some(Value::Array(a)) => a.clone(),
        Some(o @ Value::Object(_)) => {
            fixes.push("questions: one object → a list with one question".to_string());
            vec![o.clone()]
        }
        Some(Value::String(s)) => match serde_json::from_str::<Value>(s) {
            Ok(Value::Array(a)) => {
                fixes.push("questions: JSON text → list".to_string());
                a
            }
            _ => return Err(bad("questions must be a list of question objects, not a string.")),
        },
        Some(_) => return Err(bad("questions must be a list of question objects.")),
        // Eine einzelne Frage ohne Hülle: {"question": …, "options": […]}
        None if obj.contains_key("question") => {
            fixes.push("a single question without \"questions\" → questions: [ … ]".to_string());
            vec![args.clone()]
        }
        None => return Err(bad("missing required field \"questions\" (a list of 1–4 questions).")),
    };
    if list.is_empty() {
        return Err(bad("questions is empty – ask at least one question."));
    }
    if list.len() > MAX_QUESTIONS {
        return Err(bad(format!("at most {MAX_QUESTIONS} questions per call, not {} – ask the rest in a later call.", list.len())));
    }
    let mut out = Vec::new();
    for (qi, q) in list.iter().enumerate() {
        let path = format!("questions[{qi}]");
        let Some(qo) = q.as_object() else { return Err(bad(format!("{path} must be an object with \"question\" and \"options\"."))) };
        let question = text_field(qo, &["question", "text", "prompt"], &path)?.unwrap_or_default();
        if question.is_empty() {
            return Err(bad(format!("{path}.question is missing or empty.")));
        }
        if chars(&question) > MAX_QUESTION_CHARS {
            return Err(bad(format!("{path}.question is too long ({} characters, at most {MAX_QUESTION_CHARS}).", chars(&question))));
        }
        let header = text_field(qo, &["header", "title"], &path)?.unwrap_or_default();
        let header = if chars(&header) > MAX_HEADER_CHARS {
            fixes.push(format!("{path}.header: shortened to {MAX_HEADER_CHARS} characters"));
            header.chars().take(MAX_HEADER_CHARS).collect()
        } else {
            header
        };
        let mut multi = false;
        for k in ["multi_select", "multiSelect", "multiple", "multi"] {
            if let Some(v) = qo.get(k) {
                multi = truthy(v).ok_or_else(|| bad(format!("{path}.{k} must be true or false.")))?;
                if k != "multi_select" {
                    fixes.push(format!("{path}.{k} → multi_select"));
                }
                break;
            }
        }
        let raw = match qo.get("options").or_else(|| qo.get("choices")) {
            Some(Value::Array(a)) => a.clone(),
            Some(_) => return Err(bad(format!("{path}.options must be a list of {MIN_OPTIONS}–{MAX_OPTIONS} options."))),
            None => return Err(bad(format!("{path}.options is missing (a list of {MIN_OPTIONS}–{MAX_OPTIONS} options)."))),
        };
        let mut options: Vec<AskOption> = Vec::new();
        for (oi, o) in raw.iter().enumerate() {
            let op = format!("{path}.options[{oi}]");
            let opt = match o {
                Value::String(s) => AskOption { label: s.trim().to_string(), ..AskOption::default() },
                Value::Object(m) => {
                    let label = text_field(m, &["label", "text", "value", "name"], &op)?.unwrap_or_default();
                    let description = text_field(m, &["description", "detail", "details"], &op)?.unwrap_or_default();
                    let recommended = match m.get("recommended") {
                        None => false,
                        Some(v) => truthy(v).ok_or_else(|| bad(format!("{op}.recommended must be true or false.")))?,
                    };
                    AskOption { label, description, recommended }
                }
                _ => return Err(bad(format!("{op} must be an object with \"label\" (or a string)."))),
            };
            if opt.label.is_empty() {
                return Err(bad(format!("{op}.label is missing or empty.")));
            }
            if chars(&opt.label) > MAX_LABEL_CHARS {
                return Err(bad(format!("{op}.label is too long ({} characters, at most {MAX_LABEL_CHARS}); put details into \"description\".", chars(&opt.label))));
            }
            if chars(&opt.description) > MAX_DESCRIPTION_CHARS {
                return Err(bad(format!("{op}.description is too long ({} characters, at most {MAX_DESCRIPTION_CHARS}).", chars(&opt.description))));
            }
            if is_other(&opt.label) {
                fixes.push(format!("{op}: removed “{}” – the form always offers “Other …” with free text", opt.label));
                continue;
            }
            if options.iter().any(|x| x.label.to_lowercase() == opt.label.to_lowercase()) {
                return Err(bad(format!("{op}: the option “{}” appears twice – every option needs its own label.", opt.label)));
            }
            options.push(opt);
        }
        if options.len() < MIN_OPTIONS {
            return Err(bad(format!("{path} needs at least {MIN_OPTIONS} options (not counting “Other”, which is always added), it has {}. For an open question use plain text instead.", options.len())));
        }
        if options.len() > MAX_OPTIONS {
            return Err(bad(format!("{path} has {} options, at most {MAX_OPTIONS} are allowed – merge or drop some.", options.len())));
        }
        // Empfohlene zuerst; sonst bleibt die Reihenfolge des Modells.
        let (mut rec, rest): (Vec<AskOption>, Vec<AskOption>) = options.into_iter().partition(|o| o.recommended);
        rec.extend(rest);
        out.push(AskQuestion { question, header, multi, options: rec });
    }
    Ok((out, fixes))
}

/// Auswahl des Nutzers zu einer Frage.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct AnswerState {
    /// Je Antwort (Reihenfolge wie `AskQuestion::options`): gewählt?
    pub selected: Vec<bool>,
    /// „Andere …“ gewählt.
    pub other: bool,
    /// Freier Text zu „Andere …“.
    pub other_text: String,
}

/// Ein offenes Formular (eine Frage-Runde des Modells).
pub struct AskForm {
    /// Kennung des `ask_user`-Aufrufs.
    pub call_id: String,
    pub questions: Vec<AskQuestion>,
    pub answers: Vec<AnswerState>,
    /// Korrekturen an den Argumenten (stehen im Ergebnis).
    pub fixes: Vec<String>,
    /// Kanal für das Ergebnis (der laufende Aufruf wartet darauf).
    reply: Option<SyncSender<ToolResult>>,
}

impl std::fmt::Debug for AskForm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AskForm").field("call_id", &self.call_id).field("questions", &self.questions).field("answers", &self.answers).finish()
    }
}

/// Was der Nutzer mit dem Formular tut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AskReply {
    Submit,
    Cancel,
}

impl AskForm {
    pub fn new(call_id: impl Into<String>, questions: Vec<AskQuestion>, fixes: Vec<String>, reply: Option<SyncSender<ToolResult>>) -> Self {
        let answers = questions.iter().map(|q| AnswerState { selected: vec![false; q.options.len()], ..AnswerState::default() }).collect();
        AskForm { call_id: call_id.into(), questions, answers, fixes, reply }
    }

    /// Antwort `i` der Frage `q` an- bzw. abwählen. Einfachauswahl: wählt nur sie (und nicht „Andere …“).
    pub fn toggle(&mut self, q: usize, i: usize) {
        let multi = self.questions.get(q).is_some_and(|x| x.multi);
        let Some(a) = self.answers.get_mut(q) else { return };
        if i >= a.selected.len() {
            return;
        }
        if multi {
            a.selected[i] = !a.selected[i];
        } else {
            let on = !a.selected[i];
            a.selected.iter_mut().for_each(|s| *s = false);
            a.selected[i] = on;
            if on {
                a.other = false;
            }
        }
    }

    /// „Andere …“ der Frage `q` an- bzw. abwählen (Einfachauswahl: dann ohne die übrigen Antworten).
    pub fn set_other(&mut self, q: usize, on: bool) {
        let multi = self.questions.get(q).is_some_and(|x| x.multi);
        let Some(a) = self.answers.get_mut(q) else { return };
        a.other = on;
        if on && !multi {
            a.selected.iter_mut().for_each(|s| *s = false);
        }
    }

    /// Ist die Frage `q` beantwortet? Mindestens eine Antwort; „Andere …“ zählt nur mit Text.
    pub fn answered(&self, q: usize) -> bool {
        let Some(a) = self.answers.get(q) else { return false };
        let other_ok = a.other && !a.other_text.trim().is_empty();
        let other_bad = a.other && a.other_text.trim().is_empty();
        (a.selected.iter().any(|s| *s) || other_ok) && !other_bad
    }

    /// Sind alle Fragen beantwortet (Absenden möglich)?
    pub fn complete(&self) -> bool {
        (0..self.questions.len()).all(|q| self.answered(q))
    }

    /// Ergebnis für das Modell nach „Absenden“.
    pub fn result(&self) -> Value {
        let answers: Vec<Value> = self
            .questions
            .iter()
            .zip(&self.answers)
            .map(|(q, a)| {
                let picked: Vec<&str> = q.options.iter().zip(&a.selected).filter(|(_, s)| **s).map(|(o, _)| o.label.as_str()).collect();
                let mut v = json!({"question": q.question, "selected": picked});
                if a.other && !a.other_text.trim().is_empty() {
                    let t: String = a.other_text.trim().chars().take(MAX_OTHER_CHARS).collect();
                    v["other"] = json!(t);
                }
                v
            })
            .collect();
        let mut v = json!({
            "answered": true,
            "answers": answers,
            "note": "The user's answers. `selected` lists the chosen options by label; `other` is the user's own text when they chose “Other”. Continue with these answers."
        });
        if !self.fixes.is_empty() {
            v["corrected"] = json!(self.fixes);
        }
        v
    }

    /// Ergebnis für das Modell nach „Abbrechen“.
    pub fn cancel_result() -> Value {
        json!({
            "answered": false,
            "cancelled": true,
            "message": "The user closed the questions without answering – they do not want to answer them now. Do not ask the same questions again; continue with what you know, make a sensible choice and say so, or ask in plain text."
        })
    }

    /// Antwort des Nutzers abschicken: Ergebnis über den Kanal an den wartenden Aufruf. `false`, wenn „Absenden“ noch
    /// nicht geht (unvollständig) oder schon geantwortet wurde.
    pub fn reply(&mut self, r: AskReply) -> bool {
        if r == AskReply::Submit && !self.complete() {
            return false;
        }
        let Some(tx) = self.reply.take() else { return false };
        let v = match r {
            AskReply::Submit => self.result(),
            AskReply::Cancel => Self::cancel_result(),
        };
        let _ = tx.try_send(Ok(v));
        true
    }
}

/// Kurzinfo für das Protokoll: Zahl der Fragen (nie die Texte).
pub fn log_info(args: &Value) -> String {
    match args.get("questions") {
        Some(Value::Array(a)) => format!("{} questions", a.len()),
        _ => "1 question".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(v: Value) -> Result<(Vec<AskQuestion>, Vec<String>), ToolErr> {
        parse(&v)
    }

    #[test]
    fn the_example_parses_and_recommended_options_come_first() {
        let (qs, fixes) = parse(&serde_json::from_str(EXAMPLE).unwrap()).unwrap();
        assert!(fixes.is_empty());
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0].options[0].label, "SQLite");
        let (qs, _) = q(json!({"questions": [{"question": "Pick", "multi_select": true, "options": [{"label": "a"}, {"label": "b", "recommended": true}, {"label": "c"}, {"label": "d", "recommended": "true"}]}]})).unwrap();
        let labels: Vec<&str> = qs[0].options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels, ["b", "d", "a", "c"], "empfohlene zuerst, sonst Reihenfolge des Modells");
        assert!(qs[0].multi);
        assert!(qs[0].options[0].recommended && qs[0].options[1].recommended && !qs[0].options[2].recommended);
    }

    #[test]
    fn lenient_forms_are_accepted_with_a_note() {
        let (qs, fixes) = q(json!({"question": "Go?", "options": ["Yes", "No", "Other"]})).unwrap();
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0].options.len(), 2, "„Other“ entfällt, das Formular bietet es immer");
        assert!(fixes.iter().any(|f| f.contains("single question")), "{fixes:?}");
        assert!(fixes.iter().any(|f| f.contains("Other")), "{fixes:?}");
        let (qs, fixes) = q(json!({"questions": {"question": "x", "multiSelect": "yes", "options": [{"text": "a"}, {"label": "b"}]}})).unwrap();
        assert!(qs[0].multi);
        assert_eq!(qs[0].options[0].label, "a");
        assert!(fixes.iter().any(|f| f.contains("multiSelect")), "{fixes:?}");
        let (qs, _) = q(json!({"questions": "[{\"question\":\"x\",\"options\":[\"a\",\"b\"]}]"})).unwrap();
        assert_eq!(qs.len(), 1);
    }

    #[test]
    fn helpful_errors_name_the_field_and_show_an_example() {
        let e = |v: Value| q(v).unwrap_err();
        for (args, want) in [
            (json!({}), "missing required field \"questions\""),
            (json!({"questions": []}), "at least one question"),
            (json!({"questions": [{"question": "a", "options": ["x", "y"]}, {"question": "b", "options": ["x", "y"]}, {"question": "c", "options": ["x", "y"]}, {"question": "d", "options": ["x", "y"]}, {"question": "e", "options": ["x", "y"]}]}), "at most 4 questions"),
            (json!({"questions": [{"options": ["x", "y"]}]}), "questions[0].question is missing"),
            (json!({"questions": [{"question": "a", "options": ["x"]}]}), "at least 2 options"),
            (json!({"questions": [{"question": "a", "options": ["x", "Other"]}]}), "at least 2 options"),
            (json!({"questions": [{"question": "a", "options": ["1", "2", "3", "4", "5", "6", "7"]}]}), "at most 6"),
            (json!({"questions": [{"question": "a", "options": ["x", "X"]}]}), "appears twice"),
            (json!({"questions": [{"question": "a", "options": [{"label": ""}, "y"]}]}), "options[0].label is missing"),
            (json!({"questions": [{"question": "a", "options": [{"label": "x", "recommended": "maybe"}, "y"]}]}), "recommended must be true or false"),
            (json!({"questions": [{"question": "a", "options": "x, y"}]}), "options must be a list"),
            (json!({"questions": [{"question": "a", "options": [{"label": "x".repeat(81)}, "y"]}]}), "label is too long"),
            (json!({"questions": [{"question": "a".repeat(301), "options": ["x", "y"]}]}), "question is too long"),
            (json!({"questions": [7]}), "questions[0] must be an object"),
            (json!("x"), "must be an object"),
        ] {
            let (code, msg) = e(args.clone());
            assert_eq!(code, "bad_args");
            assert!(msg.contains(want), "{args}: {msg}");
            assert!(msg.contains("Example: {"), "{msg}");
        }
    }

    #[test]
    fn single_choice_replaces_and_other_needs_text() {
        let (qs, _) = q(json!({"questions": [{"question": "One", "options": ["a", "b"]}, {"question": "Many", "multi_select": true, "options": ["x", "y", "z"]}]})).unwrap();
        let mut f = AskForm::new("c1", qs, vec![], None);
        assert!(!f.complete());
        f.toggle(0, 0);
        f.toggle(0, 1);
        assert_eq!(f.answers[0].selected, [false, true], "Einfachauswahl: nur eine");
        f.set_other(0, true);
        assert_eq!(f.answers[0].selected, [false, false], "„Andere“ ersetzt die Auswahl");
        assert!(!f.answered(0), "„Andere“ ohne Text zählt nicht");
        f.answers[0].other_text = "  my own  ".into();
        assert!(f.answered(0));
        f.toggle(1, 0);
        f.toggle(1, 2);
        f.set_other(1, true);
        assert_eq!(f.answers[1].selected, [true, false, true], "Mehrfachauswahl bleibt");
        assert!(!f.complete(), "„Andere“ gewählt, aber leer");
        f.set_other(1, false);
        assert!(f.complete());
        let r = f.result();
        assert_eq!(r["answered"], true);
        assert_eq!(r["answers"][0]["selected"], json!([]));
        assert_eq!(r["answers"][0]["other"], "my own");
        assert_eq!(r["answers"][1]["selected"], json!(["x", "z"]));
        assert!(r["answers"][1].get("other").is_none());
    }

    #[test]
    fn reply_sends_once_and_cancel_tells_the_model() {
        let (qs, _) = q(json!({"questions": [{"question": "One", "options": ["a", "b"]}]})).unwrap();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let mut f = AskForm::new("c1", qs.clone(), vec!["fix".into()], Some(tx));
        assert!(!f.reply(AskReply::Submit), "unvollständig: kein Absenden");
        assert!(rx.try_recv().is_err());
        f.toggle(0, 1);
        assert!(f.reply(AskReply::Submit));
        let v = rx.try_recv().unwrap().unwrap();
        assert_eq!(v["answers"][0]["selected"], json!(["b"]));
        assert_eq!(v["corrected"], json!(["fix"]));
        assert!(!f.reply(AskReply::Cancel), "nur einmal");
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let mut f = AskForm::new("c2", qs, vec![], Some(tx));
        assert!(f.reply(AskReply::Cancel), "Abbrechen geht auch ohne Auswahl");
        let v = rx.try_recv().unwrap().unwrap();
        assert_eq!((v["answered"].clone(), v["cancelled"].clone()), (json!(false), json!(true)));
        assert!(v["message"].as_str().unwrap().contains("do not want to answer"));
    }
}
