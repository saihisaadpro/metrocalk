//! **The engine's PBR finish vocabulary, and the sphere that shows it** (ADR-164).
//!
//! Implement this feature in accordance with the Engine UI/UX Architecture Constitution.
//!
//! WHAT THE RENDERER ALREADY DOES. `material_preset()` in `editor-shell/src-tauri/src/main.rs` maps a
//! name written into `MeshRenderer.material` to `(base_color, metallic, roughness)`, and the instance
//! buffer carries it as a per-entity override of the asset's baked material. Six finishes, fourteen
//! accepted spellings, live since M11.2 (ADR-041). The editor's only route to any of it was a list of
//! six text buttons that spent tokens (`AiEditPanel`), so an author could read the WORD "Chrome" and had
//! no way at all to see what chrome looked like before paying for it.
//!
//! THIS FILE IS THE SECOND STATEMENT OF THAT TABLE, AND IT SAYS SO. The numbers below are the renderer's
//! numbers; they are here because a swatch has to shade itself and the shading has to be the same
//! surface the viewport will show. That makes this exactly the hazard `<test_and_ci_discipline>` 6 names
//! — one contract stated twice, in two languages, each validated alone — so it ships with the check that
//! compares the two: `editor/scripts/check-material-presets.mjs` parses the Rust match arms and asserts
//! this table name-for-name and number-for-number. Change one side and the gate fails.
//!
//! THE PREVIEW IS AN APPROXIMATION AND IT IS AN HONEST ONE. `MaterialSphere` is not a render — the real
//! pixels are the wgpu surface — it is a shaded swatch driven by the same three inputs the shader
//! receives, so the ORDER and CHARACTER of the finishes (which is warmer, which is glossier, which is
//! nearly matte) is true even though the lighting is a studio approximation rather than the scene's.

import { useId } from "react";

/** Linear RGB in `[0,1]`, the same space the renderer's preset table is written in. */
export type Rgb = readonly [number, number, number];

export interface MaterialPreset {
  /** The value written to `MeshRenderer.material` — the FIRST spelling of the renderer's match arm. */
  id: string;
  /** What a reader is shown. */
  label: string;
  /** The finish in one clause, for the tile's tooltip. Never jargon. */
  hint: string;
  /** The renderer's other accepted spellings for this same finish. An imported or hand-typed value
   *  matching one of these still lights up its tile — the picker must not claim "nothing is set" for a
   *  material the viewport is visibly showing. */
  aliases: readonly string[];
  /** Linear base colour, exactly as `material_preset()` states it. */
  baseColor: Rgb;
  /** Metalness `[0,1]` — 0 dielectric, 1 metal. */
  metallic: number;
  /** Perceptual roughness `[0,1]` — 0 mirror, 1 matte. */
  roughness: number;
}

/** The value that means "no override — use whatever this object already is".
 *
 *  `material_preset()` returns `None` for any name it does not know, and the renderer then draws the
 *  asset's own baked material. So clearing is an ordinary field write of an unrecognised word rather
 *  than a second command, and `"default"` is the word the projection already uses for it. */
export const MATERIAL_DEFAULT = "default";

/** The six finishes, in the order the picker offers them: the everyday metals first, then the two a
 *  beginner reaches for by name, then the one non-metal. */
export const MATERIAL_PRESETS: readonly MaterialPreset[] = [
  {
    id: "metal",
    label: "Metal",
    hint: "Brushed steel — bright, with a soft highlight",
    aliases: ["metallic", "steel", "iron"],
    baseColor: [0.56, 0.57, 0.58],
    metallic: 1.0,
    roughness: 0.35,
  },
  {
    id: "chrome",
    label: "Chrome",
    hint: "Polished to a mirror",
    aliases: ["mirror", "polished"],
    baseColor: [0.55, 0.56, 0.58],
    metallic: 1.0,
    roughness: 0.06,
  },
  {
    id: "gold",
    label: "Gold",
    hint: "Warm yellow metal",
    aliases: [],
    baseColor: [1.0, 0.78, 0.34],
    metallic: 1.0,
    roughness: 0.22,
  },
  {
    id: "copper",
    label: "Copper",
    hint: "Warm red metal, lightly satin",
    aliases: ["bronze"],
    baseColor: [0.95, 0.6, 0.4],
    metallic: 1.0,
    roughness: 0.3,
  },
  {
    id: "rusty",
    label: "Rust",
    hint: "Weathered metal — dark, rough, barely reflective",
    aliases: ["rust", "weathered"],
    baseColor: [0.42, 0.22, 0.12],
    metallic: 0.55,
    roughness: 0.65,
  },
  {
    id: "plastic",
    label: "Plastic",
    hint: "Matte off-white — no metal at all",
    aliases: ["matte"],
    baseColor: [0.8, 0.8, 0.82],
    metallic: 0.0,
    roughness: 0.55,
  },
];

