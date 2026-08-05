import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { PipeForge } from "./PipeForge";
import { fakeClient } from "../transport/test-client";
import { TauriClient } from "../transport/session";
import type { PipeBakeReport, PipeForgeStatus, UserFittingCatalogEntry } from "../transport/protocol";

const EMPTY_GRAPH = {
  handles: [],
  edges: [],
  fittings: [],
  fittingCatalog: [],
  branchFrom: null,
  editingEntity: null,
} satisfies Pick<PipeForgeStatus, "handles" | "edges" | "fittings" | "fittingCatalog" | "branchFrom" | "editingEntity">;

const READY: PipeForgeStatus = {
  ...EMPTY_GRAPH,
  active: false,
  points: 0,
  lengthM: 0,
  previewTriangles: 0,
  canBake: false,
  message: "Ready to draw",
};

const ACTIVE: PipeForgeStatus = {
  ...EMPTY_GRAPH,
  active: true,
  points: 3,
  lengthM: 4.25,
  previewTriangles: 12_480,
  canBake: true,
  message: "Click to extend · double-click to finish",
  handles: [
    { nodeId: 1, position: [0, 0, 0], connectedEdges: [1], fittingIds: [] },
    { nodeId: 2, position: [2, 0, 0], connectedEdges: [1, 2], fittingIds: [] },
    { nodeId: 3, position: [2, 0, 2.25], connectedEdges: [2], fittingIds: [] },
  ],
  edges: [
    { id: 1, from: 1, to: 2, diameterM: 0.05 },
    { id: 2, from: 2, to: 3, diameterM: 0.05 },
  ],
};

const BAKED: PipeBakeReport = {
  entityId: "pipe-1",
  handle: "mtkasset:0123456789abcdef0123456789abcdef",
  vertices: 4_200,
  triangles: 8_000,
  lodTriangles: [8_000, 4_000, 1_800],
  textureResolution: 256,
  collisionHulls: 0,
  collisionKind: "triangle mesh",
  collisionTriangles: 8_000,
  watertight: true,
  warnings: [],
  message: "Pipe asset baked",
};

test("configures the complete recipe and starts direct viewport drawing", async () => {
  const pipeForgeStart = vi.fn(() => Promise.resolve(ACTIVE));
  const onStatus = vi.fn();
  render(
    <PipeForge
      client={fakeClient({ pipeForgeStart })}
      status={READY}
      onStatus={onStatus}
      onBaked={vi.fn()}
    />,
  );

  expect(screen.getByTestId("pipe-forge").getAttribute("data-active")).toBe("false");
  const kitSelect = screen.getByTestId("pipe-forge-kit") as HTMLSelectElement;
  const qualitySelect = screen.getByTestId("pipe-forge-quality") as HTMLSelectElement;
  expect(kitSelect.value).toBe("galvanized");
  expect(Array.from(kitSelect.options, (option) => option.value)).toEqual(["galvanized", "copper", "pvc", "scifi"]);
  expect(qualitySelect.value).toBe("production");
  expect(Array.from(qualitySelect.options, (option) => option.value)).toEqual(["preview", "production", "hero"]);
  expect(screen.getByTestId("pipe-forge-auto-fittings").getAttribute("aria-pressed")).toBe("true");

  fireEvent.change(screen.getByTestId("pipe-forge-kit"), { target: { value: "copper" } });
  fireEvent.focus(screen.getByTestId("pipe-forge-diameter"));
  fireEvent.change(screen.getByTestId("pipe-forge-diameter"), { target: { value: "8.5" } });
  fireEvent.blur(screen.getByTestId("pipe-forge-diameter"));
  fireEvent.change(screen.getByTestId("pipe-forge-quality"), { target: { value: "hero" } });
  fireEvent.click(screen.getByTestId("pipe-forge-auto-fittings"));
  fireEvent.click(screen.getByTestId("pipe-forge-start"));

  await waitFor(() =>
    expect(pipeForgeStart).toHaveBeenCalledWith({
      kit: "copper",
      diameterCm: 8.5,
      quality: "hero",
      autoFittings: false,
    }),
  );
  expect(onStatus).toHaveBeenCalledWith(ACTIVE);
});

test("active drawing shows live points, metric length, preview triangles and the engine message", () => {
  render(<PipeForge client={fakeClient()} status={ACTIVE} onStatus={vi.fn()} onBaked={vi.fn()} />);

  expect(screen.getByTestId("pipe-forge").getAttribute("data-active")).toBe("true");
  expect(screen.queryByTestId("pipe-forge-setup")).toBeNull();
  expect(screen.getByTestId("pipe-forge-points").textContent).toContain("3");
  expect(screen.getByTestId("pipe-forge-length").textContent).toContain("4.25 m");
  expect(screen.getByTestId("pipe-forge-triangles").textContent).toContain("12,480");
  expect(screen.getByTestId("pipe-forge-message").textContent).toContain("Click to extend");
});

