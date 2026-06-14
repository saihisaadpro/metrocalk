import { describe, expect, it } from "vitest";
import {
  decode,
  decodeAck,
  decodeDocUpdate,
  decodeHandshake,
  encodeAck,
  encodeDocUpdate,
  encodeEphemeral,
  encodeHandshake,
  FrameKind,
  HEADER_LEN,
} from "./frame";

describe("Loro-Protocol-v1 envelope (TS mirror of M2.4 frame.rs)", () => {
  it("DocUpdate round-trips batch id + blobs", () => {
    const blobs = [new Uint8Array([1, 2, 3]), new Uint8Array([9, 9])];
    const f = encodeDocUpdate(7, 0xdead_beefn, blobs);
    const h = decode(f);
    expect(h.kind).toBe(FrameKind.DocUpdate);
    expect(h.room).toBe(7);
    const du = decodeDocUpdate(h.payload);
    expect(du.batchId).toBe(0xdead_beefn);
    expect([...du.blobs[0]]).toEqual([1, 2, 3]);
    expect([...du.blobs[1]]).toEqual([9, 9]);
  });

  it("header is the fixed 16 bytes with magic + LE room", () => {
    const f = encodeDocUpdate(0x01020304, 1n, [new Uint8Array(0)]);
    expect(String.fromCharCode(...f.subarray(0, 4))).toBe("%LOR");
    expect(f[4]).toBe(1); // proto_version
    expect(f.length).toBeGreaterThanOrEqual(HEADER_LEN);
    // room little-endian at [6..10)
    expect([f[6], f[7], f[8], f[9]]).toEqual([0x04, 0x03, 0x02, 0x01]);
  });

  it("Ack + Handshake + Ephemeral round-trip", () => {
    expect(decodeAck(decode(encodeAck(1, 42n)).payload)).toBe(42n);
    const hs = decodeHandshake(decode(encodeHandshake(1, 0xa11cen, new Uint8Array([5, 6]))).payload);
    expect(hs.peerId).toBe(0xa11cen);
    expect([...hs.knownVv]).toEqual([5, 6]);
    expect(decode(encodeEphemeral(1, new Uint8Array([1]))).kind).toBe(FrameKind.Ephemeral);
  });

  it("rejects a short frame and a bad magic", () => {
    expect(() => decode(new Uint8Array(4))).toThrow();
    const bad = encodeAck(1, 1n);
    bad[0] = "Z".charCodeAt(0);
    expect(() => decode(bad)).toThrow();
  });
});