/** The preset a stored `MeshRenderer.material` names, under any spelling the renderer accepts.
 *  `null` for the cleared state, an imported handle, or a word the renderer would ignore. */
export function materialPresetFor(value: unknown): MaterialPreset | null {
  if (typeof value !== "string" || value === "") return null;
  const name = value.toLowerCase();
  return (
    MATERIAL_PRESETS.find((preset) => preset.id === name || preset.aliases.includes(name)) ?? null
  );
}

// ── The swatch's shading ──────────────────────────────────────────────────────────────────────────

const clamp01 = (value: number) => (value < 0 ? 0 : value > 1 ? 1 : value);

/** Linear → sRGB, so a linear `0.5` reads as the mid grey a viewer expects rather than as near-black. */
function encode(channel: number): number {
  const linear = clamp01(channel);
  const srgb = linear <= 0.0031308 ? linear * 12.92 : 1.055 * linear ** (1 / 2.4) - 0.055;
  return Math.round(clamp01(srgb) * 255);
}

// ui-constitution-allow literal-ui-color: the ONE place a shaded PBR swatch becomes a CSS colour. The
// value is computed from the renderer's own base_color/metallic/roughness, so it is content — the
// surface being previewed — and not a UI colour a token could ever have expressed.
const css = (rgb: Rgb) => `rgb(${encode(rgb[0])} ${encode(rgb[1])} ${encode(rgb[2])})`;

const mix = (a: Rgb, b: Rgb, t: number): Rgb => [
  a[0] + (b[0] - a[0]) * t,
  a[1] + (b[1] - a[1]) * t,
  a[2] + (b[2] - a[2]) * t,
];

const lit = (diffuse: Rgb, spec: Rgb, kd: number, ks: number): Rgb => [
  diffuse[0] * kd + spec[0] * ks,
  diffuse[1] * kd + spec[1] * ks,
  diffuse[2] * kd + spec[2] * ks,
];

const WHITE: Rgb = [1, 1, 1];

/** The four body stops, where they sit, and the highlight — all derived from one finish.
 *
 *  THE ONE IDEA, AND THE FIRST VERSION OF THIS FUNCTION DID NOT HAVE IT. Roughness is not a brightness,
 *  it is how much of the room's own CONTRAST survives the bounce. A matte ball averages the whole
 *  studio and comes back a mid tone; a mirror returns the studio unaveraged — a blown-out softbox, a
 *  dark surround between, a bright floor along the bottom edge. Shade without that and every metal
 *  lands on the same mid grey: measured on the first capture, Metal and Chrome were indistinguishable
 *  and Plastic — a matte dielectric — came out glossier than both.
 *
 *  THE STOP POSITIONS CARRY IT AS MUCH AS THE COLOURS. A mirror's lit cap is small and falls off fast;
 *  a matte sphere's is broad and gentle. Interpolating four fixed offsets makes every finish the same
 *  shape in different colours, which is a palette, not a preview.
 *
 *  Exported so a test can assert the ORDERING that makes a swatch legible — a highlight brighter than
 *  its mid-tone, a terminator darker than both, a rim that lifts back off it — for every preset in the
 *  table, rather than a suite asserting that an `<svg>` rendered. */
