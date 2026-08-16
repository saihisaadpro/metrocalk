import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { projectionStore } from "../store/projection";
import { toastStore } from "../store/toasts";
import { fakeClient } from "../transport/test-client";
import { ShapeStudio } from "./ShapeStudio";

afterEach(() => {
  act(() => projectionStore.getState().reset());
  vi.restoreAllMocks();
});

function seedEntity(id: string, name: string, components: Record<string, Record<string, unknown>> = {}) {
  act(() => {
    projectionStore.getState().applyDelta({
      ops: [
        { op: "upsert", id, name, parentId: null },
        ...Object.entries(components).flatMap(([component, fields]) =>
          Object.entries(fields).map(([field, value]) => ({
            op: "setField" as const,
            id,
            component,
            field,
            value: value as never,
          })),
        ),
      ],
    });
  });
}

test("the shape grid renders the catalog and a click creates, selects and toasts with the undo hint", async () => {
  const client = fakeClient({ gizmoSelect: vi.fn(() => Promise.resolve(true)) });
  render(<ShapeStudio client={client} />);

  // The catalog loads and each kind becomes a card.
  const boxCard = await screen.findByTestId("shape-card-box");
  expect(screen.getByTestId("shape-card-sphere")).toBeTruthy();

  fireEvent.click(boxCard);
  await waitFor(() => expect(client.shapeSpawn).toHaveBeenCalledWith("box"));
  // The created thing is SELECTED (close the loop) and the toast carries the undo hint at the gesture.
  await waitFor(() => expect(projectionStore.getState().selectedId).toBe("shape-box"));
  expect(client.gizmoSelect).toHaveBeenCalledWith("shape-box");
  const toasts = toastStore.getState().toasts;
  expect(toasts.some((t) => t.text.includes("Ctrl-Z to undo"))).toBe(true);
});

test("a refusal is explained inline and changes no selection", async () => {
  const client = fakeClient({
    shapeSpawn: vi.fn(() =>
      Promise.resolve({ created: null, handle: null, triangles: 0, ms: 0, message: "no", reason: "the shape engine is unavailable" }),
    ),
  });
  render(<ShapeStudio client={client} />);
  fireEvent.click(await screen.findByTestId("shape-card-box"));

  const refusal = await screen.findByTestId("shape-refusal");
  expect(refusal.textContent).toContain("the shape engine is unavailable");
  expect(projectionStore.getState().selectedId).toBeNull();
});

test("drawing: Create is disabled under three points with a reason, presets fill an outline, create clears it", async () => {
  const client = fakeClient();
  render(<ShapeStudio client={client} />);
  await screen.findByTestId("shape-card-box");

  const create = screen.getByTestId("draw-create") as HTMLButtonElement;
  expect(create.disabled).toBe(true);
  expect(create.title).toContain("at least three points");

  // One click on the star preset fills a ten-point outline; Create becomes live.
  fireEvent.click(screen.getByTestId("draw-preset-star"));
  await waitFor(() => expect((screen.getByTestId("draw-create") as HTMLButtonElement).disabled).toBe(false));

  fireEvent.click(screen.getByTestId("draw-create"));
  await waitFor(() => expect(client.shapeDraw).toHaveBeenCalled());
  const [mode, profile, height] = (client.shapeDraw as ReturnType<typeof vi.fn>).mock.calls[0] as [
    string,
    [number, number][],
    number,
  ];
  expect(mode).toBe("extrude");
  expect(profile.length).toBe(10);
  expect(height).toBe(1);
  // The canvas resets after a successful create (ready for the next drawing).
  await waitFor(() => expect((screen.getByTestId("draw-create") as HTMLButtonElement).disabled).toBe(true));
});

test("revolve mode maps the profile off the spin axis and passes segments in the right slot", async () => {
  const client = fakeClient();
  render(<ShapeStudio client={client} />);
  await screen.findByTestId("shape-card-box");

  fireEvent.click(screen.getByTestId("draw-mode-revolve"));
  fireEvent.click(screen.getByTestId("draw-preset-star"));
  fireEvent.click(screen.getByTestId("draw-create"));
  await waitFor(() => expect(client.shapeDraw).toHaveBeenCalled());
  const [mode, profile, height, segments] = (client.shapeDraw as ReturnType<typeof vi.fn>).mock
    .calls[0] as [string, [number, number][], number, number];
  expect(mode).toBe("revolve");
  expect(segments).toBe(48);
  expect(height).toBe(1); // the extrude height still rides its own slot untouched
  // Revolve mapping: x becomes the distance from the axis (canvas-left) — the star preset sits
  // around canvas x≈50, so every radius lands well off the axis; y flips to point upward.
  for (const [r] of profile) {
    expect(r).toBeGreaterThan(1.5);
  }
});

