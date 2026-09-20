//! Local-only eval job queue (slice 4).
//!
//! No threads, no network, no ZMQ. The host drives the queue with
//! [`JobQueue::start_next`] / [`JobQueue::finish`] (or [`JobQueue::run_one`]).

use crate::CellId;
use std::collections::{BTreeMap, VecDeque};

/// Opaque job identifier (monotonic within a queue).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(u64);

impl JobId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Lifecycle of one cell-eval job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed { message: String },
    Cancelled,
}

impl JobStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            JobStatus::Succeeded | JobStatus::Failed { .. } | JobStatus::Cancelled
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JobError {
    #[error("unknown job id")]
    UnknownJob,
    #[error("job is not running")]
    NotRunning,
    #[error("job already finished")]
    AlreadyFinished,
    #[error("cannot cancel a finished job")]
    AlreadyTerminal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    id: JobId,
    cell: CellId,
    status: JobStatus,
}

impl Job {
    pub fn id(&self) -> JobId {
        self.id
    }

    pub fn cell(&self) -> CellId {
        self.cell
    }

    pub fn status(&self) -> &JobStatus {
        &self.status
    }
}

/// FIFO queue of cell evaluations. Single “running” slot; host is the worker.
#[derive(Debug, Default, Clone)]
pub struct JobQueue {
    next_id: u64,
    jobs: BTreeMap<JobId, Job>,
    queued: VecDeque<JobId>,
    running: Option<JobId>,
}

