//! M15.1 (ADR-071) — THE SPIKE (deliverable #1): the measured go/no-go gate for the **data-model-as-
//! digital-thread** on the shipped substrate, **kernel-free**.
//!
//! Proves the four CAD-N1 properties, each measured (≥2 runs where determinism is the claim):
//! - **(a)** two peers concurrently edit the same project → the CRDT **merges with no lost edit**
//!   (convergence + the inv-3 merge-validation).
//! - **(b)** a **BOM query stays in sync** automatically after an edit — no manual re-export.
//! - **(c)** a **released revision's content hash is immutable + reproducible + tamper-evident** — re-derive
//!   the identity from the same logical state (across a save→reload cycle) and get the same hash; a tampered
//!   byte is rejected (the M11.5 Ed25519 trust model, ADR-044).
//! - **(d)** a **branch/merge ECO** round-trips (branch → edit → review-delta → merge → converge).
//!
//! Plus the **design evidence** for hashing the *canonical logical state* rather than the raw Loro snapshot
//! (the M13.1 canonical-serialization discipline). Run with `-- --nocapture` to print the measured numbers.
//!
//! This is a headless CI gate (`cargo test --workspace`) — no dark test. Native-deterministic; the wasm
//! revision-hash path is server-authoritative (ADR-020, the standing boundary).

use metrocalk_assets::SignedProvenanceTrust;
use metrocalk_core::{Engine, FieldValue, Op};
use metrocalk_ecs::FlecsWorld;
use metrocalk_editor_shell::{
    approval_delta, bom_rollup, branch_from, merge_eco, release, state_identity, verify_revision,
};

const RUNS: usize = 3; // the ≥2-runs reproducibility discipline (<benchmark_discipline>)

fn engine(peer: u64) -> Engine<FlecsWorld> {
    Engine::new(FlecsWorld::new(), peer)
}

fn signing_trust() -> SignedProvenanceTrust {
    // A fixed seed → a reproducible signer (the key-provisioning seam is named, not built here).
    SignedProvenanceTrust::from_secret(&[15u8; 32])
}

/// A deterministic CAD-ish product: an assembly with two distinct named parts (one bracket, one bolt). No
/// RNG / no clock — the same construction every run (<benchmark_discipline> reproducible inputs).
fn seed_product(
    e: &mut Engine<FlecsWorld>,
) -> (metrocalk_core::EntityId, metrocalk_core::EntityId) {
    let asm = e.alloc_entity_id();
    let bracket = e.alloc_entity_id();
    e.commit(
        "seed-product",
        vec![
            Op::CreateEntity {
                id: asm,
                parent: None,
            },
            Op::SetField {
                entity: asm,
                component: "__meta__".into(),
                field: "name".into(),
                value: FieldValue::Str("Assembly".into()),
            },
            Op::CreateEntity {
                id: bracket,
                parent: Some(asm),
            },
            Op::SetField {
                entity: bracket,
                component: "MeshRenderer".into(),
                field: "mesh".into(),
                value: FieldValue::Str("mtkasset:bracket".into()),
            },
            Op::SetField {
                entity: bracket,
                component: "__meta__".into(),
                field: "name".into(),
                value: FieldValue::Str("bracket".into()),
            },
        ],
    )
    .unwrap();
    (asm, bracket)
}

fn add_part(
    e: &mut Engine<FlecsWorld>,
    parent: metrocalk_core::EntityId,
    handle: &str,
    name: &str,
) {
    let id = e.alloc_entity_id();
    e.commit(
        "add-part",
        vec![
            Op::CreateEntity {
                id,
                parent: Some(parent),
            },
            Op::SetField {
                entity: id,
                component: "MeshRenderer".into(),
                field: "mesh".into(),
                value: FieldValue::Str(handle.into()),
            },
            Op::SetField {
                entity: id,
                component: "__meta__".into(),
                field: "name".into(),
                value: FieldValue::Str(name.into()),
            },
        ],
    )
    .unwrap();
}

// ── Property (a): concurrent edits merge with no lost edit ───────────────────────────────────────────────

