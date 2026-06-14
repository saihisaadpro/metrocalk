//! End-to-end transport tests with two real Loro-backed engines: the producer hook over the
//! in-process transport, out-of-order convergence, idempotent re-import, reconnect resync, and the
//! optimistic-echo no-op. These exercise the `/core` ↔ `/transport` seam (the Loro boundary) that the
//! protocol-level tests in `/transport` deliberately stub out.

use metrocalk_core::{Engine, Op, ProducerHook};
use metrocalk_ecs::FlecsWorld;
use metrocalk_transport::impls::in_process_pair;
use metrocalk_transport::DeltaSink;

const ROOM: u32 = 1;

fn engine(peer: u64) -> Engine<FlecsWorld> {
    Engine::new(FlecsWorld::new(), peer)
}

/// Create `n` root entities, each its own transaction (so there are several commits to coalesce).
fn create_entities(e: &mut Engine<FlecsWorld>, n: usize) {
    for _ in 0..n {
        let id = e.alloc_entity_id();
        e.commit("create", vec![Op::CreateEntity { id, parent: None }])
            .unwrap();
    }
}

// ── producer hook over the in-process transport: end-to-end convergence ─────────

#[test]
fn producer_hook_syncs_two_engines() {
    let (ta, tb) = in_process_pair();
    let mut producer = engine(1);
    let mut consumer = engine(2);
    create_entities(&mut producer, 5);

    let mut ph = ProducerHook::attach(&producer, ta, ROOM, &[]).unwrap();
    let mut ch = ProducerHook::attach(&consumer, tb, ROOM, &consumer.version_vector()).unwrap();

    // Re-handshake now that BOTH ends are listening: `in_process_pair` installs each callback only
    // when its session is created, so the attach-time handshakes raced (real Tauri/WS transports
    // connect before attach, so they don't). This establishes peer identity both directions.
    ph.session_mut().send_handshake(&[]).unwrap();
    ch.session_mut()
        .send_handshake(&consumer.version_vector())
        .unwrap();
    ph.pump(&mut producer).unwrap();
    ch.pump(&mut consumer).unwrap();

    // One coalesced delta carries all 5 creates; consumer applies + acks; producer clears in-flight.
    ph.on_tick(&producer).unwrap();
    ch.pump(&mut consumer).unwrap();
    ph.pump(&mut producer).unwrap();

    assert_eq!(consumer.entity_count(), producer.entity_count());
    assert_eq!(consumer.entity_count(), 5);
    assert!(
        !ph.session().is_in_flight(),
        "ack cleared the in-flight DocUpdate"
    );
    assert_eq!(
        ph.session().stats().doc_updates_sent,
        1,
        "5 commits coalesced into one frame"
    );

    // Peer identity established in the handshake (ADR-002 F3): each end learned the other's PeerID.
    assert_eq!(ch.session().remote_peer_id(), Some(1));
    assert_eq!(ph.session().remote_peer_id(), Some(2));
}

// ── out-of-order import: apply update2 before its causal dependency update1 ─────

#[test]
fn out_of_order_imports_converge() {
    let mut p = engine(1);
    let e1 = p.alloc_entity_id();
    p.commit(
        "t1",
        vec![Op::CreateEntity {
            id: e1,
            parent: None,
        }],
    )
    .unwrap();
    let update1 = p.export_updates(); // tx1, from empty
    let v1 = p.version_vector();

    let e2 = p.alloc_entity_id();
    p.commit(
        "t2",
        vec![Op::CreateEntity {
            id: e2,
            parent: Some(e1),
        }],
    )
    .unwrap();
    let update2 = p.export_updates_since(&v1); // tx2 only — causally depends on tx1

    let mut c = engine(2);
    // Deliver tx2 FIRST: Loro buffers it as pending (its dependency hasn't arrived).
    c.merge(&update2).unwrap();
    assert_eq!(
        c.entity_count(),
        0,
        "tx2 alone is pending — nothing applied yet"
    );
    // Now tx1 arrives → both apply and the doc converges.
    c.merge(&update1).unwrap();
    assert_eq!(c.entity_count(), 2, "convergence once the dependency lands");
    assert_eq!(c.entity_count(), p.entity_count());
}

// ── idempotent re-import: applying the same update twice is a no-op ─────────────

#[test]
fn idempotent_reimport() {
    let mut p = engine(1);
    create_entities(&mut p, 3);
    let update = p.export_updates();

    let mut c = engine(2);
    c.merge(&update).unwrap();
    let after_first = c.entity_count();
    c.merge(&update).unwrap(); // re-import the identical bytes
    assert_eq!(
        c.entity_count(),
        after_first,
        "re-importing the same update changes nothing"
    );
    assert_eq!(after_first, 3);
}

// ── reconnect resync: a stale version vector pulls exactly the missing gap, in one update ───

#[test]
fn reconnect_resyncs_from_stale_version() {
    let mut p = engine(1);
    let mut c = engine(2);
    create_entities(&mut p, 2);
    c.merge(&p.export_updates()).unwrap();
    assert_eq!(c.entity_count(), 2);

    // Producer races ahead while the consumer is "disconnected".
    create_entities(&mut p, 4);

    // On reconnect the consumer's handshake carries its (now stale) version vector; the producer
    // exports everything since — ONE update spanning the gap (this is what `updates_since` does).
    let gap = p.export_updates_since(&c.version_vector());
    c.merge(&gap).unwrap();
    assert_eq!(c.entity_count(), p.entity_count());
    assert_eq!(c.entity_count(), 6);
}

// ── optimistic-echo: a clean apply reports no correction (repaired == false) ────

#[test]
fn echo_apply_is_a_clean_no_op() {
    let mut p = engine(1);
    create_entities(&mut p, 2);
    let update = p.export_updates();

    let mut c = engine(2);
    // `DeltaSink::apply` surfaces the optimistic-echo signal: a clean apply did not repair anything.
    let outcome = c.apply(&update).unwrap();
    assert!(
        !outcome.repaired,
        "clean apply ⇒ predicted == authoritative (no correction)"
    );
    // (The correction path — repaired == true — is driven by merge-validation; its detect/repair of
    // all 8 invalid-state classes is covered by core/tests/merge.rs. The flag threads through here.)
}
