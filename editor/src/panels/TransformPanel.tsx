/**
 * Selection-aware transform, placement, and part-structure controls.
 *
 * The viewport owns direct manipulation; this docked surface provides the complementary controls that need
 * labels, explanation, and review. Less frequent part and hierarchy actions use progressive disclosure while
 * the current gizmo and placement tools remain immediately available.
 */

import { useCallback, useEffect, useState } from "react";
import { projectionStore, useSelectedId } from "../store/projection";
import { setStatus } from "../store/ui";
import { Button, TextField } from "../theme/primitives";
import { color, font, fontSize, space } from "../theme/tokens";
import { DisclosureSection, ShortcutBadge } from "../theme/workspace";
import type { EditorClient } from "../transport/session";

type GizmoInfo = [string, boolean, boolean, string, string]; // [mode, hasSel, dragging, space, pivot]

const MODE_LABEL: Record<string, string> = {
  translate: "Move",
  rotate: "Rotate",
  scale: "Scale",
};

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

export function TransformPanel({ client }: { client: EditorClient }) {
  const sel = useSelectedId();
  const [lastComp, setLastComp] = useState<string | null>(null);
  const [snapOn, setSnapOn] = useState(true);
  const [gizmo, setGizmo] = useState<GizmoInfo | null>(null);
  const [hudLoading, setHudLoading] = useState(true);
  const [hudUnavailable, setHudUnavailable] = useState(false);
  const [reparentVal, setReparentVal] = useState("");
  const [placeVal, setPlaceVal] = useState("");

  const refreshHud = useCallback(() => {
    setHudLoading(true);
    setHudUnavailable(false);
    void client
      .gizmoDebug()
      .then((info) => setGizmo(info))
      .catch(() => {
        setGizmo(null);
        setHudUnavailable(true);
      })
      .finally(() => setHudLoading(false));
  }, [client]);

  useEffect(refreshHud, [refreshHud]);

  // Resolve the live engine selection at action time so viewport and hierarchy selection remain in sync.
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
      if (ok) projectionStore.getState().markDeactivated([id]);
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

  const currentMode = gizmo ? (MODE_LABEL[gizmo[0]] ?? gizmo[0]) : null;

  return (
    <div
      className="mtk-scroll"
      role="region"
      aria-label="Transform and placement controls"
      style={{ minHeight: 0, overflowY: "auto", background: color.bg.panel }}
    >
      {!sel && (
        <div
          role="status"
          style={{
            padding: `${space.md}px ${space.lg}px`,
            borderBottom: `1px solid ${color.border.subtle}`,
            background: color.info.bg,
            color: color.info.text,
            font: font.ui,
            fontSize: fontSize.meta,
            lineHeight: 1.45,
          }}
        >
          Select an entity in the viewport or Scene panel. The controls remain available and will explain
          when an action needs a selection.
        </div>
      )}

      <DisclosureSection
        title="Gizmo"
        summary={currentMode ? `${currentMode} · ${gizmo?.[3] ?? "world"}` : "W / E / R"}
        defaultOpen
        storageKey="transform-gizmo"
      >
        {hudLoading ? (
          <div role="status" aria-live="polite" style={helperText}>
            <span className="mtk-spinner" aria-hidden="true" /> Loading gizmo state…
          </div>
        ) : hudUnavailable ? (
          <div role="status" style={{ ...helperText, color: color.warn.text }}>
            Gizmo state is temporarily unavailable. Viewport shortcuts still work.
          </div>
        ) : (
          <div style={{ display: "flex", flexDirection: "column", gap: space.sm }}>
            <div style={{ ...actionRow, justifyContent: "space-between" }}>
              <span style={{ color: color.text.secondary, font: font.ui, fontSize: fontSize.body }}>
                {gizmo?.[1] ? `${currentMode} selected entity` : "Choose a gizmo mode"}
              </span>
              <span style={actionRow} role="group" aria-label="Gizmo keyboard shortcuts">
                <ShortcutBadge keys="W" title="Move" />
                <ShortcutBadge keys="E" title="Rotate" />
                <ShortcutBadge keys="R" title="Scale" />
              </span>
            </div>
            <div style={actionRow} role="group" aria-label="Gizmo reference controls">
              <Button
                type="button"
                compact
                onClick={() => void client.gizmoSpaceToggle().then(refreshHud).catch(() => setHudUnavailable(true))}
                title="Switch between world axes and the selected entity's local axes."
              >
                Space: {gizmo?.[3] ?? "world"}
              </Button>
              <Button
                type="button"
                compact
                onClick={() => void client.gizmoPivotToggle().then(refreshHud).catch(() => setHudUnavailable(true))}
                title="Switch the transform handle between the entity origin and selection center."
              >
                Pivot: {gizmo?.[4] ?? "origin"}
              </Button>
              <span style={helperText}>
                Hold <ShortcutBadge keys="Ctrl" ariaLabel="Control" /> for temporary snapping
              </span>
            </div>
          </div>
        )}
      </DisclosureSection>

      <DisclosureSection
        title="Placement"
        summary={snapOn ? "Magnetic snap on" : "Free placement"}
        defaultOpen
        storageKey="transform-placement"
      >
        <div style={{ display: "flex", flexDirection: "column", gap: space.md }}>
          <div style={actionRow}>
            <Button
              id="snapToggle"
              data-testid="snapToggle"
              type="button"
              variant="toggle"
              active={snapOn}
              aria-pressed={snapOn}
              onClick={toggleSnap}
              title="Align movement to nearby surfaces and authoring increments. Hold Control for temporary snapping."
            >
              Magnetic snap: {snapOn ? "On" : "Off"}
            </Button>
            <Button
              id="snapNearest"
              data-testid="snapNearest"
              type="button"
              onClick={() => void snapNearest()}
              title="Move the selected entity to the nearest compatible snap target."
            >
              Snap to nearest
            </Button>
          </div>

          <div>
            <label htmlFor="placeSentence" style={fieldLabel}>
              Placement instruction
            </label>
            <div style={{ ...actionRow, flexWrap: "nowrap" }}>
              <TextField
                id="placeSentence"
                data-testid="placeSentence"
                value={placeVal}
                onChange={(e) => setPlaceVal(e.target.value)}
                placeholder='Example: "upright, 10 cm from the edge"'
                aria-describedby="placeSentence-help"
                style={{ flex: "1 1 180px", minWidth: 0 }}
              />
              <Button id="placeBtn" data-testid="placeBtn" type="button" variant="primary" onClick={() => void placeBySentence()}>
                Place
              </Button>
            </div>
            <p id="placeSentence-help" style={{ ...helperText, marginTop: space.xs }}>
              Uses the selected entity and the visible scene as placement context.
            </p>
          </div>
        </div>
      </DisclosureSection>

      <DisclosureSection
        title="Reusable parts"
        summary={lastComp ? "Saved character ready" : "Save and instantiate"}
        defaultOpen={false}
        storageKey="transform-reuse"
      >
        <p style={{ ...helperText, marginBottom: space.sm }}>
          Save the selected character as a reusable definition, then place independent instances from it.
        </p>
        <div style={actionRow}>
          <Button id="saveChar" data-testid="saveChar" type="button" onClick={() => void saveChar()}>
            Save character for reuse
          </Button>
          <Button
            id="dropInst"
            data-testid="dropInst"
            type="button"
            disabled={!lastComp}
            title={lastComp ? "Create a fresh instance of the saved character." : "Save a character before creating an instance."}
            onClick={() => void dropInstance()}
          >
            Drop fresh instance
          </Button>
        </div>
      </DisclosureSection>

      <DisclosureSection
        title="Structure & visibility"
        summary="Advanced · undoable"
        defaultOpen={false}
        storageKey="transform-structure"
      >
        <div style={{ display: "flex", flexDirection: "column", gap: space.md }}>
          <div>
            <label htmlFor="reparentTo" style={fieldLabel}>
              Parent entity ID
            </label>
            <div style={{ ...actionRow, flexWrap: "nowrap" }}>
              <TextField
                id="reparentTo"
                data-testid="reparentTo"
                mono
                value={reparentVal}
                onChange={(e) => setReparentVal(e.target.value)}
                placeholder="Empty moves the part to scene root"
                aria-describedby="reparentTo-help"
                style={{ flex: "1 1 180px", minWidth: 0 }}
              />
              <Button id="reparentBtn" data-testid="reparentBtn" type="button" onClick={() => void reparent()}>
                Reparent
              </Button>
            </div>
            <p id="reparentTo-help" style={{ ...helperText, marginTop: space.xs }}>
              Changes hierarchy only. The operation is reversible with Ctrl-Z.
            </p>
          </div>

          <div
            style={{
              padding: space.md,
              border: `1px solid ${color.danger.border}`,
              borderRadius: "var(--mtk-radius-md, 4px)",
              background: color.danger.bg,
            }}
          >
            <div style={{ marginBottom: space.sm, color: color.danger.text, font: font.ui, fontSize: fontSize.meta }}>
              Deactivation hides the selected part but preserves its data. Root parts cannot be deactivated.
            </div>
            <Button id="deactPart" data-testid="deactPart" type="button" variant="danger" onClick={() => void deactivatePart()}>
              Deactivate selected part
            </Button>
          </div>
        </div>
      </DisclosureSection>
    </div>
  );
}
