import { fireEvent, render, screen } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { EngineRail, ENGINES, ENGINE_GROUPS, engineById } from "./EngineRail";

test("lists every sub-engine exactly once, grouped", () => {
  render(<EngineRail active="scene" onChange={vi.fn()} />);
  // The whole point: one place that answers "what can this editor do?".
  for (const e of ENGINES) {
    expect(screen.getByTestId(`engine-${e.id}`).textContent).toContain(e.label);
  }
  expect(ENGINES.length).toBe(9);
  // Grouped, because nine flat items is a wall and three groups of three is a glance.
  for (const g of ENGINE_GROUPS) {
    expect(screen.getByText(g.title)).toBeTruthy();
    expect(g.engines.length).toBe(3);
  }
  // Ids are unique — a duplicate would make two rail entries fight over the same panel.
  expect(new Set(ENGINES.map((e) => e.id)).size).toBe(ENGINES.length);
});

test("marks where you are, and only one place at a time", () => {
  render(<EngineRail active="terrain" onChange={vi.fn()} />);
  expect(screen.getByTestId("engine-terrain").getAttribute("aria-selected")).toBe("true");
  const selected = screen.getAllByRole("tab").filter((t) => t.getAttribute("aria-selected") === "true");
  expect(selected.length).toBe(1);
});

test("every entry reports where it opens, so the routing is data and not a guess", () => {
  // The app switches on exactly this field; if an engine had no surface the click would do nothing.
  for (const e of ENGINES) {
    expect(["side", "bottom"]).toContain(e.surface);
    expect(engineById(e.id).surface).toBe(e.surface);
  }
});

test("clicking asks the app to open that engine", () => {
  const onChange = vi.fn();
  render(<EngineRail active="scene" onChange={onChange} />);
  fireEvent.click(screen.getByTestId("engine-physics"));
  expect(onChange).toHaveBeenCalledWith("physics");
});

test("arrow keys walk the whole rail, across the group headings", () => {
  // The headings are for the eye. A keyboard user should not have to know they exist, and must never get
  // stuck at the bottom of a group.
  const onChange = vi.fn();
  render(<EngineRail active="terrain" onChange={onChange} />);
  fireEvent.keyDown(screen.getByTestId("engine-terrain"), { key: "ArrowDown" });
  // Terrain is the last of "World"; down must land on the first of "Assets".
  expect(onChange).toHaveBeenCalledWith("model");
  onChange.mockClear();
  // And it wraps rather than dead-ending.
  render(<EngineRail active="gameplay" onChange={onChange} />);
  fireEvent.keyDown(screen.getAllByTestId("engine-gameplay")[1], { key: "ArrowDown" });
  expect(onChange).toHaveBeenCalledWith("scene");
});

test("compact mode keeps every engine reachable when the window is narrow", () => {
  // An index you lose when the window shrinks is not an index.
  render(<EngineRail active="scene" onChange={vi.fn()} compact />);
  for (const e of ENGINES) {
    expect(screen.getByTestId(`engine-${e.id}`)).toBeTruthy();
  }
  // Labels are dropped, so the title carries the name and the blurb for a hover.
  expect(screen.getByTestId("engine-terrain").getAttribute("title")).toContain("Terrain");
  expect(screen.getByTestId("engine-terrain").getAttribute("title")).toContain("Landscape");
});

test("each engine says what it is for, in the author's language", () => {
  // The blurb is what makes a nine-item list learnable rather than nine words to decode.
  for (const e of ENGINES) {
    expect(e.blurb.length).toBeGreaterThan(8);
    expect(e.blurb[0]).toBe(e.blurb[0].toUpperCase());
  }
});
