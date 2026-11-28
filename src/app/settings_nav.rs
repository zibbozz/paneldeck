//! Einstellungsfenster (#395): Seiten mit festen Kennungen, Kategorienbaum, Suche und Tastatur – ohne App testbar.
//!
//! Die Kennungen der Seiten (`agents.access`, `editor.on_save` …) sind stabil: Sprünge aus der Oberfläche (Fensterleiste,
//! MCP-Chip, Dialoge), das Tutorial, die Befehlspalette und die Doku-Bilder nennen Seiten nur über sie.

use super::tutorial::{Spot, TutMark};
use crate::state::Lang;
use crate::style;
use eframe::egui::{self, Color32, FontId, Id, Pos2, Sense, Stroke, Ui, Vec2};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ops::Range;

macro_rules! groups {
    ($($g:ident = $de:literal, $en:literal;)*) => {
        /// Bereich des Baums (erste Ebene).
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub(super) enum Group { $($g),* }
        impl Group {
            pub(super) const ALL: &'static [Group] = &[$(Group::$g),*];
            pub(super) fn names(self) -> (&'static str, &'static str) {
                match self { $(Group::$g => ($de, $en)),* }
            }
        }
    };
}

groups! {
    General = "Allgemein", "General";
    Appearance = "Darstellung", "Appearance";
    Terminal = "Terminal", "Terminal";
    Editor = "Editor", "Editor";
    Shortcuts = "Tastenkürzel", "Shortcuts";
    Claude = "Claude", "Claude";
    Models = "Modelle", "Models";
    Agents = "Agenten", "Agents";
    Data = "Daten & Sicherung", "Data & backup";
    About = "Info", "About";
}

impl Group {
    pub(super) fn title(self, lang: Lang) -> &'static str {
        let (de, en) = self.names();
        lang.t(de, en)
    }

    /// Seiten des Bereichs in Baum-Reihenfolge.
    pub(super) fn pages(self) -> impl Iterator<Item = Page> {
        Page::ALL.iter().copied().filter(move |p| p.group() == self)
    }

    /// Bereich mit nur einer Seite (Tastenkürzel): im Baum ein Blatt ohne Pfeil.
    pub(super) fn single(self) -> bool {
        self.pages().count() == 1
    }
}

macro_rules! pages {
    ($($p:ident = $id:literal, $g:ident, $de:expr, $en:expr, [$($kw:literal),*];)*) => {
        /// Seite der Einstellungen (zweite Ebene) mit fester Kennung.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub(super) enum Page { $($p),* }
        impl Page {
            pub(super) const ALL: &'static [Page] = &[$(Page::$p),*];
            /// Stabile Kennung (`bereich.seite`), z. B. für Sprünge, die Befehlspalette und die Doku-Bilder.
            pub(super) fn id(self) -> &'static str {
                match self { $(Page::$p => $id),* }
            }
            pub(super) fn group(self) -> Group {
                match self { $(Page::$p => Group::$g),* }
            }
            pub(super) fn names(self) -> (&'static str, &'static str) {
                match self { $(Page::$p => ($de, $en)),* }
            }
            /// Gepflegte Stichwörter der Seite (beide Sprachen, auch alte Reiternamen).
            pub(super) fn keywords(self) -> &'static [&'static str] {
                match self { $(Page::$p => &[$($kw),*]),* }
            }
        }
    };
}

