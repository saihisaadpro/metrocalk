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
import { uiStore } from "../store/ui";
import { walletStore } from "../store/wallet";

interface Spies {
  pick: ReturnType<typeof vi.fn>;
  region: ReturnType<typeof vi.fn>;
  selectionIds: ReturnType<typeof vi.fn>;
  undo: ReturnType<typeof vi.fn>;
  /** The seam every selection made AWAY from the stage goes through. Watched, not replaced: what
   *  matters is that the engine is told the WHOLE set, which is the half the front end owns. */
  select: ReturnType<typeof vi.fn>;
  /** The one batched delete all three routes go through (ADR-183). */
  del: ReturnType<typeof vi.fn>;
  /** What is under the point a gesture happened at (ADR-191) — the right-click's own question. */
  candidates: ReturnType<typeof vi.fn>;
  /** The hover read (ADR-191). Watched because the CADENCE is the design: it must not fire per move. */
  peek: ReturnType<typeof vi.fn>;
  /** The gizmo-handle probe. A HIT suppresses the click, so WHEN it is armed is a selection question. */
  gizmoPickDrag: ReturnType<typeof vi.fn>;
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
      const select = vi.fn(client.selectEntities.bind(client));
      const del = vi.fn(client.deleteDeactivateMany.bind(client));
      const candidates = vi.fn(() =>
        Promise.resolve([
          { id: "hit-1", kind: "Mesh", distance: 25.7, selected: false },
          { id: "hit-2", kind: "Mesh", distance: 28.2, selected: false },
        ]),
      );
      client.pickCandidates = candidates as unknown as typeof client.pickCandidates;
      const peek = vi.fn(() => Promise.resolve("hit-1" as string | null));
      client.viewportPeek = peek as unknown as typeof client.viewportPeek;
      const gizmoPickDrag = vi.fn(() => Promise.resolve(false));
      client.gizmoPickDrag = gizmoPickDrag as unknown as typeof client.gizmoPickDrag;
      client.selectEntities = select as unknown as typeof client.selectEntities;
      client.deleteDeactivateMany = del as unknown as typeof client.deleteDeactivateMany;
      client.undo = undo as unknown as typeof client.undo;
      client.viewportPick = pick as unknown as typeof client.viewportPick;
      client.viewportPickRegion = region as unknown as typeof client.viewportPickRegion;
      client.selectionIds = selectionIds as unknown as typeof client.selectionIds;
      sessions.push({ pick, region, selectionIds, undo, select, del, candidates, peek, gizmoPickDrag });
      return client;
    },
  };
});

const { App } = await import("./App");
// The command palette is a `React.lazy` chunk and the first test to open it would pay the whole cold
// import inside its own timeout. Resolving it here spends that once, before any test's clock starts.
await import("../panels/CommandPalette");

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
  it("a click that hits NOTHING clears the UI too, because the engine just did", async () => {
    render(<App />);
    const viewport = screen.getByTestId("viewport");
    await act(async () => {
      fireEvent.click(viewport, { clientX: 500, clientY: 400 });
    });
    expect(projectionStore.getState().multiSelect).toEqual(["hit-1"]);

    spies().pick.mockResolvedValueOnce(null);
    await act(async () => {
      fireEvent.click(viewport, { clientX: 20, clientY: 20 });
    });
    // `apply_click` with no hit CLEARS the canonical selection — click-to-deselect is the whole reason
    // picking was rebuilt around a real ray. The front end used to answer that with a status line and
    // leave its own projection standing: measured on the packaged `.exe`, `selection_ids` returned []
    // while the Inspector still showed `Character 4`, the outliner row was still lit and the toolbar
    // still read `Actions · 1`.
    expect(projectionStore.getState().multiSelect).toEqual([]);
    expect(projectionStore.getState().selectedId).toBeNull();
    expect(uiStore.getState().status).toBe("nothing here");
  });

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
    // Alt is not a selection MODE: it moves the HIT, so it must NOT re-read the whole set.
    expect(spies().selectionIds.mock.calls.length).toBe(afterToggle);
  });

  it("alt and shift do NOT start a gizmo drag — they say the gesture is a selection", async () => {
    render(<App />);
    const viewport = screen.getByTestId("viewport");
    // Something is selected, which is the condition that arms the gizmo-handle probe.
    await act(async () => {
      fireEvent.click(viewport, { clientX: 500, clientY: 400 });
    });
    expect(projectionStore.getState().selectedId).toBe("hit-1");

    const gizmo = spies().gizmoPickDrag;
    gizmo.mockClear();
    pointer(viewport, "pointerdown", { button: 0, clientX: 500, clientY: 400, altKey: true });
    pointer(viewport, "pointerdown", { button: 0, clientX: 500, clientY: 400, shiftKey: true });
    // The gizmo is drawn AT the selection, so alt-clicking the object you just selected — to reach the
    // one BEHIND it — lands on the gizmo. A hit suppresses the click, and the cycle never runs:
    // measured on the packaged `.exe` as "alt-click stayed on 1_16" with two objects along the ray.
    expect(gizmo).not.toHaveBeenCalled();

    // Ctrl still arms it, deliberately: ctrl is the gizmo's own SNAP modifier, and that ambiguity is
    // resolved by whether the pointer moves, not by the key.
    pointer(viewport, "pointerdown", { button: 0, clientX: 500, clientY: 400, ctrlKey: true });
    expect(gizmo).toHaveBeenCalledTimes(1);
  });

  it("alt-click says WHERE IN THE STACK it landed, instead of nothing at all", async () => {
    render(<App />);
    await act(async () => {
      fireEvent.click(screen.getByTestId("viewport"), { clientX: 500, clientY: 400, altKey: true });
    });
    // The gesture reaches the object behind the one you can see and used to report nothing — not what
    // it took, not how many there were, not whether there was anything to cycle to. A person cannot
    // learn a gesture whose effect is invisible.
    await waitFor(() => expect(uiStore.getState().status).toContain("1 of 2 under the pointer"));
    expect(toastStore.getState().toasts.at(-1)?.text).toContain("under the pointer");
    // One extra read, on the modified click only — a plain click must not pay for it.
    expect(spies().candidates).toHaveBeenCalledTimes(1);
  });

  it("a PLAIN click never asks what else is under the cursor", async () => {
    render(<App />);
    await act(async () => {
      fireEvent.click(screen.getByTestId("viewport"), { clientX: 500, clientY: 400 });
    });
    expect(spies().candidates).not.toHaveBeenCalled();
  });
});

