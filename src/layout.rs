//! Frei teilbare Flächen als Baum: Blätter sind Sitzungs-IDs.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dir {
    /// Nebeneinander (vertikale Trennlinie).
    Horizontal,
    /// Übereinander (horizontale Trennlinie).
    Vertical,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Node {
    Leaf(u64),
    Split {
        dir: Dir,
        /// Anteil von `a` (0..1). Unsinnige Werte aus einer Datei (null, NaN, außerhalb) laden als 0.5 bzw. begrenzt.
        #[serde(deserialize_with = "lenient_ratio")]
        ratio: f32,
        a: Box<Node>,
        b: Box<Node>,
    },
}

/// Kleinster/größter Anteil einer Seite (Trennlinie ziehen, Laden, Normalisieren).
pub const RATIO_MIN: f32 = 0.1;
pub const RATIO_MAX: f32 = 0.9;

/// Anteil begrenzen; NaN/unendlich → Mitte.
pub fn clamp_ratio(r: f32) -> f32 {
    if r.is_finite() { r.clamp(RATIO_MIN, RATIO_MAX) } else { 0.5 }
}

fn lenient_ratio<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(clamp_ratio(v.as_f64().map(|x| x as f32).unwrap_or(0.5)))
}

/// Für `Option<Node>`-Felder in Dateien: ein kaputtes Layout lädt als „kein Layout“, statt die ganze Datei zu verwerfen.
/// Das gilt auch für ein Layout, das diese Version nicht versteht (z. B. eine neue Knotenart nach einem Downgrade): es wird
/// beim nächsten Speichern durch das aktuelle (ggf. leere) Layout ersetzt und ist dann weg – die Sitzungen selbst bleiben
/// erhalten (sie stehen getrennt in `sessions`) und erscheinen in der Seitenleiste.
pub fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Node>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value::<Node>(v).ok())
}

/// Wo ein minimierter Bereich stand: sein Geschwister-Teilbaum (dessen Blätter in Reihenfolge), die Teilungsrichtung,
/// ob er davor (links/oben) lag, und sein Anteil. Damit kommt er beim Zurückholen an dieselbe Stelle (`Node::insert_at_slot`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Slot {
    pub sibling: Vec<u64>,
    pub dir: Dir,
    pub before: bool,
    #[serde(deserialize_with = "lenient_ratio")]
    pub ratio: f32,
}

/// Sitzung `id` zeigen (Regel von `App::activate`). Steht sie schon im Layout, bleibt alles, wie es ist. Sonst:
/// 1. kein Layout (leerer Hub) → sie wird der einzige Bereich;
/// 2. `minimized` und ihr `slot` passt noch → genau dort wieder einfügen (`Node::insert_at_slot`);
/// 3. `minimized`, aber die Geschwister sind weg → den fokussierten Bereich (`focused`) rechts teilen;
/// 4. sonst (nur weggewechselt) → den fokussierten Bereich ersetzen.
///
/// Ist `focused` nicht im Layout, gilt für 3/4 der erste Bereich.
pub fn show(layout: &mut Option<Node>, id: u64, focused: u64, minimized: bool, slot: Option<&Slot>) {
    let Some(l) = layout else {
        *layout = Some(Node::Leaf(id));
        return;
    };
    if l.contains(id) {
        return;
    }
    let place = |l: &mut Node, at: u64| if minimized { l.split_leaf_at(at, id, Dir::Horizontal, false) } else { l.replace_leaf(at, id) };
    let placed = (minimized && slot.is_some_and(|s| l.insert_at_slot(id, s))) || place(l, focused);
    if !placed {
        let first = l.leaf_ids()[0];
        place(l, first);
    }
}

/// Bereich `id` minimieren: aus dem Layout nehmen (auch den letzten – dann `None` = leerer Hub). Liefert den gemerkten
/// Platz und den neuen Fokus (bleibt `focused`, wenn das noch im Layout ist, sonst der direkte Nachbar; 0 = nichts mehr da),
/// oder `None`, wenn `id` gar nicht im Layout stand.
pub fn minimize(layout: &mut Option<Node>, id: u64, focused: u64) -> Option<(Option<Slot>, u64)> {
    let l = layout.take()?;
    if !l.contains(id) {
        *layout = Some(l);
        return None;
    }
    let (rest, slot) = l.remove_leaf_slot(id);
    *layout = rest;
    let has = |x: u64| layout.as_ref().is_some_and(|l| l.contains(x));
    let focus = if has(focused) {
        focused
    } else {
        // angrenzende Seite des Geschwister-Teilbaums
        let near = slot.as_ref().and_then(|s| if s.before { s.sibling.first() } else { s.sibling.last() }).copied();
        near.filter(|n| has(*n)).or_else(|| layout.as_ref().map(|l| l.leaf_ids()[0])).unwrap_or(0)
    };
    Some((slot, focus))
}

