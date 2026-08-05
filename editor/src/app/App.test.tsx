import { act, render, screen, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { App } from "./App";
import { projectionStore } from "../store/projection";
import { playStore } from "../store/play";
import { walletStore } from "../store/wallet";
import { toastStore } from "../store/toasts";

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

    // describe a no-match → the Generate offer; clicking Generate debits via the shared wallet store
    fireEvent.change(screen.getByTestId("describe"), { target: { value: "a nonexistent thingamajig" } });
    fireEvent.click(screen.getByTestId("describeBtn"));
    fireEvent.click(await screen.findByTestId("genBtn"));

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

  it("groups editor capabilities into discoverable dock workspaces", () => {
    render(<App />);
    expect(screen.getAllByTestId("vpMove")).toHaveLength(1);
    expect(screen.getByTestId("viewport-tool-rail").querySelector("#vpMove")).toBeTruthy();
    expect(screen.getByTestId("vptoolbar").querySelector("#vpMove")).toBeNull();
    expect(screen.getByRole("tab", { name: /scene/i }).getAttribute("aria-selected")).toBe("true");
    fireEvent.click(screen.getByRole("tab", { name: /create/i }));
    expect(screen.getByRole("tab", { name: /create/i }).getAttribute("aria-selected")).toBe("true");
    expect(screen.getByTestId("assetbrowser")).toBeTruthy();

    fireEvent.click(screen.getByTestId("rail-right-relations"));
    expect(screen.getByRole("tab", { name: /relations/i }).getAttribute("aria-selected")).toBe("true");
    fireEvent.click(screen.getByTestId("header-workspaces"));
    fireEvent.click(screen.getByTestId("header-logic"));
    expect(screen.getByTestId("bottom-dock").className).toContain("is-open");
    expect(screen.getByRole("tab", { name: /logic/i }).getAttribute("aria-selected")).toBe("true");
  });

  it("opens the searchable command palette from Ctrl/Cmd+K", () => {
    render(<App />);
    fireEvent.keyDown(window, { key: "k", ctrlKey: true });
    expect(screen.getByTestId("command-palette")).toBeTruthy();
    expect(screen.getByRole("combobox", { name: /search commands/i })).toBeTruthy();
  });

  it("uses header-triggered focus-managed drawers at phone width", () => {
    Object.defineProperty(window, "innerWidth", { value: 500, configurable: true });
    render(<App />);
    expect(screen.queryByTestId("rail-left")).toBeNull();
    fireEvent.click(screen.getByTestId("header-scene"));
    expect(screen.getByTestId("drawer-left")).toBeTruthy();
    expect(screen.getByRole("dialog", { name: /scene and asset workspace/i })).toBeTruthy();
  });
});
