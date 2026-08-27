//! **Selecting more than one thing on the stage.**
//!
//! The engine has understood `shift` / `ctrl` / `cycle` on a viewport pick, and had a complete
//! marquee query with a real crossing-versus-enclosing policy, since picking was rebuilt. The front
//! end sent `{ x, y }` and nothing else, and had no caller for the marquee at all — so the 3D view,
//! the surface the product is about, could select exactly one object, and the only way to reach a
//! multi-selection was the list. The toolbar's own refusal said `Shift/Ctrl-click at least two
//! objects`, naming a gesture the stage did not have.
//!
//! These are the assertions that keep the gestures wired. They are deliberately about WHAT CROSSES
//! THE BOUNDARY — the modifier flags, the corner order, the follow-up read — because that is the
//! half the front end owns; whether the engine then selects the right objects is `spatial`'s own
//! `pick_region` tests and the `.exe` gate.

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { playStore } from "../store/play";
import { projectionStore } from "../store/projection";
import { toastStore } from "../store/toasts";
import { walletStore } from "../store/wallet";

interface Spies {
  pick: ReturnType<typeof vi.fn>;
  region: ReturnType<typeof vi.fn>;
  selectionIds: ReturnType<typeof vi.fn>;
  undo: ReturnType<typeof vi.fn>;
}
const sessions: Spies[] = [];

vi.mock("../transport/session", async (importOriginal) => {
  const real = await importOriginal<typeof import("../transport/session")>();
  return {
    ...real,
    createSession: () => {
      const client = real.createSession();
      const pick = vi.fn(() => Promise.resolve("hit-1" as string | null));
      const region = vi.fn(() => Promise.resolve(["a", "b", "c"]));
      const selectionIds = vi.fn(() => Promise.resolve(["a", "b"]));
      const undo = vi.fn(() => Promise.resolve(true));
      client.undo = undo as unknown as typeof client.undo;
      client.viewportPick = pick as unknown as typeof client.viewportPick;
      client.viewportPickRegion = region as unknown as typeof client.viewportPickRegion;
      client.selectionIds = selectionIds as unknown as typeof client.selectionIds;
      sessions.push({ pick, region, selectionIds, undo });
      return client;
    },
  };
});

const { App } = await import("./App");

const spies = () => sessions[sessions.length - 1];
const undoSpy = () => spies().undo;

/** A pointer event that actually carries coordinates.
 *
 *  **jsdom has no `PointerEvent`** — `typeof window.PointerEvent === "undefined"`, measured — so
 *  `fireEvent.pointerMove(el, { clientX: 400 })` constructs a bare `Event`, whose constructor ignores
 *  every mouse field in the init: the handler receives `clientX === undefined`, `Math.abs(undefined -
 *  100)` is `NaN`, `NaN >= 4` is false, and the drag threshold is never crossed. The test then fails
 *  in a way that reads exactly like the feature being broken.
 *
 *  A `MouseEvent` with a pointer event's TYPE is the honest substitute: React routes by type, so
 *  `onPointerMove` fires, and `MouseEvent` is the interface `PointerEvent` extends — every field this
 *  gesture reads (`clientX/Y`, `button`, the modifier flags) is a `MouseEvent` field. What is NOT
 *  covered here is `pointerId` and capture, which jsdom cannot express at all; those are guarded in
 *  the handler and belong to the `.exe` gate. */
function pointer(el: Element, type: string, init: MouseEventInit) {
  // `act` because the rectangle is React STATE: without it the update is scheduled and the assertion
  // that follows reads the previous render, which looks exactly like the box never being drawn.
  act(() => {
    fireEvent(el, new MouseEvent(type, { bubbles: true, cancelable: true, ...init }));
  });
}

/** Drag a box across the stage. The corner order is the caller's, because the order IS the policy. */
function dragBox(from: { x: number; y: number }, to: { x: number; y: number }, init: MouseEventInit = {}) {
  const viewport = screen.getByTestId("viewport");
  pointer(viewport, "pointerdown", { button: 0, clientX: from.x, clientY: from.y });
  pointer(viewport, "pointermove", { clientX: to.x, clientY: to.y });
  return {
    viewport,
    release: () => pointer(viewport, "pointerup", { button: 0, clientX: to.x, clientY: to.y, ...init }),
  };
}