describe("right-click asks where it happened", () => {
  it("opens on an object you have NOT selected, and offers the one behind it", async () => {
    render(<App />);
    expect(projectionStore.getState().multiSelect).toEqual([]);
    await act(async () => {
      fireEvent.contextMenu(screen.getByTestId("viewport"), { clientX: 500, clientY: 400 });
    });
    // `if (sel.length)` alone meant a right-click on an object you had not selected did NOTHING —
    // the state every other 3D tool treats as the primary way in.
    const rows = await screen.findAllByTestId("ctxcandidate");
    expect(rows.map((r) => r.dataset.id)).toEqual(["hit-1", "hit-2"]);
    expect(spies().candidates.mock.calls[0]).toEqual([500 / window.innerWidth, 400 / window.innerHeight]);
  });

  it("stays shut over empty stage with nothing selected — a menu with nothing in it is not a menu", async () => {
    render(<App />);
    spies().candidates.mockResolvedValueOnce([]);
    await act(async () => {
      fireEvent.contextMenu(screen.getByTestId("viewport"), { clientX: 500, clientY: 400 });
    });
    expect(screen.queryByTestId("ctxmenu")).toBeNull();
  });
});

// ── the verbs no gesture can express ──────────────────────────────────────────────────────────────
//
// ADR-158 gave the stage the four gestures the ENGINE already understood. Scope ("all", "none", "the
// rest") and kind ("everything like this") are not gestures at all: no rectangle can express them,
// and the imported-assembly case — 378 copies of one bolt, scattered and mostly occluded — is the one
// a marquee provably cannot reach.

const status = () => uiStore.getState().status;

async function openPalette() {
  render(<App />);
  await waitFor(() => expect(projectionStore.getState().order.length).toBeGreaterThan(0));
  await act(async () => {
    fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
  });
  return screen.findByTestId("command-palette", {}, { timeout: 10_000 });
}

function paletteRow(palette: HTMLElement, label: string): HTMLElement | undefined {
  return Array.from(palette.querySelectorAll("[role='option']")).find((o) =>
    (o.textContent ?? "").includes(label),
  ) as HTMLElement | undefined;
}

