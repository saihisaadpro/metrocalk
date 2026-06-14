//! Protocol-level tests for `/transport` — no Loro (payloads are arbitrary bytes). Covers the
//! envelope codec, fragmentation/reassembly (in-order + out-of-order), outbox-collapse backpressure,
//! Wait-for-Ack marker clearing, the reserved `%EPH` kind, ping/pong, and the in-process loopback.

use std::sync::{Arc, Mutex};

use metrocalk_transport::frame::{self, FrameKind};
use metrocalk_transport::impls::{in_process_pair, ChannelTransport, TransportError};
use metrocalk_transport::{
    ApplyError, ApplyOutcome, DeltaSession, DeltaSink, DeltaSource, InstantAck,
};

// ── test doubles ────────────────────────────────────────────────────────────

/// A source whose "version" is a commit counter; `updates_since` yields a blob iff there is anything
/// newer than the peer's acked version — exactly the real engine's contract, minus Loro.
struct FakeSource {
    counter: u64,
    blob: Vec<u8>,
}
impl DeltaSource for FakeSource {
    fn version(&self) -> Vec<u8> {
        self.counter.to_le_bytes().to_vec()
    }
    fn updates_since(&self, vv: &[u8]) -> Vec<u8> {
        let other = if vv.len() == 8 {
            u64::from_le_bytes(vv.try_into().unwrap())
        } else {
            0
        };
        if self.counter > other {
            self.blob.clone()
        } else {
            Vec::new()
        }
    }
}

#[derive(Default)]
struct RecordingSink {
    applied: Vec<Vec<u8>>,
}
impl DeltaSink for RecordingSink {
    fn apply(&mut self, update: &[u8]) -> Result<ApplyOutcome, ApplyError> {
        self.applied.push(update.to_vec());
        Ok(ApplyOutcome::default())
    }
    fn version(&self) -> Vec<u8> {
        Vec::new()
    }
}

type Recorder = Arc<Mutex<Vec<Vec<u8>>>>;

/// A `ChannelTransport` whose outbound sink records every frame it's asked to send.
fn recording_channel() -> (ChannelTransport, Recorder) {
    let rec: Recorder = Arc::new(Mutex::new(Vec::new()));
    let r2 = rec.clone();
    let ch = ChannelTransport::new(Box::new(move |b: &[u8]| {
        r2.lock().unwrap().push(b.to_vec());
        Ok::<(), TransportError>(())
    }));
    (ch, rec)
}

fn kinds(rec: &Recorder) -> Vec<FrameKind> {
    rec.lock()
        .unwrap()
        .iter()
        .map(|f| frame::decode(f).unwrap().kind)
        .collect()
}

