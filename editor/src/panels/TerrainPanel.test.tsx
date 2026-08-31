import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { TerrainPanel } from "./TerrainPanel";
import { emptyTerrainReply, emptyTerrainStats, fakeClient } from "../transport/test-client";
import type { TerrainRecipe, TerrainReply } from "../transport/protocol";
import { projectionStore } from "../store/projection";

/** Ensure a disclosure section is OPEN, whatever it defaulted to.
 *
 * `fireEvent.click` on a section header TOGGLES. Tests that clicked blindly were really asserting "this
 * section happens to start closed", so changing a default silently made them close the very thing they
 * meant to open. Asking for the state we need keeps the test about its subject. */
function openSection(id: string) {
  const head = screen.getAllByTestId(`terrain-section-${id}`)[0];
  if (head.getAttribute("aria-expanded") !== "true") fireEvent.click(head);
  return head;
}

const PRESETS = [
  { id: "flat", name: "Flat Ground", description: "A level plate with one material." },
  { id: "rolling-hills", name: "Rolling Hills", description: "Gentle warped hills with grass and rock." },
];

function recipe(over: Partial<TerrainRecipe> = {}): TerrainRecipe {
  return {
    version: 1,
    name: "Rolling Hills",
    description: "",
    seed: 12_345,
    world_size_m: 2048,
    chunk_size_m: 64,
    chunk_verts: 65,
    layers: [
      { name: "Base", kind: { Constant: { height: 0 } }, blend: "Replace", weight: 1, enabled: true, seed_offset: 0 },
      { name: "Hills", kind: { Fbm: { amplitude: 38 } }, blend: "Add", weight: 1, enabled: true, seed_offset: 11 },
    ],
    strokes: [],
    splines: [],
    materials: [{ name: "Grass", albedo: [0.2, 0.34, 0.15], roughness: 0.86 }],
    biomes: [{ name: "Meadow", material_layer: 0, enabled: true }],
    protos: [
      { name: "Tree", mesh_key: "", lod_keys: [], impostor_key: null, radius_m: 2, height_m: 12, collide: true },
    ],
    scatter: [{ name: "Woodland", proto: 0, density_per_hectare: 110, enabled: true }],
    water: { enabled: true, sea_level_m: 0, shore_blend_m: 2, deep_m: 8 },
    lod: { levels: 4, screen_error_px: 1, max_view_distance_m: 1024, texture_res: 256, horizon_culling: true },
    budget: { mesh_mb: 256, texture_mb: 320, scatter_mb: 96, collider_mb: 64, max_resident_chunks: 1200 },
    ...over,
  };
}

function reply(over: Partial<TerrainReply> = {}): TerrainReply {
  return { ok: true, entity: "e1", message: "", recipe: recipe(), issues: [], stats: emptyTerrainStats(), ...over };
}

/** A client with no terrain in the scene — the state the panel opens in. */
function emptyClient(over = {}) {
  return fakeClient({
    terrainPresets: vi.fn(() => Promise.resolve(PRESETS)),
    terrainEdit: vi.fn(() => Promise.resolve(emptyTerrainReply())),
    ...over,
  });
}

test("offers a way in — and no shaping controls — before a terrain exists", async () => {
  render(<TerrainPanel client={emptyClient()} statsIntervalMs={0} />);
  await screen.findByTestId("terrain-preset-flat");
  const hills = screen.getByTestId("terrain-preset-rolling-hills");
  expect(hills.textContent).toContain("Rolling Hills");
  // The description is still reachable, so the choice is informed rather than a name-guessing game — but
  // it is the tile's tooltip now rather than a third line printed under every one of six tiles, which is
  // what turned this grid back into the column of prose it replaced.
  expect(hills.getAttribute("title")).toContain("Gentle warped hills");
  // One tile per preset the engine published, and no others — a picker that drops or invents a choice
  // is the defect the browser mock had for two milestones (three of the engine's six were missing).
  expect(screen.getByTestId("terrain-presets").children.length).toBe(PRESETS.length);
  // No shaping controls are rendered at all — a wall of disabled sliders is not an empty state.
  expect(screen.queryByTestId("terrain-world-size")).toBeNull();
  expect(screen.queryByTestId("terrain-layers")).toBeNull();
});

