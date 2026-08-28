//! **The Transform, as a property rather than as storage** (ADR-172).
//!
//! An entity's `Transform` is stored as up to eight flat scalars — `x`/`y`/`z`, a rotation
//! quaternion `qx`/`qy`/`qz`/`qw`, and a uniform `scale` — because `FieldType` is scalar-only. That
//! is a fact about the wire. Until this module it was also a fact about the panel: the Inspector
//! rendered whichever of those keys the document happened to contain, so a fresh object showed three
//! bare letters and a rotated one grew four more, each an independently editable box on a value that
//! is only meaningful as a unit quaternion.
//!
//! Two rules follow, and this file is where they are stated:
//!
//! 1. **A Transform is complete, not sparse.** `capscene::local_transform` reads a missing field as
//!    the identity — position 0, quaternion `(0,0,0,1)`, scale 1 — so an object with only `x`/`y`/`z`
//!    is not "unrotated and unscalable", it is *at* identity rotation and scale 1. [`readTransform`]
//!    resolves exactly the same way, so the panel shows what the renderer sees.
//! 2. **A rotation is one property, in degrees.** Nobody types a quaternion. The conversion lives
//!    here, in ONE place and ONE language, and the engine's `set_rotation` takes the quaternion and
//!    normalises it — so the editor owns the Euler convention and the engine owns the invariant.
//!
//! **The convention, stated once:** intrinsic **Y-X-Z** (yaw about world up, then pitch, then roll),
//! the same order Godot and Unity's inspector use, on a Y-up right-handed engine
//! (`create_entity(0, 1, 0)` is documented as "one metre above the world origin"). Round-tripping
//! through gimbal lock is lossy in *angles* and exact in *rotation*, which is why the panel keeps the
//! angles a user typed while the field is focused and re-derives them from the quaternion otherwise.

import type { Json } from "../transport/protocol";

/** A resolved transform: what `capscene::local_transform` would read for this entity. */
export interface ResolvedTransform {
  /** World/local position in metres. */
  position: [number, number, number];
  /** Unit rotation quaternion `[qx, qy, qz, qw]`. */
  quaternion: [number, number, number, number];
  /** Uniform display scale. */
  scale: number;
}

/** The engine's identity, restated where the panel can use it. Matches `local_transform`'s defaults. */
export const IDENTITY: ResolvedTransform = {
  position: [0, 0, 0],
  quaternion: [0, 0, 0, 1],
  scale: 1,
};

/** The eight field names a `Transform` is stored as — `core/src/stdlib.rs`'s declaration, in the
 *  order a person reads them. The Inspector uses this to keep `Transform` out of the generic
 *  data-driven form, so no field is drawn twice. */
export const TRANSFORM_FIELDS = ["x", "y", "z", "qx", "qy", "qz", "qw", "scale"] as const;

