//! M9 transform / part / solver control panel (M10.1 React port). The transform GIZMO drag itself is
//! native (0 IPC/frame — App wires the viewport down/up + the W/E/R keys; the render loop moves the body),
//! so this panel is the **inspector-side** surface: rigid PART editing (save character · drop instance ·
//! deactivate-not-delete · reparent) and the intent transform SOLVER (magnetic snap · snap-to-nearest ·
//! place-by-sentence). Every "no" is surfaced (the M9 UX contract) via the status line.
//!
//! Each button acts on the LIVE gizmo selection (read via `gizmoSelected()` at click time) — so it operates
//! on the same entity the engine has selected, whether selection came from a viewport pick or (in the
//! acceptance harness) a direct `gizmo_select`. The controls render unconditionally so a select-then-click
//! sequence never races a poll. Stable ids mirror the scaffold: #saveChar · #dropInst · #deactPart ·
//! #reparentTo · #reparentBtn · #snapToggle · #snapNearest · #placeSentence · #placeBtn.

import { useEffect, useState } from "react";
import type { EditorClient } from "../transport/session";
import { setStatus } from "../store/ui";

type GizmoInfo = [string, boolean, boolean, string, string]; // [mode, hasSel, dragging, space, pivot]

const btn: React.CSSProperties = { margin: "3px 0", padding: "5px 8px", background: "#1f3a5a", color: "#dce", border: "1px solid #2a3550", borderRadius: 5, cursor: "pointer", font: "12px ui-monospace, monospace" };
const MODE_LABEL: Record<string, string> = { translate: "⬌ Move (W)", rotate: "⟳ Rotate (E)", scale: "⇲ Scale (R)" };