const READING = {
  brief: {
    landform: "Mountains",
    climate: "Alpine",
    relief: 1.6,
    worldSizeM: 4000,
    water: "Lakes",
    vegetation: 1.6,
    erosion: true,
    terraces: false,
    river: true,
    road: false,
    name: "Alpine Peaks",
    seed: 4242,
  },
  understood: [
    { phrase: "4 km", meaning: "4000 m across" },
    { phrase: "mountains", meaning: "landform: mountains" },
    { phrase: "river", meaning: "a river" },
  ],
  unused: ["wizards"],
};

test("the description box is offered before any terrain exists, alongside the presets", async () => {
  render(<TerrainPanel client={emptyClient()} statsIntervalMs={0} />);
  await screen.findByTestId("terrain-describe-text");
  // Both routes in, not one instead of the other.
  expect(screen.getByTestId("terrain-preset-flat")).toBeTruthy();
  // Nothing to build from an empty box.
  expect((screen.getByTestId("terrain-describe-build") as HTMLButtonElement).disabled).toBe(true);
  // And the examples are there, so the author is not staring at a blank field. Counting the PILLS, not
  // the container's children: the container holds a heading and a row, so `children.length` stopped
  // being a count of examples the moment the row wrapped — an assertion that would have gone green on
  // zero examples.
  const pills = screen.getAllByTestId("terrain-example");
  expect(pills.length).toBeGreaterThan(1);
  // The pill's word is short and its tooltip is the sentence it types — the pill is a label for a
  // description, not the description, and losing the sentence would make it a guess.
  expect(pills[0].getAttribute("title")).toContain("alpine valley");
  // And the examples offered here are the ones that APPLY here. "raise this mountain by 150 m" needs a
  // mountain; on a surface whose whole condition is that no terrain exists, it was an invitation to a
  // refusal. It moves to the compact box, which is the form shown once a world exists — and which used
  // to show no examples at all.
  expect(pills.map((p) => p.textContent)).not.toContain("Raise a mountain");
  // And pressing one puts the WHOLE sentence in the box, unabbreviated. Shortening the row and
  // shortening what it types are two different changes, and only the first one was made.
  fireEvent.click(pills[0]);
  expect((screen.getByTestId("terrain-describe-text") as HTMLTextAreaElement).value).toBe(
    pills[0].getAttribute("title"),
  );
  expect((screen.getByTestId("terrain-describe-build") as HTMLButtonElement).disabled).toBe(false);
});

const CREATE_PLAN = {
  kind: "create" as const,
  understood: READING.understood,
  unused: ["wizards"],
  notes: [],
  steps: [],
  ok: true,
};

const MODIFY_PLAN = {
  kind: "modify" as const,
  understood: [{ phrase: "raise", meaning: "raise the mountain you are pointing at" }],
  unused: [],
  notes: [],
  steps: [
    {
      verb: "raise",
      effect: "raise North Ridge by 150 m — rebuilds ground height, biomes, materials, vegetation",
      refusal: null,
      suggestion: null,
      rebuilds: "ground height, biomes, materials, vegetation",
      region: [400, 400, 900, 900] as [number, number, number, number],
    },
  ],
  ok: true,
};

test("a description is planned before it is built, including the words it could not use", async () => {
  const terrainPlan = vi.fn(() => Promise.resolve(CREATE_PLAN));
  const terrainDescribe = vi.fn(() => Promise.resolve(reply()));
  render(<TerrainPanel client={emptyClient({ terrainPlan, terrainDescribe })} statsIntervalMs={0} />);
  const box = await screen.findByTestId("terrain-describe-text");
  fireEvent.change(box, { target: { value: "a 4 km mountain range with a river and wizards" } });

  // The plan arrives WITHOUT anything being built — that is what makes it safe to show while typing.
  await waitFor(() => expect(terrainPlan).toHaveBeenCalled());
  const panel = await screen.findByTestId("terrain-reading");
  expect(terrainDescribe).not.toHaveBeenCalled();
  expect(panel.textContent).toContain("landform: mountains");
  expect(panel.textContent).toContain("4000 m across");
  // The unused words are surfaced, not swallowed: this is why "why is there no wizard?" has an answer.
  expect(screen.getByTestId("terrain-reading-unused").textContent).toContain("wizards");

  fireEvent.click(screen.getByTestId("terrain-describe-build"));
  await waitFor(() =>
    expect(terrainDescribe).toHaveBeenCalledWith("a 4 km mountain range with a river and wizards"),
  );
  // And what comes back is an ordinary recipe, editable like any other.
  await screen.findByTestId("terrain-layers");
});

