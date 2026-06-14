//! Component JSON Schemas — the UI stand-in for `metrocalk-core`'s metadata registry (M1.3). In
//! production the inspector pulls these from the core; here they're inline. Typed/semantic fields
//! carry a `format` so JSON Forms can route them to a custom renderer via a tester (the reason for
//! JSON Forms over RJSF's template model — see the ADR/layers note).

import type { JsonSchema7 } from "@jsonforms/core";

/** Per-component property schemas, keyed by component name. */
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

/** Build a combined JSON Schema for an entity from the components it actually has. */
export function buildEntitySchema(components: Record<string, unknown>): JsonSchema7 {
  const properties: Record<string, JsonSchema7> = {};
  for (const name of Object.keys(components)) {
    if (componentSchemas[name]) properties[name] = componentSchemas[name];
  }
  return { type: "object", properties };
}
