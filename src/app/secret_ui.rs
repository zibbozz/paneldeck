//! Linux/macOS (#149): Frage nach dem Master-Passwort für API-Schlüssel, wenn es keinen Schlüsselbund gibt
//! (`platform::secret`). Sie erscheint erst beim ersten Zugriff auf einen Schlüssel (Speichern im Formular, Chat mit einem
//! Server mit Schlüssel) – nie beim Start. Das Passwort bleibt nur im Speicher, bis Paneldeck beendet wird.
//!
//! Gibt es schon mit dem Passwort verschlüsselte Schlüssel, wird das Passwort vor dem Übernehmen an einem davon geprüft;
//! sonst wird ein neues festgelegt (zweimal eingeben).

use super::*;

#[derive(Clone, Default)]
struct PwState {
    pw: String,
    again: String,
    error: String,
}

impl App {
    pub(super) fn master_password_ui(&mut self, ctx: &egui::Context) {
        if !crate::platform::secret::password_wanted() {
            return;
        }
        let lang = self.settings.language;
        let id = Id::new("master_password_ask");
        let mut st: PwState = ctx.data_mut(|d| d.get_temp(id)).unwrap_or_default();
        // Ein vorhandener, mit dem Passwort verschlüsselter Schlüssel: daran wird geprüft.
        let existing: Option<Vec<u8>> = self
            .settings
            .llm_servers
            .iter()
            .filter(|s| crate::llm::secret::is_password_protected(&s.api_key))
            .find_map(|s| crate::llm::secret::unbase64(s.api_key.trim()));
        let (mut ok, mut cancel) = (false, ctx.input(|i| i.key_pressed(Key::Escape)));
        style::movable(ctx, id, egui::Window::new(lang.t("Master-Passwort für API-Schlüssel", "Master password for API keys")))
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_max_width(440.0);
                ui.label(lang.t(
                    "Es ist kein Schlüsselbund des Systems (Secret Service) erreichbar. Paneldeck verschlüsselt API-Schlüssel deshalb mit einem Master-Passwort.",
                    "No system keyring (Secret Service) is reachable. Paneldeck therefore encrypts API keys with a master password.",
                ));
                ui.label(RichText::new(lang.t(
                    "Es wird nirgends gespeichert und nur bis zum Beenden im Speicher behalten. Ohne das Passwort lassen sich gespeicherte Schlüssel nicht mehr lesen.",
                    "It is not stored anywhere and only kept in memory until Paneldeck quits. Without it, stored keys cannot be read any more.",
                )).small().color(style::dim(ui)));
                ui.add_space(S1);
                let first = ui.add(egui::TextEdit::singleline(&mut st.pw).password(true).hint_text(lang.t("Master-Passwort", "Master password")));
                if !ui.memory(|m| m.has_focus(first.id)) && st.pw.is_empty() && st.again.is_empty() {
                    first.request_focus();
                }
                let mut enter = first.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if existing.is_none() {
                    let r = ui.add(egui::TextEdit::singleline(&mut st.again).password(true).hint_text(lang.t("Wiederholen (neues Passwort)", "Repeat (new password)")));
                    enter |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                }
                if !st.error.is_empty() {
                    ui.colored_label(Color32::LIGHT_RED, &st.error);
                }
                ui.add_space(S2);
                ui.horizontal(|ui| {
                    ok = ui.button(lang.t("Übernehmen", "Apply")).clicked() || enter;
                    cancel |= ui.button(lang.t("Abbrechen", "Cancel")).clicked();
                });
            });
        if ok {
            st.error = if st.pw.is_empty() {
                lang.t("Bitte ein Passwort eingeben.", "Please enter a password.").into()
            } else if let Some(blob) = &existing {
                if crate::platform::secret::password_opens(&st.pw, blob) { String::new() } else { lang.t("Das Passwort passt nicht zu den gespeicherten Schlüsseln.", "The password does not match the stored keys.").into() }
            } else if st.pw != st.again {
                lang.t("Die beiden Eingaben stimmen nicht überein.", "The two entries do not match.").into()
            } else {
                String::new()
            };
            if st.error.is_empty() {
                crate::platform::secret::set_password(Some(std::mem::take(&mut st.pw)));
                self.notify_text(lang.t("Master-Passwort übernommen – bitte den letzten Schritt wiederholen.", "Master password set – please repeat the last step.").to_string());
                ctx.data_mut(|d| d.remove::<PwState>(id));
                return;
            }
        }
        if cancel {
            crate::platform::secret::set_password(None);
            ctx.data_mut(|d| d.remove::<PwState>(id));
            return;
        }
        ctx.data_mut(|d| d.insert_temp(id, st));
    }
}
