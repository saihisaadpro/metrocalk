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

import { useEffect, useRef, useState } from "react";
import { useStore } from "zustand";
import { projectionStore } from "../store/projection";
import { pushToast } from "../store/toasts";
import { Button, SelectField, TextField } from "../theme/primitives";
import { color, font, fontSize, radius, space } from "../theme/tokens";
import {
  ClauseRow,
  TargetPicker,
  ValueInput,
  defaultOp,
  defaultValue,
  fieldTy,
  grow,
} from "./ruleClause";
import type { EditorClient } from "../transport/session";
import type {
  RuleAction,
  RuleCondition,
  RuleData,
  RuleRegistryInfo,
  RuleSummary,
} from "../transport/protocol";

/** The stable hooks this builder's clause rows carry. The state-machine guard editor renders the same
 *  component under `sm-cond-*`; the vocabulary is shared, the test surface is each panel's own. */
const RULE_CLAUSE_IDS = { prefix: "rule" } as const;

const box: React.CSSProperties = { font: `${fontSize.meta}px ${font.mono}`, padding: space.lg };
/** One clause/action row: the controls wrap rather than overflow the dock at narrow widths. */
const row: React.CSSProperties = { display: "flex", gap: 4, flexWrap: "wrap", margin: "3px 0", alignItems: "center" };

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
            testIds={RULE_CLAUSE_IDS}
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
                testIds={RULE_CLAUSE_IDS}
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
              testIds={RULE_CLAUSE_IDS}
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
              testId="rule-value"
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
  /** The rule whose on/off write is in flight — one at a time, see `toggle`.
   *
   *  A **ref beside the state**, not state alone. The state is what disables the control; the ref is
   *  what actually holds the line, because two clicks in the same tick run the SAME render's `toggle`
   *  closure, in which a `useState` value is still the pre-click one. A guard that reads captured state
   *  is exactly as racy as no guard — its own test proved that by staying green when it was deleted. */
  const [pending, setPending] = useState<string | null>(null);
  const pendingRef = useRef<string | null>(null);

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
   *  every author; a toggle is not new authoring, so the offer is not surfaced here.
   *
   *  Two consequences of "replace" that the row's own cached copy would get wrong, so both are handled
   *  here rather than assumed away:
   *
   *  1. **Re-read before writing.** A replace sends the WHOLE rule, and this list has no change
   *     subscription — `refresh()` runs on mount and after its own mutations, nothing else. Writing back
   *     the copy the row last rendered would silently revert whatever changed since: an undo, a
   *     collaborator, an AI compose. The concrete case is one the toast itself invites — turn off,
   *     Ctrl-Z, and the row still reads "off" until something refreshes it.
   *  2. **One at a time.** Two fast clicks would otherwise both read `enabled: true`, both write
   *     `false`, and commit TWO undoable transactions for one intended change — after which one Ctrl-Z
   *     leaves the rule still off, having promised otherwise. `create()` already holds this line with
   *     its own `busy`. */
  async function toggle(id: string) {
    if (pendingRef.current) return;
    pendingRef.current = id;
    setPending(id);
    try {
      const current = (await client.listRules().catch(() => null))?.find((x) => x.id === id);
      if (!current) {
        pushToast("that rule is no longer in the document", "error");
        refresh();
        return;
      }
      const next = !current.rule.enabled;
      const res = await client.authorRule({ ...current.rule, enabled: next }, id).catch(() => null);
      if (!res || res.error) {
        pushToast(res?.error ?? `could not turn "${current.rule.name}" ${next ? "on" : "off"}`, "error");
      } else {
        pushToast(
          `"${current.rule.name}" is ${next ? "on — it runs again" : "off — it will not run"} · Ctrl-Z to undo`,
          "info",
        );
      }
      refresh();
    } finally {
      pendingRef.current = null;
      setPending(null);
    }
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
                  disabled={pending !== null}
                  aria-label={enabled ? `Turn off rule ${name}` : `Turn on rule ${name}`}
                  title={
                    pending !== null
                      ? "one moment — a rule is being switched"
                      : enabled
                        ? "This rule runs. Turn it off to stop it without deleting it."
                        : "This rule is off. Turn it on to let it run again."
                  }
                  onClick={() => void toggle(r.id)}
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