#[test]
fn property_a_concurrent_edits_merge_with_no_lost_edit() {
    let mut converged_hashes = Vec::new();
    for run in 0..RUNS {
        // A shared base; two peers fork it.
        let mut base = engine(1);
        let (asm, bracket) = seed_product(&mut base);
        let snapshot = base.snapshot();

        let mut peer_a = engine(10);
        peer_a.merge(&snapshot).unwrap();
        let mut peer_b = engine(20);
        peer_b.merge(&snapshot).unwrap();

        // Peer A renames a part; Peer B adds another part (a BOM quantity change) — the prompt's exact
        // concurrent-edit example. Different containers → no conflict.
        peer_a
            .commit(
                "rename",
                vec![Op::SetField {
                    entity: bracket,
                    component: "__meta__".into(),
                    field: "name".into(),
                    value: FieldValue::Str("main-bracket".into()),
                }],
            )
            .unwrap();
        add_part(&mut peer_b, asm, "mtkasset:bolt", "bolt");

        // Cross-merge (the algebraic CRDT merge + inv-3 validation, both directions).
        let report_a = peer_a.merge(&peer_b.export_updates()).unwrap();
        let report_b = peer_b.merge(&peer_a.export_updates()).unwrap();
        assert_eq!(report_a.total_violations(), 0, "merge A<-B is clean");
        assert_eq!(report_b.total_violations(), 0, "merge B<-A is clean");

        // No lost edit: A's rename survives AND B's added part survives, on BOTH peers.
        assert_eq!(
            peer_a.get_field(bracket, "__meta__", "name"),
            Some(FieldValue::Str("main-bracket".into())),
            "A's rename survived on A"
        );
        assert_eq!(
            peer_b.get_field(bracket, "__meta__", "name"),
            Some(FieldValue::Str("main-bracket".into())),
            "A's rename survived on B (no lost edit)"
        );
        let bom_a = bom_rollup(&peer_a);
        let bom_b = bom_rollup(&peer_b);
        assert_eq!(
            bom_a.total_instances, 2,
            "B's added part survived (BOM quantity)"
        );
        assert_eq!(bom_a, bom_b, "the BOM converges on both peers");

        // Convergence: identical canonical logical state on both peers.
        let ca = peer_a.canonical_state();
        let cb = peer_b.canonical_state();
        assert_eq!(
            ca, cb,
            "two concurrently-edited peers converge to the same logical state"
        );
        converged_hashes.push(state_identity(&peer_a));
        println!(
            "[a] run {run}: converged identity = {}",
            state_identity(&peer_a)
        );
    }
    // Determinism: the same scenario converges to the same identity every run.
    assert!(
        converged_hashes.windows(2).all(|w| w[0] == w[1]),
        "concurrent-merge convergence is deterministic across {RUNS} runs: {converged_hashes:?}"
    );
}

// ── Property (b): the BOM query stays in sync (no re-export) ──────────────────────────────────────────────

#[test]
fn property_b_bom_stays_in_sync_with_no_export_step() {
    let mut e = engine(1);
    let (asm, _bracket) = seed_product(&mut e);
    let before = bom_rollup(&e);
    assert_eq!(before.distinct_parts, 1);
    assert_eq!(before.total_instances, 1);

    // An edit — and the BOM reflects it on the very next query, with no export/import step.
    add_part(&mut e, asm, "mtkasset:bracket", "bracket-2"); // a second instance of the SAME part
    add_part(&mut e, asm, "mtkasset:gusset", "gusset"); // a new distinct part
    let after = bom_rollup(&e);
    assert_eq!(
        after.distinct_parts, 2,
        "the new distinct part appears with no re-export"
    );
    assert_eq!(after.total_instances, 3);
    let bracket = after
        .lines
        .iter()
        .find(|l| l.part == "mtkasset:bracket")
        .unwrap();
    assert_eq!(
        bracket.quantity, 2,
        "the quantity bumps live (no Excel export)"
    );
    println!(
        "[b] BOM in-sync: {} distinct parts, {} instances after the edit",
        after.distinct_parts, after.total_instances
    );
}

// ── Property (c): the released revision is reproducible + tamper-evident ──────────────────────────────────

