//! The viewport **action model** (M3.3) — "what can I do to this entity?" — deterministic,
//! registry-driven, offline, O(1)/action, no side effects.
//!
//! Right-clicking an entity needs a menu of exactly the actions valid for it, with every *unavailable*
//! one greyed + **explained** (the M3.1 `why_not` discipline). This is the UI-agnostic substance that
//! survives the eventual React `/editor` port: a pure query over the engine + the capability scene that
//! returns data, not DOM. The mutating actions it offers are executed through the single commit
//! pipeline (`capscene::{remove_entity, duplicate_entity}` + the M3.1 `bind`); `Focus`/`Inspect` are
//! viewport/UI ops with no mutation.

use std::collections::HashSet;

use metrocalk_core::{Engine, EntityId};
use metrocalk_ecs::{Entity, FlecsWorld, World};
use serde::Serialize;

use crate::capscene::CapScene;
use crate::reveal::required_caps;

/// A viewport action. Serialized as a stable lowercase id (`"bind"`, `"remove"`, …) for the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// Open the M3.1 reveal to bind an unmet requirement (only when the entity has one).
    Bind,
    /// **Delete the selection** — deactivate every selected object and free its dependents, one
    /// undoable transaction (`capscene::delete_deactivate_many`).
    ///
    /// The tag stays `remove` because it is a stable wire id (ADR-016, the `.exe` acceptance suite and
    /// the scaffold's control map both name it), but the verb it stands for is the scene-authoring
    /// delete, not M3.3's destructive `remove_entity`. Those were two deletes with two names, two
    /// recoverability stories and two scopes, reachable from two gestures on the same object: the
    /// authoring toolbar's `Delete` deactivated the whole selection and could be undone across a
    /// reopen, while this menu's `Remove` destroyed exactly one. A person cannot be expected to know
    /// which one they just used.
    Remove,
    /// Clone the entity + its components/caps under a fresh id — one undoable transaction.
    Duplicate,
    /// Frame the camera on the entity — no mutation, not undoable.
    Focus,
    /// Select the entity + open its inspector — no mutation.
    Inspect,
    /// M8.3: turn a dead mesh model into a correct dynamic body (RigidBody + Collider auto-derived) — one
    /// undoable transaction. Offered only for a mesh that isn't already a physics body.
    MakeDynamic,
}

impl Action {
    /// The menu label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Action::Bind => "Bind…",
            Action::Remove => "Delete",
            Action::Duplicate => "Duplicate",
            Action::Focus => "Focus",
            Action::Inspect => "Inspect",
            Action::MakeDynamic => "Make dynamic",
        }
    }

    /// Whether the action mutates the scene (→ goes through the commit pipeline, is undoable).
    #[must_use]
    pub fn mutates(self) -> bool {
        matches!(
            self,
            Action::Remove | Action::Duplicate | Action::MakeDynamic
        )
    }
}

/// Every action the model knows, in menu order — the one list, so a new verb cannot be added to the
/// answer for a live entity and forgotten in the answer for a dead one.
const EVERY_ACTION: [Action; 6] = [
    Action::Bind,
    Action::Remove,
    Action::Duplicate,
    Action::Focus,
    Action::Inspect,
    Action::MakeDynamic,
];

/// One action's availability for a selection: the action, its label, whether it's available, and —
/// when not — the specific reason (every "no" explained), plus whether it mutates.
///
/// `applies_to` is how many of the selected objects the verb would actually act on, and it exists
/// because the selection stopped being one object. A menu row over a selection of 378 that says
/// nothing about its scope tells the same lie `Delete` used to tell from the authoring toolbar: the
/// trigger read `Actions · 14` and the verb changed one of them. The number lives here rather than
/// inside the wording so the caller can put it where the gesture is, and so a test can assert scope
/// without asserting copy (`<test_and_ci_discipline>` 3).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionItem {
    pub action: Action,
    pub label: String,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub mutates: bool,
    /// How many of the selected objects this verb acts on. `0` exactly when `available` is false.
    pub applies_to: usize,
}

impl ActionItem {
    /// `applies_to` IS the availability — a verb that acts on nothing is not available, and there is
    /// no third state. Keeping them one argument is what stops a row rendering enabled with a scope
    /// of zero, which is `<ux_quality>` 6's inert control with extra steps.
    fn make(action: Action, applies_to: usize, reason: Option<String>) -> Self {
        Self {
            action,
            label: action.label().to_string(),
            available: applies_to > 0,
            reason,
            mutates: action.mutates(),
            applies_to,
        }
    }
}

/// The action model's answer about a whole selection.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionActions {
    /// How many of the requested ids are live entities — what every `applies_to` is measured against.
    pub count: usize,
    /// How many were asked about and no longer exist (a stale right-click after a Remove/Undo race).
    pub missing: usize,
    pub items: Vec<ActionItem>,
}

/// The valid actions for `id` + a reason for each unavailable one. Deterministic and side-effect-free.
///
/// The single-entity form of [`actions_for_selection`], and it is written as a *call* to it rather
/// than beside it. Two statements of one availability policy are two things the compiler checks
/// separately and never against each other — which is exactly how a set form and a single form come
/// to disagree about the same object while both compile.
#[must_use]
pub fn actions_for(engine: &Engine<FlecsWorld>, scene: &CapScene, id: EntityId) -> Vec<ActionItem> {
    actions_for_selection(engine, scene, &[id]).items
}

