# accu_book

Headless notebook core for the Accumath project family. Pure Rust. No GUI in this crate yet.

**Open-sourced at notebook 1.0.0.** Until then this repo holds the public core as it is built. The Accumath SoftFloat CAS engine stays proprietary and is **not** this crate.

## Slice roadmap

| Slice | Status | What |
|------|--------|------|
| 1 Document model | **done** (Eugene APPROVE) | cells, order, metadata, blake3 content hashes, JSON serde |
| 2 State | **done** (Eugene APPROVE) | snapshot undo/redo via `History` |
| 3 DAG | **done** (Eugene APPROVE) | explicit deps + stale invalidation |
| 4 Jobs | **done** (Eugene APPROVE) | local FIFO eval queue, cancel, typed errors |
| 5 Engine bridge | **done** (Eugene APPROVE) | `EngineBridge` trait + Null/Echo doubles — no Accumath in this crate |
| egui shell | later | desktop UI with other Accumath UI work |

## Anti-Jupyter bar

- No hidden kernel state
- No out-of-order cell lies
- No network / ZMQ kernel
- Reproducible answers when an engine is plugged in later (SoftFloat bit-identical where the engine provides them)

## Slice 1 API (headless)

- `CellId` (UUID v4), `CellKind` (`Code` \| `Markdown`), `Cell`, `Notebook`
- Public read: `Cell::{id,kind,source,metadata}`; `Notebook::{cells,iter,cell_ids,get,get_by_id}`
- Mutate: insert / append / remove / move / `set_source` / `set_kind` / `set_metadata`
- Content hashes (blake3 hex), canonical layout:
  - **cell:** `kind_tag \0 source \0` then each metadata entry in `BTreeMap` order as `key \0 value \0` (`kind_tag` is `code` or `markdown`)
  - **notebook:** for each cell in order: `uuid_16 \0` + same bytes as the cell layout above
- JSON serde round-trip (ids, order, kind, source, metadata)

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

## Session façade

`Session<E: EngineBridge>` wires History + DepGraph + JobQueue + engine + `ResultStore`.

- `edit` / `undo` / `redo` — history + dep sync + stale marks
- `enqueue` / `run_one` / `run_all` — engine eval; results live in the session only
- `chrome(cell)` — Idle / Stale / Queued / Running / Ready / Failed (Running/Queued, then hash mismatch→Stale, then Failure, then graph stale, then Ready)
- egui must drive cell chrome from `chrome()` — not from `results()` alone
- Display must not own eval output

## Slice 5 — `EngineBridge`

Trait only — Accumath stays out of this crate:

- `simplify` / `solve` / `to_latex` / `assumptions`
- `NullEngine` (all Unsupported) and `EchoEngine` (test double)
- `eval_cell_with` helper for job workers
- Hosts implement the trait (IPC / linked proprietary engine later)

## Slice 4 — `JobQueue`

Local FIFO cell-eval jobs (host-driven, no threads/network):

- `enqueue` / `start_next` / `finish` / `cancel`
- `run_one(worker)` helper
- Statuses: Queued, Running, Succeeded, Failed { message }, Cancelled

## Slice 3 — `DepGraph`

Explicit cell dependencies (no hidden kernel inference):

- `sync_notebook` / `ensure_cell` / `remove_cell`
- `set_dependencies` (rejects unknown ids and cycles)
- `mark_changed` marks the cell and transitive dependents stale
- `topological_order` for eval order

## Slice 2 — `History`

Snapshot undo/redo around a present `Notebook`:

- `History::apply(|nb| { ... })` / `replace(notebook)` — records undo, clears redo
- `undo` / `redo` / `can_undo` / `can_redo`
- Default undo depth 100 (`with_limit`)

## Related open crates

- [zenith-float](https://github.com/jscarr64/zenith-float) · [crates.io](https://crates.io/crates/zenith-float)
- [latex-rust](https://github.com/jscarr64/LaTeX-Rust) · [crates.io](https://crates.io/crates/latex-rust)
- [hdf5-rust](https://github.com/jscarr64/hdf5-rust) · [crates.io](https://crates.io/crates/hdf5-rust)
- [redb-view](https://github.com/jscarr64/redb-view) · [crates.io](https://crates.io/crates/redb-view)

Site: [accumath.net/open-source](https://www.accumath.net/open-source.html)

## License

MIT OR Apache-2.0

## `.accu` persistence (S6)

Headless save/load uses a plain-text UTF-8 `.accu` file (not JSON on disk):

```
accu 1
intake explicit|inference

CELL <uuid>
KIND code|markdown
META key value   # optional, repeatable; key has no spaces; value is one line
SOURCE <byte_len>
...exactly byte_len UTF-8 bytes of cell source...
ENDCELL
```

Source is length-prefixed so cell text may contain any UTF-8, including lines that look like delimiters.

APIs: `encode_notebook` / `decode_notebook`, `save_notebook` / `load_notebook`.
Session results, job queue, and chrome are not written.

## Intake policy (S7)

`Session` defaults to `IntakePolicy::Explicit`: a cell must get `set_dependencies` (empty is fine) before `enqueue`. `IntakePolicy::Inference` allows enqueue without a prior declaration. Switching policy does not change the DAG engine; this crate never invents dependency edges — Inference only relaxes the gate. The policy is stored on the notebook and written as an `intake` line in `.accu`.

## Ghost bindings (S9)

On cell edit, remove, or successful re-eval, `Session` calls `EngineBridge::purge_bindings` with names from the prior successful eval (`bindings_after_eval`). Default engines bind nothing; real hosts implement both hooks so Jupyter-style ghost variables cannot linger.