export function materialSwatch(preset: MaterialPreset) {
  const gloss = 1 - preset.roughness;
  const contrast = 0.25 + 0.75 * gloss;
  /** What this surface returns from the studio at one point on it: `matte` is the averaged room, and
   *  `mirror` is what a perfect reflector would send back from that direction. */
  const env = (matte: number, mirror: number) => matte + (mirror - matte) * contrast;
  // A dielectric reflects ~4% of the light at normal incidence and a metal effectively all of it, and
  // a metal has no diffuse lobe at all — the two facts that make plastic a pale matte ball and gold a
  // dark one with a hot cap and a hot rim.
  const strength = 0.04 + 0.96 * preset.metallic;
  const diffuse = mix(preset.baseColor, [0, 0, 0], preset.metallic);
  const specular = mix(WHITE, preset.baseColor, preset.metallic);
  const tone = (kd: number, sample: number, concentration = 1) =>
    css(lit(diffuse, specular, kd, strength * sample * concentration));
  return {
    /** The key lobe, upper-left: the softbox. Its energy is SPREAD by roughness, so a rough surface's
     *  cap is dimmer as well as broader — without the concentration term rust came back pale pink. */
    key: tone(0.95, env(0.55, 1.3), 0.35 + 0.65 * gloss),
    /** The lit mid-tone. It gets DARKER with gloss, not brighter: a mirror is returning the dark part
     *  of the room here, which is why a chrome ball is mostly dark with two bright edges. */
    body: tone(0.58, env(0.42, 0.3)),
    /** The terminator — the darkest band, just before the bounce comes back up. */
    core: tone(0.14, env(0.16, 0.045)),
    /** Light off the studio floor onto the lower-right edge. Without it a sphere reads as a disc. */
    rim: tone(0.34, env(0.3, 0.78)),
    /** Where the mid-tone and the terminator sit, as gradient offsets. */
    bodyStop: 26 + 26 * preset.roughness,
    coreStop: 70 + 14 * preset.roughness,
    /** The blown-out specular dot: tight and bright when smooth, broad and faint when rough. */
    hotspot: css(mix(specular, WHITE, 0.55 * gloss)),
    /** As a fraction of the sphere's radius. */
    hotspotRadius: 0.11 + 0.34 * preset.roughness,
    hotspotOpacity: 0.1 + 0.78 * gloss,
  };
}

export interface MaterialSphereProps {
  preset: MaterialPreset;
  /** Rendered size in px. The swatch is square and scales cleanly — it is all vector. */
  size?: number;
}

/** One finish, as a lit sphere on the tile's own well.
 *
 *  A gradient needs document-unique ids or the second swatch on a page inherits the first one's stops,
 *  which is a bug that looks exactly like "all my materials are grey". `useId` supplies them; its
 *  delimiters are stripped because a `url(#…)` reference has to survive being parsed as a fragment. */
export function MaterialSphere({ preset, size = 56 }: MaterialSphereProps) {
  const raw = useId();
  const uid = `mtk-mat-${raw.replace(/[^a-zA-Z0-9_-]/g, "")}-${preset.id}`;
  const swatch = materialSwatch(preset);
  const r = 23;
  return (
    <svg
      className="mtk-swatch__sphere"
      width={size}
      height={size}
      viewBox="0 0 64 64"
      role="presentation"
      focusable="false"
      aria-hidden="true"
      data-material={preset.id}
    >
      <defs>
        {/* Off-centre so the sphere is lit from the upper left, which is where every other shadow in
            this stylesheet says the light is (`elevation` casts downward). */}
        <radialGradient id={`${uid}-body`} cx="34%" cy="27%" r="82%">
          <stop offset="0%" stopColor={swatch.key} />
          <stop offset={`${swatch.bodyStop}%`} stopColor={swatch.body} />
          <stop offset={`${swatch.coreStop}%`} stopColor={swatch.core} />
          <stop offset="100%" stopColor={swatch.rim} />
        </radialGradient>
        <radialGradient id={`${uid}-spec`}>
          <stop offset="0%" stopColor={swatch.hotspot} stopOpacity={swatch.hotspotOpacity} />
          <stop offset="100%" stopColor={swatch.hotspot} stopOpacity={0} />
        </radialGradient>
        {/* The contact shadow is tinted by the object, because it is the object's own light being
            blocked — a neutral grey under a gold ball reads as a sticker. */}
        <radialGradient id={`${uid}-cast`}>
          <stop offset="0%" stopColor={swatch.core} stopOpacity={0.34} />
          <stop offset="100%" stopColor={swatch.core} stopOpacity={0} />
        </radialGradient>
      </defs>
      <ellipse cx="32" cy="56" rx="19" ry="4.5" fill={`url(#${uid}-cast)`} />
      <circle cx="32" cy="30" r={r} fill={`url(#${uid}-body)`} />
      <ellipse
        cx={32 - r * 0.42}
        cy={30 - r * 0.46}
        rx={r * swatch.hotspotRadius * 1.9}
        ry={r * swatch.hotspotRadius * 1.55}
        fill={`url(#${uid}-spec)`}
      />
    </svg>
  );
}
