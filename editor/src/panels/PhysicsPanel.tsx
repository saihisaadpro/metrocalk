//! M8 physics control panel (M10.1 React port of the scaffold's physics surface) — drop a body, the sim
//! transport (pause/resume + scrub), the "debug by looking" contact overlay, shove, edit-at-pause friction,
//! and URDF/USD interchange import. The per-frame sim runs NATIVELY on the engine thread (0 IPC/frame,
//! invariant 4 — this panel never polls per frame; a 250 ms chrome poll only refreshes the slider/label).
//!
//! Stable ids mirror the scaffold so the prompt-40 acceptance page-object greens by selector-swap:
//! #dropBall · #simToggle · #dbgToggle · #shove · #nudgeFriction · #scrub · #frameLbl · #importRobot ·
//! #impSample · #impText · #impGo · #impResult · #impClose.

import { useEffect, useState } from "react";
import type { EditorClient } from "../transport/session";
import type { ContactInfo, PhysicsWarning, TimelineTuple } from "../transport/protocol";
import { useSelectedId } from "../store/projection";
import { setStatus } from "../store/ui";

// A tiny URDF the "Paste sample arm" button injects (parity with the scaffold; the acceptance spec uses its
// own fixture). Two links → two bodies, one revolute joint with a limit, a cylinder collider.
const SAMPLE_ARM = `<?xml version="1.0"?>
<robot name="arm">
  <link name="base"><inertial><mass value="5.0"/><inertia ixx="1" ixy="0" ixz="0" iyy="1" iyz="0" izz="1"/></inertial>
    <collision><geometry><box size="0.6 0.3 0.6"/></geometry></collision></link>
  <link name="upper"><inertial><mass value="2.0"/><inertia ixx="1" ixy="0" ixz="0" iyy="1" iyz="0" izz="1"/></inertial>
    <collision><geometry><cylinder radius="0.12" length="1.0"/></geometry></collision></link>
  <joint name="shoulder" type="revolute"><parent link="base"/><child link="upper"/>
    <origin xyz="0 1.0 0" rpy="0 0 0"/><axis xyz="0 0 1"/>
    <limit lower="-1.57" upper="1.57" effort="100" velocity="1"/></joint>
</robot>`;

const ZERO_TL: TimelineTuple = [0, 0, false, false, 0];

const btn: React.CSSProperties = { margin: "3px 0", padding: "5px 8px", background: "#1f3a5a", color: "#dce", border: "1px solid #2a3550", borderRadius: 5, cursor: "pointer", font: "12px ui-monospace, monospace" };

