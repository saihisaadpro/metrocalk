import { createElement } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { projectionStore } from "../store/projection";
import { fakeClient } from "../transport/test-client";
import type { AssetLabAuditReport, AssetLabResponse } from "../transport/protocol";
import { AssetLabWorkspace, assetLabView } from "./AssetLabWorkspace";

afterEach(() => {
  cleanup();
  window.localStorage.clear();
  act(() => projectionStore.getState().reset());
});

function fact(state: "available" | "missing" | "unsupported", reason: string) {
  return { state, reason } as const;
}

function audit(over: Partial<AssetLabAuditReport> = {}): AssetLabAuditReport {
  return {
    name: "fixture",
    logicalObjects: 1,
    connectedComponents: 1,
    primitives: 1,
    nonEmptyPrimitives: 1,
    vertices: 120,
    uniquePositionVertices: 120,
    duplicatePositionVertices: 0,
    isolatedVertices: 0,
    indices: 300,
    trailingIndices: 0,
    invalidIndices: 0,
    triangles: 100,
    validTriangles: 100,
    degenerateTriangles: 0,
    duplicateTriangles: 0,
    edges: 180,
    boundaryEdges: 0,
    manifoldEdges: 180,
    nonManifoldEdges: 0,
    invalidPositions: 0,
    drawCalls: 1,
    materials: { slots: 1, usedSlots: 1, unusedSlots: 0, invalidPrimitiveAssignments: 0, invalidTextureReferences: 0 },
    textures: { count: 0, referenced: 0, totalTexels: 0, decodedBytes: 0, invalidLayouts: 0, descriptors: [] },
    normals: { completePrimitives: 1, missingVertices: 0, invalidVectors: 0, zeroLengthVectors: 0, nonUnitVectors: 0 },
    uvs: {
      completePrimitives: 1,
      missingPrimitives: 0,
      partialPrimitives: 0,
      missingVertices: 0,
      invalidCoordinates: 0,
      outOfUnitRangeVertices: 0,
      degenerateUvTriangles: 0,
      overlap: { state: "clear", trianglesConsidered: 100, pairsTested: 4_950, overlappingPairs: 0, pairBudget: 250_000, method: "bounded exact" },
    },
    bounds: { min: [0, 0, 0], max: [1, 2, 3], dimensions: [1, 2, 3] },
    estimatedBytes: { positions: 1_440, normals: 1_440, uvs: 960, indices: 1_200, skin: 0, textures: 0, materialFactors: 32, totalPayload: 5_072 },
    capabilities: {
      cleanup: fact("available", "cleanup ready"),
      planarUvGeneration: fact("available", "UV ready"),
      holeFilling: fact("unsupported", "no hole filler"),
      hiddenGeometryRemoval: fact("unsupported", "no visibility classifier"),
      remeshing: fact("unsupported", "no remesher"),
      lodGeneration: fact("available", "LOD ready"),
      lodData: fact("missing", "no embedded LOD"),
      collisionGeneration: fact("available", "collision planner ready"),
      collisionData: fact("missing", "no collider in mesh"),
      textureBakeInput: fact("missing", "no textures"),
      highToLowTextureBaking: fact("unsupported", "no high-to-low baker"),
    },
    warnings: [],
    ...over,
  };
}

test("adapter maps measured defects into non-colour issue rows and honest availability", () => {
  const measured = audit({
    nonManifoldEdges: 3,
    degenerateTriangles: 2,
    uvs: {
      ...audit().uvs,
      overlap: { ...audit().uvs.overlap, state: "inconclusive", pairsTested: 250_000 },
    },
    capabilities: { ...audit().capabilities, lodGeneration: fact("unsupported", "textured mesh cannot retain UVs") },
  });
  const issues = assetLabView.issues(measured);
  expect(issues.map((issue) => issue.title)).toEqual(expect.arrayContaining(["Non-manifold edges", "Degenerate triangles", "UV overlap check inconclusive"]));
  expect(issues.find((issue) => issue.id === "non-manifold")?.count).toBe(3);
  const availability = assetLabView.deriveAvailability(measured);
  expect(availability.optimize).toEqual({ state: "unsupported", reason: "textured mesh cannot retain UVs" });
  expect(availability.export?.state).toBe("unavailable");
  expect(availability.bake?.state).toBe("unsupported");
});

