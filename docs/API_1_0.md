# accu_book 1.0 — public API freeze

Hosts may rely on the types and functions below for the **1.0.x** line.
Breaking changes require a major bump. SoftFloat math lives in the proprietary
Accumath engine, reached only through a host-supplied [`EngineBridge`].

This crate is **headless**. No egui. No network kernel. No ZMQ.

## Crate root

| Item | Role |
|------|------|
| `CellId`, `Cell`, `CellKind`, `Notebook` | Document model |
| `IntakePolicy` | Explicit (default) / Inference — stored on `Notebook` |
| `NotebookError` | Index / unknown-cell document errors |
| `cell_content_hash`, `notebook_content_hash` | Blake3 content hashes |
| `History` | Undo/redo snapshots around a present `Notebook` |
| `DepGraph`, `DepError` | Explicit cell dependencies + stale marking |
| `Job`, `JobId`, `JobStatus`, `JobQueue`, `JobError` | Local FIFO job queue |
| `EngineBridge`, `EngineOp`, `EngineResult`, `EngineError` | Engine plug-in surface |
| `EchoEngine`, `NullEngine` | Test / offline doubles |
| `TrackingEngine` | Test double that records `purge_bindings` |
| `eval_cell_with` | Dispatch one `EngineOp` against a bridge |
| `Session`, `SessionError`, `CellChrome`, `ResultStore`, `StoredOutput` | Host façade |
| `encode_notebook`, `decode_notebook`, `save_notebook`, `load_notebook`, `PersistError` | Plain-text `.accu` I/O |

## Host loop (canonical)

1. Build or `load_notebook` a `Notebook` (intake policy travels with the file).
2. `Session::with_notebook(nb, engine)`.
3. Under **Explicit** intake: `set_dependencies` before `enqueue` (empty set is fine).
4. `enqueue` → `run_one` / `run_all` **or** `begin_next_job` + host eval + `complete_job`.
5. Read status **only** from `Session::chrome(cell_id)` (`Display`: idle/stale/queued/running/ready/failed:…).
6. `save_notebook` for persistence. Results, jobs, and chrome are **not** written.

## `EngineBridge` contract

```rust
fn simplify(&self, input: &str) -> Result<EngineResult, EngineError>;
fn solve(&self, input: &str) -> Result<EngineResult, EngineError>;
fn to_latex(&self, input: &str) -> Result<String, EngineError>;
fn assumptions(&self, input: &str) -> Result<Vec<String>, EngineError>;
fn bindings_after_eval(&self, source: &str) -> Vec<String>; // default: empty
fn purge_bindings(&mut self, names: &[String]) -> Result<(), EngineError>; // default: Ok
```

Real Accumath hosts implement SoftFloat-honest ops and binding purge.
`Unsupported` must stay honest — never a wrong closed form.

## `.accu` on-disk (stable for 1.0)

```
accu 1
intake explicit|inference

CELL <uuid>
KIND code|markdown
META key value   # key: no spaces; value: one line
SOURCE <byte_len>
<exactly byte_len UTF-8 bytes>
ENDCELL
```

## Explicitly not public API

- Private module internals and `#[cfg(test)]` helpers
- Guarantees about egui / Jupyter chrome (desktop follow-on)
- Any Accumath engine types (proprietary; not in this crate)

## Freeze date

Locked for the 1.0.0 publish track (2026-09-21). Eugene gates material API churn.