beforeEach(() => {
  sessions.length = 0;
});

afterEach(() => {
  projectionStore.getState().reset();
  playStore.getState().reset();
  walletStore.getState().reset();
  toastStore.getState().reset();
  window.localStorage.clear();
});

describe("box selection on the stage", () => {
  it("draws a rectangle while dragging and names the mode the direction asked for", () => {
    render(<App />);
    dragBox({ x: 100, y: 100 }, { x: 400, y: 300 });

    const box = screen.getByTestId("stage-marquee");
    expect(box.getAttribute("data-marquee-mode")).toBe("enclose");
    expect(box.textContent).toContain("Fully inside");
    // The caption is the whole point of drawing it: two identical rectangles select different sets,
    // and nothing else on screen says which rule is running.
  });

  it("a right-to-left drag is the OTHER mode, and says so", () => {
    render(<App />);
    dragBox({ x: 400, y: 300 }, { x: 100, y: 100 });
    expect(screen.getByTestId("stage-marquee").getAttribute("data-marquee-mode")).toBe("touch");
    expect(screen.getByTestId("stage-marquee").textContent).toContain("Touched");
  });

  it("a press that barely moves is a click, not a box", () => {
    render(<App />);
    dragBox({ x: 100, y: 100 }, { x: 102, y: 101 });
    expect(screen.queryByTestId("stage-marquee")).toBeNull();
  });

  it("sends the corners IN DRAG ORDER, because the engine reads the policy from the direction", async () => {
    render(<App />);
    // 1000x800 is jsdom's default window; the command takes normalized surface fractions.
    const { release } = dragBox({ x: 800, y: 600 }, { x: 200, y: 100 });
    await act(async () => {
      release();
    });

    expect(spies().region).toHaveBeenCalledTimes(1);
    const [x0, y0, x1, y1, extend] = spies().region.mock.calls[0] as unknown as number[];
    expect(x0).toBeGreaterThan(x1 as number);
    expect([x0, y0, x1, y1]).toEqual([
      800 / window.innerWidth,
      600 / window.innerHeight,
      200 / window.innerWidth,
      100 / window.innerHeight,
    ]);
    expect(extend).toBe(false);
    // Normalizing the corners here would silently turn every marquee into an enclose while the
    // caption on screen still said "Touched" — the exact class of drift a second statement of a
    // contract produces.
  });

  it("mirrors the engine's answer into the selection and says how many, at the gesture", async () => {
    render(<App />);
    const { release } = dragBox({ x: 100, y: 100 }, { x: 400, y: 300 });
    await act(async () => {
      release();
    });

    await waitFor(() => expect(projectionStore.getState().multiSelect).toEqual(["a", "b", "c"]));
    expect(projectionStore.getState().selectedId).toBe("c");
    expect(screen.queryByTestId("stage-marquee")).toBeNull();
    // A toast, not only the status bar: the box is gone by the time the answer arrives.
    expect(toastStore.getState().toasts.map((t) => t.text)).toContain(
      "selected 3 objects fully inside the box",
    );
  });

  it("a completed box does not ALSO pick — `click` fires after `pointerup`", async () => {
    render(<App />);
    const { viewport, release } = dragBox({ x: 100, y: 100 }, { x: 400, y: 300 });
    await act(async () => {
      release();
    });
    fireEvent.click(viewport);

    expect(spies().pick).not.toHaveBeenCalled();
    await waitFor(() => expect(projectionStore.getState().multiSelect).toEqual(["a", "b", "c"]));
    // Without the suppression the release re-selects whatever is under the cursor and the box's
    // fourteen objects are gone before the user's hand leaves the mouse.
  });

  it("the suppression lasts ONE gesture: the click after the NEXT press still picks", async () => {
    // Found by the live `.exe` run, not by this suite. A drag whose release produces no click on the
    // stage — the pointer left the window, or a driver dispatched pointerup without one — used to
    // leave the flag standing, and the user's next click was silently eaten. A viewport that ignores
    // one click is indistinguishable from a dead viewport.
    render(<App />);
    const viewport = screen.getByTestId("viewport");
    const { release } = dragBox({ x: 100, y: 100 }, { x: 400, y: 300 });
    await act(async () => {
      release();
    });
    // No click follows the release — that is the whole point.
    expect(spies().pick).not.toHaveBeenCalled();

    await act(async () => {
      pointer(viewport, "pointerdown", { button: 0, clientX: 500, clientY: 400 });
      pointer(viewport, "pointerup", { button: 0, clientX: 500, clientY: 400 });
      fireEvent.click(viewport, { clientX: 500, clientY: 400 });
    });
    expect(spies().pick).toHaveBeenCalledTimes(1);
  });

  it("shift-drag ADDS to the selection rather than replacing it", async () => {
    render(<App />);
    const { release } = dragBox({ x: 100, y: 100 }, { x: 400, y: 300 }, { shiftKey: true });
    await act(async () => {
      release();
    });
    expect(spies().region.mock.calls[0]?.[4]).toBe(true);
  });
});

