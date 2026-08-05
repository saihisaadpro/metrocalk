import { afterEach, expect, test, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { PhysicsPanel } from "./PhysicsPanel";
import { fakeClient } from "../transport/test-client";
import { projectionStore } from "../store/projection";
import { uiStore } from "../store/ui";
import type { TimelineTuple } from "../transport/protocol";

afterEach(() => {
  cleanup();
  projectionStore.getState().reset();
  uiStore.getState().setStatus("");
  window.localStorage.clear();
});

function openSection(title: string) {
  const heading = screen.getByText(title);
  const toggle = heading.closest("button");
  expect(toggle).not.toBeNull();
  fireEvent.click(toggle!);
}

test("physics groups common controls first and labels every input", () => {
  render(<PhysicsPanel client={fakeClient()} />);

  expect(screen.getByRole("region", { name: "Physics controls" })).toBeTruthy();
  expect(screen.getByLabelText("Recorded timeline").id).toBe("scrub");
  expect(screen.getByTestId("scrub").hasAttribute("disabled")).toBe(true);
  expect(screen.getByText(/Run the simulation to record frames/)).toBeTruthy();

  const diagnostics = screen.getByText("Contact diagnostics").closest("button");
  const interchange = screen.getByText("Robot interchange").closest("button");
  expect(diagnostics?.getAttribute("aria-expanded")).toBe("false");
  expect(interchange?.getAttribute("aria-expanded")).toBe("false");
  expect(screen.queryByTestId("importPanel")).toBeNull();
  expect(screen.queryByText(/M8/)).toBeNull();
});

test("simulation, body, and contact controls retain their client calls", async () => {
  const setSimRunning = vi.fn();
  const simTimeline = vi.fn(() => Promise.resolve([0, 6, false, false, 0] as TimelineTuple));
  const simOverlay = vi.fn();
  const physicsContacts = vi.fn(() => Promise.resolve([]));
  const simShove = vi.fn(() => Promise.resolve(true));
  const setField = vi.fn(() => "op");
  projectionStore.getState().select("body");
  render(
    <PhysicsPanel
      client={fakeClient({ setSimRunning, simTimeline, simOverlay, physicsContacts, simShove, setField })}
    />,
  );

  fireEvent.click(screen.getByTestId("simToggle"));
  await waitFor(() => expect(setSimRunning).toHaveBeenCalledWith(true));
  await waitFor(() => expect(screen.getByTestId("scrub").hasAttribute("disabled")).toBe(false));

  fireEvent.click(screen.getByTestId("shove"));
  await waitFor(() => expect(simShove).toHaveBeenCalledWith("body", [4, 1, 0]));
  fireEvent.click(screen.getByTestId("nudgeFriction"));
  expect(setField).toHaveBeenCalledWith("body", "Collider", "friction", 0.95);

  openSection("Contact diagnostics");
  fireEvent.click(screen.getByTestId("dbgToggle"));
  await waitFor(() => expect(simOverlay).toHaveBeenCalledWith(true));
  expect(await screen.findByText("No contacts at this frame")).toBeTruthy();
});

test("robot interchange is disclosed, labelled, and reports its reconciliation", async () => {
  const importInterchange = vi.fn((_format: string, _source: string) =>
    Promise.resolve({
      ok: true,
      format: "urdf",
      bodies: 2,
      joints: 1,
      meters_per_unit: 1,
      kilograms_per_unit: 1,
      reconciled: true,
      notes: ["cylinder collider reconciled"],
      error: null,
    }),
  );
  render(<PhysicsPanel client={fakeClient({ importInterchange })} />);

  openSection("Robot interchange");
  fireEvent.click(screen.getByTestId("importRobot"));
  expect(screen.getByLabelText("URDF or USD source").id).toBe("impText");
  fireEvent.click(screen.getByTestId("impSample"));
  fireEvent.click(screen.getByTestId("impGo"));

  await waitFor(() => expect(importInterchange).toHaveBeenCalled());
  expect(importInterchange.mock.calls[0][0]).toBe("urdf");
  const result = await screen.findByTestId("impResult");
  expect(result.textContent).toMatch(/imported 2 bodies · 1 joints/);
  expect(result.textContent).toMatch(/cylinder collider reconciled/);
  fireEvent.click(screen.getByTestId("impClose"));
  expect(screen.queryByTestId("importPanel")).toBeNull();
});
