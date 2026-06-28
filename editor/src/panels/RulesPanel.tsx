//! M12.1 (ADR-045) — the Rules panel: a **registry-fed builder** (every dropdown fed by `rule_registry`, so
//! authoring a When/If/Then conditional is **typo-proof by clicks** — no free text, no nil-refs) + the Rule
//! list. Authoring is one undoable transaction; a registry-rejected rule shows its **Blocked + explained**
//! reason inline (ADR-016); and when the engine offers a **mirror "cleanup" rule** (the missing-"off"-switch
//! guard) it's surfaced as a toast the user can accept. Running rules is M12.5.

import { useEffect, useState } from "react";
import { useStore } from "zustand";
import { projectionStore } from "../store/projection";
import { pushToast } from "../store/toasts";
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

const OPS: { op: CompareOp; label: string }[] = [
  { op: "eq", label: "=" },
  { op: "ne", label: "≠" },
  { op: "lt", label: "<" },
  { op: "le", label: "≤" },
  { op: "gt", label: ">" },
  { op: "ge", label: "≥" },
];

const box: React.CSSProperties = { font: "12px ui-monospace, monospace", padding: 10 };
const ctrl: React.CSSProperties = { font: "11px ui-monospace, monospace", padding: "1px 3px" };

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
function ValueInput({ ty, value, onChange }: { ty: string; value: FieldValue; onChange: (v: FieldValue) => void }) {
  if (ty === "boolean") {
    return (
      <select
        data-testid="rule-value"
        style={ctrl}
        value={"Bool" in value ? String(value.Bool) : "false"}
        onChange={(e) => onChange({ Bool: e.target.value === "true" })}
      >
        <option value="true">true</option>
        <option value="false">false</option>
      </select>
    );
  }
  if (ty === "integer" || ty === "number") {
    return (
      <input
        data-testid="rule-value"
        type="number"
        style={{ ...ctrl, width: 64 }}
        value={rawValue(value)}
        onChange={(e) => {
          const n = Number(e.target.value);
          onChange(ty === "integer" ? { Integer: Math.trunc(n) } : { Number: n });
        }}
      />
    );
  }
  return (
    <input
      data-testid="rule-value"
      type="text"
      style={{ ...ctrl, width: 96 }}
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
  onChange,
}: {
  reg: RuleRegistryInfo;
  entityOptions: { id: string; name: string }[];
  entity: string;
  component: string;
  field: string;
  onChange: (patch: { entity?: string; component?: string; field?: string }) => void;
}) {
  const fields = reg.components.find((c) => c.name === component)?.fields ?? [];
  return (
    <>
      <select data-testid="rule-entity" style={ctrl} value={entity} onChange={(e) => onChange({ entity: e.target.value })}>
        <option value="">— entity —</option>
        {entityOptions.map((o) => (
          <option key={o.id} value={o.id}>
            {o.name}
          </option>
        ))}
      </select>
      <select
        data-testid="rule-component"
        style={ctrl}
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
      </select>
      <select data-testid="rule-field" style={ctrl} value={field} onChange={(e) => onChange({ field: e.target.value })}>
        {fields.map((f) => (
          <option key={f.name} value={f.name}>
            {f.name}
          </option>
        ))}
      </select>
    </>
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
  const [actions, setActions] = useState<RuleAction[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const newCondition = (): RuleCondition => ({
    entity: defaultEntity,
    component: firstComp,
    field: firstField,
    op: "ge",
    value: defaultValue(fieldTy(reg, firstComp, firstField)),
  });
  const newAction = (): RuleAction => ({
    action: reg.actions[0]?.name ?? "",
    entity: defaultEntity,
    component: firstComp,
    field: firstField,
    value: defaultValue(fieldTy(reg, firstComp, firstField)),
  });

  async function create() {
    setError(null);
    setBusy(true);
    const rule: RuleData = { name, enabled: true, event, conditions, actions };
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
    <div data-testid="rule-builder" style={{ border: "1px solid #2a2d35", borderRadius: 4, padding: 8, marginBottom: 8 }}>
      <input
        data-testid="rule-name"
        type="text"
        placeholder="rule name"
        style={{ ...ctrl, width: "100%", marginBottom: 6 }}
        value={name}
        onChange={(e) => setName(e.target.value)}
      />
      <div style={{ marginBottom: 6 }}>
        <b>When</b>{" "}
        <select data-testid="rule-event" style={ctrl} value={event} onChange={(e) => setEvent(e.target.value)}>
          {reg.events.map((ev) => (
            <option key={ev.name} value={ev.name} title={ev.description}>
              {ev.name}
            </option>
          ))}
        </select>
      </div>

      <div style={{ marginBottom: 6 }}>
        <b>If</b>{" "}
        <button style={ctrl} onClick={() => setConditions([...conditions, newCondition()])}>
          + condition
        </button>
        {conditions.map((c, i) => (
          <div key={i} style={{ display: "flex", gap: 4, flexWrap: "wrap", margin: "3px 0" }}>
            <TargetPicker
              reg={reg}
              entityOptions={entityOptions}
              entity={c.entity}
              component={c.component}
              field={c.field}
              onChange={(p) => {
                const next = [...conditions];
                next[i] = { ...c, ...p };
                if (p.component || p.field) next[i].value = defaultValue(fieldTy(reg, next[i].component, next[i].field));
                setConditions(next);
              }}
            />
            <select
              data-testid="rule-op"
              style={ctrl}
              value={c.op}
              onChange={(e) => {
                const next = [...conditions];
                next[i] = { ...c, op: e.target.value as CompareOp };
                setConditions(next);
              }}
            >
              {OPS.map((o) => (
                <option key={o.op} value={o.op}>
                  {o.label}
                </option>
              ))}
            </select>
            <ValueInput
              ty={fieldTy(reg, c.component, c.field)}
              value={c.value}
              onChange={(v) => {
                const next = [...conditions];
                next[i] = { ...c, value: v };
                setConditions(next);
              }}
            />
            <button style={ctrl} onClick={() => setConditions(conditions.filter((_, j) => j !== i))}>
              ×
            </button>
          </div>
        ))}
      </div>

      <div style={{ marginBottom: 6 }}>
        <b>Then</b>{" "}
        <button style={ctrl} onClick={() => setActions([...actions, newAction()])}>
          + action
        </button>
        {actions.map((a, i) => (
          <div key={i} style={{ display: "flex", gap: 4, flexWrap: "wrap", margin: "3px 0" }}>
            <select
              data-testid="rule-action"
              style={ctrl}
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
            </select>
            <TargetPicker
              reg={reg}
              entityOptions={entityOptions}
              entity={a.entity}
              component={a.component}
              field={a.field}
              onChange={(p) => {
                const next = [...actions];
                next[i] = { ...a, ...p };
                if (p.component || p.field) next[i].value = defaultValue(fieldTy(reg, next[i].component, next[i].field));
                setActions(next);
              }}
            />
            <span>=</span>
            <ValueInput
              ty={fieldTy(reg, a.component, a.field)}
              value={a.value}
              onChange={(v) => {
                const next = [...actions];
                next[i] = { ...a, value: v };
                setActions(next);
              }}
            />
            <button style={ctrl} onClick={() => setActions(actions.filter((_, j) => j !== i))}>
              ×
            </button>
          </div>
        ))}
      </div>

      {error && (
        <div data-testid="rule-error" style={{ color: "#f88", margin: "4px 0" }}>
          {error}
        </div>
      )}
      <button data-testid="rule-create" style={{ ...ctrl, fontWeight: 700 }} disabled={busy} onClick={() => void create()}>
        {busy ? "creating…" : "Create rule"}
      </button>{" "}
      <button style={ctrl} onClick={() => onDone(null)}>
        cancel
      </button>
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

  async function acceptMirror() {
    if (!offeredMirror) return;
    const r = await client.authorRule(offeredMirror).catch(() => null);
    if (r && !r.error) pushToast(`Cleanup rule "${offeredMirror.name}" added`, "success");
    setOfferedMirror(null);
    refresh();
  }

  return (
    <div id="rules" data-testid="rules-panel" style={{ ...box, borderTop: "1px solid #2a2d35" }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 6 }}>
        <b>Rules</b>
        <button
          data-testid="rule-new"
          style={ctrl}
          disabled={!reg}
          onClick={() => setBuilding((b) => !b)}
        >
          {building ? "close" : "+ New rule"}
        </button>
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
          style={{ border: "1px solid #3a5", borderRadius: 4, padding: 6, marginBottom: 8, background: "#0c1a0c" }}
        >
          Also remove the effect on the way out? Add the cleanup rule{" "}
          <b>{offeredMirror.name}</b> (When {offeredMirror.event}).{" "}
          <button data-testid="mirror-accept" style={ctrl} onClick={() => void acceptMirror()}>
            Add cleanup rule
          </button>{" "}
          <button data-testid="mirror-dismiss" style={ctrl} onClick={() => setOfferedMirror(null)}>
            No thanks
          </button>
        </div>
      )}

      {rules.length === 0 ? (
        <div style={{ color: "#888" }}>No rules yet — author a When / If / Then rule.</div>
      ) : (
        rules.map((r) => (
          <div
            key={r.id}
            data-testid="rule-row"
            style={{ display: "flex", justifyContent: "space-between", gap: 8, padding: "3px 0", borderBottom: "1px solid #23262d" }}
          >
            <span>
              <b>{r.name}</b> · When {r.event} · {r.conditionCount} if · {r.actionCount} then
            </span>
            <button style={ctrl} onClick={() => void remove(r.id)} title="remove rule">
              ×
            </button>
          </div>
        ))
      )}
    </div>
  );
}
