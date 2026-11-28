//! Nur mit dem Cargo-Feature `readme-shots` (nie im Release): Startansichten für die README- und Doku-Screenshots
//! (`tools/docs-shots.ps1`). Die Umgebungsvariable `PANELDECK_SHOT=<ansicht>[:<argument>]` legt fest, was die Instanz nach
//! dem Start zeigt – Dialoge, Fenster und Beispieldaten öffnet bzw. erzeugt dieser Code selbst, ganz ohne Maus und Tastatur.
//! Aufgenommen wird mit dem MCP-Werkzeug `screenshot` (die Anwendung rendert sich selbst).
//!
//! * Ansichten der Todo-Liste ([`PLAN_VIEWS`]) stehen in `planning_ui/shot_view.rs`.
//! * Alle anderen hier ([`App::shot_setup`]); Beispieldaten (Ordner `acme-shop`, Transkripte, Prozesse, eine kaputte
//!   Theme-Datei) entstehen nur im Datenordner der Aufnahme bzw. daneben – die Aufnahme läuft immer in einer Kopie der
//!   .exe mit eigenem Datenordner und Schein-Profil.

use super::*;
use std::sync::atomic::{AtomicU32, Ordering};

/// Steht in jedem Screenshot-Build (`tools/release.sh` bricht ab, wenn es in der Release-.exe steht).
const SHOT_MARK: &str = "PANELDECK_README_SHOTS_BUILD";

/// Ansichten der Todo-Liste (`planning_ui::shot_view`).
pub(super) const PLAN_VIEWS: &[&str] =
    &["tickets", "board", "backlog", "releases", "dashboard", "overview", "search", "search-example", "maint-page", "maint-fix", "maint-duplicate", "maint-database"];

/// Bild, in dem die Ansicht aufgebaut wird (vorher sind Einstellungen, Datenbank und Sitzungen geladen).
const SETUP_FRAME: u32 = 3;

static FRAME: AtomicU32 = AtomicU32::new(0);

/// `PANELDECK_SHOT` als (Ansicht, Argument).
pub(super) fn spec() -> Option<(String, String)> {
    let s = std::env::var("PANELDECK_SHOT").ok()?;
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (v, a) = s.split_once(':').unwrap_or((s, ""));
    Some((v.to_string(), a.to_string()))
}

/// Gehört die Ansicht zur Todo-Liste?
pub(super) fn is_plan_view(view: &str) -> bool {
    PLAN_VIEWS.contains(&view)
}

/// Karte „Kurze Tour?“ zeigen (nur für die Ansicht `tutorial-card`; sonst nie im Screenshot-Build).
pub(super) fn wants_card() -> bool {
    spec().is_some_and(|(v, _)| v == "tutorial-card")
}

/// Abschnitt „Vorlagen“ der Seitenleiste eingeklappt (Bilder der Prozessliste: alle Prozesse sollen Platz haben).
pub(super) fn templates_closed() -> bool {
    spec().is_some_and(|(v, _)| v == "processes" || v == "process-output")
}

/// Beispielbild für die Chat-Bilder (#357): eine schlichte Bestellseite, deren Knopf die Summe überdeckt – hier erzeugt,
/// keine Datei aus dem Netz.
fn demo_screenshot() -> image::RgbaImage {
    let (w, h) = (1280u32, 800u32);
    let mut img = image::RgbaImage::from_pixel(w, h, image::Rgba([243, 244, 246, 255]));
    let mut rect = |x0: u32, y0: u32, x1: u32, y1: u32, c: [u8; 3]| {
        for y in y0..y1.min(h) {
            for x in x0..x1.min(w) {
                img.put_pixel(x, y, image::Rgba([c[0], c[1], c[2], 255]));
            }
        }
    };
    rect(0, 0, 1280, 72, [30, 41, 59]); // Kopfzeile
    rect(40, 22, 220, 50, [148, 163, 184]); // Logo
    rect(340, 130, 940, 690, [255, 255, 255]); // Karte
    for (i, y) in [180u32, 250, 320, 390].iter().enumerate() {
        rect(390, *y, 690 - i as u32 * 40, *y + 26, [203, 213, 225]); // Zeilen
        rect(780, *y, 890, *y + 26, [148, 163, 184]); // Preise
    }
    rect(390, 470, 890, 474, [226, 232, 240]); // Trennlinie
    rect(390, 500, 600, 540, [71, 85, 105]); // „Gesamt“
    rect(760, 496, 890, 544, [15, 23, 42]); // Summe
    rect(560, 510, 900, 580, [234, 88, 12]); // Knopf „Bezahlen“ (überdeckt die Summe)
    rect(640, 535, 820, 555, [255, 237, 213]);
    img
}

/// Ordner des Beispielprojekts neben dem Datenordner der Aufnahme.
fn demo_dir(data: &Path) -> PathBuf {
    data.parent().unwrap_or(data).join("acme-shop")
}

