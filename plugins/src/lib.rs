//! `metrocalk-plugins` — M12.3 (ADR-047): the **WASM plugin host**, the *honest ceiling* below the Rules
//! layer. Rules (M12.1/M12.2) *orchestrate*; genuinely **algorithmic** behaviour (a boss AI, a procedural
//! generator, a custom solver) is a **code component compiled to a sandboxed WASM plugin** — keeping that
//! line is what stops Rules collapsing into no-code spaghetti.
//!
//! The host is **Extism** (BSD-3, wasmtime-backed), wrapped behind the project-owned [`PluginHost`] trait
//! (invariant 5 — no `extism::` type in the public API, grep-gated; the backend lives in
//! [`extism_host`]). A plugin runs **sandboxed** (a memory + execution-time budget + a host-fn allow-list);
//! a misbehaving plugin is **contained + explained** ([`PluginError`]), never an engine crash. The host is
//! **native** (wasmtime); the **browser plugin host is a named seam** (Extism's JS SDK), not built here.
//! A plugin's scene effects are applied **through the one commit pipeline** as schema-validated, undoable
//! transactions (the ADR-017 patch contract) by the caller — a plugin is **not** a raw mutation path.

mod extism_host;
mod host;

pub use extism_host::{ExtismHost, ExtismPlugin};
pub use host::{PluginError, PluginHost, PluginInstance, Sandbox};