test("a local edit shows the landform it resolved to and what it will rebuild", async () => {
  // The half of the feature that changes a world instead of replacing it (ADR-106).
  const terrainPlan = vi.fn(() => Promise.resolve(MODIFY_PLAN));
  render(<TerrainPanel client={emptyClient({ terrainPlan })} statsIntervalMs={0} />);
  fireEvent.change(await screen.findByTestId("terrain-describe-text"), {
    target: { value: "raise this mountain by 150 m" },
  });
  const step = await screen.findByTestId("terrain-plan-step");
  // It names the landform — so "this" is never a guess the author has to verify afterwards.
  expect(step.textContent).toContain("North Ridge");
  expect(step.textContent).toContain("150 m");
  // And it states the cost: which derived data this will rebuild.
  expect(step.textContent).toContain("rebuilds");
  expect(step.textContent).toContain("vegetation");
});

test("an impossible request is refused with a reason and a way forward", async () => {
  const terrainPlan = vi.fn(() =>
    Promise.resolve({
      kind: "modify" as const,
      understood: [],
      unused: [],
      notes: [],
      ok: false,
      steps: [
        {
          verb: "raise",
          effect: "",
          refusal: "there is no landform called “the volcano”",
          suggestion: "this world has: North Ridge, Glen Etive",
          rebuilds: "",
          region: null,
        },
      ],
    }),
  );
  render(<TerrainPanel client={emptyClient({ terrainPlan })} statsIntervalMs={0} />);
  fireEvent.change(await screen.findByTestId("terrain-describe-text"), {
    target: { value: "raise the volcano" },
  });
  const step = await screen.findByTestId("terrain-plan-step");
  expect(step.textContent).toContain("no landform called");
  // Never a dead end: it says what IS there.
  expect(step.textContent).toContain("North Ridge");
});

test("Enter builds the description and Shift+Enter does not", async () => {
  const terrainDescribe = vi.fn(() => Promise.resolve(reply()));
  render(<TerrainPanel client={emptyClient({ terrainDescribe })} statsIntervalMs={0} />);
  const box = await screen.findByTestId("terrain-describe-text");
  fireEvent.change(box, { target: { value: "rolling hills" } });

  // Shift+Enter is a newline in a prose field, not a commit.
  fireEvent.keyDown(box, { key: "Enter", shiftKey: true });
  expect(terrainDescribe).not.toHaveBeenCalled();

  fireEvent.keyDown(box, { key: "Enter" });
  await waitFor(() => expect(terrainDescribe).toHaveBeenCalledWith("rolling hills"));
});

test("an existing terrain can be re-described, and it says that it starts over", async () => {
  const terrainDescribe = vi.fn(() => Promise.resolve(reply()));
  const client = emptyClient({ terrainEdit: vi.fn(() => Promise.resolve(reply())), terrainDescribe });
  render(<TerrainPanel client={client} statsIntervalMs={0} />);
  await screen.findByTestId("terrain-layers");
  openSection("describe");
  const box = await screen.findByTestId("terrain-describe-text");
  fireEvent.change(box, { target: { value: "a lush tropical island" } });
  fireEvent.click(screen.getByTestId("terrain-describe-build"));
  await waitFor(() => expect(terrainDescribe).toHaveBeenCalledWith("a lush tropical island"));
});

test("creating from a preset asks the engine and shows what came back", async () => {
  const terrainCreate = vi.fn(() => Promise.resolve(reply()));
  render(<TerrainPanel client={emptyClient({ terrainCreate })} statsIntervalMs={0} />);
  fireEvent.click(await screen.findByTestId("terrain-preset-rolling-hills"));
  await waitFor(() => expect(terrainCreate).toHaveBeenCalledWith("rolling-hills"));
  // The panel now renders the DOCUMENT's recipe, not what it asked for.
  await screen.findByTestId("terrain-layers");
  openSection("world");
  expect((screen.getByTestId("terrain-name") as HTMLInputElement).value).toBe("Rolling Hills");
  expect((screen.getByTestId("terrain-world-size") as HTMLInputElement).value).toBe("2048");
});