test("an active run displays the engine-authoritative recipe after reconnect", () => {
  render(
    <PipeForge
      client={fakeClient()}
      status={{
        ...ACTIVE,
        kit: "copper",
        diameterCm: 12,
        quality: "hero",
        autoFittings: false,
        points: 2,
        lengthM: 1,
        previewTriangles: 800,
        canBake: true,
        message: "Route restored",
      }}
      onStatus={() => {}}
      onBaked={() => {}}
    />,
  );
  expect(screen.getByText("Copper")).toBeTruthy();
  expect(screen.getByText("12 cm")).toBeTruthy();
  expect(screen.getByText("hero")).toBeTruthy();
  expect(screen.getByText("Manual fittings")).toBeTruthy();
});

test("Undo point applies the returned authoritative status", async () => {
  const undone = { ...ACTIVE, points: 2, lengthM: 2.75, previewTriangles: 8_240 };
  const pipeForgeUndo = vi.fn(() => Promise.resolve(undone));
  const onStatus = vi.fn();
  render(<PipeForge client={fakeClient({ pipeForgeUndo })} status={ACTIVE} onStatus={onStatus} onBaked={vi.fn()} />);

  fireEvent.click(screen.getByTestId("pipe-forge-undo"));
  await waitFor(() => expect(pipeForgeUndo).toHaveBeenCalledTimes(1));
  expect(onStatus).toHaveBeenCalledWith(undone);
});

test("Undo is unavailable before the first point and Bake stays unavailable before two points", () => {
  const noPoints: PipeForgeStatus = { ...ACTIVE, points: 0, lengthM: 0, previewTriangles: 0, canBake: false, handles: [], edges: [] };
  const onePoint: PipeForgeStatus = { ...ACTIVE, points: 1, lengthM: 0, previewTriangles: 0, canBake: true, handles: [{ nodeId: 1, position: [0, 0, 0], connectedEdges: [], fittingIds: [] }], edges: [] };
  const { rerender } = render(<PipeForge client={fakeClient()} status={noPoints} onStatus={vi.fn()} onBaked={vi.fn()} />);

  expect((screen.getByTestId("pipe-forge-undo") as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByTestId("pipe-forge-bake") as HTMLButtonElement).disabled).toBe(true);

  rerender(<PipeForge client={fakeClient()} status={onePoint} onStatus={vi.fn()} onBaked={vi.fn()} />);
  expect((screen.getByTestId("pipe-forge-undo") as HTMLButtonElement).disabled).toBe(false);
  expect((screen.getByTestId("pipe-forge-bake") as HTMLButtonElement).disabled).toBe(true);
});

