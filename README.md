# accu_book

Headless notebook core for the Accumath project family. Pure Rust. No GUI in this crate yet.

**Open-sourced at notebook 1.0.0.** Until then this repo holds the public core as it is built. The Accumath SoftFloat CAS engine stays proprietary and is **not** this crate.

## Slice roadmap

| Slice | Status | What |
|------|--------|------|
| 1 Document model | **in progress / first cut** | cells, order, metadata, blake3 content hashes, JSON serde |
| 2 State | planned | undo/redo or snapshots |
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
- Insert / append / remove / move / set_source / set_metadata
- `cell_content_hash` — blake3 hex of cell source bytes
- `notebook_content_hash` — blake3 hex over each cell in order: 16-byte UUID + NUL + source + NUL
- JSON serde round-trip

```bash
cargo test
cargo clippy --all-targets -- -D warnings
```

## Related open crates

- [zenith-float](https://github.com/jscarr64/zenith-float) · [crates.io](https://crates.io/crates/zenith-float)
- [latex-rust](https://github.com/jscarr64/LaTeX-Rust) · [crates.io](https://crates.io/crates/latex-rust)
- [hdf5-rust](https://github.com/jscarr64/hdf5-rust) · [crates.io](https://crates.io/crates/hdf5-rust)
- [redb-view](https://github.com/jscarr64/redb-view) · [crates.io](https://crates.io/crates/redb-view)

Site: [accumath.net/open-source](https://www.accumath.net/open-source.html)

## License

MIT OR Apache-2.0
