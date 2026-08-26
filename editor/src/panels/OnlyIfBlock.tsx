//! "Only if…" — conditionals the user reads as a sentence (Engine UI/UX Constitution).
//!
//! The research behind this shape: query-builder UIs (Notion/Airtable/Gmail) beat node graphs for
//! non-programmers because a clause is ONE row of plain words with no dangling wires; structured editing
//! beats free text because invalid states are unreachable by construction; and the single most valuable
//! live-feedback affordance is answering *"why did nothing happen?"* with the actual runtime value.
//!
//! So: named clause CARDS (each fixes component+field+operator, so "boolean blindness" and meaningless
//! comparisons cannot be expressed), one AND list plus one OR group (depth is 2 by construction — no
//! nesting, ever), the whole rule echoed back as a sentence, and during Play a ⛔ line naming the first
//! failing clause with its live value.

import { useCallback, useEffect, useState } from "react";
import { useSelectedId } from "../store/projection";
import { usePlaying } from "../store/play";
import { setStatus } from "../store/ui";
import { pushToast } from "../store/toasts";
import { Icon } from "../theme/icons";
import { Button, NumericField, SelectField } from "../theme/primitives";
import { color, fontSize, radius, space } from "../theme/tokens";
import type { ClauseRequest, ConditionListInfo, ConditionSpec, RoleRow } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

export interface OnlyIfBlockProps {
  client: EditorClient;
  /** The roster, so the "another object" cards can offer real, role-carrying objects. */
  roster: RoleRow[];
  /** The live near-miss line during Play ("the Score is 0, needs at least 3"). */
  blockedWhy?: string | null;
  /** Bumped by the parent when the role changes, so the sentence re-reads. */
  refreshKey?: number;
}

const EMPTY: ConditionListInfo = { all: [], any: [], roleClause: null, sentence: "" };

