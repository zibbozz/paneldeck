//! Fortschritt und Abbruch eines langen Imports (Projektsicherung, Ticket-CSV). Der Import läuft im Hintergrund-Faden; die Oberfläche
//! liest den Fortschritt (`done` von `total` Schritten) und kann den Import abbrechen. Abgebrochen wird **zwischen** den Tickets: der
//! Import liefert dann [`BackupError::Cancelled`], und weil er in einem SAVEPOINT läuft, wird alles zurückgenommen (es entsteht nichts).

use super::BackupError;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Gemeinsamer Zähler und Abbruch-Schalter. Ohne Beobachter (`Progress::default()`) kostet er nur einen Zähler je Schritt.
#[derive(Debug, Default)]
pub struct Progress {
    cancel: AtomicBool,
    done: AtomicUsize,
    total: AtomicUsize,
    /// Nur für Tests: nach so vielen Schritten bricht der Import selbst ab (0 = nie).
    #[cfg(test)]
    cancel_after: AtomicUsize,
}

impl Progress {
    /// Wie viele Schritte der Import insgesamt macht (für die Anzeige; 0 = unbekannt).
    pub fn set_total(&self, n: usize) {
        self.total.store(n, Ordering::Relaxed);
    }

    /// Ein Schritt ist getan. Gibt [`BackupError::Cancelled`] zurück, wenn abgebrochen werden soll.
    pub fn step(&self) -> Result<(), BackupError> {
        let n = self.done.fetch_add(1, Ordering::Relaxed) + 1;
        #[cfg(test)]
        {
            let at = self.cancel_after.load(Ordering::Relaxed);
            if at > 0 && n >= at {
                self.cancel();
            }
        }
        let _ = n;
        self.check()
    }

    /// Soll abgebrochen werden?
    pub fn check(&self) -> Result<(), BackupError> {
        if self.is_cancelled() { Err(BackupError::Cancelled) } else { Ok(()) }
    }

    /// Bitte abbrechen (aus der Oberfläche).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn done(&self) -> usize {
        self.done.load(Ordering::Relaxed)
    }

    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    /// Anteil 0.0–1.0; `None`, solange die Gesamtzahl unbekannt ist.
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total();
        if total == 0 { None } else { Some((self.done() as f32 / total as f32).clamp(0.0, 1.0)) }
    }

    /// Für Tests: bricht nach `n` Schritten selbst ab (deterministisch, ohne zweiten Faden).
    #[cfg(test)]
    pub fn cancelling_after(n: usize) -> Progress {
        let p = Progress::default();
        p.cancel_after.store(n, Ordering::Relaxed);
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_steps_and_reports_the_fraction() {
        let p = Progress::default();
        assert_eq!(p.fraction(), None, "ohne Gesamtzahl unbekannt");
        p.set_total(4);
        assert!(p.step().is_ok());
        assert!(p.step().is_ok());
        assert_eq!((p.done(), p.total()), (2, 4));
        assert_eq!(p.fraction(), Some(0.5));
        for _ in 0..10 {
            let _ = p.step();
        }
        assert_eq!(p.fraction(), Some(1.0), "nie über 100 %");
    }

    #[test]
    fn cancel_makes_the_next_step_fail() {
        let p = Progress::default();
        assert!(p.step().is_ok());
        assert!(!p.is_cancelled());
        p.cancel();
        assert!(p.is_cancelled());
        assert_eq!(p.step(), Err(BackupError::Cancelled));
        assert_eq!(p.check(), Err(BackupError::Cancelled));
        let q = Progress::cancelling_after(3);
        assert!(q.step().is_ok() && q.step().is_ok());
        assert_eq!(q.step(), Err(BackupError::Cancelled));
    }
}
