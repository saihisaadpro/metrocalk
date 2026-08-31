//! Custom JSON Forms renderers for Metrocalk's typed/semantic fields, registered via **testers**
//! (the JSON-Forms model that fits typed fields — vs RJSF's template model). Each renderer is matched
//! by a tester keyed on the field's `format`, its closed vocabulary, or its scalar type.
//!
//! **M14.3 (ADR-059):** the numeric control is the scrub-capable `NumericField` (drag-to-scrub +
//! keyboard nudge + type-to-set); a scrub-drag coalesces into ONE undoable transaction at pointer-up
//! (`handleChange` → the Inspector's `emitChanges` → `client.setField`, the ADR-010 optimistic-echo path —
//! NOT a commit per move). The components group under a **collapsible** section layout.
//!
//! **ADR-136 — ONE ANATOMY, AND ONE THAT CAN ACTUALLY FIRE.** Two things were wrong here and they
//! reinforced each other.
//!
//! *The anatomy.* Three of these renderers wrapped their control in a bespoke `.mtk-field-row` — a flex
//! line with a hard 92px label — and **everything else fell through to `@jsonforms/vanilla-renderers`**,
//! which emits `.control`, `.input`, `.select` and `.checkbox`: generic, un-namespaced class names that
//! this repository's stylesheet has never had a rule for, and that `check-class-hooks.mjs` cannot see
//! because they are emitted from `node_modules`. So the editor's primary property surface was three
//! design languages deep, and the design system's own `PropertyRow` — label-left / control-right, with
//! an actions slot, a help line, a ≤760px stacking rule and a coarse-pointer target floor — was used by
//! exactly one panel, and not by the inspector it was written for. Every control below is now a
//! `PropertyRow` over a `theme/primitives` control, and `vanillaRenderers` is no longer registered:
//! a scene can therefore assert that `.control`, `.input`, `.select` and `.checkbox` are **absent**,
//! which is the only form of "one anatomy" a gate can check.
//!
//! *What could fire at all.* `ColorControl` was keyed on `format: "color"` and `EntityRefControl` on
//! `Targeting.target`, both declared in a table whose components the core has never registered. The
//! entity-ref picker needed no code change — repairing the table (`schema/registry.ts`) pointed it at
//! `Joint.bodyA`/`bodyB`, the engine's real references, and it fires. The colour picker had nowhere to
//! go: the core models colour as three separate Numbers (`Light.r/g/b`, `UiStyle.r/g/b`,
//! `Sprite.tintR..A`) and registers **no `format: color` field at all**, so a control keyed on that
//! format is `mesh_frame_bench.rs` — compiling, tested, and unreachable. It is deleted rather than
//! kept, and `check-registry-vocab.mjs` is the tripwire that replaces it: the day the core publishes a
//! colour field, the gate goes red for an unrouted format until a renderer exists.

