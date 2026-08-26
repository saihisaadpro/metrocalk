//! Asset Lab -- one source-preserving workflow for inspecting, repairing, optimizing, finishing, validating,
//! and exporting a selected 3D asset. The panel is intentionally transport-agnostic: its discriminated
//! action callback can be wired to native commands without importing the editor session or protocol.

import { useEffect, useId, useMemo, useState, type ReactNode } from "react";
import { Icon } from "../theme/icons";
import { Badge, Button, NumericField } from "../theme/primitives";
import { DisclosureSection, DockTabs, EmptyPanelState, WorkspacePanel } from "../theme/workspace";
import "./AssetLabPanel.css";

export type AssetLabStage = "inspect" | "repair" | "optimize" | "uv" | "bake" | "validate" | "export";
export type AssetLabAvailabilityState = "available" | "unavailable" | "unsupported";
export type AssetLabIssueSeverity = "critical" | "error" | "warning" | "info" | "pass";
export type AssetLabIssueStatus = "open" | "fixed" | "accepted" | "blocked";

export interface AssetLabSelection {
  id: string;
  name: string;
  /** Plain-language source or kind, for example "Imported GLB" or "Pipe Forge derivative". */
  source?: string;
  revision?: string;
}

export interface AssetLabMetric {
  id: string;
  label: string;
  value: number | string;
  unit?: string;
  /** Concise measurement definition shown on hover. */
  description?: string;
}

export interface AssetLabIssue {
  id: string;
  severity: AssetLabIssueSeverity;
  status?: AssetLabIssueStatus;
  title: string;
  detail: string;
  count?: number;
}

export interface AssetLabReportView {
  summary: string;
  sourceMetrics: readonly AssetLabMetric[];
  /** Present only after a derivative has actually been produced and measured. */
  resultMetrics?: readonly AssetLabMetric[];
  issues: readonly AssetLabIssue[];
  warnings?: readonly string[];
  /** Texture-bake-specific coverage, projection, quality-gate and alignment evidence. */
  bakeEvidence?: AssetLabBakeEvidenceView;
}

export interface AssetLabBakeEvidenceView {
  resolution: number;
  requestedMaps: readonly string[];
  sourceCount: number;
  sourceTriangles: number;
  charts: number;
  atlasTexels: number;
  coveredTexels: number;
  projectedTexels: number;
  projectionMisses: number;
  dilatedTexels: number;
  coverageRatio: number;
  projectionHitRatio: number;
  requiredHitRatio: number;
  alignmentPolicy: string;
  autoAlignedSources: number;
  worldSpaceSources: number;
}

export interface AssetLabAvailability {
  state: AssetLabAvailabilityState;
  reason: string;
}

export interface AssetLabOperationState {
  status: "idle" | "busy" | "success" | "error";
  stage?: AssetLabStage;
  message?: string;
  /** Measured completion in `[0, 1]`, when the operation exposes progress. */
  progress?: number;
}

export type RepairPreset = "safe" | "standard" | "thorough";
export type OptimizationPreset =
  | "draft"
  | "balanced"
  | "highQuality"
  | "mobile"
  | "web"
  | "desktop"
  | "cinematic";

export interface AssetRepairConfig {
  preset: RepairPreset;
  weldThreshold: number;
  preserveAttributeSeams: boolean;
  removeDegenerateTriangles: boolean;
  removeDuplicateTriangles: boolean;
  removeIsolatedVertices: boolean;
  removeComponentsSmallerThanTriangles: number;
  repairWinding: boolean;
  normalRepair: "preserve" | "missingOrInvalid" | "recomputeAreaWeightedSmooth";
}

export interface AssetOptimizationConfig {
  preset: OptimizationPreset;
  targetRatio: number;
  candidateLevels: number;
  baseFraction: number;
}

export interface AssetUvConfig {
  mode: "chart" | "planarWhenAbsent" | "replaceIncompleteWithPlanar";
  resolution: 512 | 1024 | 2048 | 4096;
  paddingPx: number;
  texelsPerUnit: number | null;
  replaceTexturedUv0: boolean;
  inspectAfterGeneration: boolean;
}

export type AssetMaterialPreset = "studio-paint" | "brushed-metal" | "matte-clay" | "technical-plastic";

export interface AssetMaterialConfig {
  preset: AssetMaterialPreset;
}

export interface AssetBakeConfig {
  resolution: 512 | 1024 | 2048 | 4096;
  paddingPx: number;
  highSourceIds: readonly string[];
  cageDistance: number;
  aoDistance: number;
  aoSamples: 8 | 16 | 32 | 64;
  curvatureScale: number;
  alignmentPolicy: "autoRelated" | "worldSpace";
  minProjectionHitRatio: number;
  maps: readonly ("normal" | "ambientOcclusion" | "curvature")[];
}

export interface AssetExportConfig {
  format: "glb" | "usda" | "step";
  scope: "scene" | "asset";
}

/** One callback covers the complete workflow without coupling this panel to transport/session types. */
export type AssetLabAction =
  | { action: "inspect"; assetId: string }
  | { action: "repair"; assetId: string; config: AssetRepairConfig }
  | { action: "optimize"; assetId: string; config: AssetOptimizationConfig }
  | { action: "generateUv"; assetId: string; config: AssetUvConfig }
  | { action: "applyMaterial"; assetId: string; config: AssetMaterialConfig }
  | { action: "bake"; assetId: string; config: AssetBakeConfig }
  | { action: "validate"; assetId: string }
  | { action: "generateCollision"; assetId: string }
  | { action: "export"; assetId: string; config: AssetExportConfig };

export interface AssetLabPanelProps {
  asset: AssetLabSelection | null;
  report?: AssetLabReportView | null;
  availability?: Partial<Record<AssetLabStage, AssetLabAvailability>>;
  collisionAvailability?: AssetLabAvailability;
  operation?: AssetLabOperationState;
  /** Mesh-bearing scene objects that can contribute high-detail geometry to a projection bake. */
  bakeSources?: readonly AssetLabSelection[];
  activeStage?: AssetLabStage;
  onStageChange?: (stage: AssetLabStage) => void;
  onRun: (request: AssetLabAction) => void | Promise<void>;
  onCancel?: () => void;
  onChooseAsset?: () => void;
  onResetResult?: () => void;
}

