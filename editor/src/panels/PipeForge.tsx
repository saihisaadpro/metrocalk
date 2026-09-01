//! Pipe Forge — direct-in-viewport route drawing with progressively disclosed graph, fitting and catalog tools.
//! React controls the transaction-sized commands; native state remains authoritative across refresh and rebake.
//!
//! ## The one workspace that floats on the stage
//!
//! Every other sub-engine opens in a dock — a track whose width the shell owns. This one is
//! `position: absolute` INSIDE the viewport, beside the tool rail, because the user is clicking route
//! points into the 3D behind it and a panel they have to look away to reach is a panel that breaks the
//! gesture. `App.tsx` supplies the `left`/`width`; the height is a percentage of a box this component
//! does not own, which is the whole reason it is photographed rather than reasoned about.
//!
//! ## Why it draws none of its own controls any more
//!
//! It used to draw all of them: nineteen private `CSSProperties` constants, five raw `<select>`s, three
//! raw `<input>`s, a `<fieldset>`/`<legend>`, three `<details>`/`<summary>` pairs and two hand-built
//! `<ul>` lists — the largest single pocket of invented UI left in the editor, and the constitution's
//! flat prohibition is *"No subsystem is allowed to invent its own styling."* Two things followed from
//! that which no test could see:
//!
//! * **`<details>` made the panel unassertable.** Chrome gives a closed `<details>` body
//!   `content-visibility: hidden`, so every descendant reports a collapsed rect at the summary's
//!   position. The screenshot gate's geometry invariants then produced confident, false sentences about
//!   controls that were not on screen — the same class as the portalled overlays of ADR-149, reached
//!   through a different door. `DisclosureSection` marks its closed region `visibility: hidden`, which
//!   `visible()` skips, so the invariants can finally see this panel.
//! * **A hand-built list row escaped the panel.** The catalog rows were `display: flex;
//!   justify-content: space-between` with a text child that had no `min-width: 0`, so the flex item
//!   refused to shrink: measured at the panel's real 336px, every row lost 53px of its name and 49px of
//!   its Remove button off an `overflow-x: hidden` edge, with no scrollbar and no way to reach either.
//!
//! ## Two components adopted rather than invented
//!
//! `VectorField` and `ListRow` are byte-identical to the ones two concurrent lanes wrote first
//! (`claude/jolly-chatelet-6dfc2d` / ADR-155 and `claude/dazzling-elgamal-3acebf` / ADR-159). This
//! repository has already paid twice for two lanes inventing the same control hours apart; adopting the
//! existing spelling means the branches merge into one component instead of three.

import { useEffect, useMemo, useState } from "react";
import { Callout, Field, FieldGrid, ListRow, Metric, MetricGrid } from "../theme/fields";
import { Icon } from "../theme/icons";
import {
  Badge,
  Button,
  NumericField,
  PanelHeader,
  PropertyRow,
  ScrollArea,
  SelectField,
  TextField,
  VectorField,
} from "../theme/primitives";
import { color, elevation, font, fontSize, radius, space, z } from "../theme/tokens";
import { DisclosureSection, ShortcutBadge } from "../theme/workspace";
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

/** THE ENUM IS NOT THE LABEL. `production` was rendered straight into a badge beside "Galvanized
 *  steel" and "Auto joints" — one of four chips in the row spelled the way the wire spells it, which
 *  is the plain-language rule (`<ux_quality>` 4) failing in the smallest possible way and the easiest
 *  to miss, because the word is real English and merely lower-case. */
