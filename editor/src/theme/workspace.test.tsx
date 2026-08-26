import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { Icon } from "./icons";
import { Button } from "./primitives";
import {
  DisclosureSection,
  DockRail,
  DockTabs,
  EmptyPanelState,
  MenuPopup,
  PopupMenuGroup,
  PopupMenuItem,
  ShortcutBadge,
  WorkspacePanel,
  type DockTabItem,
} from "./workspace";

afterEach(cleanup);
beforeEach(() => window.localStorage.clear());

const tabs: DockTabItem[] = [
  { id: "scene", label: "Scene" },
  { id: "assets", label: "Assets", badge: 12 },
  { id: "unavailable", label: "Unavailable", disabled: true },
  { id: "output", label: "Output", tooltip: "Build and validation results" },
];

function TabHarness({ orientation = "horizontal" }: { orientation?: "horizontal" | "vertical" }) {
  const [active, setActive] = useState("scene");
  return (
    <DockTabs
      id="left-dock"
      tabs={tabs}
      activeId={active}
      onChange={setActive}
      ariaLabel="Left dock"
      orientation={orientation}
    />
  );
}

test("DockTabs exposes linked tab semantics and activates tabs on click", () => {
  render(<TabHarness />);
  const tablist = screen.getByRole("tablist", { name: "Left dock" });
  expect(tablist.getAttribute("aria-orientation")).toBe("horizontal");

  const scene = screen.getByRole("tab", { name: "Scene" });
  const assets = screen.getByRole("tab", { name: /Assets/ });
  expect(scene.getAttribute("aria-selected")).toBe("true");
  expect(scene.getAttribute("aria-controls")).toBe("left-dock-scene-panel");
  expect(scene.tabIndex).toBe(0);
  expect(assets.tabIndex).toBe(-1);

  fireEvent.click(assets);
  expect(assets.getAttribute("aria-selected")).toBe("true");
  expect(assets.tabIndex).toBe(0);
  expect(document.activeElement).toBe(assets);
});

test("DockTabs arrow/Home/End navigation wraps and skips disabled tabs", () => {
  render(<TabHarness />);
  const scene = screen.getByRole("tab", { name: "Scene" });
  scene.focus();

  fireEvent.keyDown(scene, { key: "ArrowLeft" });
  const output = screen.getByRole("tab", { name: "Output" });
  expect(document.activeElement).toBe(output);
  expect(output.getAttribute("aria-selected")).toBe("true");

  fireEvent.keyDown(output, { key: "Home" });
  expect(document.activeElement).toBe(scene);

  fireEvent.keyDown(scene, { key: "End" });
  expect(document.activeElement).toBe(output);

  fireEvent.keyDown(output, { key: "ArrowRight" });
  expect(document.activeElement).toBe(scene);
  expect(screen.getByRole("tab", { name: "Unavailable" }).hasAttribute("disabled")).toBe(true);
});

test("DockTabs uses Up/Down for a vertical dock", () => {
  render(<TabHarness orientation="vertical" />);
  const scene = screen.getByRole("tab", { name: "Scene" });
  scene.focus();
  fireEvent.keyDown(scene, { key: "ArrowDown" });
  expect(document.activeElement).toBe(screen.getByRole("tab", { name: /Assets/ }));
});

test("DockRail exposes compact vertical navigation and its anchored-panel state", () => {
  const onActivate = vi.fn();
  render(
    <DockRail
      side="left"
      label="Scene and creation"
      items={[
        { id: "scene", label: "Scene", icon: "S" },
        { id: "create", label: "Create", icon: "+" },
      ]}
      activeId="scene"
      popupOpen
      onActivate={onActivate}
    />,
  );
  expect(screen.getByRole("toolbar", { name: "Scene and creation" }).getAttribute("aria-orientation")).toBe("vertical");
  const scene = screen.getByRole("button", { name: "Open Scene" });
  expect(scene.getAttribute("aria-pressed")).toBe("true");
  expect(scene.getAttribute("aria-expanded")).toBe("true");
  fireEvent.click(screen.getByRole("button", { name: "Open Create" }));
  expect(onActivate).toHaveBeenCalledWith("create", expect.any(HTMLButtonElement));
});

test("DisclosureSection preserves content state while closed and persists its open state", () => {
  const { unmount } = render(
    <DisclosureSection title="Advanced" summary="Rarely changed" storageKey="advanced" defaultOpen={false}>
      <input aria-label="Iterations" defaultValue="4" />
    </DisclosureSection>,
  );
  const toggle = screen.getByRole("button", { name: /Advanced/ });
  const region = screen.getByRole("region", { hidden: true });
  expect(toggle.getAttribute("aria-expanded")).toBe("false");
  expect(region.getAttribute("aria-hidden")).toBe("true");
  expect(region.hasAttribute("inert")).toBe(true);
  expect(toggle.getAttribute("aria-describedby")).toBeTruthy();
  expect(toggle.textContent).toContain("Rarely changed");

  fireEvent.click(toggle);
  expect(toggle.getAttribute("aria-expanded")).toBe("true");
  expect(region.getAttribute("aria-hidden")).toBe("false");
  expect(region.hasAttribute("inert")).toBe(false);
  fireEvent.change(screen.getByRole("textbox", { name: "Iterations" }), { target: { value: "9" } });
  fireEvent.click(toggle);
  expect((screen.getByRole("textbox", { name: "Iterations", hidden: true }) as HTMLInputElement).value).toBe("9");

  // Re-open before remounting so the persisted preference is independently observable.
  fireEvent.click(toggle);
  unmount();
  render(
    <DisclosureSection title="Advanced" storageKey="advanced" defaultOpen={false}>
      Settings
    </DisclosureSection>,
  );
  expect(screen.getByRole("button", { name: "Advanced" }).getAttribute("aria-expanded")).toBe("true");
});

