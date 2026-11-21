//! Teilungen des Editors (#54): Jede Fläche ist eine *Gruppe* mit eigener Reiterleiste; das Layout ist derselbe Baum wie
//! im Terminal Hub (`layout::Node`, Blätter = Gruppen-Ids). Ein Reiter zeigt ein Dokument (`doc`); dasselbe Dokument kann
//! in mehreren Gruppen offen sein – jeder Reiter hat dann seine eigene Ansicht `V` (Bildlauf, Cursor), der Text ist gemeinsam.
//!
//! Reine Logik ohne egui, damit sie sich ohne Oberfläche testen lässt.

use crate::layout::{Dir, Node};
use std::collections::BTreeMap;

/// Ein Reiter: Dokument und eigene Ansicht.
#[derive(Clone, Debug)]
pub struct Tab<V> {
    pub doc: u64,
    pub view: V,
}

/// Eine Fläche mit eigener Reiterleiste.
#[derive(Clone, Debug)]
pub struct Group<V> {
    pub tabs: Vec<Tab<V>>,
    /// Index des sichtbaren Reiters (bei leerer Gruppe 0).
    pub active: usize,
}

impl<V> Default for Group<V> {
    fn default() -> Self {
        Group { tabs: Vec::new(), active: 0 }
    }
}

impl<V> Group<V> {
    pub fn active_tab(&self) -> Option<&Tab<V>> {
        self.tabs.get(self.active)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut Tab<V>> {
        self.tabs.get_mut(self.active)
    }

    pub fn find(&self, doc: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.doc == doc)
    }

    fn clamp(&mut self) {
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
    }
}

/// Alle Gruppen und ihr Layout.
#[derive(Clone, Debug)]
pub struct Panes<V> {
    pub layout: Option<Node>,
    pub groups: BTreeMap<u64, Group<V>>,
    /// Fokussierte Gruppe (0 = keine).
    pub focused: u64,
    next_id: u64,
}

impl<V> Default for Panes<V> {
    fn default() -> Self {
        Panes { layout: None, groups: BTreeMap::new(), focused: 0, next_id: 1 }
    }
}

/// Wo ein gezogener Reiter abgelegt wird.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drop {
    /// In die Reiterleiste der Gruppe an Position `index`.
    Bar { group: u64, index: usize },
    /// Neue Gruppe neben `group` (`dir`, davor oder dahinter).
    Split { group: u64, dir: Dir, before: bool },
}

impl<V: Clone + Default> Panes<V> {
    /// Gruppen-Ids in Layout-Reihenfolge.
    pub fn group_ids(&self) -> Vec<u64> {
        self.layout.as_ref().map(Node::leaf_ids).unwrap_or_default()
    }

    fn new_group(&mut self) -> u64 {
        let id = self.next_id.max(self.groups.keys().next_back().map(|k| k + 1).unwrap_or(1));
        self.next_id = id + 1;
        self.groups.insert(id, Group::default());
        id
    }

    /// Fokussierte Gruppe; legt eine an, wenn es noch keine gibt.
    pub fn ensure_group(&mut self) -> u64 {
        if self.groups.contains_key(&self.focused) && self.layout.as_ref().is_some_and(|l| l.contains(self.focused)) {
            return self.focused;
        }
        if let Some(first) = self.group_ids().first().copied() {
            self.focused = first;
            return first;
        }
        let g = self.new_group();
        self.layout = Some(Node::Leaf(g));
        self.focused = g;
        g
    }

    /// Dokument zeigen: Ist es in der fokussierten Gruppe offen, wird dieser Reiter aktiv; sonst kommt ein neuer Reiter
    /// hinter den aktiven. Liefert (Gruppe, Index).
    pub fn open(&mut self, doc: u64) -> (u64, usize) {
        let g = self.ensure_group();
        let grp = self.groups.get_mut(&g).expect("Gruppe");
        if let Some(i) = grp.find(doc) {
            grp.active = i;
            return (g, i);
        }
        let at = if grp.tabs.is_empty() { 0 } else { grp.active + 1 };
        grp.tabs.insert(at, Tab { doc, view: V::default() });
        grp.active = at;
        (g, at)
    }

