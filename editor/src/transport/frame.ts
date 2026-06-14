//! TS mirror of the M2.4 **Loro-Protocol-v1 envelope** (`/transport/src/frame.rs`). Same 16-byte
//! header (magic `frame_kind` · `proto_version` · flags · room id · payload length) + kind payloads,
//! so the desktop shell (M2.6) can point this client straight at the real Rust transport.
//!
//! The UI never parses the document format: a `%LOR` payload is opaque to the wire layer. In the
//! browser the WASM core decodes Loro update bytes into a UI-facing **projection delta** (see
//! `protocol.ts`); in this scaffold the projection delta IS the payload (documented there).

export const PROTO_VERSION = 1;
export const HEADER_LEN = 16;
export const FRAGMENT_THRESHOLD = 256 * 1024;

export enum FrameKind {
  DocUpdate = "%LOR",
  Ack = "%ACK",
  Handshake = "%HSK",
  Fragment = "%FRG",
  Ephemeral = "%EPH",
  Ping = "%PNG",
  Pong = "%PNT",
}

const KIND_BY_MAGIC = new Map<string, FrameKind>(
  Object.values(FrameKind).map((k) => [k, k]),
);

export class FrameError extends Error {}

const enc = new TextEncoder();
const dec = new TextDecoder();

function header(kind: FrameKind, room: number, payloadLen: number): Uint8Array {
  const buf = new Uint8Array(HEADER_LEN + payloadLen);
  buf.set(enc.encode(kind), 0); // magic [0..4)
  buf[4] = PROTO_VERSION; // [4]
  buf[5] = 0; // flags [5]
  const dv = new DataView(buf.buffer);
  dv.setUint32(6, room, true); // room [6..10)
  dv.setUint32(10, payloadLen, true); // payload_len [10..14)
  // [14..16) reserved = 0
  return buf;
}

function frame(kind: FrameKind, room: number, payload: Uint8Array): Uint8Array {
  const buf = header(kind, room, payload.length);
  buf.set(payload, HEADER_LEN);
  return buf;
}

export interface Header {
  kind: FrameKind;
  room: number;
  payload: Uint8Array;
}

export function decode(bytes: Uint8Array): Header {
  if (bytes.length < HEADER_LEN) throw new FrameError("frame shorter than 16-byte header");
  const magic = dec.decode(bytes.subarray(0, 4));
  const kind = KIND_BY_MAGIC.get(magic);
  if (!kind) throw new FrameError(`unknown frame magic ${JSON.stringify(magic)}`);
  if (bytes[4] !== PROTO_VERSION) throw new FrameError(`proto_version ${bytes[4]} != ${PROTO_VERSION}`);
  const dv = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const room = dv.getUint32(6, true);
  const payloadLen = dv.getUint32(10, true);
  const payload = bytes.subarray(HEADER_LEN);
  if (payload.length !== payloadLen)
    throw new FrameError(`payload len ${payloadLen} != actual ${payload.length}`);
  return { kind, room, payload };
}

// ── DocUpdate (%LOR): batch_id (u64) + N length-prefixed blobs ────────────────

export function encodeDocUpdate(room: number, batchId: bigint, blobs: Uint8Array[]): Uint8Array {
  let len = 8 + 4;
  for (const b of blobs) len += 4 + b.length;
  const p = new Uint8Array(len);
  const dv = new DataView(p.buffer);
  dv.setBigUint64(0, batchId, true);
  dv.setUint32(8, blobs.length, true);
  let off = 12;
  for (const b of blobs) {
    dv.setUint32(off, b.length, true);
    off += 4;
    p.set(b, off);
    off += b.length;
  }
  return frame(FrameKind.DocUpdate, room, p);
}

export interface DocUpdate {
  batchId: bigint;
  blobs: Uint8Array[];
}

export function decodeDocUpdate(payload: Uint8Array): DocUpdate {
  const dv = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
  const batchId = dv.getBigUint64(0, true);
  const n = dv.getUint32(8, true);
  const blobs: Uint8Array[] = [];
  let off = 12;
  for (let i = 0; i < n; i++) {
    const len = dv.getUint32(off, true);
    off += 4;
    blobs.push(payload.subarray(off, off + len));
    off += len;
  }
  return { batchId, blobs };
}

// ── Ack (%ACK) ────────────────────────────────────────────────────────────────

export function encodeAck(room: number, batchId: bigint): Uint8Array {
  const p = new Uint8Array(8);
  new DataView(p.buffer).setBigUint64(0, batchId, true);
  return frame(FrameKind.Ack, room, p);
}

export function decodeAck(payload: Uint8Array): bigint {
  return new DataView(payload.buffer, payload.byteOffset, payload.byteLength).getBigUint64(0, true);
}

// ── Handshake (%HSK): peer id (u64) + known version vector ────────────────────

export function encodeHandshake(room: number, peerId: bigint, knownVv: Uint8Array): Uint8Array {
  const p = new Uint8Array(12 + knownVv.length);
  const dv = new DataView(p.buffer);
  dv.setBigUint64(0, peerId, true);
  dv.setUint32(8, knownVv.length, true);
  p.set(knownVv, 12);
  return frame(FrameKind.Handshake, room, p);
}

export interface Handshake {
  peerId: bigint;
  knownVv: Uint8Array;
}

export function decodeHandshake(payload: Uint8Array): Handshake {
  const dv = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
  const peerId = dv.getBigUint64(0, true);
  const len = dv.getUint32(8, true);
  return { peerId, knownVv: payload.subarray(12, 12 + len) };
}

// ── Ephemeral (%EPH): the reserved presence/live-preview channel — opaque payload ──

export function encodeEphemeral(room: number, payload: Uint8Array): Uint8Array {
  return frame(FrameKind.Ephemeral, room, payload);
}

export function encodePing(room: number): Uint8Array {
  return frame(FrameKind.Ping, room, new Uint8Array(0));
}

export function encodePong(room: number): Uint8Array {
  return frame(FrameKind.Pong, room, new Uint8Array(0));
}
