//! M12.5 (ADR-049) — the **live truth-state debugger** ("debug by looking", north-star test #5 boxes 3–4).
//! In Play, click an entity → see its rules' **live truth** (✅/❌ per condition + the machine's current
//! state) — the *why* is **visible, not logged** — fire gameplay events to watch the counters climb, and
//! **scrub the decision history** backward to see exactly when a counter incremented / a transition fired.
//!
//! Everything here is a **projection** the shell serves off the Play-time `RuleReplay` (a runtime state, never
//! the Loro doc — ADR-021/034); this panel only reads + scrubs it. It shows ONLY in Play (the runtime is
//! live then); when Stopped it's hidden (authoring is the M12.1 builder's job). Stable ids
//! (`#ruleDebug`/`#fireEnemyDied`/`#ruleScrub`/`#truthRule-*`/`#truthCond-*`) for the prompt-40 page-object.

import { useEffect, useState } from "react";
import { useStore } from "zustand";
import { projectionStore } from "../store/projection";
import { usePlaying } from "../store/play";
import type { EditorClient } from "../transport/session";
import type { ConditionTruth, DecisionEvent, RuleDebugInfo } from "../transport/protocol";

const box: React.CSSProperties = { font: "12px ui-monospace, monospace", padding: 10, borderTop: "1px solid #2a2d35" };
const btn: React.CSSProperties = {
  font: "11px ui-monospace, monospace",
  padding: "2px 8px",
  background: "#1f2a3a",
  color: "#cfe3ff",
  border: "1px solid #2a3550",
  borderRadius: 4,
  cursor: "pointer",
};

/** A one-line, plain-language summary of a decision-history entry (so the history reads like a story). */
function describeDecision(d: DecisionEvent): string {
  switch (d.kind) {
    case "ruleFired":
      return `rule "${d.name ?? d.rule}" fired`;
    case "counterChanged":
      return `${d.component}.${d.field}: ${valStr(d.from)} → ${valStr(d.to)}`;
    case "fieldSet":
      return `${d.component}.${d.field} = ${valStr(d.value)}`;
    case "stateTransition":
      return `${d.machine}: ${String(d.from)} → ${String(d.to)}`;
    case "pluginInvoked":
      return `ran plugin "${d.plugin}"`;
    default:
      return "decision";
  }
}

/** Render a `FieldValue`/`RuntimeValue`-shaped scalar (or a plain string/number) as terse copy. */
function valStr(v: unknown): string {
  if (v === null || v === undefined) return "—";
  if (typeof v === "string" || typeof v === "number" || typeof v === "boolean") return String(v);
  const o = v as Record<string, unknown>;
  for (const k of ["Integer", "Number", "Bool", "Str"]) {
    if (k in o) return String(o[k]);
  }
  return String(v);
}