/// Beispielprojekt für Editor und Terminals anlegen (feste Inhalte; nur, wenn es fehlt).
fn write_demo_project(dir: &Path) {
    const FILES: &[(&str, &str)] = &[
        ("README.md", "# Acme Shop\n\nA small demo shop: orders, payments and a REST API.\n\n## Run\n\n```\ncargo run -- --port 8080\n```\n\n## Test\n\n```\ncargo test\n```\n"),
        ("Cargo.toml", "[package]\nname = \"acme-shop\"\nversion = \"0.4.0\"\nedition = \"2021\"\n\n[dependencies]\nserde = { version = \"1\", features = [\"derive\"] }\nserde_json = \"1\"\n"),
        (".gitignore", "/target\n*.log\n"),
        ("src/main.rs", "mod orders;\nmod payments;\n\nuse orders::{Order, OrderStore};\n\nfn main() {\n    let mut store = OrderStore::default();\n    let id = store.create_order(\"alice\", 4200);\n    println!(\"created order {id}\");\n    if let Some(order) = store.find_order(id) {\n        println!(\"total: {} cents\", order.total_cents);\n    }\n    payments::charge(&Order::sample(), \"card_4242\").expect(\"payment failed\");\n}\n"),
        ("src/orders.rs", "//! Orders: create, find and cancel.\n\nuse std::collections::BTreeMap;\n\n#[derive(Clone, Debug, PartialEq)]\npub struct Order {\n    pub id: u64,\n    pub customer: String,\n    pub total_cents: u64,\n    pub cancelled: bool,\n}\n\nimpl Order {\n    pub fn sample() -> Order {\n        Order { id: 1, customer: \"demo\".into(), total_cents: 1999, cancelled: false }\n    }\n}\n\n#[derive(Default)]\npub struct OrderStore {\n    next_id: u64,\n    orders: BTreeMap<u64, Order>,\n}\n\nimpl OrderStore {\n    pub fn create_order(&mut self, customer: &str, total_cents: u64) -> u64 {\n        self.next_id += 1;\n        let id = self.next_id;\n        self.orders.insert(id, Order { id, customer: customer.into(), total_cents, cancelled: false });\n        id\n    }\n\n    pub fn find_order(&self, id: u64) -> Option<&Order> {\n        self.orders.get(&id)\n    }\n\n    pub fn cancel_order(&mut self, id: u64) -> bool {\n        match self.orders.get_mut(&id) {\n            Some(order) if !order.cancelled => {\n                order.cancelled = true;\n                true\n            }\n            _ => false,\n        }\n    }\n}\n"),
        ("src/payments.rs", "//! Payments: charge an order with a saved card.\n\nuse crate::orders::Order;\n\n#[derive(Debug)]\npub enum PaymentError {\n    Declined,\n    Expired,\n}\n\npub fn charge(order: &Order, card: &str) -> Result<String, PaymentError> {\n    if card.ends_with(\"0000\") {\n        return Err(PaymentError::Declined);\n    }\n    Ok(format!(\"ch_{}_{}\", order.id, order.total_cents))\n}\n"),
        ("tests/orders.rs", "use acme_shop::orders::OrderStore;\n\n#[test]\nfn cancel_order_only_once() {\n    let mut store = OrderStore::default();\n    let id = store.create_order(\"bob\", 500);\n    assert!(store.cancel_order(id));\n    assert!(!store.cancel_order(id));\n}\n"),
        // Formatieren (#381): kompaktes JSON zum Formatieren und eines mit einem Fehler (fehlendes Komma).
        ("config/shop.json", "{\"name\":\"acme-shop\",\"port\":8080,\"currency\":\"EUR\",\"payments\":{\"provider\":\"demo\",\"retries\":3,\"cards\":[\"visa\",\"mastercard\"]},\"features\":{\"coupons\":true,\"gift_cards\":false}}\n"),
        ("config/orders.json", "{\n  \"orders\": [\n    { \"id\": 1, \"customer\": \"alice\", \"total_cents\": 4200 },\n    { \"id\": 2, \"customer\": \"bob\" \"total_cents\": 500 }\n  ]\n}\n"),
    ];
    for (name, text) in FILES {
        let p = dir.join(name);
        if !p.exists() {
            let _ = std::fs::create_dir_all(p.parent().unwrap_or(dir));
            let _ = std::fs::write(&p, text);
        }
    }
}

/// Beispiel-Transkript für die Kosten-Übersicht: `days` Tage bis heute, je Tag ein paar Antworten von `model`.
fn write_transcript(path: &Path, model: &str, scale: u64, days: u64) {
    let now = usage::today_secs();
    let mut out = String::new();
    for d in 0..days {
        let day = usage::utc_date(now.saturating_sub(d * 86_400));
        // feste, aber je Tag verschiedene Mengen (keine Zufallszahlen: gleiche Bilder bei jedem Lauf)
        let turns = 2 + (d * 7 + scale) % 5;
        for k in 0..turns {
            let f = 1 + (d * 3 + k * 5 + scale) % 9;
            let line = serde_json::json!({
                "type": "assistant",
                "timestamp": format!("{day}T{:02}:{:02}:00.000Z", 8 + k, (d * 11) % 60),
                "message": {
                    "id": format!("msg_{scale}_{d}_{k}"),
                    "model": model,
                    "stop_reason": "end_turn",
                    "usage": {
                        "input_tokens": 1_200 * f * scale,
                        "output_tokens": 900 * f * scale,
                        "cache_read_input_tokens": 18_000 * f * scale,
                        "cache_creation_input_tokens": 2_500 * f * scale,
                    }
                }
            });
            out.push_str(&line.to_string());
            out.push('\n');
        }
    }
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(path));
    let _ = std::fs::write(path, out);
}

