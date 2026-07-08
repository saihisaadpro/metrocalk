//! Re-import panel (M15.10 / ADR-080) — "update the CAD, keep all your work." After a RE-IMPORT (a second
//! CAD import over a scene that already held CAD parts), this surfaces the never-silent per-part diff: what
//! was kept (overrides re-bound onto the matched part), what was added, what was removed (its overrides
//! flagged for reassign-or-discard), and the **held low-confidence matches** — the honest fallback where the
//! matcher is unsure, put to the user as a confirm/reject card rather than silently guessed. Low-confidence is
//! NEVER auto-applied; a deleted part's overrides are flagged, not dropped. Renders nothing on a first import.

import { useEffect, useState } from "react";
import { useStore } from "zustand";
import { projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import { Badge } from "../theme/primitives";
import { color, font, fontSize, space } from "../theme/tokens";
import type { ReimportReport } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

const EMPTY: ReimportReport = { isReimport: false, rebound: 0, added: 0, removed: 0, adjudicate: 0, rows: [], orphans: [], pending: [] };

/** The fate → badge tone + label, keyed on the stable `kind` token (never drifting UI copy in the test surface). */
const FATE: Record<string, { label: string; tone: "success" | "accent" | "warn" }> = {
  unchanged: { label: "Kept", tone: "success" },
  moved: { label: "Moved", tone: "success" },
  matched: { label: "Kept (edited)", tone: "success" },
  adjudicate: { label: "Confirm?", tone: "accent" },
  removed: { label: "Removed", tone: "warn" },
  added: { label: "New", tone: "accent" },
};

function overrideSummary(material: string | null, hasJoint: boolean): string {
  const parts: string[] = [];
  if (material) parts.push(`${material} material`);
  if (hasJoint) parts.push("an animation");
  return parts.length ? parts.join(" + ") : "overrides";
}

export function ReimportPanel({ client }: { client: EditorClient }) {
  const [report, setReport] = useState<ReimportReport>(EMPTY);
  // Re-fetch when the scene population changes (a re-import lands / an undo peels it).
  const baseCount = useStore(projectionStore, (s) => Object.keys(s.base).length);

  useEffect(() => {
    let live = true;
    client
      .cadReimportReport()
      .then((r) => live && setReport(r))
      .catch(() => live && setReport(EMPTY));
    return () => {
      live = false;
    };
  }, [client, baseCount]);

  if (!report.isReimport) return null; // a first import (or no CAD) → this surface stays out of the way

  const resolve = (oldId: string, accept: boolean) => {
    client
      .cadReimportResolve(oldId, accept)
      .then((r) => {
        setReport(r);
        setStatus(accept ? "Kept — overrides re-bound onto the matched part." : "Discarded the uncertain match.");
      })
      .catch(() => {});
  };

  return (
    <div id="reimport-panel" data-testid="reimport-panel" data-rebound={report.rebound} data-removed={report.removed} data-adjudicate={report.adjudicate} style={{ padding: `${space.md}px ${space.lg}px` }}>
      <div style={{ display: "flex", alignItems: "baseline", gap: space.sm, marginBottom: space.xs, font: font.ui, fontSize: fontSize.meta, fontWeight: 600, letterSpacing: 0.4, textTransform: "uppercase", color: color.text.secondary }}>
        <span>Re-import</span>
        <Badge tone="success">{report.rebound} kept</Badge>
      </div>
      <div data-testid="reimport-summary" style={{ font: font.mono, fontSize: fontSize.meta, color: color.text.muted, marginBottom: space.sm }}>
        {report.rebound} part{report.rebound === 1 ? "" : "s"} kept your work · {report.added} added · {report.removed} removed
        {report.adjudicate > 0 && ` · ${report.adjudicate} to confirm`}
      </div>

      {/* The held low-confidence matches — the honest fallback: put the uncertain ones to the user. */}
      {report.pending.map((p) => (
        <div key={p.oldId} data-testid="reimport-adjudicate" data-old-id={p.oldId} className="mtk-card" style={{ marginBottom: space.xs, borderColor: color.accent.border }}>
          <div style={{ font: font.ui, fontSize: fontSize.body, color: color.text.primary, marginBottom: 2 }}>
            Is this the same part? <span style={{ color: color.text.muted }}>({Math.round(p.confidence * 100)}% match)</span>
          </div>
          <div style={{ font: font.ui, fontSize: fontSize.meta, color: color.text.muted, marginBottom: space.xs }}>
            Keep its {overrideSummary(p.material, p.hasJoint)} on the re-imported part?
          </div>
          <div style={{ display: "flex", gap: space.xs }}>
            <button type="button" data-testid="reimport-confirm" onClick={() => resolve(p.oldId, true)} style={{ font: font.ui, fontSize: fontSize.meta, padding: "3px 10px", borderRadius: 4, cursor: "pointer", border: `1px solid ${color.success.border}`, background: color.success.bg, color: color.success.text }}>
              Yes, keep my work
            </button>
            <button type="button" data-testid="reimport-reject" onClick={() => resolve(p.oldId, false)} style={{ font: font.ui, fontSize: fontSize.meta, padding: "3px 10px", borderRadius: 4, cursor: "pointer", border: "1px solid var(--mtk-border-subtle)", background: "transparent", color: color.text.secondary }}>
              No, it's different
            </button>
          </div>
        </div>
      ))}

      {/* Removed parts — their overrides are preserved + flagged, never silently dropped. */}
      {report.orphans.map((o) => (
        <div key={o.oldId} data-testid="reimport-orphan" data-old-id={o.oldId} className="mtk-card" style={{ marginBottom: space.xs, borderColor: color.warn.border }}>
          <div style={{ display: "flex", alignItems: "center", gap: space.sm }}>
            <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", font: font.ui, fontSize: fontSize.body, color: color.text.primary }}>{o.name || "(unnamed part)"}</span>
            <Badge tone="warn">Removed</Badge>
          </div>
          <div style={{ font: font.ui, fontSize: fontSize.meta, color: color.text.muted, marginTop: 2 }}>
            Its {overrideSummary(o.material, o.hasJoint)} {o.material || o.hasJoint ? "was held" : "is gone"} — this part is no longer in the CAD.
          </div>
        </div>
      ))}

      {/* The full per-part diff — every part's fate accounted for (never-silent). Click a kept part to select it. */}
      <div style={{ display: "flex", flexDirection: "column", gap: space.xxs, marginTop: space.xs }}>
        {report.rows.map((r, i) => {
          const f = FATE[r.kind] ?? FATE.removed;
          const selectable = r.newEntity != null;
          return (
            <button
              key={`${r.name}-${i}`}
              type="button"
              className="mtk-card"
              data-testid="reimport-row"
              data-kind={r.kind}
              data-new-entity={r.newEntity ?? ""}
              data-had-overrides={r.hadOverrides}
              disabled={!selectable}
              onClick={() => {
                if (r.newEntity) {
                  projectionStore.getState().select(r.newEntity);
                  setStatus(`${r.name} — ${r.reason}`);
                }
              }}
              title={r.reason}
              style={{ display: "block", textAlign: "left", cursor: selectable ? "pointer" : "default", opacity: selectable ? 1 : 0.7 }}
            >
              <div style={{ display: "flex", alignItems: "center", gap: space.sm }}>
                <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", font: font.ui, fontSize: fontSize.body, color: color.text.primary }}>{r.name || "(unnamed part)"}</span>
                {r.hadOverrides && r.kind !== "removed" && <Badge tone="success">your work</Badge>}
                <Badge tone={f.tone}>{f.label}</Badge>
              </div>
            </button>
          );
        })}
      </div>
    </div>
  );
}
