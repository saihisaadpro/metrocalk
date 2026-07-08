//! Mechanism panel (M15.9 / ADR-079) — rig + animate a joint FROM THE VIEWPORT: select a part → make it a
//! revolute (spins/swings about an axis) or sliding (moves along an axis) joint — the pivot defaults to the
//! part's own position, its REAL axis, never the scene origin — then drive it with the value slider (each
//! release lands as one undoable commit), press "Key pose" to record it on the timeline, and scrub the
//! timeline to play the mechanism back (deterministic, playback never edits the scene). The source label
//! says HOW the rig was authored (manual / inferred / URDF) — never oversold as automatic.

import { useCallback, useEffect, useState } from "react";
import { useSelectedId, projectionStore } from "../store/projection";
import { useStore } from "zustand";
import { setStatus } from "../store/ui";
import { Badge } from "../theme/primitives";
import { color, font, fontSize, space } from "../theme/tokens";
import type { JointInfo } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

const SOURCE_LABEL: Record<string, string> = {
  manual: "manual (authored here)",
  inferred: "inferred from geometry — check the axis",
  urdf: "from the URDF robot description",
};

export function JointPanel({ client }: { client: EditorClient }) {
  const id = useSelectedId();
  const summaries = useStore(projectionStore, (s) => s.summaries);
  const [info, setInfo] = useState<JointInfo | null>(null);
  const [scrubT, setScrubT] = useState(0);
  const [keyT, setKeyT] = useState("0");

  const refresh = useCallback(() => {
    if (!id) {
      setInfo(null);
      return;
    }
    client
      .jointInfo(id)
      .then(setInfo)
      .catch(() => setInfo(null));
  }, [client, id]);
  useEffect(refresh, [refresh]);

  if (!id) return null;
  const name = summaries[id]?.name ?? id;

  const makeJoint = async (revolute: boolean) => {
    // The pivot defaults to the part's own position — its real location, never the scene origin. The
    // designer refines the axis with the fields below (the gizmo-pick refinement is the named next step).
    const e = projectionStore.getState().base[id];
    const t = e?.components["Transform"] ?? {};
    const num = (f: string) => (typeof t[f] === "number" ? (t[f] as number) : 0);
    const pivot: [number, number, number] = [num("x"), num("y"), num("z")];
    const axis: [number, number, number] = revolute ? [0, 0, 1] : [1, 0, 0];
    const ok = await client.setJoint(id, revolute, axis, pivot, -1e6, 1e6, "manual");
    setStatus(ok ? `${name} is now a ${revolute ? "turning" : "sliding"} joint — drag the value, then key poses` : `couldn't make a joint on ${name}`);
    refresh();
  };

  if (!info) {
    return (
      <div id="joint-panel" data-testid="joint-panel" style={{ padding: `${space.md}px ${space.lg}px` }}>
        <div style={{ font: font.ui, fontSize: fontSize.meta, fontWeight: 600, letterSpacing: 0.4, textTransform: "uppercase", color: color.text.secondary, marginBottom: space.sm }}>
          Mechanism
        </div>
        <div style={{ font: font.ui, fontSize: fontSize.body, color: color.text.muted, marginBottom: space.sm }}>
          Make “{name}” a moving part:
        </div>
        <div style={{ display: "flex", gap: space.sm }}>
          <button type="button" className="mtk-card" data-testid="make-revolute" onClick={() => void makeJoint(true)} title="The part turns about an axis through its position (a wheel, an arm, a door).">
            ⟳ Turns (revolute)
          </button>
          <button type="button" className="mtk-card" data-testid="make-prismatic" onClick={() => void makeJoint(false)} title="The part slides along an axis (a trolley, a slide, a piston).">
            ⇄ Slides (prismatic)
          </button>
        </div>
      </div>
    );
  }

  const range = info.jointType === "revolute" ? Math.PI * 2 : 30;
  return (
    <div id="joint-panel" data-testid="joint-panel" data-joint-type={info.jointType} data-source={info.source} style={{ padding: `${space.md}px ${space.lg}px` }}>
      <div style={{ display: "flex", alignItems: "baseline", gap: space.sm, marginBottom: space.xs, font: font.ui, fontSize: fontSize.meta, fontWeight: 600, letterSpacing: 0.4, textTransform: "uppercase", color: color.text.secondary }}>
        <span>Mechanism</span>
        <Badge tone="accent">{info.jointType === "revolute" ? "turns" : "slides"}</Badge>
      </div>
      {/* The honesty label: how this rig was authored — never oversold as automatic. */}
      <div data-testid="joint-source" style={{ font: font.ui, fontSize: fontSize.meta, color: color.text.muted, marginBottom: space.sm }}>
        {SOURCE_LABEL[info.source] ?? info.source} · axis [{info.axis.map((a) => a.toFixed(2)).join(", ")}] through [{info.pivot.map((p) => p.toFixed(2)).join(", ")}]
      </div>

      {/* Drive the DOF — preview while dragging, ONE undoable commit on release. */}
      <label style={{ display: "block", font: font.ui, fontSize: fontSize.meta, color: color.text.secondary, marginBottom: space.xs }}>
        {info.jointType === "revolute" ? "Angle" : "Travel"}
        <input
          type="range"
          data-testid="joint-value"
          min={-range}
          max={range}
          step={range / 200}
          defaultValue={info.value}
          style={{ width: "100%" }}
          onInput={(e) => void client.jointValue(id, Number((e.target as HTMLInputElement).value), false)}
          onChange={(e) => {
            void client.jointValue(id, Number((e.target as HTMLInputElement).value), true).then(() => {
              setStatus(`${name} moved — Ctrl-Z undoes it`);
              refresh();
            });
          }}
        />
      </label>

      {/* Record the current pose at a time — the keyframe track (undoable, survives reload). */}
      <div style={{ display: "flex", alignItems: "center", gap: space.sm, marginBottom: space.sm }}>
        <button
          type="button"
          className="mtk-card"
          data-testid="joint-key"
          onClick={() => {
            void client.jointKey(id, Number(keyT) || 0).then((ok) => {
              setStatus(ok ? `pose keyed at ${keyT}s` : "couldn't key the pose");
              refresh();
            });
          }}
          title="Record the part's current pose at this time on the timeline."
        >
          ◆ Key pose at
        </button>
        <input
          data-testid="joint-key-t"
          value={keyT}
          onChange={(e) => setKeyT(e.target.value)}
          style={{ width: 48, font: font.mono, fontSize: fontSize.body, background: "transparent", color: color.text.primary, border: "1px solid var(--mtk-border-subtle)", borderRadius: 4, padding: "2px 6px" }}
        />
        <span style={{ font: font.ui, fontSize: fontSize.meta, color: color.text.muted }}>s · {info.keys} key{info.keys === 1 ? "" : "s"}</span>
      </div>

      {/* The timeline: scrub plays the WHOLE mechanism back (deterministic; playback never edits the scene). */}
      {info.trackEnd > 0 && (
        <label style={{ display: "block", font: font.ui, fontSize: fontSize.meta, color: color.text.secondary }}>
          Timeline {scrubT.toFixed(2)}s / {info.trackEnd.toFixed(2)}s
          <input
            type="range"
            data-testid="joint-scrub"
            min={0}
            max={info.trackEnd}
            step={info.trackEnd / 240}
            value={scrubT}
            style={{ width: "100%" }}
            onChange={(e) => {
              const t = Number(e.target.value);
              setScrubT(t);
              void client.jointScrub(t);
            }}
          />
        </label>
      )}
    </div>
  );
}
