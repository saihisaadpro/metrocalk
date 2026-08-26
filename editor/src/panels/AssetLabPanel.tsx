//! Asset Lab -- one source-preserving workflow for inspecting, repairing, optimizing, finishing, validating,
//! and exporting a selected 3D asset. The panel is intentionally transport-agnostic: its discriminated
//! action callback can be wired to native commands without importing the editor session or protocol.

import { useEffect, useId, useMemo, useState, type ReactNode } from "react";
import { Icon } from "../theme/icons";
import {
  Callout,
  Checkbox,
  Field,
  FieldGrid,
  Metric,
  MetricGrid,
  ProgressBar,
  type CalloutTone,
  type FieldProps,
} from "../theme/fields";
import { Badge, Button, NumericField, SelectField } from "../theme/primitives";
import { color, font, fontSize, lineHeight, space, text } from "../theme/tokens";
import { DisclosureSection, EmptyPanelState, NavRail, WorkspacePanel } from "../theme/workspace";

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
  const stageDisabled = busy || stageAvailability.state !== "available";

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

  if (!asset) {
    return (
      <WorkspacePanel
        title="Model"
        subtitle="Production preparation"
        icon={MODEL_ICON}
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

  const railItems = STAGES.map((item) => {
    const state = effectiveAvailability[item.id].state;
    return {
      id: item.id,
      label: item.label,
      icon: <Icon name={item.icon} size="md" />,
      // An unsupported stage says so in WORDS on hover, and marks itself with a glyph rather than a
      // colour. It used to carry the literal string "--", which names nothing and reads as a
      // rendering fault — `<ux_quality>` 4, plain language, in the one place a user meets first.
      tooltip: state === "unsupported" ? effectiveAvailability[item.id].reason : item.guidance,
      badge: state === "unsupported" ? <Icon name="blocked" size="sm" /> : undefined,
    };
  });

  // ONE declaration of every stage's verb, rendered in the PANEL FOOTER and never inside the scroll
  // region. This is the structural half of the redesign and `model-inspect-evidence` is the evidence
  // for it: the shipped panel painted "Inspect again" 127px BELOW a 520px dock, so the verb of the
  // whole workspace could only be reached by scrolling past the evidence it acts on.
  const actions = stageActions({
    stage,
    assetId: asset.id,
    report,
    availability: stageAvailability,
    collisionAvailability,
    repair,
    optimization,
    uv,
    material,
    bake,
    exportConfig,
    run,
  });

  return (
    <WorkspacePanel
      title="Model"
      subtitle={`${asset.name}${asset.source ? ` / ${asset.source}` : ""}`}
      icon={MODEL_ICON}
      data-testid="asset-lab"
      busy={busy}
      // The split owns its own scrolling — the rail must stay put while the stage scrolls past it.
      scroll={false}
      actions={
        report?.resultMetrics && onResetResult ? (
          <Button variant="ghost" compact onClick={onResetResult} disabled={busy} data-testid="asset-lab-reset">
            Show source metrics
          </Button>
        ) : undefined
      }
      footer={
        <>
          <span style={{ display: "inline-flex", alignItems: "center", gap: space.sm, color: color.text.muted, font: font.ui, fontSize: fontSize.micro }}>
            <Icon name="lock" size="sm" />
            Source protected — every operation creates or validates a derivative
          </span>
          <div className="mtk-action-bar">
            <OperationFeedback state={effectiveOperation} onCancel={onCancel} />
            {actions.map((action) => (
              <PrimaryAction key={action.testId} busy={busy} {...action} />
            ))}
          </div>
        </>
      }
    >
      <div className="mtk-split">
        <NavRail
          id="asset-lab-stages"
          data-testid="asset-lab-stages"
          label="Asset production stages"
          items={railItems}
          activeId={stage}
          onChange={selectStage}
          panelIdPrefix="asset-lab-"
        />
        <div
          className="mtk-split__main"
          id={`asset-lab-${stage}`}
          role="tabpanel"
          aria-labelledby={`asset-lab-stages-${stage}-tab`}
        >
          <StageIntro stage={currentStage} availability={stageAvailability} asset={asset} />

          {stage === "inspect" && <InspectStage report={report} />}
          {stage === "repair" && <RepairStage value={repair} onChange={setRepair} report={report} disabled={stageDisabled} reason={stageAvailability.reason} />}
          {stage === "optimize" && <OptimizeStage value={optimization} onChange={setOptimization} report={report} disabled={stageDisabled} reason={stageAvailability.reason} />}
          {stage === "uv" && (
            <UvStage
              value={uv}
              onChange={setUv}
              material={material}
              onMaterialChange={setMaterial}
              report={report}
              disabled={stageDisabled}
              reason={stageAvailability.reason}
            />
          )}
          {stage === "bake" && (
            <BakeStage
              value={bake}
              onChange={setBake}
              report={report}
              disabled={stageDisabled}
              reason={stageAvailability.reason}
              sources={bakeSources}
              targetId={asset.id}
            />
          )}
          {stage === "validate" && <ValidateStage report={report} />}
          {stage === "export" && <ExportStage value={exportConfig} onChange={setExportConfig} disabled={stageDisabled} reason={stageAvailability.reason} />}
        </div>
      </div>
    </WorkspacePanel>
  );
}

interface StageAction {
  label: string;
  busyLabel: string;
  disabled: boolean;
  reason: string;
  onClick: () => void;
  testId: string;
  variant?: "primary" | "secondary";
}

/** Every stage's verb(s) in one table, so the footer never has to know which stage it is showing. */
function stageActions({
  stage,
  assetId,
  report,
  availability,
  collisionAvailability,
  repair,
  optimization,
  uv,
  material,
  bake,
  exportConfig,
  run,
}: {
  stage: AssetLabStage;
  assetId: string;
  report: AssetLabReportView | null;
  availability: AssetLabAvailability;
  collisionAvailability: AssetLabAvailability;
  repair: AssetRepairConfig;
  optimization: AssetOptimizationConfig;
  uv: AssetUvConfig;
  material: AssetMaterialConfig;
  bake: AssetBakeConfig;
  exportConfig: AssetExportConfig;
  run: (request: AssetLabAction, successStage: AssetLabStage) => void;
}): StageAction[] {
  const blocked = availability.state !== "available";
  switch (stage) {
    case "inspect":
      return [{
        label: report ? "Inspect again" : "Inspect asset",
        busyLabel: "Inspecting...",
        disabled: blocked,
        reason: availability.reason,
        testId: "asset-lab-inspect",
        onClick: () => void run({ action: "inspect", assetId }, "inspect"),
      }];
    case "repair":
      return [{
        label: "Create repaired derivative",
        busyLabel: "Repairing...",
        disabled: blocked,
        reason: availability.reason,
        testId: "asset-lab-repair",
        onClick: () => void run({ action: "repair", assetId, config: repair }, "repair"),
      }];
    case "optimize":
      return [{
        label: "Create optimized derivative",
        busyLabel: "Optimizing...",
        disabled: blocked,
        reason: availability.reason,
        testId: "asset-lab-optimize",
        onClick: () => void run({ action: "optimize", assetId, config: optimization }, "optimize"),
      }];
    case "uv":
      return [
        {
          label: uv.mode === "chart" ? "Unwrap & pack derivative" : "Create UV derivative",
          busyLabel: "Generating UV0...",
          disabled: blocked,
          reason: availability.reason,
          testId: "asset-lab-uv",
          onClick: () => void run({ action: "generateUv", assetId, config: uv }, "uv"),
        },
        {
          label: "Create material derivative",
          busyLabel: "Applying material...",
          disabled: blocked,
          reason: availability.reason,
          testId: "asset-lab-material",
          variant: "secondary",
          onClick: () => void run({ action: "applyMaterial", assetId, config: material }, "uv"),
        },
      ];
    case "bake":
      return [{
        label: "Bake maps to derivative",
        busyLabel: "Projecting & rasterizing...",
        disabled: blocked || bake.maps.length === 0 || bake.highSourceIds.length === 0,
        reason: bake.highSourceIds.length === 0 ? "Choose at least one high-detail source." : availability.reason,
        testId: "asset-lab-bake",
        onClick: () => void run({ action: "bake", assetId, config: bake }, "bake"),
      }];
    case "validate":
      return [
        {
          label: "Validate derivative",
          busyLabel: "Validating...",
          disabled: blocked,
          reason: availability.reason,
          testId: "asset-lab-validate",
          onClick: () => void run({ action: "validate", assetId }, "validate"),
        },
        {
          label: "Generate convex-hull collision",
          busyLabel: "Building collision...",
          disabled: collisionAvailability.state !== "available",
          reason: collisionAvailability.reason,
          testId: "asset-lab-collision",
          variant: "secondary",
          onClick: () => void run({ action: "generateCollision", assetId }, "validate"),
        },
      ];
    case "export":
      return [{
        label: exportConfig.scope === "scene" ? "Export complete scene" : "Export selected derivative",
        busyLabel: "Exporting...",
        disabled: blocked,
        reason: availability.reason,
        testId: "asset-lab-export",
        onClick: () => void run({ action: "export", assetId, config: exportConfig }, "export"),
      }];
  }
}

/**
 * The stage's name, what it is for, and — only when it is refusing — why.
 *
 * There is no "Ready" badge any more. It sat on six of seven stages at all times, which is a badge on
 * every row: the constitution spends accent *"only where interaction requires attention"*, and a stage
 * that is ready requires none. Readiness is already stated by the footer's action being live; what
 * needed saying out loud was the opposite case, and that is the callout.
 */
function StageIntro({
  stage,
  availability,
  asset,
}: {
  stage: (typeof STAGES)[number];
  availability: AssetLabAvailability;
  asset: AssetLabSelection;
}) {
  return (
    <header style={{ display: "grid", gap: space.md }}>
      <div style={{ display: "flex", flexWrap: "wrap", alignItems: "baseline", gap: `${space.xs}px ${space.lg}px` }}>
        <h3 style={{ ...text.panelTitle, margin: 0 }}>{stage.label}</h3>
        <span style={{ font: font.mono, fontSize: fontSize.micro, color: color.text.muted }}>
          {asset.revision ?? asset.id}
        </span>
      </div>
      <p style={{ margin: 0, font: font.ui, fontSize: fontSize.body, lineHeight: lineHeight.body, color: color.text.secondary }}>
        {stage.guidance}
      </p>
      {availability.state !== "available" && (
        <Callout
          tone="warn"
          role="note"
          id={`asset-lab-${stage.id}-reason`}
          data-testid="asset-lab-stage-reason"
        >
          {availability.reason}
        </Callout>
      )}
    </header>
  );
}

function InspectStage({ report }: { report: AssetLabReportView | null }) {
  return report
    ? <ReportEvidence report={report} />
    : <Callout tone="info" role="note">No audit has been run for this asset yet.</Callout>;
}

function RepairStage({
  value,
  onChange,
  report,
  disabled,
  reason,
}: {
  value: AssetRepairConfig;
  onChange: (value: AssetRepairConfig) => void;
  report: AssetLabReportView | null;
  disabled: boolean;
  reason: string;
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
      <FieldGrid>
        <PresetField
          label="Repair preset"
          value={value.preset}
          disabled={disabled}
          reason={reason}
          onChange={(next) => choosePreset(next as RepairPreset)}
          options={[["safe", "Safe"], ["standard", "Standard"], ["thorough", "Thorough"]]}
          help={REPAIR_PRESETS[value.preset].note}
          span="wide"
        />
      </FieldGrid>
      <DisclosureSection title="Advanced repair controls" summary="Thresholds & topology" defaultOpen={false}>
        <FieldGrid>
          <NumericControl
            label="Weld threshold"
            help="Absolute distance in the asset's local units. Attribute seams remain split."
            value={value.weldThreshold}
            min={0}
            step={0.000001}
            disabled={disabled} reason={reason}
            onCommit={(weldThreshold) => onChange({ ...value, weldThreshold })}
          />
          <NumericControl
            label="Remove components smaller than"
            help="Zero keeps every connected shell."
            value={value.removeComponentsSmallerThanTriangles}
            min={0}
            step={1}
            integer
            unit="triangles"
            disabled={disabled} reason={reason}
            onCommit={(removeComponentsSmallerThanTriangles) =>
              onChange({ ...value, removeComponentsSmallerThanTriangles })
            }
          />
          <CheckGroup label="Cleanup" span="full">
            <Checkbox label="Preserve UV and normal seams" checked={value.preserveAttributeSeams} disabled={disabled} disabledReason={reason} onChange={(preserveAttributeSeams) => onChange({ ...value, preserveAttributeSeams })} />
            <Checkbox label="Remove degenerate faces" checked={value.removeDegenerateTriangles} disabled={disabled} disabledReason={reason} onChange={(removeDegenerateTriangles) => onChange({ ...value, removeDegenerateTriangles })} />
            <Checkbox label="Remove duplicate faces" checked={value.removeDuplicateTriangles} disabled={disabled} disabledReason={reason} onChange={(removeDuplicateTriangles) => onChange({ ...value, removeDuplicateTriangles })} />
            <Checkbox label="Remove isolated vertices" checked={value.removeIsolatedVertices} disabled={disabled} disabledReason={reason} onChange={(removeIsolatedVertices) => onChange({ ...value, removeIsolatedVertices })} />
            <Checkbox label="Repair reliable winding" checked={value.repairWinding} disabled={disabled} disabledReason={reason} onChange={(repairWinding) => onChange({ ...value, repairWinding })} />
          </CheckGroup>
        </FieldGrid>
      </DisclosureSection>
      {report && <ReportEvidence report={report} compact />}
      <SourceSafety />
    </>
  );
}

function OptimizeStage({
  value,
  onChange,
  report,
  disabled,
  reason,
}: {
  value: AssetOptimizationConfig;
  onChange: (value: AssetOptimizationConfig) => void;
  report: AssetLabReportView | null;
  disabled: boolean;
  reason: string;
}) {
  function choosePreset(preset: OptimizationPreset) {
    onChange({ ...value, preset, targetRatio: OPTIMIZATION_PRESETS[preset].ratio });
  }
  return (
    <>
      <FieldGrid>
        <PresetField
          label="Optimization preset"
          value={value.preset}
          disabled={disabled}
          reason={reason}
          onChange={(next) => choosePreset(next as OptimizationPreset)}
          options={[
            ["draft", "Draft"], ["balanced", "Balanced"], ["highQuality", "High quality"],
            ["mobile", "Mobile"], ["web", "Web"], ["desktop", "Desktop"], ["cinematic", "Cinematic"],
          ]}
          help={`${OPTIMIZATION_PRESETS[value.preset].note} Target ${(value.targetRatio * 100).toFixed(0)}% of source triangles.`}
          span="wide"
        />
      </FieldGrid>
      <DisclosureSection title="Advanced optimization controls" summary="Target quality" defaultOpen={false}>
        <FieldGrid>
          <NumericControl
            label="Target ratio"
            help="QEM aims for this triangle ratio while locking borders, UV seams, hard normals, material boundaries, and skin influence changes."
            value={value.targetRatio}
            min={0.01}
            max={0.99}
            step={0.01}
            disabled={disabled} reason={reason}
            span="wide"
            onCommit={(targetRatio) => onChange({ ...value, targetRatio })}
          />
        </FieldGrid>
      </DisclosureSection>
      <Callout role="note">
        Attribute-aware QEM preserves textured and rigged payloads, regenerates MikkTSpace tangents, and
        reports when semantic locks prevent the requested reduction.
      </Callout>
      {report && <ReportEvidence report={report} compact />}
      <SourceSafety />
    </>
  );
}

function UvStage({
  value,
  onChange,
  material,
  onMaterialChange,
  report,
  disabled,
  reason,
}: {
  value: AssetUvConfig;
  onChange: (value: AssetUvConfig) => void;
  material: AssetMaterialConfig;
  onMaterialChange: (value: AssetMaterialConfig) => void;
  report: AssetLabReportView | null;
  disabled: boolean;
  reason: string;
}) {
  return (
    <>
      <FieldGrid>
        <PresetField
          label="UV method"
          value={value.mode}
          disabled={disabled}
          reason={reason}
          onChange={(mode) => onChange({ ...value, mode: mode as AssetUvConfig["mode"] })}
          options={[["chart", "Chart unwrap (recommended)"], ["planarWhenAbsent", "Planar when missing"], ["replaceIncompleteWithPlanar", "Replace incomplete with planar"]]}
          help={value.mode === "chart"
            ? "xatlas segments and packs a non-overlapping square atlas, then MikkTSpace regenerates the tangent basis."
            : "Legacy dominant-plane projection is useful only for simple, mostly flat props."}
          span="wide"
        />
        {value.mode === "chart" && (
          <>
            <PresetField
              label="Atlas resolution"
              value={String(value.resolution)}
              disabled={disabled}
              reason={reason}
              onChange={(resolution) => onChange({ ...value, resolution: Number(resolution) as AssetUvConfig["resolution"] })}
              options={[["512", "512 px"], ["1024", "1K"], ["2048", "2K"], ["4096", "4K"]]}
              help="The packed rectangle is uniformly fitted into this exact square page without distorting charts."
            />
            <NumericControl
              label="Chart padding"
              help="Empty pixels reserved between packed islands for filtering and mipmaps."
              value={value.paddingPx}
              min={2}
              max={64}
              step={1}
              integer
              unit="px"
              disabled={disabled} reason={reason}
              onCommit={(paddingPx) => onChange({ ...value, paddingPx })}
            />
            {value.texelsPerUnit != null && (
              <NumericControl
                label="Texels per unit"
                help="Exact requested density before the final single-page fit."
                value={value.texelsPerUnit}
                min={1}
                max={8192}
                step={1}
                integer
                unit="px/unit"
                disabled={disabled} reason={reason}
                onCommit={(texelsPerUnit) => onChange({ ...value, texelsPerUnit })}
              />
            )}
          </>
        )}
        <PresetField
          label="Material finish"
          value={material.preset}
          disabled={disabled}
          reason={reason}
          onChange={(preset) => onMaterialChange({ preset: preset as AssetMaterialPreset })}
          options={[
            ["studio-paint", "Studio paint"],
            ["brushed-metal", "Brushed metal"],
            ["matte-clay", "Matte clay"],
            ["technical-plastic", "Technical plastic"],
          ]}
          help="Applies deterministic metallic-roughness factors to a new derivative. Existing texture pixels stay intact."
          span="wide"
        />
        <CheckGroup label="After generation" span="full">
          {value.mode === "chart" && (
            <>
              <Checkbox
                label="Automatic texel density"
                description="Fit the available atlas while preserving relative surface area."
                checked={value.texelsPerUnit == null}
                disabled={disabled} disabledReason={reason}
                onChange={(automatic) => onChange({ ...value, texelsPerUnit: automatic ? null : 256 })}
              />
              <Checkbox
                label="Replace UV0 on textured materials"
                description="Enable only when you will rebake every bound texture; otherwise existing artwork would no longer align."
                checked={value.replaceTexturedUv0}
                disabled={disabled} disabledReason={reason}
                onChange={(replaceTexturedUv0) => onChange({ ...value, replaceTexturedUv0 })}
              />
            </>
          )}
          <Checkbox
            label="Inspect range and overlap after generation"
            checked={value.inspectAfterGeneration}
            disabled={disabled} disabledReason={reason}
            onChange={(inspectAfterGeneration) => onChange({ ...value, inspectAfterGeneration })}
          />
        </CheckGroup>
      </FieldGrid>
      {report && <ReportEvidence report={report} compact />}
      <SourceSafety />
    </>
  );
}

function BakeStage({
  value,
  onChange,
  report,
  disabled,
  reason,
  sources,
  targetId,
}: {
  value: AssetBakeConfig;
  onChange: (value: AssetBakeConfig) => void;
  report: AssetLabReportView | null;
  disabled: boolean;
  reason: string;
  sources: readonly AssetLabSelection[];
  targetId: string;
}) {
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
      <FieldGrid>
        <PresetField
          label="Texture resolution"
          value={String(value.resolution)}
          disabled={disabled}
          reason={reason}
          onChange={(resolution) => onChange({ ...value, resolution: Number(resolution) as AssetBakeConfig["resolution"] })}
          options={[["512", "512 px"], ["1024", "1K"], ["2048", "2K"], ["4096", "4K (high memory)"]]}
          help="1K is the responsive preview default. Higher resolutions take proportionally longer."
        />
        <PresetField
          label="Source alignment"
          value={value.alignmentPolicy}
          disabled={disabled}
          reason={reason}
          onChange={(alignmentPolicy) => onChange({ ...value, alignmentPolicy: alignmentPolicy as AssetBakeConfig["alignmentPolicy"] })}
          options={[["autoRelated", "Auto-align related assets (recommended)"], ["worldSpace", "Preserve scene positions"]]}
          help={value.alignmentPolicy === "autoRelated"
            ? "Removes only the editor's side-by-side review offset. Unrelated sources keep their authored world transforms."
            : "Uses every source's exact scene transform. Choose this for scans or meshes already registered in world space."}
          span="wide"
        />
        <CheckGroup label="Maps to bake">
          <Checkbox label="Normal" checked={value.maps.includes("normal")} disabled={disabled} disabledReason={reason} onChange={(checked) => toggleMap("normal", checked)} />
          <Checkbox label="Ambient occlusion" checked={value.maps.includes("ambientOcclusion")} disabled={disabled} disabledReason={reason} onChange={(checked) => toggleMap("ambientOcclusion", checked)} />
          <Checkbox label="Signed curvature" checked={value.maps.includes("curvature")} disabled={disabled} disabledReason={reason} onChange={(checked) => toggleMap("curvature", checked)} />
        </CheckGroup>
        <CheckGroup label="High-detail sources" span="wide">
          {sources.length === 0
            ? <Callout role="note">No other resolved mesh objects are available in this scene.</Callout>
            : sources.map((source) => (
              <Checkbox
                key={source.id}
                label={`${source.name}${source.id === targetId ? " (selected target / self-bake)" : ""}`}
                checked={value.highSourceIds.includes(source.id)}
                disabled={disabled} disabledReason={reason}
                description={source.id === targetId
                  ? "Useful for AO and curvature; choose another mesh for a true high-to-low projection."
                  : value.alignmentPolicy === "autoRelated"
                    ? "Related derivatives auto-align to remove review offsets."
                    : "Uses this object's exact world transform in the shared bake space."}
                onChange={(checked) => toggleSource(source.id, checked)}
              />
            ))}
        </CheckGroup>
      </FieldGrid>
      <DisclosureSection title="Advanced projection controls" summary="Cage, rays & curvature" defaultOpen={false}>
        <FieldGrid>
          <NumericControl label="Cage distance" help="Search distance on either side of the low surface, in shared scene units." value={value.cageDistance} min={0.0001} max={1000} step={0.01} disabled={disabled} reason={reason} onCommit={(cageDistance) => onChange({ ...value, cageDistance })} />
          <NumericControl label="AO distance" help="Maximum hemisphere ray length, in shared scene units." value={value.aoDistance} min={0.0001} max={1000} step={0.01} disabled={disabled} reason={reason} onCommit={(aoDistance) => onChange({ ...value, aoDistance })} />
          <NumericControl label="Curvature response" help="Maps signed mean curvature into the texture range; increase for subtler large-radius surfaces." value={value.curvatureScale} min={0.0001} max={1000} step={0.01} disabled={disabled} reason={reason} onCommit={(curvatureScale) => onChange({ ...value, curvatureScale })} />
          <PresetField label="AO quality" value={String(value.aoSamples)} disabled={disabled} reason={reason} onChange={(samples) => onChange({ ...value, aoSamples: Number(samples) as AssetBakeConfig["aoSamples"] })} options={[["8", "Draft / 8 rays"], ["16", "Preview / 16 rays"], ["32", "Production / 32 rays"], ["64", "Hero / 64 rays"]]} help="Uses a deterministic cosine-weighted Hammersley sequence." />
          <PresetField label="Projection quality floor" value={String(value.minProjectionHitRatio)} disabled={disabled} reason={reason} onChange={(ratio) => onChange({ ...value, minProjectionHitRatio: Number(ratio) })} options={[["0.75", "75% / permissive"], ["0.9", "90% / production default"], ["0.98", "98% / strict"], ["1", "100% / exact coverage"]]} help="The bake fails clearly below this covered-texel hit ratio; blank or mostly-missed maps are never published." span="wide" />
        </FieldGrid>
      </DisclosureSection>
      <Callout id="asset-lab-bake-limit" role="note">
        Every requested channel, target UV, source transform, projection hit ratio, material binding and
        output dimension is validated. Partial map output is treated as an error.
      </Callout>
      {report && <ReportEvidence report={report} compact />}
      <SourceSafety />
    </>
  );
}

