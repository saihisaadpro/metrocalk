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
import { vanillaCells } from "@jsonforms/vanilla-renderers";
import { useSelectedId, useDisplayedEntity, useSummary } from "../store/projection";
import { setStatus } from "../store/ui";
import type { EditorClient } from "../transport/session";
import type { Json } from "../transport/protocol";
import { buildEntitySchema, buildEntityUiSchema } from "../schema/registry";
import {
  AssetRefControl,
  assetRefTester,
  BooleanControl,
  booleanTester,
  CollapsibleGroup,
  groupTester,
  EntityRefControl,
  entityRefTester,
  EnumControl,
  enumTester,
  NumberControl,
  numberTester,
  StringControl,
  stringTester,
  VerticalLayout,
  verticalLayoutTester,
} from "./renderers";
import { TypeIcon } from "../theme/primitives";
import { Icon } from "../theme/icons";
import { EmptyPanelState } from "../theme/workspace";
import { InspectorEmpty } from "./InspectorEmpty";
import { color, font, fontSize, space } from "../theme/tokens";

/** **`vanillaRenderers` IS DELIBERATELY NOT HERE (ADR-136).** It used to be spread in as the fallback,
 *  and it is what every boolean, plain string and vocabulary field in the inspector actually rendered
 *  through — emitting `.control`, `.input`, `.select` and `.checkbox`, generic class names this
 *  repository's stylesheet has no rules for and that `check-class-hooks.mjs` cannot see, because they
 *  come out of `node_modules` rather than out of the markup it reads. The set below covers every scalar
 *  `FieldType` the core can register (Number · Integer · Boolean · String, plus the `format` and
 *  vocabulary refinements), so there is nothing left for a fallback to catch — and its absence is what
 *  lets a `shots` scene assert those four class names are not on the page. `vanillaCells` stays: cells
 *  are the array/table path, which this inspector does not use, and JsonForms wants a non-empty list. */
const renderers = [
  { tester: stringTester, renderer: StringControl },
  { tester: numberTester, renderer: NumberControl },
  { tester: booleanTester, renderer: BooleanControl },
  { tester: enumTester, renderer: EnumControl },
  { tester: assetRefTester, renderer: AssetRefControl },
  { tester: entityRefTester, renderer: EntityRefControl },
  { tester: groupTester, renderer: CollapsibleGroup },
  { tester: verticalLayoutTester, renderer: VerticalLayout },
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
  // The state this panel is in most often, and the one it had never been designed for — see
  // `InspectorEmpty` for what replaced the one grey sentence that used to be here.
  if (!id || !entity) return <InspectorEmpty />;
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
        // THE OTHER EMPTY STATE, AND THE SENTENCE THAT NAMED A CONTROL THAT DOES NOT EXIST. "add a
        // component to this object" instructed the reader to press something no surface in this editor
        // offers: `/core` has a `RemoveComponent` op and no `AddComponent` one, and nothing in
        // `editor/src` has ever been able to emit one. What CAN add fields to an object is the entity's
        // own action list (`entityActions` → the context menu: make dynamic, add a joint, give it a
        // role), so that is what it points at now — the same anatomy as the no-selection state one
        // branch up, because two empty states in one panel that look different is how this began.
        <EmptyPanelState
          data-testid="inspectorNoFields"
          compact
          icon={<Icon name="properties" size="xl" />}
          title="No editable properties"
          description="Nothing on this object exposes an editable field yet. Right-click it for what can be added."
        />
      )}
    </div>
  );
}
