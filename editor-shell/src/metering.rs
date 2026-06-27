//! The metered paid-action seam (M7) — the synchronous sinks (marketplace **buy**, **AI-edit**) call
//! the wallet through here, so the *debit-on-success / refund-on-failure / graceful-refusal* logic is
//! one tested place the live shell and the headless tests share. (Generation is async — its
//! reserve→settle/release lives in the engine thread; the wallet calls are [`Wallet::reserve_generate`]
//! etc.)
//!
//! Ordering note (the two-log seam): the scene mutation is applied FIRST; the wallet is charged only on
//! its success (so a rejected/failed action never bills). The window between the scene commit and the
//! wallet charge is microseconds on the single engine thread — documented as the known weakest seam
//! (ADR); no real money moves.

use metrocalk_core::stdlib;
use metrocalk_core::{Engine, EntityId};
use metrocalk_economy::{creator_of, Action};
use metrocalk_ecs::FlecsWorld;
use serde_json::Value as Json;

use crate::ai::{apply_ai_patch, AiPatch, PatchOp};
use crate::bridge::ProjectionDelta;
use crate::capscene::{self, CapScene};
use crate::wallet::Wallet;

/// The "make it rustier" AI-edit sets the asset's **material** to a rusty variant — a deterministic,
/// offline stand-in for an LLM edit (the real LLM is a seam, like the generation provider). It rides
/// the M6 schema-validated AI-patch path (`MeshRenderer.material`, a known String field).
const RUSTY_MATERIAL: &str = "rusty";

/// The outcome of a metered paid action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Applied + charged `cost_tokens`; `balance_tokens` is the user's balance after.
    Charged {
        /// Tokens debited.
        cost_tokens: u32,
        /// The user's balance after the charge.
        balance_tokens: u32,
    },
    /// Refused (insufficient balance) — the honest "top up?" UX. Nothing applied, nothing charged.
    Refused {
        /// Tokens the action needs.
        needed: u32,
        /// Tokens the user has.
        have: u32,
    },
    /// The scene mutation itself failed/was rejected — never charged.
    Rejected(String),
}

/// Apply a marketplace **buy**: check affordability first (refuse gracefully when broke, no scene
/// change), apply the pre-componentized entry, then on success debit the price + accrue ~70% to the
/// creator (its id namespace). Returns the created entity id (when applied) + the outcome.
pub fn buy_marketplace(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    wallet: &mut Wallet,
    entry: &metrocalk_core::marketplace::MarketplaceEntry,
    mesh: Option<&str>,
    pos: [f32; 3],
    ref_id: &str,
) -> (Option<EntityId>, Outcome) {
    let action = Action::Buy {
        price_tokens: entry.price.unwrap_or(0),
        creator: creator_of(&entry.id),
    };
    if !wallet.can_afford(&action) {
        return (
            None,
            Outcome::Refused {
                needed: action.cost().whole_tokens(),
                have: wallet.available_tokens(),
            },
        );
    }
    match capscene::apply_marketplace_entry(engine, scene, entry, pos, mesh) {
        // Pre-checked affordable + single-threaded ⇒ a Refusal here is unreachable; and we treat any
        // non-persisted charge as a FAILED buy (not recorded) so a wallet-write failure can never leave
        // a scene buy without its charge (no free paid tier). The caller persists the scene only on
        // Outcome::Charged, which requires `wallet.last_write_ok()`.
        Ok(id) => match wallet.charge(&action, ref_id) {
            Ok(cost) if wallet.last_write_ok() => (
                Some(id),
                Outcome::Charged {
                    cost_tokens: cost,
                    balance_tokens: wallet.balance_tokens(),
                },
            ),
            _ => (
                None,
                Outcome::Rejected("wallet write failed — buy not recorded".to_string()),
            ),
        },
        Err(e) => (None, Outcome::Rejected(e.to_string())),
    }
}

/// Apply the "make it rustier" **AI-edit** to `id`: validate the material is a known finish, check
/// affordability, apply the schema-validated patch (`MeshRenderer.material = "rusty"`) through the one
/// pipeline (undoable), and on success debit the edit rate. A rejected patch (a non-existent entity, or
/// an **unknown material** that the renderer has no preset for) is never charged. `known` is supplied by
/// the caller, which owns the render material vocabulary (`material_preset`). Returns the projection
/// delta to echo (when applied) + the outcome.
pub fn ai_edit_material(
    engine: &mut Engine<FlecsWorld>,
    wallet: &mut Wallet,
    id: EntityId,
    ref_id: &str,
    material: &str,
    known: bool,
) -> (Option<ProjectionDelta>, Outcome) {
    if !known {
        // M11.2 (audit P1): an unknown material name passes the schema (it is a valid String) but the
        // renderer has no preset for it → without this guard the patch would apply + CHARGE, then render
        // unchanged — a silent no-op the user paid for. Reject BEFORE metering: never debit for an edit
        // that cannot change the picture (cost legible · every "no" explained, ADR-016).
        return (
            None,
            Outcome::Rejected(format!(
                "unknown material \"{material}\" — choose a known finish (e.g. rusty, metal, chrome, gold, copper, plastic)"
            )),
        );
    }
    if !wallet.can_afford(&Action::Edit) {
        return (
            None,
            Outcome::Refused {
                needed: Action::Edit.cost().whole_tokens(),
                have: wallet.available_tokens(),
            },
        );
    }
    // M11.2 (ADR-041): the AI-edit assigns a named PBR material preset (the render maps it to a per-entity
    // metallic-roughness override). `material` is the chosen preset (the UI palette / suggestion), defaulting
    // to "rusty" (the original "weathered-metal look"). Always through the one schema-validated patch.
    let patch = AiPatch {
        client_op_id: "ai-edit".to_string(),
        ops: vec![PatchOp::SetField {
            id: id.to_loro_key(),
            component: "MeshRenderer".to_string(),
            field: "material".to_string(),
            value: Json::String(material.to_string()),
        }],
    };
    let delta = apply_ai_patch(
        engine,
        &stdlib::standard_components(),
        "ai-edit-rustier",
        &patch,
    );
    if delta.rejects.is_empty() {
        // The patch was accepted (the edit "succeeded") → charge; a non-persisted charge is a failed
        // edit (not recorded), so a wallet-write failure can't leave a scene edit without its charge.
        match wallet.charge(&Action::Edit, ref_id) {
            Ok(cost) if wallet.last_write_ok() => (
                Some(delta),
                Outcome::Charged {
                    cost_tokens: cost,
                    balance_tokens: wallet.balance_tokens(),
                },
            ),
            _ => (
                None,
                Outcome::Rejected("wallet write failed — edit not recorded".to_string()),
            ),
        }
    } else {
        let reason = delta
            .rejects
            .first()
            .map_or_else(|| "rejected".to_string(), |r| r.reason.clone());
        (None, Outcome::Rejected(reason))
    }
}

/// The schema-validated patch the live AI-edit applies, exposed for replay (`persist::Record::AiEdit`):
/// re-apply on reload so a rusty edit survives close→reopen (the wallet is persisted separately, so
/// replay never re-charges).
#[must_use]
pub fn material_patch(id: EntityId, material: &str) -> AiPatch {
    AiPatch {
        client_op_id: "replay-ai-edit".to_string(),
        ops: vec![PatchOp::SetField {
            id: id.to_loro_key(),
            component: "MeshRenderer".to_string(),
            field: "material".to_string(),
            value: Json::String(material.to_string()),
        }],
    }
}

/// The default AI-edit material when none is named (the original "weathered-metal look").
pub const RUSTY_MATERIAL_NAME: &str = RUSTY_MATERIAL;
