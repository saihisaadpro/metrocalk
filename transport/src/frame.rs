//! Wire framing — the **Loro Syncing Protocol v1** envelope, ported into `/transport` so this crate
//! links no Loro (payloads are opaque `&[u8]`; the CI grep stays green).
//!
//! Every frame is: a fixed 16-byte header (magic `frame_kind` · `proto_version` · flags · room id ·
//! payload length) followed by a kind-specific payload. The magic *is* the frame kind (ASCII, so a
//! hex dump is readable). Carrying Loro's own `export(update)` bytes as the opaque `DocUpdate`
//! payload is what gives us version-vector causality (ordering / idempotency / out-of-order
//! tolerance) for free — see ADR / README. The envelope itself never inspects the payload.

// Wire length/count fields are u32 by the protocol spec; the casts from usize are bounded by the
// frame size (a single frame can't exceed 4 GiB — fragments cap chunks at 256 KiB), so truncation
// is impossible in practice.
#![allow(clippy::cast_possible_truncation)]

/// Protocol version carried in every header. Bump only on an incompatible envelope change.
pub const PROTO_VERSION: u8 = 1;

/// Header is fixed-width so a reader can slice the payload without a varint pass.
pub const HEADER_LEN: usize = 16;

/// Payloads larger than this are split into [`FrameKind::Fragment`]s (the initial
/// catch-up/snapshot is the case that needs it). 256 KiB matches the protocol's fragment trigger.
pub const FRAGMENT_THRESHOLD: usize = 256 * 1024;

/// Frame kinds. The 4 magic bytes are the kind tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    /// Document delta: payload is a [`DocUpdate`] batch of Loro `update` blobs. (`%LOR`)
    DocUpdate,
    /// Acknowledgement: payload is the 8-byte batch id being acked. (`%ACK`)
    Ack,
    /// Handshake: peer id + the sender's known version vector. (`%HSK`)
    Handshake,
    /// Fragment of an oversized inner frame. (`%FRG`)
    Fragment,
    /// Ephemeral (cursors/presence) — **reserved for Phase-2 collab**, unused in M2 but a
    /// first-class kind now so the wire never needs a breaking change to add it. (`%EPH`)
    Ephemeral,
    /// App-level keepalive ping. (`%PNG`)
    Ping,
    /// App-level keepalive pong. (`%PNT`)
    Pong,
}

impl FrameKind {
    const fn magic(self) -> [u8; 4] {
        match self {
            FrameKind::DocUpdate => *b"%LOR",
            FrameKind::Ack => *b"%ACK",
            FrameKind::Handshake => *b"%HSK",
            FrameKind::Fragment => *b"%FRG",
            FrameKind::Ephemeral => *b"%EPH",
            FrameKind::Ping => *b"%PNG",
            FrameKind::Pong => *b"%PNT",
        }
    }

    fn from_magic(m: &[u8]) -> Option<Self> {
        Some(match m {
            b"%LOR" => FrameKind::DocUpdate,
            b"%ACK" => FrameKind::Ack,
            b"%HSK" => FrameKind::Handshake,
            b"%FRG" => FrameKind::Fragment,
            b"%EPH" => FrameKind::Ephemeral,
            b"%PNG" => FrameKind::Ping,
            b"%PNT" => FrameKind::Pong,
            _ => return None,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    TooShort,
    BadMagic([u8; 4]),
    VersionMismatch { got: u8, want: u8 },
    LengthMismatch { declared: usize, actual: usize },
    Malformed(&'static str),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::TooShort => write!(f, "frame shorter than the 16-byte header"),
            FrameError::BadMagic(m) => write!(f, "unknown frame magic {m:?}"),
            FrameError::VersionMismatch { got, want } => {
                write!(f, "proto_version {got} != supported {want}")
            }
            FrameError::LengthMismatch { declared, actual } => {
                write!(f, "payload len {declared} != actual {actual}")
            }
            FrameError::Malformed(s) => write!(f, "malformed payload: {s}"),
        }
    }
}

impl std::error::Error for FrameError {}

// ── header ──────────────────────────────────────────────────────────────────

/// Encode a header + payload into one frame. Internal — callers use the typed `encode_*` helpers.
fn encode(kind: FrameKind, room_id: u32, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(HEADER_LEN + payload.len());
    buf.extend_from_slice(&kind.magic()); // [0..4)
    buf.push(PROTO_VERSION); // [4]
    buf.push(0); // [5] flags (reserved)
    buf.extend_from_slice(&room_id.to_le_bytes()); // [6..10)
    buf.extend_from_slice(&(payload.len() as u32).to_le_bytes()); // [10..14)
    buf.extend_from_slice(&[0u8, 0u8]); // [14..16) reserved (header → 16B)
    buf.extend_from_slice(payload);
    buf
}

/// A decoded frame header + a borrow of its payload.
#[derive(Debug)]
pub struct Header<'a> {
    pub kind: FrameKind,
    pub room_id: u32,
    pub payload: &'a [u8],
}

/// Parse one frame's header and validate the declared payload length. Returns the kind, room, and a
/// borrow of the payload (zero-copy — the in-process transport relies on this).
pub fn decode(bytes: &[u8]) -> Result<Header<'_>, FrameError> {
    if bytes.len() < HEADER_LEN {
        return Err(FrameError::TooShort);
    }
    let kind = FrameKind::from_magic(&bytes[0..4])
        .ok_or_else(|| FrameError::BadMagic(bytes[0..4].try_into().unwrap()))?;
    let version = bytes[4];
    if version != PROTO_VERSION {
        return Err(FrameError::VersionMismatch {
            got: version,
            want: PROTO_VERSION,
        });
    }
    let room_id = u32::from_le_bytes(bytes[6..10].try_into().unwrap());
    let payload_len = u32::from_le_bytes(bytes[10..14].try_into().unwrap()) as usize;
    let payload = &bytes[HEADER_LEN..];
    if payload.len() != payload_len {
        return Err(FrameError::LengthMismatch {
            declared: payload_len,
            actual: payload.len(),
        });
    }
    Ok(Header {
        kind,
        room_id,
        payload,
    })
}

