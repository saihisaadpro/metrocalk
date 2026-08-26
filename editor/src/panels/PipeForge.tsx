//! Pipe Forge — direct-in-viewport route drawing with progressively disclosed graph, fitting and catalog tools.
//! React controls the transaction-sized commands; native state remains authoritative across refresh and rebake.

import { useEffect, useMemo, useState } from "react";
import { Icon } from "../theme/icons";
import { Badge, Button, NumericField } from "../theme/primitives";
import { color, elevation, font, fontSize, radius, space, text, z } from "../theme/tokens";
import type {
  PipeBakeReport,
  PipeFittingKind,
  PipeForgeOptions,
  PipeForgeStatus,
  UserFittingCatalogEntry,
} from "../transport/protocol";
import type { EditorClient } from "../transport/session";

export interface PipeForgeProps {
  client: EditorClient;
  status: PipeForgeStatus | null;
  /** A selected entity with a persisted PipeRecipe, supplied by the projection-backed app shell. */
  editableEntityId?: string | null;
  onStatus: (status: PipeForgeStatus) => void;
  onBaked: (report: PipeBakeReport) => void;
  onPendingChange?: (pending: boolean) => void;
  style?: React.CSSProperties;
}

const DEFAULT_OPTIONS: PipeForgeOptions = {
  kit: "galvanized",
  diameterCm: 5,
  quality: "production",
  autoFittings: true,
};

const KIT_LABEL: Record<PipeForgeOptions["kit"], string> = {
  galvanized: "Galvanized steel",
  copper: "Copper",
  pvc: "PVC",
  scifi: "Sci-fi",
};

const FITTING_LABEL: Record<PipeFittingKind, string> = {
  elbow: "Elbow",
  coupling: "Coupling",
  tee: "Tee",
  valve: "Valve",
  flange: "Flange",
};

type PendingAction =
  | "start"
  | "edit"
  | "undo"
  | "bake"
  | "cancel"
  | "branch"
  | "end-branch"
  | "move-handle"
  | "remove-handle"
  | "place-fitting"
  | "remove-fitting"
  | "save-catalog"
  | "remove-catalog"
  | null;

interface CatalogDraft {
  id: string;
  label: string;
  kind: PipeFittingKind;
  assetHandle: string;
  diameterScale: number;
  lengthScale: number;
}

const EMPTY_CATALOG: CatalogDraft = {
  id: "",
  label: "",
  kind: "valve",
  assetHandle: "",
  diameterScale: 1,
  lengthScale: 1,
};

function errorMessage(error: unknown, fallback: string): string {
  return error instanceof Error && error.message ? `${fallback}: ${error.message}` : fallback;
}

function catalogIdFrom(label: string): string {
  return label
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 48);
}

