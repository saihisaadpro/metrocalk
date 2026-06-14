import { act, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { App } from "./App";
import { projectionStore } from "../store/projection";

describe("editor app — end-to-end wiring", () => {
  it("projects the 5k scene from the core and renders an inspector form on select", () => {
    render(<App />);
    // the 5k scene loaded through client → store → hierarchy (core→UI delta projection)
    expect(screen.getByText(/hierarchy/i).textContent).toContain("5000");

    // selecting an entity renders the schema-driven inspector (JSON Forms) without crashing —
    // "Entity 5" now appears in BOTH the hierarchy row and the inspector header (≥2 occurrences).
    act(() => projectionStore.getState().select("e5"));
    expect(screen.getAllByText("Entity 5").length).toBeGreaterThanOrEqual(2);
  });
});