/** The Model engine's own glyph — the same one the Engines rail and the bottom dock's tab both
 *  show for this workspace. It used to be the string "R", which is the INITIAL OF THE REPAIR
 *  STAGE below: a per-stage mnemonic borrowed for the panel that contains all seven stages, so
 *  the Model workspace introduced itself with a letter that means one of its parts. Nothing
 *  could see it — a letter is a perfectly legible icon to every invariant in the harness — and
 *  it is visible in `shell-dock-short` beside the word "Model" and again inside the empty
 *  state. Stated once here rather than three times inline, and deliberately NOT imported from
 *  `EngineRail`: a panel reaching into the shell's engine table to read a glyph is a
 *  dependency the wrong way round. */
const MODEL_ICON = <Icon name="model" size="xl" />;

const STAGES: readonly { id: AssetLabStage; label: string; icon: string; guidance: string }[] = [
  { id: "inspect", label: "Inspect", icon: "inspect", guidance: "Measure geometry, topology, UVs, materials, and readiness before changing anything." },
  { id: "repair", label: "Repair", icon: "repair", guidance: "Create a cleaned derivative with every change counted and reviewable." },
  { id: "optimize", label: "Optimize", icon: "optimize", guidance: "Build a deterministic real-time derivative and compare it with the source." },
  { id: "uv", label: "UV & Materials", icon: "uv", guidance: "Create chart UVs, pack a measured atlas, control texel density, and generate MikkTSpace tangents." },
  { id: "bake", label: "Bake", icon: "bake", guidance: "Project normal, ambient-occlusion, and signed-curvature maps from one or more high-detail scene sources." },
  { id: "validate", label: "Validate", icon: "validate", guidance: "Re-run measurable production checks on the current derivative." },
  { id: "export", label: "Export", icon: "export", guidance: "Write a validated derivative in a supported interchange format." },
];

const REPAIR_PRESETS: Record<RepairPreset, Pick<AssetRepairConfig, "weldThreshold" | "removeComponentsSmallerThanTriangles" | "normalRepair"> & { note: string }> = {
  safe: {
    weldThreshold: 0.000_001,
    removeComponentsSmallerThanTriangles: 0,
    normalRepair: "missingOrInvalid",
    note: "Preserves seams and only repairs clear defects.",
  },
  standard: {
    weldThreshold: 0.000_01,
    removeComponentsSmallerThanTriangles: 0,
    normalRepair: "missingOrInvalid",
    note: "A practical cleanup for most static production assets.",
  },
  thorough: {
    weldThreshold: 0.000_1,
    removeComponentsSmallerThanTriangles: 3,
    normalRepair: "recomputeAreaWeightedSmooth",
    note: "Removes tiny disconnected shells and rebuilds smooth normals; review the result carefully.",
  },
};

const OPTIMIZATION_PRESETS: Record<OptimizationPreset, { ratio: number; note: string }> = {
  draft: { ratio: 0.15, note: "Fast blockout and distant background use." },
  balanced: { ratio: 0.5, note: "Balanced viewport quality and cost." },
  highQuality: { ratio: 0.72, note: "Retains more source detail." },
  mobile: { ratio: 0.2, note: "Aggressive target for constrained devices." },
  web: { ratio: 0.35, note: "Moderate download and rendering budget." },
  desktop: { ratio: 0.6, note: "Desktop real-time target." },
  cinematic: { ratio: 0.85, note: "Near-source detail for hero use." },
};

const DEFAULT_AVAILABILITY: Record<AssetLabStage, AssetLabAvailability> = {
  inspect: { state: "available", reason: "Asset audit is available." },
  repair: { state: "unavailable", reason: "Run Inspect first so repairs have a measured baseline." },
  optimize: { state: "unavailable", reason: "Run Inspect first and repair blocking geometry issues." },
  uv: { state: "unavailable", reason: "Run Inspect first to establish UV condition." },
  bake: { state: "unavailable", reason: "Inspect the low target and choose at least one high-detail source." },
  validate: { state: "unavailable", reason: "Run Inspect first to establish validation checks." },
  export: { state: "unsupported", reason: "No verified export writer is connected." },
};

function availabilityLabel(state: AssetLabAvailabilityState): string {
  if (state === "available") return "Ready";
  if (state === "unsupported") return "Unsupported";
  return "Unavailable";
}

function operationFallback(stage: AssetLabStage): string {
  return {
    inspect: "Inspection complete",
    repair: "Repair derivative complete",
    optimize: "Optimization derivative complete",
    uv: "UV preparation complete",
    bake: "Bake complete",
    validate: "Validation complete",
    export: "Export complete",
  }[stage];
}

