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

/// Sitzung `id` zeigen (Regel von `App::activate`). Steht sie schon im Layout, bleibt alles, wie es ist. Sonst:
/// 1. kein Layout (leerer Hub) → sie wird der einzige Bereich;
/// 2. `minimized` → sie öffnet sich **allein**: Sie ersetzt das ganze Layout, die bisher sichtbaren Bereiche verlassen es
///    (der Aufrufer markiert sie als minimiert; sie laufen weiter);
/// 3. sonst (nur weggewechselt) → den fokussierten Bereich (`focused`, sonst den ersten) ersetzen.
///
/// Liefert die Bereiche, die durch Regel 2 das Layout verlassen haben (in Reihenfolge), sonst eine leere Liste.
pub fn show(layout: &mut Option<Node>, id: u64, focused: u64, minimized: bool) -> Vec<u64> {
    let Some(l) = layout else {
        *layout = Some(Node::Leaf(id));
        return Vec::new();
    };
    if l.contains(id) {
        return Vec::new();
    }
    if minimized {
        let gone = l.leaf_ids();
        *layout = Some(Node::Leaf(id));
        return gone;
    }
    if !l.replace_leaf(focused, id) {
        let first = l.leaf_ids()[0];
        l.replace_leaf(first, id);
    }
    Vec::new()
}

/// Bereich `id` minimieren: aus dem Layout nehmen (auch den letzten – dann `None` = leerer Hub). Sein Platz wird nicht
/// gemerkt (seit 2.3.0 öffnet ein minimierter Bereich beim Zurückholen allein, siehe `show`). Liefert den neuen Fokus
/// (bleibt `focused`, wenn das noch im Layout ist, sonst der direkte Nachbar; 0 = nichts mehr da), oder `None`, wenn `id`
/// gar nicht im Layout stand.
pub fn minimize(layout: &mut Option<Node>, id: u64, focused: u64) -> Option<u64> {
    let l = layout.take()?;
    if !l.contains(id) {
        *layout = Some(l);
        return None;
    }
    let near = l.neighbour_of(id);
    *layout = l.remove_leaf(id);
    let has = |x: u64| layout.as_ref().is_some_and(|l| l.contains(x));
    let focus = if has(focused) {
        focused
    } else {
        near.filter(|n| has(*n)).or_else(|| layout.as_ref().map(|l| l.leaf_ids()[0])).unwrap_or(0)
    };
    Some(focus)
}

/// Wohin ein Agent eine Sitzung bringt (`focus_session`/`create_session` mit `placement`, #162). Bezugspunkt ist der
/// „aktive Bereich“ (`anchor`): der Bereich der eigenen Sitzung des Agenten, wenn sie zu sehen ist, sonst der Bereich, den
/// der Nutzer fokussiert hat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// Einziger Bereich; alle anderen werden minimiert (wie ein Klick in der Seitenleiste auf eine minimierte Sitzung).
    Solo,
    /// Ersetzt nur den aktiven Bereich; der Rest der Teilung bleibt (Standard für Agenten).
    Replace,
    /// Teilt nur den aktiven Bereich, die Sitzung kommt in die rechte Hälfte.
    SplitRight,
    /// Teilt nur den aktiven Bereich, die Sitzung kommt in die untere Hälfte.
    SplitDown,
}

impl Placement {
    /// Werte der MCP-Schnittstelle (öffentliche API).
    pub const NAMES: [&'static str; 4] = ["solo", "replace", "split_right", "split_down"];

    pub fn parse(s: &str) -> Option<Placement> {
        match s {
            "solo" => Some(Placement::Solo),
            "replace" => Some(Placement::Replace),
            "split_right" => Some(Placement::SplitRight),
            "split_down" => Some(Placement::SplitDown),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Placement::Solo => "solo",
            Placement::Replace => "replace",
            Placement::SplitRight => "split_right",
            Placement::SplitDown => "split_down",
        }
    }
}

/// Ergebnis von [`place`]: welche Bereiche das Layout verlassen haben.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Placed {
    /// Verdrängt (`Solo`: alle übrigen, `Replace`: der ersetzte Bereich) – der Aufrufer markiert sie als minimiert (sie
    /// laufen weiter; Strg+Tab erreicht sie, ein Klick öffnet sie allein).
    pub minimized: Vec<u64>,
}