import {
  and,
  isBooleanControl,
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
import { createContext, useContext, type ReactNode } from "react";
import { useStore } from "zustand";
import { projectionStore } from "../store/projection";
import { Icon } from "../theme/icons";
import { Button, NumericField, PropertyRow, SelectField, TextField } from "../theme/primitives";
import { DisclosureSection } from "../theme/workspace";

/** ADR-169 — the field paths (`Component.field`) a MULTI-SELECTION disagrees about.
 *
 *  A context rather than a prop because the controls are reached through JSON Forms' dispatcher,
 *  which owns the props it passes down and has no slot for "and by the way this is a selection".
 *  Empty for a single object, which is why every control below behaves exactly as it did before.
 *
 *  WHY THE CONTROL, NOT THE PANEL, HAS TO KNOW. "Mixed" is not a value — it is the absence of one
 *  — and each control shows the absence of a value differently: an empty box with a placeholder,
 *  a select with an extra option, a checkbox in its indeterminate state. A panel-level banner saying
 *  "some of these differ" would leave every row still printing the primary's value as though it
 *  were everyone's, which is the untruth this whole change is about. */
export const MixedPaths = createContext<ReadonlySet<string>>(new Set<string>());

/** Does the current selection disagree about this control's field? */
const useMixed = (path: string): boolean => useContext(MixedPaths).has(path);

const hasFormat = (fmt: string) =>
  schemaMatches((s) => (s as { format?: string }).format === fmt);

const isEnum = schemaMatches((s) => Array.isArray((s as { enum?: unknown[] }).enum));

/** A stable DOM id per field, so a `PropertyRow`'s visible label can point at the control it names.
 *  Derived from the JSON-Forms `path` (`Transform.px`), which is already unique per row. */
const controlId = (path: string) => `mtk-prop-${path.replace(/[^\w-]/g, "-")}`;

/** The shared row every control below renders into — the whole point of ADR-136, in one function.
 *
 *  `unit` and `default` come off the curated schema (`schema/registry.ts`), which is itself gated
 *  against `core/src/stdlib.rs`. The reset appears ONLY when the schema declares a default AND the
 *  committed value differs from it: a control that is always there and usually does nothing is a
 *  control a user learns to ignore, and it would also be a lie on a field the engine has no default
 *  for. `data === undefined` is unbound, not "differs from the default", and offers no reset either. */
function Row({
  path,
  label,
  description,
  schema,
  data,
  handleChange,
  children,
}: Pick<ControlProps, "path" | "label" | "description" | "schema" | "data" | "handleChange"> & {
  children: ReactNode;
}) {
  const { unit, default: dflt } = schema as { unit?: string; default?: unknown };
  const mixed = useMixed(path);
  // A MIXED field has no `data` (that is what mixed means here), and "set every one of them back to
  // the default" is precisely the action a reset is for — so the row offers it, where an unbound
  // single field still does not.
  const resettable =
    dflt !== undefined && (mixed || (data !== undefined && data !== null && data !== dflt));
  return (
    <PropertyRow
      htmlFor={controlId(path)}
      label={label}
      help={description || undefined}
      // The unit is the design system's own column, not a suffix inside the control — otherwise a
      // `kg` row's input is narrower than a `×` row's and the sheet's right edge goes ragged.
      unit={unit}
      data-testid="prop-row"
      actions={
        resettable ? (
          <Button
            variant="ghost"
            icon
            compact
            aria-label={`Reset ${label} to ${String(dflt)}`}
            title={`Reset to ${String(dflt)}`}
            data-testid="prop-reset"
            onClick={() => handleChange(path, dflt)}
          >
            <Icon name="revert" size={13} />
          </Button>
        ) : undefined
      }
    >
      {children}
    </PropertyRow>
  );
}

/** The accessible name a control carries: the label plus its unit, because "Position X" and "Position X
 *  metres" are different questions and only the second one is answerable without looking. */
const named = (label: string, schema: unknown) => {
  const unit = (schema as { unit?: string }).unit;
  return unit ? `${label} (${unit})` : label;
};

// ── numeric (typed) — scrub-to-edit, NEVER silently coerce bad input to 0 (data-integrity) ──────────────
// The vanilla JSON-Forms number cell turns non-numeric input into 0 and commits it, silently zeroing the
// field. The `NumericField` keeps local text (partial input "-"/"1." doesn't emit), reverts invalid input
// (no emit → the committed value stands), and — the M14.3 level-up — supports drag-to-scrub + keyboard
// nudge, COALESCING a whole scrub into ONE `handleChange` at pointer-up (one undo step, the ADR-010 tx).
function NumberControlBase(props: ControlProps) {
  const { data, handleChange, path, label, schema, enabled } = props;
  const isInt = (schema as { type?: string })?.type === "integer";
  const num = typeof data === "number" ? data : Number(data);
  const value = Number.isFinite(num) ? num : 0;
  // An "unbound/default" cue: the field has no concrete projected value yet (the C6 default state).
  const mixed = useMixed(path);
  const unbound = !mixed && (data === undefined || data === null);
  const { minimum, maximum } = schema as { minimum?: number; maximum?: number };
  return (
    <Row {...props}>
      <NumericField
        id={controlId(path)}
        value={value}
        integer={isInt}
        step={isInt ? 1 : 0.1}
        min={minimum}
        max={maximum}
        disabled={enabled === false}
        invalid={unbound}
        mixed={mixed}
        title={
          mixed
            ? `${label}: the selected objects differ — type a value to set all of them`
            : unbound
              ? `${label}: no value set yet`
              : undefined
        }
        ariaLabel={named(label, schema)}
        data-testid={`num-${path}`}
        onCommit={(v) => handleChange(path, v)}
        style={{ width: "100%" }}
      />
    </Row>
  );
}
export const NumberControl = withJsonFormsControlProps(NumberControlBase);
// Rank above the vanilla number/integer cell, below the format-keyed controls (asset/entity-ref are
// string controls, so they never collide with these numeric testers).
export const numberTester = rankWith(6, or(isNumberControl, isIntegerControl));

// ── collapsible component group (the inspector's section layout) ─────────────────────────────────────────
// Replaces the vanilla GroupLayout: a section header per component (object identity · Transform · rendering ·
// health bar · other) that collapses, so a dense inspector stays scannable. Renders its children through the
// standard `JsonFormsDispatch` so every control still resolves by tester.
function GroupLayoutBase({ uischema, schema, path, renderers, cells, enabled, visible }: LayoutProps) {
  const group = uischema as unknown as { label?: string; elements: UISchemaElement[] };
  if (visible === false) return null;
  const label = group.label ?? "Component";
  const defaultOpen = label.trim().toLocaleLowerCase("en-GB") === "transform";
  return (
    <DisclosureSection
      title={label}
      defaultOpen={defaultOpen}
      storageKey={`inspector-component:${encodeURIComponent(label)}`}
      tone="card"
      landmark={false}
      unmountOnClose={false}
      data-testid="inspectorGroup"
      data-group={label}
    >
      {/* ONE SET OF COLUMNS FOR THE WHOLE GROUP. Without this wrapper each row sizes its own tracks
          from its own label, unit and reset, and nine Transform rows produce five different input
          widths. Per GROUP rather than per panel because each group is its own card with its own
          padding, and that is the boundary a reader already sees. */}
      <div className="mtk-property-sheet">
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
    </DisclosureSection>
  );
}
export const CollapsibleGroup = withJsonFormsLayoutProps(GroupLayoutBase);
export const groupTester = rankWith(5, uiTypeIs("Group"));

/** The root stack `buildEntityUiSchema` emits — one `Group` per component, in order.
 *
 *  IT IS HERE BECAUSE DROPPING `vanillaRenderers` DROPPED THE LAYOUTS TOO, and that is not a detail
 *  the type system or the tests could report: JSON Forms answers a uischema element it cannot resolve
 *  by rendering the literal string **"No applicable renderer found."** into the panel, in red, and
 *  carries on. `tsc` was clean, every gate that reads source was green, and the first Chromium capture
 *  showed that sentence where seventeen property rows should have been. A renderer registry is a
 *  contract about a whole tree, and only something that renders the tree can check it. */
function VerticalLayoutBase({ uischema, schema, path, renderers, cells, enabled, visible }: LayoutProps) {
  const layout = uischema as unknown as { elements: UISchemaElement[] };
  if (visible === false) return null;
  return (
    <div className="mtk-inspector-stack">
      {layout.elements.map((el, i) => (
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
  );
}
export const VerticalLayout = withJsonFormsLayoutProps(VerticalLayoutBase);
export const verticalLayoutTester = rankWith(3, uiTypeIs("VerticalLayout"));

// ── closed vocabulary — the core states these in `ui_hint`, e.g. "enum: directional|point|spot" ────────
// A free-text box on a closed vocabulary invites a value the core will reject, and the rejection is the
// first time the user learns the vocabulary exists. Seven components carry one (`RigidBody.kind`,
// `Collider.shape`, `Joint.kind`, `Light.kind`, `PlayIf.join`, `ShapeRecipe.kind`, `GameRole.role`);
// before ADR-136 the editor read `ui_hint` nowhere and rendered every one of them as a text box.
function EnumControlBase(props: ControlProps) {
  const { data, handleChange, path, label, schema, enabled } = props;
  const options = ((schema as { enum?: unknown[] }).enum ?? []).map(String);
  const mixed = useMixed(path);
  const value = !mixed && typeof data === "string" ? data : "";
  return (
    <Row {...props}>
      <SelectField
        id={controlId(path)}
        aria-label={named(label, schema)}
        disabled={enabled === false}
        value={value}
        title={mixed ? "The selected objects differ — pick one to set all of them" : value || undefined}
        data-mixed={mixed ? "1" : undefined}
        data-testid={`enum-${path}`}
        onChange={(e) => handleChange(path, e.target.value)}
        style={{ width: "100%" }}
      >
        {/* The mixed state is a real option so the closed select can SHOW it; it is never a value the
            user can choose back to, because "un-set them" is not a thing this control can do. */}
        {mixed && <option value="" disabled>Mixed</option>}
        {/* A value the core sent that is NOT in the vocabulary must stay visible and selected rather
            than silently reading as the first option — that would show the user a value the entity
            does not have, and committing any other field would then write it. */}
        {value && !options.includes(value) && <option value={value}>{value} — not in this vocabulary</option>}
        {options.map((o) => (
          <option key={o} value={o}>
            {o}
          </option>
        ))}
      </SelectField>
    </Row>
  );
}
export const EnumControl = withJsonFormsControlProps(EnumControlBase);
// Above the plain-string control, below the format-keyed ones: a field can carry both a format and an
// enum, and the format is the more specific statement.
export const enumTester = rankWith(8, and(isStringControl, isEnum));

// ── asset reference (`format: "asset"`) ───────────────────────────────────────────────────────────────
// SIX core fields carry this format — `Sprite.texture`, `MeshRenderer.mesh`, `MeshRenderer.material`,
// `AudioSource.clip`, `Animator.controller`, `Script.source` — and until ADR-136 the editor had no
// renderer for it, so each one rendered as a bare text box a user was invited to type a content hash
// into, with nothing on screen saying it was a reference rather than a name.
//
// WHAT THIS DELIBERATELY IS NOT: a browse button. The asset browser is a panel with its own selection
// model and wiring it from here is a real piece of work, not a two-line prop — and a button that opens
// nothing is worse than no button. What it IS: the reference stated as a reference — monospaced, its
// full value in the title so a truncated hash is still readable, and an explicit empty state instead of
// a blank box. Every part of that is wired.
function AssetRefControlBase(props: ControlProps) {
  const { data, handleChange, path, label, enabled } = props;
  const mixed = useMixed(path);
  const value = !mixed && typeof data === "string" ? data : "";
  return (
    <Row {...props}>
      <TextField
        id={controlId(path)}
        mono
        aria-label={`${label} (asset reference)`}
        disabled={enabled === false}
        value={value}
        title={mixed ? "The selected objects reference different assets" : value || undefined}
        placeholder={mixed ? "Mixed" : "no asset"}
        data-mixed={mixed ? "1" : undefined}
        data-testid={`asset-${path}`}
        onChange={(e) => handleChange(path, e.target.value)}
        style={{ width: "100%" }}
      />
    </Row>
  );
}
export const AssetRefControl = withJsonFormsControlProps(AssetRefControlBase);
export const assetRefTester = rankWith(10, and(isStringControl, hasFormat("asset")));

// ── entity reference / bind-target picker (`format: "entity-ref"`) ───────────────────────────────────
// The core's real entity references are `Joint.bodyA` and `Joint.bodyB`. This control has existed since
// M2.5 and could never fire, because the table pointed it at `Targeting.target` — a component the core
// does not have. Repairing the table is the whole fix; the control below is unchanged in substance.
function EntityRefControlBase(props: ControlProps) {
  const { data, handleChange, path, label, schema, enabled } = props;
  // Reads the live summary projection for the candidate list. The real ranking is the ECS
  // compatibility query (M1.5); here it's name-sorted with that hook documented.
  const summaries = useStore(projectionStore, (s) => s.summaries);
  const options = Object.values(summaries)
    .sort((a, b) => a.name.localeCompare(b.name))
    .slice(0, 200); // never dump 5k into a <select>; the graph is the at-scale picker
  const mixed = useMixed(path);
  const value = !mixed && typeof data === "string" ? data : "";
  const dangling = value && !options.some((o) => o.id === value);
  // The selected entity's NAME in the title: inside a 320px dock the closed select ellipsises, and a
  // name is the only thing here a user can identify the target by ("Counterwe…" is not one).
  const selectedName = options.find((o) => o.id === value)?.name;
  return (
    <Row {...props}>
      <SelectField
        id={controlId(path)}
        aria-label={named(label, schema)}
        disabled={enabled === false}
        value={value}
        title={mixed ? "The selected objects point at different entities" : selectedName ?? (value || undefined)}
        data-mixed={mixed ? "1" : undefined}
        data-testid={`ref-${path}`}
        onChange={(e) => handleChange(path, e.target.value)}
        style={{ width: "100%" }}
      >
        {mixed ? <option value="" disabled>Mixed</option> : <option value="">— none —</option>}
        {/* A reference to an entity that is not in the projection (deleted, or not yet loaded) keeps
            its id on screen. Dropping it would silently re-point the joint at nothing. */}
        {dangling && <option value={value}>{value} — not in this scene</option>}
        {options.map((o) => (
          <option key={o.id} value={o.id}>
            {o.name}
          </option>
        ))}
      </SelectField>
    </Row>
  );
}
export const EntityRefControl = withJsonFormsControlProps(EntityRefControlBase);
export const entityRefTester = rankWith(10, and(isStringControl, hasFormat("entity-ref")));

// ── boolean ───────────────────────────────────────────────────────────────────────────────────────────
// Previously the vanilla `.checkbox` renderer — an unstyled control in the middle of a design-system
// panel. `accent-color` in `global.css` is the shared checkbox contract (the same one `AssetLabPanel`
// uses); what this adds is the row it sits in.
function BooleanControlBase(props: ControlProps) {
  const { data, handleChange, path, label, schema, enabled } = props;
  const mixed = useMixed(path);
  return (
    <Row {...props}>
      <input
        id={controlId(path)}
        type="checkbox"
        aria-label={named(label, schema)}
        aria-checked={mixed ? "mixed" : undefined}
        disabled={enabled === false}
        checked={data === true}
        // A checkbox has a THIRD state and the DOM only reaches it through the property, never an
        // attribute — so a mixed tick shows as indeterminate rather than as a confident "off".
        ref={(el) => {
          if (el) el.indeterminate = mixed;
        }}
        title={mixed ? "The selected objects differ — tick to set all of them" : undefined}
        data-mixed={mixed ? "1" : undefined}
        data-testid={`bool-${path}`}
        onChange={(e) => handleChange(path, e.target.checked)}
      />
    </Row>
  );
}
export const BooleanControl = withJsonFormsControlProps(BooleanControlBase);
export const booleanTester = rankWith(6, isBooleanControl);

// ── plain string ──────────────────────────────────────────────────────────────────────────────────────
// The lowest-ranked control, and the one that used to be `@jsonforms/vanilla-renderers`' `.input`. Kept
// last so that a field which is really an asset, a reference or a vocabulary never lands here.
function StringControlBase(props: ControlProps) {
  const { data, handleChange, path, label, schema, enabled } = props;
  const mixed = useMixed(path);
  return (
    <Row {...props}>
      <TextField
        id={controlId(path)}
        aria-label={named(label, schema)}
        disabled={enabled === false}
        value={!mixed && typeof data === "string" ? data : ""}
        placeholder={mixed ? "Mixed" : undefined}
        title={mixed ? "The selected objects differ — type a value to set all of them" : undefined}
        data-mixed={mixed ? "1" : undefined}
        data-testid={`text-${path}`}
        onChange={(e) => handleChange(path, e.target.value)}
        style={{ width: "100%" }}
      />
    </Row>
  );
}
export const StringControl = withJsonFormsControlProps(StringControlBase);
export const stringTester = rankWith(4, isStringControl);

/** **THE REGISTRY — ONE LIST, TWO PANELS.**
 *
 *  `vanillaRenderers` IS DELIBERATELY NOT HERE (ADR-136). It used to be spread in as the fallback, and
 *  it is what every boolean, plain string and vocabulary field in the inspector actually rendered
 *  through — emitting `.control`, `.input`, `.select` and `.checkbox`, generic class names this
 *  repository's stylesheet has no rules for and that `check-class-hooks.mjs` cannot see, because they
 *  come out of `node_modules` rather than out of the markup it reads. The set below covers every scalar
 *  `FieldType` the core can register (Number · Integer · Boolean · String, plus the `format` and
 *  vocabulary refinements), so there is nothing left for a fallback to catch — and its absence is what
 *  lets a `shots` scene assert those four class names are not on the page. `vanillaCells` stays: cells
 *  are the array/table path, which this inspector does not use, and JsonForms wants a non-empty list.
 *
 *  It lives HERE rather than in `Inspector.tsx` because ADR-169 gave it a second consumer. A
 *  multi-selection form that resolved one control differently from the single-object form would be a
 *  drift nothing could see — both panels read this array. */
export const inspectorRenderers = [
  { tester: stringTester, renderer: StringControl },
  { tester: numberTester, renderer: NumberControl },
  { tester: booleanTester, renderer: BooleanControl },
  { tester: enumTester, renderer: EnumControl },
  { tester: assetRefTester, renderer: AssetRefControl },
  { tester: entityRefTester, renderer: EntityRefControl },
  { tester: groupTester, renderer: CollapsibleGroup },
  { tester: verticalLayoutTester, renderer: VerticalLayout },
];
