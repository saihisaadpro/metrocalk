//! The shared asset primitives (ADR-144). Two of these assert a RULE rather than a rendering, and both
//! rules exist because a tile in a 300px dock has room for about two chips: which capability a tile
//! states when it has both, and that the star is never drawn inside the button it sits next to.

import { expect, test, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { AssetChip, AssetGrid, AssetTile } from "./assets";

const base = {
  label: "Rusty Medieval Sword",
  kind: "marketplace",
  tier: "Marketplace",
  actionLabel: "Buy and add Rusty Medieval Sword for 4 tokens",
  onActivate: () => {},
};

test("the note states the CONSTRAINT — or the collection, which out-ranks it — and the tier is the footer", () => {
  const { unmount } = render(<AssetTile {...base} provides={["Renderable"]} requires={["Spatial"]} />);
  // Both are real information and there is room on the preview for one. A reader deciding whether to
  // place something needs to know what it will refuse to attach to; the full lists are in the tooltip.
  expect(screen.getByTestId("asset-note").textContent).toBe("needs Spatial");
  expect(screen.getByText("Marketplace")).toBeTruthy();
  expect(screen.getByTitle(/attaches to something providing Spatial/)).toBeTruthy();
  expect(screen.getByTitle(/brings Renderable/)).toBeTruthy();
  unmount();

  const both = render(<AssetTile {...base} provides={["Renderable"]} requires={["Spatial"]} tag="Companions (acme)" />);
  // `needs Spatial` is true of every Prop in the library; `Companions (acme)` is true of this one.
  expect(screen.getByTestId("asset-note").textContent).toBe("Companions (acme)");
  both.unmount();

  render(<AssetTile {...base} provides={["Renderable", "Lighting"]} requires={[]} />);
  expect(screen.getByTestId("asset-note").textContent).toBe("adds Renderable");
  expect(screen.getByTitle(/brings Renderable, Lighting/)).toBeTruthy();
});

test("a tile with nothing to note draws no note at all rather than an empty chip", () => {
  render(<AssetTile {...base} />);
  expect(screen.queryByTestId("asset-note")).toBeNull();
});

test("the star is a SIBLING of the tile's button, never a control drawn inside another control", () => {
  render(<AssetTile {...base} price={4} favourite onToggleFavourite={() => {}} />);
  const activate = screen.getByRole("button", { name: base.actionLabel });
  const star = screen.getByTestId("asset-favourite");
  // `shoot.mjs` R3 is about two controls sharing pixels; nesting is the DOM form of the same defect, and
  // a button inside a button is invalid markup that browsers resolve by guessing.
  expect(activate.contains(star)).toBe(false);
  expect(star.getAttribute("aria-pressed")).toBe("true");
  // Pressing the star must not be pressing the tile: they are two different decisions.
  expect(star.closest("button")).toBe(star);
});

test("a priced tile states its price before the action that would spend it", () => {
  render(<AssetTile {...base} price={4} />);
  const price = screen.getByTestId("asset-price");
  expect(price.textContent).toContain("4");
  expect(screen.getByTitle("costs 4 tokens")).toBeTruthy();
  // A tooltip is a discoverable explanation; the number itself is on the tile, before the press.
  // The price sits INSIDE the tile's own button on purpose — it is a label, and R3 is about two
  // controls sharing pixels. Being inside is what lets it use the preview's empty corner.
  expect(screen.getByRole("button", { name: base.actionLabel }).contains(price)).toBe(true);
});

test("a refusing tile explains itself instead of going quietly dark", () => {
  render(<AssetTile {...base} disabled disabledReason="Needs a Spatial component on the target" />);
  const activate = screen.getByRole("button", { name: base.actionLabel });
  expect(activate.hasAttribute("disabled")).toBe(true);
  // `<ux_quality>` 4 / `shoot.mjs` R9: a disabled control says why, in plain words.
  expect(activate.getAttribute("title")).toBe("Needs a Spatial component on the target");
});

test("a chip is a real toggle when it can be toggled and inert text when it cannot", () => {
  const onToggle = vi.fn();
  const { unmount } = render(
    <AssetChip icon="star" pressed onToggle={onToggle} data-testid="filter">
      Favourites
    </AssetChip>,
  );
  const filter = screen.getByTestId("filter");
  expect(filter.tagName).toBe("BUTTON");
  expect(filter.getAttribute("aria-pressed")).toBe("true");
  unmount();

  render(<AssetChip data-testid="tag">Companions (acme)</AssetChip>);
  const tag = screen.getByTestId("tag");
  // A tag that renders as a button is a promise of an action it does not have.
  expect(tag.tagName).toBe("SPAN");
  expect(tag.getAttribute("aria-pressed")).toBeNull();
});

test("the grid is a named group, so a screen reader hears which collection it is in", () => {
  render(
    <AssetGrid label="Characters">
      <AssetTile {...base} />
    </AssetGrid>,
  );
  expect(screen.getByRole("group", { name: "Characters" })).toBeTruthy();
});
