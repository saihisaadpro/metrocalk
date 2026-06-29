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
  Object.defineProperty(window, "innerWidth", { value: 1024, configurable: true });
});

describe("editor app — end-to-end wiring", () => {
  it("projects the first-run SAMPLE scene (named, not the 5k fixture) and renders an inspector form on select", () => {
    render(<App />);
    // the small NAMED first-run scene loads through client → store → hierarchy (C10: never 5000 rows).
    // Keys off the stable `data-testid="hierarchy"` (not the restyled header copy — structured-signal rule).
    expect(screen.getByTestId("hierarchy").textContent).not.toContain("5000");
    // a meaningful named starter entity appears (not "Entity N")
    expect(screen.getAllByText("Player").length).toBeGreaterThanOrEqual(1);

    // selecting it renders the schema-driven inspector header (≥2: the hierarchy row + the inspector)
    act(() => projectionStore.getState().select("player"));
    expect(screen.getAllByText("Player").length).toBeGreaterThanOrEqual(2);
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

  it("an empty scene shows a real empty-state with one next step (C10)", () => {
    render(<App />);
    expect(screen.queryByTestId("emptyState")).toBeNull(); // the sample scene is not empty
    act(() => projectionStore.getState().reset());
    expect(screen.getByTestId("emptyState").textContent).toMatch(/describe your first object/i);
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
    act(() => fireEvent.click(screen.getByTestId("rail-left")));
    expect(screen.getByTestId("drawer-left")).toBeTruthy();
  });
});