export function AssetLabPanel({
  asset,
  report = null,
  availability,
  collisionAvailability = { state: "unsupported", reason: "No verified collision generator is connected." },
  operation,
  bakeSources = [],
  activeStage,
  onStageChange,
  onRun,
  onCancel,
  onChooseAsset,
  onResetResult,
}: AssetLabPanelProps) {
  const [localStage, setLocalStage] = useState<AssetLabStage>("inspect");
  const [localOperation, setLocalOperation] = useState<AssetLabOperationState>({ status: "idle" });
  const [repair, setRepair] = useState<AssetRepairConfig>({
    preset: "safe",
    weldThreshold: REPAIR_PRESETS.safe.weldThreshold,
    preserveAttributeSeams: true,
    removeDegenerateTriangles: true,
    removeDuplicateTriangles: true,
    removeIsolatedVertices: true,
    removeComponentsSmallerThanTriangles: 0,
    repairWinding: true,
    normalRepair: REPAIR_PRESETS.safe.normalRepair,
  });
  const [optimization, setOptimization] = useState<AssetOptimizationConfig>({
    preset: "balanced",
    targetRatio: OPTIMIZATION_PRESETS.balanced.ratio,
    candidateLevels: 10,
    baseFraction: 0.003,
  });
  const [uv, setUv] = useState<AssetUvConfig>({ mode: "chart", resolution: 2048, paddingPx: 8, texelsPerUnit: null, replaceTexturedUv0: false, inspectAfterGeneration: true });
  const [material, setMaterial] = useState<AssetMaterialConfig>({ preset: "studio-paint" });
  const [bake, setBake] = useState<AssetBakeConfig>({
    resolution: 1024,
    paddingPx: 16,
    highSourceIds: [],
    cageDistance: 0.1,
    aoDistance: 0.25,
    aoSamples: 16,
    curvatureScale: 0.05,
    alignmentPolicy: "autoRelated",
    minProjectionHitRatio: 0.9,
    maps: ["normal", "ambientOcclusion", "curvature"],
  });
  const [exportConfig, setExportConfig] = useState<AssetExportConfig>({ format: "glb", scope: "scene" });

  useEffect(() => {
    if (!asset || !bakeSources.some((source) => source.id === asset.id)) return;
    setBake((current) => current.highSourceIds.length > 0
      ? current
      : { ...current, highSourceIds: [asset.id] });
  }, [asset?.id, bakeSources]);

  const stage = activeStage ?? localStage;
  const currentStage = STAGES.find((item) => item.id === stage) ?? STAGES[0];
  const effectiveAvailability = useMemo(() => {
    const defaults = { ...DEFAULT_AVAILABILITY };
    if (report) {
      defaults.repair = { state: "available", reason: "A measured source baseline is available." };
      defaults.optimize = { state: "available", reason: "Optimization can be attempted against the measured source." };
      defaults.uv = { state: "available", reason: "UV preparation can be attempted against the measured source." };
      defaults.validate = { state: "available", reason: "Validation checks have a measured baseline." };
    }
    return { ...defaults, ...availability };
  }, [availability, report]);
  const stageAvailability = effectiveAvailability[stage];
  const effectiveOperation = operation ?? localOperation;
  const busy = effectiveOperation.status === "busy";

  function selectStage(next: string) {
    const nextStage = next as AssetLabStage;
    if (activeStage == null) setLocalStage(nextStage);
    setLocalOperation({ status: "idle" });
    onStageChange?.(nextStage);
  }

  async function run(request: AssetLabAction, successStage: AssetLabStage) {
    if (!asset || busy || effectiveAvailability[successStage].state !== "available") return;
    setLocalOperation({ status: "busy", stage: successStage, message: `Running ${currentStage.label.toLowerCase()}...` });
    try {
      await onRun(request);
      setLocalOperation({ status: "success", stage: successStage, message: operationFallback(successStage) });
    } catch (cause) {
      const detail = cause instanceof Error && cause.message ? cause.message : "The operation did not complete.";
      setLocalOperation({ status: "error", stage: successStage, message: detail });
    }
  }

  const tabs = STAGES.map((item) => ({
    id: item.id,
    label: item.label,
    icon: <Icon name={item.icon} size="md" />,
    tooltip: item.guidance,
    badge:
      effectiveAvailability[item.id].state === "unsupported" ? (
        <span aria-label="Unsupported">--</span>
      ) : undefined,
    panelId: `asset-lab-${item.id}-panel`,
  }));

  if (!asset) {
    return (
      <WorkspacePanel
        title="Model"
        subtitle="Production preparation"
        icon={MODEL_ICON}
        className="asset-lab"
        data-testid="asset-lab"
      >
        <EmptyPanelState
          data-testid="asset-lab-empty"
          icon={MODEL_ICON}
          title="Select one mesh asset"
          description="Model audits and produces a reviewable derivative. It never overwrites the source asset."
          primaryAction={
            onChooseAsset ? (
              <Button variant="primary" onClick={onChooseAsset} data-testid="asset-lab-choose">
                Choose an asset
              </Button>
            ) : undefined
          }
        />
      </WorkspacePanel>
    );
  }

  return (
    <WorkspacePanel
      title="Model"
      subtitle={`${asset.name}${asset.source ? ` / ${asset.source}` : ""}`}
      icon={MODEL_ICON}
      className="asset-lab"
      data-testid="asset-lab"
      busy={busy}
      actions={
        report?.resultMetrics && onResetResult ? (
          <Button variant="ghost" compact onClick={onResetResult} disabled={busy} data-testid="asset-lab-reset">
            Show source metrics
          </Button>
        ) : undefined
      }
      tabs={
        <DockTabs
          id="asset-lab-stages"
          data-testid="asset-lab-stages"
          ariaLabel="Asset production stages"
          tabs={tabs}
          activeId={stage}
          onChange={selectStage}
          style={{ overflowX: "auto", scrollbarWidth: "thin" }}
        />
      }
      footer={<span>Source protected / Every operation creates or validates a derivative</span>}
    >
      <div
        id={`asset-lab-${stage}-panel`}
        role="tabpanel"
        aria-labelledby={`asset-lab-stages-${stage}-tab`}
        className="asset-lab__stage"
      >
        <StageIntro stage={currentStage} availability={stageAvailability} />

        {stage === "inspect" && (
          <InspectStage
            asset={asset}
            report={report}
            busy={busy}
            disabled={stageAvailability.state !== "available"}
            disabledReason={stageAvailability.reason}
            onRun={() => void run({ action: "inspect", assetId: asset.id }, "inspect")}
          />
        )}
        {stage === "repair" && (
          <RepairStage
            value={repair}
            onChange={setRepair}
            report={report}
            busy={busy}
            availability={stageAvailability}
            onRun={() => void run({ action: "repair", assetId: asset.id, config: repair }, "repair")}
          />
        )}
        {stage === "optimize" && (
          <OptimizeStage
            value={optimization}
            onChange={setOptimization}
            report={report}
            busy={busy}
            availability={stageAvailability}
            onRun={() =>
              void run({ action: "optimize", assetId: asset.id, config: optimization }, "optimize")
            }
          />
        )}
        {stage === "uv" && (
          <UvStage
            value={uv}
            onChange={setUv}
            material={material}
            onMaterialChange={setMaterial}
            report={report}
            busy={busy}
            availability={stageAvailability}
            onRunUv={() => void run({ action: "generateUv", assetId: asset.id, config: uv }, "uv")}
            onRunMaterial={() => void run({ action: "applyMaterial", assetId: asset.id, config: material }, "uv")}
          />
        )}
        {stage === "bake" && (
          <BakeStage
            value={bake}
            onChange={setBake}
            report={report}
            busy={busy}
            availability={stageAvailability}
            sources={bakeSources}
            targetId={asset.id}
            onRun={() => void run({ action: "bake", assetId: asset.id, config: bake }, "bake")}
          />
        )}
        {stage === "validate" && (
          <ValidateStage
            report={report}
            busy={busy}
            availability={stageAvailability}
            collisionAvailability={collisionAvailability}
            onRun={() => void run({ action: "validate", assetId: asset.id }, "validate")}
            onCollision={() => void run({ action: "generateCollision", assetId: asset.id }, "validate")}
          />
        )}
        {stage === "export" && (
          <ExportStage
            value={exportConfig}
            onChange={setExportConfig}
            busy={busy}
            availability={stageAvailability}
            onRun={() =>
              void run({ action: "export", assetId: asset.id, config: exportConfig }, "export")
            }
          />
        )}

        <OperationFeedback state={effectiveOperation} onCancel={onCancel} />
      </div>
    </WorkspacePanel>
  );
}