/// **The valid actions for a SELECTION** — the same model, asked about many objects at once.
///
/// The editor's selection has been a set since M10.6, and every surface that *consumes* one was
/// rebuilt around that: ADR-169's inspector edits the set, ADR-158's marquee produces one, ADR-176's
/// `Select similar` routinely answers with 378 objects. The action model was still answering about
/// exactly one, so the most direct surface in the product — right-click — offered the weakest verbs
/// in it.
///
/// **The rule, per verb: act on the whole set, or say which one you act on.**
/// - `Remove` · `Focus` · `Inspect` apply to every live member.
/// - `Make dynamic` applies to the members that are dead meshes — a *partial* count, which is the
///   honest answer for a mixed selection and is why `applies_to` is a number and not a flag.
/// - `Bind…` and `Duplicate` are **primary-only**: the reveal asks its question about one requirer,
///   and a clone lands beside one source. They report `applies_to == 1` over any selection, so the
///   caller can name the object rather than telling a person "duplicate" about 378 things and
///   handing back one.
///
/// **One pass, not N.** The bindings are swept once for the whole selection rather than once per
/// entity — the same reason `capscene::delete_deactivate_many` sweeps them once. Per-entity this is
/// O(N·B) with B every binding in the document, and a 378-object right-click on the 15,711-part
/// assembly would pay for that inside the 16 ms the menu has to open in.
#[must_use]
pub fn actions_for_selection(
    engine: &Engine<FlecsWorld>,
    scene: &CapScene,
    ids: &[EntityId],
) -> SelectionActions {
    // De-duplicate while keeping order: the primary is the LAST id (the projection store's own
    // convention), and a repeated id would otherwise inflate every count the menu prints.
    let mut live: Vec<(EntityId, Entity)> = Vec::with_capacity(ids.len());
    let mut seen: HashSet<EntityId> = HashSet::new();
    let mut missing = 0usize;
    for &id in ids {
        if !seen.insert(id) {
            continue;
        }
        match engine.ecs_entity(id) {
            Some(ecs) => live.push((id, ecs)),
            None => missing += 1,
        }
    }

    if live.is_empty() {
        // Nothing live to act on. The two ways to arrive here are different facts about the world and
        // a person can act on exactly one of them, so they do not share a sentence.
        let reason = if missing > 0 {
            "the selected objects no longer exist"
        } else {
            "nothing is selected"
        };
        return SelectionActions {
            count: 0,
            missing,
            items: EVERY_ACTION
                .into_iter()
                .map(|a| ActionItem::make(a, 0, Some(reason.to_string())))
                .collect(),
        };
    }

    let count = live.len();
    // The primary is the LAST id — the object the user acted on most recently, and the one the
    // inspector and the reveal are already showing.
    let (primary, primary_ecs) = *live.last().expect("live is non-empty");

    // ONE sweep of the bindings for the whole selection: which capability each selected requirer has
    // already had satisfied by something it is bound to.
    let selected: HashSet<EntityId> = live.iter().map(|(id, _)| *id).collect();
    let mut satisfied: HashSet<(EntityId, Entity)> = HashSet::new();
    for (from, _, to) in engine.bindings() {
        if !selected.contains(&from) {
            continue;
        }
        if let Some(to_ecs) = engine.ecs_entity(to) {
            for cap in engine.world().targets(to_ecs, scene.rels.provides) {
                satisfied.insert((from, cap));
            }
        }
    }

    // Bind…: the reveal is a question about ONE requirer's unmet capability, so it is answered about
    // the primary however many objects are selected. A multi-capability requirer bound for one cap can
    // still bind the others — hence the per-(entity, cap) correlation above rather than "has any
    // binding".
    let requires = required_caps(engine.world(), primary_ecs, scene.rels);
    let (bind_scope, bind_reason) = if requires.is_empty() {
        (
            0,
            Some("requires no capabilities, so there is nothing to bind".to_string()),
        )
    } else if !requires.iter().any(|c| !satisfied.contains(&(primary, *c))) {
        (
            0,
            Some("all required capabilities are already bound".to_string()),
        )
    } else {
        (1, None)
    };

    // Make dynamic (M8.3): every selected dead mesh becomes a body in one gesture. A mixed selection
    // gets the count it CAN act on rather than a flat refusal — but a count is not an explanation, so
    // the reason still names what the rest of them are.
    let dynamic_ready = live
        .iter()
        .filter(|(id, _)| crate::physics_intent::looks_dynamic(engine, *id))
        .count();
    let already_bodies = live
        .iter()
        .filter(|(id, _)| engine.components_of(*id).contains_key("RigidBody"))
        .count();
    let md_reason = if dynamic_ready > 0 {
        (dynamic_ready < count).then(|| {
            format!(
                "{dynamic_ready} of {count} can — the rest are already bodies, or are not meshes"
            )
        })
    } else if already_bodies == count {
        Some(if count == 1 {
            "already a physics body".to_string()
        } else {
            "all of them are already physics bodies".to_string()
        })
    } else {
        Some("only a mesh model can be made dynamic".to_string())
    };

    SelectionActions {
        count,
        missing,
        items: vec![
            ActionItem::make(Action::Bind, bind_scope, bind_reason),
            ActionItem::make(Action::Remove, count, None),
            // Primary-only, said through the number rather than through prose the caller would have to
            // parse: one clone, beside one source, however many are selected.
            ActionItem::make(Action::Duplicate, 1, None),
            ActionItem::make(Action::Focus, count, None),
            ActionItem::make(Action::Inspect, count, None),
            ActionItem::make(Action::MakeDynamic, dynamic_ready, md_reason),
        ],
    }
}