const QUALITY_LABEL: Record<PipeForgeOptions["quality"], string> = {
  preview: "Preview",
  production: "Production",
  hero: "Hero",
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

/** What a point on the route is, in one word a reader already knows. */
function pointRole(connectedEdges: number): string {
  if (connectedEdges > 2) return "junction";
  return connectedEdges <= 1 ? "end of a run" : "along the run";
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
  /** The user has typed a position that has not been applied yet, so the 750 ms status poll must not
   *  overwrite it. Cleared by a successful move, and by selecting a different point. */
  const [positionDirty, setPositionDirty] = useState(false);
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

  // A TYPED POSITION SURVIVES THE POLL. `App.tsx` re-reads `pipe_forge_status` every 750 ms, which
  // hands this effect a fresh `handles` array and — before `positionDirty` — unconditionally reset the
  // three boxes to the engine's coordinates. So a number the user typed vanished within 750 ms, and
  // "Apply position" then sent the OLD value: an edit silently discarded and an action that did the
  // opposite of what its own field showed. The resync is right whenever the user has NOT typed, and
  // whenever the SELECTION moves (a different point's position is not a stale edit of this one).
  useEffect(() => {
    if (handles.length === 0) {
      setSelectedHandleId(null);
      setPositionDirty(false);
      return;
    }
    const next = handles.find((handle) => handle.nodeId === selectedHandleId) ?? handles[0];
    const reselected = next.nodeId !== selectedHandleId;
    setSelectedHandleId(next.nodeId);
    if (reselected || !positionDirty) {
      setHandlePosition([...next.position]);
      if (reselected) setPositionDirty(false);
    }
  }, [handles, positionDirty, selectedHandleId]);

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
      // NOT ALSO `setError(report.message)`. A refused bake was announced twice from one reply — a
      // polite `role="status"` report and an assertive `role="alert"` strip, both saying the same
      // sentence in two different boxes. `BakeReport` renders `report.message` when it failed.
      onBaked(report);
    } catch (cause) {
      setError(errorMessage(cause, editing ? "Couldn’t rebake this route" : "Couldn’t bake this asset"));
    } finally {
      setPending(null);
    }
  }

  async function saveCatalogEntry() {
    const label = catalog.label.trim();
    const id = catalogTargetId;
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
  // ONE SENTENCE, COMPUTED ONCE, USED BY EVERY REFUSAL IN THE SECTION. Six controls here are disabled
  // by "no point is selected" or "something else is running", and before this each of them either
  // repeated the sentence or — measured by R9 on the first capture this panel ever had — said nothing
  // at all. `disabledReason` does not imply `disabled`, so the reason is computed beside the boolean
  // that consumed it rather than inside the control that displays it.
  const busyReason = busy ? "One moment — the last action is still running" : undefined;
  const needsPoint = busyReason ?? (selectedHandle ? undefined : "Pick a point on the route first");
  // The saved-fitting the engine would actually write, which is NOT always the one the user typed:
  // `saveCatalogEntry` falls back to `catalogIdFrom(label)`. Computing it ONCE, here, is what makes the
  // button's own label honest and the collision visible — see the `Callout` beside the save action.
  const catalogTargetId = catalog.id.trim() || catalogIdFrom(catalog.label);
  const catalogCollision = fittingCatalog.find((entry) => entry.id === catalogTargetId) ?? null;

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
        // HEADER · SCROLLING BODY · PINNED FOOTER, instead of one long document that scrolls whole.
        // Eleven actions in this panel funnel their failure into one strip at the very end of it, and
        // with both sections open that strip sits ~600px below the button that caused it, inside a
        // scroller that does not scroll itself. A footer that cannot leave the viewport is the
        // `<ux_quality>` 2 rule — feedback at the gesture, not in a gutter — expressed as a layout.
        // NO `overflowX: hidden` any more either: that declaration is what turned a row too wide for
        // the panel into content with no scrollbar and no way to reach it. The rows now WRAP
        // (`.mtk-list-row`), so nothing needs clipping.
        overflow: "hidden",
        display: "flex",
        flexDirection: "column",
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
      <PanelHeader
        style={{
          flex: "none",
          background: color.bg.raised,
          borderTopLeftRadius: radius.xl,
          borderTopRightRadius: radius.xl,
        }}
        title={
          <span data-testid="pipe-forge-title" style={{ display: "inline-flex", alignItems: "center", gap: space.sm }}>
            <Icon name="pipe" size="lg" style={{ color: color.accent.base }} />
            Pipe Forge
          </span>
        }
        right={<Badge tone={drawing ? "success" : "neutral"}>{editing ? "Editing" : drawing ? "Drawing" : "Ready"}</Badge>}
      />

      <ScrollArea style={{ flex: "1 1 auto", overscrollBehavior: "contain" }}>
      {!drawing && !completed && (
        <div data-testid="pipe-forge-setup" style={{ display: "flex", flexDirection: "column", gap: space.md, padding: space.lg, minWidth: 0 }}>
          {editableEntityId && (
            <Callout tone="info" title="This pipe can be reopened">
              Its points, branches and fittings all come back, and the asset it already made stays where it
              is until you rebuild it.
              <Button
                data-testid="pipe-forge-edit"
                variant="secondary"
                compact
                disabled={busy}
                disabledReason={busyReason}
                onClick={() => void editSelected()}
                style={{ width: "100%", marginTop: space.sm }}
              >
                {pending === "edit" ? "Opening route…" : "Edit selected pipe"}
              </Button>
            </Callout>
          )}

          <SetupControls options={options} disabled={busy} onChange={setOptions} />
          <Button
            data-testid="pipe-forge-start"
            variant="primary"
            disabled={busy || options.diameterCm <= 0}
            disabledReason={busyReason ?? (options.diameterCm <= 0 ? "Give the pipe a diameter above zero first" : undefined)}
            onClick={() => void startDrawing()}
            style={{ width: "100%" }}
          >
            {pending === "start" ? "Starting…" : "Draw new pipe"}
          </Button>
          <p style={hint}>Then click in the viewport to place each point.</p>
          {status?.message && (
            <p data-testid="pipe-forge-message" style={hint}>
              {status.message}
            </p>
          )}
        </div>
      )}

      {drawing && status && (
        // A FLEX COLUMN, NOT A GRID, AND `.mtk-disclosure` IS THE WHOLE REASON. It carries
        // `flex: none` so a section can never be shrunk below its own header — and `flex: none`
        // is inert in a grid, where the three sections were instead squeezed and clipped their
        // own contents. Measured by the gate the moment a section was opened: the position
        // editor cut 144px, "Apply position" 174px, the catalog's last row 542px, all by
        // `.mtk-disclosure__content`'s `overflow: hidden`, with nothing on screen to say so.
        <div data-testid="pipe-forge-active" style={{ display: "flex", flexDirection: "column", gap: space.md, padding: space.lg, minWidth: 0 }}>
          <div style={{ display: "flex", alignItems: "center", gap: space.xs, flexWrap: "wrap" }}>
            <Badge tone="accent">{KIT_LABEL[activeOptions.kit]}</Badge>
            <Badge>{activeOptions.diameterCm} cm</Badge>
            <Badge>{QUALITY_LABEL[activeOptions.quality]}</Badge>
            <Badge>{activeOptions.autoFittings ? "Auto joints" : "Manual fittings"}</Badge>
          </div>
          {/* THESE FOUR ARE READ-ONLY NOW, AND SAYING SO IS THE WHOLE POINT. `SetupControls` is mounted
              only while NOT drawing, so the first viewport click replaces four editable rows with four
              badges that look like the same information — an honest-state failure (`<ux_quality>` 6):
              a user who wants a different diameter sees the number, reaches for it, and finds nothing.
              One sentence, next to the thing it is about. */}
          <p style={hint}>Set when this route started. Cancel and draw again to change them.</p>

          {/* THE MEASUREMENTS THE REST OF THE EDITOR DRAWS. Three numbers in three hairline cells of a
              bordered grid was this panel's own invention; `Metric` is deliberately borderless for the
              reason its docstring gives — six numbers in six boxes is six rectangles competing with the
              numbers inside them — and its value is mono and tabular so the column stays aligned while
              it changes, which is exactly what a live route does. */}
          <MetricGrid minColumn={84} data-testid="pipe-forge-stats">
            <Metric data-testid="pipe-forge-points" label="Points" value={routeNodes} description="Points placed on this route so far." />
            <Metric data-testid="pipe-forge-length" label="Length" value={status.lengthM.toFixed(2)} unit="m" description="Total centreline length of the run." />
            <Metric
              data-testid="pipe-forge-triangles"
              label="Triangles"
              value={status.previewTriangles.toLocaleString("en-GB")}
              description="Size of the preview mesh. The baked asset is built again at the chosen quality."
            />
          </MetricGrid>

          {status.branchFrom != null && (
            <Callout tone="info" role="status" data-testid="pipe-forge-branch-mode" title="Drawing a branch">
              Click in the viewport to run a branch off point {status.branchFrom}. Finish it to go back to the
              main route.
              <Button
                data-testid="pipe-forge-end-branch"
                variant="secondary"
                compact
                disabled={busy}
                disabledReason={busyReason}
                onClick={() => void updateStatus("end-branch", "Couldn’t finish this branch", () => client.pipeForgeEndBranch())}
                style={{ marginTop: space.sm }}
              >
                {pending === "end-branch" ? "Finishing…" : "Finish branch"}
              </Button>
            </Callout>
          )}

          {/* The live region and the shortcuts are ONE block, because they answer one question — "what
              do I do now?" — and a `minHeight` reserving a line for a sentence that is always present
              is a box that looks broken on the frame where it is empty. */}
          {/* THE INSTRUCTION IS NOT IN THE LIVE REGION, AND THAT IS THE FIX. An `aria-live` region
              announces CHANGES to its contents; text already present when it mounts is never spoken.
              So "Click in the viewport to place the first point" — the one sentence that tells a
              screen-reader user what the panel wants from them — was the one sentence it could not
              deliver. It is ordinary prose now, read in document order, and the live region carries
              only what the engine actually says as it happens. */}
          <div style={{ display: "grid", gap: space.xs }}>
            {/* ONLY WHILE THE ENGINE HAS NOT SPOKEN. Splitting the instruction out of the live region
                is right, but rendering both is not: the engine's own message for a route in progress
                is *"Click again to extend the run."* — the same sentence — so the panel printed it
                twice, one line under the other. Caught in the capture, invisible to every claim,
                because two identical sentences satisfy `text_present` exactly as well as one. */}
            {!status.message && (
              <p style={hint}>
                {routeNodes === 0 ? "Click in the viewport to place the first point." : "Click again to extend the run."}
              </p>
            )}
            <p data-testid="pipe-forge-message" aria-live="polite" style={hint}>
              {status.message}
            </p>
            <span style={{ ...hint, display: "flex", alignItems: "center", gap: space.xs, flexWrap: "wrap" }}>
              <ShortcutBadge keys={["Ctrl", "Z"]} ariaLabel="Control plus Z" /> undo a point
              <ShortcutBadge keys="Esc" /> cancel
            </span>
          </div>

          <div style={{ display: "grid", gridTemplateColumns: "1fr 1.2fr", gap: space.xs }}>
            <Button
              data-testid="pipe-forge-undo"
              variant="secondary"
              compact
              disabled={busy || routeNodes === 0}
              disabledReason={busyReason ?? (routeNodes === 0 ? "There are no points to undo yet" : undefined)}
              onClick={() => void updateStatus("undo", "Couldn’t undo that point", () => client.pipeForgeUndo())}
            >
              {pending === "undo" ? "Undoing…" : "Undo point"}
            </Button>
            <Button
              data-testid="pipe-forge-bake"
              variant="primary"
              compact
              disabled={!canBake}
              disabledReason={
                canBake
                  ? undefined
                  : (busyReason ??
                    (routeNodes < 2
                      ? "Place at least two points before building the asset"
                      : "The route has a problem to fix first — the message above says which"))
              }
              title={canBake ? (editing ? "Rebuild the selected asset from its editable route" : "Create a game-ready mesh asset") : undefined}
              onClick={() => void bakeAsset()}
            >
              {pending === "bake" ? "Building…" : editing ? "Rebake asset" : "Finish & bake"}
            </Button>
          </div>

          <DisclosureSection
            data-testid="pipe-forge-network"
            tone="card"
            defaultOpen={false}
            storageKey="pipe-forge-network"
            title="Route network"
            summary={`${handles.length} points · ${edges.length} runs`}
          >
            <p style={hint}>Pick a point to branch from it, or to type its exact position.</p>
            <Field label="Point on the route" htmlFor="pipe-forge-handle-select">
              <SelectField
                id="pipe-forge-handle-select"
                data-testid="pipe-forge-handle"
                value={selectedHandleId ?? ""}
                disabled={busy || handles.length === 0}
                title={handles.length === 0 ? "Place a point in the viewport first" : undefined}
                onChange={(event) => setSelectedHandleId(Number(event.target.value))}
              >
                {handles.length === 0 && <option value="">Place a point first</option>}
                {handles.map((handle) => (
                  <option key={handle.nodeId} value={handle.nodeId}>
                    Point {handle.nodeId} · {pointRole(handle.connectedEdges.length)}
                  </option>
                ))}
              </SelectField>
            </Field>

            {/* THREE NUMBERS THAT ARE READ, EDITED AND UNDONE TOGETHER ARE ONE PROPERTY. They were a
                `<fieldset>` with a `<legend>` and three 11px monospace letters in their own columns —
                the OS form widgets the Property Controls rule exists to remove, and a layout whose
                letter column made the three boxes start at three different x positions. */}
            <Field label="Position" unit="m" htmlFor="pipe-forge-handle-position-x" disabled={busy || !selectedHandle}>
              <VectorField
                data-testid="pipe-forge-handle-position"
                idFor={(field) => `pipe-forge-handle-position-${field}`}
                disabled={busy || !selectedHandle}
                step={0.1}
                axes={(["x", "y", "z"] as const).map((field, index) => ({
                  field,
                  tag: field.toUpperCase(),
                  label: `Point position ${field.toUpperCase()} in metres`,
                  value: handlePosition[index],
                  min: -100_000,
                  max: 100_000,
                }))}
                onCommit={(field, value) => {
                  const index = { x: 0, y: 1, z: 2 }[field as "x" | "y" | "z"];
                  setPositionDirty(true);
                  setHandlePosition((current) => current.map((item, i) => (i === index ? value : item)) as [number, number, number]);
                }}
              />
            </Field>
            <Button
              data-testid="pipe-forge-move-handle"
              variant="secondary"
              compact
              disabled={busy || !selectedHandle}
              disabledReason={needsPoint}
              title={busy || !selectedHandle ? undefined : "Move this point and rebuild the run through it"}
              onClick={() => {
                if (!selectedHandle) return;
                void updateStatus("move-handle", "Couldn’t move this point", () =>
                  client.pipeForgeMoveHandle(selectedHandle.nodeId, ...handlePosition),
                ).then((moved) => {
                  if (moved) setPositionDirty(false);
                });
              }}
              style={{ width: "100%" }}
            >
              {pending === "move-handle" ? "Moving…" : "Apply position"}
            </Button>
            {positionDirty && (
              <Callout tone="info" data-testid="pipe-forge-position-dirty">
                Typed position isn’t applied yet — press Apply position to move the point.
              </Callout>
            )}

            {/* The action belongs to the value beside it, so it goes in the row's OWN actions column
                rather than in a second grid the panel invents to sit them side by side. */}
            {/* A `Field`, NOT A `PropertyRow` WITH AN ACTION, and 336px is the reason. The row's label
                column is `minmax(84px, 0.72fr)`; with the control, the unit and an action competing
                for the rest, "Branch diameter" wrapped onto two lines and the button was squeezed
                beside it. Label above the control is the section's own rhythm — the point picker and
                the position editor are both `Field`s — and it leaves the action a full-width line of
                its own, matching Apply position and Remove this point. */}
            <Field label="Branch diameter" unit="cm" htmlFor="pipe-forge-branch-diameter" disabled={busy || !selectedHandle}>
              <NumericField
                id="pipe-forge-branch-diameter"
                data-testid="pipe-forge-branch-diameter"
                ariaLabel="Branch diameter in centimetres"
                value={branchDiameterCm}
                min={1}
                max={200}
                step={0.5}
                disabled={busy || !selectedHandle || status.branchFrom != null}
                onCommit={setBranchDiameterCm}
                style={{ width: "100%" }}
              />
            </Field>
            <Button
              data-testid="pipe-forge-begin-branch"
              variant="secondary"
              compact
              disabled={busy || !selectedHandle || status.branchFrom != null || branchDiameterCm <= 0}
              disabledReason={
                needsPoint ??
                (status.branchFrom != null
                  ? "Finish the branch you are drawing first"
                  : branchDiameterCm <= 0
                    ? "Give the branch a diameter above zero"
                    : undefined)
              }
              title={busy || !selectedHandle || status.branchFrom != null || branchDiameterCm <= 0 ? undefined : "The next viewport click starts a branch at this point"}
              onClick={() => selectedHandle && void updateStatus("branch", "Couldn’t start a branch here", () => client.pipeForgeBeginBranch(selectedHandle.nodeId, branchDiameterCm))}
              style={{ width: "100%" }}
            >
              {pending === "branch" ? "Starting…" : "Draw branch"}
            </Button>

            <Button
              data-testid="pipe-forge-remove-handle"
              variant="danger"
              compact
              disabled={busy || !leafRemovable}
              disabledReason={
                leafRemovable
                  ? undefined
                  : (needsPoint ?? "Only a point at the end of a run can be removed, and a route needs at least two")
              }
              onClick={() => selectedHandle && void updateStatus("remove-handle", "Couldn’t remove this point", () => client.pipeForgeRemoveHandle(selectedHandle.nodeId))}
              style={{ width: "100%" }}
            >
              {pending === "remove-handle" ? "Removing…" : "Remove this point"}
            </Button>
          </DisclosureSection>

          <DisclosureSection
            data-testid="pipe-forge-fittings"
            tone="card"
            defaultOpen={false}
            storageKey="pipe-forge-fittings"
            title="Fittings"
            summary={`${fittings.length} placed`}
          >
            <p style={hint}>
              Add a tee, valve or flange at a point on the route. The built-in ones are drawn by the engine,
              so they always appear.
            </p>
            <FieldGrid minColumn={120}>
              <Field label="Kind" htmlFor="pipe-forge-fitting-kind">
                <SelectField
                  id="pipe-forge-fitting-kind"
                  data-testid="pipe-forge-fitting-kind"
                  value={fittingKind}
                  disabled={busy}
                  onChange={(event) => setFittingKind(event.target.value as PipeFittingKind)}
                >
                  {(Object.keys(FITTING_LABEL) as PipeFittingKind[]).map((kind) => (
                    <option key={kind} value={kind}>{FITTING_LABEL[kind]}</option>
                  ))}
                </SelectField>
              </Field>
              <Field label="Style" htmlFor="pipe-forge-fitting-catalog">
                <SelectField
                  id="pipe-forge-fitting-catalog"
                  data-testid="pipe-forge-fitting-catalog"
                  value={fittingCatalogId}
                  disabled={busy}
                  onChange={(event) => setFittingCatalogId(event.target.value)}
                >
                  <option value="">Built-in {FITTING_LABEL[fittingKind].toLowerCase()}</option>
                  {matchingCatalog.map((entry) => <option key={entry.id} value={entry.id}>{entry.label}</option>)}
                </SelectField>
              </Field>
            </FieldGrid>
            <Button
              data-testid="pipe-forge-place-fitting"
              variant="secondary"
              compact
              disabled={busy || !selectedHandle}
              disabledReason={needsPoint}
              title={selectedHandle ? `Place at point ${selectedHandle.nodeId}` : undefined}
              onClick={() => selectedHandle && void updateStatus("place-fitting", "Couldn’t place this fitting", () => client.pipeForgePlaceFitting(selectedHandle.nodeId, fittingKind, fittingCatalogId || undefined))}
              style={{ width: "100%" }}
            >
              {pending === "place-fitting" ? "Placing…" : `Place ${FITTING_LABEL[fittingKind].toLowerCase()}`}
            </Button>

            {fittings.length === 0 ? (
              <Callout tone="neutral">
                Nothing placed by hand. The engine still adds elbows and tees where the route turns or splits.
              </Callout>
            ) : (
              <div role="list" aria-label="Placed fittings" style={{ display: "grid", gap: space.xxs }}>
                {fittings.map((fitting) => (
                  <ListRow key={fitting.id} role="listitem">
                    <span style={{ flex: "1 1 120px", minWidth: 0 }}>
                      <strong>{FITTING_LABEL[fitting.kind]}</strong> · point {fitting.nodeId}
                      {fitting.automatic ? " · added by the engine" : ""}
                    </span>
                    <Button
                      data-testid={`pipe-forge-remove-fitting-${fitting.id}`}
                      variant="ghost"
                      compact
                      disabled={busy || fitting.automatic}
                      disabledReason={
                        fitting.automatic
                          ? "The engine adds this one to follow the shape of the route — change the route to change it"
                          : busyReason
                      }
                      aria-label={`Remove ${FITTING_LABEL[fitting.kind]} from point ${fitting.nodeId}`}
                      onClick={() => void updateStatus("remove-fitting", "Couldn’t remove this fitting", () => client.pipeForgeRemoveFitting(fitting.id))}
                    >
                      Remove
                    </Button>
                  </ListRow>
                ))}
              </div>
            )}
          </DisclosureSection>

          <DisclosureSection
            data-testid="pipe-forge-catalog"
            tone="card"
            defaultOpen={false}
            storageKey="pipe-forge-catalog"
            title="Your fittings"
            summary={`${fittingCatalog.length} saved`}
          >
            <p style={hint}>
              Save a fitting of your own so you can place it again. If its model isn’t available, the engine
              draws a stand-in the same size, so the route always builds.
            </p>
            <FieldGrid minColumn={120}>
              <Field label="Name" htmlFor="pipe-forge-catalog-label" span="full">
                <TextField
                  id="pipe-forge-catalog-label"
                  data-testid="pipe-forge-catalog-label"
                  value={catalog.label}
                  disabled={busy}
                  placeholder="e.g. Isolation valve"
                  onChange={(event) => setCatalog((current) => ({ ...current, label: event.target.value }))}
                />
              </Field>
              <Field label="Kind" htmlFor="pipe-forge-catalog-kind">
                <SelectField
                  id="pipe-forge-catalog-kind"
                  data-testid="pipe-forge-catalog-kind"
                  value={catalog.kind}
                  disabled={busy}
                  onChange={(event) => setCatalog((current) => ({ ...current, kind: event.target.value as PipeFittingKind }))}
                >
                  {(Object.keys(FITTING_LABEL) as PipeFittingKind[]).map((kind) => (
                    <option key={kind} value={kind}>{FITTING_LABEL[kind]}</option>
                  ))}
                </SelectField>
              </Field>
              <Field label="Reference" htmlFor="pipe-forge-catalog-id" help="How saved projects refer to it. Left empty, it is made from the name.">
                <TextField
                  id="pipe-forge-catalog-id"
                  data-testid="pipe-forge-catalog-id"
                  aria-describedby="pipe-forge-catalog-id-help"
                  value={catalog.id}
                  disabled={busy}
                  placeholder="Made from the name"
                  onChange={(event) => setCatalog((current) => ({ ...current, id: event.target.value }))}
                />
              </Field>
              <Field label="Model file" htmlFor="pipe-forge-catalog-asset" span="full" help="Optional. Paste an asset reference, or leave it empty to use a generated stand-in.">
                <TextField
                  id="pipe-forge-catalog-asset"
                  data-testid="pipe-forge-catalog-asset"
                  aria-describedby="pipe-forge-catalog-asset-help"
                  value={catalog.assetHandle}
                  disabled={busy}
                  placeholder="mtkasset:…"
                  onChange={(event) => setCatalog((current) => ({ ...current, assetHandle: event.target.value }))}
                />
              </Field>
              <Field label="Width" unit="×" htmlFor="pipe-forge-catalog-diameter-scale">
                <NumericField
                  id="pipe-forge-catalog-diameter-scale"
                  data-testid="pipe-forge-catalog-diameter-scale"
                  ariaLabel="Fitting width, as a multiple of the pipe diameter"
                  value={catalog.diameterScale}
                  min={0.1}
                  max={10}
                  step={0.05}
                  disabled={busy}
                  onCommit={(diameterScale) => setCatalog((current) => ({ ...current, diameterScale }))}
                  style={{ width: "100%" }}
                />
              </Field>
              <Field label="Length" unit="×" htmlFor="pipe-forge-catalog-length-scale">
                <NumericField
                  id="pipe-forge-catalog-length-scale"
                  data-testid="pipe-forge-catalog-length-scale"
                  ariaLabel="Fitting length, as a multiple of the pipe diameter"
                  value={catalog.lengthScale}
                  min={0.1}
                  max={10}
                  step={0.05}
                  disabled={busy}
                  onCommit={(lengthScale) => setCatalog((current) => ({ ...current, lengthScale }))}
                  style={{ width: "100%" }}
                />
              </Field>
            </FieldGrid>
            <Button
              data-testid="pipe-forge-save-catalog"
              variant="secondary"
              compact
              disabled={busy || !catalog.label.trim()}
              disabledReason={busyReason ?? (catalog.label.trim() ? undefined : "Give the fitting a name first")}
              onClick={() => void saveCatalogEntry()}
              style={{ width: "100%" }}
            >
              {pending === "save-catalog" ? "Saving…" : catalogCollision ? "Update this fitting" : "Save fitting"}
            </Button>
            {/* THE BUTTON USED TO READ THE WRONG ID. It said "Save fitting" whenever the Reference box
                was empty — but an empty box means the id is derived FROM THE NAME, so re-typing a name
                that already exists silently overwrote that entry under a label promising a new one.
                One computed id, read by the save, the label and this warning. */}
            {catalogCollision && (
              <Callout tone="warn" data-testid="pipe-forge-catalog-collision">
                “{catalogCollision.label}” already uses the reference <code>{catalogTargetId}</code>. Saving
                replaces it.
              </Callout>
            )}

            {fittingCatalog.length > 0 && (
              <div role="list" aria-label="User fitting catalog" style={{ display: "grid", gap: space.xxs }}>
                {fittingCatalog.map((entry) => {
                  const inUse = fittings.some((fitting) => fitting.catalogId === entry.id);
                  return (
                    <ListRow key={entry.id} role="listitem">
                      <Button
                        variant="ghost"
                        compact
                        disabled={busy}
                        disabledReason={busyReason}
                        aria-label={`Load ${entry.label} into the form above`}
                        title={busy ? undefined : `Load “${entry.label}” into the form above, replacing what is there`}
                        onClick={() => setCatalog({ ...entry, assetHandle: entry.assetHandle ?? "" })}
                        style={{ flex: "1 1 120px", minWidth: 0, justifyContent: "flex-start", textAlign: "left", height: "auto", whiteSpace: "normal" }}
                      >
                        <span style={{ display: "grid", gap: space.xxs, minWidth: 0 }}>
                          <span>{entry.label}</span>
                          <span style={{ ...hint, fontWeight: 400 }}>{FITTING_LABEL[entry.kind]} · {entry.diameterScale}× width</span>
                        </span>
                      </Button>
                      <Button
                        data-testid={`pipe-forge-remove-catalog-${entry.id}`}
                        variant="ghost"
                        compact
                        disabled={busy || inUse}
                        disabledReason={inUse ? "It is placed on this route — remove those fittings first" : busyReason}
                        aria-label={`Remove ${entry.label} from your saved fittings`}
                        onClick={() => void updateStatus("remove-catalog", "Couldn’t remove this catalog fitting", () => client.pipeForgeRemoveCatalog(entry.id))}
                      >
                        Remove
                      </Button>
                    </ListRow>
                  );
                })}
              </div>
            )}
          </DisclosureSection>

          <Button
            data-testid="pipe-forge-cancel"
            variant="ghost"
            compact
            disabled={busy}
            disabledReason={busyReason}
            onClick={() => void updateStatus("cancel", "Couldn’t close Pipe Forge", () => client.pipeForgeCancel())}
            style={{ width: "100%" }}
          >
            {pending === "cancel" ? "Closing…" : editing ? "Discard route edits" : "Cancel"}
          </Button>
        </div>
      )}

      {completed && (
        <div style={{ display: "grid", gap: space.xs, padding: space.lg }}>
          <div style={{ display: "grid", gridTemplateColumns: editableEntityId ? "1fr 1fr" : "1fr", gap: space.xs }}>
            {editableEntityId && (
              <Button data-testid="pipe-forge-edit" variant="secondary" compact disabled={busy} disabledReason={busyReason} onClick={() => void editSelected()}>
                {pending === "edit" ? "Opening…" : "Edit selected pipe"}
              </Button>
            )}
            <Button data-testid="pipe-forge-start" variant="secondary" compact disabled={busy} disabledReason={busyReason} onClick={() => void startDrawing()}>
              {pending === "start" ? "Starting…" : "Draw another pipe"}
            </Button>
          </div>
          <p style={{ ...hint, textAlign: "center" }}>Model is open below for validation and export.</p>
        </div>
      )}
      </ScrollArea>

      {/* THE FOOTER, AND IT DOES NOT SCROLL. What the last action produced and why the last action
          failed are the two things the user is looking for the instant they press something, so they
          are the two things that stay on screen. */}
      {(lastReport || error) && (
        <div style={{ flex: "none", display: "grid", gap: space.sm, padding: `${space.sm}px ${space.lg}px ${space.lg}px`, borderTop: `1px solid ${color.border.subtle}` }}>
          {lastReport && <BakeReport report={lastReport} />}
          {error && (
            <Callout tone="danger" role="alert" data-testid="pipe-forge-error">
              {error}
            </Callout>
          )}
        </div>
      )}
    </section>
  );
}

