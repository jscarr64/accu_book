//! Session façade: History + DepGraph + JobQueue + EngineBridge + result store.
//!
//! UI / egui reads results only through [`Session`] — the view never owns eval output.

use crate::dag::{DepError, DepGraph};
use crate::engine::{eval_cell_with, EngineBridge, EngineError, EngineOp, EngineResult};
use crate::history::History;
use crate::jobs::{JobError, JobId, JobQueue};
use crate::{cell_content_hash, CellId, IntakePolicy, Notebook};
use std::fmt;
use std::collections::{BTreeMap, BTreeSet};

/// Eval output kept by the session (not by the view).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredOutput {
    Success {
        op: EngineOp,
        result: EngineResult,
        content_hash: String,
    },
    Failure {
        op: EngineOp,
        message: String,
        content_hash: String,
    },
}

impl StoredOutput {
    pub fn content_hash(&self) -> &str {
        match self {
            StoredOutput::Success { content_hash, .. }
            | StoredOutput::Failure { content_hash, .. } => content_hash,
        }
    }

    pub fn op(&self) -> EngineOp {
        match self {
            StoredOutput::Success { op, .. } | StoredOutput::Failure { op, .. } => *op,
        }
    }
}

/// Result store keyed by cell — display never owns this map.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ResultStore {
    by_cell: BTreeMap<CellId, StoredOutput>,
}

impl ResultStore {
    pub fn get(&self, id: &CellId) -> Option<&StoredOutput> {
        self.by_cell.get(id)
    }

    pub fn insert(&mut self, id: CellId, output: StoredOutput) {
        self.by_cell.insert(id, output);
    }

    pub fn remove(&mut self, id: &CellId) -> Option<StoredOutput> {
        self.by_cell.remove(id)
    }

    pub fn len(&self) -> usize {
        self.by_cell.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_cell.is_empty()
    }
}


/// Coarse cell chrome for UI (session is source of truth).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CellChrome {
    Idle,
    Stale,
    Queued,
    Running,
    Ready,
    Failed { message: String },
}

impl fmt::Display for CellChrome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CellChrome::Idle => write!(f, "idle"),
            CellChrome::Stale => write!(f, "stale"),
            CellChrome::Queued => write!(f, "queued"),
            CellChrome::Running => write!(f, "running"),
            CellChrome::Ready => write!(f, "ready"),
            CellChrome::Failed { message } => write!(f, "failed:{message}"),
        }
    }
}

/// Wired notebook session. `E` is typically Accumath-behind-IPC or a test double.
pub struct Session<E: EngineBridge> {
    history: History,
    deps: DepGraph,
    jobs: JobQueue,
    results: ResultStore,
    engine: E,
    default_op: EngineOp,
    /// Cells that have received at least one `set_dependencies` call.
    declared: BTreeSet<CellId>,
    /// Bindings attributed to each cell from the last successful eval.
    cell_bindings: BTreeMap<CellId, Vec<String>>,
}

impl<E: EngineBridge> Session<E> {
    pub fn new(engine: E) -> Self {
        Self::with_notebook(Notebook::new(), engine)
    }

    pub fn with_notebook(notebook: Notebook, engine: E) -> Self {
        let mut deps = DepGraph::new();
        deps.sync_notebook(&notebook);
        Self {
            history: History::new(notebook),
            deps,
            jobs: JobQueue::new(),
            results: ResultStore::default(),
            engine,
            default_op: EngineOp::Simplify,
            declared: BTreeSet::new(),
            cell_bindings: BTreeMap::new(),
        }
    }

    pub fn set_default_op(&mut self, op: EngineOp) {
        self.default_op = op;
    }

    pub fn default_op(&self) -> EngineOp {
        self.default_op
    }

    pub fn intake_policy(&self) -> IntakePolicy {
        self.notebook().intake_policy()
    }

    pub fn set_intake_policy(&mut self, policy: IntakePolicy) {
        self.history.apply(|nb| nb.set_intake_policy(policy));
    }

    /// True if `set_dependencies` was called for this cell at least once.
    pub fn dependencies_declared(&self, id: &CellId) -> bool {
        self.declared.contains(id)
    }

