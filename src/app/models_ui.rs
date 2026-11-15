//! Einstellungen → Modelle (LLM-Server und Einstellungen je Modell) und der Dialog „Neuer Chat“ (#63).
//!
//! Die Oberfläche besteht aus freien Funktionen über den Einstellungsdaten (headless testbar); `App` verbindet sie mit
//! `Settings::llm_servers`, dem Speichern und dem [`Connector`] (Modellliste im Hintergrund abrufen).

use crate::llm::config::{self, LlmServer};
use crate::llm::{self, LlmError, ModelInfo, ServerKind, Transport};
use crate::mcp::McpLevel;
use crate::state::{ChatMeta, Lang, Settings};
use crate::style::{self, S1, S2};
use eframe::egui::{self, Color32, RichText, Ui};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;

fn warn_col() -> Color32 {
    Color32::from_rgb(0xe5, 0xc0, 0x7b)
}

fn err_col() -> Color32 {
    Color32::from_rgb(0xe0, 0x6c, 0x75)
}

fn kind_label(k: ServerKind, lang: Lang) -> &'static str {
    match k {
        ServerKind::Ollama => "Ollama",
        ServerKind::OpenAi => lang.t("OpenAI-kompatibel (LM Studio, llama.cpp, vLLM)", "OpenAI-compatible (LM Studio, llama.cpp, vLLM)"),
    }
}

