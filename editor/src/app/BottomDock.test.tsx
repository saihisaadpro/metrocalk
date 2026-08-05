import { useState } from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, test } from "vitest";
import { projectionStore } from "../store/projection";
import { fakeClient } from "../transport/test-client";
import { BottomDock, type BottomWorkspace } from "./BottomDock";

function Harness({ initialActive = "asset", initialOpen = false }: { initialActive?: BottomWorkspace; initialOpen?: boolean }) {
  const [active, setActive] = useState<BottomWorkspace>(initialActive);
  const [open, setOpen] = useState(initialOpen);
  return (
    <BottomDock
      client={fakeClient()}
      active={active}
      open={open}
      playing={false}
      onChange={setActive}
      onOpenChange={setOpen}
    />
  );
}

beforeEach(() => {
  projectionStore.getState().select(null);
});

test("collapsed dock shows one workspace summary and keeps selection separate from explicit expansion", async () => {
  render(<Harness initialActive="asset" />);

  expect(screen.queryByRole("tablist", { name: "Task workspaces" })).toBeNull();
  const summary = screen.getByTestId("bottom-workspace-summary");
  expect(summary.textContent).toContain("Asset Lab");
  expect(screen.getByTestId("bottom-dock-toggle").getAttribute("aria-expanded")).toBe("false");

  fireEvent.click(summary);
  expect(screen.getByRole("menu", { name: "Choose task workspace" })).toBeTruthy();
  expect(screen.getAllByRole("menuitem")).toHaveLength(6);
  fireEvent.click(screen.getByRole("menuitem", { name: /runtime/i }));

  await waitFor(() => expect(screen.getByTestId("bottom-workspace-summary").textContent).toContain("Runtime"));
  expect(screen.getByTestId("bottom-dock").className).not.toContain("is-open");
  expect(screen.queryByRole("tablist", { name: "Task workspaces" })).toBeNull();

  fireEvent.click(screen.getByTestId("bottom-dock-toggle"));
  await waitFor(() => expect(screen.getByTestId("bottom-dock").className).toContain("is-open"));
  expect(screen.getByRole("tablist", { name: "Task workspaces" })).toBeTruthy();
  expect(screen.getByRole("tab", { name: /runtime/i }).getAttribute("aria-selected")).toBe("true");
  expect(document.getElementById("bottom-workspaces-runtime-panel")).toBeTruthy();
});

test("selecting the active tab keeps the dock open and only the explicit control collapses it", async () => {
  render(<Harness initialActive="runtime" initialOpen />);

  const activeTab = screen.getByRole("tab", { name: /runtime/i });
  fireEvent.click(activeTab);
  expect(screen.getByTestId("bottom-dock").className).toContain("is-open");
  expect(screen.getByTestId("bottom-dock-toggle").getAttribute("aria-expanded")).toBe("true");

  fireEvent.click(screen.getByTestId("bottom-dock-toggle"));
  await waitFor(() => expect(screen.getByTestId("bottom-dock").className).not.toContain("is-open"));
  expect(screen.queryByRole("tablist", { name: "Task workspaces" })).toBeNull();
  expect(screen.getByTestId("bottom-workspace-summary").textContent).toContain("Runtime");
  expect(document.getElementById("bottom-workspaces-runtime-panel")).toBeTruthy();
});
