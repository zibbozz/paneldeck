//! Linux/macOS (#149): Frage nach dem Master-Passwort für API-Schlüssel, wenn es keinen Schlüsselbund gibt
//! (`platform::secret`). Sie erscheint erst beim ersten Zugriff auf einen Schlüssel (Speichern im Formular, Chat mit einem
//! Server mit Schlüssel) – nie beim Start. Behalten werden nur die abgeleiteten Schlüssel, bis Paneldeck beendet wird.
//!
//! Die Prüfung (PBKDF2) läuft im Hintergrund (`secret::begin_unlock`): Gibt es schon mit dem Passwort verschlüsselte
//! Schlüssel, muss es sie öffnen; sonst wird ein neues festgelegt (zweimal eingeben). Die Eingaben stehen in einem
//! eigenen Feld der App (`App::master_pw`), nie in egui-Daten, und werden nach Gebrauch überschrieben.

use super::*;
use zeroize::Zeroizing;

/// Eingaben der Frage (werden beim Verwerfen überschrieben, `Zeroizing`).
#[derive(Default)]
pub(super) struct PwForm {
    pw: Zeroizing<String>,
    again: Zeroizing<String>,
    error: String,
}

impl PwForm {
    fn clear(&mut self) {
        *self = PwForm::default();
    }
}

impl App {
    pub(super) fn master_password_ui(&mut self, ctx: &egui::Context) {
        use crate::platform::secret::{self as sec, PwState as Check};
        if !sec::password_wanted() {
            return;
        }
        let lang = self.settings.language;
        let id = Id::new("master_password_ask");
        // Mit dem Passwort verschlüsselte Schlüssel: daran wird geprüft (alle, damit jeder Salt abgeleitet ist).
        let blobs: Vec<Vec<u8>> = self
            .settings
            .llm_servers
            .iter()
            .filter(|s| crate::llm::secret::is_password_protected(&s.api_key))
            .filter_map(|s| crate::llm::secret::unbase64(s.api_key.trim()))
            .collect();
        let check = sec::password_state();
        match check {
            Check::Done => {
                sec::cancel_password();
                self.master_pw.clear();
                self.notify_text(lang.t("Master-Passwort übernommen – bitte den letzten Schritt wiederholen.", "Master password set – please repeat the last step.").to_string());
                return;
            }
            Check::Wrong => self.master_pw.error = lang.t("Das Passwort passt nicht zu den gespeicherten Schlüsseln.", "The password does not match the stored keys.").into(),
            _ => {}
        }
        let working = check == Check::Working;
        if working {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let st = &mut self.master_pw;
        let (mut ok, mut cancel) = (false, ctx.input(|i| i.key_pressed(Key::Escape)));
        style::movable(ctx, id, egui::Window::new(lang.t("Master-Passwort für API-Schlüssel", "Master password for API keys")))
            .collapsible(false)
            .resizable(false)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.set_max_width(440.0);
                ui.label(lang.t(
                    "Es ist kein Schlüsselbund des Systems erreichbar. Paneldeck verschlüsselt API-Schlüssel deshalb mit einem Master-Passwort.",
                    "No system keyring is reachable. Paneldeck therefore encrypts API keys with a master password.",
                ));
                ui.label(RichText::new(lang.t(
                    "Es wird nirgends gespeichert und nur bis zum Beenden im Speicher behalten. Ohne das Passwort lassen sich gespeicherte Schlüssel nicht mehr lesen.",
                    "It is not stored anywhere and only kept in memory until Paneldeck quits. Without it, stored keys cannot be read any more.",
                )).small().color(style::dim(ui)));
                ui.add_space(S1);
                ui.add_enabled_ui(!working, |ui| {
                    let first = ui.add(egui::TextEdit::singleline(&mut *st.pw).password(true).hint_text(lang.t("Master-Passwort", "Master password")));
                    if !ui.memory(|m| m.has_focus(first.id)) && st.pw.is_empty() && st.again.is_empty() {
                        first.request_focus();
                    }
                    let mut enter = first.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if blobs.is_empty() {
                        let r = ui.add(egui::TextEdit::singleline(&mut *st.again).password(true).hint_text(lang.t("Wiederholen (neues Passwort)", "Repeat (new password)")));
                        enter |= r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    }
                    ok = enter;
                });
                if working {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(lang.t("Passwort wird geprüft …", "Checking the password …"));
                    });
                } else if !st.error.is_empty() {
                    ui.colored_label(Color32::LIGHT_RED, &st.error);
                }
                ui.add_space(S2);
                ui.horizontal(|ui| {
                    ok |= ui.add_enabled(!working, egui::Button::new(lang.t("Übernehmen", "Apply"))).clicked();
                    cancel |= ui.button(lang.t("Abbrechen", "Cancel")).clicked();
                });
            });
        if ok && !working {
            st.error = if st.pw.is_empty() {
                lang.t("Bitte ein Passwort eingeben.", "Please enter a password.").into()
            } else if blobs.is_empty() && *st.pw != *st.again {
                lang.t("Die beiden Eingaben stimmen nicht überein.", "The two entries do not match.").into()
            } else {
                String::new()
            };
            if st.error.is_empty() {
                let pw = std::mem::take(&mut st.pw);
                st.again = Zeroizing::default();
                sec::begin_unlock(pw, blobs);
            }
        }
        if cancel {
            sec::cancel_password();
            self.master_pw.clear();
        }
    }
}