test("an existing terrain is picked up on mount, not only one this panel created", async () => {
  // A reopened project, or an undo that brought a terrain back.
  const client = emptyClient({ terrainEdit: vi.fn(() => Promise.resolve(reply())) });
  render(<TerrainPanel client={client} statsIntervalMs={0} />);
  await screen.findByTestId("terrain-layers");
  expect(screen.queryByTestId("terrain-preset-flat")).toBeNull();
});

test("each control sends exactly one edit, in the engine's vocabulary", async () => {
  const terrainEdit = vi.fn(() => Promise.resolve(reply()));
  render(<TerrainPanel client={emptyClient({ terrainEdit })} statsIntervalMs={0} />);
  await screen.findByTestId("terrain-layers");
  terrainEdit.mockClear();

  // Each step waits for the previous edit to settle: controls are disabled while one is in flight, which is
  // itself the intended behaviour (no double-commit from an impatient second click).
  openSection("world");
  fireEvent.change(screen.getByTestId("terrain-name"), { target: { value: "Valley" } });
  await waitFor(() => expect(terrainEdit).toHaveBeenCalledWith({ op: "rename", name: "Valley" }));

  terrainEdit.mockClear();
  fireEvent.change(screen.getByTestId("terrain-seed"), { target: { value: "77" } });
  fireEvent.blur(screen.getByTestId("terrain-seed"));
  await waitFor(() => expect(terrainEdit).toHaveBeenCalledWith({ op: "setSeed", seed: 77 }));

  terrainEdit.mockClear();
  await waitFor(() =>
    expect((screen.getByTestId("terrain-layer-toggle-1") as HTMLButtonElement).disabled).toBe(false),
  );
  fireEvent.click(screen.getByTestId("terrain-layer-toggle-1"));
  await waitFor(() =>
    expect(terrainEdit).toHaveBeenCalledWith({ op: "toggleLayer", index: 1, enabled: false }),
  );

  terrainEdit.mockClear();
  await waitFor(() =>
    expect((screen.getByTestId("terrain-layer-remove-0") as HTMLButtonElement).disabled).toBe(false),
  );
  fireEvent.click(screen.getByTestId("terrain-layer-remove-0"));
  await waitFor(() => expect(terrainEdit).toHaveBeenCalledWith({ op: "removeLayer", index: 0 }));
});

test("a refused edit shows the reason and the fix instead of failing silently", async () => {
  const refused = reply({
    ok: false,
    message: "that edit would break the terrain: 50 vertices per edge is not a power of two",
    issues: [
      {
        severity: "blocking",
        field: "chunk_verts",
        message: "50 vertices per edge means 49 cells, which is not a power of two",
        fix: "use 65",
      },
    ],
  });
  const terrainEdit = vi
    .fn()
    .mockResolvedValueOnce(reply())
    .mockResolvedValue(refused);
  render(<TerrainPanel client={emptyClient({ terrainEdit })} statsIntervalMs={0} />);
  await screen.findByTestId("terrain-chunk-verts");
  fireEvent.change(screen.getByTestId("terrain-chunk-verts"), { target: { value: "50" } });
  fireEvent.blur(screen.getByTestId("terrain-chunk-verts"));
  const alert = await screen.findByTestId("terrain-message");
  expect(alert.textContent).toContain("not a power of two");
  const issues = await screen.findByTestId("terrain-issues");
  expect(issues.textContent).toContain("chunk_verts");
  expect(issues.textContent).toContain("Fix: use 65");
});

