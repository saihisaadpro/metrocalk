//! THE REGISTRY-FED CLAUSE — one statement of it, for the two builders that had written it twice.
//!
//! A **condition** in this engine is always the same sentence: `<entity>.<component>.<field> <op>
//! <value>`, assembled entirely by clicks from what `rule_registry` declares, so a nil-ref or a typo'd
//! field is unreachable by construction. `metrocalk_core::rules` evaluates exactly one of these,
//! whether it arrived as a Rule's `If` or as the guard on a state-machine transition — a transition
//! **is** a Rule (ADR-046).
//!
//! It was authored twice all the same: `RulesPanel` and `StateGraphPanel` each declared their own
//! `fieldTy` / `defaultValue` / `rawValue` / `ValueInput` / target pickers, and the copies had already
//! drifted in a way a user meets. `RulesPanel` narrows the operator list by the field's TYPE — a
//! boolean or a string gets `=` and `≠` only, because every ordering comparison on a two-valued
//! domain collapses to an equality or to a constant, and no string field the registry declares is a
//! quantity — and it says WHY in the row's title. The state-machine copy offered all six on
//! everything, so the same clause about the same field offered `lit > false` in one editor and
//! refused to in the other, one tab apart in the same dock.
//!
//! The two remain distinguishable where they genuinely differ: their stable test hooks. Everything
//! else — the vocabulary, the coercion rules, the row anatomy — is this file.

import { ListRow } from "../theme/fields";
import { Button, NumericField, SelectField, TextField } from "../theme/primitives";
import { Icon } from "../theme/icons";
import type { CompareOp, FieldValue, RuleCondition, RuleRegistryInfo } from "../transport/protocol";

const EQUALITY: { op: CompareOp; label: string }[] = [
  { op: "eq", label: "=" },
  { op: "ne", label: "≠" },
];
const ORDERING: { op: CompareOp; label: string }[] = [
  { op: "lt", label: "<" },
  { op: "le", label: "≤" },
  { op: "gt", label: ">" },
  { op: "ge", label: "≥" },
];

/** The operators that can **mean** something about a field of this type.
 *
 *  `CompareOp::eval` orders every scalar kind — `false < true`, strings lexicographically — so the core
 *  accepts all six on anything. Accepting is not the same as being worth offering, and a builder whose
 *  whole claim is that a clause assembled by clicks is a clause worth having must not offer one that
 *  can never fail. This narrows what is *offered*, never what the core can evaluate: a clause authored
 *  elsewhere with `lit ≥ false` still loads, still runs, and still means what it meant. */
export function opsFor(ty: string): { op: CompareOp; label: string }[] {
  return ty === "boolean" || ty === "string" ? EQUALITY : [...EQUALITY, ...ORDERING];
}

/** Why an operator list is short, said in the row rather than left as a mystery (`<ux_quality>` 4). */
export function opsHint(ty: string): string {
  if (ty === "boolean") return "A true/false field is either equal or not — ordering one always answers the same way.";
  if (ty === "string") return "A name is either equal or not — ordering names would compare them alphabetically.";
  return "How to compare the field with the value.";
}

/** Keep a clause's operator legal for the field it now points at. Re-targeting a clause from `hp` to
 *  `visible` used to leave `>` selected on a list that no longer offers it — a select showing a value it
 *  has no option for renders as though nothing is chosen. Same coercion the value already does. */
export function coerceOp(op: CompareOp, ty: string): CompareOp {
  return opsFor(ty).some((o) => o.op === op) ? op : "eq";
}

/** A new clause opens on an operator that suits its field, rather than on whichever one happened to be
 *  legal for the registry's first component. */
export function defaultOp(ty: string): CompareOp {
  return ty === "boolean" || ty === "string" ? "eq" : "ge";
}

export function fieldTy(reg: RuleRegistryInfo, component: string, field: string): string {
  return reg.components.find((c) => c.name === component)?.fields.find((f) => f.name === field)?.ty ?? "string";
}

export function defaultValue(ty: string): FieldValue {
  if (ty === "integer") return { Integer: 0 };
  if (ty === "number") return { Number: 0 };
  if (ty === "boolean") return { Bool: false };
  return { Str: "" };
}

/** Read the scalar out of an externally-tagged FieldValue for an input's `value`. */
export function rawValue(v: FieldValue): string {
  if ("Integer" in v) return String(v.Integer);
  if ("Number" in v) return String(v.Number);
  if ("Bool" in v) return String(v.Bool);
  return v.Str;
}

/** The pickers SHARE the row instead of each claiming an intrinsic width — otherwise the shared control
 *  sizing pushes the row past the dock and the remove button wraps onto a line of its own, which reads as
 *  a broken row rather than a dense one. They grow into whatever the dock gives them and shrink to these
 *  floors before the row is allowed to wrap. */
export const grow = (basis: number): React.CSSProperties => ({ flex: `1 1 ${basis}px`, minWidth: 64 });
export const fixed = (w: number): React.CSSProperties => ({ flex: `0 0 ${w}px` });

/** The stable hooks a black-box test keys on. The two builders differ here and nowhere else. */
export interface ClauseTestIds {
  /** `rule` → `rule-entity`, `rule-component`, … ; `sm-cond` → `sm-cond-entity`, … */
  prefix: string;
  /** Set where the ROW itself is queried (the state-machine guard list counts its rows). */
  row?: string;
}

/** A value input whose KIND is dictated by the field's registry type (typo-proof: an integer field gets a
 *  number input, a boolean a true/false select) — the value can never be the wrong shape for the field. */
