//! Engine bridge trait (slice 5).
//!
//! `accu_book` never depends on the proprietary Accumath engine. A host
//! (desktop app, test double, or future IPC adapter) implements
//! [`EngineBridge`] and plugs it into the job queue.

/// Result of a successful engine operation (plain + LaTeX forms).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineResult {
    pub plain: String,
    pub latex: String,
}

impl EngineResult {
    pub fn new(plain: impl Into<String>, latex: impl Into<String>) -> Self {
        Self {
            plain: plain.into(),
            latex: latex.into(),
        }
    }
}

/// Typed engine failures — honest Unsupported, never a wrong answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EngineError {
    #[error("unsupported engine operation: {op}")]
    Unsupported { op: String },
    #[error("invalid input: {message}")]
    InvalidInput { message: String },
    #[error("engine failed: {message}")]
    Failed { message: String },
}

/// Minimal CAS surface the notebook core needs. Inputs are source strings for
/// now (no AST in this crate). Implementations may live behind local IPC.
pub trait EngineBridge {
    fn simplify(&self, input: &str) -> Result<EngineResult, EngineError>;
    fn solve(&self, input: &str) -> Result<EngineResult, EngineError>;
    fn to_latex(&self, input: &str) -> Result<String, EngineError>;
    /// Assumption annotations for `input` (e.g. domain notes), as plain strings.
    fn assumptions(&self, input: &str) -> Result<Vec<String>, EngineError>;

    /// Names this eval is considered to bind (host-defined). Default: none.
    fn bindings_after_eval(&self, _source: &str) -> Vec<String> {
        Vec::new()
    }

    /// Drop bindings so they cannot linger after edit/remove/re-eval (anti Jupyter-ghost).
    fn purge_bindings(&mut self, _names: &[String]) -> Result<(), EngineError> {
        Ok(())
    }
}

/// Default test/offline engine: every op is [`EngineError::Unsupported`].
#[derive(Debug, Default, Clone, Copy)]
pub struct NullEngine;

impl EngineBridge for NullEngine {
    fn simplify(&self, _input: &str) -> Result<EngineResult, EngineError> {
        Err(EngineError::Unsupported {
            op: "simplify".into(),
        })
    }

    fn solve(&self, _input: &str) -> Result<EngineResult, EngineError> {
        Err(EngineError::Unsupported { op: "solve".into() })
    }

    fn to_latex(&self, _input: &str) -> Result<String, EngineError> {
        Err(EngineError::Unsupported {
            op: "to_latex".into(),
        })
    }

    fn assumptions(&self, _input: &str) -> Result<Vec<String>, EngineError> {
        Err(EngineError::Unsupported {
            op: "assumptions".into(),
        })
    }
}

/// Test double: `to_latex` wraps input in `$...$`; simplify/solve echo plain=latex=input;
/// assumptions returns empty ok.
#[derive(Debug, Default, Clone, Copy)]
pub struct EchoEngine;

impl EngineBridge for EchoEngine {
    fn simplify(&self, input: &str) -> Result<EngineResult, EngineError> {
        if input.trim().is_empty() {
            return Err(EngineError::InvalidInput {
                message: "empty input".into(),
            });
        }
        Ok(EngineResult::new(input, input))
    }

    fn solve(&self, input: &str) -> Result<EngineResult, EngineError> {
        self.simplify(input)
    }

    fn to_latex(&self, input: &str) -> Result<String, EngineError> {
        if input.trim().is_empty() {
            return Err(EngineError::InvalidInput {
                message: "empty input".into(),
            });
        }
        Ok(format!("${input}$"))
    }

    fn assumptions(&self, _input: &str) -> Result<Vec<String>, EngineError> {
        Ok(Vec::new())
    }
}


/// Test double that records purge calls and binds one name per non-empty source:
/// `bind:<first_char>` (e.g. source `"x+1"` → `["bind:x"]`).
#[derive(Debug, Default, Clone)]
pub struct TrackingEngine {
    pub purged: Vec<String>,
}