/// Blatt `gone` hat das Layout verlassen: In den Geschwistern von `slot` tritt an seine Stelle `with` (die Geschwister aus
/// seinem eigenen Slot, also das, was an seinen Platz gerückt ist); doppelte fallen weg. Ohne `with` (er war allein) wird
/// er nur gestrichen. So folgt eine Kette minimierter Bereiche dem, was tatsächlich noch im Layout steht.
pub fn reroute_sibling(slot: &mut Slot, gone: u64, with: &[u64]) {
    if !slot.sibling.contains(&gone) {
        return;
    }
    let mut out: Vec<u64> = Vec::with_capacity(slot.sibling.len() + with.len());
    for id in &slot.sibling {
        if *id == gone {
            for w in with {
                if !out.contains(w) {
                    out.push(*w);
                }
            }
        } else if !out.contains(id) {
            out.push(*id);
        }
    }
    slot.sibling = out;
}

/// Für `Option<Slot>`-Felder in Dateien: Unlesbares lädt als `None`.
pub fn lenient_slot<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Slot>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value::<Slot>(v).ok())
}

impl Node {
    /// Anzahl der Blätter.
    pub fn leaf_count(&self) -> usize {
        match self {
            Node::Leaf(_) => 1,
            Node::Split { a, b, .. } => a.leaf_count() + b.leaf_count(),
        }
    }

    /// Blätter als neue Liste (in Reihenfolge links/oben → rechts/unten).
    pub fn leaf_ids(&self) -> Vec<u64> {
        let mut v = Vec::new();
        self.leaves(&mut v);
        v
    }

    /// Bereinigt den Baum: entfernt Blätter, die `valid` ablehnt, und doppelte Blätter (das erste bleibt), lässt Teilungen
    /// mit nur einer Seite in diese Seite zusammenfallen und begrenzt die Anteile (NaN → 0.5). `None`, wenn nichts übrig bleibt.
    pub fn normalize(self, mut valid: impl FnMut(u64) -> bool) -> Option<Node> {
        let mut seen = std::collections::HashSet::new();
        self.norm(&mut valid, &mut seen)
    }

    fn norm(self, valid: &mut impl FnMut(u64) -> bool, seen: &mut std::collections::HashSet<u64>) -> Option<Node> {
        match self {
            Node::Leaf(id) => (valid(id) && seen.insert(id)).then_some(Node::Leaf(id)),
            Node::Split { dir, ratio, a, b } => match (a.norm(valid, seen), b.norm(valid, seen)) {
                (Some(a), Some(b)) => Some(Node::Split { dir, ratio: clamp_ratio(ratio), a: Box::new(a), b: Box::new(b) }),
                (Some(x), None) | (None, Some(x)) => Some(x),
                (None, None) => None,
            },
        }
    }

    /// Ist der Baum schon normal (siehe `normalize`)? Billiger Test, damit nur bei Bedarf neu gebaut wird.
    pub fn is_normal(&self, mut valid: impl FnMut(u64) -> bool) -> bool {
        let mut seen = std::collections::HashSet::new();
        let mut ok = true;
        self.visit(&mut |n| match n {
            Node::Leaf(id) => ok &= valid(*id) && seen.insert(*id),
            Node::Split { ratio, .. } => ok &= ratio.is_finite() && (RATIO_MIN..=RATIO_MAX).contains(ratio),
        });
        ok
    }

    fn visit(&self, f: &mut impl FnMut(&Node)) {
        f(self);
        if let Node::Split { a, b, .. } = self {
            a.visit(f);
            b.visit(f);
        }
    }

