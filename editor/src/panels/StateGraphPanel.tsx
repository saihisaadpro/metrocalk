//! M12.2 (ADR-046) — the **state-graph panel**: a registry-fed builder for **state machines as data**
//! (states + transitions, each transition an M12.1 Rule) drawn in the **visual state-graph** that reuses the
//! M2.5 React Flow layer ([`StateGraph`]). Every structural edit — add/rename/delete a state, draw/edit a
//! transition — is **one undoable transaction** committed through `author_state_machine` (a projection edit,
//! invariant 1/3 — never a direct graph-lib mutation). A dangling / typo'd transition is **Blocked +
//! explained** inline (ADR-016); **unreachable** states are surfaced as an explained warning. The Then of
//! every transition is the **auto-built** "enter `to`" action — so the effect can never typo the state field
//! (typo-proof by construction). Running the machine + the live current-state read are **M12.5** (the seam).

import { useEffect, useState } from "react";
import { useStore } from "zustand";
import { projectionStore } from "../store/projection";
import { pushToast } from "../store/toasts";
import { StateGraph } from "../graph/StateGraph";
import type { EditorClient } from "../transport/session";
import type {
  CompareOp,
  FieldValue,
  RuleCondition,
  RuleRegistryInfo,
  StateMachine,
  StateMachineInfo,
  Transition,
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

/** Mirrors `core::state_machine::ENTER_STATE_ACTION` — the verb a transition's Then uses to enter `to`. */
const ENTER_ACTION = "SetField";

function fieldTy(reg: RuleRegistryInfo, component: string, field: string): string {
  return reg.components.find((c) => c.name === component)?.fields.find((f) => f.name === field)?.ty ?? "string";
}
function defaultValue(ty: string): FieldValue {
  if (ty === "integer") return { Integer: 0 };
  if (ty === "number") return { Number: 0 };
  if (ty === "boolean") return { Bool: false };
  return { Str: "" };
}
function rawValue(v: FieldValue): string {
  if ("Integer" in v) return String(v.Integer);
  if ("Number" in v) return String(v.Number);
  if ("Bool" in v) return String(v.Bool);
  return v.Str;
}

/** The canonical "enter `to`" action (typo-proof: the Then is generated from the machine's own state field,
 *  never hand-typed) — the TS twin of `StateMachine::enter_action`. */
function enterAction(m: StateMachine, to: string) {
  return { action: ENTER_ACTION, entity: m.entity, component: m.component, field: m.field, value: { Str: to } };
}

/** Build a full [`Transition`] from its editable parts, deriving the canonical enter-action so the transition
 *  is always a valid "move the state to `to`" Rule. */
function mkTransition(
  m: StateMachine,
  t: { id: string; from: string; to: string; event: string; conditions: RuleCondition[] },
): Transition {
  return {
    id: t.id,
    from: t.from,
    to: t.to,
    rule: {
      name: `${t.from} -> ${t.to}`,
      enabled: true,
      event: t.event,
      conditions: t.conditions,
      actions: [enterAction(m, t.to)],
    },
  };
}

/** A value input whose KIND is dictated by the field's registry type (typo-proof — same discipline as the
 *  Rules builder). */
function ValueInput({ ty, value, onChange }: { ty: string; value: FieldValue; onChange: (v: FieldValue) => void }) {
  if (ty === "boolean") {
    return (
      <select
        data-testid="sm-cond-value"
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
        data-testid="sm-cond-value"
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
      data-testid="sm-cond-value"
      type="text"
      style={{ ...ctrl, width: 96 }}
      value={rawValue(value)}
      onChange={(e) => onChange({ Str: e.target.value })}
    />
  );
}

/** The optional **If** guard editor for a transition — registry-fed `component.field op value` rows (reusing
 *  the typo-proof vocabulary). Empty = the transition fires whenever its event does (in M12.5). */
function ConditionEditor({
  reg,
  entityOptions,
  conditions,
  onChange,
}: {
  reg: RuleRegistryInfo;
  entityOptions: { id: string; name: string }[];
  conditions: RuleCondition[];
  onChange: (next: RuleCondition[]) => void;
}) {
  const firstComp = reg.components[0]?.name ?? "";
  const firstField = reg.components[0]?.fields[0]?.name ?? "";
  const add = () =>
    onChange([
      ...conditions,
      {
        entity: entityOptions[0]?.id ?? "",
        component: firstComp,
        field: firstField,
        op: "ge",
        value: defaultValue(fieldTy(reg, firstComp, firstField)),
      },
    ]);
  return (
    <div style={{ marginLeft: 14 }}>
      <button data-testid="sm-add-cond" style={ctrl} onClick={add}>
        + if
      </button>
      {conditions.map((c, i) => {
        const set = (patch: Partial<RuleCondition>) => {
          const next = [...conditions];
          next[i] = { ...c, ...patch };
          if (patch.component || patch.field) {
            next[i].value = defaultValue(fieldTy(reg, next[i].component, next[i].field));
          }
          onChange(next);
        };
        const compFields = reg.components.find((x) => x.name === c.component)?.fields ?? [];
        return (
          <div key={i} data-testid="sm-cond" style={{ display: "flex", gap: 4, flexWrap: "wrap", margin: "3px 0" }}>
            <select data-testid="sm-cond-entity" style={ctrl} value={c.entity} onChange={(e) => set({ entity: e.target.value })}>
              <option value="">— entity —</option>
              {entityOptions.map((o) => (
                <option key={o.id} value={o.id}>
                  {o.name}
                </option>
              ))}
            </select>
            <select
              data-testid="sm-cond-component"
              style={ctrl}
              value={c.component}
              onChange={(e) => {
                const comp = reg.components.find((x) => x.name === e.target.value);
                set({ component: e.target.value, field: comp?.fields[0]?.name ?? "" });
              }}
            >
              {reg.components.map((comp) => (
                <option key={comp.name} value={comp.name}>
                  {comp.name}
                </option>
              ))}
            </select>
            <select data-testid="sm-cond-field" style={ctrl} value={c.field} onChange={(e) => set({ field: e.target.value })}>
              {compFields.map((f) => (
                <option key={f.name} value={f.name}>
                  {f.name}
                </option>
              ))}
            </select>
            <select data-testid="sm-cond-op" style={ctrl} value={c.op} onChange={(e) => set({ op: e.target.value as CompareOp })}>
              {OPS.map((o) => (
                <option key={o.op} value={o.op}>
                  {o.label}
                </option>
              ))}
            </select>
            <ValueInput ty={fieldTy(reg, c.component, c.field)} value={c.value} onChange={(v) => set({ value: v })} />
            <button style={ctrl} onClick={() => onChange(conditions.filter((_, j) => j !== i))}>
              ×
            </button>
          </div>
        );
      })}
    </div>
  );
}

export function StateGraphPanel({ client }: { client: EditorClient }) {
  const [reg, setReg] = useState<RuleRegistryInfo | null>(null);
  const [machines, setMachines] = useState<StateMachineInfo[]>([]);
  const [draft, setDraft] = useState<StateMachine | null>(null);
  const [currentId, setCurrentId] = useState<string | null>(null);
  const [current, setCurrent] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [unreachable, setUnreachable] = useState<string[]>([]);
  const [stateEdits, setStateEdits] = useState<Record<number, string>>({});

  const summaries = useStore(projectionStore, (s) => s.summaries);
  const selectedId = useStore(projectionStore, (s) => s.selectedId);
  const entityOptions = Object.values(summaries)
    .sort((a, b) => a.name.localeCompare(b.name))
    .slice(0, 200)
    .map((s) => ({ id: s.id, name: s.name }));

  const refreshList = () => client.stateMachines().then(setMachines).catch(() => {});
  useEffect(() => {
    void client.ruleRegistry().then(setReg).catch(() => {});
    refreshList();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client]);

  // Only components with a String field can hold a state name (typo-proof target — the validator enforces it).
  const stringComps = reg ? reg.components.filter((c) => c.fields.some((f) => f.ty === "string")) : [];

  function newDraft(): StateMachine {
    const comp = stringComps.find((c) => c.name === "QuestState") ?? stringComps[0];
    const field = comp?.fields.find((f) => f.ty === "string")?.name ?? "state";
    return {
      name: "quest",
      entity: selectedId ?? entityOptions[0]?.id ?? "",
      component: comp?.name ?? "QuestState",
      field,
      states: ["Hunting", "ReadyForBoss", "FacingBoss"],
      initial: "Hunting",
      transitions: [],
    };
  }

  /** Commit a machine through the pipeline (`author_state_machine`) — one undoable tx. On success, refetch so
   *  the draft reflects the committed machine (server-stamped transition ids) + the unreachable warning; on a
   *  Blocked reason, keep the draft + show the explanation inline (ADR-016). */
  async function save(next: StateMachine) {
    setDraft(next);
    setError(null);
    let r;
    try {
      r = await client.authorStateMachine(next, currentId);
    } catch {
      setError("could not save the state machine - please try again");
      return;
    }
    if (r.error) {
      setError(r.error);
      setUnreachable([]);
      return;
    }
    setUnreachable(r.unreachable);
    const id = r.id;
    setCurrentId(id);
    const infos = await client.stateMachines().catch(() => [] as StateMachineInfo[]);
    setMachines(infos);
    const info = id ? infos.find((m) => m.id === id) : undefined;
    if (info) {
      setDraft(info.machine);
      setCurrent(info.current);
    }
  }

  function startNew() {
    if (!reg) return;
    setCurrentId(null);
    setCurrent(null);
    setUnreachable([]);
    setStateEdits({});
    const d = newDraft();
    setDraft(d);
    void save(d);
  }

  function loadMachine(info: StateMachineInfo) {
    setCurrentId(info.id);
    setCurrent(info.current);
    setDraft(info.machine);
    setError(null);
    setUnreachable([]);
    setStateEdits({});
  }

  async function deleteMachine() {
    if (!currentId) return;
    await client.deleteStateMachine(currentId).catch(() => false);
    pushToast("State machine removed · Ctrl-Z to undo", "info");
    setDraft(null);
    setCurrentId(null);
    refreshList();
  }

  // ── state edits (each one undoable tx) ──────────────────────────────────
  function addState() {
    if (!draft) return;
    let n = draft.states.length + 1;
    let name = `State${n}`;
    while (draft.states.includes(name)) name = `State${++n}`;
    save({ ...draft, states: [...draft.states, name] });
  }
  function renameState(idx: number, raw: string) {
    if (!draft) return;
    const oldName = draft.states[idx];
    const newName = raw.trim();
    if (!newName || newName === oldName || draft.states.includes(newName)) return;
    const states = draft.states.map((s) => (s === oldName ? newName : s));
    const initial = draft.initial === oldName ? newName : draft.initial;
    const transitions = draft.transitions.map((t) =>
      mkTransition(draft, {
        id: t.id,
        from: t.from === oldName ? newName : t.from,
        to: t.to === oldName ? newName : t.to,
        event: t.rule.event,
        conditions: t.rule.conditions,
      }),
    );
    save({ ...draft, states, initial, transitions });
  }
  function deleteState(idx: number) {
    if (!draft) return;
    const name = draft.states[idx];
    const states = draft.states.filter((_, i) => i !== idx);
    if (states.length === 0) return; // a machine needs at least one state
    const initial = draft.initial === name ? states[0] : draft.initial;
    // Drop transitions that touch the removed state (no dangling edge left behind).
    const transitions = draft.transitions.filter((t) => t.from !== name && t.to !== name);
    save({ ...draft, states, initial, transitions });
  }
  function setInitial(name: string) {
    if (!draft) return;
    save({ ...draft, initial: name });
  }

  // ── transition edits (each one undoable tx) ─────────────────────────────
  function addTransition() {
    if (!draft || draft.states.length === 0) return;
    const from = draft.initial;
    const to = draft.states.find((s) => s !== from) ?? from;
    const event = reg?.events[0]?.name ?? "";
    // A blank id → the shell stamps a stable, peer-namespaced edge id on commit.
    const t = mkTransition(draft, { id: "", from, to, event, conditions: [] });
    save({ ...draft, transitions: [...draft.transitions, t] });
  }
  function editTransition(idx: number, patch: { from?: string; to?: string; event?: string; conditions?: RuleCondition[] }) {
    if (!draft) return;
    const t = draft.transitions[idx];
    const next = mkTransition(draft, {
      id: t.id,
      from: patch.from ?? t.from,
      to: patch.to ?? t.to,
      event: patch.event ?? t.rule.event,
      conditions: patch.conditions ?? t.rule.conditions,
    });
    const transitions = [...draft.transitions];
    transitions[idx] = next;
    save({ ...draft, transitions });
  }
  function deleteTransition(idx: number) {
    if (!draft) return;
    save({ ...draft, transitions: draft.transitions.filter((_, i) => i !== idx) });
  }

  // ── target (entity / component / field) ─────────────────────────────────
  function setTarget(patch: { entity?: string; component?: string; field?: string }) {
    if (!draft) return;
    let next = { ...draft, ...patch };
    if (patch.component) {
      const comp = stringComps.find((c) => c.name === patch.component);
      const field = comp?.fields.find((f) => f.ty === "string")?.name ?? draft.field;
      next = { ...next, field };
    }
    // Re-derive each transition's enter-action against the new target.
    next.transitions = next.transitions.map((t) =>
      mkTransition(next, { id: t.id, from: t.from, to: t.to, event: t.rule.event, conditions: t.rule.conditions }),
    );
    save(next);
  }

  return (
    <div id="stategraph" data-testid="state-graph-panel" style={{ ...box, borderTop: "1px solid #2a2d35" }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 6 }}>
        <b>State machines</b>
        <span>
          <button data-testid="sm-new" style={ctrl} disabled={!reg} onClick={startNew}>
            + New machine
          </button>{" "}
          {currentId && (
            <button data-testid="sm-delete" style={ctrl} onClick={() => void deleteMachine()} title="remove machine">
              delete
            </button>
          )}
        </span>
      </div>

      {/* machine list */}
      {machines.length > 0 && (
        <div style={{ marginBottom: 8 }}>
          {machines.map((m) => (
            <button
              key={m.id}
              data-testid="sm-row"
              data-sm-id={m.id}
              style={{ ...ctrl, marginRight: 4, fontWeight: m.id === currentId ? 700 : 400 }}
              onClick={() => loadMachine(m)}
            >
              {m.machine.name} ({m.machine.states.length})
            </button>
          ))}
        </div>
      )}

      {draft && reg && (
        <>
          {/* target */}
          <div style={{ display: "flex", gap: 4, flexWrap: "wrap", marginBottom: 6 }}>
            <input
              data-testid="sm-name"
              type="text"
              placeholder="machine name"
              style={{ ...ctrl, width: 120 }}
              value={draft.name}
              onChange={(e) => setDraft({ ...draft, name: e.target.value })}
              onBlur={() => save(draft)}
            />
            <select data-testid="sm-entity" style={ctrl} value={draft.entity} onChange={(e) => setTarget({ entity: e.target.value })}>
              <option value="">— entity —</option>
              {entityOptions.map((o) => (
                <option key={o.id} value={o.id}>
                  {o.name}
                </option>
              ))}
            </select>
            <select data-testid="sm-component" style={ctrl} value={draft.component} onChange={(e) => setTarget({ component: e.target.value })}>
              {stringComps.map((c) => (
                <option key={c.name} value={c.name}>
                  {c.name}
                </option>
              ))}
            </select>
            <select data-testid="sm-field" style={ctrl} value={draft.field} onChange={(e) => setTarget({ field: e.target.value })}>
              {(stringComps.find((c) => c.name === draft.component)?.fields ?? [])
                .filter((f) => f.ty === "string")
                .map((f) => (
                  <option key={f.name} value={f.name}>
                    {f.name}
                  </option>
                ))}
            </select>
          </div>

          {/* the visual state-graph (reuses the M2.5 React Flow layer) */}
          <StateGraph machine={draft} current={current} />

          {error && (
            <div data-testid="sm-error" style={{ color: "#f88", margin: "4px 0" }}>
              {error}
            </div>
          )}
          {unreachable.length > 0 && (
            <div data-testid="sm-unreachable" style={{ color: "#fb4", margin: "4px 0" }}>
              Unreachable from {draft.initial}: {unreachable.join(", ")} — add a transition into{" "}
              {unreachable.length === 1 ? "it" : "them"}.
            </div>
          )}

          {/* states */}
          <div style={{ marginTop: 6 }}>
            <b>States</b>{" "}
            <button data-testid="sm-add-state" style={ctrl} onClick={addState}>
              + state
            </button>
            {draft.states.map((s, i) => (
              <div key={i} data-testid="sm-state" style={{ display: "flex", gap: 4, alignItems: "center", margin: "3px 0" }}>
                <input
                  data-testid="sm-state-name"
                  type="text"
                  style={{ ...ctrl, width: 120 }}
                  value={stateEdits[i] ?? s}
                  onChange={(e) => setStateEdits({ ...stateEdits, [i]: e.target.value })}
                  onBlur={(e) => {
                    renameState(i, e.target.value);
                    const rest = { ...stateEdits };
                    delete rest[i];
                    setStateEdits(rest);
                  }}
                />
                <label style={{ color: "#9bf" }}>
                  <input type="radio" data-testid="sm-initial" name="sm-initial" checked={draft.initial === s} onChange={() => setInitial(s)} /> initial
                </label>
                <button data-testid="sm-state-delete" style={ctrl} onClick={() => deleteState(i)} title="delete state">
                  ×
                </button>
              </div>
            ))}
          </div>

          {/* transitions */}
          <div style={{ marginTop: 6 }}>
            <b>Transitions</b>{" "}
            <button data-testid="sm-add-transition" style={ctrl} onClick={addTransition}>
              + transition
            </button>
            {draft.transitions.map((t, i) => (
              <div key={t.id || i} data-testid="sm-transition" data-edge-id={t.id} style={{ border: "1px solid #23262d", borderRadius: 4, padding: 4, margin: "3px 0" }}>
                <div style={{ display: "flex", gap: 4, flexWrap: "wrap", alignItems: "center" }}>
                  <select data-testid="sm-trans-from" style={ctrl} value={t.from} onChange={(e) => editTransition(i, { from: e.target.value })}>
                    {draft.states.map((s) => (
                      <option key={s} value={s}>
                        {s}
                      </option>
                    ))}
                  </select>
                  <span>→</span>
                  <select data-testid="sm-trans-to" style={ctrl} value={t.to} onChange={(e) => editTransition(i, { to: e.target.value })}>
                    {draft.states.map((s) => (
                      <option key={s} value={s}>
                        {s}
                      </option>
                    ))}
                  </select>
                  <span>when</span>
                  <select data-testid="sm-trans-event" style={ctrl} value={t.rule.event} onChange={(e) => editTransition(i, { event: e.target.value })}>
                    {reg.events.map((ev) => (
                      <option key={ev.name} value={ev.name} title={ev.description}>
                        {ev.name}
                      </option>
                    ))}
                  </select>
                  <button data-testid="sm-trans-delete" style={ctrl} onClick={() => deleteTransition(i)} title="delete transition">
                    ×
                  </button>
                </div>
                <ConditionEditor
                  reg={reg}
                  entityOptions={entityOptions}
                  conditions={t.rule.conditions}
                  onChange={(conds) => editTransition(i, { conditions: conds })}
                />
              </div>
            ))}
          </div>
        </>
      )}

      {!draft && machines.length === 0 && (
        <div style={{ color: "#888" }}>No state machines yet — click “+ New machine” to build one.</div>
      )}
    </div>
  );
}