pages! {
    GeneralStart = "general.start", General, "Sprache & Start", "Language & startup", ["allgemein", "general", "sprache", "language"];
    GeneralNotifications = "general.notifications", General, "Benachrichtigungen", "Notifications", ["toast", "hinweis", "notice"];
    GeneralWindow = "general.window", General, "Fenster & Tray", "Window & tray", ["fenster", "window", "tray", "infobereich"];
    GeneralIntegration = "general.integration", General,
        if cfg!(windows) { "Windows-Integration" } else { "Desktop-Integration" },
        if cfg!(windows) { "Windows integration" } else { "Desktop integration" },
        ["fenster", "window", "explorer", "autostart", "kontextmenü", "context menu"];
    AppearanceTheme = "appearance.theme", Appearance, "Theme", "Theme", ["darstellung", "appearance", "farben", "colors"];
    AppearanceFonts = "appearance.fonts", Appearance, "Schrift & Skalierung", "Fonts & scaling", ["darstellung", "appearance", "font", "zoom"];
    AppearanceTransparency = "appearance.transparency", Appearance, "Transparenz & Animation", "Transparency & animation", ["darstellung", "appearance"];
    AppearancePalettes = "appearance.palettes", Appearance, "Farbpaletten", "Color palettes", ["farben", "colors", "colours", "palette"];
    TerminalShell = "terminal.shell", Terminal, "Shell & Umgebung", "Shell & environment", ["allgemein", "general"];
    TerminalBehavior = "terminal.behavior", Terminal, "Verhalten", "Behaviour", ["behavior", "darstellung", "appearance"];
    TerminalClipboard = "terminal.clipboard", Terminal, "Zwischenablage & Einfügen", "Clipboard & paste", ["darstellung", "appearance", "kopieren", "copy"];
    TerminalProfiles = "terminal.profiles", Terminal, "Profile", "Profiles", ["profil", "profile"];
    TerminalRemote = "terminal.remote", Terminal, "Andere Rechner", "Other machines", ["ssh", "remote", "tmux"];
    EditorGeneral = "editor.general", Editor, "Allgemein", "General", ["editor"];
    EditorSearchFormat = "editor.search_format", Editor, "Suche & Formatieren", "Search & formatting", ["editor"];
    EditorSyntax = "editor.syntax", Editor, "Sprachen & Syntax", "Languages & syntax", ["editor", "syntax"];
    EditorSmart = "editor.smart", Editor, "Smart-Modus", "Smart mode", ["editor", "lsp", "sprachserver", "language server"];
    EditorDiff = "editor.diff", Editor, "Vergleich", "Comparison", ["editor", "diff", "compare"];
    EditorOnSave = "editor.on_save", Editor, "Aktionen beim Speichern", "Actions on save", ["editor", "save actions", "speichern"];
    Shortcuts = "shortcuts", Shortcuts, "Tastenkürzel", "Shortcuts", ["keys", "tasten", "hotkey", "keyboard"];
    ClaudeGeneral = "claude.general", Claude, "Allgemein", "General", ["claude code"];
    ClaudeSession = "claude.session", Claude, "Neue Sitzungen", "New sessions", ["standards", "defaults", "modell", "model"];
    ClaudeAccess = "claude.access", Claude, "Zugriff", "Access", ["standards", "defaults", "permission"];
    ClaudeAdvanced = "claude.advanced", Claude, "Erweitert", "Advanced", ["standards", "defaults"];
    ClaudeCosts = "claude.costs", Claude, "Kosten & Budget", "Costs & budget", ["preise", "prices", "usd"];
    ClaudeHooks = "claude.hooks", Claude, "Hooks", "Hooks", ["settings.json"];
    ModelsServers = "models.servers", Models, "Server & Modelle", "Servers & models", ["ollama", "llm", "chat"];
    ModelsTools = "models.tools", Models, "Werkzeuge der Chats", "Tools in chats", ["chat", "agent"];
    ModelsCompact = "models.compact", Models, "Kompaktieren", "Compacting", ["chat", "compact"];
    AgentsAccess = "agents.access", Agents, "Zugriff", "Access", ["mcp", "stufe", "level"];
    AgentsEditor = "agents.editor", Agents, "Editor", "Editor", ["mcp", "editor"];
    AgentsSetup = "agents.setup", Agents, "Einrichten & Systemprompt", "Setup & system prompt", ["mcp", "claude code", "register"];
    AgentsLog = "agents.log", Agents, "Aufrufprotokoll", "Call log", ["mcp", "log", "protokoll"];
    DataBackup = "data.backup", Data, "Sicherung", "Backup", ["datenbank", "database"];
    DataRestore = "data.restore", Data, "Wiederherstellen", "Restore", ["sicherung", "backup"];
    DataMaintenance = "data.maintenance", Data, "Pflege & Aufräumen", "Care & clean-up", ["wartung", "maintenance", "cleanup"];
    DataDiagnostics = "data.diagnostics", Data, "Diagnose", "Diagnostics", ["fehlerbericht", "bug report", "datenordner", "data folder"];
    AboutVersion = "about.version", About, "Version", "Version", ["info", "about", "programm", "program"];
    AboutUpdates = "about.updates", About, "Updates", "Updates", ["info", "about", "aktualisierung"];
    AboutTutorial = "about.tutorial", About, "Tutorial", "Tutorial", ["info", "about", "tour", "hilfe", "help"];
    AboutLicenses = "about.licenses", About, "Lizenzen", "Licenses", ["info", "about", "license", "lizenz"];
}

impl Page {
    /// Startseite: das Fenster öffnet ohne Sprungziel immer hier.
    pub(super) const START: Page = Page::GeneralStart;

    /// Seite zu einer Kennung (Doku-Bilder: `settings:<kennung>`).
    #[cfg_attr(not(any(test, feature = "readme-shots")), allow(dead_code))]
    pub(super) fn from_id(id: &str) -> Option<Page> {
        Page::ALL.iter().copied().find(|p| p.id() == id)
    }

