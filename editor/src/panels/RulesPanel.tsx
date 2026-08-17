//! M12.1 (ADR-045) — the Rules panel: a **registry-fed builder** (every dropdown fed by `rule_registry`, so
//! authoring a When/If/Then conditional is **typo-proof by clicks** — no free text, no nil-refs) + the Rule
//! list. Authoring is one undoable transaction; a registry-rejected rule shows its **Blocked + explained**
//! reason inline (ADR-016); and when the engine offers a **mirror "cleanup" rule** (the missing-"off"-switch
//! guard) it's surfaced as a toast the user can accept. Running rules is M12.5.
//!
//! The **If** is two claims, not one: an AND list every clause of which must hold, plus at most ONE OR group
//! ("either …") of which any single alternative is enough. That is the shape `metrocalk_core::rules::
//! RuleData` has always evaluated and persisted; until now this builder could only author the AND half, so a
//! rule meaning "if it's lit OR it's wet" was unreachable through the panel that presents itself as the
//! whole of When/If/Then. One group, never nested — the authored depth ceiling is two, which is what keeps a
//! conditional readable as a sentence (the same ceiling the per-object "Only if…" cards hold themselves to).

import { useEffect, useState } from "react";
import { useStore } from "zustand";
import { projectionStore } from "../store/projection";
import { pushToast } from "../store/toasts";
import { Button, NumericField, SelectField, TextField } from "../theme/primitives";
import { color, font, fontSize, radius, space } from "../theme/tokens";
import type { EditorClient } from "../transport/session";
import type {
  CompareOp,
  FieldValue,
  RuleAction,
  RuleCondition,
  RuleData,
  RuleRegistryInfo,
  RuleSummary,
} from "../transport/protocol";

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
 *  accepts all six on anything. Accepting is not the same as being worth offering, and this builder's
 *  whole claim is that a clause assembled by clicks is a clause worth having:
 *
 *  * **boolean** — on a two-valued totally-ordered domain every ordering comparison collapses to an
 *    equality or to a constant. `lit > false` *is* `lit = true`; `lit ≥ false` is true of every value the
 *    field can hold, so it is a clause that can never fail. Nothing is lost by removing them, and a
 *    guaranteed-true If stops being one click away.
 *  * **string** — ordering is alphabetical, and every string field the registry defines is a categorical
 *    name (`state`, `role`, `kind`, `shape`, `preset`, `source`, `anchor`, `kit`, `join`, `current`).
 *    "the state's name sorts before `idle`" is not a thing an author means, and the bare `<` never said
 *    that was the question.
 *
 *  Numeric fields keep all six. This narrows what is *offered*, never what the core can evaluate: a rule
 *  authored elsewhere with `lit ≥ false` still loads, still runs, and still means what it meant. */
function opsFor(ty: string): { op: CompareOp; label: string }[] {
  return ty === "boolean" || ty === "string" ? EQUALITY : [...EQUALITY, ...ORDERING];
}
/** Why an operator list is short, said in the row rather than left as a mystery (`<ux_quality>` 4). */
function opsHint(ty: string): string {
  if (ty === "boolean") return "A true/false field is either equal or not — ordering one always answers the same way.";
  if (ty === "string") return "A name is either equal or not — ordering names would compare them alphabetically.";
  return "How to compare the field with the value.";
}
/** Keep a clause's operator legal for the field it now points at. Re-targeting a clause from `hp` to
 *  `visible` used to leave `>` selected on a list that no longer offers it — a select showing a value it
 *  has no option for renders as though nothing is chosen. Same coercion the value already does. */
function coerceOp(op: CompareOp, ty: string): CompareOp {
  return opsFor(ty).some((o) => o.op === op) ? op : "eq";
}
/** A new clause opens on an operator that suits its field, rather than on whichever one happened to be
 *  legal for the registry's first component. */
function defaultOp(ty: string): CompareOp {
  return ty === "boolean" || ty === "string" ? "eq" : "ge";
}

const box: React.CSSProperties = { font: `${fontSize.meta}px ${font.mono}`, padding: space.lg };
/** One clause/action row: the controls wrap rather than overflow the dock at narrow widths. */
const row: React.CSSProperties = { display: "flex", gap: 4, flexWrap: "wrap", margin: "3px 0", alignItems: "center" };
/** The pickers SHARE the row instead of each claiming an intrinsic width — otherwise the shared control
 *  sizing pushes the row past the dock and the remove button wraps onto a line of its own, which reads as
 *  a broken row rather than a dense one. They grow into whatever the dock gives them and shrink to these
 *  floors before the row is allowed to wrap. */
