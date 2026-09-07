# Repository Guidelines

## Project Structure & Module Organization

This is a single Rust 2024 workspace (Rust 1.88+) with a shared engine and
thin platform adapters:

- `crates/sumpter-core/`: platform-neutral configuration, routing, scheduling, access control, and protocol conversion.
- `crates/sumpter-runtime/`: shared SQLite event storage and queries.
- `crates/sumpter-engine/`: platform-neutral HTTP proxy pipeline, relay, retry, replay, and runtime integration.
- `adapters/linux/sumpter-linux-adapter/`: Linux platform boundary, Admin facade, and server composition.
- `adapters/macos/sumpter-macos-adapter/`: macOS platform boundary, Admin facade, and server composition.
- `apps/linux/sumpterd/` and `apps/macos/sumpterd/`: thin daemon/sidecar entry points.

`platforms/linux/webui/`, `platforms/linux/web/`, and
`platforms/macos/app/` are platform UI and packaging inputs outside the Rust
workspace. They are intentionally not changed by engine architecture work.

Unit tests live beside modules under `src/`; integration tests and fixtures live under each crate's `tests/`. `config.example.json` is the safe schema-v7 example. Treat `docs/upstream/` as migration reference, not the current API contract. Current architecture and doc index live in `docs/architecture.md` and `docs/README.md`. Never commit `target/` artifacts.

## Build, Test, and Development Commands

Cargo may require `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"` on Homebrew macOS setups.

- `cargo fmt --all -- --check` checks formatting without rewriting files.
- `cargo check --workspace --locked` checks every shared crate, adapter, and app.
- `cargo test --workspace --locked` runs shared, Linux adapter, and macOS adapter tests.
- `cargo clippy --workspace --all-targets -- -D warnings` is the workspace lint gate.

The workspace has no platform feature matrix: platform behavior is injected by
the adapter crates. Do not use `--all-features` as a substitute for adapter
tests. The runnable binaries are `sumpterd-linux` and `sumpterd-macos`.

## Coding Style & Naming Conventions

Use `rustfmt` output (four-space indentation). Name modules and functions `snake_case`, types and traits `UpperCamelCase`, and constants `SCREAMING_SNAKE_CASE`. Keep core, runtime, and engine platform-neutral; put platform decisions in the corresponding `adapters/<platform>/` crate and inject them through `sumpter_engine::PlatformBoundary`. Prefer typed errors and explicit `Result` handling over production panics.

## Testing Guidelines

Use `#[test]` for synchronous behavior and `#[tokio::test]` for async flows. Name integration files after their contract, such as `routing.rs` or `replay_conformance.rs`. Add regression tests for behavior changes, covering both profiles when shared engine behavior changes. No numeric coverage threshold is configured; use focused assertions and fixtures.

## Commit & Pull Request Guidelines

This checkout contains no Git history from which to infer a house style. Use concise Conventional Commit subjects with a crate scope, for example `fix(runtime): preserve request-chain ordering`. Pull requests should explain the behavior and affected crate/platform, link the issue when available, list exact verification commands, and call out schema, security, or compatibility changes. Include screenshots only when an external facade or rendered documentation changes.

## Security & Configuration

Keep real API keys and runtime databases out of the repository. Extend `config.example.json` only with disabled synthetic endpoints and `.invalid` hosts. The project uses bundled SQLite; do not install or introduce an external database for local development.