describe("selecting without a gesture", () => {
  // THREE MOUNTS, NOT SIX, AND THE REASON IS MEASURED. Every case here renders the whole shell and
  // waits on the palette's lazily-imported chunk; adding six of them to this file turned a 79/79
  // suite into one where OTHER files timed out at their own budgets — contention, not a defect in
  // them (`test-setup.ts` has the argument). Assertions that share a scene share a mount.
  it("the palette carries all five verbs — off the SHIPPED list — and Select similar explains its refusal", async () => {
    const palette = await openPalette();

    for (const label of ["Find objects", "Select all", "Select none", "Invert selection", "Select similar"]) {
      expect(paletteRow(palette, label), label).toBeTruthy();
    }
    // A disabled control that says WHY (`<ux_quality>` 4 and 6) rather than a row that does nothing
    // when pressed and gives no reason. Nothing is selected on a fresh shell, so this is its state.
    const similar = paletteRow(palette, "Select similar");
    expect(similar?.getAttribute("aria-disabled")).toBe("true");
    expect(similar?.textContent).toContain("Select an object first");
  });

  it("Ctrl+A selects everything and tells the ENGINE the whole set — but inside a text field it belongs to the field", async () => {
    render(<App />);
    await waitFor(() => expect(projectionStore.getState().order.length).toBeGreaterThan(0));
    const all = projectionStore.getState().order;

    const field = document.createElement("input");
    document.body.appendChild(field);
    await act(async () => {
      fireEvent.keyDown(field, { key: "a", ctrlKey: true });
    });
    // Select-all-TEXT is the field's, and stealing it would be the same defect as stealing Ctrl-Z.
    expect(projectionStore.getState().multiSelect).toEqual([]);
    expect(spies().select).not.toHaveBeenCalled();
    field.remove();

    await act(async () => {
      fireEvent.keyDown(document.body, { key: "a", ctrlKey: true });
    });
    expect(projectionStore.getState().multiSelect).toEqual(all);
    // The WHOLE set, not a primary — the failure `select_entities` exists to make impossible.
    expect(spies().select).toHaveBeenLastCalledWith(all);
    expect(status()).toBe(`Selected all ${all.length} objects`);
  });

  it("Ctrl+F opens the Scene workspace and puts the caret in the box that searches it", async () => {
    // The chord every person already knows for "where is it" was unbound (ADR-185): the scene search
    // box was reachable only by opening the Scene workspace and clicking into it, and the outliner is
    // a `hidden` tabpanel in every other workspace — so the chord has to open the panel BEFORE it
    // asks for focus, or it focuses something nobody can see.
    render(<App />);
    await waitFor(() => expect(projectionStore.getState().order.length).toBeGreaterThan(0));

    await act(async () => {
      fireEvent.click(screen.getByRole("tab", { name: /Build/ }));
    });
    expect(document.getElementById("engine-panel-scene")?.hasAttribute("hidden")).toBe(true);

    await act(async () => {
      fireEvent.keyDown(document.body, { key: "f", ctrlKey: true });
    });
    const box = screen.getByRole("searchbox", { name: "Search scene objects" });
    expect(document.getElementById("engine-panel-scene")?.hasAttribute("hidden")).toBe(false);
    expect(document.activeElement).toBe(box);

    // Inside a text field the chord belongs to the field, for the same reason Ctrl-Z and Ctrl-A do.
    const field = document.createElement("input");
    document.body.appendChild(field);
    field.focus();
    await act(async () => {
      fireEvent.keyDown(field, { key: "f", ctrlKey: true });
    });
    expect(document.activeElement).toBe(field);
    field.remove();
  });

  it("Select similar reaches the copies no box contains; Invert takes the complement", async () => {
    render(<App />);
    await waitFor(() => expect(projectionStore.getState().order.length).toBeGreaterThan(0));
    act(() => {
      projectionStore.getState().bulkLoad([
        { id: "b1", name: "Bolt", parentId: null, components: { MeshRenderer: { mesh: "mtkasset:bolt" } } },
        { id: "beam", name: "Beam", parentId: null, components: { MeshRenderer: { mesh: "mtkasset:beam" } } },
        { id: "b2", name: "Bolt", parentId: null, components: { MeshRenderer: { mesh: "mtkasset:bolt" } } },
      ]);
      projectionStore.getState().select("b1");
    });

    await act(async () => {
      fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
    });
    await act(async () => {
      fireEvent.click(paletteRow(await screen.findByTestId("command-palette", {}, { timeout: 10_000 }), "Select similar")!);
    });

    // The scattered set a rectangle cannot reach: the beam BETWEEN the two bolts is left alone, and
    // the sentence names what the match was made on rather than only counting.
    await waitFor(() => expect(projectionStore.getState().multiSelect).toEqual(["b1", "b2"]));
    expect(spies().select).toHaveBeenLastCalledWith(["b1", "b2"]);
    expect(status()).toBe("Selected 2 objects sharing the geometry of Bolt");

    await act(async () => {
      fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
    });
    await act(async () => {
      fireEvent.click(paletteRow(await screen.findByTestId("command-palette", {}, { timeout: 10_000 }), "Invert selection")!);
    });

    expect(projectionStore.getState().multiSelect).toEqual(["beam"]);
    expect(status()).toContain("inverted");
  });
});