describe("the way out the editor promises out loud", () => {
  it("Ctrl-Z undoes with a BUTTON focused, because a button does not own a chord", async () => {
    // Found live in the packaged `.exe`, not here: delete a selection from the Actions menu, focus
    // returns to the trigger BUTTON, the toast and the status line both say "recoverable with Ctrl-Z",
    // and Ctrl-Z did nothing — the guard that keeps W/E/R/F from switching tools while a control has
    // the keyboard was applied to the undo chord too.
    render(<App />);
    const button = screen.getByTestId("authoring-more");
    button.focus();
    expect(document.activeElement).toBe(button);

    await act(async () => {
      fireEvent.keyDown(window, { key: "z", ctrlKey: true });
    });
    expect(undoSpy()).toHaveBeenCalledTimes(1);
  });

  it("…and still keeps its hands off a text field's own undo stack", async () => {
    render(<App />);
    const search = screen.getByPlaceholderText(/search objects/i);
    search.focus();

    await act(async () => {
      fireEvent.keyDown(window, { key: "z", ctrlKey: true });
    });
    expect(undoSpy()).not.toHaveBeenCalled();
  });
});

describe("modified clicks on the stage", () => {
  it("a plain click sends no modifiers and selects the one object it hit", async () => {
    render(<App />);
    await act(async () => {
      fireEvent.click(screen.getByTestId("viewport"), { clientX: 500, clientY: 400 });
    });
    expect(spies().pick.mock.calls[0]?.[2]).toEqual({ extend: false, toggle: false, cycle: false });
    expect(spies().selectionIds).not.toHaveBeenCalled();
    expect(projectionStore.getState().multiSelect).toEqual(["hit-1"]);
  });

  it("shift extends, and the resulting SET is read back rather than predicted", async () => {
    render(<App />);
    await act(async () => {
      fireEvent.click(screen.getByTestId("viewport"), { clientX: 500, clientY: 400, shiftKey: true });
    });
    expect(spies().pick.mock.calls[0]?.[2]).toMatchObject({ extend: true });
    // The hit alone cannot say what is selected: a ctrl-click that DESELECTED still hit something.
    expect(spies().selectionIds).toHaveBeenCalledTimes(1);
    expect(projectionStore.getState().multiSelect).toEqual(["a", "b"]);
  });

  it("ctrl toggles and alt cycles — the two the engine has always understood and never been asked", async () => {
    render(<App />);
    const viewport = screen.getByTestId("viewport");
    await act(async () => {
      fireEvent.click(viewport, { clientX: 500, clientY: 400, ctrlKey: true });
    });
    expect(spies().pick.mock.calls[0]?.[2]).toMatchObject({ toggle: true });
    // Ctrl DOES need the set read back: a toggle that deselected still hit something.
    const afterToggle = spies().selectionIds.mock.calls.length;
    expect(afterToggle).toBe(1);

    await act(async () => {
      fireEvent.click(viewport, { clientX: 500, clientY: 400, altKey: true });
    });
    expect(spies().pick.mock.calls[1]?.[2]).toMatchObject({ cycle: true, extend: false, toggle: false });
    // Alt is not a selection mode: it moves the HIT, so it must NOT cost a second round trip.
    expect(spies().selectionIds.mock.calls.length).toBe(afterToggle);
  });
});
