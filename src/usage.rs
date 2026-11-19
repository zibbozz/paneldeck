//! Token- und Kostenanzeige aus den lokalen Claude-Transkripten (~/.claude/projects/**.jsonl).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

#[derive(Clone, Default, Debug)]
pub struct Usage {
    pub total: Tokens,
    /// Kontextgröße der letzten Antwort (Input + Cache).
    pub context: u64,
    pub turns: u32,
    pub model: String,
    /// Token je Modell (für die Kostenberechnung mit den aktuellen Preisen).
    pub by_model: HashMap<String, Tokens>,
    /// Token je Tag (UTC, `YYYY-MM-DD`) und Modell.
    pub by_day: BTreeMap<String, HashMap<String, Tokens>>,
    /// Letzte Änderung der Transkriptdatei (für den Status ohne Hooks).
    pub modified: Option<std::time::SystemTime>,
    /// Was der letzte Eintrag der Unterhaltung über die laufende Runde sagt ([`tail_of`]), und wo er in der Datei steht.
    pub tail: Option<Tail>,
    /// Zuletzt genommene Marke ([`UsageWatcher::mark`]): ihre Nummer und die Länge der Datei in diesem Augenblick.
    /// Alles ab dieser Stelle wurde nach der Meldung geschrieben, für die die Marke steht.
    pub mark: Option<(u64, u64)>,
    /// Nummer der Marke, die schon genommen war, als das Transkript zuletzt bis zu seinem Ende gelesen wurde – `tail`
    /// ist dann der Stand nach dieser Marke.
    pub read_mark: u64,
}

impl Usage {
    /// Belegt das Transkript, dass die Runde nach der Marke `seq` zu Ende ging? Der letzte Eintrag muss hinter der
    /// Stelle stehen, bis zu der die Datei bei der Marke reichte – verglichen wird die Lage in der Datei, keine Uhrzeit
    /// (die Uhr in WSL kann abweichen, und ein Eintrag kurz vor der Meldung ist kein Beleg).
    pub fn ended_after(&self, seq: u64) -> bool {
        match (self.tail, self.mark) {
            (Some(t), Some((m, len))) => seq != 0 && m == seq && self.read_mark == seq && t.kind.ended() && t.pos >= len,
            _ => false,
        }
    }
}

/// Stand der laufenden Runde laut Transkript.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TailKind {
    /// Die Runde läuft noch oder der Eintrag belegt nichts (Prompt, Werkzeugaufruf, Werkzeug-Ergebnis, Teilantwort).
    Open,
    /// Vom Nutzer unterbrochen (Esc) oder eine Freigabe abgelehnt – dafür gibt es keinen Stop-Hook.
    Interrupted,
    /// Mit einer Fehlermeldung statt einer Antwort beendet (API-Fehler, Limit erreicht).
    Error,
    /// Antwort fertig.
    Finished,
}

impl TailKind {
    /// Die Runde ist zu Ende; Claude wartet auf den nächsten Prompt.
    pub fn ended(self) -> bool {
        self != TailKind::Open
    }
}

/// Letzter aussagekräftiger Eintrag eines Transkripts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tail {
    pub kind: TailKind,
    /// Stelle in der Datei, an der die Zeile des Eintrags beginnt.
    pub pos: u64,
}

/// Was ein Transkript-Eintrag über die Runde des Haupt-Agenten sagt; `None`, wenn er nichts dazu sagt (Einträge von
/// Unter-Agenten, Verwaltungszeilen, Ausgaben lokaler Befehle) – dann gilt weiter der Eintrag davor.
///
/// Claude Code schreibt nach einer Unterbrechung die Nutzer-Nachricht `[Request interrupted by user]` bzw.
/// `[Request interrupted by user for tool use]` (abgelehnte Freigabe), nach einem API-Fehler eine Antwort mit
/// `isApiErrorMessage` bzw. dem Modell `<synthetic>`, am normalen Ende eine Antwort mit `stop_reason` `end_turn`.
pub fn tail_of(v: &Value) -> Option<TailKind> {
    if v["isSidechain"].as_bool() == Some(true) || v["isMeta"].as_bool() == Some(true) {
        return None;
    }
    let msg = &v["message"];
    let kind = match v["type"].as_str()? {
        "user" => {
            // Text der Nachricht: Zeichenkette oder erster Textblock.
            let text = match &msg["content"] {
                Value::String(s) => s.as_str(),
                Value::Array(a) => a.iter().find(|b| b["type"] == "text").and_then(|b| b["text"].as_str()).unwrap_or(""),
                _ => "",
            }
            .trim_start();
            if text.starts_with("[Request interrupted by user") {
                TailKind::Interrupted
            } else if ["<local-command-", "<command-name>", "<command-message>", "<bash-input>", "<bash-stdout>", "<bash-stderr>"].iter().any(|p| text.starts_with(p)) {
                // Lokale Befehle (`/cost`, `!ls`) beginnen und beenden keine Runde.
                return None;
            } else {
                TailKind::Open
            }
        }
        "assistant" => {
            if v["isApiErrorMessage"].as_bool() == Some(true) || msg["model"] == "<synthetic>" {
                TailKind::Error
            } else {
                match msg["stop_reason"].as_str() {
                    Some("end_turn" | "stop_sequence" | "refusal") => TailKind::Finished,
                    _ => TailKind::Open,
                }
            }
        }
        // Dauer der Runde: Claude Code schreibt sie an deren Ende.
        "system" if v["subtype"] == "turn_duration" => TailKind::Finished,
        _ => return None,
    };
    Some(kind)
}

