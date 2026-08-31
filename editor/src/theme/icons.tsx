//! **THE ONE ICON SET** (ADR-137) — every mark the editor draws, as geometry rather than as text.
//!
//! WHY THIS FILE EXISTS, IN A SENTENCE THE CAPTURES ALREADY WROTE. `primitives.tsx` had already been
//! forced to draw the five transport controls as inline SVG, because `animation-timeline-tracks.png`
//! photographed four buttons of which three were empty boxes; the same session added `RevertIcon` for the
//! same reason and `check-glyph-coverage.mjs` to stop the next one landing silently. What none of that
//! did was answer the general question: the editor was still drawing **ninety-odd other controls** with
//! characters — `▤` for Scene, `⬡` for Model, `◉` for Physics, `⌕` for search, `⌘` for relations, `▧`
//! for a box, `●` for a sphere — and thirty-five **colour emoji** arriving from the Rust catalogs. Those
//! are not a font-coverage bug; every one of them renders. They are a DESIGN bug, and a worse one:
//! `●` and `⬤` and `◍` are three different optical weights standing in for sphere, cylinder and ring in
//! one nine-button grid, and an emoji drops a full-colour pictograph into a monochrome light workbench.
//! The UI/UX Constitution names Icons and icon size as centralized tokens and forbids a subsystem
//! inventing its own styling; a character picked per call site is exactly that.
//!
//! WHY THAT IS A ROOT CAUSE AND NOT A RESTYLE. A glyph is chosen at the call site, sized by the text
//! metrics of whichever font happened to answer, coloured by inheritance and aligned on the baseline —
//! so no two of them agree and none of them can be a token. An `<Icon>` is chosen from a NAMED
//! vocabulary, sized from `control.icon`, drawn on one 24-unit grid at one stroke weight, inherits
//! `currentColor` (so every toggle/active/disabled variant keeps working), and carries `data-icon` —
//! the stable signal a test keys on, which is the one thing a character never was.
//!
//! THE VOCABULARY CROSSES THE IPC BOUNDARY AS NAMES, NOT CHARACTERS. `shape_forge.rs`,
//! `role_intent.rs`, `vfx_intent.rs`, `cinema_intent.rs` and `condition_intent.rs` each publish a
//! catalog whose entries carry an `icon`. That field is a name out of THIS set now — `"fire"`, not a
//! pictograph — so the engine states its icon vocabulary once and the editor renders it, instead of two
//! languages each holding half a picture. `scripts/check-icon-vocab.mjs` compares the two directions
//! (ADR-134's rule: a contract stated twice is validated by neither compiler).
//!
//! ADDING ONE. Put it in `ICONS` under a name that says what it MEANS, not what it looks like
//! (`gameplay`, not `flag`); draw it inside `0 0 24 24`; leave `fill` alone unless the mark is genuinely
//! solid; keep it to three elements. If an existing name already means the thing, add an ALIAS rather
//! than a second drawing — `ICON_ALIASES` is the declared list of "these two are the same picture", so
//! a near-duplicate is a decision someone recorded rather than drift nobody noticed.

import type { CSSProperties, ReactNode } from "react";
import { control } from "./tokens";

/** The grid every icon is drawn on. One number, so a new icon cannot land at a different scale. */
const VIEW_BOX = "0 0 24 24";
/** One stroke weight for the whole set — the thing that makes ninety marks read as one family. */
const STROKE_WIDTH = 1.7;

/** Solid marks. `fill`/`stroke` per element, so the set stays stroke-first and solids are deliberate. */
const solid = { fill: "currentColor", stroke: "none" } as const;

// ── The set ───────────────────────────────────────────────────────────────────────────────────────
// Grouped by the surface that asks for them. Each entry is the SVG children; the wrapper below supplies
// the viewBox, the stroke and the size.