function ValidateStage({ report }: { report: AssetLabReportView | null }) {
  return report
    ? <ReportEvidence report={report} />
    : <Callout tone="info" role="note">Validation needs an inspection baseline.</Callout>;
}

function ExportStage({ value, disabled, reason, onChange }: { value: AssetExportConfig; disabled: boolean; reason: string; onChange: (value: AssetExportConfig) => void }) {
  return (
    <>
      <FieldGrid>
        <PresetField
          label="Export scope"
          value={value.scope}
          disabled={disabled}
          reason={reason}
          onChange={(scope) => onChange({ scope: scope as AssetExportConfig["scope"], format: scope === "asset" ? "glb" : value.format })}
          options={[["scene", "Complete scene (recommended)"], ["asset", "Selected derivative only"]]}
          help={value.scope === "scene"
            ? "Keeps hierarchy, reusable mesh instances, skins and representable animation."
            : "Writes only the selected derivative as one portable asset."}
          span="wide"
        />
        <PresetField
          label="Export format"
          value={value.format}
          disabled={disabled}
          reason={reason}
          onChange={(format) => onChange({ ...value, format: format as AssetExportConfig["format"] })}
          options={value.scope === "scene"
            ? [["glb", "GLB / self-contained scene"], ["usda", "USDA / readable hierarchy"]]
            : [["glb", "GLB / embedded textures"]]}
          help={value.format === "glb"
            ? "Best portable result: one binary file with geometry, materials and textures."
            : "Deterministic ASCII USD for hierarchy and technical interchange; texture payloads are reported as omitted."}
          span="wide"
        />
      </FieldGrid>
      <DisclosureSection title="What will be preserved?" summary="Export fidelity" defaultOpen={false}>
        <Callout role="note">
          GLB preserves the scene tree, shared meshes, embedded RGBA textures, imported skins and standard
          transform animation. USDA preserves the authored hierarchy, transforms, skin metadata and typed
          animation in readable form, but it is not binary USDC or packaged USDZ. After export, the editor
          reports every preserved, converted or omitted feature.
        </Callout>
      </DisclosureSection>
      <Callout role="note">Export writes a new file and never replaces source assets or scene history.</Callout>
    </>
  );
}