function StageIntro({
  stage,
  availability,
}: {
  stage: (typeof STAGES)[number];
  availability: AssetLabAvailability;
}) {
  const unavailable = availability.state !== "available";
  return (
    <div className="asset-lab__intro">
      <div>
        <h3>{stage.label}</h3>
        <p>{stage.guidance}</p>
      </div>
      <Badge tone={availability.state === "available" ? "success" : "warn"} title={availability.reason}>
        {availabilityLabel(availability.state)}
      </Badge>
      {unavailable && (
        <div id={`asset-lab-${stage.id}-reason`} className="asset-lab__availability" role="note">
          {availability.reason}
        </div>
      )}
    </div>
  );
}

function InspectStage({
  asset,
  report,
  busy,
  disabled,
  disabledReason,
  onRun,
}: {
  asset: AssetLabSelection;
  report: AssetLabReportView | null;
  busy: boolean;
  disabled: boolean;
  disabledReason: string;
  onRun: () => void;
}) {
  return (
    <>
      <div className="asset-lab__source-card">
        <span className="asset-lab__source-icon" aria-hidden="true"><Icon name="validate" size="sm" /></span>
        <span><strong>{asset.name}</strong><small>{asset.revision ?? asset.id}</small></span>
        <Badge>Source</Badge>
      </div>
      {report ? <ReportEvidence report={report} /> : <Guidance>No audit has been run for this asset yet.</Guidance>}
      <PrimaryAction
        label={report ? "Inspect again" : "Inspect asset"}
        busyLabel="Inspecting..."
        busy={busy}
        disabled={disabled}
        reason={disabledReason}
        onClick={onRun}
        testId="asset-lab-inspect"
      />
    </>
  );
}

function RepairStage({
  value,
  onChange,
  report,
  busy,
  availability,
  onRun,
}: {
  value: AssetRepairConfig;
  onChange: (value: AssetRepairConfig) => void;
  report: AssetLabReportView | null;
  busy: boolean;
  availability: AssetLabAvailability;
  onRun: () => void;
}) {
  function choosePreset(preset: RepairPreset) {
    const mapped = REPAIR_PRESETS[preset];
    onChange({
      ...value,
      preset,
      weldThreshold: mapped.weldThreshold,
      removeComponentsSmallerThanTriangles: mapped.removeComponentsSmallerThanTriangles,
      normalRepair: mapped.normalRepair,
    });
  }
  return (
    <>
      <PresetField
        label="Repair preset"
        value={value.preset}
        disabled={busy || availability.state !== "available"}
        onChange={(next) => choosePreset(next as RepairPreset)}
        options={[
          ["safe", "Safe"],
          ["standard", "Standard"],
          ["thorough", "Thorough"],
        ]}
        note={REPAIR_PRESETS[value.preset].note}
      />
      <DisclosureSection title="Advanced repair controls" summary="Thresholds & topology" defaultOpen={false}>
        <fieldset className="asset-lab__form" disabled={busy || availability.state !== "available"}>
          <NumericControl
            label="Weld threshold"
            description="Absolute distance in the asset's local units. Attribute seams remain split."
            value={value.weldThreshold}
            min={0}
            step={0.000001}
            onCommit={(weldThreshold) => onChange({ ...value, weldThreshold })}
          />
          <NumericControl
            label="Remove components smaller than"
            description="Zero keeps every connected shell."
            value={value.removeComponentsSmallerThanTriangles}
            min={0}
            step={1}
            integer
            suffix="triangles"
            onCommit={(removeComponentsSmallerThanTriangles) =>
              onChange({ ...value, removeComponentsSmallerThanTriangles })
            }
          />
          <Check label="Preserve UV and normal seams" checked={value.preserveAttributeSeams} onChange={(preserveAttributeSeams) => onChange({ ...value, preserveAttributeSeams })} />
          <Check label="Remove degenerate faces" checked={value.removeDegenerateTriangles} onChange={(removeDegenerateTriangles) => onChange({ ...value, removeDegenerateTriangles })} />
          <Check label="Remove duplicate faces" checked={value.removeDuplicateTriangles} onChange={(removeDuplicateTriangles) => onChange({ ...value, removeDuplicateTriangles })} />
          <Check label="Remove isolated vertices" checked={value.removeIsolatedVertices} onChange={(removeIsolatedVertices) => onChange({ ...value, removeIsolatedVertices })} />
          <Check label="Repair reliable winding" checked={value.repairWinding} onChange={(repairWinding) => onChange({ ...value, repairWinding })} />
        </fieldset>
      </DisclosureSection>
      {report && <ReportEvidence report={report} compact />}
      <SourceSafety />
      <PrimaryAction label="Create repaired derivative" busyLabel="Repairing..." busy={busy} disabled={availability.state !== "available"} reason={availability.reason} onClick={onRun} testId="asset-lab-repair" />
    </>
  );
}

