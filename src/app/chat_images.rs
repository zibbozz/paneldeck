//! Bilder in Chats im Hub (#357): Einfügen aus der Zwischenablage, über die Büroklammer und per Ablegen aus dem
//! Explorer; je Bild Ordner und Bildverständnis des Modells an den Chat; Ablehnung durch den Server merken und die
//! Antwort ohne Bild wiederholen; Bilder aus `screenshot`/`read_file` für Modelle mit Bildverständnis.

use super::chat_images_ui::ImageReq;
use super::*;
use crate::chat::images::{self, ImageEntry, ImageSink};

impl App {
    /// Bilderordner des Chats `id`.
    pub(super) fn chat_images_dir(&self, id: u64) -> std::path::PathBuf {
        images::dir(&self.dir, id)
    }

    /// Kann das Modell des Chats Bilder lesen (eigene Angabe bzw. gemerkte Ablehnung vor Erkanntem; `None` = unbekannt)?
    pub(super) fn chat_vision(&self, id: u64) -> Option<bool> {
        let m = self.entries.get(&id)?.meta.chat.as_ref()?;
        self.settings.llm_servers.iter().find(|s| s.id == m.server)?.vision_supported(m.model.trim())
    }

    /// Bilder des Chats für die nächste Anfrage (`TurnContext::images`).
    pub(super) fn chat_turn_images(&self, id: u64) -> images::TurnImages {
        let entries = self.entries.get(&id).and_then(|e| e.chat.as_ref()).map(|c| c.file.images.clone()).unwrap_or_default();
        images::TurnImages { dir: Some(self.chat_images_dir(id)), entries }
    }

    /// Bild aus der Oberfläche: Strg+V (Bild in der Zwischenablage) oder Büroklammer (Datei-Auswahl).
    pub(super) fn chat_image(&mut self, ctx: &egui::Context, id: u64, req: ImageReq) {
        match req {
            ImageReq::Paste => {
                // Nur ein Bild ohne Text (Text fügt egui selbst ein).
                if let Some(rgba) = crate::platform::clipboard::image() {
                    // Riesige Bilder gar nicht erst aufbereiten (Speicher).
                    if let Err(e) = images::check_size(rgba.width(), rgba.height()) {
                        if let Some(c) = self.entries.get_mut(&id).and_then(|e| e.chat.as_mut()) {
                            c.img.error = Some(e);
                        }
                        return;
                    }
                    self.chat_add_image(ctx, id, move |dir, n| images::import_rgba(&dir, n, rgba));
                }
            }
            ImageReq::Pick => {
                let start = self.entries.get(&id).map(|e| e.meta.cwd.clone()).unwrap_or_default();
                let files = crate::platform::notify::pick_images(&start);
                self.chat_add_image_files(ctx, id, files);
            }
        }
    }

    /// Bilddateien in den Chat (Datei-Auswahl, Ablegen aus dem Explorer): je Datei ein Platzhalter an der Cursor-Position.
    pub(super) fn chat_add_image_files(&mut self, ctx: &egui::Context, id: u64, files: Vec<std::path::PathBuf>) {
        for f in files {
            self.chat_add_image(ctx, id, move |dir, n| images::import_file(&dir, n, &f));
        }
    }

    /// Nummer vergeben, Platzhalter einfügen und das Bild im Hintergrund aufbereiten (Senden wartet darauf).
    fn chat_add_image(&mut self, ctx: &egui::Context, id: u64, work: impl FnOnce(std::path::PathBuf, u32) -> Result<ImageEntry, String> + Send + 'static) {
        let dir = self.chat_images_dir(id);
        let Some(c) = self.entries.get_mut(&id).and_then(|e| e.chat.as_mut()) else { return };
        let n = c.img.reserve(&c.file);
        super::chat_images_ui::insert_token(ctx, super::chat_ui::input_id(id), &mut c.draft, n);
        c.focus_input = true;
        let rctx = ctx.clone();
        let d = dir.clone();
        c.img.start(n, dir, move || work(d, n), std::sync::Arc::new(move || rctx.request_repaint()));
        ctx.request_repaint();
    }

