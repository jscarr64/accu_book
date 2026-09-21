# Changelog

## 1.0.0 — 2026-09-21

First stable headless release.

- Document model: `Notebook` / `Cell` / content hashes
- `History` undo/redo, `DepGraph`, `JobQueue`
- `EngineBridge` plug-in (Echo/Null/Tracking doubles)
- `Session` façade with `IntakePolicy` (Explicit default), ghost binding purge, and `chrome()` host status
- Plain-text `.accu` save/load (length-prefixed source; intake on disk)
- Public API freeze: `docs/API_1_0.md`
- Example: `cargo run --example echo_run`

No egui UI in this crate. Accumath SoftFloat engine stays proprietary.
