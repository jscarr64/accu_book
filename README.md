# accu_book

Headless notebook core for the Accumath project family. Pure Rust. No GUI in this crate yet.

**Open-sourced at notebook 1.0.0.** Until then this repo holds the public core as it is built. The Accumath SoftFloat CAS engine stays proprietary and is **not** this crate.

## Slice roadmap

| Slice | Status | What |
|------|--------|------|
| 1 Document model | **done** (Eugene APPROVE) | cells, order, metadata, blake3 content hashes, JSON serde |
| 2 State | **in progress** | snapshot undo/redo via `History` |
| 3 DAG | planned | cell dependency / invalidation |
| 4 Jobs | planned | local eval queue, cancel, typed errors |
| 5 Engine bridge | planned | trait only (simplify/solve/to_latex/assumptions) — no proprietary engine inside this crate |
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
