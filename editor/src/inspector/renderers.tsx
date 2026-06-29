//! Custom JSON Forms renderers for Metrocalk's typed/semantic fields, registered via **testers**
//! (the JSON-Forms model that fits typed fields — vs RJSF's template model). Each renderer is matched
//! by a tester keyed on the field's `format`; everything else falls through to the vanilla renderers.

import { and, isIntegerControl, isNumberControl, isStringControl, or, rankWith, schemaMatches, type ControlProps } from "@jsonforms/core";
import { withJsonFormsControlProps } from "@jsonforms/react";
import { useEffect, useState } from "react";
import { useStore } from "zustand";
import { projectionStore } from "../store/projection";

const hasFormat = (fmt: string) =>
  schemaMatches((s) => (s as { format?: string }).format === fmt);

// ── numeric (typed) — NEVER silently coerce bad input to 0 (data-integrity) ───────────────────
// The vanilla JSON-Forms number cell turns non-numeric input into 0 and commits it, silently zeroing
// the field. This renderer keeps local text so partial input ("-", "1.") doesn't emit, rejects
// non-numeric (no emit → the committed value stands) with a visible invalid state, and blurs on Enter so
// the next Ctrl-Z is a SCENE undo (not the input's text undo).
function NumberControlBase({ data, handleChange, path, label, schema, enabled }: ControlProps) {
  const isInt = (schema as { type?: string })?.type === "integer";
  const [text, setText] = useState(data == null ? "" : String(data));
  // Resync when the projection changes the value externally (an authoritative delta, undo, reselect).
  useEffect(() => {
    setText(data == null ? "" : String(data));
  }, [data]);
  const parsed = text.trim() === "" ? null : Number(text);
  const valid = parsed !== null && Number.isFinite(parsed) && (!isInt || Number.isInteger(parsed));
  return (
    <label style={{ display: "flex", gap: 8, alignItems: "center", margin: "4px 0" }}>
      <span style={{ minWidth: 90 }}>{label}</span>
      <input
        type="text"
        inputMode={isInt ? "numeric" : "decimal"}
        disabled={enabled === false}
        value={text}
        title={text.trim() !== "" && !valid ? `Enter a ${isInt ? "whole number" : "number"} — this value was not applied` : undefined}
        onChange={(e) => {
          const v = e.target.value;
          setText(v);
          if (v.trim() === "") return; // empty is not 0 — emit nothing (no silent zeroing)
          const n = Number(v);
          if (Number.isFinite(n) && (!isInt || Number.isInteger(n))) handleChange(path, n); // valid only
          // invalid → DO NOT emit; the prior committed value stands, and the field shows the invalid state
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") (e.target as HTMLInputElement).blur(); // commit + release focus → Ctrl-Z = scene undo
        }}
        style={{
          flex: 1,
          background: "#0a0c12",
          color: valid || text.trim() === "" ? "#cde" : "#f9a8a8",
          border: `1px solid ${valid || text.trim() === "" ? "#2a3550" : "#a33"}`,
          borderRadius: 4,
          font: "12px ui-monospace, monospace",
          padding: "2px 6px",
        }}
      />
    </label>
  );
}
export const NumberControl = withJsonFormsControlProps(NumberControlBase);
// Rank above the vanilla number/integer cell, below the format-keyed controls (color/entity-ref are
// string controls, so they never collide with these numeric testers).
export const numberTester = rankWith(6, or(isNumberControl, isIntegerControl));

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
