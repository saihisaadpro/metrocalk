//! M12.3 (ADR-047) — driving a **sandboxed WASM plugin** and landing its effect as an **undoable
//! transaction**. The honest ceiling: Rules orchestrate, a plugin computes the genuinely algorithmic work.
//!
//! The shell loads + runs the plugin through the project-owned [`metrocalk_plugins::PluginHost`] trait (no
//! `extism::` type crosses — grep-gated, invariant 5), sandboxed (a memory + time budget + a host-fn
//! allow-list). The plugin's output is an **AiPatch** — the exact ADR-017 patch contract — so its scene
//! effect is **schema-validated + applied through the one commit pipeline** ([`apply_ai_patch`]): undoable
//! on success, rejected-as-UX on a malformed/invalid op. A plugin is **not** a privileged mutation path; a
//! misbehaving plugin (trap / timeout / OOM / bad output) is **contained + explained**, never a crash.

use metrocalk_core::{ComponentMeta, Engine};
use metrocalk_ecs::World;
use metrocalk_plugins::{ExtismHost, PluginError, PluginHost, PluginInstance, Sandbox};

use crate::ai::{apply_ai_patch, AiPatch};
use crate::bridge::ProjectionDelta;

/// The checked-in WASM for the M12.3 example plugin (`plugins/example-plugin`, built to wasm32). A real
/// marketplace / AI-tier plugin would resolve from the content-addressed store; checking the example in
/// keeps the round-trip self-contained + reproducible.
const ARRANGE_WASM: &[u8] = include_bytes!("../../plugins/tests/fixtures/arrange.wasm");

/// Resolve a registered plugin name to its sandboxed `.wasm`. The plugin **vocabulary** is the registry's
/// [`metrocalk_core::PluginMeta`] (`stdlib::standard_plugins`); this maps a name to its bytes. By
/// convention the plugin's exported entry function shares the plugin name.
#[must_use]
pub fn plugin_wasm(name: &str) -> Option<&'static [u8]> {
    match name {
        "arrange" => Some(ARRANGE_WASM),
        _ => None,
    }
}

/// Run a registered plugin `name` with `input` (its own JSON contract) under the sandbox, and land its
/// proposed effect as a **schema-validated, undoable transaction** (the ADR-017 patch contract). Returns
/// the [`ProjectionDelta`] to echo — `confirms` on success, `rejects` with a reason if the plugin's output
/// is malformed/invalid (rejection-as-UX) — or `Err(PluginError)` if the sandboxed run itself failed
/// (contained + explained: a missing plugin, a trap, a timeout, an over-budget, or non-AiPatch output).
///
/// # Errors
/// A [`PluginError`] if the plugin can't be found/loaded, the sandboxed call traps/times-out/exceeds its
/// budget, or its output isn't a well-formed `AiPatch`.
pub fn run_plugin<W: World>(
    engine: &mut Engine<W>,
    schema: &[ComponentMeta],
    name: &str,
    input: &str,
) -> Result<ProjectionDelta, PluginError> {
    let wasm =
        plugin_wasm(name).ok_or_else(|| PluginError::Load(format!("no such plugin: '{name}'")))?;

    // Sandboxed load + call. A misbehaving plugin (trap / timeout / OOM / disallowed host fn) is contained
    // + explained right here — it never crashes the engine thread.
    let mut plugin = ExtismHost::new().load(wasm, &Sandbox::restrictive())?;
    let out = plugin.call(name, input.as_bytes())?;

    // The plugin's output IS an AiPatch (the ADR-017 shape). A non-AiPatch output is contained as BadOutput.
    let patch: AiPatch =
        serde_json::from_slice(&out).map_err(|e| PluginError::BadOutput(e.to_string()))?;

    // Validate + apply through the ONE commit pipeline — undoable on success, rejected-as-UX on an invalid
    // op (a plugin can't reach past the registry schema + engine state). The plugin is NOT a raw path.
    Ok(apply_ai_patch(engine, schema, "run-plugin", &patch))
}