function OptimizeStage({
  value,
  onChange,
  report,
  busy,
  availability,
  onRun,
}: {
  value: AssetOptimizationConfig;
  onChange: (value: AssetOptimizationConfig) => void;
  report: AssetLabReportView | null;
  busy: boolean;
  availability: AssetLabAvailability;
  onRun: () => void;
}) {
  function choosePreset(preset: OptimizationPreset) {
    onChange({ ...value, preset, targetRatio: OPTIMIZATION_PRESETS[preset].ratio });
  }
  return (
    <>
      <PresetField
        label="Optimization preset"
        value={value.preset}
        disabled={busy || availability.state !== "available"}
        onChange={(next) => choosePreset(next as OptimizationPreset)}
        options={[
          ["draft", "Draft"], ["balanced", "Balanced"], ["highQuality", "High quality"],
          ["mobile", "Mobile"], ["web", "Web"], ["desktop", "Desktop"], ["cinematic", "Cinematic"],
        ]}
        note={`${OPTIMIZATION_PRESETS[value.preset].note} Target ${(value.targetRatio * 100).toFixed(0)}% of source triangles.`}
      />
      <DisclosureSection title="Advanced optimization controls" summary="Target quality" defaultOpen={false}>
        <fieldset className="asset-lab__form" disabled={busy || availability.state !== "available"}>
          <NumericControl label="Target ratio" description="QEM aims for this triangle ratio while locking borders, UV seams, hard normals, material boundaries, and skin influence changes." value={value.targetRatio} min={0.01} max={0.99} step={0.01} onCommit={(targetRatio) => onChange({ ...value, targetRatio })} />
        </fieldset>
      </DisclosureSection>
      <Guidance>Attribute-aware QEM preserves textured and rigged payloads, regenerates MikkTSpace tangents, and reports when semantic locks prevent the requested reduction.</Guidance>
      {report && <ReportEvidence report={report} compact />}
      <SourceSafety />
      <PrimaryAction label="Create optimized derivative" busyLabel="Optimizing..." busy={busy} disabled={availability.state !== "available"} reason={availability.reason} onClick={onRun} testId="asset-lab-optimize" />
    </>
  );
}

function UvStage({
  value,
  onChange,
  material,
  onMaterialChange,
  report,
  busy,
  availability,
  onRunUv,
  onRunMaterial,
}: {
  value: AssetUvConfig;
  onChange: (value: AssetUvConfig) => void;
  material: AssetMaterialConfig;
  onMaterialChange: (value: AssetMaterialConfig) => void;
  report: AssetLabReportView | null;
  busy: boolean;
  availability: AssetLabAvailability;
  onRunUv: () => void;
  onRunMaterial: () => void;
}) {
  return (
    <>
      <PresetField label="UV method" value={value.mode} disabled={busy || availability.state !== "available"} onChange={(mode) => onChange({ ...value, mode: mode as AssetUvConfig["mode"] })} options={[["chart", "Chart unwrap (recommended)"], ["planarWhenAbsent", "Planar when missing"], ["replaceIncompleteWithPlanar", "Replace incomplete with planar"]]} note={value.mode === "chart" ? "xatlas segments and packs a non-overlapping square atlas, then MikkTSpace regenerates the tangent basis." : "Legacy dominant-plane projection is useful only for simple, mostly flat props."} />
      <fieldset className="asset-lab__form asset-lab__form--plain" disabled={busy || availability.state !== "available"}>
        {value.mode === "chart" && (
          <>
            <PresetField label="Atlas resolution" value={String(value.resolution)} disabled={busy} onChange={(resolution) => onChange({ ...value, resolution: Number(resolution) as AssetUvConfig["resolution"] })} options={[["512", "512 px"], ["1024", "1K"], ["2048", "2K"], ["4096", "4K"]]} note="The packed rectangle is uniformly fitted into this exact square page without distorting charts." />
            <NumericControl label="Chart padding" description="Empty pixels reserved between packed islands for filtering and mipmaps." value={value.paddingPx} min={2} max={64} step={1} integer suffix="px" onCommit={(paddingPx) => onChange({ ...value, paddingPx })} />
            <Check label="Automatic texel density" checked={value.texelsPerUnit == null} description="Fit the available atlas while preserving relative surface area." onChange={(automatic) => onChange({ ...value, texelsPerUnit: automatic ? null : 256 })} />
            {value.texelsPerUnit != null && <NumericControl label="Texels per unit" description="Exact requested density before the final single-page fit." value={value.texelsPerUnit} min={1} max={8192} step={1} integer suffix="px/unit" onCommit={(texelsPerUnit) => onChange({ ...value, texelsPerUnit })} />}
            <Check label="Replace UV0 on textured materials" checked={value.replaceTexturedUv0} description="Enable only when you will rebake every bound texture; otherwise existing artwork would no longer align." onChange={(replaceTexturedUv0) => onChange({ ...value, replaceTexturedUv0 })} />
          </>
        )}
        <Check label="Inspect range and overlap after generation" checked={value.inspectAfterGeneration} onChange={(inspectAfterGeneration) => onChange({ ...value, inspectAfterGeneration })} />
      </fieldset>
      <PresetField
        label="Material finish"
        value={material.preset}
        disabled={busy || availability.state !== "available"}
        onChange={(preset) => onMaterialChange({ preset: preset as AssetMaterialPreset })}
        options={[
          ["studio-paint", "Studio paint"],
          ["brushed-metal", "Brushed metal"],
          ["matte-clay", "Matte clay"],
          ["technical-plastic", "Technical plastic"],
        ]}
        note="Applies deterministic metallic-roughness factors to a new derivative. Existing texture pixels stay intact."
      />
      {report && <ReportEvidence report={report} compact />}
      <SourceSafety />
      <div className="asset-lab__action-row">
        <PrimaryAction label={value.mode === "chart" ? "Unwrap & pack derivative" : "Create UV derivative"} busyLabel="Generating UV0..." busy={busy} disabled={availability.state !== "available"} reason={availability.reason} onClick={onRunUv} testId="asset-lab-uv" />
        <PrimaryAction label="Create material derivative" busyLabel="Applying material..." busy={busy} disabled={availability.state !== "available"} reason={availability.reason} onClick={onRunMaterial} testId="asset-lab-material" />
      </div>
    </>
  );
}

