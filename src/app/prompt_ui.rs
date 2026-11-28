//! Einstellungen → Modelle: Schalter für die Teile des Master-Prompts der Chats (#341) – global mit Vorschau und
//! Token-Zähler, je Modell als eigene Auswahl samt Wahl des Familienzusatzes.
//!
//! Der Zähler nimmt den Tokenizer des Servers, wo es einen gibt (`llm::count_tokens`: llama.cpp-Server und vLLM über
//! `POST /tokenize`, im Hintergrund), sonst Paneldecks Schätzung mit `~`. Er zählt nach jeder Änderung neu (das
//! Ergebnis je Text wird behalten, ein Server ohne Tokenizer wird nicht erneut gefragt).

use crate::chat::llm_backend::PromptPlan;
use crate::chat::master_prompt::Variant;
use crate::chat::tools::ToolPolicy;
use crate::llm::config::{LlmModel, LlmServer, LlmServers};
use crate::llm::prompt_parts::{self, PromptOverride, PromptPart, PromptParts};
use crate::llm::{self, LlmError, ServerKind, Transport};
use crate::mcp::McpLevel;
use crate::state::Lang;
use crate::style::{self, S1};
use eframe::egui::{self, RichText, Ui};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Arc;

/// Höhe der Vorschau (scrollt).
const PREVIEW_H: f32 = 220.0;

/// Ergebnis des Zählers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Counted {
    /// Tokenizer des Servers.
    Server(usize),
    /// Schätzung (kein Tokenizer, Fehler oder noch keine Antwort).
    Estimate(usize),
    /// Der Server zählt gerade; bis dahin die Schätzung.
    Pending(usize),
}

impl Counted {
    /// `4342 Tokens (Tokenizer des Servers)` bzw. `~4342 Tokens (geschätzt)`.
    pub fn text(self, lang: Lang) -> String {
        match (self, lang) {
            (Counted::Server(n), Lang::De) => format!("{n} Tokens (Tokenizer des Servers)"),
            (Counted::Server(n), Lang::En) => format!("{n} tokens (server tokenizer)"),
            (Counted::Estimate(n), Lang::De) => format!("~{n} Tokens (geschätzt)"),
            (Counted::Estimate(n), Lang::En) => format!("~{n} tokens (estimate)"),
            (Counted::Pending(n), Lang::De) => format!("~{n} Tokens (der Server zählt …)"),
            (Counted::Pending(n), Lang::En) => format!("~{n} tokens (the server is counting …)"),
        }
    }
}

struct CountJob {
    key: u64,
    server: (String, String),
    rx: Receiver<Result<Option<usize>, LlmError>>,
    cancel: Arc<AtomicBool>,
}

/// Zählt Prompt-Tokens im Hintergrund (höchstens ein Abruf zur Zeit; ein neuer Text bricht den alten ab).
pub(super) struct TokenCounter {
    transport: Arc<dyn Transport>,
    done: HashMap<u64, Counted>,
    /// Server (Kennung, Adresse) ohne Tokenizer: nicht erneut fragen.
    no_tokenizer: HashSet<(String, String)>,
    job: Option<CountJob>,
}

impl Default for TokenCounter {
    fn default() -> Self {
        TokenCounter::with_transport(llm::system_transport())
    }
}

impl TokenCounter {
    pub fn with_transport(transport: Arc<dyn Transport>) -> Self {
        TokenCounter { transport, done: HashMap::new(), no_tokenizer: HashSet::new(), job: None }
    }