    /// Entfernt das Blatt `target` (wie `remove_leaf`) und merkt sich, wo es stand (`Slot`). Der Slot ist `None`, wenn das
    /// Blatt fehlte oder allein war.
    pub fn remove_leaf_slot(self, target: u64) -> (Option<Node>, Option<Slot>) {
        let slot = self.slot_of(target);
        (self.remove_leaf(target), slot)
    }

    fn slot_of(&self, target: u64) -> Option<Slot> {
        let Node::Split { dir, ratio, a, b } = self else { return None };
        match (a.as_ref(), b.as_ref()) {
            (Node::Leaf(x), other) if *x == target => Some(Slot { sibling: other.leaf_ids(), dir: *dir, before: true, ratio: *ratio }),
            (other, Node::Leaf(x)) if *x == target => Some(Slot { sibling: other.leaf_ids(), dir: *dir, before: false, ratio: *ratio }),
            _ => a.slot_of(target).or_else(|| b.slot_of(target)),
        }
    }

    /// Fügt `id` an der gemerkten Stelle wieder ein: Gibt es den Geschwister-Teilbaum noch genau so, wird er geteilt
    /// (gleiche Richtung, Seite und Anteil); sonst wird das erste noch vorhandene Blatt daraus geteilt. `false`, wenn von
    /// den Geschwistern nichts mehr im Baum ist (oder `id` schon drin ist).
    pub fn insert_at_slot(&mut self, id: u64, slot: &Slot) -> bool {
        if self.contains(id) || slot.sibling.is_empty() {
            return false;
        }
        if let Some(n) = self.find_subtree(&slot.sibling) {
            let old = std::mem::replace(n, Node::Leaf(id));
            let (a, b) = if slot.before { (Node::Leaf(id), old) } else { (old, Node::Leaf(id)) };
            *n = Node::Split { dir: slot.dir, ratio: clamp_ratio(slot.ratio), a: Box::new(a), b: Box::new(b) };
            return true;
        }
        let have = self.leaf_ids();
        let Some(anchor) = slot.sibling.iter().find(|s| have.contains(s)).copied() else { return false };
        if !self.split_leaf_at(anchor, id, slot.dir, slot.before) {
            return false;
        }
        // Anteil der wiederhergestellten Seite übernehmen.
        self.set_ratio_of_parent(id, clamp_ratio(slot.ratio));
        true
    }

    fn find_subtree(&mut self, leaves: &[u64]) -> Option<&mut Node> {
        if self.leaf_ids() == leaves {
            return Some(self);
        }
        match self {
            Node::Leaf(_) => None,
            Node::Split { a, b, .. } => {
                if a.has_subtree(leaves) {
                    a.find_subtree(leaves)
                } else {
                    b.find_subtree(leaves)
                }
            }
        }
    }

    fn has_subtree(&self, leaves: &[u64]) -> bool {
        self.leaf_ids() == leaves || matches!(self, Node::Split { a, b, .. } if a.has_subtree(leaves) || b.has_subtree(leaves))
    }

    fn set_ratio_of_parent(&mut self, id: u64, r: f32) -> bool {
        match self {
            Node::Leaf(_) => false,
            Node::Split { ratio, a, b, .. } => {
                if matches!(a.as_ref(), Node::Leaf(x) if *x == id) || matches!(b.as_ref(), Node::Leaf(x) if *x == id) {
                    *ratio = r;
                    return true;
                }
                a.set_ratio_of_parent(id, r) || b.set_ratio_of_parent(id, r)
            }
        }
    }

    /// Gleichmäßiges Raster aus den Sitzungen (abwechselnd horizontal/vertikal geteilt).
    pub fn grid(ids: &[u64], dir: Dir) -> Option<Node> {
        match ids.len() {
            0 => None,
            1 => Some(Node::Leaf(ids[0])),
            n => {
                let (l, r) = ids.split_at(n.div_ceil(2));
                let next = if dir == Dir::Horizontal { Dir::Vertical } else { Dir::Horizontal };
                Some(Node::Split { dir, ratio: l.len() as f32 / n as f32, a: Box::new(Node::grid(l, next)?), b: Box::new(Node::grid(r, next)?) })
            }
        }
    }
    pub fn leaves(&self, out: &mut Vec<u64>) {
        match self {
            Node::Leaf(id) => out.push(*id),
            Node::Split { a, b, .. } => {
                a.leaves(out);
                b.leaves(out);
            }
        }
    }

