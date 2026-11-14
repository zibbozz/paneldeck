//! Berechnungen ohne Oberfläche und ohne Datenbank – reine Logik, keine Abhängigkeit von egui.
//!
//! * [`duration`] – Dauer-Eingabe (`1d 2h 15m`, `1.5h`, `2`) lesen und kurz ausgeben (`1d 2h 15m`), Summen, Rundung.
//! * [`calendar`] – Arbeitstage (Wochentage + Feiertage): zählen, verschieben, Tage durchlaufen.
//! * [`burndown`] – Burndown-Linien (Ideal/Ist) aus Umfangs-Ereignissen sowie Velocity über mehrere Zeiträume.
//! * [`csv`] – CSV-Schreiber und streng passender Leser im eigenen Format (Rundreise-sicher).
//! * [`progress`] – Fortschritt eines übergeordneten Eintrags aus seinen Kindern, Summen von Zeiteinträgen.
//!
//! Regeln für alle Teile: Zeit („heute“, „jetzt“) wird immer als Parameter übergeben, nie aus der Systemuhr gelesen;
//! alle Meldungen, die Nutzer sehen können, gibt es auf Englisch (Standard) und Deutsch (`message(lang)`).
//!
//! Die Funktionen sind noch nicht an die Oberfläche angeschlossen; daher `dead_code` hier erlaubt, bis sie benutzt werden.
#![allow(dead_code, unused_imports)]

pub mod burndown;
pub mod calendar;
pub mod csv;
pub mod duration;
pub mod progress;