export function PipeForge({
  client,
  status,
  editableEntityId = null,
  onStatus,
  onBaked,
  onPendingChange,
  style,
}: PipeForgeProps) {
  const [options, setOptions] = useState<PipeForgeOptions>(DEFAULT_OPTIONS);
  const [pending, setPending] = useState<PendingAction>(null);
  const [error, setError] = useState<string | null>(null);
  const [lastReport, setLastReport] = useState<PipeBakeReport | null>(null);
  const [selectedHandleId, setSelectedHandleId] = useState<number | null>(null);
  const [handlePosition, setHandlePosition] = useState<[number, number, number]>([0, 0, 0]);
  const [branchDiameterCm, setBranchDiameterCm] = useState(DEFAULT_OPTIONS.diameterCm);
  const [fittingKind, setFittingKind] = useState<PipeFittingKind>("tee");
  const [fittingCatalogId, setFittingCatalogId] = useState("");
  const [catalog, setCatalog] = useState<CatalogDraft>(EMPTY_CATALOG);

  const sessionActive = status?.active === true;
  // A bake reply fences an older polled Drawing status. Starting/editing clears the report first.
  const completed = Boolean(lastReport?.handle && lastReport.entityId);
  const drawing = sessionActive && !completed;
  const editing = drawing && Boolean(status?.editingEntity);
  const handles = status?.handles ?? [];
  const edges = status?.edges ?? [];
  const fittings = status?.fittings ?? [];
  const fittingCatalog = status?.fittingCatalog ?? [];
  const selectedHandle = handles.find((handle) => handle.nodeId === selectedHandleId) ?? null;
  const matchingCatalog = useMemo(
    () => fittingCatalog.filter((entry) => entry.kind === fittingKind),
    [fittingCatalog, fittingKind],
  );
  const activeOptions: PipeForgeOptions = {
    kit: drawing && status?.kit ? status.kit : options.kit,
    diameterCm: drawing && status?.diameterCm != null ? status.diameterCm : options.diameterCm,
    quality: drawing && status?.quality ? status.quality : options.quality,
    autoFittings: drawing && status?.autoFittings != null ? status.autoFittings : options.autoFittings,
  };

  useEffect(() => onPendingChange?.(pending !== null), [onPendingChange, pending]);

  useEffect(() => {
    if (handles.length === 0) {
      setSelectedHandleId(null);
      return;
    }
    const next = handles.find((handle) => handle.nodeId === selectedHandleId) ?? handles[0];
    setSelectedHandleId(next.nodeId);
    setHandlePosition([...next.position]);
  }, [handles, selectedHandleId]);

  useEffect(() => {
    if (fittingCatalogId && !matchingCatalog.some((entry) => entry.id === fittingCatalogId)) {
      setFittingCatalogId("");
    }
  }, [fittingCatalogId, matchingCatalog]);

  async function updateStatus(action: Exclude<PendingAction, null>, fallback: string, command: () => Promise<PipeForgeStatus>) {
    if (pending) return false;
    setPending(action);
    setError(null);
    try {
      onStatus(await command());
      return true;
    } catch (cause) {
      setError(errorMessage(cause, fallback));
      return false;
    } finally {
      setPending(null);
    }
  }

  async function startDrawing() {
    if (pending) return;
    setLastReport(null);
    await updateStatus("start", "Couldn’t start drawing", () => client.pipeForgeStart(options));
  }

  async function editSelected() {
    if (!editableEntityId || pending) return;
    setLastReport(null);
    await updateStatus("edit", "Couldn’t reopen this pipe", () => client.pipeForgeEdit(editableEntityId));
  }

  async function bakeAsset() {
    const routeNodes = handles.length || status?.points || 0;
    if (pending || !status?.canBake || routeNodes < 2) return;
    setPending("bake");
    setError(null);
    try {
      const report = await client.pipeForgeBake();
      setLastReport(report);
      if (!report.handle || !report.entityId) setError(report.message || "The asset could not be placed");
      onBaked(report);
    } catch (cause) {
      setError(errorMessage(cause, editing ? "Couldn’t rebake this route" : "Couldn’t bake this asset"));
    } finally {
      setPending(null);
    }
  }

  async function saveCatalogEntry() {
    const label = catalog.label.trim();
    const id = catalog.id.trim() || catalogIdFrom(label);
    if (!label || !id) {
      setError("Give the fitting a recognisable name before saving it");
      return;
    }
    const entry: UserFittingCatalogEntry = {
      id,
      label,
      kind: catalog.kind,
      assetHandle: catalog.assetHandle.trim() || null,
      diameterScale: catalog.diameterScale,
      lengthScale: catalog.lengthScale,
    };
    if (await updateStatus("save-catalog", "Couldn’t save this catalog fitting", () => client.pipeForgeUpsertCatalog(entry))) {
      setCatalog(EMPTY_CATALOG);
    }
  }

  const routeNodes = handles.length || status?.points || 0;
  const canBake = Boolean(drawing && status?.canBake && routeNodes >= 2 && !pending);
  const busy = pending !== null;
  const leafRemovable = Boolean(selectedHandle && selectedHandle.connectedEdges.length <= 1 && handles.length > 2);

  return (
    <section
      id="pipe-forge"
      data-testid="pipe-forge"
      data-active={drawing ? "true" : "false"}
      aria-label="Pipe Forge"
      aria-busy={busy}
      // Chrome owns these events; they must never become route points, camera motion or a context menu.
      onPointerDown={(event) => event.stopPropagation()}
      onPointerMove={(event) => event.stopPropagation()}
      onPointerUp={(event) => event.stopPropagation()}
      onClick={(event) => event.stopPropagation()}
      onWheel={(event) => event.stopPropagation()}
      onContextMenu={(event) => event.stopPropagation()}
      style={{
        position: "absolute",
        top: space.xxl + space.xl + space.xs,
        left: space.sm,
        zIndex: z.chrome,
        width: 336,
        maxWidth: "calc(100% - 16px)",
        maxHeight: "calc(100% - 104px)",
        overflowX: "hidden",
        overflowY: "auto",
        overscrollBehavior: "contain",
        pointerEvents: "auto",
        color: color.text.primary,
        background: color.bg.raised,
        border: `1px solid ${drawing ? color.accent.border : color.border.default}`,
        borderRadius: radius.xl,
        boxShadow: elevation.e2,
        font: font.ui,
        fontSize: fontSize.body,
        ...style,
      }}
    >
      <header style={headerStyle}>
        <div style={{ display: "flex", alignItems: "center", gap: space.sm }}>
          <Icon name="pipe" size="lg" style={{ color: color.accent.base }} />
          <span style={text.panelTitle}>Pipe Forge</span>
        </div>
        <Badge tone={drawing ? "success" : "neutral"}>{editing ? "Editing" : drawing ? "Drawing" : "Ready"}</Badge>
      </header>

      {!drawing && !completed && (
        <div data-testid="pipe-forge-setup" style={{ padding: space.lg }}>
          {editableEntityId && (
            <div style={{ padding: space.sm, marginBottom: space.md, border: `1px solid ${color.accent.border}`, borderRadius: radius.lg, background: color.accent.subtle }}>
              <div style={{ fontWeight: 650, marginBottom: space.xs }}>Editable pipe selected</div>
              <div style={{ ...hintStyle, marginBottom: space.sm }}>Restore its route handles, branches and semantic fittings without losing the baked asset.</div>
              <Button
                data-testid="pipe-forge-edit"
                variant="secondary"
                compact
                disabled={busy}
                title="Open the selected PipeRecipe for non-destructive handle editing"
                onClick={() => void editSelected()}
                style={{ width: "100%" }}
              >
                {pending === "edit" ? "Opening route…" : "Edit selected pipe"}
              </Button>
            </div>
          )}

          <SetupControls options={options} disabled={busy} onChange={setOptions} />
          <Button
            data-testid="pipe-forge-start"
            variant="primary"
            disabled={busy || options.diameterCm <= 0}
            onClick={() => void startDrawing()}
            style={{ width: "100%" }}
          >
            {pending === "start" ? "Starting…" : "Draw new pipe"}
          </Button>
          <div style={{ ...hintStyle, marginTop: space.sm }}>Then click in the viewport to place each point.</div>
          {status?.message && <div data-testid="pipe-forge-message" style={{ ...hintStyle, marginTop: space.xs }}>{status.message}</div>}
        </div>
      )}

      {drawing && status && (
        <div data-testid="pipe-forge-active" style={{ padding: space.lg }}>
          <div style={{ display: "flex", alignItems: "center", gap: space.xs, marginBottom: space.md, flexWrap: "wrap" }}>
            <Badge tone="accent">{KIT_LABEL[activeOptions.kit]}</Badge>
            <Badge>{activeOptions.diameterCm} cm</Badge>
            <Badge>{activeOptions.quality}</Badge>
            <Badge>{activeOptions.autoFittings ? "Auto joints" : "Manual fittings"}</Badge>
          </div>

          <div style={statGridStyle}>
            <Stat dataTestId="pipe-forge-points" label="Handles" value={String(routeNodes)} />
            <Stat dataTestId="pipe-forge-length" label="Length" value={`${status.lengthM.toFixed(2)} m`} />
            <Stat dataTestId="pipe-forge-triangles" label="Triangles" value={status.previewTriangles.toLocaleString("en-GB")} />
          </div>

          {status.branchFrom != null && (
            <div data-testid="pipe-forge-branch-mode" role="status" style={{ padding: space.sm, marginTop: space.sm, borderRadius: radius.lg, color: color.accent.base, background: color.accent.subtle, border: `1px solid ${color.accent.border}` }}>
              <div style={{ fontWeight: 650 }}>Branch drawing is active</div>
              <div style={{ ...hintStyle, color: "inherit", margin: `${space.xs}px 0` }}>Click the viewport to extend from handle {status.branchFrom}. Finish the branch to resume the main route.</div>
              <Button data-testid="pipe-forge-end-branch" variant="secondary" compact disabled={busy} onClick={() => void updateStatus("end-branch", "Couldn’t finish this branch", () => client.pipeForgeEndBranch())}>
                {pending === "end-branch" ? "Finishing…" : "Finish branch"}
              </Button>
            </div>
          )}

          <div data-testid="pipe-forge-message" aria-live="polite" style={{ ...hintStyle, minHeight: 28, margin: `${space.sm}px 0` }}>
            {status.message || (routeNodes === 0 ? "Click the viewport to place the first point." : "Click again to extend the run.")}
            <div style={{ marginTop: space.xs }}>Ctrl/Cmd+Z undo point · Esc cancel</div>
          </div>

          <div style={{ display: "grid", gridTemplateColumns: "1fr 1.2fr", gap: space.xs }}>
            <Button data-testid="pipe-forge-undo" variant="secondary" compact disabled={busy || routeNodes === 0} onClick={() => void updateStatus("undo", "Couldn’t undo that point", () => client.pipeForgeUndo())}>
              {pending === "undo" ? "Undoing…" : "Undo point"}
            </Button>
            <Button
              data-testid="pipe-forge-bake"
              variant="primary"
              compact
              disabled={!canBake}
              title={!canBake ? "Place at least two valid handles and resolve route errors before baking" : editing ? "Rebuild the selected asset from its editable route" : "Create a game-ready mesh asset"}
              onClick={() => void bakeAsset()}
            >
              {pending === "bake" ? "Building…" : editing ? "Rebake asset" : "Finish & bake"}
            </Button>
          </div>

          <details data-testid="pipe-forge-network" style={detailsStyle}>
            <summary style={summaryStyle}>Route network <span style={summaryMetaStyle}>{handles.length} handles · {edges.length} edges</span></summary>
            <div style={detailsBodyStyle}>
              <p style={sectionHintStyle}>Select a stable handle to create branches or make precise post-bake corrections.</p>
              <label style={stackedLabelStyle}>
                <span style={labelStyle}>Route handle</span>
                <select
                  data-testid="pipe-forge-handle"
                  aria-label="Route handle"
                  className="mtk-input"
                  value={selectedHandleId ?? ""}
                  disabled={busy || handles.length === 0}
                  onChange={(event) => setSelectedHandleId(Number(event.target.value))}
                >
                  {handles.length === 0 && <option value="">Place a point first</option>}
                  {handles.map((handle) => (
                    <option key={handle.nodeId} value={handle.nodeId}>
                      Handle {handle.nodeId} · {handle.connectedEdges.length > 2 ? "junction" : handle.connectedEdges.length <= 1 ? "end" : "route"}
                    </option>
                  ))}
                </select>
              </label>

              <fieldset disabled={busy || !selectedHandle} style={fieldsetStyle}>
                <legend style={legendStyle}>Position in metres</legend>
                <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: space.xs }}>
                  {(["X", "Y", "Z"] as const).map((axis, index) => (
                    <label key={axis} style={axisLabelStyle}>
                      <span>{axis}</span>
                      <NumericField
                        data-testid={`pipe-forge-handle-${axis.toLowerCase()}`}
                        ariaLabel={`${axis} handle position in metres`}
                        value={handlePosition[index]}
                        min={-100_000}
                        max={100_000}
                        step={0.1}
                        disabled={busy || !selectedHandle}
                        onCommit={(value) => setHandlePosition((current) => current.map((item, itemIndex) => itemIndex === index ? value : item) as [number, number, number])}
                      />
                    </label>
                  ))}
                </div>
                <Button
                  data-testid="pipe-forge-move-handle"
                  variant="secondary"
                  compact
                  disabled={busy || !selectedHandle}
                  title="Move the selected route handle and rebuild connected preview segments"
                  onClick={() => selectedHandle && void updateStatus("move-handle", "Couldn’t move this handle", () => client.pipeForgeMoveHandle(selectedHandle.nodeId, ...handlePosition))}
                  style={{ width: "100%", marginTop: space.xs }}
                >
                  {pending === "move-handle" ? "Moving…" : "Apply position"}
                </Button>
              </fieldset>

              <div style={{ display: "grid", gridTemplateColumns: "1fr auto", alignItems: "end", gap: space.xs, marginTop: space.md }}>
                <label style={stackedLabelStyle}>
                  <span style={labelStyle}>Branch diameter (cm)</span>
                  <NumericField
                    data-testid="pipe-forge-branch-diameter"
                    ariaLabel="Branch diameter in centimetres"
                    value={branchDiameterCm}
                    min={1}
                    max={200}
                    step={0.5}
                    disabled={busy || !selectedHandle || status.branchFrom != null}
                    onCommit={setBranchDiameterCm}
                  />
                </label>
                <Button
                  data-testid="pipe-forge-begin-branch"
                  variant="secondary"
                  compact
                  disabled={busy || !selectedHandle || status.branchFrom != null || branchDiameterCm <= 0}
                  title="The next viewport click starts a branch at this stable handle"
                  onClick={() => selectedHandle && void updateStatus("branch", "Couldn’t start a branch here", () => client.pipeForgeBeginBranch(selectedHandle.nodeId, branchDiameterCm))}
                >
                  {pending === "branch" ? "Starting…" : "Draw branch"}
                </Button>
              </div>

              <Button
                data-testid="pipe-forge-remove-handle"
                variant="danger"
                compact
                disabled={busy || !leafRemovable}
                title={leafRemovable ? "Remove this unbranched route end" : "Only a leaf handle can be removed; keep at least two route handles"}
                onClick={() => selectedHandle && void updateStatus("remove-handle", "Couldn’t remove this handle", () => client.pipeForgeRemoveHandle(selectedHandle.nodeId))}
                style={{ width: "100%", marginTop: space.md }}
              >
                {pending === "remove-handle" ? "Removing…" : "Remove leaf handle"}
              </Button>
            </div>
          </details>

          <details data-testid="pipe-forge-fittings" style={detailsStyle}>
            <summary style={summaryStyle}>Fittings <span style={summaryMetaStyle}>{fittings.length} placed</span></summary>
            <div style={detailsBodyStyle}>
              <p style={sectionHintStyle}>Attach semantic tees, valves and flanges. Built-in fittings always have a production-safe procedural fallback.</p>
              <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: space.xs }}>
                <label style={stackedLabelStyle}>
                  <span style={labelStyle}>Kind</span>
                  <select data-testid="pipe-forge-fitting-kind" aria-label="Fitting kind" className="mtk-input" value={fittingKind} disabled={busy} onChange={(event) => setFittingKind(event.target.value as PipeFittingKind)}>
                    {(Object.keys(FITTING_LABEL) as PipeFittingKind[]).map((kind) => <option key={kind} value={kind}>{FITTING_LABEL[kind]}</option>)}
                  </select>
                </label>
                <label style={stackedLabelStyle}>
                  <span style={labelStyle}>Style</span>
                  <select data-testid="pipe-forge-fitting-catalog" aria-label="Fitting catalog style" className="mtk-input" value={fittingCatalogId} disabled={busy} onChange={(event) => setFittingCatalogId(event.target.value)}>
                    <option value="">Built-in {FITTING_LABEL[fittingKind].toLowerCase()}</option>
                    {matchingCatalog.map((entry) => <option key={entry.id} value={entry.id}>{entry.label}</option>)}
                  </select>
                </label>
              </div>
              <Button
                data-testid="pipe-forge-place-fitting"
                variant="secondary"
                compact
                disabled={busy || !selectedHandle}
                title={selectedHandle ? `Place at handle ${selectedHandle.nodeId}` : "Select a route handle first"}
                onClick={() => selectedHandle && void updateStatus("place-fitting", "Couldn’t place this fitting", () => client.pipeForgePlaceFitting(selectedHandle.nodeId, fittingKind, fittingCatalogId || undefined))}
                style={{ width: "100%", marginTop: space.sm }}
              >
                {pending === "place-fitting" ? "Placing…" : `Place ${FITTING_LABEL[fittingKind].toLowerCase()}`}
              </Button>

              {fittings.length === 0 ? (
                <div style={emptyInlineStyle}>No explicit fittings. Topology can still generate automatic elbows and tees.</div>
              ) : (
                <ul aria-label="Placed fittings" style={listStyle}>
                  {fittings.map((fitting) => (
                    <li key={fitting.id} style={listRowStyle}>
                      <span><strong>{FITTING_LABEL[fitting.kind]}</strong> · handle {fitting.nodeId}{fitting.automatic ? " · automatic" : ""}</span>
                      <Button
                        data-testid={`pipe-forge-remove-fitting-${fitting.id}`}
                        variant="ghost"
                        compact
                        disabled={busy || fitting.automatic}
                        title={fitting.automatic ? "Automatic fittings follow route topology" : `Remove ${FITTING_LABEL[fitting.kind]}`}
                        aria-label={`Remove ${FITTING_LABEL[fitting.kind]} from handle ${fitting.nodeId}`}
                        onClick={() => void updateStatus("remove-fitting", "Couldn’t remove this fitting", () => client.pipeForgeRemoveFitting(fitting.id))}
                      >
                        Remove
                      </Button>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </details>

          <details data-testid="pipe-forge-catalog" style={detailsStyle}>
            <summary style={summaryStyle}>Fitting catalog <span style={summaryMetaStyle}>{fittingCatalog.length} custom</span></summary>
            <div style={detailsBodyStyle}>
              <p style={sectionHintStyle}>Register project-specific fitting assets. The asset handle is preserved for export; a bounded semantic proxy keeps authoring dependable.</p>
              <label style={stackedLabelStyle}>
                <span style={labelStyle}>Display name</span>
                <input data-testid="pipe-forge-catalog-label" aria-label="Catalog fitting display name" className="mtk-input" value={catalog.label} disabled={busy} placeholder="e.g. Isolation valve" onChange={(event) => setCatalog((current) => ({ ...current, label: event.target.value }))} />
              </label>
              <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: space.xs }}>
                <label style={stackedLabelStyle}>
                  <span style={labelStyle}>Kind</span>
                  <select data-testid="pipe-forge-catalog-kind" aria-label="Catalog fitting kind" className="mtk-input" value={catalog.kind} disabled={busy} onChange={(event) => setCatalog((current) => ({ ...current, kind: event.target.value as PipeFittingKind }))}>
                    {(Object.keys(FITTING_LABEL) as PipeFittingKind[]).map((kind) => <option key={kind} value={kind}>{FITTING_LABEL[kind]}</option>)}
                  </select>
                </label>
                <label style={stackedLabelStyle}>
                  <span style={labelStyle}>Stable ID</span>
                  <input data-testid="pipe-forge-catalog-id" aria-label="Catalog fitting stable ID" className="mtk-input" value={catalog.id} disabled={busy} placeholder="Generated from name" onChange={(event) => setCatalog((current) => ({ ...current, id: event.target.value }))} />
                </label>
              </div>
              <label style={stackedLabelStyle}>
                <span style={labelStyle}>Asset handle (optional)</span>
                <input data-testid="pipe-forge-catalog-asset" aria-label="Catalog fitting asset handle" className="mtk-input" value={catalog.assetHandle} disabled={busy} placeholder="mtkasset:…" onChange={(event) => setCatalog((current) => ({ ...current, assetHandle: event.target.value }))} />
              </label>
              <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: space.xs }}>
                <label style={stackedLabelStyle}>
                  <span style={labelStyle}>Diameter scale</span>
                  <NumericField data-testid="pipe-forge-catalog-diameter-scale" ariaLabel="Catalog fitting diameter scale" value={catalog.diameterScale} min={0.1} max={10} step={0.05} disabled={busy} onCommit={(diameterScale) => setCatalog((current) => ({ ...current, diameterScale }))} />
                </label>
                <label style={stackedLabelStyle}>
                  <span style={labelStyle}>Length scale</span>
                  <NumericField data-testid="pipe-forge-catalog-length-scale" ariaLabel="Catalog fitting length scale" value={catalog.lengthScale} min={0.1} max={10} step={0.05} disabled={busy} onCommit={(lengthScale) => setCatalog((current) => ({ ...current, lengthScale }))} />
                </label>
              </div>
              <Button data-testid="pipe-forge-save-catalog" variant="secondary" compact disabled={busy || !catalog.label.trim()} onClick={() => void saveCatalogEntry()} style={{ width: "100%", marginTop: space.sm }}>
                {pending === "save-catalog" ? "Saving…" : catalog.id && fittingCatalog.some((entry) => entry.id === catalog.id) ? "Update catalog fitting" : "Add to catalog"}
              </Button>

              {fittingCatalog.length > 0 && (
                <ul aria-label="User fitting catalog" style={listStyle}>
                  {fittingCatalog.map((entry) => {
                    const inUse = fittings.some((fitting) => fitting.catalogId === entry.id);
                    return (
                      <li key={entry.id} style={listRowStyle}>
                        <button
                          type="button"
                          className="mtk-btn mtk-btn--ghost"
                          title={`Edit ${entry.label}`}
                          onClick={() => setCatalog({ ...entry, assetHandle: entry.assetHandle ?? "" })}
                          style={{ minWidth: 0, textAlign: "left", color: color.text.primary }}
                        >
                          <strong>{entry.label}</strong><br /><span style={hintStyle}>{FITTING_LABEL[entry.kind]} · {entry.diameterScale}× diameter</span>
                        </button>
                        <Button
                          data-testid={`pipe-forge-remove-catalog-${entry.id}`}
                          variant="ghost"
                          compact
                          disabled={busy || inUse}
                          title={inUse ? "Remove its placed fittings before deleting this catalog entry" : `Remove ${entry.label} from this route`}
                          aria-label={`Remove ${entry.label} from fitting catalog`}
                          onClick={() => void updateStatus("remove-catalog", "Couldn’t remove this catalog fitting", () => client.pipeForgeRemoveCatalog(entry.id))}
                        >
                          Remove
                        </Button>
                      </li>
                    );
                  })}
                </ul>
              )}
            </div>
          </details>

          <Button data-testid="pipe-forge-cancel" variant="ghost" compact disabled={busy} onClick={() => void updateStatus("cancel", "Couldn’t close Pipe Forge", () => client.pipeForgeCancel())} style={{ width: "100%", marginTop: space.sm }}>
            {pending === "cancel" ? "Closing…" : editing ? "Discard route edits" : "Cancel"}
          </Button>
        </div>
      )}

      {lastReport && (
        <BakeReport report={lastReport} />
      )}

      {completed && (
        <div style={{ padding: space.sm }}>
          <div style={{ display: "grid", gridTemplateColumns: editableEntityId ? "1fr 1fr" : "1fr", gap: space.xs }}>
            {editableEntityId && (
              <Button data-testid="pipe-forge-edit" variant="secondary" compact disabled={busy} onClick={() => void editSelected()}>
                {pending === "edit" ? "Opening…" : "Edit selected pipe"}
              </Button>
            )}
            <Button data-testid="pipe-forge-start" variant="secondary" compact disabled={busy} onClick={() => void startDrawing()}>
              {pending === "start" ? "Starting…" : "Draw another pipe"}
            </Button>
          </div>
          <div style={{ ...hintStyle, marginTop: space.xs, textAlign: "center" }}>Model is open below for validation and export.</div>
        </div>
      )}

      {error && <div data-testid="pipe-forge-error" role="alert" style={errorStyle}>{error}</div>}
    </section>
  );
}

