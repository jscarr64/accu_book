//! Explicit cell dependency DAG and stale invalidation (slice 3).
//!
//! Dependencies are declared — never inferred from hidden kernel state.
//! Metadata / source NUL rules from hashing still apply elsewhere; this module
//! only tracks [`CellId`] edges.

use crate::CellId;
use crate::Notebook;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// Directed edges: cell → cells it depends on (must run / be fresh before it).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DepGraph {
    /// `id -> dependencies` (outgoing: “I need these”).
    deps: BTreeMap<CellId, BTreeSet<CellId>>,
    /// `id -> dependents` (incoming reverse index).
    dependents: BTreeMap<CellId, BTreeSet<CellId>>,
    stale: BTreeSet<CellId>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DepError {
    #[error("unknown cell id in dependency graph")]
    UnknownCell,
    #[error("dependency edge would create a cycle")]
    Cycle,
}

impl DepGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn contains(&self, id: &CellId) -> bool {
        self.deps.contains_key(id)
    }

    pub fn ensure_cell(&mut self, id: CellId) {
        self.deps.entry(id).or_default();
        self.dependents.entry(id).or_default();
    }

    /// Register every cell in `notebook`; remove graph nodes no longer present.
    pub fn sync_notebook(&mut self, notebook: &Notebook) {
        let live: BTreeSet<CellId> = notebook.cell_ids().into_iter().collect();
        for id in live.iter().copied() {
            self.ensure_cell(id);
        }
        let obsolete: Vec<CellId> = self
            .deps
            .keys()
            .copied()
            .filter(|id| !live.contains(id))
            .collect();
        for id in obsolete {
            let _ = self.remove_cell(&id);
        }
    }

    pub fn remove_cell(&mut self, id: &CellId) -> bool {
        if !self.deps.contains_key(id) {
            return false;
        }
        // Dependents must go stale — deleting an input is not a silent refresh.
        self.mark_changed(id);
        if let Some(deps) = self.deps.remove(id) {
            for dep in deps {
                if let Some(set) = self.dependents.get_mut(&dep) {
                    set.remove(id);
                }
            }
        }
        if let Some(dependents) = self.dependents.remove(id) {
            for d in dependents {
                if let Some(set) = self.deps.get_mut(&d) {
                    set.remove(id);
                }
            }
        }
        self.stale.remove(id);
        true
    }

    pub fn dependencies(&self, id: &CellId) -> Option<&BTreeSet<CellId>> {
        self.deps.get(id)
    }

    pub fn dependents(&self, id: &CellId) -> Option<&BTreeSet<CellId>> {
        self.dependents.get(id)
    }

    /// Replace the dependency list for `id`. Rejects unknown ids and cycles.
    pub fn set_dependencies(
        &mut self,
        id: CellId,
        deps: impl IntoIterator<Item = CellId>,
    ) -> Result<(), DepError> {
        if !self.deps.contains_key(&id) {
            return Err(DepError::UnknownCell);
        }
        let new_deps: BTreeSet<CellId> = deps.into_iter().collect();
        for dep in &new_deps {
            if !self.deps.contains_key(dep) {
                return Err(DepError::UnknownCell);
            }
            if *dep == id {
                return Err(DepError::Cycle);
            }
        }
        // Tentatively apply, then detect cycle from `id`.
        let old_deps = self.deps.get(&id).cloned().unwrap_or_default();
        for dep in &old_deps {
            if let Some(set) = self.dependents.get_mut(dep) {
                set.remove(&id);
            }
        }
        for dep in &new_deps {
            self.dependents.entry(*dep).or_default().insert(id);
        }
        self.deps.insert(id, new_deps.clone());

        if self.topological_order().is_err() {
            // roll back
            for dep in &new_deps {
                if let Some(set) = self.dependents.get_mut(dep) {
                    set.remove(&id);
                }
            }
            for dep in &old_deps {
                self.dependents.entry(*dep).or_default().insert(id);
            }
            self.deps.insert(id, old_deps);
            return Err(DepError::Cycle);
        }
        self.mark_changed(&id);
        Ok(())
    }

    /// Mark `id` and every transitive dependent stale.
    pub fn mark_changed(&mut self, id: &CellId) {
        if !self.deps.contains_key(id) {
            return;
        }
        let mut q = VecDeque::from([*id]);
        let mut seen = BTreeSet::new();
        while let Some(cur) = q.pop_front() {
            if !seen.insert(cur) {
                continue;
            }
            self.stale.insert(cur);
            if let Some(deps) = self.dependents.get(&cur) {
                for d in deps {
                    q.push_back(*d);
                }
            }
        }
    }

    pub fn is_stale(&self, id: &CellId) -> bool {
        self.stale.contains(id)
    }

    pub fn clear_stale(&mut self, id: &CellId) {
        self.stale.remove(id);
    }

    pub fn clear_all_stale(&mut self) {
        self.stale.clear();
    }

    pub fn stale_ids(&self) -> impl Iterator<Item = CellId> + '_ {
        self.stale.iter().copied()
    }

    /// Topological order of all registered cells (dependencies before dependents).
    pub fn topological_order(&self) -> Result<Vec<CellId>, DepError> {
        let mut indeg: BTreeMap<CellId, usize> = BTreeMap::new();
        for id in self.deps.keys().copied() {
            indeg.insert(id, self.deps.get(&id).map(|s| s.len()).unwrap_or(0));
        }
        let mut ready: Vec<CellId> = indeg
            .iter()
            .filter(|(_, d)| **d == 0)
            .map(|(id, _)| *id)
            .collect();
        ready.sort_by_key(CellId::as_uuid);
        let mut q = VecDeque::from(ready);
        let mut out = Vec::new();
        while let Some(n) = q.pop_front() {
            out.push(n);
            if let Some(dependents) = self.dependents.get(&n) {
                let mut nexts: Vec<CellId> = dependents.iter().copied().collect();
                nexts.sort_by_key(CellId::as_uuid);
                for d in nexts {
                    if let Some(e) = indeg.get_mut(&d) {
                        *e = e.saturating_sub(1);
                        if *e == 0 {
                            q.push_back(d);
                        }
                    }
                }
            }
        }
        if out.len() != self.deps.len() {
            return Err(DepError::Cycle);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cell, Notebook};
    use uuid::Uuid;

    fn id() -> CellId {
        CellId::from(Uuid::new_v4())
    }

    #[test]
    fn sync_and_set_deps() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("a".into()));
        nb.append(Cell::new_code("b".into()));
        let a = nb.cells()[0].id();
        let b = nb.cells()[1].id();
        let mut g = DepGraph::new();
        g.sync_notebook(&nb);
        g.set_dependencies(b, [a]).unwrap();
        assert!(g.dependencies(&b).unwrap().contains(&a));
        assert!(g.dependents(&a).unwrap().contains(&b));
    }

    #[test]
    fn rejects_cycle() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("a".into()));
        nb.append(Cell::new_code("b".into()));
        let a = nb.cells()[0].id();
        let b = nb.cells()[1].id();
        let mut g = DepGraph::new();
        g.sync_notebook(&nb);
        g.set_dependencies(b, [a]).unwrap();
        assert_eq!(g.set_dependencies(a, [b]), Err(DepError::Cycle));
        // a still has no deps
        assert!(g.dependencies(&a).unwrap().is_empty());
    }

    #[test]
    fn rejects_self_edge() {
        let a = id();
        let mut g = DepGraph::new();
        g.ensure_cell(a);
        assert_eq!(g.set_dependencies(a, [a]), Err(DepError::Cycle));
    }

    #[test]
    fn mark_changed_marks_transitive_dependents() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("a".into()));
        nb.append(Cell::new_code("b".into()));
        nb.append(Cell::new_code("c".into()));
        let a = nb.cells()[0].id();
        let b = nb.cells()[1].id();
        let c = nb.cells()[2].id();
        let mut g = DepGraph::new();
        g.sync_notebook(&nb);
        g.set_dependencies(b, [a]).unwrap();
        g.set_dependencies(c, [b]).unwrap();
        g.mark_changed(&a);
        assert!(g.is_stale(&a));
        assert!(g.is_stale(&b));
        assert!(g.is_stale(&c));
        g.clear_stale(&b);
        assert!(!g.is_stale(&b));
        assert!(g.is_stale(&c));
    }

    #[test]
    fn topological_order_deps_first() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("a".into()));
        nb.append(Cell::new_code("b".into()));
        nb.append(Cell::new_code("c".into()));
        let a = nb.cells()[0].id();
        let b = nb.cells()[1].id();
        let c = nb.cells()[2].id();
        let mut g = DepGraph::new();
        g.sync_notebook(&nb);
        g.set_dependencies(b, [a]).unwrap();
        g.set_dependencies(c, [b]).unwrap();
        let order = g.topological_order().unwrap();
        let ia = order.iter().position(|x| *x == a).unwrap();
        let ib = order.iter().position(|x| *x == b).unwrap();
        let ic = order.iter().position(|x| *x == c).unwrap();
        assert!(ia < ib && ib < ic);
    }

    #[test]
    fn remove_cell_drops_edges_and_stale() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("a".into()));
        nb.append(Cell::new_code("b".into()));
        let a = nb.cells()[0].id();
        let b = nb.cells()[1].id();
        let mut g = DepGraph::new();
        g.sync_notebook(&nb);
        g.set_dependencies(b, [a]).unwrap();
        g.mark_changed(&a);
        assert!(g.remove_cell(&b));
        assert!(!g.contains(&b));
        assert!(!g.dependents(&a).unwrap().contains(&b));
        assert!(!g.is_stale(&b));
    }

    #[test]
    fn unknown_dep_rejected() {
        let a = id();
        let ghost = id();
        let mut g = DepGraph::new();
        g.ensure_cell(a);
        assert_eq!(g.set_dependencies(a, [ghost]), Err(DepError::UnknownCell));
    }

    #[test]
    fn remove_cell_stales_former_dependents() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("a".into()));
        nb.append(Cell::new_code("b".into()));
        let a = nb.cells()[0].id();
        let b = nb.cells()[1].id();
        let mut g = DepGraph::new();
        g.sync_notebook(&nb);
        g.set_dependencies(b, [a]).unwrap();
        g.clear_all_stale();
        assert!(!g.is_stale(&b));
        assert!(g.remove_cell(&a));
        assert!(!g.contains(&a));
        assert!(g.is_stale(&b));
        assert!(!g.is_stale(&a));
    }

    #[test]
    fn set_dependencies_success_marks_stale() {
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("a".into()));
        nb.append(Cell::new_code("b".into()));
        nb.append(Cell::new_code("c".into()));
        let a = nb.cells()[0].id();
        let b = nb.cells()[1].id();
        let c = nb.cells()[2].id();
        let mut g = DepGraph::new();
        g.sync_notebook(&nb);
        g.set_dependencies(b, [a]).unwrap();
        g.set_dependencies(c, [b]).unwrap();
        g.clear_all_stale();
        g.set_dependencies(b, []).unwrap();
        assert!(g.is_stale(&b));
        assert!(g.is_stale(&c));
        assert!(!g.is_stale(&a));
    }
}