function BakeStage({ value, onChange, report, busy, availability, sources, targetId, onRun }: { value: AssetBakeConfig; onChange: (value: AssetBakeConfig) => void; report: AssetLabReportView | null; busy: boolean; availability: AssetLabAvailability; sources: readonly AssetLabSelection[]; targetId: string; onRun: () => void }) {
  const disabled = busy || availability.state !== "available";
  function toggleMap(map: AssetBakeConfig["maps"][number], checked: boolean) {
    const maps = checked ? [...value.maps, map] : value.maps.filter((item) => item !== map);
    onChange({ ...value, maps });
  }
  function toggleSource(id: string, checked: boolean) {
    const highSourceIds = checked
      ? [...value.highSourceIds, id]
      : value.highSourceIds.filter((item) => item !== id);
    onChange({ ...value, highSourceIds });
  }
  return (
    <>
      <fieldset className="asset-lab__form asset-lab__form--plain" disabled={disabled} aria-describedby="asset-lab-bake-limit">
        <PresetField label="Texture resolution" value={String(value.resolution)} disabled={disabled} onChange={(resolution) => onChange({ ...value, resolution: Number(resolution) as AssetBakeConfig["resolution"] })} options={[["512", "512 px"], ["1024", "1K"], ["2048", "2K"], ["4096", "4K (high memory)"]]} note="1K is the responsive preview default. Higher resolutions use the same bounded production rasterizer and take proportionally longer." />
        <PresetField
          label="Source alignment"
          value={value.alignmentPolicy}
          disabled={disabled}
          onChange={(alignmentPolicy) => onChange({ ...value, alignmentPolicy: alignmentPolicy as AssetBakeConfig["alignmentPolicy"] })}
          options={[["autoRelated", "Auto-align related assets (recommended)"], ["worldSpace", "Preserve scene positions"]]}
          note={value.alignmentPolicy === "autoRelated"
            ? "Recognizes this derivative family and removes only the editor's side-by-side review offset. Unrelated sources keep their authored world transforms."
            : "Uses every source's exact scene transform. Choose this for scans or source meshes you have already registered in world space."}
        />
        <fieldset className="asset-lab__source-list">
          <legend>High-detail sources</legend>
          {sources.length === 0
            ? <Guidance>No other resolved mesh objects are available in this scene.</Guidance>
            : sources.map((source) => (
              <Check key={source.id} label={`${source.name}${source.id === targetId ? " (selected target / self-bake)" : ""}`} checked={value.highSourceIds.includes(source.id)} description={source.id === targetId ? "Useful for AO and curvature; choose another mesh for a true high-to-low projection." : value.alignmentPolicy === "autoRelated" ? "Related derivatives auto-align to remove review offsets; unrelated sources retain their authored world transform." : "Uses this object's exact world transform in the shared bake space."} onChange={(checked) => toggleSource(source.id, checked)} />
            ))}
        </fieldset>
        <div className="asset-lab__checks" role="group" aria-label="Bake maps">
          <Check label="Normal" checked={value.maps.includes("normal")} onChange={(checked) => toggleMap("normal", checked)} />
          <Check label="Ambient occlusion" checked={value.maps.includes("ambientOcclusion")} onChange={(checked) => toggleMap("ambientOcclusion", checked)} />
          <Check label="Signed curvature" checked={value.maps.includes("curvature")} onChange={(checked) => toggleMap("curvature", checked)} />
        </div>
      </fieldset>
      <DisclosureSection title="Advanced projection controls" summary="Cage, rays & curvature" defaultOpen={false}>
        <fieldset className="asset-lab__form" disabled={disabled}>
          <NumericControl label="Cage distance" description="Search distance on either side of the low surface in shared scene units." value={value.cageDistance} min={0.0001} max={1000} step={0.01} onCommit={(cageDistance) => onChange({ ...value, cageDistance })} />
          <NumericControl label="AO distance" description="Maximum hemisphere ray length in shared scene units." value={value.aoDistance} min={0.0001} max={1000} step={0.01} onCommit={(aoDistance) => onChange({ ...value, aoDistance })} />
          <PresetField label="AO quality" value={String(value.aoSamples)} disabled={disabled} onChange={(samples) => onChange({ ...value, aoSamples: Number(samples) as AssetBakeConfig["aoSamples"] })} options={[["8", "Draft · 8 rays"], ["16", "Preview · 16 rays"], ["32", "Production · 32 rays"], ["64", "Hero · 64 rays"]]} note="Uses a deterministic cosine-weighted Hammersley sequence." />
          <NumericControl label="Curvature response" description="Maps signed mean curvature into the texture range; increase for subtler large-radius surfaces." value={value.curvatureScale} min={0.0001} max={1000} step={0.01} onCommit={(curvatureScale) => onChange({ ...value, curvatureScale })} />
          <PresetField label="Projection quality floor" value={String(value.minProjectionHitRatio)} disabled={disabled} onChange={(ratio) => onChange({ ...value, minProjectionHitRatio: Number(ratio) })} options={[["0.75", "75% / permissive"], ["0.9", "90% / production default"], ["0.98", "98% / strict"], ["1", "100% / exact coverage"]]} note="The bake fails clearly below this covered-texel hit ratio; correctly-sized blank or mostly missed maps are never published." />
        </fieldset>
      </DisclosureSection>
      <Guidance id="asset-lab-bake-limit">Every requested channel, target UV, source transform, projection hit ratio, material binding, and output dimension is validated. Partial map output is treated as an error.</Guidance>
      {report && <ReportEvidence report={report} compact />}
      <SourceSafety />
      <PrimaryAction label="Bake maps to derivative" busyLabel="Projecting & rasterizing..." busy={busy} disabled={availability.state !== "available" || value.maps.length === 0 || value.highSourceIds.length === 0} reason={value.highSourceIds.length === 0 ? "Choose at least one high-detail source." : availability.reason} onClick={onRun} testId="asset-lab-bake" />
    </>
  );
}

