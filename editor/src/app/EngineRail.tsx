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
      { id: "scene", label: "Scene", icon: "▤", blurb: "Everything in this world", surface: "side" },
      { id: "build", label: "Build", icon: "＋", blurb: "Place and create objects", surface: "side" },
      { id: "terrain", label: "Terrain", icon: "▲", blurb: "Landscape, water and vegetation", surface: "side" },
    ],
  },
  {
    title: "Assets",
    engines: [
      { id: "model", label: "Model", icon: "⬡", blurb: "Repair, optimise and export meshes", surface: "bottom" },
      { id: "import", label: "Import", icon: "↓", blurb: "CAD fidelity and re-import", surface: "bottom" },
      { id: "animate", label: "Animate", icon: "◆", blurb: "Timelines, rigs and motion", surface: "bottom" },
    ],
  },
  {
    title: "Behaviour",
    engines: [
      { id: "physics", label: "Physics", icon: "◉", blurb: "Simulation, collision and mechanisms", surface: "side" },
      { id: "logic", label: "Logic", icon: "◇", blurb: "Rules, states and bindings", surface: "bottom" },
      { id: "gameplay", label: "Gameplay", icon: "⚑", blurb: "Author and run this scene as a match", surface: "side" },
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
  style?: CSSProperties;
}

/**
 * The rail.
 *
 * Rendered as a single `tablist` so a screen reader announces it as one set of nine choices rather than
 * three unrelated toolbars, and so arrow keys walk the whole list — the group headings are decoration for
 * the eye, not structure to navigate.
 */
export function EngineRail({ active, onChange, badges = {}, compact = false, style }: EngineRailProps) {
  const width = compact ? 56 : 132;
  return (
    <nav
      data-testid="engine-rail"
      aria-label="Sub-engines"
      style={{
        width,
        flex: `0 0 ${width}px`,
        display: "flex",
        flexDirection: "column",
        gap: space.xs,
        padding: `${space.sm} ${space.xs}`,
        background: color.bg.panel,
        borderRight: `1px solid ${color.border.subtle}`,
        overflowY: "auto",
        overflowX: "hidden",
        ...style,
      }}
    >
      {!compact && (
        <div
          style={{ ...text.eyebrow, padding: `0 ${space.sm} ${space.xs}`, color: color.text.faint }}
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
                style={{ ...text.eyebrow, padding: `${space.xs} ${space.xs} 2px` }}
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
                  <span aria-hidden style={{ fontSize: fontSize.label, lineHeight: 1, width: 16, textAlign: "center" }}>
                    {engine.icon}
                  </span>
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
