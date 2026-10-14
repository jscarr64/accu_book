# Changelog

## 2.0.0 (unreleased)

### Changed (breaking)
- MSRV raised to Rust 1.99 (`rust-version = "1.99"` in `Cargo.toml`), and the toolchain is pinned with `rust-toolchain.toml` (channel `1.99`, components `clippy` and `rustfmt`). Raising the MSRV is a breaking change for downstream builds on older compilers, which is why this is a major bump from 1.0.0.

### Migration
- Build with Rust 1.99 or newer (`rustup toolchain install 1.99`). Inside this repo, rustup picks the pinned toolchain up automatically.
- Update your dependency to `accu_book = "2.0.0"`. There are no API changes in this release.

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
