//! M15.1 (ADR-071) — the **data-model-as-digital-thread**: the CAD revision / PDM model on the shipped
//! substrate, **kernel-free**.
//!
//! The dossier's strongest finding: PDM/PLM — the "digital thread" the CAD industry theorizes as DTaaS —
//! is, structurally, what Metrocalk already is. Onshape rebuilt CAD on a cloud database with branch/merge
//! version-control + real-time collaboration over the Parasolid kernel; **that is a CRDT op-log, which we
//! have shipped (Loro, ADR-002), plus a cryptographic provenance Onshape does not even have**. This module
//! is the **CAD-domain surfacing** of that latent property — *not* a new version system.
//!
//! A **released revision** = an op-log logical state ([`crate::pipeline::Engine::canonical_state`]) + a
//! **content hash that is its identity**, tamper-evident via the **M11.5 Ed25519 trust model** (ADR-044) —
//! the thing Onshape lacks. The crypto lives in `metrocalk-assets` behind the `ProvenanceVerifier` trait
//! (invariant 5); this module owns the *data model* (the [`Revision`] record + the deterministic
//! [`Revision::metadata_source`] the signature binds), and the editor shell's PDM service does the signing.
//!
//! A **revision lifecycle** (InWork → InReview → Released → Obsolete) is a small enforced state machine
//! ([`Lifecycle`]), and the same lifecycle is expressible as an **M12.2 [`StateMachine`]**
//! ([`lifecycle_state_machine`], ADR-046) — the *hook* for governed transitions. Full enterprise PLM
//! governance (21 CFR Part 11 / ITAR / e-signatures / ERP/MES) is **process+legal+sales, not a data
//! model** — a **named future**, explicitly out of scope and never claimed shipped.
//!
//! **Honest scope** (don't paper over it): this reuses the shipped substrate; it does NOT build a parallel
//! version system, and the "digital thread" claim is scoped to **what the op-log actually carries**
//! (requirement → geometry → BOM → approval as replayable op-log entries), *not* the ERP/MES/field-IoT
//! integrations a full PLM thread spans (those are named seams). Real-time multi-user is CRDT
//! eventual-consistency (excellent for concurrent/async/offline-first), **not** an authoritative-lock model
//! (the seamed-governance future). No geometry-kernel dependency — the value lands kernel-free.

use crate::registry::{ComponentMeta, EventMeta, FieldType};
use crate::rules::RuleData;
use crate::state_machine::{StateMachine, Transition};
use serde::{Deserialize, Serialize};
use std::fmt;

/// The serialization version of a revision's canonical metadata. Versioned so the binding format can
/// evolve without an older signature silently verifying under new semantics (mirrors `mtk-prov-v1`).
const REVISION_MANIFEST_VERSION: &str = "mtk-rev-v1";

/// A released revision's **lifecycle** state (the PDM maturity of a revision). A small, *enforced* state
/// machine: [`Lifecycle::can_transition_to`] is the allowed-edge guard. The same lifecycle is also
/// expressible as an M12.2 [`StateMachine`] (see [`lifecycle_state_machine`]) — the hook for governed
/// transitions (approvals, e-signatures), which is the named enterprise-governance future, not this tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lifecycle {
    /// Being authored — editable, not yet submitted. The initial state of a fresh revision.
    InWork,
    /// Submitted for review — awaiting approval (or send-back to `InWork`).
    InReview,
    /// Approved + released — **immutable**: its content hash is frozen and a new edit branches a *new*
    /// revision, never mutating this one. The only onward transition is supersession (`Obsolete`).
    Released,
    /// Superseded — a terminal state (a newer released revision replaced it).
    Obsolete,
}

impl Lifecycle {
    /// Every lifecycle state, in maturity order (the M12.2 machine's `states`).
    #[must_use]
    pub const fn all() -> [Lifecycle; 4] {
        [
            Lifecycle::InWork,
            Lifecycle::InReview,
            Lifecycle::Released,
            Lifecycle::Obsolete,
        ]
    }