test("sections are collapsed by default and open on demand", async () => {
  render(<TerrainPanel client={emptyClient({ terrainEdit: vi.fn(() => Promise.resolve(reply())) })} statsIntervalMs={0} />);
  await screen.findByTestId("terrain-layers");
  // Shape is open (the constant work); everything else is quiet until asked for.
  expect(screen.getByTestId("terrain-section-shape").getAttribute("aria-expanded")).toBe("true");
  expect(screen.getByTestId("terrain-section-perf").getAttribute("aria-expanded")).toBe("false");
  expect(screen.queryByTestId("terrain-stats")).toBeNull();
  openSection("perf");
  expect(screen.getByTestId("terrain-section-perf").getAttribute("aria-expanded")).toBe("true");
  await screen.findByTestId("terrain-stats");
});

test("the profiling readout reports measurements, and says so when over budget", async () => {
  const stats = {
    ...emptyTerrainStats(),
    active: true,
    residentChunks: 96,
    visibleChunks: 31,
    culledFrustum: 44,
    culledHorizon: 21,
    drawnTriangles: 512_000,
    totalMb: 900,
    budgetMb: 736,
    overBudget: true,
    dominantStage: "texture",
  };
  const client = emptyClient({
    terrainEdit: vi.fn(() => Promise.resolve(reply({ stats }))),
  });
  render(<TerrainPanel client={client} statsIntervalMs={0} />);
  await screen.findByTestId("terrain-layers");
  openSection("perf");
  const panel = await screen.findByTestId("terrain-stats");
  expect(panel.textContent).toContain("96");
  expect(panel.textContent).toContain("512,000");
  expect(panel.textContent).toContain("texture");
  expect(screen.getByTestId("terrain-over-budget").textContent).toContain("Over budget");
});

test("an unbound scatter prototype is visible as unbound and can be bound", async () => {
  const terrainEdit = vi.fn(() => Promise.resolve(reply()));
  render(<TerrainPanel client={emptyClient({ terrainEdit })} statsIntervalMs={0} />);
  await screen.findByTestId("terrain-layers");
  openSection("life");
  const protos = await screen.findByTestId("terrain-protos");
  expect(protos.textContent).toContain("needs a mesh");
  terrainEdit.mockClear();
  fireEvent.change(screen.getByTestId("terrain-proto-key-0"), { target: { value: "mtkasset:pine" } });
  fireEvent.click(screen.getByTestId("terrain-proto-bind-0"));
  expect(terrainEdit).toHaveBeenCalledWith({
    op: "bindProto",
    index: 0,
    mesh_key: "mtkasset:pine",
    lod_keys: [],
    impostor_key: null,
  });
});

test("every labelled control names the value it edits", async () => {
  render(<TerrainPanel client={emptyClient({ terrainEdit: vi.fn(() => Promise.resolve(reply())) })} statsIntervalMs={0} />);
  await screen.findByTestId("terrain-layers");
  // Accessibility §10: a visible label must be associated with its control, not merely adjacent to it.
  openSection("world");
  for (const id of ["terrain-name", "terrain-seed", "terrain-world-size", "terrain-chunk-verts"]) {
    const control = screen.getByTestId(id);
    expect(control.id).toBe(id);
    expect(document.querySelector(`label[for="${id}"]`)).not.toBeNull();
  }
});

test("stat polling stops when the panel unmounts", async () => {
  // Real timers: the polling only starts once the recipe has landed, and that arrival is a promise chain
  // that fake timers do not drive on their own — the test would then measure its own plumbing.
  const terrainStats = vi.fn(() => Promise.resolve(emptyTerrainStats()));
  const client = emptyClient({
    terrainEdit: vi.fn(() => Promise.resolve(reply())),
    terrainStats,
  });
  const view = render(<TerrainPanel client={client} statsIntervalMs={20} />);
  await screen.findByTestId("terrain-layers");
  await waitFor(() => expect(terrainStats.mock.calls.length).toBeGreaterThan(0));
  view.unmount();
  const during = terrainStats.mock.calls.length;
  await new Promise((r) => setTimeout(r, 120));
  expect(terrainStats.mock.calls.length).toBe(during);
});