    /// Gruppe `group` teilen: Die neue Gruppe zeigt das aktive Dokument der alten (eigene Ansicht, Kopie der alten) und
    /// bekommt den Fokus. Eine leere Gruppe wird nicht geteilt. Liefert die neue Gruppe.
    pub fn split(&mut self, group: u64, dir: Dir) -> Option<u64> {
        let tab = self.groups.get(&group)?.active_tab()?.clone();
        let g = self.new_group();
        self.groups.get_mut(&g).expect("neu").tabs.push(tab);
        let ok = self.layout.as_mut().is_some_and(|l| l.split_leaf_at(group, g, dir, false));
        if !ok {
            self.groups.remove(&g);
            return None;
        }
        self.focused = g;
        Some(g)
    }

    /// Reiter schließen. Eine dadurch leere Gruppe verschwindet aus dem Layout, solange es noch andere gibt. Liefert das
    /// Dokument, wenn es danach in keinem Reiter mehr offen ist (der Aufrufer gibt es frei).
    pub fn close_tab(&mut self, group: u64, index: usize) -> Option<u64> {
        let grp = self.groups.get_mut(&group)?;
        if index >= grp.tabs.len() {
            return None;
        }
        let doc = grp.tabs.remove(index).doc;
        if grp.active > index || grp.active >= grp.tabs.len() {
            grp.active = grp.active.saturating_sub(1);
        }
        grp.clamp();
        if grp.tabs.is_empty() {
            self.drop_group_if_more(group);
        }
        (!self.shows(doc)).then_some(doc)
    }

    /// Leere Gruppe entfernen, wenn es noch eine andere gibt (die letzte Gruppe bleibt als leere Fläche stehen).
    fn drop_group_if_more(&mut self, group: u64) {
        if self.group_ids().len() <= 1 {
            return;
        }
        let near = self.neighbour(group);
        if let Some(l) = self.layout.take() {
            self.layout = l.remove_leaf(group);
        }
        self.groups.remove(&group);
        if self.focused == group {
            self.focused = near.or_else(|| self.group_ids().first().copied()).unwrap_or(0);
        }
    }

    fn neighbour(&self, group: u64) -> Option<u64> {
        let ids = self.group_ids();
        let i = ids.iter().position(|g| *g == group)?;
        ids.get(i + 1).or_else(|| i.checked_sub(1).and_then(|j| ids.get(j))).copied()
    }

    /// Ist das Dokument in irgendeinem Reiter offen?
    pub fn shows(&self, doc: u64) -> bool {
        self.groups.values().any(|g| g.find(doc).is_some())
    }

    /// Alle Reiter, die das Dokument zeigen: (Gruppe, Index).
    pub fn tabs_of(&self, doc: u64) -> Vec<(u64, usize)> {
        let mut v = Vec::new();
        for (gid, g) in &self.groups {
            for (i, t) in g.tabs.iter().enumerate() {
                if t.doc == doc {
                    v.push((*gid, i));
                }
            }
        }
        v
    }

    /// Alle offenen Dokumente (jedes einmal, Layout-Reihenfolge).
    pub fn docs(&self) -> Vec<u64> {
        let mut v = Vec::new();
        for g in self.group_ids() {
            for t in &self.groups[&g].tabs {
                if !v.contains(&t.doc) {
                    v.push(t.doc);
                }
            }
        }
        v
    }

    /// Gezogenen Reiter ablegen. Innerhalb derselben Leiste wird nur umsortiert; in eine andere Gruppe wandert er (gibt es
    /// das Dokument dort schon, wird nur dieser Reiter aktiv und der gezogene verschwindet). `Drop::Split` legt eine neue
    /// Gruppe an. Die Zielgruppe bekommt den Fokus. Liefert die Zielgruppe.
    pub fn move_tab(&mut self, from: u64, index: usize, to: Drop) -> Option<u64> {
        let src_len = self.groups.get(&from)?.tabs.len();
        if index >= src_len {
            return None;
        }
        match to {
            Drop::Bar { group, index: at } if group == from => {
                let grp = self.groups.get_mut(&from)?;
                let tab = grp.tabs.remove(index);
                let at = if at > index { at - 1 } else { at }.min(grp.tabs.len());
                grp.tabs.insert(at, tab);
                grp.active = at;
                self.focused = from;
                Some(from)
            }
            Drop::Bar { group, index: at } => {
                if !self.groups.contains_key(&group) {
                    return None;
                }
                let tab = self.take_tab(from, index);
                let grp = self.groups.get_mut(&group)?;
                if let Some(i) = grp.find(tab.doc) {
                    grp.active = i;
                } else {
                    let at = at.min(grp.tabs.len());
                    grp.tabs.insert(at, tab);
                    grp.active = at;
                }
                self.focused = group;
                self.drop_if_empty(from);
                Some(group)
            }
            Drop::Split { group, dir, before } => {
                if group == from && src_len == 1 {
                    return Some(from); // der einzige Reiter neben sich selbst: nichts zu tun
                }
                if !self.layout.as_ref().is_some_and(|l| l.contains(group)) {
                    return None;
                }
                let tab = self.take_tab(from, index);
                let g = self.new_group();
                {
                    let ng = self.groups.get_mut(&g).expect("neu");
                    ng.tabs.push(tab);
                }
                if let Some(l) = self.layout.as_mut() {
                    l.split_leaf_at(group, g, dir, before);
                }
                self.focused = g;
                self.drop_if_empty(from);
                Some(g)
            }
        }
    }

