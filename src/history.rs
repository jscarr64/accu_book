//! Snapshot undo/redo for [`Notebook`] (slice 2).
//!
//! Callers mutate through [`History::apply`] or [`History::replace`] so each
//! edit pushes a full notebook snapshot onto the undo stack.

use crate::Notebook;
use std::collections::VecDeque;

/// Undo/redo stack around a present [`Notebook`].
#[derive(Debug, Clone)]
pub struct History {
    past: VecDeque<Notebook>,
    future: VecDeque<Notebook>,
    present: Notebook,
    limit: usize,
}

const DEFAULT_LIMIT: usize = 100;

impl Default for History {
    fn default() -> Self {
        Self::new(Notebook::new())
    }
}

impl History {
    pub fn new(notebook: Notebook) -> Self {
        Self::with_limit(notebook, DEFAULT_LIMIT)
    }

    pub fn with_limit(notebook: Notebook, limit: usize) -> Self {
        History {
            past: VecDeque::new(),
            future: VecDeque::new(),
            present: notebook,
            limit: limit.max(1),
        }
    }

    pub fn present(&self) -> &Notebook {
        &self.present
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    pub fn undo_len(&self) -> usize {
        self.past.len()
    }

    pub fn redo_len(&self) -> usize {
        self.future.len()
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Snapshot present, clear redo, then run `f` on the present notebook.
    pub fn apply<F>(&mut self, f: F)
    where
        F: FnOnce(&mut Notebook),
    {
        self.push_undo_snapshot();
        self.future.clear();
        f(&mut self.present);
    }

    /// Snapshot present, clear redo, then replace present with `notebook`.
    pub fn replace(&mut self, notebook: Notebook) {
        self.push_undo_snapshot();
        self.future.clear();
        self.present = notebook;
    }

    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.past.pop_back() else {
            return false;
        };
        self.future.push_back(std::mem::replace(&mut self.present, prev));
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.future.pop_back() else {
            return false;
        };
        self.past.push_back(std::mem::replace(&mut self.present, next));
        true
    }

    fn push_undo_snapshot(&mut self) {
        self.past.push_back(self.present.clone());
        while self.past.len() > self.limit {
            self.past.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Cell;

    #[test]
    fn undo_redo_one_edit() {
        let mut h = History::new(Notebook::new());
        h.apply(|n| n.append(Cell::new_code("1".into())));
        assert_eq!(h.present().len(), 1);
        assert!(h.undo());
        assert!(h.present().is_empty());
        assert!(h.redo());
        assert_eq!(h.present().len(), 1);
        assert_eq!(h.present().cells()[0].source(), "1");
    }

    #[test]
    fn two_applies_undo_twice_redo_once() {
        let mut h = History::default();
        h.apply(|n| n.append(Cell::new_code("a".into())));
        h.apply(|n| n.append(Cell::new_markdown("b".into())));
        assert_eq!(h.present().len(), 2);
        assert!(h.undo());
        assert_eq!(h.present().len(), 1);
        assert!(h.undo());
        assert!(h.present().is_empty());
        assert!(h.redo());
        assert_eq!(h.present().len(), 1);
        assert_eq!(h.present().cells()[0].source(), "a");
    }

    #[test]
    fn apply_after_undo_clears_redo() {
        let mut h = History::default();
        h.apply(|n| n.append(Cell::new_code("a".into())));
        h.apply(|n| n.append(Cell::new_code("b".into())));
        assert!(h.undo());
        assert_eq!(h.redo_len(), 1);
        h.apply(|n| n.append(Cell::new_code("c".into())));
        assert_eq!(h.redo_len(), 0);
        assert!(!h.redo());
        assert_eq!(h.present().len(), 2);
        assert_eq!(h.present().cells()[1].source(), "c");
    }

    #[test]
    fn limit_drops_oldest() {
        let mut h = History::with_limit(Notebook::new(), 2);
        h.apply(|n| n.append(Cell::new_code("1".into())));
        h.apply(|n| n.append(Cell::new_code("2".into())));
        h.apply(|n| n.append(Cell::new_code("3".into())));
        assert_eq!(h.undo_len(), 2);
        assert!(h.undo());
        assert!(h.undo());
        assert!(!h.undo());
        // after two undos we should be at the state after first apply (one cell),
        // because the empty initial snapshot was dropped when limit was exceeded.
        assert_eq!(h.present().len(), 1);
        assert_eq!(h.present().cells()[0].source(), "1");
    }

    #[test]
    fn replace_is_undoable() {
        let mut h = History::default();
        h.apply(|n| n.append(Cell::new_code("old".into())));
        let mut nb = Notebook::new();
        nb.append(Cell::new_markdown("new".into()));
        h.replace(nb);
        assert_eq!(h.present().cells()[0].source(), "new");
        assert!(h.undo());
        assert_eq!(h.present().cells()[0].source(), "old");
    }
}