function SetupControls({ options, disabled, onChange }: { options: PipeForgeOptions; disabled: boolean; onChange: React.Dispatch<React.SetStateAction<PipeForgeOptions>> }) {
  return (
    <>
      <label style={rowStyle}>
        <span style={labelStyle}>Material kit</span>
        <select data-testid="pipe-forge-kit" aria-label="Material kit" className="mtk-input" value={options.kit} disabled={disabled} onChange={(event) => onChange((current) => ({ ...current, kit: event.target.value as PipeForgeOptions["kit"] }))} style={controlStyle}>
          <option value="galvanized">Galvanized steel</option><option value="copper">Copper</option><option value="pvc">PVC</option><option value="scifi">Sci-fi</option>
        </select>
      </label>
      <label style={rowStyle}>
        <span style={labelStyle}>Diameter</span>
        <span style={{ display: "flex", alignItems: "center", gap: space.xs }}>
          <NumericField data-testid="pipe-forge-diameter" ariaLabel="Diameter in centimetres" value={options.diameterCm} min={1} max={200} step={0.5} disabled={disabled} onCommit={(diameterCm) => onChange((current) => ({ ...current, diameterCm }))} style={{ width: 72 }} />
          <span style={unitStyle}>cm</span>
        </span>
      </label>
      <label style={rowStyle}>
        <span style={labelStyle}>Build quality</span>
        <select data-testid="pipe-forge-quality" aria-label="Build quality" className="mtk-input" value={options.quality} disabled={disabled} onChange={(event) => onChange((current) => ({ ...current, quality: event.target.value as PipeForgeOptions["quality"] }))} style={controlStyle}>
          <option value="preview">Preview</option><option value="production">Production</option><option value="hero">Hero</option>
        </select>
      </label>
      <div style={{ ...rowStyle, marginBottom: space.md }}>
        <span style={labelStyle}>Joint detail</span>
        <Button data-testid="pipe-forge-auto-fittings" variant="toggle" active={options.autoFittings} compact aria-pressed={options.autoFittings} disabled={disabled} onClick={() => onChange((current) => ({ ...current, autoFittings: !current.autoFittings }))} title="Infer semantic elbows, tees and visible collars from route topology">
          Auto fittings {options.autoFittings ? "On" : "Off"}
        </Button>
      </div>
    </>
  );
}

