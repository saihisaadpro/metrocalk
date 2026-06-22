//! Schema-driven inspector — renders editable forms from the registry's JSON Schema via JSON Forms,
//! with custom renderers for the typed fields. A field edit produces an optimistic store update +
//! a JSON-Patch transaction (`client.setField`) — the same language the AI layer emits.
//!
//! It subscribes only to `displayed[selectedId]`, so it re-renders on that entity's deltas and never
//! on the 5k tree (verified in `store/projection.test.tsx`).

import { JsonForms } from "@jsonforms/react";
import { vanillaCells, vanillaRenderers } from "@jsonforms/vanilla-renderers";
import { useSelectedId, useDisplayedEntity } from "../store/projection";
import type { EditorClient } from "../transport/session";
import type { Json } from "../transport/protocol";
import { buildEntitySchema } from "../schema/registry";
import { ColorControl, colorTester, EntityRefControl, entityRefTester } from "./renderers";

const renderers = [
  ...vanillaRenderers,
  { tester: colorTester, renderer: ColorControl },
  { tester: entityRefTester, renderer: EntityRefControl },
];

type Components = Record<string, Record<string, Json>>;

/** Diff the JSON Forms data against the projected components and emit one `setField` per changed
 *  field. Diffing against the projection means the mount-time onChange (data === projection) is a
 *  no-op, so we never echo our own state back as an edit. */
function emitChanges(client: EditorClient, id: string, before: Components, after: Components) {
  for (const [component, fields] of Object.entries(after)) {
    for (const [field, value] of Object.entries(fields)) {
      if (before[component]?.[field] !== value) {
        client.setField(id, component, field, value as Json);
      }
    }
  }
}

export function Inspector({ client }: { client: EditorClient }) {
  const id = useSelectedId();
  const entity = useDisplayedEntity(id ?? "");
  if (!id || !entity) {
    return (
      <div id="inspector" style={{ padding: 12, color: "#888" }}>
        Select an entity to inspect.
      </div>
    );
  }
  const schema = buildEntitySchema(entity.components);
  // A real empty-state (C6) — never a blank pane: when the entity carries no *editable* (schema-backed)
  // properties, say so + name the next step, rather than rendering nothing beside the header.
  const hasFields = !!schema.properties && Object.keys(schema.properties).length > 0;
  return (
    <div id="inspector" style={{ padding: 12 }}>
      <div style={{ fontWeight: 700, marginBottom: 8 }}>{entity.name}</div>
      {hasFields ? (
        <JsonForms
          schema={schema}
          data={entity.components}
          renderers={renderers}
          cells={vanillaCells}
          onChange={({ data }) => emitChanges(client, id, entity.components, data as Components)}
        />
      ) : (
        <div data-testid="inspectorEmpty" style={{ color: "#888", fontSize: 12 }}>
          No editable properties yet — add a component to this object.
        </div>
      )}
    </div>
  );
}