function ReportEvidence({ report, compact = false }: { report: AssetLabReportView; compact?: boolean }) {
  return (
    <section style={{ display: "grid", gap: space.lg, minWidth: 0 }} aria-label="Asset evidence">
      {/* The RESULT, in the primary colour — the stage guidance above it is static help in the
          secondary one. Set to the same colour they read as one four-line paragraph, which is what
          the first capture of this layout showed. */}
      <p style={{ margin: 0, font: font.ui, fontSize: fontSize.body, lineHeight: lineHeight.body, color: color.text.primary }}>
        {report.summary}
      </p>
      {report.bakeEvidence && <BakeEvidence evidence={report.bakeEvidence} />}
      <MetricComparison source={report.sourceMetrics} result={report.resultMetrics} />
      <IssueList issues={report.issues} compact={compact} />
      {report.warnings?.map((warning) => (
        <Callout tone="warn" role="note" key={warning}>{warning}</Callout>
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
      aria-label="Bake projection evidence"
      data-testid="asset-lab-bake-evidence"
      data-projected-texels={evidence.projectedTexels}
      data-covered-texels={evidence.coveredTexels}
      data-projection-hit-ratio={evidence.projectionHitRatio}
      data-required-hit-ratio={evidence.requiredHitRatio}
      data-alignment-policy={evidence.alignmentPolicy}
      data-auto-aligned-sources={evidence.autoAlignedSources}
      data-world-space-sources={evidence.worldSpaceSources}
      style={{ display: "grid", gap: space.md, minWidth: 0 }}
    >
      <Callout tone={passed ? "success" : "danger"} title={`Projection quality ${hitPercent}%`}>
        {passed ? "Passed" : "Rejected"} / minimum {requiredPercent}%
      </Callout>
      <MetricGrid>
        <Metric
          label="Projected texels"
          value={evidence.projectedTexels.toLocaleString("en-GB")}
          description={`of ${evidence.coveredTexels.toLocaleString("en-GB")} covered`}
        />
        <Metric
          label="Atlas coverage"
          value={`${coveragePercent}%`}
          description={`${evidence.charts || "Unfinished"} UV charts / ${evidence.dilatedTexels.toLocaleString("en-GB")} padded`}
        />
        <Metric
          label="Source registration"
          value={evidence.alignmentPolicy === "autoRelated" ? "Automatic" : "World space"}
          description={`${evidence.autoAlignedSources} related aligned / ${evidence.worldSpaceSources} world-space`}
        />
      </MetricGrid>
      <p style={{ margin: 0, font: font.ui, fontSize: fontSize.micro, color: color.text.muted }}>
        {evidence.projectedTexels.toLocaleString("en-GB")} of {evidence.coveredTexels.toLocaleString("en-GB")} covered
        texels / {evidence.charts || "Unfinished"} UV charts / {evidence.autoAlignedSources} related aligned
        / {evidence.worldSpaceSources} world-space
      </p>
    </section>
  );
}

function MetricComparison({ source, result }: { source: readonly AssetLabMetric[]; result?: readonly AssetLabMetric[] }) {
  const resultById = new Map(result?.map((metric) => [metric.id, metric]));
  return (
    <MetricGrid data-testid="asset-lab-metrics">
      {source.map((before) => {
        const after = resultById.get(before.id);
        return (
          <Metric
            key={before.id}
            label={before.label}
            value={formatMetric(before)}
            after={after ? formatMetric(after) : undefined}
            description={before.description}
          />
        );
      })}
    </MetricGrid>
  );
}

function formatMetric(metric: AssetLabMetric): string {
  const value = typeof metric.value === "number" ? metric.value.toLocaleString("en-GB") : metric.value;
  return `${value}${metric.unit ? ` ${metric.unit}` : ""}`;
}

const ISSUE_TONE: Record<AssetLabIssueSeverity, CalloutTone> = {
  critical: "danger",
  error: "danger",
  warning: "warn",
  info: "neutral",
  pass: "success",
};

/**
 * Findings as callouts, in as many columns as the surface has room for.
 *
 * The shipped list gave every finding a full-width box with a coloured 1px border AND a coloured
 * status pill — two statements of the same severity, drawn as an outline the constitution's "minimal
 * borders / avoid hard borders" line rules out, on a row 1240px wide holding one short sentence. One
 * tinted surface with a glyph says it once, and three columns of them replace four stacked bands.
 */
function IssueList({ issues, compact }: { issues: readonly AssetLabIssue[]; compact: boolean }) {
  if (issues.length === 0) {
    return <Callout tone="success" role="status">No measured issues</Callout>;
  }
  return (
    <ul
      aria-label="Measured issues"
      style={{
        display: "grid",
        gridTemplateColumns: "repeat(auto-fit, minmax(280px, 1fr))",
        gap: space.md,
        margin: 0,
        padding: 0,
        listStyle: "none",
        maxHeight: compact ? 240 : undefined,
        overflow: compact ? "auto" : undefined,
        minWidth: 0,
      }}
    >
      {issues.map((issue) => (
        <li key={issue.id} data-severity={issue.severity} data-status={issue.status ?? "open"} style={{ minWidth: 0 }}>
          <Callout
            tone={ISSUE_TONE[issue.severity]}
            title={
              <span style={{ display: "flex", alignItems: "baseline", justifyContent: "space-between", gap: space.md }}>
                <span style={{ minWidth: 0 }}>{issue.title}{issue.count != null ? ` / ${issue.count}` : ""}</span>
                <Badge tone={issue.status === "fixed" ? "success" : issue.severity === "warning" || issue.severity === "critical" || issue.severity === "error" ? "warn" : "neutral"}>
                  {issue.status ?? issue.severity}
                </Badge>
              </span>
            }
          >
            {issue.detail}
          </Callout>
        </li>
      ))}
    </ul>
  );
}

function PresetField({ label, value, options, help, disabled, reason, onChange, span }: {
  label: string;
  value: string;
  options: readonly (readonly [string, string])[];
  help: string;
  disabled: boolean;
  /** Why this control is refusing, in the user's words. */
  reason: string;
  onChange: (value: string) => void;
  span?: FieldProps["span"];
}) {
  const id = useId();
  return (
    <Field label={label} htmlFor={id} help={help} span={span} disabled={disabled}>
      <SelectField
        id={id}
        aria-label={label}
        value={value}
        disabled={disabled}
        // A refusing control must say why (`<ux_quality>` 4/6) — the same sentence the stage's own
        // callout is showing, on the control a pointer is actually over. The shots harness's R9 check
        // fails a disabled control that carries neither a title nor a described-by.
        title={disabled ? reason : undefined}
        onChange={(event) => onChange(event.target.value)}
      >
        {options.map(([option, optionLabel]) => <option value={option} key={option}>{optionLabel}</option>)}
      </SelectField>
    </Field>
  );
}

function NumericControl({ label, help, unit, span, reason, ...props }: {
  label: string;
  help: string;
  unit?: string;
  span?: FieldProps["span"];
  /** Why this control is refusing, in the user's words. */
  reason: string;
} & Omit<React.ComponentProps<typeof NumericField>, "ariaLabel" | "id" | "title">) {
  const id = useId();
  return (
    <Field label={label} htmlFor={id} help={help} unit={unit} span={span} disabled={props.disabled}>
      <NumericField {...props} id={id} ariaLabel={label} title={props.disabled ? reason : undefined} />
    </Field>
  );
}

/** A named set of checkboxes: one legend, one column of rows, at the field grid's rhythm. */
function CheckGroup({ label, children, span = "one" }: { label: string; children: ReactNode; span?: FieldProps["span"] }) {
  return (
    <fieldset
      className={["mtk-field", span === "wide" && "mtk-field--wide", span === "full" && "mtk-field--full"].filter(Boolean).join(" ")}
      style={{ margin: 0, padding: 0, border: "none", minWidth: 0 }}
    >
      <legend className="mtk-field__label" style={{ padding: 0 }}>{label}</legend>
      <div style={{ display: "grid", gap: space.xxs, minWidth: 0 }}>{children}</div>
    </fieldset>
  );
}

function SourceSafety() {
  return (
    <Callout tone="success" title="Source stays unchanged">
      Review the derivative before using it in the scene.
    </Callout>
  );
}

function PrimaryAction({ label, busyLabel, busy, disabled, reason, onClick, testId, variant = "primary" }: StageAction & { busy: boolean }) {
  return (
    <span style={{ display: "inline-flex", alignItems: "center", gap: space.md, minWidth: 0 }}>
      {/* The reason is VISIBLE, not only a tooltip. `<ux_quality>` 6: an enabled button always does
          something or says why it can't — and the words have to be readable without a pointer. */}
      {disabled && (
        <small
          id={`${testId}-reason`}
          style={{ font: font.ui, fontSize: fontSize.micro, lineHeight: lineHeight.compact, color: color.text.muted, maxWidth: 260 }}
        >
          {reason}
        </small>
      )}
      <Button
        variant={variant}
        data-testid={testId}
        disabled={busy || disabled}
        disabledReason={reason}
        aria-describedby={disabled ? `${testId}-reason` : undefined}
        onClick={onClick}
      >
        {busy ? busyLabel : label}
      </Button>
    </span>
  );
}

function OperationFeedback({ state, onCancel }: { state: AssetLabOperationState; onCancel?: () => void }) {
  if (state.status === "idle") return null;
  const role = state.status === "error" ? "alert" : "status";
  const progress = state.progress == null ? undefined : Math.max(0, Math.min(1, state.progress));
  const tone = state.status === "error" ? "danger" : state.status === "success" ? "success" : "neutral";
  return (
    <div
      role={role}
      aria-live={state.status === "error" ? "assertive" : "polite"}
      data-testid="asset-lab-operation"
      style={{ display: "flex", alignItems: "center", gap: space.md, minWidth: 0, maxWidth: 420 }}
    >
      <Callout tone={tone} icon={state.status === "busy" ? <Icon name="spinner" size="sm" /> : undefined}>
        <span style={{ display: "grid", gap: space.xs, minWidth: 0 }}>
          <strong style={{ fontWeight: 600 }}>{state.message ?? (state.status === "busy" ? "Working..." : state.status)}</strong>
          {progress != null && <ProgressBar value={progress} label="Operation progress" />}
        </span>
      </Callout>
      {state.status === "busy" && onCancel && <Button variant="secondary" compact onClick={onCancel}>Cancel</Button>}
    </div>
  );
}