function BakeReport({ report }: { report: PipeBakeReport }) {
  const succeeded = Boolean(report.handle && report.entityId);
  return (
    <div data-testid="pipe-forge-report" role="status" aria-live="polite" style={{ padding: `${space.sm}px ${space.lg}px`, color: succeeded ? color.success.text : color.danger.text, background: succeeded ? color.success.bg : color.danger.bg, borderTop: `1px solid ${succeeded ? color.success.border : color.danger.border}` }}>
      <div style={{ fontWeight: 650, marginBottom: space.xs }}>{succeeded ? "Asset ready" : "Bake needs attention"}</div>
      <div style={{ ...hintStyle, color: "inherit" }}>
        {report.triangles.toLocaleString("en-GB")} triangles · {report.lodTriangles.length} LODs · PBR {report.textureResolution}px<br />
        {report.watertight ? "Watertight" : "Not watertight"} · {report.collisionTriangles > 0 ? `${report.collisionKind.charAt(0).toUpperCase()}${report.collisionKind.slice(1)} collision · ${report.collisionTriangles.toLocaleString("en-GB")} triangles` : "Collision not built"}
      </div>
      {report.warnings.map((warning) => <div key={warning} role="alert" style={{ ...hintStyle, color: "inherit", marginTop: space.xs }}>{warning}</div>)}
    </div>
  );
}