test("DisclosureSection exposes semantic state, card tone, and explicit unmount mode", () => {
  const { rerender } = render(
    <DisclosureSection id="material-settings" title="Material" tone="card" headingLevel={4} open={false} unmountOnClose>
      <button>Choose material</button>
    </DisclosureSection>,
  );
  const toggle = screen.getByRole("button", { name: "Material" });
  expect(toggle.closest("h4")).not.toBeNull();
  expect(toggle.closest("section")?.id).toBe("material-settings");
  expect(toggle.closest("section")?.className).toContain("mtk-disclosure--card");
  expect(toggle.closest("section")?.getAttribute("data-state")).toBe("closed");
  expect(screen.queryByRole("button", { name: "Choose material" })).toBeNull();

  rerender(
    <DisclosureSection id="material-settings" title="Material" tone="card" headingLevel={4} open unmountOnClose>
      <button>Choose material</button>
    </DisclosureSection>,
  );
  expect(screen.getByRole("button", { name: "Choose material" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Material" }).closest("section")?.getAttribute("data-state")).toBe("open");
});

test("DisclosureSection supports controlled state and keeps header actions independent", () => {
  const onOpenChange = vi.fn();
  const onReset = vi.fn();
  render(
    <DisclosureSection
      title="Bake settings"
      open={false}
      onOpenChange={onOpenChange}
      actions={<button onClick={onReset}>Reset</button>}
    >
      Options
    </DisclosureSection>,
  );
  fireEvent.click(screen.getByRole("button", { name: "Reset" }));
  expect(onReset).toHaveBeenCalledTimes(1);
  expect(onOpenChange).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Bake settings" }));
  expect(onOpenChange).toHaveBeenCalledWith(true);
});

test("WorkspacePanel composes a labelled fixed header, toolbar, scroll body, tabs, and footer", () => {
  render(
    <WorkspacePanel
      title="Inspector"
      subtitle="Cube"
      actions={<button>More</button>}
      actionsLabel="Inspector actions"
      tabs={<div data-testid="tabs">tabs</div>}
      footer={<div data-testid="footer">Saved</div>}
      padded
      busy
      data-testid="panel"
    >
      <div>Properties</div>
    </WorkspacePanel>,
  );
  const panel = screen.getByTestId("panel");
  expect(panel.tagName).toBe("SECTION");
  expect(panel.getAttribute("aria-labelledby")).toBe(screen.getByRole("heading", { name: "Inspector" }).id);
  expect(panel.getAttribute("aria-busy")).toBe("true");
  expect(screen.getByRole("toolbar", { name: "Inspector actions" })).toBeTruthy();
  expect(screen.getByText("Properties").parentElement?.className).toContain("mtk-scroll");
  expect(screen.getByText("Properties").parentElement?.className).toContain("is-padded");
  expect(screen.getByTestId("tabs")).toBeTruthy();
  expect(screen.getByTestId("footer")).toBeTruthy();
});

test("EmptyPanelState provides actionable guidance and ShortcutBadge has a spoken key sequence", () => {
  render(
    <EmptyPanelState
      title="Nothing selected"
      description="Select an object in the viewport to inspect it."
      icon={<Icon name="requirer" size="xl" />}
      primaryAction={<Button>Select all</Button>}
      secondaryAction={<ShortcutBadge keys={["Ctrl", "A"]} />}
    />,
  );
  expect(screen.getByRole("status").textContent).toContain("Nothing selected");
  expect(screen.getByRole("button", { name: "Select all" })).toBeTruthy();
  expect(screen.getByLabelText("Ctrl plus A").tagName).toBe("KBD");
});

test("MenuPopup owns trigger semantics, keyboard entry, selection dismissal, and focus return", async () => {
  const choose = vi.fn();
  render(
    <MenuPopup
      id="workspace-picker"
      label="Choose workspace"
      trigger="Workspaces"
      triggerProps={{ "data-testid": "workspace-trigger", variant: "secondary" }}
    >
      {(close) => (
        <PopupMenuGroup label="Editors">
          <PopupMenuItem label="Animation" description="Keys, curves and clips" onSelect={choose} onRequestClose={close} />
          <PopupMenuItem label="Physics" description="Simulation controls" onSelect={() => {}} onRequestClose={close} />
        </PopupMenuGroup>
      )}
    </MenuPopup>,
  );

  const trigger = screen.getByTestId("workspace-trigger");
  expect(trigger.getAttribute("aria-expanded")).toBe("false");
  fireEvent.keyDown(trigger, { key: "ArrowDown" });
  expect(trigger.getAttribute("aria-expanded")).toBe("true");
  expect(screen.getByRole("menu", { name: "Choose workspace" })).toBeTruthy();
  const animation = screen.getByRole("menuitem", { name: /Animation/ });
  const physics = screen.getByRole("menuitem", { name: /Physics/ });
  expect(animation.className).toContain("is-without-icon");
  expect(document.activeElement).toBe(animation);
  fireEvent.keyDown(animation, { key: "End" });
  expect(document.activeElement).toBe(physics);
  fireEvent.click(animation);
  await waitFor(() => expect(screen.queryByRole("menu", { name: "Choose workspace" })).toBeNull());
  expect(choose).toHaveBeenCalledTimes(1);
  expect(document.activeElement).toBe(trigger);
});