    fn take_tab(&mut self, group: u64, index: usize) -> Tab<V> {
        let grp = self.groups.get_mut(&group).expect("Gruppe");
        let tab = grp.tabs.remove(index);
        if grp.active > index {
            grp.active -= 1;
        }
        grp.clamp();
        tab
    }

    fn drop_if_empty(&mut self, group: u64) {
        if self.groups.get(&group).is_some_and(|g| g.tabs.is_empty()) {
            self.drop_group_if_more(group);
        }
    }

    /// Layout bereinigen: Blätter ohne Gruppe fallen weg, Gruppen außerhalb des Layouts auch; der Fokus liegt auf einer
    /// vorhandenen Gruppe. Leere Gruppen bleiben nur als einzige Fläche.
    pub fn normalize(&mut self) {
        let groups = &self.groups;
        self.layout = self.layout.take().and_then(|l| l.normalize(|g| groups.contains_key(&g)));
        let ids = self.group_ids();
        self.groups.retain(|g, _| ids.contains(g));
        for g in self.groups.values_mut() {
            g.clamp();
        }
        let empty: Vec<u64> = ids.iter().copied().filter(|g| self.groups[g].tabs.is_empty()).collect();
        for g in empty {
            self.drop_group_if_more(g);
        }
        if !self.groups.contains_key(&self.focused) {
            self.focused = self.group_ids().first().copied().unwrap_or(0);
        }
        if let Some(max) = self.groups.keys().next_back() {
            self.next_id = self.next_id.max(max + 1);
        }
    }