function Stat({ dataTestId, label, value }: { dataTestId: string; label: string; value: string }) {
  return <div data-testid={dataTestId} style={{ minWidth: 0, padding: space.sm, textAlign: "center", background: color.bg.inset }}><div style={{ fontSize: fontSize.micro, color: color.text.muted, marginBottom: space.xxs }}>{label}</div><div style={{ font: font.mono, fontSize: fontSize.meta, color: color.text.primary, overflow: "hidden", textOverflow: "ellipsis" }}>{value}</div></div>;
}

const headerStyle: React.CSSProperties = { position: "sticky", top: 0, zIndex: 1, display: "flex", alignItems: "center", justifyContent: "space-between", gap: space.sm, padding: `${space.sm}px ${space.lg}px`, background: color.bg.raised, borderBottom: `1px solid ${color.border.subtle}` };
const rowStyle: React.CSSProperties = { display: "flex", alignItems: "center", justifyContent: "space-between", gap: space.md, minHeight: 30, marginBottom: space.sm };
const labelStyle: React.CSSProperties = { color: color.text.secondary, fontSize: fontSize.body };
const controlStyle: React.CSSProperties = { width: 144, minWidth: 0 };
const unitStyle: React.CSSProperties = { font: font.mono, fontSize: fontSize.meta, color: color.text.muted };
const hintStyle: React.CSSProperties = { color: color.text.muted, fontSize: fontSize.meta, lineHeight: 1.35 };
const statGridStyle: React.CSSProperties = { display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 1, overflow: "hidden", border: `1px solid ${color.border.subtle}`, borderRadius: radius.lg, background: color.border.subtle };
const detailsStyle: React.CSSProperties = { marginTop: space.sm, border: `1px solid ${color.border.subtle}`, borderRadius: radius.lg, background: color.bg.inset };
const summaryStyle: React.CSSProperties = { padding: `${space.sm}px ${space.md}px`, cursor: "pointer", fontWeight: 650, color: color.text.secondary };
const summaryMetaStyle: React.CSSProperties = { float: "right", font: font.mono, fontSize: fontSize.micro, color: color.text.muted };
const detailsBodyStyle: React.CSSProperties = { padding: `0 ${space.md}px ${space.md}px`, borderTop: `1px solid ${color.border.subtle}` };
const sectionHintStyle: React.CSSProperties = { ...hintStyle, margin: `${space.sm}px 0` };
const stackedLabelStyle: React.CSSProperties = { display: "grid", gap: space.xxs, marginTop: space.xs, minWidth: 0 };
const fieldsetStyle: React.CSSProperties = { padding: space.sm, margin: `${space.sm}px 0 0`, border: `1px solid ${color.border.subtle}`, borderRadius: radius.md };
const legendStyle: React.CSSProperties = { padding: `0 ${space.xs}px`, color: color.text.muted, fontSize: fontSize.meta };
const axisLabelStyle: React.CSSProperties = { display: "grid", gridTemplateColumns: "auto 1fr", alignItems: "center", gap: space.xxs, minWidth: 0, color: color.text.muted, font: font.mono, fontSize: fontSize.micro };
const emptyInlineStyle: React.CSSProperties = { ...hintStyle, marginTop: space.sm, padding: space.sm, borderRadius: radius.md, background: color.bg.raised };
const listStyle: React.CSSProperties = { display: "grid", gap: space.xs, padding: 0, margin: `${space.sm}px 0 0`, listStyle: "none" };
const listRowStyle: React.CSSProperties = { display: "flex", alignItems: "center", justifyContent: "space-between", gap: space.sm, padding: space.xs, borderRadius: radius.md, background: color.bg.raised, fontSize: fontSize.meta };
const errorStyle: React.CSSProperties = { padding: `${space.sm}px ${space.lg}px`, color: color.danger.text, background: color.danger.bg, borderTop: `1px solid ${color.danger.border}` };