// ── Channel-path micro-bench: envelope encode+decode of a coalesced 60 Hz delta ────────────────
// The end-to-end Tauri Channel number (M2.1's ~3.4 ms over WebView2) is re-confirmed at M2.6 when
// the real shell is wired; here we bench the part the Channel path adds in /transport — enveloping
// and parsing a frame — to show it's negligible against the 16.6 ms frame budget. Run twice:
// `cargo test -p metrocalk-transport channel_envelope -- --nocapture`.
#[test]
fn channel_envelope_throughput() {
    use std::time::Instant;
    // A representative coalesced 60 Hz delta (a handful of small component edits ≈ 256 B).
    let payload = vec![0x5Au8; 256];
    let mut times = Vec::with_capacity(5000);
    for i in 0..5000u64 {
        let start = Instant::now();
        let frame = frame::encode_doc_update(1, i, &[&payload]);
        let h = frame::decode(&frame).unwrap();
        let du = frame::decode_doc_update(h.payload).unwrap();
        std::hint::black_box(&du);
        times.push(start.elapsed().as_secs_f64() * 1_000_000.0); // microseconds
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = times[times.len() / 2];
    let p99 = times[times.len() * 99 / 100];
    eprintln!(
        "Channel envelope encode+decode (256 B delta): p50={p50:.3} us p99={p99:.3} us (n=5000)"
    );
}

// ── envelope codec ────────────────────────────────────────────────────────────

#[test]
fn doc_update_round_trips() {
    let blobs: Vec<&[u8]> = vec![b"alpha", b"bravo", b"charlie"];
    let f = frame::encode_doc_update(42, 0xDEAD_BEEF, &blobs);
    let h = frame::decode(&f).unwrap();
    assert_eq!(h.kind, FrameKind::DocUpdate);
    assert_eq!(h.room_id, 42);
    let du = frame::decode_doc_update(h.payload).unwrap();
    assert_eq!(du.batch_id, 0xDEAD_BEEF);
    assert_eq!(
        du.blobs,
        vec![b"alpha".to_vec(), b"bravo".to_vec(), b"charlie".to_vec()]
    );
}

#[test]
fn all_kinds_and_errors() {
    assert_eq!(
        frame::decode(&frame::encode_ack(1, 9)).unwrap().kind,
        FrameKind::Ack
    );
    assert_eq!(
        frame::decode(&frame::encode_handshake(1, 7, b"vv"))
            .unwrap()
            .kind,
        FrameKind::Handshake
    );
    assert_eq!(
        frame::decode(&frame::encode_ephemeral(1, b"x"))
            .unwrap()
            .kind,
        FrameKind::Ephemeral
    );
    assert_eq!(
        frame::decode(&frame::encode_ping(1)).unwrap().kind,
        FrameKind::Ping
    );
    assert_eq!(
        frame::decode(&frame::encode_pong(1)).unwrap().kind,
        FrameKind::Pong
    );
    let hs = frame::decode_handshake(
        frame::decode(&frame::encode_handshake(1, 7, b"vv"))
            .unwrap()
            .payload,
    )
    .unwrap();
    assert_eq!(hs.peer_id, 7);
    assert_eq!(hs.known_vv, b"vv");

    assert!(matches!(
        frame::decode(b"short"),
        Err(frame::FrameError::TooShort)
    ));
    let mut bad = frame::encode_ping(1);
    bad[0] = b'Z'; // corrupt magic
    assert!(matches!(
        frame::decode(&bad),
        Err(frame::FrameError::BadMagic(_))
    ));
    let mut wrongver = frame::encode_ping(1);
    wrongver[4] = 9; // proto_version
    assert!(matches!(
        frame::decode(&wrongver),
        Err(frame::FrameError::VersionMismatch { .. })
    ));
}

// ── fragmentation / reassembly (through the public session API) ─────────────────

fn big_source() -> FakeSource {
    // 300 KiB > the 256 KiB fragment threshold ⇒ the initial-catch-up case.
    FakeSource {
        counter: 1,
        blob: vec![0xABu8; 300 * 1024],
    }
}

#[test]
fn large_update_fragments_and_reassembles() {
    let (tx, sent) = recording_channel();
    let mut sender = DeltaSession::new(tx, 1, 100, Box::new(InstantAck));
    sender.tick(&big_source()).unwrap();

    let frames = sent.lock().unwrap().clone();
    assert!(
        frames.len() > 1,
        "300 KiB update must split into multiple fragments"
    );
    assert!(frames
        .iter()
        .all(|f| frame::decode(f).unwrap().kind == FrameKind::Fragment));

    // Deliver the fragments to a receiver — IN ORDER — and confirm the blob reassembles.
    let (rx, _rx_sent) = recording_channel();
    let mut receiver = DeltaSession::new(rx, 1, 200, Box::new(InstantAck));
    let mut sink = RecordingSink::default();
    for f in &frames {
        receiver.transport_mut().deliver(f);
    }
    receiver.pump(&mut sink, 0).unwrap();
    assert_eq!(sink.applied.len(), 1);
    assert_eq!(sink.applied[0].len(), 300 * 1024);
}

#[test]
fn fragments_reassemble_out_of_order() {
    let (tx, sent) = recording_channel();
    let mut sender = DeltaSession::new(tx, 1, 100, Box::new(InstantAck));
    sender.tick(&big_source()).unwrap();
    let mut frames = sent.lock().unwrap().clone();
    frames.reverse(); // deliver last fragment first

    let (rx, _) = recording_channel();
    let mut receiver = DeltaSession::new(rx, 1, 200, Box::new(InstantAck));
    let mut sink = RecordingSink::default();
    for f in &frames {
        receiver.transport_mut().deliver(f);
    }
    receiver.pump(&mut sink, 0).unwrap();
    assert_eq!(
        sink.applied.len(),
        1,
        "reassembly must not depend on fragment arrival order"
    );
    assert_eq!(sink.applied[0].len(), 300 * 1024);
}

#[test]
fn fragment_timeout_sweeps_incomplete() {
    let (rx, _) = recording_channel();
    let mut receiver = DeltaSession::new(rx, 1, 200, Box::new(InstantAck));
    let mut sink = RecordingSink::default();

    // Deliver only the FIRST fragment of a multi-part message, at t=0.
    let (tx, sent) = recording_channel();
    let mut sender = DeltaSession::new(tx, 1, 100, Box::new(InstantAck));
    sender.tick(&big_source()).unwrap();
    let frames = sent.lock().unwrap().clone();
    receiver.transport_mut().deliver(&frames[0]);
    receiver.pump(&mut sink, 0).unwrap();
    assert!(sink.applied.is_empty(), "partial message must not deliver");

    // A later sweep past the timeout drops the dangling reassembly.
    let dropped = receiver.sweep_fragments(5_000, 1_000);
    assert_eq!(dropped.len(), 1, "stale partial reassembly should be swept");
}

// ── backpressure: outbox-collapse (one update spanning the gap, not N queued) ───

#[test]
fn backpressure_collapses_to_one_update() {
    let (tx, sent) = recording_channel();
    let mut sender = DeltaSession::new(tx, 7, 100, Box::new(InstantAck));
    let mut src = FakeSource {
        counter: 0,
        blob: vec![1, 2, 3, 4],
    };

    // First commit + tick → one DocUpdate is now in flight (unacked).
    src.counter = 1;
    sender.tick(&src).unwrap();
    assert_eq!(sender.stats().doc_updates_sent, 1);
    assert!(sender.is_in_flight());
    assert_eq!(sender.unacked_count(), 1);

    // The sink is stalled (no ACK). 5 more commits + 5 ticks must NOT queue 5 frames.
    for c in 2..=6 {
        src.counter = c;
        sender.tick(&src).unwrap();
    }
    assert_eq!(
        sender.stats().doc_updates_sent,
        1,
        "stalled sink must not queue N frames"
    );

    // ACK the in-flight batch → the gap (commits 2..=6) coalesces into ONE spanning update.
    let mut nul = RecordingSink::default();
    sender.transport_mut().deliver(&frame::encode_ack(7, 1));
    sender.pump(&mut nul, 0).unwrap();
    assert!(!sender.is_in_flight());
    assert_eq!(
        sender.unacked_count(),
        0,
        "ACK clears the unacknowledged marker"
    );

    sender.tick(&src).unwrap();
    assert_eq!(
        sender.stats().doc_updates_sent,
        2,
        "exactly one more update spans the whole gap"
    );

    // The two DocUpdates sent were the only non-trivial outbound frames.
    assert_eq!(
        kinds(&sent)
            .iter()
            .filter(|k| **k == FrameKind::DocUpdate)
            .count(),
        2
    );
}

// ── reserved %EPH kind: round-trips, ignored by the M2 session (no ack, no apply) ──

#[test]
fn ephemeral_is_reserved_and_ignored() {
    let (rx, sent) = recording_channel();
    let mut session = DeltaSession::new(rx, 1, 200, Box::new(InstantAck));
    let mut sink = RecordingSink::default();
    session
        .transport_mut()
        .deliver(&frame::encode_ephemeral(1, b"cursor@42"));
    session.pump(&mut sink, 0).unwrap();
    assert!(
        sink.applied.is_empty(),
        "%EPH carries no document delta in M2"
    );
    assert!(sent.lock().unwrap().is_empty(), "%EPH must not be acked");
}

#[test]
fn ping_is_answered_with_pong() {
    let (rx, sent) = recording_channel();
    let mut session = DeltaSession::new(rx, 1, 200, Box::new(InstantAck));
    let mut sink = RecordingSink::default();
    session.transport_mut().deliver(&frame::encode_ping(1));
    session.pump(&mut sink, 0).unwrap();
    assert_eq!(kinds(&sent), vec![FrameKind::Pong]);
}

// ── in-process loopback: full send → apply → ack cycle through the real transport ──

#[test]
fn in_process_send_apply_ack() {
    let (a, b) = in_process_pair();
    let mut alice = DeltaSession::new(a, 1, 100, Box::new(InstantAck));
    let mut bob = DeltaSession::new(b, 1, 200, Box::new(InstantAck));
    let mut bob_sink = RecordingSink::default();
    let mut alice_sink = RecordingSink::default();

    // Alice sends a delta; it lands in Bob's inbox synchronously (in-process).
    alice
        .tick(&FakeSource {
            counter: 1,
            blob: vec![9, 9, 9],
        })
        .unwrap();
    assert!(alice.is_in_flight());

    // Bob applies it and acks; the ack lands in Alice's inbox.
    bob.pump(&mut bob_sink, 0).unwrap();
    assert_eq!(bob_sink.applied, vec![vec![9, 9, 9]]);

    // Alice processes the ack → in-flight clears.
    alice.pump(&mut alice_sink, 0).unwrap();
    assert!(!alice.is_in_flight());
    assert_eq!(alice.unacked_count(), 0);
}

// ── local WebSocket (native, `ws` feature): real round-trip + latency print ────────

#[cfg(feature = "ws")]
#[test]
fn websocket_loopback_roundtrip_and_latency() {
    use metrocalk_transport::impls::{WebSocketTransport, WsServer};
    use metrocalk_transport::DeltaTransport;
    use std::time::Instant;

    let server = WsServer::spawn_echo().unwrap();
    let ws = WebSocketTransport::connect(&server.addr).expect("ws connect");
    let mut session = DeltaSession::new(ws, 1, 100, Box::new(InstantAck));
    let mut sink = RecordingSink::default();

    // One round-trip correctness check: send a delta, the echo bounces it back, we apply + ack.
    session
        .tick(&FakeSource {
            counter: 1,
            blob: vec![7; 1024],
        })
        .unwrap();
    let mut got = false;
    for _ in 0..500 {
        session.pump(&mut sink, 0).unwrap();
        if !sink.applied.is_empty() {
            got = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(got, "WebSocket echo did not round-trip a delta");
    assert_eq!(sink.applied[0].len(), 1024);

    // Latency: measure N small-frame round-trips (send → echo → recv) for a real WS number.
    let mut lat = Vec::new();
    for i in 0..200u64 {
        let frame = frame::encode_doc_update(1, 10_000 + i, &[&[1u8; 256]]);
        let start = Instant::now();
        session.transport_mut().send(&frame).unwrap();
        let mut tmp = RecordingSink::default();
        loop {
            session.pump(&mut tmp, 0).unwrap();
            if !tmp.applied.is_empty() {
                break;
            }
            if start.elapsed().as_millis() > 200 {
                break;
            }
        }
        lat.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    lat.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = lat[lat.len() / 2];
    let p99 = lat[lat.len() * 99 / 100];
    eprintln!(
        "WS loopback round-trip: p50={p50:.3} ms p99={p99:.3} ms (n={})",
        lat.len()
    );
}