    /// The stable string tag (used in the signed metadata + the M12.2 state names + the React surface).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Lifecycle::InWork => "InWork",
            Lifecycle::InReview => "InReview",
            Lifecycle::Released => "Released",
            Lifecycle::Obsolete => "Obsolete",
        }
    }

    /// Parse a lifecycle tag (the inverse of [`as_str`](Self::as_str)).
    #[must_use]
    pub fn from_tag(s: &str) -> Option<Lifecycle> {
        Lifecycle::all().into_iter().find(|l| l.as_str() == s)
    }

    /// Whether this revision's content is **frozen** (released or obsolete) — an edit must branch a new
    /// revision rather than mutate it (the released-is-immutable property).
    #[must_use]
    pub const fn is_frozen(self) -> bool {
        matches!(self, Lifecycle::Released | Lifecycle::Obsolete)
    }

    /// Whether a transition `self → to` is allowed (the enforced lifecycle graph):
    /// `InWork → InReview`, `InReview → {Released, InWork}` (approve / send-back), `Released → Obsolete`.
    /// Skipping review (`InWork → Released`), editing a released revision (`Released → InWork`), and any
    /// transition out of the terminal `Obsolete` are **rejected** (Blocked + explained, ADR-016).
    #[must_use]
    pub const fn can_transition_to(self, to: Lifecycle) -> bool {
        matches!(
            (self, to),
            (Lifecycle::InWork, Lifecycle::InReview)
                | (Lifecycle::InReview, Lifecycle::Released | Lifecycle::InWork)
                | (Lifecycle::Released, Lifecycle::Obsolete)
        )
    }
}

impl fmt::Display for Lifecycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a lifecycle transition was rejected — a **plain-language, ASCII-safe** explanation (ADR-016:
/// Blocked + explained, never a silent bad transition), legible through every IPC layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LifecycleError {
    /// The requested edge is not in the allowed lifecycle graph.
    InvalidTransition {
        /// The current lifecycle state.
        from: Lifecycle,
        /// The rejected target state.
        to: Lifecycle,
    },
}

impl fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LifecycleError::InvalidTransition { from, to } => write!(
                f,
                "cannot move a revision from {from} to {to}: the allowed lifecycle is \
                 InWork -> InReview -> Released -> Obsolete (a released revision is immutable; \
                 edit it by branching a new revision)"
            ),
        }
    }
}

impl std::error::Error for LifecycleError {}

/// A **released-revision record** — an op-log logical state's content hash + its PDM metadata, optionally
/// signed (M11.5 Ed25519). This is the *data model* of the digital thread; the editor shell's PDM service
/// (`metrocalk-editor-shell::pdm`) computes the content hash (`metrocalk-assets::AssetId`) and the
/// signature (`SignedProvenanceTrust`, behind the `ProvenanceVerifier` trait), then stamps them here.
///
/// The identity is **content-addressed + reproducible**: re-deriving the canonical logical state on reload
/// yields the same `state_hash`, so a released revision is immutable and tamper-evident — re-derive the
/// hash from the same state and you get the same identity, and a tampered byte breaks the signature.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Revision {
    /// The monotonic revision number within a thread (1, 2, 3, …).
    pub number: u64,
    /// The `state_hash` of the parent revision this one descends from (`None` for the first revision) —
    /// the digital-thread back-link.
    pub parent: Option<String>,
    /// The content-addressed identity = the content hash of the canonical logical state
    /// ([`crate::pipeline::Engine::canonical_state`]). Empty until the revision is released/sealed.
    pub state_hash: String,
    /// The PDM lifecycle state.
    pub lifecycle: Lifecycle,
    /// Who authored/released the revision (a project identity string).
    pub author: String,
    /// The hex Ed25519 signature over [`metadata_source`](Self::metadata_source) bound to `state_hash`
    /// (M11.5 / ADR-044). `None` = unsigned (an in-work draft).
    pub signature: Option<String>,
    /// The hex Ed25519 public key of the signer — checked against the trusted-key set on verify. `None`
    /// = unsigned.
    pub signer: Option<String>,
}

impl Revision {
    /// A fresh **in-work** revision (unsigned, no state hash yet — the shell seals it on release).
    #[must_use]
    pub fn new(number: u64, parent: Option<String>, author: impl Into<String>) -> Self {
        Self {
            number,
            parent,
            state_hash: String::new(),
            lifecycle: Lifecycle::InWork,
            author: author.into(),
            signature: None,
            signer: None,
        }
    }

    /// The **canonical metadata assertion set** that the signature binds — a deterministic, versioned,
    /// float-free string carrying the revision's **immutable identity** (its number, parent, and author).
    ///
    /// Deliberately **excludes** two things: the `state_hash` (bound separately, by the content-address —
    /// the M11.5 `Provenance.content_hash`), and the `lifecycle` (mutable *workflow status*: a Released
    /// revision legitimately becomes Obsolete when superseded, which must not break its release signature —
    /// obsolescence is a later annotation, not a re-identification). What is signed is *what was released*:
    /// which revision, of which parent, by whom, of which content. Tampering any of those changes these
    /// bytes (or the bound content hash) and the signature no longer verifies. Versioned (`mtk-rev-v1`) so
    /// the format can evolve without an older signature silently verifying.
    #[must_use]
    pub fn metadata_source(&self) -> String {
        format!(
            "{REVISION_MANIFEST_VERSION};number={};parent={};author={}",
            self.number,
            self.parent.as_deref().unwrap_or(""),
            self.author,
        )
    }

