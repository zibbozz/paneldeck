//! Warteschlange der Läufe (#383): Ein Lauf = alle Aktionen nach dem Schreiben für eine Datei (die Regeln in
//! Listenreihenfolge). Je Datei läuft höchstens ein Lauf; wird sie währenddessen erneut gespeichert, wartet genau ein
//! weiterer Lauf mit dem neuesten Stand (ein schon wartender wird ersetzt). Insgesamt laufen höchstens `limit` Läufe
//! gleichzeitig (Standard 2, z. B. bei *Alle speichern*).

use super::matching::PlannedRule;
use super::SaveOrigin;
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;

/// Ein Lauf.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: u64,
    /// Datei ([`super::path_key`] als Schlüssel).
    pub file: PathBuf,
    pub key: String,
    pub rules: Vec<PlannedRule>,
    pub origin: SaveOrigin,
    /// Fehler der Text-Aktionen je Regel (Regel-Kennung, Meldung): der Rest dieser Regel entfällt.
    pub failed_rules: Vec<(u64, String)>,
}

/// Ergebnis von [`Queue::submit`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Submitted {
    /// Neu eingereiht.
    Queued,
    /// Ein wartender Lauf derselben Datei wurde durch den neuen ersetzt.
    Replaced,
}

#[derive(Default, Debug)]
pub struct Queue {
    waiting: VecDeque<Job>,
    running: HashSet<String>,
    next: u64,
}

impl Queue {
    /// Lauf einreihen (Kennung wird vergeben). Wartet schon einer für dieselbe Datei, ersetzt ihn der neue (an seiner
    /// Stelle).
    pub fn submit(&mut self, mut job: Job) -> (u64, Submitted) {
        self.next += 1;
        job.id = self.next;
        let id = job.id;
        if let Some(w) = self.waiting.iter_mut().find(|w| w.key == job.key) {
            // Regeln beider Läufe (Vereinigung, Listenreihenfolge); der neue Stand der Datei gewinnt.
            let mut rules = job.rules.clone();
            let mut failed = job.failed_rules.clone();
            for r in &w.rules {
                if !rules.iter().any(|x| x.rule == r.rule) {
                    rules.push(r.clone());
                    failed.extend(w.failed_rules.iter().filter(|(x, _)| *x == r.rule).cloned());
                }
            }
            rules.sort_by_key(|r| r.pos);
            job.rules = rules;
            job.failed_rules = failed;
            *w = job;
            return (id, Submitted::Replaced);
        }
        self.waiting.push_back(job);
        (id, Submitted::Queued)
    }

    /// Läufe, die jetzt starten dürfen (in der Reihenfolge des Einreihens; nie zwei für dieselbe Datei; höchstens
    /// `limit` gleichzeitig). Sie gelten danach als laufend, bis [`Queue::finished`].
    pub fn take_ready(&mut self, limit: usize) -> Vec<Job> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < self.waiting.len() && self.running.len() < limit.max(1) {
            if self.running.contains(&self.waiting[i].key) {
                i += 1;
                continue;
            }
            let j = self.waiting.remove(i).expect("index in range");
            self.running.insert(j.key.clone());
            out.push(j);
        }
        out
    }

    /// Lauf für die Datei `key` beendet.
    pub fn finished(&mut self, key: &str) {
        self.running.remove(key);
    }

    /// Wartenden Lauf einer Datei verwerfen.
    #[cfg(test)]
    pub fn drop_waiting(&mut self, key: &str) -> bool {
        let n = self.waiting.len();
        self.waiting.retain(|w| w.key != key);
        n != self.waiting.len()
    }

    /// Alle wartenden verwerfen (Beenden, Abbrechen).
    pub fn clear_waiting(&mut self) -> usize {
        let n = self.waiting.len();
        self.waiting.clear();
        n
    }

    /// Wartende Läufe (Rückfrage beim Beenden).
    pub fn waiting_jobs(&self) -> impl Iterator<Item = &Job> {
        self.waiting.iter()
    }

    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    #[cfg(test)]
    pub fn running(&self) -> usize {
        self.running.len()
    }

    #[cfg(test)]
    pub fn is_waiting(&self, key: &str) -> bool {
        self.waiting.iter().any(|w| w.key == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(key: &str, tag: &str) -> Job {
        Job { id: 0, file: PathBuf::from(key), key: key.into(), rules: vec![PlannedRule { rule: 1, name: tag.into(), post: vec![], builtins: vec![], pos: 0 }], origin: SaveOrigin::User, failed_rules: vec![] }
    }

    #[test]
    fn rapid_saves_coalesce_to_one_more_run_with_the_latest_state() {
        let mut q = Queue::default();
        q.submit(job("a", "v1"));
        let started = q.take_ready(2);
        assert_eq!(started.len(), 1);
        // Während des Laufs dreimal gespeichert: genau ein wartender Lauf mit dem neuesten Stand.
        assert_eq!(q.submit(job("a", "v2")).1, Submitted::Queued);
        assert_eq!(q.submit(job("a", "v3")).1, Submitted::Replaced);
        assert_eq!(q.submit(job("a", "v4")).1, Submitted::Replaced);
        assert_eq!(q.waiting(), 1);
        assert!(q.take_ready(2).is_empty(), "nie zwei Läufe derselben Datei");
        q.finished("a");
        let next = q.take_ready(2);
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].rules[0].name, "v4");
        q.finished("a");
        assert_eq!((q.running(), q.waiting()), (0, 0));
    }

    /// Ein ersetzter wartender Lauf verliert keine Regeln: Vereinigung in Listenreihenfolge, neuester Stand gewinnt.
    #[test]
    fn replacing_a_waiting_run_merges_the_rules() {
        let pr = |rule: u64, pos: usize, name: &str| PlannedRule { rule, name: name.into(), post: vec![], builtins: vec![], pos };
        let mut q = Queue::default();
        q.submit(job("a", "running"));
        q.take_ready(2);
        let mut j1 = job("a", "");
        j1.rules = vec![pr(3, 2, "c-old"), pr(1, 0, "a-old")];
        j1.failed_rules = vec![(3, "x".into())];
        q.submit(j1);
        let mut j2 = job("a", "");
        j2.rules = vec![pr(1, 0, "a-new"), pr(2, 1, "b-new")];
        assert_eq!(q.submit(j2).1, Submitted::Replaced);
        q.finished("a");
        let next = q.take_ready(2).remove(0);
        assert_eq!(next.rules.iter().map(|r| (r.rule, r.name.as_str())).collect::<Vec<_>>(), [(1, "a-new"), (2, "b-new"), (3, "c-old")]);
        assert_eq!(next.failed_rules, vec![(3, "x".to_string())]);
    }

    #[test]
    fn save_all_runs_at_most_limit_in_parallel() {
        let mut q = Queue::default();
        for k in ["a", "b", "c", "d", "e"] {
            q.submit(job(k, k));
        }
        let first = q.take_ready(2);
        assert_eq!(first.iter().map(|j| j.key.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        assert!(q.take_ready(2).is_empty());
        q.finished("a");
        assert_eq!(q.take_ready(2)[0].key, "c");
        q.finished("b");
        q.finished("c");
        assert_eq!(q.take_ready(3).len(), 2);
        assert_eq!(q.clear_waiting(), 0);
        q.submit(job("x", "x"));
        assert!(q.is_waiting("x") && q.drop_waiting("x") && !q.is_waiting("x"));
    }
}
