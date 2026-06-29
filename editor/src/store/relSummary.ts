//! The DEV/TEST stand-in for the real `/core` relational projection (M14.2 / ADR-058). The authoritative
//! relational summary is computed in Rust (`editor-shell/src/bridge.rs`) from the real `(Provides/Requires,
//! cap)` ECS pairs + `bindings()`. This module is the faithful mirror the in-process MockCore + the
//! `bulkLoad` perf/test path use, so the hierarchy/Requirers read the SAME structured signal (`kind`, `rel`)
//! in `npm run dev` / Vitest and on the live `.exe` — the C6 vocabulary, keyed off real component names.

import type { Json, RelSummary } from "../transport/protocol";

/** The salient type for an entity's type-icon, from its components (the real `/core` vocabulary). */
export function deriveKind(components: Record<string, Record<string, Json>>): string {
  if ("Light" in components) return "light";
  if ("Camera" in components) return "camera";
  if ("RigidBody" in components || "Collider" in components) return "physics";
  if ("AudioSource" in components) return "audio";
  if ("HealthBar" in components) return "requirer";
  if ("MeshRenderer" in components) return "mesh";
  return "default";
}

/** The relational summary: a `HealthBar` REQUIRES Health (the real requirer marker — an ECS pair on the live
 *  core), a `Health` component PROVIDES Health; `needsBinding` = a required cap not yet satisfied by a binding
 *  (`bound` outgoing bindings). The authoritative form is the Rust cap query; this mirrors it for dev/test. */
export function deriveRel(components: Record<string, Record<string, Json>>, bound = 0): RelSummary {
  const requires = "HealthBar" in components ? ["Health"] : [];
  const provides = "Health" in components ? ["Health"] : [];
  return { requires, provides, bound, needsBinding: requires.length > 0 && bound === 0, isGroup: false };
}