/// Sitzung `id` nach `placement` ins Layout bringen, bezogen auf den aktiven Bereich `anchor` (steht er nicht im Layout,
/// gilt der erste Bereich). Ohne Layout (leerer Hub) wird sie in jedem Fall der einzige Bereich. Steht sie schon im Layout,
/// ändert nur `Solo` etwas; `Replace` und die Teilungen lassen das Layout dann, wie es ist.
///
/// `protected`: Bereiche, die nie verdrängt werden (Freigabe-Frage offen, Kette des Agenten – Review zu #162). `Solo` lässt
/// sie stehen (die Sitzung kommt links daneben), `Replace` teilt einen geschützten aktiven Bereich statt ihn zu ersetzen.
pub fn place(layout: &mut Option<Node>, id: u64, anchor: u64, placement: Placement, protected: &dyn Fn(u64) -> bool) -> Placed {
    let Some(l) = layout.take() else {
        *layout = Some(Node::Leaf(id));
        return Placed::default();
    };
    if placement == Placement::Solo {
        let gone: Vec<u64> = l.leaf_ids().into_iter().filter(|x| *x != id && !protected(*x)).collect();
        let mut rest = Some(l);
        for g in &gone {
            rest = rest.and_then(|r| r.remove_leaf(*g));
        }
        *layout = Some(match rest {
            None => Node::Leaf(id),
            Some(r) if r.contains(id) => r,
            Some(r) => Node::Split { dir: Dir::Horizontal, ratio: 0.5, a: Box::new(Node::Leaf(id)), b: Box::new(r) },
        });
        return Placed { minimized: gone };
    }
    let mut l = l;
    if l.contains(id) {
        *layout = Some(l);
        return Placed::default();
    }
    let anchor = if l.contains(anchor) { anchor } else { l.leaf_ids()[0] };
    let placed = match placement {
        Placement::Replace if !protected(anchor) => {
            l.replace_leaf(anchor, id);
            Placed { minimized: vec![anchor] }
        }
        _ => {
            let dir = if placement == Placement::SplitDown { Dir::Vertical } else { Dir::Horizontal };
            l.split_leaf_at(anchor, id, dir, false);
            Placed::default()
        }
    };
    *layout = Some(l);
    placed
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

    /// Das Blatt, das an `target` angrenzt: Ist `target` eine Seite einer Teilung, das nächstgelegene Blatt der anderen
    /// Seite (deren erstes, wenn `target` davor lag, sonst deren letztes). `None`, wenn `target` fehlt oder allein ist.
    fn neighbour_of(&self, target: u64) -> Option<u64> {
        let Node::Split { a, b, .. } = self else { return None };
        match (a.as_ref(), b.as_ref()) {
            (Node::Leaf(x), other) if *x == target => other.leaf_ids().first().copied(),
            (other, Node::Leaf(x)) if *x == target => other.leaf_ids().last().copied(),
            _ => a.neighbour_of(target).or_else(|| b.neighbour_of(target)),
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
    fn placement_names_roundtrip() {
        for n in Placement::NAMES {
            assert_eq!(Placement::parse(n).unwrap().name(), n);
        }
        assert_eq!(Placement::parse("Solo"), None);
        assert_eq!(Placement::parse(""), None);
    }

    #[test]
    fn place_acts_only_on_the_active_pane() {
        let free = |_: u64| false;
        // (1 | 2) / 3, aktiver Bereich 2
        let base = || split(Dir::Vertical, 0.5, split(Dir::Horizontal, 0.5, Node::Leaf(1), Node::Leaf(2)), Node::Leaf(3));
        // replace: nur 2 wird ersetzt (und minimiert), der Rest der Teilung bleibt
        let mut l = Some(base());
        assert_eq!(place(&mut l, 9, 2, Placement::Replace, &free), Placed { minimized: vec![2] });
        assert_eq!(l, Some(split(Dir::Vertical, 0.5, split(Dir::Horizontal, 0.5, Node::Leaf(1), Node::Leaf(9)), Node::Leaf(3))));
        // split_right: nur 2 wird geteilt, 9 rechts daneben
        let mut l = Some(base());
        assert_eq!(place(&mut l, 9, 2, Placement::SplitRight, &free), Placed::default());
        let want = split(Dir::Vertical, 0.5, split(Dir::Horizontal, 0.5, Node::Leaf(1), split(Dir::Horizontal, 0.5, Node::Leaf(2), Node::Leaf(9))), Node::Leaf(3));
        assert_eq!(l, Some(want));
        // split_down: 9 unter 3
        let mut l = Some(base());
        place(&mut l, 9, 3, Placement::SplitDown, &free);
        assert_eq!(l, Some(split(Dir::Vertical, 0.5, split(Dir::Horizontal, 0.5, Node::Leaf(1), Node::Leaf(2)), split(Dir::Vertical, 0.5, Node::Leaf(3), Node::Leaf(9)))));
        // solo: allein, die anderen verlassen das Layout (der Aufrufer minimiert sie)
        let mut l = Some(base());
        let r = place(&mut l, 9, 2, Placement::Solo, &free);
        assert_eq!((l, r.minimized), (Some(Node::Leaf(9)), vec![1, 2, 3]));
        // solo mit einer schon sichtbaren Sitzung: sie bleibt allein übrig
        let mut l = Some(base());
        assert_eq!(place(&mut l, 2, 1, Placement::Solo, &free).minimized, vec![1, 3]);
        assert_eq!(l, Some(Node::Leaf(2)));
        // schon sichtbar: replace/split ändern nichts
        for p in [Placement::Replace, Placement::SplitRight, Placement::SplitDown] {
            let mut l = Some(base());
            assert_eq!(place(&mut l, 3, 1, p, &free), Placed::default());
            assert_eq!(l, Some(base()), "{p:?}");
        }
        // aktiver Bereich nicht im Layout: der erste Bereich
        let mut l = Some(base());
        assert_eq!(place(&mut l, 9, 42, Placement::Replace, &free).minimized, vec![1]);
        // leerer Hub: einziger Bereich, egal wie
        for p in [Placement::Solo, Placement::Replace, Placement::SplitRight, Placement::SplitDown] {
            let mut none = None;
            assert_eq!(place(&mut none, 5, 0, p, &free), Placed::default());
            assert_eq!(none, Some(Node::Leaf(5)));
        }
    }

    #[test]
    fn place_never_displaces_protected_panes() {
        // (1 | 2) / 3; 2 wartet auf eine Freigabe bzw. gehört zur Kette des Agenten
        let base = || split(Dir::Vertical, 0.5, split(Dir::Horizontal, 0.5, Node::Leaf(1), Node::Leaf(2)), Node::Leaf(3));
        let prot = |x: u64| x == 2;
        // replace auf den geschützten aktiven Bereich: teilen statt ersetzen, nichts minimiert
        let mut l = Some(base());
        assert_eq!(place(&mut l, 9, 2, Placement::Replace, &prot), Placed::default());
        assert_eq!(l, Some(split(Dir::Vertical, 0.5, split(Dir::Horizontal, 0.5, Node::Leaf(1), split(Dir::Horizontal, 0.5, Node::Leaf(2), Node::Leaf(9))), Node::Leaf(3))));
        // replace auf einen anderen Bereich: normal
        let mut l = Some(base());
        assert_eq!(place(&mut l, 9, 1, Placement::Replace, &prot).minimized, vec![1]);
        // solo: der geschützte Bereich bleibt stehen, die Sitzung links daneben
        let mut l = Some(base());
        assert_eq!(place(&mut l, 9, 1, Placement::Solo, &prot).minimized, vec![1, 3]);
        assert_eq!(l, Some(split(Dir::Horizontal, 0.5, Node::Leaf(9), Node::Leaf(2))));
        // solo mit einer schon sichtbaren Sitzung: geschützte bleiben, Struktur dazwischen bleibt
        let mut l = Some(base());
        assert_eq!(place(&mut l, 3, 1, Placement::Solo, &prot).minimized, vec![1]);
        assert_eq!(l, Some(split(Dir::Vertical, 0.5, Node::Leaf(2), Node::Leaf(3))));
        // alles geschützt: solo minimiert nichts
        let mut l = Some(base());
        assert!(place(&mut l, 9, 1, Placement::Solo, &|_| true).minimized.is_empty());
        assert_eq!(l.unwrap().leaf_ids(), vec![9, 1, 2, 3]);
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
    fn show_follows_the_open_rule() {
        let h = |a: u64, b: u64| split(Dir::Horizontal, 0.5, Node::Leaf(a), Node::Leaf(b));
        // 1. leerer Hub → einziger Bereich (minimiert oder nicht), niemand verdrängt
        for min in [true, false] {
            let mut l = None;
            assert!(show(&mut l, 5, 0, min).is_empty());
            assert_eq!(l, Some(Node::Leaf(5)));
        }
        // schon drin → unverändert (auch wenn noch als minimiert markiert)
        for min in [true, false] {
            let mut l = Some(h(1, 2));
            assert!(show(&mut l, 2, 1, min).is_empty());
            assert_eq!(l, Some(h(1, 2)));
        }
        // 2. minimiert → allein; alle bisher sichtbaren Bereiche verlassen das Layout (in Reihenfolge)
        let mut l = Some(split(Dir::Vertical, 0.3, Node::Leaf(3), h(1, 2)));
        assert_eq!(show(&mut l, 7, 1, true), vec![3, 1, 2]);
        assert_eq!(l, Some(Node::Leaf(7)));
        // auch bei unbekanntem Fokus
        let mut l = Some(Node::Leaf(4));
        assert_eq!(show(&mut l, 7, 99, true), vec![4]);
        assert_eq!(l, Some(Node::Leaf(7)));
        // 3. nur weggewechselt → fokussierten Bereich ersetzen, sonst niemand verdrängt
        let mut l = Some(h(1, 2));
        assert!(show(&mut l, 3, 2, false).is_empty());
        assert_eq!(l, Some(h(1, 3)));
        // Fokus unbekannt → erster Bereich
        let mut l = Some(h(1, 2));
        assert!(show(&mut l, 3, 99, false).is_empty());
        assert_eq!(l, Some(h(3, 2)));
        assert!(l.unwrap().is_normal(|_| true));
    }

    #[test]
    fn minimize_keeps_focus_or_moves_it_to_the_neighbour() {
        // 1 | 2 | 3 (geschachtelt: 1 | (2 | 3))
        let orig = split(Dir::Horizontal, 0.33, Node::Leaf(1), split(Dir::Horizontal, 0.5, Node::Leaf(2), Node::Leaf(3)));
        // anderer Bereich fokussiert → Fokus bleibt
        let mut l = Some(orig.clone());
        assert_eq!(minimize(&mut l, 2, 1), Some(1));
        assert_eq!(l, Some(split(Dir::Horizontal, 0.33, Node::Leaf(1), Node::Leaf(3))));
        // fokussierter rechter Bereich → linker Nachbar (letztes Blatt der Geschwister)
        let mut l = Some(split(Dir::Horizontal, 0.5, split(Dir::Vertical, 0.5, Node::Leaf(1), Node::Leaf(2)), Node::Leaf(3)));
        assert_eq!(minimize(&mut l, 3, 3), Some(2));
        // fokussierter linker Bereich → rechter Nachbar (erstes Blatt der Geschwister)
        let mut l = Some(split(Dir::Horizontal, 0.5, Node::Leaf(3), split(Dir::Vertical, 0.5, Node::Leaf(1), Node::Leaf(2))));
        assert_eq!(minimize(&mut l, 3, 3), Some(1));
        // letzter Bereich → leerer Hub, Fokus 0
        let mut l = Some(Node::Leaf(7));
        assert_eq!(minimize(&mut l, 7, 7), Some(0));
        assert!(l.is_none());
        // nicht im Layout → nichts
        let mut l = Some(orig.clone());
        assert!(minimize(&mut l, 9, 1).is_none());
        assert_eq!(l, Some(orig));
        let mut none = None;
        assert!(minimize(&mut none, 1, 1).is_none());
    }

    #[test]
    fn restoring_minimized_panes_swaps_them_one_by_one() {
        // Alles minimieren → leerer Hub; jeder geholte Bereich öffnet allein, der vorige wird minimiert.
        let orig = split(Dir::Vertical, 0.6, split(Dir::Horizontal, 0.4, Node::Leaf(1), Node::Leaf(2)), split(Dir::Horizontal, 0.7, Node::Leaf(3), Node::Leaf(4)));
        let mut l = Some(orig);
        let mut minimized: Vec<u64> = Vec::new();
        let mut focus = 1;
        for id in [2, 4, 1, 3] {
            focus = minimize(&mut l, id, focus).unwrap();
            minimized.push(id);
            if let Some(n) = &l {
                assert!(n.is_normal(|_| true));
            }
        }
        assert!(l.is_none() && focus == 0, "alle minimiert → leerer Hub");
        let mut shown = None;
        for id in [3, 1, 4, 2] {
            let gone = show(&mut l, id, focus, true);
            assert_eq!(gone, shown.into_iter().collect::<Vec<_>>(), "{id}: nur der vorige verlässt das Layout");
            assert_eq!(l, Some(Node::Leaf(id)));
            focus = id;
            shown = Some(id);
        }
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
