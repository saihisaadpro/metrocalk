//! Component JSON Schemas — the UI stand-in for `metrocalk-core`'s metadata registry (M1.3). A curated
//! schema exists only for components with a **typed/semantic field** that needs a custom renderer (a
//! `format` the JSON-Forms tester routes — e.g. `color`, `entity-ref`). Everything else is rendered by a
//! **data-driven** schema inferred from the projected component values (M10.10 closeout) — so the inspector
//! renders the REAL `/core` vocabulary (Transform/Health/HealthBar/MeshRenderer · whatever the core sends),
//! not just a hardcoded list. This is the fix for C6 live: the projection carries the values, so the
//! inspector never needs a registry that mirrors every core component.

import type { JsonSchema7, UISchemaElement } from "@jsonforms/core";
import type { Json } from "../transport/protocol";

/** Per-component property schemas for fields that carry a TYPED renderer (a `format`). Plain fields are
 *  inferred from the data instead, so this only needs entries where a custom control must fire. */
export const componentSchemas: Record<string, JsonSchema7> = {
  Transform: {
    type: "object",
    properties: {
      x: { type: "number" },
      y: { type: "number" },
      z: { type: "number" },
    },
  },
  Material: {
    type: "object",
    properties: {
      // `format: color` → the custom ColorControl renderer (typed field, not a bare string box)
      color: { type: "string", format: "color" },
      metalness: { type: "number", minimum: 0, maximum: 1 },
    },
  },
  Health: {
    type: "object",
    properties: {
      max: { type: "integer", minimum: 0 },
      regen: { type: "number" },
    },
  },
  Provides: {
    type: "object",
    properties: {
      // enum → JSON Forms' built-in enum control (the right fit for a closed vocabulary)
      capability: { type: "string", enum: ["Health", "Shield", "Click", "Damage", "Light"] },
    },
  },
  Targeting: {
    type: "object",
    properties: {
      // `format: entity-ref` → the custom EntityRefControl picker (bind-target, ranked by compat)
      target: { type: "string", format: "entity-ref" },
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
 *  (a `format`) is preferred per-field when one exists — so the inspector renders the real `/core`
 *  vocabulary while still routing color/entity-ref fields to their custom controls. */
export function buildEntitySchema(components: Record<string, Record<string, Json>>): JsonSchema7 {
  const properties: Record<string, JsonSchema7> = {};
  for (const [name, fields] of Object.entries(components)) {
    const curated = componentSchemas[name]?.properties as Record<string, JsonSchema7> | undefined;
    const props: Record<string, JsonSchema7> = {};
    for (const [field, value] of Object.entries(fields)) {
      props[field] = curated?.[field] ?? inferField(value);
    }
    properties[name] = { type: "object", properties: props };
  }
  return { type: "object", properties };
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