export function OnlyIfBlock({ client, roster, blockedWhy, refreshKey = 0 }: OnlyIfBlockProps) {
  const selected = useSelectedId();
  const playing = usePlaying();
  const [specs, setSpecs] = useState<ConditionSpec[]>([]);
  const [list, setList] = useState<ConditionListInfo>(EMPTY);
  const [pick, setPick] = useState<string>("");
  const [num, setNum] = useState(1);
  const [obj, setObj] = useState("");
  const [asAny, setAsAny] = useState(false);
  const [busy, setBusy] = useState(false);
  const [refusal, setRefusal] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    client
      .conditionCatalog()
      .then((s) => live && setSpecs(s))
      .catch((e: unknown) => console.error("condition_catalog failed", e));
    return () => {
      live = false;
    };
  }, [client]);

  const reload = useCallback(() => {
    if (!selected) {
      setList(EMPTY);
      return;
    }
    client
      .conditionList(selected)
      .then(setList)
      .catch((e: unknown) => console.error("condition_list failed", e));
  }, [client, selected]);

  useEffect(() => {
    setRefusal(null);
    setPick("");
    reload();
  }, [reload, refreshKey]);

  if (!selected) return null;

  const spec = specs.find((s) => s.kind === pick) ?? null;
  // Only role-carrying objects other than this one can be talked about — an object with no role can
  // never be "gone", so it is not offered rather than authored into a permanently-false clause.
  const others = roster.filter((r) => r.entity !== selected);

  async function add() {
    if (!selected || !spec) return;
    setBusy(true);
    setRefusal(null);
    const request: ClauseRequest = {
      kind: spec.kind,
      number: spec.needs === "number" ? num : null,
      object: spec.needs === "object" ? obj || others[0]?.entity || null : null,
      any: asAny,
    };
    try {
      const reply = await client.conditionAdd(selected, request);
      if (reply.reason) {
        setRefusal(reply.reason);
        setStatus(`Condition refused: ${reply.reason}`);
      } else {
        pushToast(`${reply.message} · Ctrl-Z to undo`, "success");
        setStatus(reply.message);
        setPick("");
        reload();
      }
    } catch (e) {
      console.error("condition_add failed", e);
      pushToast("The condition could not be added — please try again", "error");
    } finally {
      setBusy(false);
    }
  }

  async function remove(index: number, any: boolean) {
    if (!selected) return;
    setBusy(true);
    try {
      const reply = await client.conditionRemove(selected, index, any);
      if (reply.reason) setRefusal(reply.reason);
      else {
        pushToast(`${reply.message} · Ctrl-Z to undo`, "success");
        reload();
      }
    } catch (e) {
      console.error("condition_remove failed", e);
    } finally {
      setBusy(false);
    }
  }

  const chip = (label: string, onRemove?: () => void, muted = false) => (
    <span
      key={label}
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: space.xs,
        padding: `2px ${space.sm}px`,
        borderRadius: radius.sm,
        fontSize: fontSize.meta,
        background: muted ? color.bg.inset : color.accent.subtle,
        border: `1px solid ${muted ? color.border.subtle : color.accent.border}`,
        color: muted ? color.text.muted : color.text.primary,
      }}
    >
      {label}
      {onRemove && (
        <Button variant="ghost" compact aria-label={`Remove: ${label}`} disabled={busy || playing} onClick={onRemove}>
          ×
        </Button>
      )}
    </span>
  );

  const canAdd = !!spec && (spec.needs !== "object" || others.length > 0);

  return (
    <section data-testid="onlyif-block" style={{ display: "grid", gap: space.xs }}>
      <div style={{ fontSize: fontSize.meta, color: color.text.muted }}>
        Only if… <span style={{ opacity: 0.8 }}>— add a condition and this only happens when it holds</span>
      </div>

      {/* The live near-miss: the answer to "I touched it and nothing happened". */}
      {playing && blockedWhy && (
        <div
          data-testid="onlyif-blocked"
          role="status"
          style={{
            padding: `${space.xs}px ${space.sm}px`,
            borderRadius: radius.sm,
            fontSize: fontSize.meta,
            background: color.warn.bg,
            border: `1px solid ${color.warn.border}`,
            color: color.warn.text,
          }}
        >
          <Icon name="blocked" size="sm" /> Blocked just now — {blockedWhy}
        </div>
      )}

      <div style={{ display: "flex", flexWrap: "wrap", gap: space.xs, alignItems: "center" }}>
        {list.roleClause && chip(`${list.roleClause} (from the role)`, undefined, true)}
        {list.all.map((c) => chip(c.reads, () => void remove(c.index, false)))}
        {list.any.length > 0 && (
          <span data-testid="onlyif-any-group" style={{ fontSize: fontSize.meta, color: color.text.muted }}>
            either
          </span>
        )}
        {list.any.map((c) => chip(c.reads, () => void remove(c.index, true)))}
      </div>

      {!playing && (
        <div style={{ display: "flex", flexWrap: "wrap", gap: space.xs, alignItems: "center" }}>
          <SelectField
            data-testid="onlyif-pick"
            aria-label="Add a condition"
            value={pick}
            disabled={busy}
            onChange={(e) => setPick(e.target.value)}
            style={{ fontSize: fontSize.meta }}
          >
            <option value="">+ add a condition…</option>
            {specs.map((s) => (
              <option key={s.kind} value={s.kind} title={s.blurb}>
                {s.label}
              </option>
            ))}
          </SelectField>

          {spec?.needs === "number" && (
            <NumericField
              data-testid="onlyif-number"
              ariaLabel="Condition value"
              value={num}
              step={1}
              integer
              min={0}
              max={999}
              onCommit={setNum}
            />
          )}
          {spec?.needs === "object" && (
            <SelectField
              data-testid="onlyif-object"
              aria-label="Which object"
              value={obj || others[0]?.entity || ""}
              onChange={(e) => setObj(e.target.value)}
              style={{ fontSize: fontSize.meta }}
            >
              {others.map((r) => (
                <option key={r.entity} value={r.entity}>
                  {r.name}
                </option>
              ))}
            </SelectField>
          )}

          {spec && (
            <>
              <Button
                data-testid="onlyif-add"
                variant="secondary"
                compact
                disabled={busy || !canAdd}
                title={
                  canAdd
                    ? "Add this condition — one step, Ctrl-Z undoes it"
                    : "Give another object a role first, so there is something to talk about"
                }
                onClick={() => void add()}
              >
                {asAny ? "+ or" : "+ and"}
              </Button>
              <Button
                data-testid="onlyif-join"
                variant="ghost"
                compact
                active={asAny}
                title="Join as an alternative (any one of the alternatives is enough)"
                onClick={() => setAsAny(!asAny)}
              >
                {asAny ? "as an alternative" : "make it an alternative"}
              </Button>
            </>
          )}
        </div>
      )}

      {list.sentence && (
        <div
          data-testid="onlyif-sentence"
          style={{ fontSize: fontSize.meta, color: color.text.secondary, fontStyle: "italic" }}
        >
          {list.sentence}
        </div>
      )}

      {refusal && (
        <div
          data-testid="onlyif-refusal"
          role="status"
          style={{
            padding: `${space.xs}px ${space.sm}px`,
            borderRadius: radius.sm,
            fontSize: fontSize.meta,
            background: color.danger.bg,
            border: `1px solid ${color.danger.border}`,
            color: color.danger.text,
          }}
        >
          {refusal}
        </div>
      )}
    </section>
  );
}