impl Usage {
    /// Geschätzte Kosten in USD mit den aktuell eingestellten Preisen.
    pub fn cost(&self) -> f64 {
        self.by_model.iter().map(|(m, t)| cost_of(m, t)).sum()
    }

    /// Geschätzte Kosten je Tag (UTC).
    pub fn cost_by_day(&self) -> BTreeMap<String, f64> {
        self.by_day.iter().map(|(d, ms)| (d.clone(), ms.iter().map(|(m, t)| cost_of(m, t)).sum())).collect()
    }
}

/// Preise in USD pro Million Token (Input, Output) je Modellfamilie plus Cache-Faktoren (Anteil des Input-Preises).
/// Voreinstellung sind grobe Listenpreise – nur eine Schätzung.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prices {
    pub opus: [f64; 2],
    pub sonnet: [f64; 2],
    pub haiku: [f64; 2],
    pub cache_read: f64,
    pub cache_write: f64,
}

impl Default for Prices {
    fn default() -> Self {
        Self { opus: [15.0, 75.0], sonnet: [3.0, 15.0], haiku: [1.0, 5.0], cache_read: 0.1, cache_write: 1.25 }
    }
}

static PRICES: std::sync::RwLock<Option<Prices>> = std::sync::RwLock::new(None);

pub fn set_prices(p: Prices) {
    *PRICES.write().unwrap_or_else(|e| e.into_inner()) = Some(p);
}

fn current_prices() -> Prices {
    PRICES.read().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
}

/// Preise (Input, Output) für ein Modell; unbekannte Modelle werden wie Sonnet gerechnet.
pub fn price(model: &str) -> (f64, f64) {
    let p = current_prices();
    let m = model.to_lowercase();
    let [i, o] = if m.contains("opus") {
        p.opus
    } else if m.contains("haiku") {
        p.haiku
    } else {
        p.sonnet
    };
    (i, o)
}

fn cost_of(model: &str, t: &Tokens) -> f64 {
    let p = current_prices();
    let (pi, po) = price(model);
    (t.input as f64 * pi + t.output as f64 * po + t.cache_read as f64 * pi * p.cache_read + t.cache_write as f64 * pi * p.cache_write) / 1e6
}