    /// Gruppe aus einer gespeicherten Liste übernehmen (Wiederherstellen); die Id bleibt.
    pub fn insert_group(&mut self, id: u64, g: Group<V>) {
        self.groups.insert(id, g);
        self.next_id = self.next_id.max(id + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type P = Panes<u32>;

    fn ids(p: &P, g: u64) -> Vec<u64> {
        p.groups[&g].tabs.iter().map(|t| t.doc).collect()
    }

    #[test]
    fn opening_reuses_the_tab_of_the_focused_group() {
        let mut p = P::default();
        let (g, i) = p.open(10);
        assert_eq!((i, p.focused), (0, g));
        p.open(11);
        p.open(12);
        assert_eq!(ids(&p, g), [10, 11, 12]);
        assert_eq!(p.open(10), (g, 0), "schon offen: nur aktivieren");
        assert_eq!(p.groups[&g].active, 0);
        p.open(13);
        assert_eq!(ids(&p, g), [10, 13, 11, 12], "neuer Reiter hinter dem aktiven");
    }

    #[test]
    fn split_shows_the_same_document_with_its_own_view() {
        let mut p = P::default();
        let (g, _) = p.open(10);
        p.groups.get_mut(&g).unwrap().tabs[0].view = 7;
        let n = p.split(g, Dir::Horizontal).unwrap();
        assert_ne!(n, g);
        assert_eq!(p.focused, n);
        assert_eq!(ids(&p, n), [10]);
        assert_eq!(p.groups[&n].tabs[0].view, 7, "Ansicht wird kopiert, danach getrennt");
        p.groups.get_mut(&n).unwrap().tabs[0].view = 9;
        assert_eq!(p.groups[&g].tabs[0].view, 7);
        assert_eq!(p.tabs_of(10).len(), 2);
        assert_eq!(p.docs(), [10]);
        assert!(p.split(999, Dir::Vertical).is_none());
    }

    #[test]
    fn closing_returns_the_document_only_when_no_tab_shows_it() {
        let mut p = P::default();
        let (g, _) = p.open(10);
        let n = p.split(g, Dir::Vertical).unwrap();
        assert_eq!(p.close_tab(n, 0), None, "noch in der anderen Gruppe offen");
        assert_eq!(p.group_ids(), [g], "leere Gruppe fällt weg");
        assert_eq!(p.focused, g);
        assert_eq!(p.close_tab(g, 0), Some(10));
        assert_eq!(p.group_ids(), [g], "die letzte Gruppe bleibt als leere Fläche");
        assert!(p.groups[&g].tabs.is_empty());
        assert_eq!(p.close_tab(g, 0), None);
    }

    #[test]
    fn closing_keeps_a_sensible_active_tab() {
        let mut p = P::default();
        let (g, _) = p.open(1);
        p.open(2);
        p.open(3);
        p.groups.get_mut(&g).unwrap().active = 2;
        p.close_tab(g, 0);
        assert_eq!(p.groups[&g].active, 1, "aktiver Reiter bleibt derselbe (3)");
        assert_eq!(p.groups[&g].active_tab().unwrap().doc, 3);
        p.close_tab(g, 1);
        assert_eq!(p.groups[&g].active_tab().unwrap().doc, 2);
    }

    #[test]
    fn tabs_move_between_bars_and_into_new_splits() {
        let mut p = P::default();
        let (a, _) = p.open(1);
        p.open(2);
        p.open(3);
        // Umsortieren in derselben Leiste
        assert_eq!(p.move_tab(a, 0, Drop::Bar { group: a, index: 3 }), Some(a));
        assert_eq!(ids(&p, a), [2, 3, 1]);
        // In eine neue Teilung rechts
        let b = p.move_tab(a, 2, Drop::Split { group: a, dir: Dir::Horizontal, before: false }).unwrap();
        assert_eq!(ids(&p, a), [2, 3]);
        assert_eq!(ids(&p, b), [1]);
        assert_eq!(p.group_ids(), [a, b]);
        // In die andere Leiste
        assert_eq!(p.move_tab(a, 0, Drop::Bar { group: b, index: 0 }), Some(b));
        assert_eq!(ids(&p, b), [2, 1]);
        // Dokument schon im Ziel: kein zweiter Reiter, nur aktivieren
        p.open(3); // Fokus liegt auf b → 3 kommt nach b
        assert_eq!(ids(&p, b), [2, 3, 1]);
        assert_eq!(p.move_tab(a, 0, Drop::Bar { group: b, index: 0 }), Some(b));
        assert_eq!(p.group_ids(), [b], "Quelle leer → weg");
        assert_eq!(ids(&p, b), [2, 3, 1]);
        assert_eq!(p.groups[&b].active_tab().unwrap().doc, 3);
        // Einziger Reiter neben sich selbst: nichts
        let mut q = P::default();
        let (g, _) = q.open(5);
        assert_eq!(q.move_tab(g, 0, Drop::Split { group: g, dir: Dir::Vertical, before: true }), Some(g));
        assert_eq!(q.group_ids(), [g]);
    }

    #[test]
    fn normalize_repairs_a_loaded_layout() {
        let mut p = P::default();
        p.insert_group(4, Group { tabs: vec![Tab { doc: 1, view: 0 }], active: 9 });
        p.insert_group(8, Group::default());
        p.layout = Some(Node::Split { dir: Dir::Horizontal, ratio: f32::NAN, a: Box::new(Node::Leaf(4)), b: Box::new(Node::Split { dir: Dir::Vertical, ratio: 0.5, a: Box::new(Node::Leaf(8)), b: Box::new(Node::Leaf(99)) }) });
        p.focused = 99;
        p.normalize();
        assert_eq!(p.group_ids(), [4], "unbekannte Blätter und leere Gruppen fallen weg");
        assert_eq!(p.focused, 4);
        assert_eq!(p.groups[&4].active, 0);
        let (g, _) = p.open(2);
        assert_eq!(g, 4);
        let n = p.split(4, Dir::Horizontal).unwrap();
        assert!(n > 8, "neue Ids nach den geladenen");
    }
}