test("Bake also respects engine validation even when the run has two points", () => {
  const invalid: PipeForgeStatus = { ...ACTIVE, points: 2, canBake: false, message: "The last segment is too short" };
  render(<PipeForge client={fakeClient()} status={invalid} onStatus={vi.fn()} onBaked={vi.fn()} />);

  expect((screen.getByTestId("pipe-forge-bake") as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByTestId("pipe-forge-message").textContent).toContain("too short");
});

test("Bake Asset returns the complete build report to the host", async () => {
  const pipeForgeBake = vi.fn(() => Promise.resolve(BAKED));
  const onBaked = vi.fn();
  const client = fakeClient({ pipeForgeBake });
  const onStatus = vi.fn();
  const { rerender } = render(<PipeForge client={client} status={ACTIVE} onStatus={onStatus} onBaked={onBaked} />);

  fireEvent.click(screen.getByTestId("pipe-forge-bake"));
  await waitFor(() => expect(pipeForgeBake).toHaveBeenCalledTimes(1));
  expect(onBaked).toHaveBeenCalledWith(BAKED);
  expect(screen.getByTestId("pipe-forge-report").textContent).toContain("Asset ready");
  expect(screen.getByTestId("pipe-forge-report").textContent).toContain("8,000 triangles");
  expect(screen.getByTestId("pipe-forge-report").textContent).toContain("3 LODs");
  expect(screen.getByTestId("pipe-forge-report").textContent).toContain("PBR 256px");
  expect(screen.getByTestId("pipe-forge-report").textContent).toContain("Watertight");
  expect(screen.getByTestId("pipe-forge-report").textContent).toContain("Triangle mesh collision · 8,000 triangles");

  rerender(<PipeForge client={client} status={READY} onStatus={onStatus} onBaked={onBaked} />);
  expect(screen.queryByTestId("pipe-forge-active")).toBeNull();
  expect(screen.queryByTestId("pipe-forge-setup")).toBeNull();
  expect(screen.getByTestId("pipe-forge-start").textContent).toContain("Draw another pipe");
  expect(screen.getByText(/Asset Lab is open below/)).toBeTruthy();
});

test("Cancel ends the tool with the returned authoritative status", async () => {
  const pipeForgeCancel = vi.fn(() => Promise.resolve(READY));
  const onStatus = vi.fn();
  render(<PipeForge client={fakeClient({ pipeForgeCancel })} status={ACTIVE} onStatus={onStatus} onBaked={vi.fn()} />);

  fireEvent.click(screen.getByTestId("pipe-forge-cancel"));
  await waitFor(() => expect(pipeForgeCancel).toHaveBeenCalledTimes(1));
  expect(onStatus).toHaveBeenCalledWith(READY);
});

test("overlay controls do not leak clicks into viewport point placement", () => {
  const viewportClick = vi.fn();
  const viewportPointerDown = vi.fn();
  const viewportWheel = vi.fn();
  render(
    <div onClick={viewportClick} onPointerDown={viewportPointerDown} onWheel={viewportWheel}>
      <PipeForge client={fakeClient()} status={READY} onStatus={vi.fn()} onBaked={vi.fn()} />
    </div>,
  );

  fireEvent.pointerDown(screen.getByTestId("pipe-forge-kit"));
  fireEvent.click(screen.getByTestId("pipe-forge-kit"));
  fireEvent.wheel(screen.getByTestId("pipe-forge-diameter"));
  fireEvent.click(screen.getByTestId("pipe-forge-start"));
  expect(viewportPointerDown).not.toHaveBeenCalled();
  expect(viewportClick).not.toHaveBeenCalled();
  expect(viewportWheel).not.toHaveBeenCalled();
});

test("a failed tool command is explained and the action becomes available again", async () => {
  const pipeForgeStart = vi.fn(() => Promise.reject(new Error("viewport unavailable")));
  render(<PipeForge client={fakeClient({ pipeForgeStart })} status={READY} onStatus={vi.fn()} onBaked={vi.fn()} />);

  fireEvent.click(screen.getByTestId("pipe-forge-start"));
  expect((await screen.findByRole("alert")).textContent).toContain("viewport unavailable");
  await waitFor(() => expect((screen.getByTestId("pipe-forge-start") as HTMLButtonElement).disabled).toBe(false));
});

test("reopens the selected baked PipeRecipe for post-bake route editing", async () => {
  const edited: PipeForgeStatus = { ...ACTIVE, editingEntity: "pipe-42", message: "Editable route restored" };
  const pipeForgeEdit = vi.fn(() => Promise.resolve(edited));
  const onStatus = vi.fn();
  render(
    <PipeForge
      client={fakeClient({ pipeForgeEdit })}
      status={READY}
      editableEntityId="pipe-42"
      onStatus={onStatus}
      onBaked={vi.fn()}
    />,
  );

  expect(screen.getByText(/restore its route handles/i)).toBeTruthy();
  fireEvent.click(screen.getByTestId("pipe-forge-edit"));
  await waitFor(() => expect(pipeForgeEdit).toHaveBeenCalledWith("pipe-42"));
  expect(onStatus).toHaveBeenCalledWith(edited);
});

test("an engine-refused post-bake edit keeps the selected pipe actionable and explains the transform constraint", async () => {
  const refused: PipeForgeStatus = {
    ...READY,
    message: "Route handles require an unscaled, unrotated pipe entity; preserve its translation and reset rotation/scale first",
  };
  const pipeForgeEdit = vi.fn(() => Promise.resolve(refused));
  const onStatus = vi.fn();
  const { rerender } = render(
    <PipeForge
      client={fakeClient({ pipeForgeEdit })}
      status={READY}
      editableEntityId="pipe-scaled"
      onStatus={onStatus}
      onBaked={vi.fn()}
    />,
  );

  fireEvent.click(screen.getByTestId("pipe-forge-edit"));
  await waitFor(() => expect(pipeForgeEdit).toHaveBeenCalledWith("pipe-scaled"));
  expect(onStatus).toHaveBeenCalledWith(refused);

  rerender(
    <PipeForge
      client={fakeClient({ pipeForgeEdit })}
      status={refused}
      editableEntityId="pipe-scaled"
      onStatus={onStatus}
      onBaked={vi.fn()}
    />,
  );
  expect(screen.getByTestId("pipe-forge-message").textContent).toContain("unscaled, unrotated");
  expect((screen.getByTestId("pipe-forge-edit") as HTMLButtonElement).disabled).toBe(false);
  expect(screen.getByText(/reset rotation\/scale first/i)).toBeTruthy();
});

test("route network moves a stable handle and starts and ends a viewport branch", async () => {
  const branchStatus: PipeForgeStatus = { ...ACTIVE, branchFrom: 1, message: "Click to extend the branch" };
  const pipeForgeMoveHandle = vi.fn(() => Promise.resolve(ACTIVE));
  const pipeForgeBeginBranch = vi.fn(() => Promise.resolve(branchStatus));
  const pipeForgeEndBranch = vi.fn(() => Promise.resolve({ ...ACTIVE, branchFrom: null }));
  const client = fakeClient({ pipeForgeMoveHandle, pipeForgeBeginBranch, pipeForgeEndBranch });
  const onStatus = vi.fn();
  const { rerender } = render(<PipeForge client={client} status={ACTIVE} onStatus={onStatus} onBaked={vi.fn()} />);

  fireEvent.click(screen.getByTestId("pipe-forge-network").querySelector("summary")!);
  fireEvent.change(screen.getByTestId("pipe-forge-handle"), { target: { value: "1" } });
  fireEvent.focus(screen.getByTestId("pipe-forge-handle-x"));
  fireEvent.change(screen.getByTestId("pipe-forge-handle-x"), { target: { value: "3.5" } });
  fireEvent.blur(screen.getByTestId("pipe-forge-handle-x"));
  fireEvent.click(screen.getByTestId("pipe-forge-move-handle"));
  await waitFor(() => expect(pipeForgeMoveHandle).toHaveBeenCalledWith(1, 3.5, 0, 0));

  fireEvent.focus(screen.getByTestId("pipe-forge-branch-diameter"));
  fireEvent.change(screen.getByTestId("pipe-forge-branch-diameter"), { target: { value: "2.5" } });
  fireEvent.blur(screen.getByTestId("pipe-forge-branch-diameter"));
  fireEvent.click(screen.getByTestId("pipe-forge-begin-branch"));
  await waitFor(() => expect(pipeForgeBeginBranch).toHaveBeenCalledWith(1, 2.5));

  rerender(<PipeForge client={client} status={branchStatus} onStatus={onStatus} onBaked={vi.fn()} />);
  expect(screen.getByTestId("pipe-forge-branch-mode").textContent).toContain("handle 1");
  fireEvent.click(screen.getByTestId("pipe-forge-end-branch"));
  await waitFor(() => expect(pipeForgeEndBranch).toHaveBeenCalledTimes(1));
});

test("semantic fitting controls place and remove a user-catalog valve", async () => {
  const entry: UserFittingCatalogEntry = {
    id: "isolation-valve",
    label: "Isolation valve",
    kind: "valve",
    assetHandle: "mtkasset:valve",
    diameterScale: 1.2,
    lengthScale: 1.5,
  };
  const withCatalog: PipeForgeStatus = { ...ACTIVE, fittingCatalog: [entry] };
  const placed: PipeForgeStatus = {
    ...withCatalog,
    fittings: [{ id: 9, nodeId: 1, kind: "valve", catalogId: entry.id, automatic: false }],
    handles: withCatalog.handles.map((handle) => handle.nodeId === 1 ? { ...handle, fittingIds: [9] } : handle),
  };
  const pipeForgePlaceFitting = vi.fn(() => Promise.resolve(placed));
  const pipeForgeRemoveFitting = vi.fn(() => Promise.resolve(withCatalog));
  const client = fakeClient({ pipeForgePlaceFitting, pipeForgeRemoveFitting });
  const { rerender } = render(<PipeForge client={client} status={withCatalog} onStatus={vi.fn()} onBaked={vi.fn()} />);

  fireEvent.click(screen.getByTestId("pipe-forge-fittings").querySelector("summary")!);
  fireEvent.change(screen.getByTestId("pipe-forge-fitting-kind"), { target: { value: "valve" } });
  fireEvent.change(screen.getByTestId("pipe-forge-fitting-catalog"), { target: { value: entry.id } });
  fireEvent.click(screen.getByTestId("pipe-forge-place-fitting"));
  await waitFor(() => expect(pipeForgePlaceFitting).toHaveBeenCalledWith(1, "valve", "isolation-valve"));

  rerender(<PipeForge client={client} status={placed} onStatus={vi.fn()} onBaked={vi.fn()} />);
  fireEvent.click(screen.getByTestId("pipe-forge-fittings").querySelector("summary")!);
  fireEvent.click(screen.getByTestId("pipe-forge-remove-fitting-9"));
  await waitFor(() => expect(pipeForgeRemoveFitting).toHaveBeenCalledWith(9));
});

test("catalog flow saves bounded fitting metadata and removes an unused entry", async () => {
  const entry: UserFittingCatalogEntry = {
    id: "isolation-valve",
    label: "Isolation valve",
    kind: "valve",
    assetHandle: "mtkasset:valve-hero",
    diameterScale: 1.25,
    lengthScale: 1.5,
  };
  const saved: PipeForgeStatus = { ...ACTIVE, fittingCatalog: [entry] };
  const pipeForgeUpsertCatalog = vi.fn(() => Promise.resolve(saved));
  const pipeForgeRemoveCatalog = vi.fn(() => Promise.resolve(ACTIVE));
  const client = fakeClient({ pipeForgeUpsertCatalog, pipeForgeRemoveCatalog });
  const { rerender } = render(<PipeForge client={client} status={ACTIVE} onStatus={vi.fn()} onBaked={vi.fn()} />);

  fireEvent.click(screen.getByTestId("pipe-forge-catalog").querySelector("summary")!);
  fireEvent.change(screen.getByTestId("pipe-forge-catalog-label"), { target: { value: "Isolation valve" } });
  fireEvent.change(screen.getByTestId("pipe-forge-catalog-kind"), { target: { value: "valve" } });
  fireEvent.change(screen.getByTestId("pipe-forge-catalog-asset"), { target: { value: "mtkasset:valve-hero" } });
  for (const [testId, value] of [["pipe-forge-catalog-diameter-scale", "1.25"], ["pipe-forge-catalog-length-scale", "1.5"]] as const) {
    fireEvent.focus(screen.getByTestId(testId));
    fireEvent.change(screen.getByTestId(testId), { target: { value } });
    fireEvent.blur(screen.getByTestId(testId));
  }
  fireEvent.click(screen.getByTestId("pipe-forge-save-catalog"));
  await waitFor(() => expect(pipeForgeUpsertCatalog).toHaveBeenCalledWith(entry));

  rerender(<PipeForge client={client} status={saved} onStatus={vi.fn()} onBaked={vi.fn()} />);
  fireEvent.click(screen.getByTestId("pipe-forge-catalog").querySelector("summary")!);
  fireEvent.click(screen.getByTestId("pipe-forge-remove-catalog-isolation-valve"));
  await waitFor(() => expect(pipeForgeRemoveCatalog).toHaveBeenCalledWith("isolation-valve"));
});

test("Tauri transport sends the exact graph, fitting and catalog command payloads", async () => {
  const invoke = vi.fn((command: string) => command === "connect" ? Promise.resolve(null) : Promise.resolve(ACTIVE));
  const core = {
    invoke,
    Channel: class { onmessage: (message: unknown) => void = () => {}; },
  };
  const client = new TauriClient(core as unknown as ConstructorParameters<typeof TauriClient>[0]);
  const entry: UserFittingCatalogEntry = { id: "gate", label: "Gate valve", kind: "valve", assetHandle: null, diameterScale: 1, lengthScale: 1.4 };

  await client.pipeForgeEdit("pipe-7");
  await client.pipeForgeBeginBranch(4, 3.5);
  await client.pipeForgeEndBranch();
  await client.pipeForgeMoveHandle(5, 1, 2, 3);
  await client.pipeForgeRemoveHandle(5);
  await client.pipeForgePlaceFitting(4, "valve", "gate");
  await client.pipeForgePlaceFitting(4, "tee");
  await client.pipeForgeRemoveFitting(12);
  await client.pipeForgeUpsertCatalog(entry);
  await client.pipeForgeRemoveCatalog("gate");

  expect(invoke.mock.calls.slice(1)).toEqual([
    ["pipe_forge_edit", { id: "pipe-7" }],
    ["pipe_forge_begin_branch", { nodeId: 4, diameterCm: 3.5 }],
    ["pipe_forge_end_branch"],
    ["pipe_forge_move_handle", { nodeId: 5, x: 1, y: 2, z: 3 }],
    ["pipe_forge_remove_handle", { nodeId: 5 }],
    ["pipe_forge_place_fitting", { nodeId: 4, kind: "valve", catalogId: "gate" }],
    ["pipe_forge_place_fitting", { nodeId: 4, kind: "tee", catalogId: null }],
    ["pipe_forge_remove_fitting", { fittingId: 12 }],
    ["pipe_forge_upsert_catalog", { entry }],
    ["pipe_forge_remove_catalog", { id: "gate" }],
  ]);
});