describe("the key every editor binds", () => {
  // ONE MOUNT for all three assertions, for the reason the block above states in full: this file
  // renders the whole shell per case, and six of them turned a green suite into one where unrelated
  // files timed out.
  it("Delete removes the WHOLE selection in one transaction — but inside a text field it belongs to the field", async () => {
    render(<App />);
    await waitFor(() => expect(projectionStore.getState().order.length).toBeGreaterThan(0));
    act(() => {
      projectionStore.getState().bulkLoad([
        { id: "d1", name: "Bolt", parentId: null, components: {} },
        { id: "d2", name: "Bolt", parentId: null, components: {} },
        { id: "d3", name: "Beam", parentId: null, components: {} },
      ]);
      projectionStore.getState().setSelection(["d1", "d2"]);
    });

    // (a) A text field owns Delete over its own characters. Same line Ctrl+A and Ctrl-Z draw.
    const field = document.createElement("input");
    document.body.appendChild(field);
    await act(async () => {
      fireEvent.keyDown(field, { key: "Delete" });
    });
    expect(spies().del).not.toHaveBeenCalled();
    field.remove();

    // (b) Everywhere else it is the scene's, and it takes the whole selection — not the primary.
    await act(async () => {
      fireEvent.keyDown(document.body, { key: "Delete" });
    });
    expect(spies().del).toHaveBeenCalledTimes(1);
    expect(spies().del).toHaveBeenCalledWith(["d1", "d2"]);
    await waitFor(() => expect(status()).toBe("deleted 2 objects — recoverable with Ctrl-Z"));

    // (c) Both halves of the selection are cleared: the store the inspector reads AND the engine the
    //     picture is outlined from. A store-only clear leaves the renderer outlining what is gone.
    expect(projectionStore.getState().multiSelect).toEqual([]);
    expect(spies().select).toHaveBeenLastCalledWith([]);
    expect(projectionStore.getState().deactivated.d1).toBe(true);

    // (d) With nothing selected the key is silent rather than refusing loudly — there is no gesture
    //     to explain, and a toast per stray keypress is noise.
    await act(async () => {
      fireEvent.keyDown(document.body, { key: "Delete" });
    });
    expect(spies().del).toHaveBeenCalledTimes(1);
  });
});

describe("the pointer names what it rests on", () => {
  it("asks once the pointer STOPS, and never per move", async () => {
    render(<App />);
    const viewport = screen.getByTestId("viewport");
    for (let i = 0; i < 5; i += 1) pointer(viewport, "pointermove", { clientX: 400 + i * 20, clientY: 300 });
    // Invariant 4: the hot path never crosses the boundary. Five moves, nothing asked.
    expect(spies().peek).not.toHaveBeenCalled();

    await act(async () => {
      await new Promise((r) => setTimeout(r, 260));
    });
    expect(spies().peek).toHaveBeenCalledTimes(1);
    expect(spies().peek.mock.calls[0]).toEqual([480 / window.innerWidth, 300 / window.innerHeight]);
    // And it NAMES it — the tooltip is the whole point of the read having a caller at last.
    await waitFor(() => expect(screen.getByTestId("stage-hover")).toBeTruthy());
  });

  it("asks about the STAGE, not about the controls floating on it", async () => {
    render(<App />);
    const viewport = screen.getByTestId("viewport");
    pointer(viewport, "pointermove", { clientX: 500, clientY: 400 });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 260));
    });
    await waitFor(() => expect(screen.getByTestId("stage-hover")).toBeTruthy());
    const asked = spies().peek.mock.calls.length;

    // The shell paints ~35 controls INSIDE the viewport. Without the `onStageSurface` gate, resting on
    // one of them names whatever object happens to be behind it — the same class of defect as a wheel
    // over a floating panel zooming the camera.
    const chrome = viewport.querySelector("button");
    if (!chrome) throw new Error("no control floats over the stage in this state — the gate is untestable here");
    pointer(chrome, "pointermove", { clientX: 12, clientY: 12 });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 260));
    });
    expect(spies().peek.mock.calls.length).toBe(asked);
    // DISMISSED, not merely not-updated: a tooltip left standing describes something the pointer left.
    expect(screen.queryByTestId("stage-hover")).toBeNull();
  });

  it("a press is a gesture, not a question", async () => {
    render(<App />);
    const viewport = screen.getByTestId("viewport");
    pointer(viewport, "pointermove", { clientX: 500, clientY: 400 });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 260));
    });
    await waitFor(() => expect(screen.getByTestId("stage-hover")).toBeTruthy());
    pointer(viewport, "pointerdown", { button: 0, clientX: 500, clientY: 400 });
    expect(screen.queryByTestId("stage-hover")).toBeNull();
  });
});
