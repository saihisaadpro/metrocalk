//! The Engines rail — the editor's one index of what this engine can do.
//!
//! ## The problem it solves
//!
//! Capability was scattered across three docks with no organising principle: terrain lived in a right-hand
//! Inspector tab, animation at the bottom, physics on the right, logic at the bottom. The docks were
//! *positions*, not categories, so nothing was predictable — an author could not guess where anything was,
//! and there was no single place that answered "what can I do here?".
//!
//! This rail is that place. Every sub-engine is listed, once, grouped by what you are working on. **You
//! always start here**, and the active entry stays lit, so you always know which engine you are in.
//!
//! ## Why the surfaces still differ
//!
//! An animation timeline needs width; a stack of terrain sliders needs a side panel. Forcing both into one
//! column would make one of them bad. So the rail routes each engine to the surface whose *shape* fits it —
//! and because you always arrive from the rail, that is a routing detail rather than something to memorise.
//! [`EngineDef.surface`] records which, and it is the single fact the app switches on.
//!
//! The Inspector on the right is deliberately NOT in this list: it is not a sub-engine, it is the answer to
//! "what is selected?", and it must stay the same wherever you are.

import type { CSSProperties } from "react";
import { Icon } from "../theme/icons";
import { color, font, fontSize, radius, space, text } from "../theme/tokens";

/** Where an engine's workspace opens. Decided by the shape of its content, not by taste. */
export type EngineSurface = "side" | "bottom";

export interface EngineDef {
  id: EngineId;
  label: string;
  icon: string;
  /** One line, in the author's language: what this engine is *for*. */
  blurb: string;
  surface: EngineSurface;
}

export type EngineId =
  | "scene"
  | "build"
  | "terrain"
  | "model"
  | "import"
  | "animate"
  | "physics"
  | "logic"
  | "gameplay";

interface EngineGroup {
  title: string;
  engines: EngineDef[];
}

/**
 * The sub-engines, grouped by the question they answer.
 *
 * Three groups of three is deliberate: it is short enough to scan without reading, and the grouping teaches
 * the mental model — build a world, make the things in it, give them behaviour.
 */
export const ENGINE_GROUPS: EngineGroup[] = [
  {
    title: "World",
    engines: [
      { id: "scene", label: "Scene", icon: "scene", blurb: "Everything in this world", surface: "side" },
      { id: "build", label: "Build", icon: "build", blurb: "Place and create objects", surface: "side" },
      { id: "terrain", label: "Terrain", icon: "terrain", blurb: "Landscape, water and vegetation", surface: "side" },
    ],
  },
  {
    title: "Assets",
    engines: [
      { id: "model", label: "Model", icon: "model", blurb: "Repair, optimise and export meshes", surface: "bottom" },
      { id: "import", label: "Import", icon: "import", blurb: "CAD fidelity and re-import", surface: "bottom" },
      { id: "animate", label: "Animate", icon: "animate", blurb: "Timelines, rigs and motion", surface: "bottom" },
    ],
  },
  {
    title: "Behaviour",
    engines: [
      { id: "physics", label: "Physics", icon: "physics", blurb: "Simulation, collision and mechanisms", surface: "side" },
      { id: "logic", label: "Logic", icon: "logic", blurb: "Rules, states and bindings", surface: "bottom" },
      { id: "gameplay", label: "Gameplay", icon: "gameplay", blurb: "Author and run this scene as a match", surface: "side" },
    ],
  },
];

/** Every engine, flat — for lookups and the command palette. */
export const ENGINES: EngineDef[] = ENGINE_GROUPS.flatMap((g) => g.engines);

/** Look one up. */
export function engineById(id: EngineId): EngineDef {
  return ENGINES.find((e) => e.id === id) ?? ENGINES[0];
}

export interface EngineRailProps {
  active: EngineId;
  onChange: (id: EngineId) => void;
  /** Live counts or state, keyed by engine id — e.g. the scene's object count, "live" during Play. */
  badges?: Partial<Record<EngineId, string | number>>;
  /** Icons only. Used when the window is too narrow to afford the labels. */
  compact?: boolean;
  /** Optional class for the rail element. The shell does NOT pass its `mtk-shell-card` here: the card
   *  is a painted surface and `style` below carries the Play-mode dim, which is an `opacity` — one
   *  element cannot be both without making the panel translucent over a transparent native root. The
   *  shell wraps the rail in the card instead, so the surface paints and the contents dim. */
  className?: string;
  style?: CSSProperties;
}

/**
 * The rail.
 *
 * Rendered as a single `tablist` so a screen reader announces it as one set of nine choices rather than
 * three unrelated toolbars, and so arrow keys walk the whole list — the group headings are decoration for
 * the eye, not structure to navigate.
 */
