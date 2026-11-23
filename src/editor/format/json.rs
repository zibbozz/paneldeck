//! JSON und JSONC formatieren, minifizieren, prüfen, Schlüssel sortieren (#381). Gerüst – wird vom JSON-Agenten gefüllt.

use super::{Op, Options, Outcome};
use std::sync::atomic::AtomicBool;

/// Kommen Kommentare (`//` oder `/* */` außerhalb von Zeichenketten) vor?
pub fn has_comments(_text: &str) -> bool {
    false
}

/// Siehe [`super::run`].
pub fn run(_jsonc: bool, _op: Op, text: &str, _opts: Options, _cancel: &AtomicBool, _progress: &dyn Fn(f32)) -> Outcome {
    Outcome::Text(text.to_string())
}