    pub(super) fn title(self, lang: Lang) -> &'static str {
        let (de, en) = self.names();
        lang.t(de, en)
    }

    /// „Bereich › Seite“ (Bereiche mit nur einer Seite: nur ihr Name).
    pub(super) fn path(self, lang: Lang) -> String {
        if self.group().single() {
            self.title(lang).to_string()
        } else {
            format!("{} › {}", self.group().title(lang), self.title(lang))
        }
    }
}

// ------------------------------------------------------------------------------------------------- Suche

/// Für die Suche vereinheitlichtes Zeichen: klein, Umlaute ohne Punkte (`Ü` findet auch `u`).
pub(super) fn fold(c: char) -> char {
    let l = c.to_lowercase().next().unwrap_or(c);
    match l {
        'ä' => 'a',
        'ö' => 'o',
        'ü' => 'u',
        _ => l,
    }
}

/// Suchanfrage: Wörter, die alle vorkommen müssen (in Deutsch oder Englisch, Groß/Klein egal).
#[derive(Clone, Debug, Default)]
pub(super) struct Query {
    terms: Vec<Vec<char>>,
}

impl Query {
    pub(super) fn new(q: &str) -> Query {
        Query { terms: q.split_whitespace().map(|w| w.chars().map(fold).collect::<Vec<char>>()).filter(|t| !t.is_empty()).collect() }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    pub(super) fn terms(&self) -> impl Iterator<Item = &[char]> {
        self.terms.iter().map(|t| t.as_slice())
    }

    /// Alle Wörter kommen in mindestens einem der Texte vor.
    pub(super) fn matches_any(&self, texts: &[&str]) -> bool {
        !self.is_empty() && self.terms.iter().all(|t| texts.iter().any(|x| find(x, t).is_some()))
    }

    /// Fundstellen aller Wörter in `hay` (Byte-Bereiche, sortiert, ohne Überlappung) – zum Hervorheben.
    pub(super) fn ranges(&self, hay: &str) -> Vec<Range<usize>> {
        let mut v: Vec<Range<usize>> = Vec::new();
        for t in &self.terms {
            let mut from = 0;
            while let Some(r) = find(&hay[from..], t) {
                let r = (r.start + from)..(r.end + from);
                from = r.end;
                v.push(r);
                if from >= hay.len() {
                    break;
                }
            }
        }
        v.sort_by_key(|r| r.start);
        let mut out: Vec<Range<usize>> = Vec::new();
        for r in v {
            match out.last_mut() {
                Some(l) if r.start <= l.end => l.end = l.end.max(r.end),
                _ => out.push(r),
            }
        }
        out
    }
}

/// Erste Fundstelle des (vereinheitlichten) Worts `term` in `hay` als Byte-Bereich.
pub(super) fn find(hay: &str, term: &[char]) -> Option<Range<usize>> {
    let hc: Vec<(usize, char)> = hay.char_indices().map(|(i, c)| (i, fold(c))).collect();
    if term.is_empty() || term.len() > hc.len() {
        return None;
    }
    (0..=hc.len() - term.len()).find(|&s| (0..term.len()).all(|k| hc[s + k].1 == term[k])).map(|s| {
        let end = hc.get(s + term.len()).map(|x| x.0).unwrap_or(hay.len());
        hc[s].0..end
    })
}

/// Trifft die Suche den Namen der Seite oder ihres Bereichs (beide Sprachen, Stichwörter der Seite)?
pub(super) fn page_title_hit(q: &Query, p: Page) -> bool {
    let (de, en) = p.names();
    let (g_de, g_en) = p.group().names();
    let mut texts = vec![de, en, g_de, g_en];
    texts.extend_from_slice(p.keywords());
    q.matches_any(&texts)
}

// ------------------------------------------------------------------------------------------------- Baum

/// Eintrag des Baums.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Node {
    Group(Group),
    Page(Page),
}

/// Tasten des Baums.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum NavKey {
    Up,
    Down,
    Left,
    Right,
}