impl App {
    /// Jedes Bild (vor dem Zeichnen): Ansicht der Todo-Liste vormerken bzw. die übrigen Ansichten aufbauen.
    pub(super) fn dev_shot_mode(&mut self, ctx: &egui::Context) {
        let Some((view, arg)) = spec() else { return };
        if is_plan_view(&view) {
            if planning_ui::shot_view::pending_spec().is_some() && self.mode != titlebar::Mode::Tasks {
                self.mode = titlebar::Mode::Tasks;
            }
            return;
        }
        let f = FRAME.fetch_add(1, Ordering::Relaxed);
        if f == SETUP_FRAME {
            eprintln!("{SHOT_MARK}: {view}");
            self.shot_setup(ctx, &view, &arg);
        } else if f > SETUP_FRAME {
            self.shot_keep(ctx, &view, f - SETUP_FRAME);
        }
        // weiter zeichnen, damit Schritte nach dem Aufbau (Bild für Bild) auch ohne Eingaben ankommen
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    /// Sitzung `id` umbenennen.
    fn shot_name(&mut self, id: u64, name: &str) {
        if let Some(e) = self.entries.get_mut(&id) {
            e.meta.name = name.to_string();
            e.meta.custom_name = true;
        }
    }

    /// In der PowerShell-Sitzung `id`: neutraler Prompt `PS C:\Dev\acme-shop>` (kein Pfad des Aufnahme-Rechners im
    /// Bild), Bildschirm leeren und optional `cmd` so ausführen, als wäre er am Prompt getippt worden.
    fn shot_run(&self, id: u64, demo: &Path, cmd: Option<&str>) {
        const PROMPT: &str = r"PS C:\Dev\acme-shop> ";
        let Some(s) = self.entries.get(&id).and_then(|e| e.live.as_ref()) else { return };
        let mut line = format!("function global:prompt {{ '{PROMPT}' }}; Set-Location -LiteralPath '{}'; Clear-Host", demo.display());
        if let Some(c) = cmd {
            line.push_str(&format!("; Write-Host '{PROMPT}{c}'; {c}"));
        }
        s.send_text_agent(&line, true);
    }

    /// Sitzungen des Hauptfensters im aktiven Projekt (die erste ist die beim Start angelegte).
    fn shot_sessions(&self) -> Vec<u64> {
        let mut v: Vec<u64> = self.entries.iter().filter(|(_, e)| e.meta.project == self.active_project && e.meta.window == 0 && !e.is_chat()).map(|(i, _)| *i).collect();
        v.sort_unstable();
        v
    }

    /// Projekt anlegen (nur in der Seitenleiste der Aufnahme).
    fn shot_project(&mut self, name: &str, folder: &Path) -> u64 {
        let id = self.alloc_project_id();
        self.projects.push(Project { id, name: name.to_string(), color: hub_color(id + 2), folder: folder.to_string_lossy().to_string(), ..Project::default() });
        id
    }

    /// Terminal Hub mit Projekten, drei geteilten Sitzungen und Claude-Status (`app`), Basis auch für `kosten`.
    fn shot_hub(&mut self, ctx: &egui::Context) {
        let demo = demo_dir(&self.dir);
        write_demo_project(&demo);
        self.set_mode(titlebar::Mode::Terminals);
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == self.active_project) {
            p.name = "Acme Shop".into();
            p.folder = demo.to_string_lossy().to_string();
        }
        // Der Demo-Datenordner hat keine gemerkten Sitzungen (der Hub startet leer): die erste selbst anlegen.
        let first = match self.shot_sessions().first().copied() {
            Some(id) => id,
            None => {
                let pid = self.active_project;
                self.new_session(ctx, NewOpts { project: Some(pid), ..NewOpts::default() })
            }
        };
        self.split_at(ctx, first, Dir::Horizontal, false);
        let second = self.shot_sessions().into_iter().find(|i| *i != first);
        if let Some(second) = second {
            self.split_at(ctx, second, Dir::Vertical, false);
        }
        let ids = self.shot_sessions();
        for (id, name) in ids.iter().zip(["api-server", "checkout-ui", "docs"]) {
            self.shot_name(*id, name);
        }
        // zwei weitere Projekte mit je einer Sitzung (nur in der Seitenleiste)
        let web = self.shot_project("Website", &demo);
        let n = self.new_session(ctx, NewOpts { project: Some(web), quiet: true, name: Some("landing-page".into()), ..NewOpts::default() });
        self.shot_name(n, "landing-page");
        let ops = self.shot_project("Ops", &demo);
        let n = self.new_session(ctx, NewOpts { project: Some(ops), quiet: true, name: Some("deploy".into()), ..NewOpts::default() });
        self.shot_name(n, "deploy");
        if let Some(f) = ids.first() {
            self.activate(*f);
        }
    }

    /// Claude-Status der Beispielsitzungen (jedes Bild neu: ohne echten Claude fiele er sonst zurück).
    /// Die wartende Sitzung zeigt in der Karte „Wartet auf dich“, was sie verlangt (#390) – wie von Claude gemeldet
    /// (die Meldung ist immer Englisch).
    fn shot_status(&mut self) {
        let ids = self.shot_sessions();
        let states = [(ClaudeStatus::Permission, "Claude needs your permission to use Bash"), (ClaudeStatus::Working, ""), (ClaudeStatus::Idle, "")];
        for (id, (st, msg)) in ids.iter().zip(states) {
            if let Some(e) = self.entries.get_mut(id) {
                e.launched_claude = true;
                e.status = st;
                e.status_msg = msg.to_string();
                let call = crate::hooks::ToolCall { seq: 1, id: None, detail: "cargo test --release".into() };
                e.track.request = (st == ClaudeStatus::Permission).then(|| crate::hooks::ToolRequest { tool: "Bash".into(), calls: vec![call], confirmed: true });
            }
        }
    }

    /// Beispiel-Transkripte an die Sitzungen hängen (Kosten-Übersicht).
    fn shot_usage(&mut self) {
        let base = self.dir.parent().unwrap_or(&self.dir).join("shot-transcripts");
        let models = [("claude-opus-4-1", 3), ("claude-sonnet-4-5", 4), ("claude-haiku-4-5", 2), ("claude-sonnet-4-5", 1), ("claude-opus-4-1", 1)];
        let mut ids: Vec<u64> = self.entries.iter().filter(|(_, e)| !e.is_chat()).map(|(i, _)| *i).collect();
        ids.sort_unstable();
        for (n, (id, (model, scale))) in ids.iter().zip(models).enumerate() {
            let p = base.join(format!("session-{n}.jsonl"));
            if !p.exists() {
                write_transcript(&p, model, scale, 14 - n as u64 * 2);
            }
            self.usage.set_path(*id, p);
        }
    }