    pub fn notebook(&self) -> &Notebook {
        self.history.present()
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn deps(&self) -> &DepGraph {
        &self.deps
    }

    pub fn jobs(&self) -> &JobQueue {
        &self.jobs
    }

    pub fn results(&self) -> &ResultStore {
        &self.results
    }

    pub fn engine(&self) -> &E {
        &self.engine
    }

    /// Bindings recorded for `id` from the last successful eval (test/hosts).
    pub fn cell_bindings(&self, id: &CellId) -> Option<&[String]> {
        self.cell_bindings.get(id).map(Vec::as_slice)
    }

    fn purge_cell_bindings(&mut self, id: &CellId) {
        if let Some(names) = self.cell_bindings.remove(id) {
            let _ = self.engine.purge_bindings(&names);
        }
    }

    /// Mutate the notebook under undo history; sync deps; mark content changes stale.
    pub fn edit<F>(&mut self, f: F)
    where
        F: FnOnce(&mut Notebook),
    {
        let before_hashes: BTreeMap<CellId, String> = self
            .notebook()
            .iter()
            .map(|c| (c.id(), cell_content_hash(c)))
            .collect();
        let before_ids: BTreeSet<CellId> = before_hashes.keys().copied().collect();

        self.history.apply(f);
        self.deps.sync_notebook(self.history.present());

        let after_ids: BTreeSet<CellId> = self.notebook().cell_ids().into_iter().collect();
        for id in before_ids.difference(&after_ids) {
            self.results.remove(id);
            self.declared.remove(id);
            self.purge_cell_bindings(id);
        }

        let mut dirty: Vec<CellId> = Vec::new();
        for cell in self.notebook().iter() {
            let id = cell.id();
            let hash = cell_content_hash(cell);
            match before_hashes.get(&id) {
                None => dirty.push(id),
                Some(old) if old != &hash => dirty.push(id),
                Some(_) => {}
            }
        }
        for id in dirty {
            self.purge_cell_bindings(&id);
            self.deps.mark_changed(&id);
        }
    }

    pub fn undo(&mut self) -> bool {
        if !self.history.undo() {
            return false;
        }
        self.resync_after_history();
        true
    }

    pub fn redo(&mut self) -> bool {
        if !self.history.redo() {
            return false;
        }
        self.resync_after_history();
        true
    }

    pub fn set_dependencies(
        &mut self,
        id: CellId,
        deps: impl IntoIterator<Item = CellId>,
    ) -> Result<(), DepError> {
        self.deps.ensure_cell(id);
        self.deps.set_dependencies(id, deps)?;
        self.declared.insert(id);
        Ok(())
    }

    pub fn enqueue(&mut self, cell: CellId) -> Result<JobId, SessionError> {
        if self.notebook().get_by_id(&cell).is_none() {
            return Err(SessionError::UnknownCell);
        }
        if self.intake_policy() == IntakePolicy::Explicit && !self.declared.contains(&cell) {
            return Err(SessionError::UndeclaredDependencies { cell });
        }
        Ok(self.jobs.enqueue(cell))
    }

    pub fn cancel(&mut self, id: JobId) -> Result<(), JobError> {
        self.jobs.cancel(id)
    }

    /// Promote the next queued job to Running (host-driven loop). Prefer [`Self::run_one`]
    /// when the session should also call the engine.
    pub fn begin_next_job(&mut self) -> Option<(JobId, CellId)> {
        self.jobs.start_next()
    }

    /// Finish a job started with [`Self::begin_next_job`] without touching results.
    /// Hosts that eval outside `run_one` should store outputs themselves, then call this.
    pub fn complete_job(&mut self, id: JobId, result: Result<(), String>) -> Result<(), JobError> {
        self.jobs.finish(id, result)
    }

    /// Start next job, run engine, store output; `clear_stale` only on success.
    pub fn run_one(&mut self) -> Option<(JobId, Result<(), SessionError>)> {
        let (id, cell) = self.jobs.start_next()?;
        let Some(c) = self.notebook().get_by_id(&cell) else {
            let _ = self.jobs.finish(id, Err("cell removed".into()));
            return Some((id, Err(SessionError::UnknownCell)));
        };
        let source = c.source().to_string();
        let hash = cell_content_hash(c);
        let op = self.default_op;
        match eval_cell_with(&self.engine, op, &source) {
            Ok(result) => {
                let finish = self.jobs.finish(id, Ok(()));
                if finish.is_ok() {
                    self.purge_cell_bindings(&cell);
                    let names = self.engine.bindings_after_eval(&source);
                    if !names.is_empty() {
                        self.cell_bindings.insert(cell, names);
                    }
                    self.results.insert(
                        cell,
                        StoredOutput::Success {
                            op,
                            result,
                            content_hash: hash,
                        },
                    );
                    self.deps.clear_stale(&cell);
                }
                Some((id, finish.map_err(SessionError::Job)))
            }
            Err(e) => {
                let msg = e.to_string();
                let finish = self.jobs.finish(id, Err(msg.clone()));
                if finish.is_ok() {
                    self.results.insert(
                        cell,
                        StoredOutput::Failure {
                            op,
                            message: msg,
                            content_hash: hash,
                        },
                    );
                }
                Some((id, finish.map_err(SessionError::Job)))
            }
        }
    }

    pub fn run_all(&mut self) {
        while self.run_one().is_some() {}
    }

    pub fn chrome(&self, id: &CellId) -> CellChrome {
        if self.jobs.is_cell_running(id) {
            return CellChrome::Running;
        }
        if self.jobs.is_cell_queued(id) {
            return CellChrome::Queued;
        }
        let current_hash = self.notebook().get_by_id(id).map(cell_content_hash);
        if let (Some(out), Some(hash)) = (self.results.get(id), current_hash.as_ref()) {
            if out.content_hash() != hash {
                return CellChrome::Stale;
            }
        }
        if let Some(StoredOutput::Failure { message, .. }) = self.results.get(id) {
            return CellChrome::Failed {
                message: message.clone(),
            };
        }
        if self.deps.is_stale(id) {
            return CellChrome::Stale;
        }
        match self.results.get(id) {
            Some(StoredOutput::Success { .. }) => CellChrome::Ready,
            Some(StoredOutput::Failure { .. }) => unreachable!("handled above"),
            None => CellChrome::Idle,
        }
    }

    fn resync_after_history(&mut self) {
        self.deps.sync_notebook(self.history.present());
        let live: BTreeSet<CellId> = self.notebook().cell_ids().into_iter().collect();
        let drop: Vec<CellId> = self
            .results
            .by_cell
            .keys()
            .copied()
            .filter(|id| !live.contains(id))
            .collect();
        for id in drop {
            self.results.remove(&id);
            self.declared.remove(&id);
            self.purge_cell_bindings(&id);
        }
        let mut dirty: Vec<CellId> = Vec::new();
        let mut fresh: Vec<CellId> = Vec::new();
        for cell in self.notebook().iter() {
            let id = cell.id();
            let hash = cell_content_hash(cell);
            if let Some(out) = self.results.get(&id) {
                if out.content_hash() != hash {
                    dirty.push(id);
                } else {
                    fresh.push(id);
                }
            }
        }
        for id in dirty {
            self.deps.mark_changed(&id);
        }
        for id in fresh {
            self.deps.clear_stale(&id);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SessionError {
    #[error("unknown cell")]
    UnknownCell,
    #[error("cell {cell} has no declared dependencies under Explicit intake")]
    UndeclaredDependencies { cell: CellId },
    #[error(transparent)]
    Job(#[from] JobError),
    #[error(transparent)]
    Dep(#[from] DepError),
    #[error(transparent)]
    Engine(#[from] EngineError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{EchoEngine, NullEngine, TrackingEngine};
    use crate::Cell;

    #[test]
    fn happy_path_echo_ready() {
        let mut s = Session::new(EchoEngine);
        s.edit(|nb| nb.append(Cell::new_code("x+1".into())));
        let id = s.notebook().cells()[0].id();
        assert_eq!(s.chrome(&id), CellChrome::Stale);
        s.set_dependencies(id, []).unwrap();
        s.enqueue(id).unwrap();
        assert_eq!(s.chrome(&id), CellChrome::Queued);
        let (_jid, res) = s.run_one().unwrap();
        res.unwrap();
        assert_eq!(s.chrome(&id), CellChrome::Ready);
        assert!(!s.deps().is_stale(&id));
        match s.results().get(&id).unwrap() {
            StoredOutput::Success { result, .. } => assert_eq!(result.plain, "x+1"),
            _ => panic!("expected success"),
        }
    }

    #[test]
    fn null_engine_failure_chrome() {
        let mut s = Session::new(NullEngine);
        s.edit(|nb| nb.append(Cell::new_code("x".into())));
        let id = s.notebook().cells()[0].id();
        assert_eq!(s.chrome(&id), CellChrome::Stale);
        s.set_dependencies(id, []).unwrap();
        s.enqueue(id).unwrap();
        s.run_one().unwrap().1.unwrap();
        // Failed must win over graph stale after a real edit→enqueue→fail path
        assert!(matches!(s.chrome(&id), CellChrome::Failed { message } if message.contains("unsupported")));
        assert!(matches!(
            s.results().get(&id),
            Some(StoredOutput::Failure { .. })
        ));
    }

    #[test]
    fn edit_away_from_success_is_stale_not_ready() {
        let mut s = Session::new(EchoEngine);
        s.edit(|nb| nb.append(Cell::new_code("a".into())));
        let id = s.notebook().cells()[0].id();
        s.set_dependencies(id, []).unwrap();
        s.enqueue(id).unwrap();
        s.run_all();
        assert_eq!(s.chrome(&id), CellChrome::Ready);
        s.edit(|nb| {
            let id = nb.cells()[0].id();
            nb.set_source(&id, "b".into()).unwrap();
        });
        assert_eq!(s.chrome(&id), CellChrome::Stale);
        assert!(matches!(
            s.results().get(&id),
            Some(StoredOutput::Success { .. })
        ));
    }

    #[test]
    fn edit_source_marks_stale_again() {
        let mut s = Session::new(EchoEngine);
        s.edit(|nb| nb.append(Cell::new_code("a".into())));
        let id = s.notebook().cells()[0].id();
        s.set_dependencies(id, []).unwrap();
        s.enqueue(id).unwrap();
        s.run_all();
        assert_eq!(s.chrome(&id), CellChrome::Ready);
        s.edit(|nb| {
            let id = nb.cells()[0].id();
            nb.set_source(&id, "b".into()).unwrap();
        });
        assert_eq!(s.chrome(&id), CellChrome::Stale);
    }

    #[test]
    fn undo_restores_graph_and_marks_hash_mismatch_stale() {
        let mut s = Session::new(EchoEngine);
        s.edit(|nb| nb.append(Cell::new_code("a".into())));
        let id = s.notebook().cells()[0].id();
        s.set_dependencies(id, []).unwrap();
        s.enqueue(id).unwrap();
        s.run_all();
        assert_eq!(s.chrome(&id), CellChrome::Ready);
        s.edit(|nb| {
            let id = nb.cells()[0].id();
            nb.set_source(&id, "b".into()).unwrap();
        });
        assert!(s.undo());
        // source back to "a"; stored hash still "a" from success → Ready if not stale
        // resync: hash matches stored → should not mark stale
        assert_eq!(s.notebook().cells()[0].source(), "a");
        assert_eq!(s.chrome(&id), CellChrome::Ready);
        // redo to "b" → hash mismatch → stale
        assert!(s.redo());
        assert_eq!(s.notebook().cells()[0].source(), "b");
        assert_eq!(s.chrome(&id), CellChrome::Stale);
    }

    #[test]
    fn remove_cell_stales_dependent() {
        let mut s = Session::new(EchoEngine);
        s.edit(|nb| {
            nb.append(Cell::new_code("a".into()));
            nb.append(Cell::new_code("b".into()));
        });
        let a = s.notebook().cells()[0].id();
        let b = s.notebook().cells()[1].id();
        s.set_dependencies(b, [a]).unwrap();
        s.deps.clear_all_stale();
        s.edit(|nb| {
            let a = nb.cells()[0].id();
            nb.remove(&a).unwrap();
        });
        assert!(s.deps().is_stale(&b));
        assert!(s.results().get(&a).is_none());
    }

    #[test]
    fn default_intake_is_explicit() {
        let s = Session::new(EchoEngine);
        assert_eq!(s.intake_policy(), IntakePolicy::Explicit);
    }

    #[test]
    fn explicit_rejects_undeclared_enqueue() {
        let mut s = Session::new(EchoEngine);
        s.edit(|nb| nb.append(Cell::new_code("x".into())));
        let id = s.notebook().cells()[0].id();
        let err = s.enqueue(id).unwrap_err();
        assert!(matches!(
            err,
            SessionError::UndeclaredDependencies { cell } if cell == id
        ));
        assert!(!s.dependencies_declared(&id));
    }

    #[test]
    fn explicit_allows_after_empty_declaration() {
        let mut s = Session::new(EchoEngine);
        s.edit(|nb| nb.append(Cell::new_code("x".into())));
        let id = s.notebook().cells()[0].id();
        s.set_dependencies(id, []).unwrap();
        assert!(s.dependencies_declared(&id));
        s.enqueue(id).unwrap();
        s.run_all();
        assert_eq!(s.chrome(&id), CellChrome::Ready);
    }

    #[test]
    fn inference_allows_undeclared_enqueue() {
        let mut s = Session::new(EchoEngine);
        s.set_intake_policy(IntakePolicy::Inference);
        assert_eq!(s.intake_policy(), IntakePolicy::Inference);
        s.edit(|nb| nb.append(Cell::new_code("x".into())));
        let id = s.notebook().cells()[0].id();
        assert!(!s.dependencies_declared(&id));
        s.enqueue(id).unwrap();
        s.run_all();
        assert_eq!(s.chrome(&id), CellChrome::Ready);
    }

    #[test]
    fn toggling_back_to_explicit_enforces_again() {
        let mut s = Session::new(EchoEngine);
        s.set_intake_policy(IntakePolicy::Inference);
        s.edit(|nb| nb.append(Cell::new_code("x".into())));
        let id = s.notebook().cells()[0].id();
        s.enqueue(id).unwrap();
        s.run_all();
        s.set_intake_policy(IntakePolicy::Explicit);
        // already declared? no — inference enqueue did not declare
        let err = s.enqueue(id).unwrap_err();
        assert!(matches!(err, SessionError::UndeclaredDependencies { .. }));
        s.set_dependencies(id, []).unwrap();
        s.enqueue(id).unwrap();
    }

    #[test]
    fn session_reopen_keeps_inference_policy() {
        let mut s = Session::new(EchoEngine);
        s.set_intake_policy(IntakePolicy::Inference);
        s.edit(|nb| nb.append(Cell::new_code("z".into())));
        let text = crate::encode_notebook(s.notebook());
        let nb2 = crate::decode_notebook(&text).unwrap();
        assert_eq!(nb2.intake_policy(), IntakePolicy::Inference);
        let mut s2 = Session::with_notebook(nb2, EchoEngine);
        assert_eq!(s2.intake_policy(), IntakePolicy::Inference);
        let id = s2.notebook().cells()[0].id();
        s2.enqueue(id).unwrap(); // undeclared OK under Inference
        s2.run_all();
        assert_eq!(s2.chrome(&id), CellChrome::Ready);
    }


    #[test]
    fn edit_purges_prior_bindings() {
        let mut s = Session::new(TrackingEngine::default());
        s.set_intake_policy(IntakePolicy::Inference);
        s.edit(|nb| nb.append(Cell::new_code("x+1".into())));
        let id = s.notebook().cells()[0].id();
        s.enqueue(id).unwrap();
        s.run_all();
        assert_eq!(s.cell_bindings(&id), Some(["bind:x".to_string()].as_slice()));
        s.edit(|nb| {
            let id = nb.cells()[0].id();
            nb.set_source(&id, "y+1".into()).unwrap();
        });
        assert!(s.cell_bindings(&id).is_none());
        assert_eq!(s.engine().purged, vec!["bind:x".to_string()]);
    }

    #[test]
    fn remove_purges_prior_bindings() {
        let mut s = Session::new(TrackingEngine::default());
        s.set_intake_policy(IntakePolicy::Inference);
        s.edit(|nb| nb.append(Cell::new_code("a".into())));
        let id = s.notebook().cells()[0].id();
        s.enqueue(id).unwrap();
        s.run_all();
        assert_eq!(s.cell_bindings(&id), Some(["bind:a".to_string()].as_slice()));
        s.edit(|nb| {
            let id = nb.cells()[0].id();
            nb.remove(&id).unwrap();
        });
        assert!(s.cell_bindings(&id).is_none());
        assert_eq!(s.engine().purged, vec!["bind:a".to_string()]);
    }

    #[test]
    fn reeval_purges_old_then_binds_new() {
        let mut s = Session::new(TrackingEngine::default());
        s.set_intake_policy(IntakePolicy::Inference);
        s.edit(|nb| nb.append(Cell::new_code("a".into())));
        let id = s.notebook().cells()[0].id();
        s.enqueue(id).unwrap();
        s.run_all();
        s.edit(|nb| {
            let id = nb.cells()[0].id();
            nb.set_source(&id, "b".into()).unwrap();
        });
        s.set_dependencies(id, []).unwrap();
        s.enqueue(id).unwrap();
        s.run_all();
        assert_eq!(s.cell_bindings(&id), Some(["bind:b".to_string()].as_slice()));
        // purged on edit, and again at start of successful re-eval (empty then)
        assert!(s.engine().purged.iter().filter(|n| *n == "bind:a").count() >= 1);
    }


    /// Host chrome contract (S10): `chrome()` is the only status source for wrappers/CLI.
    #[test]
    fn chrome_covers_idle_stale_queued_running_ready_failed() {
        let mut s = Session::new(EchoEngine);
        s.set_intake_policy(IntakePolicy::Inference);

        // Idle: cell present, never run, not marked stale yet — append via edit marks stale.
        // Fresh notebook cell without edit mark: use with_notebook.
        let mut nb = Notebook::new();
        nb.append(Cell::new_code("q".into()));
        let id = nb.cells()[0].id();
        let mut s_idle = Session::with_notebook(nb, EchoEngine);
        s_idle.set_intake_policy(IntakePolicy::Inference);
        assert_eq!(s_idle.chrome(&id), CellChrome::Idle);
        assert_eq!(s_idle.chrome(&id).to_string(), "idle");

        // Stale after edit
        s.edit(|nb| nb.append(Cell::new_code("a".into())));
        let id = s.notebook().cells()[0].id();
        assert_eq!(s.chrome(&id), CellChrome::Stale);
        assert_eq!(s.chrome(&id).to_string(), "stale");

        // Queued
        s.enqueue(id).unwrap();
        assert_eq!(s.chrome(&id), CellChrome::Queued);
        assert_eq!(s.chrome(&id).to_string(), "queued");

        // Running
        let (jid, cell) = s.begin_next_job().unwrap();
        assert_eq!(cell, id);
        assert_eq!(s.chrome(&id), CellChrome::Running);
        assert_eq!(s.chrome(&id).to_string(), "running");
        s.complete_job(jid, Ok(())).unwrap();

        // Ready via run_one path (re-enqueue after complete without results → still need eval)
        s.set_dependencies(id, []).unwrap();
        s.enqueue(id).unwrap();
        s.run_all();
        assert_eq!(s.chrome(&id), CellChrome::Ready);
        assert_eq!(s.chrome(&id).to_string(), "ready");

        // Failed
        let mut sf = Session::new(NullEngine);
        sf.set_intake_policy(IntakePolicy::Inference);
        sf.edit(|nb| nb.append(Cell::new_code("z".into())));
        let fid = sf.notebook().cells()[0].id();
        sf.enqueue(fid).unwrap();
        sf.run_all();
        assert!(matches!(sf.chrome(&fid), CellChrome::Failed { .. }));
        assert!(sf.chrome(&fid).to_string().starts_with("failed:"));
    }

}
