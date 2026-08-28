//! **The words the editor uses about a file format**, in one place.
//!
//! `FormatsPanel` (what this build can read and write) and `ExportDialog` (write it now) describe the
//! same registry to the same person, minutes apart. Two copies of "what does `subset` mean" is the
//! twin-panel drift this repository has already paid for twice — `StateGraphPanel` and `RulesPanel`
//! each grew their own value editor and disagreed about which operators a boolean admits. So the
//! fidelity vocabulary, the capability list and its ORDER live here, and both surfaces read them.
//!
//! The order of [`CARRIES`] is deliberate and is not alphabetical: it is the order a person actually
//! asks the questions in, which is why a shared list is worth more than a shared type.

import type { FormatSpec } from "../transport/protocol";

/** What each fidelity tier promises, and the hint that keeps "supported" from meaning four things. */
export const FIDELITY_COPY: Record<string, { label: string; hint: string }> = {
  full: { label: "Full", hint: "Everything the engine's scene model holds survives" },
  subset: { label: "Subset", hint: "A stated, tested part of the format — the rest is reported" },
  seam: { label: "Seam", hint: "Recognised and explained here, decoded elsewhere" },
};

/** Which way a format flows, in a reader's words rather than the enum's. */
export const DIRECTION_COPY: Record<string, string> = {
  import: "Read",
  export: "Write",
  both: "Read + write",
};

/** One capability a format either carries or does not. */
export interface CarrySpec {
  key: keyof FormatSpec["carries"];
  /** Sentence-case, for a list ("Carries geometry, hierarchy…"). */
  label: string;
  /** Title-case, for a row in a checklist. */
  title: string;
}

/** The capability flags, in the order a person actually asks about them. */
export const CARRIES: readonly CarrySpec[] = [
  { key: "geometry", label: "geometry", title: "Geometry" },
  { key: "hierarchy", label: "hierarchy", title: "Hierarchy" },
  { key: "materials", label: "materials", title: "Materials" },
  { key: "textures", label: "textures", title: "Textures" },
  { key: "skinning", label: "skinning", title: "Skinning" },
  { key: "animation", label: "animation", title: "Animation" },
  { key: "cameras", label: "cameras", title: "Cameras" },
  { key: "metadata", label: "engineering data", title: "Engineering data" },
  { key: "physics", label: "physics", title: "Physics" },
];

/** True when this format can be WRITTEN by this build — the export dialog's whole membership rule. */
export const writesScenes = (spec: FormatSpec): boolean =>
  spec.available && (spec.direction === "export" || spec.direction === "both");

/**
 * The `format` argument `scene_export` accepts for a spec — its CANONICAL extension.
 *
 * Derived, never restated. `formats.rs` declares the accepted set once
 * (`EXPORT_ARGS` = `glb · usda · usd · step · stp`) and asserts in
 * `every_writable_format_is_addressable_by_its_canonical_extension` that every writable format's
 * `extensions[0]` is in it, so this function needs no table of its own and a new writer needs no edit
 * here. [`EXPORT_ARGS`] below is the TypeScript half of that pairing: it exists so a format whose
 * canonical extension the command would REFUSE is refused visibly, in the dialog, with a reason —
 * rather than being offered and failing at the click.
 */
export const exportArgFor = (spec: FormatSpec): string => (spec.extensions[0] ?? "").toLowerCase();

/** Mirror of `formats::EXPORT_ARGS`. See [`exportArgFor`] for why this is not a mapping table. */
export const EXPORT_ARGS: readonly string[] = ["glb", "usda", "usd", "step", "stp"];

/** True when `scene_export` will act on this argument. */
export const exportArgAccepted = (arg: string): boolean => EXPORT_ARGS.includes(arg.replace(/^\./, "").toLowerCase());
