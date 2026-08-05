import { afterEach, expect, test, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { TransformPanel } from "./TransformPanel";
import { fakeClient } from "../transport/test-client";
import { projectionStore } from "../store/projection";
import { uiStore } from "../store/ui";

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

test("transform controls are labelled, dock-embeddable, and progressively disclosed", async () => {
  render(<TransformPanel client={fakeClient()} />);

  expect(screen.getByRole("region", { name: "Transform and placement controls" })).toBeTruthy();
  expect(screen.getByText(/Select an entity in the viewport/)).toBeTruthy();
  expect(screen.getByLabelText("Placement instruction").id).toBe("placeSentence");
  expect(screen.getByRole("button", { name: /Magnetic snap: On/ }).getAttribute("aria-pressed")).toBe("true");

  const reuseToggle = screen.getByText("Reusable parts").closest("button");
  const reuseContent = document.getElementById(reuseToggle!.getAttribute("aria-controls")!);
  expect(reuseToggle!.getAttribute("aria-expanded")).toBe("false");
  expect(reuseContent?.hidden).toBe(true);
  openSection("Reusable parts");
  expect(reuseToggle!.getAttribute("aria-expanded")).toBe("true");
  expect(screen.getByTestId("saveChar")).toBeTruthy();

  openSection("Structure & visibility");
  expect(screen.getByLabelText("Parent entity ID").id).toBe("reparentTo");
  expect(screen.getByTestId("deactPart").textContent).toMatch(/Deactivate selected part/);

  await screen.findByText(/Choose a gizmo mode/);
  expect(screen.getByRole("group", { name: "Gizmo keyboard shortcuts" })).toBeTruthy();
  expect(screen.queryByText(/M9/)).toBeNull();
});

test("transform actions preserve the existing editor client workflow", async () => {
  const setSnap = vi.fn();
  const snapQuery = vi.fn(() =>
    Promise.resolve([{ id: "target", kind: "surface", x: 0, y: 0, z: 0, distance: 1, why: "faces align" }]),
  );
  const applyConstraint = vi.fn(() => Promise.resolve({ ok: true, reason: null, intents: [] }));
  const placementSentence = vi.fn(() => Promise.resolve({ ok: true, reason: null, intents: ["upright"] }));
  const saveCharacter = vi.fn(() => Promise.resolve("saved-character"));
  const instantiateCharacter = vi.fn(() => Promise.resolve("instance"));
  const reparentPart = vi.fn();
  const setPartActive = vi.fn(() => Promise.resolve(true));
  const client = fakeClient({
    gizmoSelected: () => Promise.resolve("selected"),
    setSnap,
    snapQuery,
    applyConstraint,
    placementSentence,
    saveCharacter,
    instantiateCharacter,
    reparentPart,
    setPartActive,
  });
  projectionStore.getState().select("selected");
  render(<TransformPanel client={client} />);

  fireEvent.click(screen.getByTestId("snapToggle"));
  expect(setSnap).toHaveBeenCalledWith(false);

  fireEvent.click(screen.getByTestId("snapNearest"));
  await waitFor(() => expect(applyConstraint).toHaveBeenCalledWith("selected", "snap", "target", 0));

  fireEvent.change(screen.getByLabelText("Placement instruction"), { target: { value: "upright" } });
  fireEvent.click(screen.getByTestId("placeBtn"));
  await waitFor(() => expect(placementSentence).toHaveBeenCalledWith("selected", "upright"));

  openSection("Reusable parts");
  fireEvent.click(screen.getByTestId("saveChar"));
  await waitFor(() => expect(screen.getByTestId("dropInst").hasAttribute("disabled")).toBe(false));
  fireEvent.click(screen.getByTestId("dropInst"));
  await waitFor(() => expect(instantiateCharacter).toHaveBeenCalledWith("saved-character"));

  openSection("Structure & visibility");
  fireEvent.change(screen.getByLabelText("Parent entity ID"), { target: { value: "parent" } });
  fireEvent.click(screen.getByTestId("reparentBtn"));
  await waitFor(() => expect(reparentPart).toHaveBeenCalledWith("selected", "parent"));
  fireEvent.click(screen.getByTestId("deactPart"));
  await waitFor(() => expect(setPartActive).toHaveBeenCalledWith("selected", false));
});
