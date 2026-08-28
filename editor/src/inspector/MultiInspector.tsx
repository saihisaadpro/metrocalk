//! **The Inspector when more than one thing is selected (ADR-169).**
//!
//! The editor has let a user select many objects since M10.6 — shift-click a range, ctrl-click a set,
//! marquee the viewport — and exactly one verb read that selection (`Group`). The Inspector read
//! `selectedId`, so twelve selected lights showed one light's properties with nothing on screen saying
//! the other eleven were selected at all: change the intensity and eleven objects did not move.
//!
//! Meanwhile the engine could already do this. `capscene::multi_edit` sets one field on N entities as
//! ONE atomic, undoable transaction and has since M10.6; its only route to a user was a hard-coded
//! "Move up 5 m" item in a popup menu.
//!
//! **What this panel is:** the same schema-driven form, over the INTERSECTION of the selection
//! (`shared.ts`), where a field the objects disagree about reads "Mixed" and setting it writes to all
//! of them in one transaction. Everything the form does is the single-object Inspector's behaviour;
//! the two differences are which components are offered and where an edit goes.

import { useState } from "react";
import { JsonForms } from "@jsonforms/react";
import { vanillaCells } from "@jsonforms/vanilla-renderers";
import { useStore } from "zustand";
import { projectionStore } from "../store/projection";
import { pushToast } from "../store/toasts";
import { setStatus } from "../store/ui";
import type { EditorClient } from "../transport/session";
import type { Json } from "../transport/protocol";
import { buildEntitySchema, buildEntityUiSchema } from "../schema/registry";
import { inspectorRenderers, MixedPaths } from "./renderers";
import { describeKind, selectionMakeup, sharedShape, type Components } from "./shared";
import { TransformSection } from "./TransformSection";
import { readTransform, withoutTransform } from "./transform";
import { TypeIcon } from "../theme/primitives";
import { color, font, fontSize, space } from "../theme/tokens";

/** How many object names the header prints before it stops and counts the rest. Three fits the
 *  300px dock at the body size; a fourth wraps, and a wrapped list of names is not a scan. */
const NAMES_SHOWN = 3;

