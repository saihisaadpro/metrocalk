//! Component JSON Schemas — the UI stand-in for `metrocalk-core`'s metadata registry (M1.3). A curated
//! schema exists only for components with a **typed/semantic field** that needs a custom renderer (a
//! `format` the JSON-Forms tester routes, or a closed vocabulary). Everything else is rendered by a
//! **data-driven** schema inferred from the projected component values (M10.10 closeout) — so the inspector
//! renders the REAL `/core` vocabulary (Transform/Health/HealthBar/MeshRenderer · whatever the core sends),
//! not just a hardcoded list. This is the fix for C6 live: the projection carries the values, so the
//! inspector never needs a registry that mirrors every core component.
//!
//! **AND FOR THREE MILESTONES THAT HEADER WAS FALSE ABOUT ITS OWN TABLE (ADR-136).** The curated entries
//! were `Transform{x,y,z}`, `Material`, `Health{max}`, `Provides` and `Targeting`. The core registers
//! `Transform{px..sz}` and `Health{hp,maxHp,regen}`, and has **never registered `Material`, `Provides`
//! or `Targeting` at all** — they are the dev `MockCore`'s own vocabulary (`transport/session.ts`).
//! Nothing noticed because the data-driven fallback is good enough to hide a table that is wrong: a
//! Number infers to a number whatever the field is called. Only the fields where inference picks the
//! WRONG CONTROL failed visibly, and those failed completely — `ColorControl` and `EntityRefControl`
//! were both keyed on components that do not exist, so neither could ever fire against the real core,
//! while the core's own eight `format` fields and seven closed vocabularies had no entry at all.
//!
//! The table below is now written against `core/src/stdlib.rs`, and
//! **`editor/scripts/check-registry-vocab.mjs` compares the two on every push** — both directions: a
//! name the core does not have, and a `format` or `ui_hint` vocabulary the core publishes and this
//! table stays silent about. Fields with neither are deliberately absent: inference gets them right.

import type { JsonSchema7, UISchemaElement } from "@jsonforms/core";
import type { Json } from "../transport/protocol";

/** A curated field schema. `unit` is the one non-standard key: JSON Schema has no unit concept and the
 *  core's registry carries units only as prose inside `ui_hint` ("companion move speed, metres per
 *  second"), so the readable half is restated here where a control can render it. Every unit below is
 *  the engine's own — metres, degrees, kilograms — never a guess. */
export type FieldSchema = JsonSchema7 & { unit?: string };

/** One component's properties. Narrower than `JsonSchema7` on purpose: the inspector's uischema
 *  generator emits one `Control` per leaf, so a curated entry that was not an object of leaves would
 *  render as nothing at all. */
export interface ComponentSchema {
  type: "object";
  properties: Record<string, FieldSchema>;
}

/** Per-component property schemas for fields the data-driven fallback would render with the WRONG
 *  control — a `format` a tester routes, or a closed vocabulary the core states in `ui_hint`. Plain
 *  scalars are inferred from the projected value instead.
 *
 *  `title` and `description` are standard JSON Schema and JSON Forms already reads them as the label
 *  and the help line, so a cryptic wire name (`px`, `hp`, `sx`) can read as a sentence without a
 *  second lookup table to drift. `default` is what the row's inline reset reverts to; it is declared
 *  only where the engine genuinely has one (an unrotated transform is 0, an unscaled one is 1). */
