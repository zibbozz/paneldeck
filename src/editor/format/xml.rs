//! XML behutsam formatieren, minifizieren, prüfen (#381). Gerüst – wird vom XML-Agenten gefüllt.

use super::{Op, Options, Outcome};
use std::sync::atomic::AtomicBool;

/// Siehe [`super::run`].
pub fn run(_op: Op, text: &str, _opts: Options, _cancel: &AtomicBool, _progress: &dyn Fn(f32)) -> Outcome {
    Outcome::Text(text.to_string())
}
