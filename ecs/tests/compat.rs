//! The product's compatibility query, expressed and run entirely through the public [`World`] trait
//! (no `flecs_ecs` in sight), cross-checked against the spike's numbers.

use metrocalk_ecs::scene::{build_scene, compat_clauses, SceneParams};
use metrocalk_ecs::{FlecsWorld, Target, World};

#[test]
fn compat_query_matches_spike() {
    let mut w = FlecsWorld::new();
    let s = build_scene(&mut w, &SceneParams::preset_5k(), false);
    let q = w.build_query(&compat_clauses(&s));

    let got = w.matches(&q).len();
    // (1) the wrapper agrees with the independently-tracked ground truth, and
    assert_eq!(
        got, s.expected_compat,
        "wrapper query vs independently-tracked truth"
    );
    // (2) that truth equals the spike's number — same seed, same scene, through the wrapper.
    assert_eq!(got, 211, "spike ② compat-match parity @5k");
    assert_eq!(
        s.edge_count, 1999,
        "spike ② edge-count parity (2000 minus self-loops)"
    );
}

#[test]
fn edges_and_targets_agree() {
    let mut w = FlecsWorld::new();
    let s = build_scene(&mut w, &SceneParams::preset_5k(), false);

    // for_each_edge visits exactly the binding edges.
    let mut via_edges = 0usize;
    w.for_each_edge(s.binds_to, &mut |_src, _tgt| via_edges += 1);
    assert_eq!(via_edges, s.edge_count);

    // read-target, summed over all entities, must equal the same edge count.
    let via_targets: usize = s
        .entities
        .iter()
        .map(|&e| w.targets(e, s.binds_to).len())
        .sum();
    assert_eq!(via_targets, s.edge_count);
}

// Cross-OS digest pins — FNV-1a over generation decisions is platform-stable, so CI (ubuntu)
// re-checking these exact values proves byte-identical scenes across OS. (Filled from a measured run.)
const DIGEST_5K: u64 = 11_506_122_691_080_605_209;
const DIGEST_20K: u64 = 1_779_473_442_952_625_601;

#[test]
fn scene_is_deterministic_and_storage_independent() {
    let digest = |sparse: bool| {
        let mut w = FlecsWorld::new();
        build_scene(&mut w, &SceneParams::preset_5k(), sparse).digest
    };
    let d1 = digest(false);
    assert_eq!(
        d1,
        digest(false),
        "same seed+params ⇒ identical digest across runs"
    );
    assert_eq!(
        d1,
        digest(true),
        "the sparse storage variant is the same logical scene"
    );

    let mut w = FlecsWorld::new();
    let s20 = build_scene(&mut w, &SceneParams::preset_20k(), false);
    assert_eq!(s20.expected_compat, 830, "spike ② @20k compat parity");

    assert_eq!(d1, DIGEST_5K, "5k digest pin (cross-OS)");
    assert_eq!(s20.digest, DIGEST_20K, "20k digest pin (cross-OS)");
}

#[test]
fn dontfragment_breaks_per_entity_target_reads() {
    // The decisive F1 finding (why M1 keeps DENSE storage — see progress/M1.md): under DontFragment
    // (sparse) the compat query and for_each_edge (a wildcard query) still work, but per-entity
    // targets() — Flecs each_target/target_for — does NOT ("target_for does not yet work for
    // DontFragment components"). targets() powers the inspector's "what does this entity bind to?",
    // so adopting sparse now would silently break that later.
    let mut w = FlecsWorld::new();
    let s = build_scene(&mut w, &SceneParams::preset_5k(), true);

    // for_each_edge (wildcard query) still sees every edge under sparse.
    let mut via_edges = 0usize;
    w.for_each_edge(s.binds_to, &mut |_src, _tgt| via_edges += 1);
    assert_eq!(via_edges, s.edge_count, "for_each_edge works under sparse");

    // but per-entity targets() is incomplete under sparse — the limitation that keeps M1 dense.
    let via_targets: usize = s
        .entities
        .iter()
        .map(|&e| w.targets(e, s.binds_to).len())
        .sum();
    assert_ne!(
        via_targets, s.edge_count,
        "targets() is known-broken under DontFragment (F1 verdict: stay dense)"
    );
}

#[test]
fn deferred_mutation_applies() {
    let mut w = FlecsWorld::new();
    let (a, rel, target, tag) = (
        w.create_entity(),
        w.create_entity(),
        w.create_entity(),
        w.create_entity(),
    );

    w.defer(&mut |w| {
        w.add_pair(a, rel, target);
        w.add_tag(a, tag);
    });

    assert!(w.has_pair(a, rel, Target::Exact(target)));
    assert!(w.has_pair(a, rel, Target::Any));
    assert!(w.has_tag(a, tag));
    assert_eq!(w.targets(a, rel), vec![target]);

    w.remove_pair(a, rel, target);
    assert!(!w.has_pair(a, rel, Target::Any));
}