fn default_url(k: ServerKind) -> &'static str {
    match k {
        ServerKind::Ollama => config::OLLAMA_LOCAL,
        ServerKind::OpenAi => "http://localhost:1234/v1",
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Verbinden (Modellliste im Hintergrund)

/// Ruft Modelllisten im Hintergrund ab (je Server höchstens ein Abruf). Ergebnisse holt [`Connector::poll`] ab.
pub(super) struct Connector {
    transport: Arc<dyn Transport>,
    jobs: HashMap<String, (Receiver<Result<Vec<ModelInfo>, LlmError>>, Arc<AtomicBool>)>,
    /// Letzter Fehler je Server (Text in der Sprache beim Abholen).
    pub errors: HashMap<String, String>,
}

impl Default for Connector {
    fn default() -> Self {
        Connector::with_transport(llm::system_transport())
    }
}

impl Connector {
    pub fn with_transport(transport: Arc<dyn Transport>) -> Self {
        Connector { transport, jobs: HashMap::new(), errors: HashMap::new() }
    }

    pub fn busy(&self, id: &str) -> bool {
        self.jobs.contains_key(id)
    }

    /// Abruf starten (läuft schon einer für diesen Server, passiert nichts).
    pub fn start(&mut self, server: &LlmServer, lang: Lang, repaint: Arc<dyn Fn() + Send + Sync>) {
        if self.busy(&server.id) {
            return;
        }
        self.errors.remove(&server.id);
        let ep = match server.endpoint(lang) {
            Ok(ep) => ep,
            Err(e) => {
                self.errors.insert(server.id.clone(), e);
                return;
            }
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (t, c) = (self.transport.clone(), cancel.clone());
        std::thread::spawn(move || {
            let _ = tx.send(llm::list_models(t.as_ref(), &ep, &c));
            repaint();
        });
        self.jobs.insert(server.id.clone(), (rx, cancel));
    }

    /// Fertige Abrufe übernehmen: Modellliste in `last_models` des Servers. `true`, wenn sich Einstellungen änderten.
    pub fn poll(&mut self, servers: &mut [LlmServer], lang: Lang) -> bool {
        let mut changed = false;
        let mut done = Vec::new();
        for (id, (rx, _)) in &self.jobs {
            match rx.try_recv() {
                Ok(r) => {
                    done.push(id.clone());
                    match r {
                        Ok(mut models) => {
                            models.truncate(config::MAX_LAST_MODELS);
                            if let Some(s) = servers.iter_mut().find(|s| &s.id == id) {
                                s.last_models = models;
                                changed = true;
                            }
                        }
                        Err(e) => {
                            self.errors.insert(id.clone(), e.text(lang));
                        }
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    done.push(id.clone());
                    self.errors.insert(id.clone(), lang.t("Abruf unerwartet beendet.", "The request ended unexpectedly.").into());
                }
            }
        }
        for id in done {
            self.jobs.remove(&id);
        }
        // Gelöschte Server: laufende Abrufe abbrechen.
        self.jobs.retain(|id, (_, c)| {
            let keep = servers.iter().any(|s| &s.id == id);
            if !keep {
                c.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            keep
        });
        changed
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Reiter „Modelle“

/// Formular „Server hinzufügen / bearbeiten“.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ServerForm {
    /// `None` = neuer Server.
    pub id: Option<String>,
    pub kind: ServerKind,
    pub name: String,
    pub url: String,
    /// Neu eingegebener Schlüssel (Klartext nur im Speicher); leer = vorhandenen behalten.
    pub key: String,
    pub has_key: bool,
    pub clear_key: bool,
    pub show_key: bool,
    /// Wartezeit auf Daten in Sekunden (0 = Standard).
    pub timeout: u32,
    pub error: Option<String>,
}

impl ServerForm {
    pub fn new_server(kind: ServerKind) -> Self {
        ServerForm { id: None, kind, name: String::new(), url: default_url(kind).into(), key: String::new(), has_key: false, clear_key: false, show_key: false, timeout: 0, error: None }
    }

    pub fn edit(s: &LlmServer) -> Self {
        ServerForm {
            id: Some(s.id.clone()),
            kind: s.kind,
            name: s.name.clone(),
            url: s.url.clone(),
            key: String::new(),
            has_key: !s.api_key.is_empty(),
            clear_key: false,
            show_key: false,
            timeout: s.receive_timeout_secs.unwrap_or(0),
            error: None,
        }
    }

    /// Fehler der Adresse beim Tippen (ohne Namensauflösung).
    pub fn url_error(&self, lang: Lang) -> Option<String> {
        config::quick_url_check(&self.url).err().map(|e| e.text(lang))
    }

    /// Formular übernehmen (neuer Server oder Änderung). Rückgabe: Kennung des Servers.
    pub fn apply(&self, servers: &mut Vec<LlmServer>, lang: Lang) -> Result<String, String> {
        if let Some(e) = self.url_error(lang) {
            return Err(e);
        }
        let key = self.key.trim();
        let api_key = if !key.is_empty() {
            if !llm::http::bearer_ok(key) {
                return Err(LlmError::BadKey.text(lang));
            }
            Some(llm::secret::protect(key).map_err(|e| e.text(lang))?)
        } else if self.clear_key {
            Some(String::new())
        } else {
            None
        };
        let name = if self.name.trim().is_empty() { kind_label(self.kind, lang).split(" (").next().unwrap_or("").to_string() } else { self.name.trim().to_string() };
        let timeout = (self.timeout > 0).then(|| self.timeout.clamp(5, 3600));
        let url = self.url.trim().to_string();
        let idx = match &self.id {
            Some(id) => servers.iter().position(|s| &s.id == id).ok_or_else(|| lang.t("Der Server wurde inzwischen entfernt.", "The server has been removed meanwhile.").to_string())?,
            None => {
                let id = config::new_server_id(servers);
                servers.push(LlmServer { id, ..LlmServer::default() });
                servers.len() - 1
            }
        };
        let s = &mut servers[idx];
        // Andere Adresse oder anderer Typ: die gemerkte Modellliste gehört zum alten Server.
        if s.url != url || s.kind != self.kind {
            s.last_models.clear();
        }
        s.kind = self.kind;
        s.name = name;
        s.url = url;
        s.receive_timeout_secs = timeout;
        if let Some(k) = api_key {
            s.api_key = k;
        }
        Ok(s.id.clone())
    }
}

/// Zustand des Reiters (nicht gespeichert).
#[derive(Default)]
pub(super) struct ModelsUi {
    pub form: Option<ServerForm>,
    /// Server, dessen Entfernen gerade bestätigt werden muss.
    pub confirm_remove: Option<String>,
}

/// Was der Reiter in diesem Bild ausgelöst hat.
#[derive(Default, Debug, PartialEq, Eq)]
pub(super) struct ModelsOut {
    /// Einstellungen geändert (speichern).
    pub changed: bool,
    /// Modellliste dieser Server abrufen.
    pub connect: Vec<String>,
}

/// Zeile: Beschriftung links (feste Breite), Steuerelement rechts – wie in den übrigen Reitern.
fn row(ui: &mut Ui, label: &str, add: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        let w = 170.0_f32.min(ui.available_width() * 0.4);
        ui.allocate_ui_with_layout(egui::Vec2::new(w, 22.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.label(label);
        });
        add(ui);
    });
}

/// Inhalt des Reiters „Modelle“.
pub(super) fn models_tab(ui: &mut Ui, lang: Lang, servers: &mut Vec<LlmServer>, st: &mut ModelsUi, conn: &Connector, expert: bool) -> ModelsOut {
    let mut out = ModelsOut::default();
    ui.label(RichText::new(lang.t(
        "Lokale Modelle (Ollama oder OpenAI-kompatible Server) antworten in Chats. Erlaubt sind nur dieser Rechner und private Netze. Die Stufe je Modell legt fest, was ein Modell in seinen Chats darf – sie ist unabhängig von der MCP-Stufe für externe Agenten (Reiter „Agenten“).",
        "Local models (Ollama or OpenAI-compatible servers) answer in chats. Only this computer and private networks are allowed. The level per model decides what a model may do in its chats – it is independent of the MCP level for external agents (“Agents” tab).",
    )).small().color(style::dim(ui)));
    ui.add_space(S2);
    let skipped = config::skipped_entries();
    if !skipped.is_empty() {
        ui.label(RichText::new(format!(
            "{} {}",
            lang.t("Nicht lesbare Server-Einträge in settings.json wurden übersprungen (fallen beim nächsten Speichern weg):", "Unreadable server entries in settings.json were skipped (they are dropped on the next save):"),
            skipped.join("; ")
        )).small().color(warn_col()));
        ui.add_space(S1);
    }

    if servers.is_empty() && st.form.is_none() {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.label(RichText::new(lang.t("Noch kein Server eingerichtet.", "No server set up yet.")).strong());
            ui.horizontal_wrapped(|ui| {
                ui.label(lang.t("Vorschlag: Ollama auf diesem Rechner (http://localhost:11434)", "Suggestion: Ollama on this computer (http://localhost:11434)"));
                if ui.button(lang.t("Hinzufügen", "Add")).clicked() {
                    let id = config::new_server_id(servers);
                    servers.push(LlmServer { id, name: "Ollama".into(), kind: ServerKind::Ollama, url: config::OLLAMA_LOCAL.into(), ..LlmServer::default() });
                    out.changed = true;
                }
            });
        });
        ui.add_space(S2);
    }

    let mut remove: Option<String> = None;
    for s in servers.iter_mut() {
        server_block(ui, lang, s, st, conn, expert, &mut out, &mut remove);
        ui.add_space(S2);
    }
    if let Some(id) = remove {
        servers.retain(|s| s.id != id);
        st.confirm_remove = None;
        if st.form.as_ref().and_then(|f| f.id.as_ref()) == Some(&id) {
            st.form = None;
        }
        out.changed = true;
    }

    if st.form.is_none() && ui.button(lang.t("+ Server hinzufügen", "+ Add server")).clicked() {
        st.form = Some(ServerForm::new_server(ServerKind::Ollama));
    }
    // Formular für einen neuen Server (Bearbeiten steht beim Server selbst).
    if st.form.as_ref().is_some_and(|f| f.id.is_none()) {
        ui.add_space(S1);
        server_form(ui, lang, servers, st, &mut out);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn server_block(ui: &mut Ui, lang: Lang, s: &mut LlmServer, st: &mut ModelsUi, conn: &Connector, expert: bool, out: &mut ModelsOut, remove: &mut Option<String>) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(s.label()).strong());
            ui.label(RichText::new(format!("{} · {}", kind_label(s.kind, lang).split(" (").next().unwrap_or(""), s.url)).small().color(style::dim(ui)));
            if !s.api_key.is_empty() {
                ui.label(RichText::new("🔑").small()).on_hover_text(lang.t("API-Schlüssel gespeichert (verschlüsselt für deinen Windows-Benutzer)", "API key stored (encrypted for your Windows user)"));
            }
        });
        ui.horizontal(|ui| {
            let busy = conn.busy(&s.id);
            if ui.add_enabled(!busy, egui::Button::new(lang.t("Verbinden", "Connect"))).on_hover_text(lang.t("Modellliste vom Server abrufen", "Fetch the model list from the server")).clicked() {
                out.connect.push(s.id.clone());
            }
            if busy {
                ui.spinner();
                ui.label(RichText::new(lang.t("Verbinde …", "Connecting …")).small().color(style::dim(ui)));
            }
            if ui.button(lang.t("Bearbeiten", "Edit")).clicked() {
                st.form = Some(ServerForm::edit(s));
            }
            if st.confirm_remove.as_deref() == Some(s.id.as_str()) {
                ui.label(RichText::new(lang.t("Wirklich entfernen? Chats mit diesem Server antworten dann nicht mehr.", "Really remove? Chats with this server will stop answering.")).small().color(warn_col()));
                if ui.button(lang.t("Entfernen", "Remove")).clicked() {
                    *remove = Some(s.id.clone());
                }
                if ui.button(lang.t("Abbrechen", "Cancel")).clicked() {
                    st.confirm_remove = None;
                }
            } else if ui.button(lang.t("Entfernen …", "Remove …")).clicked() {
                st.confirm_remove = Some(s.id.clone());
            }
        });
        if let Some(e) = conn.errors.get(&s.id) {
            ui.label(RichText::new(e).small().color(err_col()));
        }
        if st.form.as_ref().and_then(|f| f.id.as_deref()) == Some(s.id.as_str()) {
            let mut one = vec![s.clone()];
            server_form(ui, lang, &mut one, st, out);
            if let Some(n) = one.pop() {
                *s = n;
            }
        }
        let names = s.model_names();
        if names.is_empty() {
            ui.label(RichText::new(lang.t("Noch keine Modelle bekannt – „Verbinden“ ruft die Liste ab.", "No models known yet – “Connect” fetches the list.")).small().color(style::dim(ui)));
        }
        for name in names {
            model_row(ui, lang, s, &name, expert, out);
        }
    });
}

