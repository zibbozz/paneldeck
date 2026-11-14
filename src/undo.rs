//! Änderungshistorie (Rückgängig/Wiederholen), generisch über einen Operationstyp.
//!
//! Eine Operation beschreibt eine abgeschlossene Änderung und kann sich selbst ausführen (`apply`) und umkehren (`inverse`).
//! Die Liste bleibt komplett erhalten, auch wenn man zurückgeht (damit „Wiederholen“ wieder vorwärts geht); eine neue
//! Änderung nach einem Rückgängig verwirft die „Zukunft“. Die Historie ist begrenzt (älteste Einträge fallen zuerst weg)
//! und lebt nur im Speicher. Sie wird von der Todo-Liste benutzt, ist aber nicht darauf festgelegt.

/// Eine umkehrbare Änderung.
pub trait Op {
    /// Worauf die Änderung wirkt (z. B. eine Datenbankverbindung).
    type Ctx: ?Sized;
    /// Die Änderung ausführen.
    fn apply(&self, ctx: &Self::Ctx) -> Result<(), String>;
    /// Die Gegenoperation (`op.inverse().inverse()` ist wieder `op`).
    fn inverse(&self) -> Self;
}

pub struct History<O: Op> {
    ops: Vec<O>,
    /// Anzahl der „vergangenen“ Einträge: `ops[..cursor]` lassen sich zurücknehmen, `ops[cursor..]` wiederholen.
    cursor: usize,
    capacity: usize,
}