export function PhysicsPanel({ client }: { client: EditorClient }) {
  const selectedId = useSelectedId();
  const [running, setRunning] = useState(false);
  const [debuggerOn, setDebuggerOn] = useState(false);
  const [tl, setTl] = useState<TimelineTuple>(ZERO_TL);
  const [contacts, setContacts] = useState<ContactInfo[]>([]);
  const [warnings, setWarnings] = useState<PhysicsWarning[]>([]);
  const [importOpen, setImportOpen] = useState(false);
  const [importText, setImportText] = useState("");
  const [importResult, setImportResult] = useState("");

  // The ONLY periodic read: a 250 ms chrome poll (NOT per-frame) gated on running||debuggerOn — refreshes
  // the transport slider/label + the contacts overlay. The sim itself advances natively (invariant 4).
  useEffect(() => {
    if (!running && !debuggerOn) return;
    let live = true;
    const tick = async () => {
      const t = await client.simTimeline().catch(() => ZERO_TL);
      if (!live) return;
      setTl(t);
      if (debuggerOn) {
        const c = await client.physicsContacts().catch(() => []);
        if (live) setContacts(c);
      }
    };
    void tick();
    const h = setInterval(() => void tick(), 250);
    return () => {
      live = false;
      clearInterval(h);
    };
  }, [client, running, debuggerOn]);

  async function refreshWarnings(id: string) {
    setWarnings(await client.physicsCheck(id).catch(() => []));
  }

  async function dropBall() {
    const x = (Math.random() - 0.5) * 2;
    const z = (Math.random() - 0.5) * 2;
    const id = await client.spawnBody(x, 8, z).catch(() => null);
    if (id) {
      client.setSimRunning(true);
      setRunning(true);
      setStatus(`dropped a ball · ${id}`);
    }
  }

  async function toggleSim() {
    // Read the LIVE run-state and flip it (robust to an external set_sim_running — the acceptance harness
    // establishes a known state via the command, then clicks this toggle).
    const t = await client.simTimeline().catch(() => tl);
    const next = !t[2];
    client.setSimRunning(next);
    setRunning(next);
    setTl(t);
    setStatus(next ? "sim running" : "sim paused");
  }

  async function toggleDebugger() {
    const t = await client.simTimeline().catch(() => tl);
    const next = !t[3]; // overlays flag
    client.simOverlay(next);
    setDebuggerOn(next);
    if (next) setContacts(await client.physicsContacts().catch(() => []));
    else setContacts([]);
    setStatus(next ? "contact debugger ON" : "contact debugger off");
  }

  async function shove() {
    if (!selectedId) {
      setStatus("select a body to shove");
      return;
    }
    const ok = await client.simShove(selectedId, [4.0, 1.0, 0.0]).catch(() => false);
    setStatus(ok ? `shoved ${selectedId}` : "that entity isn't a physics body");
  }

  function nudgeFriction() {
    if (!selectedId) {
      setStatus("select a body to add friction");
      return;
    }
    // One undoable, reload-persistent transaction (the deterministic replay re-derives it) — the generic
    // commit path, not a physics command (matches the scaffold).
    client.setField(selectedId, "Collider", "friction", 0.95);
    setStatus("added friction (Ctrl-Z to undo)");
    setTimeout(() => void refreshWarnings(selectedId), 80);
  }

  async function scrub(frame: number) {
    client.setSimRunning(false);
    setRunning(false);
    const t = await client.simScrub(frame).catch(() => tl);
    setTl(t);
  }

  async function runImport() {
    const source = importText.trim();
    if (!source) {
      setImportResult("paste a URDF or USD document first");
      return;
    }
    const fmt = source.includes("<robot") ? "urdf" : source.toLowerCase().includes("usd") ? "usd" : "urdf";
    const r = await client.importInterchange(fmt, source).catch(() => null);
    if (!r || !r.ok) {
      setImportResult(`import failed: ${r?.error ?? "unknown error"}`);
      return;
    }
    // Stable structured text the acceptance page-object reads (#impResult): "imported N bodies" + the
    // explained reconciliation notes (cylinder→capsule, unenforced joint limit, …).
    const lines = [`imported ${r.bodies} bodies · ${r.joints} joints (${r.format})`, ...r.notes];
    setImportResult(lines.join("\n"));
    setStatus(`imported ${r.bodies} bodies (Ctrl-Z to peel)`);
  }

  return (
    <div style={{ padding: "8px 12px", borderTop: "1px solid #2a2d35", font: "12px ui-monospace, monospace" }}>
      <div style={{ opacity: 0.6, fontSize: 11, marginBottom: 4 }}>Physics (M8)</div>

      <button id="dropBall" data-testid="dropBall" onClick={() => void dropBall()} style={btn}>🏀 Drop a ball</button>{" "}
      <button id="simToggle" data-testid="simToggle" onClick={() => void toggleSim()} style={btn}>{running ? "⏸ Pause sim" : "▶ Resume sim"}</button>{" "}
      <button id="dbgToggle" data-testid="dbgToggle" onClick={() => void toggleDebugger()} style={{ ...btn, background: debuggerOn ? "#3a2f5a" : "#1f3a5a" }}>🔬 Debugger</button>

      <div style={{ marginTop: 6 }}>
        <button id="shove" data-testid="shove" onClick={() => void shove()} style={btn}>👊 Shove</button>{" "}
        <button id="nudgeFriction" data-testid="nudgeFriction" onClick={nudgeFriction} style={btn}>🧊 +Friction</button>
      </div>

      <div style={{ marginTop: 6, display: "flex", alignItems: "center", gap: 8 }}>
        <input
          id="scrub"
          data-testid="scrub"
          type="range"
          min={0}
          max={Math.max(1, tl[1])}
          value={tl[0]}
          onChange={(e) => void scrub(Number(e.target.value))}
          style={{ flex: 1 }}
        />
        <span id="frameLbl" data-testid="frameLbl" style={{ opacity: 0.7 }}>
          frame {tl[0]}/{tl[1]}{tl[3] ? " · 🔬" : ""}
        </span>
      </div>

      <div style={{ marginTop: 6 }}>
        <button id="importRobot" data-testid="importRobot" onClick={() => setImportOpen(true)} style={btn}>🤖 Import URDF/USD</button>
      </div>

      {warnings.length > 0 && (
        <div id="physWarn" data-testid="physWarn" style={{ marginTop: 6, background: "#3a2f16", border: "1px solid #6a5a1f", borderRadius: 5, padding: 6 }}>
          {warnings.map((w, i) => (
            <div key={i} style={{ marginBottom: 4 }}>
              ⚠ {w.message}{" "}
              <button
                onClick={() => {
                  if (!selectedId) return;
                  void client.physicsFix(selectedId, w.fixAction).then(() => void refreshWarnings(selectedId)).catch(() => {});
                }}
                style={{ ...btn, padding: "1px 6px", margin: 0 }}
              >
                {w.fixLabel}
              </button>
            </div>
          ))}
        </div>
      )}

      {debuggerOn && (
        <div id="contacts" data-testid="contacts" style={{ marginTop: 6, maxHeight: 120, overflowY: "auto", background: "#0d1018", border: "1px solid #2a3550", borderRadius: 5, padding: 6 }}>
          <div style={{ opacity: 0.6 }}>contacts ({contacts.length})</div>
          {contacts.slice(0, 12).map((c, i) => (
            <div key={i} style={{ color: c.friction_saturated ? "#fbbf24" : "#9cd", fontSize: 11 }}>{c.explain}</div>
          ))}
        </div>
      )}

      {importOpen && (
        <div id="importPanel" data-testid="importPanel" style={{ marginTop: 6, background: "#0d1018", border: "1px solid #2a3550", borderRadius: 6, padding: 8 }}>
          <div style={{ display: "flex", justifyContent: "space-between", marginBottom: 4 }}>
            <span style={{ opacity: 0.7 }}>Import URDF / USD-Physics</span>
            <button id="impClose" data-testid="impClose" onClick={() => setImportOpen(false)} style={{ ...btn, padding: "1px 8px", margin: 0 }}>Close</button>
          </div>
          <button id="impSample" data-testid="impSample" onClick={() => setImportText(SAMPLE_ARM)} style={{ ...btn, padding: "2px 8px" }}>Paste sample arm</button>
          <textarea
            id="impText"
            data-testid="impText"
            value={importText}
            onChange={(e) => setImportText(e.target.value)}
            placeholder="paste a URDF or USD-Physics document…"
            style={{ width: "100%", height: 80, marginTop: 4, background: "#0a0c12", color: "#cde", border: "1px solid #2a3550", borderRadius: 4, font: "11px ui-monospace, monospace" }}
          />
          <button id="impGo" data-testid="impGo" onClick={() => void runImport()} style={{ ...btn, marginTop: 4 }}>Import</button>
          {/* always present while the panel is open so the page-object's getText() never hits a missing node */}
          <pre id="impResult" data-testid="impResult" style={{ marginTop: 4, whiteSpace: "pre-wrap", color: "#9cd", fontSize: 11, minHeight: 14 }}>{importResult}</pre>
        </div>
      )}
    </div>
  );
}
