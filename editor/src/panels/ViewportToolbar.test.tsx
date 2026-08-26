import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { fakeClient } from "../transport/test-client";
import { ViewportToolbar } from "./ViewportToolbar";

afterEach(() => vi.useRealTimers());

async function settleToolbar() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
  });
}

test("Snap button sends the same state it displays", async () => {
  const setSnap = vi.fn();
  render(<ViewportToolbar client={fakeClient({ setSnap })} />);
  await settleToolbar();

  const transform = screen.getByTestId("vpTransform");
  fireEvent.click(transform);
  expect(screen.getByTestId("vpMove")).toBeTruthy();
  expect(screen.getByTestId("vpRotate")).toBeTruthy();
  expect(screen.getByTestId("vpScale")).toBeTruthy();
  let snap = screen.getByTestId("vpSnap");
  expect(snap.getAttribute("aria-checked")).toBe("true");
  await act(async () => {
    fireEvent.click(snap);
    await Promise.resolve();
  });
  expect(setSnap).toHaveBeenLastCalledWith(false);
  expect(transform.getAttribute("aria-expanded")).toBe("false");

  fireEvent.click(transform);
  snap = screen.getByTestId("vpSnap");
  expect(snap.getAttribute("aria-checked")).toBe("false");
  await act(async () => {
    fireEvent.click(snap);
    await Promise.resolve();
  });
  expect(setSnap).toHaveBeenLastCalledWith(true);
  expect(transform.getAttribute("aria-expanded")).toBe("false");
});

test("centres two persistent triggers and can defer transform modes to the primary tool rail", async () => {
  // A FAKE THAT DOES NOT CONTRADICT ITSELF. The toolbar treats `gizmo_debug` as the one authoritative
  // gizmo state and re-reads it on a slow poll — that is the whole "no desync" design. The default fake
  // answers `gizmoSpaceToggle` with "local" and then keeps answering `gizmoDebug` with "world" forever,
  // so whether this assertion passed came down to whether a poll happened to land between the click and
  // the read. Making the toggle move the state the poll reports is what the live engine does, and it is
  // what lets this test assert the behaviour instead of the race.
  let gizmoSpace = "world";
  const client = fakeClient({
    gizmoSpaceToggle: () => {
      gizmoSpace = gizmoSpace === "world" ? "local" : "world";
      return Promise.resolve(gizmoSpace);
    },
    gizmoDebug: () => Promise.resolve(["translate", false, false, gizmoSpace, "origin"]),
  });
  render(<ViewportToolbar client={client} showTransformTools={false} />);
  await settleToolbar();
  const toolbar = screen.getByTestId("vptoolbar");
  expect(toolbar.style.left).toBe("50%");
  expect(toolbar.style.transform).toBe("translateX(-50%)");
  expect(toolbar.querySelectorAll("button")).toHaveLength(2);
  expect(screen.getByTestId("vpTransform")).toBeTruthy();
  expect(screen.getByTestId("vpView")).toBeTruthy();
  expect(screen.queryByTestId("vpMove")).toBeNull();
  expect(screen.queryByTestId("vpRotate")).toBeNull();
  expect(screen.queryByTestId("vpScale")).toBeNull();

  expect(screen.queryByTestId("vpSpace")).toBeNull();
  fireEvent.click(screen.getByTestId("vpTransform"));
  expect(screen.getByRole("menu", { name: "Transform settings" })).toBeTruthy();
  expect(screen.getByTestId("vpSpace")).toBeTruthy();
  expect(screen.getByTestId("vpPivot")).toBeTruthy();
  expect(screen.getByTestId("vpSnap")).toBeTruthy();

  // WAIT FOR THE STATE, not for a fixed number of microtasks. Counting ticks makes the assertion a
  // claim about how many promises happen to be in flight in this component — it went red the day the
  // toolbar gained one more async read at mount, while the behaviour it is about was unchanged.
  fireEvent.click(screen.getByTestId("vpSpace"));
  await waitFor(() => expect(screen.getByTestId("vpTransform").textContent).toContain("Local"));
});

