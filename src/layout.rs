//! Frei teilbare Flächen als Baum: Blätter sind Sitzungs-IDs.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dir {
    /// Nebeneinander (vertikale Trennlinie).
    Horizontal,
    /// Übereinander (horizontale Trennlinie).
    Vertical,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Node {
    Leaf(u64),
    Split { dir: Dir, ratio: f32, a: Box<Node>, b: Box<Node> },
}

impl Node {
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
