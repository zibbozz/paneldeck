//! Dashboards: benannte Raster aus Kacheln, je Projekt oder global.
//!
//! * **Layout** ([`Layout`], JSON mit Format-Version): Liste von Kacheln `{id, kind, config, x, y, w, h}` auf einem Raster mit
//!   [`GRID_COLUMNS`] (12) Spalten; die Zeilen wachsen nach unten (bis [`MAX_ROWS`]). Position und Größe sind ganze
//!   Rasterzellen. Kachelarten ([`TileKind`]): Suche, Sprint, Burndown, Zahlen/Diagramm, Zeiterfassung, Favoriten, Markierte,
//!   Alle Projekte.
//! * **Standard**: [`default_dashboard_layout`] ist **eine große Such-Kachel**.
//! * **Prüfung** ([`validate_layout`]): Kacheln liegen im Raster, überlappen nicht, Nummern sind eindeutig, Einstellungen und
//!   Projektbezüge passen. **Hilfen** für die Oberfläche: [`LayoutG::normalize`] (zurechtrücken + Überlappungen nach unten
//!   schieben), [`LayoutG::pack`] (zusätzlich nach oben verdichten), [`LayoutG::move_tile`]/[`LayoutG::resize_tile`] (die
//!   bewegte Kachel behält ihren Platz, die anderen weichen aus), [`LayoutG::free_spot`] und [`LayoutG::add_tile`].
//! * **Standard-Dashboard**: je Bereich (Projekt oder global) höchstens eins ([`set_default`] ist atomar; der Teil-Unique-Index
//!   erzwingt es zusätzlich). [`startup_dashboard`] liefert das beim Start zu ladende.
//! * **Export/Import**: [`DashboardDoc`] (eigenes Format mit Versionsfeld). Projekte stehen darin als **Schlüssel** statt als
//!   Kennung. Der Import legt **immer ein neues** Dashboard an (nie Standard, Namen bei Bedarf „X (2)“), ändert nie
//!   Bestehendes und meldet unbekannte Projekte im [`ImportReport`].

use super::{atomic, clean_name, copy_name, ensure_name_free, names_in, numbered_name, DocProblem, Res, ViewError, MAX_JQL_CHARS, MAX_NAME_CHARS};
use crate::planning::common;
use crate::planning::projects::{editable, project_by_key};
use crate::planning::PlanError;
use crate::state::Lang;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// Spalten des Rasters.
pub const GRID_COLUMNS: u32 = 12;
/// Zeilen des Rasters insgesamt (`y + h` darf nicht darüber liegen).
pub const MAX_ROWS: u32 = 1000;
/// Höchste Kachel in Zeilen.
pub const MAX_TILE_HEIGHT: u32 = 24;
/// Höchstzahl der Kacheln eines Dashboards.
pub const MAX_TILES: usize = 40;
/// Höchstzahl der Spalten einer Such-Kachel und Länge eines Spaltennamens.
pub const MAX_TILE_COLUMNS: usize = 20;
pub const MAX_COLUMN_CHARS: usize = 40;
/// Aktuelle Format-Version des Layouts.
pub const LAYOUT_VERSION: u32 = 1;
/// Kennung des Export-Dokuments.
pub const FORMAT: &str = "paneldeck-dashboard";
/// Aktuelle Format-Version des Export-Dokuments.
pub const DOC_VERSION: u32 = 1;
/// Spalten der Standard-Such-Kachel (Kennungen, die die Oberfläche kennt).
pub const DEFAULT_SEARCH_COLUMNS: [&str; 5] = ["key", "type", "summary", "status", "priority"];

fn layout_version() -> u32 {
    LAYOUT_VERSION
}

/// Gruppierung einer Zahlen-/Diagramm-Kachel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CountGroup {
    Status,
    Type,
    Priority,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChartKind {
    Pie,
    Bar,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeRange {
    Day,
    Week,
}

/// Art und Einstellungen einer Kachel. `P` ist der Projektbezug: `i64` (Datenbank) oder `String` (Schlüssel im Export-Dokument).
/// `project_id: None` heißt „das Projekt des Dashboards“ (bei Zeiterfassung: alle Projekte bzw. das des Dashboards).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "config", rename_all = "snake_case", bound(serialize = "P: Serialize", deserialize = "P: Deserialize<'de>"))]
pub enum TileKind<P> {
    /// Ticket-Suche: JQL (leer = alle), anzuzeigende Spalten, Titel (leer = Standardtitel).
    Search {
        #[serde(default)]
        jql: String,
        #[serde(default)]
        columns: Vec<String>,
        #[serde(default)]
        title: String,
    },
    Sprint {
        #[serde(default)]
        project_id: Option<P>,
    },
    Burndown {
        #[serde(default)]
        project_id: Option<P>,
    },
    /// Anzahl der Tickets einer Abfrage nach Status/Typ/Priorität als Torte oder Balken.
    Counts {
        #[serde(default)]
        jql: String,
        group_by: CountGroup,
        chart: ChartKind,
    },
    TimeTracking {
        range: TimeRange,
        #[serde(default)]
        project_id: Option<P>,
    },
    Favorites,
    Flagged,
    Projects,
}

impl<P> TileKind<P> {
    /// Kurzname der Art (Kennung im JSON).
    pub fn name(&self) -> &'static str {
        match self {
            TileKind::Search { .. } => "search",
            TileKind::Sprint { .. } => "sprint",
            TileKind::Burndown { .. } => "burndown",
            TileKind::Counts { .. } => "counts",
            TileKind::TimeTracking { .. } => "time_tracking",
            TileKind::Favorites => "favorites",
            TileKind::Flagged => "flagged",
            TileKind::Projects => "projects",
        }
    }

    /// Größe (Spalten, Zeilen) einer neuen Kachel dieser Art.
    pub fn default_size(&self) -> (u32, u32) {
        match self {
            TileKind::Search { .. } => (12, 12),
            TileKind::Sprint { .. } | TileKind::Burndown { .. } => (6, 8),
            TileKind::Counts { .. } => (4, 8),
            TileKind::TimeTracking { .. } => (6, 8),
            TileKind::Favorites | TileKind::Flagged => (4, 8),
            TileKind::Projects => (4, 6),
        }
    }

    /// Braucht die Kachel ein Projekt (Sprint, Burndown)? Im Projekt-Dashboard ist das das eigene.
    fn needs_project(&self) -> bool {
        matches!(self, TileKind::Sprint { .. } | TileKind::Burndown { .. })
    }

    fn project(&self) -> Option<&Option<P>> {
        match self {
            TileKind::Sprint { project_id } | TileKind::Burndown { project_id } | TileKind::TimeTracking { project_id, .. } => Some(project_id),
            _ => None,
        }
    }

    fn set_project(&mut self, p: Option<P>) {
        if let TileKind::Sprint { project_id } | TileKind::Burndown { project_id } | TileKind::TimeTracking { project_id, .. } = self {
            *project_id = p;
        }
    }

    /// Wandelt den Projektbezug um; `f` liefert `None` für ein unbekanntes Projekt (das dann `None` wird, `false` im Ergebnis).
    fn map_project<Q>(self, f: &mut dyn FnMut(P) -> Option<Q>) -> (TileKind<Q>, bool) {
        let mut ok = true;
        let mut m = |p: Option<P>| match p {
            Some(p) => {
                let r = f(p);
                ok &= r.is_some();
                r
            }
            None => None,
        };
        let kind = match self {
            TileKind::Search { jql, columns, title } => TileKind::Search { jql, columns, title },
            TileKind::Sprint { project_id } => TileKind::Sprint { project_id: m(project_id) },
            TileKind::Burndown { project_id } => TileKind::Burndown { project_id: m(project_id) },
            TileKind::Counts { jql, group_by, chart } => TileKind::Counts { jql, group_by, chart },
            TileKind::TimeTracking { range, project_id } => TileKind::TimeTracking { range, project_id: m(project_id) },
            TileKind::Favorites => TileKind::Favorites,
            TileKind::Flagged => TileKind::Flagged,
            TileKind::Projects => TileKind::Projects,
        };
        (kind, ok)
    }
}

/// Eine Kachel: Nummer (im Dashboard eindeutig), Platz auf dem Raster und Art samt Einstellungen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TileG<P> {
    pub id: u32,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    #[serde(flatten)]
    pub kind: TileKind<P>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutG<P> {
    #[serde(default = "layout_version")]
    pub version: u32,
    #[serde(default = "Vec::new")]
    pub tiles: Vec<TileG<P>>,
}

pub type Tile = TileG<i64>;
pub type Layout = LayoutG<i64>;

impl<P> Default for LayoutG<P> {
    fn default() -> Self {
        LayoutG { version: LAYOUT_VERSION, tiles: Vec::new() }
    }
}

fn overlap<P>(a: &TileG<P>, b: &TileG<P>) -> bool {
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
}

impl<P> LayoutG<P> {
    pub fn tile(&self, id: u32) -> Option<&TileG<P>> {
        self.tiles.iter().find(|t| t.id == id)
    }

    /// Nächste freie Kachel-Nummer.
    pub fn next_id(&self) -> u32 {
        self.tiles.iter().map(|t| t.id).max().map_or(1, |m| m + 1)
    }

    /// Größen und Positionen ins Raster zwingen (mindestens 1×1, höchstens 12 Spalten und [`MAX_TILE_HEIGHT`] Zeilen).
    pub fn clamp(&mut self) {
        for t in &mut self.tiles {
            t.w = t.w.clamp(1, GRID_COLUMNS);
            t.h = t.h.clamp(1, MAX_TILE_HEIGHT);
            t.x = t.x.min(GRID_COLUMNS - t.w);
            t.y = t.y.min(MAX_ROWS - t.h);
        }
    }