export function EngineRail({ active, onChange, badges = {}, compact = false, className, style }: EngineRailProps) {
  // NO WIDTH OF ITS OWN, AND THAT IS THE CHANGE. It used to restate the grid track `layout.ts`
  // reserves, because the rail WAS that track: it painted the panel background and drew the hairline
  // separating it from the dock beside it. It is now the contents of a card that floats inside the
  // track with a gutter around it, so the track's width and the rail's width are deliberately no
  // longer the same number — and a `flex: 0 0 132px` inside a card that has 120px to give is the
  // overflow R7 already caught once in this file. It fills the card; the card fills what the gutter
  // leaves of the track; `layout.ts` still owns the track. One width, one owner, as before.
  return (
    <nav
      data-testid="engine-rail"
      aria-label="Sub-engines"
      className={className}
      style={{
        width: "100%",
        flex: "1 1 auto",
        display: "flex",
        flexDirection: "column",
        gap: space.xs,
        // `border-box`, so the padding below comes out of the 100% rather than being added to it —
        // the same reason it was here when the width was a fixed track number. Content-box made the
        // rail 141px wide in a 132px track and `shots` R7 measured exactly that: **9px of the rail
        // cut, no scrollbar**, across 14 scenes, in the run meant to prove the padding fix below.
        boxSizing: "border-box",
        // `px`, on all three paddings in this file. React appends the unit to a NUMBER, never inside
        // a template string, so `` `${space.sm} ${space.xs}` `` reached the DOM as the invalid length
        // list "6 4" and the browser dropped the whole declaration — the rail, its heading and every
        // group title have therefore had NO padding since they were written, which is why the labels
        // sat flush against the window edge while their own items were indented. Nothing could see
        // it: `tsc` types this as a string, the constitution ratchet read colours and motion but not
        // units, and R1–R8 are all silent because a missing padding overflows nothing, clips nothing,
        // overlaps nothing and is perfectly readable. Found by LOOKING at `shell-wide.png`, and now
        // gated by the ratchet's `unitless-length` rule (ADR-128).
        padding: `${space.sm}px ${space.xs}px`,
        // No background and no right-hand hairline. The card behind the rail (`.mtk-shell-card`)
        // paints the surface now, and the gap the shell gutter leaves is what separates the rail
        // from the dock beside it — a line drawn on top of a gap says the same thing twice. Leaving
        // the background here would also be actively wrong: `style` below carries the Play-mode dim,
        // which is an `opacity`, and an opacity composites an element WITH its own background.
        overflowY: "auto",
        overflowX: "hidden",
        ...style,
      }}
    >
      {!compact && (
        <div
          style={{ ...text.eyebrow, padding: `0 ${space.sm}px ${space.xs}px`, color: color.text.muted }}
        >
          Engines
        </div>
      )}
      <div role="tablist" aria-label="Sub-engines" aria-orientation="vertical" style={{ display: "grid", gap: space.xs }}>
        {ENGINE_GROUPS.map((group) => (
          <div key={group.title} style={{ display: "grid", gap: 2 }}>
            {!compact && (
              <div
                aria-hidden
                style={{ ...text.eyebrow, padding: `${space.xs}px ${space.xs}px 2px` }}
              >
                {group.title}
              </div>
            )}
            {group.engines.map((engine) => {
              const on = engine.id === active;
              const badge = badges[engine.id];
              return (
                // ui-constitution-allow raw-native-control: the rail tab is a bespoke roving-tabindex tablist entry with its own active-marker anatomy — the shared Button's states would fight the tab semantics
                <button
                  key={engine.id}
                  type="button"
                  role="tab"
                  id={`engine-tab-${engine.id}`}
                  aria-selected={on}
                  aria-controls={`engine-panel-${engine.id}`}
                  tabIndex={on ? 0 : -1}
                  data-testid={`engine-${engine.id}`}
                  title={`${engine.label} — ${engine.blurb}`}
                  onClick={() => onChange(engine.id)}
                  onKeyDown={(e) => {
                    // Arrow keys walk the WHOLE rail, across the group headings: the headings are for the
                    // eye, and a keyboard user should not have to know they exist.
                    const i = ENGINES.findIndex((x) => x.id === engine.id);
                    const step = e.key === "ArrowDown" ? 1 : e.key === "ArrowUp" ? -1 : 0;
                    if (!step) return;
                    e.preventDefault();
                    const next = ENGINES[(i + step + ENGINES.length) % ENGINES.length];
                    onChange(next.id);
                    document.getElementById(`engine-tab-${next.id}`)?.focus();
                  }}
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: space.xs,
                    width: "100%",
                    padding: compact ? space.xs : `${space.xs} ${space.sm}`,
                    justifyContent: compact ? "center" : "flex-start",
                    borderRadius: radius.sm,
                    border: "1px solid transparent",
                    background: on ? color.accent.subtle : "transparent",
                    color: on ? color.accent.base : color.text.secondary,
                    borderColor: on ? color.accent.border : "transparent",
                    // A solid marker on the leading edge. Colour alone is a weak "you are here" at this
                    // size, and it fails for anyone who cannot separate the two hues.
                    boxShadow: on ? `inset 3px 0 0 0 ${color.accent.base}` : "none",
                    font: font.ui,
                    fontSize: fontSize.body,
                    fontWeight: on ? 600 : 400,
                    cursor: "pointer",
                    textAlign: "left",
                    minHeight: 30,
                  }}
                >
                  <Icon name={engine.icon} size="md" />
                  {!compact && <span style={{ flex: 1, whiteSpace: "nowrap" }}>{engine.label}</span>}
                  {badge != null && (
                    <span
                      style={{
                        fontSize: fontSize.micro,
                        fontFamily: font.mono,
                        color: on ? color.accent.base : color.text.muted,
                      }}
                    >
                      {badge}
                    </span>
                  )}
                </button>
              );
            })}
          </div>
        ))}
      </div>
    </nav>
  );
}

export default EngineRail;