    /// Whether the revision carries a cryptographic signature (a released, sealed revision).
    #[must_use]
    pub fn is_signed(&self) -> bool {
        self.signature.is_some() && self.signer.is_some()
    }

    /// Move the revision to a new lifecycle state, enforcing the allowed graph
    /// ([`Lifecycle::can_transition_to`]).
    ///
    /// # Errors
    /// [`LifecycleError::InvalidTransition`] when the edge is not allowed — Blocked + explained.
    pub fn transition_to(&mut self, to: Lifecycle) -> Result<(), LifecycleError> {
        if self.lifecycle.can_transition_to(to) {
            self.lifecycle = to;
            Ok(())
        } else {
            Err(LifecycleError::InvalidTransition {
                from: self.lifecycle,
                to,
            })
        }
    }
}

// ── The M12.2 lifecycle hook (ADR-046) ──────────────────────────────────────────────────────────────────

/// The component a revision's lifecycle state lives on, for the M12.2 [`StateMachine`] representation:
/// `RevisionLifecycle { state: String }`. Register it (`Registry::register`) before validating
/// [`lifecycle_state_machine`] — the state field MUST be a registry `String` field (the M12.2 rule).
#[must_use]
pub fn revision_lifecycle_component() -> ComponentMeta {
    ComponentMeta::builder("RevisionLifecycle")
        .field("state", FieldType::String, true)
        .build()
}

/// The lifecycle events the M12.2 transitions trigger on — register them
/// (`Registry::register_event`) before validating [`lifecycle_state_machine`]. These are the *hook* for
/// governed transitions (an approval event carrying an e-signature is the enterprise-governance future).
#[must_use]
pub fn lifecycle_events() -> Vec<EventMeta> {
    vec![
        EventMeta::new(
            "SubmitForReview",
            "An author submits a revision for review.",
        ),
        EventMeta::new("Approve", "A reviewer approves + releases a revision."),
        EventMeta::new(
            "RequestChanges",
            "A reviewer sends a revision back to in-work.",
        ),
        EventMeta::new(
            "Supersede",
            "A newer released revision supersedes this one.",
        ),
    ]
}

