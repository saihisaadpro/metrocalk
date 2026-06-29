//! M12.5 (ADR-049) — the **Play-time Rules session**: build a deterministic [`RuleRecording`] from the
//! authored scene at Play-start, so the engine thread can run the Rules + state machines as a **projection**
//! (never the ECS/Loro doc — ADR-021/ADR-034) and serve the live truth-state debugger + the time-travelable
//! decision history (the M8.4 channel reused, `core::rule_runtime`).
//!
//! This module is the **seam** between the authoritative [`Engine`] (the authored Rules/machines/scene on
//! Loro) and the pure [`core::rule_runtime`] runtime. It only *reads* the engine (`&Engine`): the runtime
//! mutates a [`RuntimeState`], **not** the document — so a Rule firing in Play can never corrupt the authored
//! scene or land on the undo stack, and Stop's snapshot-restore (ADR-034) wipes the whole run.
//!
//! [`core::rule_runtime`]: metrocalk_core::rule_runtime
//! [`RuleRecording`]: metrocalk_core::RuleRecording
//! [`RuntimeState`]: metrocalk_core::RuntimeState

use metrocalk_core::rule_runtime::partition_deterministic;
use metrocalk_core::{Engine, FlaggedRule, Registry, RuleRecording, RuntimeState};
use metrocalk_ecs::World;

/// What [`build_recording`] produces: the deterministic [`RuleRecording`] to replay (the M8.4-sibling
/// channel), plus the rules **flagged out** of it (a `RunPlugin` to a non-deterministic plugin — surfaced so
/// the UI can explain *why* a rule won't run in Play, never silently dropped).
pub struct PlaySession {
    /// The deterministic recording the engine thread replays each Play frame.
    pub recording: RuleRecording,
    /// Rules excluded from the deterministic path (non-deterministic plugin) — surfaced to the user.
    pub flagged: Vec<FlaggedRule>,
}

/// Capture a [`PlaySession`] from the authored scene at Play-start (ADR-049 deliverable 1). The initial
/// runtime state is seeded from every entity's **resolved** components (base + overrides — the same read the
/// renderer/inspector use), so a running Rule reads exactly what the user authored; the rules + state machines
/// are the authored set; and a rule whose `RunPlugin` action targets a **non-deterministic** plugin is held
/// out of the deterministic replay path (deliverable 5), using the registry's `PluginMeta.deterministic`
/// flag. Pure reads — the engine/document is never mutated.
#[must_use]
pub fn build_recording<W: World>(engine: &Engine<W>, registry: &Registry<W>) -> PlaySession {
    // Seed the frame-0 runtime state from the authored scene (resolved = base + overrides).
    let mut initial = RuntimeState::new();
    for id in engine.entity_ids() {
        let resolved = engine.resolved_components(id);
        if !resolved.is_empty() {
            initial.seed(&id.to_loro_key(), &resolved);
        }
    }

    // The authored Rules / state machines, partitioned by the determinism gate (M8.1 lockstep): only a
    // known-deterministic plugin may run in the replay path.
    let (kept, flagged) = partition_deterministic(&engine.rules(), |name| {
        registry.plugin(name).map(|p| p.deterministic)
    });

    let recording = RuleRecording::new(initial, kept, engine.state_machines());
    PlaySession { recording, flagged }
}
