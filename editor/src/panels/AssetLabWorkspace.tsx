//! Transport adapter for the Asset Lab presentation. It turns the native audit into compact evidence,
//! derives honest stage availability, and owns loading/error/success feedback for long-running work.

import { useEffect, useMemo, useRef, useState } from "react";
import { projectionStore, useDisplayedEntity, useEntityOrder, useSelectedId } from "../store/projection";
import { setStatus } from "../store/ui";
import { pushToast } from "../store/toasts";
import type {
  AssetLabAuditReport,
  AssetLabAvailabilityFact,
  AssetLabResponse,
} from "../transport/protocol";
import type { EditorClient } from "../transport/session";
import {
  AssetLabPanel,
  type AssetLabAction,
  type AssetLabAvailability,
  type AssetLabIssue,
  type AssetLabMetric,
  type AssetLabOperationState,
  type AssetLabReportView,
  type AssetLabStage,
} from "./AssetLabPanel";

export function AssetLabWorkspace({ client }: { client: EditorClient }) {
  const selectedId = useSelectedId();
  const entityOrder = useEntityOrder();
  const entity = useDisplayedEntity(selectedId ?? "");
  const meshHandle = entity?.components.MeshRenderer?.mesh;
  const asset = selectedId && typeof meshHandle === "string" && meshHandle.length > 0
    ? { id: selectedId, name: entity?.name || "Selected mesh", source: "Scene asset", revision: shortHandle(meshHandle) }
    : null;
  const bakeSources = useMemo(() => {
    const displayed = projectionStore.getState().displayed;
    return entityOrder.flatMap((id) => {
      const candidate = displayed[id];
      const handle = candidate?.components.MeshRenderer?.mesh;
      return candidate && typeof handle === "string" && handle.length > 0
        ? [{ id, name: candidate.name || shortHandle(handle), source: shortHandle(handle) }]
        : [];
    });
  }, [entityOrder, selectedId, meshHandle]);
  const [response, setResponse] = useState<AssetLabResponse | null>(null);
  const [operation, setOperation] = useState<AssetLabOperationState>({ status: "idle" });
  const requestRevision = useRef(0);
  const completedDerivativeSelection = useRef<string | null>(null);

  useEffect(() => {
    if (completedDerivativeSelection.current === selectedId) {
      completedDerivativeSelection.current = null;
      return;
    }
    requestRevision.current += 1;
    const revision = requestRevision.current;
    setResponse(null);
    if (!asset) {
      setOperation({ status: "idle" });
      return;
    }
    setOperation({ status: "busy", stage: "inspect", message: "Inspecting selected mesh..." });
    void client.assetLabAudit(asset.id).then((next) => {
      if (requestRevision.current !== revision) return;
      setResponse((current) => next.audit ? next : {
        ...next,
        audit: current?.audit ?? null,
        change: current?.change ?? null,
      });
      setOperation(next.ok
        ? { status: "success", stage: "inspect", message: next.message }
        : { status: "error", stage: "inspect", message: next.message });
    }).catch((cause: unknown) => {
      if (requestRevision.current !== revision) return;
      setOperation({ status: "error", stage: "inspect", message: errorMessage(cause) });
    });
  // Mesh handle is included so a re-import/swap of the selected entity invalidates stale evidence.
  }, [client, selectedId, meshHandle]);

  const report = useMemo(() => responseToView(response), [response]);
  const availability = useMemo(() => deriveAvailability(response?.audit ?? null), [response]);
  const collisionAvailability = useMemo<AssetLabAvailability>(() => {
    if (entity?.components.Collider) {
      const shape = entity.components.Collider.shape;
      return { state: "unavailable", reason: `This scene object already has ${typeof shape === "string" ? shape : "a"} collision.` };
    }
    const fact = response?.audit?.capabilities.collisionGeneration;
    return fact ? factAvailability(fact) : { state: "unavailable", reason: "Inspect the selected mesh first." };
  }, [entity?.components.Collider, response]);

  async function run(action: AssetLabAction) {
    requestRevision.current += 1;
    const revision = requestRevision.current;
    const stage = actionStage(action);
    setOperation({ status: "busy", stage, message: busyMessage(action) });
    try {
      const next = await execute(client, action);
      if (requestRevision.current !== revision) return;
      if (!next.ok) {
        if (next.bakeEvidence) {
          setResponse((current) => ({
            ...next,
            audit: next.audit ?? current?.audit ?? null,
            change: next.change ?? current?.change ?? null,
          }));
        }
        throw new Error(next.message || "Asset Lab could not complete the operation");
      }
      setResponse((current) => action.action === "export" ? {
        ...next,
        // Export replies intentionally carry delivery/fidelity facts rather than repeating the potentially
        // large mesh audit. Keep the last measured asset evidence so a successful export cannot make the
        // same, still-valid writer look unsupported or erase the before/after and bake report.
        audit: next.audit ?? current?.audit ?? null,
        change: next.change ?? current?.change ?? null,
        bakeEvidence: next.bakeEvidence ?? current?.bakeEvidence ?? null,
      } : next);
      setOperation({ status: "success", stage, message: next.message });
      setStatus(next.message);
      pushToast(next.message, action.action === "export" ? "info" : "success");
      if (next.createdEntity && next.createdEntity !== selectedId) {
        completedDerivativeSelection.current = next.createdEntity;
        projectionStore.getState().select(next.createdEntity);
        void client.gizmoSelect(next.createdEntity).catch((error: unknown) =>
          console.error("gizmoSelect failed (Asset Lab derivative selection may be out of sync)", error),
        );
      }
    } catch (cause) {
      if (requestRevision.current !== revision) return;
      const message = errorMessage(cause);
      setOperation({ status: "error", stage, message });
      setStatus(message);
      throw cause;
    }
  }

  function resetResult() {
    const before = response?.change?.before;
    if (!before) return;
    setResponse({
      ...response,
      audit: before,
      change: null,
      createdEntity: null,
      createdHandle: null,
      message: "Showing the source audit",
      warnings: before.warnings,
      bakeEvidence: null,
    });
    setOperation({ status: "idle" });
  }

  return (
    <AssetLabPanel
      asset={asset}
      report={report}
      availability={availability}
      collisionAvailability={collisionAvailability}
      bakeSources={bakeSources}
      operation={operation}
      onRun={run}
      onResetResult={response?.change ? resetResult : undefined}
    />
  );
}

