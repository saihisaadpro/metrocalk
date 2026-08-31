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
//!
//! **ADR-169 — and when MORE THAN ONE thing is selected.** This file is the single-object form plus the
//! one-line decision of which form to show. The multi-selection form is a separate component and not a
//! branch inside this one, because the two subscribe differently: this one watches `displayed[id]` and
//! must keep doing so (a field edit on one object never re-renders against a 5k tree), while the other
//! has to watch the whole map. Two components, two hook sets, one seam.

import { JsonForms } from "@jsonforms/react";
import { vanillaCells } from "@jsonforms/vanilla-renderers";
import { useSelectedId, useDisplayedEntity, useMultiSelect, useSummary } from "../store/projection";
import { setStatus } from "../store/ui";
import type { EditorClient } from "../transport/session";
import type { Json } from "../transport/protocol";
import { buildEntitySchema, buildEntityUiSchema } from "../schema/registry";
import { inspectorRenderers } from "./renderers";
import { MultiInspector } from "./MultiInspector";
import { TransformSection } from "./TransformSection";
import { readTransform, withoutTransform } from "./transform";
import { pushToast } from "../store/toasts";
import { TypeIcon } from "../theme/primitives";
import { Icon } from "../theme/icons";
import { EmptyPanelState } from "../theme/workspace";
import { InspectorEmpty } from "./InspectorEmpty";
import { color, font, fontSize, space } from "../theme/tokens";

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

/** The panel the Inspector dock mounts: one object, or the selection.
 *
 *  The multi-selection is ORDERED PRIMARY-FIRST before it is handed on. `multiSelect` keeps the order
 *  the user built the selection in and the primary is wherever the last click put it; the panel uses
 *  the first entry to fix which object's component and field ORDER the form follows, so a form that
 *  re-ordered itself because a different object happened to be first would be a defect that only ever
 *  showed up on a shift-click at one end of a range. */
export function Inspector({ client }: { client: EditorClient }) {
  const primary = useSelectedId();
  const multi = useMultiSelect();
  if (multi.length >= 2) {
    const ordered = primary ? [primary, ...multi.filter((i) => i !== primary)] : multi;
    return <MultiInspector client={client} ids={ordered} />;
  }
  return <SingleInspector client={client} />;
}

function SingleInspector({ client }: { client: EditorClient }) {
  const id = useSelectedId();
  const entity = useDisplayedEntity(id ?? "");
  const summary = useSummary(id ?? "");
  // The state this panel is in most often, and the one it had never been designed for — see
  // `InspectorEmpty` for what replaced the one grey sentence that used to be here.
  if (!id || !entity) return <InspectorEmpty />;
  // ADR-172 — `Transform` is drawn by its own section, not by the data-driven form. It is the one
  // component whose STORAGE and whose PROPERTIES are different shapes: eight sparse scalars on the
  // wire, three properties on screen, one of them (rotation) a quaternion nobody types.
  const rest = withoutTransform(entity.components);
  const schema = buildEntitySchema(rest);
  const transform = entity.components.Transform ? readTransform(entity.components.Transform) : null;
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
      {transform && (
        <TransformSection
          transforms={[transform]}
          selectionKey={id}
          onSetPosition={(field, value) => {
            client.setField(id, "Transform", field, value);
            setStatus(`edit Transform.${field}`);
          }}
          onSetScale={(value) => {
            client.setField(id, "Transform", "scale", value);
            setStatus("edit Transform.scale");
          }}
          onSetRotation={(quat) => {
            setStatus("edit Transform.rotation");
            void client
              .setRotation([id], quat)
              .then((r) => {
                if (r.ok) {
                  setStatus("rotation set · Ctrl-Z to undo");
                  return;
                }
                // The engine's own sentence, at the control — a rotation can be refused (a target
                // with no place in the world, four numbers that are not a rotation) and "nothing
                // happened" is the one answer a property field must never give.
                const said = r.reason ?? "the engine refused that rotation";
                setStatus(said);
                pushToast(said, "error");
              })
              .catch((e: unknown) => console.error("setRotation failed", e));
          }}
        />
      )}
      {hasFields ? (
        <JsonForms
          schema={schema}
          uischema={buildEntityUiSchema(rest)}
          data={rest}
          renderers={inspectorRenderers}
          cells={vanillaCells}
          onChange={({ data }) => emitChanges(client, id, rest, data as Components)}
        />
      ) : (
        // THE OTHER EMPTY STATE, AND THE SENTENCE THAT NAMED A CONTROL THAT DOES NOT EXIST. "add a
        // component to this object" instructed the reader to press something no surface in this editor
        // offers: `/core` has a `RemoveComponent` op and no `AddComponent` one, and nothing in
        // `editor/src` has ever been able to emit one. What CAN add fields to an object is the entity's
        // own action list (`entityActions` → the context menu: make dynamic, add a joint, give it a
        // role), so that is what it points at now — the same anatomy as the no-selection state one
        // branch up, because two empty states in one panel that look different is how this began.
        //
        // Guarded on `!transform` (ADR-172): a Transform-only object HAS an editable property on
        // screen — the section above it — so "no editable properties" would be contradicted by the
        // control directly above the sentence.
        !transform && (
          <EmptyPanelState
            data-testid="inspectorNoFields"
            compact
            icon={<Icon name="properties" size="xl" />}
            title="No editable properties"
            description="Nothing on this object exposes an editable field yet. Right-click it for what can be added."
          />
        )
      )}
    </div>
  );
}
