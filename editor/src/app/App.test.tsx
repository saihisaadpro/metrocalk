import { act, render, screen, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { App } from "./App";
import { projectionStore } from "../store/projection";
import { playStore } from "../store/play";
import { walletStore } from "../store/wallet";
import { toastStore } from "../store/toasts";
import { uiStore } from "../store/ui";

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
    await waitFor(
      () => expect(document.getElementById("inspector")?.textContent).toContain("Player"),
      { timeout: 10_000 },
    );
  }, 10_000);

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
    expect(screen.getByTestId("emptyPipe")).toBeTruthy();
    expect(screen.getByTestId("emptyAssets")).toBeTruthy();
    expect(screen.getByTestId("emptyImport")).toBeTruthy();
  });

  it("a spend in one panel updates the displayed balance EVERYWHERE (single source of truth — C7)", async () => {
    render(<App />);
    const bal = await screen.findByTestId("balance");
    await waitFor(() => expect(bal.textContent).toBe("100"));

    // Describe lives in the Build workspace, which is an on-demand rail destination: it is MOUNTED WHEN
    // OPENED now, not rendered-and-hidden behind every other engine. So the test opens it, exactly as a
    // user must. (It used to be reachable without this click because the dock rendered all six engines
    // at once and hid five — a testid you can query from a workspace you are not in is not evidence that
    // a user can reach it.)
    // EVERY WAIT ON A LAZY WORKSPACE IS GIVEN REAL TIME. `findBy*` defaults to a **1000 ms** budget, and
    // opening Build resolves a dynamic `import()` — so this line was a wall-clock assertion inside a
    // parallel test runner, the same class as the `performance.now()` bound removed from
    // `projection.test.tsx`. Measured on an idle machine at HEAD, before this change: two runs of this
    // file alone, one green and one red on exactly this line. The test above it already carries
    // `{ timeout: 10_000 }` for the same reason, which is the tell that the budget — not the code — was
    // what this file kept failing on. A flake is a failure; nothing about the CLAIM changes.
    fireEvent.click(screen.getByTestId("engine-build"));
    fireEvent.change(await screen.findByTestId("describe", {}, { timeout: 10_000 }), {
      target: { value: "a nonexistent thingamajig" },
    });
    fireEvent.click(screen.getByTestId("describeBtn"));
    fireEvent.click(await screen.findByTestId("genBtn", {}, { timeout: 10_000 }));

    // the top-bar Wallet's displayed balance dropped — it reads the SAME store the DescribeBar wrote to
    await waitFor(() => expect(bal.textContent).toBe("90"), { timeout: 10_000 });
  }, 20_000);

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
    await screen.findByTestId("pipe-forge-start", {}, { timeout: 5_000 });
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
    expect(await screen.findByTestId("assetbrowser")).toBeTruthy();

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
    // WAIT FOR THE APP TO BE LISTENING. `App` installs its window `keydown` handler in an effect, and
    // a chord fired in the same tick as `render` is dispatched at a window nothing is bound to yet —
    // so the palette never opens and the failure reads as "the chord is not wired". It flakes with
    // machine load, which is what makes it worthless as a gate; this test failed on the untouched
    // main checkout while the box was busy (`<test_and_ci_discipline>` 4 — a flake is a failure).
    await waitFor(() => expect(screen.getByTestId("engine-scene")).toBeTruthy());
    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    expect(await screen.findByTestId("command-palette")).toBeTruthy();
    expect(await screen.findByRole("combobox", { name: /search commands/i })).toBeTruthy();
  });

  it("Ctrl/Cmd+D is BOUND to duplicate, and the palette row teaches the chord (ADR-196)", async () => {
    render(<App />);
    await waitFor(() => expect(screen.getByTestId("engine-scene")).toBeTruthy());
    // The chord did nothing at all before this: the only routes to a copy were a row inside a popup
    // menu in the left dock and a right-click row. A refusal that says what to do is the proof it is
    // bound — silence is what it used to do.
    fireEvent.keyDown(window, { key: "d", ctrlKey: true });
    await waitFor(() => expect(uiStore.getState().status).toBe("Select an object to duplicate"));

    // And it is DISCOVERABLE: a palette row carries the chord, which is where a person learns it.
    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    const palette = await screen.findByTestId("command-palette");
    await waitFor(() => expect(palette.textContent).toContain("Duplicate"));
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