/// Zustand des Einstellungsfensters je Hub-Fenster (wird mit der Fensteransicht getauscht, `multiwin`).
pub(super) struct Nav {
    /// Gezeigte Seite.
    pub page: Page,
    /// Suchtext.
    pub query: String,
    /// Filter „Nur geänderte“.
    pub only_modified: bool,
    /// Auf dieser Seite trotz Suche alle Zeilen zeigen („Alle zeigen“).
    pub show_all: Option<Page>,
    /// Aufgeklappte Bereiche (ohne Suche bzw. Filter; mit Filter sind alle Bereiche mit Treffern offen).
    pub open: HashSet<Group>,
    /// Markierter Eintrag des Baums (Tastatur); eine Seite darin ist zugleich die gezeigte.
    pub cursor: Node,
    /// Suchfeld im nächsten Bild fokussieren.
    pub focus_search: bool,
    /// Fenster nach vorn holen (Sprung, während es schon offen ist).
    pub raise: bool,
    /// War im vorigen Bild offen (Öffnen erkennen).
    pub was_open: bool,
    /// Lage beim Öffnen (Position, Größe in Punkten) – bleibt, solange das Fenster offen ist.
    pub init: Option<(Option<Pos2>, Vec2)>,
    /// Zuletzt gesehene Lage (Position, Größe): gilt beim nächsten Öffnen, nur zur Laufzeit.
    pub geo: Option<(Pos2, Vec2)>,
    /// Fokussiertes Element am Ende des vorigen Bilds (Esc: erst ein anderes Feld verlassen).
    pub last_focus: Option<Id>,
    /// Offene Rückfrage „Zurücksetzen?“: Einträge (Schlüssel), die zurückgesetzt werden sollen.
    pub ask: Option<Vec<&'static str>>,
    /// Geänderte Einträge, solange sich die Einstellungen nicht ändern (nicht in jedem Bild neu vergleichen).
    pub mod_cache: super::settings_items::ModCache,
}

impl Default for Nav {
    fn default() -> Self {
        Nav {
            page: Page::START,
            query: String::new(),
            only_modified: false,
            show_all: None,
            open: HashSet::from([Page::START.group()]),
            cursor: Node::Page(Page::START),
            focus_search: true,
            raise: false,
            was_open: false,
            init: None,
            geo: None,
            last_focus: None,
            ask: None,
            mod_cache: None,
        }
    }
}

impl Nav {
    /// Seite zeigen (klappt ihren Bereich auf).
    pub(super) fn select(&mut self, p: Page) {
        if self.page != p {
            self.show_all = None;
        }
        self.page = p;
        self.cursor = Node::Page(p);
        self.open.insert(p.group());
    }

    /// Fenster (neu) öffnen bei Seite `p`: Suche und Filter leer, Suchfeld fokussiert, Fenster nach vorn.
    pub(super) fn open_at(&mut self, p: Page) {
        self.select(p);
        self.query.clear();
        self.only_modified = false;
        self.show_all = None;
        self.focus_search = true;
        self.raise = true;
        self.ask = None;
    }

    /// Sichtbare Einträge des Baums. `filter` = nur diese Seiten (Suche/„Nur geänderte“; deren Bereiche sind offen).
    pub(super) fn visible(&self, filter: Option<&BTreeSet<Page>>) -> Vec<Node> {
        let mut v = Vec::new();
        for &g in Group::ALL {
            let pages: Vec<Page> = g.pages().filter(|p| filter.is_none_or(|f| f.contains(p))).collect();
            if pages.is_empty() {
                continue;
            }
            if g.single() {
                v.push(Node::Page(pages[0]));
                continue;
            }
            v.push(Node::Group(g));
            if filter.is_some() || self.open.contains(&g) {
                v.extend(pages.into_iter().map(Node::Page));
            }
        }
        v
    }

    /// Taste im Baum; `true`, wenn sich die gezeigte Seite geändert hat.
    pub(super) fn key(&mut self, k: NavKey, filter: Option<&BTreeSet<Page>>) -> bool {
        let before = self.page;
        let vis = self.visible(filter);
        if vis.is_empty() {
            return false;
        }
        let idx = vis.iter().position(|n| *n == self.cursor).or_else(|| vis.iter().position(|n| *n == Node::Page(self.page))).unwrap_or(0);
        match k {
            NavKey::Up | NavKey::Down => {
                let i = if k == NavKey::Up { idx.saturating_sub(1) } else { (idx + 1).min(vis.len() - 1) };
                self.cursor = vis[i];
                if let Node::Page(p) = vis[i] {
                    self.select(p);
                }
            }
            NavKey::Left => match self.cursor {
                Node::Page(p) if !p.group().single() => self.cursor = Node::Group(p.group()),
                Node::Group(g) if filter.is_none() => {
                    self.open.remove(&g);
                }
                _ => {}
            },
            NavKey::Right => {
                if let Node::Group(g) = self.cursor {
                    if filter.is_none() && !self.open.contains(&g) {
                        self.open.insert(g);
                    } else if let Some(p) = g.pages().find(|p| filter.is_none_or(|f| f.contains(p))) {
                        self.select(p);
                    }
                }
            }
        }
        self.page != before
    }