    fn key(server: Option<&LlmServer>, model: &str, text: &str) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        server.map(|s| (&s.id, &s.url, s.kind == ServerKind::Ollama)).hash(&mut h);
        (model, text).hash(&mut h);
        h.finish()
    }

    /// Fertigen Abruf übernehmen.
    fn poll(&mut self) {
        let Some(j) = &self.job else { return };
        let r = match j.rx.try_recv() {
            Ok(r) => Some(r),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(LlmError::Cancelled)),
        };
        let Some(r) = r else { return };
        let j = self.job.take().expect("job");
        let fallback = match self.done.get(&j.key) {
            Some(Counted::Pending(n) | Counted::Estimate(n) | Counted::Server(n)) => *n,
            None => 0,
        };
        let c = match r {
            Ok(Some(n)) => Counted::Server(n),
            Ok(None) => {
                self.no_tokenizer.insert(j.server);
                Counted::Estimate(fallback)
            }
            Err(_) => Counted::Estimate(fallback),
        };
        self.done.insert(j.key, c);
    }

    /// Tokens von `text` für `model` auf `server` (`None` = kein Server). `est` = Paneldecks Schätzung (dieselbe wie
    /// für Kontextlänge und Fassung); sie gilt, solange bzw. wenn der Server nicht zählen kann. Startet bei Bedarf den
    /// Abruf.
    pub fn count(&mut self, server: Option<&LlmServer>, model: &str, text: &str, est: usize, lang: Lang, repaint: &Arc<dyn Fn() + Send + Sync>) -> Counted {
        self.poll();
        let key = Self::key(server, model, text);
        if let Some(c) = self.done.get(&key) {
            return *c;
        }
        let ask = server.filter(|s| s.kind == ServerKind::OpenAi && !self.no_tokenizer.contains(&(s.id.clone(), s.url.clone())));
        let Some((s, Ok(ep))) = ask.map(|s| (s, s.endpoint(lang))) else {
            self.done.insert(key, Counted::Estimate(est));
            return Counted::Estimate(est);
        };
        if let Some(j) = self.job.take() {
            j.cancel.store(true, Ordering::Relaxed);
            self.done.remove(&j.key);
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let (t, c, m, body, rp) = (self.transport.clone(), cancel.clone(), model.to_string(), text.to_string(), repaint.clone());
        std::thread::spawn(move || {
            let _ = tx.send(llm::count_tokens(t.as_ref(), &ep, &m, &body, &c));
            rp();
        });
        self.job = Some(CountJob { key, server: (s.id.clone(), s.url.clone()), rx, cancel });
        self.done.insert(key, Counted::Pending(est));
        Counted::Pending(est)
    }
}