fn server_form(ui: &mut Ui, lang: Lang, servers: &mut Vec<LlmServer>, st: &mut ModelsUi, out: &mut ModelsOut) {
    let Some(f) = st.form.as_mut() else { return };
    let mut close = false;
    egui::Frame::new().fill(ui.visuals().faint_bg_color).inner_margin(egui::Margin::same(8)).corner_radius(egui::CornerRadius::same(style::RADIUS)).show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        ui.label(RichText::new(if f.id.is_none() { lang.t("Neuer Server", "New server") } else { lang.t("Server bearbeiten", "Edit server") }).strong());
        row(ui, lang.t("Typ", "Type"), |ui| {
            let before = f.kind;
            egui::ComboBox::from_id_salt("llm_kind").selected_text(kind_label(f.kind, lang)).width(300.0).show_ui(ui, |ui| {
                for k in [ServerKind::Ollama, ServerKind::OpenAi] {
                    ui.selectable_value(&mut f.kind, k, kind_label(k, lang));
                }
            });
            // Passende Standardadresse, solange die alte unverändert war.
            if f.kind != before && (f.url.trim().is_empty() || f.url.trim() == default_url(before)) {
                f.url = default_url(f.kind).into();
            }
        });
        row(ui, lang.t("Name", "Name"), |ui| {
            ui.add(egui::TextEdit::singleline(&mut f.name).hint_text(kind_label(f.kind, lang).split(" (").next().unwrap_or("")).desired_width(300.0));
        });
        row(ui, lang.t("Adresse", "Address"), |ui| {
            ui.add(egui::TextEdit::singleline(&mut f.url).desired_width(300.0));
        });
        if let Some(e) = f.url_error(lang) {
            ui.label(RichText::new(e).small().color(err_col()));
        }
        row(ui, lang.t("API-Schlüssel", "API key"), |ui| {
            let hint = if f.has_key && !f.clear_key { lang.t("gespeichert – leer lassen zum Behalten", "stored – leave empty to keep") } else { lang.t("optional", "optional") };
            ui.add(egui::TextEdit::singleline(&mut f.key).password(!f.show_key).hint_text(hint).desired_width(220.0));
            if ui.small_button(if f.show_key { lang.t("Verbergen", "Hide") } else { lang.t("Zeigen", "Show") }).clicked() {
                f.show_key = !f.show_key;
            }
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new(lang.t("Der Schlüssel wird verschlüsselt für deinen Windows-Benutzer gespeichert (DPAPI).", "The key is stored encrypted for your Windows user (DPAPI).")).small().color(style::dim(ui)));
            if f.has_key {
                ui.checkbox(&mut f.clear_key, lang.t("Schlüssel entfernen", "Remove key"));
            }
        });
        row(ui, lang.t("Wartezeit", "Timeout"), |ui| {
            ui.add(egui::DragValue::new(&mut f.timeout).range(0..=3600).suffix(" s"));
            ui.label(RichText::new(lang.t("auf Daten, auch auf das erste Token (0 = Standard 300 s)", "for data, including the first token (0 = default 300 s)")).small().color(style::dim(ui)));
        });
        if let Some(e) = &f.error {
            ui.label(RichText::new(e).small().color(err_col()));
        }
        ui.horizontal(|ui| {
            if ui.button(RichText::new(lang.t("Speichern", "Save")).strong()).clicked() {
                match f.apply(servers, lang) {
                    Ok(_) => {
                        close = true;
                        out.changed = true;
                    }
                    Err(e) => f.error = Some(e),
                }
            }
            if ui.button(lang.t("Abbrechen", "Cancel")).clicked() {
                close = true;
            }
        });
    });
    if close {
        st.form = None;
    }
}

fn level_label(l: McpLevel, lang: Lang) -> &'static str {
    match l {
        McpLevel::Off => lang.t("Aus", "Off"),
        McpLevel::Read => lang.t("Lesen", "Read"),
        McpLevel::Control => lang.t("Steuern", "Control"),
    }
}

