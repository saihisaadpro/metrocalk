import { afterEach, expect, test, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import {
  AssetLabPanel,
  type AssetLabAction,
  type AssetLabAvailability,
  type AssetLabReportView,
} from "./AssetLabPanel";

afterEach(() => {
  cleanup();
  window.localStorage.clear();
});

const asset = { id: "mesh-7", name: "Valve housing", source: "Imported GLB", revision: "rev 4" };

const report: AssetLabReportView = {
  summary: "One static mesh inspected. Two topology issues need review.",
  sourceMetrics: [
    { id: "vertices", label: "Vertices", value: 6_500 },
    { id: "triangles", label: "Triangles", value: 12_000 },
    { id: "drawCalls", label: "Draw calls", value: 4 },
  ],
  resultMetrics: [
    { id: "vertices", label: "Vertices", value: 2_900 },
    { id: "triangles", label: "Triangles", value: 4_800 },
    { id: "drawCalls", label: "Draw calls", value: 4 },
  ],
  issues: [
    {
      id: "non-manifold",
      severity: "warning",
      status: "open",
      title: "Non-manifold edges",
      detail: "Three edges have more than two incident faces.",
      count: 3,
    },
    {
      id: "normals",
      severity: "pass",
      status: "fixed",
      title: "Normals",
      detail: "All result normals are finite and unit length.",
    },
  ],
  warnings: ["UV overlap check reached its configured pair budget."],
};

const AVAILABLE: AssetLabAvailability = { state: "available", reason: "Ready for this asset." };

test("a missing selection has an actionable, source-preserving empty state", () => {
  const choose = vi.fn();
  render(<AssetLabPanel asset={null} onRun={vi.fn()} onChooseAsset={choose} />);

  expect(screen.getByTestId("asset-lab-empty").textContent).toMatch(/Select one mesh asset/);
  expect(screen.getByTestId("asset-lab-empty").textContent).toMatch(/never overwrites the source/i);
  fireEvent.click(screen.getByTestId("asset-lab-choose"));
  expect(choose).toHaveBeenCalledTimes(1);
  expect(screen.queryByRole("tablist")).toBeNull();
});

test("stage tabs follow keyboard navigation and repair sends the exact reviewed configuration", async () => {
  const run = vi.fn((_request: AssetLabAction) => Promise.resolve());
  render(<AssetLabPanel asset={asset} report={report} onRun={run} />);

  const inspectTab = screen.getByRole("tab", { name: "Inspect" });
  expect(inspectTab.getAttribute("aria-selected")).toBe("true");
  fireEvent.keyDown(inspectTab, { key: "ArrowRight" });
  const repairTab = screen.getByRole("tab", { name: "Repair" });
  expect(repairTab.getAttribute("aria-selected")).toBe("true");
  expect(document.activeElement).toBe(repairTab);

  const disclosure = screen.getByRole("button", { name: /Advanced repair controls/ });
  expect(disclosure.getAttribute("aria-expanded")).toBe("false");
  fireEvent.click(disclosure);
  expect(disclosure.getAttribute("aria-expanded")).toBe("true");
  expect(screen.getByLabelText("Weld threshold")).toBeTruthy();

  fireEvent.change(screen.getByLabelText("Repair preset"), { target: { value: "thorough" } });
  expect(screen.getByText(/Removes tiny disconnected shells/)).toBeTruthy();
  fireEvent.click(screen.getByTestId("asset-lab-repair"));

  await waitFor(() => expect(run).toHaveBeenCalledTimes(1));
  expect(run.mock.calls[0][0]).toEqual({
    action: "repair",
    assetId: "mesh-7",
    config: {
      preset: "thorough",
      weldThreshold: 0.0001,
      preserveAttributeSeams: true,
      removeDegenerateTriangles: true,
      removeDuplicateTriangles: true,
      removeIsolatedVertices: true,
      removeComponentsSmallerThanTriangles: 3,
      repairWinding: true,
      normalRepair: "recomputeAreaWeightedSmooth",
    },
  });
  expect(await screen.findByText("Repair derivative complete")).toBeTruthy();
});

test("before and after evidence, severity, and issue status are understandable without colour", () => {
  render(<AssetLabPanel asset={asset} report={report} onRun={vi.fn()} />);

  const metrics = screen.getByTestId("asset-lab-metrics");
  expect(metrics.textContent).toMatch(/12,000/);
  expect(metrics.textContent).toMatch(/4,800/);
  expect(screen.getByRole("list", { name: "Measured issues" }).textContent).toMatch(
    /Non-manifold edges \/ 3/,
  );
  expect(screen.getByText("open")).toBeTruthy();
  expect(screen.getByText("fixed")).toBeTruthy();
  expect(screen.getByText(/pair budget/)).toBeTruthy();
});

test("an unprepared bake and unsupported export stay disabled and explain why", () => {
  const run = vi.fn();
  render(<AssetLabPanel asset={asset} report={report} onRun={run} />);

  fireEvent.click(screen.getByRole("tab", { name: /Bake/ }));
  const bakeButton = screen.getByTestId("asset-lab-bake") as HTMLButtonElement;
  expect(bakeButton.disabled).toBe(true);
  expect(screen.getAllByText(/Choose at least one high-detail source/).length).toBeGreaterThan(0);
  expect((screen.getByLabelText("Texture resolution") as HTMLSelectElement).disabled).toBe(true);
  fireEvent.click(bakeButton);

  fireEvent.click(screen.getByRole("tab", { name: /Export/ }));
  const exportButton = screen.getByTestId("asset-lab-export") as HTMLButtonElement;
  expect(exportButton.disabled).toBe(true);
  expect(screen.getAllByText(/No verified export writer/).length).toBeGreaterThan(0);
  expect(run).not.toHaveBeenCalled();
});

test("an available bake exposes real options and sends the selected maps", async () => {
  const run = vi.fn((_request: AssetLabAction) => Promise.resolve());
  const high = { id: "mesh-high", name: "Valve scan", source: "Scene mesh", revision: "rev 1" };
  render(
    <AssetLabPanel
      asset={asset}
      report={report}
      availability={{ bake: AVAILABLE }}
      bakeSources={[asset, high]}
      activeStage="bake"
      onRun={run}
    />,
  );

  expect(screen.getByTestId("asset-lab-metrics").textContent).toContain("4,800");
  expect((screen.getByLabelText("Source alignment") as HTMLSelectElement).value).toBe("autoRelated");
  fireEvent.change(screen.getByLabelText("Texture resolution"), { target: { value: "1024" } });
  fireEvent.click(screen.getByRole("checkbox", { name: /Valve scan/ }));
  fireEvent.click(screen.getByLabelText("Ambient occlusion"));
  await waitFor(() => expect((screen.getByTestId("asset-lab-bake") as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByTestId("asset-lab-bake"));

  await waitFor(() => expect(run).toHaveBeenCalledTimes(1));
  expect(run.mock.calls[0][0]).toMatchObject({
    action: "bake",
    assetId: "mesh-7",
    config: {
      resolution: 1024,
      highSourceIds: ["mesh-7", "mesh-high"],
      alignmentPolicy: "autoRelated",
      minProjectionHitRatio: 0.9,
      maps: ["normal", "curvature"],
    },
  });
});

test("bake evidence exposes nonzero projection, quality threshold, and alignment without relying on colour", () => {
  render(
    <AssetLabPanel
      asset={asset}
      report={{
        ...report,
        bakeEvidence: {
          resolution: 512,
          requestedMaps: ["normal", "ambientOcclusion", "curvature"],
          sourceCount: 1,
          sourceTriangles: 12_000,
          charts: 4,
          atlasTexels: 262_144,
          coveredTexels: 120_000,
          projectedTexels: 118_800,
          projectionMisses: 1_200,
          dilatedTexels: 8_400,
          coverageRatio: 120_000 / 262_144,
          projectionHitRatio: 0.99,
          requiredHitRatio: 0.9,
          alignmentPolicy: "autoRelated",
          autoAlignedSources: 1,
          worldSpaceSources: 0,
        },
      }}
      availability={{ bake: AVAILABLE }}
      bakeSources={[asset]}
      activeStage="bake"
      onRun={vi.fn()}
    />,
  );

  const evidence = screen.getByTestId("asset-lab-bake-evidence");
  expect(evidence.getAttribute("data-projected-texels")).toBe("118800");
  expect(evidence.getAttribute("data-covered-texels")).toBe("120000");
  expect(evidence.getAttribute("data-projection-hit-ratio")).toBe("0.99");
  expect(evidence.getAttribute("data-required-hit-ratio")).toBe("0.9");
  expect(evidence.getAttribute("data-alignment-policy")).toBe("autoRelated");
  expect(evidence.textContent).toMatch(/99\.0%/);
  expect(evidence.textContent).toMatch(/Passed \/ minimum 90%/);
  expect(evidence.textContent).toMatch(/1 related aligned/);
});

test("UV and Materials offers a deterministic material derivative without overwriting the source", async () => {
  const run = vi.fn((_request: AssetLabAction) => Promise.resolve());
  render(
    <AssetLabPanel
      asset={asset}
      report={report}
      availability={{ uv: AVAILABLE }}
      activeStage="uv"
      onRun={run}
    />,
  );

  fireEvent.change(screen.getByLabelText("Material finish"), { target: { value: "brushed-metal" } });
  fireEvent.click(screen.getByTestId("asset-lab-material"));
  await waitFor(() => expect(run).toHaveBeenCalledTimes(1));
  expect(run.mock.calls[0][0]).toEqual({
    action: "applyMaterial",
    assetId: "mesh-7",
    config: { preset: "brushed-metal" },
  });
  expect(screen.getByText(/Source stays unchanged/)).toBeTruthy();
});

test("validation exposes collision generation only when the engine reports it available", async () => {
  const run = vi.fn((_request: AssetLabAction) => Promise.resolve());
  const { rerender } = render(
    <AssetLabPanel asset={asset} report={report} activeStage="validate" onRun={run} />,
  );
  expect((screen.getByTestId("asset-lab-collision") as HTMLButtonElement).disabled).toBe(true);

  rerender(
    <AssetLabPanel
      asset={asset}
      report={report}
      activeStage="validate"
      collisionAvailability={AVAILABLE}
      onRun={run}
    />,
  );
  fireEvent.click(screen.getByTestId("asset-lab-collision"));
  await waitFor(() => expect(run).toHaveBeenCalledTimes(1));
  expect(run.mock.calls[0][0]).toEqual({ action: "generateCollision", assetId: "mesh-7" });
});

test("complete-scene export is the recommended default and exposes an honest USDA choice", async () => {
  const run = vi.fn((_request: AssetLabAction) => Promise.resolve());
  render(
    <AssetLabPanel
      asset={asset}
      report={report}
      activeStage="export"
      availability={{ export: AVAILABLE }}
      onRun={run}
    />,
  );

  expect((screen.getByLabelText("Export scope") as HTMLSelectElement).value).toBe("scene");
  fireEvent.change(screen.getByLabelText("Export format"), { target: { value: "usda" } });
  expect(screen.getByText(/not binary USDC or packaged USDZ/i)).toBeTruthy();
  fireEvent.click(screen.getByTestId("asset-lab-export"));

  await waitFor(() => expect(run).toHaveBeenCalledTimes(1));
  expect(run.mock.calls[0][0]).toEqual({
    action: "export",
    assetId: "mesh-7",
    config: { format: "usda", scope: "scene" },
  });
});

test("busy, cancel, error, and success states are announced", async () => {
  const cancel = vi.fn();
  const { rerender } = render(
    <AssetLabPanel
      asset={asset}
      report={report}
      activeStage="optimize"
      operation={{ status: "busy", stage: "optimize", message: "Building candidates", progress: 0.4 }}
      onRun={vi.fn()}
      onCancel={cancel}
    />,
  );
  expect(screen.getByTestId("asset-lab").getAttribute("aria-busy")).toBe("true");
  expect((screen.getByRole("progressbar") as HTMLProgressElement).value).toBe(0.4);
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(cancel).toHaveBeenCalledTimes(1);

  rerender(
    <AssetLabPanel
      asset={asset}
      report={report}
      activeStage="repair"
      availability={{ repair: AVAILABLE }}
      onRun={() => Promise.reject(new Error("Repair kernel rejected a non-manifold fan"))}
    />,
  );
  fireEvent.click(screen.getByTestId("asset-lab-repair"));
  expect((await screen.findByRole("alert")).textContent).toMatch(/non-manifold fan/);

  rerender(
    <AssetLabPanel
      asset={asset}
      report={report}
      activeStage="validate"
      availability={{ validate: AVAILABLE }}
      onRun={() => Promise.resolve()}
    />,
  );
  fireEvent.click(screen.getByTestId("asset-lab-validate"));
  expect(await screen.findByText("Validation complete")).toBeTruthy();
});