function num(value: Json | undefined, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

/** Resolve an entity's projected `Transform` map the way the engine's reader does: a field that is
 *  absent is the identity, not a hole. Returns the identity for an entity with no Transform at all,
 *  so a caller never has to branch on absence twice. */
export function readTransform(fields: Record<string, Json> | undefined): ResolvedTransform {
  if (!fields) return { ...IDENTITY, position: [0, 0, 0], quaternion: [0, 0, 0, 1] };
  return {
    position: [num(fields.x, 0), num(fields.y, 0), num(fields.z, 0)],
    quaternion: [num(fields.qx, 0), num(fields.qy, 0), num(fields.qz, 0), num(fields.qw, 1)],
    scale: num(fields.scale, 1),
  };
}

const DEG = 180 / Math.PI;
const RAD = Math.PI / 180;

/** Round a degree value to the precision a person can read and re-type without drift accumulating.
 *  1e-4° is ~0.4 arc-seconds — far below anything authorable, and it stops `45` round-tripping as
 *  `44.99999999999999`, which is what an unrounded conversion actually produces. */
function tidy(deg: number): number {
  const r = Math.round(deg * 1e4) / 1e4;
  // `-0` reads as "-0" in a text field and is the same angle as 0.
  return Object.is(r, -0) ? 0 : r;
}

/** A unit quaternion → intrinsic Y-X-Z Euler angles in degrees `[x, y, z]` (pitch, yaw, roll).
 *
 *  A non-unit input is normalised first: the document cannot hold one (the engine normalises on
 *  write) but a projection can arrive mid-flight from any writer, and `asin` of an out-of-range value
 *  is `NaN`, which would paint an empty box rather than a number. */
export function quatToEulerDeg(q: readonly [number, number, number, number]): [number, number, number] {
  const length = Math.hypot(q[0], q[1], q[2], q[3]);
  if (!Number.isFinite(length) || length < 1e-9) return [0, 0, 0];
  const [x, y, z, w] = q.map((c) => c / length) as [number, number, number, number];

  // Intrinsic Y-X-Z: pitch (about X) is the one that gimbal-locks, so it is clamped rather than
  // allowed to produce NaN at exactly ±90°.
  const sinPitch = Math.max(-1, Math.min(1, 2 * (w * x - y * z)));
  const pitch = Math.asin(sinPitch);
  if (Math.abs(sinPitch) > 0.999999) {
    // Gimbal lock: yaw and roll are no longer independent. Put the whole rotation in yaw and leave
    // roll at zero — a stable, reproducible choice rather than a numerically arbitrary split.
    return [tidy(pitch * DEG), tidy(2 * Math.atan2(y, w) * DEG), 0];
  }
  const yaw = Math.atan2(2 * (w * y + x * z), 1 - 2 * (x * x + y * y));
  const roll = Math.atan2(2 * (w * z + x * y), 1 - 2 * (x * x + z * z));
  return [tidy(pitch * DEG), tidy(yaw * DEG), tidy(roll * DEG)];
}

/** Intrinsic Y-X-Z Euler angles in degrees `[x, y, z]` → a unit quaternion `[qx, qy, qz, qw]`.
 *  The exact inverse of [`quatToEulerDeg`] as a ROTATION; the angles themselves round-trip except
 *  through gimbal lock, where infinitely many angle triples name the same rotation. */
export function eulerDegToQuat(euler: readonly [number, number, number]): [number, number, number, number] {
  const [px, py, pz] = euler.map((d) => (Number.isFinite(d) ? d * RAD : 0) / 2);
  const [sx, cx] = [Math.sin(px), Math.cos(px)];
  const [sy, cy] = [Math.sin(py), Math.cos(py)];
  const [sz, cz] = [Math.sin(pz), Math.cos(pz)];
  // q = qY * qX * qZ (intrinsic Y-X-Z).
  return [
    sx * cy * cz + cx * sy * sz,
    cx * sy * cz - sx * cy * sz,
    cx * cy * sz - sx * sy * cz,
    cx * cy * cz + sx * sy * sz,
  ];
}

/** Do two selections' worth of values agree? Used to decide whether a row reads a number or `Mixed`
 *  (ADR-169's rule, applied to a derived property). The tolerance is the display precision: two
 *  rotations a user cannot tell apart in the box must not read as `Mixed`. */
export function agreeWithin(values: number[], epsilon = 1e-4): boolean {
  if (values.length === 0) return false;
  const first = values[0];
  return values.every((v) => Math.abs(v - first) <= epsilon);
}

/** The components a **data-driven** form should draw: everything except `Transform`, which
 *  [`TransformSection`](./TransformSection.tsx) owns. Stated here rather than at each call site so
 *  the two Inspectors cannot disagree about which panel draws which component — a disagreement whose
 *  visible form is either a row drawn twice or a row drawn by nobody. */
export function withoutTransform<T>(components: Record<string, T>): Record<string, T> {
  if (!("Transform" in components)) return components;
  const rest = { ...components };
  delete rest.Transform;
  return rest;
}