const ICONS = {
  // Engines rail — the nine sub-engines (EngineRail.tsx / BottomDock.tsx).
  scene: (
    <>
      <path d="M12 3.2 3.6 7.4 12 11.6l8.4-4.2z" />
      <path d="M3.6 12.2 12 16.4l8.4-4.2" />
      <path d="M3.6 16.6 12 20.8l8.4-4.2" />
    </>
  ),
  build: (
    <>
      <rect x="3.5" y="3.5" width="17" height="17" rx="4" />
      <path d="M12 8.4v7.2M8.4 12h7.2" />
    </>
  ),
  terrain: <path d="M2.6 18.8 9 9.2l3.9 5.6 2.7-3.6 5.8 7.6z" />,
  model: (
    <>
      <path d="M12 3.3 20 7.6v8.8L12 20.7 4 16.4V7.6z" />
      <path d="M4 7.6 12 12l8-4.4M12 12v8.7" />
    </>
  ),
  import: (
    <>
      <path d="M12 3.4v10.4" />
      <path d="M7.8 9.8 12 14l4.2-4.2" />
      <path d="M4.4 16.8v2.4a1.4 1.4 0 0 0 1.4 1.4h12.4a1.4 1.4 0 0 0 1.4-1.4v-2.4" />
    </>
  ),
  animate: (
    <>
      <path d="M3.4 12h4.2M16.4 12h4.2" />
      <path d="M12 7.6 16.4 12 12 16.4 7.6 12z" />
    </>
  ),
  physics: (
    <>
      <circle cx="12" cy="12" r="2.6" {...solid} />
      <ellipse cx="12" cy="12" rx="9.2" ry="4.1" transform="rotate(-28 12 12)" />
    </>
  ),
  logic: (
    <>
      <circle cx="6.2" cy="6.6" r="2.4" />
      <circle cx="6.2" cy="17.4" r="2.4" />
      <circle cx="17.8" cy="12" r="2.4" />
      <path d="m8.5 7.7 7 3.2M8.5 16.3l7-3.2" />
    </>
  ),
  gameplay: (
    <>
      <path d="M6 20.6V4" />
      <path d="M6 4.6h10.8l-2.3 3.7 2.3 3.7H6z" />
    </>
  ),

  // Header + shell chrome.
  menu: <path d="M4 7h16M4 12h16M4 17h16" />,
  settings: (
    <>
      <circle cx="12" cy="12" r="3.2" />
      <circle cx="12" cy="12" r="6.9" />
      <path d="M12 2.6v2.3M12 19.1v2.3M20.5 7.2l-2 1.2M5.5 15.6l-2 1.2M20.5 16.8l-2-1.2M5.5 8.4l-2-1.2" />
    </>
  ),
  search: (
    <>
      <circle cx="10.8" cy="10.8" r="6.2" />
      <path d="m15.4 15.4 4.4 4.4" />
    </>
  ),
  undo: (
    <>
      <path d="M4 9.6h9.6a5.4 5.4 0 1 1 0 10.8H8.4" />
      <path d="M7.6 5.2 3.2 9.6l4.4 4.4" />
    </>
  ),
  redo: (
    <>
      <path d="M20 9.6h-9.6a5.4 5.4 0 1 0 0 10.8h5.2" />
      <path d="m16.4 5.2 4.4 4.4-4.4 4.4" />
    </>
  ),
  play: <path d="M7.8 5 18.8 12 7.8 19z" fill="currentColor" />,
  pause: (
    <>
      <rect x="7.2" y="5.2" width="3.8" height="13.6" rx="1.3" {...solid} />
      <rect x="13" y="5.2" width="3.8" height="13.6" rx="1.3" {...solid} />
    </>
  ),
  stop: <rect x="6.2" y="6.2" width="11.6" height="11.6" rx="2.4" {...solid} />,
  prev: (
    <>
      <rect x="4.8" y="5.4" width="3" height="13.2" rx="1.1" {...solid} />
      <path d="M19.2 5.4v13.2L9.4 12z" fill="currentColor" />
    </>
  ),
  next: (
    <>
      <rect x="16.2" y="5.4" width="3" height="13.2" rx="1.1" {...solid} />
      <path d="M4.8 5.4v13.2L14.6 12z" fill="currentColor" />
    </>
  ),
  command: (
    <>
      <rect x="8.6" y="8.6" width="6.8" height="6.8" rx="1.2" />
      <path d="M8.6 8.6H6.9a2.3 2.3 0 1 1 2.3-2.3v2.3zM15.4 8.6h1.7a2.3 2.3 0 1 0-2.3-2.3v2.3zM8.6 15.4H6.9a2.3 2.3 0 1 0 2.3 2.3v-2.3zM15.4 15.4h1.7a2.3 2.3 0 1 1-2.3 2.3v-2.3z" />
    </>
  ),
  tokens: (
    <>
      <circle cx="12" cy="12" r="8.2" />
      <path d="M12 8.2 15.8 12 12 15.8 8.2 12z" />
    </>
  ),
  close: <path d="M6.6 6.6 17.4 17.4M17.4 6.6 6.6 17.4" />,
  /** Back to a declared default — the inspector row's inline reset (ADR-136). Counter-clockwise on
   *  purpose: `rotate` turns forwards and this one puts a value back. */
  revert: (
    <>
      <path d="M3.8 12a8.2 8.2 0 1 0 2.4-5.8" />
      <path d="M3.4 4.2v4.8h4.8" />
    </>
  ),
  plus: <path d="M12 5.2v13.6M5.2 12h13.6" />,
  minus: <path d="M5.2 12h13.6" />,
  "arrow-right": <path d="M4.2 12h15.6M13.8 6l6 6-6 6" />,
  "arrow-left": <path d="M19.8 12H4.2M10.2 6l-6 6 6 6" />,
  "arrow-up": <path d="M12 19.8V4.2M6 10.2l6-6 6 6" />,
  "arrow-down": <path d="M12 4.2v15.6M6 13.8l6 6 6-6" />,
  swap: <path d="M4.2 8.6h15.6M15.8 4.6l4 4-4 4M19.8 15.4H4.2M8.2 11.4l-4 4 4 4" />,
  "chevron-down": <path d="m6.8 9.6 5.2 5.2 5.2-5.2" />,
  "chevron-up": <path d="m6.8 14.4 5.2-5.2 5.2 5.2" />,
  "chevron-left": <path d="m14.4 6.8-5.2 5.2 5.2 5.2" />,
  "chevron-right": <path d="m9.6 6.8 5.2 5.2-5.2 5.2" />,

  // Docks + panel chrome.
  properties: (
    <>
      <path d="M4.4 7.6h9M18 7.6h1.6M4.4 16.4h4.4M13.4 16.4h6.2" />
      <circle cx="15.4" cy="7.6" r="2.2" />
      <circle cx="10.8" cy="16.4" r="2.2" />
    </>
  ),
  relations: (
    <>
      <circle cx="17.4" cy="6.4" r="2.4" />
      <circle cx="6.4" cy="12" r="2.4" />
      <circle cx="17.4" cy="17.6" r="2.4" />
      <path d="m8.6 10.9 6.6-3.3M8.6 13.1l6.6 3.3" />
    </>
  ),
  pin: (
    <>
      <path d="M9.2 3.8h5.6l-.8 5 3 3.2H7l3-3.2z" />
      <path d="M12 12v8.2" />
    </>
  ),
  problems: (
    <>
      <circle cx="12" cy="12" r="8.2" />
      <path d="M12 7.6v5" />
      <circle cx="12" cy="16" r="1" {...solid} />
    </>
  ),
  runtime: <path d="M3.4 12h3.8l2.6-6.6 4.2 13.2 2.6-6.6h4" />,
  assets: (
    <>
      <rect x="4" y="4" width="7" height="7" rx="1.8" />
      <rect x="13" y="4" width="7" height="7" rx="1.8" />
      <rect x="4" y="13" width="7" height="7" rx="1.8" />
      <rect x="13" y="13" width="7" height="7" rx="1.8" />
    </>
  ),

  // Viewport tools.
  cursor: <path d="M5.4 3.6 19.2 9.6l-5.9 1.7-1.7 5.9z" />,
  move: (
    <>
      <path d="M12 3.6v16.8M3.6 12h16.8" />
      <path d="m9.4 6.2 2.6-2.6 2.6 2.6M9.4 17.8l2.6 2.6 2.6-2.6M6.2 9.4 3.6 12l2.6 2.6M17.8 9.4l2.6 2.6-2.6 2.6" />
    </>
  ),
  rotate: (
    <>
      <path d="M20.2 12a8.2 8.2 0 1 1-2.4-5.8" />
      <path d="M20.6 4.2v4.8h-4.8" />
    </>
  ),
  scale: (
    <>
      <rect x="3.4" y="3.4" width="6.2" height="6.2" rx="1.4" />
      <rect x="11.6" y="11.6" width="9" height="9" rx="2" />
      <path d="m9.8 9.8 1.6 1.6" />
    </>
  ),
  measure: (
    <>
      <path d="m3.6 15.2 11.6-11.6 5.2 5.2L8.8 20.4z" />
      <path d="m7.2 13 1.8 1.8M10.2 10 12 11.8M13.2 7l1.8 1.8" />
    </>
  ),
  snap: (
    <>
      <path d="M6.2 4.4v7.6a5.8 5.8 0 0 0 11.6 0V4.4h-3.8v7.6a2 2 0 0 1-4 0V4.4z" />
      <path d="M6.2 8.4H10M14 8.4h3.8" />
    </>
  ),
  camera: (
    <>
      <rect x="2.6" y="7.4" width="13" height="9.2" rx="2" />
      <path d="m15.6 13 5.8 3.4V7.6L15.6 11z" />
    </>
  ),
  view: (
    <>
      <path d="M2.6 12S6 5.9 12 5.9 21.4 12 21.4 12 18 18.1 12 18.1 2.6 12 2.6 12z" />
      <circle cx="12" cy="12" r="2.9" />
    </>
  ),
  grid: <path d="M3.6 9.2h16.8M3.6 14.8h16.8M9.2 3.6v16.8M14.8 3.6v16.8" />,
  // ADR-193 — a delivery frame inside the stage that is the wrong shape for it. The outer rectangle
  // is the viewport, the inner one is the film: the icon IS the thing the control draws.
  frame: (
    <>
      <rect x="2.6" y="4.2" width="18.8" height="15.6" rx="2" />
      <path d="M2.6 8.6h18.8M2.6 15.4h18.8" />
    </>
  ),

  // Asset Lab stages — one mark per stage of the repair/optimise pipeline (they were the LETTERS
  // `I R O UV B V E`, which is a legend, not an icon set).
  repair: (
    <>
      <path d="M17.4 3.4a4.6 4.6 0 0 0-4.3 6.2L3.8 18.9a1.8 1.8 0 0 0 2.5 2.5l9.3-9.3a4.6 4.6 0 0 0 5.6-6z" />
      <path d="m14.6 9.4 4.4-4.4" />
    </>
  ),
  optimize: (
    <>
      <path d="M4.4 18.6a8.6 8.6 0 1 1 15.2 0" />
      <path d="m12 15.6 3.8-5" />
    </>
  ),
  bake: (
    <>
      <path d="M12 2.8 19.2 7 12 11.2 4.8 7z" />
      <path d="M12 13.4v5M9.4 15.8l2.6 2.6 2.6-2.6" />
      <path d="M3.6 21h16.8" />
    </>
  ),
  validate: (
    <>
      <path d="M12 3.2 19.8 6v6c0 4.6-3.2 7.6-7.8 8.8C7.4 19.6 4.2 16.6 4.2 12V6z" />
      <path d="m8.8 11.8 2.4 2.4 4.2-4.6" />
    </>
  ),
  export: (
    <>
      <path d="M12 15.4V4.4" />
      <path d="m7.8 8.6 4.2-4.2 4.2 4.2" />
      <path d="M4.4 16.8v2.4a1.4 1.4 0 0 0 1.4 1.4h12.4a1.4 1.4 0 0 0 1.4-1.4v-2.4" />
    </>
  ),

  // Shape catalog (shape_forge.rs) — one isometric line drawing per primitive, at one weight, which is
  // precisely what three different filled circles could never be.
  sphere: (
    <>
      <circle cx="12" cy="12" r="8.2" />
      <ellipse cx="12" cy="12" rx="8.2" ry="3.3" />
    </>
  ),
  cylinder: (
    <>
      <ellipse cx="12" cy="6.6" rx="6.6" ry="2.8" />
      <path d="M5.4 6.6v10.8a6.6 2.8 0 0 0 13.2 0V6.6" />
    </>
  ),
  cone: (
    <>
      <path d="M5.6 17.2 12 3.6l6.4 13.6" />
      <ellipse cx="12" cy="17.2" rx="6.4" ry="2.7" />
    </>
  ),
  // FLAT, not isometric, and against the grain of its neighbours on purpose: an ellipse inside an
  // ellipse IS an eye at 20 px, which is what `view` is, and "Ring" sitting in a nine-button shape grid
  // cannot afford to be read as an eye. An annulus is unmistakable at any size.
  torus: (
    <>
      <circle cx="12" cy="12" r="8.4" />
      <circle cx="12" cy="12" r="3.6" />
    </>
  ),
  capsule: <rect x="6.4" y="2.8" width="11.2" height="18.4" rx="5.6" />,
  wedge: (
    <>
      <path d="M3.6 18.6h16.8L3.6 7.8z" />
      <path d="M3.6 12.6h8.8" />
    </>
  ),
  prism: (
    <>
      <path d="M8.6 3.2h6.8l3.4 3.6-3.4 3.6H8.6L5.2 6.8z" />
      <path d="M5.2 6.8v10.4l3.4 3.6h6.8l3.4-3.6V6.8M8.6 10.4v10.4M15.4 10.4v10.4" />
    </>
  ),
  tube: (
    <>
      <ellipse cx="12" cy="6.6" rx="6.6" ry="2.8" />
      <ellipse cx="12" cy="6.6" rx="2.9" ry="1.2" />
      <path d="M5.4 6.6v10.8a6.6 2.8 0 0 0 13.2 0V6.6" />
    </>
  ),

  // Shape Studio operations.
  extrude: (
    <>
      <path d="M4.4 20h15.2" />
      <path d="M8.4 11.6h7.2v5.2H8.4z" />
      <path d="M12 8.8V3.4M9.2 6.2 12 3.4l2.8 2.8" />
    </>
  ),
  revolve: (
    <>
      <path d="M12 3.4v17.2" />
      <ellipse cx="12" cy="12" rx="7.4" ry="4.2" />
    </>
  ),
  union: (
    <>
      <rect x="3.4" y="3.4" width="10.4" height="10.4" rx="2" fill="currentColor" opacity="0.22" stroke="none" />
      <rect x="10.2" y="10.2" width="10.4" height="10.4" rx="2" fill="currentColor" opacity="0.22" stroke="none" />
      <rect x="3.4" y="3.4" width="10.4" height="10.4" rx="2" />
      <rect x="10.2" y="10.2" width="10.4" height="10.4" rx="2" />
    </>
  ),
  subtract: (
    <>
      <rect x="3.4" y="3.4" width="10.4" height="10.4" rx="2" />
      <rect x="10.2" y="10.2" width="10.4" height="10.4" rx="2" strokeDasharray="2.6 2.4" />
    </>
  ),
  intersect: (
    <>
      <rect x="10.2" y="10.2" width="3.6" height="3.6" fill="currentColor" stroke="none" />
      <rect x="3.4" y="3.4" width="10.4" height="10.4" rx="2" />
      <rect x="10.2" y="10.2" width="10.4" height="10.4" rx="2" />
    </>
  ),
  meld: (
    <>
      <path d="M3.6 6.8h3.6a4 4 0 0 1 3.4 1.9l1.6 2.6a4 4 0 0 0 3.4 1.9h3.4" />
      <path d="M3.6 17.2h3.6a4 4 0 0 0 3.4-1.9l1.6-2.6a4 4 0 0 1 3.4-1.9h3.4" />
      <path d="m17 9.4 2.8 2.8-2.8 2.8" />
    </>
  ),
  pipe: (
    <>
      <path d="M4.4 8.4h5.2a3 3 0 0 1 3 3v1.2a3 3 0 0 0 3 3h4" />
      <circle cx="4.4" cy="8.4" r="1.6" {...solid} />
      <circle cx="19.6" cy="15.6" r="1.6" {...solid} />
    </>
  ),

  // Entity / asset type marks (the `TypeIcon` kinds).
  mesh: (
    <>
      <path d="M12 3.4 20.6 12 12 20.6 3.4 12z" />
      <path d="M3.4 12h17.2M12 3.4v17.2" />
    </>
  ),
  group: (
    <path d="M3.6 6.4a1.6 1.6 0 0 1 1.6-1.6h3.4l2 2.4h7.8a1.6 1.6 0 0 1 1.6 1.6v8.8a1.6 1.6 0 0 1-1.6 1.6H5.2a1.6 1.6 0 0 1-1.6-1.6z" />
  ),
  light: (
    <>
      <path d="M9.4 16.8a5.8 5.8 0 1 1 5.2 0v2.4a1.2 1.2 0 0 1-1.2 1.2h-2.8a1.2 1.2 0 0 1-1.2-1.2z" />
      <path d="M9.6 17.6h4.8" />
    </>
  ),
  requirer: <path d="M12 3.6 20.4 12 12 20.4 3.6 12z" strokeDasharray="3 2.4" />,
  character: (
    <>
      <circle cx="12" cy="7.6" r="3.4" />
      <path d="M4.8 20.4a7.2 7.2 0 0 1 14.4 0" />
    </>
  ),
  rule: (
    <path d="M9.4 3.8H8.2a2.4 2.4 0 0 0-2.4 2.4v3.4A2.4 2.4 0 0 1 3.4 12a2.4 2.4 0 0 1 2.4 2.4v3.4a2.4 2.4 0 0 0 2.4 2.4h1.2M14.6 3.8h1.2a2.4 2.4 0 0 1 2.4 2.4v3.4A2.4 2.4 0 0 0 20.6 12a2.4 2.4 0 0 0-2.4 2.4v3.4a2.4 2.4 0 0 1-2.4 2.4h-1.2" />
  ),
  audio: <path d="M4 10v4M8 6.6v10.8M12 4.4v15.2M16 7.8v8.4M20 10.4v3.2" />,
  // A TOGGLE NEEDS BOTH OF ITS STATES DRAWN. The timeline's mute and lock pair had been reaching for
  // whatever was nearest — a waveform against a dashed ring, a rounded square against a no-entry sign —
  // and `animation-timeline-tracks.png` shows exactly what that reads as: a "gear" on the muted row and
  // a blank box on every unlocked one. The off state of a toggle is a drawing, not the absence of one.
  "sound-on": (
    <>
      <path d="M11.4 4.4 6.6 8.6H3.4v6.8h3.2l4.8 4.2z" />
      <path d="M15.2 9.2a4 4 0 0 1 0 5.6M18 6.4a8 8 0 0 1 0 11.2" />
    </>
  ),
  "sound-off": (
    <>
      <path d="M11.4 4.4 6.6 8.6H3.4v6.8h3.2l4.8 4.2z" />
      <path d="m15.4 9.6 5.2 4.8M20.6 9.6l-5.2 4.8" />
    </>
  ),
  lock: (
    <>
      <rect x="4.6" y="10.4" width="14.8" height="9.8" rx="2.2" />
      <path d="M8 10.4V7.8a4 4 0 0 1 8 0v2.6" />
    </>
  ),
  unlock: (
    <>
      <rect x="4.6" y="10.4" width="14.8" height="9.8" rx="2.2" />
      <path d="M8 10.4V7.8a4 4 0 0 1 7.5-1.9" />
    </>
  ),
  marketplace: (
    <>
      <path d="m11.4 3.6-7.8 7.8a1.8 1.8 0 0 0 0 2.6l6.4 6.4a1.8 1.8 0 0 0 2.6 0l7.8-7.8V3.6z" />
      <circle cx="16.4" cy="7.6" r="1.5" />
    </>
  ),
  imported: (
    <>
      <path d="M13.4 3.6H7.2a1.8 1.8 0 0 0-1.8 1.8v13.2a1.8 1.8 0 0 0 1.8 1.8h9.6a1.8 1.8 0 0 0 1.8-1.8V8.6z" />
      <path d="M13.4 3.6v5h5" />
    </>
  ),
  local: (
    <>
      <rect x="3.4" y="5.4" width="17.2" height="13.2" rx="2.4" />
      <path d="M3.4 12h17.2" />
      <circle cx="7.4" cy="15.4" r="1" {...solid} />
    </>
  ),
  shape: <rect x="4.4" y="4.4" width="15.2" height="15.2" rx="3.2" />,

  // Status marks.
  check: <path d="m5.4 12.6 4.4 4.4 8.8-9.8" />,
  warning: (
    <>
      <path d="M12 4.2 21.2 19.4H2.8z" />
      <path d="M12 10v4" />
      <circle cx="12" cy="16.8" r="1" {...solid} />
    </>
  ),
  blocked: (
    <>
      <circle cx="12" cy="12" r="8.2" />
      <path d="m6.2 6.2 11.6 11.6" />
    </>
  ),
  info: (
    <>
      <circle cx="12" cy="12" r="8.2" />
      <path d="M12 11.2v5" />
      <circle cx="12" cy="7.9" r="1" {...solid} />
    </>
  ),
  error: (
    <>
      <circle cx="12" cy="12" r="8.2" />
      <path d="m8.8 8.8 6.4 6.4M15.2 8.8l-6.4 6.4" />
    </>
  ),
  link: (
    <>
      <path d="M10.2 13.8a3.6 3.6 0 0 0 5.4.4l2.6-2.6a3.6 3.6 0 0 0-5.1-5.1L11.6 8" />
      <path d="M13.8 10.2a3.6 3.6 0 0 0-5.4-.4l-2.6 2.6a3.6 3.6 0 0 0 5.1 5.1L12.4 16" />
    </>
  ),
  star: <path d="m12 3.4 2.7 5.5 6.1.9-4.4 4.3 1 6.1-5.4-2.9-5.4 2.9 1-6.1-4.4-4.3 6.1-.9z" fill="currentColor" />,
  "star-outline": <path d="m12 3.4 2.7 5.5 6.1.9-4.4 4.3 1 6.1-5.4-2.9-5.4 2.9 1-6.1-4.4-4.3 6.1-.9z" />,
  heart: <path d="M12 20.4S3.6 15.2 3.6 9.8a4.6 4.6 0 0 1 8.4-2.6 4.6 4.6 0 0 1 8.4 2.6c0 5.4-8.4 10.6-8.4 10.6z" fill="currentColor" />,
  trophy: (
    <>
      <path d="M7.6 4.4h8.8v3.8a4.4 4.4 0 0 1-8.8 0z" />
      <path d="M7.6 5.6H4.8a2.8 2.8 0 0 0 2.8 3.6M16.4 5.6h2.8a2.8 2.8 0 0 1-2.8 3.6" />
      <path d="M12 12.6v3.6M8.6 19.8h6.8" />
    </>
  ),
  diamond: <path d="M12 3.6 20.4 12 12 20.4 3.6 12z" fill="currentColor" />,
  "diamond-outline": <path d="M12 3.6 20.4 12 12 20.4 3.6 12z" />,
  sparkle: (
    <>
      <path d="M11.4 3.6 13.2 9l5.4 1.8-5.4 1.8-1.8 5.4-1.8-5.4L4.2 10.8 9.6 9z" />
      <path d="M18.4 15.4v3.4M20.1 17.1h-3.4" />
    </>
  ),
  sword: (
    <>
      <path d="m20.4 3.6-9.6 9.6M20.4 3.6h-3.8M20.4 3.6v3.8" />
      <path d="m6.4 13.4 4.2 4.2-2.8 2.8-4.2-4.2z" />
      <path d="m8.6 15.6 3.8-3.8" />
    </>
  ),

  // Gameplay roles (role_intent.rs).
  gem: (
    <>
      <path d="M8.4 3.6h7.2l3.8 5.2L12 20.4 4.6 8.8z" />
      <path d="M4.6 8.8h14.8M8.4 3.6 12 20.4l3.6-16.8" />
    </>
  ),
  wall: (
    <>
      <rect x="3.4" y="5.4" width="17.2" height="13.2" rx="1.8" />
      <path d="M3.4 12h17.2M9.6 5.4V12M14.4 12v6.6" />
    </>
  ),
  prop: (
    <>
      <rect x="8.2" y="11" width="9.6" height="9" rx="1.6" transform="rotate(-10 13 15.5)" />
      <path d="M4.2 5.6v3.6M7.4 3.8v3.6" />
    </>
  ),
  skull: (
    <>
      <path d="M12 3.4a7.4 7.4 0 0 0-4.4 13.4v1.8a1.4 1.4 0 0 0 1.4 1.4h6a1.4 1.4 0 0 0 1.4-1.4v-1.8A7.4 7.4 0 0 0 12 3.4z" />
      <circle cx="9.4" cy="11.4" r="1.7" />
      <circle cx="14.6" cy="11.4" r="1.7" />
    </>
  ),
  waypoint: (
    <>
      <path d="M12 20.8s6.2-5.8 6.2-10.2a6.2 6.2 0 1 0-12.4 0c0 4.4 6.2 10.2 6.2 10.2z" />
      <circle cx="12" cy="10.4" r="2.4" />
    </>
  ),
  hourglass: (
    <>
      <path d="M7 3.8h10M7 20.2h10" />
      <path d="M7.8 3.8v3.1L12 11l4.2-4.1V3.8M7.8 20.2v-3.1L12 13l4.2 4.1v3.1" />
    </>
  ),
  bolt: <path d="M13.6 2.8 5.6 13.6h5.2l-.6 7.6 8.2-11h-5.4z" />,
  player: (
    <>
      <rect x="2.8" y="7.4" width="18.4" height="9.2" rx="4.6" />
      <path d="M7.6 10.6v3.2M6 12.2h3.2" />
      <circle cx="15.6" cy="11.4" r="1.1" {...solid} />
      <circle cx="17.8" cy="13.4" r="1.1" {...solid} />
    </>
  ),

  // VFX catalog (vfx_intent.rs).
  fire: (
    <path d="M12 20.6c3.5 0 6.2-2.5 6.2-5.8 0-4.6-4.6-5.4-3.6-11.4-3.2 1.4-5.4 4.6-5.4 7.4 0 1.6-1 2.2-1.8 1.2-.6-.8-.8-1.8-.8-2.6-1.2 1.4-1.8 3.2-1.8 5.4 0 3.3 2.9 5.8 7.2 5.8z" />
  ),
  smoke: (
    <>
      <path d="M6.8 16.4h10a3.4 3.4 0 0 0 .4-6.8 5 5 0 0 0-9.4-1.4 3.6 3.6 0 0 0-1 8.2z" />
      <path d="M8.2 19.8h7.6" />
    </>
  ),
  explosion: (
    <path d="m12 2.8 2.1 4.6 4.8-1.6-1.6 4.8 4.6 2.1-4.6 2.1 1.6 4.8-4.8-1.6L12 21.2l-2.1-4.6-4.8 1.6 1.6-4.8-4.6-2.1 4.6-2.1-1.6-4.8 4.8 1.6z" />
  ),
  sparks: (
    <>
      <path d="M12.8 3.4 8 11h3.4l-.6 5.4L15.6 9h-3.4z" />
      <path d="M4.2 6 5.8 7.6M19.8 16.4 18.2 14.8M4.8 17.4l1.8-1.6" />
    </>
  ),
  pickup: (
    <>
      <path d="M12 20.4v-10" />
      <path d="m8.2 13.8 3.8-3.8 3.8 3.8" />
      <path d="M6.2 5.4 7 7.2M17.8 5.4 17 7.2M12 3.2v2" />
    </>
  ),
  fountain: (
    <>
      <path d="M12 20.4v-9.2" />
      <path d="M12 11.2c0-3.4 2.6-6.2 6-6.2M12 11.2c0-3.4-2.6-6.2-6-6.2" />
      <path d="M5.2 20.4h13.6" />
    </>
  ),
  dust: (
    <>
      <path d="M4.4 16.4h8M7 19.6h9M9.4 13.2h9.2" />
      <circle cx="18.6" cy="16.4" r="1.1" {...solid} />
      <circle cx="5.8" cy="13.2" r="1.1" {...solid} />
    </>
  ),
  aura: (
    <>
      <circle cx="12" cy="12" r="3.6" />
      <circle cx="12" cy="12" r="7.6" strokeDasharray="2.6 2.8" />
    </>
  ),
  steam: <path d="M7.6 20.4c0-3 2.6-3 2.6-6s-2.6-3-2.6-6M14.4 20.4c0-3 2.6-3 2.6-6s-2.6-3-2.6-6" />,
  embers: (
    <>
      <circle cx="8.4" cy="15.4" r="2.4" {...solid} />
      <circle cx="15.6" cy="10.6" r="1.7" {...solid} />
      <circle cx="14" cy="17.8" r="1.2" {...solid} />
      <path d="M6.2 6.4 7.8 8M17.8 5.2l-1.4 1.6" />
    </>
  ),
  rain: (
    <>
      <path d="M7 15h10a3.8 3.8 0 0 0 .4-7.6 5.4 5.4 0 0 0-10.2-1.4A3.9 3.9 0 0 0 7 15z" />
      <path d="m9 18-1 2.6M13 18l-1 2.6M17 18l-1 2.6" />
    </>
  ),
  snow: <path d="M12 3.4v17.2M4.6 7.8l14.8 8.4M19.4 7.8 4.6 16.2" />,
  splash: <path d="M12 3.6s5.6 6.4 5.6 9.8a5.6 5.6 0 1 1-11.2 0C6.4 10 12 3.6 12 3.6z" />,
  shockwave: (
    <>
      <circle cx="12" cy="12" r="2.4" {...solid} />
      <circle cx="12" cy="12" r="6" />
      <circle cx="12" cy="12" r="9.4" strokeDasharray="2.4 2.8" />
    </>
  ),
  confetti: (
    <>
      <path d="M4.4 19.6 9.8 6.4l7.8 7.8z" />
      <path d="m14.6 4.6 1.4 1.4M19 5.8l-1.4 1.4M19.6 10.4 18 11M13.4 9.4l1.6.6" />
    </>
  ),
  bubbles: (
    <>
      <circle cx="9" cy="14.6" r="4.4" />
      <circle cx="16.4" cy="9" r="2.8" />
      <circle cx="16.8" cy="17" r="1.8" />
    </>
  ),
  portal: <path d="M12 12a2 2 0 1 1 2 2 4 4 0 1 1-4-4 6 6 0 1 1 6 6" />,

  // Cinema catalog (cinema_intent.rs) — a shot is a camera MOVE, so each mark draws the move.
  sunrise: (
    <>
      <circle cx="12" cy="14.6" r="3.8" />
      <path d="M2.8 18.6h18.4M12 4.4v2.6M5.6 8l1.8 1.8M18.4 8l-1.8 1.8" />
    </>
  ),
  clapper: (
    <>
      <path d="M3.4 9.4h17.2v9.4a1.6 1.6 0 0 1-1.6 1.6H5a1.6 1.6 0 0 1-1.6-1.6z" />
      <path d="m3.8 9.4 1.4-4 15.4 1.7-.6 2.3" />
      <path d="m8.6 9.4.9-3.6M13.6 9.4l.9-3.1" />
    </>
  ),
  "zoom-in": (
    <>
      <circle cx="10.8" cy="10.8" r="6.2" />
      <path d="m15.4 15.4 4.4 4.4M8.4 10.8h4.8M10.8 8.4v4.8" />
    </>
  ),
  crane: (
    <>
      <path d="M4.6 20.4V4.6h11" />
      <path d="m4.6 8 8-3.4" />
      <path d="M15.6 4.6v4.4" />
      <rect x="13.4" y="9" width="4.4" height="3.4" rx="0.8" />
    </>
  ),
  "look-up": (
    <>
      <path d="M12 3.6v10.8" />
      <path d="m7.4 8.2 4.6-4.6 4.6 4.6" />
      <path d="M4.4 20.4h15.2" />
    </>
  ),
  "look-down": (
    <>
      <path d="M12 20.4V9.6" />
      <path d="m7.4 15.8 4.6 4.6 4.6-4.6" />
      <path d="M4.4 3.6h15.2" />
    </>
  ),
  overshoulder: (
    <>
      <circle cx="7.4" cy="8.4" r="3" />
      <path d="M2.6 19.6a4.8 4.8 0 0 1 9.6 0" />
      <rect x="12.8" y="5.6" width="8.6" height="8.6" rx="1.8" />
    </>
  ),
  parachute: (
    <>
      <path d="M4.2 11.2a7.8 7.8 0 0 1 15.6 0z" />
      <path d="M4.2 11.2 12 20.4l7.8-9.2" />
      <path d="M12 11.2v9.2" />
    </>
  ),
  confront: (
    <>
      <circle cx="7" cy="9" r="2.8" />
      <circle cx="17" cy="9" r="2.8" />
      <path d="M2.8 19.6a4.4 4.4 0 0 1 8.4 0M12.8 19.6a4.4 4.4 0 0 1 8.4 0" />
    </>
  ),
  detail: (
    <>
      <circle cx="10.6" cy="10.6" r="6" />
      <path d="m15.2 15.2 4.6 4.6" />
      <rect x="8.4" y="8.4" width="4.4" height="4.4" rx="1" />
    </>
  ),
  pullback: (
    <>
      <path d="M4 10V4h6M20 14v6h-6" />
      <path d="m4 4 6.4 6.4M20 20l-6.4-6.4" />
    </>
  ),
  sweep: (
    <>
      <path d="M3.6 16.4c4-6.4 12.8-6.4 16.8 0" />
      <path d="m17.4 13.2 3 3.2-3 2.8" />
    </>
  ),
} as const;