/// Datum (UTC) `YYYY-MM-DD` zu Sekunden seit 1970.
pub fn utc_date(secs: u64) -> String {
    let z = (secs / 86400) as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn today_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

/// Zustand je Transkript: pro Nachrichten-ID wird der zuletzt gesehene Stand gezählt.
#[derive(Default)]
pub struct Tracked {
    pub path: Option<PathBuf>,
    /// Transkript über (WSL-Distribution, Konversations-ID) suchen, solange `path` unbekannt ist.
    pub locate: Option<(String, String)>,
    last_locate: Option<std::time::Instant>,
    offset: u64,
    per_msg: HashMap<String, (String, Tokens, String)>,
    pub usage: Usage,
}

impl Tracked {
    #[cfg(test)]
    pub fn feed_line(&mut self, line: &str) {
        self.feed_line_at(line, 0);
    }

    /// Eine Zeile des Transkripts auswerten; `pos` = Stelle in der Datei, an der sie beginnt.
    pub fn feed_line_at(&mut self, line: &str, pos: u64) {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return };
        if let Some(kind) = tail_of(&v) {
            self.usage.tail = Some(Tail { kind, pos });
        }
        if v["type"] != "assistant" {
            return;
        }
        let msg = &v["message"];
        let u = &msg["usage"];
        if !u.is_object() {
            return;
        }
        let g = |k: &str| u[k].as_u64().unwrap_or(0);
        let t = Tokens {
            input: g("input_tokens"),
            output: g("output_tokens"),
            cache_read: g("cache_read_input_tokens"),
            cache_write: g("cache_creation_input_tokens"),
        };
        let model = msg["model"].as_str().unwrap_or("").to_string();
        if model == "<synthetic>" {
            return;
        }
        let id = msg["id"].as_str().map(|s| s.to_string()).unwrap_or_else(|| v["uuid"].as_str().unwrap_or("").to_string());
        let day: String = v["timestamp"].as_str().map(|s| s.chars().take(10).collect()).unwrap_or_default();
        let old = self.per_msg.insert(id, (model.clone(), t, day.clone()));
        if let Some((om, ot, od)) = old {
            if let Some(m) = self.usage.by_day.get_mut(&od).and_then(|d| d.get_mut(&om)) {
                m.input -= ot.input;
                m.output -= ot.output;
                m.cache_read -= ot.cache_read;
                m.cache_write -= ot.cache_write;
            }
            self.usage.total.input -= ot.input;
            self.usage.total.output -= ot.output;
            self.usage.total.cache_read -= ot.cache_read;
            self.usage.total.cache_write -= ot.cache_write;
            if let Some(m) = self.usage.by_model.get_mut(&om) {
                m.input -= ot.input;
                m.output -= ot.output;
                m.cache_read -= ot.cache_read;
                m.cache_write -= ot.cache_write;
            }
        } else {
            self.usage.turns += 1;
        }
        self.usage.total.input += t.input;
        self.usage.total.output += t.output;
        self.usage.total.cache_read += t.cache_read;
        self.usage.total.cache_write += t.cache_write;
        if !day.is_empty() {
            let d = self.usage.by_day.entry(day).or_default().entry(model.clone()).or_default();
            d.input += t.input;
            d.output += t.output;
            d.cache_read += t.cache_read;
            d.cache_write += t.cache_write;
        }
        let m = self.usage.by_model.entry(model.clone()).or_default();
        m.input += t.input;
        m.output += t.output;
        m.cache_read += t.cache_read;
        m.cache_write += t.cache_write;
        self.usage.context = t.input + t.cache_read + t.cache_write;
        if !model.is_empty() {
            self.usage.model = model;
        }
    }

    /// Liest neue, vollständige Zeilen seit dem letzten Aufruf.
    #[cfg(test)]
    pub fn poll(&mut self) {
        self.poll_marked(0);
    }

    /// Nummer der zuletzt genommenen Marke (0 = noch keine).
    pub fn mark_seq(&self) -> u64 {
        self.usage.mark.map_or(0, |m| m.0)
    }

    /// Wie [`Tracked::poll`]; ist `want` neuer als die letzte Marke, wird vor dem Lesen die Länge der Datei als Marke
    /// `want` festgehalten ([`Usage::mark`]).
    pub fn poll_marked(&mut self, want: u64) {
        if self.path.is_none() {
            if let Some((distro, sid)) = self.locate.clone() {
                if self.last_locate.is_none_or(|t| t.elapsed() > Duration::from_secs(10)) {
                    self.last_locate = Some(std::time::Instant::now());
                    self.path = find_in_wsl(&distro, &sid);
                }
            }
        }
        let Some(path) = self.path.clone() else { return };
        let Ok(mut f) = std::fs::File::open(&path) else { return };
        let meta = f.metadata();
        self.usage.modified = meta.as_ref().ok().and_then(|m| m.modified().ok());
        let Ok(len) = meta.map(|m| m.len()) else { return };
        if len < self.offset {
            let (locate, modified) = (self.locate.take(), self.usage.modified);
            *self = Tracked { path: Some(path), locate, ..Tracked::default() };
            self.usage.modified = modified;
        }
        // Marke vor dem Lesen: Was jetzt schon in der Datei steht (auch eine angefangene Zeile), stand vor der Meldung da.
        if want > self.mark_seq() {
            self.usage.mark = Some((want, len));
        }
        if len == self.offset {
            self.usage.read_mark = self.mark_seq();
            return;
        }
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut buf = Vec::new();
        if f.take(64 << 20).read_to_end(&mut buf).is_err() {
            return;
        }
        let Some(end) = buf.iter().rposition(|b| *b == b'\n') else { return };
        let mut pos = self.offset;
        for raw in buf[..end].split(|b| *b == b'\n') {
            self.feed_line_at(String::from_utf8_lossy(raw).trim_end_matches('\r'), pos);
            pos += raw.len() as u64 + 1;
        }
        self.offset += end as u64 + 1;
        // Bis zum Ende gelesen (keine angefangene Zeile, nicht an der Lesegrenze abgeschnitten): `tail` ist der Stand
        // nach der Marke.
        if end + 1 == buf.len() && (buf.len() as u64) < (64 << 20) {
            self.usage.read_mark = self.mark_seq();
        }
    }
}

