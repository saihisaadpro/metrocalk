//! Import report (M15.7 / ADR-077) — the never-silent "explain every no" surface applied to CAD import.
//! Every imported part is accounted for by its **honesty class** (exact B-rep · tessellation-only · proxy ·
//! access-denied · AI-reconstructed · failed), read straight off the ECS `CadPart.fidelity` component (so it
//! survives reload and reflects whatever CAD is in the scene right now). The header is the "1,280 parts →
//! 596 exact, 684 tessellation-only, 0 failed" breakdown the milestone calls for; the filter chips are the
//! ECS query ("show tessellation-only parts"); each row explains its fidelity + a one-click fix path and
//! selects the entity on click. Renders NOTHING when the scene has no CAD (total 0), so it never clutters a
//! non-CAD project.

import { useEffect, useState } from "react";
import { useStore } from "zustand";
import { projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import { Badge } from "../theme/primitives";
import { color, font, fontSize, space } from "../theme/tokens";
import type { CadReport } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

const EMPTY: CadReport = { total: 0, exactBrep: 0, tessellationOnly: 0, aiReconstructed: 0, proxy: 0, accessDenied: 0, failed: 0, parts: [] };

/** The plain-language class label · badge tone · why-this-fidelity · a one-click fix path, per honesty
 *  class — the "explain every no" copy, keyed on the stable fidelity token (never drifting UI copy in the
 *  test surface). */
const CLASS: Record<string, { label: string; tone: "success" | "accent" | "warn"; reason: string; fix?: string }> = {
  "exact-brep": { label: "Exact B-rep", tone: "success", reason: "Exact geometry resolved — precision retained." },
  "tessellation-only": { label: "Tessellation-only", tone: "accent", reason: "Rendered from the embedded tessellation cache (a visualization mesh; exact B-rep not resolved).", fix: "Re-export as STEP AP242 to resolve exact B-rep + semantic PMI." },
  "ai-reconstructed": { label: "AI-reconstructed", tone: "accent", reason: "A confidence-scored B-rep candidate reconstructed from the mesh.", fix: "Review + accept the candidate, or re-export as STEP AP242." },
  proxy: { label: "Proxy", tone: "warn", reason: "Proprietary / undecodable geometry — placed as a bounding proxy at its real transform, never a silent empty shell.", fix: "Enable the licensed CAD kernel, or re-export as STEP AP242." },
  "access-denied": { label: "Access-denied", tone: "warn", reason: "The part is encrypted / DRM-protected.", fix: "Unlock the source DRM, or re-export unencrypted." },
  failed: { label: "Failed", tone: "warn", reason: "The geometry cache was present but degenerate (0 triangles) — placed as a diagnosed proxy.", fix: "Re-export / verify the source tessellation." },
};

const CHIPS: { token: string; label: string; count: (r: CadReport) => number }[] = [
  { token: "all", label: "All", count: (r) => r.total },
  { token: "exact-brep", label: "Exact", count: (r) => r.exactBrep },
  { token: "tessellation-only", label: "Tessellation", count: (r) => r.tessellationOnly },
  { token: "ai-reconstructed", label: "AI", count: (r) => r.aiReconstructed },
  { token: "proxy", label: "Proxy", count: (r) => r.proxy },
  { token: "access-denied", label: "Denied", count: (r) => r.accessDenied },
  { token: "failed", label: "Failed", count: (r) => r.failed },
];

export function ImportReport({ client }: { client: EditorClient }) {
  const [report, setReport] = useState<CadReport>(EMPTY);
  const [filter, setFilter] = useState<string>("all");
  // Re-fetch when the scene's entity population changes (a CAD import lands / an undo peels it) — the base
  // map's size is the cheap change signal; the report is a read, so a stale refetch is harmless.
  const baseCount = useStore(projectionStore, (s) => Object.keys(s.base).length);

  useEffect(() => {
    let live = true;
    client
      .cadReport()
      .then((r) => {
        if (live) setReport(r);
      })
      .catch(() => {
        if (live) setReport(EMPTY);
      });
    return () => {
      live = false;
    };
  }, [client, baseCount]);

  if (report.total === 0) return null; // no CAD in the scene → this surface stays out of the way

  const belowExact = report.total - report.exactBrep;
  const rows = report.parts.filter((p) => filter === "all" || p.fidelity === filter);

  return (
    <div
      id="import-report"
      data-testid="import-report"
      data-total={report.total}
      data-below-exact={belowExact}
      style={{ padding: `${space.md}px ${space.lg}px` }}
    >
      <div style={{ display: "flex", alignItems: "baseline", gap: space.sm, marginBottom: space.xs, font: font.ui, fontSize: fontSize.meta, fontWeight: 600, letterSpacing: 0.4, textTransform: "uppercase", color: color.text.secondary }}>
        <span>Import report</span>
        <Badge tone="accent">{report.total}</Badge>
      </div>
      {/* The breakdown line the milestone calls for — every part accounted for, nothing silent. */}
      <div data-testid="import-summary" style={{ font: font.mono, fontSize: fontSize.meta, color: color.text.muted, marginBottom: space.sm }}>
        {report.total} part{report.total === 1 ? "" : "s"} · {report.exactBrep} exact · {report.tessellationOnly} tessellation-only
        {report.proxy > 0 && ` · ${report.proxy} proxy`}
        {report.accessDenied > 0 && ` · ${report.accessDenied} access-denied`}
        {report.aiReconstructed > 0 && ` · ${report.aiReconstructed} AI`}
        {" · "}
        <span style={{ color: report.failed > 0 ? color.warn.text : color.success.text }}>{report.failed} failed</span>
      </div>
      {/* Filter chips = the ECS query ("show tessellation-only parts"); only show classes that occur. */}
      <div style={{ display: "flex", flexWrap: "wrap", gap: space.xxs, marginBottom: space.sm }}>
        {CHIPS.filter((c) => c.count(report) > 0).map((c) => (
          <button
            key={c.token}
            type="button"
            data-testid={`filter-${c.token}`}
            aria-pressed={filter === c.token}
            onClick={() => setFilter(c.token)}
            style={{
              font: font.ui,
              fontSize: fontSize.meta,
              padding: "2px 8px",
              borderRadius: 4,
              cursor: "pointer",
              border: `1px solid ${filter === c.token ? color.accent.border : "var(--mtk-border-subtle)"}`,
              background: filter === c.token ? color.accent.subtle : "transparent",
              color: filter === c.token ? color.accent.base : color.text.secondary,
            }}
          >
            {c.label} {c.count(report)}
          </button>
        ))}
      </div>
      <div style={{ display: "flex", flexDirection: "column", gap: space.xxs }}>
        {rows.map((p) => {
          const cls = CLASS[p.fidelity] ?? CLASS.failed;
          return (
            <button
              key={p.id}
              type="button"
              className="mtk-card"
              data-testid="import-row"
              data-id={p.id}
              data-fidelity={p.fidelity}
              onClick={() => {
                projectionStore.getState().select(p.id);
                setStatus(`${p.name} — ${cls.reason}`);
              }}
              title={cls.fix ? `${cls.reason}\n\nFix: ${cls.fix}` : cls.reason}
              style={{ display: "block", textAlign: "left" }}
            >
              <div style={{ display: "flex", alignItems: "center", gap: space.sm }}>
                <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", font: font.ui, fontSize: fontSize.body, color: color.text.primary }}>
                  {p.name}
                </span>
                <Badge tone={cls.tone}>{cls.label}</Badge>
              </div>
              {cls.fix && (
                <div style={{ font: font.ui, fontSize: fontSize.meta, color: color.text.muted, marginTop: 2 }}>
                  Fix: {cls.fix}
                </div>
              )}
            </button>
          );
        })}
      </div>
    </div>
  );
}