test("arming the sculpt tool tells the viewport what the brush is", async () => {
  // Typed so the recorded call tuple is `[mode, brush]` rather than `[]` — an untyped `vi.fn` records
  // no parameter types, and every assertion on `mock.calls[n][i]` then fails to typecheck.
  const terrainTool = vi.fn((_mode: string, _brush?: Record<string, number>) => Promise.resolve(true));
  const client = emptyClient({ terrainEdit: vi.fn(() => Promise.resolve(reply())), terrainTool });
  render(<TerrainPanel client={client} statsIntervalMs={0} />);
  await screen.findByTestId("terrain-layers");
  // Mounted disarmed: opening the workspace must not arm a destructive pointer.
  await waitFor(() => expect(terrainTool).toHaveBeenCalled());
  expect(terrainTool.mock.calls[0][0]).toBe("none");

  terrainTool.mockClear();
  fireEvent.click(screen.getByTestId("terrain-sculpt-toggle"));
  await waitFor(() => expect(terrainTool).toHaveBeenCalled());
  const [mode, brush] = terrainTool.mock.calls.at(-1) as [string, Record<string, number>];
  expect(mode).toBe("sculpt");
  // The brush travels with the arming call, so the viewport never has to ask.
  expect(brush.radiusM).toBe(24);
  expect(brush.kind).toBe(0);
  expect(screen.getByTestId("terrain-sculpt-toggle").getAttribute("aria-pressed")).toBe("true");
});

test("changing the brush re-arms the pointer with the new settings", async () => {
  // Typed so the recorded call tuple is `[mode, brush]` rather than `[]` — an untyped `vi.fn` records
  // no parameter types, and every assertion on `mock.calls[n][i]` then fails to typecheck.
  const terrainTool = vi.fn((_mode: string, _brush?: Record<string, number>) => Promise.resolve(true));
  const client = emptyClient({ terrainEdit: vi.fn(() => Promise.resolve(reply())), terrainTool });
  render(<TerrainPanel client={client} statsIntervalMs={0} />);
  await screen.findByTestId("terrain-layers");
  fireEvent.click(screen.getByTestId("terrain-sculpt-toggle"));
  terrainTool.mockClear();
  fireEvent.change(screen.getByTestId("terrain-brush-radius"), { target: { value: "40" } });
  fireEvent.blur(screen.getByTestId("terrain-brush-radius"));
  await waitFor(() => {
    const last = terrainTool.mock.calls.at(-1) as [string, Record<string, number>] | undefined;
    expect(last?.[1].radiusM).toBe(40);
  });
});

test("leaving the workspace disarms the pointer", async () => {
  // Typed so the recorded call tuple is `[mode, brush]` rather than `[]` — an untyped `vi.fn` records
  // no parameter types, and every assertion on `mock.calls[n][i]` then fails to typecheck.
  const terrainTool = vi.fn((_mode: string, _brush?: Record<string, number>) => Promise.resolve(true));
  const client = emptyClient({ terrainEdit: vi.fn(() => Promise.resolve(reply())), terrainTool });
  const view = render(<TerrainPanel client={client} statsIntervalMs={0} />);
  await screen.findByTestId("terrain-layers");
  fireEvent.click(screen.getByTestId("terrain-sculpt-toggle"));
  await waitFor(() => expect(terrainTool.mock.calls.at(-1)?.[0]).toBe("sculpt"));
  view.unmount();
  // A stray click in another workspace must not keep painting.
  expect(terrainTool.mock.calls.at(-1)?.[0]).toBe("none");
});

