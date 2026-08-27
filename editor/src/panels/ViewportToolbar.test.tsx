import { act, fireEvent, render, screen } from "@testing-library/react";
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
  render(<ViewportToolbar client={fakeClient()} showTransformTools={false} />);
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

  // `waitFor`, not a COUNT of microtasks. `gizmoSpaceToggle()` is a promise whose `.then` sets state,
  // and "two `await Promise.resolve()`" is a guess about how many ticks that takes — right on a quiet
  // machine and wrong on a loaded one, which is why this assertion failed in isolation while passing
  // inside the full parallel run. Same class as `330781a`: an assertion about the clock wearing the
  // shape of an assertion about behaviour.
  // THE CLICK AND THE SETTLE INSIDE ONE `act`, then a plain assertion.
  //
  // What was here counted microtasks — "two `await Promise.resolve()`" — for an update that does not
  // land in them: the reply resolves, `PopupMenuItem` chains a `.finally()` to dismiss the menu, and
  // React commits after that. A count is right on a quiet machine and wrong on a busy one, which is
  // why this file passed inside the full parallel run and failed on its own (`330781a`'s class).
  //
  // `waitFor` is not the fix either, and that is worth recording: its wrapper turns the act
  // environment OFF, so React schedules through the real scheduler and the polling races a starved
  // task queue — it made the flake rarer and inverted it (green alone, red under load) rather than
  // removing it. Inside `act` the queue is flushed when the scope exits, and the macrotask is what
  // lets the promise chain settle before that happens. No clock left in the assertion.
  await act(async () => {
    fireEvent.click(screen.getByTestId("vpSpace"));
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
  expect(screen.getByTestId("vpTransform").textContent).toContain("Local");
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