export function RuleDebugPanel({ client }: { client: EditorClient }) {
  const playing = usePlaying();
  const selectedId = useStore(projectionStore, (s) => s.selectedId);
  const summaries = useStore(projectionStore, (s) => s.summaries);
  const selectedName = selectedId ? (summaries[selectedId]?.name ?? selectedId) : null;

  const [info, setInfo] = useState<RuleDebugInfo | null>(null);

  // Refresh the truth-state whenever Play turns on or the selection changes — the click-to-debug read.
  useEffect(() => {
    if (!playing) {
      setInfo(null);
      return;
    }
    let live = true;
    client
      .ruleDebug(selectedId)
      .then((i) => {
        if (live) setInfo(i);
      })
      .catch(() => {
        /* a failed read leaves the panel empty — never crash the chrome */
      });
    return () => {
      live = false;
    };
  }, [client, playing, selectedId]);

  if (!playing) return null; // the debugger is a Play-time surface

  async function fireKill() {
    setInfo(await client.fireRuleEvent("EnemyDied", null, selectedId));
  }
  async function scrub(frame: number) {
    setInfo(await client.ruleScrub(frame, selectedId));
  }

  const truth = info?.truth ?? null;
  const head = info?.head ?? 0;
  const frame = info?.frame ?? 0;
  const decisions = info?.decisions ?? [];
  const flagged = info?.flagged ?? [];

  return (
    <div id="ruleDebug" data-testid="ruleDebug" style={box}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", marginBottom: 6 }}>
        <strong>Rule debugger</strong>
        <button id="fireEnemyDied" data-testid="fireEnemyDied" onClick={() => void fireKill()} style={btn} title="fire an EnemyDied event into the running rules">
          ⚔ Kill an enemy
        </button>
      </div>

      {!selectedId && <div style={{ opacity: 0.6 }}>click an entity to see its live rule truth-state</div>}

      {selectedId && truth && (truth.rules.length > 0 || truth.machines.length > 0) ? (
        <div>
          <div style={{ opacity: 0.7, marginBottom: 4 }}>truth-state for {selectedName} — debug by looking, not Debug.Log</div>

          {/* The machines' live current state: "✅ state = FacingBoss". */}
          {truth.machines.map((m) => (
            <div key={m.machine} id={`truthMachine-${m.machine}`} data-testid={`truthMachine-${m.machine}`} style={{ marginBottom: 4 }}>
              <span style={{ color: "#7fe39a" }}>✅</span> {m.display}
            </div>
          ))}

          {/* Each rule with its per-condition ✅/❌ — the why made visible. */}
          {truth.rules.map((r) => (
            <div key={r.rule} id={`truthRule-${r.rule}`} data-testid={`truthRule-${r.rule}`} style={{ marginTop: 6 }}>
              <div style={{ fontWeight: 600 }}>
                {r.fires ? <span style={{ color: "#7fe39a" }}>● fires</span> : <span style={{ color: "#9aa4b2" }}>○ idle</span>} {r.name}{" "}
                <span style={{ opacity: 0.5 }}>(When {r.event})</span>
              </div>
              {r.conditions.map((c, i) => (
                <Condition key={i} cond={c} rule={r.rule} idx={i} />
              ))}
              {info?.explanations.find((e) => e.rule === r.rule) && (
                <div data-testid={`explain-${r.rule}`} style={{ opacity: 0.7, fontStyle: "italic", marginTop: 2 }}>
                  {info.explanations.find((e) => e.rule === r.rule)?.text}
                </div>
              )}
            </div>
          ))}
        </div>
      ) : selectedId && truth ? (
        <div style={{ opacity: 0.6 }}>no rules reference {selectedName} yet</div>
      ) : null}

      {/* Determinism: a non-deterministic plugin is held out of the lockstep path — surfaced, never silent. */}
      {flagged.length > 0 && (
        <div data-testid="ruleFlagged" style={{ marginTop: 8, color: "#fbbf24" }}>
          {flagged.map((f) => (
            <div key={f.rule}>⚠ {f.reason}</div>
          ))}
        </div>
      )}

      {/* Time-travel: scrub the decision history — watch exactly WHEN a counter incremented (box 4). */}
      <div style={{ marginTop: 10, borderTop: "1px solid #2a2d35", paddingTop: 8 }}>
        <div style={{ display: "flex", alignItems: "center", gap: 8, marginBottom: 4 }}>
          <span style={{ opacity: 0.7 }}>decision history</span>
          <span data-testid="ruleFrame" style={{ opacity: 0.5 }}>frame {frame} / {head}</span>
        </div>
        <input
          id="ruleScrub"
          data-testid="ruleScrub"
          type="range"
          min={0}
          max={head}
          value={frame}
          disabled={head === 0}
          onChange={(e) => void scrub(Number(e.target.value))}
          style={{ width: "100%" }}
        />
        <div style={{ maxHeight: 120, overflowY: "auto", marginTop: 4 }}>
          {decisions.length === 0 ? (
            <div style={{ opacity: 0.5 }}>no decisions yet — fire an event</div>
          ) : (
            decisions.map((d, i) => (
              <div key={i} data-testid="decisionRow" style={{ opacity: 0.85 }}>
                <span style={{ opacity: 0.5 }}>f{d.frame}</span> {describeDecision(d)}
              </div>
            ))
          )}
        </div>
      </div>
    </div>
  );
}

/** One condition row: ✅/❌ + the human display copy (the structured `satisfied`/`actual`/`expected` is the
 *  stable truth the assertion keys off; this is the look). */
function Condition({ cond, rule, idx }: { cond: ConditionTruth; rule: string; idx: number }) {
  return (
    <div id={`truthCond-${rule}-${idx}`} data-testid={`truthCond-${rule}-${idx}`} data-satisfied={cond.satisfied} style={{ marginLeft: 12 }}>
      <span style={{ color: cond.satisfied ? "#7fe39a" : "#f08a8a" }}>{cond.satisfied ? "✅" : "❌"}</span> {cond.display}
    </div>
  );
}