test("before/after response becomes comparable metric evidence", () => {
  const before = audit();
  const after = audit({ vertices: 65, triangles: 50, validTriangles: 50, indices: 150 });
  const response: AssetLabResponse = {
    ok: true,
    message: "Optimized variant created",
    sourceEntity: "1_2",
    sourceHandle: "mtkasset:source",
    createdEntity: "1_3",
    createdHandle: "mtkasset:result",
    audit: after,
    change: { before, after, changes: ["100 to 50 triangles"], warnings: [] },
    warnings: [],
    exportedPath: null,
    bakeEvidence: {
      resolution: 512,
      requestedMaps: ["normal", "ambientOcclusion", "curvature"],
      sourceCount: 1,
      sourceTriangles: 200,
      charts: 3,
      atlasTexels: 262_144,
      coveredTexels: 100_000,
      projectedTexels: 99_000,
      projectionMisses: 1_000,
      dilatedTexels: 4_000,
      coverageRatio: 100_000 / 262_144,
      projectionHitRatio: 0.99,
      requiredHitRatio: 0.9,
      alignmentPolicy: "autoRelated",
      autoAlignedSources: 1,
      worldSpaceSources: 0,
    },
  };
  const view = assetLabView.responseToView(response);
  expect(view?.sourceMetrics.find((metric) => metric.id === "triangles")?.value).toBe(100);
  expect(view?.resultMetrics?.find((metric) => metric.id === "triangles")?.value).toBe(50);
  expect(view?.issues.some((issue) => issue.status === "fixed" && /100 to 50/.test(issue.detail))).toBe(true);
  expect(view?.bakeEvidence?.projectedTexels).toBe(99_000);
  expect(view?.bakeEvidence?.projectionHitRatio).toBeGreaterThanOrEqual(view?.bakeEvidence?.requiredHitRatio ?? 1);
  expect(view?.bakeEvidence?.alignmentPolicy).toBe("autoRelated");
});

test("valid skinned assets remain eligible for complete-scene export", () => {
  const measured = audit({
    estimatedBytes: { ...audit().estimatedBytes, skin: 4_096, totalPayload: 9_168 },
  });
  expect(assetLabView.deriveAvailability(measured).export).toEqual({
    state: "available",
    reason: "Complete-scene GLB and hierarchy-focused USDA export are available.",
  });
});

test("successful export retains the measured availability instead of falling back to unsupported copy", async () => {
  const measured = audit();
  const assetLabAudit = vi.fn(() => Promise.resolve<AssetLabResponse>({
    ok: true,
    message: "Inspected fixture",
    sourceEntity: "mesh-7",
    sourceHandle: "mtkasset:fixture",
    createdEntity: null,
    createdHandle: null,
    audit: measured,
    change: null,
    warnings: [],
    exportedPath: null,
    bakeEvidence: null,
  }));
  const assetLabExport = vi.fn(() => Promise.resolve<AssetLabResponse>({
    ok: true,
    message: "Exported GLB to fixture.glb",
    sourceEntity: "mesh-7",
    sourceHandle: "mtkasset:fixture",
    createdEntity: null,
    createdHandle: null,
    audit: null,
    change: null,
    warnings: [],
    exportedPath: "fixture.glb",
    bakeEvidence: null,
  }));

  act(() => {
    projectionStore.getState().bulkLoad([{
      id: "mesh-7",
      name: "Valve housing",
      parentId: null,
      components: { MeshRenderer: { mesh: "mtkasset:fixture" } },
    }]);
    projectionStore.getState().select("mesh-7");
  });

  render(createElement(AssetLabWorkspace, {
    client: fakeClient({ assetLabAudit, assetLabExport }),
  }));

  await waitFor(() => expect(screen.getByTestId("asset-lab-metrics").textContent).toContain("100"));
  fireEvent.click(screen.getByRole("tab", { name: /Export/ }));
  fireEvent.change(screen.getByLabelText("Export scope"), { target: { value: "asset" } });

  const exportButton = screen.getByTestId("asset-lab-export") as HTMLButtonElement;
  expect(exportButton.disabled).toBe(false);
  expect(screen.queryByText(/No verified export writer is connected/i)).toBeNull();
  fireEvent.click(exportButton);

  await waitFor(() => expect(assetLabExport).toHaveBeenCalledWith("mesh-7", undefined));
  await waitFor(() => expect(screen.getByTestId("asset-lab-operation").textContent).toContain("Exported GLB to fixture.glb"));
  expect((screen.getByTestId("asset-lab-export") as HTMLButtonElement).disabled).toBe(false);
  expect(screen.queryByText(/No verified export writer is connected/i)).toBeNull();
  fireEvent.click(screen.getByRole("tab", { name: /^Inspect$/ }));
  expect(screen.getByTestId("asset-lab-metrics").textContent).toContain("100");
});