function SetupControls({ options, disabled, onChange }: { options: PipeForgeOptions; disabled: boolean; onChange: React.Dispatch<React.SetStateAction<PipeForgeOptions>> }) {
  return (
    // `.mtk-property-sheet`, NOT a bare grid, and the capture is why. A `.mtk-property-row` outside a
    // sheet computes its OWN tracks, and the label column is `minmax(84px, 0.72fr)` — content-
    // influenced — so "Material kit", "Diameter" and "Build quality" each sized their label column
    // differently and the four control cells started and ended at three different x positions. The
    // sheet declares the tracks once and the rows take them with `grid-template-columns: subgrid`,
    // which is the only thing that makes one column out of four rows.
    <div className="mtk-property-sheet" style={{ minWidth: 0 }}>
      {/* `PropertyRow`, NOT four hand-built flex rows with a 144px control. The three controls had
          three different widths — a 144px select, a 72px number and a toggle sized by its own label —
          so the sheet's right edge went ragged and the unit hung outside the last control's box. The
          shared row puts the unit in a COLUMN of its own for exactly that reason (ADR-136). */}
      <PropertyRow label="Material kit" htmlFor="pipe-forge-kit">
        <SelectField
          id="pipe-forge-kit"
          data-testid="pipe-forge-kit"
          value={options.kit}
          disabled={disabled}
          onChange={(event) => onChange((current) => ({ ...current, kit: event.target.value as PipeForgeOptions["kit"] }))}
        >
          {(Object.keys(KIT_LABEL) as PipeForgeOptions["kit"][]).map((kit) => (
            <option key={kit} value={kit}>{KIT_LABEL[kit]}</option>
          ))}
        </SelectField>
      </PropertyRow>
      <PropertyRow label="Diameter" unit="cm" htmlFor="pipe-forge-diameter">
        <NumericField
          id="pipe-forge-diameter"
          data-testid="pipe-forge-diameter"
          ariaLabel="Diameter in centimetres"
          value={options.diameterCm}
          min={1}
          max={200}
          step={0.5}
          disabled={disabled}
          onCommit={(diameterCm) => onChange((current) => ({ ...current, diameterCm }))}
          // MANDATORY, NOT COSMETIC. `NumericField` writes `style={{ width: 80, ...style }}` inline,
          // and an inline width beats `.mtk-property-row__control`. Without this the box stays 80px
          // and the ragged right edge the shared row exists to fix survives the migration.
          style={{ width: "100%" }}
        />
      </PropertyRow>
      <PropertyRow label="Build quality" htmlFor="pipe-forge-quality">
        <SelectField
          id="pipe-forge-quality"
          data-testid="pipe-forge-quality"
          value={options.quality}
          disabled={disabled}
          onChange={(event) => onChange((current) => ({ ...current, quality: event.target.value as PipeForgeOptions["quality"] }))}
        >
          {(Object.keys(QUALITY_LABEL) as PipeForgeOptions["quality"][]).map((quality) => (
            <option key={quality} value={quality}>{QUALITY_LABEL[quality]}</option>
          ))}
        </SelectField>
      </PropertyRow>
      {/* A TOGGLE, AND IT STAYS ONE. `aria-pressed` is asserted by both the Vitest suite and the
          packaged-`.exe` e2e, so this is the shared `Button variant="toggle"` rather than the shared
          `Checkbox` — the same component family, the semantics the contract names. */}
      <PropertyRow label="Joints" htmlFor="pipe-forge-auto-fittings">
        <Button
          id="pipe-forge-auto-fittings"
          data-testid="pipe-forge-auto-fittings"
          variant="toggle"
          active={options.autoFittings}
          compact
          aria-pressed={options.autoFittings}
          disabled={disabled}
          onClick={() => onChange((current) => ({ ...current, autoFittings: !current.autoFittings }))}
          title="Add elbows, tees and collars where the route turns or splits"
          style={{ width: "100%" }}
        >
          {options.autoFittings ? "Added automatically" : "Placed by hand"}
        </Button>
      </PropertyRow>
    </div>
  );
}