/// Zustand der Prompt-Schalter im Reiter „Modelle“ (nicht gespeichert).
#[derive(Default)]
pub(super) struct PromptUi {
    /// Globale Schalter (`Settings::chat_prompt_parts`; die App gleicht sie vor und nach dem Zeichnen ab).
    pub global: PromptParts,
    /// Modell der globalen Vorschau (Server-Kennung, Name); `None` = das erste bekannte.
    pub preview_for: Option<(String, String)>,
    pub counter: TokenCounter,
    /// Geplante Prompts (Schlüssel: Server, Einstellungen des Modells, Schalter, Modus).
    previews: HashMap<u64, PromptPlan>,
    /// Neuzeichnen, wenn der Server fertig gezählt hat (setzt die App).
    pub repaint: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl PromptUi {
    fn repaint(&self) -> Arc<dyn Fn() + Send + Sync> {
        self.repaint.clone().unwrap_or_else(|| Arc::new(|| {}))
    }

    /// Prompt, wie ihn ein Chat mit diesem Modell bekäme (zwischengespeichert, Rendern kostet).
    fn plan(&mut self, server: &LlmServer, model: &str, expert: bool) -> PromptPlan {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        serde_json::to_string(server).unwrap_or_default().hash(&mut h);
        (model, expert, serde_json::to_string(&self.global).unwrap_or_default()).hash(&mut h);
        let key = h.finish();
        if self.previews.len() > 64 {
            self.previews.clear();
        }
        let global = self.global;
        self.previews.entry(key).or_insert_with(|| plan_for(server, model, global, expert)).clone()
    }
}

/// Plant den Prompt genau wie eine Anfrage (`llm_backend::plan_prompt`: Teile, Familie, Kontextlänge, volle Fassung
/// oder Kurzfassung) – ohne Projekt und ohne Abgleich mit dem Server. Wie ein Chat mit Projektordner: die
/// Agenten-Werkzeuge (#338) mit ihren Standard-Einstellungen sind dabei (der größte übliche Fall).
pub(super) fn plan_for(server: &LlmServer, model: &str, global: PromptParts, expert: bool) -> PromptPlan {
    let agent = crate::chat::agent::AgentEnv { root: Some(std::path::PathBuf::from("project")), ..Default::default() };
    // Ohne Projekt: keine Grenze des Projekts (`TurnContext::project_cap` = Steuern) – dieselbe Regel wie bei einer Anfrage.
    let cx = crate::chat::TurnContext { expert, prompt_parts: global, agent: agent.clone(), ..crate::chat::TurnContext::default() };
    let policy = ToolPolicy::new(server.model_cfg(model), server.tools_supported(model), expert).with_agent(agent).with_project_cap(cx.project_cap);
    crate::chat::llm_backend::plan_prompt(server, model, &policy, &cx)
}

/// Was der Zähler zählt: Systemprompt und Werkzeug-Schemas der Anfrage.
fn counted_text(p: &PromptPlan) -> String {
    let tools = p.tools_json();
    if tools.is_empty() { p.system.clone() } else { format!("{}\n{tools}", p.system) }
}

/// Die acht Schalter für `parts` (Beispiele nur mit Werkzeug-Beschreibungen); Rückgabe: der umgeschaltete Teil.
fn part_checkboxes(ui: &mut Ui, lang: Lang, salt: &str, parts: PromptParts) -> Option<(PromptPart, bool)> {
    let mut changed = None;
    egui::Grid::new(("prompt_parts", salt)).num_columns(2).spacing([16.0, 2.0]).show(ui, |ui| {
        for (k, p) in PromptPart::ALL.into_iter().enumerate() {
            let mut on = parts.get(p);
            let enabled = p != PromptPart::Examples || parts.get(PromptPart::ToolDocs);
            let r = ui.add_enabled(enabled, egui::Checkbox::new(&mut on, p.label(lang))).on_hover_text(p.hint(lang));
            if r.changed() {
                changed = Some((p, on));
            }
            if k % 2 == 1 {
                ui.end_row();
            }
        }
    });
    let safety = changed.filter(|(p, _)| *p == PromptPart::Safety).map_or(parts.get(PromptPart::Safety), |(_, on)| on);
    if !safety {
        ui.label(RichText::new(format!("⚠ {}", prompt_parts::safety_off_warning(lang))).small().color(crate::style::ui().warning));
    }
    changed
}

/// Vorschau (scrollbar, Festbreitenschrift) mit Fassung und Kontextlänge, darunter der Token-Zähler.
fn preview_and_count(ui: &mut Ui, lang: Lang, salt: &str, plan: &PromptPlan, count: Counted) {
    let variant = match (plan.variant, lang) {
        (Variant::Full, Lang::De) => format!("volle Fassung, Kontext {}", plan.ctx_len),
        (Variant::Full, Lang::En) => format!("full version, context {}", plan.ctx_len),
        (Variant::Compact, Lang::De) => format!("Kurzfassung – die volle passt nicht in den Kontext {}", plan.ctx_len),
        (Variant::Compact, Lang::En) => format!("compact version – the full one does not fit into the context {}", plan.ctx_len),
    };
    egui::CollapsingHeader::new(RichText::new(format!("{} ({variant})", lang.t("Vorschau", "Preview"))).small()).id_salt(("prompt_preview", salt)).show(ui, |ui| {
        egui::Frame::new().fill(ui.visuals().extreme_bg_color).corner_radius(egui::CornerRadius::same(style::RADIUS)).inner_margin(egui::Margin::same(6)).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt(("prompt_preview_scroll", salt)).max_height(PREVIEW_H).auto_shrink([false, true]).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(&plan.system).monospace().size(11.5)).wrap());
            });
        });
    });
    let col = if matches!(count, Counted::Server(_)) { ui.visuals().text_color() } else { style::dim(ui) };
    let split = match lang {
        Lang::De => format!(" · Systemprompt ~{} + Werkzeug-Schemas ~{}", plan.sys_tokens, plan.tool_tokens),
        Lang::En => format!(" · system prompt ~{} + tool schemas ~{}", plan.sys_tokens, plan.tool_tokens),
    };
    ui.label(RichText::new(format!("{}{split}", count.text(lang))).small().color(col)).on_hover_text(lang.t(
        "Systemprompt samt Werkzeug-Schemas der Anfrage. Mit Tokenizer des Servers (llama.cpp, vLLM) gezählt, sonst Paneldecks Schätzung (~) – dieselbe, nach der Kontextlänge und Fassung gewählt werden.",
        "System prompt plus the request's tool schemas. Counted with the server's tokenizer (llama.cpp, vLLM), otherwise Paneldeck's estimate (~) – the same one that picks the context length and the version.",
    ));
}

