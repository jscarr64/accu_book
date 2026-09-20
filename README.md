# accu_book

Headless notebook crate for the Accumath project family.

## Status

Pre-1.0. Core slices (document model, state, dependency DAG, jobs, engine bridge) are planned on the Accumath build spine as **#16**. The desktop egui shell comes later with other Accumath UI work.

**Open source at notebook 1.0.0.** Until then this repository is the public home for planning and eventual release. The Accumath SoftFloat CAS engine remains proprietary and is not this crate.

## Design bar (anti-Jupyter)

- Explicit cell dependencies — no hidden kernel state
- Deterministic, checkable results (SoftFloat bit-identical where the engine provides them)
- Fully local / air-gap capable — no Jupyter ZMQ or network kernel
- Honest typed errors instead of silent wrong output
- Headless core usable without a GUI

## Related open crates

- [zenith-float](https://github.com/jscarr64/zenith-float) · [crates.io](https://crates.io/crates/zenith-float)
- [latex-rust](https://github.com/jscarr64/LaTeX-Rust) · [crates.io](https://crates.io/crates/latex-rust)
- [hdf5-rust](https://github.com/jscarr64/hdf5-rust) · [crates.io](https://crates.io/crates/hdf5-rust)
- [redb-view](https://github.com/jscarr64/redb-view) · [crates.io](https://crates.io/crates/redb-view)

Project site: [accumath.net](https://www.accumath.net/open-source.html)

## License

License file will be set when the 1.0.0 open-source release is cut.
