//! M12.2 (ADR-046) — the **state-machine editor**: a registry-fed builder for **state machines as data**
//! (states + transitions, each transition an M12.1 Rule) drawn in the **visual state-graph** that reuses the
//! M2.5 React Flow layer ([`StateGraph`]). Every structural edit — add/rename/delete a state, draw/edit a
//! transition — is **one undoable transaction** committed through `author_state_machine` (a projection edit,
//! invariant 1/3 — never a direct graph-lib mutation). A dangling / typo'd transition is **Blocked +
//! explained** inline (ADR-016); **unreachable** states are surfaced as an explained warning. The Then of
//! every transition is the **auto-built** "enter `to`" action — so the effect can never typo the state field
//! (typo-proof by construction). Running the machine + the live current-state read are **M12.5** (the seam).
//!
//! IT DREW ITS OWN CONTROLS, AND ITS OWN LAYOUT. Until this pass the panel was 25 raw browser widgets —
//! `<select>`, `<input>`, `<button>` — carrying one hand-written `ctrl` style of monospace 11px on a 30px
//! box, under `<b>` tags used as headings, with a bare OS radio for the initial state and `×` for every
//! delete. None of it came from the shared control family, so the one editor a beginner reaches for to say
//! "when this happens, go there" looked like a different application from the Model workspace one tab over.
//! It now composes from the same vocabulary as every other surface: `WorkspacePanel` · `NavRail` ·
//! `CanvasSplit` · `DisclosureSection` · `Field` · `Callout` · `Radio` · `ListRow` · `Button`.
//!
//! AND THE SHAPE OF THE PANEL WAS WRONG FOR THE DOCK IT LIVES IN. The Logic dock is a WIDE, SHORT band
//! (~1240x520). Stacking the graph above the states above the transitions put both editable lists below
//! the fold, so the panel photographed as a diagram with nothing to do — the same defect ADR-124 found in
//! the Model workspace, whose primary action was painted 127px below the dock. The subject now takes the
//! room it deserves and the controls that shape it sit beside it, which is the constitution's own layout
//! rule ("viewport centre, properties right") applied one level down.
//!
//! THREE SILENT REFUSALS ARE NOW SPOKEN. Renaming a state to a name already taken, or to nothing, used to
//! revert the field on blur with no message anywhere; deleting the last state did nothing at all; and a
//! guard's operator list offered `<` and `>` on booleans and names, which the Rules builder one tab over
//! had already removed as meaningless. The first says why, at the row; the second is a disabled control
//! with its reason; the third is gone, because both builders now render the SAME clause row
//! ([`ruleClause`]) rather than two copies that had drifted.

import { useEffect, useId, useState } from "react";
import { useStore } from "zustand";
import { usePlaying } from "../store/play";
import { projectionStore } from "../store/projection";
import { pushToast } from "../store/toasts";
import { StateGraph } from "../graph/StateGraph";
import { Callout, Field, FieldGrid, ListRow, Radio, RadioGroup } from "../theme/fields";
import { Icon } from "../theme/icons";
import { Badge, Button, SelectField, TextField } from "../theme/primitives";
import { color, font, fontSize } from "../theme/tokens";
import {
  CanvasSplit,
  DisclosureSection,
  EmptyPanelState,
  NavRail,
  WorkspacePanel,
} from "../theme/workspace";
import { ClauseRow, defaultOp, defaultValue, fieldTy } from "./ruleClause";
import type { EditorClient } from "../transport/session";
import type {
  RuleCondition,
  RuleRegistryInfo,
  StateMachine,
  StateMachineInfo,
  Transition,
} from "../transport/protocol";

/** Mirrors `core::state_machine::ENTER_STATE_ACTION` — the verb a transition's Then uses to enter `to`. */
const ENTER_ACTION = "SetField";

/** The stable hooks this editor's guard rows carry; the Rules builder renders the same component under
 *  `rule-*`. One vocabulary, two test surfaces (see [`ruleClause`]). */
const GUARD_IDS = { prefix: "sm-cond", row: "sm-cond" } as const;

const MACHINE_ICON = <Icon name="logic" size="md" />;

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

/** The optional **Only if** guard on a transition — registry-fed `component.field op value` rows, rendered
 *  by the SAME clause component the Rules builder uses. Empty = the transition fires whenever its event
 *  does (in M12.5). */