export function ValueInput({
  ty,
  value,
  onChange,
  ariaLabel,
  testId,
}: {
  ty: string;
  value: FieldValue;
  onChange: (v: FieldValue) => void;
  ariaLabel: string;
  testId: string;
}) {
  if (ty === "boolean") {
    return (
      <SelectField
        aria-label={ariaLabel}
        data-testid={testId}
        style={fixed(84)}
        value={"Bool" in value ? String(value.Bool) : "false"}
        onChange={(e) => onChange({ Bool: e.target.value === "true" })}
      >
        <option value="true">true</option>
        <option value="false">false</option>
      </SelectField>
    );
  }
  if (ty === "integer" || ty === "number") {
    return (
      <NumericField
        ariaLabel={ariaLabel}
        data-testid={testId}
        value={Number(rawValue(value)) || 0}
        integer={ty === "integer"}
        style={{ width: 72, ...fixed(72) }}
        onCommit={(n) => onChange(ty === "integer" ? { Integer: Math.trunc(n) } : { Number: n })}
      />
    );
  }
  return (
    <TextField
      aria-label={ariaLabel}
      data-testid={testId}
      style={fixed(96)}
      value={rawValue(value)}
      onChange={(e) => onChange({ Str: e.target.value })}
    />
  );
}

/** A `component.field` picker fed by the registry (only real components + their real fields are offerable). */
export function TargetPicker({
  reg,
  entityOptions,
  entity,
  component,
  field,
  contextLabel,
  testIds,
  onChange,
}: {
  reg: RuleRegistryInfo;
  entityOptions: { id: string; name: string }[];
  entity: string;
  component: string;
  field: string;
  contextLabel: string;
  testIds: ClauseTestIds;
  onChange: (patch: { entity?: string; component?: string; field?: string }) => void;
}) {
  const fields = reg.components.find((c) => c.name === component)?.fields ?? [];
  return (
    <>
      <SelectField
        aria-label={`${contextLabel} entity`}
        data-testid={`${testIds.prefix}-entity`}
        style={grow(88)}
        value={entity}
        onChange={(e) => onChange({ entity: e.target.value })}
      >
        <option value="">— entity —</option>
        {entityOptions.map((o) => (
          <option key={o.id} value={o.id}>
            {o.name}
          </option>
        ))}
      </SelectField>
      <SelectField
        aria-label={`${contextLabel} component`}
        data-testid={`${testIds.prefix}-component`}
        style={grow(88)}
        value={component}
        onChange={(e) => {
          const c = reg.components.find((x) => x.name === e.target.value);
          onChange({ component: e.target.value, field: c?.fields[0]?.name ?? "" });
        }}
      >
        {reg.components.map((c) => (
          <option key={c.name} value={c.name}>
            {c.name}
          </option>
        ))}
      </SelectField>
      <SelectField
        aria-label={`${contextLabel} field`}
        data-testid={`${testIds.prefix}-field`}
        style={grow(72)}
        value={field}
        onChange={(e) => onChange({ field: e.target.value })}
      >
        {fields.map((f) => (
          <option key={f.name} value={f.name}>
            {f.name}
          </option>
        ))}
      </SelectField>
    </>
  );
}

/** One clause — `<entity>.<component>.<field> <op> <value>`, plus its remove control.
 *
 *  A Rule's AND list, its OR group and a transition's guard render through this one component on
 *  purpose: they are the same sentence joined differently, so a divergence between them could only
 *  ever be a bug. `label` ("Condition 1" / "Alternative 1" / "Transition 2 condition 1") is what makes
 *  the repeated controls tell an assistive reader — and a black-box test — which row they belong to. */
export function ClauseRow({
  reg,
  entityOptions,
  clause,
  label,
  testIds,
  onChange,
  onRemove,
}: {
  reg: RuleRegistryInfo;
  entityOptions: { id: string; name: string }[];
  clause: RuleCondition;
  label: string;
  testIds: ClauseTestIds;
  onChange: (next: RuleCondition) => void;
  onRemove: () => void;
}) {
  const ty = fieldTy(reg, clause.component, clause.field);
  return (
    <ListRow data-testid={testIds.row}>
      <TargetPicker
        reg={reg}
        entityOptions={entityOptions}
        entity={clause.entity}
        component={clause.component}
        field={clause.field}
        contextLabel={label}
        testIds={testIds}
        onChange={(p) => {
          const next = { ...clause, ...p };
          if (p.component || p.field) {
            const t = fieldTy(reg, next.component, next.field);
            next.value = defaultValue(t);
            next.op = coerceOp(next.op, t);
          }
          onChange(next);
        }}
      />
      <SelectField
        aria-label={`${label} comparison operator`}
        data-testid={`${testIds.prefix}-op`}
        title={opsHint(ty)}
        style={fixed(68)}
        value={clause.op}
        onChange={(e) => onChange({ ...clause, op: e.target.value as CompareOp })}
      >
        {opsFor(ty).map((o) => (
          <option key={o.op} value={o.op}>
            {o.label}
          </option>
        ))}
      </SelectField>
      <ValueInput
        ariaLabel={`${label} value`}
        testId={`${testIds.prefix}-value`}
        ty={ty}
        value={clause.value}
        onChange={(v) => onChange({ ...clause, value: v })}
      />
      <Button variant="ghost" compact icon aria-label={`Remove ${label.toLowerCase()}`} onClick={onRemove}>
        <Icon name="close" size="sm" />
      </Button>
    </ListRow>
  );
}