/// Alle bekannten Modelle (Server, Name).
fn all_models(servers: &LlmServers) -> Vec<(&LlmServer, String)> {
    servers.iter().flat_map(|s| s.model_names().into_iter().map(move |n| (s, n))).collect()
}

/// Plan, Zähler und Anzeige für ein Modell.
fn show_plan(ui: &mut Ui, lang: Lang, salt: &str, server: &LlmServer, real_server: bool, model: &str, st: &mut PromptUi, expert: bool) {
    let plan = st.plan(server, model, expert);
    let rp = st.repaint();
    let count = st.counter.count(real_server.then_some(server), model, &counted_text(&plan), plan.sys_tokens + plan.tool_tokens, lang, &rp);
    preview_and_count(ui, lang, salt, &plan, count);
}

/// Abschnitt oben im Reiter „Modelle“: globale Schalter, Vorschau für ein wählbares Modell, Token-Zähler. `true`, wenn
/// sich die Schalter änderten.
pub(super) fn global_section(ui: &mut Ui, lang: Lang, servers: &LlmServers, st: &mut PromptUi, expert: bool) -> bool {
    let mut changed = false;
    egui::CollapsingHeader::new(RichText::new(lang.t("Anweisungen für Chat-Modelle (Master-Prompt)", "Instructions for chat models (master prompt)")).strong())
        .id_salt("prompt_global")
        .show(ui, |ui| {
            ui.label(RichText::new(lang.t(
                "Welche Teile der Anweisungen jedes Chat-Modell bekommt. Weniger Teile sparen Kontext; je Modell lässt sich das in seinen Einstellungen abweichend wählen.",
                "Which parts of the instructions every chat model gets. Fewer parts save context; each model can differ in its own settings.",
            )).small().color(style::dim(ui)));
            ui.add_space(S1);
            if let Some((p, on)) = part_checkboxes(ui, lang, "global", st.global) {
                st.global.set(p, on);
                changed = true;
            }
            // Vorschau für ein Modell (mit dessen Stufe, Werkzeugen und eigener Auswahl).
            let models = all_models(servers);
            let pick = st.preview_for.clone().and_then(|(sid, n)| models.iter().position(|(s, m)| s.id == sid && *m == n)).unwrap_or(0);
            let picked = models.get(pick).map(|(s, n)| ((*s).clone(), n.clone()));
            ui.horizontal(|ui| {
                ui.label(RichText::new(lang.t("Vorschau für", "Preview for")).small());
                let shown = match &picked {
                    Some((s, n)) => format!("{} · {n}", s.label()),
                    None => lang.t("Beispiel (Stufe Steuern)", "Example (level Control)").to_string(),
                };
                egui::ComboBox::from_id_salt("prompt_preview_for").selected_text(shown).show_ui(ui, |ui| {
                    for (s, n) in &models {
                        let sel = picked.as_ref().is_some_and(|(ps, pn)| ps.id == s.id && pn == n);
                        if ui.selectable_label(sel, format!("{} · {n}", s.label())).clicked() {
                            st.preview_for = Some((s.id.clone(), n.clone()));
                        }
                    }
                });
            });
            match picked {
                Some((s, n)) => show_plan(ui, lang, "global", &s, true, &n, st, expert),
                None => {
                    let mut s = LlmServer::default();
                    s.set_model_cfg(LlmModel { level: McpLevel::Control, level_chosen: true, ..LlmModel::named("example") });
                    show_plan(ui, lang, "global", &s, false, "example", st, expert);
                }
            }
        });
    changed
}