    pub fn contains(&self, id: u64) -> bool {
        let mut v = Vec::new();
        self.leaves(&mut v);
        v.contains(&id)
    }

    /// Ersetzt das Blatt `target` durch einen Split aus `target` und `new_id`.
    #[cfg(test)]
    pub fn split_leaf(&mut self, target: u64, new_id: u64, dir: Dir) -> bool {
        match self {
            Node::Leaf(id) if *id == target => {
                *self = Node::Split {
                    dir,
                    ratio: 0.5,
                    a: Box::new(Node::Leaf(target)),
                    b: Box::new(Node::Leaf(new_id)),
                };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => a.split_leaf(target, new_id, dir) || b.split_leaf(target, new_id, dir),
        }
    }

    /// Teilt das Blatt `target`; `new_id` landet davor (links/oben) oder dahinter (rechts/unten).
    pub fn split_leaf_at(&mut self, target: u64, new_id: u64, dir: Dir, before: bool) -> bool {
        match self {
            Node::Leaf(id) if *id == target => {
                let (a, b) = if before { (new_id, target) } else { (target, new_id) };
                *self = Node::Split { dir, ratio: 0.5, a: Box::new(Node::Leaf(a)), b: Box::new(Node::Leaf(b)) };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => a.split_leaf_at(target, new_id, dir, before) || b.split_leaf_at(target, new_id, dir, before),
        }
    }

    /// Ersetzt das Blatt `target` durch einen ganzen Teilbaum.
    pub fn replace_leaf_with(&mut self, target: u64, sub: Node) -> bool {
        let mut sub = Some(sub);
        self.put_sub(target, &mut sub, None)
    }

    /// Teilt das Blatt `target` und setzt den Teilbaum davor (links/oben) oder dahinter.
    pub fn split_leaf_with(&mut self, target: u64, sub: Node, dir: Dir, before: bool) -> bool {
        let mut sub = Some(sub);
        self.put_sub(target, &mut sub, Some((dir, before)))
    }

    fn put_sub(&mut self, target: u64, sub: &mut Option<Node>, split: Option<(Dir, bool)>) -> bool {
        match self {
            Node::Leaf(id) if *id == target => {
                let Some(s) = sub.take() else { return false };
                *self = match split {
                    None => s,
                    Some((dir, before)) => {
                        let (a, b) = if before { (s, Node::Leaf(target)) } else { (Node::Leaf(target), s) };
                        Node::Split { dir, ratio: 0.5, a: Box::new(a), b: Box::new(b) }
                    }
                };
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => a.put_sub(target, sub, split) || b.put_sub(target, sub, split),
        }
    }

    /// Vertauscht zwei Sitzungen im Baum.
    pub fn swap_leaves(&mut self, x: u64, y: u64) {
        match self {
            Node::Leaf(id) => {
                if *id == x {
                    *id = y;
                } else if *id == y {
                    *id = x;
                }
            }
            Node::Split { a, b, .. } => {
                a.swap_leaves(x, y);
                b.swap_leaves(x, y);
            }
        }
    }

    /// Ersetzt die Sitzung eines Blatts.
    pub fn replace_leaf(&mut self, old: u64, new: u64) -> bool {
        match self {
            Node::Leaf(id) if *id == old => {
                *id = new;
                true
            }
            Node::Leaf(_) => false,
            Node::Split { a, b, .. } => a.replace_leaf(old, new) || b.replace_leaf(old, new),
        }
    }

    /// Entfernt ein Blatt; der Geschwisterknoten rückt nach. Gibt `None` zurück,
    /// wenn danach nichts übrig bleibt.
    pub fn remove_leaf(self, target: u64) -> Option<Node> {
        match self {
            Node::Leaf(id) if id == target => None,
            Node::Leaf(_) => Some(self),
            Node::Split { dir, ratio, a, b } => match (a.remove_leaf(target), b.remove_leaf(target)) {
                (Some(a), Some(b)) => Some(Node::Split { dir, ratio, a: Box::new(a), b: Box::new(b) }),
                (Some(x), None) | (None, Some(x)) => Some(x),
                (None, None) => None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtree_insert() {
        let mut n = Node::Leaf(1);
        assert!(n.split_leaf_with(1, Node::grid(&[2, 3, 4], Dir::Horizontal).unwrap(), Dir::Horizontal, false));
        let mut v = Vec::new();
        n.leaves(&mut v);
        assert_eq!(v, vec![1, 2, 3, 4]);
        assert!(n.replace_leaf_with(1, Node::Leaf(9)));
        let mut v = Vec::new();
        n.leaves(&mut v);
        assert_eq!(v, vec![9, 2, 3, 4]);
        assert!(!n.replace_leaf_with(77, Node::Leaf(5)));
    }

    #[test]
    fn split_at_and_swap() {
        let mut n = Node::Leaf(1);
        n.split_leaf(1, 2, Dir::Horizontal);
        assert!(n.split_leaf_at(2, 3, Dir::Vertical, true));
        let mut v = vec![];
        n.leaves(&mut v);
        assert_eq!(v, vec![1, 3, 2]);
        n.swap_leaves(1, 2);
        let mut v = vec![];
        n.leaves(&mut v);
        assert_eq!(v, vec![2, 3, 1]);
    }

    fn split(dir: Dir, ratio: f32, a: Node, b: Node) -> Node {
        Node::Split { dir, ratio, a: Box::new(a), b: Box::new(b) }
    }

    #[test]
    fn normalize_drops_unknown_and_duplicate_leaves_and_collapses() {
        // 1 | (2 / 1) | 9 – 9 ist unbekannt, die zweite 1 ein Duplikat
        let n = split(Dir::Horizontal, 0.5, Node::Leaf(1), split(Dir::Horizontal, 0.5, split(Dir::Vertical, 0.5, Node::Leaf(2), Node::Leaf(1)), Node::Leaf(9)));
        assert!(!n.is_normal(|id| id != 9));
        let n = n.normalize(|id| id != 9).unwrap();
        assert_eq!(n, split(Dir::Horizontal, 0.5, Node::Leaf(1), Node::Leaf(2)), "einseitige Teilungen fallen zusammen");
        assert!(n.is_normal(|id| id != 9));
        assert!(Node::Leaf(3).normalize(|_| false).is_none(), "nichts übrig → kein Layout");
        assert!(split(Dir::Vertical, 0.5, Node::Leaf(4), Node::Leaf(4)).normalize(|_| true) == Some(Node::Leaf(4)));
    }

    #[test]
    fn normalize_clamps_ratios_and_rejects_nan() {
        for (r, want) in [(f32::NAN, 0.5), (f32::INFINITY, 0.5), (-3.0, RATIO_MIN), (0.0, RATIO_MIN), (1.0, RATIO_MAX), (7.0, RATIO_MAX), (0.3, 0.3)] {
            let n = split(Dir::Horizontal, r, Node::Leaf(1), Node::Leaf(2));
            assert_eq!(n.is_normal(|_| true), r == 0.3, "{r}");
            match n.normalize(|_| true) {
                Some(Node::Split { ratio, .. }) => assert_eq!(ratio, want, "{r}"),
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(clamp_ratio(f32::NEG_INFINITY), 0.5);
    }

    #[test]
    fn broken_layouts_from_a_file_load_leniently() {
        #[derive(Deserialize)]
        struct F {
            #[serde(default, deserialize_with = "lenient")]
            layout: Option<Node>,
        }
        let parse = |s: &str| serde_json::from_str::<F>(s).unwrap().layout;
        // NaN wird von serde_json als null geschrieben – das darf die Datei nicht unlesbar machen.
        let n = parse(r#"{"layout":{"Split":{"dir":"Horizontal","ratio":null,"a":{"Leaf":1},"b":{"Leaf":2}}}}"#).unwrap();
        assert_eq!(n, split(Dir::Horizontal, 0.5, Node::Leaf(1), Node::Leaf(2)));
        let n = parse(r#"{"layout":{"Split":{"dir":"Vertical","ratio":5,"a":{"Leaf":1},"b":{"Leaf":2}}}}"#).unwrap();
        assert!(matches!(n, Node::Split { ratio, .. } if ratio == RATIO_MAX));
        assert!(parse(r#"{"layout":{"Split":{"dir":"Diagonal","ratio":0.5,"a":{"Leaf":1}}}}"#).is_none(), "kaputter Knoten → kein Layout");
        assert!(parse(r#"{"layout":"egal"}"#).is_none());
        assert!(parse(r#"{}"#).is_none());
        assert!(parse(r#"{"layout":null}"#).is_none());
        // Rundreise
        let n = split(Dir::Vertical, 0.25, Node::Leaf(3), split(Dir::Horizontal, 0.5, Node::Leaf(4), Node::Leaf(5)));
        let s = format!("{{\"layout\":{}}}", serde_json::to_string(&n).unwrap());
        assert_eq!(parse(&s), Some(n));
    }

    #[test]
    fn minimized_pane_comes_back_where_it_was() {
        // (1 | 2) / 3, 3 unten mit 30 %
        let orig = split(Dir::Vertical, 0.7, split(Dir::Horizontal, 0.4, Node::Leaf(1), Node::Leaf(2)), Node::Leaf(3));
        for id in [1, 2, 3] {
            let (rest, slot) = orig.clone().remove_leaf_slot(id);
            let mut rest = rest.unwrap();
            let slot = slot.unwrap();
            assert!(!rest.contains(id));
            assert!(rest.insert_at_slot(id, &slot), "{id}");
            assert_eq!(rest, orig, "{id}: gleiche Stelle, Richtung und Anteil");
        }
        // Allein: kein Slot
        let (rest, slot) = Node::Leaf(1).remove_leaf_slot(1);
        assert!(rest.is_none() && slot.is_none());
        // Unbekanntes Blatt: unverändert, kein Slot
        let (rest, slot) = orig.clone().remove_leaf_slot(9);
        assert_eq!(rest, Some(orig.clone()));
        assert!(slot.is_none());
    }

    #[test]
    fn minimized_pane_falls_back_to_a_remaining_sibling_leaf() {
        // 3 minimiert (Geschwister: 1|2), danach wird 1 geschlossen → 3 kommt unter 2 zurück
        let orig = split(Dir::Vertical, 0.7, split(Dir::Horizontal, 0.4, Node::Leaf(1), Node::Leaf(2)), Node::Leaf(3));
        let (rest, slot) = orig.remove_leaf_slot(3);
        let slot = slot.unwrap();
        assert_eq!(slot.sibling, vec![1, 2]);
        let mut rest = rest.unwrap().remove_leaf(1).unwrap();
        assert!(rest.insert_at_slot(3, &slot));
        assert_eq!(rest, split(Dir::Vertical, 0.7, Node::Leaf(2), Node::Leaf(3)));
        // Geschwister ganz weg → false, Baum unverändert
        let mut other = Node::Leaf(8);
        assert!(!other.insert_at_slot(3, &slot));
        assert_eq!(other, Node::Leaf(8));
        // schon drin → false
        let mut has = split(Dir::Horizontal, 0.5, Node::Leaf(1), Node::Leaf(3));
        assert!(!has.insert_at_slot(3, &slot));
        // Geschwister umsortiert (2|1): erstes vorhandenes Blatt wird geteilt
        let mut swapped = split(Dir::Horizontal, 0.4, Node::Leaf(2), Node::Leaf(1));
        assert!(swapped.insert_at_slot(3, &slot));
        assert_eq!(swapped.leaf_count(), 3);
        assert!(swapped.is_normal(|_| true));
    }

    #[test]
    fn slot_round_trips_and_loads_leniently() {
        let s = Slot { sibling: vec![1, 2], dir: Dir::Horizontal, before: true, ratio: 0.3 };
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Slot>(&j).unwrap(), s);
        #[derive(Deserialize)]
        struct F {
            #[serde(default, deserialize_with = "lenient_slot")]
            slot: Option<Slot>,
        }
        assert!(serde_json::from_str::<F>(r#"{"slot":{"sibling":"x"}}"#).unwrap().slot.is_none());
        assert!(serde_json::from_str::<F>(r#"{}"#).unwrap().slot.is_none());
    }

    #[test]
    fn show_follows_the_reinsert_rule() {
        let h = |a: u64, b: u64| split(Dir::Horizontal, 0.5, Node::Leaf(a), Node::Leaf(b));
        // 1. leerer Hub → einziger Bereich (auch minimiert)
        let mut l = None;
        show(&mut l, 5, 0, true, None);
        assert_eq!(l, Some(Node::Leaf(5)));
        // schon drin → unverändert
        let mut l = Some(h(1, 2));
        show(&mut l, 2, 1, true, None);
        assert_eq!(l, Some(h(1, 2)));
        // 2. minimiert, Platz noch da → an alter Stelle
        let orig = split(Dir::Vertical, 0.3, Node::Leaf(3), h(1, 2));
        let mut l = Some(orig.clone());
        let (slot, focus) = minimize(&mut l, 3, 3).unwrap();
        assert_eq!(focus, 1, "Fokus geht an den Nachbarn darunter");
        show(&mut l, 3, focus, true, slot.as_ref());
        assert_eq!(l, Some(orig));
        // 3. minimiert, Geschwister weg → fokussierten Bereich rechts teilen
        let slot = Slot { sibling: vec![42], dir: Dir::Vertical, before: true, ratio: 0.3 };
        let mut l = Some(h(1, 2));
        show(&mut l, 3, 2, true, Some(&slot));
        assert_eq!(l, Some(split(Dir::Horizontal, 0.5, Node::Leaf(1), h(2, 3))));
        // minimiert ohne Slot (war der letzte Bereich) → ebenso
        let mut l = Some(Node::Leaf(1));
        show(&mut l, 3, 1, true, None);
        assert_eq!(l, Some(h(1, 3)));
        // 4. nur weggewechselt → fokussierten Bereich ersetzen
        let mut l = Some(h(1, 2));
        show(&mut l, 3, 2, false, None);
        assert_eq!(l, Some(h(1, 3)));
        // Fokus unbekannt → erster Bereich
        let mut l = Some(h(1, 2));
        show(&mut l, 3, 99, false, None);
        assert_eq!(l, Some(h(3, 2)));
        let mut l = Some(h(1, 2));
        show(&mut l, 3, 99, true, None);
        assert_eq!(l, Some(split(Dir::Horizontal, 0.5, h(1, 3), Node::Leaf(2))));
        assert!(l.unwrap().is_normal(|_| true));
    }

    #[test]
    fn minimize_keeps_focus_or_moves_it_to_the_neighbour() {
        // 1 | 2 | 3 (geschachtelt: 1 | (2 | 3))
        let orig = split(Dir::Horizontal, 0.33, Node::Leaf(1), split(Dir::Horizontal, 0.5, Node::Leaf(2), Node::Leaf(3)));
        // anderer Bereich fokussiert → Fokus bleibt
        let mut l = Some(orig.clone());
        let (_, f) = minimize(&mut l, 2, 1).unwrap();
        assert_eq!(f, 1);
        // fokussierter rechter Bereich → linker Nachbar (letztes Blatt der Geschwister)
        let mut l = Some(split(Dir::Horizontal, 0.5, split(Dir::Vertical, 0.5, Node::Leaf(1), Node::Leaf(2)), Node::Leaf(3)));
        let (slot, f) = minimize(&mut l, 3, 3).unwrap();
        assert_eq!(f, 2);
        assert_eq!(slot.unwrap().sibling, vec![1, 2]);
        // letzter Bereich → leerer Hub, kein Slot, Fokus 0
        let mut l = Some(Node::Leaf(7));
        assert_eq!(minimize(&mut l, 7, 7), Some((None, 0)));
        assert!(l.is_none());
        // nicht im Layout → nichts
        let mut l = Some(orig.clone());
        assert!(minimize(&mut l, 9, 1).is_none());
        assert_eq!(l, Some(orig));
        let mut none = None;
        assert!(minimize(&mut none, 1, 1).is_none());
    }

    #[test]
    fn slots_follow_a_chain_of_minimized_and_closed_panes() {
        let h = |a: u64, b: u64, r: f32| split(Dir::Horizontal, r, Node::Leaf(a), Node::Leaf(b));
        for close in [false, true] {
            // H(1,2) → 1 minimieren → 2 rechts teilen (2|3) → 2 minimieren bzw. schließen → 1 zurück: links neben 3, 30 %
            let mut l = Some(h(1, 2, 0.3));
            let (s1, f) = minimize(&mut l, 1, 1).unwrap();
            let mut s1 = s1.unwrap();
            assert_eq!(l, Some(Node::Leaf(2)));
            show(&mut l, 3, f, true, None); // neue Sitzung 3 teilt 2
            assert_eq!(l, Some(h(2, 3, 0.5)));
            let (rest, s2) = l.take().unwrap().remove_leaf_slot(2); // minimieren oder schließen: gleiche Baumänderung
            l = rest;
            reroute_sibling(&mut s1, 2, &s2.map(|s| s.sibling).unwrap_or_default());
            assert_eq!(s1.sibling, vec![3], "close={close}");
            show(&mut l, 1, 3, true, Some(&s1));
            assert_eq!(l, Some(h(1, 3, 0.3)), "close={close}: genau an der alten Stelle");
        }
        // Teilbaum-Geschwister: 9 minimiert neben (2|4); 2 geht (Geschwister 4) → (4)
        let mut s = Slot { sibling: vec![2, 4], dir: Dir::Vertical, before: false, ratio: 0.5 };
        reroute_sibling(&mut s, 2, &[4]);
        assert_eq!(s.sibling, vec![4]);
        // ohne eigenen Slot (war allein): nur streichen; fremdes Blatt: unverändert
        reroute_sibling(&mut s, 4, &[]);
        assert!(s.sibling.is_empty());
        let mut t = Slot { sibling: vec![5], dir: Dir::Vertical, before: false, ratio: 0.5 };
        reroute_sibling(&mut t, 8, &[1]);
        assert_eq!(t.sibling, vec![5]);
    }

    #[test]
    fn minimize_everything_then_restore_in_reverse_and_forward_order() {
        let orig = split(Dir::Vertical, 0.6, split(Dir::Horizontal, 0.4, Node::Leaf(1), Node::Leaf(2)), split(Dir::Horizontal, 0.7, Node::Leaf(3), Node::Leaf(4)));
        let mut l = Some(orig.clone());
        let mut slots = std::collections::HashMap::new();
        let mut focus = 1;
        for id in [2, 4, 1, 3] {
            let (s, f) = minimize(&mut l, id, focus).unwrap();
            slots.insert(id, s);
            focus = f;
            if let Some(n) = &l {
                assert!(n.is_normal(|_| true));
            }
        }
        assert!(l.is_none(), "alle minimiert → leerer Hub");
        // umgekehrte Reihenfolge stellt das Original exakt wieder her
        for id in [3, 1, 4, 2] {
            show(&mut l, id, focus, true, slots[&id].as_ref());
            focus = id;
        }
        assert_eq!(l, Some(orig.clone()));
        // Vorwärts (gleiche Reihenfolge wie beim Minimieren): nicht unbedingt dieselbe Anordnung, aber alle Bereiche
        // erreichbar, jeder genau einmal, Baum normal
        let mut l = Some(orig.clone());
        let mut slots = std::collections::HashMap::new();
        let mut focus = 1;
        for id in [2, 4, 1, 3] {
            let (s, f) = minimize(&mut l, id, focus).unwrap();
            slots.insert(id, s);
            focus = f;
        }
        for id in [2, 4, 1, 3] {
            show(&mut l, id, focus, true, slots[&id].as_ref());
            focus = id;
            let n = l.as_ref().unwrap();
            assert!(n.contains(id) && n.is_normal(|_| true), "{id}: {n:?}");
        }
        let mut got = l.unwrap().leaf_ids();
        got.sort();
        assert_eq!(got, vec![1, 2, 3, 4]);
    }

    #[test]
    fn len_and_leaf_ids() {
        let n = Node::grid(&[5, 6, 7], Dir::Horizontal).unwrap();
        assert_eq!(n.leaf_count(), 3);
        assert_eq!(n.leaf_ids(), vec![5, 6, 7]);
        assert!(n.is_normal(|_| true), "grid erzeugt gültige Anteile");
    }

    #[test]
    fn split_and_remove() {
        let mut n = Node::Leaf(1);
        assert!(n.split_leaf(1, 2, Dir::Horizontal));
        assert!(n.split_leaf(2, 3, Dir::Vertical));
        let mut v = vec![];
        n.leaves(&mut v);
        assert_eq!(v, vec![1, 2, 3]);
        let n = n.remove_leaf(2).unwrap();
        let mut v = vec![];
        n.leaves(&mut v);
        assert_eq!(v, vec![1, 3]);
        assert!(n.remove_leaf(1).unwrap().remove_leaf(3).is_none());
    }
}
