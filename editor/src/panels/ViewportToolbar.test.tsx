import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { fakeClient } from "../transport/test-client";
import { uiStore } from "../store/ui";
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

  await act(async () => {
    fireEvent.click(screen.getByTestId("vpSpace"));
    await Promise.resolve();
    await Promise.resolve();
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

test("Frame selected frames the WHOLE selection and raises the way back out", async () => {
  // Two defects in one control (ADR-194). It asked `gizmo_selected()` for the PRIMARY and framed that
  // one under a status line reading "framed the selection" — and it raised no banner, so the `Escape`
  // that exits focus (gated on the banner's state in the shell) never fired: framing greyed the scene
  // and left the only way back inside a context menu.
  const focusSelection = vi.fn(() => Promise.resolve({ framed: 6, distance: 31.5, primary: "e2" }));
  const onFocus = vi.fn();
  render(
    <ViewportToolbar
      client={fakeClient({ focusSelection, gizmoDebug: () => Promise.resolve(["translate", true, false, "world", "origin"]) })}
      showTransformTools={false}
      onFocus={onFocus}
    />,
  );
  await settleToolbar();

  fireEvent.click(screen.getByTestId("vpView"));
  await act(async () => {
    fireEvent.click(screen.getByTestId("vpFrameSel"));
    await Promise.resolve();
    await Promise.resolve();
  });

  expect(focusSelection).toHaveBeenCalledTimes(1);
  expect(onFocus).toHaveBeenCalledWith("6 objects", 31.5);
  expect(uiStore.getState().status).toBe("framed all 6 selected objects");
});

test("Frame selected that frames nothing keeps the banner down and says what to do", async () => {
  const onFocus = vi.fn();
  render(
    <ViewportToolbar
      client={fakeClient({
        focusSelection: vi.fn(() => Promise.resolve({ framed: 0, distance: 60, primary: null })),
        gizmoDebug: () => Promise.resolve(["translate", true, false, "world", "origin"]),
      })}
      showTransformTools={false}
      onFocus={onFocus}
    />,
  );
  await settleToolbar();

  fireEvent.click(screen.getByTestId("vpView"));
  await act(async () => {
    fireEvent.click(screen.getByTestId("vpFrameSel"));
    await Promise.resolve();
    await Promise.resolve();
  });

  expect(onFocus).not.toHaveBeenCalled();
  expect(uiStore.getState().status).toBe("select something to frame (F)");
});