/// Abschnitt in den Einstellungen eines Modells: eigene Auswahl der Teile, Familienzusatz, Vorschau mit Zähler.
/// Ändert nur `cfg` (die App speichert).
pub(super) fn model_section(ui: &mut Ui, lang: Lang, server: &LlmServer, name: &str, cfg: &mut LlmModel, st: &mut PromptUi, expert: bool) {
    let salt = format!("{}/{name}", server.id);
    egui::CollapsingHeader::new(RichText::new(lang.t("Anweisungen (Master-Prompt)", "Instructions (master prompt)")).small()).id_salt(("prompt_model", salt.as_str())).show(ui, |ui| {
        let mut own = !cfg.prompt_parts.is_empty();
        if ui.checkbox(&mut own, lang.t("Eigene Auswahl für dieses Modell", "Own selection for this model")).on_hover_text(lang.t("Aus: es gelten die Anweisungen oben auf dieser Seite.", "Off: the instructions at the top of this page apply.")).changed() {
            cfg.prompt_parts = if own { PromptOverride::all_from(st.global) } else { PromptOverride::default() };
        }
        if own {
            if let Some((p, on)) = part_checkboxes(ui, lang, &salt, cfg.prompt_parts.resolve(st.global)) {
                cfg.prompt_parts.set(p, Some(on));
            }
        } else if !st.global.get(PromptPart::Safety) {
            ui.label(RichText::new(format!("⚠ {}", prompt_parts::safety_off_warning(lang))).small().color(crate::style::ui().warning));
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(lang.t("Modellfamilie", "Model family")).small());
            let detected = crate::chat::master_prompt::family(name).map(|f| f.key());
            let label_of = |k: &str| prompt_parts::FAMILIES.iter().find(|(f, _)| *f == k).map(|(_, l)| *l);
            let auto = match (detected.and_then(label_of), lang) {
                (Some(l), Lang::De) => format!("Automatisch (erkannt: {l})"),
                (Some(l), Lang::En) => format!("Automatic (detected: {l})"),
                (None, _) => lang.t("Automatisch (keine erkannt)", "Automatic (none detected)").to_string(),
            };
            let shown = cfg.prompt_family.as_deref().and_then(label_of).map_or(auto.clone(), str::to_string);
            egui::ComboBox::from_id_salt(("prompt_family", salt.as_str())).selected_text(shown).show_ui(ui, |ui| {
                if ui.selectable_label(cfg.prompt_family.is_none(), auto).clicked() {
                    cfg.prompt_family = None;
                }
                for (k, l) in prompt_parts::FAMILIES {
                    if ui.selectable_label(cfg.prompt_family.as_deref() == Some(*k), *l).clicked() {
                        cfg.prompt_family = Some(k.to_string());
                    }
                }
            });
        });
        // Vorschau mit den Einstellungen, wie sie gerade im Formular stehen.
        let mut s = server.clone();
        s.set_model_cfg(cfg.clone());
        show_plan(ui, lang, &salt, &s, true, name, st, expert);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::http::fake::{Canned, Fake};
    use crate::llm::http::Method;
    use std::time::{Duration, Instant};

    fn wait(c: &mut TokenCounter, s: Option<&LlmServer>, text: &str) -> Counted {
        let rp: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        let t = Instant::now();
        loop {
            let r = c.count(s, "m1", text, 100 + text.len(), Lang::En, &rp);
            if !matches!(r, Counted::Pending(_)) || t.elapsed() > Duration::from_secs(5) {
                return r;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn oa(id: &str) -> LlmServer {
        LlmServer { id: id.into(), kind: ServerKind::OpenAi, url: "http://127.0.0.1:8080/v1".into(), ..Default::default() }
    }

    #[test]
    fn counter_uses_the_server_tokenizer_and_falls_back_to_an_estimate() {
        let fake = Arc::new(Fake::default().with(Method::Post, "http://127.0.0.1:8080/tokenize", Canned::ok(r#"{"tokens":[1,2,3]}"#)));
        let mut c = TokenCounter::with_transport(fake.clone());
        let s = oa("a");
        assert!(matches!(c.count(Some(&s), "m1", "hello", 1, Lang::En, &(Arc::new(|| {}) as Arc<dyn Fn() + Send + Sync>)), Counted::Pending(_)));
        assert_eq!(wait(&mut c, Some(&s), "hello"), Counted::Server(3));
        assert_eq!(wait(&mut c, Some(&s), "hello"), Counted::Server(3), "behalten, nicht neu gefragt");
        assert_eq!(fake.log().len(), 1);
        // Ollama und ohne Server: nur Schätzung, keine Anfrage.
        let ol = LlmServer { id: "o".into(), kind: ServerKind::Ollama, url: "http://localhost:11434".into(), ..Default::default() };
        assert_eq!(wait(&mut c, Some(&ol), "abc"), Counted::Estimate(103));
        assert_eq!(wait(&mut c, None, "abc"), Counted::Estimate(103));
        assert_eq!(fake.log().len(), 1);
        // Kein Tokenizer (404): Schätzung, der Server wird danach nicht mehr gefragt.
        let none = Arc::new(Fake::default().with(Method::Post, "http://127.0.0.1:8080/tokenize", Canned::status(404, "")));
        let mut c = TokenCounter::with_transport(none.clone());
        assert_eq!(wait(&mut c, Some(&s), "one"), Counted::Estimate(103));
        assert_eq!(wait(&mut c, Some(&s), "two"), Counted::Estimate(103));
        assert_eq!(none.log().len(), 1);
        assert_eq!(Counted::Server(12).text(Lang::De), "12 Tokens (Tokenizer des Servers)");
        assert_eq!(Counted::Estimate(12).text(Lang::En), "~12 tokens (estimate)");
    }

    /// Ein Bild zeichnen: Texte und ihre Mittelpunkte.
    fn run(ctx: &egui::Context, t: f64, events: Vec<egui::Event>, f: &mut dyn FnMut(&mut Ui)) -> Vec<(String, egui::Pos2)> {
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(900.0, 1400.0))), time: Some(t), events, ..Default::default() };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| f(ui));
        });
        out.shapes.iter().filter_map(|s| if let egui::Shape::Text(t) = &s.shape { Some((t.galley.text().to_string(), t.visual_bounding_rect().center())) } else { None }).collect()
    }

    /// Auf einen Text klicken (drei Bilder: hin, drücken, loslassen) und danach ein Bild zeichnen.
    fn click(ctx: &egui::Context, t: &mut f64, texts: &[(String, egui::Pos2)], what: &str, f: &mut dyn FnMut(&mut Ui)) -> Vec<(String, egui::Pos2)> {
        let pos = texts.iter().find(|(s, _)| s == what).unwrap_or_else(|| panic!("{what}: {texts:?}")).1;
        let btn = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        for evs in [vec![egui::Event::PointerMoved(pos)], vec![btn(true)], vec![btn(false)], vec![]] {
            *t += 0.05;
            run(ctx, *t, evs, f);
        }
        *t += 0.5;
        run(ctx, *t, vec![], f)
    }

    #[test]
    fn global_switches_update_the_preview_count_and_warn_without_safety_rules() {
        let ctx = egui::Context::default();
        let mut st = PromptUi { counter: TokenCounter::with_transport(Arc::new(Fake::default())), ..PromptUi::default() };
        let servers = LlmServers::default();
        let mut changed = false;
        let mut draw = |ui: &mut Ui| changed |= global_section(ui, Lang::En, &servers, &mut st, false);
        let mut t = 0.0;
        let texts = run(&ctx, t, vec![], &mut draw);
        let texts = click(&ctx, &mut t, &texts, "Instructions for chat models (master prompt)", &mut draw);
        assert!(texts.iter().any(|(s, _)| s == "Safety rules"), "{texts:?}");
        assert!(texts.iter().any(|(s, _)| s == "Example (level Control)"), "{texts:?}");
        assert!(texts.iter().any(|(s, _)| s.starts_with('~') && s.contains(" tokens (estimate) · system prompt ~")), "Zähler ohne Server: geschätzt: {texts:?}");
        assert!(!texts.iter().any(|(s, _)| s.starts_with("⚠ Safety rules off")));
        let texts = click(&ctx, &mut t, &texts, "Safety rules", &mut draw);
        assert!(texts.iter().any(|(s, _)| s.starts_with("⚠ Safety rules off")), "{texts:?}");
        drop(texts);
        drop(draw);
        assert!(changed && !st.global.get(PromptPart::Safety));
    }
}