// ── DocUpdate (%LOR): a batch of Loro update blobs + an 8-byte batch id for ACK ────────────────

/// Encode a `DocUpdate`: an 8-byte batch id (echoed in the `Ack`) followed by N length-prefixed
/// Loro `update` blobs. A frame-tick coalescer normally produces ONE blob spanning the tick; the
/// batch supports N for the rare multi-source case without a wire change.
pub fn encode_doc_update(room_id: u32, batch_id: u64, blobs: &[&[u8]]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(&batch_id.to_le_bytes());
    p.extend_from_slice(&(blobs.len() as u32).to_le_bytes());
    for b in blobs {
        p.extend_from_slice(&(b.len() as u32).to_le_bytes());
        p.extend_from_slice(b);
    }
    encode(FrameKind::DocUpdate, room_id, &p)
}

/// A decoded `DocUpdate` payload.
#[derive(Debug)]
pub struct DocUpdate {
    pub batch_id: u64,
    pub blobs: Vec<Vec<u8>>,
}

pub fn decode_doc_update(payload: &[u8]) -> Result<DocUpdate, FrameError> {
    let mut r = Reader::new(payload);
    let batch_id = r.u64()?;
    let n = r.u32()? as usize;
    let mut blobs = Vec::with_capacity(n);
    for _ in 0..n {
        let len = r.u32()? as usize;
        blobs.push(r.bytes(len)?.to_vec());
    }
    Ok(DocUpdate { batch_id, blobs })
}

// ── Ack (%ACK) ────────────────────────────────────────────────────────────────

pub fn encode_ack(room_id: u32, batch_id: u64) -> Vec<u8> {
    encode(FrameKind::Ack, room_id, &batch_id.to_le_bytes())
}

pub fn decode_ack(payload: &[u8]) -> Result<u64, FrameError> {
    Reader::new(payload).u64()
}

// ── Handshake (%HSK): establish peer identity + the sender's known version ─────────────────────

pub fn encode_handshake(room_id: u32, peer_id: u64, known_vv: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(12 + known_vv.len());
    p.extend_from_slice(&peer_id.to_le_bytes());
    p.extend_from_slice(&(known_vv.len() as u32).to_le_bytes());
    p.extend_from_slice(known_vv);
    encode(FrameKind::Handshake, room_id, &p)
}

#[derive(Debug)]
pub struct Handshake {
    pub peer_id: u64,
    pub known_vv: Vec<u8>,
}

pub fn decode_handshake(payload: &[u8]) -> Result<Handshake, FrameError> {
    let mut r = Reader::new(payload);
    let peer_id = r.u64()?;
    let len = r.u32()? as usize;
    let known_vv = r.bytes(len)?.to_vec();
    Ok(Handshake { peer_id, known_vv })
}

// ── Fragment (%FRG): wrap an oversized inner frame ─────────────────────────────────────────────

/// Split `inner` (a complete enveloped frame) into ≤`FRAGMENT_THRESHOLD`-sized fragments sharing
/// `msg_id`. Each fragment payload is: msg_id · total_len · offset · chunk_len · chunk.
pub fn fragment(room_id: u32, msg_id: u64, inner: &[u8]) -> Vec<Vec<u8>> {
    let total = inner.len();
    let mut out = Vec::new();
    let mut offset = 0usize;
    while offset < total {
        let end = (offset + FRAGMENT_THRESHOLD).min(total);
        let chunk = &inner[offset..end];
        let mut p = Vec::with_capacity(20 + chunk.len());
        p.extend_from_slice(&msg_id.to_le_bytes());
        p.extend_from_slice(&(total as u32).to_le_bytes());
        p.extend_from_slice(&(offset as u32).to_le_bytes());
        p.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
        p.extend_from_slice(chunk);
        out.push(encode(FrameKind::Fragment, room_id, &p));
        offset = end;
    }
    out
}

#[derive(Debug)]
pub struct FragmentPart {
    pub msg_id: u64,
    pub total_len: usize,
    pub offset: usize,
    pub chunk: Vec<u8>,
}

pub fn decode_fragment(payload: &[u8]) -> Result<FragmentPart, FrameError> {
    let mut r = Reader::new(payload);
    let msg_id = r.u64()?;
    let total_len = r.u32()? as usize;
    let offset = r.u32()? as usize;
    let chunk_len = r.u32()? as usize;
    let chunk = r.bytes(chunk_len)?.to_vec();
    Ok(FragmentPart {
        msg_id,
        total_len,
        offset,
        chunk,
    })
}

// ── Ephemeral / Ping / Pong ───────────────────────────────────────────────────

/// Reserved Phase-2 ephemeral frame (presence/cursors). Round-trips opaque bytes; unused in M2.
pub fn encode_ephemeral(room_id: u32, payload: &[u8]) -> Vec<u8> {
    encode(FrameKind::Ephemeral, room_id, payload)
}

pub fn encode_ping(room_id: u32) -> Vec<u8> {
    encode(FrameKind::Ping, room_id, &[])
}

pub fn encode_pong(room_id: u32) -> Vec<u8> {
    encode(FrameKind::Pong, room_id, &[])
}

// ── tiny LE reader ────────────────────────────────────────────────────────────

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], FrameError> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or(FrameError::Malformed("length overflow"))?;
        if end > self.buf.len() {
            return Err(FrameError::Malformed("payload truncated"));
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32, FrameError> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, FrameError> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }
}
