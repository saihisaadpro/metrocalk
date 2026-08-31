import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, test, vi, afterEach } from "vitest";
import { GroundSketch } from "./GroundSketch";
import { fakeClient } from "../transport/test-client";
import { projectionStore } from "../store/projection";
import { toastStore } from "../store/toasts";
import type { GroundSketchState } from "../transport/protocol";

/** One read-model, overridden field by field — the same discipline the fake client uses, so a test
 *  states only the part it is about and cannot drift from the reply's real shape. */
function state(over: Partial<GroundSketchState> = {}): GroundSketchState {
  return {
    active: true,
    points: [],
    cursor: null,
    snap: "",
    closes: false,
    closed: false,
    segmentM: 0,
    perimeterM: 0,
    areaM2: 0,
    widthM: 0,
    depthM: 0,
    planeY: 0,
    gridM: 0.25,
    angleSnap: true,
    canBuild: false,
    message: "nothing is drawn yet — click on the ground to place the first corner",
    ...over,
  };
}

const RECTANGLE = state({
  points: [
    [0, 0, 0],
    [12, 0, 0],
    [12, 0, 8],
    [0, 0, 8],
  ],
  cursor: [0, 0, 0],
  snap: "closes the shape",
  closes: true,
  widthM: 12,
  depthM: 8,
  areaM2: 96,
  perimeterM: 40,
  segmentM: 8,
  canBuild: true,
  message: "4 corners · 12.00 × 8.00 m · 96.0 m² — click the first corner again to finish, or raise it now",
});

afterEach(() => {
  toastStore.getState().reset();
});

test("the readout is the drawing's real dimensions, not a count of clicks", () => {
  render(<GroundSketch client={fakeClient()} state={RECTANGLE} onState={vi.fn()} />);

  // The measurement, in metres, is the largest thing on the panel — this is what the author watches.
  expect(screen.getByTestId("ground-sketch-dims").textContent).toContain("12.00 × 8.00 m");
  expect(screen.getByTestId("ground-sketch-dims").textContent).toContain("4 corners");
  expect(screen.getByTestId("ground-sketch-dims").textContent).toContain("96.00 m²");
  expect(screen.getByTestId("ground-sketch-dims").textContent).toContain("40.00 m around");
  // And the live segment names what the snap decided, so a snapped corner is a claim, not a surprise.
  expect(screen.getByTestId("ground-sketch-live").textContent).toContain("next 8.00 m");
  expect(screen.getByTestId("ground-sketch-live").textContent).toContain("closes the shape");
});

test("with the cursor off the ground the panel says so instead of showing a stale distance", () => {
  render(<GroundSketch client={fakeClient()} state={state({ points: [[0, 0, 0]], segmentM: 4 })} onState={vi.fn()} />);
  expect(screen.getByTestId("ground-sketch-live").textContent).toBe("point at the ground to aim");
});

test("raising the outline places, selects and says what it cost, with the way back", async () => {
  const sketchCommit = vi.fn(() => Promise.resolve({ created: "e9", handle: "mtkasset:x", triangles: 48, ms: 4, message: "Raised your drawing into a solid · 48 triangles", reason: null }));
  const gizmoSelect = vi.fn(() => Promise.resolve(true));
  render(<GroundSketch client={fakeClient({ sketchCommit, gizmoSelect })} state={RECTANGLE} onState={vi.fn()} />);

  fireEvent.click(screen.getByTestId("ground-sketch-raise"));

  await waitFor(() => expect(sketchCommit).toHaveBeenCalledWith(3));
  await waitFor(() => expect(projectionStore.getState().selectedId).toBe("e9"));
  expect(gizmoSelect).toHaveBeenCalledWith("e9");
  const toast = toastStore.getState().toasts.at(-1);
  expect(toast?.kind).toBe("success");
  expect(toast?.text).toContain("Ctrl-Z to undo");
});

test("a refusal is shown in the engine's own words and changes nothing", async () => {
  const sketchCommit = vi.fn(() => Promise.resolve({ created: null, handle: null, triangles: 0, ms: 0, message: "", reason: "the corners are all on one line, so the outline encloses nothing" }));
  projectionStore.getState().select(null);
  render(<GroundSketch client={fakeClient({ sketchCommit })} state={{ ...RECTANGLE, areaM2: 0 }} onState={vi.fn()} />);

  fireEvent.click(screen.getByTestId("ground-sketch-raise"));

  await waitFor(() => expect(toastStore.getState().toasts.at(-1)?.kind).toBe("error"));
  expect(toastStore.getState().toasts.at(-1)?.text).toContain("all on one line");
  expect(projectionStore.getState().selectedId).toBeNull();
});

