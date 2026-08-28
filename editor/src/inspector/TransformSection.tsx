//! **The Transform, drawn as the property it is** (ADR-172).
//!
//! Every other component in the Inspector is drawn from the projection: whatever fields arrived, one
//! row each. For twenty-four components that is right. For the one component every object carries it
//! is wrong twice over.
//!
//! * **A sparse Transform is not a partial object.** `capscene::create_entity` writes `x`/`y`/`z` and
//!   nothing else, so a data-driven form offered three rows and no way to rotate or scale — while the
//!   renderer was already drawing that object with an identity rotation and a scale of 1, because
//!   `local_transform` reads an absent field as the identity. The panel showed less than the engine
//!   knew.
//! * **Four numbers are one rotation.** `qx`/`qy`/`qz`/`qw` appeared as four independent number boxes
//!   the moment an object was rotated by the gizmo. Nobody types a quaternion, and anybody who typed
//!   into ONE of those boxes committed a quaternion of length ≠ 1 — not a rotation at all — in four
//!   separate transactions.
//!
//! So this section is declared rather than derived: position, rotation and scale, always, for
//! anything carrying a `Transform`. The titles, units and defaults come from the curated schema
//! (`schema/registry.ts`), which is gated against `core/src/stdlib.rs`, so the vocabulary is still
//! stated once. The rotation is shown in **degrees** and written as ONE normalised quaternion through
//! `set_rotation` — one transaction, one undo step, for one gesture.

import { useEffect, useMemo, useState } from "react";
import { NumericField, PropertyRow, Button } from "../theme/primitives";
import { DisclosureSection } from "../theme/workspace";
import { Icon } from "../theme/icons";
import { componentSchemas } from "../schema/registry";
import { agreeWithin, eulerDegToQuat, quatToEulerDeg, type ResolvedTransform } from "./transform";

const schema = componentSchemas.Transform.properties;

/** Position axes, in the order a person reads them, paired with the curated field they write. */
const POSITION = [
  { field: "x", tag: "X" },
  { field: "y", tag: "Y" },
  { field: "z", tag: "Z" },
] as const;

/** The three Euler rows. They borrow `qx`/`qy`/`qz`'s curated titles and unit — the degrees a person
 *  types ARE the rotation those components encode, and stating the label twice is how the two drift. */
const ROTATION = [
  { field: "qx", tag: "X" },
  { field: "qy", tag: "Y" },
  { field: "qz", tag: "Z" },
] as const;

export interface TransformSectionProps {
  /** One resolved transform per selected object, **primary first**. A single selection is a list of one. */
  transforms: ResolvedTransform[];
  /** Identifies WHICH objects these are, so a pending rotation is dropped when the selection changes
   *  rather than carried onto whatever was selected next. */
  selectionKey: string;
  /** Commit one position axis (`x`/`y`/`z`) across the selection. */
  onSetPosition: (field: "x" | "y" | "z", value: number) => void;
  /** Commit the uniform scale across the selection. */
  onSetScale: (value: number) => void;
  /** Commit a rotation across the selection, as ONE normalised quaternion. */
  onSetRotation: (quat: [number, number, number, number]) => void;
}

const rowId = (field: string) => `mtk-prop-Transform-${field}`;