    /// Jedes Bild (nach dem Abholen der Antworten): Ordner und Bildverständnis an jeden Chat, fertige Aufbereitungen
    /// übernehmen. Hat der Server ein Bild abgelehnt (nur gemeldet, solange das Bildverständnis des Modells unbekannt
    /// war), wird nach dem Ende der Runde „keine Bilder“ am Modell gemerkt – nie gegen eine erkannte Fähigkeit oder eine
    /// Wahl des Nutzers – und die Antwort einmal ohne Bild wiederholt.
    pub(super) fn poll_chat_images(&mut self, ctx: &egui::Context) {
        let ids: Vec<u64> = self.entries.iter().filter(|(_, e)| e.chat.is_some()).map(|(id, _)| *id).collect();
        for id in ids {
            let dir = self.chat_images_dir(id);
            let vision = self.chat_vision(id);
            let Some(e) = self.entries.get_mut(&id) else { continue };
            let meta = e.meta.chat.clone();
            let Some(c) = e.chat.as_mut() else { continue };
            c.img.dir = Some(dir);
            c.img.vision = vision;
            let changed = c.poll_images();
            // Erst nach dem Ende der Runde (der Fehler kommt nach `VisionRejected`; vorher ginge das Wiederholen nicht).
            let rejected = c.img.rejected && !c.busy();
            let retry = rejected && !c.img.auto_retried;
            if rejected {
                c.img.rejected = false;
                c.img.auto_retried = true;
            }
            if changed {
                self.save_chat(id);
            }
            if rejected {
                if let Some(m) = meta
                    && let Some(s) = self.settings.llm_servers.iter_mut().find(|s| s.id == m.server)
                {
                    let name = m.model.trim();
                    let mut cfg = s.model_cfg(name);
                    if cfg.vision.is_none() && s.info(name).and_then(|i| i.vision).is_none() {
                        cfg.vision = Some(false);
                        s.set_model_cfg(cfg);
                        state::save_json(&self.dir.join("settings.json"), &self.settings);
                    }
                }
                let vision = self.chat_vision(id);
                if let Some(c) = self.entries.get_mut(&id).and_then(|e| e.chat.as_mut()) {
                    c.img.vision = vision;
                }
                if retry && vision == Some(false) {
                    self.chat_action(ctx, id, &Action::ChatRegenerate(id));
                }
            }
        }
    }

    /// Wohin ein Werkzeug des Chats sein Bild legt (#357): nur `screenshot` und `read_file` auf eine Bilddatei, nur für
    /// Modelle, die Bilder lesen können (unbekannt = versuchen). Vergibt dabei die Nummer.
    pub(super) fn chat_tool_image_sink(&mut self, chat: u64, call: &crate::chat::store::ToolCall) -> Option<ImageSink> {
        let wants = match call.name.as_str() {
            "screenshot" => true,
            "read_file" => call.arguments.get("path").and_then(|p| p.as_str()).is_some_and(images::is_image_name),
            _ => false,
        };
        if !wants || self.chat_vision(chat) == Some(false) {
            return None;
        }
        let dir = self.chat_images_dir(chat);
        let c = self.entries.get_mut(&chat).and_then(|e| e.chat.as_mut())?;
        let n = c.img.reserve(&c.file);
        // Nur dieser Aufruf darf ein Bild bringen (mit dieser Nummer oder einem schon gelesenen von `read_file`).
        c.img.expected.insert(call.id.clone(), n);
        let reuse = if call.name == "read_file" { c.file.images.iter().filter(|e| e.source == images::Source::Tool && !e.source_path.is_empty()).cloned().collect() } else { Vec::new() };
        Some(ImageSink { dir, n, reuse })
    }
}