    /// Nächste bzw. vorige Seite (Strg+Tab / Strg+Umschalt+Tab), im Kreis; mit Filter nur gefilterte Seiten.
    pub(super) fn step(&mut self, delta: i32, filter: Option<&BTreeSet<Page>>) {
        let pages: Vec<Page> = Page::ALL.iter().copied().filter(|p| filter.is_none_or(|f| f.contains(p))).collect();
        if pages.is_empty() {
            return;
        }
        let i = pages.iter().position(|p| *p == self.page).map(|i| i as i32).unwrap_or(-1);
        let n = pages.len() as i32;
        let j = if i < 0 { if delta > 0 { 0 } else { n - 1 } } else { (i + delta).rem_euclid(n) };
        self.select(pages[j as usize]);
    }
}

// ------------------------------------------------------------------------------------------------- Oberfläche

/// Id des Suchfelds (Strg+F, Fokus beim Öffnen).
pub(super) fn search_id() -> Id {
    Id::new("settings_search")
}

/// Suchfeld mit Lupe und Löschknopf. Liefert `true`, wenn sich der Text geändert hat.
pub(super) fn search_box(ui: &mut Ui, nav: &mut Nav, lang: Lang) -> bool {
    let id = search_id();
    let accent = style::accent();
    let mut changed = false;
    let w = ui.available_width();
    let focused = ui.memory(|m| m.has_focus(id));
    let frame = egui::Frame::new()
        .fill(style::ui().input_bg)
        .stroke(Stroke::new(1.0_f32, if focused { accent } else { style::ui().widget_border }))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(8, 5));
    frame.show(ui, |ui| {
        ui.set_width(w - 18.0);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(16.0), Sense::hover());
            style::paint_icon(ui.painter(), style::Icon::Search, r.center(), style::dim(ui));
            let clear_w = if nav.query.is_empty() { 0.0 } else { 22.0 };
            let te = egui::TextEdit::singleline(&mut nav.query)
                .id(id)
                .frame(false)
                .desired_width(ui.available_width() - clear_w)
                .hint_text(lang.t("Suchen (Strg+F) …", "Search (Ctrl+F) …"));
            let r = ui.add(te);
            changed |= r.changed();
            if nav.focus_search {
                r.request_focus();
                nav.focus_search = false;
            }
            // Pfeiltasten gehören dem Baum, nicht der Fokus-Wanderung von egui.
            ui.memory_mut(|m| m.set_focus_lock_filter(id, egui::EventFilter { tab: false, horizontal_arrows: true, vertical_arrows: true, escape: false }));
            if !nav.query.is_empty() && ui.add(egui::Button::new(egui::RichText::new("✕").color(style::dim(ui))).frame(false)).on_hover_text(lang.t("Suche leeren", "Clear search")).clicked() {
                nav.query.clear();
                nav.focus_search = true;
                changed = true;
            }
        });
    });
    changed
}

/// Was der Baum zeigt.
pub(super) struct TreeView<'a> {
    pub lang: Lang,
    /// Nur diese Seiten (Suche bzw. „Nur geänderte“).
    pub filter: Option<&'a BTreeSet<Page>>,
    /// Zahl je Seite: Treffer (Suche) bzw. geänderte Einstellungen.
    pub badges: &'a BTreeMap<Page, usize>,
    /// Zahlen in Akzentfarbe (geänderte) statt gedämpft (Treffer).
    pub accent_badges: bool,
}

/// Zeilenhöhe des Baums.
pub(super) const TREE_ROW_H: f32 = 26.0;