function shortHandle(handle: string): string {
  const suffix = handle.includes(":") ? handle.slice(handle.indexOf(":") + 1) : handle;
  return `Asset ${suffix.slice(0, 8)}`;
}

function errorMessage(cause: unknown): string {
  return cause instanceof Error && cause.message ? cause.message : "Asset Lab operation failed";
}

function actionStage(action: AssetLabAction): AssetLabStage {
  if (action.action === "generateUv" || action.action === "applyMaterial") return "uv";
  if (action.action === "generateCollision") return "validate";
  return action.action;
}

function busyMessage(action: AssetLabAction): string {
  return {
    inspect: "Inspecting selected mesh...",
    repair: "Repairing a source-preserving derivative...",
    optimize: "Evaluating deterministic optimization candidates...",
    generateUv: "Preparing UV0 and validating overlap...",
    applyMaterial: "Applying PBR factors to a new derivative...",
    bake: "Baking requested texture maps...",
    validate: "Validating geometry and asset payloads...",
    generateCollision: "Building an undoable convex-hull collider...",
    export: action.action === "export" && action.config.scope === "scene"
      ? `Writing complete-scene ${action.config.format.toUpperCase()}...`
      : "Writing selected embedded-texture GLB...",
  }[action.action];
}

async function execute(client: EditorClient, action: AssetLabAction): Promise<AssetLabResponse> {
  switch (action.action) {
    case "inspect":
    case "validate":
      return client.assetLabAudit(action.assetId);
    case "repair":
      return client.assetLabProcess(action.assetId, {
        operation: "repair",
        preset: action.config.preset,
        weldThreshold: action.config.weldThreshold,
        removeDegenerateTriangles: action.config.removeDegenerateTriangles,
        removeDuplicateTriangles: action.config.removeDuplicateTriangles,
        removeIsolatedVertices: action.config.removeIsolatedVertices,
        removeComponentsSmallerThanTriangles: action.config.removeComponentsSmallerThanTriangles,
        repairWinding: action.config.repairWinding,
        normalRepair: action.config.normalRepair,
        preserveAttributeSeams: action.config.preserveAttributeSeams,
      });
    case "optimize":
      return client.assetLabProcess(action.assetId, {
        operation: "optimize",
        preset: action.config.preset,
        targetRatio: action.config.targetRatio,
        candidateLevels: action.config.candidateLevels,
        baseFraction: action.config.baseFraction,
        preserveAttributeSeams: true,
      });
    case "generateUv":
      return client.assetLabProcess(action.assetId, {
        operation: "uv",
        preset: action.config.mode,
        resolution: action.config.resolution,
        paddingPx: action.config.paddingPx,
        texelsPerUnit: action.config.texelsPerUnit,
        replaceTexturedUv0: action.config.replaceTexturedUv0,
        preserveAttributeSeams: true,
      });
    case "applyMaterial":
      return client.assetLabProcess(action.assetId, {
        operation: "material",
        preset: action.config.preset,
        preserveAttributeSeams: true,
      });
    case "export":
      if (action.config.scope === "scene") {
        const scene = await client.sceneExport(
          action.config.format,
          (globalThis as typeof globalThis & { __MTK_SCENE_EXPORT_PATH__?: string }).__MTK_SCENE_EXPORT_PATH__,
        );
        return {
          ok: scene.ok,
          message: scene.message,
          sourceEntity: action.assetId,
          sourceHandle: null,
          createdEntity: null,
          createdHandle: null,
          audit: null,
          change: null,
          warnings: scene.fidelity
            .filter((entry) => entry.status !== "preserved")
            .map((entry) => `${entry.status === "omitted" ? "Omitted" : "Converted"}: ${entry.feature} (${entry.count}) — ${entry.detail}`),
          exportedPath: scene.exportedPath,
          bakeEvidence: null,
        };
      }
      return client.assetLabExport(
        action.assetId,
        (globalThis as typeof globalThis & { __MTK_ASSET_LAB_EXPORT_PATH__?: string }).__MTK_ASSET_LAB_EXPORT_PATH__,
      );
    case "generateCollision": {
      const applied = await client.makeStatic(action.assetId);
      if (!applied) throw new Error("Collision generation was rejected; review the Physics panel for details");
      const audited = await client.assetLabAudit(action.assetId);
      return { ...audited, ok: true, message: "Convex-hull collision added — Ctrl/Cmd+Z to undo" };
    }
    case "bake":
      return client.assetLabProcess(action.assetId, {
        operation: "bake",
        resolution: action.config.resolution,
        paddingPx: action.config.paddingPx,
        highSourceIds: [...action.config.highSourceIds],
        maps: [...action.config.maps],
        cageDistance: action.config.cageDistance,
        aoDistance: action.config.aoDistance,
        aoSamples: action.config.aoSamples,
        curvatureScale: action.config.curvatureScale,
        alignmentPolicy: action.config.alignmentPolicy,
        minProjectionHitRatio: action.config.minProjectionHitRatio,
        preserveAttributeSeams: true,
      });
  }
}

