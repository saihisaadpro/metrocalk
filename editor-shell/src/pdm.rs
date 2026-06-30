//! M15.1 (ADR-071) — the **PDM digital-thread service**: released revisions with a content-addressed,
//! tamper-evident, reproducible identity (the property Onshape lacks), and ECO branch/merge over the
//! shipped CRDT — all **kernel-free**.
//!
//! This is the editor-shell layer that ties together the pieces no single crate owns: the **data model**
//! ([`metrocalk_core::Revision`] / [`Lifecycle`]), the **content-address + signature** (the M11.5
//! `metrocalk_assets::SignedProvenanceTrust` behind the `ProvenanceVerifier` trait — invariant 5 keeps
//! `ed25519-dalek` confined to `assets/src/signed.rs`), and the **op-log branch/merge** (the
//! [`Engine`] CRDT). It builds **no new versioning machinery** — branch/merge/offline/real-time-collab are
//! Loro properties we already have; this is the CAD-domain surfacing.
//!
//! - [`release`] seals a revision: its identity is the content hash of the **canonical logical state**
//!   ([`Engine::canonical_state`]) — reproducible across a save→reload cycle and **invariant under op-log
//!   compaction** (unlike a hash of the raw snapshot bytes), so a released revision is immutable + stable.
//!   The Ed25519 signature binds that content hash + the revision's immutable metadata; [`verify`] rejects
//!   a tampered state byte (content binding), a tampered metadata field (signature), or an untrusted signer
//!   (the forgery guard — the exact M11.5 reject chain, ADR-044).
//! - [`branch_from`] / [`approval_delta`] / [`merge_eco`] are the **ECO** (engineering-change-order) flow:
//!   a branch is an op-log divergence (a fresh [`Engine`] on a distinct peer id), the approval delta is the
//!   updates since the branch point, and the merge is the algebraic CRDT merge + the inv-3 merge-validation
//!   — pull-requests for CAD, gated on a clean merge.
//!
//! **Honest boundary:** enterprise PLM *governance* (21 CFR Part 11 / ITAR / e-signatures / ERP-MES) is
//! process+legal+scale, **not** a data model — the [`Lifecycle`] state machine is the *hook*, the rest is a
//! named future. Multi-user is CRDT eventual-consistency (concurrent/async/offline-first), not an
//! authoritative lock. Native-deterministic; the web revision-hash path is server-authoritative (ADR-020).

use metrocalk_assets::{
    AssetId, Provenance, ProvenanceVerifier, SignedProvenanceTrust, TamperError,
};
use metrocalk_core::{Engine, Lifecycle, PipelineError, Revision};
use metrocalk_ecs::{FlecsWorld, World};

/// Why a release could not be sealed — a plain-language, ASCII-safe reason (ADR-016).
#[derive(Debug)]
pub enum PdmError {
    /// Release was attempted with a **verify-only** trust (no signing key) — a released revision must be
    /// cryptographically signed (provision a signing key; key provisioning is the named seam).
    NotSigningCapable,
    /// An underlying engine/op-log error.
    Pipeline(PipelineError),
}

impl std::fmt::Display for PdmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PdmError::NotSigningCapable => f.write_str(
                "cannot release a revision with a verify-only trust: a released revision must be signed \
                 (provide a signing key)",
            ),
            PdmError::Pipeline(e) => write!(f, "engine error while branching/merging: {e}"),
        }
    }
}

impl std::error::Error for PdmError {}

impl From<PipelineError> for PdmError {
    fn from(e: PipelineError) -> Self {
        PdmError::Pipeline(e)
    }
}

/// The **content-addressed identity** of an engine's current logical state — the revision's identity by
/// construction: identical logical state → identical hash, reproducible across reload + invariant under
/// op-log compaction (it hashes [`Engine::canonical_state`], not the raw snapshot). Pure, deterministic.
#[must_use]
pub fn state_identity<W: World>(engine: &Engine<W>) -> String {
    AssetId::of_bytes(engine.canonical_state().as_bytes())
        .as_str()
        .to_string()
}

/// Reconstruct the M11.5 `Provenance` record a revision's signature was sealed over — the content hash (the
/// state identity) + the metadata source (the immutable identity fields). The signed payload is its
/// `canonical_assertions()`; verifying re-hashes the live state and checks it against this.
fn sealed_provenance(rev: &Revision) -> Provenance {
    Provenance {
        source: rev.metadata_source(),
        content_hash: rev.state_hash.clone(),
        signature: rev.signature.clone(),
        signer: rev.signer.clone(),
        ..Provenance::default()
    }
}