impl JobQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    pub fn queued_len(&self) -> usize {
        self.queued.len()
    }

    pub fn running_id(&self) -> Option<JobId> {
        self.running
    }

    pub fn get(&self, id: JobId) -> Option<&Job> {
        self.jobs.get(&id)
    }

    /// Enqueue an eval for `cell`. Returns the new job id.
    pub fn enqueue(&mut self, cell: CellId) -> JobId {
        self.next_id = self.next_id.saturating_add(1);
        let id = JobId(self.next_id);
        self.jobs.insert(
            id,
            Job {
                id,
                cell,
                status: JobStatus::Queued,
            },
        );
        self.queued.push_back(id);
        id
    }

    /// If nothing is running, promote the next queued job to `Running`.
    pub fn start_next(&mut self) -> Option<(JobId, CellId)> {
        if self.running.is_some() {
            return None;
        }
        let id = self.queued.pop_front()?;
        let job = self.jobs.get_mut(&id)?;
        job.status = JobStatus::Running;
        let cell = job.cell;
        self.running = Some(id);
        Some((id, cell))
    }

    /// Complete the running job (or any id that is `Running`).
    pub fn finish(&mut self, id: JobId, result: Result<(), String>) -> Result<(), JobError> {
        let job = self.jobs.get_mut(&id).ok_or(JobError::UnknownJob)?;
        match job.status {
            JobStatus::Running => {}
            JobStatus::Succeeded | JobStatus::Failed { .. } | JobStatus::Cancelled => {
                return Err(JobError::AlreadyFinished);
            }
            JobStatus::Queued => return Err(JobError::NotRunning),
        }
        job.status = match result {
            Ok(()) => JobStatus::Succeeded,
            Err(message) => JobStatus::Failed { message },
        };
        if self.running == Some(id) {
            self.running = None;
        }
        Ok(())
    }

    /// Cancel a queued or running job. Terminal jobs error.
    pub fn cancel(&mut self, id: JobId) -> Result<(), JobError> {
        let job = self.jobs.get_mut(&id).ok_or(JobError::UnknownJob)?;
        match job.status {
            JobStatus::Queued => {
                self.queued.retain(|q| *q != id);
                job.status = JobStatus::Cancelled;
                Ok(())
            }
            JobStatus::Running => {
                job.status = JobStatus::Cancelled;
                if self.running == Some(id) {
                    self.running = None;
                }
                Ok(())
            }
            JobStatus::Succeeded | JobStatus::Failed { .. } | JobStatus::Cancelled => {
                Err(JobError::AlreadyTerminal)
            }
        }
    }

    /// Convenience: `start_next`, run `worker(cell)`, then `finish`.
    ///
    /// If the job was cancelled while the worker ran (host must cooperate by
    /// checking status), `finish` may return [`JobError::AlreadyFinished`].
    pub fn run_one<F>(&mut self, mut worker: F) -> Option<(JobId, Result<(), JobError>)>
    where
        F: FnMut(CellId) -> Result<(), String>,
    {
        let (id, cell) = self.start_next()?;
        if !matches!(self.jobs.get(&id).map(|j| &j.status), Some(JobStatus::Running)) {
            return Some((id, Err(JobError::NotRunning)));
        }
        let result = worker(cell);
        let finish = self.finish(id, result);
        Some((id, finish))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn cell() -> CellId {
        CellId::from(Uuid::new_v4())
    }

    #[test]
    fn enqueue_start_finish_success() {
        let mut q = JobQueue::new();
        let c = cell();
        let id = q.enqueue(c);
        assert_eq!(q.get(id).unwrap().status(), &JobStatus::Queued);
        let (started, cell_id) = q.start_next().unwrap();
        assert_eq!(started, id);
        assert_eq!(cell_id, c);
        assert_eq!(q.running_id(), Some(id));
        q.finish(id, Ok(())).unwrap();
        assert_eq!(q.get(id).unwrap().status(), &JobStatus::Succeeded);
        assert!(q.running_id().is_none());
    }

    #[test]
    fn finish_failure_stores_message() {
        let mut q = JobQueue::new();
        let id = q.enqueue(cell());
        q.start_next().unwrap();
        q.finish(id, Err("boom".into())).unwrap();
        assert_eq!(
            q.get(id).unwrap().status(),
            &JobStatus::Failed {
                message: "boom".into()
            }
        );
    }

    #[test]
    fn cancel_queued() {
        let mut q = JobQueue::new();
        let id = q.enqueue(cell());
        q.cancel(id).unwrap();
        assert_eq!(q.get(id).unwrap().status(), &JobStatus::Cancelled);
        assert_eq!(q.queued_len(), 0);
        assert!(q.start_next().is_none());
    }

    #[test]
    fn cancel_running_frees_slot() {
        let mut q = JobQueue::new();
        let a = q.enqueue(cell());
        let b = q.enqueue(cell());
        q.start_next().unwrap();
        q.cancel(a).unwrap();
        assert_eq!(q.get(a).unwrap().status(), &JobStatus::Cancelled);
        assert!(q.running_id().is_none());
        let (id, _) = q.start_next().unwrap();
        assert_eq!(id, b);
    }

    #[test]
    fn fifo_order() {
        let mut q = JobQueue::new();
        let c1 = cell();
        let c2 = cell();
        let j1 = q.enqueue(c1);
        let j2 = q.enqueue(c2);
        assert_eq!(q.start_next().unwrap().0, j1);
        q.finish(j1, Ok(())).unwrap();
        assert_eq!(q.start_next().unwrap().0, j2);
    }

    #[test]
    fn cannot_start_second_while_running() {
        let mut q = JobQueue::new();
        q.enqueue(cell());
        q.enqueue(cell());
        assert!(q.start_next().is_some());
        assert!(q.start_next().is_none());
    }

    #[test]
    fn cancel_terminal_errors() {
        let mut q = JobQueue::new();
        let id = q.enqueue(cell());
        q.start_next().unwrap();
        q.finish(id, Ok(())).unwrap();
        assert_eq!(q.cancel(id), Err(JobError::AlreadyTerminal));
    }

    #[test]
    fn run_one_helper() {
        let mut q = JobQueue::new();
        let c = cell();
        let id = q.enqueue(c);
        let (done, res) = q.run_one(|cell| {
            assert_eq!(cell, c);
            Ok(())
        })
        .unwrap();
        assert_eq!(done, id);
        res.unwrap();
        assert_eq!(q.get(id).unwrap().status(), &JobStatus::Succeeded);
    }

    #[test]
    fn run_one_worker_err() {
        let mut q = JobQueue::new();
        let id = q.enqueue(cell());
        let (_, res) = q
            .run_one(|_| Err("nope".into()))
            .unwrap();
        res.unwrap();
        assert!(matches!(
            q.get(id).unwrap().status(),
            JobStatus::Failed { message } if message == "nope"
        ));
    }
}