function factAvailability(fact: AssetLabAvailabilityFact): AssetLabAvailability {
  return {
    state: fact.state === "available" ? "available" : fact.state === "unsupported" ? "unsupported" : "unavailable",
    reason: fact.reason,
  };
}

function deriveAvailability(audit: AssetLabAuditReport | null): Partial<Record<AssetLabStage, AssetLabAvailability>> {
  if (!audit) return {};
  const exportBlocked = audit.invalidIndices > 0
    || audit.trailingIndices > 0
    || audit.invalidPositions > 0
    || audit.degenerateTriangles > 0
    || audit.normals.invalidVectors > 0
    || audit.normals.zeroLengthVectors > 0
    || audit.normals.nonUnitVectors > 0
    || audit.materials.invalidPrimitiveAssignments > 0
    || audit.materials.invalidTextureReferences > 0
    || audit.textures.invalidLayouts > 0;
  return {
    inspect: { state: "available", reason: "Read-only audit is available for this mesh." },
    repair: factAvailability(audit.capabilities.cleanup),
    optimize: factAvailability(audit.capabilities.lodGeneration),
    uv: factAvailability(audit.capabilities.planarUvGeneration),
    bake: factAvailability(audit.capabilities.highToLowTextureBaking),
    validate: { state: "available", reason: "Re-run the same bounded production checks at any time." },
    export: exportBlocked
      ? { state: "unavailable", reason: "Repair invalid geometry, material, or texture data before export." }
      : { state: "available", reason: "Complete-scene GLB and hierarchy-focused USDA export are available." },
  };
}

function responseToView(response: AssetLabResponse | null): AssetLabReportView | null {
  const audit = response?.audit;
  if (!audit) return null;
  const before = response.change?.before ?? audit;
  const after = response.change?.after;
  const changeIssues: AssetLabIssue[] = response.change?.changes.map((change, index) => ({
    id: `change-${index}`,
    severity: "pass",
    status: "fixed",
    title: "Applied change",
    detail: change,
  })) ?? [];
  return {
    summary: response.message || auditSummary(audit),
    sourceMetrics: metrics(before),
    resultMetrics: after ? metrics(after) : undefined,
    issues: [...issues(audit), ...changeIssues],
    warnings: unique([...(response.warnings ?? []), ...(response.change?.warnings ?? []), ...audit.warnings]),
    bakeEvidence: response.bakeEvidence ?? undefined,
  };
}