export function MultiInspector({ client, ids }: { client: EditorClient; ids: string[] }) {
  // ONE subscription for the whole selection. `useDisplayedEntity` is per-id and hooks cannot be
  // called in a loop over a varying list, so this component — which is only mounted while a
  // multi-selection exists — reads the map and the single-object Inspector keeps its per-entity
  // subscription untouched (the 5k-tree property this store was built for).
  const displayed = useStore(projectionStore, (s) => s.displayed);
  const summaries = useStore(projectionStore, (s) => s.summaries);
  const [refusal, setRefusal] = useState<string | null>(null);

  const present = ids.filter((id) => displayed[id]);
  const entities: Components[] = present.map((id) => displayed[id].components);
  // ADR-172 — the Transform is drawn by its own section when EVERY selected object carries one.
  // When only some do it stays in the intersection, so the existing partial-field count reports it
  // honestly rather than a second mechanism saying the same thing in different words.
  const allPlaced = present.length > 0 && entities.every((c) => !!c.Transform);
  const transforms = allPlaced ? entities.map((c) => readTransform(c.Transform)) : [];
  const shape = sharedShape(allPlaced ? entities.map(withoutTransform) : entities);
  const makeup = selectionMakeup(present.map((id) => summaries[id]?.kind));
  const names = present.map((id) => displayed[id].name || id);

  const schema = buildEntitySchema(shape.components as Record<string, Record<string, Json>>);
  const hasFields = !!schema.properties && Object.keys(schema.properties).length > 0;

  /** Diff JSON Forms' data against the shared shape and send one batched edit per changed field —
   *  the multi-selection twin of the single-object panel's `emitChanges`.
   *
   *  **THE MOUNT-TIME HAZARD, AND WHY THERE IS NO EXTRA GUARD AGAINST IT.** JSON Forms is entitled to
   *  fill a schema-declared `default` into its data before the first render, and a mixed field's data
   *  is `undefined` — so an injected default would diff as a change and write itself to EVERY selected
   *  object before the user touched anything. The obvious defence, swallowing the first `onChange`,
   *  was written and REMOVED: this configuration does not fire one at mount, so the guard ate the
   *  user's first edit instead of a phantom. What stands in its place is a test that mounts the panel
   *  with a mixed field and requires zero edits — a guard that goes red if the behaviour ever changes,
   *  rather than a mechanism that breaks the panel today to defend against it. */
  function emit(before: Components, after: Components) {
    for (const [component, fields] of Object.entries(after)) {
      for (const [field, value] of Object.entries(fields)) {
        if (before[component]?.[field] === value) continue;
        setRefusal(null);
        // The same "edit <component>.<field>" token the single-object panel sets, so the prompt-40
        // E2E keys on one string for both — with the count, because "edit Light.intensity" on twelve
        // objects and on one are different events and the status line is where a user learns which.
        setStatus(`edit ${present.length}× ${component}.${field}`);
        void client
          .multiEdit(present, component, field, value as Json)
          .then((r) => {
            if (r.ok) {
              // NO TOAST ON SUCCESS, and the reason is the keyboard. An arrow-key nudge on a numeric
              // field commits on every press, so a toast per write would stack one per keystroke —
              // and the result is already visible in the rows themselves, which is where feedback for
              // a direct manipulation belongs. The status line carries the count and the undo.
              setStatus(`${component}.${field} on ${r.changed} · Ctrl-Z to undo`);
              return;
            }
            // AT THE GESTURE, NOT IN THE GUTTER: the sentence lands under the header of the panel the
            // user is looking at, and the toast carries it too. A property control that answers
            // "nothing happened" is the failure this whole path exists to avoid.
            const said = r.reason ?? "the engine refused that edit";
            setRefusal(said);
            setStatus(said);
            pushToast(said, "error");
          })
          .catch((e: unknown) => {
            console.error("multiEdit failed", e);
            setRefusal("that edit could not be sent to the engine");
          });
      }
    }
  }

  /** The whole selection turned to one rotation — ONE transaction, the same refusal grammar as a
   *  batched field edit. `multiEdit` cannot stand in for this: it writes one field to N entities and
   *  a rotation is four fields to N entities. */
  function rotateAll(quat: [number, number, number, number]) {
    setRefusal(null);
    setStatus(`edit ${present.length}x Transform.rotation`);
    void client
      .setRotation(present, quat)
      .then((r) => {
        if (r.ok) {
          setStatus(`rotation on ${r.changed} - Ctrl-Z to undo`);
          return;
        }
        const said = r.reason ?? "the engine refused that rotation";
        setRefusal(said);
        setStatus(said);
        pushToast(said, "error");
      })
      .catch((e: unknown) => {
        console.error("setRotation failed", e);
        setRefusal("that rotation could not be sent to the engine");
      });
  }

  const title =
    makeup.length === 1
      ? describeKind(makeup[0].kind, present.length)
      : `${present.length} objects`;
  const subtitle =
    makeup.length === 1
      ? names.slice(0, NAMES_SHOWN).join(" · ") +
        (names.length > NAMES_SHOWN ? ` · +${names.length - NAMES_SHOWN} more` : "")
      : makeup.map((m) => describeKind(m.kind, m.count)).join(" · ");

  return (
    <div id="inspector" data-testid="inspectorMulti" style={{ padding: space.lg }}>
      <div style={{ display: "flex", alignItems: "center", gap: space.sm, marginBottom: space.md }}>
        {/* One kind → that kind's icon. A mixed selection gets the neutral one rather than the
            primary's, because an icon is a claim about what you are looking at. */}
        <TypeIcon kind={makeup.length === 1 ? makeup[0].kind : "default"} size={24} />
        <div style={{ minWidth: 0 }}>
          <div
            data-testid="multiTitle"
            style={{
              font: font.ui,
              fontSize: fontSize.title,
              fontWeight: 600,
              color: color.text.primary,
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
          >
            {title} selected
          </div>
          <div
            data-testid="multiSubtitle"
            style={{
              font: font.ui,
              fontSize: fontSize.micro,
              color: color.text.muted,
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
            title={names.join(" · ")}
          >
            {subtitle}
          </div>
        </div>
      </div>

      {refusal && (
        <div
          data-testid="multiRefusal"
          role="status"
          style={{
            font: font.ui,
            fontSize: fontSize.body,
            color: color.danger.text,
            marginBottom: space.md,
          }}
        >
          {refusal}
        </div>
      )}

      {allPlaced && (
        <TransformSection
          transforms={transforms}
          selectionKey={present.join(",")}
          onSetPosition={(field, value) => emit({}, { Transform: { [field]: value } } as Components)}
          onSetScale={(value) => emit({}, { Transform: { scale: value } } as Components)}
          onSetRotation={rotateAll}
        />
      )}
      {/* SOMETHING IS EDITABLE when the selection shares fields OR shares a place in the world — the
          Transform section is drawn from its own resolved values, not from the intersection, so
          `hasFields` alone stopped being the question the moment it existed (ADR-172). The partial
          count belongs to this branch and not to the form: it is a statement about the SELECTION. */}
      {hasFields || allPlaced ? (
        <>
          {hasFields && (
          <MixedPaths.Provider value={shape.mixed}>
            <JsonForms
              schema={schema}
              uischema={buildEntityUiSchema(shape.components as Record<string, Record<string, Json>>)}
              data={shape.components}
              renderers={inspectorRenderers}
              cells={vanillaCells}
              onChange={({ data }) =>
                emit(shape.components as Components, data as Components)
              }
            />
          </MixedPaths.Provider>
          )}
          {shape.partialFields > 0 && (
            // PROGRESSIVE DISCLOSURE, AND AN HONEST COUNT. The fields only part of the selection
            // carries are not editable here — `engine.commit` is all-or-nothing and `Op::SetField`
            // CREATES a component it does not find, so offering one would either refuse the whole
            // batch or invent a half-built component on the objects that lack it. Saying so, with the
            // number and the next step, is the difference between a filtered panel and a lying one.
            <div
              data-testid="multiPartial"
              style={{
                font: font.ui,
                fontSize: fontSize.micro,
                color: color.text.muted,
                marginTop: space.md,
              }}
            >
              {shape.partialFields} propert{shape.partialFields === 1 ? "y is" : "ies are"} on only
              some of these
              {shape.partialComponents.length > 0
                ? ` (${shape.partialComponents.join(", ")})`
                : ""}{" "}
              — select one object to edit {shape.partialFields === 1 ? "it" : "them"}.
            </div>
          )}
        </>
      ) : (
        <div
          data-testid="multiEmpty"
          style={{ color: color.text.muted, fontSize: fontSize.body, padding: `${space.md}px 0` }}
        >
          These {present.length} objects have no properties in common — select one to edit its own.
        </div>
      )}
    </div>
  );
}