    /// Doppelte Nummern ersetzen (die erste behält ihre).
    fn fix_ids(&mut self) {
        let mut seen = std::collections::HashSet::new();
        let mut next = self.next_id();
        for t in &mut self.tiles {
            if !seen.insert(t.id) {
                t.id = next;
                seen.insert(next);
                next += 1;
            }
        }
    }

    /// Reihenfolge, in der die Kacheln gesetzt werden: die festgehaltene zuerst, dann nach (y, x, Nummer).
    fn placement_order(&self, pinned: Option<u32>) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.tiles.len()).collect();
        idx.sort_by_key(|&i| {
            let t = &self.tiles[i];
            (Some(t.id) != pinned, t.y, t.x, t.id)
        });
        idx
    }

    fn normalize_with(&mut self, pinned: Option<u32>) {
        self.clamp();
        self.fix_ids();
        let order = self.placement_order(pinned);
        let mut placed: Vec<usize> = Vec::new();
        for i in order {
            loop {
                let bottom = placed.iter().filter(|&&j| overlap(&self.tiles[i], &self.tiles[j])).map(|&j| self.tiles[j].y + self.tiles[j].h).max();
                match bottom {
                    Some(b) => self.tiles[i].y = b,
                    None => break,
                }
            }
            placed.push(i);
        }
    }

    /// Rückt alles ins Raster und schiebt überlappende Kacheln nach unten (Reihenfolge: oben vor unten, links vor rechts).
    /// Danach besteht [`validate_layout`] die Rasterprüfung (solange nicht mehr als [`MAX_TILES`] Kacheln da sind).
    pub fn normalize(&mut self) {
        self.normalize_with(None);
    }

    /// Wie [`normalize`](Self::normalize), zusätzlich rückt jede Kachel so weit nach oben, wie es geht (keine Lücken).
    pub fn pack(&mut self) {
        self.normalize();
        let order = self.placement_order(None);
        let mut placed: Vec<usize> = Vec::new();
        for i in order {
            let mut y = 0;
            while placed.iter().any(|&j| {
                let (a, b) = (&self.tiles[i], &self.tiles[j]);
                a.x < b.x + b.w && b.x < a.x + a.w && y < b.y + b.h && b.y < y + a.h
            }) {
                y += 1;
            }
            self.tiles[i].y = y;
            placed.push(i);
        }
    }

    /// Setzt die Kachel an eine neue Stelle; die anderen weichen nach unten aus. `false`, wenn es die Kachel nicht gibt.
    pub fn move_tile(&mut self, id: u32, x: u32, y: u32) -> bool {
        match self.tiles.iter_mut().find(|t| t.id == id) {
            Some(t) => {
                t.x = x;
                t.y = y;
                self.normalize_with(Some(id));
                true
            }
            None => false,
        }
    }

    /// Ändert die Größe der Kachel; die anderen weichen nach unten aus. `false`, wenn es die Kachel nicht gibt.
    pub fn resize_tile(&mut self, id: u32, w: u32, h: u32) -> bool {
        match self.tiles.iter_mut().find(|t| t.id == id) {
            Some(t) => {
                t.w = w;
                t.h = h;
                self.normalize_with(Some(id));
                true
            }
            None => false,
        }
    }

    /// Erste freie Stelle (zeilenweise von oben links) für eine Kachel der Größe `w`×`h`.
    pub fn free_spot(&self, w: u32, h: u32) -> (u32, u32) {
        let (w, h) = (w.clamp(1, GRID_COLUMNS), h.clamp(1, MAX_TILE_HEIGHT));
        let mut y = 0;
        loop {
            for x in 0..=(GRID_COLUMNS - w) {
                let free = !self.tiles.iter().any(|t| x < t.x + t.w && t.x < x + w && y < t.y + t.h && t.y < y + h);
                if free {
                    return (x, y);
                }
            }
            y += 1;
        }
    }

    /// Fügt eine Kachel an der ersten freien Stelle ein (Größe `None` = Standardgröße der Art) und liefert ihre Nummer;
    /// `None`, wenn schon [`MAX_TILES`] Kacheln da sind.
    pub fn add_tile(&mut self, kind: TileKind<P>, size: Option<(u32, u32)>) -> Option<u32> {
        if self.tiles.len() >= MAX_TILES {
            return None;
        }
        let (w, h) = size.unwrap_or_else(|| kind.default_size());
        let (w, h) = (w.clamp(1, GRID_COLUMNS), h.clamp(1, MAX_TILE_HEIGHT));
        let (x, y) = self.free_spot(w, h);
        let id = self.next_id();
        self.tiles.push(TileG { id, x, y, w, h, kind });
        Some(id)
    }

    /// Entfernt die Kachel; `false`, wenn es sie nicht gab.
    pub fn remove_tile(&mut self, id: u32) -> bool {
        let n = self.tiles.len();
        self.tiles.retain(|t| t.id != id);
        self.tiles.len() != n
    }

    fn map_projects<Q>(self, f: &mut dyn FnMut(P) -> Option<Q>) -> (LayoutG<Q>, usize) {
        let mut dropped = 0;
        let mut tiles = Vec::new();
        for t in self.tiles {
            let needs = t.kind.needs_project();
            let (kind, ok) = t.kind.map_project(f);
            if !ok && needs {
                dropped += 1;
                continue;
            }
            tiles.push(TileG { id: t.id, x: t.x, y: t.y, w: t.w, h: t.h, kind });
        }
        (LayoutG { version: self.version, tiles }, dropped)
    }
}

/// Das Standard-Layout eines neuen Dashboards: **eine große Such-Kachel** über die ganze Breite.
pub fn default_dashboard_layout() -> Layout {
    Layout {
        version: LAYOUT_VERSION,
        tiles: vec![Tile { id: 1, x: 0, y: 0, w: GRID_COLUMNS, h: 12, kind: TileKind::Search { jql: String::new(), columns: DEFAULT_SEARCH_COLUMNS.iter().map(|s| s.to_string()).collect(), title: String::new() } }],
    }
}

fn project_usable(conn: &Connection, id: i64) -> Res<bool> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM projects WHERE id = ?1 AND system = 0", [id], |r| r.get(0))?;
    Ok(n > 0)
}