function ValidateStage({ report, busy, availability, collisionAvailability, onRun, onCollision }: { report: AssetLabReportView | null; busy: boolean; availability: AssetLabAvailability; collisionAvailability: AssetLabAvailability; onRun: () => void; onCollision: () => void }) {
  return (
    <>
      {report ? <ReportEvidence report={report} /> : <Guidance>Validation needs an inspection baseline.</Guidance>}
      <div className="asset-lab__action-row">
        <PrimaryAction label="Validate derivative" busyLabel="Validating..." busy={busy} disabled={availability.state !== "available"} reason={availability.reason} onClick={onRun} testId="asset-lab-validate" />
        <PrimaryAction label="Generate convex-hull collision" busyLabel="Building collision..." busy={busy} disabled={collisionAvailability.state !== "available"} reason={collisionAvailability.reason} onClick={onCollision} testId="asset-lab-collision" />
      </div>
    </>
  );
}

function ExportStage({ value, busy, availability, onChange, onRun }: { value: AssetExportConfig; busy: boolean; availability: AssetLabAvailability; onChange: (value: AssetExportConfig) => void; onRun: () => void }) {
  const disabled = busy || availability.state !== "available";
  return (
    <>
      <fieldset className="asset-lab__form asset-lab__form--plain" disabled={disabled}>
        <PresetField
          label="Export scope"
          value={value.scope}
          disabled={disabled}
          onChange={(scope) => onChange({ scope: scope as AssetExportConfig["scope"], format: scope === "asset" ? "glb" : value.format })}
          options={[["scene", "Complete scene (recommended)"], ["asset", "Selected derivative only"]]}
          note={value.scope === "scene" ? "Keeps hierarchy, reusable mesh instances, skins and representable animation." : "Writes only the selected derivative as one portable asset."}
        />
        <PresetField
          label="Export format"
          value={value.format}
          disabled={disabled}
          onChange={(format) => onChange({ ...value, format: format as AssetExportConfig["format"] })}
          options={value.scope === "scene"
            ? [["glb", "GLB / self-contained scene"], ["usda", "USDA / readable hierarchy"]]
            : [["glb", "GLB / embedded textures"]]}
          note={value.format === "glb"
            ? "Best portable result: one binary file with geometry, materials and textures."
            : "Deterministic ASCII USD for hierarchy and technical interchange; texture payloads are reported as omitted."}
        />
      </fieldset>
      <DisclosureSection title="What will be preserved?" summary="Export fidelity" defaultOpen={false}>
        <Guidance>
          GLB preserves the scene tree, shared meshes, embedded RGBA textures, imported skins and standard transform animation.
          USDA preserves the authored hierarchy, transforms, skin metadata and typed animation in readable form, but it is not binary USDC or packaged USDZ.
          After export, the editor reports every preserved, converted or omitted feature.
        </Guidance>
      </DisclosureSection>
      <Guidance>Export writes a new file and never replaces source assets or scene history.</Guidance>
      <PrimaryAction label={value.scope === "scene" ? "Export complete scene" : "Export selected derivative"} busyLabel="Exporting..." busy={busy} disabled={availability.state !== "available"} reason={availability.reason} onClick={onRun} testId="asset-lab-export" />
    </>
  );
}

function ReportEvidence({ report, compact = false }: { report: AssetLabReportView; compact?: boolean }) {
  return (
    <section className={`asset-lab__evidence${compact ? " is-compact" : ""}`} aria-label="Asset evidence">
      <p className="asset-lab__summary">{report.summary}</p>
      {report.bakeEvidence && <BakeEvidence evidence={report.bakeEvidence} />}
      <MetricComparison source={report.sourceMetrics} result={report.resultMetrics} />
      <IssueList issues={report.issues} />
      {report.warnings?.map((warning) => (
        <div className="asset-lab__warning" role="note" key={warning}>! {warning}</div>
      ))}
    </section>
  );
}

function BakeEvidence({ evidence }: { evidence: AssetLabBakeEvidenceView }) {
  const hitPercent = (evidence.projectionHitRatio * 100).toFixed(1);
  const requiredPercent = (evidence.requiredHitRatio * 100).toFixed(0);
  const coveragePercent = (evidence.coverageRatio * 100).toFixed(1);
  const passed = evidence.projectedTexels > 0
    && evidence.projectionHitRatio >= evidence.requiredHitRatio;
  return (
    <section
      className={`asset-lab__bake-evidence ${passed ? "is-pass" : "is-fail"}`}
      aria-label="Bake projection evidence"
      data-testid="asset-lab-bake-evidence"
      data-projected-texels={evidence.projectedTexels}
      data-covered-texels={evidence.coveredTexels}
      data-projection-hit-ratio={evidence.projectionHitRatio}
      data-required-hit-ratio={evidence.requiredHitRatio}
      data-alignment-policy={evidence.alignmentPolicy}
      data-auto-aligned-sources={evidence.autoAlignedSources}
      data-world-space-sources={evidence.worldSpaceSources}
    >
      <div><span>Projection quality</span><strong>{hitPercent}%</strong><small>{passed ? "Passed" : "Rejected"} / minimum {requiredPercent}%</small></div>
      <div><span>Projected texels</span><strong>{evidence.projectedTexels.toLocaleString("en-GB")}</strong><small>of {evidence.coveredTexels.toLocaleString("en-GB")} covered</small></div>
      <div><span>Atlas coverage</span><strong>{coveragePercent}%</strong><small>{evidence.charts || "Unfinished"} UV charts / {evidence.dilatedTexels.toLocaleString("en-GB")} padded</small></div>
      <div><span>Source registration</span><strong>{evidence.alignmentPolicy === "autoRelated" ? "Automatic" : "World space"}</strong><small>{evidence.autoAlignedSources} related aligned / {evidence.worldSpaceSources} world-space</small></div>
    </section>
  );
}