function GuardEditor({
  reg,
  entityOptions,
  conditions,
  contextLabel,
  onChange,
}: {
  reg: RuleRegistryInfo;
  entityOptions: { id: string; name: string }[];
  conditions: RuleCondition[];
  contextLabel: string;
  onChange: (next: RuleCondition[]) => void;
}) {
  const firstComp = reg.components[0]?.name ?? "";
  const firstField = reg.components[0]?.fields[0]?.name ?? "";
  const add = () => {
    const ty = fieldTy(reg, firstComp, firstField);
    onChange([
      ...conditions,
      {
        entity: entityOptions[0]?.id ?? "",
        component: firstComp,
        field: firstField,
        op: defaultOp(ty),
        value: defaultValue(ty),
      },
    ]);
  };
  return (
    <div className="mtk-list-row__group">
      <div className="mtk-list-row__group-head">
        <span className="mtk-list-row__lead">
          {conditions.length === 0 ? "Fires whenever the event happens" : "Only if every one of these holds"}
        </span>
        <Button
          variant="ghost"
          compact
          data-testid="sm-add-cond"
          title="Add a condition that must hold for this transition to fire"
          onClick={add}
        >
          <Icon name="plus" size="sm" /> Add a condition
        </Button>
      </div>
      {conditions.map((c, i) => (
        <ClauseRow
          key={i}
          reg={reg}
          entityOptions={entityOptions}
          clause={c}
          label={`${contextLabel} condition ${i + 1}`}
          testIds={GUARD_IDS}
          onChange={(next) => onChange(conditions.map((existing, j) => (j === i ? next : existing)))}
          onRemove={() => onChange(conditions.filter((_, j) => j !== i))}
        />
      ))}
    </div>
  );
}