/** Every name the set can draw. */
export type IconName = keyof typeof ICONS;

/** **Declared duplicates.** Two catalog entries that genuinely mean the same picture point at ONE
 *  drawing, on purpose and in writing — a spinner IS a rotation, a companion IS the heart, a poison
 *  cloud IS the skull the VFX catalog already used. Without this list the set grows a second
 *  near-identical mark every time a catalog does, which is how an icon family stops being one. */
const ICON_ALIASES = {
  // Gameplay roles → their mark.
  collectible: "gem",
  solid: "wall",
  spinner: "rotate",
  companion: "heart",
  enemy: "skull",
  vanishing: "hourglass",
  hazard: "bolt",
  // Conditions → their mark.
  score_at_least: "star",
  score_under: "star-outline",
  still_active: "diamond",
  touched_by_player: "player",
  touched_by_companion: "heart",
  other_gone: "sparkle",
  other_still_there: "diamond-outline",
  // VFX → their mark.
  poison: "skull",
  // Cinema shots → their mark.
  establish: "sunrise",
  hero: "clapper",
  closeup: "zoom-in",
  orbit: "rotate", // a shot that circles the subject IS the circular arrow
  reveal: "crane",
  looming: "look-up",
  vista: "terrain",
  birdseye: "look-down",
  dropin: "parachute",
  // Generic names the panels ask for.
  generated: "sparkle",
  default: "shape",
  collapse: "chevron-left",
  select: "cursor",
  box: "model", // the same drawing, declared rather than copied
  inspect: "detail",
  uv: "grid",
  expand: "chevron-right",
} as const satisfies Record<string, IconName>;