export function TransformSection({
  transforms,
  selectionKey,
  onSetPosition,
  onSetScale,
  onSetRotation,
}: TransformSectionProps) {
  const primary = transforms[0];
  const many = transforms.length > 1;

  // KEYED ON THE NUMBERS, NOT ON THE OBJECT. `readTransform` builds a fresh object on every render,
  // so a dependency on `transforms[0]` changes identity every time the panel re-renders for any
  // reason — which made the memo below recompute constantly and, worse, made the effect that clears
  // the pending rotation fire on the very next render after a commit. The first capture of this
  // section shows the result: the reset was clicked, one transaction went out, and the box still read
  // `45`, because the draft had already been thrown away and the projection had not yet answered.
  const q = primary ? primary.quaternion : ([0, 0, 0, 1] as const);
  const [qx, qy, qz, qw] = q;
  const derived = useMemo(
    () => quatToEulerDeg([qx, qy, qz, qw]),
    [qx, qy, qz, qw],
  );
  const perObjectEuler = useMemo(
    () => transforms.map((t) => quatToEulerDeg(t.quaternion)),
    [transforms],
  );

  // WHAT THE USER TYPED, HELD UNTIL THE ENGINE ANSWERS. `set_rotation` is a command, not the
  // optimistic `setField` echo, so the projection catches up a round trip later; without this the
  // three boxes snap back to the old angles between the commit and the reply. Cleared when the
  // committed rotation actually moves — including by an undo or another editor — and when the
  // selection changes, so a pending angle is never carried onto a different object.
  const [draft, setDraft] = useState<[number, number, number] | null>(null);
  const [dx, dy, dz] = derived;
  useEffect(() => setDraft(null), [dx, dy, dz, selectionKey]);

  if (!primary) return null;
  const euler = draft ?? derived;

  const commitAngle = (index: number, value: number) => {
    const next: [number, number, number] = [...euler];
    next[index] = value;
    setDraft(next);
    onSetRotation(eulerDegToQuat(next));
  };

  const rotationMixed = (index: number) =>
    many && !agreeWithin(perObjectEuler.map((e) => e[index]));

  const reset = (label: string, value: number, onClick: () => void) => (
    <Button
      variant="ghost"
      icon
      compact
      aria-label={`Reset ${label} to ${value}`}
      title={`Reset to ${value}`}
      data-testid="prop-reset"
      onClick={onClick}
    >
      <Icon name="revert" size={13} />
    </Button>
  );

  return (
    <DisclosureSection
      title="Transform"
      defaultOpen
      storageKey="inspector-component:Transform"
      tone="card"
      landmark={false}
      unmountOnClose={false}
      data-testid="inspectorGroup"
      data-group="Transform"
    >
      {/* One set of columns for the whole group, exactly as the data-driven groups do — otherwise
          each row sizes its own tracks from its own label and the seven inputs differ in width. */}
      <div className="mtk-property-sheet">
        {POSITION.map(({ field, tag }, i) => {
          const values = transforms.map((t) => t.position[i]);
          const mixed = many && !agreeWithin(values);
          const value = values[0];
          const title = schema[field].title ?? `Position ${tag}`;
          return (
            <PropertyRow
              key={field}
              htmlFor={rowId(field)}
              label={title}
              unit={schema[field].unit}
              data-testid="prop-row"
              actions={
                // A MIXED row still offers its reset, exactly as the schema-driven rows do: "set every
                // one of them back to 0" is precisely what a reset is for, and it is the one action a
                // mixed row can take that is unambiguous.
                mixed || value !== 0
                  ? reset(title, 0, () => onSetPosition(field, 0))
                  : undefined
              }
            >
              <NumericField
                id={rowId(field)}
                value={value}
                mixed={mixed}
                step={0.1}
                ariaLabel={`${title} (m)`}
                data-testid={`num-Transform.${field}`}
                title={
                  mixed
                    ? `${title}: the selected objects differ — type a value to set all of them`
                    : undefined
                }
                onCommit={(v) => onSetPosition(field, v)}
                style={{ width: "100%" }}
              />
            </PropertyRow>
          );
        })}

        {ROTATION.map(({ field, tag }, i) => {
          const mixed = rotationMixed(i);
          const title = schema[field].title ?? `Rotation ${tag}`;
          return (
            <PropertyRow
              key={field}
              htmlFor={rowId(field)}
              label={title}
              unit={schema[field].unit}
              data-testid="prop-row"
              // The one row in this panel whose value is DERIVED, so it says so where a reader will
              // look: the help line names what an edit does to the whole rotation, and to the whole
              // selection when there is one.
              help={
                i === 2
                  ? many
                    ? "Turns every selected object to this rotation, in one undoable edit."
                    : undefined
                  : undefined
              }
              actions={
                mixed || euler[i] !== 0
                  ? reset(title, 0, () => commitAngle(i, 0))
                  : undefined
              }
            >
              <NumericField
                id={rowId(field)}
                value={euler[i]}
                mixed={mixed}
                step={1}
                scrubSpeed={1}
                ariaLabel={`${title} (degrees)`}
                data-testid={`num-Transform.${field}`}
                title={
                  mixed
                    ? `${title}: the selected objects are rotated differently — typing sets all of them to this rotation`
                    : undefined
                }
                onCommit={(v) => commitAngle(i, v)}
                style={{ width: "100%" }}
              />
            </PropertyRow>
          );
        })}

        {(() => {
          const values = transforms.map((t) => t.scale);
          const mixed = many && !agreeWithin(values);
          const value = values[0];
          const title = schema.scale.title ?? "Scale";
          return (
            <PropertyRow
              htmlFor={rowId("scale")}
              label={title}
              unit={schema.scale.unit}
              data-testid="prop-row"
              actions={
                mixed || value !== 1 ? reset(title, 1, () => onSetScale(1)) : undefined
              }
            >
              <NumericField
                id={rowId("scale")}
                value={value}
                mixed={mixed}
                step={0.1}
                // The floor is the curated schema's, not a second opinion: the table is where this
                // editor states what a field may hold, and `check-registry-vocab.mjs` gates it.
                min={schema.scale.minimum}
                ariaLabel={`${title} (x)`}
                data-testid="num-Transform.scale"
                title={
                  mixed
                    ? `${title}: the selected objects differ — type a value to set all of them`
                    : undefined
                }
                onCommit={(v) => onSetScale(v)}
                style={{ width: "100%" }}
              />
            </PropertyRow>
          );
        })()}
      </div>
    </DisclosureSection>
  );
}