impl<O: Op> History<O> {
    pub fn new(capacity: usize) -> Self {
        Self { ops: Vec::new(), cursor: 0, capacity: capacity.max(1) }
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    #[cfg(test)]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    /// Die Änderung, die `undo` als Nächstes zurücknimmt (für Hinweise wie „Rückgängig: …“).
    pub fn next_undo(&self) -> Option<&O> {
        self.cursor.checked_sub(1).and_then(|i| self.ops.get(i))
    }

    /// Die Änderung, die `redo` als Nächstes wiederholt.
    pub fn next_redo(&self) -> Option<&O> {
        self.ops.get(self.cursor)
    }

    pub fn can_redo(&self) -> bool {
        self.cursor < self.ops.len()
    }

    /// Neue Größe (mindestens 1); zu alte Einträge fallen weg.
    pub fn set_capacity(&mut self, capacity: usize) {
        self.capacity = capacity.max(1);
        self.trim();
    }

    fn trim(&mut self) {
        if self.ops.len() > self.capacity {
            let drop = self.ops.len() - self.capacity;
            self.ops.drain(..drop);
            self.cursor = self.cursor.saturating_sub(drop);
        }
    }

    /// Eine bereits ausgeführte Änderung festhalten; verwirft die Wiederholen-Einträge.
    pub fn push(&mut self, op: O) {
        self.ops.truncate(self.cursor);
        self.ops.push(op);
        self.cursor = self.ops.len();
        self.trim();
    }

    /// Die letzte Änderung zurücknehmen. `Ok(false)` = nichts zu tun. Schlägt die Ausführung fehl (z. B. weil das Ziel
    /// inzwischen gelöscht wurde), wird der Eintrag verworfen und der Fehler zurückgegeben.
    pub fn undo(&mut self, ctx: &O::Ctx) -> Result<bool, String> {
        if self.cursor == 0 {
            return Ok(false);
        }
        let i = self.cursor - 1;
        match self.ops[i].inverse().apply(ctx) {
            Ok(()) => {
                self.cursor = i;
                Ok(true)
            }
            Err(e) => {
                self.ops.remove(i);
                self.cursor = i;
                Err(e)
            }
        }
    }

    /// Die zuletzt zurückgenommene Änderung wiederholen (Rückgabe wie bei `undo`).
    pub fn redo(&mut self, ctx: &O::Ctx) -> Result<bool, String> {
        if self.cursor >= self.ops.len() {
            return Ok(false);
        }
        match self.ops[self.cursor].apply(ctx) {
            Ok(()) => {
                self.cursor += 1;
                Ok(true)
            }
            Err(e) => {
                self.ops.remove(self.cursor);
                Err(e)
            }
        }
    }

    /// Einträge behalten, für die `keep` wahr ist (z. B. nach dem endgültigen Löschen eines Eintrags, auf den sich
    /// Änderungen beziehen). Die Position im Verlauf bleibt erhalten.
    pub fn retain(&mut self, mut keep: impl FnMut(&O) -> bool) {
        let mut idx = 0;
        let mut new_cursor = 0;
        let cursor = self.cursor;
        self.ops.retain(|op| {
            let k = keep(op);
            if k && idx < cursor {
                new_cursor += 1;
            }
            idx += 1;
            k
        });
        self.cursor = new_cursor;
    }

    #[cfg(test)]
    pub fn clear(&mut self) {
        self.ops.clear();
        self.cursor = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// Testoperation: addiert `delta` zu einem Zähler; `fail` lässt die Ausführung scheitern.
    #[derive(Clone, Debug, PartialEq)]
    struct Add {
        delta: i64,
        fail: bool,
    }
    impl Op for Add {
        type Ctx = RefCell<i64>;
        fn apply(&self, ctx: &RefCell<i64>) -> Result<(), String> {
            if self.fail {
                return Err("kaputt".into());
            }
            *ctx.borrow_mut() += self.delta;
            Ok(())
        }
        fn inverse(&self) -> Self {
            Add { delta: -self.delta, fail: self.fail }
        }
    }
    fn add(d: i64) -> Add {
        Add { delta: d, fail: false }
    }

    /// Änderung ausführen und festhalten, wie es die Aufrufer tun.
    fn do_op(h: &mut History<Add>, c: &RefCell<i64>, d: i64) {
        add(d).apply(c).unwrap();
        h.push(add(d));
    }

    #[test]
    fn undo_redo_roundtrip() {
        let c = RefCell::new(0);
        let mut h = History::new(10);
        assert!(!h.can_undo() && !h.can_redo());
        assert_eq!(h.undo(&c), Ok(false));
        assert_eq!(h.redo(&c), Ok(false));
        for d in [1, 10, 100] {
            do_op(&mut h, &c, d);
        }
        assert_eq!(*c.borrow(), 111);
        assert_eq!(h.undo(&c), Ok(true));
        assert_eq!(*c.borrow(), 11);
        assert_eq!(h.undo(&c), Ok(true));
        assert_eq!(h.undo(&c), Ok(true));
        assert_eq!(*c.borrow(), 0);
        assert!(!h.can_undo());
        assert_eq!(h.undo(&c), Ok(false));
        // die komplette Liste bleibt: wieder vorwärts
        assert_eq!(h.len(), 3);
        assert_eq!(h.redo(&c), Ok(true));
        assert_eq!(h.redo(&c), Ok(true));
        assert_eq!(h.redo(&c), Ok(true));
        assert_eq!(*c.borrow(), 111);
        assert!(!h.can_redo());
        assert_eq!(h.redo(&c), Ok(false));
    }

    #[test]
    fn new_change_discards_future() {
        let c = RefCell::new(0);
        let mut h = History::new(10);
        do_op(&mut h, &c, 1);
        do_op(&mut h, &c, 2);
        do_op(&mut h, &c, 4);
        h.undo(&c).unwrap();
        h.undo(&c).unwrap();
        assert_eq!(*c.borrow(), 1);
        assert!(h.can_redo());
        do_op(&mut h, &c, 8);
        assert!(!h.can_redo(), "Zukunfts-Änderungen werden verworfen");
        assert_eq!(h.len(), 2);
        assert_eq!(*c.borrow(), 9);
        h.undo(&c).unwrap();
        h.undo(&c).unwrap();
        assert_eq!(*c.borrow(), 0);
    }

    #[test]
    fn capacity_drops_oldest() {
        let c = RefCell::new(0);
        let mut h = History::new(3);
        for d in [1, 2, 4, 8, 16] {
            do_op(&mut h, &c, d);
        }
        assert_eq!(h.len(), 3);
        // nur die letzten drei lassen sich zurücknehmen
        while h.undo(&c).unwrap() {}
        assert_eq!(*c.borrow(), 1 + 2);
    }

    #[test]
    fn lowering_capacity_trims_and_keeps_position() {
        let c = RefCell::new(0);
        let mut h = History::new(10);
        for d in [1, 2, 4, 8] {
            do_op(&mut h, &c, d);
        }
        h.undo(&c).unwrap(); // Stand 7, Cursor 3 von 4
        h.set_capacity(2); // älteste zwei fallen weg: bleiben [4, 8], Cursor 1
        assert_eq!(h.len(), 2);
        assert!(h.can_undo() && h.can_redo());
        h.undo(&c).unwrap();
        assert_eq!(*c.borrow(), 3);
        assert!(!h.can_undo());
        h.redo(&c).unwrap();
        h.redo(&c).unwrap();
        assert_eq!(*c.borrow(), 15);
        h.set_capacity(0);
        assert_eq!(h.capacity(), 1, "mindestens ein Schritt");
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn capacity_one_still_works() {
        let c = RefCell::new(0);
        let mut h = History::new(1);
        do_op(&mut h, &c, 5);
        do_op(&mut h, &c, 7);
        assert_eq!(h.len(), 1);
        h.undo(&c).unwrap();
        assert_eq!(*c.borrow(), 5);
        assert!(!h.can_undo());
    }

    #[test]
    fn failing_entry_is_dropped() {
        let c = RefCell::new(0);
        let mut h = History::new(10);
        do_op(&mut h, &c, 1);
        h.push(Add { delta: 100, fail: true });
        assert_eq!(h.undo(&c), Err("kaputt".to_string()));
        assert_eq!(h.len(), 1, "der fehlerhafte Eintrag ist weg");
        assert_eq!(h.undo(&c), Ok(true));
        assert_eq!(*c.borrow(), 0);
        // Fehler beim Wiederholen: Eintrag wird verworfen, der Rest bleibt
        let mut h = History::new(10);
        h.push(add(1));
        h.push(Add { delta: 5, fail: true });
        h.push(add(2));
        h.cursor = 1; // Stand: nur der erste Eintrag ist „getan“
        assert_eq!(h.redo(&c), Err("kaputt".to_string()));
        assert_eq!(h.len(), 2);
        assert_eq!(h.redo(&c), Ok(true));
        assert_eq!(*c.borrow(), 2);
    }

    #[test]
    fn retain_keeps_cursor() {
        let c = RefCell::new(0);
        let mut h = History::new(10);
        for d in [1, 2, 4, 8] {
            do_op(&mut h, &c, d);
        }
        h.undo(&c).unwrap(); // Cursor 3
        h.retain(|op| op.delta != 2); // vergangenen Eintrag entfernen
        assert_eq!(h.len(), 3);
        assert!(h.can_redo());
        h.undo(&c).unwrap();
        h.undo(&c).unwrap();
        assert!(!h.can_undo(), "nur noch zwei vergangene Einträge");
        h.clear();
        assert_eq!(h.len(), 0);
        assert!(!h.can_undo() && !h.can_redo());
    }
}