/// **Release** the engine's current state as an immutable, signed revision (the seal-and-release action).
///
/// The revision's identity is the content hash of the canonical logical state; the Ed25519 signature
/// (reusing the M11.5 `SignedProvenanceTrust`) binds that hash + the revision's immutable metadata
/// (number/parent/author). The returned revision is `Released`, content-addressed, and tamper-evident.
///
/// # Errors
/// [`PdmError::NotSigningCapable`] if `trust` is verify-only (a released revision must be signed).
pub fn release<W: World>(
    engine: &Engine<W>,
    number: u64,
    parent: Option<String>,
    author: impl Into<String>,
    trust: &SignedProvenanceTrust,
) -> Result<Revision, PdmError> {
    if trust.public_key_hex().is_none() {
        return Err(PdmError::NotSigningCapable);
    }
    let mut rev = Revision::new(number, parent, author);
    rev.lifecycle = Lifecycle::Released;

    // Sign the canonical logical state (the M11.5 reuse, verbatim): seal stamps content_hash =
    // AssetId::of_bytes(canon_state) (the identity) and signs canonical_assertions (which binds the
    // content hash + the metadata source). canon_state is float-free of the JSON 1-ULP hazard (ADR-050).
    let canon = engine.canonical_state();
    let record = Provenance {
        source: rev.metadata_source(),
        ..Provenance::default()
    };
    let sealed = trust.seal(canon.as_bytes(), record);
    rev.state_hash = sealed.content_hash;
    rev.signature = sealed.signature;
    rev.signer = sealed.signer;
    Ok(rev)
}

/// **Verify** a released revision against the live state of `engine` (which must hold the revision's
/// reloaded state). Rejects a tampered state byte (the content no longer hashes to `state_hash`), a tampered
/// metadata field (the signature no longer covers `metadata_source`), an untrusted signer (the forgery
/// guard), or an unsigned record — the exact M11.5 reject chain (ADR-044).
///
/// # Errors
/// [`TamperError`] when the revision does not faithfully describe `engine`'s state, or its signature is
/// missing / forged / untrusted.
pub fn verify<W: World>(
    engine: &Engine<W>,
    rev: &Revision,
    trust: &SignedProvenanceTrust,
) -> Result<(), TamperError> {
    let canon = engine.canonical_state();
    trust.verify(canon.as_bytes(), &sealed_provenance(rev))
}

// ── ECO (engineering change order) — branch / approval-delta / merge ─────────────────────────────────────

/// The outcome of merging an ECO branch back into the main line — the inv-3 merge-validation result, gated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EcoOutcome {
    /// Invalid-state-class violations the merge-validation layer found (0 = a clean merge).
    pub violations: usize,
    /// Repairs the merge-validation layer applied (a non-zero count means an entity was re-keyed/pruned —
    /// surface it to the reviewer, never treat it as benign).
    pub repairs: usize,
    /// Whether the ECO is **approved**: a clean merge (no violations) — the "reviewed branch merged with an
    /// approval delta" gate.
    pub approved: bool,
}

/// Open an **ECO branch** from a released base: a fresh [`Engine`] on a **distinct peer id** seeded by
/// merging the base snapshot (a real branch is a separate actor identity — sharing a peer id corrupts CRDT
/// identity, ADR-002 F3). The branch diverges the op-log; authors `commit` on it.
///
/// # Errors
/// [`PipelineError`] if the base snapshot can't be imported (malformed bytes are an explained error, never
/// a panic).
pub fn branch_from(
    base_snapshot: &[u8],
    peer_id: u64,
) -> Result<Engine<FlecsWorld>, PipelineError> {
    let mut branch = Engine::new(FlecsWorld::new(), peer_id);
    branch.merge(base_snapshot)?;
    Ok(branch)
}

/// The **approval delta** — the op-log updates the branch added since the branch point (`base_vv` =
/// [`Engine::version_vector`] captured at branch time). This is the reviewable change set of the ECO (the
/// "approval delta merged back").
#[must_use]
pub fn approval_delta<W: World>(branch: &Engine<W>, base_vv: &[u8]) -> Vec<u8> {
    branch.export_updates_since(base_vv)
}