test("a refused parameter edit resyncs the field instead of re-submitting the refusal", async () => {
  const client = fakeClient({
    shapeUpdate: vi.fn(() =>
      Promise.resolve({ created: null, handle: null, triangles: 0, ms: 0, message: "no", reason: "radius must be between 0.05 and 25 metres" }),
    ),
  });
  seedEntity("s1", "Sphere", {
    ShapeRecipe: { source: JSON.stringify({ v: 1, kind: "sphere", params: { radius: 0.5, segments: 32 } }), kind: "sphere" },
  });
  act(() => projectionStore.getState().select("s1"));
  render(<ShapeStudio client={client} />);

  const radius = (await screen.findByTestId("shape-param-radius")) as HTMLInputElement;
  fireEvent.focus(radius);
  fireEvent.change(radius, { target: { value: "30" } });
  fireEvent.blur(radius);
  await screen.findByTestId("shape-refusal");
  expect(client.shapeUpdate).toHaveBeenCalledTimes(1);
  // The remounted field shows the document's real value again — no sticky refused text.
  const resynced = (await screen.findByTestId("shape-param-radius")) as HTMLInputElement;
  await waitFor(() => expect(resynced.value).toBe("0.5"));
  // And a blur without change does NOT fire another update (no burned undo steps).
  fireEvent.focus(resynced);
  fireEvent.blur(resynced);
  await new Promise((r) => setTimeout(r, 50));
  expect(client.shapeUpdate).toHaveBeenCalledTimes(1);
});

/** jsdom has no layout: give the canvas a rect so pointer coordinates map into view space, and
 *  dispatch MouseEvents with pointer types (jsdom's PointerEvent drops clientX). */
function armCanvas() {
  const canvas = screen.getByTestId("draw-canvas");
  vi.spyOn(canvas, "getBoundingClientRect").mockReturnValue({
    x: 0, y: 0, left: 0, top: 0, width: 200, height: 140, right: 200, bottom: 140,
    toJSON: () => ({}),
  } as DOMRect);
  const at = (type: string, clientX: number, clientY: number) =>
    fireEvent(canvas, new MouseEvent(type, { clientX, clientY, bubbles: true }));
  return {
    canvas,
    click: (x: number, y: number) => {
      at("pointerdown", x, y);
      at("pointerup", x, y);
    },
    down: (x: number, y: number) => at("pointerdown", x, y),
    move: (x: number, y: number) => at("pointermove", x, y),
    up: (x: number, y: number) => at("pointerup", x, y),
  };
}

test("canvas clicks add snapped points, the readout shows real metres, undo-point removes the last", async () => {
  const client = fakeClient();
  render(<ShapeStudio client={client} />);
  await screen.findByTestId("shape-card-box");
  const { canvas, click } = armCanvas();

  // Clicks land off-grid; snap (on by default) pulls them to the 0.25 m lattice.
  click(21, 19);
  click(119, 21);
  click(71, 99);
  expect(canvas.getAttribute("aria-label")).toContain("3 so far");
  // Snapped canvas points (10,10) (60,10) (35,50) → a 5.00 m × 4.00 m plan.
  expect(screen.getByTestId("draw-dims").textContent).toContain("5.00 × 4.00 m · 3 points");

  fireEvent.click(screen.getByTestId("draw-undo"));
  expect(canvas.getAttribute("aria-label")).toContain("2 so far");
});

test("dragging across the canvas draws a freehand stroke, simplified to its corners", async () => {
  const client = fakeClient();
  render(<ShapeStudio client={client} />);
  await screen.findByTestId("shape-card-box");
  const { canvas, down, move, up } = armCanvas();

  down(40, 28);
  move(80, 28);
  move(120, 28);
  move(120, 63);
  move(120, 98);
  move(80, 98);
  move(40, 98);
  up(40, 98);
  // Seven samples collapse to the four corners of the rectangle they trace.
  expect(canvas.getAttribute("aria-label")).toContain("4 so far");
  expect(screen.getByTestId("draw-dims").textContent).toContain("4.00 × 3.50 m · 4 points");
});

test("dragging an existing point moves it with snap — precision editing on the canvas", async () => {
  const client = fakeClient();
  render(<ShapeStudio client={client} />);
  await screen.findByTestId("shape-card-box");
  const { click, down, move, up } = armCanvas();

  click(20, 20); // (10,10)u
  click(120, 20); // (60,10)u
  click(70, 100); // (35,50)u
  // Grab the first point (client 20,20 = canvas 10,10) and pull it to canvas (30,10).
  down(20, 20);
  move(61, 21);
  up(61, 21);
  expect(screen.getByTestId("draw-dims").textContent).toContain("3.00 × 4.00 m · 3 points");
});