/** A name the UI may ask for: a drawing, or a declared alias for one. */
export type IconToken = IconName | keyof typeof ICON_ALIASES;

/** Resolve a token to the drawing it names. An unknown token resolves to `null`, never to a guess —
 *  a caller that wants a substitute says which one (`Icon`'s `fallback`). */
export function resolveIcon(token: string): IconName | null {
  if (token in ICONS) return token as IconName;
  const alias = (ICON_ALIASES as Record<string, IconName>)[token];
  return alias ?? null;
}

/** Every token the set answers to — drawings and aliases. `check-icon-vocab.mjs` reads this. */
export function iconTokens(): string[] {
  return [...Object.keys(ICONS), ...Object.keys(ICON_ALIASES)].sort();
}

export type IconSize = keyof typeof control.icon;

export interface IconProps {
  /** A drawing name or a declared alias. */
  name: string;
  /** A token from `control.icon` (sm 14 · md 16 · lg 20 · xl 24), or an explicit pixel size. */
  size?: IconSize | number;
  /** A class for the rare case a surface needs to position the mark (the viewport tool rail). */
  className?: string;
  /** What to draw when `name` is not in the set. Defaults to nothing — see below. */
  fallback?: IconName;
  style?: CSSProperties;
  /** Only when the icon is the control's ONLY content and no `aria-label` is nearby. */
  title?: string;
}

