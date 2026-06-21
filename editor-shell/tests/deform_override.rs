//! M9.5 / G5: a **fidelity deformation is saved as a G2 override** (ADR-029). The deform's only persisted
//! state is the **sparse moved-handle targets** (NOT baked geometry — invariant 2); the surface is
//! *reproduced* deterministically by the ARAP deformer from those targets. So a deform is undoable (one
//! Ctrl-Z), reload-persistent (it journals as a `Record::Deform`, replayed through `set_part_deform`), and
//! merge-aware — it rides the exact same override pipeline as a part transform (M9.2 / ADR-026).

// Test geometry: literal grid coordinates + index/precision casts over tiny counts — intentional.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use metrocalk_core::{Engine, EntityId, Op};
use metrocalk_deform::{DeformMesh, Region};
use metrocalk_ecs::FlecsWorld;
use metrocalk_editor_shell::capscene;
use metrocalk_editor_shell::persist::Record;

fn engine(peer: u64) -> Engine<FlecsWorld> {
    Engine::new(FlecsWorld::new(), peer)
}

fn make_part(e: &mut Engine<FlecsWorld>) -> EntityId {
    let id = e.alloc_entity_id();
    e.commit("create-part", vec![Op::CreateEntity { id, parent: None }])
        .unwrap();
    id
}

/// An `n×n` grid (XY plane) with the boundary anchored and the center as handle 0 — a localized deform
/// region of interest.
fn grid_region(n: usize) -> (DeformMesh, Region) {
    let idx = |i: usize, j: usize| (j * n + i) as u32;
    let mut positions = Vec::new();
    for j in 0..n {
        for i in 0..n {
            positions.push([i as f64, j as f64, 0.0]);
        }
    }
    let mut triangles = Vec::new();
    for j in 0..n - 1 {
        for i in 0..n - 1 {
            triangles.push([idx(i, j), idx(i + 1, j), idx(i + 1, j + 1)]);
            triangles.push([idx(i, j), idx(i + 1, j + 1), idx(i, j + 1)]);
        }
    }
    let mut anchors = Vec::new();
    for j in 0..n {
        for i in 0..n {
            if i == 0 || j == 0 || i == n - 1 || j == n - 1 {
                anchors.push(idx(i, j));
            }
        }
    }
    let center = idx(n / 2, n / 2);
    (
        DeformMesh {
            positions,
            triangles,
        },
        Region {
            handles: vec![center],
            anchors,
        },
    )
}

#[test]
fn a_deform_is_saved_as_an_override_and_reproduces_the_surface() {
    let mut e = engine(1);
    let part = make_part(&mut e);
    let (mesh, region) = grid_region(7);
    let center = region.handles[0] as usize;
    let rest_c = mesh.positions[center];

    // No deform yet → the reconstructed surface is the rest mesh.
    let base = capscene::reconstruct_part_deform(&mesh, &region, &e, part).expect("reconstruct");
    assert!(
        (base[center][2] - rest_c[2]).abs() < 1e-9,
        "no deform → rest surface"
    );

    // Save a deform: lift handle 0 (the center) by +1 in z.
    let target = [rest_c[0] as f32, rest_c[1] as f32, (rest_c[2] + 1.0) as f32];
    capscene::set_part_deform(&mut e, part, &[(0, target)]).unwrap();

    // It's stored as a SPARSE per-field override (read back) — not baked geometry.
    let stored = capscene::part_deform_handles(&e, part);
    assert_eq!(stored.len(), 1, "one handle override");
    assert_eq!(stored[0].0, 0, "handle index");
    assert!((stored[0].1[2] - 1.0).abs() < 1e-5, "the stored target");

    // The surface is reproduced deterministically: the handle lands, the bump propagates, anchors hold.
    let out = capscene::reconstruct_part_deform(&mesh, &region, &e, part).expect("reconstruct");
    assert!(
        (out[center][2] - (rest_c[2] + 1.0)).abs() < 1e-4,
        "handle landed at target"
    );
    let neighbor = center - 1;
    assert!(
        out[neighbor][2] > 0.05,
        "the bump propagated to a neighbor (smooth flow)"
    );
    let again = capscene::reconstruct_part_deform(&mesh, &region, &e, part).expect("reconstruct");
    assert_eq!(
        out, again,
        "the reconstruction is deterministic (same override → same surface)"
    );
}

#[test]
fn a_deform_override_is_undoable() {
    let mut e = engine(1);
    let part = make_part(&mut e);
    capscene::set_part_deform(&mut e, part, &[(0, [2.0, 2.0, 1.0])]).unwrap();
    assert_eq!(
        capscene::part_deform_handles(&e, part).len(),
        1,
        "deform saved"
    );
    assert!(e.undo(), "the deform commit is undoable");
    assert!(
        capscene::part_deform_handles(&e, part).is_empty(),
        "Ctrl-Z reverted the deform (the surface returns to rest)"
    );
}

#[test]
fn a_deform_survives_reload_via_the_persist_log() {
    // The shell reloads by re-seeding deterministically + replaying the journal. A `Record::Deform`
    // serializes to the log and replays back through `set_part_deform`, so the deform surfaces on reopen.
    let handles = vec![(0usize, [3.0f32, 3.0, 1.2])];

    // Session A: save the deform + journal it as a Deform record.
    let mut a = engine(1);
    let part_a = make_part(&mut a);
    capscene::set_part_deform(&mut a, part_a, &handles).unwrap();
    let record = Record::Deform {
        id: part_a.to_loro_key(),
        handles: handles.clone(),
    };

    // It round-trips through the JSON log line format persist::Log writes/reads.
    let line = serde_json::to_string(&record).unwrap();
    let restored: Record = serde_json::from_str(&line).unwrap();

    // Session B: a fresh deterministically-seeded engine (same peer → same id allocation) replays it.
    let mut b = engine(1);
    let part_b = make_part(&mut b);
    assert_eq!(
        part_b, part_a,
        "deterministic id (the reload-seed guarantee, ADR-013)"
    );
    let Record::Deform {
        id,
        handles: replayed,
    } = restored
    else {
        panic!("expected a Deform record");
    };
    assert_eq!(EntityId::from_loro_key(&id), Some(part_b));
    capscene::set_part_deform(&mut b, part_b, &replayed).unwrap();

    // The deform surfaced after reload.
    let after = capscene::part_deform_handles(&b, part_b);
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].0, 0);
    assert!(
        (after[0].1[2] - 1.2).abs() < 1e-5,
        "the saved handle target persisted across reload"
    );
}
