//! The viewport **action model** (M3.3) — "what can I do to this entity?" — deterministic,
//! registry-driven, offline, O(1)/action, no side effects.
//!
//! Right-clicking an entity needs a menu of exactly the actions valid for it, with every *unavailable*
//! one greyed + **explained** (the M3.1 `why_not` discipline). This is the UI-agnostic substance that
//! survives the eventual React `/editor` port: a pure query over the engine + the capability scene that
//! returns data, not DOM. The mutating actions it offers are executed through the single commit
//! pipeline (`capscene::{remove_entity, duplicate_entity}` + the M3.1 `bind`); `Focus`/`Inspect` are
//! viewport/UI ops with no mutation.

use metrocalk_core::{Engine, EntityId};
use metrocalk_ecs::FlecsWorld;
use serde::Serialize;

use crate::capscene::CapScene;
use crate::reveal::required_caps;

/// A viewport action. Serialized as a stable lowercase id (`"bind"`, `"remove"`, …) for the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// Open the M3.1 reveal to bind an unmet requirement (only when the entity has one).
    Bind,
    /// Delete the entity + its edges — one undoable transaction.
    Remove,
    /// Clone the entity + its components/caps under a fresh id — one undoable transaction.
    Duplicate,
    /// Frame the camera on the entity — no mutation, not undoable.
    Focus,
    /// Select the entity + open its inspector — no mutation.
    Inspect,
}

impl Action {
    /// The menu label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Action::Bind => "Bind…",
            Action::Remove => "Remove",
            Action::Duplicate => "Duplicate",
            Action::Focus => "Focus",
            Action::Inspect => "Inspect",
        }
    }

    /// Whether the action mutates the scene (→ goes through the commit pipeline, is undoable).
    #[must_use]
    pub fn mutates(self) -> bool {
        matches!(self, Action::Remove | Action::Duplicate)
    }
}

/// One action's availability for an entity: the action, its label, whether it's available, and — when
/// not — the specific reason (every "no" explained), plus whether it mutates.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionItem {
    pub action: Action,
    pub label: String,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub mutates: bool,
}

impl ActionItem {
    fn make(action: Action, available: bool, reason: Option<String>) -> Self {
        Self {
            action,
            label: action.label().to_string(),
            available,
            reason,
            mutates: action.mutates(),
        }
    }
}

/// The valid actions for `id` + a reason for each unavailable one. Deterministic and side-effect-free.
/// `Bind…` is the only conditional action: available iff the entity is a requirer (has ≥1 required
/// capability) that isn't already bound — exactly the M3.1 "has an unbound required cap" condition;
/// otherwise greyed with the specific reason. Remove / Duplicate / Focus / Inspect are always available
/// for a live scene entity (a non-existent id greys everything — the only universal "no").
#[must_use]
pub fn actions_for(engine: &Engine<FlecsWorld>, scene: &CapScene, id: EntityId) -> Vec<ActionItem> {
    let Some(ecs) = engine.ecs_entity(id) else {
        // Not a live entity — nothing applies (a stale right-click after a Remove/Undo race).
        return [
            Action::Bind,
            Action::Remove,
            Action::Duplicate,
            Action::Focus,
            Action::Inspect,
        ]
        .into_iter()
        .map(|a| ActionItem::make(a, false, Some("entity no longer exists".to_string())))
        .collect();
    };

    // Bind…: reuse the reveal's required-caps read. Available iff the entity requires something and
    // hasn't already taken an outgoing binding to satisfy it.
    let requires = required_caps(engine.world(), ecs, scene.rels);
    let already_bound = engine.bindings().iter().any(|(from, _, _)| *from == id);
    let (bind_ok, bind_reason) = if requires.is_empty() {
        (
            false,
            Some("requires no capabilities — nothing to bind".to_string()),
        )
    } else if already_bound {
        (false, Some("already bound to a provider".to_string()))
    } else {
        (true, None)
    };

    vec![
        ActionItem::make(Action::Bind, bind_ok, bind_reason),
        ActionItem::make(Action::Remove, true, None),
        ActionItem::make(Action::Duplicate, true, None),
        ActionItem::make(Action::Focus, true, None),
        ActionItem::make(Action::Inspect, true, None),
    ]
}