impl EngineBridge for TrackingEngine {
    fn simplify(&self, input: &str) -> Result<EngineResult, EngineError> {
        EchoEngine.simplify(input)
    }
    fn solve(&self, input: &str) -> Result<EngineResult, EngineError> {
        EchoEngine.solve(input)
    }
    fn to_latex(&self, input: &str) -> Result<String, EngineError> {
        EchoEngine.to_latex(input)
    }
    fn assumptions(&self, input: &str) -> Result<Vec<String>, EngineError> {
        EchoEngine.assumptions(input)
    }
    fn bindings_after_eval(&self, source: &str) -> Vec<String> {
        match source.chars().next().filter(|c| !c.is_whitespace()) {
            Some(c) => vec![format!("bind:{c}")],
            None => Vec::new(),
        }
    }
    fn purge_bindings(&mut self, names: &[String]) -> Result<(), EngineError> {
        self.purged.extend(names.iter().cloned());
        Ok(())
    }
}

/// Run one queued job by reading cell source from `get_source` and calling `op`.
pub fn eval_cell_with<E>(
    engine: &E,
    op: EngineOp,
    source: &str,
) -> Result<EngineResult, EngineError>
where
    E: EngineBridge + ?Sized,
{
    match op {
        EngineOp::Simplify => engine.simplify(source),
        EngineOp::Solve => engine.solve(source),
        EngineOp::ToLatex => engine
            .to_latex(source)
            .map(|latex| EngineResult::new(source, latex)),
        EngineOp::Assumptions => engine.assumptions(source).map(|list| {
            let plain = list.join("; ");
            EngineResult::new(plain.clone(), plain)
        }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineOp {
    Simplify,
    Solve,
    ToLatex,
    Assumptions,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Cell, CellId, JobQueue, JobStatus};
    use uuid::Uuid;

    #[test]
    fn null_engine_unsupported() {
        let e = NullEngine;
        assert!(matches!(
            e.simplify("x"),
            Err(EngineError::Unsupported { op }) if op == "simplify"
        ));
        assert!(matches!(e.solve("x"), Err(EngineError::Unsupported { .. })));
        assert!(matches!(
            e.to_latex("x"),
            Err(EngineError::Unsupported { .. })
        ));
        assert!(matches!(
            e.assumptions("x"),
            Err(EngineError::Unsupported { .. })
        ));
    }

    #[test]
    fn echo_engine_ok_paths() {
        let e = EchoEngine;
        let r = e.simplify("x+1").unwrap();
        assert_eq!(r.plain, "x+1");
        assert_eq!(e.to_latex("x+1").unwrap(), "$x+1$");
        assert!(e.assumptions("x").unwrap().is_empty());
        assert!(matches!(
            e.simplify("  "),
            Err(EngineError::InvalidInput { .. })
        ));
    }

    #[test]
    fn eval_cell_with_ops() {
        let e = EchoEngine;
        let r = eval_cell_with(&e, EngineOp::ToLatex, "y").unwrap();
        assert_eq!(r.latex, "$y$");
        let r = eval_cell_with(&e, EngineOp::Assumptions, "y").unwrap();
        assert_eq!(r.plain, "");
    }

    #[test]
    fn job_queue_plus_echo_engine() {
        let mut q = JobQueue::new();
        let cell = CellId::from(Uuid::new_v4());
        let id = q.enqueue(cell);
        let engine = EchoEngine;
        let source = "1+1";
        let (done, finish) = q
            .run_one(|c| {
                assert_eq!(c, cell);
                eval_cell_with(&engine, EngineOp::Simplify, source)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            })
            .unwrap();
        assert_eq!(done, id);
        finish.unwrap();
        assert_eq!(q.get(id).unwrap().status(), &JobStatus::Succeeded);
    }

    #[test]
    fn job_queue_null_engine_fails_job() {
        let mut q = JobQueue::new();
        let id = q.enqueue(Cell::new_code("x".into()).id());
        let engine = NullEngine;
        let (_, finish) = q
            .run_one(|_| {
                eval_cell_with(&engine, EngineOp::Simplify, "x")
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            })
            .unwrap();
        finish.unwrap();
        assert!(matches!(
            q.get(id).unwrap().status(),
            JobStatus::Failed { message } if message.contains("unsupported")
        ));
    }
}
