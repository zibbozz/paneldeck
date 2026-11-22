//! Einstellungen des Reiters „Zeit“ (`settings.json` → `time`, seit 3.2.0, #164). Alles optional und je Wert tolerant:
//! Fehlt ein Wert oder ist er unlesbar, gilt der Standard.
//!
//! Standards (Entscheidung zu #164): Wecker und Timer-Ende mit Klang und Toast **an**; Termin-Erinnerungen (Kalender) und
//! das Blinken der Taskleiste **aus**; Geburtstags-Erinnerungen mit Toast (die Erinnerung selbst ist je Eintrag aus);
//! Zeiterfassung an Tickets **aus**.

use super::notify::Kind;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const VOLUME_RANGE: (u32, u32) = (0, 100);
/// Wie lange ein Wecker höchstens klingelt (Minuten), danach verstummt er.
pub const RING_MINUTES_RANGE: (u32, u32) = (1, 60);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeSettings {
    pub alarm_sound: bool,
    pub alarm_toast: bool,
    pub timer_sound: bool,
    pub timer_toast: bool,
    /// Termin-Erinnerungen des Kalenders.
    pub reminder_sound: bool,
    pub reminder_toast: bool,
    pub birthday_sound: bool,
    pub birthday_toast: bool,
    /// Taskleistenknopf blinken lassen (alle Arten).
    pub flash: bool,
    /// Lautstärke in Prozent.
    pub volume: u32,
    /// Höchstdauer des Klingelns eines Weckers in Minuten (Timer: eine Minute).
    pub ring_minutes: u32,
    /// Zeit von Timern und Stoppuhren an Tickets buchen (Ticketfeld in den Details, Rückfrage beim Stoppen).
    pub ticket_booking: bool,
    /// Sekunden in Weltuhren zeigen.
    pub clock_seconds: bool,
}

impl Default for TimeSettings {
    fn default() -> Self {
        TimeSettings {
            alarm_sound: true,
            alarm_toast: true,
            timer_sound: true,
            timer_toast: true,
            reminder_sound: false,
            reminder_toast: false,
            birthday_sound: false,
            birthday_toast: true,
            flash: false,
            volume: 80,
            ring_minutes: 10,
            ticket_booking: false,
            clock_seconds: false,
        }
    }
}

impl TimeSettings {
    pub fn from_value(v: &Value) -> TimeSettings {
        let d = TimeSettings::default();
        let Some(o) = v.as_object() else { return d };
        let flag = |k: &str, def: bool| o.get(k).and_then(Value::as_bool).unwrap_or(def);
        let num = |k: &str, def: u32, (lo, hi): (u32, u32)| o.get(k).and_then(Value::as_u64).map(|n| n.clamp(lo as u64, hi as u64) as u32).unwrap_or(def);
        TimeSettings {
            alarm_sound: flag("alarm_sound", d.alarm_sound),
            alarm_toast: flag("alarm_toast", d.alarm_toast),
            timer_sound: flag("timer_sound", d.timer_sound),
            timer_toast: flag("timer_toast", d.timer_toast),
            reminder_sound: flag("reminder_sound", d.reminder_sound),
            reminder_toast: flag("reminder_toast", d.reminder_toast),
            birthday_sound: flag("birthday_sound", d.birthday_sound),
            birthday_toast: flag("birthday_toast", d.birthday_toast),
            flash: flag("flash", d.flash),
            volume: num("volume", d.volume, VOLUME_RANGE),
            ring_minutes: num("ring_minutes", d.ring_minutes, RING_MINUTES_RANGE),
            ticket_booking: flag("ticket_booking", d.ticket_booking),
            clock_seconds: flag("clock_seconds", d.clock_seconds),
        }
    }

    /// `deserialize_with` für `Settings::time`.
    pub fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<TimeSettings, D::Error> {
        let v = Value::deserialize(d)?;
        Ok(TimeSettings::from_value(&v))
    }

    pub fn is_default(&self) -> bool {
        *self == TimeSettings::default()
    }

    /// Klang für diese Art an?
    pub fn sound(&self, k: Kind) -> bool {
        match k {
            Kind::Alarm => self.alarm_sound,
            Kind::Timer => self.timer_sound,
            Kind::Reminder => self.reminder_sound,
            Kind::Birthday => self.birthday_sound,
        }
    }

    /// Toast für diese Art an?
    pub fn toast(&self, k: Kind) -> bool {
        match k {
            Kind::Alarm => self.alarm_toast,
            Kind::Timer => self.timer_toast,
            Kind::Reminder => self.reminder_toast,
            Kind::Birthday => self.birthday_toast,
        }
    }

    pub fn volume_f(&self) -> f32 {
        self.volume.min(VOLUME_RANGE.1) as f32 / 100.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_and_tolerant_reading() {
        let d = TimeSettings::default();
        assert!(d.alarm_sound && d.alarm_toast && d.timer_sound && d.timer_toast);
        assert!(!d.reminder_toast && !d.reminder_sound && !d.flash && !d.ticket_booking, "Termine, Blinken, Zeiterfassung aus");
        assert_eq!(TimeSettings::from_value(&json!(null)), d);
        assert_eq!(TimeSettings::from_value(&json!("kaputt")), d);
        let s = TimeSettings::from_value(&json!({"flash": true, "volume": 500, "alarm_toast": "ja", "ring_minutes": 0, "future": 1}));
        assert!(s.flash);
        assert_eq!(s.volume, 100);
        assert!(s.alarm_toast, "falscher Typ → Standard");
        assert_eq!(s.ring_minutes, 1);
        // Rundweg über serde
        let txt = serde_json::to_string(&s).unwrap();
        assert_eq!(TimeSettings::from_value(&serde_json::from_str::<Value>(&txt).unwrap()), s);
        assert!(d.toast(Kind::Alarm) && !d.toast(Kind::Reminder) && d.toast(Kind::Birthday) && !d.sound(Kind::Birthday));
    }
}