    /// Chat mit einer Plan-Karte (#358): eingeschalteter Schalter *Plan* (Stufe „hoch“) neben *Senden* und ein
    /// vorgeschlagener Plan mit *Ausführen* / *Änderungen anfragen* / *Verwerfen* / *Exportieren*. Server und Modell
    /// stehen nur in den Einstellungen der Aufnahme; es wird nichts gesendet.
    fn shot_chat_plan(&mut self, ctx: &egui::Context) {
        use crate::chat::plan::{Plan, PlanLevel, PlanStatus};
        use crate::llm::config::{LlmModel, LlmServer, OLLAMA_LOCAL};
        const SERVER: &str = "shot-ollama";
        const MODEL: &str = "qwen3:14b";
        let demo = demo_dir(&self.dir);
        write_demo_project(&demo);
        self.set_mode(titlebar::Mode::Terminals);
        let pid = self.active_project;
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == pid) {
            p.name = "Acme Shop".into();
            p.folder = demo.to_string_lossy().to_string();
        }
        if !self.settings.llm_servers.list.iter().any(|s| s.id == SERVER) {
            let mut s = LlmServer {
                id: SERVER.into(),
                name: "Ollama".into(),
                url: OLLAMA_LOCAL.into(),
                last_models: vec![crate::llm::ModelInfo { name: MODEL.into(), tools: Some(true), context_length: Some(32768), vision: Some(false) }],
                ..LlmServer::default()
            };
            s.set_model_cfg(LlmModel { level: crate::mcp::McpLevel::Control, level_chosen: true, ..LlmModel::named(MODEL) });
            self.settings.llm_servers.list.push(s);
        }
        let lang = self.settings.language;
        let chat = state::ChatMeta { server: SERVER.into(), model: MODEL.into() };
        let id = self.new_session(ctx, NewOpts { project: Some(pid), cwd: Some(demo.to_string_lossy().to_string()), chat: Some(chat), ..NewOpts::default() });
        self.shot_name(id, lang.t("Bestellungen stornieren", "Cancel orders"));
        let question = lang.t(
            "Stornierte Bestellungen sollen eine Erstattung auslösen. Plane das bitte zuerst.",
            "Cancelled orders should trigger a refund. Please plan this first.",
        );
        let plan = lang.t(
            "# Plan: Erstattung beim Stornieren\n\n## Ziel\n`cancel_order` löst für bezahlte Bestellungen eine Erstattung aus; doppelte Stornos erstatten nichts.\n\n## Befunde\n- `src/orders.rs`: `cancel_order` setzt nur `cancelled = true`.\n- `src/payments.rs`: `charge` gibt eine Zahlungs-Kennung zurück, eine Erstattung gibt es noch nicht.\n\n## Schritte\n1. `src/payments.rs`: `refund(charge_id)` mit eigenem Fehlertyp ergänzen\n2. `src/orders.rs`: Zahlungs-Kennung in `Order` speichern\n3. `src/orders.rs`: `cancel_order` ruft `refund` nur beim ersten Storno auf\n4. `tests/orders.rs`: Tests für Erstattung und doppeltes Storno\n5. `cargo test` ausführen\n\n## Risiken\n- Bestellungen ohne Zahlung: keine Erstattung versuchen.\n\n## Tests\n- `cargo test`",
            "# Plan: Refund on cancel\n\n## Goal\n`cancel_order` triggers a refund for paid orders; cancelling twice refunds nothing.\n\n## Findings\n- `src/orders.rs`: `cancel_order` only sets `cancelled = true`.\n- `src/payments.rs`: `charge` returns a charge id; there is no refund yet.\n\n## Steps\n1. `src/payments.rs`: add `refund(charge_id)` with its own error type\n2. `src/orders.rs`: store the charge id in `Order`\n3. `src/orders.rs`: `cancel_order` calls `refund` only on the first cancel\n4. `tests/orders.rs`: tests for the refund and a double cancel\n5. Run `cargo test`\n\n## Risks\n- Orders without a payment: do not try a refund.\n\n## Tests\n- `cargo test`",
        );
        if let Some(c) = self.entries.get_mut(&id).and_then(|e| e.chat.as_mut()) {
            use crate::chat::{ChatMsg, Role};
            c.file.messages = vec![ChatMsg::new(Role::User, question), ChatMsg::new(Role::Assistant, plan)];
            c.file.plan = Some(Plan { level: PlanLevel::High, status: PlanStatus::Proposed, markdown: plan.trim().to_string(), revision: 1, created: crate::chat::now_secs(), ..Plan::default() });
            c.planning.on = true;
            c.planning.level = PlanLevel::High;
            c.focus_input = false;
            c.touch();
        }
        self.activate(id);
    }

    /// Chat nach dem Kompaktieren (#359): ein Beispielserver (ohne Verbindung), ein Chat mit zwei kompaktierten Runden,
    /// der Zusammenfassung (zugeklappt, „Original anzeigen“) und zwei behaltenen Runden – alles im Speicher.
    fn shot_chat_compacted(&mut self, ctx: &egui::Context) {
        use crate::chat::compact;
        use crate::chat::store::{ChatTodo, SummaryInfo, TodoStatus, ToolCall, TouchedFile};
        use crate::llm::config::{LlmModel, LlmServer};
        let lang = self.settings.language;
        let demo = demo_dir(&self.dir);
        write_demo_project(&demo);
        self.set_mode(titlebar::Mode::Terminals);
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == self.active_project) {
            p.name = "Acme Shop".into();
            p.folder = demo.to_string_lossy().to_string();
        }
        let model = "qwen3:8b";
        if !self.settings.llm_servers.iter().any(|s| s.id == "shot-ollama") {
            let mut s = LlmServer { id: "shot-ollama".into(), name: "Ollama".into(), kind: crate::llm::ServerKind::Ollama, url: crate::llm::config::OLLAMA_LOCAL.into(), ..LlmServer::default() };
            s.set_model_cfg(LlmModel { level: crate::mcp::McpLevel::Control, level_chosen: true, context_length: Some(32768), ..LlmModel::named(model) });
            self.settings.llm_servers.push(s);
        }
        let pid = self.active_project;
        let meta = state::ChatMeta { server: "shot-ollama".into(), model: model.into() };
        let id = self.new_session(ctx, NewOpts { project: Some(pid), chat: Some(meta), name: Some(lang.t("Bestellungen stornieren", "Cancel orders").into()), ..NewOpts::default() });
        let msg = |role: chatmod::Role, text: &str| chatmod::ChatMsg::new(role, text);
        let old = |role: chatmod::Role, text: &str| {
            let mut m = msg(role, text);
            m.compacted = true;
            m
        };
        let mut call = old(chatmod::Role::Assistant, "");
        call.tool_calls.push(ToolCall { id: "c1".into(), name: "read_file".into(), arguments: serde_json::json!({"path": "src/orders.rs"}) });
        let mut result = old(chatmod::Role::Tool, "{\"path\":\"src/orders.rs\",\"lines\":42}");
        result.tool_call_id = Some("c1".into());
        result.tool_name = Some("read_file".into());
        result.status = Some(chatmod::ToolStatus::Ok);
        let files = vec![TouchedFile { path: "src/orders.rs".into(), changed: true }, TouchedFile { path: "tests/orders.rs".into(), changed: false }];
        let todos = vec![
            ChatTodo { content: lang.t("Stornieren nur einmal erlauben", "Allow cancelling only once").into(), status: TodoStatus::Completed },
            ChatTodo { content: lang.t("Test für doppeltes Stornieren", "Test for cancelling twice").into(), status: TodoStatus::InProgress },
        ];
        let text = lang.t(
            "Du willst, dass eine Bestellung nur einmal storniert werden kann. `cancel_order` in `src/orders.rs` gibt jetzt `false` zurück, wenn sie schon storniert ist; `cargo test` läuft grün. Offen: ein Test für doppeltes Stornieren.",
            "You want an order to be cancellable only once. `cancel_order` in `src/orders.rs` now returns `false` when it is already cancelled; `cargo test` passes. Still open: a test for cancelling twice.",
        );
        let mut summary = msg(chatmod::Role::System, &compact::summary_content(text, &files, &todos, None));
        summary.summary = Some(SummaryInfo { messages: 6, files, text: text.into(), omitted: 0 });
        let messages = vec![
            old(chatmod::Role::User, lang.t("Wie storniere ich eine Bestellung?", "How do I cancel an order?")),
            call,
            result,
            old(chatmod::Role::Assistant, lang.t("Mit `OrderStore::cancel_order(id)`.", "With `OrderStore::cancel_order(id)`.")),
            old(chatmod::Role::User, lang.t("Sorge dafür, dass das nur einmal geht.", "Make sure that only works once.")),
            old(chatmod::Role::Assistant, lang.t("Erledigt – `cancel_order` prüft jetzt `cancelled`.", "Done – `cancel_order` now checks `cancelled`.")),
            summary,
            msg(chatmod::Role::User, lang.t("Gibt es schon einen Test dafür?", "Is there a test for it yet?")),
            msg(chatmod::Role::Assistant, lang.t("Ja: `cancel_order_only_once` in `tests/orders.rs`.", "Yes: `cancel_order_only_once` in `tests/orders.rs`.")),
            msg(chatmod::Role::User, lang.t("Gut, dann schreib die Doku dazu.", "Good, then write the docs for it.")),
            msg(chatmod::Role::Assistant, lang.t("Ich ergänze in der README einen Abschnitt „Bestellungen stornieren“.", "I'll add a section “Cancelling orders” to the README.")),
        ];
        if let Some(c) = self.entries.get_mut(&id).and_then(|e| e.chat.as_mut()) {
            c.file.messages = messages;
            c.file.todos = todos;
            c.focus_input = false;
            c.changed_from = 0;
            c.touch();
        }
        self.activate(id);
    }

    /// Chat mit Bildern (#357): ein Beispiel-Server mit einem Vision-Modell, eine eigene Nachricht mit `[Image#1]` (ein
    /// hier erzeugtes Beispielbild) und eine Antwort; das Popup öffnet `shot_keep`. Nur im Speicher und im Datenordner
    /// der Aufnahme.
    fn shot_chat_images(&mut self, ctx: &egui::Context) {
        let lang = self.settings.language;
        self.set_mode(titlebar::Mode::Terminals);
        if !self.settings.llm_servers.iter().any(|s| s.id == "shot-ollama") {
            let model = crate::llm::ModelInfo { name: "gemma3:4b".into(), tools: Some(true), context_length: Some(131_072), vision: Some(true) };
            self.settings.llm_servers.push(crate::llm::config::LlmServer { id: "shot-ollama".into(), name: "Ollama".into(), url: "http://localhost:11434".into(), last_models: vec![model], ..Default::default() });
        }
        let pid = self.active_project;
        let name = lang.t("Checkout-Fehler", "Checkout bug").to_string();
        let id = self.new_session(ctx, NewOpts { project: Some(pid), name: Some(name.clone()), chat: Some(state::ChatMeta { server: "shot-ollama".into(), model: "gemma3:4b".into() }), ..NewOpts::default() });
        self.shot_name(id, &name);
        let dir = self.chat_images_dir(id);
        let entry = chatmod::images::import_rgba(&dir, 1, demo_screenshot());
        if let Some(c) = self.entries.get_mut(&id).and_then(|e| e.chat.as_mut()) {
            let mut q = chatmod::ChatMsg::new(chatmod::Role::User, lang.t("[Image#1] Warum überdeckt der Knopf „Bezahlen“ den Gesamtbetrag?", "[Image#1] Why does the “Pay now” button cover the order total?"));
            q.images = vec![1];
            let a = chatmod::ChatMsg::new(
                chatmod::Role::Assistant,
                lang.t(
                    "Im Bild liegt der Knopf **Bezahlen** über der Zeile *Gesamt*: Der Knopf ist absolut positioniert, die Karte darüber hat aber keine feste Höhe.\n\nAm einfachsten: den Knopf in den normalen Fluss der Karte nehmen (`position: static`) und darüber `margin-top: 16px` geben.",
                    "In the image the **Pay now** button sits on top of the *Total* row: the button is absolutely positioned, but the card above it has no fixed height.\n\nThe simplest fix: put the button back into the card's normal flow (`position: static`) and give it `margin-top: 16px`.",
                ),
            );
            c.file.messages = vec![q, a];
            if let Ok(e) = entry {
                c.file.images = vec![e];
            }
            c.img.dir = Some(dir);
            c.touch();
        }
        self.activate(id);
    }

    /// Umgebungsvariablen einer Sitzung (#384): Sitzung im Beispielprojekt mit normalen, einer geheimen, einer
    /// vorangestellten und einer entfernten Variable; die Einstellungen der Sitzung öffnen auf dem Reiter „Umgebung“.
    fn shot_session_env(&mut self, ctx: &egui::Context) {
        use crate::session_env::{EnvVar, Mode};
        let demo = demo_dir(&self.dir);
        write_demo_project(&demo);
        self.set_mode(titlebar::Mode::Terminals);
        let pid = self.active_project;
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == pid) {
            p.name = "Acme Shop".into();
            p.folder = demo.to_string_lossy().to_string();
        }
        let mut env = vec![
            EnvVar::plain("API_URL", "http://localhost:3000"),
            EnvVar::plain("NODE_ENV", "development"),
            EnvVar { mode: Mode::Prepend, ..EnvVar::plain("PATH", r"C:\Dev\acme-shop\tools") },
            EnvVar { remove: true, ..EnvVar::plain("HTTP_PROXY", "") },
        ];
        // Beispielwert, verschlüsselt im Datenordner der Aufnahme (geht das nicht, fehlt die Zeile).
        if let Ok(s) = EnvVar::secret("GITHUB_TOKEN", "ghp_example_only") {
            env.insert(2, s);
        }
        let id = self.new_session(ctx, NewOpts { project: Some(pid), cwd: Some(demo.to_string_lossy().to_string()), name: Some("api-server".into()), env, ..NewOpts::default() });
        self.shot_name(id, "api-server");
        self.activate(id);
        self.open_session_settings(id);
        if let Some(f) = self.session_settings.as_mut() {
            f.start_tab = Some(super::opts_tabs::Tab::Environment);
            // neutraler Ordner statt des Aufnahme-Ordners (nur im Formular, gespeichert wird nichts)
            f.v.cwd = if cfg!(windows) { r"C:\Dev\acme-shop".into() } else { "/home/dev/acme-shop".into() };
        }
    }

    /// Ansicht aufbauen (einmal).
    fn shot_setup(&mut self, ctx: &egui::Context, view: &str, arg: &str) {
        let demo = demo_dir(&self.dir);
        if matches!(view, "processes" | "process-output" | "process-new") && self.shot_sessions().is_empty() {
            // eine Sitzung im Beispielordner, damit der Hauptbereich ein Terminal zeigt (der Demo-Hub startet leer)
            write_demo_project(&demo);
            let pid = self.active_project;
            if let Some(p) = self.projects.iter_mut().find(|p| p.id == pid) {
                p.name = "Acme Shop".into();
                p.folder = demo.to_string_lossy().to_string();
            }
            let id = self.new_session(ctx, NewOpts { project: Some(pid), ..NewOpts::default() });
            self.shot_name(id, "api-server");
        }
        match view {
            // ---- README
            "app" => self.shot_hub(ctx),
            "kosten" => {
                self.shot_hub(ctx);
                self.settings.daily_budget = 25.0;
                self.shot_usage();
                self.cost_open = true;
            }
            "agenten" => {
                self.settings_open = true;
                self.settings_tab = 7;
            }
            // ---- Prozesse
            "processes" | "process-output" => {
                let pid = self.active_project;
                let crashed = self.shot_procs_demo(ctx, pid);
                if view == "process-output" {
                    self.procs.view = Some(crashed);
                }
            }
            "process-new" => {
                let pid = self.active_project;
                self.shot_procs_demo(ctx, pid);
                self.shot_proc_editor(pid);
            }
            // ---- Kalender (#164): Beispieltermine rund um heute
            "calendar-month" | "calendar-week" | "calendar-dialog" => {
                let zone = crate::calendar::zone::SystemZone;
                let env = calendar_ui::CalEnv { conn: self.db.as_ref().map(|d| &d.conn), lang: self.settings.language, now: chrono::Local::now().naive_local(), zone: &zone };
                if let Some(c) = env.conn {
                    calendar_ui::demo_seed(c, env.now.date(), env.lang);
                }
                self.cal.demo_prepare(&env, view.trim_start_matches("calendar-"));
                // im Unterreiter „Kalender“ des Zeit-Reiters
                self.set_mode(titlebar::Mode::Time);
                self.time.sub = time_ui::Sub::Calendar;
            }
            // ---- Aufräumen
            "cleanup" => self.open_cleanup(ctx),
            "cleanup-confirm" => self.shot_cleanup_confirm(ctx),
            "cleanup-result" => self.shot_cleanup_result(),
            // ---- Editor
            "editor" | "editor-find" | "editor-disk-changed" | "editor-search" | "editor-syntax" => {
                write_demo_project(&demo);
                self.set_mode(titlebar::Mode::Editor);
                self.ed.set_root(&demo);
                for f in ["README.md", "src/orders.rs"] {
                    let _ = self.editor_open(&demo.join(f), true, false);
                }
                if view == "editor" {
                    self.ed.open_path(&demo.join("src/main.rs"), true);
                }
                if view == "editor-syntax" {
                    // Syntax-Modus (#312): Rust links, Markdown rechts, beide mit Farben.
                    self.ed.open_path(&demo.join("src/main.rs"), true);
                    let ids: Vec<u64> = self.ed.docs.keys().copied().collect();
                    for id in ids {
                        self.ed.set_doc_mode(id, Some(crate::editor::syntax::Mode::Syntax));
                    }
                }
                if view == "editor-search" {
                    self.editor_svc.search_open = true;
                    self.editor_svc.search.pattern = "order".into();
                    let q = self.editor_svc.search.query(&self.settings.editor);
                    let repaint = ctx.clone();
                    self.editor_svc.find.start(demo.clone(), q, move || repaint.request_repaint());
                }
            }
            // Menüleiste und Formatieren (#381): links das formatierte JSON, rechts ein Fehler mit „Gehe zu“.
            "editor-format" => {
                write_demo_project(&demo);
                self.set_mode(titlebar::Mode::Editor);
                self.ed.set_root(&demo);
                let _ = self.editor_open(&demo.join("config/shop.json"), true, false);
                self.ed.open_path(&demo.join("config/orders.json"), true);
            }
            // Aktionen beim Speichern (#383): Regel-Dialog über dem Editor.
            "editor-on-save" => {
                write_demo_project(&demo);
                self.set_mode(titlebar::Mode::Editor);
                self.ed.set_root(&demo);
                let _ = self.editor_open(&demo.join("src/orders.rs"), true, false);
                self.shot_save_actions_demo(&demo);
            }
            // Vergleich und Änderungsmarken (#385): ungespeicherte Änderungen in orders.rs gegen die gespeicherte Datei.
            "editor-diff" | "editor-gutter" => {
                write_demo_project(&demo);
                self.set_mode(titlebar::Mode::Editor);
                self.ed.set_root(&demo);
                if view == "editor-gutter" {
                    self.settings.editor.change_markers = true;
                    self.settings.editor.marker_base = crate::editor_svc::settings::MarkerBase::Disk;
                }
                let p = demo.join("src/orders.rs");
                if let Ok(id) = self.editor_open(&p, true, false) {
                    self.editor_shot_edits(id);
                    if view == "editor-diff" {
                        let prefs = self.editor_prefs();
                        let lang = self.settings.language;
                        let _ = self.ed.compare(id, super::editor_ui::shot_target_disk(), &prefs, lang);
                    }
                }
            }
            "editor-settings" => {
                self.set_mode(titlebar::Mode::Editor);
                self.settings_open = true;
                self.settings_tab = editor_services::SETTINGS_TAB;
            }
            // ---- Reiter „Zeit“ (#164): Beispieldaten in der Datenbank der Aufnahme
            "time-alarms" | "time-timers" | "time-clocks" => {
                self.set_mode(titlebar::Mode::Time);
                let lang = self.settings.language;
                if let Some(db) = self.db.as_ref() {
                    self.time.shot_demo(&db.conn, view, lang);
                }
            }
            // ---- Themes
            "theme-appearance" => {
                let bad = self.themes_dir().join("ocean-night.json");
                let _ = std::fs::create_dir_all(self.themes_dir());
                let _ = std::fs::write(&bad, "{\n  \"name\": \"Ocean night\",\n  \"colors\": { \"bg\": \"#0b1d2a\", \"fg\": \"#d8e6f0\", \"accent\": \"#3fa7ff\"\n");
                self.reload_themes();
                self.settings_open = true;
                self.settings_tab = 1;
            }
            "theme-generate" => {
                self.settings_open = true;
                self.settings_tab = 1;
                self.theme_new();
            }
            "theme-editor" => self.shot_theme_editor("accent", false),
            "theme-color" => self.shot_theme_editor("selection", false),
            "theme-checks" => self.shot_theme_editor("ansi8", true),
            // ---- Chats
            "chat-plan" => self.shot_chat_plan(ctx),
            "chat-compacted" => self.shot_chat_compacted(ctx),
            // ---- Tutorial
            "chat-images" => self.shot_chat_images(ctx),
            "tutorial-card" => self.set_mode(titlebar::Mode::Terminals),
            "tutorial-tour" => {
                let (tour, step) = arg.split_once('/').unwrap_or(("basics", "basics.sidebar"));
                self.tut_shot(ctx, tour, step);
            }
            // ---- Chats (#355, #356): Verlauf nur im Speicher, kein Server nötig
            "chat-background-command" | "chat-ask-form" => self.shot_chat(ctx, view == "chat-ask-form"),
            // ---- Umgebungsvariablen (#384)
            "session-env" => self.shot_session_env(ctx),
            _ => eprintln!("{SHOT_MARK}: unknown view {view}"),
        }
    }

    /// Chat mit lokalem Modell (#355/#356): eingerichteter Beispiel-Server (nur in den Einstellungen der Aufnahme), ein Chat
    /// im Beispielprojekt mit festem Verlauf. `ask`: offenes `ask_user`-Formular mit Empfehlung; sonst ein beendeter
    /// Hintergrund-Befehl samt Nachricht von Paneldeck und seinem Terminal rechts daneben.
    fn shot_chat(&mut self, ctx: &egui::Context, ask: bool) {
        use crate::chat::store::{ChatEvent, ChatMsg, Role, ToolCall, ToolStatus};
        let lang = self.settings.language;
        let demo = demo_dir(&self.dir);
        write_demo_project(&demo);
        self.set_mode(titlebar::Mode::Terminals);
        let pid = self.active_project;
        if let Some(p) = self.projects.iter_mut().find(|p| p.id == pid) {
            p.name = "Acme Shop".into();
            p.folder = demo.to_string_lossy().to_string();
        }
        let model = "qwen3:8b";
        let server = crate::llm::config::LlmServer {
            id: "shot-ollama".into(),
            name: "Ollama".into(),
            url: "http://localhost:11434".into(),
            models: vec![crate::llm::config::LlmModel { level: crate::mcp::McpLevel::Control, level_chosen: true, ..crate::llm::config::LlmModel::named(model) }],
            last_models: vec![crate::llm::ModelInfo { name: model.into(), tools: Some(true), context_length: Some(40960), vision: None }],
            ..Default::default()
        };
        self.settings.llm_servers.list.retain(|s| s.id != server.id);
        self.settings.llm_servers.list.push(server);
        let meta = crate::state::ChatMeta { server: "shot-ollama".into(), model: model.into() };
        let name = if ask { lang.t("Test-Setup", "Test setup") } else { lang.t("Build-Helfer", "Build helper") };
        let chat = self.new_session(ctx, NewOpts { project: Some(pid), cwd: Some(demo.to_string_lossy().to_string()), name: Some(name.into()), chat: Some(meta), ..NewOpts::default() });
        let call = |id: &str, tool: &str, args: serde_json::Value| ToolCall { id: id.into(), name: tool.into(), arguments: args };
        let answer = |text: &str, calls: Vec<ToolCall>| {
            let mut m = ChatMsg::new(Role::Assistant, text);
            m.tool_calls = calls;
            m
        };
        let result = |c: &ToolCall, content: serde_json::Value| crate::chat::logic::tool_result(c, ToolStatus::Ok, content.to_string());
        let mut msgs = Vec::new();
        if ask {
            msgs.push(ChatMsg::new(Role::User, lang.t("Richte Tests für den Shop ein.", "Set up tests for the shop.")));
            let q = call(
                "call_1_0",
                "ask_user",
                serde_json::json!({"questions": [
                    {"question": lang.t("Welches Test-Framework soll ich einrichten?", "Which test framework should I set up?"), "header": "Tests", "options": [
                        {"label": lang.t("Eingebaute Rust-Tests", "Built-in Rust tests"), "description": lang.t("cargo test, keine neue Abhängigkeit", "cargo test, no new dependency"), "recommended": true},
                        {"label": "nextest", "description": lang.t("schneller, braucht cargo-nextest", "faster, needs cargo-nextest")}
                    ]},
                    {"question": lang.t("Was soll zusätzlich geprüft werden?", "What should be checked as well?"), "multi_select": true, "options": [
                        {"label": "clippy", "recommended": true}, {"label": "rustfmt"}, {"label": lang.t("Abdeckung", "Coverage")}
                    ]}
                ]}),
            );
            msgs.push(answer(lang.t("Bevor ich anfange, zwei kurze Fragen:", "Before I start, two quick questions:"), vec![q.clone()]));
            if let Some(c) = self.entries.get_mut(&chat).and_then(|e| e.chat.as_mut()) {
                c.file.messages = msgs;
                if let Ok((qs, fixes)) = crate::chat::ask::parse(&q.arguments) {
                    c.shot_ask(q, crate::chat::ask::AskForm::new("call_1_0", qs, fixes, None));
                    // Vorauswahl wie von Hand: die empfohlene Antwort der ersten Frage
                    if let Some(f) = c.ask.as_mut() {
                        f.toggle(0, 0);
                    }
                }
                c.touch();
            }
            self.activate(chat);
            return;
        }
        msgs.push(ChatMsg::new(Role::User, lang.t("Baue den Shop im Release-Modus, im Hintergrund.", "Build the shop in release mode, in the background.")));
        let rc = call("call_1_0", "run_command", serde_json::json!({"command": "cargo build --release", "background": true}));
        msgs.push(answer("", vec![rc.clone()]));
        msgs.push(result(&rc, serde_json::json!({"id": "bg1", "status": "running", "background": true, "shell": "PowerShell 7"})));
        msgs.push(answer(lang.t("Der Build läuft im Hintergrund als bg1. Ich melde mich, wenn er fertig ist.", "The build runs in the background as bg1. I will report back when it is done."), vec![]));
        let ev = ChatEvent { kind: ChatEvent::BACKGROUND_COMMAND.into(), id: "bg1".into(), command: "cargo build --release".into(), state: "exited".into(), exit_code: Some(0), duration_ms: 74_000, error: None };
        let mut m = ChatMsg::new(Role::System, crate::chat::bg_commands::event_text(&ev, 3, None));
        m.event = Some(ev);
        msgs.push(m);
        let out = call("call_5_0", "command_output", serde_json::json!({"id": "bg1", "tail": 3}));
        msgs.push(answer("", vec![out.clone()]));
        msgs.push(result(&out, serde_json::json!({"id": "bg1", "status": "exited", "finished": true, "text": "   Compiling acme-shop v0.1.0\n    Finished `release` profile [optimized] target(s) in 1m 14s"})));
        msgs.push(answer(lang.t("Der Release-Build ist fertig (Exit-Code 0, 1 min 14 s). Die Programmdatei liegt in `target/release/acme-shop`.", "The release build finished (exit code 0, 1 min 14 s). The binary is in `target/release/acme-shop`."), vec![]));
        if let Some(c) = self.entries.get_mut(&chat).and_then(|e| e.chat.as_mut()) {
            c.file.messages = msgs;
            c.touch();
        }
        // Terminal des Befehls unter dem Chat (Ordner mit dem Namen des Chats, wie bei Kindern eines Agenten)
        let chat_name = self.entries.get(&chat).map(|e| e.display_name()).unwrap_or_default();
        let group = Some(chat_name.clone());
        let term = self.new_session(ctx, NewOpts { project: Some(pid), cwd: Some(demo.to_string_lossy().to_string()), quiet: true, group, ..NewOpts::default() });
        self.shot_name(term, &format!("{chat_name} – bg1: cargo build --release"));
        // Neutraler Prompt wie in `shot_run`, dazu die Ausgabe des Builds (ohne echtes cargo auf dem Aufnahme-Rechner).
        if let Some(s) = self.entries.get(&term).and_then(|e| e.live.as_ref()) {
            let line = format!(
                "function global:prompt {{ 'PS C:\\Dev\\acme-shop> ' }}; Set-Location -LiteralPath '{}'; Clear-Host; Write-Host 'PS C:\\Dev\\acme-shop> cargo build --release'; Write-Host '   Compiling acme-shop v0.1.0'; Write-Host '    Finished release profile [optimized] target(s) in 1m 14s'",
                demo.display()
            );
            s.send_text_agent(&line, true);
        }
        self.activate(chat);
        self.place(term, crate::layout::Placement::SplitRight, None, &[]);
        self.activate(chat);
    }

    /// Schritte nach dem Aufbau (`n` = Bilder seit dem Aufbau, ca. 10 je Sekunde).
    fn shot_keep(&mut self, ctx: &egui::Context, view: &str, n: u32) {
        let demo = demo_dir(&self.dir);
        match view {
            "app" | "kosten" => {
                self.shot_status();
                if n == 40 {
                    // echte Ausgaben im Beispielordner (die Sitzungen starten dort, Set-Location nur zur Sicherheit)
                    let cmds = ["Get-ChildItem -Recurse -File -Name", "Get-Content README.md", "Get-Content Cargo.toml"];
                    for (id, cmd) in self.shot_sessions().iter().zip(cmds) {
                        self.shot_run(*id, &demo, (view == "app").then_some(cmd));
                    }
                }
                if view == "kosten" && n % 20 == 0 {
                    self.shot_usage();
                }
            }
            // neutraler Prompt hinter dem Dialog (kein Pfad des Aufnahme-Rechners)
            "session-env" if n == 30 => {
                if let Some(id) = self.shot_sessions().first().copied() {
                    self.shot_run(id, &demo, None);
                }
            }
            "processes" | "process-output" | "process-new" if n == 40 => {
                if let Some(id) = self.shot_sessions().first().copied() {
                    self.shot_run(id, &demo, Some("Get-ChildItem -Name"));
                }
            }
            // Bild-Popup über dem Chat (#357)
            "chat-images" if n == 10 => {
                if let Some(id) = self.entries.iter().find(|(_, e)| e.is_chat()).map(|(i, _)| *i) {
                    chat_images_ui::open(ctx, id, 1);
                }
            }
            "editor-find" if n == 10 => {
                self.editor_find_bar(true);
                let f = &mut self.ed.find;
                f.query = r"fn (\w+)_order".into();
                f.regex = true;
                f.replacement = "fn ${1}_purchase".into();
                f.replace_mode = true;
            }
            "editor-format" if n == 10 => {
                let groups: Vec<(u64, u64)> = self.ed.panes.groups.iter().filter_map(|(g, grp)| grp.active_tab().map(|t| (*g, t.doc))).collect();
                let lang = self.settings.language;
                for (g, doc) in groups {
                    let validate = self.ed.docs.get(&doc).and_then(|d| d.path.as_ref()).is_some_and(|p| p.ends_with("orders.json"));
                    let op = if validate { crate::editor::format::Op::Validate } else { crate::editor::format::Op::Format };
                    let c = ctx.clone();
                    self.ed.start_format(g, op, false, move || c.request_repaint(), lang);
                }
            }
            "editor-gutter" if n == 15 => {
                let p = demo.join("src/orders.rs");
                if let Some(id) = self.ed.doc_for_path(&p) {
                    self.editor_shot_popup(id, 26);
                }
            }
            "editor-disk-changed" if n == 15 => {
                // eigene, ungespeicherte Änderung im Puffer und eine fremde auf der Platte
                let p = demo.join("src/orders.rs");
                if let Some(id) = self.ed.doc_for_path(&p) {
                    let _ = self.editor_apply_agent_edit_bytes(id, vec![(0..0, "// TODO: validate totals before saving\n".to_string())]);
                }
                if let Ok(t) = std::fs::read_to_string(&p) {
                    let _ = std::fs::write(&p, format!("{t}\n// changed by another program\n"));
                }
                self.editor_disk_changed(&p);
            }
            _ => {}
        }
        let _ = ctx;
    }
}
