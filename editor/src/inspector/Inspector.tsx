//! Schema-driven inspector — renders editable forms from the registry's JSON Schema via JSON Forms,
//! with custom renderers for the typed fields. A field edit produces an optimistic store update +
//! a JSON-Patch transaction (`client.setField`) — the same language the AI layer emits.
//!
//! It subscribes only to `displayed[selectedId]`, so it re-renders on that entity's deltas and never
//! on the 5k tree (verified in `store/projection.test.tsx`).
//!
//! **M14.3 (ADR-059) — a real properties editor:** components group under a **collapsible** section layout
//! (`CollapsibleGroup`), every raw input is the M14.1 styled `NumericField` (now **scrub-to-edit** —
//! drag/keyboard/type, each commit one ADR-010 optimistic transaction, a scrub coalescing to one undo step),
//! a header names the object (icon + mono id), and a true empty-state replaces a blank pane.

import { JsonForms } from "@jsonforms/react";
import { vanillaCells, vanillaRenderers } from "@jsonforms/vanilla-renderers";
import { useSelectedId, useDisplayedEntity, useSummary } from "../store/projection";
import { setStatus } from "../store/ui";
import type { EditorClient } from "../transport/session";
import type { Json } from "../transport/protocol";
import { buildEntitySchema, buildEntityUiSchema } from "../schema/registry";
import {
  ColorControl,
  colorTester,
  CollapsibleGroup,
  groupTester,
  EntityRefControl,
  entityRefTester,
  NumberControl,
  numberTester,
} from "./renderers";
import { TypeIcon } from "../theme/primitives";
import { color, font, fontSize, space } from "../theme/tokens";

const renderers = [
  ...vanillaRenderers,
  { tester: colorTester, renderer: ColorControl },
  { tester: entityRefTester, renderer: EntityRefControl },
  { tester: numberTester, renderer: NumberControl },
  { tester: groupTester, renderer: CollapsibleGroup },
];

type Components = Record<string, Record<string, Json>>;

/** Diff the JSON Forms data against the projected components and emit one `setField` per changed
 *  field. Diffing against the projection means the mount-time onChange (data === projection) is a
 *  no-op, so we never echo our own state back as an edit. A scrub-drag commits a single `handleChange`
 *  at pointer-up, so this emits exactly ONE `setField` for the whole drag (one undo step). */
function emitChanges(client: EditorClient, id: string, before: Components, after: Components) {
  for (const [component, fields] of Object.entries(after)) {
    for (const [field, value] of Object.entries(fields)) {
      if (before[component]?.[field] !== value) {
        client.setField(id, component, field, value as Json);
        // a stable "edit <component>.<field>" token the prompt-40 E2E keys on (intentional, not cosmetic)
        setStatus(`edit ${component}.${field}`);
      }
    }
  }
}

export function Inspector({ client }: { client: EditorClient }) {
  const id = useSelectedId();
  const entity = useDisplayedEntity(id ?? "");
  const summary = useSummary(id ?? "");
  if (!id || !entity) {
    return (
      <div id="inspector" style={{ padding: space.lg, color: color.text.muted, fontSize: fontSize.body }}>
        Select an entity to inspect.
      </div>
    );
  }
  const schema = buildEntitySchema(entity.components);
  // A real empty-state (C6) — never a blank pane: when the entity carries no *editable* (schema-backed)
  // properties, say so + name the next step, rather than rendering nothing beside the header.
  const hasFields = !!schema.properties && Object.keys(schema.properties).length > 0;
  const kind = summary?.kind ?? "default";
  const named = !!entity.name && entity.name !== id;
  return (
    <div id="inspector" style={{ padding: space.lg }}>
      <div style={{ display: "flex", alignItems: "center", gap: space.sm, marginBottom: space.md }}>
        <TypeIcon kind={kind} size={24} />
        <div style={{ minWidth: 0 }}>
          <div style={{ font: font.ui, fontSize: fontSize.title, fontWeight: 600, color: color.text.primary, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {named ? entity.name : id}
          </div>
          <div style={{ font: font.mono, fontSize: fontSize.micro, color: color.text.muted }}>{id}</div>
        </div>
      </div>
      {hasFields ? (
        <JsonForms
          schema={schema}
          uischema={buildEntityUiSchema(entity.components)}
          data={entity.components}
          renderers={renderers}
          cells={vanillaCells}
          onChange={({ data }) => emitChanges(client, id, entity.components, data as Components)}
        />
      ) : (
        <div data-testid="inspectorEmpty" style={{ color: color.text.muted, fontSize: fontSize.body, padding: `${space.md}px 0` }}>
          No editable properties yet — add a component to this object.
        </div>
      )}
    </div>
  );
}
