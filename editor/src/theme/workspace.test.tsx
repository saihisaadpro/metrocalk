import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { Icon } from "./icons";
import { Button } from "./primitives";
import {
  ChoiceCard,
  ChoiceGrid,
  DisclosureSection,
  DockRail,
  DockTabs,
  EmptyPanelState,
  MenuPopup,
  NavRail,
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

// ── NavRail (ADR-147) ─────────────────────────────────────────────────────────────────────────────

const RAIL_ITEMS = [
  { id: "inspect", label: "Inspect", icon: <Icon name="inspect" size="md" />, tooltip: "Measure before changing." },
  { id: "repair", label: "Repair", icon: <Icon name="repair" size="md" /> },
  { id: "export", label: "Export", icon: <Icon name="export" size="md" />, tooltip: "No verified export writer is connected." },
];

test("NavRail: a real tablist whose selected item is the only tab stop", () => {
  render(<NavRail id="stages" label="Stages" items={RAIL_ITEMS} activeId="repair" onChange={vi.fn()} />);

  expect(screen.getByRole("tablist", { name: "Stages" }).getAttribute("aria-orientation")).toBe("vertical");
  // ONE stop in the tab order for the whole rail: seven stages must not become seven Tab presses
  // between the panel header and its content.
  const stops = screen.getAllByRole("tab").filter((tab) => tab.getAttribute("tabindex") === "0");
  expect(stops).toHaveLength(1);
  expect(stops[0].textContent).toContain("Repair");
});

test("NavRail: arrows move the selection AND the focus, and wrap at both ends", () => {
  function Harness() {
    const [active, setActive] = useState("inspect");
    return <NavRail id="stages" label="Stages" items={RAIL_ITEMS} activeId={active} onChange={setActive} />;
  }
  render(<Harness />);

  fireEvent.keyDown(screen.getByRole("tab", { name: "Inspect" }), { key: "ArrowDown" });
  expect(screen.getByRole("tab", { name: "Repair" }).getAttribute("aria-selected")).toBe("true");
  expect(document.activeElement).toBe(screen.getByRole("tab", { name: "Repair" }));

  // Wrapping is the claim: a rail that stops at the last item strands a keyboard user at the end.
  fireEvent.keyDown(screen.getByRole("tab", { name: "Repair" }), { key: "ArrowUp" });
  expect(screen.getByRole("tab", { name: "Inspect" }).getAttribute("aria-selected")).toBe("true");
  fireEvent.keyDown(screen.getByRole("tab", { name: "Inspect" }), { key: "ArrowUp" });
  expect(screen.getByRole("tab", { name: "Export" }).getAttribute("aria-selected")).toBe("true");
});

test("NavRail: each tab names the panel it controls, so the pair is one relationship and not two ids", () => {
  render(
    <NavRail id="stages" label="Stages" items={RAIL_ITEMS} activeId="inspect" onChange={vi.fn()} panelIdPrefix="asset-lab-" />,
  );
  const tab = screen.getByRole("tab", { name: "Inspect" });
  expect(tab.id).toBe("stages-inspect-tab");
  expect(tab.getAttribute("aria-controls")).toBe("asset-lab-inspect");
});

test("NavRail: a disabled item is skipped by the arrows and states its reason in words", () => {
  const onChange = vi.fn();
  const items = [
    RAIL_ITEMS[0],
    { ...RAIL_ITEMS[1], disabled: true, disabledReason: "Run Inspect first." },
    RAIL_ITEMS[2],
  ];
  render(<NavRail id="stages" label="Stages" items={items} activeId="inspect" onChange={onChange} />);

  expect(screen.getByRole("tab", { name: "Repair" }).getAttribute("title")).toBe("Run Inspect first.");
  fireEvent.keyDown(screen.getByRole("tab", { name: "Inspect" }), { key: "ArrowDown" });
  // Straight past Repair — landing on a stage that refuses is a move that does nothing.
  expect(onChange).toHaveBeenCalledWith("export");
});

test("NavRail: clicking an item reports it, and a tooltip is the item's plain-language hint", () => {
  const onChange = vi.fn();
  render(<NavRail id="stages" label="Stages" items={RAIL_ITEMS} activeId="inspect" onChange={onChange} />);
  expect(screen.getByRole("tab", { name: "Inspect" }).getAttribute("title")).toBe("Measure before changing.");
  fireEvent.click(screen.getByRole("tab", { name: "Export" }));
  expect(onChange).toHaveBeenCalledWith("export");
});

// ── ChoiceGrid / ChoiceCard ───────────────────────────────────────────────────────────────────────
//
// jsdom implements no layout, so the RESPONSIVE half of this component — the `auto-fit` column count
// that replaced four hardcoded `repeat(2, 1fr)` grids — is not testable here and is asserted by the
// `gameplay-wide` shot instead. What IS testable is the part four panels each had a private opinion
// about: what a card announces, what it says when it refuses, and whether the sentence that explains
// it is rendered or merely hovered.

test("ChoiceCard: the description is RENDERED, not hidden in a title", () => {
  render(
    <ChoiceGrid label="Assign a role">
      <ChoiceCard label="Collectible" description="Spins; vanishes and scores" onSelect={() => {}} />
    </ChoiceGrid>,
  );
  // The whole point of the primitive: a blurb that exists only as a `title` is invisible to touch, to
  // the keyboard and to anyone reading the panel rather than hunting in it.
  expect(screen.getByText("Spins; vanishes and scores")).toBeTruthy();
  expect(screen.getByRole("group", { name: "Assign a role" })).toBeTruthy();
});

test("ChoiceCard: a card that names the current state is pressed, and pressing it reports", () => {
  const onSelect = vi.fn();
  render(
    <ChoiceGrid label="Assign a role">
      <ChoiceCard label="Collectible" selected onSelect={onSelect} />
      <ChoiceCard label="Spinner" onSelect={() => {}} />
    </ChoiceGrid>,
  );
  expect(screen.getByRole("button", { name: /Collectible/ }).getAttribute("aria-pressed")).toBe("true");
  expect(screen.getByRole("button", { name: /Spinner/ }).getAttribute("aria-pressed")).toBe("false");
  fireEvent.click(screen.getByRole("button", { name: /Collectible/ }));
  expect(onSelect).toHaveBeenCalledTimes(1);
});

test("ChoiceCard: a disabled card says WHY in its title and does not fire", () => {
  const onSelect = vi.fn();
  render(
    <ChoiceGrid label="Assign a role">
      <ChoiceCard
        label="Collectible"
        disabled
        disabledReason="Select an object first."
        title="Spins; vanishes and scores"
        onSelect={onSelect}
      />
    </ChoiceGrid>,
  );
  const card = screen.getByRole("button", { name: /Collectible/ }) as HTMLButtonElement;
  expect(card.disabled).toBe(true);
  // `<ux_quality>` 6 — a control that cannot act says why, in words, where the pointer already is.
  // The reason REPLACES the description here rather than joining it: a `title` is one string.
  expect(card.title).toBe("Select an object first.");
  fireEvent.click(card);
  expect(onSelect).not.toHaveBeenCalled();
});
