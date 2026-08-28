//! Import report (M15.7 / ADR-077) — the never-silent "explain every no" surface applied to CAD import.
//! Every imported part is accounted for by its **honesty class** (exact B-rep · tessellation-only · proxy ·
//! access-denied · AI-reconstructed · failed), read straight off the ECS `CadPart.fidelity` component (so it
//! survives reload and reflects whatever CAD is in the scene right now). The header is the "1,280 parts →
//! 596 exact, 684 tessellation-only, 0 failed" breakdown the milestone calls for; the filter chips are the
//! ECS query ("show tessellation-only parts"); each row explains its fidelity + a one-click fix path and
//! selects the entity on click. Renders NOTHING when the scene has no CAD (total 0), so it never clutters a
//! non-CAD project.
//!
//! **ADR-163 — it is now a way to FIND a part, not only a way to be told how many there are.** It asked the
//! shell for the first 500 rows and filtered them in the browser, so on the assembly this panel exists for —
//! 15,711 parts — the header said "412 proxy", the chip said 412, clicking it produced an empty list, and
//! nothing on screen said why: those 412 were not among the 500 alphabetically-first rows the shell had
//! sent. The search, the class filter and the paging all run in the ENGINE now, over one query, so the list
//! and every number printed beside it are the same answer. What is on screen says what it is showing.

