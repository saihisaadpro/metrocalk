//! Custom JSON Forms renderers for Metrocalk's typed/semantic fields, registered via **testers**
//! (the JSON-Forms model that fits typed fields — vs RJSF's template model). Each renderer is matched
//! by a tester keyed on the field's `format`; everything else falls through to the vanilla renderers.
//!
//! **M14.3 (ADR-059):** the numeric control is now the scrub-capable `NumericField` (drag-to-scrub +
//! keyboard nudge + type-to-set); a scrub-drag coalesces into ONE undoable transaction at pointer-up
//! (`handleChange` → the Inspector's `emitChanges` → `client.setField`, the ADR-010 optimistic-echo path —
//! NOT a commit per move). The components group under a **collapsible** section layout.

import {
  and,
  isIntegerControl,
  isNumberControl,
  isStringControl,
  or,
  rankWith,
  schemaMatches,
  uiTypeIs,
  type ControlProps,
  type LayoutProps,
  type UISchemaElement,
} from "@jsonforms/core";
import { JsonFormsDispatch, withJsonFormsControlProps, withJsonFormsLayoutProps } from "@jsonforms/react";
import { useState } from "react";
import { useStore } from "zustand";
import { projectionStore } from "../store/projection";
import { NumericField } from "../theme/primitives";

const hasFormat = (fmt: string) =>
  schemaMatches((s) => (s as { format?: string }).format === fmt);

// ── numeric (typed) — scrub-to-edit, NEVER silently coerce bad input to 0 (data-integrity) ──────────────
// The vanilla JSON-Forms number cell turns non-numeric input into 0 and commits it, silently zeroing the
// field. The `NumericField` keeps local text (partial input "-"/"1." doesn't emit), reverts invalid input
// (no emit → the committed value stands), and — the M14.3 level-up — supports drag-to-scrub + keyboard
// nudge, COALESCING a whole scrub into ONE `handleChange` at pointer-up (one undo step, the ADR-010 tx).
function NumberControlBase({ data, handleChange, path, label, schema, enabled }: ControlProps) {
  const isInt = (schema as { type?: string })?.type === "integer";
  const num = typeof data === "number" ? data : Number(data);
  const value = Number.isFinite(num) ? num : 0;
  // An "unbound/default" cue: the field has no concrete projected value yet (the C6 default state).
  const unbound = data === undefined || data === null;
  return (
    <label className="mtk-field-row">
      <span className="mtk-field-label">{label}</span>
      <NumericField
        value={value}
        integer={isInt}
        step={isInt ? 1 : 0.1}
        disabled={enabled === false}
        invalid={unbound}
        title={unbound ? `${label}: no value set yet` : undefined}
        ariaLabel={label}
        data-testid={`num-${path}`}
        onCommit={(v) => handleChange(path, v)}
        style={{ flex: 1, width: "auto" }}
      />
    </label>
  );
}
export const NumberControl = withJsonFormsControlProps(NumberControlBase);
// Rank above the vanilla number/integer cell, below the format-keyed controls (color/entity-ref are
// string controls, so they never collide with these numeric testers).
export const numberTester = rankWith(6, or(isNumberControl, isIntegerControl));

// ── collapsible component group (the inspector's section layout) ─────────────────────────────────────────
// Replaces the vanilla GroupLayout: a section header per component (object identity · Transform · rendering ·
// health bar · other) that collapses, so a dense inspector stays scannable. Renders its children through the
// standard `JsonFormsDispatch` so every control (numeric/color/entity-ref) still resolves by tester.
function GroupLayoutBase({ uischema, schema, path, renderers, cells, enabled, visible }: LayoutProps) {
  const group = uischema as unknown as { label?: string; elements: UISchemaElement[] };
  const [open, setOpen] = useState(true);
  if (visible === false) return null;
  return (
    <div className="mtk-group" data-testid="inspectorGroup" data-group={group.label}>
      <button type="button" className="mtk-group-head" onClick={() => setOpen((o) => !o)} aria-expanded={open}>
        <span className={"mtk-group-caret" + (open ? " is-open" : "")}>▸</span>
        {group.label ?? "Component"}
      </button>
      {open && (
        <div className="mtk-group-body">
          {group.elements.map((el, i) => (
            <JsonFormsDispatch
              key={i}
              uischema={el}
              schema={schema}
              path={path}
              renderers={renderers}
              cells={cells}
              enabled={enabled}
            />
          ))}
        </div>
      )}
    </div>
  );
}
export const CollapsibleGroup = withJsonFormsLayoutProps(GroupLayoutBase);
export const groupTester = rankWith(5, uiTypeIs("Group"));

// ── color (typed field, not a bare text box) ──────────────────────────────────
function ColorControlBase({ data, handleChange, path, label, enabled }: ControlProps) {
  return (
    <label className="mtk-field-row">
      <span className="mtk-field-label">{label}</span>
      <input
        type="color"
        disabled={enabled === false}
        value={typeof data === "string" ? data : "#ffffff"}
        onChange={(e) => handleChange(path, e.target.value)}
        style={{ width: 36, height: 22, background: "transparent", border: "1px solid var(--mtk-border)", borderRadius: 4, cursor: "pointer" }}
      />
    </label>
  );
}
export const ColorControl = withJsonFormsControlProps(ColorControlBase);
export const colorTester = rankWith(10, and(isStringControl, hasFormat("color")));

// ── entity-ref / bind-target picker (ranked by the compat query — stubbed alphabetical here) ──
function EntityRefControlBase({ data, handleChange, path, label }: ControlProps) {
  // Reads the live summary projection for the candidate list. The real ranking is the ECS
  // compatibility query (M1.5); here it's name-sorted with that hook documented.
  const summaries = useStore(projectionStore, (s) => s.summaries);
  const options = Object.values(summaries)
    .sort((a, b) => a.name.localeCompare(b.name))
    .slice(0, 200); // never dump 5k into a <select>; the graph is the at-scale picker
  return (
    <label className="mtk-field-row">
      <span className="mtk-field-label">{label}</span>
      <select
        className="mtk-input"
        value={typeof data === "string" ? data : ""}
        onChange={(e) => handleChange(path, e.target.value)}
        style={{ flex: 1 }}
      >
        <option value="">— none —</option>
        {options.map((o) => (
          <option key={o.id} value={o.id}>
            {o.name}
          </option>
        ))}
      </select>
    </label>
  );
}
export const EntityRefControl = withJsonFormsControlProps(EntityRefControlBase);
export const entityRefTester = rankWith(10, and(isStringControl, hasFormat("entity-ref")));