function MetricComparison({ source, result }: { source: readonly AssetLabMetric[]; result?: readonly AssetLabMetric[] }) {
  const resultById = new Map(result?.map((metric) => [metric.id, metric]));
  return (
    <div className="asset-lab__metrics" data-testid="asset-lab-metrics">
      {source.map((before) => {
        const after = resultById.get(before.id);
        return (
          <div className="asset-lab__metric" key={before.id} title={before.description}>
            <span>{before.label}</span>
            <strong>{formatMetric(before)}</strong>
            {after && <><span className="asset-lab__metric-arrow" aria-hidden="true">-&gt;</span><strong className="is-after">{formatMetric(after)}</strong><span className="asset-lab__sr-only"> after</span></>}
          </div>
        );
      })}
    </div>
  );
}

function formatMetric(metric: AssetLabMetric): string {
  const value = typeof metric.value === "number" ? metric.value.toLocaleString("en-GB") : metric.value;
  return `${value}${metric.unit ? ` ${metric.unit}` : ""}`;
}

function IssueList({ issues }: { issues: readonly AssetLabIssue[] }) {
  if (issues.length === 0) return <div className="asset-lab__all-clear" role="status">OK / No measured issues</div>;
  return (
    <ul className="asset-lab__issues" aria-label="Measured issues">
      {issues.map((issue) => (
        <li key={issue.id} data-severity={issue.severity} data-status={issue.status ?? "open"}>
          <span className="asset-lab__issue-mark" aria-hidden="true">{issue.severity === "pass" ? "OK" : issue.severity === "info" ? "i" : "!"}</span>
          <span><strong>{issue.title}{issue.count != null ? ` / ${issue.count}` : ""}</strong><small>{issue.detail}</small></span>
          <Badge tone={issue.status === "fixed" ? "success" : issue.severity === "warning" || issue.severity === "critical" || issue.severity === "error" ? "warn" : "neutral"}>{issue.status ?? issue.severity}</Badge>
        </li>
      ))}
    </ul>
  );
}

function PresetField({ label, value, options, note, disabled, onChange }: { label: string; value: string; options: readonly (readonly [string, string])[]; note: string; disabled: boolean; onChange: (value: string) => void }) {
  const id = useId();
  return (
    <label className="asset-lab__preset" htmlFor={id}>
      <span>{label}</span>
      <select id={id} aria-label={label} className="mtk-input" value={value} disabled={disabled} onChange={(event) => onChange(event.target.value)}>
        {options.map(([option, optionLabel]) => <option value={option} key={option}>{optionLabel}</option>)}
      </select>
      <small>{note}</small>
    </label>
  );
}

function NumericControl({ label, description, suffix, ...props }: { label: string; description: string; suffix?: string } & Omit<React.ComponentProps<typeof NumericField>, "ariaLabel">) {
  return (
    <label className="asset-lab__numeric">
      <span title={description}>{label}</span>
      <span><NumericField {...props} ariaLabel={label} />{suffix && <small>{suffix}</small>}</span>
      <small>{description}</small>
    </label>
  );
}

function Check({ label, description, checked, disabled = false, onChange }: { label: string; description?: string; checked: boolean; disabled?: boolean; onChange: (checked: boolean) => void }) {
  return (
    <label className={`asset-lab__check${disabled ? " is-disabled" : ""}`} title={description}>
      <input type="checkbox" checked={checked} disabled={disabled} onChange={(event) => onChange(event.target.checked)} />
      <span>{label}{description && <small>{description}</small>}</span>
    </label>
  );
}

function Guidance({ children, id }: { children: ReactNode; id?: string }) {
  return <div id={id} className="asset-lab__guidance" role="note">{children}</div>;
}

function SourceSafety() {
  return <div className="asset-lab__source-safety"><Icon name="validate" size="md" /><span><strong>Source stays unchanged</strong><small>Review the derivative before using it in the scene.</small></span></div>;
}

function PrimaryAction({ label, busyLabel, busy, disabled, reason, onClick, testId }: { label: string; busyLabel: string; busy: boolean; disabled: boolean; reason: string; onClick: () => void; testId: string }) {
  return (
    <div className="asset-lab__primary-action">
      <Button variant="primary" data-testid={testId} disabled={busy || disabled} aria-describedby={disabled ? `${testId}-reason` : undefined} title={disabled ? reason : undefined} onClick={onClick}>{busy ? busyLabel : label}</Button>
      {disabled && <small id={`${testId}-reason`}>{reason}</small>}
    </div>
  );
}

function OperationFeedback({ state, onCancel }: { state: AssetLabOperationState; onCancel?: () => void }) {
  if (state.status === "idle") return null;
  const role = state.status === "error" ? "alert" : "status";
  const progress = state.progress == null ? undefined : Math.max(0, Math.min(1, state.progress));
  return (
    <div className={`asset-lab__operation is-${state.status}`} role={role} aria-live={state.status === "error" ? "assertive" : "polite"} data-testid="asset-lab-operation">
      <span>{state.status === "busy" ? "..." : state.status === "success" ? "OK" : "!"}</span>
      <div><strong>{state.message ?? (state.status === "busy" ? "Working..." : state.status)}</strong>{progress != null && <progress max={1} value={progress} aria-label="Operation progress" />}</div>
      {state.status === "busy" && onCancel && <Button variant="secondary" compact onClick={onCancel}>Cancel</Button>}
    </div>
  );
}
