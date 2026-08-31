import { act, render, screen, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { App } from "./App";
import { projectionStore } from "../store/projection";
import { playStore } from "../store/play";
import { walletStore } from "../store/wallet";
import { toastStore } from "../store/toasts";

// THE TWO HEAVY FILES GET THEIR OWN NUMBER, AND HERE IS THE MEASUREMENT.
//
// A 5000ms per-test default is a wall-clock budget, and vitest runs 77 files in parallel on
// whatever cores are left. Measured on this tree, 2026-08-26: run ALONE this file takes 12.8s,
// its slowest test 3634ms; run inside the full suite — on an otherwise IDLE box — it
// timed out at 5000ms, in different tests on different runs, which is the signature of
// contention rather than of a hang. Raising the GLOBAL timeout would hide every real hang in
// the suite; these are the files that need the room, so this is where the number lives. 20s is
// ~5.5x this file's own worst case and still nothing like the forever a deadlock takes.
//
// 2026-08-31: BOTH NUMBERS HAVE TO MOVE, AND ONLY ONE OF THEM EVER DID. The per-test budget above was
// 20s while Testing Library's `asyncUtilTimeout` (`src/test-setup.ts`) is 15s, so a `findBy` waiting
// on a lazily-imported workspace gave up at 15s inside a 20s test and the extra 5s bought nothing.
// The suite has meanwhile grown 77 -> 89 files as three lanes merged, and the four failures were all
// the same shape: a chunk Vite transforms on demand, under a parallel runner.
//
// AND THE OBVIOUS FIX IS THE WRONG ONE, MEASURED: raising `asyncUtilTimeout` with a file-level
// `configure()` made the run WORSE, 4 failures -> 13 and 219s -> 685s. `configure()` is global to the
// worker, vitest reuses a worker across files, and a 30s async budget therefore leaked into every
// later file in that worker — so `AnimationGraphEditor`, which had been green all evening, started
// holding a slot for 30s per failed query and starving everything behind it. A budget is scoped to
// the CALL that needs it or it is not scoped at all: the three waits below name their own, and the
// per-test number only has to be larger than the largest of them.
vi.setConfig({ testTimeout: 40_000 });

/** The wait for a workspace Vite has not transformed yet. Named once so the three sites that need it
 *  cannot drift apart, and so the number is attached to its reason. */
const LAZY_CHUNK = { timeout: 25_000 } as const;

afterEach(() => {
  projectionStore.getState().reset();
  playStore.getState().reset();
  walletStore.getState().reset();
  toastStore.getState().reset();
  window.localStorage.removeItem("metrocalk:shell-layout:v1:left-collapsed");
  window.localStorage.removeItem("metrocalk:shell-layout:v1:right-collapsed");
  window.localStorage.removeItem("metrocalk:shell-layout:v1:tool-rail-minimized");
  Object.defineProperty(window, "innerWidth", { value: 1024, configurable: true });
});

describe("editor app — end-to-end wiring", () => {
  it("projects the first-run SAMPLE scene (named, not the 5k fixture) and renders an inspector form on select", async () => {
    render(<App />);
    // the small NAMED first-run scene loads through client → store → hierarchy (C10: never 5000 rows).
    // Keys off the stable `data-testid="hierarchy"` (not the restyled header copy — structured-signal rule).
    expect(screen.getByTestId("hierarchy").textContent).not.toContain("5000");
    // a meaningful named starter entity appears (not "Entity N")
    expect(screen.getAllByText("Player").length).toBeGreaterThanOrEqual(1);

    // selecting it renders the schema-driven inspector header (≥2: the hierarchy row + the inspector)
    act(() => projectionStore.getState().select("player"));
    fireEvent.click(screen.getByTestId("rail-right-properties"));
    // No local timeout: the deadline for "a lazy workspace has resolved" is stated once, in
    // `test-setup.ts`. This wait carried its own 10 s and still crossed it in a full-suite run.
    await waitFor(() => expect(document.getElementById("inspector")?.textContent).toContain("Player"));
  });

  it("Play is unmistakable ON THE STAGE: a persistent badge appears only while playing (C2)", () => {
    render(<App />);
    expect(screen.queryByTestId("playStageBadge")).toBeNull();
    act(() => playStore.getState().refresh({ playing: true, paused: false }));
    const badge = screen.getByTestId("playStageBadge");
    expect(badge.textContent).toMatch(/playing/i);
    // Stop is reachable from the stage badge too (not only the toolbar)
    expect(screen.getByTestId("stageStop")).toBeTruthy();
  });

  it("an empty scene offers local procedural, library, and import paths (C10)", () => {
    render(<App />);
    expect(screen.queryByTestId("emptyState")).toBeNull(); // the sample scene is not empty
    act(() => projectionStore.getState().reset());
    expect(screen.getByTestId("emptyState").textContent).toMatch(/start with something tangible/i);
    expect(screen.queryByTestId("onboarding")).toBeNull();
    expect(screen.getByTestId("emptyDescribe")).toBeTruthy(); // the CTA this card's header always promised
    expect(screen.getByTestId("emptyPipe")).toBeTruthy();
    expect(screen.getByTestId("emptyAssets")).toBeTruthy();
    expect(screen.getByTestId("emptyImport")).toBeTruthy();
  });

  it("north star #2 is ON THE STAGE at first paint — no workspace to open, no chunk to wait for", () => {
    render(<App />);
    // No rail click, no `findBy`, no lazy boundary: the composer is in the first render. It used to take
    // Build → scroll → open a collapsed disclosure headed "Optional assisted creation".
    const composer = screen.getByTestId("describebar");
    expect(composer).toBeTruthy();
    expect(screen.getByTestId("describe")).toBeTruthy();
    // And it is a CHILD of the one stage-footer anchor rather than a fourth surface writing its own
    // `position: absolute; left: 50%`. That containment is the thing that makes a collision impossible;
    // asserting the two elements' rects would assert nothing in jsdom, which lays nothing out.
    const footer = screen.getByTestId("stage-footer");
    expect(footer.contains(composer)).toBe(true);
    expect(footer.contains(screen.getByTestId("onboarding"))).toBe(true);
  });

  it("Play owns the stage: the composer withdraws with the rest of the stage's authoring surfaces", () => {
    render(<App />);
    expect(screen.getByTestId("describebar")).toBeTruthy();
    act(() => playStore.getState().refresh({ playing: true, paused: false }));
    // One statement, one anchor: withdrawing the footer withdraws everything anchored to it, which is
    // what stops a future stage surface being added and forgotten in the `!playing` guard.
    expect(screen.queryByTestId("stage-footer")).toBeNull();
    expect(screen.queryByTestId("describebar")).toBeNull();
  });

  it("a spend in one panel updates the displayed balance EVERYWHERE (single source of truth — C7)", async () => {
    render(<App />);
    const bal = await screen.findByTestId("balance");
    await waitFor(() => expect(bal.textContent).toBe("100"));

    // Describe is ON THE STAGE — no rail click, no workspace to open, no lazy chunk to wait for. It used
    // to be a collapsed disclosure inside the Build workspace, and this test had to open Build to reach
    // north star #2 at all; that click was the measurement of the defect and its absence here is the fix.
    fireEvent.change(screen.getByTestId("describe"), { target: { value: "a nonexistent thingamajig" } });
    fireEvent.click(screen.getByTestId("describeBtn"));
    fireEvent.click(await screen.findByTestId("genBtn", {}, LAZY_CHUNK));

    // the top-bar Wallet's displayed balance dropped — it reads the SAME store the DescribeBar wrote to
    await waitFor(() => expect(bal.textContent).toBe("90"));
  });

  it("the stage holds priority on resize: below the breakpoint the panels collapse to icon rails (C8)", () => {
    render(<App />);
    expect(screen.queryByTestId("rail-left")).toBeNull(); // jsdom ~1024px → panels inline

    act(() => {
      Object.defineProperty(window, "innerWidth", { value: 800, configurable: true });
      window.dispatchEvent(new Event("resize"));
    });

    // narrow → side panels collapse to rails; the stage (viewport) survives (never collapses first)
    expect(screen.getByTestId("rail-left")).toBeTruthy();
    expect(screen.getByTestId("rail-right")).toBeTruthy();
    expect(screen.getByText(/native wgpu viewport/i)).toBeTruthy();

    // a rail re-opens the panel as an overlay drawer (the panels stay reachable)
    act(() => fireEvent.click(screen.getByTestId("rail-left-scene")));
    expect(screen.getByTestId("drawer-left")).toBeTruthy();

    act(() => {
      Object.defineProperty(window, "innerWidth", { value: 1024, configurable: true });
      window.dispatchEvent(new Event("resize"));
    });

    // Widening preserves the rails instead of restoring both panels and making the viewport narrower.
    expect(screen.getByTestId("rail-left")).toBeTruthy();
    expect(screen.getByTestId("rail-right")).toBeTruthy();
    expect(screen.queryByTestId("drawer-left")).toBeNull();
  });

  it("lets workstation users collapse, preview, and pin each dock without hiding the stage", () => {
    Object.defineProperty(window, "innerWidth", { value: 1440, configurable: true });
    render(<App />);
    expect(screen.queryByTestId("rail-right")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Collapse Inspector dock" }));
    expect(screen.getByTestId("rail-right")).toBeTruthy();
    expect(screen.getByTestId("viewport")).toBeTruthy();
    expect(window.localStorage.getItem("metrocalk:shell-layout:v1:right-collapsed")).toBe("true");

    fireEvent.click(screen.getByTestId("rail-right-properties"));
    expect(screen.getByTestId("dock-flyout-right")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Pin Inspector dock" }));
    expect(screen.queryByTestId("rail-right")).toBeNull();
    expect(window.localStorage.getItem("metrocalk:shell-layout:v1:right-collapsed")).toBe("false");
  });

  it("routes viewport clicks to Pipe Forge, then bakes a selectable asset", async () => {
    render(<App />);
    fireEvent.click(screen.getByRole("button", { name: "Pipe" }));
    // Pipe Forge is intentionally a production code-split workspace; allow its test transform to resolve.
    await screen.findByTestId("pipe-forge-start", {}, LAZY_CHUNK);
    fireEvent.click(screen.getByTestId("pipe-forge-start"));
    await waitFor(() => expect(screen.getByTestId("pipe-forge").getAttribute("data-active")).toBe("true"));

    const viewport = screen.getByTestId("viewport");
    fireEvent.click(viewport, { clientX: 100, clientY: 100 });
    fireEvent.click(viewport, { clientX: 220, clientY: 140 });
    await waitFor(() => expect(screen.getByTestId("pipe-forge-points").textContent).toContain("2"));
    expect((screen.getByTestId("pipe-forge-bake") as HTMLButtonElement).disabled).toBe(false);

    fireEvent.click(screen.getByTestId("pipe-forge-bake"));
    await waitFor(() => expect(screen.getAllByText("Galvanized steel pipe").length).toBeGreaterThanOrEqual(1));
    await waitFor(() => expect(screen.getByTestId("pipe-forge").getAttribute("data-active")).toBe("false"));
    const selected = projectionStore.getState().selectedId;
    expect(selected).toBeTruthy();
    expect(projectionStore.getState().displayed[selected!]?.components.PipeRecipe).toBeTruthy();

    // The projection-backed selection is recognised as an editable procedural source, not a dead mesh.
    fireEvent.click(await screen.findByTestId("pipe-forge-edit"));
    await waitFor(() => expect(screen.getByTestId("pipe-forge").getAttribute("data-active")).toBe("true"));
    expect(screen.getByTestId("pipe-forge-bake").textContent).toMatch(/rebake asset/i);
  });

  it("puts every sub-engine on ONE rail, and routes each to a real workspace", async () => {
    render(<App />);
    expect(screen.getAllByTestId("vpMove")).toHaveLength(1);
    expect(screen.getByTestId("viewport-tool-rail").querySelector("#vpMove")).toBeTruthy();
    expect(screen.getByTestId("vptoolbar").querySelector("#vpMove")).toBeNull();

    // The rail is the single index, and it says where you are.
    expect(screen.getByTestId("engine-scene").getAttribute("aria-selected")).toBe("true");

    // A side engine opens in the left column.
    fireEvent.click(screen.getByTestId("engine-build"));
    expect(screen.getByTestId("engine-build").getAttribute("aria-selected")).toBe("true");
    // `find`, not `get`: the Build workspace is an on-demand chunk, so it arrives a tick after the
    // click. The rail's `aria-selected` is synchronous and stays so — where you are must never wait on
    // a network — and the gap between the two is what `LazyWorkspace`'s named "Loading build workspace…"
    // is for.
    expect(await screen.findByTestId("assetbrowser", {}, LAZY_CHUNK)).toBeTruthy();

    // A bottom engine opens the bottom dock — same rail, different surface, because a timeline needs width.
    fireEvent.click(screen.getByTestId("engine-logic"));
    expect(screen.getByTestId("engine-logic").getAttribute("aria-selected")).toBe("true");
    expect(screen.getByTestId("bottom-dock").className).toContain("is-open");

    // Exactly one engine is ever active: "where am I?" has one answer.
    const lit = screen
      .getAllByRole("tab")
      .filter((t) => t.id.startsWith("engine-tab-") && t.getAttribute("aria-selected") === "true");
    expect(lit).toHaveLength(1);

    // And the Inspector stayed about the SELECTION throughout — it holds no sub-engines any more.
    fireEvent.click(screen.getByTestId("rail-right-relations"));
    expect(screen.getByRole("tab", { name: /relations/i }).getAttribute("aria-selected")).toBe("true");
    // Terrain is still a tab — on the RAIL, where it belongs. What must be gone is Terrain as an
    // *Inspector* tab: the Inspector answers "what is selected?", and terrain is meaningful with nothing
    // selected at all.
    const inspectorTabs = screen
      .getAllByRole("tab")
      .filter((t) => t.id.startsWith("inspector-workspaces-"))
      .map((t) => t.textContent ?? "");
    expect(inspectorTabs).toHaveLength(2);
    expect(inspectorTabs.join(" ")).not.toMatch(/terrain|physics|match/i);
    expect(screen.queryByTestId("header-workspaces")).toBeNull();
  });

  it("a dock workspace that is not an engine leaves the rail where it was", async () => {
    // Three of the bottom dock's tabs are not sub-engines: Problems and Runtime are diagnostics, and
    // Formats is a report about what this build can read and write. Selecting one used to CAST its id
    // to an EngineId and set it, so the rail lit nothing at all — the "two controls, two answers to
    // where am I" state the whole rail exists to remove, and no test noticed.
    render(<App />);
    await waitFor(() => expect(screen.getByTestId("engine-scene")).toBeTruthy());

    fireEvent.click(screen.getByTestId("engine-logic"));
    expect(screen.getByTestId("engine-logic").getAttribute("aria-selected")).toBe("true");

    // Move the bottom dock to a NON-engine workspace.
    fireEvent.click(screen.getByRole("tab", { name: /formats/i }));

    // Exactly one engine is still lit, and it is the one the user last chose.
    const lit = screen
      .getAllByRole("tab")
      .filter((t) => t.id.startsWith("engine-tab-") && t.getAttribute("aria-selected") === "true");
    expect(lit).toHaveLength(1);
    expect(screen.getByTestId("engine-logic").getAttribute("aria-selected")).toBe("true");
  });

  it("opens the searchable command palette from Ctrl/Cmd+K", async () => {
    render(<App />);
    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    expect(await screen.findByTestId("command-palette", {}, LAZY_CHUNK)).toBeTruthy();
    expect(await screen.findByRole("combobox", { name: /search commands/i })).toBeTruthy();
  });

  it("uses header-triggered focus-managed drawers at phone width", () => {
    Object.defineProperty(window, "innerWidth", { value: 500, configurable: true });
    render(<App />);
    expect(screen.queryByTestId("rail-left")).toBeNull();
    fireEvent.click(screen.getByTestId("header-scene"));
    expect(screen.getByTestId("drawer-left")).toBeTruthy();
    expect(screen.getByRole("dialog", { name: /scene and asset workspace/i })).toBeTruthy();
  });

  // THE SHORT-WINDOW REGIME, AS FAR AS JSDOM CAN CARRY IT. What is being pinned here is the WIRING —
  // that the measured column reaches `dockForm`, that its answer reaches the dock as a stable
  // `data-dock-form`, and that the stage's own overlays read it and withdraw. The geometry itself
  // (the sheet's box, the workspace's content height, the tool rail's occlusion) is not assertable
  // here at all and is not attempted: jsdom returns 0 for every rectangle, which is the whole reason
  // `pnpm shots` exists and why `shell-dock-short` is the scene that proves the pixels.
  //
  // `clientHeight` is stubbed rather than laid out, and the two custom properties are stubbed with
  // it, because `getComputedStyle` in jsdom resolves neither — a test that read them for real would
  // be measuring jsdom's CSS engine, not this shell's decision.
  it("floats the dock over the stage on a window too short to hold both, and withdraws the stage's own controls", async () => {
    const shortColumn = 399; // a 480px window: 320px of stage floor + a 42px bar + 188px of workspace does not fit
    const proto = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientHeight");
    Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get: () => shortColumn });
    const realStyle = window.getComputedStyle;
    window.getComputedStyle = ((el: Element, pe?: string | null) => {
      const cs = realStyle.call(window, el, pe ?? undefined);
      return {
        ...cs,
        getPropertyValue: (n: string) =>
          n === "--mtk-bottom-bar-height" ? "42px" : n === "--mtk-dock-content-min" ? "188px" : cs.getPropertyValue(n),
      } as CSSStyleDeclaration;
    }) as typeof window.getComputedStyle;
    try {
      render(<App />);
      // Closed, the dock is its bar and still a track — the sheet form is spent only on `.is-open`,
      // so a closed dock must never take the stage's controls away.
      expect(screen.getByTestId("bottom-dock").getAttribute("data-dock-form")).toBe("sheet");
      expect(screen.getByTestId("viewport-tool-rail")).toBeTruthy();

      fireEvent.click(screen.getByTestId("bottom-dock-toggle"));
      await waitFor(() => expect(screen.queryByTestId("viewport-tool-rail")).toBeNull());
      // Withdrawn, not covered: R3 caught these sharing pixels with the dock's tab strip, and at a
      // 400px window all five transform tools and the toolbar are behind the sheet.
      expect(screen.queryByTestId("vptoolbar")).toBeNull();
      expect(screen.queryByTestId("vpMove")).toBeNull();

      // And it is reversible by the same click that caused it — the control the whole design rests
      // on. A withdrawal that does not come back is a deletion.
      fireEvent.click(screen.getByTestId("bottom-dock-toggle"));
      await waitFor(() => expect(screen.getByTestId("viewport-tool-rail")).toBeTruthy());
    } finally {
      window.getComputedStyle = realStyle;
      if (proto) Object.defineProperty(HTMLElement.prototype, "clientHeight", proto);
      else delete (HTMLElement.prototype as unknown as Record<string, unknown>).clientHeight;
    }
  });

  // THE CONTROL THAT MATTERS: the same wiring on an ordinary window must change nothing. A test that
  // only ever asserts the degraded form passes just as well against a shell that is ALWAYS degraded.
  it("leaves an ordinary window alone — the dock is a track and the stage keeps its controls", () => {
    render(<App />);
    expect(screen.getByTestId("bottom-dock").getAttribute("data-dock-form")).toBe("docked");
    fireEvent.click(screen.getByTestId("bottom-dock-toggle"));
    expect(screen.getByTestId("viewport-tool-rail")).toBeTruthy();
    expect(screen.getByTestId("vptoolbar")).toBeTruthy();
  });
});
