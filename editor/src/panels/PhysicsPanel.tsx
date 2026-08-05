/**
 * Simulation controls, body actions, contact diagnostics, and robot-description import.
 *
 * Simulation transport and common body actions stay at the top. Contact inspection and interchange are
 * intentionally disclosed as advanced workflows so the everyday physics surface remains compact.
 */

import { useEffect, useState } from "react";
import { useSelectedId } from "../store/projection";
import { setStatus } from "../store/ui";
import { Button } from "../theme/primitives";
import { color, font, fontSize, radius, space } from "../theme/tokens";
import { DisclosureSection, ShortcutBadge } from "../theme/workspace";
import type { ContactInfo, PhysicsWarning, TimelineTuple } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

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

const actionRow: React.CSSProperties = {
  display: "flex",
  flexWrap: "wrap",
  alignItems: "center",
  gap: space.sm,
};

const fieldLabel: React.CSSProperties = {
  display: "block",
  marginBottom: space.xs,
  color: color.text.secondary,
  font: font.ui,
  fontSize: fontSize.meta,
  fontWeight: 600,
};

const helperText: React.CSSProperties = {
  margin: 0,
  color: color.text.muted,
  font: font.ui,
  fontSize: fontSize.meta,
  lineHeight: 1.45,
};

export function PhysicsPanel({ client }: { client: EditorClient }) {
  const selectedId = useSelectedId();
  const [running, setRunning] = useState(false);
  const [debuggerOn, setDebuggerOn] = useState(false);
  const [tl, setTl] = useState<TimelineTuple>(ZERO_TL);
  const [contacts, setContacts] = useState<ContactInfo[]>([]);
  const [warnings, setWarnings] = useState<PhysicsWarning[]>([]);
  const [dropBusy, setDropBusy] = useState(false);
  const [importOpen, setImportOpen] = useState(false);
  const [importBusy, setImportBusy] = useState(false);
  const [importText, setImportText] = useState("");
  const [importResult, setImportResult] = useState("");

  // The simulation runs natively. This low-frequency read only refreshes transport chrome and diagnostics.
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
    setDropBusy(true);
    try {
      const x = (Math.random() - 0.5) * 2;
      const z = (Math.random() - 0.5) * 2;
      const id = await client.spawnBody(x, 8, z).catch(() => null);
      if (id) {
        client.setSimRunning(true);
        setRunning(true);
        setStatus(`dropped a ball · ${id}`);
      } else {
        setStatus("couldn't create a test body — try again");
      }
    } finally {
      setDropBusy(false);
    }
  }

  async function toggleSim() {
    const t = await client.simTimeline().catch(() => tl);
    const next = !t[2];
    client.setSimRunning(next);
    setRunning(next);
    setTl(t);
    setStatus(next ? "sim running" : "sim paused");
  }

  async function toggleDebugger() {
    const t = await client.simTimeline().catch(() => tl);
    const next = !t[3];
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

    setImportBusy(true);
    try {
      const fmt = source.includes("<robot") ? "urdf" : source.toLowerCase().includes("usd") ? "usd" : "urdf";
      const r = await client.importInterchange(fmt, source).catch(() => null);
      if (!r || !r.ok) {
        setImportResult(`import failed: ${r?.error ?? "unknown error"}`);
        return;
      }
      const lines = [`imported ${r.bodies} bodies · ${r.joints} joints (${r.format})`, ...r.notes];
      setImportResult(lines.join("\n"));
      setStatus(`imported ${r.bodies} bodies (Ctrl-Z to peel)`);
    } finally {
      setImportBusy(false);
    }
  }

  const hasFrames = tl[1] > 0;
  const importHasError = importResult.startsWith("import failed") || importResult.startsWith("paste ");

  return (
    <div
      className="mtk-scroll"
      role="region"
      aria-label="Physics controls"
      style={{ minHeight: 0, overflowY: "auto", background: color.bg.panel }}
    >
      <DisclosureSection
        title="Simulation"
        summary={hasFrames ? `Frame ${tl[0]} of ${tl[1]}` : "Ready"}
        defaultOpen
        storageKey="physics-simulation"
      >
        <div style={{ display: "flex", flexDirection: "column", gap: space.md }}>
          <div style={actionRow} role="group" aria-label="Simulation controls">
            <Button
              id="dropBall"
              data-testid="dropBall"
              type="button"
              disabled={dropBusy}
              onClick={() => void dropBall()}
              title="Create a dynamic test body above the scene and start simulation."
            >
              {dropBusy ? (
                <>
                  <span className="mtk-spinner" aria-hidden="true" /> Creating body…
                </>
              ) : (
                "Drop test ball"
              )}
            </Button>
            <Button
              id="simToggle"
              data-testid="simToggle"
              type="button"
              variant="primary"
              onClick={() => void toggleSim()}
              aria-label={running ? "Pause simulation" : "Resume simulation"}
            >
              {running ? "Pause simulation" : "Resume simulation"}
            </Button>
          </div>

          <div>
            <label htmlFor="scrub" style={fieldLabel}>
              Recorded timeline
            </label>
            <div style={{ ...actionRow, flexWrap: "nowrap" }}>
              <input
                id="scrub"
                data-testid="scrub"
                type="range"
                min={0}
                max={Math.max(1, tl[1])}
                value={tl[0]}
                disabled={!hasFrames}
                aria-describedby="frameLbl scrub-help"
                aria-valuetext={`Frame ${tl[0]} of ${tl[1]}`}
                title={hasFrames ? "Scrubbing pauses the simulation." : "Run the simulation to record frames before scrubbing."}
                onChange={(e) => void scrub(Number(e.target.value))}
                style={{ flex: "1 1 auto", minWidth: 0, accentColor: color.accent.base }}
              />
              <span id="frameLbl" data-testid="frameLbl" style={{ color: color.text.secondary, font: font.mono, fontSize: fontSize.meta, whiteSpace: "nowrap" }}>
                frame {tl[0]}/{tl[1]}{tl[3] ? " · debug" : ""}
              </span>
            </div>
            <p id="scrub-help" style={{ ...helperText, marginTop: space.xs }}>
              {hasFrames
                ? "Scrubbing pauses playback and restores the selected recorded frame."
                : "Run the simulation to record frames before scrubbing."}
            </p>
          </div>
        </div>
      </DisclosureSection>

      <DisclosureSection
        title="Selected body"
        summary={selectedId ? "Body actions" : "Selection required"}
        defaultOpen
        storageKey="physics-body"
      >
        {!selectedId && (
          <p role="status" style={{ ...helperText, marginBottom: space.sm, color: color.info.text }}>
            Select a physics body to use these actions. If the selection is incompatible, the status bar will explain why.
          </p>
        )}
        <div style={actionRow}>
          <Button
            id="shove"
            data-testid="shove"
            type="button"
            onClick={() => void shove()}
            title="Apply a short forward-and-up impulse to the selected physics body."
          >
            Apply test shove
          </Button>
          <Button
            id="nudgeFriction"
            data-testid="nudgeFriction"
            type="button"
            onClick={nudgeFriction}
            title="Set the selected collider's friction to 0.95. This is undoable."
          >
            Increase friction
          </Button>
        </div>
      </DisclosureSection>

      {warnings.length > 0 && (
        <div
          id="physWarn"
          data-testid="physWarn"
          role="alert"
          aria-label="Physics checks"
          style={{
            margin: space.md,
            padding: space.md,
            border: `1px solid ${color.warn.border}`,
            borderRadius: radius.lg,
            background: color.warn.bg,
          }}
        >
          <div style={{ marginBottom: space.sm, color: color.warn.text, font: font.ui, fontSize: fontSize.meta, fontWeight: 600 }}>
            Physics setup needs attention
          </div>
          <div style={{ display: "flex", flexDirection: "column", gap: space.sm }}>
            {warnings.map((w, i) => (
              <div key={`${w.fixAction}-${i}`} style={{ ...actionRow, justifyContent: "space-between" }}>
                <span style={{ flex: "1 1 180px", color: color.text.secondary, font: font.ui, fontSize: fontSize.body }}>
                  {w.message}
                </span>
                <Button
                  type="button"
                  compact
                  disabled={!selectedId}
                  title={selectedId ? `Apply suggested fix: ${w.fixLabel}` : "Select the affected body before applying this fix."}
                  onClick={() => {
                    if (!selectedId) return;
                    void client.physicsFix(selectedId, w.fixAction).then(() => void refreshWarnings(selectedId)).catch(() => {});
                  }}
                >
                  {w.fixLabel}
                </Button>
              </div>
            ))}
          </div>
        </div>
      )}

      <DisclosureSection
        title="Contact diagnostics"
        summary={debuggerOn ? `${contacts.length} contacts · overlay on` : "Advanced"}
        defaultOpen={false}
        storageKey="physics-diagnostics"
      >
        <p style={{ ...helperText, marginBottom: space.sm }}>
          Draw live contact overlays in the viewport and inspect collision explanations. This may add visual clutter in dense scenes.
        </p>
        <Button
          id="dbgToggle"
          data-testid="dbgToggle"
          type="button"
          variant="toggle"
          active={debuggerOn}
          aria-pressed={debuggerOn}
          onClick={() => void toggleDebugger()}
        >
          Contact overlay: {debuggerOn ? "On" : "Off"}
        </Button>

        {debuggerOn && (
          <div
            id="contacts"
            data-testid="contacts"
            className="mtk-scroll"
            role="log"
            aria-live="polite"
            aria-label="Live physics contacts"
            style={{
              maxHeight: 160,
              marginTop: space.md,
              overflowY: "auto",
              padding: space.md,
              border: `1px solid ${color.border.subtle}`,
              borderRadius: radius.lg,
              background: color.bg.inset,
            }}
          >
            <div style={{ marginBottom: contacts.length ? space.xs : 0, color: color.text.secondary, font: font.ui, fontSize: fontSize.meta, fontWeight: 600 }}>
              {contacts.length ? `${contacts.length} live contact${contacts.length === 1 ? "" : "s"}` : "No contacts at this frame"}
            </div>
            {contacts.slice(0, 12).map((c, i) => (
              <div
                key={`${c.explain}-${i}`}
                style={{ padding: `${space.xs}px 0`, color: c.friction_saturated ? color.warn.text : color.text.secondary, font: font.mono, fontSize: fontSize.meta, lineHeight: 1.4 }}
              >
                {c.explain}
              </div>
            ))}
          </div>
        )}
      </DisclosureSection>

      <DisclosureSection
        title="Robot interchange"
        summary="URDF / USD"
        defaultOpen={false}
        storageKey="physics-interchange"
      >
        <p style={{ ...helperText, marginBottom: space.sm }}>
          Import rigid bodies and joints from a URDF robot description or USD Physics document. Review reconciliation notes after import.
        </p>
        <Button id="importRobot" data-testid="importRobot" type="button" onClick={() => setImportOpen(true)}>
          Open importer
        </Button>

        {importOpen && (
          <div
            id="importPanel"
            data-testid="importPanel"
            aria-busy={importBusy || undefined}
            style={{
              marginTop: space.md,
              padding: space.md,
              border: `1px solid ${color.border.subtle}`,
              borderRadius: radius.lg,
              background: color.bg.inset,
            }}
          >
            <div style={{ ...actionRow, justifyContent: "space-between", marginBottom: space.sm }}>
              <div style={{ color: color.text.primary, font: font.ui, fontSize: fontSize.body, fontWeight: 600 }}>
                Import robot description
              </div>
              <Button id="impClose" data-testid="impClose" type="button" variant="ghost" compact disabled={importBusy} onClick={() => setImportOpen(false)}>
                Close
              </Button>
            </div>

            <div style={{ ...actionRow, marginBottom: space.sm }}>
              <Button id="impSample" data-testid="impSample" type="button" compact disabled={importBusy} onClick={() => setImportText(SAMPLE_ARM)}>
                Use sample arm
              </Button>
              <span style={helperText}>
                Sample includes two bodies and one revolute joint.
              </span>
            </div>

            <label htmlFor="impText" style={fieldLabel}>
              URDF or USD source
            </label>
            <textarea
              id="impText"
              data-testid="impText"
              className="mtk-input mtk-input--mono"
              value={importText}
              disabled={importBusy}
              onChange={(e) => setImportText(e.target.value)}
              placeholder="Paste a URDF or USD Physics document"
              aria-describedby="impText-help"
              spellCheck={false}
              style={{ boxSizing: "border-box", width: "100%", minHeight: 112, resize: "vertical", lineHeight: 1.4 }}
            />
            <p id="impText-help" style={{ ...helperText, marginTop: space.xs }}>
              The import is one undoable scene operation. Unsupported details are listed below instead of being silently dropped.
            </p>

            <div style={{ ...actionRow, marginTop: space.md }}>
              <Button id="impGo" data-testid="impGo" type="button" variant="primary" disabled={importBusy} onClick={() => void runImport()}>
                {importBusy ? (
                  <>
                    <span className="mtk-spinner" aria-hidden="true" /> Importing…
                  </>
                ) : (
                  "Import into scene"
                )}
              </Button>
              <span style={helperText}>
                Undo with <ShortcutBadge keys={["Ctrl", "Z"]} ariaLabel="Control plus Z" />
              </span>
            </div>

            <pre
              id="impResult"
              data-testid="impResult"
              role={importHasError ? "alert" : "status"}
              aria-live="polite"
              style={{
                minHeight: 18,
                margin: `${space.md}px 0 0`,
                padding: importResult ? space.md : 0,
                border: importResult ? `1px solid ${importHasError ? color.danger.border : color.success.border}` : "none",
                borderRadius: radius.md,
                background: importResult ? (importHasError ? color.danger.bg : color.success.bg) : "transparent",
                color: importResult ? (importHasError ? color.danger.text : color.success.text) : color.text.muted,
                font: font.mono,
                fontSize: fontSize.meta,
                lineHeight: 1.45,
                whiteSpace: "pre-wrap",
              }}
            >
              {importResult}
            </pre>
          </div>
        )}
      </DisclosureSection>
    </div>
  );
}
