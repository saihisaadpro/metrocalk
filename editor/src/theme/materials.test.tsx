//! The material vocabulary and its swatch (ADR-164).
//!
//! WHAT THESE ASSERT, AND WHAT THEY DELIBERATELY DO NOT. The three NUMBERS per finish are a contract
//! with the renderer and are gated by `scripts/check-material-presets.mjs`, which reads both tables —
//! restating them here would be a third copy that only agrees with the second. What is left, and what
//! nothing else can see, is the two properties the picture depends on:
//!
//!  * the lookup accepts every spelling the renderer accepts, so a stored or imported value lights up
//!    the right swatch rather than none;
//!  * the shading ORDERS itself — highlight brighter than mid-tone, mid-tone brighter than terminator,
//!    for every preset — which is what makes a circle read as a sphere. A gradient whose stops are in
//!    the wrong order renders a flat disc, and every selector assertion in the suite passes while it does.

import { expect, test } from "vitest";
import { render } from "@testing-library/react";
import {
  MATERIAL_DEFAULT,
  MATERIAL_PRESETS,
  MaterialSphere,
  materialPresetFor,
  materialSwatch,
} from "./materials";

/** `rgb(r g b)` → the perceived luminance of those three channels. */
function luma(css: string): number {
  const m = /rgb\((\d+) (\d+) (\d+)\)/.exec(css);
  expect(m, `expected an rgb() triple, got ${css}`).not.toBeNull();
  const [r, g, b] = [Number(m![1]), Number(m![2]), Number(m![3])];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

test("every accepted spelling resolves to its finish — an alias must not read as 'nothing set'", () => {
  for (const preset of MATERIAL_PRESETS) {
    expect(materialPresetFor(preset.id)?.id).toBe(preset.id);
    for (const alias of preset.aliases) {
      expect(materialPresetFor(alias)?.id, `alias ${alias}`).toBe(preset.id);
    }
    // The renderer lowercases nothing, but a projection can carry a value a human typed.
    expect(materialPresetFor(preset.id.toUpperCase())?.id).toBe(preset.id);
  }
});

test("the cleared value, an empty string and an imported handle all resolve to no finish", () => {
  expect(materialPresetFor(MATERIAL_DEFAULT)).toBeNull();
  expect(materialPresetFor("")).toBeNull();
  expect(materialPresetFor("mtkasset:9f2c11")).toBeNull();
  expect(materialPresetFor(undefined)).toBeNull();
  expect(materialPresetFor(42)).toBeNull();
});

test("no two finishes share a spelling — one value cannot select two swatches", () => {
  const seen = new Map<string, string>();
  for (const preset of MATERIAL_PRESETS) {
    for (const name of [preset.id, ...preset.aliases]) {
      expect(seen.has(name), `"${name}" is claimed by both ${seen.get(name)} and ${preset.id}`).toBe(false);
      seen.set(name, preset.id);
    }
  }
});

test("the swatch shades in order — key > body > core, for every finish", () => {
  for (const preset of MATERIAL_PRESETS) {
    const swatch = materialSwatch(preset);
    const key = luma(swatch.key);
    const body = luma(swatch.body);
    const core = luma(swatch.core);
    expect(key, `${preset.id}: the lit lobe must be brighter than the mid-tone`).toBeGreaterThan(body);
    expect(body, `${preset.id}: the mid-tone must be brighter than the terminator`).toBeGreaterThan(core);
    // The bounce is what stops a sphere reading as a disc: the rim comes back UP from the terminator.
    expect(luma(swatch.rim), `${preset.id}: the rim must lift off the terminator`).toBeGreaterThan(core);
  }
});

test("roughness decides the highlight, not the colour — a mirror concentrates it, a matte spreads it", () => {
  const chrome = MATERIAL_PRESETS.find((p) => p.id === "chrome")!;
  const rust = MATERIAL_PRESETS.find((p) => p.id === "rusty")!;
  const smooth = materialSwatch(chrome);
  const rough = materialSwatch(rust);
  expect(smooth.hotspotRadius).toBeLessThan(rough.hotspotRadius);
  expect(smooth.hotspotOpacity).toBeGreaterThan(rough.hotspotOpacity);
});

test("a metal has no diffuse, so gold's dark side is DARK — the fact that makes metals read as metal", () => {
  const gold = MATERIAL_PRESETS.find((p) => p.id === "gold")!;
  const plastic = MATERIAL_PRESETS.find((p) => p.id === "plastic")!;
  // Same lighting, opposite metalness: the dielectric's terminator keeps most of its albedo, the
  // metal's collapses. A preview that got this backwards would show six variations of one ball.
  expect(luma(materialSwatch(gold).core)).toBeLessThan(luma(materialSwatch(plastic).core));
});

test("two spheres on one page do not share a gradient id (the 'all my materials are grey' bug)", () => {
  const { container } = render(
    <>
      <MaterialSphere preset={MATERIAL_PRESETS[0]} />
      <MaterialSphere preset={MATERIAL_PRESETS[1]} />
    </>,
  );
  const ids = [...container.querySelectorAll("radialGradient")].map((g) => g.id);
  expect(ids.length).toBe(6);
  expect(new Set(ids).size).toBe(6);
  // And every reference resolves to one of them — a `url(#…)` naming nothing paints black.
  for (const shape of container.querySelectorAll("circle, ellipse")) {
    const ref = /url\(#(.+)\)/.exec(shape.getAttribute("fill") ?? "")?.[1];
    expect(ids).toContain(ref);
  }
});

test("the sphere is decorative — it never becomes a second accessible name for its tile", () => {
  const { container } = render(<MaterialSphere preset={MATERIAL_PRESETS[2]} />);
  const svg = container.querySelector("svg")!;
  expect(svg.getAttribute("aria-hidden")).toBe("true");
  expect(svg.getAttribute("data-material")).toBe("gold");
});
