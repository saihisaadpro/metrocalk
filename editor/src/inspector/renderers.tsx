//! Custom JSON Forms renderers for Metrocalk's typed/semantic fields, registered via **testers**
//! (the JSON-Forms model that fits typed fields — vs RJSF's template model). Each renderer is matched
//! by a tester keyed on the field's `format`; everything else falls through to the vanilla renderers.

import { and, isStringControl, rankWith, schemaMatches, type ControlProps } from "@jsonforms/core";
import { withJsonFormsControlProps } from "@jsonforms/react";
import { useStore } from "zustand";
import { projectionStore } from "../store/projection";

const hasFormat = (fmt: string) =>
  schemaMatches((s) => (s as { format?: string }).format === fmt);

// ── color (typed field, not a bare text box) ──────────────────────────────────
function ColorControlBase({ data, handleChange, path, label, enabled }: ControlProps) {
  return (
    <label style={{ display: "flex", gap: 8, alignItems: "center", margin: "4px 0" }}>
      <span style={{ minWidth: 90 }}>{label}</span>
      <input
        type="color"
        disabled={enabled === false}
        value={typeof data === "string" ? data : "#ffffff"}
        onChange={(e) => handleChange(path, e.target.value)}
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
    <label style={{ display: "flex", gap: 8, alignItems: "center", margin: "4px 0" }}>
      <span style={{ minWidth: 90 }}>{label}</span>
      <select value={typeof data === "string" ? data : ""} onChange={(e) => handleChange(path, e.target.value)}>
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