const grow = (basis: number): React.CSSProperties => ({ flex: `1 1 ${basis}px`, minWidth: 64 });
const fixed = (w: number): React.CSSProperties => ({ flex: `0 0 ${w}px` });

function fieldTy(reg: RuleRegistryInfo, component: string, field: string): string {
  return reg.components.find((c) => c.name === component)?.fields.find((f) => f.name === field)?.ty ?? "string";
}
function defaultValue(ty: string): FieldValue {
  if (ty === "integer") return { Integer: 0 };
  if (ty === "number") return { Number: 0 };
  if (ty === "boolean") return { Bool: false };
  return { Str: "" };
}
/** Read the scalar out of an externally-tagged FieldValue for an input's `value`. */
function rawValue(v: FieldValue): string {
  if ("Integer" in v) return String(v.Integer);
  if ("Number" in v) return String(v.Number);
  if ("Bool" in v) return String(v.Bool);
  return v.Str;
}

/** A value input whose KIND is dictated by the field's registry type (typo-proof: an integer field gets a
 *  number input, a boolean a true/false select) — the value can never be the wrong shape for the field. */
function ValueInput({
  ty,
  value,
  onChange,
  ariaLabel,
}: {
  ty: string;
  value: FieldValue;
  onChange: (v: FieldValue) => void;
  ariaLabel: string;
}) {
  if (ty === "boolean") {
    return (
      <SelectField
        aria-label={ariaLabel}
        data-testid="rule-value"
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
        data-testid="rule-value"
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
      data-testid="rule-value"
      style={fixed(96)}
      value={rawValue(value)}
      onChange={(e) => onChange({ Str: e.target.value })}
    />
  );
}

/** A `component.field` picker fed by the registry (only real components + their real fields are offerable). */
function TargetPicker({
  reg,
  entityOptions,
  entity,
  component,
  field,
  contextLabel,
  onChange,
}: {
  reg: RuleRegistryInfo;
  entityOptions: { id: string; name: string }[];
  entity: string;
  component: string;
  field: string;
  contextLabel: string;
  onChange: (patch: { entity?: string; component?: string; field?: string }) => void;
}) {
  const fields = reg.components.find((c) => c.name === component)?.fields ?? [];
  return (
    <>
      <SelectField
        aria-label={`${contextLabel} entity`}
        data-testid="rule-entity"
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
        data-testid="rule-component"
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
        data-testid="rule-field"
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

/** One **If** clause — `<entity>.<component>.<field> <op> <value>`, plus its remove control.
 *
 *  The AND list and the OR group render through this one component on purpose: an alternative is exactly a
 *  condition that happens to be joined differently, so a divergence between "a condition row" and "an
 *  alternative row" could only ever be a bug. `label` ("Condition 1" / "Alternative 1") is what makes the
 *  repeated controls tell an assistive reader — and a black-box test — which row they belong to. */
function ClauseRow({
  reg,
  entityOptions,
  clause,
  label,
  onChange,
  onRemove,
}: {
  reg: RuleRegistryInfo;
  entityOptions: { id: string; name: string }[];
  clause: RuleCondition;
  label: string;
  onChange: (next: RuleCondition) => void;
  onRemove: () => void;
}) {
  const ty = fieldTy(reg, clause.component, clause.field);
  return (
    <div style={row}>
      <TargetPicker
        reg={reg}
        entityOptions={entityOptions}
        entity={clause.entity}
        component={clause.component}
        field={clause.field}
        contextLabel={label}
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
        data-testid="rule-op"
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
      <ValueInput ariaLabel={`${label} value`} ty={ty} value={clause.value} onChange={(v) => onChange({ ...clause, value: v })} />
      <Button variant="ghost" compact icon aria-label={`Remove ${label.toLowerCase()}`} onClick={onRemove}>
        ×
      </Button>
    </div>
  );
}

/** The registry-fed builder (test #5, the clicks path). `onDone` carries the engine-offered mirror
 *  "cleanup" rule (or `null`) up to the panel, which surfaces an explicit accept control. */
function RuleBuilder({
  reg,
  client,
  onDone,
}: {
  reg: RuleRegistryInfo;
  client: EditorClient;
  onDone: (mirror: RuleData | null) => void;
}) {
  const summaries = useStore(projectionStore, (s) => s.summaries);
  const selectedId = useStore(projectionStore, (s) => s.selectedId);
  const entityOptions = Object.values(summaries)
    .sort((a, b) => a.name.localeCompare(b.name))
    .slice(0, 200)
    .map((s) => ({ id: s.id, name: s.name }));
  const defaultEntity = selectedId ?? entityOptions[0]?.id ?? "";
  const firstComp = reg.components[0]?.name ?? "";
  const firstField = reg.components[0]?.fields[0]?.name ?? "";

  const [name, setName] = useState("");
  const [event, setEvent] = useState(reg.events[0]?.name ?? "");
  const [conditions, setConditions] = useState<RuleCondition[]>([]);
  const [anyOf, setAnyOf] = useState<RuleCondition[]>([]);
  const [actions, setActions] = useState<RuleAction[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const newCondition = (): RuleCondition => ({
    entity: defaultEntity,
    component: firstComp,
    field: firstField,
    op: defaultOp(fieldTy(reg, firstComp, firstField)),
    value: defaultValue(fieldTy(reg, firstComp, firstField)),
  });
  const newAction = (): RuleAction => ({
    action: reg.actions[0]?.name ?? "",
    entity: defaultEntity,
    component: firstComp,
    field: firstField,
    value: defaultValue(fieldTy(reg, firstComp, firstField)),
  });

  /** Replace clause `i` of one list without disturbing the other — the AND list and the OR group are
   *  independent, so editing an alternative must never renumber a condition. */
  const patchAt = (
    list: RuleCondition[],
    set: (next: RuleCondition[]) => void,
    i: number,
    next: RuleCondition,
  ) => set(list.map((c, j) => (j === i ? next : c)));

  async function create() {
    setError(null);
    setBusy(true);
    // `any_of` is snake_case because `metrocalk_core::rules::RuleData` is plain serde, not camelCase.
    const rule: RuleData = { name, enabled: true, event, conditions, any_of: anyOf, actions };
    try {
      const r = await client.authorRule(rule);
      if (r.error) {
        setError(r.error); // Blocked + explained (ADR-016)
        return;
      }
      pushToast(`Rule "${name || "untitled"}" created · Ctrl-Z to undo`, "success");
      onDone(r.mirror); // hand the offered mirror "cleanup" rule up to the panel (offered, never forced)
    } catch {
      setError("could not author the rule — please try again");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div data-testid="rule-builder" style={{ border: `1px solid ${color.border.subtle}`, borderRadius: radius.lg, padding: space.md, marginBottom: space.md, background: color.bg.panel }}>
      <TextField
        aria-label="Rule name"
        data-testid="rule-name"
        placeholder="rule name"
        style={{ width: "100%", marginBottom: 6 }}
        value={name}
        onChange={(e) => setName(e.target.value)}
      />
      <div style={{ marginBottom: 6 }}>
        <b>When</b>{" "}
        <SelectField
          aria-label="Rule trigger event"
          data-testid="rule-event"
          value={event}
          onChange={(e) => setEvent(e.target.value)}
        >
          {reg.events.map((ev) => (
            <option key={ev.name} value={ev.name} title={ev.description}>
              {ev.name}
            </option>
          ))}
        </SelectField>
      </div>

      <div style={{ marginBottom: 6 }}>
        <b>If</b>{" "}
        <Button
          compact
          title="Add a clause that must hold — every condition has to be true"
          onClick={() => setConditions([...conditions, newCondition()])}
        >
          + condition
        </Button>{" "}
        <Button
          compact
          data-testid="rule-add-alternative"
          title="Add an alternative — any one of the alternatives is enough, on top of every condition"
          onClick={() => setAnyOf([...anyOf, newCondition()])}
        >
          + alternative
        </Button>
        {conditions.map((c, i) => (
          <ClauseRow
            key={i}
            reg={reg}
            entityOptions={entityOptions}
            clause={c}
            label={`Condition ${i + 1}`}
            onChange={(next) => patchAt(conditions, setConditions, i, next)}
            onRemove={() => setConditions(conditions.filter((_, j) => j !== i))}
          />
        ))}

        {anyOf.length > 0 && (
          <div data-testid="rule-anyof" style={{ marginTop: 4, paddingLeft: space.sm, borderLeft: `2px solid ${color.border.subtle}` }}>
            {/* Plain language, not "any_of": what the group MEANS, said once, where it applies. */}
            <div style={{ color: color.text.muted, margin: "3px 0" }}>
              {conditions.length > 0 ? "…and either" : "either"}{" "}
              <span style={{ opacity: 0.8 }}>— any one of these is enough</span>
            </div>
            {anyOf.map((c, i) => (
              <ClauseRow
                key={i}
                reg={reg}
                entityOptions={entityOptions}
                clause={c}
                label={`Alternative ${i + 1}`}
                onChange={(next) => patchAt(anyOf, setAnyOf, i, next)}
                onRemove={() => setAnyOf(anyOf.filter((_, j) => j !== i))}
              />
            ))}
          </div>
        )}
      </div>

      <div style={{ marginBottom: 6 }}>
        <b>Then</b>{" "}
        <Button compact onClick={() => setActions([...actions, newAction()])}>
          + action
        </Button>
        {actions.map((a, i) => (
          <div key={i} style={row}>
            <SelectField
              aria-label={`Action ${i + 1} type`}
              data-testid="rule-action"
              style={grow(96)}
              value={a.action}
              onChange={(e) => {
                const next = [...actions];
                next[i] = { ...a, action: e.target.value };
                setActions(next);
              }}
            >
              {reg.actions.map((ac) => (
                <option key={ac.name} value={ac.name} title={ac.description}>
                  {ac.name}
                </option>
              ))}
            </SelectField>
            <TargetPicker
              reg={reg}
              entityOptions={entityOptions}
              entity={a.entity}
              component={a.component}
              field={a.field}
              contextLabel={`Action ${i + 1}`}
              onChange={(p) => {
                const next = [...actions];
                next[i] = { ...a, ...p };
                if (p.component || p.field) next[i].value = defaultValue(fieldTy(reg, next[i].component, next[i].field));
                setActions(next);
              }}
            />
            <span>=</span>
            <ValueInput
              ariaLabel={`Action ${i + 1} value`}
              ty={fieldTy(reg, a.component, a.field)}
              value={a.value}
              onChange={(v) => {
                const next = [...actions];
                next[i] = { ...a, value: v };
                setActions(next);
              }}
            />
            <Button variant="ghost" compact icon aria-label={`Remove action ${i + 1}`} onClick={() => setActions(actions.filter((_, j) => j !== i))}>
              ×
            </Button>
          </div>
        ))}
      </div>

      {error && (
        <div data-testid="rule-error" style={{ color: color.danger.text, margin: `${space.xs}px 0` }}>
          {error}
        </div>
      )}
      <Button variant="primary" data-testid="rule-create" disabled={busy} onClick={() => void create()}>
        {busy ? "creating…" : "Create rule"}
      </Button>{" "}
      <Button variant="ghost" onClick={() => onDone(null)}>
        cancel
      </Button>
    </div>
  );
}

export function RulesPanel({ client }: { client: EditorClient }) {
  const [reg, setReg] = useState<RuleRegistryInfo | null>(null);
  const [rules, setRules] = useState<RuleSummary[]>([]);
  const [building, setBuilding] = useState(false);
  const [offeredMirror, setOfferedMirror] = useState<RuleData | null>(null);

  const refresh = () => void client.listRules().then(setRules).catch(() => {});
  useEffect(() => {
    void client.ruleRegistry().then(setReg).catch(() => {});
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client]);

  async function remove(id: string) {
    await client.deleteRule(id).catch(() => false);
    pushToast("Rule removed · Ctrl-Z to undo", "info");
    refresh();
  }

  /** Turn a rule off (or back on) by **replacing** it through the same `author_rule(rule, id)` path the
   *  builder uses. `RuleData.enabled` was already honoured by the runtime — a disabled rule does not fire —
   *  but no authoring surface could ever set it `false`, so it was a live semantic nothing could reach.
   *
   *  Replace rather than a new `set_rule_enabled` command: the replace path already registry-validates,
   *  commits ONE undoable transaction, and appends the replay record that survives reload. A second way to
   *  write a rule would be a second thing to keep correct. The engine offers its mirror "cleanup" rule on
   *  every author; a toggle is not new authoring, so the offer is not surfaced here. */
  async function toggle(r: RuleSummary) {
    const next = !r.rule.enabled;
    const res = await client.authorRule({ ...r.rule, enabled: next }, r.id).catch(() => null);
    if (!res || res.error) {
      pushToast(res?.error ?? `could not turn "${r.rule.name}" ${next ? "on" : "off"}`, "error");
    } else {
      pushToast(`"${r.rule.name}" is ${next ? "on — it runs again" : "off — it will not run"} · Ctrl-Z to undo`, "info");
    }
    refresh();
  }

  async function acceptMirror() {
    if (!offeredMirror) return;
    const r = await client.authorRule(offeredMirror).catch(() => null);
    if (r && !r.error) pushToast(`Cleanup rule "${offeredMirror.name}" added`, "success");
    setOfferedMirror(null);
    refresh();
  }

  return (
    <div id="rules" data-testid="rules-panel" style={{ ...box, borderTop: `1px solid ${color.border.subtle}` }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 6 }}>
        <b>Rules</b>
        <Button data-testid="rule-new" compact disabled={!reg} onClick={() => setBuilding((b) => !b)}>
          {building ? "close" : "+ New rule"}
        </Button>
      </div>

      {building && reg && (
        <RuleBuilder
          reg={reg}
          client={client}
          onDone={(mirror) => {
            setBuilding(false);
            refresh();
            if (mirror) setOfferedMirror(mirror);
          }}
        />
      )}

      {offeredMirror && (
        <div
          data-testid="mirror-offer"
          style={{ border: `1px solid ${color.success.border}`, borderRadius: radius.lg, padding: space.md, marginBottom: space.md, background: color.success.bg }}
        >
          Also remove the effect on the way out? Add the cleanup rule{" "}
          <b>{offeredMirror.name}</b> (When {offeredMirror.event}).{" "}
          <Button data-testid="mirror-accept" compact onClick={() => void acceptMirror()}>
            Add cleanup rule
          </Button>{" "}
          <Button data-testid="mirror-dismiss" variant="ghost" compact onClick={() => setOfferedMirror(null)}>
            No thanks
          </Button>
        </div>
      )}

      {rules.length === 0 ? (
        <div style={{ color: color.text.muted }}>No rules yet — author a When / If / Then rule.</div>
      ) : (
        rules.map((r) => {
          // Read off the rule, never off a count sent beside it — the two cannot disagree if there is
          // only one of them. The OR group stays a SEPARATE figure: "2 if" about a rule whose second
          // claim is "any one of these" would be a false statement about when it fires.
          const { name, enabled, event, conditions, any_of, actions } = r.rule;
          const anyOf = any_of?.length ?? 0;
          return (
            <div
              key={r.id}
              data-testid="rule-row"
              data-enabled={String(enabled)}
              style={{ display: "flex", justifyContent: "space-between", gap: space.md, padding: `${space.xs}px 0`, borderBottom: `1px solid ${color.border.subtle}` }}
            >
              {/* An off rule reads as off — dimmed, and it SAYS what off means rather than wearing a badge
                  the reader has to interpret (`<ux_quality>` 4). */}
              <span style={enabled ? undefined : { opacity: 0.55 }}>
                <b>{name}</b> · When {event} · {conditions.length} if
                {anyOf > 0 ? ` · any of ${anyOf}` : ""} · {actions.length} then
                {!enabled && <span style={{ color: color.text.muted }}> · off — does not run</span>}
              </span>
              <span style={{ display: "flex", gap: space.xs, flex: "0 0 auto", alignItems: "center" }}>
                <Button
                  variant="ghost"
                  compact
                  data-testid="rule-toggle"
                  aria-label={enabled ? `Turn off rule ${name}` : `Turn on rule ${name}`}
                  title={enabled ? "This rule runs. Turn it off to stop it without deleting it." : "This rule is off. Turn it on to let it run again."}
                  onClick={() => void toggle(r)}
                >
                  {enabled ? "turn off" : "turn on"}
                </Button>
                <Button
                  variant="ghost"
                  compact
                  icon
                  aria-label={`Remove rule ${name}`}
                  onClick={() => void remove(r.id)}
                  title="remove rule"
                >
                  ×
                </Button>
              </span>
            </div>
          );
        })
      )}
    </div>
  );
}