/** WHAT THE ACTION BOUGHT, WHERE THE ACTION WAS (`<ux_quality>` 1 and 3). The numbers are the shared
 *  `Metric`, so the one place in the editor that reports a build reports it the way every other
 *  measurement in the editor is reported. */
function BakeReport({ report }: { report: PipeBakeReport }) {
  const succeeded = Boolean(report.handle && report.entityId);
  return (
    <Callout
        data-testid="pipe-forge-report"
        role="status"
        tone={succeeded ? "success" : "danger"}
        title={succeeded ? "Asset ready" : "Bake needs attention"}
      >
        <div style={{ display: "grid", gap: space.sm, marginTop: space.xs }}>
          <MetricGrid minColumn={88}>
            {/* EACH TILE IS ADDRESSABLE, so a test asserts the number it means. The suite used to read
                `"8,000 triangles"` off the whole report and was satisfied by the COLLISION sentence
                further down, which happens to contain the same substring — a green that would have
                survived deleting the triangle count entirely. */}
            <Metric data-testid="pipe-forge-report-triangles" label="Triangles" value={report.triangles.toLocaleString("en-GB")} description="Triangles in the built mesh." />
            <Metric data-testid="pipe-forge-report-lods" label="Detail levels" value={report.lodTriangles.length} description="Simpler copies drawn when the pipe is far away." />
            <Metric data-testid="pipe-forge-report-texture" label="Texture" value={`${report.textureResolution}px`} description="Resolution of the generated PBR material maps." />
          </MetricGrid>
          <span>
            {report.watertight ? "Watertight" : "Not watertight"} ·{" "}
            {report.collisionTriangles > 0
              ? `${report.collisionKind.charAt(0).toUpperCase()}${report.collisionKind.slice(1)} collision · ${report.collisionTriangles.toLocaleString("en-GB")} triangles`
              : "Collision not built"}
          </span>
          {/* ONE ANNOUNCEMENT PER OUTCOME. A failed bake used to be spoken twice — once by this
              `role="status"` and once by a `role="alert"` error strip fed from the same reply — and
              each warning inside it interrupted the status it was part of. The report is the single
              voice; `bakeAsset` no longer also writes the reply into `error`. */}
          {!succeeded && report.message && <span>{report.message}</span>}
          {report.warnings.map((warning) => (
            <span key={warning}>{warning}</span>
          ))}
        </div>
    </Callout>
  );
}

/** The panel's ONE remaining local style, and it is text rather than a control: a muted line at the
 *  meta size, composed entirely from tokens. Everything that was a box, a border, a row or a control
 *  is now a shared component — the nineteen `CSSProperties` constants that used to live here are gone. */
const hint: React.CSSProperties = { margin: 0, color: color.text.muted, fontSize: fontSize.meta, lineHeight: 1.35 };