/// **Merge an ECO** back into the main line: apply the branch's delta, run the inv-3 merge-validation, and
/// gate approval on a clean merge. The merge is the algebraic CRDT merge (no lost edit) + the repair pass.
///
/// # Errors
/// [`PipelineError`] if the delta can't be imported.
pub fn merge_eco<W: World>(
    main: &mut Engine<W>,
    delta: &[u8],
) -> Result<EcoOutcome, PipelineError> {
    let report = main.merge(delta)?;
    let violations = report.total_violations();
    Ok(EcoOutcome {
        violations,
        repairs: report.total_repairs,
        approved: violations == 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrocalk_core::{FieldValue, Op};

    fn engine(peer: u64) -> Engine<FlecsWorld> {
        Engine::new(FlecsWorld::new(), peer)
    }

    fn signing_trust() -> SignedProvenanceTrust {
        SignedProvenanceTrust::from_secret(&[42u8; 32])
    }

    fn seed_part(e: &mut Engine<FlecsWorld>, name: &str) -> metrocalk_core::EntityId {
        let id = e.alloc_entity_id();
        e.commit(
            "seed",
            vec![
                Op::CreateEntity { id, parent: None },
                Op::SetField {
                    entity: id,
                    component: "__meta__".into(),
                    field: "name".into(),
                    value: FieldValue::Str(name.into()),
                },
            ],
        )
        .unwrap();
        id
    }

    #[test]
    fn a_released_revision_is_signed_and_content_addressed() {
        let mut e = engine(1);
        seed_part(&mut e, "bracket");
        let trust = signing_trust();
        let rev = release(&e, 1, None, "alice", &trust).unwrap();
        assert_eq!(rev.lifecycle, Lifecycle::Released);
        assert!(rev.is_signed(), "a released revision is signed");
        assert!(
            rev.state_hash.starts_with("mtkasset:"),
            "content-addressed identity"
        );
        // The faithful, untouched revision verifies.
        verify(&e, &rev, &trust).expect("an untouched released revision verifies");
    }

    #[test]
    fn a_verify_only_trust_cannot_release() {
        let e = engine(1);
        let verify_only = SignedProvenanceTrust::verifier(&[]);
        assert!(matches!(
            release(&e, 1, None, "alice", &verify_only),
            Err(PdmError::NotSigningCapable)
        ));
    }

    #[test]
    fn a_tampered_state_is_rejected() {
        let mut e = engine(1);
        let id = seed_part(&mut e, "bracket");
        let trust = signing_trust();
        let rev = release(&e, 1, None, "alice", &trust).unwrap();
        // Mutate the released state — verify against the now-different state must reject (content binding).
        e.commit(
            "tamper",
            vec![Op::SetField {
                entity: id,
                component: "__meta__".into(),
                field: "name".into(),
                value: FieldValue::Str("forged".into()),
            }],
        )
        .unwrap();
        assert!(
            verify(&e, &rev, &trust).is_err(),
            "a tampered state is rejected"
        );
    }

    #[test]
    fn a_tampered_metadata_field_is_rejected() {
        let mut e = engine(1);
        seed_part(&mut e, "bracket");
        let trust = signing_trust();
        let rev = release(&e, 1, None, "alice", &trust).unwrap();
        // Forge the author AFTER signing — the content hash is unchanged, but the signed metadata source no
        // longer matches → the signature fails (the metadata is signed, not just the bytes).
        let mut forged = rev.clone();
        forged.author = "mallory".into();
        assert!(
            verify(&e, &forged, &trust).is_err(),
            "a forged author is caught by the signature"
        );
    }

    #[test]
    fn an_untrusted_signer_is_rejected_but_the_trusted_one_verifies() {
        let mut e = engine(1);
        seed_part(&mut e, "bracket");
        let signer = signing_trust();
        let rev = release(&e, 1, None, "alice", &signer).unwrap();
        // A verifier trusting a DIFFERENT key rejects it (the forgery guard).
        let other = SignedProvenanceTrust::from_secret(&[7u8; 32]);
        let wrong = SignedProvenanceTrust::verifier(&[other.public_key_hex().unwrap()]);
        assert!(
            verify(&e, &rev, &wrong).is_err(),
            "an untrusted signer is rejected"
        );
        // A verifier trusting the signer's key accepts it.
        let right = SignedProvenanceTrust::verifier(&[signer.public_key_hex().unwrap()]);
        right
            .verify(e.canonical_state().as_bytes(), &sealed_provenance(&rev))
            .expect("a verifier trusting the signer accepts the revision");
    }
}