test("the snap toggle frees clicks from the grid", async () => {
  const client = fakeClient();
  render(<ShapeStudio client={client} />);
  await screen.findByTestId("shape-card-box");
  const { click } = armCanvas();

  fireEvent.click(screen.getByTestId("draw-snap")); // off
  click(21, 19); // canvas (10.5, 9.5) stays unsnapped
  click(121, 19); // (60.5, 9.5)
  click(21, 99); // (10.5, 49.5)
  expect(screen.getByTestId("draw-dims").textContent).toContain("5.00 × 4.00 m · 3 points");
  // Same span as the snapped test but from raw positions — 60.5−10.5 and 49.5−9.5 exactly.
});

test("the taper rides shapeDraw and the gear preset fills a 36-point outline", async () => {
  const client = fakeClient();
  render(<ShapeStudio client={client} />);
  await screen.findByTestId("shape-card-box");

  const taper = (await screen.findByTestId("draw-taper")) as HTMLInputElement;
  fireEvent.focus(taper);
  fireEvent.change(taper, { target: { value: "0.4" } });
  fireEvent.blur(taper);

  fireEvent.click(screen.getByTestId("draw-preset-gear"));
  expect(screen.getByTestId("draw-canvas").getAttribute("aria-label")).toContain("36 so far");
  fireEvent.click(screen.getByTestId("draw-create"));
  await waitFor(() => expect(client.shapeDraw).toHaveBeenCalled());
  const call = (client.shapeDraw as ReturnType<typeof vi.fn>).mock.calls[0] as unknown[];
  expect(call[0]).toBe("extrude");
  expect((call[1] as [number, number][]).length).toBe(36);
  expect(call[4]).toBe(0.4);
});

test("combine asks for two objects, then joins them and selects the result", async () => {
  const client = fakeClient();
  seedEntity("a1", "Box");
  seedEntity("b2", "Sphere");
  render(<ShapeStudio client={client} />);
  await screen.findByTestId("shape-card-box");

  // With fewer than two selected: an explained hint, disabled verbs with the reason in the title.
  expect(screen.getByTestId("combine-hint").textContent).toContain("Select two objects");
  expect((screen.getByTestId("combine-union") as HTMLButtonElement).disabled).toBe(true);

  act(() => {
    projectionStore.getState().select("a1");
    projectionStore.getState().toggleSelect("b2");
  });
  await waitFor(() => expect((screen.getByTestId("combine-union") as HTMLButtonElement).disabled).toBe(false));

  fireEvent.click(screen.getByTestId("combine-union"));
  await waitFor(() => expect(client.shapeCombine).toHaveBeenCalledWith("a1", "b2", "union"));
  await waitFor(() => expect(projectionStore.getState().selectedId).toBe("combined-union"));
});

test("meld passes the blend radius through", async () => {
  const client = fakeClient();
  seedEntity("a1", "Sphere A");
  seedEntity("b2", "Sphere B");
  render(<ShapeStudio client={client} />);
  await screen.findByTestId("shape-card-box");

  act(() => {
    projectionStore.getState().select("a1");
    projectionStore.getState().toggleSelect("b2");
  });
  await waitFor(() => expect((screen.getByTestId("combine-meld") as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByTestId("combine-meld"));
  await waitFor(() => expect(client.shapeMeld).toHaveBeenCalledWith("a1", "b2", 0.25));
});

test("the selected shape shows live parameters and a commit re-bakes through shapeUpdate", async () => {
  const client = fakeClient();
  seedEntity("s1", "Sphere", {
    ShapeRecipe: { source: JSON.stringify({ v: 1, kind: "sphere", params: { radius: 0.5, segments: 32 } }), kind: "sphere" },
  });
  act(() => projectionStore.getState().select("s1"));
  render(<ShapeStudio client={client} />);
  await screen.findByTestId("shape-params");

  const radius = (await screen.findByTestId("shape-param-radius")) as HTMLInputElement;
  expect(radius.value).toBe("0.5");
  fireEvent.focus(radius);
  fireEvent.change(radius, { target: { value: "0.8" } });
  fireEvent.blur(radius);
  await waitFor(() => expect(client.shapeUpdate).toHaveBeenCalledWith("s1", { radius: 0.8 }));
});

test("a combined shape explains that its parameters live in its sources", async () => {
  const client = fakeClient();
  seedEntity("c1", "Union of Box and Sphere", {
    ShapeRecipe: { source: JSON.stringify({ v: 1, kind: "union", sources: ["x", "y"] }), kind: "union" },
  });
  act(() => projectionStore.getState().select("c1"));
  render(<ShapeStudio client={client} />);

  const params = await screen.findByTestId("shape-params");
  expect(params.textContent).toContain("no editable parameters");
});