/**
 * **One icon.** Sized from `control.icon`, drawn on the 24-unit grid at one stroke weight, coloured by
 * `currentColor` so every hover/active/disabled variant already works, and stamped with `data-icon` —
 * the stable signal tests key on, because a character never was one.
 *
 * `aria-hidden` by default: an icon beside a label must not be announced twice, and an icon-only
 * control carries its own `aria-label` on the BUTTON. Passing `title` opts into an accessible name for
 * the rare standalone mark.
 *
 * AN UNKNOWN NAME DRAWS NOTHING AND SAYS SO. It renders an empty, still-sized placeholder carrying
 * `data-icon-missing`, so the layout does not jump and a capture gate can fail on it — rather than
 * silently substituting a question mark, which is how the character era hid its own gaps.
 */
export function Icon({ name, size = "md", fallback, className, style, title }: IconProps): ReactNode {
  const resolved = resolveIcon(name) ?? fallback ?? null;
  const px = typeof size === "number" ? size : control.icon[size];
  return (
    <svg
      data-icon={resolved ?? name}
      data-icon-missing={resolved ? undefined : "true"}
      aria-hidden={title ? undefined : true}
      role={title ? "img" : undefined}
      className={className}
      focusable="false"
      width={px}
      height={px}
      viewBox={VIEW_BOX}
      fill="none"
      stroke="currentColor"
      strokeWidth={STROKE_WIDTH}
      strokeLinecap="round"
      strokeLinejoin="round"
      style={{ display: "inline-block", verticalAlign: "-0.14em", flex: "none", ...style }}
    >
      {title ? <title>{title}</title> : null}
      {resolved ? ICONS[resolved] : null}
    </svg>
  );
}
