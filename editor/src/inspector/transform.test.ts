import { describe, expect, it } from "vitest";
import {
  agreeWithin,
  eulerDegToQuat,
  quatToEulerDeg,
  readTransform,
  TRANSFORM_FIELDS,
} from "./transform";
import { componentSchemas } from "../schema/registry";

/** Rotate a vector by a quaternion — the operation the renderer performs, used here to assert that a
 *  round trip preserves the ROTATION even where it cannot preserve the angles. */
function rotate(q: readonly [number, number, number, number], v: readonly [number, number, number]) {
  const [x, y, z, w] = q;
  const t: [number, number, number] = [
    2 * (y * v[2] - z * v[1]),
    2 * (z * v[0] - x * v[2]),
    2 * (x * v[1] - y * v[0]),
  ];
  return [
    v[0] + w * t[0] + (y * t[2] - z * t[1]),
    v[1] + w * t[1] + (z * t[0] - x * t[2]),
    v[2] + w * t[2] + (x * t[1] - y * t[0]),
  ];
}

describe("readTransform — a Transform is complete, not sparse", () => {
  it("resolves a missing field to the engine's identity, not to nothing", () => {
    // What `capscene::create_entity` actually writes: three position fields and no more. The panel
    // must still be able to offer a rotation and a scale, because `local_transform` reads this
    // entity as identity-rotated and scale 1 — which is what the renderer draws.
    const t = readTransform({ x: 1, y: 2, z: 3 });
    expect(t.position).toEqual([1, 2, 3]);
    expect(t.quaternion).toEqual([0, 0, 0, 1]);
    expect(t.scale).toBe(1);
  });

  it("resolves an entity with no Transform at all to the identity", () => {
    expect(readTransform(undefined)).toEqual({
      position: [0, 0, 0],
      quaternion: [0, 0, 0, 1],
      scale: 1,
    });
  });

  it("ignores a non-numeric or non-finite value rather than painting NaN", () => {
    const t = readTransform({ x: "2.5" as never, y: Number.NaN as never, scale: 3 });
    expect(t.position).toEqual([0, 0, 0]);
    expect(t.scale).toBe(3);
  });
});

describe("the rotation a person types", () => {
  it("reads the identity quaternion as zero degrees", () => {
    expect(quatToEulerDeg([0, 0, 0, 1])).toEqual([0, 0, 0]);
  });

  it("round-trips the angles a user actually types", () => {
    for (const euler of [
      [0, 45, 0],
      [0, 90, 0],
      [30, 0, 0],
      [0, 0, -15],
      [10, 20, 30],
      [-45, 135, 60],
      [0, 180, 0],
    ] as [number, number, number][]) {
      const back = quatToEulerDeg(eulerDegToQuat(euler));
      expect(back.map((v) => Math.round(v * 100) / 100)).toEqual(euler);
    }
  });

  it("produces a UNIT quaternion for every angle triple", () => {
    for (const euler of [
      [0, 0, 0],
      [90, 0, 0],
      [-90, 45, 12],
      [359, -720, 33.3],
    ] as [number, number, number][]) {
      const q = eulerDegToQuat(euler);
      expect(Math.hypot(...q)).toBeCloseTo(1, 12);
    }
  });

  it("agrees with the rotation the renderer applies — 90° yaw sends +X to -Z", () => {
    // The concrete check that the CONVENTION is right and not merely self-consistent: a right-handed
    // Y-up engine turning 90° about +Y takes the +X axis onto -Z.
    const q = eulerDegToQuat([0, 90, 0]);
    const [x, y, z] = rotate(q, [1, 0, 0]);
    expect(x).toBeCloseTo(0, 6);
    expect(y).toBeCloseTo(0, 6);
    expect(z).toBeCloseTo(-1, 6);
  });

  it("survives gimbal lock with a rotation that is still correct", () => {
    // At pitch ±90° the angles are no longer unique, so the ANGLES may differ; the rotation may not.
    const q = eulerDegToQuat([90, 30, 0]);
    const back = eulerDegToQuat(quatToEulerDeg(q));
    for (const v of [
      [1, 0, 0],
      [0, 1, 0],
      [0, 0, 1],
    ] as [number, number, number][]) {
      const a = rotate(q, v);
      const b = rotate(back, v);
      a.forEach((component, i) => expect(component).toBeCloseTo(b[i], 5));
    }
  });

  it("never returns NaN for a degenerate or non-unit quaternion", () => {
    expect(quatToEulerDeg([0, 0, 0, 0])).toEqual([0, 0, 0]);
    expect(quatToEulerDeg([Number.NaN, 0, 0, 1])).toEqual([0, 0, 0]);
    // A quaternion of length 5 is what four independent number boxes used to be able to commit.
    const scaled = quatToEulerDeg([0, 5 * Math.SQRT1_2, 0, 5 * Math.SQRT1_2]);
    expect(scaled[1]).toBeCloseTo(90, 4);
  });
});

describe("agreeWithin", () => {
  it("is false for an empty selection and true for values inside the display precision", () => {
    expect(agreeWithin([])).toBe(false);
    expect(agreeWithin([45, 45.00001])).toBe(true);
    expect(agreeWithin([45, 45.01])).toBe(false);
  });
});

describe("the panel and the registry state the vocabulary once", () => {
  it("TRANSFORM_FIELDS is exactly the curated Transform schema's fields", () => {
    // ADR-172 — the editor's two statements of the Transform vocabulary, compared. The curated table
    // is checked against `core/src/stdlib.rs` by `check-registry-vocab.mjs`; this closes the loop to
    // the list the Inspector uses to keep those fields out of the generic form. A field in one and
    // not the other is either a row drawn twice or a row drawn by nobody.
    expect([...TRANSFORM_FIELDS].sort()).toEqual(
      Object.keys(componentSchemas.Transform.properties).sort(),
    );
  });
});