/// Prüft ein Layout für ein Dashboard des Bereichs (`project_id` = Projekt des Dashboards, `None` = global).
pub fn validate_layout(conn: &Connection, project_id: Option<i64>, layout: &Layout) -> Res<()> {
    if layout.tiles.len() > MAX_TILES {
        return Err(ViewError::TooManyTiles { max: MAX_TILES });
    }
    let mut seen = std::collections::HashSet::new();
    for t in &layout.tiles {
        if !seen.insert(t.id) {
            return Err(ViewError::TileIdTwice { tile: t.id });
        }
    }
    for t in &layout.tiles {
        let fits = t.w >= 1 && t.h >= 1 && t.w <= GRID_COLUMNS && t.h <= MAX_TILE_HEIGHT && u64::from(t.x) + u64::from(t.w) <= u64::from(GRID_COLUMNS) && u64::from(t.y) + u64::from(t.h) <= u64::from(MAX_ROWS);
        if !fits {
            return Err(ViewError::TileGrid { tile: t.id });
        }
    }
    for (i, a) in layout.tiles.iter().enumerate() {
        for b in &layout.tiles[i + 1..] {
            if overlap(a, b) {
                return Err(ViewError::TileOverlap { a: a.id, b: b.id });
            }
        }
    }
    for t in &layout.tiles {
        let bad_config = || ViewError::TileConfig { tile: t.id };
        match &t.kind {
            TileKind::Search { jql, columns, title } => {
                if jql.chars().count() > MAX_JQL_CHARS || title.chars().count() > MAX_NAME_CHARS || columns.len() > MAX_TILE_COLUMNS {
                    return Err(bad_config());
                }
                if columns.iter().any(|c| c.trim().is_empty() || c.chars().count() > MAX_COLUMN_CHARS) {
                    return Err(bad_config());
                }
            }
            TileKind::Counts { jql, .. } => {
                if jql.chars().count() > MAX_JQL_CHARS {
                    return Err(bad_config());
                }
            }
            _ => {}
        }
        if let Some(p) = t.kind.project() {
            match p {
                Some(id) => {
                    if !project_usable(conn, *id)? {
                        return Err(ViewError::TileProject { tile: t.id });
                    }
                }
                None => {
                    if t.kind.needs_project() && project_id.is_none() {
                        return Err(ViewError::TileProject { tile: t.id });
                    }
                }
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ Dashboards

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DashboardScope {
    Project,
    Global,
}

impl DashboardScope {
    pub fn as_str(self) -> &'static str {
        match self {
            DashboardScope::Project => "project",
            DashboardScope::Global => "global",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Dashboard {
    pub id: i64,
    pub name: String,
    pub scope: DashboardScope,
    /// Das Projekt eines Projekt-Dashboards; bei globalen `None`.
    pub project_id: Option<i64>,
    pub position: i64,
    pub is_default: bool,
    pub layout: Layout,
}

#[derive(Debug, Clone)]
pub struct NewDashboard {
    pub name: String,
    /// `Some` = Dashboard dieses Projekts, `None` = global.
    pub project_id: Option<i64>,
    /// `None` = [`default_dashboard_layout`].
    pub layout: Option<Layout>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateDashboard {
    pub name: Option<String>,
    pub layout: Option<Layout>,
}

const COLS: &str = "id, name, scope, project_id, position, is_default, layout";

/// Liest ein Layout; ein unlesbares wird zum Standard-Layout (das Dashboard bleibt benutzbar).
pub fn parse_layout(text: &str) -> Layout {
    serde_json::from_str(text).unwrap_or_else(|_| default_dashboard_layout())
}

fn layout_json(l: &Layout) -> String {
    let mut l = l.clone();
    l.version = LAYOUT_VERSION;
    serde_json::to_string(&l).unwrap_or_else(|_| "{}".to_string())
}

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Dashboard> {
    let scope = if r.get::<_, String>(2)? == "project" { DashboardScope::Project } else { DashboardScope::Global };
    Ok(Dashboard { id: r.get(0)?, name: r.get(1)?, scope, project_id: r.get(3)?, position: r.get(4)?, is_default: r.get::<_, i64>(5)? != 0, layout: parse_layout(&r.get::<_, String>(6)?) })
}

/// `WHERE`-Teil und Parameter des Bereichs.
fn scope_sql(project_id: Option<i64>) -> (&'static str, Vec<i64>) {
    match project_id {
        Some(p) => ("project_id = ?1", vec![p]),
        None => ("project_id IS NULL", vec![]),
    }
}

pub fn get_dashboard(conn: &Connection, id: i64) -> Res<Dashboard> {
    conn.query_row(&format!("SELECT {COLS} FROM dashboards WHERE id = ?1"), [id], from_row).optional()?.ok_or_else(ViewError::not_found)
}

/// Die Dashboards genau dieses Bereichs (`None` = nur die globalen).
pub fn list_dashboards(conn: &Connection, project_id: Option<i64>) -> Res<Vec<Dashboard>> {
    let (w, p) = scope_sql(project_id);
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM dashboards WHERE {w} ORDER BY position, id"))?;
    Ok(st.query_map(rusqlite::params_from_iter(p.iter()), from_row)?.collect::<Result<Vec<_>, _>>()?)
}

/// Alle im Projekt wählbaren Dashboards (Wechsler): erst die des Projekts, dann die globalen.
pub fn list_available(conn: &Connection, project_id: i64) -> Res<Vec<Dashboard>> {
    let mut out = list_dashboards(conn, Some(project_id))?;
    out.extend(list_dashboards(conn, None)?);
    Ok(out)
}

/// Das Standard-Dashboard genau dieses Bereichs.
pub fn default_dashboard(conn: &Connection, project_id: Option<i64>) -> Res<Option<Dashboard>> {
    Ok(list_dashboards(conn, project_id)?.into_iter().find(|d| d.is_default))
}

/// Das beim Aufruf zu ladende Dashboard: im Projekt dessen Standard, sonst der globale Standard; `None`, wenn es keins gibt.
pub fn startup_dashboard(conn: &Connection, project_id: Option<i64>) -> Res<Option<Dashboard>> {
    if let Some(p) = project_id {
        if let Some(d) = default_dashboard(conn, Some(p))? {
            return Ok(Some(d));
        }
    }
    default_dashboard(conn, None)
}

fn check_scope(conn: &Connection, project_id: Option<i64>) -> Res<()> {
    if let Some(p) = project_id {
        editable(conn, p)?;
    }
    Ok(())
}

pub fn create_dashboard(conn: &Connection, n: &NewDashboard) -> Res<i64> {
    check_scope(conn, n.project_id)?;
    let name = clean_name(&n.name)?;
    let layout = n.layout.clone().unwrap_or_else(default_dashboard_layout);
    atomic(conn, || {
        validate_layout(conn, n.project_id, &layout)?;
        insert(conn, &name, n.project_id, &layout)
    })
}

fn insert(conn: &Connection, name: &str, project_id: Option<i64>, layout: &Layout) -> Res<i64> {
    let (w, p) = scope_sql(project_id);
    ensure_name_free(conn, "dashboards", w, &p, name, None)?;
    let pos = common::next_position(conn, "dashboards", w, &p)?;
    let scope = if project_id.is_some() { DashboardScope::Project } else { DashboardScope::Global };
    conn.execute("INSERT INTO dashboards (name, scope, project_id, position, is_default, layout) VALUES (?1, ?2, ?3, ?4, 0, ?5)", params![name, scope.as_str(), project_id, pos, layout_json(layout)])?;
    Ok(conn.last_insert_rowid())
}

pub fn update_dashboard(conn: &Connection, id: i64, u: &UpdateDashboard) -> Res<()> {
    let cur = get_dashboard(conn, id)?;
    let name = match &u.name {
        Some(s) => clean_name(s)?,
        None => cur.name.clone(),
    };
    atomic(conn, || {
        let layout = match &u.layout {
            Some(l) => {
                validate_layout(conn, cur.project_id, l)?;
                l.clone()
            }
            None => cur.layout.clone(),
        };
        let (w, p) = scope_sql(cur.project_id);
        ensure_name_free(conn, "dashboards", w, &p, &name, Some(id))?;
        conn.execute("UPDATE dashboards SET name = ?1, layout = ?2 WHERE id = ?3", params![name, layout_json(&layout), id])?;
        Ok(())
    })
}

/// Macht das Dashboard zum Standard seines Bereichs; das bisherige Standard-Dashboard des Bereichs wird dabei abgelöst (atomar).
pub fn set_default(conn: &Connection, id: i64) -> Res<()> {
    let d = get_dashboard(conn, id)?;
    atomic(conn, || {
        conn.execute("UPDATE dashboards SET is_default = 0 WHERE COALESCE(project_id, 0) = COALESCE(?1, 0) AND is_default = 1", params![d.project_id])?;
        conn.execute("UPDATE dashboards SET is_default = 1 WHERE id = ?1", [id])?;
        Ok(())
    })
}

/// Nimmt dem Bereich das Standard-Dashboard (es gibt dann keins).
pub fn clear_default(conn: &Connection, project_id: Option<i64>) -> Res<()> {
    conn.execute("UPDATE dashboards SET is_default = 0 WHERE COALESCE(project_id, 0) = COALESCE(?1, 0)", params![project_id])?;
    Ok(())
}

/// Liefert das Standard-Dashboard des Bereichs und legt bei Bedarf eines an: gibt es keins, wird das erste zum Standard;
/// gibt es gar keins, entsteht „Dashboard“ mit dem Standard-Layout (eine große Such-Kachel).
pub fn ensure_default_dashboard(conn: &Connection, project_id: Option<i64>, lang: Lang) -> Res<Dashboard> {
    atomic(conn, || {
        check_scope(conn, project_id)?;
        if let Some(d) = default_dashboard(conn, project_id)? {
            return Ok(d);
        }
        let id = match list_dashboards(conn, project_id)?.first() {
            Some(first) => first.id,
            None => create_dashboard(conn, &NewDashboard { name: lang.t("Dashboard", "Dashboard").to_string(), project_id, layout: None })?,
        };
        set_default(conn, id)?;
        get_dashboard(conn, id)
    })
}

/// Neue Reihenfolge der Dashboards eines Bereichs; `ids` muss genau dessen Dashboards enthalten.
pub fn reorder_dashboards(conn: &Connection, project_id: Option<i64>, ids: &[i64]) -> Res<()> {
    let (w, p) = scope_sql(project_id);
    Ok(common::reorder(conn, "dashboards", w, &p, ids)?)
}

/// Löscht das Dashboard. War es der Standard, gibt es im Bereich danach keinen mehr (siehe [`ensure_default_dashboard`]).
pub fn delete_dashboard(conn: &Connection, id: i64) -> Res<()> {
    get_dashboard(conn, id)?;
    conn.execute("DELETE FROM dashboards WHERE id = ?1", [id])?;
    Ok(())
}

/// Dupliziert ein Dashboard in denselben Bereich (Name „Copy of X“ / „Kopie von X“, nicht Standard, gleiches Layout).
pub fn duplicate_dashboard(conn: &Connection, id: i64, lang: Lang) -> Res<i64> {
    let d = get_dashboard(conn, id)?;
    atomic(conn, || {
        let (w, p) = scope_sql(d.project_id);
        let taken = names_in(conn, "dashboards", w, &p)?;
        insert(conn, &copy_name(lang, &d.name, &taken), d.project_id, &d.layout)
    })
}

/// Räumt Kacheln auf, deren Projekt es nicht mehr gibt (Sprint/Burndown: Kachel entfällt, Zeiterfassung: alle Projekte).
/// Liefert die Zahl der geänderten Dashboards. Nach dem Löschen eines Projekts aufrufen.
pub fn prune_dashboard_refs(conn: &Connection) -> Res<usize> {
    atomic(conn, || {
        let mut changed = 0;
        let all: Vec<Dashboard> = {
            let mut st = conn.prepare(&format!("SELECT {COLS} FROM dashboards ORDER BY id"))?;
            st.query_map([], from_row)?.collect::<Result<_, _>>()?
        };
        for d in all {
            let mut db_err: Option<ViewError> = None;
            let (layout, dropped) = d.layout.clone().map_projects(&mut |p: i64| match project_usable(conn, p) {
                Ok(true) => Some(p),
                Ok(false) => None,
                Err(e) => {
                    db_err = Some(e);
                    None
                }
            });
            if let Some(e) = db_err {
                return Err(e);
            }
            if dropped > 0 || layout != d.layout {
                conn.execute("UPDATE dashboards SET layout = ?1 WHERE id = ?2", params![layout_json(&layout), d.id])?;
                changed += 1;
            }
        }
        Ok(changed)
    })
}

// ------------------------------------------------------------------ Export / Import

/// Export-Dokument eines Dashboards. Projekte stehen als Schlüssel (`PAN`), nicht als Kennung.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DashboardDoc {
    pub format: String,
    pub version: u32,
    pub name: String,
    pub scope: DashboardScope,
    /// Schlüssel des Projekts eines Projekt-Dashboards.
    #[serde(default)]
    pub project: Option<String>,
    pub layout: LayoutG<String>,
}

impl DashboardDoc {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// Liest ein Dokument und prüft Format und Version.
    pub fn from_json(text: &str) -> Res<DashboardDoc> {
        let text = text.trim_start_matches('\u{feff}');
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| ViewError::Doc(DocProblem::Json(e.to_string())))?;
        if v.get("format").and_then(serde_json::Value::as_str) != Some(FORMAT) {
            return Err(ViewError::Doc(DocProblem::Format));
        }
        let found = v.get("version").and_then(serde_json::Value::as_u64).unwrap_or(0) as u32;
        if found > DOC_VERSION {
            return Err(ViewError::Doc(DocProblem::Version { found, supported: DOC_VERSION }));
        }
        serde_json::from_value(v).map_err(|e| ViewError::Doc(DocProblem::Json(e.to_string())))
    }
}

/// Exportiert das Dashboard. Kacheln, die ein nicht mehr vorhandenes Projekt brauchen, entfallen.
pub fn export_dashboard(conn: &Connection, id: i64) -> Res<DashboardDoc> {
    let d = get_dashboard(conn, id)?;
    let mut db_err: Option<ViewError> = None;
    let (layout, _) = d.layout.map_projects(&mut |p: i64| match conn.query_row("SELECT key FROM projects WHERE id = ?1 AND system = 0", [p], |r| r.get::<_, String>(0)).optional() {
        Ok(k) => k,
        Err(e) => {
            db_err = Some(e.into());
            None
        }
    });
    if let Some(e) = db_err {
        return Err(e);
    }
    let project = match d.project_id {
        Some(p) => Some(conn.query_row("SELECT key FROM projects WHERE id = ?1", [p], |r| r.get::<_, String>(0))?),
        None => None,
    };
    Ok(DashboardDoc { format: FORMAT.to_string(), version: DOC_VERSION, name: d.name, scope: d.scope, project, layout })
}

/// Wohin der Import das Dashboard legt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportTarget {
    /// Wie im Dokument: global oder das Projekt mit dem dort genannten Schlüssel (unbekannt → Fehler).
    Document,
    /// Als globales Dashboard.
    Global,
    /// Als Dashboard dieses Projekts.
    Project(i64),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportReport {
    /// Projekt-Schlüssel aus dem Dokument, die es hier nicht gibt (ohne Doppelte).
    pub unknown_projects: Vec<String>,
    /// Kacheln, die entfielen, weil sie ein unbekanntes Projekt brauchen (Sprint, Burndown).
    pub dropped_tiles: usize,
    /// Der Name war schon vergeben und wurde mit „ (2)“ o. ä. ergänzt.
    pub renamed: bool,
}

impl ImportReport {
    /// Hinweis für den Nutzer, wenn etwas nicht übernommen werden konnte; sonst `None`.
    pub fn message(&self, lang: Lang) -> Option<String> {
        if self.unknown_projects.is_empty() && self.dropped_tiles == 0 {
            return None;
        }
        let list = self.unknown_projects.join(", ");
        Some(match lang {
            Lang::De => format!("Unbekannte Projekte: {list}. Entfallene Kacheln: {}.", self.dropped_tiles),
            Lang::En => format!("Unknown projects: {list}. Tiles left out: {}.", self.dropped_tiles),
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub id: i64,
    pub report: ImportReport,
}

/// Importiert das Dokument als **neues** Dashboard (nie Standard); Bestehendes bleibt unangetastet. Unbekannte Projekte werden
/// gemeldet: Sprint-/Burndown-Kacheln dafür entfallen, bei der Zeiterfassung gilt dann „alle Projekte“. Beim Ziel „global“
/// übernehmen Sprint-/Burndown-Kacheln ohne Projekt das Projekt des Dokuments (sonst entfallen sie). Atomar.
pub fn import_dashboard(conn: &Connection, doc: &DashboardDoc, target: ImportTarget) -> Res<Imported> {
    if doc.format != FORMAT {
        return Err(ViewError::Doc(DocProblem::Format));
    }
    if doc.version > DOC_VERSION {
        return Err(ViewError::Doc(DocProblem::Version { found: doc.version, supported: DOC_VERSION }));
    }
    let name = clean_name(&doc.name)?;
    atomic(conn, || {
        let project_id: Option<i64> = match target {
            ImportTarget::Global => None,
            ImportTarget::Project(p) => Some(p),
            ImportTarget::Document => match (doc.scope, &doc.project) {
                (DashboardScope::Global, _) => None,
                (DashboardScope::Project, Some(key)) => Some(project_by_key(conn, key).map_err(|e| if e == PlanError::NotFound { ViewError::Doc(DocProblem::Reference(key.clone())) } else { e.into() })?.id),
                (DashboardScope::Project, None) => return Err(ViewError::Doc(DocProblem::Reference("project".to_string()))),
            },
        };
        check_scope(conn, project_id)?;
        let mut report = ImportReport::default();
        // Beim Ziel „global“ bekommen projektlose Sprint-/Burndown-Kacheln das Projekt des Dokuments.
        let mut layout = doc.layout.clone();
        if project_id.is_none() {
            layout.tiles.iter_mut().for_each(|t| {
                if t.kind.needs_project() && matches!(t.kind.project(), Some(None)) {
                    t.kind.set_project(doc.project.clone());
                }
            });
        }
        let mut db_err: Option<ViewError> = None;
        let (mapped, mut dropped) = layout.map_projects(&mut |key: String| match project_by_key(conn, &key) {
            Ok(p) if !p.system => Some(p.id),
            Ok(_) => None,
            Err(e) => {
                if e != PlanError::NotFound {
                    db_err = Some(e.into());
                }
                if !report.unknown_projects.contains(&key) {
                    report.unknown_projects.push(key);
                }
                None
            }
        });
        if let Some(e) = db_err {
            return Err(e);
        }
        // Sprint-/Burndown-Kacheln ohne Projekt im globalen Ziel (Dokument ohne Projekt) entfallen ebenfalls.
        let mut mapped = mapped;
        if project_id.is_none() {
            let before = mapped.tiles.len();
            mapped.tiles.retain(|t| !(t.kind.needs_project() && matches!(t.kind.project(), Some(None))));
            dropped += before - mapped.tiles.len();
        }
        report.dropped_tiles = dropped;
        validate_layout(conn, project_id, &mapped)?;
        let (w, p) = scope_sql(project_id);
        let taken = names_in(conn, "dashboards", w, &p)?;
        let final_name = numbered_name(&name, &taken);
        report.renamed = final_name != name;
        let id = insert(conn, &final_name, project_id, &mapped)?;
        Ok(Imported { id, report })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::projects::delete_project;
    use crate::planning::views::testutil::{db, project};
    use serde_json::json;

    fn search(jql: &str) -> TileKind<i64> {
        TileKind::Search { jql: jql.into(), columns: vec!["key".into()], title: String::new() }
    }

    fn tile(id: u32, x: u32, y: u32, w: u32, h: u32) -> Tile {
        Tile { id, x, y, w, h, kind: TileKind::Favorites }
    }

    fn layout(tiles: Vec<Tile>) -> Layout {
        Layout { version: LAYOUT_VERSION, tiles }
    }

    fn new(name: &str, project_id: Option<i64>) -> NewDashboard {
        NewDashboard { name: name.into(), project_id, layout: None }
    }

    // ---------------------------------------------------------------- Layout-Format

    #[test]
    fn default_layout_is_one_big_search_tile() {
        let l = default_dashboard_layout();
        assert_eq!(l.tiles.len(), 1);
        let t = &l.tiles[0];
        assert_eq!((t.x, t.y, t.w), (0, 0, GRID_COLUMNS));
        assert!(t.h >= 8);
        assert!(matches!(&t.kind, TileKind::Search { jql, columns, .. } if jql.is_empty() && columns.len() == DEFAULT_SEARCH_COLUMNS.len()));
        let c = db();
        validate_layout(&c, None, &l).unwrap();
        validate_layout(&c, Some(project(&c, "AA", false)), &l).unwrap();
    }

    #[test]
    fn layout_json_has_the_documented_shape() {
        let l = Layout {
            version: 1,
            tiles: vec![
                Tile { id: 1, x: 0, y: 0, w: 6, h: 4, kind: TileKind::Search { jql: "type = Defect".into(), columns: vec!["key".into(), "status".into()], title: "Bugs".into() } },
                Tile { id: 2, x: 6, y: 0, w: 6, h: 4, kind: TileKind::Counts { jql: String::new(), group_by: CountGroup::Priority, chart: ChartKind::Pie } },
                Tile { id: 3, x: 0, y: 4, w: 4, h: 4, kind: TileKind::Sprint { project_id: Some(7) } },
                Tile { id: 4, x: 4, y: 4, w: 4, h: 4, kind: TileKind::TimeTracking { range: TimeRange::Week, project_id: None } },
                Tile { id: 5, x: 8, y: 4, w: 4, h: 4, kind: TileKind::Projects },
            ],
        };
        let v = serde_json::to_value(&l).unwrap();
        assert_eq!(v["version"], 1);
        assert_eq!(v["tiles"][0], json!({"id": 1, "x": 0, "y": 0, "w": 6, "h": 4, "kind": "search", "config": {"jql": "type = Defect", "columns": ["key", "status"], "title": "Bugs"}}));
        assert_eq!(v["tiles"][1]["config"], json!({"jql": "", "group_by": "priority", "chart": "pie"}));
        assert_eq!(v["tiles"][2]["config"], json!({"project_id": 7}));
        assert_eq!(v["tiles"][3]["kind"], "time_tracking");
        assert_eq!(v["tiles"][4]["kind"], "projects");
        assert!(v["tiles"][4].get("config").is_none());
        let back: Layout = serde_json::from_value(v).unwrap();
        assert_eq!(back, l);
    }

    #[test]
    fn every_tile_kind_round_trips_through_json() {
        let kinds: Vec<TileKind<i64>> = vec![
            search("a"),
            TileKind::Sprint { project_id: None },
            TileKind::Burndown { project_id: Some(3) },
            TileKind::Counts { jql: "x".into(), group_by: CountGroup::Status, chart: ChartKind::Bar },
            TileKind::Counts { jql: "x".into(), group_by: CountGroup::Type, chart: ChartKind::Pie },
            TileKind::TimeTracking { range: TimeRange::Day, project_id: Some(2) },
            TileKind::Favorites,
            TileKind::Flagged,
            TileKind::Projects,
        ];
        for (i, k) in kinds.into_iter().enumerate() {
            let t = Tile { id: i as u32 + 1, x: 0, y: 0, w: 4, h: 4, kind: k };
            let s = serde_json::to_string(&t).unwrap();
            assert_eq!(serde_json::from_str::<Tile>(&s).unwrap(), t, "{s}");
            assert!(t.kind.default_size().0 <= GRID_COLUMNS);
            assert!(!t.kind.name().is_empty());
        }
    }

    #[test]
    fn missing_config_fields_get_defaults() {
        let t: Tile = serde_json::from_str(r#"{"id":1,"x":0,"y":0,"w":2,"h":2,"kind":"search","config":{}}"#).unwrap();
        assert!(matches!(t.kind, TileKind::Search { ref jql, .. } if jql.is_empty()));
        let l: Layout = serde_json::from_str("{}").unwrap();
        assert!(l.tiles.is_empty());
        assert_eq!(l.version, LAYOUT_VERSION);
        assert!(serde_json::from_str::<Tile>(r#"{"id":1,"x":0,"y":0,"w":2,"h":2,"kind":"unbekannt"}"#).is_err());
    }

    // ---------------------------------------------------------------- Geometrie

    #[test]
    fn clamp_forces_tiles_into_the_grid() {
        let mut l = layout(vec![tile(1, 10, 0, 6, 2), tile(2, 0, 0, 0, 0), tile(3, 0, 5000, 99, 99)]);
        l.clamp();
        assert_eq!((l.tiles[0].x, l.tiles[0].w), (6, 6));
        assert_eq!((l.tiles[1].w, l.tiles[1].h), (1, 1));
        assert_eq!((l.tiles[2].w, l.tiles[2].h), (GRID_COLUMNS, MAX_TILE_HEIGHT));
        assert_eq!(l.tiles[2].y, MAX_ROWS - MAX_TILE_HEIGHT);
    }

    #[test]
    fn normalize_pushes_overlaps_down_and_keeps_free_tiles() {
        let mut l = layout(vec![tile(1, 0, 0, 6, 4), tile(2, 3, 2, 6, 4), tile(3, 6, 10, 3, 3)]);
        l.normalize();
        let c = db();
        validate_layout(&c, None, &l).unwrap();
        assert_eq!((l.tile(1).unwrap().x, l.tile(1).unwrap().y), (0, 0), "die obere linke bleibt");
        assert_eq!((l.tile(2).unwrap().x, l.tile(2).unwrap().y), (3, 4), "die überlappende rutscht darunter");
        assert_eq!((l.tile(3).unwrap().x, l.tile(3).unwrap().y), (6, 10), "freie Kacheln bleiben");
        // gültiges Layout bleibt unverändert
        let before = l.clone();
        l.normalize();
        assert_eq!(l, before);
    }

    #[test]
    fn normalize_resolves_chains_and_duplicate_ids() {
        let mut l = layout(vec![tile(1, 0, 0, 12, 3), tile(1, 0, 0, 12, 3), tile(1, 0, 1, 12, 3)]);
        l.normalize();
        let ids: Vec<u32> = l.tiles.iter().map(|t| t.id).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 3, "{ids:?}");
        let c = db();
        validate_layout(&c, None, &l).unwrap();
        let mut ys: Vec<u32> = l.tiles.iter().map(|t| t.y).collect();
        ys.sort();
        assert_eq!(ys, vec![0, 3, 6]);
    }

    #[test]
    fn pack_closes_gaps_upwards() {
        let mut l = layout(vec![tile(1, 0, 5, 6, 2), tile(2, 6, 9, 6, 2), tile(3, 0, 20, 12, 2)]);
        l.pack();
        assert_eq!((l.tile(1).unwrap().y, l.tile(2).unwrap().y), (0, 0));
        assert_eq!(l.tile(3).unwrap().y, 2);
        let c = db();
        validate_layout(&c, None, &l).unwrap();
        // schon verdichtet: ändert sich nichts
        let before = l.clone();
        l.pack();
        assert_eq!(l, before);
    }

    #[test]
    fn moving_pins_the_moved_tile_and_others_yield() {
        let mut l = layout(vec![tile(1, 0, 0, 6, 4), tile(2, 6, 0, 6, 4)]);
        assert!(l.move_tile(2, 0, 0));
        assert_eq!((l.tile(2).unwrap().x, l.tile(2).unwrap().y), (0, 0), "die bewegte Kachel behält ihren Platz");
        assert_eq!(l.tile(1).unwrap().y, 4, "die andere weicht aus");
        assert!(!l.move_tile(99, 0, 0));
        // außerhalb des Rasters wird zurechtgerückt
        assert!(l.move_tile(2, 100, 0));
        assert_eq!(l.tile(2).unwrap().x, GRID_COLUMNS - 6);
        let c = db();
        validate_layout(&c, None, &l).unwrap();
    }

    #[test]
    fn resizing_pushes_neighbours_down() {
        let mut l = layout(vec![tile(1, 0, 0, 6, 4), tile(2, 6, 0, 6, 4)]);
        assert!(l.resize_tile(1, 12, 4));
        assert_eq!((l.tile(1).unwrap().w, l.tile(1).unwrap().y), (12, 0));
        assert_eq!(l.tile(2).unwrap().y, 4);
        assert!(l.resize_tile(1, 0, 0));
        assert_eq!((l.tile(1).unwrap().w, l.tile(1).unwrap().h), (1, 1));
        assert!(!l.resize_tile(42, 1, 1));
    }

    #[test]
    fn free_spot_and_add_tile() {
        let mut l = layout(vec![tile(1, 0, 0, 8, 4)]);
        assert_eq!(l.free_spot(4, 4), (8, 0));
        assert_eq!(l.free_spot(6, 2), (0, 4));
        let id = l.add_tile(TileKind::Flagged, None).unwrap();
        assert_eq!(id, 2);
        let t = l.tile(2).unwrap();
        assert_eq!((t.w, t.h), TileKind::<i64>::Flagged.default_size());
        // 4 breit passt neben die erste (x 8..12), liegt oben
        assert_eq!((t.x, t.y), (8, 0));
        let id3 = l.add_tile(TileKind::Favorites, Some((12, 3))).unwrap();
        assert_eq!(l.tile(id3).unwrap().y, 8, "unter alles");
        let c = db();
        validate_layout(&c, None, &l).unwrap();
        assert!(l.remove_tile(1));
        assert!(!l.remove_tile(1));
        assert_eq!(l.next_id(), 4);
        // Obergrenze
        let mut full = Layout::default();
        for _ in 0..MAX_TILES {
            assert!(full.add_tile(TileKind::Projects, Some((1, 1))).is_some());
        }
        assert!(full.add_tile(TileKind::Projects, Some((1, 1))).is_none());
        validate_layout(&c, None, &full).unwrap();
    }

    // ---------------------------------------------------------------- Prüfung

    #[test]
    fn validate_checks_grid_overlap_and_ids() {
        let c = db();
        let v = |l: Layout| validate_layout(&c, None, &l);
        assert_eq!(v(layout(vec![])), Ok(()));
        assert_eq!(v(layout(vec![tile(1, 0, 0, 12, 1)])), Ok(()));
        assert_eq!(v(layout(vec![tile(1, 1, 0, 12, 1)])), Err(ViewError::TileGrid { tile: 1 }));
        assert_eq!(v(layout(vec![tile(1, 0, 0, 0, 1)])), Err(ViewError::TileGrid { tile: 1 }));
        assert_eq!(v(layout(vec![tile(1, 0, 0, 1, 0)])), Err(ViewError::TileGrid { tile: 1 }));
        assert_eq!(v(layout(vec![tile(1, 0, 0, 13, 1)])), Err(ViewError::TileGrid { tile: 1 }));
        assert_eq!(v(layout(vec![tile(1, 0, 0, 1, MAX_TILE_HEIGHT + 1)])), Err(ViewError::TileGrid { tile: 1 }));
        assert_eq!(v(layout(vec![tile(1, 0, MAX_ROWS - 1, 1, 2)])), Err(ViewError::TileGrid { tile: 1 }));
        assert_eq!(v(layout(vec![tile(1, u32::MAX, 0, 2, 1)])), Err(ViewError::TileGrid { tile: 1 }));
        assert_eq!(v(layout(vec![tile(1, 0, 0, 4, 4), tile(2, 3, 3, 4, 4)])), Err(ViewError::TileOverlap { a: 1, b: 2 }));
        assert_eq!(v(layout(vec![tile(1, 0, 0, 4, 4), tile(2, 4, 0, 4, 4), tile(3, 0, 4, 12, 1)])), Ok(()), "Berühren ist keine Überlappung");
        assert_eq!(v(layout(vec![tile(1, 0, 0, 2, 2), tile(1, 2, 0, 2, 2)])), Err(ViewError::TileIdTwice { tile: 1 }));
        let many = layout((0..=MAX_TILES as u32).map(|i| tile(i + 1, 0, i, 1, 1)).collect());
        assert_eq!(v(many), Err(ViewError::TooManyTiles { max: MAX_TILES }));
    }

    #[test]
    fn validate_checks_tile_settings() {
        let c = db();
        let one = |kind| validate_layout(&c, None, &layout(vec![Tile { id: 1, x: 0, y: 0, w: 4, h: 4, kind }]));
        assert_eq!(one(TileKind::Search { jql: "j".repeat(MAX_JQL_CHARS + 1), columns: vec![], title: String::new() }), Err(ViewError::TileConfig { tile: 1 }));
        assert_eq!(one(TileKind::Search { jql: String::new(), columns: vec![], title: "t".repeat(81) }), Err(ViewError::TileConfig { tile: 1 }));
        assert_eq!(one(TileKind::Search { jql: String::new(), columns: vec![" ".into()], title: String::new() }), Err(ViewError::TileConfig { tile: 1 }));
        assert_eq!(one(TileKind::Search { jql: String::new(), columns: vec!["c".repeat(41)], title: String::new() }), Err(ViewError::TileConfig { tile: 1 }));
        assert_eq!(one(TileKind::Search { jql: String::new(), columns: (0..=MAX_TILE_COLUMNS).map(|i| format!("c{i}")).collect(), title: String::new() }), Err(ViewError::TileConfig { tile: 1 }));
        assert_eq!(one(TileKind::Counts { jql: "j".repeat(MAX_JQL_CHARS + 1), group_by: CountGroup::Status, chart: ChartKind::Bar }), Err(ViewError::TileConfig { tile: 1 }));
        assert_eq!(one(search("ok")), Ok(()));
    }

    #[test]
    fn validate_checks_project_references() {
        let c = db();
        let p = project(&c, "AA", false);
        let sys = crate::planning::projects::project_by_key(&c, "TODO").unwrap().id;
        let one = |scope: Option<i64>, kind| validate_layout(&c, scope, &layout(vec![Tile { id: 1, x: 0, y: 0, w: 4, h: 4, kind }]));
        // global: Sprint/Burndown brauchen ein Projekt
        assert_eq!(one(None, TileKind::Sprint { project_id: None }), Err(ViewError::TileProject { tile: 1 }));
        assert_eq!(one(None, TileKind::Burndown { project_id: None }), Err(ViewError::TileProject { tile: 1 }));
        assert_eq!(one(None, TileKind::Sprint { project_id: Some(p) }), Ok(()));
        // im Projekt-Dashboard ist „kein Projekt“ = das eigene
        assert_eq!(one(Some(p), TileKind::Sprint { project_id: None }), Ok(()));
        assert_eq!(one(Some(p), TileKind::Burndown { project_id: None }), Ok(()));
        // unbekannt oder System-Projekt
        assert_eq!(one(None, TileKind::Sprint { project_id: Some(9999) }), Err(ViewError::TileProject { tile: 1 }));
        assert_eq!(one(None, TileKind::Burndown { project_id: Some(sys) }), Err(ViewError::TileProject { tile: 1 }));
        // Zeiterfassung: ohne Projekt überall erlaubt
        assert_eq!(one(None, TileKind::TimeTracking { range: TimeRange::Day, project_id: None }), Ok(()));
        assert_eq!(one(None, TileKind::TimeTracking { range: TimeRange::Day, project_id: Some(9999) }), Err(ViewError::TileProject { tile: 1 }));
    }

    // ---------------------------------------------------------------- Datenbank

    #[test]
    fn creates_global_and_project_dashboards() {
        let c = db();
        let p = project(&c, "AA", false);
        let g = create_dashboard(&c, &new("  Mein   Dashboard ", None)).unwrap();
        let d = create_dashboard(&c, &NewDashboard { name: "Projekt".into(), project_id: Some(p), layout: Some(layout(vec![tile(1, 0, 0, 12, 2)])) }).unwrap();
        let gd = get_dashboard(&c, g).unwrap();
        assert_eq!((gd.name.as_str(), gd.scope, gd.project_id, gd.is_default, gd.position), ("Mein Dashboard", DashboardScope::Global, None, false, 0));
        assert_eq!(gd.layout, default_dashboard_layout());
        let pd = get_dashboard(&c, d).unwrap();
        assert_eq!((pd.scope, pd.project_id), (DashboardScope::Project, Some(p)));
        assert_eq!(pd.layout.tiles.len(), 1);
        assert_eq!(list_dashboards(&c, None).unwrap().len(), 1);
        assert_eq!(list_dashboards(&c, Some(p)).unwrap().len(), 1);
        assert_eq!(list_available(&c, p).unwrap().iter().map(|d| d.id).collect::<Vec<_>>(), vec![d, g]);
        assert_eq!(get_dashboard(&c, 9999), Err(ViewError::not_found()));
    }

    #[test]
    fn rejects_bad_input() {
        let c = db();
        let p = project(&c, "AA", false);
        let sys = crate::planning::projects::project_by_key(&c, "TODO").unwrap().id;
        assert_eq!(create_dashboard(&c, &new(" ", None)), Err(PlanError::NameEmpty.into()));
        assert_eq!(create_dashboard(&c, &new(&"n".repeat(81), None)), Err(PlanError::NameTooLong { max: 80 }.into()));
        assert_eq!(create_dashboard(&c, &new("A", Some(9999))), Err(ViewError::not_found()));
        assert_eq!(create_dashboard(&c, &new("A", Some(sys))), Err(PlanError::SystemProtected.into()));
        let overlapping = Some(layout(vec![tile(1, 0, 0, 4, 4), tile(2, 0, 0, 4, 4)]));
        assert_eq!(create_dashboard(&c, &NewDashboard { name: "A".into(), project_id: Some(p), layout: overlapping }), Err(ViewError::TileOverlap { a: 1, b: 2 }));
        assert!(list_dashboards(&c, Some(p)).unwrap().is_empty());
    }

    #[test]
    fn names_are_unique_per_scope() {
        let c = db();
        let (p, q) = (project(&c, "AA", false), project(&c, "BB", false));
        create_dashboard(&c, &new("Team", Some(p))).unwrap();
        assert_eq!(create_dashboard(&c, &new("team", Some(p))), Err(PlanError::NameTaken.into()));
        assert!(create_dashboard(&c, &new("Team", Some(q))).is_ok());
        assert!(create_dashboard(&c, &new("Team", None)).is_ok());
        assert_eq!(create_dashboard(&c, &new("TEAM", None)), Err(PlanError::NameTaken.into()));
    }

    #[test]
    fn updates_name_and_layout() {
        let c = db();
        let p = project(&c, "AA", false);
        let a = create_dashboard(&c, &new("A", Some(p))).unwrap();
        let b = create_dashboard(&c, &new("B", Some(p))).unwrap();
        let mut l = default_dashboard_layout();
        l.tiles[0].w = 6;
        l.add_tile(TileKind::Flagged, None).unwrap();
        update_dashboard(&c, a, &UpdateDashboard { layout: Some(l.clone()), ..Default::default() }).unwrap();
        assert_eq!(get_dashboard(&c, a).unwrap().layout, l);
        assert_eq!(get_dashboard(&c, a).unwrap().name, "A");
        update_dashboard(&c, a, &UpdateDashboard { name: Some("a2".into()), ..Default::default() }).unwrap();
        assert_eq!(get_dashboard(&c, a).unwrap().layout, l);
        assert_eq!(update_dashboard(&c, a, &UpdateDashboard { name: Some("b".into()), ..Default::default() }), Err(PlanError::NameTaken.into()));
        // ungültiges Layout ändert nichts (auch den Namen nicht)
        let bad = layout(vec![tile(1, 0, 0, 20, 1)]);
        assert_eq!(update_dashboard(&c, b, &UpdateDashboard { name: Some("neu".into()), layout: Some(bad) }), Err(ViewError::TileGrid { tile: 1 }));
        assert_eq!(get_dashboard(&c, b).unwrap().name, "B");
        // Sprint-Kachel ohne Projekt: im Projekt-Dashboard in Ordnung, im globalen nicht
        let sprint = layout(vec![Tile { id: 1, x: 0, y: 0, w: 4, h: 4, kind: TileKind::Sprint { project_id: None } }]);
        update_dashboard(&c, b, &UpdateDashboard { layout: Some(sprint.clone()), ..Default::default() }).unwrap();
        let g = create_dashboard(&c, &new("G", None)).unwrap();
        assert_eq!(update_dashboard(&c, g, &UpdateDashboard { layout: Some(sprint), ..Default::default() }), Err(ViewError::TileProject { tile: 1 }));
        assert_eq!(update_dashboard(&c, 9999, &UpdateDashboard::default()), Err(ViewError::not_found()));
    }

    #[test]
    fn at_most_one_default_per_scope() {
        let c = db();
        let p = project(&c, "AA", false);
        let (g1, g2) = (create_dashboard(&c, &new("G1", None)).unwrap(), create_dashboard(&c, &new("G2", None)).unwrap());
        let (p1, p2) = (create_dashboard(&c, &new("P1", Some(p))).unwrap(), create_dashboard(&c, &new("P2", Some(p))).unwrap());
        assert_eq!(default_dashboard(&c, None).unwrap(), None);
        set_default(&c, g1).unwrap();
        set_default(&c, p1).unwrap();
        assert_eq!(default_dashboard(&c, None).unwrap().unwrap().id, g1);
        assert_eq!(default_dashboard(&c, Some(p)).unwrap().unwrap().id, p1);
        set_default(&c, g2).unwrap();
        assert_eq!(default_dashboard(&c, None).unwrap().unwrap().id, g2);
        assert!(!get_dashboard(&c, g1).unwrap().is_default);
        assert_eq!(default_dashboard(&c, Some(p)).unwrap().unwrap().id, p1, "der andere Bereich bleibt");
        set_default(&c, p2).unwrap();
        set_default(&c, p2).unwrap();
        let n: i64 = c.query_row("SELECT COUNT(*) FROM dashboards WHERE is_default = 1", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 2);
        // Der Index verhindert ein zweites Standard-Dashboard auch an der Funktion vorbei
        assert!(c.execute("UPDATE dashboards SET is_default = 1 WHERE id = ?1", [g1]).is_err());
        clear_default(&c, None).unwrap();
        assert_eq!(default_dashboard(&c, None).unwrap(), None);
        assert!(default_dashboard(&c, Some(p)).unwrap().is_some());
        assert_eq!(set_default(&c, 9999), Err(ViewError::not_found()));
    }

    #[test]
    fn startup_dashboard_falls_back_to_the_global_default() {
        let c = db();
        let p = project(&c, "AA", false);
        assert_eq!(startup_dashboard(&c, Some(p)).unwrap(), None);
        let g = create_dashboard(&c, &new("G", None)).unwrap();
        set_default(&c, g).unwrap();
        assert_eq!(startup_dashboard(&c, Some(p)).unwrap().unwrap().id, g);
        assert_eq!(startup_dashboard(&c, None).unwrap().unwrap().id, g);
        let d = create_dashboard(&c, &new("P", Some(p))).unwrap();
        set_default(&c, d).unwrap();
        assert_eq!(startup_dashboard(&c, Some(p)).unwrap().unwrap().id, d);
        assert_eq!(startup_dashboard(&c, None).unwrap().unwrap().id, g);
    }

    #[test]
    fn ensure_default_creates_or_promotes() {
        let c = db();
        let p = project(&c, "AA", false);
        let d = ensure_default_dashboard(&c, Some(p), Lang::De).unwrap();
        assert!(d.is_default);
        assert_eq!(d.layout, default_dashboard_layout());
        assert_eq!(ensure_default_dashboard(&c, Some(p), Lang::En).unwrap().id, d.id);
        assert_eq!(list_dashboards(&c, Some(p)).unwrap().len(), 1);
        // vorhandene Dashboards ohne Standard: das erste wird Standard, es entsteht keins dazu
        let a = create_dashboard(&c, &new("A", None)).unwrap();
        create_dashboard(&c, &new("B", None)).unwrap();
        assert_eq!(ensure_default_dashboard(&c, None, Lang::En).unwrap().id, a);
        assert_eq!(list_dashboards(&c, None).unwrap().len(), 2);
        // Wird der Standard gelöscht, gibt es keinen; ensure macht wieder einen
        delete_dashboard(&c, a).unwrap();
        assert_eq!(default_dashboard(&c, None).unwrap(), None);
        assert!(ensure_default_dashboard(&c, None, Lang::En).unwrap().is_default);
        assert_eq!(ensure_default_dashboard(&c, Some(9999), Lang::En), Err(ViewError::not_found()));
    }

    #[test]
    fn reorders_deletes_and_cascades() {
        let c = db();
        let p = project(&c, "AA", false);
        let ids: Vec<i64> = ["A", "B", "C"].iter().map(|n| create_dashboard(&c, &new(n, Some(p))).unwrap()).collect();
        reorder_dashboards(&c, Some(p), &[ids[1], ids[2], ids[0]]).unwrap();
        assert_eq!(list_dashboards(&c, Some(p)).unwrap().iter().map(|d| d.name.clone()).collect::<Vec<_>>(), vec!["B", "C", "A"]);
        assert_eq!(reorder_dashboards(&c, Some(p), &[ids[0]]), Err(PlanError::ReorderMismatch.into()));
        assert_eq!(reorder_dashboards(&c, None, &[ids[0]]), Err(PlanError::ReorderMismatch.into()));
        delete_dashboard(&c, ids[0]).unwrap();
        assert_eq!(delete_dashboard(&c, ids[0]), Err(ViewError::not_found()));
        let g = create_dashboard(&c, &new("G", None)).unwrap();
        delete_project(&c, p).unwrap();
        assert_eq!(get_dashboard(&c, ids[1]), Err(ViewError::not_found()));
        assert!(get_dashboard(&c, g).is_ok(), "globale bleiben");
    }

    #[test]
    fn duplicates_into_the_same_scope_without_default() {
        let c = db();
        let p = project(&c, "AA", false);
        let a = create_dashboard(&c, &NewDashboard { name: "Team".into(), project_id: Some(p), layout: Some(layout(vec![tile(1, 0, 0, 6, 2)])) }).unwrap();
        set_default(&c, a).unwrap();
        let d1 = duplicate_dashboard(&c, a, Lang::En).unwrap();
        let d2 = duplicate_dashboard(&c, a, Lang::De).unwrap();
        let (x, y) = (get_dashboard(&c, d1).unwrap(), get_dashboard(&c, d2).unwrap());
        assert_eq!((x.name.as_str(), y.name.as_str()), ("Copy of Team", "Kopie von Team"));
        assert!(!x.is_default && !y.is_default);
        assert_eq!(x.project_id, Some(p));
        assert_eq!(x.layout, get_dashboard(&c, a).unwrap().layout);
        assert!(get_dashboard(&c, a).unwrap().is_default);
        assert_eq!(duplicate_dashboard(&c, 9999, Lang::En), Err(ViewError::not_found()));
    }

    #[test]
    fn unreadable_layout_falls_back_to_the_default() {
        let c = db();
        let id = create_dashboard(&c, &new("A", None)).unwrap();
        c.execute("UPDATE dashboards SET layout = 'kaputt' WHERE id = ?1", [id]).unwrap();
        assert_eq!(get_dashboard(&c, id).unwrap().layout, default_dashboard_layout());
    }

    #[test]
    fn scope_check_constraint_is_enforced() {
        let c = db();
        let p = project(&c, "AA", false);
        assert!(c.execute("INSERT INTO dashboards (name, scope, project_id, layout) VALUES ('x', 'project', NULL, '{}')", []).is_err());
        assert!(c.execute("INSERT INTO dashboards (name, scope, project_id, layout) VALUES ('x', 'global', ?1, '{}')", [p]).is_err());
        assert!(c.execute("INSERT INTO dashboards (name, scope, project_id, layout) VALUES ('x', 'weird', NULL, '{}')", []).is_err());
    }

    // ---------------------------------------------------------------- Aufräumen

    #[test]
    fn prune_removes_tiles_of_deleted_projects() {
        let c = db();
        let (p, q) = (project(&c, "AA", false), project(&c, "BB", false));
        let l = layout(vec![
            Tile { id: 1, x: 0, y: 0, w: 4, h: 4, kind: TileKind::Sprint { project_id: Some(q) } },
            Tile { id: 2, x: 4, y: 0, w: 4, h: 4, kind: TileKind::TimeTracking { range: TimeRange::Week, project_id: Some(q) } },
            Tile { id: 3, x: 8, y: 0, w: 4, h: 4, kind: TileKind::Burndown { project_id: Some(p) } },
            Tile { id: 4, x: 0, y: 4, w: 4, h: 4, kind: TileKind::Favorites },
        ]);
        let g = create_dashboard(&c, &NewDashboard { name: "G".into(), project_id: None, layout: Some(l) }).unwrap();
        assert_eq!(prune_dashboard_refs(&c).unwrap(), 0);
        delete_project(&c, q).unwrap();
        assert_eq!(prune_dashboard_refs(&c).unwrap(), 1);
        let got = get_dashboard(&c, g).unwrap().layout;
        assert_eq!(got.tiles.iter().map(|t| t.id).collect::<Vec<_>>(), vec![2, 3, 4]);
        assert!(matches!(got.tiles[0].kind, TileKind::TimeTracking { project_id: None, .. }));
        assert_eq!(prune_dashboard_refs(&c).unwrap(), 0);
        validate_layout(&c, None, &got).unwrap();
    }

    // ---------------------------------------------------------------- Export / Import

    fn rich_layout(sprint_project: i64, time_project: i64, burndown_project: Option<i64>) -> Layout {
        layout(vec![
            Tile { id: 1, x: 0, y: 0, w: 12, h: 6, kind: TileKind::Search { jql: "type = Defect ORDER BY priority".into(), columns: vec!["key".into(), "status".into()], title: "Defects".into() } },
            Tile { id: 2, x: 0, y: 6, w: 4, h: 4, kind: TileKind::Sprint { project_id: Some(sprint_project) } },
            Tile { id: 3, x: 4, y: 6, w: 4, h: 4, kind: TileKind::Burndown { project_id: burndown_project } },
            Tile { id: 4, x: 8, y: 6, w: 4, h: 4, kind: TileKind::TimeTracking { range: TimeRange::Day, project_id: Some(time_project) } },
            Tile { id: 5, x: 0, y: 10, w: 4, h: 4, kind: TileKind::Counts { jql: String::new(), group_by: CountGroup::Type, chart: ChartKind::Bar } },
            Tile { id: 6, x: 4, y: 10, w: 4, h: 4, kind: TileKind::Projects },
        ])
    }

    #[test]
    fn export_uses_project_keys_and_round_trips() {
        let c = db();
        let (p, q) = (project(&c, "AA", false), project(&c, "BB", false));
        let id = create_dashboard(&c, &NewDashboard { name: "Defects".into(), project_id: Some(p), layout: Some(rich_layout(q, p, None)) }).unwrap();
        set_default(&c, id).unwrap();
        let doc = export_dashboard(&c, id).unwrap();
        assert_eq!((doc.format.as_str(), doc.version, doc.scope, doc.project.as_deref()), (FORMAT, DOC_VERSION, DashboardScope::Project, Some("AA")));
        let v: serde_json::Value = serde_json::from_str(&doc.to_json()).unwrap();
        assert_eq!(v["layout"]["tiles"][1]["config"], json!({"project_id": "BB"}));
        assert_eq!(v["layout"]["tiles"][3]["config"]["project_id"], "AA");
        assert!(v["layout"]["tiles"][2]["config"]["project_id"].is_null());
        assert_eq!(DashboardDoc::from_json(&doc.to_json()).unwrap(), doc);
        // Import ins selbe Projekt: neues Dashboard mit gleichem Inhalt, nicht Standard, Name „(2)“
        let imp = import_dashboard(&c, &doc, ImportTarget::Document).unwrap();
        assert_ne!(imp.id, id);
        assert_eq!(imp.report, ImportReport { unknown_projects: vec![], dropped_tiles: 0, renamed: true });
        let (old, new) = (get_dashboard(&c, id).unwrap(), get_dashboard(&c, imp.id).unwrap());
        assert_eq!(new.name, "Defects (2)");
        assert_eq!(new.layout, old.layout);
        assert_eq!((new.project_id, new.scope, new.is_default), (Some(p), DashboardScope::Project, false));
        assert!(old.is_default, "Bestehendes bleibt");
        // erneut: „(3)“
        assert_eq!(get_dashboard(&c, import_dashboard(&c, &doc, ImportTarget::Document).unwrap().id).unwrap().name, "Defects (3)");
    }

    #[test]
    fn global_export_import_round_trip() {
        let c = db();
        let p = project(&c, "AA", false);
        let g = create_dashboard(&c, &NewDashboard { name: "Alles".into(), project_id: None, layout: Some(layout(vec![Tile { id: 1, x: 0, y: 0, w: 6, h: 4, kind: TileKind::Sprint { project_id: Some(p) } }, Tile { id: 2, x: 6, y: 0, w: 6, h: 4, kind: TileKind::Favorites }])) }).unwrap();
        let doc = export_dashboard(&c, g).unwrap();
        assert_eq!((doc.scope, doc.project.clone()), (DashboardScope::Global, None));
        // in eine zweite Datenbank (gleiche Projekte, andere Kennungen) importieren
        let c2 = db();
        let (_x, p2) = (project(&c2, "ZZ", false), project(&c2, "AA", false));
        let imp = import_dashboard(&c2, &doc, ImportTarget::Document).unwrap();
        let d = get_dashboard(&c2, imp.id).unwrap();
        assert_eq!(d.name, "Alles");
        assert!(!imp.report.renamed);
        assert!(matches!(d.layout.tiles[0].kind, TileKind::Sprint { project_id: Some(x) } if x == p2));
    }

    #[test]
    fn unknown_projects_are_reported_and_handled() {
        let c = db();
        let (p, q) = (project(&c, "AA", false), project(&c, "BB", false));
        let id = create_dashboard(&c, &NewDashboard { name: "D".into(), project_id: None, layout: Some(rich_layout(q, p, Some(p))) }).unwrap();
        let mut doc = export_dashboard(&c, id).unwrap();
        doc.layout.tiles[2].kind = TileKind::Burndown { project_id: None };
        let c2 = db();
        project(&c2, "AA", false); // BB fehlt
        let imp = import_dashboard(&c2, &doc, ImportTarget::Global).unwrap();
        assert_eq!(imp.report.unknown_projects, vec!["BB".to_string()]);
        // Sprint (BB) entfällt; Burndown ohne Projekt im globalen Ziel entfällt ebenso
        assert_eq!(imp.report.dropped_tiles, 2);
        let l = get_dashboard(&c2, imp.id).unwrap().layout;
        assert_eq!(l.tiles.iter().map(|t| t.id).collect::<Vec<_>>(), vec![1, 4, 5, 6]);
        assert!(imp.report.message(Lang::En).unwrap().contains("BB"));
        assert!(imp.report.message(Lang::De).unwrap().contains("BB"));
        assert_ne!(imp.report.message(Lang::En), imp.report.message(Lang::De));
        // Zeiterfassung mit unbekanntem Projekt: bleibt, gilt für alle
        let mut doc2 = doc.clone();
        doc2.layout.tiles[3].kind = TileKind::TimeTracking { range: TimeRange::Day, project_id: Some("NOPE".into()) };
        let imp2 = import_dashboard(&c2, &doc2, ImportTarget::Global).unwrap();
        assert!(imp2.report.unknown_projects.contains(&"NOPE".to_string()));
        let l2 = get_dashboard(&c2, imp2.id).unwrap().layout;
        assert!(matches!(l2.tiles.iter().find(|t| t.id == 4).unwrap().kind, TileKind::TimeTracking { project_id: None, .. }));
        assert_eq!(ImportReport::default().message(Lang::En), None);
    }

    #[test]
    fn import_targets() {
        let c = db();
        let (p, q) = (project(&c, "AA", false), project(&c, "BB", false));
        // Dokument eines Projekt-Dashboards von AA mit Burndown ohne Projekt
        let src = create_dashboard(&c, &NewDashboard { name: "S".into(), project_id: Some(p), layout: Some(layout(vec![Tile { id: 1, x: 0, y: 0, w: 4, h: 4, kind: TileKind::Burndown { project_id: None } }])) }).unwrap();
        let doc = export_dashboard(&c, src).unwrap();
        // in ein anderes Projekt
        let to_q = import_dashboard(&c, &doc, ImportTarget::Project(q)).unwrap();
        let d = get_dashboard(&c, to_q.id).unwrap();
        assert_eq!((d.project_id, d.scope), (Some(q), DashboardScope::Project));
        assert!(matches!(d.layout.tiles[0].kind, TileKind::Burndown { project_id: None }), "folgt dem Zielprojekt");
        // global: Burndown übernimmt das Projekt des Dokuments
        let to_g = import_dashboard(&c, &doc, ImportTarget::Global).unwrap();
        let g = get_dashboard(&c, to_g.id).unwrap();
        assert_eq!((g.project_id, g.scope), (None, DashboardScope::Global));
        assert!(matches!(g.layout.tiles[0].kind, TileKind::Burndown { project_id: Some(x) } if x == p));
        assert_eq!(to_g.report.dropped_tiles, 0);
        // unbekanntes Zielprojekt
        assert_eq!(import_dashboard(&c, &doc, ImportTarget::Project(9999)), Err(ViewError::not_found()));
        // Dokument mit unbekanntem Projekt als Ziel
        let mut other = doc.clone();
        other.project = Some("NIX".into());
        assert_eq!(import_dashboard(&c, &other, ImportTarget::Document), Err(ViewError::Doc(DocProblem::Reference("NIX".into()))));
        other.project = None;
        assert!(matches!(import_dashboard(&c, &other, ImportTarget::Document), Err(ViewError::Doc(DocProblem::Reference(_)))));
        // nichts Halbes zurückgeblieben
        assert_eq!(list_dashboards(&c, Some(p)).unwrap().len(), 1);
    }

    #[test]
    fn import_never_changes_existing_and_is_atomic() {
        let c = db();
        let p = project(&c, "AA", false);
        let id = create_dashboard(&c, &new("A", Some(p))).unwrap();
        set_default(&c, id).unwrap();
        let before = get_dashboard(&c, id).unwrap();
        let mut doc = export_dashboard(&c, id).unwrap();
        import_dashboard(&c, &doc, ImportTarget::Document).unwrap();
        assert_eq!(get_dashboard(&c, id).unwrap(), before);
        // ungültiges Layout im Dokument: Fehler, nichts angelegt
        doc.layout.tiles.push(TileG { id: 1, x: 0, y: 0, w: 1, h: 1, kind: TileKind::Flagged });
        let n_before = list_dashboards(&c, Some(p)).unwrap().len();
        assert_eq!(import_dashboard(&c, &doc, ImportTarget::Document), Err(ViewError::TileIdTwice { tile: 1 }));
        assert_eq!(list_dashboards(&c, Some(p)).unwrap().len(), n_before);
    }

    #[test]
    fn documents_are_checked_for_format_and_version() {
        let c = db();
        let id = create_dashboard(&c, &new("A", None)).unwrap();
        let doc = export_dashboard(&c, id).unwrap();
        let text = doc.to_json();
        assert!(DashboardDoc::from_json(&format!("\u{feff}{text}")).is_ok(), "BOM wird toleriert");
        assert_eq!(DashboardDoc::from_json("{}"), Err(ViewError::Doc(DocProblem::Format)));
        assert_eq!(DashboardDoc::from_json(r#"{"format":"anderes"}"#), Err(ViewError::Doc(DocProblem::Format)));
        assert!(matches!(DashboardDoc::from_json("kaputt"), Err(ViewError::Doc(DocProblem::Json(_)))));
        let newer = text.replace("\"version\": 1", "\"version\": 99");
        assert_eq!(DashboardDoc::from_json(&newer), Err(ViewError::Doc(DocProblem::Version { found: 99, supported: DOC_VERSION })));
        let mut d2 = doc.clone();
        d2.version = 99;
        assert!(matches!(import_dashboard(&c, &d2, ImportTarget::Global), Err(ViewError::Doc(DocProblem::Version { .. }))));
        let mut d3 = doc;
        d3.format = "x".into();
        assert_eq!(import_dashboard(&c, &d3, ImportTarget::Global), Err(ViewError::Doc(DocProblem::Format)));
        let mut d4 = d3.clone();
        d4.format = FORMAT.into();
        d4.name = "  ".into();
        assert_eq!(import_dashboard(&c, &d4, ImportTarget::Global), Err(PlanError::NameEmpty.into()));
    }

    #[test]
    fn export_drops_tiles_of_deleted_projects() {
        let c = db();
        let (p, q) = (project(&c, "AA", false), project(&c, "BB", false));
        let g = create_dashboard(&c, &NewDashboard { name: "G".into(), project_id: None, layout: Some(layout(vec![Tile { id: 1, x: 0, y: 0, w: 4, h: 4, kind: TileKind::Sprint { project_id: Some(q) } }, Tile { id: 2, x: 4, y: 0, w: 4, h: 4, kind: TileKind::Burndown { project_id: Some(p) } }])) }).unwrap();
        delete_project(&c, q).unwrap();
        let doc = export_dashboard(&c, g).unwrap();
        assert_eq!(doc.layout.tiles.len(), 1);
        assert!(matches!(&doc.layout.tiles[0].kind, TileKind::Burndown { project_id: Some(k) } if k == "AA"));
        assert_eq!(export_dashboard(&c, 9999), Err(ViewError::not_found()));
    }
}