#[test]
fn property_c_revision_hash_is_reproducible_and_tamper_evident() {
    let trust = signing_trust();

    // Build a state once and capture its released identity.
    let mut origin = engine(1);
    seed_product(&mut origin);
    let rev = release(&origin, 1, None, "alice", &trust).unwrap();
    let snapshot = origin.snapshot();
    println!("[c] released revision identity = {}", rev.state_hash);

    // REPRODUCIBLE: re-derive the identity from the SAME logical state, reloaded into a fresh engine on a
    // DIFFERENT peer, ≥2 runs — every reload yields the same content hash (immutable + reproducible).
    let mut reloaded_ids = Vec::new();
    for run in 0..RUNS {
        let mut reloaded = engine(100 + run as u64);
        reloaded.merge(&snapshot).unwrap();
        let id = state_identity(&reloaded);
        reloaded_ids.push(id.clone());
        // The faithful reload verifies against the signature (the content binding + signer trust hold).
        verify_revision(&reloaded, &rev, &trust)
            .expect("a faithful reload of a released revision verifies");
        println!("[c] run {run}: reloaded identity = {id}");
    }
    assert!(
        reloaded_ids.iter().all(|h| *h == rev.state_hash),
        "a released revision's identity is reproducible across reload ({RUNS} runs): {reloaded_ids:?} vs {}",
        rev.state_hash
    );

    // TAMPER-EVIDENT: mutate the reloaded state — verify must reject (the content no longer hashes to the
    // signed identity).
    let mut tampered = engine(200);
    tampered.merge(&snapshot).unwrap();
    let ids = tampered.entity_ids();
    let any = ids.into_iter().next().unwrap();
    tampered
        .commit(
            "tamper",
            vec![Op::SetField {
                entity: any,
                component: "__meta__".into(),
                field: "name".into(),
                value: FieldValue::Str("forged".into()),
            }],
        )
        .unwrap();
    assert!(
        verify_revision(&tampered, &rev, &trust).is_err(),
        "a tampered released state is rejected (tamper-evident)"
    );

    // FORGERY GUARD: a verifier trusting a different key rejects even a faithful revision.
    let mut faithful = engine(300);
    faithful.merge(&snapshot).unwrap();
    let attacker = SignedProvenanceTrust::from_secret(&[99u8; 32]);
    let untrusting = SignedProvenanceTrust::verifier(&[attacker.public_key_hex().unwrap()]);
    assert!(
        verify_revision(&faithful, &rev, &untrusting).is_err(),
        "an untrusted signer is rejected (the forgery guard — the property Onshape lacks)"
    );
}

// ── Property (d): a branch/merge ECO round-trips ─────────────────────────────────────────────────────────

#[test]
fn property_d_branch_merge_eco_round_trips() {
    for run in 0..RUNS {
        // The released main line.
        let mut main = engine(1);
        let (asm, _) = seed_product(&mut main);
        let base_vv = main.version_vector();
        let base_snapshot = main.snapshot();

        // Open an ECO branch (a distinct peer id), edit it (add a part — the engineering change).
        let mut branch = branch_from(&base_snapshot, 2).unwrap();
        add_part(&mut branch, asm, "mtkasset:washer", "washer");

        // The approval delta = the branch's change since the branch point.
        let delta = approval_delta(&branch, &base_vv);
        assert!(
            !delta.is_empty(),
            "the ECO carries a non-empty approval delta"
        );

        // Merge the reviewed branch back — gated on a clean merge (no violations).
        let outcome = merge_eco(&mut main, &delta).unwrap();
        assert!(
            outcome.approved,
            "a clean ECO merge is approved (run {run}): {outcome:?}"
        );
        assert_eq!(outcome.violations, 0);

        // Round-trip: main now holds the change and converges with the branch.
        assert_eq!(
            main.canonical_state(),
            branch.canonical_state(),
            "main converges with the merged branch (the ECO round-trips)"
        );
        assert_eq!(
            bom_rollup(&main).total_instances,
            2,
            "the ECO's added part is in main's BOM"
        );
        println!(
            "[d] run {run}: ECO merged, approved={}, delta {} bytes",
            outcome.approved,
            delta.len()
        );
    }
}

// ── Design evidence: hash the canonical logical state, not the raw snapshot (the M13.1 discipline) ────────

#[test]
fn design_evidence_canonical_state_is_the_stable_identity_basis() {
    // Build a state, snapshot it, reload it into a fresh engine, and compare two candidate identity bases:
    //   (1) canonical_state() — the logical deep value (the chosen basis), and
    //   (2) the raw snapshot bytes re-exported after reload.
    let mut origin = engine(1);
    seed_product(&mut origin);
    let canon_origin = origin.canonical_state();
    let snap_origin = origin.snapshot();

    let mut reloaded = engine(2);
    reloaded.merge(&snap_origin).unwrap();
    let canon_reloaded = reloaded.canonical_state();
    let snap_reloaded = reloaded.snapshot();

    // The canonical logical state is identical across the reload (the chosen, reproducible basis).
    assert_eq!(
        canon_origin, canon_reloaded,
        "canonical_state is reproducible across a save->reload cycle (the chosen identity basis)"
    );
    let snapshot_stable = snap_origin == snap_reloaded;
    println!(
        "[design] canonical_state stable across reload = {} ; raw-snapshot-bytes stable across reload = {}",
        canon_origin == canon_reloaded,
        snapshot_stable
    );
    println!(
        "[design] -> the revision identity hashes canonical_state ({} bytes), not the raw snapshot ({} bytes), \
         so it is invariant under op-log compaction + reload",
        canon_origin.len(),
        snap_origin.len()
    );
    // We do NOT assert snapshot-byte stability either way — the point is that canonical_state is the
    // robust basis regardless (it is invariant under op-log compaction, which a snapshot-byte hash is not).
}