export function StateGraphPanel({ client }: { client: EditorClient }) {
  const fieldId = useId();
  // `StateMachineInfo.current` DEFAULTS TO `initial` on the shell, so it is a state name whether the
  // scene is running or not. Reading it as "where the machine is right now" would put a live readout
  // on a machine that has never run and mark its start state `live` in the graph — a claim the payload
  // cannot support (`<ux_quality>` 6, honest state). Whether the sim is running is a different fact,
  // and this store is the one that holds it.
  const playing = usePlaying();
  const [reg, setReg] = useState<RuleRegistryInfo | null>(null);
  const [machines, setMachines] = useState<StateMachineInfo[]>([]);
  const [draft, setDraft] = useState<StateMachine | null>(null);
  const [currentId, setCurrentId] = useState<string | null>(null);
  const [current, setCurrent] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [unreachable, setUnreachable] = useState<string[]>([]);
  const [stateEdits, setStateEdits] = useState<Record<number, string>>({});
  /** A rename the machine will not accept, said AT the list rather than swallowed on blur. */
  const [renameRefusal, setRenameRefusal] = useState<string | null>(null);
  /** The dock has a hard 520px ceiling, and the graph plus two section headings leave 68px for a
   *  143px transition card. Putting the graph away is what makes the second half of this editor
   *  usable at that height — the constitution's "everything collapsible", where it actually bites. */
  const [graphHidden, setGraphHidden] = useState(false);

  const summaries = useStore(projectionStore, (s) => s.summaries);
  const selectedId = useStore(projectionStore, (s) => s.selectedId);
  const entityOptions = Object.values(summaries)
    .sort((a, b) => a.name.localeCompare(b.name))
    .map((s) => ({ id: s.id, name: s.name }))
    .slice(0, 200);

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
    setRenameRefusal(null);
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
    setRenameRefusal(null);
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
    setRenameRefusal(null);
  }

  async function deleteMachine() {
    if (!currentId) return;
    await client.deleteStateMachine(currentId).catch(() => false);
    pushToast("State machine removed · Ctrl-Z to undo", "info");
    setDraft(null);
    setCurrentId(null);
    setCurrent(null);
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
  /** A rename the machine cannot accept, in the user's words — or `null` when it can. `""` means "the
   *  field was left as it was", which is not a refusal and must not produce a message. */
  function renameRefusalFor(draft: StateMachine, oldName: string, raw: string): string | null {
    const newName = raw.trim();
    if (newName === oldName) return null;
    if (!newName) return "A state needs a name — this one is back to how it was.";
    if (draft.states.includes(newName)) return `This machine already has a state called “${newName}”.`;
    return null;
  }
  function renameState(idx: number, raw: string) {
    if (!draft) return;
    const oldName = draft.states[idx];
    const newName = raw.trim();
    const refusal = renameRefusalFor(draft, oldName, raw);
    if (refusal) {
      setRenameRefusal(refusal);
      return;
    }
    setRenameRefusal(null);
    if (!newName || newName === oldName) return;
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
    if (states.length === 0) return; // a machine needs at least one state — the control says so and refuses
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

  const registryReason = reg ? undefined : "Still reading the component registry — this is ready in a moment.";
  const newMachine = (
    <Button
      variant={draft ? "secondary" : "primary"}
      compact
      data-testid="sm-new"
      disabled={!reg}
      disabledReason={registryReason}
      onClick={startNew}
    >
      <Icon name="plus" size="sm" /> New machine
    </Button>
  );

  if (!draft) {
    return (
      <WorkspacePanel
        title="State machines"
        subtitle="Behaviour as states, and the events that move between them"
        icon={MACHINE_ICON}
        data-testid="state-graph-panel"
        id="stategraph"
        actions={machines.length > 0 ? newMachine : undefined}
      >
        {machines.length > 0 ? (
          <div className="mtk-split">
            <MachineRail machines={machines} activeId={null} onPick={loadMachine} />
            <div className="mtk-split__main">
              <EmptyPanelState
                icon={MACHINE_ICON}
                title="Open a machine to edit it"
                description="Its states, the events between them and where the current state is stored all live here."
              />
            </div>
          </div>
        ) : (
          <EmptyPanelState
            data-testid="sm-empty"
            icon={MACHINE_ICON}
            title="No state machines yet"
            description="A state machine says what an object is doing — closed, opening, jammed — and which event moves it to the next one. Each edit is one undoable step."
            primaryAction={newMachine}
          />
        )}
      </WorkspacePanel>
    );
  }

  const entityName = summaries[draft.entity]?.name;
  const exits = new Map<string, number>();
  for (const t of draft.transitions) exits.set(t.from, (exits.get(t.from) ?? 0) + 1);
  const lastState = draft.states.length <= 1;
  const noEvents = (reg?.events.length ?? 0) === 0;

  return (
    <WorkspacePanel
      title="State machines"
      subtitle={`${draft.name} · ${entityName ?? "no object chosen"} · ${draft.component}.${draft.field}`}
      icon={MACHINE_ICON}
      data-testid="state-graph-panel"
      id="stategraph"
      // The rail must stay put while the editor beside it scrolls.
      scroll={false}
      actions={
        <>
          <Button
            variant="ghost"
            compact
            data-testid="sm-toggle-graph"
            // No `aria-expanded`/`aria-controls`: the graph is UNMOUNTED while hidden, so a control
            // relationship would point at nothing, and the label already says both the state and the
            // action. One signal, and it is the one a reader can see.
            title={graphHidden ? "Show the graph again" : "Put the graph away and give its room to the transitions"}
            onClick={() => setGraphHidden((hidden) => !hidden)}
          >
            <Icon name={graphHidden ? "expand" : "collapse"} size="sm" /> {graphHidden ? "Show graph" : "Hide graph"}
          </Button>
          {newMachine}
          {currentId && (
            <Button
              variant="ghost"
              compact
              data-testid="sm-delete"
              title="Remove this machine — Ctrl-Z brings it back"
              onClick={() => void deleteMachine()}
            >
              Remove
            </Button>
          )}
        </>
      }
      footer={
        <>
          <span className="mtk-workspace-panel__note">
            <Icon name="undo" size="sm" />
            Every edit here is saved as one undoable step
          </span>
          <div className="mtk-action-bar">
            {playing && current ? (
              <Badge tone="success" title="Where this machine is right now, live from the running scene">
                now in {current}
              </Badge>
            ) : (
              <span className="mtk-workspace-panel__note">Press Play to watch it move</span>
            )}
          </div>
        </>
      }
    >
      <div className="mtk-split">
        {machines.length > 0 && <MachineRail machines={machines} activeId={currentId} onPick={loadMachine} />}
        <div className="mtk-split__main">
          {error && (
            <Callout tone="danger" role="alert" data-testid="sm-error" title="This machine was not saved">
              {error}
            </Callout>
          )}
          {!draft.entity && (
            <Callout tone="warn" role="status" data-testid="sm-no-entity" title="This machine drives no object yet">
              Open “Where the state is stored” and choose the object whose state this machine moves.
            </Callout>
          )}
          {unreachable.length > 0 && (
            <Callout tone="warn" role="status" data-testid="sm-unreachable" title="Some states cannot be reached">
              Nothing leads to {unreachable.join(", ")} from {draft.initial} — add a transition into{" "}
              {unreachable.length === 1 ? "it" : "them"}.
            </Callout>
          )}

          <CanvasSplit
            canvasHidden={graphHidden}
            canvas={<StateGraph machine={draft} current={playing ? current : null} />}
            below={
            <DisclosureSection
              id="sm-transitions"
              title="Transitions"
              summary={draft.transitions.length === 0 ? "none yet" : `${draft.transitions.length}`}
              actions={
                <Button
                  variant="ghost"
                  compact
                  data-testid="sm-add-transition"
                  disabled={noEvents}
                  disabledReason="No events are registered yet, and a transition needs one to fire on."
                  onClick={addTransition}
                >
                  <Icon name="plus" size="sm" /> Add transition
                </Button>
              }
            >
              {draft.transitions.length === 0 && reg && (
                <p className="mtk-section-note">
                  Nothing moves this machine yet. A transition is “when this event happens, go to that state”.
                </p>
              )}
              {reg &&
                draft.transitions.map((t, i) => (
                  <ListRow key={t.id || i} tone="card" data-testid="sm-transition" data-id={t.id}>
                    <div className="mtk-list-row__line">
                      <SelectField
                        aria-label={`Transition ${i + 1} source state`}
                        data-testid="sm-trans-from"
                        // Same cap as the state-name field: a state name is a word, and two one-word
                        // dropdowns sharing a wide row should not each be 400px.
                        style={{ flex: "1 1 88px", minWidth: 72, maxWidth: 220 }}
                        value={t.from}
                        onChange={(e) => editTransition(i, { from: e.target.value })}
                      >
                        {draft.states.map((s) => (
                          <option key={s} value={s}>
                            {s}
                          </option>
                        ))}
                      </SelectField>
                      <Icon name="arrow-right" size="sm" />
                      <SelectField
                        aria-label={`Transition ${i + 1} destination state`}
                        data-testid="sm-trans-to"
                        style={{ flex: "1 1 88px", minWidth: 72, maxWidth: 220 }}
                        value={t.to}
                        onChange={(e) => editTransition(i, { to: e.target.value })}
                      >
                        {draft.states.map((s) => (
                          <option key={s} value={s}>
                            {s}
                          </option>
                        ))}
                      </SelectField>
                      <span className="mtk-list-row__lead">when</span>
                      <SelectField
                        aria-label={`Transition ${i + 1} trigger event`}
                        data-testid="sm-trans-event"
                        style={{ flex: "1 1 120px", minWidth: 96 }}
                        value={t.rule.event}
                        onChange={(e) => editTransition(i, { event: e.target.value })}
                      >
                        {reg.events.map((ev) => (
                          <option key={ev.name} value={ev.name} title={ev.description}>
                            {ev.name}
                          </option>
                        ))}
                      </SelectField>
                      <Button
                        variant="ghost"
                        compact
                        icon
                        aria-label={`Delete transition ${i + 1} from ${t.from} to ${t.to}`}
                        data-testid="sm-trans-delete"
                        title="Delete this transition"
                        onClick={() => deleteTransition(i)}
                      >
                        <Icon name="close" size="sm" />
                      </Button>
                    </div>
                    <GuardEditor
                      reg={reg}
                      entityOptions={entityOptions}
                      conditions={t.rule.conditions}
                      contextLabel={`Transition ${i + 1}`}
                      onChange={(conds) => editTransition(i, { conditions: conds })}
                    />
                  </ListRow>
                ))}
            </DisclosureSection>
            }
          >
            <DisclosureSection
              id="sm-states"
              title="States"
              summary={`${draft.states.length} · start is ${draft.initial}`}
              actions={
                <Button variant="ghost" compact data-testid="sm-add-state" onClick={addState}>
                  <Icon name="plus" size="sm" /> Add state
                </Button>
              }
            >
              {/* The help line IS the column caption: with it above the list, the mark on each row needs
                  no repeated word beside it, and the full sentence stays on the input for a screen
                  reader. Six rows each carrying the word "Start" is the noise the references never have. */}
              <RadioGroup label="Start state" help="Pick the state the machine starts in.">
                {draft.states.map((s, i) => {
                  const outs = exits.get(s) ?? 0;
                  return (
                    <ListRow key={i} data-testid="sm-state" data-id={s}>
                      <Radio
                        name="sm-initial"
                        data-testid="sm-initial"
                        label="Start"
                        labelHidden
                        ariaLabel={`Set ${s} as initial state`}
                        checked={draft.initial === s}
                        onChange={() => setInitial(s)}
                      />
                      <TextField
                        aria-label={`State ${i + 1} name`}
                        data-testid="sm-state-name"
                        // A state name is a word, not a paragraph: the field stops growing well before
                        // the row does, so a wide panel does not hand one word 900px.
                        style={{ flex: "1 1 88px", minWidth: 72, maxWidth: 260 }}
                        value={stateEdits[i] ?? s}
                        onChange={(e) => setStateEdits({ ...stateEdits, [i]: e.target.value })}
                        onBlur={(e) => {
                          renameState(i, e.target.value);
                          const rest = { ...stateEdits };
                          delete rest[i];
                          setStateEdits(rest);
                        }}
                      />
                      <span className="mtk-list-row__meta">{outs === 0 ? "no way out" : `${outs} exit${outs === 1 ? "" : "s"}`}</span>
                      <Button
                        variant="ghost"
                        compact
                        icon
                        aria-label={`Delete state ${s}`}
                        data-testid="sm-state-delete"
                        disabled={lastState}
                        disabledReason="A machine needs at least one state."
                        title={lastState ? undefined : `Delete ${s} and every transition that touches it`}
                        onClick={() => deleteState(i)}
                      >
                        <Icon name="close" size="sm" />
                      </Button>
                    </ListRow>
                  );
                })}
              </RadioGroup>
              {renameRefusal && (
                <Callout tone="warn" role="status" data-testid="sm-rename-refusal">
                  {renameRefusal}
                </Callout>
              )}
            </DisclosureSection>

            <DisclosureSection
              id="sm-target"
              title="Where the state is stored"
              summary={`${draft.component}.${draft.field}`}
              defaultOpen={false}
            >
              <FieldGrid minColumn={130}>
                <Field label="Name" htmlFor={`${fieldId}-name`} span="full" help="What you will call this machine.">
                  <TextField
                    id={`${fieldId}-name`}
                    aria-label="State machine name"
                    data-testid="sm-name"
                    placeholder="machine name"
                    value={draft.name}
                    onChange={(e) => setDraft({ ...draft, name: e.target.value })}
                    onBlur={() => save(draft)}
                  />
                </Field>
                <Field
                  label="Object"
                  htmlFor={`${fieldId}-entity`}
                  span="full"
                  help="The object this machine drives."
                >
                  <SelectField
                    id={`${fieldId}-entity`}
                    aria-label="State machine target entity"
                    data-testid="sm-entity"
                    value={draft.entity}
                    onChange={(e) => setTarget({ entity: e.target.value })}
                  >
                    <option value="">— entity —</option>
                    {entityOptions.map((o) => (
                      <option key={o.id} value={o.id}>
                        {o.name}
                      </option>
                    ))}
                  </SelectField>
                </Field>
                <Field
                  label="Component"
                  htmlFor={`${fieldId}-component`}
                  help="Only a component with a text field can hold a state name."
                >
                  <SelectField
                    id={`${fieldId}-component`}
                    aria-label="State machine target component"
                    data-testid="sm-component"
                    value={draft.component}
                    onChange={(e) => setTarget({ component: e.target.value })}
                  >
                    {stringComps.map((c) => (
                      <option key={c.name} value={c.name}>
                        {c.name}
                      </option>
                    ))}
                  </SelectField>
                </Field>
                <Field label="Field" htmlFor={`${fieldId}-field`} help="Where the current state's name is written.">
                  <SelectField
                    id={`${fieldId}-field`}
                    aria-label="State machine state field"
                    data-testid="sm-field"
                    value={draft.field}
                    onChange={(e) => setTarget({ field: e.target.value })}
                  >
                    {(stringComps.find((c) => c.name === draft.component)?.fields ?? [])
                      .filter((f) => f.ty === "string")
                      .map((f) => (
                        <option key={f.name} value={f.name}>
                          {f.name}
                        </option>
                      ))}
                  </SelectField>
                </Field>
              </FieldGrid>
            </DisclosureSection>
          </CanvasSplit>
        </div>
      </div>
    </WorkspacePanel>
  );
}

/** The machines this scene holds, as the rail every workspace with more than one document uses. Each row
 *  is a real tab, so the whole list is one stop in the tab order and the arrows move within it. */
function MachineRail({
  machines,
  activeId,
  onPick,
}: {
  machines: StateMachineInfo[];
  activeId: string | null;
  onPick: (info: StateMachineInfo) => void;
}) {
  return (
    <NavRail
      id="state-machines"
      data-testid="sm-list"
      label="State machines in this scene"
      activeId={activeId ?? ""}
      items={machines.map((m) => ({
        id: m.id,
        label: m.machine.name,
        icon: <Icon name="logic" size="md" />,
        tooltip: `${m.machine.states.length} states · ${m.machine.transitions.length} transitions`,
        badge: <span style={{ color: color.text.muted, font: font.ui, fontSize: fontSize.micro }}>{m.machine.states.length}</span>,
      }))}
      onChange={(id) => {
        const info = machines.find((m) => m.id === id);
        if (info) onPick(info);
      }}
    />
  );
}