export const componentSchemas: Record<string, ComponentSchema> = {
  // Position/rotation/scale. Nine flat scalars, because `FieldType` is scalar-only — which is exactly
  // why the units matter: `px` alone does not say metres, and the engine is metric everywhere
  // (`unit_from_bounds`, `worldSizeM`, `diameterCm`).
  Transform: {
    type: "object",
    properties: {
      px: { type: "number", title: "Position X", unit: "m", default: 0 },
      py: { type: "number", title: "Position Y", unit: "m", default: 0 },
      pz: { type: "number", title: "Position Z", unit: "m", default: 0 },
      rx: { type: "number", title: "Rotation X", unit: "°", default: 0 },
      ry: { type: "number", title: "Rotation Y", unit: "°", default: 0 },
      rz: { type: "number", title: "Rotation Z", unit: "°", default: 0 },
      sx: { type: "number", title: "Scale X", unit: "×", default: 1 },
      sy: { type: "number", title: "Scale Y", unit: "×", default: 1 },
      sz: { type: "number", title: "Scale Z", unit: "×", default: 1 },
    },
  },
  Health: {
    type: "object",
    properties: {
      // The core's ui_hint for `hp` is "slider 0..maxHp" — the floor is expressible here, the ceiling
      // is another field's value and JSON Schema cannot say that, so it is not claimed.
      hp: { type: "integer", title: "Health", minimum: 0 },
      maxHp: { type: "integer", title: "Max health", minimum: 0 },
      regen: { type: "number", title: "Regeneration", unit: "/s" },
    },
  },
  Sprite: {
    type: "object",
    properties: { texture: { type: "string", title: "Texture", format: "asset" } },
  },
  MeshRenderer: {
    type: "object",
    properties: {
      mesh: { type: "string", title: "Mesh", format: "asset" },
      material: { type: "string", title: "Material", format: "asset" },
      castShadows: { type: "boolean", title: "Cast shadows" },
    },
  },
  RigidBody: {
    type: "object",
    properties: {
      kind: { type: "string", title: "Body", enum: ["dynamic", "fixed", "kinematicPosition", "kinematicVelocity"] },
      mass: { type: "number", title: "Mass", unit: "kg", minimum: 0 },
      linearDamping: { type: "number", title: "Linear damping", minimum: 0 },
      angularDamping: { type: "number", title: "Angular damping", minimum: 0 },
      gravityScale: { type: "number", title: "Gravity scale", unit: "×", default: 1 },
    },
  },
  Collider: {
    type: "object",
    properties: {
      shape: {
        type: "string",
        title: "Shape",
        enum: ["ball", "cuboid", "capsule", "convexHull", "triMesh", "convexDecomposition", "voxels", "sdf"],
      },
      isTrigger: { type: "boolean", title: "Trigger", description: "Reports overlaps without blocking them." },
      density: { type: "number", title: "Density", minimum: 0 },
      friction: { type: "number", title: "Friction", minimum: 0 },
      restitution: { type: "number", title: "Bounciness", minimum: 0, maximum: 1 },
      radius: { type: "number", title: "Radius", unit: "m", minimum: 0 },
      halfX: { type: "number", title: "Half width", unit: "m", minimum: 0 },
      halfY: { type: "number", title: "Half height", unit: "m", minimum: 0 },
      halfZ: { type: "number", title: "Half depth", unit: "m", minimum: 0 },
      halfHeight: { type: "number", title: "Half length", unit: "m", minimum: 0 },
    },
  },
  // THE ENTITY REFERENCES THE ENGINE ACTUALLY HAS. `EntityRefControl` has existed since M2.5 and has
  // never had a live field to fire on: it was keyed on `Targeting.target`, which the core does not
  // register. These two are the real ones (`field_fmt(.., Some("entity-ref"))`).
  Joint: {
    type: "object",
    properties: {
      kind: { type: "string", title: "Joint", enum: ["revolute", "fixed", "spherical"] },
      bodyA: { type: "string", title: "Body A", format: "entity-ref" },
      bodyB: { type: "string", title: "Body B", format: "entity-ref" },
    },
  },
  AudioSource: {
    type: "object",
    properties: {
      clip: { type: "string", title: "Clip", format: "asset" },
      volume: { type: "number", title: "Volume", minimum: 0, maximum: 1, default: 1 },
      looping: { type: "boolean", title: "Loop" },
    },
  },
  Light: {
    type: "object",
    properties: {
      kind: { type: "string", title: "Light", enum: ["directional", "point", "spot"] },
      intensity: { type: "number", title: "Intensity", minimum: 0 },
      // The core models colour as three separate Numbers, not one string, so there is no `format:
      // color` field anywhere in the registry and nothing for a colour picker to fire on. Naming the
      // channels is what this table can honestly do about it today; see ADR-136's owed item.
      r: { type: "number", title: "Red", minimum: 0, maximum: 1 },
      g: { type: "number", title: "Green", minimum: 0, maximum: 1 },
      b: { type: "number", title: "Blue", minimum: 0, maximum: 1 },
      range: { type: "number", title: "Range", unit: "m", minimum: 0 },
      castShadows: { type: "boolean", title: "Cast shadows" },
    },
  },
  Animator: {
    type: "object",
    properties: {
      controller: { type: "string", title: "Controller", format: "asset" },
      speed: { type: "number", title: "Speed", unit: "×", default: 1 },
    },
  },
  Script: {
    type: "object",
    properties: {
      source: { type: "string", title: "Source", format: "asset" },
      enabled: { type: "boolean", title: "Enabled", default: true },
    },
  },
  PlayIf: {
    type: "object",
    properties: {
      join: {
        type: "string",
        title: "Match",
        enum: ["all", "any"],
        description: "Whether every clause must hold, or any one of them.",
      },
    },
  },
  ShapeRecipe: {
    type: "object",
    properties: {
      kind: {
        type: "string",
        title: "Shape",
        enum: [
          "box", "sphere", "cylinder", "cone", "torus", "capsule", "wedge",
          "prism", "extrude", "revolve", "union", "carve", "intersect", "meld",
        ],
      },
    },
  },
  // Units the core states as prose in its own `ui_hint`s ("touch trigger / aggro reach, metres",
  // "companion move speed, metres per second"), read back out where the number is edited. `role`'s
  // vocabulary is deliberately NOT restated here — the Roles section already renders it with
  // descriptions and per-role tuning, and is declared as such in `scripts/registry-vocab.json`.
  GameRole: {
    type: "object",
    properties: {
      radius: { type: "number", title: "Reach", unit: "m", minimum: 0 },
      speed: { type: "number", title: "Move speed", unit: "m/s", minimum: 0 },
      range: { type: "number", title: "Attack reach", unit: "m", minimum: 0 },
      follow: { type: "number", title: "Follow distance", unit: "m", minimum: 0 },
    },
  },
  TerrainRecipe: {
    type: "object",
    properties: {
      worldSizeM: { type: "number", title: "World size", unit: "m", minimum: 0 },
      chunkSizeM: { type: "number", title: "Chunk size", unit: "m", minimum: 0 },
    },
  },
  PipeRecipe: {
    type: "object",
    properties: {
      diameterCm: { type: "number", title: "Diameter", unit: "cm", minimum: 0 },
      lengthM: { type: "number", title: "Length", unit: "m", minimum: 0 },
    },
  },
  // NOT A CORE COMPONENT, AND DECLARED AS SUCH in `scripts/registry-vocab.json`. The dev `MockCore`
  // models a capability as a component field so bind-by-intent has something to reject under
  // `npm run dev` and in vitest; the core models the same thing as metadata on `ComponentMeta`, which
  // never reaches an entity's projected components. The entry stays so the mock's own entities render
  // a vocabulary rather than a free-text box, and the gate keeps the divergence visible.
  Provides: {
    type: "object",
    properties: {
      capability: { type: "string", title: "Provides", enum: ["Health", "Shield", "Click", "Damage", "Light"] },
    },
  },
};