export function TransformPanel({ client }: { client: EditorClient }) {
  const [lastComp, setLastComp] = useState<string | null>(null);
  const [snapOn, setSnapOn] = useState(true);
  const [gizmo, setGizmo] = useState<GizmoInfo | null>(null);
  const [reparentVal, setReparentVal] = useState("");
  const [placeVal, setPlaceVal] = useState("");

  const refreshHud = () => void client.gizmoDebug().then(setGizmo).catch(() => setGizmo(null));
  useEffect(() => {
    refreshHud();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Resolve the live engine selection for a part action; explain when nothing is selected (principle 1).
  async function withSelection(label: string, fn: (id: string) => void | Promise<void>) {
    const id = await client.gizmoSelected().catch(() => null);
    if (!id) {
      setStatus(`select a part first to ${label}`);
      return;
    }
    await fn(id);
  }

  async function saveChar() {
    await withSelection("save it", async (id) => {
      const c = await client.saveCharacter(id).catch(() => null);
      if (c) {
        setLastComp(c);
        setStatus(`saved character ${c} — Drop a fresh instance to reuse it`);
      } else {
        setStatus("couldn't save this — select a character part");
      }
    });
  }
  async function dropInstance() {
    if (!lastComp) {
      setStatus("save a character first, then drop an instance");
      return;
    }
    const inst = await client.instantiateCharacter(lastComp).catch(() => null);
    setStatus(inst ? `dropped a fresh instance ${inst}` : "couldn't drop an instance");
  }
  async function deactivatePart() {
    await withSelection("deactivate it", async (id) => {
      const ok = await client.setPartActive(id, false).catch(() => false);
      setStatus(ok ? "part deactivated (data preserved — Ctrl-Z restores it)" : "deactivate failed (root parts can't be hidden)");
    });
  }
  async function reparent() {
    await withSelection("reparent it", (id) => {
      const to = reparentVal.trim();
      client.reparentPart(id, to === "" ? null : to);
      setStatus(`reparented${to ? " under " + to : " to root"} (Ctrl-Z to undo)`);
    });
  }
  function toggleSnap() {
    const next = !snapOn;
    setSnapOn(next);
    client.setSnap(next);
    setStatus(`magnetic snap ${next ? "ON" : "off"}`);
  }
  async function snapNearest() {
    await withSelection("snap it", async (id) => {
      const hits = await client.snapQuery(id, 100.0).catch(() => []);
      if (hits.length === 0) {
        setStatus("no snap targets within reach");
        return;
      }
      const res = await client.applyConstraint(id, "snap", hits[0].id, 0.0).catch(() => null);
      setStatus(res?.ok ? `snapped: ${hits[0].why}` : `couldn't snap: ${res?.reason ?? "no solution"}`);
    });
  }
  async function placeBySentence() {
    await withSelection("place it", async (id) => {
      const text = placeVal.trim();
      if (!text) {
        setStatus("type a placement sentence");
        return;
      }
      const res = await client.placementSentence(id, text).catch(() => null);
      if (res?.ok) setStatus(`placed: ${res.intents.join(", ")}`);
      else setStatus(`couldn't place: ${res?.reason ?? "didn't understand that"}`);
    });
  }

  return (
    <div style={{ padding: "8px 12px", borderTop: "1px solid #2a2d35", font: "12px ui-monospace, monospace" }}>
      <div style={{ opacity: 0.6, fontSize: 11, marginBottom: 4 }}>
        Transform (M9) {gizmo?.[1] ? `· ${MODE_LABEL[gizmo[0]] ?? gizmo[0]}` : "· press W/E/R on a selection"}
      </div>

      {/* gizmo space / pivot toggles (G1 HUD) */}
      <div style={{ marginBottom: 4 }}>
        <button onClick={() => void client.gizmoSpaceToggle().then(refreshHud).catch(() => {})} style={{ ...btn, padding: "2px 8px" }}>space: {gizmo?.[3] ?? "world"}</button>{" "}
        <button onClick={() => void client.gizmoPivotToggle().then(refreshHud).catch(() => {})} style={{ ...btn, padding: "2px 8px" }}>pivot: {gizmo?.[4] ?? "origin"}</button>
        <span style={{ opacity: 0.45, marginLeft: 6 }}>Ctrl = snap</span>
      </div>

      {/* G2 — rigid part editing */}
      <button id="saveChar" data-testid="saveChar" onClick={() => void saveChar()} style={btn}>💾 Save character for reuse</button>{" "}
      <button id="dropInst" data-testid="dropInst" onClick={() => void dropInstance()} disabled={!lastComp} style={{ ...btn, opacity: lastComp ? 1 : 0.5 }}>📋 Drop a fresh instance</button>{" "}
      <button id="deactPart" data-testid="deactPart" onClick={() => void deactivatePart()} style={{ ...btn, background: "#5a1f2f" }}>🗑 Deactivate part (Ctrl-Z restores)</button>
      <div style={{ marginTop: 4, display: "flex", gap: 4, alignItems: "center" }}>
        <span style={{ opacity: 0.6 }}>reparent under</span>
        <input id="reparentTo" data-testid="reparentTo" value={reparentVal} onChange={(e) => setReparentVal(e.target.value)} placeholder="entity id — empty = root" style={{ flex: 1, background: "#0a0c12", color: "#cde", border: "1px solid #2a3550", borderRadius: 4, font: "11px ui-monospace, monospace" }} />
        <button id="reparentBtn" data-testid="reparentBtn" onClick={() => void reparent()} style={{ ...btn, padding: "2px 8px", margin: 0 }}>Reparent</button>
      </div>

      {/* G4 — the intent transform solver */}
      <div style={{ marginTop: 6 }}>
        <button id="snapToggle" data-testid="snapToggle" onClick={toggleSnap} style={{ ...btn, background: snapOn ? "#1f3a5a" : "#2a2d35" }}>🧲 Magnetic snap: {snapOn ? "ON" : "off"}</button>{" "}
        <button id="snapNearest" data-testid="snapNearest" onClick={() => void snapNearest()} style={btn}>↪ Snap to nearest</button>
      </div>
      <div style={{ marginTop: 4, display: "flex", gap: 4, alignItems: "center" }}>
        <input id="placeSentence" data-testid="placeSentence" value={placeVal} onChange={(e) => setPlaceVal(e.target.value)} placeholder='e.g. "upright, 10 cm from the edge"' style={{ flex: 1, background: "#0a0c12", color: "#cde", border: "1px solid #2a3550", borderRadius: 4, font: "11px ui-monospace, monospace" }} />
        <button id="placeBtn" data-testid="placeBtn" onClick={() => void placeBySentence()} style={{ ...btn, padding: "2px 8px", margin: 0 }}>Place</button>
      </div>
    </div>
  );
}