test("progressively discloses framing, camera, and rendering controls", async () => {
  const viewPreset = vi.fn();
  const setRenderProfile = vi.fn(() => Promise.resolve("cad" as const));
  render(<ViewportToolbar client={fakeClient({ viewPreset, setRenderProfile })} showTransformTools={false} />);
  await settleToolbar();

  const trigger = screen.getByTestId("vpView");
  expect(trigger.getAttribute("aria-expanded")).toBe("false");
  expect(screen.queryByTestId("vpFrameAll")).toBeNull();
  expect(screen.queryByTestId("vpTop")).toBeNull();
  expect(screen.queryByTestId("vpRenderProfile")).toBeNull();

  fireEvent.click(trigger);
  expect(trigger.getAttribute("aria-expanded")).toBe("true");
  expect(screen.getByRole("menu", { name: "View and framing" })).toBeTruthy();
  expect(screen.getByTestId("vpFrameSel").getAttribute("aria-disabled")).toBe("true");
  expect(screen.getByTestId("vpFrameSel").textContent).toContain("Select an object to frame it");
  expect(screen.getByTestId("vpFrameAll")).toBeTruthy();
  expect(screen.getByTestId("vpTop")).toBeTruthy();
  expect(screen.getByTestId("vpFront")).toBeTruthy();
  expect(screen.getByTestId("vpSide")).toBeTruthy();
  expect(screen.getByTestId("vpPersp")).toBeTruthy();
  expect(screen.getByTestId("vpOrient")).toBeTruthy();
  expect(screen.getByTestId("vpRenderProfile")).toBeTruthy();

  fireEvent.click(screen.getByTestId("vpTop"));
  expect(viewPreset).toHaveBeenCalledWith("top");
  expect(screen.queryByRole("menu", { name: "View and framing" })).toBeNull();

  fireEvent.click(trigger);
  await act(async () => {
    fireEvent.click(screen.getByTestId("vpRenderProfile"));
    await Promise.resolve();
  });
  expect(setRenderProfile).toHaveBeenCalledWith("cad");
  expect(screen.queryByRole("menu", { name: "View and framing" })).toBeNull();
});

test("supports arrow navigation, Escape dismissal, and focus restoration", async () => {
  render(<ViewportToolbar client={fakeClient()} showTransformTools={false} />);
  await settleToolbar();

  const trigger = screen.getByTestId("vpView");
  fireEvent.keyDown(trigger, { key: "ArrowDown" });
  const menu = screen.getByRole("menu", { name: "View and framing" });
  expect(document.activeElement).toBe(screen.getByTestId("vpFrameSel"));

  fireEvent.keyDown(menu, { key: "ArrowDown" });
  expect(document.activeElement).toBe(screen.getByTestId("vpFrameAll"));
  fireEvent.keyDown(menu, { key: "ArrowDown" });
  expect(document.activeElement).toBe(screen.getByTestId("vpTop"));
  fireEvent.keyDown(menu, { key: "End" });
  expect(document.activeElement).toBe(screen.getByTestId("vpRenderProfile"));

  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.queryByRole("menu", { name: "View and framing" })).toBeNull();
  expect(document.activeElement).toBe(trigger);
});

test("starts independent live reads together and schedules the next poll only after settlement", async () => {
  vi.useFakeTimers();
  let resolveFirstGizmo!: (value: ["translate", boolean, boolean, string, string]) => void;
  const gizmoDebug = vi
    .fn()
    .mockImplementationOnce(
      () =>
        new Promise<["translate", boolean, boolean, string, string]>((resolve) => {
          resolveFirstGizmo = resolve;
        }),
    )
    .mockResolvedValue(["translate", false, false, "world", "origin"]);
  const cameraDebug = vi.fn(() => Promise.resolve([0.785, 0.5, 60, 0, 0, 0]));
  const renderProfileDebug = vi.fn(() => Promise.resolve("cinematic" as const));

  const { unmount } = render(
    <ViewportToolbar client={fakeClient({ gizmoDebug, cameraDebug, renderProfileDebug })} />,
  );
  expect(gizmoDebug).toHaveBeenCalledTimes(1);
  expect(cameraDebug).toHaveBeenCalledTimes(1);
  expect(renderProfileDebug).toHaveBeenCalledTimes(1);

  act(() => vi.advanceTimersByTime(1_000));
  expect(gizmoDebug).toHaveBeenCalledTimes(1);

  await act(async () => {
    resolveFirstGizmo(["translate", false, false, "world", "origin"]);
    await Promise.resolve();
    await Promise.resolve();
  });
  act(() => vi.advanceTimersByTime(499));
  expect(gizmoDebug).toHaveBeenCalledTimes(1);
  await act(async () => {
    vi.advanceTimersByTime(1);
    await Promise.resolve();
  });
  expect(gizmoDebug).toHaveBeenCalledTimes(2);
  expect(cameraDebug).toHaveBeenCalledTimes(2);
  expect(renderProfileDebug).toHaveBeenCalledTimes(2);

  unmount();
  vi.useRealTimers();
});

