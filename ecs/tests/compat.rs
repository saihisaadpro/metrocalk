//! The product's compatibility query, expressed and run entirely through the public [`World`] trait
//! (no `flecs_ecs` in sight), cross-checked against the spike's numbers.

use metrocalk_ecs::scene::{build_compat_scene, CompatScene};
use metrocalk_ecs::{Clause, FlecsWorld, Target, Term, World};

fn compat_clauses(s: &CompatScene) -> Vec<Clause> {
    vec![
        Clause::with(Term::Pair {
            rel: s.provides,
            target: Target::Exact(s.health),
        }),
        Clause::without(Term::Pair {
            rel: s.binds_to,
            target: Target::Any,
        }),
    ]
}

#[test]
fn compat_query_matches_spike() {
    let mut w = FlecsWorld::new();
    let s = build_compat_scene(&mut w, 5000, 2000);
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
    let s = build_compat_scene(&mut w, 5000, 2000);

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