import { useEffect, useMemo, useRef, useState } from "react";
import { useStore } from "zustand";
import { projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import { Callout } from "../theme/fields";
import { Badge, Button, SearchField } from "../theme/primitives";
import { color, font, fontSize, space, text } from "../theme/tokens";
import type { CadReport } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

const EMPTY: CadReport = { total: 0, exactBrep: 0, tessellationOnly: 0, aiReconstructed: 0, proxy: 0, accessDenied: 0, failed: 0, matched: 0, offset: 0, parts: [] };

/** Rows per page. 200 is what a person can scan and what the DOM can hold at 60 Hz; the shell will send
 *  up to 2,000, and neither number is the reason a part is unreachable — the pager and the search are. */
const PAGE = 200;

/** How long the search waits after the last keystroke. The engine answers a query over a pre-built row
 *  list in ~1.6 ms at 15,000 parts (ADR-163), so this is not protecting the engine from the typing — it
 *  is protecting the LIST from re-ordering under the reader's eyes on every letter. */
const SETTLE_MS = 180;

/** The plain-language class label · badge tone · why-this-fidelity · a one-click fix path, per honesty
 *  class — the "explain every no" copy, keyed on the stable fidelity token (never drifting UI copy in the
 *  test surface). */
const CLASS: Record<string, { label: string; tone: "success" | "accent" | "warn"; reason: string; fix?: string }> = {
  "exact-brep": { label: "Exact B-rep", tone: "success", reason: "Exact geometry resolved — precision retained." },
  "tessellation-only": { label: "Tessellation-only", tone: "accent", reason: "Rendered from the embedded tessellation cache (a visualization mesh; exact B-rep not resolved).", fix: "Re-export as STEP AP242 to resolve exact B-rep + semantic PMI." },
  "ai-reconstructed": { label: "AI-reconstructed", tone: "accent", reason: "A confidence-scored B-rep candidate reconstructed from the mesh.", fix: "Review + accept the candidate, or re-export as STEP AP242." },
  proxy: { label: "Unresolved", tone: "warn", reason: "Proprietary / undecodable geometry is listed at its real transform without inventing visible geometry.", fix: "Enable the licensed CAD kernel, add the matching STEP companion, or re-export as STEP AP242." },
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
  const [query, setQuery] = useState("");
  const [settled, setSettled] = useState("");
  const [offset, setOffset] = useState(0);
  // The scene's own part count, remembered from the last unasked question. The panel hides itself when
  // the project has no CAD — and that test must NOT be `report.total`, which is the count of the CURRENT
  // query's matches: a search that finds nothing would otherwise make the whole panel, including the
  // search box that produced it, vanish under the user's hands.
  const [sceneTotal, setSceneTotal] = useState(0);
  // Re-fetch when the scene's entity population changes (a CAD import lands / an undo peels it) — the base
  // map's size is the cheap change signal; the report is a read, so a stale refetch is harmless.
  const baseCount = useStore(projectionStore, (s) => Object.keys(s.base).length);
  const selected = useStore(projectionStore, (s) => s.selectedId);
  // Monotonic request id: a slow answer to an old query must never overwrite a fresh one.
  const latest = useRef(0);

  useEffect(() => {
    const id = window.setTimeout(() => setSettled(query), SETTLE_MS);
    return () => window.clearTimeout(id);
  }, [query]);

  useEffect(() => {
    const ticket = ++latest.current;
    client
      .cadReportPage(settled, filter, offset, PAGE)
      .then((r) => {
        if (ticket !== latest.current) return;
        // A page that fell off the end of a report that SHRANK under it — an undo peeling a re-import
        // away while you were on page 9 — is the one state where "no rows" and "no matches" are
        // different things, and the sentence below cannot tell them apart. Go back to the first page
        // rather than print "No CAD part" over a report that has 3,000 of them.
        if (r.matched > 0 && r.parts.length === 0 && r.offset > 0) {
          setOffset(0);
          return;
        }
        setReport(r);
        if (!settled.trim() && filter === "all") setSceneTotal(r.total);
      })
      .catch(() => {
        if (ticket === latest.current) setReport(EMPTY);
      });
  }, [client, baseCount, settled, filter, offset]);

  const searching = settled.trim().length > 0;
  const narrowed = searching || filter !== "all";
  // Nothing to say and nothing asked → stay out of the way, exactly as before. Once the user has asked a
  // question, the panel owes them an answer even when the answer is "none".
  const invisible = sceneTotal === 0 && !narrowed;

  const chips = useMemo(() => CHIPS.filter((c) => c.count(report) > 0 || c.token === filter), [report, filter]);

  if (invisible) return null;

  const belowExact = report.total - report.exactBrep;
  const rows = report.parts;
  const first = rows.length === 0 ? 0 : report.offset + 1;
  const last = report.offset + rows.length;
  const classLabel = filter === "all" ? "" : ` ${CLASS[filter]?.label.toLowerCase() ?? filter}`;
  // One sentence that can only ever describe the rows directly under it, because every number in it came
  // back with them: where the page starts, how long it is, and how many rows the current question has.
  const showing = rows.length === 0
    ? `No${classLabel || " CAD"} part${searching ? ` matches “${settled.trim()}”` : ""}`
    : `Showing ${first.toLocaleString()}–${last.toLocaleString()} of ${report.matched.toLocaleString()}${classLabel} part${report.matched === 1 ? "" : "s"}${searching ? ` matching “${settled.trim()}”` : ""}`;

  function clear() {
    setQuery("");
    setSettled("");
    setFilter("all");
    setOffset(0);
  }

  return (
    <div
      id="import-report"
      data-testid="import-report"
      data-total={report.total}
      data-below-exact={belowExact}
      data-matched={report.matched}
      data-offset={report.offset}
      data-shown={rows.length}
      style={{ padding: `${space.md}px ${space.lg}px` }}
    >
      <div style={{ display: "flex", alignItems: "baseline", gap: space.sm, marginBottom: space.xs, ...text.eyebrow }}>
        <span>Import report</span>
        <Badge tone="accent">{report.total.toLocaleString()}</Badge>
      </div>
      {/* The breakdown line the milestone calls for — every part accounted for, nothing silent. With a
          search running it describes the MATCHES, which is what the rows below it are. */}
      <div data-testid="import-summary" style={{ font: font.mono, fontSize: fontSize.meta, color: color.text.muted, marginBottom: space.sm }}>
        {/* Localised, because this line's whole job is to be read at a glance and "15711" is not:
            the assembly it describes has five digits and the separator is what makes it a number. */}
        {report.total.toLocaleString()} part{report.total === 1 ? "" : "s"} · {report.exactBrep.toLocaleString()} exact · {report.tessellationOnly.toLocaleString()} tessellation-only
        {report.proxy > 0 && ` · ${report.proxy.toLocaleString()} proxy`}
        {report.accessDenied > 0 && ` · ${report.accessDenied.toLocaleString()} access-denied`}
        {report.aiReconstructed > 0 && ` · ${report.aiReconstructed.toLocaleString()} AI`}
        {" · "}
        <span style={{ color: report.failed > 0 ? color.warn.text : color.success.text }}>{report.failed.toLocaleString()} failed</span>
      </div>
      {/* Search runs in the ENGINE, over the part's display name AND its source reference — the two names
          a CAD part has, of which the user was handed exactly one. */}
      <SearchField
        data-testid="import-search"
        value={query}
        aria-label="Search imported parts by name or source reference"
        placeholder="Find a part by name or reference…"
        onChange={(e) => {
          setQuery(e.target.value);
          setOffset(0);
        }}
        style={{ width: "100%", boxSizing: "border-box", marginBottom: space.sm }}
      />
      {/* Filter chips = the ECS query ("show tessellation-only parts"), and the count on each one is now
          the count the LIST will hold, because the shell applies the same filter that produced them. */}
      <div style={{ display: "flex", flexWrap: "wrap", gap: space.xxs, marginBottom: space.sm }}>
        {chips.map((c) => (
          <button
            key={c.token}
            type="button"
            data-testid={`filter-${c.token}`}
            aria-pressed={filter === c.token}
            onClick={() => {
              setFilter(c.token);
              setOffset(0);
            }}
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
            {c.label} {c.count(report).toLocaleString()}
          </button>
        ))}
      </div>
      {/* What this page is, in words, from the payload that produced it. */}
      <div
        data-testid="import-showing"
        style={{ display: "flex", alignItems: "center", gap: space.sm, flexWrap: "wrap", font: font.ui, fontSize: fontSize.meta, color: color.text.muted, marginBottom: space.sm }}
      >
        <span style={{ flex: 1, minWidth: 0 }}>{showing}</span>
        {report.matched > PAGE && (
          <span style={{ display: "flex", gap: space.xxs }}>
            <Button
              compact
              data-testid="import-prev"
              disabled={report.offset === 0}
              disabledReason="You are on the first page of this list"
              // Only when it can act: an explicit `title` OVERRIDES `disabledReason` on `Button`, so
              // writing one unconditionally would replace the refusal's sentence with a range that
              // does not exist.
              title={report.offset === 0 ? undefined : `Show parts ${Math.max(0, report.offset - PAGE) + 1}–${report.offset}`}
              onClick={() => setOffset(Math.max(0, report.offset - PAGE))}
            >
              Previous
            </Button>
            <Button
              compact
              data-testid="import-next"
              disabled={last >= report.matched}
              disabledReason="You are on the last page of this list"
              title={last >= report.matched ? undefined : `Show parts ${last + 1}–${Math.min(last + PAGE, report.matched)}`}
              onClick={() => setOffset(report.offset + PAGE)}
            >
              Next
            </Button>
          </span>
        )}
      </div>
      {/* An empty list is a RESULT, and it says so with the way back out beside it. The old panel drew
          nothing here at all — a chip reading "Proxy 412" above a blank space. */}
      {rows.length === 0 && (
        <Callout tone="neutral" data-testid="import-empty">
          <span>
            {narrowed
              ? `Nothing here matches. ${sceneTotal.toLocaleString()} part${sceneTotal === 1 ? "" : "s"} were imported in total.`
              : "No CAD parts in this project yet."}
          </span>
          {narrowed && (
            <div style={{ marginTop: space.xs }}>
              <Button compact data-testid="import-clear" onClick={clear}>
                Clear the search and filter
              </Button>
            </div>
          )}
        </Callout>
      )}
      <div style={{ display: "flex", flexDirection: "column", gap: space.xxs }}>
        {rows.map((p) => {
          const cls = CLASS[p.fidelity] ?? CLASS.failed;
          const reason = p.reason ?? cls.reason;
          const fix = p.fix ?? cls.fix;
          return (
            <div
              key={p.id}
              className={["mtk-card", selected === p.id && "is-selected"].filter(Boolean).join(" ")}
              data-testid="import-row"
              data-id={p.id}
              data-fidelity={p.fidelity}
              style={{ alignItems: "flex-start", cursor: "default" }}
            >
              <button
                type="button"
                data-testid="import-select"
                onClick={() => {
                  projectionStore.getState().select(p.id);
                  // AND the engine's selection, which this panel never touched: without it the row
                  // highlighted here while the viewport, the gizmo and the inspector stayed on whatever
                  // was selected before — one list disagreeing with the 3D view about what is selected.
                  void client.gizmoSelect(p.id).catch((e) => console.error("gizmoSelect failed (engine selection may be out of sync)", e));
                  setStatus(`${p.name} — ${reason}`);
                }}
                title={fix ? `${reason}\n\nFix: ${fix}` : reason}
                style={{ flex: 1, minWidth: 0, display: "block", textAlign: "left", background: "none", border: "none", padding: 0, color: "inherit", font: "inherit", cursor: "pointer" }}
              >
                <div style={{ display: "flex", alignItems: "center", gap: space.sm }}>
                  <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", font: font.ui, fontSize: fontSize.body, color: color.text.primary }}>
                    {p.name}
                  </span>
                  <Badge tone={cls.tone}>{cls.label}</Badge>
                </div>
                {/* All three are `string | null` — the shell sends the key holding null when it has
                    nothing to say — so this line is absent, not empty, when none of them carries a
                    value. `data-testid` rather than the prose, so the test keys on a stable token. */}
                {(p.reference || p.strategy || p.sourceFormat) && (
                  <div data-testid="import-row-provenance" style={{ font: font.mono, fontSize: fontSize.meta, color: color.text.muted, marginTop: 2, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                    {[p.reference, p.strategy, p.sourceFormat].filter(Boolean).join(" · ")}
                  </div>
                )}
                {fix && (
                  <div data-testid="import-row-fix" style={{ font: font.ui, fontSize: fontSize.meta, color: color.text.muted, marginTop: 2 }}>
                    Fix: {fix}
                  </div>
                )}
              </button>
              {/* Selecting one part of a 262 m assembly and never being shown where it is was the other
                  half of "I cannot find it". The camera move is its own control because it moves the
                  view, and a list you are scanning must not fly the camera on every click. */}
              <Button
                compact
                data-testid="import-frame"
                title={`Move the camera to ${p.name}`}
                onClick={() => {
                  projectionStore.getState().select(p.id);
                  void client.gizmoSelect(p.id).catch(() => {});
                  client.focusEntity(p.id);
                  setStatus(`Framed ${p.name}`);
                }}
              >
                Frame
              </Button>
            </div>
          );
        })}
      </div>
    </div>
  );
}
