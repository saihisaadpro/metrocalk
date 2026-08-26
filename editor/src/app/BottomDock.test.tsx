import { useState } from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, expect, test } from "vitest";
import { projectionStore } from "../store/projection";
import { fakeClient } from "../transport/test-client";
import { BottomDock, type AnimateWorkspace, type BottomWorkspace } from "./BottomDock";

function Harness({ initialActive = "asset", initialOpen = false, initialAnimate = "properties" }: { initialActive?: BottomWorkspace; initialOpen?: boolean; initialAnimate?: AnimateWorkspace }) {
  const [active, setActive] = useState<BottomWorkspace>(initialActive);
  const [open, setOpen] = useState(initialOpen);
  const [animate, setAnimate] = useState<AnimateWorkspace>(initialAnimate);
  return (
    <BottomDock
      client={fakeClient()}
      active={active}
      open={open}
      playing={false}
      animate={animate}
      onChange={setActive}
      onAnimateChange={setAnimate}
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
  // The dock uses the SAME word as the Engines rail — one capability, one name.
  expect(summary.textContent).toContain("Model");
  expect(screen.getByTestId("bottom-dock-toggle").getAttribute("aria-expanded")).toBe("false");

  fireEvent.click(summary);
  expect(screen.getByRole("menu", { name: "Choose task workspace" })).toBeTruthy();
  // Named rather than counted. A bare number goes stale every time a workspace is added and teaches
  // whoever hits it to just bump the digit; naming them means a new workspace has to be acknowledged
  // here, and a DUPLICATE entry — a panel accidentally mounted twice — still fails on the length.
  const WORKSPACES = ["Model", "Import", "Formats", "Animate", "Logic", "Problems", "Runtime"];
  const items = screen.getAllByRole("menuitem").map((el) => el.textContent ?? "");
  expect(items).toHaveLength(WORKSPACES.length);
  for (const name of WORKSPACES) {
    expect(items.filter((t) => t.includes(name))).toHaveLength(1);
  }
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