/** Infer a JSON-Schema field type from a projected value (the data-driven fallback). */
function inferField(value: Json): JsonSchema7 {
  if (typeof value === "number") return { type: "number" };
  if (typeof value === "boolean") return { type: "boolean" };
  if (typeof value === "string") return { type: "string" };
  // arrays / nested objects / null → a read-only string view (rare; the core's leaf fields are scalars)
  return { type: "string" };
}

/** Build a combined JSON Schema for an entity from the components it ACTUALLY has, data-driven: every
 *  component + field present is rendered (type inferred from its value), and a curated typed-field schema
 *  (a `format`, a closed vocabulary, a unit) is preferred per-field when one exists — so the inspector
 *  renders the real `/core` vocabulary while still routing asset/entity-ref fields to their controls.
 *
 *  A curated entry is used only when the entity ACTUALLY CARRIES that field: the loop walks the
 *  projection, never the table. That is what keeps a table entry from inventing a row — the failure the
 *  old `Transform{x,y,z}` would have produced the moment anything read the table forwards. */
export function buildEntitySchema(components: Record<string, Record<string, Json>>): JsonSchema7 {
  const properties: Record<string, JsonSchema7> = {};
  for (const [name, fields] of Object.entries(components)) {
    const curated = componentSchemas[name]?.properties;
    const props: Record<string, JsonSchema7> = {};
    for (const [field, value] of Object.entries(fields)) {
      // A curated schema whose declared type disagrees with the value on the wire is NOT applied: the
      // projection is the authority about what arrived, and rendering a boolean control over a string
      // is how a control silently edits the wrong thing. `check-registry-vocab.mjs` makes that
      // disagreement red at rest; this keeps it harmless at runtime.
      const c = curated?.[field];
      props[field] = c && agrees(c, value) ? c : inferField(value);
    }
    properties[name] = { type: "object", properties: props };
  }
  return { type: "object", properties };
}

/** Does a curated field schema describe the value that actually arrived? `integer` is a `number` on the
 *  wire, and a `null`/absent value is unbound rather than wrong — the control shows its unbound cue. */
function agrees(schema: FieldSchema, value: Json): boolean {
  if (value === null || value === undefined) return true;
  if (schema.type === "integer" || schema.type === "number") return typeof value === "number";
  if (schema.type === "boolean") return typeof value === "boolean";
  if (schema.type === "string") return typeof value === "string";
  return true;
}

/** Build the **UI schema** to pair with [`buildEntitySchema`] — a `Group` per component, a `Control` per
 *  leaf field (scoped `#/properties/<component>/properties/<field>`). JsonForms' auto-generation does NOT
 *  recurse into object-typed properties (the vanilla renderers have no recursing object control), so
 *  without this an entity's nested component fields render as NOTHING. Generating the leaf controls
 *  explicitly is what makes the inspector show real, **editable** properties (the C6 fix). */
export function buildEntityUiSchema(components: Record<string, Record<string, Json>>): UISchemaElement {
  const groups = Object.entries(components).map(([component, fields]) => ({
    type: "Group",
    label: component,
    elements: Object.keys(fields).map((field) => ({
      type: "Control",
      scope: `#/properties/${component}/properties/${field}`,
    })),
  }));
  return { type: "VerticalLayout", elements: groups } as unknown as UISchemaElement;
}
