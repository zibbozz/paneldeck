//! Benachrichtigungen des Reiters „Zeit“ (#164) – kleine, wiederverwendbare Schnittstelle ohne egui.
//!
//! Wer etwas melden will (Wecker, Timer-Ende, Geburtstag, später die Termin-Erinnerungen des Kalenders), ruft
//! [`fire`] bzw. [`fire_note`] auf – von jedem Faden aus. Die Meldung landet in einer Warteschlange; die Anwendung holt sie
//! im Hauptdurchlauf jedes Bilds ab (`App::time_notify_tick`) und setzt sie nach den Einstellungen um
//! (`settings::TimeSettings`): Klang, Toast (Desktop-Benachrichtigung, bei aktivem Fenster ein Toast in Paneldeck) und
//! – standardmäßig aus – das Blinken des Taskleistenknopfs. Ein klingelnder Klang mit `ring`-Schlüssel wiederholt sich,
//! bis [`stop`] mit demselben Schlüssel kommt (oder die Höchstdauer erreicht ist).
//!
//! ```ignore
//! use crate::time::notify::{self, Kind};
//! notify::fire(Kind::Reminder, "Standup", "09:30 – in 5 min", None);
//! ```

use std::sync::{Mutex, OnceLock};

/// Art der Meldung (bestimmt, welche Schalter gelten).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Alarm,
    Timer,
    /// Termin-Erinnerung (Kalender; ausgelöst vom Kalender-Teil).
    #[allow(dead_code)]
    Reminder,
    Birthday,
}

/// Eine Meldung.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub kind: Kind,
    pub title: String,
    pub body: String,
    /// Klang (`builtin:<name>` oder `file:<pfad>`); `None` = Standardklang der Art.
    pub sound: Option<String>,
    /// Schlüssel für einen wiederholten Klang (z. B. `alarm:3`); `None` = einmal abspielen.
    pub ring: Option<String>,
}

impl Note {
    pub fn new(kind: Kind, title: &str, body: &str) -> Note {
        Note { kind, title: title.to_string(), body: body.to_string(), sound: None, ring: None }
    }

    pub fn sound(mut self, s: Option<&str>) -> Note {
        self.sound = s.map(str::to_string);
        self
    }

    pub fn ring(mut self, key: &str) -> Note {
        self.ring = Some(key.to_string());
        self
    }
}

/// Auftrag in der Warteschlange.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cmd {
    Fire(Note),
    /// Klingeln mit diesem Schlüssel beenden.
    Stop(String),
}

static QUEUE: Mutex<Vec<Cmd>> = Mutex::new(Vec::new());
static WAKER: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// Meldung auslösen: Art, Titel, Text, Klang (`None` = Standardklang der Art).
pub fn fire(kind: Kind, title: &str, body: &str, sound: Option<&str>) {
    fire_note(Note::new(kind, title, body).sound(sound));
}

pub fn fire_note(n: Note) {
    push(Cmd::Fire(n));
}

/// Wiederholten Klang mit Schlüssel `key` beenden.
pub fn stop(key: &str) {
    push(Cmd::Stop(key.to_string()));
}

/// Höchstzahl wartender Aufträge; darüber fällt die älteste Meldung weg – nie ein `Stop` (sonst klingelte ein
/// ausgeschalteter Wecker bis zum Programmende weiter).
pub const MAX_QUEUE: usize = 200;

fn push(c: Cmd) {
    if let Ok(mut q) = QUEUE.lock() {
        enqueue(&mut q, c);
    }
    if let Some(w) = WAKER.get() {
        w();
    }
}

fn enqueue(q: &mut Vec<Cmd>, c: Cmd) {
    // gleiche Stops zusammenfassen
    if let Cmd::Stop(k) = &c
        && q.iter().any(|x| matches!(x, Cmd::Stop(y) if y == k))
    {
        return;
    }
    q.push(c);
    while q.len() > MAX_QUEUE {
        match q.iter().position(|x| matches!(x, Cmd::Fire(_))) {
            Some(i) => {
                q.remove(i);
            }
            None => break,
        }
    }
}

/// Alle wartenden Aufträge holen (Anwendung, je Bild).
pub fn take() -> Vec<Cmd> {
    QUEUE.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
}

/// Weckfunktion der Anwendung (z. B. `ctx.request_repaint`), damit eine Meldung aus einem anderen Faden sofort umgesetzt wird.
pub fn set_waker(f: Box<dyn Fn() + Send + Sync>) {
    let _ = WAKER.set(f);
}

#[cfg(test)]
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_collects_fires_and_stops_in_order() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _ = take();
        fire(Kind::Reminder, "Standup", "in 5 min", None);
        fire_note(Note::new(Kind::Alarm, "Wecker", "07:00").sound(Some("builtin:bell")).ring("alarm:1"));
        stop("alarm:1");
        let got = take();
        assert_eq!(got.len(), 3);
        assert_eq!(got[0], Cmd::Fire(Note::new(Kind::Reminder, "Standup", "in 5 min")));
        assert!(matches!(&got[1], Cmd::Fire(n) if n.ring.as_deref() == Some("alarm:1") && n.sound.as_deref() == Some("builtin:bell")));
        assert_eq!(got[2], Cmd::Stop("alarm:1".into()));
        assert!(take().is_empty());
    }

    #[test]
    fn a_full_queue_drops_old_fires_but_never_a_stop() {
        let mut q = Vec::new();
        enqueue(&mut q, Cmd::Stop("alarm:1".into()));
        enqueue(&mut q, Cmd::Stop("alarm:1".into()));
        assert_eq!(q.len(), 1, "gleiche Stops zusammengefasst");
        for i in 0..(MAX_QUEUE + 50) {
            enqueue(&mut q, Cmd::Fire(Note::new(Kind::Timer, &i.to_string(), "")));
        }
        assert_eq!(q.len(), MAX_QUEUE);
        assert_eq!(q[0], Cmd::Stop("alarm:1".into()), "der Stop bleibt");
        assert!(matches!(&q[1], Cmd::Fire(n) if n.title == "51"), "die ältesten Meldungen fielen weg");
    }
}
