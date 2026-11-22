//! Platz des Unterreiters „Kalender“ (#164). Der Kalender selbst entsteht getrennt (Branch `feature/3.2-calendar`);
//! beim Zusammenführen ersetzt dessen `show` den Aufruf von [`show`] in `TimeUi::show` (Markierung „KALENDER-PLATZ“).

use crate::state::Lang;
use crate::style;
use eframe::egui::{RichText, Ui};

/// Platzhalter, bis der Kalender da ist.
pub(super) fn show(ui: &mut Ui, lang: Lang) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(lang.t("Kalender – kommt in dieser Version", "Calendar – coming in this version")).size(18.0).color(style::dim(ui)));
    });
}