// ── the View menu's Cameras group ────────────────────────────────────────────────────────────────
// This is where a scene camera is CREATED, and it is the half of the camera surface the `shots`
// harness cannot photograph on this branch: `MenuPopup` renders through `theme/Popover`, which
// portals to `document.body` by design, and every claim `shoot.mjs` evaluates is scoped to the shot
// frame. jsdom has no such boundary, so the coverage lives here until the portal fix lands and the
// capture is owed. Asserting the behaviour rather than the copy: which command fired, with what.

test("the View menu offers to save the current view, and says so when there are none yet", async () => {
  const client = fakeClient();
  client.sceneCameras = vi.fn(() => Promise.resolve([]));
  render(<ViewportToolbar client={client} />);
  await settleToolbar();

  fireEvent.click(screen.getByTestId("vpView"));
  expect(screen.getByTestId("vpSaveCamera")).toBeTruthy();
  // An empty list SAYS it is empty and explains the next step, rather than showing a bare heading.
  const none = screen.getByTestId("vpNoCameras");
  expect(none.getAttribute("aria-disabled")).toBe("true");
  expect(none.textContent).toMatch(/frame a view you like/i);
  // Nothing to leave, so no way out is offered.
  expect(screen.queryByTestId("vpFreeLook")).toBeNull();

  await act(async () => {
    fireEvent.click(screen.getByTestId("vpSaveCamera"));
    await Promise.resolve();
  });
  // The FIRST camera in a scene is authored active, so Play has something to render from at once.
  expect(client.addCameraHere).toHaveBeenCalledWith(null, true);
});

test("a saved camera is a row that looks through it, and Free look appears only once you are inside one", async () => {
  const camera = {
    id: "cam-1",
    name: "Down the line",
    pos: [12, 3, 0] as [number, number, number],
    lookAt: [0, 1, 0] as [number, number, number],
    fovDeg: 34,
    near: 1,
    far: 90,
    active: true,
  };
  const client = fakeClient();
  client.sceneCameras = vi.fn(() => Promise.resolve([camera]));
  render(<ViewportToolbar client={client} />);
  await settleToolbar();

  fireEvent.click(screen.getByTestId("vpView"));
  const row = screen.getByTestId("vpCamera");
  expect(row.textContent).toMatch(/Down the line/);
  // The lens is stated in the row, in millimetres — the scent that tells two cameras apart.
  expect(row.textContent).toMatch(/39 mm/);
  expect(row.getAttribute("aria-checked")).toBe("false");
  expect(screen.queryByTestId("vpFreeLook")).toBeNull();

  await act(async () => {
    fireEvent.click(row);
    await Promise.resolve();
    await Promise.resolve();
  });
  expect(client.lookThroughCamera).toHaveBeenCalledWith(true);

  fireEvent.click(screen.getByTestId("vpView"));
  expect(screen.getByTestId("vpCamera").getAttribute("aria-checked")).toBe("true");
  await act(async () => {
    fireEvent.click(screen.getByTestId("vpFreeLook"));
    await Promise.resolve();
  });
  expect(client.lookThroughCamera).toHaveBeenLastCalledWith(false);
});

test("a camera saved before cameras could aim says so in the menu, not only in the Inspector", async () => {
  const client = fakeClient();
  client.sceneCameras = vi.fn(() =>
    Promise.resolve([
      {
        id: "cam-old",
        name: "Overview",
        pos: [1, 2, 3] as [number, number, number],
        lookAt: null,
        fovDeg: 55,
        near: 0.1,
        far: 500,
        active: true,
      },
    ]),
  );
  render(<ViewportToolbar client={client} />);
  await settleToolbar();
  fireEvent.click(screen.getByTestId("vpView"));
  expect(screen.getByTestId("vpCamera").textContent).toMatch(/follows the editor/i);
});
