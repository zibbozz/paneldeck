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
        let id = self.projects.iter().map(|p| p.id).max().unwrap_or(0) + 1;
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
    fn shot_status(&mut self) {
        let lang = self.settings.language;
        let ids = self.shot_sessions();
        let states = [
            (ClaudeStatus::Permission, lang.t("Bash: cargo test --release", "Bash: cargo test --release")),
            (ClaudeStatus::Working, ""),
            (ClaudeStatus::Idle, ""),
        ];
        for (id, (st, msg)) in ids.iter().zip(states) {
            if let Some(e) = self.entries.get_mut(id) {
                e.launched_claude = true;
                e.status = st;
                e.status_msg = msg.to_string();
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
            // ---- Aufräumen
            "cleanup" => self.open_cleanup(ctx),
            "cleanup-confirm" => self.shot_cleanup_confirm(ctx),
            "cleanup-result" => self.shot_cleanup_result(),
            // ---- Editor
            "editor" | "editor-find" | "editor-disk-changed" | "editor-search" => {
                write_demo_project(&demo);
                self.set_mode(titlebar::Mode::Editor);
                self.ed.set_root(&demo);
                for f in ["README.md", "src/orders.rs"] {
                    let _ = self.editor_open(&demo.join(f), true, false);
                }
                if view == "editor" {
                    self.ed.open_path(&demo.join("src/main.rs"), true);
                }
                if view == "editor-search" {
                    self.editor_svc.search_open = true;
                    self.editor_svc.search.pattern = "order".into();
                    let q = self.editor_svc.search.query(&self.settings.editor);
                    let repaint = ctx.clone();
                    self.editor_svc.find.start(demo.clone(), q, move || repaint.request_repaint());
                }
            }
            "editor-settings" => {
                self.set_mode(titlebar::Mode::Editor);
                self.settings_open = true;
                self.settings_tab = editor_services::SETTINGS_TAB;
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
            // ---- Tutorial
            "tutorial-card" => self.set_mode(titlebar::Mode::Terminals),
            "tutorial-tour" => {
                let (tour, step) = arg.split_once('/').unwrap_or(("basics", "basics.sidebar"));
                self.tut_shot(ctx, tour, step);
            }
            _ => eprintln!("{SHOT_MARK}: unknown view {view}"),
        }
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
            "processes" | "process-output" | "process-new" if n == 40 => {
                if let Some(id) = self.shot_sessions().first().copied() {
                    self.shot_run(id, &demo, Some("Get-ChildItem -Name"));
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