function metrics(audit: AssetLabAuditReport): AssetLabMetric[] {
  const dims = audit.bounds?.dimensions;
  return [
    { id: "triangles", label: "Triangles", value: audit.triangles, description: "Indexed triangle-list faces across all primitives." },
    { id: "vertices", label: "Vertices", value: audit.vertices, description: "Render vertices including intentional UV and normal seams." },
    { id: "drawCalls", label: "Draw calls", value: audit.drawCalls, description: "Non-empty material primitives." },
    { id: "components", label: "Connected parts", value: audit.connectedComponents },
    { id: "materials", label: "Materials", value: audit.materials.slots },
    { id: "textures", label: "Textures", value: audit.textures.count },
    { id: "payload", label: "Source payload", value: audit.estimatedBytes.totalPayload / (1024 * 1024), unit: "MiB", description: "Decoded geometry, material, skin, and texture bytes." },
    { id: "bounds", label: "Local size", value: dims ? dims.map((v) => formatNumber(v)).join(" x ") : "Unavailable" },
  ];
}

function formatNumber(value: number): string {
  return Math.abs(value) >= 100 ? value.toFixed(0) : value.toFixed(2).replace(/\.00$/, "");
}

function auditSummary(audit: AssetLabAuditReport): string {
  return `${audit.triangles.toLocaleString("en-GB")} triangles, ${audit.vertices.toLocaleString("en-GB")} vertices, ${audit.drawCalls} draw calls.`;
}

function issues(audit: AssetLabAuditReport): AssetLabIssue[] {
  const out: AssetLabIssue[] = [];
  addIssue(out, audit.invalidIndices + audit.trailingIndices, "invalid-indices", "error", "Invalid triangle indices", "Indices are out of range or do not form complete triangles.");
  addIssue(out, audit.invalidPositions, "invalid-positions", "critical", "Invalid vertex positions", "One or more positions are not finite.");
  addIssue(out, audit.degenerateTriangles, "degenerate", "warning", "Degenerate triangles", "Faces have effectively zero 3D area.");
  addIssue(out, audit.duplicateTriangles, "duplicate-faces", "warning", "Duplicate triangles", "Multiple faces resolve to the same welded topology.");
  addIssue(out, audit.nonManifoldEdges, "non-manifold", "warning", "Non-manifold edges", "Edges have more than two incident faces.");
  addIssue(out, audit.isolatedVertices, "isolated", "warning", "Isolated vertices", "Vertices are not referenced by any valid face.");
  addIssue(out, audit.normals.missingVertices + audit.normals.invalidVectors + audit.normals.zeroLengthVectors, "normals", "warning", "Normal stream needs repair", "Normals are missing, invalid, or zero length.");
  addIssue(out, audit.uvs.missingVertices + audit.uvs.invalidCoordinates, "uv0", "warning", "UV0 is incomplete", "Some render vertices have no finite UV0 coordinate.");
  if (audit.uvs.overlap.state === "detected") {
    addIssue(out, Math.max(1, audit.uvs.overlap.overlappingPairs), "uv-overlap", "warning", "UV overlap detected", "Positive-area overlap was measured within the bounded exact test.");
  } else if (audit.uvs.overlap.state === "inconclusive") {
    out.push({ id: "uv-inconclusive", severity: "info", status: "open", title: "UV overlap check inconclusive", detail: `The ${audit.uvs.overlap.pairBudget.toLocaleString("en-GB")} pair budget was exhausted.` });
  }
  addIssue(out, audit.materials.invalidPrimitiveAssignments, "material-slots", "error", "Invalid material slots", "A primitive references a material slot that does not exist.");
  addIssue(out, audit.materials.invalidTextureReferences + audit.textures.invalidLayouts, "textures", "error", "Invalid texture payload", "Texture references or decoded RGBA dimensions are invalid.");
  return out;
}

function addIssue(
  out: AssetLabIssue[],
  count: number,
  id: string,
  severity: AssetLabIssue["severity"],
  title: string,
  detail: string,
) {
  if (count > 0) out.push({ id, severity, status: "open", title, detail, count });
}

function unique(values: readonly string[]): string[] {
  return [...new Set(values.filter(Boolean))];
}

/** Exported for focused adapter tests without exposing transport details to the presentational panel. */
export const assetLabView = { metrics, issues, deriveAvailability, responseToView };