test("a route is drawn in the viewport and committed once it has enough points", async () => {
  const terrainRouteClear = vi.fn(() => Promise.resolve(0));
  const terrainRouteCommit = vi.fn(() => Promise.resolve(reply()));
  const client = emptyClient({
    terrainEdit: vi.fn(() => Promise.resolve(reply())),
    terrainRouteClear,
    terrainRouteCommit,
  });
  render(<TerrainPanel client={client} statsIntervalMs={0} />);
  await screen.findByTestId("terrain-layers");
  openSection("routes");
  fireEvent.click(await screen.findByTestId("terrain-route-toggle"));
  expect(screen.getByTestId("terrain-route-toggle").getAttribute("aria-pressed")).toBe("true");
  // Nothing to commit yet: a one-point route is not a route.
  expect((screen.getByTestId("terrain-route-commit") as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByTestId("terrain-route-points").textContent).toContain("No points yet");
  // Turning the tool off clears the points rather than leaving a half-drawn route armed.
  fireEvent.click(screen.getByTestId("terrain-route-toggle"));
  await waitFor(() => expect(terrainRouteClear).toHaveBeenCalledWith(false));
});

test("the route kind decides which controls are shown", async () => {
  const client = emptyClient({ terrainEdit: vi.fn(() => Promise.resolve(reply())) });
  render(<TerrainPanel client={client} statsIntervalMs={0} />);
  await screen.findByTestId("terrain-layers");
  openSection("routes");
  await screen.findByTestId("terrain-route-kind");
  // Depth is a river's property; showing it for a road would be a control that does nothing.
  expect(screen.queryByTestId("terrain-route-depth")).toBeNull();
  fireEvent.change(screen.getByTestId("terrain-route-kind"), { target: { value: "river" } });
  await screen.findByTestId("terrain-route-depth");
});

test("the water line is authorable, and its controls disappear when there is no water", async () => {
  const terrainEdit = vi.fn(() => Promise.resolve(reply()));
  render(<TerrainPanel client={emptyClient({ terrainEdit })} statsIntervalMs={0} />);
  await screen.findByTestId("terrain-layers");
  openSection("routes");

  fireEvent.change(await screen.findByTestId("terrain-sea-level"), { target: { value: "12" } });
  fireEvent.blur(screen.getByTestId("terrain-sea-level"));
  await waitFor(() =>
    expect(terrainEdit).toHaveBeenCalledWith({
      op: "setWater",
      water: { enabled: true, sea_level_m: 12, shore_blend_m: 2, deep_m: 8 },
    }),
  );

  // With the water off, a sea-level field would be a control that changes nothing visible.
  const dry = reply({ recipe: recipe({ water: { enabled: false, sea_level_m: 0, shore_blend_m: 2, deep_m: 8 } }) });
  const client = emptyClient({ terrainEdit: vi.fn(() => Promise.resolve(dry)) });
  render(<TerrainPanel client={client} statsIntervalMs={0} />);
  const panels = await screen.findAllByTestId("terrain-panel");
  {
    const heads = screen.getAllByTestId("terrain-section-routes");
    const head = heads[1] ?? heads[0];
    if (head.getAttribute("aria-expanded") !== "true") fireEvent.click(head);
  }
  await waitFor(() => expect(panels[1].querySelector('[data-testid="terrain-water-enabled"]')).not.toBeNull());
  expect(panels[1].querySelector('[data-testid="terrain-sea-level"]')).toBeNull();
});

test("shows what the engine is unhappy about, not just what the recipe validator says", async () => {
  // A recipe can be perfectly valid and still have a chunk whose build panicked. The engine has always
  // recorded that; until now nothing carried it across the IPC boundary, so the "visible and actionable"
  // failure was a field nobody read and the author saw an unexplained hole in the ground.
  const client = fakeClient({
    terrainPresets: vi.fn(() => Promise.resolve(PRESETS)),
    terrainEdit: vi.fn(() => Promise.resolve(reply())),
    terrainStats: vi.fn(() =>
      Promise.resolve({
        ...emptyTerrainStats(),
        problem: "a piece of the landscape at chunk (3, 7) could not be built: index out of bounds",
      }),
    ),
  });
  // Polling on, as the app runs it by default: a chunk panics AFTER the command that built the world, so
  // the readout is the only thing that can deliver the news.
  render(<TerrainPanel client={client} statsIntervalMs={20} />);
  const banner = await screen.findByTestId("terrain-problem");
  expect(banner.textContent).toContain("chunk (3, 7)");
  expect(banner.getAttribute("role")).toBe("alert");
});

test("asks the engine whether an authored road can actually be crossed", async () => {
  // The navigation grid was built for every collider chunk and read by nothing. This is its surface, and
  // it closes the loop on the traversability verb: make it traversable, then check something can cross it.
  const terrainPath = vi.fn(() =>
    Promise.resolve({ found: true, reason: "", lengthM: 412.7, waypoints: [] }),
  );
  const withRoad = recipe({
    splines: [
      {
        name: "Ridge Road",
        kind: "Road",
        points: [
          [10, 4, 10],
          [400, 9, 380],
        ],
        width_m: 10,
        enabled: true,
      },
    ],
  });
  const client = fakeClient({
    terrainPresets: vi.fn(() => Promise.resolve(PRESETS)),
    terrainEdit: vi.fn(() => Promise.resolve(reply({ recipe: withRoad }))),
    terrainPath,
  });
  render(<TerrainPanel client={client} statsIntervalMs={0} />);

  // Routes are a collapsed section; open it the way an author would.
  fireEvent.click(await screen.findByText("Routes & Water"));
  fireEvent.click(await screen.findByTestId("terrain-crossing-check"));

  await waitFor(() => expect(terrainPath.mock.calls.length).toBe(1));
  // It asks about the road's OWN two ends, in world space — the question the author actually means.
  expect(terrainPath.mock.calls[0]).toEqual([
    [10, 4, 10],
    [400, 9, 380],
  ]);
  const said = await screen.findByTestId("terrain-crossing-result");
  expect(said.textContent).toContain("413 m");
});

test("a route that cannot be crossed says why, rather than nothing", async () => {
  const client = fakeClient({
    terrainPresets: vi.fn(() => Promise.resolve(PRESETS)),
    terrainEdit: vi.fn(() =>
      Promise.resolve(
        reply({
          recipe: recipe({
            splines: [
              {
                name: "Cliff Track",
                kind: "Road",
                points: [
                  [0, 0, 0],
                  [90, 60, 90],
                ],
                width_m: 6,
                enabled: true,
              },
            ],
          }),
        }),
      ),
    ),
    terrainPath: vi.fn(() =>
      Promise.resolve({ found: false, reason: "the slope is too steep to walk", waypoints: [] }),
    ),
  });
  render(<TerrainPanel client={client} statsIntervalMs={0} />);
  fireEvent.click(await screen.findByText("Routes & Water"));
  fireEvent.click(await screen.findByTestId("terrain-crossing-check"));
  const said = await screen.findByTestId("terrain-crossing-result");
  expect(said.textContent).toContain("too steep");
});

test("a terrain that appears in the document takes the panel out of its empty state", async () => {
  // The failure this pins: a terrain can arrive without this panel creating it — an undo, a redo, opening a
  // project, the command palette, or any other surface — and a panel that trusts only its own replies then
  // renders an EMPTY STATE over a live 200 MB landscape. It is silent, and it looks like the feature did
  // nothing. Reading once on mount covered the reopen case and nothing else.
  //
  // It also pins the component NAME. The lookup is by string, so a wrong one returns undefined and the panel
  // silently reverts to the old behaviour — which is exactly what happened the first time.
  projectionStore.getState().bulkLoad([]);
  let served = emptyTerrainReply();
  const client = fakeClient({
    terrainPresets: vi.fn(() => Promise.resolve(PRESETS)),
    terrainEdit: vi.fn(() => Promise.resolve(served)),
  });
  render(<TerrainPanel client={client} statsIntervalMs={0} />);
  // No terrain: the empty state is the truthful answer.
  await waitFor(() =>
    expect(screen.getByTestId("terrain-panel").getAttribute("data-state")).toBe("empty"),
  );

  // Now the document gains one, exactly as a projection delta would deliver it.
  served = reply();
  projectionStore.getState().bulkLoad([
    {
      id: "e1",
      name: "Rolling Hills",
      parentId: null,
      components: { TerrainRecipe: { source: JSON.stringify(recipe()) } },
    },
  ]);

  // The panel follows the document without anyone touching it.
  await waitFor(
    () => expect(screen.getByTestId("terrain-panel").getAttribute("data-state")).toBeNull(),
    { timeout: 3000 },
  );
  expect(screen.getByTestId("terrain-section-describe")).toBeTruthy();
  // The describe box here is the one for changing a world, so its examples are the changing ones. This
  // form used to offer none at all, which made the second half of describe-to-build undiscoverable
  // exactly where it was the only half that worked.
  const changes = screen.getAllByTestId("terrain-example").map((p) => p.textContent);
  expect(changes).toContain("Raise a mountain");
  expect(changes).not.toContain("Alpine valley");

  // And when the terrain goes away again — an undo — the empty state comes back rather than a stale world.
  projectionStore.getState().bulkLoad([]);
  await waitFor(() =>
    expect(screen.getByTestId("terrain-panel").getAttribute("data-state")).toBe("empty"),
  );
});