/// Kategorienbaum. Klick auf einen Bereich klappt ihn auf bzw. zu (und zeigt beim Aufklappen seine erste Seite), Klick
/// auf eine Seite zeigt sie. Liefert `true`, wenn sich die gezeigte Seite geändert hat.
pub(super) fn tree(ui: &mut Ui, nav: &mut Nav, v: &TreeView) -> bool {
    let before = nav.page;
    let text = ui.visuals().text_color();
    let dim = style::dim(ui);
    let accent = style::accent();
    let font = FontId::proportional(13.5);
    for node in nav.visible(v.filter) {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), TREE_ROW_H), Sense::CLICK);
        let p = ui.painter();
        let selected = matches!(node, Node::Page(pg) if pg == nav.page);
        if selected {
            p.rect_filled(rect, 6.0, accent.gamma_multiply(0.28));
        } else if resp.hovered() {
            p.rect_filled(rect, 6.0, ui.visuals().widgets.hovered.bg_fill);
        }
        if nav.cursor == node && !selected {
            p.rect_stroke(rect.shrink(0.5), 6.0, Stroke::new(1.0_f32, accent.gamma_multiply(0.6)), egui::StrokeKind::Inside);
        }
        let (label, x, badge) = match node {
            Node::Group(g) => {
                let open = v.filter.is_some() || nav.open.contains(&g);
                arrow(p, Pos2::new(rect.left() + 12.0, rect.center().y), open, dim);
                (g.title(v.lang), rect.left() + 24.0, None)
            }
            Node::Page(pg) if pg.group().single() => (pg.title(v.lang), rect.left() + 24.0, v.badges.get(&pg).copied()),
            Node::Page(pg) => (pg.title(v.lang), rect.left() + 38.0, v.badges.get(&pg).copied()),
        };
        let col = if selected { ui.visuals().widgets.active.fg_stroke.color } else { text };
        let g = p.layout_no_wrap(label.to_string(), font.clone(), col);
        p.galley(Pos2::new(x, rect.center().y - g.size().y / 2.0), g, col);
        if let Some(n) = badge.filter(|n| *n > 0) {
            let bc = if v.accent_badges { accent } else { dim };
            p.text(Pos2::new(rect.right() - 10.0, rect.center().y), egui::Align2::RIGHT_CENTER, n.to_string(), FontId::proportional(12.5), bc);
        }
        // Tutorial (#143): Einträge des Baums als Ziele (früher die Reiter).
        let resp = match node {
            Node::Page(Page::Shortcuts) => resp.spot(Spot::TabShortcuts),
            Node::Page(Page::TerminalProfiles) => resp.spot(Spot::TabProfiles),
            Node::Group(Group::Models) => resp.spot(Spot::TabModels),
            Node::Group(Group::Editor) => resp.spot(Spot::TabEditor),
            _ => resp,
        };
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            match node {
                Node::Group(g) => {
                    nav.cursor = node;
                    if v.filter.is_none() {
                        if nav.open.contains(&g) {
                            nav.open.remove(&g);
                        } else {
                            nav.open.insert(g);
                            if nav.page.group() != g
                                && let Some(first) = g.pages().next()
                            {
                                nav.select(first);
                            }
                        }
                    }
                }
                Node::Page(pg) => nav.select(pg),
            }
        }
    }
    nav.page != before
}

/// Dreieck des Baums: rechts (zu) bzw. unten (offen).
fn arrow(p: &egui::Painter, c: Pos2, open: bool, col: Color32) {
    let pts = if open {
        vec![Pos2::new(c.x - 4.0, c.y - 2.0), Pos2::new(c.x + 4.0, c.y - 2.0), Pos2::new(c.x, c.y + 3.0)]
    } else {
        vec![Pos2::new(c.x - 2.0, c.y - 4.0), Pos2::new(c.x - 2.0, c.y + 4.0), Pos2::new(c.x + 3.0, c.y)]
    };
    p.add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
}

/// Schmale Fenster (unter etwa 700 px): statt des Baums eine Auswahl oben. Liefert `true` bei einem Seitenwechsel.
pub(super) fn picker(ui: &mut Ui, nav: &mut Nav, v: &TreeView) -> bool {
    let before = nav.page;
    let label = |p: Page| match v.badges.get(&p).filter(|n| **n > 0) {
        Some(n) => format!("{}  ({n})", p.path(v.lang)),
        None => p.path(v.lang),
    };
    let mut sel = nav.page;
    egui::ComboBox::from_id_salt("settings_picker").width(ui.available_width() - 8.0).height(420.0).selected_text(label(nav.page)).show_ui(ui, |ui| {
        for p in Page::ALL.iter().copied().filter(|p| v.filter.is_none_or(|f| f.contains(p))) {
            ui.selectable_value(&mut sel, p, label(p));
        }
    });
    if sel != nav.page {
        nav.select(sel);
    }
    nav.page != before
}

/// Breite, unter der der Baum zur Auswahl oben wird.
pub(super) const NARROW_W: f32 = 700.0;

