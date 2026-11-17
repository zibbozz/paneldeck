//! Rückfrage vor dem Öffnen eines OSC-8-Links mit einem anderen Schema als http/https (#183, Entscheidung des Nutzers):
//! zeigt das ganze Ziel (das Programm im Terminal bestimmt es, der sichtbare Text kann etwas anderes behaupten) und
//! öffnet erst nach „Öffnen“. Esc oder Klick daneben bricht ab. Die Anfrage kommt aus `termview` über
//! `hyperlink::request_confirm` und wird im Fenster gezeigt, in dem geklickt wurde.

use super::*;

pub(super) struct LinkAsk {
    viewport: egui::ViewportId,
    uri: String,
    /// Zuletzt gezeichnet: Wird das Fenster nicht mehr gezeichnet (geschlossen, wieder angedockt), verfällt die Frage.
    shown: Instant,
    /// Fokus des Terminals dahinter schon abgegeben (einmal beim ersten Zeichnen).
    focus_taken: bool,
}

/// Nach so langer Zeit ohne Zeichnen in ihrem Fenster verfällt die Rückfrage.
const STALE: Duration = Duration::from_secs(2);

impl App {
    /// Ist die Rückfrage in diesem Fenster offen? (Dann behält dort kein Terminal den Fokus.)
    pub(super) fn link_ask_here(&self, ctx: &egui::Context) -> bool {
        self.link_ask.as_ref().is_some_and(|a| a.viewport == ctx.viewport_id())
    }

    /// Neue Anfrage übernehmen und die offene Rückfrage im passenden Fenster zeigen.
    pub(super) fn link_ask_ui(&mut self, ctx: &egui::Context) {
        if let Some((viewport, uri)) = crate::hyperlink::take_request(ctx) {
            self.link_ask = Some(LinkAsk { viewport, uri, shown: Instant::now(), focus_taken: false });
        }
        if self.link_ask.as_ref().is_some_and(|a| a.shown.elapsed() > STALE) {
            self.link_ask = None;
        }
        let Some(ask) = self.link_ask.as_mut() else { return };
        if ask.viewport != ctx.viewport_id() {
            return;
        }
        ask.shown = Instant::now();
        // Tasten gehören der Rückfrage, nicht dem Terminal dahinter (abgedockte Fenster zeichnen sie vor dem Terminal).
        if !ask.focus_taken {
            ask.focus_taken = true;
            models_ui::release_focus(ctx);
        }
        let ask = self.link_ask.as_ref().expect("gerade geprüft");
        let lang = self.settings.language;
        let file = match crate::hyperlink::classify(&ask.uri) {
            crate::hyperlink::LinkKind::Ask { file: Some(p) } => Some(p),
            _ => None,
        };
        let host = crate::hyperlink::host(&ask.uri).map(str::to_string);
        let mut open = false;
        let mut cancel = false;
        let r = egui::Modal::new(egui::Id::new("osc8_link_ask")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(RichText::new(lang.t("Link öffnen?", "Open link?")).heading());
            ui.add_space(6.0);
            ui.label(lang.t(
                "Ein Programm im Terminal hat diesen Link gesetzt. Der angezeigte Text kann etwas anderes behaupten – öffne ihn nur, wenn du dem Ziel vertraust:",
                "A program in the terminal set this link. The visible text may claim something else – only open it if you trust the target:",
            ));
            ui.add_space(6.0);
            egui::Frame::NONE.fill(ui.visuals().extreme_bg_color).inner_margin(egui::Margin::same(8)).corner_radius(4.0).show(ui, |ui| {
                egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(&ask.uri).monospace()).wrap());
                });
            });
            // Was tatsächlich geöffnet wird: Rechnername bzw. der dekodierte lokale Pfad.
            if let Some(h) = host.as_deref().filter(|h| !h.is_empty()) {
                ui.add_space(4.0);
                ui.label(RichText::new(format!("{} {h}", lang.t("Rechner:", "Host:"))).monospace());
            }
            if let Some(p) = &file {
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(format!("{} {p}", lang.t("Datei:", "File:"))).monospace()).wrap());
                ui.add_space(4.0);
                ui.label(RichText::new(lang.t(
                    "Ausführbare Dateien werden nicht gestartet, sondern im Ordner angezeigt.",
                    "Executable files are not started but shown in their folder.",
                )).small().color(style::dim(ui)));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let cancel_btn = ui.button(lang.t("Abbrechen", "Cancel"));
                if !ui.memory(|m| m.focused().is_some()) {
                    cancel_btn.request_focus();
                }
                if cancel_btn.clicked() {
                    cancel = true;
                }
                if ui.button(lang.t("Öffnen", "Open")).clicked() {
                    open = true;
                }
            });
        });
        if r.should_close() {
            cancel = true;
        }
        if open {
            if let Some(a) = self.link_ask.take() {
                crate::hyperlink::open(&a.uri);
            }
        } else if cancel {
            self.link_ask = None;
        }
    }
}
