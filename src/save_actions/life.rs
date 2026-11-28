//! Zeitlimit und Abbrechen eines Befehls (#383): wie in der Prozessliste (`procs::Life`) – bei Ablauf oder *Abbrechen*
//! erst Strg+C, nach 5 s der ganze Prozessbaum; Ergebnis „Zeitlimit“ bzw. „abgebrochen“ statt eines Exit-Codes.

use crate::procs::{self, Effect, Flavor, Life, Status};
use std::time::{Duration, Instant};

/// Warum ein Befehl angehalten wurde.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopWhy {
    TimeLimit,
    Cancelled,
}

/// Ergebnis eines Befehls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    /// Exit-Code ≠ 0 (bzw. unbekannt).
    Failed(Option<i64>),
    /// Befehl/Shell nicht gefunden.
    NotFound,
    TimeLimit,
    Cancelled,
}

/// Lebenslauf eines laufenden Befehls.
#[derive(Clone, Debug)]
pub struct CmdLife {
    pub life: Life,
    started: Instant,
    timeout: Duration,
    pub stopped: Option<StopWhy>,
}

impl CmdLife {
    /// Gerade gestartet.
    pub fn started(now: Instant, timeout: Duration) -> CmdLife {
        let mut life = Life::default();
        life.on_spawned(Ok(()));
        CmdLife { life, started: now, timeout, stopped: None }
    }

    /// Zeit weiter: Zeitlimit erreicht → anhalten; danach die Fristen der Prozessliste.
    pub fn tick(&mut self, now: Instant) -> Vec<Effect> {
        let mut fx = Vec::new();
        if self.stopped.is_none() && self.life.status == Status::Running && now.saturating_duration_since(self.started) >= self.timeout {
            self.stopped = Some(StopWhy::TimeLimit);
            fx.extend(self.life.stop(now));
        }
        fx.extend(self.life.tick(now));
        fx
    }

    /// *Abbrechen*.
    pub fn cancel(&mut self, now: Instant) -> Vec<Effect> {
        if self.stopped.is_some() || !self.life.status.active() {
            return Vec::new();
        }
        self.stopped = Some(StopWhy::Cancelled);
        self.life.stop(now)
    }

    /// Ist der Lauf vorbei (beendet oder aufgegeben, weil sich der Baum nicht beenden ließ)?
    pub fn done(&self) -> bool {
        !self.life.status.active()
    }

    /// Der Prozess ist beendet.
    pub fn exited(&mut self, flavor: Flavor, code: Option<i64>) -> Outcome {
        let _ = self.life.on_exit(flavor, code);
        self.outcome(flavor, code)
    }

    /// Ergebnis (auch, wenn der Baum nicht zu beenden war).
    pub fn outcome(&self, flavor: Flavor, code: Option<i64>) -> Outcome {
        match self.stopped {
            Some(StopWhy::TimeLimit) => Outcome::TimeLimit,
            Some(StopWhy::Cancelled) => Outcome::Cancelled,
            None => match procs::classify_exit(flavor, code) {
                Status::Exited => Outcome::Ok,
                Status::Failed => Outcome::NotFound,
                _ => Outcome::Failed(code),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_limit_sends_ctrl_c_then_kills_the_tree() {
        let t0 = Instant::now();
        let mut l = CmdLife::started(t0, Duration::from_secs(60));
        assert!(l.tick(t0 + Duration::from_secs(59)).is_empty());
        assert_eq!(l.tick(t0 + Duration::from_secs(60)), [Effect::CtrlC]);
        assert_eq!(l.stopped, Some(StopWhy::TimeLimit));
        assert!(l.tick(t0 + Duration::from_secs(62)).is_empty());
        assert_eq!(l.tick(t0 + Duration::from_secs(65)), [Effect::KillTree]);
        assert_eq!(l.exited(Flavor::Pwsh, Some(1)), Outcome::TimeLimit);
        assert!(l.done());
    }

    #[test]
    fn cancel_and_normal_ends() {
        let t0 = Instant::now();
        let mut l = CmdLife::started(t0, Duration::from_secs(60));
        assert_eq!(l.cancel(t0), [Effect::CtrlC]);
        assert!(l.cancel(t0).is_empty(), "nur einmal");
        assert_eq!(l.exited(Flavor::Bash, Some(130)), Outcome::Cancelled);
        let mut ok = CmdLife::started(t0, Duration::from_secs(5));
        assert_eq!(ok.exited(Flavor::Pwsh, Some(0)), Outcome::Ok);
        let mut bad = CmdLife::started(t0, Duration::from_secs(5));
        assert_eq!(bad.exited(Flavor::Cmd, Some(2)), Outcome::Failed(Some(2)));
        let mut nf = CmdLife::started(t0, Duration::from_secs(5));
        assert_eq!(nf.exited(Flavor::Pwsh, Some(procs::NOT_FOUND_WIN)), Outcome::NotFound);
        // Ein Baum, der sich nicht beenden lässt, gilt nach den Fristen als vorbei.
        let mut stuck = CmdLife::started(t0, Duration::from_secs(1));
        stuck.tick(t0 + Duration::from_secs(1));
        stuck.tick(t0 + Duration::from_secs(6));
        stuck.tick(t0 + Duration::from_secs(12));
        assert!(stuck.done());
        assert_eq!(stuck.outcome(Flavor::Pwsh, None), Outcome::TimeLimit);
    }
}