test("every control that refuses says why, in words, before it is pressed", () => {
  render(<GroundSketch client={fakeClient()} state={state()} onState={vi.fn()} />);

  const raise = screen.getByTestId("ground-sketch-raise") as HTMLButtonElement;
  expect(raise.disabled).toBe(true);
  expect(raise.title).toBe("Click on the ground to place the first corner");
  expect((screen.getByTestId("ground-sketch-place") as HTMLButtonElement).title).toContain("a length is measured from it");
  expect((screen.getByTestId("ground-sketch-undo") as HTMLButtonElement).title).toBe("There is no corner to take back");
  expect((screen.getByTestId("ground-sketch-clear") as HTMLButtonElement).title).toBe("There is nothing drawn to clear");
});

test("a half-drawn outline explains what it still needs rather than going silently dark", () => {
  render(<GroundSketch client={fakeClient()} state={state({ points: [[0, 0, 0], [4, 0, 0]], widthM: 4 })} onState={vi.fn()} />);
  const raise = screen.getByTestId("ground-sketch-raise") as HTMLButtonElement;
  expect(raise.disabled).toBe(true);
  expect(raise.title).toContain("three corners that enclose something — 2 so far");
});

test("the typed length places a corner at exactly that distance", async () => {
  const sketchPointExact = vi.fn(() => Promise.resolve(state({ points: [[0, 0, 0], [7.5, 0, 0]] })));
  const onState = vi.fn();
  render(<GroundSketch client={fakeClient({ sketchPointExact })} state={state({ points: [[0, 0, 0]] })} onState={onState} />);

  fireEvent.change(screen.getByTestId("ground-sketch-length"), { target: { value: "7.5" } });
  fireEvent.blur(screen.getByTestId("ground-sketch-length"));
  fireEvent.click(screen.getByTestId("ground-sketch-place"));

  await waitFor(() => expect(sketchPointExact).toHaveBeenCalledWith(7.5));
  await waitFor(() => expect(onState).toHaveBeenCalled());
});

test("the precision controls drive the engine's snap, not a copy of it in the panel", async () => {
  const sketchTool = vi.fn(() => Promise.resolve(state({ gridM: 1 })));
  render(<GroundSketch client={fakeClient({ sketchTool })} state={state()} onState={vi.fn()} />);

  fireEvent.change(screen.getByTestId("ground-sketch-grid"), { target: { value: "1" } });
  await waitFor(() => expect(sketchTool).toHaveBeenCalledWith(true, 1, true));

  fireEvent.click(screen.getByTestId("ground-sketch-angle"));
  await waitFor(() => expect(sketchTool).toHaveBeenLastCalledWith(true, 0.25, false));
});

test("undo and start over are two different promises, and both go to the engine", async () => {
  const sketchUndo = vi.fn(() => Promise.resolve(state({ points: [[0, 0, 0]] })));
  const sketchClear = vi.fn(() => Promise.resolve(state()));
  render(<GroundSketch client={fakeClient({ sketchUndo, sketchClear })} state={RECTANGLE} onState={vi.fn()} />);

  fireEvent.click(screen.getByTestId("ground-sketch-undo"));
  await waitFor(() => expect(sketchUndo).toHaveBeenCalled());
  fireEvent.click(screen.getByTestId("ground-sketch-clear"));
  await waitFor(() => expect(sketchClear).toHaveBeenCalled());
});

test("the readout follows the cursor without the panel ever deciding what it says", async () => {
  vi.useFakeTimers();
  try {
    const sketchState = vi.fn(() => Promise.resolve(state({ points: [[0, 0, 0]], cursor: [3, 0, 0], segmentM: 3, snap: "grid" })));
    const onState = vi.fn();
    const { unmount } = render(<GroundSketch client={fakeClient({ sketchState })} state={state()} onState={onState} />);

    await vi.advanceTimersByTimeAsync(350);
    expect(sketchState.mock.calls.length).toBeGreaterThanOrEqual(3);
    expect(onState).toHaveBeenCalledWith(expect.objectContaining({ segmentM: 3, snap: "grid" }));

    // A poll that outlives its panel is a background IPC nobody asked for.
    unmount();
    const after = sketchState.mock.calls.length;
    await vi.advanceTimersByTimeAsync(500);
    expect(sketchState.mock.calls.length).toBe(after);
  } finally {
    vi.useRealTimers();
  }
});
