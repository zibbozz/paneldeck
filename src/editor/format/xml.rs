//! XML behutsam formatieren, minifizieren, prüfen (#381). Gerüst – wird vom XML-Agenten gefüllt.

use super::{Op, Options, Outcome};
use std::sync::atomic::AtomicBool;

/// Siehe [`super::run`].
pub fn run(_op: Op, text: &str, _opts: Options, _cancel: &AtomicBool, _progress: &dyn Fn(f32)) -> Outcome {
    Outcome::Text(text.to_string())
}

/// Wie [`run`], `fragment` = Auswahl mit mehreren oder keinen Elementen auf oberster Ebene. Vorläufiger Platzhalter
/// für die Menüleiste (#381) – die echte Umsetzung kommt mit dem XML-Zweig.
pub fn run_with(op: Op, text: &str, opts: Options, _fragment: bool, cancel: &AtomicBool, progress: &dyn Fn(f32)) -> Outcome {
    run(op, text, opts, cancel, progress)
}