/// Breite des Baums.
pub(super) const TREE_W: f32 = 260.0;

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{Event, Modifiers, PointerButton, RawInput, Rect};

    #[test]
    fn page_ids_are_unique_stable_and_round_trip() {
        let mut seen = HashSet::new();
        for p in Page::ALL {
            let id = p.id();
            assert!(seen.insert(id), "doppelte Kennung {id}");
            assert_eq!(Page::from_id(id), Some(*p));
            assert!(id.chars().all(|c| c.is_ascii_lowercase() || c == '.' || c == '_'), "{id}");
            // Kennung beginnt mit dem Bereich (außer der einzelnen Seite „shortcuts“).
            let group = format!("{:?}", p.group()).to_lowercase();
            assert!(id == "shortcuts" || id.starts_with(&format!("{}.", if group == "data" { "data" } else { &group })), "{id} gehört zu {group}");
        }
        assert_eq!(Page::from_id("nope"), None);
        // Die Kennungen aus der Entscheidung (#395) gibt es.
        for id in ["general.start", "appearance.transparency", "terminal.behavior", "editor.on_save", "editor.smart", "shortcuts", "claude.access", "models.servers", "agents.access", "agents.editor", "data.backup", "about.licenses"] {
            assert!(Page::from_id(id).is_some(), "{id}");
        }
    }

    #[test]
    fn every_group_has_pages_and_two_levels_only() {
        for g in Group::ALL {
            assert!(g.pages().count() >= 1, "{g:?}");
            let (de, en) = g.names();
            assert!(!de.is_empty() && !en.is_empty());
        }
        assert!(Group::Shortcuts.single());
        assert!(!Group::General.single());
        for p in Page::ALL {
            let (de, en) = p.names();
            assert!(!de.is_empty() && !en.is_empty(), "{p:?}");
        }
        assert_eq!(Page::AgentsAccess.path(Lang::De), "Agenten › Zugriff");
        assert_eq!(Page::AgentsAccess.path(Lang::En), "Agents › Access");
        assert_eq!(Page::Shortcuts.path(Lang::En), "Shortcuts");
    }

    #[test]
    fn search_folds_case_and_umlauts_and_highlights() {
        let q = Query::new("ÜBER");
        assert!(q.matches_any(&["Darüber hinaus"]));
        assert!(Query::new("uber").matches_any(&["Darüber"]));
        assert_eq!(find("Darüber", &['u', 'b']), Some(3..6), "Byte-Bereich über das Ü");
        let q = Query::new("transp anim");
        assert!(q.matches_any(&["Transparenz", "Animation"]), "jedes Wort irgendwo");
        assert!(!q.matches_any(&["Transparenz"]));
        assert!(!Query::new("  ").matches_any(&["x"]), "leere Suche trifft nichts");
        let q = Query::new("ab");
        assert_eq!(q.ranges("abxAB ab"), vec![0..2, 3..5, 6..8]);
        assert_eq!(Query::new("abc bcd").ranges("abcd"), vec![0..4], "überlappende Treffer zusammengefasst");
    }

    #[test]
    fn page_titles_match_in_both_languages() {
        assert!(page_title_hit(&Query::new("opacity transp"), Page::AppearanceTransparency) || page_title_hit(&Query::new("transp"), Page::AppearanceTransparency));
        assert!(page_title_hit(&Query::new("agenten"), Page::AgentsLog), "Bereichsname");
        assert!(page_title_hit(&Query::new("agents"), Page::AgentsLog));
        assert!(page_title_hit(&Query::new("farben"), Page::AppearancePalettes), "alter Reitername „Farben“");
        assert!(!page_title_hit(&Query::new("budget"), Page::AboutVersion));
    }

    #[test]
    fn keys_walk_the_tree_like_jetbrains() {
        let mut nav = Nav::default();
        assert_eq!(nav.page, Page::GeneralStart);
        // ↓ geht zur nächsten Seite im offenen Bereich
        assert!(nav.key(NavKey::Down, None));
        assert_eq!(nav.page, Page::GeneralNotifications);
        // ← springt zum Bereich, noch einmal ← klappt zu
        assert!(!nav.key(NavKey::Left, None));
        assert_eq!(nav.cursor, Node::Group(Group::General));
        nav.key(NavKey::Left, None);
        assert!(!nav.open.contains(&Group::General));
        assert!(!nav.visible(None).contains(&Node::Page(Page::GeneralStart)), "zugeklappt");
        // → klappt auf, noch einmal → zeigt die erste Seite
        nav.key(NavKey::Right, None);
        assert!(nav.open.contains(&Group::General));
        assert!(nav.key(NavKey::Right, None));
        assert_eq!(nav.page, Page::GeneralStart);
        // ↓ über einen zugeklappten Bereich: Markierung auf dem Bereich, Seite bleibt
        for _ in 0..4 {
            nav.key(NavKey::Down, None);
        }
        assert_eq!(nav.cursor, Node::Group(Group::Appearance));
        assert_eq!(nav.page, Page::GeneralIntegration);
        // Tastenkürzel ist ein Blatt ohne Bereich
        assert!(nav.visible(None).contains(&Node::Page(Page::Shortcuts)));
        assert!(!nav.visible(None).contains(&Node::Group(Group::Shortcuts)));
    }

    #[test]
    fn ctrl_tab_steps_through_pages_and_respects_the_filter() {
        let mut nav = Nav::default();
        nav.step(1, None);
        assert_eq!(nav.page, Page::GeneralNotifications);
        nav.step(-2, None);
        assert_eq!(nav.page, *Page::ALL.last().unwrap(), "im Kreis");
        assert!(nav.open.contains(&Group::About), "Bereich der neuen Seite aufgeklappt");
        let f: BTreeSet<Page> = [Page::AppearanceTheme, Page::AgentsLog].into();
        nav.step(1, Some(&f));
        assert_eq!(nav.page, Page::AppearanceTheme);
        nav.step(1, Some(&f));
        assert_eq!(nav.page, Page::AgentsLog);
        // Mit Filter sind die Bereiche mit Treffern offen, andere fehlen.
        let vis = nav.visible(Some(&f));
        assert_eq!(vis, vec![Node::Group(Group::Appearance), Node::Page(Page::AppearanceTheme), Node::Group(Group::Agents), Node::Page(Page::AgentsLog)]);
    }

    #[test]
    fn opening_resets_search_and_shows_the_page() {
        let mut nav = Nav { query: "abc".into(), only_modified: true, show_all: Some(Page::AboutTutorial), ..Nav::default() };
        nav.open_at(Page::AgentsAccess);
        assert_eq!((nav.page, nav.query.as_str(), nav.only_modified, nav.show_all), (Page::AgentsAccess, "", false, None));
        assert!(nav.focus_search && nav.raise && nav.open.contains(&Group::Agents));
    }

    struct H {
        ctx: egui::Context,
        t: f64,
        texts: Vec<(String, Rect)>,
    }

    impl H {
        fn run(&mut self, events: Vec<Event>, mut f: impl FnMut(&mut Ui)) {
            self.t += 0.1;
            let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(300.0, 1600.0))), time: Some(self.t), events, ..Default::default() };
            let out = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| f(ui));
            });
            self.texts = out.shapes.iter().filter_map(|s| if let egui::Shape::Text(t) = &s.shape { Some((t.galley.text().to_string(), t.visual_bounding_rect())) } else { None }).collect();
        }
        fn pos(&self, s: &str) -> Pos2 {
            self.texts.iter().find(|(t, _)| t == s).map(|(_, r)| r.center()).unwrap_or_else(|| panic!("{s} fehlt: {:?}", self.texts.iter().map(|t| &t.0).collect::<Vec<_>>()))
        }
        fn has(&self, s: &str) -> bool {
            self.texts.iter().any(|(t, _)| t == s)
        }
        fn click(&mut self, s: &str, mut f: impl FnMut(&mut Ui)) {
            let pos = self.pos(s);
            self.run(vec![Event::PointerMoved(pos)], &mut f);
            self.run(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }], &mut f);
            self.run(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }], &mut f);
        }
    }

    /// Baum ohne Fenster: Bereiche und Seiten in beiden Sprachen, Klick auf Bereich klappt auf und zeigt die erste Seite,
    /// Klick auf eine Seite zeigt sie; mit Filter nur Seiten mit Treffern samt Zahl.
    #[test]
    fn tree_shows_groups_pages_and_counts() {
        for lang in [Lang::En, Lang::De] {
            let mut nav = Nav::default();
            let badges: BTreeMap<Page, usize> = [(Page::GeneralNotifications, 2)].into();
            let mut h = H { ctx: egui::Context::default(), t: 0.0, texts: Vec::new() };
            let view = |nav: &mut Nav, ui: &mut Ui| {
                tree(ui, nav, &TreeView { lang, filter: None, badges: &badges, accent_badges: true });
            };
            h.run(vec![], |ui| view(&mut nav, ui));
            assert!(h.has(Group::General.title(lang)) && h.has(Page::GeneralStart.title(lang)) && h.has(Group::About.title(lang)));
            assert!(h.has("2"), "Zahl der geänderten Einstellungen");
            assert!(!h.has(Page::AgentsLog.title(lang)), "Agenten zugeklappt");
            h.click(Group::Agents.title(lang), |ui| view(&mut nav, ui));
            assert_eq!(nav.page, Page::AgentsAccess, "Aufklappen zeigt die erste Seite");
            h.run(vec![], |ui| view(&mut nav, ui));
            h.click(Page::AgentsLog.title(lang), |ui| view(&mut nav, ui));
            assert_eq!(nav.page, Page::AgentsLog);
            // Filter: nur Seiten mit Treffern
            let f: BTreeSet<Page> = [Page::AppearanceTransparency].into();
            let hits: BTreeMap<Page, usize> = [(Page::AppearanceTransparency, 2)].into();
            h.run(vec![], |ui| {
                tree(ui, &mut nav, &TreeView { lang, filter: Some(&f), badges: &hits, accent_badges: false });
            });
            assert!(h.has(Page::AppearanceTransparency.title(lang)) && h.has(Group::Appearance.title(lang)));
            assert!(!h.has(Group::General.title(lang)) && !h.has(Page::AgentsLog.title(lang)));
        }
    }
}