/// The revision lifecycle modeled as an **M12.2 [`StateMachine`]** (ADR-046) — the *hook* deliverable: the
/// same `InWork → InReview → Released → Obsolete` lifecycle, expressed as the shipped state-machine data
/// (each transition an M12.1 Rule with the canonical enter-state action). Validate it with
/// `validate_state_machine` after registering [`revision_lifecycle_component`] + [`lifecycle_events`].
///
/// `entity` is the revision-thread entity's [`crate::entity_id::EntityId::to_loro_key`] string. Full
/// enterprise governance (gated approvals, e-signatures advancing the live `current` slot — the M12.5
/// seam) is the **named future**; this proves the lifecycle is the substrate's, not a bespoke FSM.
#[must_use]
pub fn lifecycle_state_machine(entity: &str) -> StateMachine {
    let mut sm = StateMachine {
        name: "RevisionLifecycle".into(),
        entity: entity.into(),
        component: "RevisionLifecycle".into(),
        field: "state".into(),
        states: Lifecycle::all().iter().map(|l| l.as_str().into()).collect(),
        initial: Lifecycle::InWork.as_str().into(),
        transitions: vec![],
    };
    // Each transition IS an M12.1 Rule whose Then is the canonical enter-state action (built from the
    // machine, so the effect can never typo the state field) — the M12.2 reuse, not a parallel model.
    let edge =
        |sm: &StateMachine, id: &str, from: Lifecycle, to: Lifecycle, event: &str| Transition {
            id: id.into(),
            from: from.as_str().into(),
            to: to.as_str().into(),
            rule: RuleData {
                name: format!("{from} -> {to}"),
                enabled: true,
                event: event.into(),
                conditions: vec![],
                actions: vec![sm.enter_action(to.as_str())],
            },
        };
    sm.transitions = vec![
        edge(
            &sm,
            "submit",
            Lifecycle::InWork,
            Lifecycle::InReview,
            "SubmitForReview",
        ),
        edge(
            &sm,
            "approve",
            Lifecycle::InReview,
            Lifecycle::Released,
            "Approve",
        ),
        edge(
            &sm,
            "reject",
            Lifecycle::InReview,
            Lifecycle::InWork,
            "RequestChanges",
        ),
        edge(
            &sm,
            "obsolete",
            Lifecycle::Released,
            Lifecycle::Obsolete,
            "Supersede",
        ),
    ];
    sm
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::Op;
    use crate::state_machine::validate_state_machine;
    use crate::stdlib::standard_actions;
    use crate::{Engine, Registry};
    use metrocalk_ecs::FlecsWorld;

    #[test]
    fn lifecycle_allows_only_the_real_graph() {
        // The forward maturity path + the send-back edge are allowed.
        assert!(Lifecycle::InWork.can_transition_to(Lifecycle::InReview));
        assert!(Lifecycle::InReview.can_transition_to(Lifecycle::Released));
        assert!(Lifecycle::InReview.can_transition_to(Lifecycle::InWork));
        assert!(Lifecycle::Released.can_transition_to(Lifecycle::Obsolete));
    }

    #[test]
    fn lifecycle_blocks_skips_and_mutating_a_released_revision() {
        // Skipping review is rejected (no InWork -> Released).
        assert!(!Lifecycle::InWork.can_transition_to(Lifecycle::Released));
        // A released revision is immutable — you cannot send it back to in-work.
        assert!(!Lifecycle::Released.can_transition_to(Lifecycle::InWork));
        // Obsolete is terminal.
        assert!(!Lifecycle::Obsolete.can_transition_to(Lifecycle::Released));
        assert!(Lifecycle::Released.is_frozen());
        assert!(!Lifecycle::InWork.is_frozen());
    }

    #[test]
    fn transition_to_is_blocked_and_explained() {
        let mut rev = Revision::new(1, None, "alice");
        // The full valid chain works.
        rev.transition_to(Lifecycle::InReview).unwrap();
        rev.transition_to(Lifecycle::Released).unwrap();
        rev.transition_to(Lifecycle::Obsolete).unwrap();
        // An invalid edge is Blocked + explained (the message names both states + the rule).
        let mut draft = Revision::new(2, None, "bob");
        let err = draft.transition_to(Lifecycle::Released).unwrap_err();
        assert_eq!(
            err,
            LifecycleError::InvalidTransition {
                from: Lifecycle::InWork,
                to: Lifecycle::Released
            }
        );
        let msg = err.to_string();
        assert!(msg.contains("InWork") && msg.contains("Released") && msg.contains("immutable"));
        assert!(
            msg.is_ascii(),
            "the explanation is ASCII-legible through IPC"
        );
    }

    #[test]
    fn metadata_source_is_deterministic_versioned_and_lifecycle_independent() {
        let rev = Revision {
            number: 7,
            parent: Some("mtkasset:abc".into()),
            state_hash: "mtkasset:def".into(),
            lifecycle: Lifecycle::Released,
            author: "alice".into(),
            signature: None,
            signer: None,
        };
        let s = rev.metadata_source();
        assert_eq!(s, "mtk-rev-v1;number=7;parent=mtkasset:abc;author=alice");
        // Deterministic (no clock / no float) — the same record always serializes identically.
        assert_eq!(s, rev.metadata_source());
        // The state hash is NOT in the metadata source (it is bound separately by the content-address).
        assert!(!s.contains("mtkasset:def"));
        // The lifecycle is NOT in the signed identity — a Released revision becoming Obsolete must not
        // break its release signature (obsolescence is a later status, not a re-identification).
        let mut obsoleted = rev.clone();
        obsoleted.lifecycle = Lifecycle::Obsolete;
        assert_eq!(
            s,
            obsoleted.metadata_source(),
            "lifecycle does not affect the signed identity"
        );
        // Tampering an IMMUTABLE field (author/parent/number) DOES change the signed bytes.
        let mut tampered = rev.clone();
        tampered.author = "mallory".into();
        assert_ne!(s, tampered.metadata_source());
    }

    #[test]
    fn lifecycle_is_modeled_as_a_valid_m12_2_state_machine() {
        // The "modeled with M12.2 as the hook" deliverable, proven (not just claimed): the lifecycle is a
        // valid M12.2 StateMachine over the registry-typed RevisionLifecycle component.
        let mut reg = Registry::new(FlecsWorld::new());
        reg.register(revision_lifecycle_component())
            .expect("RevisionLifecycle registers");
        for ev in lifecycle_events() {
            reg.register_event(ev);
        }
        for ac in standard_actions() {
            reg.register_action(ac);
        }

        let mut engine = Engine::new(FlecsWorld::new(), 1);
        let e = engine.alloc_entity_id();
        engine
            .commit(
                "thread",
                vec![Op::CreateEntity {
                    id: e,
                    parent: None,
                }],
            )
            .expect("create the revision-thread entity");
        let key = e.to_loro_key();

        let sm = lifecycle_state_machine(&key);
        assert_eq!(sm.states.len(), 4);
        assert_eq!(sm.initial, "InWork");
        assert_eq!(sm.transitions.len(), 4);

        let report = validate_state_machine(&reg, &sm, |id| engine.entity_exists(id))
            .expect("the revision lifecycle validates as an M12.2 state machine");
        // No island states — every lifecycle state is reachable from InWork.
        assert!(
            report.unreachable.is_empty(),
            "every lifecycle state is reachable from InWork (got islands: {:?})",
            report.unreachable
        );
    }
}