/// Sucht `<id>.jsonl` unter `\\wsl$\<Distro>\{home\*,root}\.claude\projects\*`.
fn find_in_wsl(distro: &str, session_id: &str) -> Option<PathBuf> {
    let base = PathBuf::from(format!(r"\\wsl$\{distro}"));
    let mut homes = vec![base.join("root")];
    if let Ok(rd) = std::fs::read_dir(base.join("home")) {
        homes.extend(rd.flatten().map(|e| e.path()));
    }
    for h in homes {
        let proj = h.join(".claude").join("projects");
        let Ok(rd) = std::fs::read_dir(&proj) else { continue };
        for d in rd.flatten() {
            let p = d.path().join(format!("{session_id}.jsonl"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// Hintergrund-Thread, der die Transkripte aller Sitzungen beobachtet.
#[derive(Clone, Default)]
pub struct UsageWatcher {
    shared: Arc<Mutex<HashMap<u64, Tracked>>>,
    /// Zuletzt berechnete Werte für die Oberfläche – lesbar, ohne auf den Hintergrund-Thread zu warten.
    published: Arc<Mutex<HashMap<u64, Usage>>>,
    /// Entfernte Sitzungen: ein gerade laufender Durchgang darf sie nicht wieder einfügen.
    dead: Arc<Mutex<std::collections::HashSet<u64>>>,
    /// Verlangte Marken je Sitzung ([`UsageWatcher::mark`]). Getrennt von `shared`: dort fehlt ein Eintrag, solange der
    /// Hintergrund-Thread ihn gerade liest.
    marks: Arc<Mutex<HashMap<u64, u64>>>,
    /// Weckt den Hintergrund-Thread für eine neue Marke.
    wake: Arc<(Mutex<bool>, std::sync::Condvar)>,
}

/// Abstand der regelmäßigen Durchgänge über alle Transkripte.
const POLL_EVERY: Duration = Duration::from_secs(2);

impl UsageWatcher {
    pub fn start(ctx: eframe::egui::Context) -> Self {
        let w = UsageWatcher::default();
        let (shared, published, dead, marks, wake) = (w.shared.clone(), w.published.clone(), w.dead.clone(), w.marks.clone(), w.wake.clone());
        std::thread::spawn(move || {
            let mut last_full = std::time::Instant::now();
            loop {
                // Bis zum nächsten regelmäßigen Durchgang warten – oder bis eine Marke verlangt wird.
                {
                    let (lock, cv) = &*wake;
                    let mut woken = lock.lock().unwrap_or_else(|e| e.into_inner());
                    let left = POLL_EVERY.saturating_sub(last_full.elapsed());
                    if !*woken && !left.is_zero() {
                        woken = cv.wait_timeout(woken, left).unwrap_or_else(|e| e.into_inner()).0;
                    }
                    *woken = false;
                }
                let full = last_full.elapsed() >= POLL_EVERY;
                if full {
                    last_full = std::time::Instant::now();
                }
                let wanted = marks.lock().unwrap_or_else(|e| e.into_inner()).clone();
                // Zwischen den Durchgängen nur Sitzungen mit einer neuen Marke.
                let ids: Vec<u64> = shared.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|(id, t)| full || wanted.get(*id).is_some_and(|w| *w > t.mark_seq())).map(|(id, _)| *id).collect();
                let mut changed = false;
                for id in ids {
                    // Dateizugriffe (bis 64 MB lesen, JSON parsen, \\wsl$-Suche) laufen OHNE Sperre – sonst hängt die
                    // Oberfläche, die jedes Bild die Werte abfragt, solange ein großes Transkript gelesen wird.
                    let Some(mut t) = shared.lock().unwrap_or_else(|e| e.into_inner()).remove(&id) else { continue };
                    let before = (t.usage.turns, t.usage.total);
                    t.poll_marked(wanted.get(&id).copied().unwrap_or(0));
                    if (t.usage.turns, t.usage.total) != before {
                        changed = true;
                    }
                    if dead.lock().unwrap_or_else(|e| e.into_inner()).contains(&id) {
                        continue;
                    }
                    let mut map = shared.lock().unwrap_or_else(|e| e.into_inner());
                    // Wurde inzwischen ein neuer Pfad gesetzt (Eintrag existiert wieder), gewinnt der neuere Stand.
                    if !map.contains_key(&id) {
                        published.lock().unwrap_or_else(|e| e.into_inner()).insert(id, t.usage.clone());
                        map.insert(id, t);
                    }
                }
                if changed {
                    ctx.request_repaint();
                }
            }
        });
        w
    }

    /// Marke `seq` für das Transkript der Sitzung verlangen: Der Hintergrund-Thread hält gleich die jetzige Länge der
    /// Datei fest ([`Usage::mark`]). Der Hub ruft das bei jeder Hook-Meldung auf – was danach im Transkript erscheint,
    /// ist nach der Meldung geschrieben ([`Usage::ended_after`]). Nummern steigen, eine ältere wird ignoriert.
    pub fn mark(&self, id: u64, seq: u64) {
        {
            let mut m = self.marks.lock().unwrap_or_else(|e| e.into_inner());
            let cur = m.entry(id).or_insert(0);
            if seq <= *cur {
                return;
            }
            *cur = seq;
        }
        let (lock, cv) = &*self.wake;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cv.notify_one();
    }

    pub fn set_path(&self, id: u64, path: PathBuf) {
        let mut map = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        let t = map.entry(id).or_default();
        if t.path.as_ref() != Some(&path) {
            *t = Tracked { path: Some(path), ..Tracked::default() };
        }
    }

    /// Für Sitzungen ohne Hook-Daten (z. B. WSL): Transkript später selbst suchen.
    pub fn set_locate(&self, id: u64, distro: &str, session_id: &str) {
        let mut map = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        let t = map.entry(id).or_default();
        if t.path.is_none() && t.locate.as_ref().map(|(d, s)| (d.as_str(), s.as_str())) != Some((distro, session_id)) {
            t.locate = Some((distro.to_string(), session_id.to_string()));
            t.last_locate = None;
        }
    }

    pub fn get(&self, id: u64) -> Option<Usage> {
        self.published.lock().unwrap_or_else(|e| e.into_inner()).get(&id).filter(|u| u.turns > 0).cloned()
    }

    /// Belegt das Transkript der Sitzung, dass die Runde nach der Marke `seq` zu Ende ging ([`Usage::ended_after`])?
    pub fn ended_after(&self, id: u64, seq: u64) -> bool {
        self.published.lock().unwrap_or_else(|e| e.into_inner()).get(&id).is_some_and(|u| u.ended_after(seq))
    }

    /// Neu beginnen (neue Claude-Konversation oder kein Claude mehr): Pfad und Zahlen vergessen. Anders als `remove`
    /// kann die Sitzung danach wieder einen Pfad bekommen. Ein leerer Eintrag bleibt stehen, damit eine gerade laufende
    /// Abfrage im Hintergrund den alten Stand nicht zurückschreibt.
    pub fn reset(&self, id: u64) {
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).insert(id, Tracked::default());
        self.published.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    }

    pub fn remove(&self, id: u64) {
        self.marks.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        self.dead.lock().unwrap_or_else(|e| e.into_inner()).insert(id);
        self.shared.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
        self.published.lock().unwrap_or_else(|e| e.into_inner()).remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_forgets_the_transcript_but_accepts_a_new_one() {
        let w = UsageWatcher::default();
        w.set_path(7, PathBuf::from("a.jsonl"));
        w.published.lock().unwrap().insert(7, Usage { turns: 3, ..Usage::default() });
        assert!(w.get(7).is_some());
        w.reset(7);
        assert!(w.get(7).is_none(), "keine alten Zahlen");
        assert!(w.shared.lock().unwrap().get(&7).is_some_and(|t| t.path.is_none()));
        w.set_path(7, PathBuf::from("b.jsonl"));
        assert_eq!(w.shared.lock().unwrap().get(&7).and_then(|t| t.path.clone()), Some(PathBuf::from("b.jsonl")));
        assert!(!w.dead.lock().unwrap().contains(&7), "anders als remove: bleibt beobachtbar");
    }

    /// Was das Ende des Transkripts über die Runde sagt (Rückfall für den Status, wenn kein Stop-Hook kommt).
    #[test]
    fn the_tail_tells_how_the_turn_ended() {
        let kind = |line: &str| tail_of(&serde_json::from_str::<Value>(line).unwrap());
        // Unterbrechung mit Esc und abgelehnte Freigabe: Claude Code schreibt eine Nutzer-Nachricht, sendet aber kein Stop
        assert_eq!(kind(r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#), Some(TailKind::Interrupted));
        assert_eq!(kind(r#"{"type":"user","message":{"role":"user","content":"[Request interrupted by user for tool use]"}}"#), Some(TailKind::Interrupted));
        // API-Fehler, Limit erreicht
        assert_eq!(kind(r#"{"type":"assistant","isApiErrorMessage":true,"message":{"model":"<synthetic>","content":[{"type":"text","text":"API Error: 529"}]}}"#), Some(TailKind::Error));
        assert_eq!(kind(r#"{"type":"assistant","message":{"model":"<synthetic>","content":[]}}"#), Some(TailKind::Error));
        // Normales Ende
        assert_eq!(kind(r#"{"type":"assistant","message":{"model":"claude-opus-5","stop_reason":"end_turn","content":[]}}"#), Some(TailKind::Finished));
        assert_eq!(kind(r#"{"type":"system","subtype":"turn_duration","durationMs":1200}"#), Some(TailKind::Finished));
        // Die Runde läuft: Prompt, Werkzeugaufruf, Werkzeug-Ergebnis, Teilantwort
        assert_eq!(kind(r#"{"type":"user","message":{"role":"user","content":"mach weiter"}}"#), Some(TailKind::Open));
        assert_eq!(kind(r#"{"type":"assistant","message":{"model":"claude-opus-5","stop_reason":"tool_use","content":[]}}"#), Some(TailKind::Open));
        assert_eq!(kind(r#"{"type":"assistant","message":{"model":"claude-opus-5","stop_reason":null,"content":[]}}"#), Some(TailKind::Open));
        assert_eq!(kind(r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":"ok"}]}}"#), Some(TailKind::Open));
        // Sagt nichts: Unter-Agenten, Verwaltungszeilen, lokale Befehle, eingeschobene Hinweise
        for line in [
            r#"{"type":"assistant","isSidechain":true,"message":{"model":"claude-opus-5","stop_reason":"end_turn"}}"#,
            r#"{"type":"user","isMeta":true,"message":{"role":"user","content":"Caveat"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"<local-command-stdout>ok</local-command-stdout>"}}"#,
            r#"{"type":"user","message":{"role":"user","content":"<command-name>/cost</command-name>"}}"#,
            r#"{"type":"system","subtype":"api_error"}"#,
            r#"{"type":"file-history-snapshot"}"#,
            r#"{"type":"summary","summary":"x"}"#,
            r#"{"kein":"Typ"}"#,
        ] {
            assert_eq!(kind(line), None, "{line}");
        }
        assert!(TailKind::Interrupted.ended() && TailKind::Error.ended() && TailKind::Finished.ended() && !TailKind::Open.ended());
    }

    /// Ein Beleg zählt nur, wenn er in der Datei hinter der Marke der letzten Meldung steht (Review: keine Uhrzeiten –
    /// die Uhr in WSL kann abweichen, und ein Eintrag kurz vor der Meldung ist kein Beleg).
    #[test]
    fn evidence_counts_only_behind_the_mark_of_the_last_event() {
        let dir = std::env::temp_dir().join(format!("usage-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.jsonl");
        let prompt = r#"{"type":"user","message":{"role":"user","content":"los"}}"#;
        let call = r#"{"type":"assistant","message":{"id":"a","model":"m","stop_reason":"tool_use","usage":{"output_tokens":1}}}"#;
        let done = r#"{"type":"assistant","message":{"id":"b","model":"m","stop_reason":"end_turn","usage":{"output_tokens":1}}}"#;
        let stop = r#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user]"}]}}"#;
        let snap = r#"{"type":"file-history-snapshot"}"#;
        // Eine fertige Runde steht schon im Transkript, dann kommt eine Meldung (Marke 1): kein Beleg – das Ende stand
        // vorher da (z. B. ein Hintergrund-Agent fragt gleich nach dem Ende der Runde nach einer Freigabe)
        let old = format!("{prompt}\n{done}\n");
        std::fs::write(&p, &old).unwrap();
        let mut t = Tracked { path: Some(p.clone()), ..Tracked::default() };
        t.poll();
        assert_eq!(t.usage.tail, Some(Tail { kind: TailKind::Finished, pos: prompt.len() as u64 + 1 }));
        assert!(!t.usage.ended_after(1), "noch keine Marke");
        t.poll_marked(1);
        assert_eq!(t.usage.mark, Some((1, old.len() as u64)));
        assert!(!t.usage.ended_after(1), "das Ende stand schon vor der Meldung da");
        // Die Runde läuft weiter und wird unterbrochen: der Eintrag steht hinter der Marke
        std::fs::write(&p, format!("{old}{prompt}\n{call}\n")).unwrap();
        t.poll();
        assert!(!t.usage.ended_after(1), "die Runde läuft");
        std::fs::write(&p, format!("{old}{prompt}\n{call}\n{stop}\n{snap}\n")).unwrap();
        t.poll();
        assert_eq!(t.usage.tail.map(|x| x.kind), Some(TailKind::Interrupted));
        assert!(t.usage.ended_after(1));
        assert!(!t.usage.ended_after(2) && !t.usage.ended_after(0), "nur für genau diese Marke");
        // Die nächste Meldung (Marke 2) kommt: derselbe Eintrag ist jetzt alt
        t.poll_marked(2);
        assert!(!t.usage.ended_after(1) && !t.usage.ended_after(2));
        // Eine angefangene Zeile: der Stand ist noch nicht bis zum Ende gelesen – kein Beleg, bis sie fertig ist
        let full = format!("{old}{prompt}\n{call}\n{stop}\n{snap}\n{prompt}\n");
        std::fs::write(&p, format!("{full}{}", &stop[..20])).unwrap();
        t.poll_marked(3);
        assert_eq!(t.usage.mark, Some((3, full.len() as u64 + 20)), "die Marke nimmt die angefangene Zeile mit");
        assert_eq!(t.usage.read_mark, 2);
        std::fs::write(&p, format!("{full}{stop}\n")).unwrap();
        t.poll();
        assert_eq!(t.usage.read_mark, 3);
        assert!(!t.usage.ended_after(3), "die Zeile begann vor der Marke");
        std::fs::write(&p, format!("{full}{stop}\n{prompt}\n{done}\n")).unwrap();
        t.poll();
        assert!(t.usage.ended_after(3));
        // Ältere Marken werden ignoriert; eine neu angelegte (kürzere) Datei vergisst alles
        t.poll_marked(2);
        assert_eq!(t.mark_seq(), 3);
        std::fs::write(&p, format!("{prompt}\n")).unwrap();
        t.poll();
        assert_eq!((t.usage.tail.map(|x| x.kind), t.usage.mark), (Some(TailKind::Open), None));
        // Ohne Datei gibt es keine Marke
        let mut none = Tracked { path: Some(dir.join("fehlt.jsonl")), ..Tracked::default() };
        none.poll_marked(5);
        assert_eq!(none.mark_seq(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Der Beobachter nimmt eine verlangte Marke sofort, nicht erst beim nächsten regelmäßigen Durchgang.
    #[test]
    fn the_watcher_takes_a_requested_mark_at_once() {
        let dir = std::env::temp_dir().join(format!("usage-mark-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.jsonl");
        std::fs::write(&p, "{\"type\":\"user\",\"message\":{\"content\":\"los\"}}\n").unwrap();
        let w = UsageWatcher::start(eframe::egui::Context::default());
        w.set_path(7, p.clone());
        w.mark(7, 4);
        w.mark(7, 3); // ältere Nummer: ignoriert
        let t0 = std::time::Instant::now();
        let taken = || w.published.lock().unwrap().get(&7).and_then(|u| u.mark);
        while taken().is_none() && t0.elapsed() < Duration::from_millis(1500) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(taken().map(|m| m.0), Some(4), "vor dem regelmäßigen Durchgang ({POLL_EVERY:?})");
        assert!(!w.ended_after(7, 4));
        let mut s = std::fs::read_to_string(&p).unwrap();
        s.push_str("{\"type\":\"user\",\"message\":{\"content\":\"[Request interrupted by user]\"}}\n");
        std::fs::write(&p, s).unwrap();
        let t0 = std::time::Instant::now();
        while !w.ended_after(7, 4) && t0.elapsed() < Duration::from_secs(6) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(w.ended_after(7, 4));
        assert!(!w.ended_after(8, 4), "andere Sitzung");
        w.remove(7);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn counts_each_message_once_and_keeps_latest() {
        let mut t = Tracked::default();
        let l = |out: u64| {
            format!(
                r#"{{"type":"assistant","message":{{"id":"m1","model":"claude-sonnet-5-5","usage":{{"input_tokens":10,"output_tokens":{out},"cache_read_input_tokens":1000,"cache_creation_input_tokens":200}}}}}}"#
            )
        };
        t.feed_line(&l(5));
        t.feed_line(&l(50)); // gleiche Nachricht, spätere Zählung ersetzt die frühere
        t.feed_line(r#"{"type":"user","message":{}}"#);
        t.feed_line(r#"{"type":"assistant","message":{"id":"m2","model":"claude-opus-5","usage":{"input_tokens":1,"output_tokens":2}}}"#);
        assert_eq!(t.usage.turns, 2);
        assert_eq!(t.usage.total.output, 52);
        assert_eq!(t.usage.total.input, 11);
        assert_eq!(t.usage.context, 1);
        assert!(t.usage.cost() > 0.0);
    }

    #[test]
    fn dates() {
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(86400 * 365), "1971-01-01");
        assert_eq!(utc_date(1730764800), "2024-11-05");
        assert_eq!(utc_date(1709164800), "2024-02-29");
    }

    #[test]
    fn empty_cost_sum_is_not_negative_zero() {
        // Die Summe einer leeren f64-Liste ist -0.0 und würde als „$-0.00“ angezeigt; die Anzeige addiert deshalb 0.0.
        let none: Vec<f64> = Vec::new();
        assert_eq!(format!("{:.2}", none.iter().sum::<f64>()), "-0.00");
        assert_eq!(format!("{:.2}", none.iter().sum::<f64>() + 0.0), "0.00");
    }

    #[test]
    fn tracks_cost_per_day() {
        let mut t = Tracked::default();
        let l = |id: &str, ts: &str, out: u64| {
            format!(r#"{{"type":"assistant","timestamp":"{ts}","message":{{"id":"{id}","model":"claude-sonnet-5-5","usage":{{"output_tokens":{out}}}}}}}"#)
        };
        t.feed_line(&l("a", "2026-11-04T23:00:00Z", 5));
        t.feed_line(&l("a", "2026-11-04T23:00:01Z", 100)); // Streaming-Update ersetzt
        t.feed_line(&l("b", "2026-11-05T01:00:00Z", 10));
        assert_eq!(t.usage.by_day["2026-11-04"]["claude-sonnet-5-5"].output, 100);
        assert_eq!(t.usage.by_day["2026-11-05"]["claude-sonnet-5-5"].output, 10);
        let c = t.usage.cost_by_day();
        assert!(c["2026-11-04"] > c["2026-11-05"]);
    }

    #[test]
    fn poll_reads_incrementally() {
        let dir = std::env::temp_dir().join(format!("usage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.jsonl");
        std::fs::write(&p, "{\"type\":\"assistant\",\"message\":{\"id\":\"a\",\"model\":\"x\",\"usage\":{\"output_tokens\":7}}}\n{\"type\":\"assi").unwrap();
        let mut t = Tracked { path: Some(p.clone()), ..Tracked::default() };
        t.poll();
        assert_eq!(t.usage.total.output, 7);
        let mut s = std::fs::read_to_string(&p).unwrap();
        s.push_str("stant\",\"message\":{\"id\":\"b\",\"model\":\"x\",\"usage\":{\"output_tokens\":3}}}\n");
        std::fs::write(&p, s).unwrap();
        t.poll();
        assert_eq!(t.usage.total.output, 10);
    }

    #[test]
    fn prices_are_configurable() {
        let mut t = Tracked::default();
        t.feed_line(r#"{"type":"assistant","message":{"id":"x","model":"claude-opus-5","usage":{"input_tokens":1000000,"output_tokens":0}}}"#);
        set_prices(Prices::default());
        assert!((t.usage.cost() - 15.0).abs() < 1e-9);
        set_prices(Prices { opus: [10.0, 50.0], ..Prices::default() });
        assert!((t.usage.cost() - 10.0).abs() < 1e-9);
        set_prices(Prices::default());
    }

    #[test]
    fn formats() {
        assert_eq!(fmt_tokens(950), "950");
        assert_eq!(fmt_tokens(12_300), "12.3k");
        assert_eq!(fmt_tokens(2_500_000), "2.5M");
    }
}
