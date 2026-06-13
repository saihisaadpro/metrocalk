# Metrocalk

The vibe-coding game engine — intent-driven, UX-first. See [`metrocalk.md`](metrocalk.md) for the
vision, [`architecture.md`](architecture.md) for current state + the 5 invariants, and
[`decisions/`](decisions/) for the ADRs.

> Status: **M1 (foundation build)**. M0 spikes passed — see [`M0-gate-review.md`](M0-gate-review.md).

## Layout

```
core/        Rust lib — ECS wrapper, registry, commit pipeline, renderer   (workspace member)
transport/   Rust lib — the deltas-only protocol trait (3 impls land M2+)  (workspace member)
plugins/     Rust lib — Extism host + MCP seam (Phase 2+, stub)            (workspace member)
editor/      React/TS UI — NOT a cargo member (scaffolded M2–3)
spikes/      M0 throwaway spikes (loro, flecs, wasm) — excluded from the workspace
decisions/   ADRs (immutable)        prompts/   AI dev-session prompts
progress.md  Now/Next dashboard      progress/  per-milestone logs
```

## Toolchain

Rust **stable** (1.92+) with `rustfmt`, `clippy`, and the `wasm32-unknown-unknown` target. With
`rustup`, [`rust-toolchain.toml`](rust-toolchain.toml) installs all of that automatically on first
`cargo` invocation.

## Build / test / lint

```sh
cargo build              # workspace (core, transport, plugins) — excludes spikes
cargo test               # workspace tests
cargo fmt --all --check  # formatting
cargo clippy --workspace --all-targets -- -D warnings   # lints (pedantic, tuned; unsafe forbidden)
```

The workspace forbids `unsafe` (`Cargo.toml` `[workspace.lints]`); the future ecs-wrapper crate is
the documented exception. Spikes build standalone in their own dirs (e.g. `cd spikes/wasm`), not via
the workspace.

## Browser target

`flecs_ecs` does **not** cross to wasm32, so `core` is native-only; the browser query backend is
pure-Rust over the Loro projection (ADR-006). The `wasm-tripwire` CI builds the crates that *do*
cross (wgpu / loro) for `wasm32` on every push. See [`spikes/wasm/CONSTRAINTS.md`](spikes/wasm/CONSTRAINTS.md).

## CI

- `ci.yml` — `fmt` + `clippy -D warnings` + `cargo test` across the workspace.
- `wasm-tripwire.yml` — builds the wasm32 target so web-incompatible changes are caught immediately.
