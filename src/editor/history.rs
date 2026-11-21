//! Undo/redo for documents: transactions of change sets, grouped while typing, each with an origin.

use super::change::ChangeSet;
use super::selection::Selections;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Who made an edit. Agent edits are marked in the UI ("by agent") and stay undoable like any other edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditOrigin {
    #[default]
    User,
    Agent,
}

/// Kind of edit, used to group consecutive edits into one undo step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EditKind {
    /// Typed characters (merge with the previous typing step).
    Typing,
    /// Backspace (merges with previous backspaces).
    DeleteBack,
    /// Delete key (merges with previous deletes).
    DeleteForward,
    /// Anything else (paste, newline, line operations, replace …): always its own step.
    Other,
}

impl EditKind {
    fn merges(self) -> bool {
        !matches!(self, EditKind::Other)
    }
}

/// One undo step: one or more change sets applied in order.
#[derive(Clone, Debug)]
pub struct Transaction {
    pub(crate) steps: Vec<Arc<ChangeSet>>,
    pub origin: EditOrigin,
    pub kind: EditKind,
    pub(crate) sel_before: Selections,
    pub(crate) sel_after: Selections,
    pub(crate) last_at: Instant,
    /// State id of the text after this transaction (changes when steps are merged in).
    pub(crate) id: u64,
    size: usize,
}

/// Limits of the history.
#[derive(Clone, Copy, Debug)]
pub struct HistoryLimits {
    /// Maximum number of undo steps.
    pub max_steps: usize,
    /// Rough memory budget of all steps in bytes (oldest steps are dropped first; the newest always stays).
    pub max_bytes: usize,
    /// Typing within this time after the previous keystroke is merged into one undo step.
    pub merge_window: Duration,
}

impl Default for HistoryLimits {
    fn default() -> Self {
        Self { max_steps: 10_000, max_bytes: 512 * 1024 * 1024, merge_window: Duration::from_millis(1500) }
    }
}

#[derive(Debug, Default)]
pub(crate) struct History {
    pub(crate) undo: Vec<Transaction>,
    pub(crate) redo: Vec<Transaction>,
    pub(crate) limits: HistoryLimits,
    /// State id of the text before the oldest kept transaction.
    pub(crate) base_id: u64,
    next_id: u64,
    group_open: bool,
    bytes: usize,
}

impl History {
    pub(crate) fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Current state id (id of the newest applied transaction).
    pub(crate) fn state_id(&self) -> u64 {
        self.undo.last().map(|t| t.id).unwrap_or(self.base_id)
    }

    /// End the current typing group: the next edit starts a new undo step.
    pub(crate) fn break_group(&mut self) {
        self.group_open = false;
    }

    /// Record an applied change set. Returns the new state id.
    pub(crate) fn record(
        &mut self,
        cs: Arc<ChangeSet>,
        origin: EditOrigin,
        kind: EditKind,
        before: &Selections,
        after: &Selections,
    ) -> u64 {
        let now = Instant::now();
        let size = cs.heap_size();
        let id = self.new_id();
        for t in self.redo.drain(..) {
            self.bytes = self.bytes.saturating_sub(t.size);
        }
        let merge = self.group_open
            && kind.merges()
            && self.undo.last().is_some_and(|t| {
                t.kind == kind
                    && t.origin == origin
                    && now.duration_since(t.last_at) <= self.limits.merge_window
                    && same_positions(&t.sel_after, before)
            });
        if merge {
            let t = self.undo.last_mut().expect("checked");
            t.steps.push(cs);
            t.sel_after = after.clone();
            t.last_at = now;
            t.id = id;
            t.size += size;
        } else {
            self.undo.push(Transaction {
                steps: vec![cs],
                origin,
                kind,
                sel_before: before.clone(),
                sel_after: after.clone(),
                last_at: now,
                id,
                size,
            });
        }
        self.bytes += size;
        self.group_open = kind.merges();
        self.trim();
        id
    }

    fn trim(&mut self) {
        while self.undo.len() > 1 && (self.undo.len() > self.limits.max_steps || self.bytes > self.limits.max_bytes) {
            let t = self.undo.remove(0);
            self.bytes = self.bytes.saturating_sub(t.size);
            self.base_id = t.id;
        }
    }

    pub(crate) fn pop_undo(&mut self) -> Option<Transaction> {
        self.group_open = false;
        self.undo.pop()
    }

    pub(crate) fn push_redo(&mut self, t: Transaction) {
        self.redo.push(t);
    }

    pub(crate) fn pop_redo(&mut self) -> Option<Transaction> {
        self.group_open = false;
        self.redo.pop()
    }

    pub(crate) fn push_undo(&mut self, t: Transaction) {
        self.undo.push(t);
    }

    pub(crate) fn clear(&mut self) {
        self.base_id = self.state_id();
        self.undo.clear();
        self.redo.clear();
        self.bytes = 0;
        self.group_open = false;
    }
}

/// Same cursor positions (ignoring goal columns and sync revision)?
fn same_positions(a: &Selections, b: &Selections) -> bool {
    a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.anchor == y.anchor && x.head == y.head)
}