/// Kurzbeschreibung eines Modells in der Kopfzeile: Werkzeuge und Kontextlänge.
pub(super) fn model_summary(s: &LlmServer, name: &str, lang: Lang) -> String {
    let tools = match s.tools_supported(name) {
        Some(true) => lang.t("Werkzeuge ✓", "tools ✓"),
        Some(false) => lang.t("keine Werkzeuge", "no tools"),
        None => lang.t("Werkzeuge ?", "tools ?"),
    };
    let ctx = s.context_length(name);
    let ctx = if ctx >= 1024 && ctx % 1024 == 0 { format!("{}k", ctx / 1024) } else { ctx.to_string() };
    let missing = if !s.last_models.is_empty() && s.info(name).is_none() { lang.t(" · vom Server nicht mehr gemeldet", " · no longer reported by the server") } else { "" };
    format!("{tools} · {} {ctx}{missing}", lang.t("Kontext", "context"))
}

fn model_row(ui: &mut Ui, lang: Lang, s: &mut LlmServer, name: &str, expert: bool, out: &mut ModelsOut) {
    let before = s.model_cfg(name);
    let mut cfg = before.clone();
    let mut job = egui::text::LayoutJob::default();
    job.append(name, 0.0, egui::TextFormat { font_id: egui::FontId::monospace(12.5), color: ui.visuals().text_color(), ..Default::default() });
    job.append(&model_summary(s, name, lang), 10.0, egui::TextFormat { font_id: egui::FontId::proportional(11.5), color: style::dim(ui), ..Default::default() });
    egui::CollapsingHeader::new(job).id_salt(("llm_model", s.id.as_str(), name)).show(ui, |ui| {
        row(ui, lang.t("Stufe", "Level"), |ui| {
            for l in [McpLevel::Off, McpLevel::Read, McpLevel::Control] {
                ui.selectable_value(&mut cfg.level, l, level_label(l, lang));
            }
        });
        ui.label(RichText::new(match cfg.level {
            McpLevel::Off => lang.t("Nur Chat, keine Paneldeck-Werkzeuge.", "Chat only, no Paneldeck tools."),
            McpLevel::Read => lang.t("Sitzungen, Terminalinhalt, Verlauf und Todo-Liste lesen.", "Read sessions, terminal content, history and the to-do list."),
            McpLevel::Control => lang.t("Zusätzlich Eingaben senden, Befehle ausführen, Sitzungen anlegen/schließen – Terminalinhalt kann Anweisungen enthalten, denen ein Modell fälschlich folgt.", "Also send input, run commands, create/close sessions – terminal content may contain instructions a model follows by mistake."),
        }).small().color(if cfg.level == McpLevel::Control { warn_col() } else { style::dim(ui) }));
        row(ui, lang.t("Nachfragen", "Ask first"), |ui| {
            ui.checkbox(&mut cfg.ask_write, lang.t("Vor Schreib-Aktionen nachfragen", "Ask before write actions"));
        });
        row(ui, lang.t("Max. Schritte", "Max steps"), |ui| {
            ui.add(egui::DragValue::new(&mut cfg.max_steps).range(1..=config::MAX_STEPS));
            ui.label(RichText::new(lang.t("Werkzeugaufrufe je Antwort", "tool calls per answer")).small().color(style::dim(ui)));
        });
        row(ui, lang.t("Werkzeugaufrufe", "Tool calling"), |ui| {
            let detected = s.info(name).and_then(|i| i.tools);
            let auto = format!("{} ({})", lang.t("Automatisch", "Auto"), match detected {
                Some(true) => lang.t("erkannt: ja", "detected: yes"),
                Some(false) => lang.t("erkannt: nein", "detected: no"),
                None => lang.t("unbekannt", "unknown"),
            });
            ui.selectable_value(&mut cfg.tools, None, auto);
            ui.selectable_value(&mut cfg.tools, Some(true), lang.t("An", "On"));
            ui.selectable_value(&mut cfg.tools, Some(false), lang.t("Aus", "Off"));
        });
        row(ui, lang.t("Kontextlänge", "Context length"), |ui| {
            let mut own = cfg.context_length.is_some();
            if ui.checkbox(&mut own, lang.t("eigene", "custom")).changed() {
                cfg.context_length = own.then(|| s.info(name).and_then(|i| i.context_length).unwrap_or(config::DEFAULT_CONTEXT).clamp(config::MIN_CONTEXT, config::MAX_CONTEXT));
            }
            if let Some(c) = cfg.context_length.as_mut() {
                ui.add(egui::DragValue::new(c).range(config::MIN_CONTEXT..=config::MAX_CONTEXT).speed(256.0).suffix(" tokens"));
            } else {
                let d = s.info(name).and_then(|i| i.context_length);
                ui.label(RichText::new(match d {
                    Some(n) => format!("{} {n}", lang.t("vom Server:", "from the server:")),
                    None => format!("{} {}", lang.t("Standard", "default"), config::DEFAULT_CONTEXT),
                }).small().color(style::dim(ui)));
            }
        });
        if cfg.level != McpLevel::Off {
            ui.label(RichText::new(lang.t("Werkzeuge (abgewählte darf das Modell nicht nutzen):", "Tools (unchecked ones are not available to the model):")).small().color(style::dim(ui)));
            let tools: Vec<_> = crate::mcp::tools().into_iter().filter(|t| t.level <= cfg.level && t.group.available(expert)).collect();
            egui::Grid::new(("llm_tools", s.id.as_str(), name)).num_columns(2).spacing([16.0, 2.0]).show(ui, |ui| {
                for (k, t) in tools.iter().enumerate() {
                    let mut on = !cfg.disabled_tools.iter().any(|d| d == t.name);
                    let desc: String = t.description.chars().take(300).collect();
                    if ui.checkbox(&mut on, RichText::new(t.name).monospace().size(12.0)).on_hover_text(desc).changed() {
                        if on {
                            cfg.disabled_tools.retain(|d| d != t.name);
                        } else {
                            cfg.disabled_tools.push(t.name.to_string());
                        }
                    }
                    if k % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
        }
    });
    if cfg != before {
        s.set_model_cfg(cfg);
        out.changed = true;
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Dialog „Neuer Chat“

/// Zustand des Dialogs „Neuer Chat“ (je Fenster).
#[derive(Clone, Debug, PartialEq, Default)]
pub(super) struct NewChatDlg {
    pub server: String,
    pub model: String,
    /// Ordner und Seitenleisten-Ordner des neuen Chats (aus dem Ordner-Kontextmenü).
    pub cwd: Option<String>,
    pub group: Option<String>,
}

impl NewChatDlg {
    /// Vorauswahl: zuletzt gewähltes Modell, sonst der erste Server und sein erstes Modell.
    pub fn new(settings: &Settings, cwd: Option<String>, group: Option<String>) -> Self {
        let servers = &settings.llm_servers;
        let last = settings.last_chat_model.as_ref().filter(|m| servers.iter().any(|s| s.id == m.server));
        let (server, model) = match last {
            Some(m) => (m.server.clone(), m.model.clone()),
            None => (servers.first().map(|s| s.id.clone()).unwrap_or_default(), String::new()),
        };
        let mut d = NewChatDlg { server, model, cwd, group };
        d.fix_model(servers);
        d
    }

    /// Modell passend zum Server: ein unbekanntes wird durch das erste bekannte ersetzt (leer, wenn keins bekannt ist).
    pub fn fix_model(&mut self, servers: &[LlmServer]) {
        let names = servers.iter().find(|s| s.id == self.server).map(|s| s.model_names()).unwrap_or_default();
        if !names.contains(&self.model) {
            self.model = names.into_iter().next().unwrap_or_default();
        }
    }

    pub fn meta(&self, servers: &[LlmServer]) -> Option<ChatMeta> {
        let ok = servers.iter().any(|s| s.id == self.server) && !self.model.trim().is_empty();
        ok.then(|| ChatMeta { server: self.server.clone(), model: self.model.trim().to_string() })
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
pub(super) struct DlgOut {
    pub create: bool,
    pub cancel: bool,
    pub connect: Option<String>,
    pub open_settings: bool,
}

/// Inhalt des Dialogs „Neuer Chat“. Enter legt den Chat an (wenn ein Modell gewählt ist), Esc bricht ab.
pub(super) fn new_chat_body(ui: &mut Ui, d: &mut NewChatDlg, servers: &[LlmServer], conn: &Connector, lang: Lang) -> DlgOut {
    let mut out = DlgOut::default();
    if servers.is_empty() {
        ui.label(lang.t(
            "Für Chats brauchst du einen Server mit lokalen Modellen, z. B. Ollama. Richte ihn in Einstellungen → Modelle ein.",
            "Chats need a server with local models, e.g. Ollama. Set it up in Settings → Models.",
        ));
        ui.add_space(S2);
        ui.horizontal(|ui| {
            if ui.button(RichText::new(lang.t("Einstellungen öffnen", "Open settings")).strong()).clicked() {
                out.open_settings = true;
            }
            if ui.button(lang.t("Abbrechen", "Cancel")).clicked() {
                out.cancel = true;
            }
        });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            out.cancel = true;
        }
        return out;
    }
    if !servers.iter().any(|s| s.id == d.server) {
        d.server = servers[0].id.clone();
    }
    d.fix_model(servers);
    let server = servers.iter().find(|s| s.id == d.server).unwrap_or(&servers[0]);
    egui::Grid::new("newchat_grid").num_columns(2).spacing([8.0, 8.0]).show(ui, |ui| {
        ui.label(lang.t("Server", "Server"));
        egui::ComboBox::from_id_salt("newchat_server").selected_text(server.label()).width(260.0).show_ui(ui, |ui| {
            for s in servers {
                if ui.selectable_label(s.id == d.server, s.label()).clicked() && s.id != d.server {
                    d.server = s.id.clone();
                    d.model.clear();
                }
            }
        });
        ui.end_row();
        ui.label(lang.t("Modell", "Model"));
        let names = server.model_names();
        if names.is_empty() {
            ui.label(RichText::new(lang.t("Noch keine Modelle bekannt", "No models known yet")).color(style::dim(ui)));
        } else {
            egui::ComboBox::from_id_salt("newchat_model").selected_text(d.model.clone()).width(260.0).show_ui(ui, |ui| {
                for n in &names {
                    ui.selectable_value(&mut d.model, n.clone(), n.as_str());
                }
            });
        }
        ui.end_row();
    });
    if !d.model.is_empty() {
        ui.label(RichText::new(model_summary(server, &d.model, lang)).small().color(style::dim(ui)));
    }
    ui.horizontal(|ui| {
        let busy = conn.busy(&server.id);
        if ui.add_enabled(!busy, egui::Button::new(lang.t("Verbinden", "Connect"))).on_hover_text(lang.t("Modellliste vom Server abrufen", "Fetch the model list from the server")).clicked() {
            out.connect = Some(server.id.clone());
        }
        if busy {
            ui.spinner();
        }
        if ui.link(lang.t("Einstellungen → Modelle", "Settings → Models")).clicked() {
            out.open_settings = true;
        }
    });
    if let Some(e) = conn.errors.get(&server.id) {
        ui.label(RichText::new(e).small().color(err_col()));
    }
    ui.add_space(S2);
    let ok = d.meta(servers).is_some();
    ui.horizontal(|ui| {
        if ui.add_enabled(ok, egui::Button::new(RichText::new(lang.t("Chat beginnen", "Start chat")).strong())).clicked() {
            out.create = true;
        }
        if ui.button(lang.t("Abbrechen", "Cancel")).clicked() {
            out.cancel = true;
        }
    });
    let (enter, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
    if enter && ok {
        out.create = true;
    }
    if esc {
        out.cancel = true;
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// Anbindung an die App

/// Index des Reiters „Modelle“ in den Einstellungen (am Ende angehängt, damit feste Indizes gültig bleiben).
pub(super) const SETTINGS_TAB: usize = 10;

impl super::App {
    fn llm_repaint(ctx: &egui::Context) -> Arc<dyn Fn() + Send + Sync> {
        let c = ctx.clone();
        Arc::new(move || c.request_repaint())
    }

    fn llm_connect(&mut self, ctx: &egui::Context, id: &str) {
        let lang = self.settings.language;
        if let Some(s) = self.settings.llm_servers.iter().find(|s| s.id == id) {
            self.llm_conn.start(s, lang, Self::llm_repaint(ctx));
        }
    }

    /// Reiter Einstellungen → Modelle. Änderungen werden sofort gespeichert (Schlüssel und Stufen sind Sicherheitseinstellungen).
    pub(super) fn models_settings_ui(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let lang = self.settings.language;
        let out = models_tab(ui, lang, &mut self.settings.llm_servers, &mut self.models_ui, &self.llm_conn, crate::mcp::expert_mode());
        for id in &out.connect {
            self.llm_connect(ctx, id);
        }
        if out.changed {
            crate::state::save_json(&self.dir.join("settings.json"), &self.settings);
        }
    }

    /// Dialog „Neuer Chat“ (je Fenster).
    pub(super) fn new_chat_ui(&mut self, ctx: &egui::Context) {
        let Some(mut d) = self.new_chat.take() else { return };
        let lang = self.settings.language;
        let mut out = DlgOut::default();
        egui::Window::new(lang.t("Neuer Chat", "New chat")).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO).show(ctx, |ui| {
            out = new_chat_body(ui, &mut d, &self.settings.llm_servers, &self.llm_conn, lang);
        });
        if let Some(id) = &out.connect {
            self.llm_connect(ctx, id);
        }
        if out.open_settings {
            self.settings_open = true;
            self.settings_tab = SETTINGS_TAB;
            return;
        }
        if out.create {
            if let Some(meta) = d.meta(&self.settings.llm_servers) {
                if self.settings.last_chat_model.as_ref() != Some(&meta) {
                    self.settings.last_chat_model = Some(meta.clone());
                    crate::state::save_json(&self.dir.join("settings.json"), &self.settings);
                }
                self.new_session(ctx, super::NewOpts { cwd: d.cwd.clone(), group: d.group.clone(), chat: Some(meta), ..super::NewOpts::default() });
                return;
            }
        }
        if !out.cancel {
            self.new_chat = Some(d);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(name: &str, tools: Option<bool>) -> ModelInfo {
        ModelInfo { name: name.into(), tools, context_length: Some(4096) }
    }
    use crate::llm::http::fake::{Canned, Fake};
    use crate::llm::http::Method;
    use eframe::egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
    use std::time::{Duration, Instant};

    struct H {
        ctx: egui::Context,
        t: f64,
        texts: Vec<String>,
        shapes: Vec<egui::epaint::ClippedShape>,
    }

    impl H {
        fn new() -> H {
            H { ctx: egui::Context::default(), t: 0.0, texts: Vec::new(), shapes: Vec::new() }
        }
        fn run<R: Default>(&mut self, events: Vec<Event>, mut f: impl FnMut(&mut Ui) -> R) -> R {
            self.t += 0.05;
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 1400.0))), time: Some(self.t), events, ..Default::default() };
            let mut res = R::default();
            let full = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    res = f(ui);
                });
            });
            self.texts = full.shapes.iter().filter_map(|s| if let egui::Shape::Text(t) = &s.shape { Some(t.galley.text().to_string()) } else { None }).collect();
            self.shapes = full.shapes;
            res
        }
        fn has(&self, s: &str) -> bool {
            self.texts.iter().any(|t| t.contains(s))
        }
        fn pos_of(&self, text: &str) -> Pos2 {
            self.shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text() == text => Some(t.visual_bounding_rect().center()),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("{text} nicht gefunden in {:?}", self.texts))
        }
        /// Klick auf den Text `text`; liefert die Ergebnisse der drei Bilder (zusammengefasst über `merge`).
        fn click<R: Default>(&mut self, text: &str, mut f: impl FnMut(&mut Ui) -> R, mut merge: impl FnMut(&mut R, R)) -> R {
            let pos = self.pos_of(text);
            let btn = |pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
            let mut acc = self.run(vec![Event::PointerMoved(pos)], &mut f);
            for e in [btn(true), btn(false)] {
                let r = self.run(vec![e], &mut f);
                merge(&mut acc, r);
            }
            acc
        }
    }

    fn merge_models(a: &mut ModelsOut, b: ModelsOut) {
        a.changed |= b.changed;
        a.connect.extend(b.connect);
    }

    fn merge_dlg(a: &mut DlgOut, b: DlgOut) {
        a.create |= b.create;
        a.cancel |= b.cancel;
        a.open_settings |= b.open_settings;
        a.connect = a.connect.take().or(b.connect);
    }

    #[test]
    fn models_tab_suggests_ollama_adds_server_and_shows_url_errors() {
        for lang in [Lang::En, Lang::De] {
            let conn = Connector::with_transport(Arc::new(Fake::default()));
            let mut servers: Vec<LlmServer> = Vec::new();
            let mut st = ModelsUi::default();
            let mut h = H::new();
            h.run(vec![], |ui| models_tab(ui, lang, &mut servers, &mut st, &conn, false));
            assert!(h.has(lang.t("Ollama auf diesem Rechner", "Ollama on this computer")), "{:?}", h.texts);
            assert!(h.has(lang.t("unabhängig von der MCP-Stufe", "independent of the MCP level")));
            let o = h.click(lang.t("Hinzufügen", "Add"), |ui| models_tab(ui, lang, &mut servers, &mut st, &conn, false), merge_models);
            assert!(o.changed);
            assert_eq!(servers.len(), 1);
            assert_eq!((servers[0].kind, servers[0].url.as_str()), (ServerKind::Ollama, "http://localhost:11434"));
            assert!(o.connect.is_empty(), "kein automatischer Abruf");
            h.run(vec![], |ui| models_tab(ui, lang, &mut servers, &mut st, &conn, false));
            assert!(h.has(lang.t("Noch keine Modelle bekannt", "No models known yet")));
            // Verbinden meldet den Server
            let o = h.click(lang.t("Verbinden", "Connect"), |ui| models_tab(ui, lang, &mut servers, &mut st, &conn, false), merge_models);
            assert_eq!(o.connect, vec![servers[0].id.clone()]);
            // Neuer Server mit öffentlicher Adresse: Fehler sofort, Speichern legt nichts an.
            h.click(lang.t("+ Server hinzufügen", "+ Add server"), |ui| models_tab(ui, lang, &mut servers, &mut st, &conn, false), merge_models);
            assert!(st.form.as_ref().is_some_and(|f| f.id.is_none()));
            st.form.as_mut().unwrap().url = "http://8.8.8.8:11434".into();
            h.run(vec![], |ui| models_tab(ui, lang, &mut servers, &mut st, &conn, false));
            assert!(h.has(lang.t("Adresse nicht erlaubt", "Address not allowed")), "{:?}", h.texts);
            h.run(vec![], |ui| models_tab(ui, lang, &mut servers, &mut st, &conn, false));
            let o = h.click(lang.t("Speichern", "Save"), |ui| models_tab(ui, lang, &mut servers, &mut st, &conn, false), merge_models);
            assert!(!o.changed && servers.len() == 1 && st.form.as_ref().is_some_and(|f| f.error.is_some()));
            // Gültige private Adresse: gespeichert
            let f = st.form.as_mut().unwrap();
            f.url = "http://192.168.1.20:1234/v1".into();
            f.kind = ServerKind::OpenAi;
            f.error = None;
            h.run(vec![], |ui| models_tab(ui, lang, &mut servers, &mut st, &conn, false));
            let o = h.click(lang.t("Speichern", "Save"), |ui| models_tab(ui, lang, &mut servers, &mut st, &conn, false), merge_models);
            assert!(o.changed && st.form.is_none());
            assert_eq!(servers.len(), 2);
            let want = lang.t("OpenAI-kompatibel", "OpenAI-compatible");
            assert_eq!((servers[1].kind, servers[1].url.as_str(), servers[1].name.as_str()), (ServerKind::OpenAi, "http://192.168.1.20:1234/v1", want));
        }
    }

    #[test]
    fn model_row_controls_change_the_model_config() {
        for lang in [Lang::En, Lang::De] {
            let conn = Connector::with_transport(Arc::new(Fake::default()));
            let mut servers = vec![LlmServer { id: "s1".into(), name: "Box".into(), url: "http://localhost:11434".into(), last_models: vec![info("llama3", Some(true)), info("tiny", Some(false))], ..Default::default() }];
            let mut st = ModelsUi::default();
            let mut h = H::new();
            h.run(vec![], |ui| models_tab(ui, lang, &mut servers, &mut st, &conn, false));
            assert!(h.has("llama3") && h.has(lang.t("keine Werkzeuge", "no tools")), "{:?}", h.texts);
            let head = format!("llama3{}", model_summary(&servers[0], "llama3", lang));
            h.click(&head, |ui| models_tab(ui, lang, &mut servers, &mut st, &conn, false), merge_models);
            for _ in 0..5 {
                h.run(vec![], |ui| models_tab(ui, lang, &mut servers, &mut st, &conn, false));
            }
            assert!(h.has(lang.t("Vor Schreib-Aktionen nachfragen", "Ask before write actions")), "{:?}", h.texts);
            assert!(h.has("list_sessions"), "Werkzeugliste bei Stufe Lesen");
            assert!(!h.has("send_text"), "Schreib-Werkzeuge erst bei Steuern");
            // Stufe Steuern
            let o = h.click(lang.t("Steuern", "Control"), |ui| models_tab(ui, lang, &mut servers, &mut st, &conn, false), merge_models);
            assert!(o.changed);
            assert_eq!(servers[0].model_cfg("llama3").level, McpLevel::Control);
            h.run(vec![], |ui| models_tab(ui, lang, &mut servers, &mut st, &conn, false));
            assert!(h.has("send_text"), "{:?}", h.texts);
            // Werkzeug abwählen
            let o = h.click("list_sessions", |ui| models_tab(ui, lang, &mut servers, &mut st, &conn, false), merge_models);
            assert!(o.changed);
            assert_eq!(servers[0].model_cfg("llama3").disabled_tools, vec!["list_sessions".to_string()]);
            // Nachfragen ausschalten, Werkzeugaufrufe „Aus“
            h.click(lang.t("Vor Schreib-Aktionen nachfragen", "Ask before write actions"), |ui| models_tab(ui, lang, &mut servers, &mut st, &conn, false), merge_models);
            assert!(!servers[0].model_cfg("llama3").ask_write);
            h.click(lang.t("An", "On"), |ui| models_tab(ui, lang, &mut servers, &mut st, &conn, false), merge_models);
            assert_eq!(servers[0].tools_supported("llama3"), Some(true));
            h.click(lang.t("eigene", "custom"), |ui| models_tab(ui, lang, &mut servers, &mut st, &conn, false), merge_models);
            assert_eq!(servers[0].model_cfg("llama3").context_length, Some(4096));
            assert_eq!(servers[0].models.len(), 1, "nur das geänderte Modell wird gespeichert");
        }
    }

    #[test]
    fn edit_form_keeps_and_clears_the_key_and_validates() {
        let lang = Lang::En;
        let mut servers = vec![LlmServer { id: "s1".into(), name: "A".into(), url: "http://localhost:11434".into(), api_key: "BLOB".into(), last_models: vec![info("m", None)], ..Default::default() }];
        let mut f = ServerForm::edit(&servers[0]);
        assert!(f.has_key && f.key.is_empty());
        f.name = "B".into();
        assert_eq!(f.apply(&mut servers, lang).unwrap(), "s1");
        assert_eq!((servers[0].name.as_str(), servers[0].api_key.as_str(), servers[0].last_models.len()), ("B", "BLOB", 1), "Schlüssel bleibt, Liste bleibt");
        f.clear_key = true;
        f.url = "http://localhost:11435".into();
        f.apply(&mut servers, lang).unwrap();
        assert!(servers[0].api_key.is_empty());
        assert!(servers[0].last_models.is_empty(), "neue Adresse: alte Modellliste verworfen");
        f.key = "bad\nkey".into();
        assert!(f.apply(&mut servers, lang).unwrap_err().contains("API key"));
        f.url = "http://8.8.8.8".into();
        f.key.clear();
        assert!(f.apply(&mut servers, lang).unwrap_err().contains("Address not allowed"));
        assert_eq!(servers[0].url, "http://localhost:11435", "nichts übernommen");
    }

    #[cfg(windows)]
    #[test]
    fn new_key_is_stored_encrypted() {
        let mut servers = Vec::new();
        let mut f = ServerForm::new_server(ServerKind::OpenAi);
        f.key = " sk-secret ".into();
        let id = f.apply(&mut servers, Lang::En).unwrap();
        let s = servers.iter().find(|s| s.id == id).unwrap();
        assert!(!s.api_key.is_empty() && !s.api_key.contains("sk-secret"));
        assert_eq!(llm::secret::unprotect(&s.api_key).unwrap(), "sk-secret");
        assert_eq!(s.endpoint(Lang::En).unwrap().api_key.as_deref(), Some("sk-secret"));
    }

    #[test]
    fn connector_stores_the_model_list_and_reports_errors() {
        let tags = r#"{"models":[{"name":"llama3"},{"name":"phi"}]}"#;
        let show = r#"{"capabilities":["completion","tools"],"model_info":{"llama.context_length":8192}}"#;
        let fake = Fake::default()
            .with(Method::Get, "http://localhost:11434/api/tags", Canned::ok(tags))
            .with(Method::Post, "http://localhost:11434/api/show", Canned::ok(show));
        let mut c = Connector::with_transport(Arc::new(fake));
        let mut servers = vec![
            LlmServer { id: "a".into(), url: "http://localhost:11434".into(), ..Default::default() },
            LlmServer { id: "b".into(), url: "http://localhost:9".into(), ..Default::default() },
            LlmServer { id: "k".into(), url: "http://localhost:11434".into(), api_key: "%%%".into(), ..Default::default() },
        ];
        for s in servers.clone() {
            c.start(&s, Lang::En, Arc::new(|| {}));
        }
        assert!(c.errors.get("k").is_some_and(|e| e.contains("decrypted")), "Schlüsselfehler sofort");
        assert!(c.busy("a") && c.busy("b") && !c.busy("k"));
        let t = Instant::now();
        let mut changed = false;
        while (c.busy("a") || c.busy("b")) && t.elapsed() < Duration::from_secs(5) {
            changed |= c.poll(&mut servers, Lang::En);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(changed);
        assert_eq!(servers[0].last_models, vec![ModelInfo { name: "llama3".into(), tools: Some(true), context_length: Some(8192) }, ModelInfo { name: "phi".into(), tools: Some(true), context_length: Some(8192) }]);
        assert!(servers[1].last_models.is_empty());
        assert!(c.errors.contains_key("b"), "unbekannte Adresse der Attrappe: Fehler");
    }

    fn settings_with(servers: Vec<LlmServer>, last: Option<ChatMeta>) -> Settings {
        Settings { llm_servers: servers, last_chat_model: last, ..Settings::default() }
    }

    #[test]
    fn dialog_preselects_and_fixes_the_model() {
        let a = LlmServer { id: "a".into(), name: "A".into(), last_models: vec![info("m1", None), info("m2", None)], ..Default::default() };
        let b = LlmServer { id: "b".into(), name: "B".into(), ..Default::default() };
        let s = settings_with(vec![a.clone(), b.clone()], None);
        let d = NewChatDlg::new(&s, None, None);
        assert_eq!((d.server.as_str(), d.model.as_str()), ("a", "m1"));
        let s = settings_with(vec![a.clone(), b.clone()], Some(ChatMeta { server: "a".into(), model: "m2".into() }));
        assert_eq!(NewChatDlg::new(&s, None, None).model, "m2", "letzte Wahl");
        let s = settings_with(vec![a.clone(), b.clone()], Some(ChatMeta { server: "a".into(), model: "weg".into() }));
        assert_eq!(NewChatDlg::new(&s, None, None).model, "m1", "unbekanntes Modell → erstes");
        let s = settings_with(vec![a.clone(), b.clone()], Some(ChatMeta { server: "gone".into(), model: "m2".into() }));
        assert_eq!(NewChatDlg::new(&s, None, None).server, "a");
        let s = settings_with(vec![b.clone(), a.clone()], Some(ChatMeta { server: "b".into(), model: "x".into() }));
        let d = NewChatDlg::new(&s, Some("C:\\x".into()), Some("grp".into()));
        assert_eq!((d.server.as_str(), d.model.as_str(), d.meta(&s.llm_servers)), ("b", "", None), "ohne Modell kein Chat");
        let d = NewChatDlg { server: "a".into(), model: "m2".into(), ..Default::default() };
        assert_eq!(d.meta(&s.llm_servers), Some(ChatMeta { server: "a".into(), model: "m2".into() }));
    }

    #[test]
    fn dialog_ui_without_server_offers_settings() {
        for lang in [Lang::En, Lang::De] {
            let conn = Connector::with_transport(Arc::new(Fake::default()));
            let mut d = NewChatDlg::default();
            let mut h = H::new();
            h.run(vec![], |ui| new_chat_body(ui, &mut d, &[], &conn, lang));
            assert!(h.has(lang.t("Einstellungen → Modelle", "Settings → Models")), "{:?}", h.texts);
            let o = h.click(lang.t("Einstellungen öffnen", "Open settings"), |ui| new_chat_body(ui, &mut d, &[], &conn, lang), merge_dlg);
            assert!(o.open_settings && !o.create);
            let o = h.run(vec![Event::Key { key: Key::Escape, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }], |ui| new_chat_body(ui, &mut d, &[], &conn, lang));
            assert!(o.cancel);
        }
    }

    #[test]
    fn dialog_ui_connect_enter_and_start() {
        for lang in [Lang::En, Lang::De] {
            let conn = Connector::with_transport(Arc::new(Fake::default()));
            let empty = vec![LlmServer { id: "a".into(), name: "Box".into(), ..Default::default() }];
            let mut d = NewChatDlg { server: "a".into(), ..Default::default() };
            let mut h = H::new();
            h.run(vec![], |ui| new_chat_body(ui, &mut d, &empty, &conn, lang));
            assert!(h.has(lang.t("Noch keine Modelle bekannt", "No models known yet")));
            let enter = || Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE };
            assert!(!h.run(vec![enter()], |ui| new_chat_body(ui, &mut d, &empty, &conn, lang)).create, "ohne Modell legt Enter nichts an");
            let o = h.click(lang.t("Verbinden", "Connect"), |ui| new_chat_body(ui, &mut d, &empty, &conn, lang), merge_dlg);
            assert_eq!(o.connect.as_deref(), Some("a"));
            // Nach dem Verbinden: Modell vorausgewählt, Enter legt an
            let full = vec![LlmServer { last_models: vec![info("llama3", Some(false))], ..empty[0].clone() }];
            h.run(vec![], |ui| new_chat_body(ui, &mut d, &full, &conn, lang));
            assert_eq!(d.model, "llama3");
            assert!(h.has(lang.t("keine Werkzeuge", "no tools")), "{:?}", h.texts);
            assert!(h.run(vec![enter()], |ui| new_chat_body(ui, &mut d, &full, &conn, lang)).create);
            let o = h.click(lang.t("Chat beginnen", "Start chat"), |ui| new_chat_body(ui, &mut d, &full, &conn, lang), merge_dlg);
            assert!(o.create);
        }
    }
}
