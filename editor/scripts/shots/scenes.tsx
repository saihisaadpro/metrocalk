//! The scenes this harness can photograph — one entry per panel state worth looking at.
//!
//! WHY A REGISTRY AND NOT ONE HARNESS PER PANEL. The predecessor of this file was written twice, by
//! hand, in a `progress/` folder, for one panel each time — and each time it found a real defect that
//! a green vitest suite could not see (a remove button that wrapped onto its own line; a stale-closure
//! no-op in the harness's own mock). A tool that has earned its keep twice belongs in the repository
//! with a gate on it, not in an evidence folder where the next session rewrites it. `mesh_frame_bench.rs`
//! is the standing lesson: it compiled cleanly through four renderer generations while every contract
//! inside it rotted, because nothing ran it.
//!
//! WHAT A SCENE IS. A pure `render()` over a mock client — no network, no Tauri, no GPU. `shoot.mjs`
//! opens each one in a real Chromium, screenshots it, and FAILS if the page threw or the frame came
//! back blank. That last part is the difference between this and a build-only job: a scene that stops
//! mounting is a red gate, not a smaller number in a log nobody diffs.

import { useEffect, useState, type ReactNode } from "react";
import { App } from "../../src/app/App";
import { AssetBrowser } from "../../src/panels/AssetBrowser";
import {
  AssetLabPanel,
  type AssetLabReportView,
  type AssetLabStage,
} from "../../src/panels/AssetLabPanel";
import { STAGE_MIN } from "../../src/app/layout";
import { BindingGraph } from "../../src/graph/BindingGraph";
import { Icon, iconTokens } from "../../src/theme/icons";
import { Inspector } from "../../src/inspector/Inspector";
import { StateGraph } from "../../src/graph/StateGraph";
import { AnimationGraphEditor } from "../../src/graph/AnimationGraphEditor";
import { animationGraphPorts, createLocomotionGraphPreset } from "../../src/graph/animation-graph-model";
import { animationEditorStore, animationWorkspaceKey } from "../../src/store/animation";
import { StateGraphPanel } from "../../src/panels/StateGraphPanel";
import { DockTabs } from "../../src/theme/workspace";
import { AnimationWorkspace } from "../../src/panels/AnimationWorkspace";
import { Diagnostics } from "../../src/panels/Diagnostics";
import { ImportReport } from "../../src/panels/ImportReport";
import { Reveal } from "../../src/panels/Reveal";
import { RigPanel, type RigDocument } from "../../src/panels/RigPanel";
import { PipeForge } from "../../src/panels/PipeForge";
import RIG_MIXAMO from "../../src/panels/__fixtures__/rig-characterization.json";
import RIG_BLOCKED from "../../src/panels/__fixtures__/rig-not-retargetable.json";
import { MatchPanel } from "../../src/panels/MatchPanel";
import { PhysicsPanel } from "../../src/panels/PhysicsPanel";
import { PosePreview, type PoseDocument } from "../../src/panels/PosePreview";
import POSE_PREVIEW from "../../src/panels/__fixtures__/pose-preview.json";
import { assetShelfStore } from "../../src/store/assetShelf";
import { playStore } from "../../src/store/play";
import { projectionStore } from "../../src/store/projection";
import type {
  AnimationGraphDebugInfo,
  AnimationGraphStateInfo,
  AnimationPropertyInfo,
  AnimationTrackInfo,
  AnimationWorkspaceInfo,
  CadReport,
  CadReportPart,
  EffectSpec,
  MatchValidation,
  PipeBakeReport,
  PipeForgeStatus,
  RevealResponse,
  RoleRow,
  RoleSpec,
  ShotSpec,
  RuleRegistryInfo,
  StateMachine,
  StateMachineInfo,
  TimelineTuple,
} from "../../src/transport/protocol";
import { createMockSession, type EditorClient } from "../../src/transport/session";

/** What must be true of the rendered scene, checked by `shoot.mjs` in the page before it captures.
 *
 *  WHY A SCENE MUST ASSERT SOMETHING. The first version of this gate had three bars — no page error,
 *  a root that mounted, and a PNG with more than two distinct colours — and an adversarial review
 *  broke all three at once: deleting **every import row** from the panel left the gate green, because
 *  the header and the filter chips still painted pixels and the mounted root belonged to the harness,
 *  not to the scene. Re-introducing the exact defect this session fixed (rendering a literal `null ·
 *  null · null`) was green too.
 *
 *  A capture that proves "something rendered" is not evidence about a panel. So the claim a scene
 *  makes is written down next to the scene, in the same file, and it FAILS the run — which also makes
 *  `looking_for` executable prose instead of a comment nobody re-reads. */
export type Expect = {
  /** Selectors that must exist, with a minimum count. `["[data-testid='import-row']", 6]`. */
  present?: [selector: string, atLeast: number][];
  /** Selectors that must NOT exist. */
  absent?: string[];
  /** Substrings that must not appear in the frame's rendered text — `"null"` is the standing one. */
  text_absent?: string[];
  /** Substrings that must appear. */
  text_present?: string[];
  /** Pairs that must share a line — their vertical extents overlap.
   *
   *  Added because this file's own rule was broken one scene after it was written. The wide
   *  Diagnostics scene said "badge, message and fix on ONE line" in `looking_for` and asserted only
   *  that the button EXISTED, so it passed green while the row wrapped at every width — the fix for
   *  320 px had quietly become the layout everywhere. `looking_for` that nothing evaluates is the
   *  thing this harness was built to stop, and "did it wrap" is the one question a capture answers
   *  and a DOM assertion cannot. */
  same_line?: [a: string, b: string][];
  /** The exact dual: `a` and `b` are separate sentences, on separate lines, `a` above `b`.
   *
   *  `same_line` could only ever assert the defect it was written against here. The rig panel's
   *  blocking diagnostic ran its twelve-item comma list straight into the instruction that resolves
   *  it — "…Right Lower Arm, Right Hand Assign each one in the rig panel" — with no terminator and no
   *  break, so the seam was a colour change mid-line. Every `present`/`text_present` claim on that
   *  scene passed; reading the PNG is what found it. Order is part of the claim, because two blocks
   *  that swapped places are still stacked, and a fix printed above its own complaint is not this. */
  stacked?: [above: string, below: string][];
  /** Elements that must MEASURE at least this many CSS pixels wide.
   *
   *  The stage-is-sacred rule (`<ux_quality>` 5) is a product principle about a **measurement**, and
   *  until now the only thing asserting it was `layout.test.ts` — which compares the *string*
   *  `panelLayout` returns (`"340px minmax(320px, 1fr) 300px"`). A grid template is a declaration of
   *  intent; whether the stage is actually 320 px wide once real content is inside the docks is a
   *  question jsdom cannot be asked. A dock whose content will not shrink pushes the stage below its
   *  floor while `panelLayout` keeps returning exactly the string the unit test wants. */
  min_width?: [selector: string, px: number][];
  /** Elements that must MEASURE at least this many CSS pixels TALL.
   *
   *  The vertical axis had no assertion of any kind — not a measurement, and not even the string
   *  `min_width` was written to improve on. The shell's whole vertical stack (header · stage · bottom
   *  dock · status bar) is composed in a stylesheet out of `vh` units and absolute minimums, none of
   *  which can see each other, and every shell scene was captured at one window height. So the one
   *  regime where the parts do not fit — a short window with a task workspace open — had never been
   *  photographed, measured, or reasoned about. */
  min_height?: [selector: string, px: number][];
  /** The DUAL of `min_width`: the element must measure NO MORE than this, read from its raw rect.
   *
   *  A floor cannot see a box that grew. `.mtk-graph-card` is drawn at `GRAPH_CARD_WIDTH`, which
   *  `columnLayout` also subtracts from the column gap and `GraphSurface` also hands to React Flow —
   *  one number, three readers. Under the default `content-box` the browser drew 232 instead of 208
   *  and all three were wrong by 24px per column, while every `min_width` in the suite passed more
   *  easily than before. This is how a geometry contract that lives in TypeScript gets checked
   *  against the pixels that CSS actually produced. */
  max_width?: [selector: string, px: number][];
  /** The vertical dual of `max_width`. */
  max_height?: [selector: string, px: number][];
  /** EVERY element matching each selector must be fully on screen — its own box, intersected with
   *  every ancestor that clips, must not have lost anything.
   *
   *  `present` counts elements in the DOM, which is a different question from whether a user can see
   *  or click one. A `.mtk-dock-tab` strip scrolls with `scrollbar-width: none`, so a tab past the
   *  edge is in the DOM, in the accessibility tree, focusable by keyboard — and invisible, with
   *  nothing on screen suggesting it is there. */
  unclipped?: string[];
  /** EVERY element matching each selector shows all of its text — none of it replaced by an ellipsis.
   *
   *  The claim `unclipped` and `min_width` between them cannot make. A control that is exactly the
   *  size it chose to be, at a legal click target, with nothing cut away by an ancestor, painting
   *  `M…` where `Model` should be, satisfies both. The bottom dock read `M… I… Fo… An… L… Pro… Ru…`
   *  at a 1280 window and the Animate strip read `Ti… Cu… Gr…` at 508px of dock, and every rule in
   *  the driver agreed with both. */
  untruncated?: string[];
  /** The named children TILE their container's content box on screen: their measured heights add up
   *  to its, with no gap, no overlap and nothing cut away.
   *
   *  THE ONE CLAIM THAT WORKS WHERE A FLOOR CANNOT. Every shell scene inherits `min_height` on the
   *  viewport at `STAGE_MIN`, and that is right in every regime but one. Below a ~443px window the
   *  chrome (81px), the stage's 320px floor and the dock's 42px bar want more room than the window
   *  has — the floor is not being violated by a greedy panel, it is arithmetically unreachable. The
   *  shell's answer is the one `dockGridColumns` already wrote down for the other axis: *if even the
   *  rails and the floor do not fit, the stage absorbs the remainder.* A gate cannot assert a floor
   *  there, and the escape hatch that would let a scene waive one is precisely what ADR-124 refused
   *  to build — an exemption mechanism gets used for the first real defect.
   *
   *  So the claim changes shape instead of weakening. What is still true, and is the whole of what
   *  the user is owed, is that **nothing took space it should not have**: the stage got everything
   *  the bar left, exactly, and neither of them is cut. Conservation is strictly stronger than a
   *  floor here — it fails if the dock exceeds its bar, if the stage dips further than it must, if
   *  the two overlap, and if either is clipped — and it needs no number to argue about. */
  fills?: [container: string, children: string[]][];
  /** The stage's protected height, capped by what its column actually has left. Set once by `shell()`;
   *  see the driver for why this is not a `min_height` entry. */
  stage_floor?: number;
};

export type Scene = {
  /** Stable id — the `?scene=` parameter and the PNG's filename. Never derived from a title. */
  id: string;
  /** What a reader should be checking in the capture. Printed by `shoot.mjs` beside the filename. */
  looking_for: string;
  /** The machine-checkable part of `looking_for`. A scene without one is rejected by the driver. */
  expect: Expect;
  /** Cap the FRAME's width inside the default window — a panel photographed at the width of the dock
   *  it lives in. The window stays 620 px, so a component that reads `window.innerWidth` sees 620 no
   *  matter what this says. */
  width?: number;
  /** Resize the WINDOW itself. Required for anything responsive: `App` lays itself out from
   *  `window.innerWidth`, so a CSS `maxWidth` on the frame would photograph a 1440 px-wide box that
   *  had computed its own layout for 620 px — a capture that is wrong in exactly the way a capture is
   *  supposed to catch. Mutually exclusive with `width`: two statements of one number is the drift
   *  this repository gates for everywhere else, and the driver rejects a scene that sets both. */
  viewport?: { width: number; height: number };
  /** Selectors clicked, in order, before anything is asserted or captured.
   *
   *  A dock is a FIXED-WIDTH track holding one of several workspaces, and which one is a click, not
   *  a prop — so without this the gate photographs the default workspace and nothing else, and the
   *  four others go the way `mesh_frame_bench.rs` went. `layout.ts` still carries the scar of that
   *  exact class ("which left the terrain workspace overflowing its 280px dock the moment it moved
   *  out of the Inspector") — found, as ever, by a human.
   *
   *  A selector that matches nothing FAILS the scene rather than being skipped: a click that quietly
   *  did nothing would photograph the default state under a caption claiming another, which is worse
   *  than no capture at all. */
  click?: string[];
  setup?: () => void;
  render: () => ReactNode;
};

const client = (cadReport: () => Promise<CadReport>) => ({ cadReport }) as unknown as EditorClient;

const revealClient = (r: RevealResponse) =>
  ({ revealTargets: () => Promise.resolve(r), bind: () => "op-1" }) as unknown as EditorClient;

/** The five explanatory fields exactly as the shell sends them when it has nothing to say: the key is
 *  PRESENT, holding `null`. Bare `Option<String>` with no `skip_serializing_if` (ADR-123). A harness
 *  that omitted them would photograph a payload the real core cannot produce — which is the C6 failure
 *  (green against the mock, wrong against `/core`) reached through a screenshot instead of a test. */
const NOTHING_TO_SAY = { reference: null, strategy: null, reason: null, fix: null, sourceFormat: null };

const part = (id: string, name: string, fidelity: string, extra: Partial<CadReportPart> = {}): CadReportPart =>
  ({ id, name, fidelity, ...NOTHING_TO_SAY, ...extra });

const report = (parts: CadReportPart[]): CadReport => ({
  total: parts.length,
  exactBrep: parts.filter((p) => p.fidelity === "exact-brep").length,
  tessellationOnly: parts.filter((p) => p.fidelity === "tessellation-only").length,
  aiReconstructed: parts.filter((p) => p.fidelity === "ai-reconstructed").length,
  proxy: parts.filter((p) => p.fidelity === "proxy").length,
  accessDenied: parts.filter((p) => p.fidelity === "access-denied").length,
  failed: parts.filter((p) => p.fidelity === "failed").length,
  parts,
});

const seedScene = () =>
  projectionStore.getState().bulkLoad([
    { id: "e1", name: "Plate", parentId: null, components: { CadPart: { fidelity: "exact-brep" } } },
  ] as never);

const ALL_NULL = [
  part("e1", "Plate", "exact-brep"),
  part("e2", "Weld Gun", "tessellation-only"),
  part("e3", "Conveyor", "tessellation-only"),
  part("e4", "Overhead Crane", "proxy"),
  part("e5", "Hydraulic Ram", "access-denied"),
  part("e6", "Cable Tray", "failed"),
];

const DETAILED = [
  part("e1", "Plate", "exact-brep", { reference: "PLATE-A/3", sourceFormat: "STEP AP242" }),
  part("e7", "Gearbox", "proxy", {
    reference: "GEARBOX-A/1",
    strategy: "kernel-unavailable",
    sourceFormat: "CATPart",
    reason: "The licensed CAD kernel is not installed on this machine.",
    fix: "Install the kernel, or supply the STEP companion file.",
  }),
  part("e8", "Impeller", "ai-reconstructed", {
    reference: "IMP-77",
    strategy: "mesh-to-brep",
    sourceFormat: "STL",
    reason: "Reconstructed from the mesh at 0.82 confidence.",
  }),
];

/** The reveal, with names of the length the product actually produces. `<visual_acceptance>` §2 is
 *  explicit that a fixture hides the bugs real content exposes, and here "real content" is not a
 *  guess: a 3DXML factory cell lands 3,387 entities whose names come from CATIA's product tree
 *  ("Overhead Crane Assembly Rev C — Long Travel Girder"), and the reasons come from the registry's
 *  "explain every no" rule, which spells them out in whole sentences. A seeded `Cube_01` would prove
 *  nothing about either. */
const CAD_REVEAL: RevealResponse = {
  required: ["PowerSource", "ControlSignal"],
  compatible: [
    { id: "c1", name: "Overhead Crane Assembly Rev C — Long Travel Girder", distance: 2.4, affinity: 92 },
    { id: "c2", name: "Weld Cell Transformer 480V", distance: 5.1, affinity: 71 },
    { id: "c3", name: "Busbar", distance: 9.8, affinity: 40 },
  ],
  greyed: [
    {
      id: "g1",
      name: "Hydraulic Power Unit — Skid Mounted, 210 bar",
      reason: "it supplies hydraulic pressure, not electrical power",
    },
  ],
  bound: [{ id: "b1", name: "Main Distribution Panel MDP-1", kind: "PowerSource" }],
};

const selectCadEntity = () => {
  const s = projectionStore.getState();
  s.bulkLoad([{ id: "sel", name: "Weld Gun 7", parentId: null, components: {} }] as never);
  s.select("sel");
};

/** Diagnostics reads the SAME reveal as the picker (one round-trip, perf audit F2), so it inherits the
 *  same long names — and puts the best-ranked one inside a button label ("Bind to …"). `HealthBar` with
 *  no binding is what `deriveRel` turns into `needsBinding`, which is the branch that renders the fix. */
const selectUnwiredEntity = () => {
  const s = projectionStore.getState();
  s.bulkLoad([
    { id: "diag", name: "Weld Gun 7", parentId: null, components: { HealthBar: {} } },
  ] as never);
  s.select("diag");
};

// ── the animate timeline, with tracks on it ───────────────────────────────────────────────────────

/** WHY THIS SCENE EXISTS. `shell-dock-animate` photographs the Animate dock on a scene where nothing
 *  is selected — so it captures a ruler, two annotation lanes and an empty state, and **not one track
 *  row**. Every part of the timeline that only appears once there is something to animate — the track
 *  header with its mute and lock, the zebra lane, the key diamonds, the selected-row treatment, the
 *  playhead crossing a lane rather than only the ruler — had never been in a capture at all. A
 *  surface whose populated state is unphotographed is a surface where the populated state is the one
 *  that breaks, which is the whole argument this harness was built on.
 *
 *  The client is a stub rather than the MockCore because MockCore's sample scene has no authored
 *  animation: getting tracks through it would mean authoring them at runtime through the transport,
 *  which photographs the editing path and not the timeline. */
const ANIMATION_TICKS = 60;

function animTrack(
  id: string,
  name: string,
  property: string,
  keys: number[],
  extra: Partial<AnimationTrackInfo> = {},
): AnimationTrackInfo {
  return {
    id,
    name,
    targetId: "rig",
    targetName: "Weld Gun 7",
    component: "Transform",
    property,
    valueKind: "float",
    interpolation: "cubic",
    enabled: true,
    locked: false,
    context: "3d",
    editorKind: "scalar",
    bindingState: "ready",
    bindingReason: "Bound to Transform." + property,
    runtimeSink: "transform",
    keys: keys.map((tick, index) => ({
      id: `${id}-k${index}`,
      tick,
      seconds: tick / ANIMATION_TICKS,
      value: index % 2 === 0 ? 0 : 1.5,
      inTangent: null,
      outTangent: null,
    })),
    ...extra,
  };
}

function animProperty(
  component: string,
  property: string,
  label: string,
  value: number,
  extra: Partial<AnimationPropertyInfo> = {},
): AnimationPropertyInfo {
  return {
    component,
    property,
    label: `${component} · ${label}`,
    valueKind: "float",
    value,
    animatable: true,
    reason: null,
    context: "3d",
    editorKind: "scalar",
    bindingState: "ready",
    bindingReason: `Bound to ${component}.${property}`,
    runtimeSink: "transform",
    ...extra,
  };
}

const ANIMATED: AnimationWorkspaceInfo = {
  revision: "rev-7",
  sequenceId: "main",
  sequenceName: "Weld pass",
  ticksPerSecond: ANIMATION_TICKS,
  durationTick: 180,
  currentTick: 68,
  playing: false,
  loopPolicy: "loop",
  selectedId: "rig",
  selectedName: "Weld Gun 7",
  /* THE FIXTURE CONTRADICTED ITSELF AND THE CAPTURE PRINTED BOTH HALVES. `contexts[3d]` claimed
     `properties: 4` — which the readiness box renders as "4 properties · 4 tracks" — while this array
     was EMPTY, so the Property select fell to its no-verified-properties option and `Add key at 68t`
     rendered disabled, three inches below a line saying there were four of them. The real engine
     cannot produce that pair: `contexts[].properties` is a count OF this array. So the four channels
     the four tracks are keyed on are stated here as well, which is what the count was always about,
     and the capture stops being evidence for a state the engine has no way to reach
     (`<verification_states_and_convergence>` (b) — a mock that answers with a payload of its own). */
  properties: [
    animProperty("Transform", "position.y", "Position Y", 0.42),
    animProperty("Transform", "rotation.z", "Rotation Z", 0),
    animProperty("Transform", "scale", "Scale", 1),
    animProperty("Emitter", "rate", "Rate", 120, {
      bindingState: "preview_only",
      bindingReason: "Preview only until the emitter is bound.",
      runtimeSink: null,
    }),
  ],
  tracks: [
    animTrack("t1", "Transform · Position Y", "position.y", [0, 45, 96, 150]),
    animTrack("t2", "Transform · Rotation Z", "rotation.z", [0, 68, 180]),
    animTrack("t3", "Transform · Scale", "scale", [24, 120], { locked: true }),
    animTrack("t4", "Emitter · Rate", "rate", [0, 90], {
      enabled: false,
      component: "Emitter",
      bindingState: "preview_only",
      bindingReason: "Preview only until the emitter is bound.",
    }),
  ],
  markers: [
    { id: "m1", ownerId: "rig", name: "Contact", tick: 45, seconds: 0.75, color: null },
    { id: "m2", ownerId: "rig", name: "Release", tick: 150, seconds: 2.5, color: null },
  ],
  events: [{ id: "e1", ownerId: "rig", name: "spark", tick: 96, seconds: 1.6, payload: null }],
  contexts: [
    { context: "2d", state: "unsupported", properties: 0, tracks: 0, reason: "No 2D channels are present on this selection.", action: null },
    { context: "3d", state: "ready", properties: 4, tracks: 4, reason: "Four bound Transform channels.", action: null },
    { context: "ui", state: "unsupported", properties: 0, tracks: 0, reason: "No UI channels are present on this selection.", action: null },
  ],
  asset: null,
  issues: [],
};

const animationClient = () =>
  ({
    animationState: () => Promise.resolve(ANIMATED),
    animationPlaybackState: () =>
      Promise.resolve({ playing: false, currentTick: ANIMATED.currentTick, durationTick: ANIMATED.durationTick, loopPolicy: ANIMATED.loopPolicy, crossedEvents: [], eventsTruncated: false }),
  }) as unknown as EditorClient;

/** A simulation that has recorded 300 frames and is running — the state `#simToggle` pauses into, so
 *  the scene reaches its subject through the panel's own transport rather than by injecting state.
 *  `simTimeline` is `[frame, frames, running, debugOverlay, _]` (`TimelineTuple`). */
const physicsClient = () =>
  ({
    simTimeline: () => Promise.resolve([128, 300, true, false, 0] as TimelineTuple),
    simScrub: (frame: number) => Promise.resolve([frame, 300, false, false, 0] as TimelineTuple),
    setSimRunning: () => undefined,
    physicsContacts: () => Promise.resolve([]),
    physicsCheck: () => Promise.resolve([]),
  }) as unknown as EditorClient;

const selectAnimatedEntity = () => {
  const s = projectionStore.getState();
  s.bulkLoad([{ id: "rig", name: "Weld Gun 7", parentId: null, components: { Transform: {} } }] as never);
  s.select("rig");
};

// ── the graph editors ─────────────────────────────────────────────────────────────────────────────

/** THE SIGNATURE INTERFACE NOTHING HAS EVER PHOTOGRAPHED. The constitution names the binding editor
 *  one of the engine's signature surfaces and spells out what it owes — smooth curves, lightweight
 *  nodes, generous spacing, immediately-understandable relationships, search, filters, zoom, mini
 *  map. `BindingGraph` is 67 lines that hand React Flow a `data.label` and let the vendored
 *  stylesheet draw the rest, and `StateGraph` is the same 67 lines re-pointed at state data.
 *
 *  Neither has ever appeared in a capture, in this harness or anywhere else, so every one of those
 *  obligations has been unwatched since the day it was written down. The unit tests cannot see it:
 *  React Flow measures the pane with `getBoundingClientRect`, which is 0×0 in jsdom, so it renders
 *  no nodes at all there — the ONE surface in this editor whose populated state is unreachable
 *  except in a real browser.
 *
 *  The fixture is a real bind neighbourhood with all three edge statuses on it at once, because the
 *  status colour rule is the only thing distinguishing a live wire from a refused one and a capture
 *  containing one status proves nothing about the other two. */
const BIND_NEIGHBOURHOOD = {
  summaries: {
    gun: { id: "gun", name: "Weld Gun 7", parentId: null, kind: "requirer" },
    mdp: { id: "mdp", name: "Main Distribution Panel MDP-1", parentId: null, kind: "mesh" },
    plc: { id: "plc", name: "Cell PLC — Safety Interlock", parentId: null, kind: "mesh" },
    hpu: { id: "hpu", name: "Hydraulic Power Unit — Skid Mounted, 210 bar", parentId: null, kind: "mesh" },
    hmi: { id: "hmi", name: "Weld Cell HMI", parentId: null, kind: "requirer" },
    log: { id: "log", name: "Line Supervisor Log", parentId: null, kind: "requirer" },
  },
  edges: {
    "mdp|PowerSource|gun": { id: "mdp|PowerSource|gun", from: "mdp", rel: "PowerSource", to: "gun", status: "confirmed" },
    "plc|ControlSignal|gun": { id: "plc|ControlSignal|gun", from: "plc", rel: "ControlSignal", to: "gun", status: "pending" },
    "hpu|PowerSource|gun": { id: "hpu|PowerSource|gun", from: "hpu", rel: "PowerSource", to: "gun", status: "rejected" },
    "gun|WeldEvent|hmi": { id: "gun|WeldEvent|hmi", from: "gun", rel: "WeldEvent", to: "hmi", status: "confirmed" },
    "gun|WeldEvent|log": { id: "gun|WeldEvent|log", from: "gun", rel: "WeldEvent", to: "log", status: "confirmed" },
  },
} as const;

/** THE SECOND REGIME, AND THE ONE THE CHROME EXISTS FOR. Six nodes fit on a canvas; a real cell
 *  does not. `NEIGHBOUR_CAP` (48) and `minimapFrom` (12) are two thresholds that only mean anything
 *  above a size no capture had ever reached, so the mini map, the "showing 48 of N" notice and the
 *  behaviour of a column tall enough to need scrolling were all unwatched by construction — the same
 *  argument as `animation-timeline-tracks` versus the empty Animate dock.
 *
 *  Generated rather than typed out, from a fixed table, so the fixture is reproducible and the count
 *  is a number the scene can assert against rather than a length someone has to keep in sync. */
const DENSE_KINDS = ["mesh", "light", "camera", "physics", "audio", "imported"] as const;

function denseNeighbourhood(providers: number, consumers: number) {
  const summaries: Record<string, unknown> = {
    cell: { id: "cell", name: "Weld Cell 12 — Controller", parentId: null, kind: "requirer" },
  };
  const edges: Record<string, unknown> = {};
  for (let i = 0; i < providers; i++) {
    const id = `p${i}`;
    summaries[id] = {
      id,
      name: `Feeder ${String(i + 1).padStart(2, "0")} — ${DENSE_KINDS[i % DENSE_KINDS.length]}`,
      parentId: null,
      kind: DENSE_KINDS[i % DENSE_KINDS.length],
    };
    const eid = `${id}|Supplies|cell`;
    edges[eid] = { id: eid, from: id, rel: "Supplies", to: "cell", status: i % 7 === 3 ? "pending" : "confirmed" };
  }
  for (let i = 0; i < consumers; i++) {
    const id = `c${i}`;
    summaries[id] = { id, name: `Station ${String(i + 1).padStart(2, "0")} readout`, parentId: null, kind: "requirer" };
    const eid = `cell|Reports|${id}`;
    edges[eid] = { id: eid, from: "cell", rel: "Reports", to: id, status: "confirmed" };
  }
  return { summaries, edges };
}

const DENSE = denseNeighbourhood(9, 7);

const seedDenseNeighbourhood = () => {
  projectionStore.setState({
    summaries: { ...DENSE.summaries } as never,
    edges: { ...DENSE.edges } as never,
    order: Object.keys(DENSE.summaries),
    selectedId: "cell",
    multiSelect: ["cell"],
  });
};

const seedBindNeighbourhood = () => {
  projectionStore.setState({
    summaries: { ...BIND_NEIGHBOURHOOD.summaries } as never,
    edges: { ...BIND_NEIGHBOURHOOD.edges } as never,
    order: Object.keys(BIND_NEIGHBOURHOOD.summaries),
    selectedId: "gun",
    multiSelect: ["gun"],
  });
};

/** A door with a real event vocabulary rather than `A → B`: the transition LABELS are what the state
 *  graph exists to make readable, and three of them leave the same node. */
const DOOR_MACHINE: StateMachine = {
  name: "Airlock Door",
  entity: "door",
  component: "DoorState",
  field: "phase",
  states: ["Locked", "Closed", "Opening", "Open", "Closing", "Jammed"],
  initial: "Locked",
  transitions: (
    [
      ["t1", "Locked", "Closed", "badge_accepted"],
      ["t2", "Closed", "Opening", "open_requested"],
      ["t3", "Opening", "Open", "travel_complete"],
      ["t4", "Open", "Closing", "close_requested"],
      ["t5", "Closing", "Closed", "travel_complete"],
      ["t6", "Opening", "Jammed", "obstruction_detected"],
      ["t7", "Jammed", "Closing", "obstruction_cleared"],
    ] as const
  ).map(([id, from, to, event]) => ({
    id,
    from,
    to,
    rule: { name: event, enabled: true, event, conditions: [], actions: [] },
  })),
};

/** Both graph scenes are captured in a box the size of the dock panel that actually holds them —
 *  `EditorDocks` gives the binding graph a `minHeight: 220` fill inside a 300px track, and the
 *  Rules panel gives the state graph 240px. A graph photographed at 1440×900 is a graph nobody has
 *  ever seen: the whole question is whether it stays legible in the space it really gets. */
function graphFrame(height: number, children: ReactNode) {
  return (
    <div
      style={{
        display: "grid",
        height,
        padding: 16,
        background: "var(--mtk-bg-panel)",
      }}
    >
      {children}
    </div>
  );
}

function graphScenes(): Scene[] {
  return [
    {
      id: "binding-graph-neighbourhood",
      looking_for:
        "north-star #1 as a PICTURE: the selected object in the middle, what powers it on the left, " +
        "what it feeds on the right, and every wire labelled with the relation it carries. The three " +
        "edge statuses must be told apart at a glance — a confirmed bind, a pending one still in " +
        "flight, and a refused one — which means the wire's own colour has to be VISIBLE either " +
        "side of its label, and the legend has to say what each colour means. Three columns need " +
        "960px and this canvas has 698, so the fit stops at its readability floor and the mini map " +
        "appears to say there is more graph off to the side",
      viewport: { width: 760, height: 460 },
      setup: seedBindNeighbourhood,
      expect: {
        present: [
          [".react-flow__node", 6],
          [".react-flow__edge", 5],
          [".react-flow__minimap", 1],
          [".react-flow__minimap-node", 6],
          // The positive half of the motion rule: exactly the pending bind marches, because it is
          // the only thing here that is genuinely still in flight.
          [".react-flow__edge.animated", 1],
        ],
        text_present: ["Weld Gun 7", "PowerSource", "ControlSignal"],
        text_absent: ["null", "undefined", "NaN"],
        min_width: [[".mtk-graph-card[data-emphasis='selected']", 150]],
        unclipped: [".mtk-graph-controls button", ".mtk-graph-legend", "[data-testid='binding-graph-search']"],
      },
      render: () => graphFrame(428, <BindingGraph />),
    },
    {
      id: "state-graph-door",
      looking_for:
        "the same canvas, the same node card and the same edge language pointed at a state machine: " +
        "six states, seven transitions, the initial state marked as the initial one and the live " +
        "state marked as live — two DIFFERENT marks, because a graph where 'where it starts' and " +
        "'where it is' look alike answers neither question. Six columns do not fit a 260px dock at a " +
        "readable size, so the fit stops at its floor and the rest is reached by panning — this " +
        "canvas is 164px tall, below the height where a mini map is navigation rather than " +
        "furniture, so Fit is the affordance instead. Legible-and-navigable beats all-of-it-at-once, " +
        "and no capture here is allowed to look fine only at 2x",
      viewport: { width: 760, height: 340 },
      expect: {
        present: [
          [".react-flow__node", 6],
          [".react-flow__edge", 7],
        ],
        text_present: ["Opening", "obstruction_detected"],
        text_absent: ["null", "undefined", "NaN"],
        // The LIVE state is the one the user came to find, so it is the one that has to be on screen
        // and legible — 148px is the compact card at zoom 1, and 110 is it at the fit floor.
        min_width: [[".mtk-graph-card[data-emphasis='live']", 110]],
        // MOTION MEANS IN-FLIGHT, AND ONLY IN-FLIGHT — asserted here in the negative and in
        // `binding-graph-neighbourhood` in the positive. React Flow's `animated` flag is a CSS class
        // that dashes a wire and marches it, and the inline style from `graphEdgeStyle` cannot see
        // it: an "active" transition rendered DASHED while the legend swatch beside it, drawn from
        // the same function, rendered SOLID. A key that disagrees with its own diagram is worse than
        // no key, and nothing but this line can tell the two apart in a capture.
        absent: [".react-flow__edge.animated"],
        unclipped: [".mtk-graph-controls button", ".mtk-graph-legend"],
      },
      render: () => graphFrame(292, <StateGraph machine={DOOR_MACHINE} current="Opening" />),
    },
    {
      id: "binding-graph-narrow",
      looking_for:
        "the SAME graph in the 300 px Inspector track it actually ships in — `EditorDocks` gives it a " +
        "220 px fill under the Reveal list. A card is 208 px wide by design, which leaves 92 px for " +
        "two gutters, so this is the width where the node, the search field and the zoom pill either " +
        "fit or start eating each other (they ate each other: 30px of the `+` button). Three columns " +
        "cannot be framed here at a readable size, so the card stays readable, Fit and the pan carry " +
        "the navigation, and the chrome is whole",
      viewport: { width: 300, height: 420 },
      setup: seedBindNeighbourhood,
      expect: {
        present: [[".react-flow__node", 6]],
        // No mini map at this size, deliberately: 268px of canvas is below the floor where a
        // thumbnail is navigation rather than furniture. Fit is the affordance, and it must be whole.
        absent: [".react-flow__minimap"],
        min_width: [[".mtk-graph-card[data-emphasis='selected']", 150]],
        // 208px at the 0.75 fit floor is 156. `content-box` would draw 232, which is 174 — over this
        // ceiling and under every floor in the suite, which is exactly why the ceiling exists.
        max_width: [[".mtk-graph-card[data-emphasis='selected']", 160]],
        // The BUTTONS, not the pill. The pill is `overflow: hidden`, so it stayed whole while the
        // `Fit` button inside it was cut clean off — an assertion on the container is an assertion
        // about the container.
        unclipped: [".mtk-graph-controls button", "[data-testid='binding-graph-search']"],
        text_present: ["Fit"],
        text_absent: ["null", "undefined", "NaN"],
      },
      render: () => graphFrame(388, <BindingGraph />),
    },
    {
      id: "binding-graph-dense",
      looking_for:
        "seventeen nodes on the same canvas: the mini map has appeared (it is what makes a graph " +
        "this size navigable rather than merely large), the two columns stay in their lanes, and " +
        "every card is still READABLE — the failure this scene is written against is precisely a " +
        "column that grows until the fit has zoomed the whole thing down to nothing, which is what " +
        "it did at 0.39 before the fit had a floor",
      viewport: { width: 900, height: 620 },
      setup: seedDenseNeighbourhood,
      expect: {
        present: [
          [".react-flow__node", 17],
          [".react-flow__minimap", 1],
          // THE MINI MAP HAS TO HAVE THE GRAPH IN IT. This is not pedantry: it shipped BLANK. React
          // Flow draws each mini-map node by reading `nodeHasDimensions` on the USER node, and in a
          // controlled flow with no `onNodesChange` the measured size never gets written back — so
          // every node fails the check, the component renders its mask and nothing else, and what
          // floats over the graph is an empty white card. Every assertion in this scene passed while
          // that was true; a human reading the PNG is what found it.
          [".react-flow__minimap-node", 17],
        ],
        text_present: ["Weld Cell 12 — Controller"],
        text_absent: ["null", "undefined", "NaN"],
        // Legibility, measured on the card the user selected — which the fit always keeps centred.
        // 208px is the card at zoom 1; 156 is it at the fit floor. Before the floor existed this
        // measured 82px and the type inside it was 5px.
        min_width: [[".mtk-graph-card[data-emphasis='selected']", 150]],
        max_width: [[".mtk-graph-card[data-emphasis='selected']", 160]],
        unclipped: [".mtk-graph-controls button", ".mtk-graph-legend"],
      },
      render: () => graphFrame(588, <BindingGraph />),
    },
    ...animationGraphScenes(),
  ];
}

// ── the animation graph editor ────────────────────────────────────────────────────────────────────

/** THE ENGINE'S LARGEST GRAPH EDITOR, NEVER ONCE PHOTOGRAPHED. `AnimationGraphEditor` is a three-pane
 *  authoring surface — palette, canvas, inspector — and the single densest concentration of
 *  constitution debt in the editor: raw `<select>`/`<input>`/`<textarea>` controls outside the shared
 *  field family, a bespoke node card, and an inspector drawn at 10-11px, below the bottom of the type
 *  scale. Like the binding graph, its populated state is unreachable in jsdom (React Flow measures the
 *  pane with `getBoundingClientRect`), so a browser capture is the only thing that can see it. */
const GRAPH_SEQUENCE = "walk-cycle";
const GRAPH_WORKSPACE = animationWorkspaceKey({
  projectId: "shots",
  scope: { kind: "entity", id: "rig" },
});

const GRAPH_PRESET = createLocomotionGraphPreset(GRAPH_SEQUENCE);

const GRAPH_STATE = {
  schemaVersion: 2,
  sequenceId: GRAPH_SEQUENCE,
  revision: "rev-42",
  graph: GRAPH_PRESET,
  nodePresentation: GRAPH_PRESET.nodes.map((node) => ({
    nodeId: node.id,
    ports: animationGraphPorts(node.kind),
    readiness: node.id === "node-run" ? "warning" : "ready",
    readinessReason: node.id === "node-run"
      ? "Run clip is shorter than the blend it feeds; the tail is held."
      : "Compiled and bound.",
  })),
  sources: [
    { id: "walk-cycle", name: "Walk cycle", kind: "authored_sequence", logicalAssetId: null, revisionId: "r7", durationTick: 9_000, readiness: "ready", reason: "Authored in this project.", action: null },
    { id: "run-cycle", name: "Run cycle (imported)", kind: "imported_clip", logicalAssetId: "clip-run", revisionId: "r2", durationTick: 6_000, readiness: "ready", reason: "Imported clip, retargeted.", action: null },
    { id: "idle-breathe", name: "Idle breathing", kind: "clip_instance", logicalAssetId: "clip-idle", revisionId: "r1", durationTick: 12_000, readiness: "blocked", reason: "The source rig has no bind pose.", action: "Characterize the rig, then reimport." },
  ],
  compile: {
    state: "ready",
    authoredRevision: "rev-42",
    compiledRevision: "rev-42",
    compiledHash: "9f2c41",
    lastGoodRevision: "rev-42",
    lastGoodHash: "9f2c41",
    message: "Graph compiled.",
  },
  diagnostics: [
    {
      id: "diag-run-tail",
      severity: "warning",
      code: "sample_outside_hull",
      message: "Run sits at 1.0 with no sample beyond it, so the blend clamps instead of extrapolating.",
      fix: "Add a sample past 1.0, or lower the Run sample position.",
      nodeId: "node-locomotion-blend",
      edgeId: null,
      portId: null,
    },
  ],
} as unknown as AnimationGraphStateInfo;

const GRAPH_DEBUG = {
  graphId: GRAPH_PRESET.id,
  graphRevision: "rev-42",
  compiledHash: "9f2c41",
  instanceId: "instance-1",
  rawTick: 4_500,
  localTick: 4_500,
  activeNodes: [
    { nodeId: "node-locomotion-machine", weight: 1, localTick: 4_500, stateId: "state-moving", costMicros: 41 },
    { nodeId: "node-locomotion-blend", weight: 1, localTick: 4_500, stateId: null, costMicros: 26 },
    { nodeId: "node-walk", weight: 0.62, localTick: 4_500, stateId: null, costMicros: 11 },
    { nodeId: "node-run", weight: 0.38, localTick: 4_500, stateId: null, costMicros: 9 },
  ],
  activeEdges: [
    { edgeId: "edge-walk-blend", weight: 0.62 },
    { edgeId: "edge-run-blend", weight: 0.38 },
  ],
  transition: null,
  parameterValues: {},
  watches: [],
  eventsTruncated: false,
  evaluationCostMicros: 87,
  truncated: false,
} as unknown as AnimationGraphDebugInfo;

const animationGraphClient = () =>
  ({
    animationGraphState: () => Promise.resolve(GRAPH_STATE),
    animationGraphDebug: () => Promise.resolve(GRAPH_DEBUG),
    animationGraphSetPreviewParameters: () => Promise.resolve({ ok: true, message: "", accepted: {} }),
    animationGraphClearPreviewParameters: () => Promise.resolve({ ok: true, message: "", accepted: {} }),
  }) as unknown as EditorClient;

/** Palette and inspector are BOTH closed by default, so the scene opens them the way a user does —
 *  through the toolbar's own toggles — and then selects the state-machine node, which is the one that
 *  reveals the deepest inspector this editor has (states, transitions, conditions). */
const openGraphPanes = () => {
  const store = animationEditorStore.getState();
  store.ensure(GRAPH_WORKSPACE);
  store.updateGraph(GRAPH_WORKSPACE, (current) => ({
    ...current,
    paletteOpen: true,
    inspectorOpen: true,
    listAlternativeOpen: false,
  }));
  store.setGraphSelection(GRAPH_WORKSPACE, ["node-locomotion-machine"], []);
};

/** A DEFINITE HEIGHT, because that is what the editor actually gets. `AnimationWorkspace` puts this
 *  surface in a `minmax(0, 1fr)` row of a `height: 100%` grid, so its own `overflow: hidden` and its
 *  panes' `overflow: auto` are load-bearing. Rendered into the harness frame's `min-height: 100vh`
 *  instead, the grid would resolve `1fr` against max-content, no pane would ever scroll, and the
 *  capture would report a clipping failure the dock does not have. */
function animationGraphEditor(height: number) {
  return (
    <div style={{ display: "grid", height, minHeight: 0, background: "var(--mtk-bg-panel)" }}>
      <AnimationGraphEditor
        client={animationGraphClient()}
        sequenceId={GRAPH_SEQUENCE}
        workspaceKey={GRAPH_WORKSPACE}
      />
    </div>
  );
}

function animationGraphScenes(): Scene[] {
  return [
    {
      id: "animation-graph-authoring",
      looking_for:
        "the three-pane authoring surface at the width the Animate dock actually gives it: a node " +
        "palette that reads as a set of pickable cards rather than a wall of underlined links, a " +
        "canvas whose node cards carry the SAME anatomy as every other graph in the engine (a " +
        "small-caps role over the name, a quiet mono line under it), and an inspector built from the " +
        "shared field family at the shared type scale. Nothing here may be smaller than the body " +
        "size the type scale bottoms out at, and no control may be a raw browser widget",
      viewport: { width: 1280, height: 860 },
      setup: openGraphPanes,
      expect: {
        present: [
          [".react-flow__node", 7],
          [".react-flow__edge", 5],
          // THE CARD'S ANATOMY, ASSERTED RATHER THAN DESCRIBED. A small-caps role line above the
          // name is the whole difference between this graph and the anonymous white boxes it drew
          // before — and it is exactly what `text_present` cannot see, because the words were always
          // in the DOM, just under the name and at the same weight as it.
          [".mtk-graph-card", 7],
          [".mtk-graph-card__eyebrow", 7],
          // TWO live chips, not three: the state machine is ALSO running, and its selected ring
          // takes precedence over its live one — one card cannot wear two emphases, and "the one you
          // are editing" is the one the reader is looking for.
          [".mtk-graph-card__chip", 2],
          // Every refusal the palette can express at once — a blocked source, the operator this
          // graph already has, and the four gates that are not built. A card that is dead and says
          // nothing about why is the defect; `ChoiceCard`'s `disabledReason` is the answer, and R9
          // in the driver checks that each of these carries one.
          [".animation-graph-palette .mtk-choice[disabled]", 5],
          ["[data-testid='animation-graph-editor']", 1],
        ],
        text_present: ["Speed blend", "Locomotion states", "Final output"],
        text_absent: ["null", "undefined", "NaN"],
        unclipped: ["[data-testid='animation-graph-toolbar-actions'] button", ".mtk-graph-legend", ".mtk-graph-controls button"],
      },
      render: () => animationGraphEditor(800),
    },
  ];
}

export const SCENES: Scene[] = [
  {
    id: "icon-set-specimen",
    looking_for:
      "THE WHOLE ICON SET ON ONE SHEET — every mark the editor can draw, at the size a dock actually " +
      "uses it, on one 24-unit grid at one stroke weight. This is the capture the character era could " +
      "never produce: a glyph is sized by the text metrics of whichever font answered, so ninety marks " +
      "had ninety weights and there was no sheet to look at. What a reader is checking here is " +
      "FAMILY: no mark noticeably heavier or lighter than its neighbours, none clipped by the 24-box, " +
      "none reading as a filled blob beside outlined siblings, and every one legible at 20 px rather " +
      "than only at 2x. An empty cell is a blank control, which is the ADR-131 defect itself",
    expect: {
      // One tile per token, each holding a real drawing. `data-icon-missing` is what `Icon` stamps on
      // a name it could not resolve, so its ABSENCE is the claim: the sheet is not quietly padded
      // with holes. The count is not asserted as a number — a number goes stale the day someone adds
      // a mark, and the interesting failure is a blank tile, not a different total.
      present: [["[data-testid='icon-specimen']", 100], ["[data-testid='icon-specimen'] svg path, [data-testid='icon-specimen'] svg rect, [data-testid='icon-specimen'] svg circle, [data-testid='icon-specimen'] svg ellipse", 100]],
      absent: ["[data-icon-missing]"],
      text_absent: ["undefined", "null"],
    },
    viewport: { width: 1240, height: 1400 },
    render: () => (
      <div style={{ padding: 20, background: "var(--mtk-bg-base)", font: "var(--mtk-font-ui)" }}>
        <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(132px, 1fr))", gap: 8 }}>
          {iconTokens().map((token) => (
            <div
              key={token}
              data-testid="icon-specimen"
              style={{
                display: "flex",
                alignItems: "center",
                gap: 10,
                padding: "8px 10px",
                borderRadius: 10,
                background: "var(--mtk-bg-panel)",
                border: "1px solid var(--mtk-border-subtle)",
                color: "var(--mtk-text)",
                minWidth: 0,
              }}
            >
              <Icon name={token} size="lg" />
              <span
                style={{
                  fontSize: 11,
                  color: "var(--mtk-text-muted)",
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  whiteSpace: "nowrap",
                }}
              >
                {token}
              </span>
            </div>
          ))}
        </div>
      </div>
    ),
  },
  {
    id: "import-report-nulls",
    looking_for:
      "every row explains itself from its honesty class; NO provenance line anywhere; the word " +
      "'null' appears nowhere on screen",
    expect: {
      // Six rows, and five of them below exact — so deleting the row list, or dropping the class
      // fallback that makes a null `fix` still explain itself, fails here rather than photographing
      // a header with nothing under it.
      present: [["[data-testid='import-row']", 6], ["[data-testid='import-row-fix']", 5]],
      absent: ["[data-testid='import-row-provenance']"],
      // The defect this session fixed, as a standing assertion: a narrowing that lets `null` reach
      // the DOM prints the four characters below.
      text_absent: ["null", "undefined"],
    },
    setup: seedScene,
    render: () => <ImportReport client={client(() => Promise.resolve(report(ALL_NULL)))} />,
  },
  {
    id: "import-report-detailed",
    looking_for:
      "the part's own reason/fix/provenance replace the class defaults; the provenance line reads " +
      "'ref · strategy · format' and truncates rather than wrapping",
    expect: {
      present: [["[data-testid='import-row']", 3], ["[data-testid='import-row-provenance']", 3]],
      text_present: ["GEARBOX-A/1 · kernel-unavailable · CATPart", "Install the kernel"],
      text_absent: ["null", "undefined"],
    },
    setup: seedScene,
    render: () => <ImportReport client={client(() => Promise.resolve(report(DETAILED)))} />,
  },
  {
    id: "import-report-narrow",
    looking_for:
      "at 360 px the badge stays on the header row and the provenance line ellipsises — nothing " +
      "wraps onto a line of its own (the ADR-120 defect class)",
    width: 360,
    expect: {
      present: [["[data-testid='import-row']", 3], ["[data-testid='import-row-provenance']", 3]],
      text_absent: ["null", "undefined"],
    },
    setup: seedScene,
    render: () => <ImportReport client={client(() => Promise.resolve(report(DETAILED)))} />,
  },
  {
    id: "import-report-empty",
    looking_for:
      "a non-CAD project: the panel renders NOTHING rather than an empty frame, so it never " +
      "clutters a scene it has nothing to say about",
    // The one scene that is legitimately a flat frame, and therefore the one that needs its claim
    // written down hardest: "renders nothing" and "the fetch threw" look identical in a PNG. The
    // sentinel is rendered by the SCENE, after the client resolves, so a rejected promise fails.
    expect: {
      absent: ["[data-testid='import-report']"],
      present: [["[data-testid='empty-sentinel']", 1]],
    },
    setup: seedScene,
    render: () => (
      <>
        <ImportReport client={client(() => Promise.resolve(report([])))} />
        <EmptySentinel client={client(() => Promise.resolve(report([])))} />
      </>
    ),
  },
  {
    id: "reveal-cad-names",
    looking_for:
      "north-star #1 with the names a CAD import really produces: every candidate row shows its " +
      "name AND its '· match NN', the greyed row shows the whole reason, and no row runs past the " +
      "panel",
    expect: {
      present: [
        ["[data-testid='candidate']", 3],
        ["[data-testid='greyed']", 1],
        ["[data-testid='bound']", 1],
      ],
      // The affinity is the RANKING — a row that shows a name and loses its score is a row the user
      // cannot rank by, which is the whole point of the surface.
      text_present: ["· match 92", "· match 40", "not electrical power"],
      text_absent: ["null", "undefined", "NaN"],
    },
    setup: selectCadEntity,
    render: () => <Reveal client={revealClient(CAD_REVEAL)} />,
  },
  {
    id: "reveal-cad-names-narrow",
    looking_for:
      "the same reveal in a 320 px drawer — the width a side panel actually collapses to. The score " +
      "must survive the squeeze, because a candidate list without its ranking is just a list",
    width: 320,
    expect: {
      present: [
        ["[data-testid='candidate']", 3],
        ["[data-testid='greyed']", 1],
      ],
      text_present: ["· match 92", "not electrical power"],
      text_absent: ["null", "undefined", "NaN"],
    },
    setup: selectCadEntity,
    render: () => <Reveal client={revealClient(CAD_REVEAL)} />,
  },
  {
    id: "diagnostics-one-click-fix",
    looking_for:
      "'every no explained' with its one-click fix: the warn row states what is missing and the Bind " +
      "button names the target it would bind — both inside a 320 px drawer",
    width: 320,
    expect: {
      present: [
        ["[data-testid='diag-fix']", 1],
        ["[data-testid='diag-greyed']", 1],
      ],
      // The button must still SAY what it will bind. A fix control that has lost its object is the
      // <ux_quality> rule-1 failure: the control that starts an action no longer owns its outcome.
      text_present: ["Bind to", "Needs"],
      text_absent: ["null", "undefined", "NaN"],
    },
    setup: selectUnwiredEntity,
    render: () => <Diagnostics client={revealClient(CAD_REVEAL)} />,
  },
  {
    id: "diagnostics-one-click-fix-wide",
    looking_for:
      "the SAME row with room: badge, message and fix on ONE line. The wrap that rescues 320 px must " +
      "not fire at a width where everything already fits, or the fix traded one defect for another",
    expect: {
      present: [["[data-testid='diag-fix']", 1]],
      text_present: ["Bind to", "Needs"],
      text_absent: ["null", "undefined", "NaN"],
      // The whole point of this scene, and the reason it exists beside the 320 px one.
      same_line: [["[data-testid='diag-row'] > span", "[data-testid='diag-fix']"]],
    },
    setup: selectUnwiredEntity,
    render: () => <Diagnostics client={revealClient(CAD_REVEAL)} />,
  },
  {
    id: "animation-timeline-tracks",
    looking_for:
      "the timeline with something on it: four track rows under the ruler, each with its name, its " +
      "key count and its mute/lock pair in a header column that stays put, and its keys as diamonds " +
      "on a gridded lane. The playhead is a line THROUGH the lanes with a handle on the ruler, the " +
      "muted track is visibly muted and the locked one is visibly locked. EXACTLY ONE row carries a " +
      "badge — the one that needs attention. Three green `ready` pills on three healthy rows is four " +
      "accent colours inside a 178px column all saying that nothing is wrong, and the constitution " +
      "spends accent only where interaction requires attention; the readiness word is still on the " +
      "row as `data-binding-state` and in the track name's tooltip. The transport above is the shared " +
      "floating pill, the same object PhysicsPanel scrubs its recording with",
    width: 1000,
    setup: selectAnimatedEntity,
    expect: {
      present: [
        ["[data-testid='animation-track']", 4],
        ["[data-testid='animation-key']", 11],
        ["[data-testid='animation-marker']", 2],
        ["[data-testid='animation-event']", 1],
        // The row that DOES need attention keeps its badge. Without this the rule above would be
        // satisfied by a timeline that had simply stopped rendering badges at all.
        ["[data-binding-state='preview_only'] .mtk-badge", 1],
      ],
      // ...and no healthy row has one. `absent` is the only half of this claim a capture cannot make
      // on its own: a missing pill and a pill that is merely off-screen photograph identically.
      absent: ["[data-binding-state='ready'] .mtk-badge"],
      // The marker names are in their `title`/`aria-label`, not painted on the 30px lane, so the
      // claim is the count above plus the sequence and track names that ARE painted.
      text_present: ["Transform · Position Y", "Weld pass"],
      // The transport row is the one that used to collide: seven items at one gap on a strip that
      // scrolled without a scrollbar. Every control on it must be whole and on screen.
      unclipped: [
        "[data-testid='animation-transport'] .mtk-btn",
        "[data-testid='animation-transport'] .mtk-select",
        "[data-testid='animation-timeline-toolbar'] .mtk-btn",
        "[data-testid='animation-timeline-toolbar'] .mtk-select",
        "#animation-contexts .mtk-dock-tab",
        "#animation-surfaces .mtk-dock-tab",
      ],
      // AND THEY NAME THREE DIFFERENT EDITORS, so shrinking them in step is not a yield. `Timeline`,
      // `Curves` and `Graph` sat at the strip's 44px `min-width` floor reading `Ti… Cu… Gr…` in the
      // dock at a 1280 window — every rule above green, because a tab at its floor with nothing
      // clipped is what those rules are about. The caption beside them is the one thing on the row
      // that may be cut, and it is excluded here by selector rather than by exemption.
      untruncated: ["#animation-contexts .mtk-dock-tab__label", "#animation-surfaces .mtk-dock-tab__label"],
      // The time read-out sits ON the transport row rather than above or below it — a timecode that
      // wraps to its own line is the row having run out of width.
      same_line: [
        ["[data-testid='animation-time']", "[data-testid='animation-play']"],
        ["[data-testid='animation-time']", "[data-testid='animation-scrub']"],
      ],
      text_absent: ["null", "undefined", "NaN"],
    },
    render: () => <AnimationWorkspace client={animationClient()} />,
  },
  {
    id: "animation-curve-editor",
    looking_for:
      "the curve view, which nothing in this repository had ever photographed. What it was: a blue " +
      "polyline and some circles on an unruled white rectangle — a picture of a SHAPE, with no way " +
      "to see that two keys are level and no way to know which direction is 'more'. What it is: the " +
      "shared CurveCanvas, with a graticule at the same ten divisions the timeline ruler labels and " +
      "both axes NAMED, exactly as the reference's easing panel labels acceleration against time. " +
      "Four keys of Position Y, cubic, sampled through the same interpolation the engine plays back",
    width: 1000,
    setup: selectAnimatedEntity,
    // Selecting a track first is not decoration: the curve view has nothing to draw without one, and
    // a scene that skipped this would photograph the "Select a numeric track" empty state under a
    // caption describing a curve.
    click: ["[data-testid='animation-track'] .mtk-timeline__track-select", "#animation-surfaces-curves-tab"],
    expect: {
      present: [
        ["[data-testid='animation-curves']", 1],
        ["[data-testid='animation-curve-path']", 1],
        // The graticule is the whole point of the change. Eleven columns and five rows.
        [".mtk-curve__grid line", 16],
        [".mtk-curve__axis", 2],
      ],
      // The empty states this scene must NOT be photographing.
      text_absent: ["Select a numeric track", "no numeric curve", "null", "undefined", "NaN"],
      text_present: ["value", "time", "Transform · Position Y"],
      unclipped: [".mtk-curve__svg", ".mtk-curve__axis"],
      // The vertical axis label is beside the plot, not above it — a rotated caption that has wrapped
      // onto its own row is the grid having taken the width the label needed.
      same_line: [[".mtk-curve__axis--value", ".mtk-curve__svg"]],
      stacked: [[".mtk-curve__svg", ".mtk-curve__axis--time"]],
    },
    render: () => <AnimationWorkspace client={animationClient()} />,
  },
  {
    id: "physics-recorded-transport",
    looking_for:
      "THE SECOND CONSUMER, and the reason the timeline framework is a framework rather than a " +
      "rename. Physics records frames and lets you scrub them, and it had built that surface by " +
      "hand: a `<label>`, a bare range stretched across a nowrap row, and a 12px mono `frame " +
      "128/300` pinned to its right — no play control next to the scrubber, no keyboard step, and a " +
      "read-out styled nothing like the 15px semibold one the animation transport uses for the same " +
      "job. It is the SAME `Transport` pill now, with the same read-out rhythm and the same attached " +
      "button run, plus single-frame stepping that `simScrub` has always been able to serve and the " +
      "surface simply never offered",
    width: 1000,
    // `tl` starts at zero and only refreshes once the panel believes something is running, so the
    // recording has to be reached the way a user reaches it rather than injected.
    click: ["#simToggle"],
    expect: {
      present: [
        ["[data-testid='sim-transport']", 1],
        ["[data-testid='frameLbl']", 1],
        ["[data-testid='simStepBack']", 1],
        ["[data-testid='simStepForward']", 1],
        ["[data-testid='scrub']", 1],
      ],
      text_present: ["128", "300 frames"],
      text_absent: ["null", "undefined", "NaN"],
      unclipped: ["[data-testid='sim-transport'] .mtk-btn", "[data-testid='frameLbl']"],
      // The four parts of a transport are ONE row. This is the claim the hand-rolled version could
      // not have made, because its play control was in a different block from its scrubber.
      same_line: [
        ["[data-testid='frameLbl']", "#simToggle"],
        ["#simToggle", "[data-testid='scrub']"],
      ],
    },
    render: () => <PhysicsPanel client={physicsClient()} />,
  },
  ...rigScenes(),
  ...poseScenes(),
  ...graphScenes(),
  ...stateMachineScenes(),
  ...inspectorScenes(),
  ...assetScenes(),
  ...modelScenes(),
  ...pipeForgeScenes(),
  ...gameplayScenes(),
  ...shellScenes(),
];

// ── the inspector ─────────────────────────────────────────────────────────────────────────────────

/** THE PANEL THIS HARNESS HAD NEVER PHOTOGRAPHED (ADR-136).
 *
 *  Twenty-eight scenes, four of them the whole shell, and in every one of them the Inspector reads
 *  "Select an entity to inspect." — so the editor's PRIMARY property surface, the one every other
 *  panel's numbers eventually route through, had no capture at all. What the first one showed is why
 *  that matters: three different row anatomies on one panel (a bespoke `.mtk-field-row` for numbers, a
 *  `DisclosureSection` for groups, and `@jsonforms/vanilla-renderers`' unstyled `.control` for
 *  everything else), and typed controls keyed on components the core has never registered.
 *
 *  THE ENTITY BELOW IS THE CORE'S OWN VOCABULARY, not the dev mock's. `Transform{px..sz}`,
 *  `MeshRenderer{mesh,material,castShadows}`, `RigidBody{kind,mass}` and `Joint{kind,bodyA,bodyB}` are
 *  exactly what `core/src/stdlib.rs` registers, down to the `format` on the two body references —
 *  which is what makes this a capture of the real product rather than of the mock. A scene that seeded
 *  `Material`/`Targeting` would photograph a payload the core cannot produce, which is the C6 failure
 *  reached through a screenshot instead of a test. `check-registry-vocab.mjs` is the gate that keeps
 *  the two in step; this is the picture of them being in step. */
function inspectorSetup(open: string[], extra: Record<string, Record<string, unknown>> = {}) {
  return () => {
    // The disclosure remembers its own state, and only `Transform` opens by default — so a capture of
    // the other groups needs them opened the way a returning user has them, through the same key the
    // component reads. Clicking them instead would work and would also photograph a panel mid-
    // interaction, with the pointer's own focus ring in the frame.
    for (const label of open) {
      window.localStorage.setItem(`metrocalk:disclosure:inspector-component:${encodeURIComponent(label)}`, "open");
    }
    projectionStore.getState().bulkLoad([
      { id: "e-mast", name: "Mast", parentId: null, components: { Transform: { px: 0, py: 0, pz: 0 } } },
      { id: "e-counterweight", name: "Counterweight", parentId: null, components: { Transform: { px: -4, py: 0, pz: 0 } } },
      {
        id: "e-boom",
        name: "Boom Arm",
        parentId: null,
        components: {
          Transform: { px: 2.5, py: 1.2, pz: -0.75, rx: 0, ry: 45, rz: 0, sx: 1, sy: 1, sz: 1 },
          MeshRenderer: { mesh: "sha256:9f3c81aa4e07", material: "sha256:1ab77d5c90e2", castShadows: true },
          RigidBody: { kind: "dynamic", mass: 1250 },
          Joint: { kind: "revolute", bodyA: "e-mast", bodyB: "e-counterweight" },
          ...extra,
        },
      },
    ] as never);
    projectionStore.getState().select("e-boom");
  };
}

/** A client that records rather than sends. `setField` is the only method these scenes can reach. */
function recordingClient(record: (entry: string) => void) {
  return {
    setField: (id: string, component: string, field: string, value: unknown) =>
      record(`${id}.${component}.${field}=${JSON.stringify(value)}`),
  } as unknown as EditorClient;
}

/** The dock track the Inspector actually lives in, at a window wide enough that the ≤760px stacking
 *  rule is NOT in play — the two regimes are separate scenes because they are separate layouts, and a
 *  capture that conflates them proves neither. The width is stated once, here. */
function dockTrack(children: ReactNode) {
  return (
    <div data-testid="inspector-track" style={{ width: 320, borderLeft: "1px solid var(--mtk-border)" }}>
      {children}
    </div>
  );
}

function inspectorScenes(): Scene[] {
  return [
    {
      id: "inspector-real-vocabulary",
      looking_for:
        "the Inspector, populated, for the first time in this repository — and populated with the " +
        "vocabulary `core/src/stdlib.rs` actually registers. Every row is one anatomy (label left, " +
        "control right, unit after the number, reset only where the value differs from a declared " +
        "default); `Joint.bodyA`/`bodyB` are ENTITY PICKERS naming Mast and Counterweight, which is " +
        "the first time that control has ever been able to fire; `MeshRenderer.mesh` is a monospaced " +
        "asset reference rather than a box inviting you to type a content hash; and `RigidBody.kind` " +
        "and `Joint.kind` are the closed vocabularies the core states in `ui_hint`, not free text",
      // TALL ENOUGH TO HOLD ALL FOUR GROUPS OPEN, because `unclipped` below is only worth asserting
      // where the rows are actually on screen: the first run of this scene was 900px and the harness
      // reported the Joint group 210px past the bottom edge, which is true and is about the WINDOW,
      // not about the panel. In the shell the track scrolls; here the window is made to fit so that
      // every row can be measured rather than half of them excused.
      viewport: { width: 1200, height: 1200 },
      setup: inspectorSetup(["MeshRenderer", "RigidBody", "Joint"]),
      expect: {
        present: [
          // 9 Transform + 3 MeshRenderer + 2 RigidBody + 3 Joint.
          ["[data-testid='prop-row']", 17],
          // px/py/pz metres, rx/ry/rz degrees, sx/sy/sz multiples, and `RigidBody.mass` in kilograms —
          // TEN, not nine. An adversarial review caught the miscount, and it matters more than an
          // off-by-one: `present` is at-least, so a scene that under-counts is a scene that would stay
          // green with a unit deleted. A unit the core states in prose and the editor never showed.
          ["[data-testid='prop-unit']", 10],
          // THE THREE CONTROLS THAT COULD NOT FIRE BEFORE. Two entity references and one asset
          // reference — the core declares eight such fields and the editor routed none of them.
          //
          // ANCHORED ON WHAT EACH CONTROL *IS*, not on its id. Every control in the panel derives its
          // id the same way (`controlId(path)`), so `#mtk-prop-MeshRenderer-mesh` alone is satisfied by
          // the plain `StringControl` this fix replaced — the review's sharpest finding, and it would
          // have left the headline "no renderer for `format: asset`" claim asserted by nothing. The
          // MONO face is `AssetRefControl`'s and no other control here asks for it, so the compound
          // selector fails the moment the asset renderer is removed. `bodyA`/`bodyB`/`kind` are rescued
          // by the `<option>` text below, which only a `<select>` can produce.
          ["#mtk-prop-Joint-bodyA", 1],
          ["#mtk-prop-Joint-bodyB", 1],
          ["#mtk-prop-MeshRenderer-mesh.mtk-input--mono", 1],
          ["#mtk-prop-MeshRenderer-material.mtk-input--mono", 1],
          ["#mtk-prop-RigidBody-kind", 1],
          // px 2.5, py 1.2, pz -0.75 and ry 45 differ from their declared defaults, so four rows offer a
          // reset. The COUNT cannot say the other five must not, because `present` is at-least — an
          // unconditional reset on all seventeen rows satisfies it. The two `absent` entries below are
          // what actually state it, and they are stated per row, on values that ARE their default.
          ["[data-testid='prop-reset']", 4],
          ["[aria-label='Reset Position X to 0']", 1],
        ],
        // ONE ANATOMY, AND THE HONEST ACCOUNT OF WHAT THIS CAN AND CANNOT SEE. The first four are
        // `@jsonforms/vanilla-renderers`' class names; every boolean, plain string and vocabulary field
        // in this panel rendered through them, and `check-class-hooks.mjs` cannot see them at all
        // because they are emitted from `node_modules`. What this does NOT catch — measured, not
        // assumed — is `...vanillaRenderers` being spread back into the array: its testers rank 1–4
        // against 3–10 for the set here, so every control still resolves to a Metrocalk one and the
        // page is unchanged. What it DOES catch is the case that matters: a field shape no custom
        // tester matches, reaching a fallback. Today that shape renders JSON Forms' "No applicable
        // renderer found." instead, which `text_absent` names. `.mtk-field-row` is the retired row and
        // its rule is deleted, so `check-class-hooks` would reject it returning before a capture could.
        //
        // A RESET ON A ROW THAT IS ALREADY ITS DEFAULT is the claim the count above cannot make.
        // `Rotation X` is 0 against a default of 0 and `Scale X` is 1 against 1; each is named by the
        // aria-label its own reset would carry, so an unconditional reset fires here even though every
        // `present` entry still passes.
        absent: [
          ".control", ".input", ".select", ".checkbox", ".mtk-field-row",
          "[aria-label='Reset Rotation X to 0']",
          "[aria-label='Reset Scale X to 1']",
        ],
        // `Mast`, `Counterweight`, `dynamic` and `revolute` are `<option>` TEXT, so a claim about them
        // is a claim about what a reader can see. The asset reference is an `<input value=…>` and is
        // deliberately NOT claimed here: `text_present` reads `textContent`, which an input's value is
        // not part of — so a scene can prove the asset CONTROL is present (`#mtk-prop-MeshRenderer-mesh`
        // above) and cannot prove what it displays. That is a real limit of this harness rather than a
        // limit of the panel. Closing gate: a `value_present` claim beside `text_present` in
        // `shoot.mjs`; owner is whoever next has that file free, since a concurrent lane holds it.
        text_present: [
          "Position X", "Rotation Y", "Scale Z", "Cast shadows", "Mass",
          "Mast", "Counterweight", "dynamic", "revolute",
        ],
        // A ROW IS A ROW at this width: the label and the control it names share a line. Anchored on
        // one known control rather than "the first `.mtk-property-row__label`", because `same_line`
        // reads the first match of each selector and two unrelated first-matches can overlap by luck.
        same_line: [["label[for='mtk-prop-Transform-px']", "#mtk-prop-Transform-px"]],
        // Every row whole inside a 320px track. The retired anatomy spent a fixed 92px on the label
        // and had no actions column at all, so this is the measurement that says the new one fits.
        unclipped: ["[data-testid='prop-row']"],
        // `No applicable renderer found` is the sentence JSON Forms paints when a uischema element
        // resolves to no renderer. It is not a crash and not a console error — the panel renders it, in
        // red, and carries on — so nothing but a claim about the text can see it. It is how this
        // session found that dropping `vanillaRenderers` had also dropped the LAYOUTS.
        text_absent: ["null", "undefined", "NaN", "[object Object]", "No applicable renderer found"],
      },
      render: () => dockTrack(<Inspector client={recordingClient(() => {})} />),
    },
    {
      id: "inspector-narrow-window",
      looking_for:
        "the SAME panel below 760px, where the design system's property row stacks its label above " +
        "its control so the value keeps the full width. The inspector never had this rule — its own " +
        "row was a flex line with a hard 92px label at every width — and inheriting it is most of " +
        "what moving onto `PropertyRow` buys. The reset stays on the control's line, not the label's",
      // The WINDOW is the subject here, not a frame width: `@media (max-width: 760px)` reads the
      // viewport, so capping the frame inside a 620px window would photograph a panel that had already
      // stacked for a reason the caption does not name. Tall for the same reason as the scene above —
      // a stacked row is nearly twice the height of a side-by-side one, which is the trade.
      viewport: { width: 620, height: 1400 },
      setup: inspectorSetup(["MeshRenderer"]),
      expect: {
        // SCOPED TO THE OPEN GROUPS, and the scoping is the finding. `DisclosureSection` keeps its
        // content MOUNTED when closed (`unmountOnClose={false}`, so a collapsed section costs no
        // re-render and keeps scroll position), which means a bare `[data-testid='prop-row']` counts
        // five rows nobody can see and then reports them as clipped — correctly, because they are.
        // A claim about what a reader can reach has to say which rows those are.
        present: [
          ["[data-state='open'] [data-testid='prop-row']", 12],
          ["[data-state='open'] [data-testid='prop-unit']", 9],
        ],
        absent: [".control", ".input", ".select", ".checkbox", ".mtk-field-row"],
        // THE DUAL OF THE SCENE ABOVE, and the reason both exist. The same two elements that must
        // share a line at 1200px must be on separate lines here, label first. A stylesheet that lost
        // the media query passes one of these scenes and fails the other, which is exactly the
        // discrimination a single capture cannot give.
        stacked: [["label[for='mtk-prop-Transform-px']", "#mtk-prop-Transform-px"]],
        unclipped: ["[data-state='open'] [data-testid='prop-row']"],
        text_present: ["Position X", "Cast shadows"],
        text_absent: ["null", "undefined", "NaN", "No applicable renderer found"],
      },
      render: () => <Inspector client={recordingClient(() => {})} />,
    },
    {
      id: "inspector-mount-is-not-an-edit",
      looking_for:
        "MOUNTING THE INSPECTOR EMITS NOTHING. `emitChanges` diffs JSON Forms' data against the " +
        "projection so the mount-time `onChange` is a no-op — a contract this panel has always " +
        "claimed in a comment and never checked anywhere. It matters now because the curated schema " +
        "declares `default` for the first time (that is what the row's reset reverts to), and " +
        "JSON Forms' validator is entitled to fill a default into `data` before the first render. " +
        "If it ever does, the panel writes a transaction nobody asked for, the document is dirty " +
        "before it is touched, and the undo stack opens with an edit the user did not make",
      viewport: { width: 620, height: 1100 },
      setup: inspectorSetup([]),
      expect: {
        // AND THE PANEL MUST ACTUALLY HAVE RENDERED. An adversarial review found this scene passing
        // vacuously: its only claims were the probe's own wrapper and an empty log, so an Inspector
        // that rendered NOTHING — a crashed selection, a deleted `<JsonForms>` block, the empty state —
        // emits no transaction either and photographs as green. "It did not write" is only evidence
        // when paired with "it did render", so the nine open Transform rows are claimed here too.
        present: [
          ["[data-testid='mount-edits']", 1],
          ["[data-state='open'] [data-testid='prop-row']", 9],
        ],
        text_present: ["0 transactions since mount", "Position X"],
        // The failure states, spelled out: any non-zero count, and the specific fields whose declared
        // defaults are the ones a validator would inject.
        text_absent: [
          "1 transaction", "2 transactions", "3 transactions", "undefined", "NaN",
          "No applicable renderer found", "No editable properties yet",
        ],
      },
      render: () => <EditProbe />,
    },
    {
      id: "inspector-reset-writes-the-default",
      looking_for:
        "THE RESET ACTUALLY RESETS. The same probe, with the first reset in the sheet clicked — " +
        "`Position X`, which is 2.5 against a declared default of 0. One transaction, and it is " +
        "`Transform.px=0`: the value the schema declares, on the field whose row was clicked, through " +
        "the same `setField` path a typed edit takes. Without this scene the whole reset affordance " +
        "is proven by the presence of a button, and a button that emits nothing photographs the same",
      viewport: { width: 620, height: 1100 },
      setup: inspectorSetup([]),
      // The FIRST reset in the document is Position X's — Transform is the group that opens by
      // default and `px` is its first field. Clicking by testid rather than by position in a list
      // would be no more specific: they all carry the same one, which is why the assertion below
      // names the field the click must have reached.
      click: ["[data-testid='prop-reset']"],
      expect: {
        present: [["[data-testid='edit-log']", 1]],
        text_present: ["1 transaction since mount", "e-boom.Transform.px=0"],
        // Not two transactions, and not the wrong field: a reset that also nudged its neighbours, or
        // one wired to the row below it, would still print "a transaction" and look right.
        text_absent: ["2 transactions", "Transform.py", "Transform.ry", "undefined", "NaN"],
      },
      render: () => <EditProbe />,
    },
  ];
}

/** Mounts the real Inspector and RENDERS the transactions it emitted.
 *
 *  A scene cannot assert about a callback, so the callback's effect is put on screen — which also
 *  makes it something a reader can check in the PNG, the bar this harness sets for everything else.
 *  Two scenes use it and they are duals: one asserts the list is EMPTY after mounting, the other
 *  clicks a reset and asserts exactly what it wrote. Without the second, "the row has a reset button"
 *  is all that is ever proven, and a button that emits nothing looks identical in a screenshot. */
function EditProbe() {
  const [log, setLog] = useState<string[]>([]);
  const [settled, setSettled] = useState(false);
  // One frame after mount: JSON Forms validates and may fill defaults during its own first effect, so
  // reading synchronously during render would report a number taken before the risk had happened.
  useEffect(() => {
    const t = setTimeout(() => setSettled(true), 0);
    return () => clearTimeout(t);
  }, []);
  return (
    <div style={{ width: 320 }}>
      <div
        data-testid="mount-edits"
        style={{ padding: "8px 12px", font: "12px var(--mtk-font-ui)", color: "var(--mtk-text-secondary)" }}
      >
        {settled ? `${log.length === 1 ? "1 transaction" : `${log.length} transactions`} since mount` : "settling…"}
        {log.length > 0 && <div data-testid="edit-log" style={{ fontFamily: "var(--mtk-font-mono)" }}>{log.join(" · ")}</div>}
      </div>
      <Inspector client={recordingClient((e) => setLog((l) => [...l, e]))} />
    </div>
  );
}

// ── the pose preview ──────────────────────────────────────────────────────────────────────────────

/** THE ONE CAPTURE THAT SHOWS THE ANIMATION ITSELF. Every other scene in this file photographs a
 *  panel; this one photographs a POSE — five skeletons drawn from coordinates that came out of the
 *  real `bind_sequence → sample → Skeleton::globals` path (`character/tests/pose_preview_fixture.rs`
 *  regenerates them and fails if they drift).
 *
 *  It is the claim made visible: ONE clip, keyed to humanoid bones rather than to a skeleton, moving
 *  two characters that share no bone name, are not the same height, and do not even rest in the same
 *  pose — with no retarget asset, no bone mapping and no user step between them. In Unreal that
 *  arrangement costs three authored assets; in Unity the clip silently T-poses.
 *
 *  A still preview is also the feature, not just the evidence: both incumbents make "did the retarget
 *  work" a question you answer by pressing play and looking at the character. */
function poseScenes(): Scene[] {
  return [
    {
      id: "pose-preview-one-clip-two-rigs",
      looking_for:
        "five stick figures at ONE shared scale: the Mixamo rig at rest and animated, the taller " +
        "A-posed Unreal rig at rest and animated by the SAME clip, and the retargeted result. Both " +
        "animated figures must visibly differ from their own rest pose — arms up — and the Unreal " +
        "figures must be visibly TALLER, because a shared scale is what makes the comparison honest",
      viewport: { width: 900, height: 620 },
      expect: {
        // 5 figures x 18 bones. Drop a figure, or stop drawing bones, and this goes red rather than
        // photographing a caption over an empty box.
        present: [
          ["[data-testid='pose-figure']", 5],
          ["[data-testid='pose-bone']", 90],
          ["[data-testid='pose-caption']", 5],
          // THE TINT, ASSERTED. 8 left-side bones per figure x 5 figures. The first version of this
          // preview inferred the side from the joint name in TypeScript, matched `upperarm_l` and not
          // `LeftArm`, and drew the two Mixamo figures entirely untinted under a caption promising
          // otherwise — every assertion above stayed green. Only reading the PNG caught it.
          // 7 a side per figure (shoulder · upper arm · lower arm · hand · upper leg · lower leg ·
          // foot) x 5 figures. The hips are the root and draw no segment; the spine, chest, neck and
          // head are the centre column and are deliberately untinted.
          ["[data-testid='pose-bone'][data-side='left']", 35],
          ["[data-testid='pose-bone'][data-side='right']", 35],
        ],
        text_present: [
          "Raise arms",
          "humanoid",
          "4/4 channels bound on mixamo",
          "4/4 channels bound on unreal",
          "nothing to report",
          "no retarget asset, no mapping, no user step",
          // The last two figures are pixel-identical and the panel now says why. Asserted because the
          // sentence is conditional on a MEASUREMENT (`routesAgree`): a capture that stopped showing
          // it would mean the two routes had separated, which is a defect, not a copy change.
          "identical on purpose",
        ],
        // Every figure must be entirely on screen — a preview whose last figure is past the edge has
        // silently deleted the comparison it exists to make.
        unclipped: ["[data-testid='pose-figure']"],
        text_absent: ["null", "undefined", "NaN"],
      },
      render: () => <PosePreview doc={POSE_PREVIEW as PoseDocument} />,
    },
  ];
}

// ── the rig panel ─────────────────────────────────────────────────────────────────────────────────

/** THE CHARACTER-ANIMATION CLAIM, PHOTOGRAPHED. The engine's differentiator against Unreal and Unity
 *  is that a character's humanoid characterization is INFERRED at import rather than authored — no IK
 *  Rig per skeleton, no IK Retargeter per pair, no Avatar configuration screen. That claim is only
 *  worth anything if the answer is legible the moment the panel opens, and legibility is exactly the
 *  property a passing unit test cannot report on.
 *
 *  BOTH DOCUMENTS BELOW ARE REAL. They are not hand-written mocks: `skeleton/tests/rig_contract.rs`
 *  generates them from `metrocalk_skeleton::characterize` and fails if the committed JSON and the Rust
 *  output ever disagree. So a scene here cannot photograph a payload the core is incapable of
 *  producing — the C6 failure this file's own header warns about, reached through a screenshot. */
function rigScenes(): Scene[] {
  return [
    {
      id: "rig-recognized",
      looking_for:
        "a Mixamo character opens ALREADY CHARACTERIZED: the headline names the convention and says " +
        "nothing needs mapping, every bone row shows the source joint it matched AND why, and the two " +
        "tail bones are listed as KEPT — the bones Unity's humanoid enum silently discards",
      expect: {
        // 21 mapped bones, each with its joint name and its evidence sentence. Delete the evidence
        // column — the thing that makes a wrong row visible without clicking it — and this goes red.
        present: [
          ["[data-testid='rig-row']", 21],
          ["[data-testid='rig-row-joint']", 21],
          ["[data-testid='rig-row-evidence']", 21],
          ["[data-testid='rig-extra-joint']", 2],
        ],
        // The headline claim, and the preservation claim, in the words the user actually reads.
        text_present: [
          "Recognized as a",
          "Mixamo",
          "Nothing to map by hand",
          "Ready to retarget",
          "mixamorig:LeftUpLeg",
          "Tail_01",
        ],
        // A ROW IS A ROW. The bone's name and the joint it matched must share a line — the two grid
        // columns side by side. The joint name and its evidence sentence deliberately STACK beneath it
        // (a `mixamorig:` name plus a convention sentence does not fit one 620 px line, and forcing it
        // would ellipsise away the half that makes a wrong row visible), so the claim is about the
        // columns, not the stack. The first version of this scene asserted the stack shared a line and
        // this gate rejected it — which is the point of writing the claim down beside the scene.
        same_line: [["[data-testid='rig-row-bone']", "[data-testid='rig-row-joint']"]],
        text_absent: ["null", "undefined", "NaN"],
      },
      render: () => <RigPanel doc={RIG_MIXAMO as RigDocument} />,
    },
    {
      id: "rig-recognized-narrow",
      looking_for:
        "the same rig in a 320 px drawer — the width a side dock actually collapses to. Bone names are " +
        "long (`mixamorig:RightShoulder`) and MUST ellipsise rather than wrap: a wrapped row turns a " +
        "21-row table nobody has to read into a wall nobody does (the ADR-120 defect class)",
      width: 320,
      expect: {
        present: [
          ["[data-testid='rig-row']", 21],
          ["[data-testid='rig-row-joint']", 21],
        ],
        text_present: ["Mixamo", "Ready to retarget"],
        text_absent: ["null", "undefined"],
      },
      render: () => <RigPanel doc={RIG_MIXAMO as RigDocument} />,
    },
    {
      id: "rig-not-retargetable",
      looking_for:
        "the FAILURE state, which is the one that has to be legible: a rig with no limbs says it is not " +
        "retargetable, NAMES all 12 missing required bones, and carries the fix beside the complaint — " +
        "against Unity's Avatar screen, whose only failure indicator is a red cross with no sentence",
      expect: {
        present: [
          ["[data-testid='rig-diagnostic']", 1],
          ["[data-testid='rig-diagnostic-fix']", 1],
          ["[data-testid='rig-missing-required']", 1],
        ],
        text_present: [
          "Not retargetable",
          "Left Upper Arm",
          "Assign each one in the rig panel",
          "12 required bone(s)",
        ],
        // The status is the first thing read, so it must sit ON the header row rather than wrapping
        // under the title — the badge is the answer to "can I animate this character at all".
        same_line: [["[data-testid='rig-title']", "[data-testid='rig-status-badge']"]],
        // The complaint and its fix are two sentences: the twelve-name list has no terminator, so
        // inline they read as one run-on line whose only seam is a colour change. This is the claim
        // that could not be written until `stacked` existed.
        stacked: [
          ["[data-testid='rig-diagnostic-message']", "[data-testid='rig-diagnostic-fix']"],
        ],
        // THE CONTRADICTION THAT ONLY A CAPTURE CAUGHT. Every assertion above passed while the
        // headline congratulated the user with "Nothing to map by hand" directly over a blocking
        // diagnostic naming twelve bones to map by hand. Nothing in `present`/`text_present` could
        // see it — the defect was that two true sentences disagreed — so the reading of the PNG is
        // what found it, and this is the line that stops it coming back.
        text_absent: ["null", "undefined", "Nothing to map by hand"],
      },
      render: () => <RigPanel doc={RIG_BLOCKED as RigDocument} />,
    },
  ];
}

// ── the shell, composed ───────────────────────────────────────────────────────────────────────────

/** THE ONE THING THE PANEL SCENES CANNOT SEE. Every scene above photographs a panel *in isolation* —
 *  deliberately, so a capture says something about the panel and nothing about its neighbours. The
 *  cost of that isolation was logged as owed the day the harness landed: **the invariants cannot see
 *  a panel colliding with a sibling**, because no capture has ever contained two.
 *
 *  These scenes contain all of them. `App` takes no props and falls back to the in-process MockCore
 *  outside Tauri, so the whole editor — Engines rail, left dock, stage, Inspector, header, bottom
 *  dock — mounts and lays itself out for real. R1–R4 then apply across the composition for nothing:
 *  the sibling collision R3 was written for is finally in frame.
 *
 *  AND THEY MAKE THE STAGE RULE A MEASUREMENT. `<ux_quality>` 5 — "the stage gets layout priority;
 *  panels yield/collapse on resize; the stage never collapses first" — is a product principle, and
 *  the only thing asserting it is `layout.test.ts`, which checks that `panelLayout(w)` returns the
 *  *string* `"340px minmax(320px, 1fr) 300px"`. That is a statement of intent. Whether the stage is
 *  320 px wide once the docks hold real content is a different question, it is the one the user
 *  experiences, and jsdom cannot be asked it: a dock whose content will not shrink pushes the stage
 *  under its floor while `panelLayout` keeps returning exactly the string the unit test wants.
 *
 *  One scene per layout regime `panelLayout` defines, at a width INSIDE each band rather than on its
 *  edge — a scene pinned to the breakpoint value tests the comparison operator, not the layout. */
/** A `function`, not a `const` arrow — like `shellScenes` below, and for the same reason. The first
 *  draft made it an arrow and the whole bundle died on load with `Cannot access 'shell' before
 *  initialization`: hoisting `shellScenes` moved the *call* above `SCENES` but left the `const` it
 *  closes over in its temporal dead zone. Worth recording because of how it surfaced — the driver
 *  reads its registry off `window.__MTK_SHOTS__` in the built bundle, so a module that throws on
 *  load reports **zero scenes** and fails the run. The version of this driver that regexed
 *  `scenes.tsx` for `id:` lines would have found eight ids in a file that cannot execute. */
function shell(
  id: string,
  /** The window. A bare number is a width at the default 900px height — the shape every scene had
   *  when height was a thing nobody had asked a question about. `[w, h]` names both, and it exists
   *  because the regime that was broken is a SHORT window: a fixed 900 is one more number stated once
   *  and then never varied, which is how an entire axis goes unwatched. */
  size: number | [width: number, height: number],
  looking_for: string,
  expect: Expect,
  click?: string[],
): Scene {
  const [width, height] = typeof size === "number" ? [size, 900] : size;
  return {
    id,
    looking_for,
    viewport: { width, height },
    click,
    expect: {
      ...expect,
      present: [["[data-testid='viewport']", 1], ...(expect.present ?? [])],
      // The stage's protected floor, measured. STAGE_MIN is imported from the layout module rather
      // than typed as 320 here: a floor written down twice is a floor that only moves in one place.
      // It is the floor on BOTH axes — one number, not two, and the honest reading of "the stage is
      // sacred": a stage 320px wide and 40px tall has not kept its floor in any sense a user would
      // recognise. `--mtk-stage-min` on `.mtk-stage-column` is set from this same constant, so the
      // dock's `max-height` and this assertion cannot disagree about what the floor is.
      min_width: [["[data-testid='viewport']", STAGE_MIN], ...(expect.min_width ?? [])],
      // The vertical floor is NOT a `min_height` entry, and the difference is the whole of what
      // ADR-126 left owed. `min_height` states a flat number, and below a ~443px window the number
      // 320 is not a rule the layout is breaking — it is arithmetic the window cannot satisfy, with
      // the chrome and the dock's bar already spoken for. A gate that is wrong in one regime gets a
      // waiver in that regime, and then the waiver gets used for the first real defect. `stage_floor`
      // caps the floor by what the column actually has left, measured, so the claim is true
      // everywhere and no scene needs an exemption.
      stage_floor: STAGE_MIN,
      ...(expect.min_height ? { min_height: expect.min_height } : {}),
      // AND THE PANEL THAT YIELDED IS STILL WHOLE. The stage-is-sacred rule has two halves and only
      // one of them was ever written down: the stage keeps its floor, AND what gives way gives way
      // by getting smaller — not by being cut off behind an `overflow: hidden` that shows no
      // scrollbar. Asserting only the first half certifies the wrong repair, and this is not a
      // hypothetical: state the floor on the viewport itself (`minHeight: STAGE_MIN` inline, where
      // the viewport's layout already lives) and leave the dock unyielding, and every stage-floor
      // assertion here goes **green** while the dock loses **38px at 1440×700, 92px at 640 and 242
      // of its 321px at 480** — a quarter of a panel, with its own tab strip in the part that was
      // cut away. Measured, not reasoned: `shell-dock-short` passed under exactly that mutation
      // before this line existed.
      //
      // Universal rather than per-scene, unlike `.mtk-dock-tab`: every shell scene has exactly one
      // bottom dock in every layout regime, open or closed, and there is no width or height at
      // which part of it is meant to be off screen. That is what makes it a rule and not a claim
      // with an owner.
      unclipped: ["[data-testid='bottom-dock']", ...(expect.unclipped ?? [])],
      text_absent: ["undefined", "NaN", ...(expect.text_absent ?? [])],
    },
    render: () => <App />,
  };
}

/** A function, not a `const`, purely so the shell scenes can be *read* after the panel scenes while
 *  still being spliced into the array above them — hoisting is the only thing buying that order. */
function shellScenes(): Scene[] {
  return [
  shell(
    "shell-wide",
    1440,
    "the whole editor at a desktop width: Engines rail · left dock · stage · Inspector, all four " +
      "tracks open at once. This is the first capture in the repository that contains two panels, " +
      "so it is the first one where a panel can be caught colliding with its neighbour",
    {
      present: [
        ["[data-testid='engine-rail']", 1],
        ["[data-testid='hierarchy']", 1],
        ["[data-testid='editor-header']", 1],
      ],
      // Wide open: the docks are panels, not rails. If this ever flips, the layout collapsed at a
      // width where it had room — the opposite defect to the stage being squeezed.
      absent: ["[data-testid='rail-left']"],
    },
  ),
  shell(
    "shell-compact",
    1100,
    "below 1200 the open docks take their compact widths (300/260) and stay open. The stage must " +
      "have absorbed the difference, not the other way round",
    {
      present: [
        ["[data-testid='engine-rail']", 1],
        ["[data-testid='hierarchy']", 1],
      ],
      absent: ["[data-testid='rail-left']"],
    },
  ),
  shell(
    "shell-rails",
    900,
    "below 980 both docks collapse to icon rails so the stage keeps the space — the yield step " +
      "that the stage-priority rule exists to produce. The Engines rail stays: an index you have to " +
      "open a drawer to reach is not an index",
    {
      present: [
        ["[data-testid='rail-left']", 1],
        ["[data-testid='rail-right']", 1],
        ["[data-testid='engine-rail']", 1],
      ],
      // The panels are gone, not merely narrow — the rail IS the collapsed state.
      absent: ["[data-testid='hierarchy']"],
    },
  ),
  shell(
    "shell-overlay",
    600,
    "below 620 the shell is one column of stage, with both docks reachable as header-opened " +
      "drawers. Even here the stage holds its floor — the layout that gives up the viewport to keep " +
      "a panel is the failure this whole rule is written against",
    {
      present: [["[data-testid='editor-header']", 1]],
      absent: ["[data-testid='rail-left']", "[data-testid='rail-right']", "[data-testid='hierarchy']"],
    },
  ),

  // THE STATE THE GRID TEMPLATE SAYS IS FINE AND THE WINDOW SAYS IS NOT. Two clicks at 1000 px —
  // open the Inspector rail, press "Pin this panel open" — and the four tracks come to
  // 132 + 300 + 320 + 260 = 1012 in a 1000 px window. `layout.test.ts` is green throughout, and
  // correctly so: it asserts the STRING `dockGridColumns` returns, and the string is exactly what it
  // is supposed to be. It has no way to add the numbers up against a window it cannot see. What the
  // browser does with that template is push the Inspector 12 px off the right edge of the screen,
  // where it cannot be read, reached or scrolled to — the stage held its floor and the docks, which
  // are the things the rule says must yield, did not.
  shell(
    "shell-pinned-inspector",
    1000,
    "both docks open at 1000 px — the width where the fixed tracks plus the stage's protected floor " +
      "add up to more than the window. Nothing may be painted past the right edge: the rule is that " +
      "the PANELS yield, and a panel that keeps its width by leaving the screen has not yielded",
    {
      present: [["[data-testid='inspector-dock']", 1]],
    },
    ["[data-testid='rail-right'] button", 'button[aria-label="Pin Inspector dock"]'],
  ),

  // THE AXIS NOTHING HAD EVER LOOKED AT. Every scene above — and every panel scene before them — is
  // captured at one window HEIGHT, and the bottom dock is closed in all of them, so the shell's
  // vertical stack has never been composed under pressure. It does not survive it. The dock's height
  // is a `vh` clamp with an absolute minimum (320px for Model, 330px for Animate) that has never
  // known the header and status bar exist, and below it sat a viewport at `min-height: 0` — so the
  // stage paid the entire difference. On HEAD, measured: **282px of stage at 1440×700**, 18px at 420,
  // and **zero at 400** with the dock still holding all 321 of its pixels; at 360 the dock ran 17px
  // off the bottom of the screen with its own tab strip down there.
  //
  // These three open the dock (a click, like the left dock's workspaces) at the heights where the
  // parts stop fitting. They are the vertical twin of `shell-pinned-inspector`, and D3's sentence
  // applies unchanged: adding it up against the window is the part nothing was doing.
  shell(
    "shell-dock-model",
    [1440, 700],
    "the Model workspace open on an ordinary laptop window. The dock's preferred height here is " +
      "48vh = 336px against 619px of stage column — it must give back what does not fit, because " +
      "the rule is that the PANELS yield",
    {
      present: [["[data-testid='bottom-dock']", 1]],
      // The dock is open, not merely present: an assertion that passes on the closed 42px bar would
      // be green for the one state that was never broken.
      min_height: [["[data-testid='bottom-dock']", 64]],
    },
    ["[data-testid='bottom-dock-toggle']"],
  ),
  shell(
    "shell-dock-animate",
    [1440, 640],
    "Animate — the tallest workspace the dock has (a 330px absolute minimum) on the shortest window " +
      "that still leaves room for it. The one where the dock's own floor and the stage's floor are " +
      "closest to colliding",
    {
      present: [["[data-testid='bottom-dock']", 1]],
      min_height: [["[data-testid='bottom-dock']", 64]],
    },
    [
      "[data-testid='bottom-workspace-summary']",
      "[data-testid='bottom-workspace-option-animation']",
      "[data-testid='bottom-dock-toggle']",
    ],
  ),
  // SEVEN WORKSPACES ARE AUTHORED AND FIVE OF THEM WERE REACHABLE. 1280 is the width where the
  // bottom dock is at its narrowest with both side docks open (508px), and on HEAD the strip needed
  // 610 of it: `Runtime` measured **0 of its 92px** — not narrow, not truncated, absent — with
  // `Problems` down to 48. The strip is `overflow-x: auto` with `scrollbar-width: none`, which is
  // what let R1 and R2 both wave it through: each exempts a scrollable ancestor on the grounds that
  // "the user can pan to it", and the stylesheet had removed the only thing that says so.
  //
  // Every `.mtk-dock-tab` in frame, not just the dock's — the Animate workspace's own two strips are
  // here as well, and they were losing `UI` and `Graph` to the same mechanism.
  shell(
    "shell-dock-tabs",
    [1280, 900],
    "the bottom dock open at the width where its tab strip is narrowest. All seven workspace tabs " +
      "must be entirely on screen: a strip that pans without a scrollbar deletes its last tabs in " +
      "silence, and Runtime — the live simulation diagnostics — was the one being deleted",
    {
      present: [["[data-testid='bottom-dock']", 1], [".mtk-dock-tab", 7]],
      unclipped: [".mtk-dock-tab"],
      // AND EVERY LABEL THAT IS DRAWN IS DRAWN WHOLE. `unclipped` proved the tabs were REACHABLE and
      // stopped there, so the strip was free to shrink all seven in step until they read
      // `M… I… Fo… An… L… Pro… Ru…` — legal targets, none clipped, and no way to tell Model from
      // Import. A hidden label measures 0 against 0 and passes, which is the point: below the width
      // where the labels fit, the unselected tabs become icons and the current one keeps its name.
      untruncated: [".mtk-dock-tab__label"],
      // The current workspace is NAMED at every width — the one tab that may not become a square.
      min_width: [[".mtk-dock-tab[aria-selected='true']", 70]],
      text_present: ["Runtime"],
    },
    [
      "[data-testid='bottom-workspace-summary']",
      "[data-testid='bottom-workspace-option-animation']",
      "[data-testid='bottom-dock-toggle']",
    ],
  ),

  // THREE WINDOW HEIGHTS, AND THEY ARE THREE DIFFERENT RULES. `dockForm()` divides the vertical axis
  // into regimes the way `panelLayout` divides the horizontal one, and a scene inside each is the only
  // way a threshold that drifts turns something red. One scene would pin whichever side it happened to
  // land on and leave the other free to move — the same argument as "a scene pinned to the breakpoint
  // value tests the comparison operator, not the layout", one axis over.
  shell(
    "shell-dock-docked",
    [1440, 640],
    "640px tall — just INSIDE the regime where the dock is still a track below the stage. Both " +
      "floors hold here (320px of stage, a 188px workspace) and the dock yields the difference, so " +
      "the sheet must NOT have taken over: this is the scene that turns red if the threshold drifts " +
      "upward and starts floating a dock that had room to sit down",
    {
      present: [["[data-testid='bottom-dock'][data-dock-form='docked']", 1]],
      // The stage keeps its own controls when it is not under a sheet — the other half of the claim
      // below, and the thing that makes the withdrawal a rule instead of a disappearance.
      absent: ["[data-dock-form='sheet']"],
      min_height: [["[data-testid='viewport-tool-rail']", 1]],
    },
    ["[data-testid='bottom-dock-toggle']"],
  ),

  shell(
    "shell-dock-short",
    [1440, 480],
    "480px tall with a workspace open — the window where the dock cannot be a track at all. Before " +
      "ADR-126 the stage measured 78px here; after it the stage was right and the WORKSPACE was 37px " +
      "of its 188, falling to 1px below 440. The dock is now a sheet OVER the stage: the stage keeps " +
      "its whole column, the workspace gets a real content box, and the stage's own tool rail is " +
      "withdrawn rather than left underneath to steal clicks",
    {
      present: [["[data-testid='bottom-dock'][data-dock-form='sheet']", 1]],
      // The defect this whole slice is about, stated as a measurement: the workspace has a content
      // box a user can read. At HEAD this measured 37px, and 1px below a 440px window.
      min_height: [[".mtk-bottom-dock__content", 188]],
      // R3 caught these sharing pixels with the dock's tab strip in the run meant to prove the sheet
      // worked. At 400px window height ALL FIVE transform tools and the viewport toolbar are covered,
      // so they are withdrawn — a covered control is worse than an absent one, and this is the line
      // that fails if one of them comes back underneath the sheet.
      // `onboarding` is the third one, and it is here because a human looked at the PNG. Every
      // assertion in this scene passed while the first-run card was painted across the workspace's
      // own description — R3 compares controls and what the card covered was prose, so nothing said
      // anything. It is the same rule as the two above: a stage overlay withdraws when the stage is
      // under a sheet, rather than floating above the thing the user just opened.
      absent: [
        "[data-testid='viewport-tool-rail']",
        "[data-testid='vptoolbar']",
        "[data-testid='onboarding']",
      ],
    },
    ["[data-testid='bottom-dock-toggle']"],
  ),

  shell(
    "shell-dock-floor",
    [1440, 420],
    "420px tall with the dock CLOSED — the regime ADR-126 named as owed and could not represent. " +
      "The chrome (81px), the stage's 320px floor and the dock's 42px bar want more room than the " +
      "window has, so the floor is not being violated by a greedy panel, it is unreachable. The " +
      "shell's answer is the one the horizontal axis already gives: the stage absorbs the remainder. " +
      "What is still owed, and is asserted here, is that it absorbs ALL of it — the bar is whole and " +
      "on screen, the stage is exactly what the bar left, and nothing is cut between them",
    {
      present: [["[data-testid='bottom-dock'][data-dock-form='sheet']", 1]],
      // The dock is CLOSED here, so it is its bar and still a track: the sheet form only spends
      // itself on `.is-open`. Conservation is the claim; see `Expect.fills`.
      fills: [[".mtk-stage-column", ["[data-testid='viewport']", "[data-testid='bottom-dock']"]]],
      // And the seven workspaces are still reachable from it — the bar is not decoration.
      unclipped: ["[data-testid='bottom-dock-toggle']"],
    },
  ),

  // THE FOUR OTHER THINGS THE LEFT DOCK CAN BE. The dock is a 300 px track between 980 and 1199 —
  // the narrowest it ever gets while still open — and which workspace is inside it is a click. The
  // scenes above photograph the default one, so on their own they would gate a fifth of the surface
  // and leave the rest exactly as unwatched as they were. `layout.ts` already carries the scar of
  // this class in a comment: the terrain workspace overflowed its dock the moment it moved out of
  // the Inspector, and the repair was to widen the track by hand after a human noticed.
  ...(
    [
      ["build", "Build — place and create: the toolbar, the asset browser and the describe bar stacked in one 300 px column", "[data-testid='authbar']"],
      ["terrain", "Terrain — the widest workspace in the dock, and the one that has already overflowed it once", "[data-testid='terrain-presets']"],
      ["physics", "Physics — numeric read-outs and a contact list, the shape most likely to demand width", "[data-testid='dropBall']"],
      ["gameplay", "Gameplay — roles, cinema and VFX stacked in a column narrower than any of them", "[data-testid='match-panel']"],
    ] as const
  ).map(([engine, blurb, marker]) =>
    shell(
      `shell-${engine}`,
      1000,
      `${blurb}. Photographed at 1000 px, where the dock is at its narrowest that is still open`,
      {
        present: [[marker, 1]],
        // The stage floor is the point: a dock that will not yield takes the difference out of the
        // viewport, and this is the width where it has the least room to hide.
        min_width: [["[data-testid='left-dock']", 1]],
      },
      [`[data-testid='engine-${engine}']`],
    ),
  ),
  ];
}

// ── the asset library ─────────────────────────────────────────────────────────────────────────────

/** THE OTHER PANEL THIS HARNESS HAD NEVER PHOTOGRAPHED (ADR-144), and the last of the constitution's
 *  six migration gates.
 *
 *  These scenes render against `createMockSession()` — the SAME client the dev build and every shell
 *  scene use — rather than a fixture written here. That is deliberate and it is the whole reason these
 *  captures are worth anything: the dev mock used to answer `catalog()` with two items under the keys
 *  `Health` and `UI`, which are neither the canonical buckets the real command sends nor a payload with
 *  a price, a capability or a marketplace tier anywhere in it. A scene with its own fixture would have
 *  photographed a beautiful library that nothing in the product could produce. The mock mirrors
 *  `core/src/stdlib.rs` and `core/src/marketplace.rs`'s `builtin_catalog` now, so what is on screen is
 *  what the engine actually publishes — and `text_absent: ["std:"]` is the standing assertion that the
 *  canonical bucket key never reaches a reader again. */
function assetScenes(): Scene[] {
  const library = () => <AssetBrowser client={createMockSession()} />;

  /** Types into the panel's REAL search field after mount, through the same `input` event a keystroke
   *  produces, so the searched state is reached by the code path a user reaches it by. A scene cannot
   *  type — `click` is the only gesture the driver has — and an `initialQuery` prop would be production
   *  API shaped by the camera. Throwing when the field is missing is the point: a silent no-op would
   *  photograph the browse view under a caption claiming a search. */
  function SearchedLibrary({ query }: { query: string }) {
    useEffect(() => {
      const input = document.getElementById("assetSearch");
      if (!(input instanceof HTMLInputElement)) throw new Error("no #assetSearch to type into");
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
      setter?.call(input, query);
      input.dispatchEvent(new Event("input", { bubbles: true }));
    }, [query]);
    return library();
  }

  return [
    {
      id: "asset-library",
      looking_for:
        "the asset library as LARGE PREVIEWS on a grid, at the 300px width the Build dock gives it. " +
        "Six collections, each headed by a word a person would say — the shipped panel printed the " +
        "canonical map key and filed things under `std:Props`. Every tile carries its tier, the " +
        "capability that decides whether it will attach, and a price where there is one; the three " +
        "marketplace entries are the real `builtin_catalog` ones, at their real prices",
      width: 300,
      expect: {
        present: [
          ["[data-testid='asset-tile']", 12],
          ["[data-testid='asset-category']", 6],
          ["[data-testid='asset-price']", 3],
          ["[data-testid='asset-filter']", 2],
          // THE MARK IS THE ITEM'S WHERE THE ICON SET KNOWS IT. Written as a claim rather than left to
          // the eye, because the first build drew the SOURCE tier's icon and every local item in the
          // library rendered the same glyph — a grid of "large previews" that previewed nothing, and a
          // capture that looks perfectly fine until you notice all six marks are identical. Three
          // distinct drawings on one screen is the smallest statement that the resolution is per-item.
          ["[data-icon='camera']", 1],
          ["[data-icon='light']", 1],
          ["[data-icon='character']", 2],
        ],
        // The defect the rebuild exists to fix, as a standing claim: the engine's own namespace must
        // not appear in user copy. `null`/`undefined` are the harness's standing pair.
        text_absent: ["std:", "null", "undefined"],
        text_present: ["Props", "Characters", "Marketplace", "Rusty Medieval Sword"],
        // A "large preview" is a measurement, not an adjective. Two columns inside 300px minus the
        // panel's own padding is ~124px a tile, and the preview is a square inside it — so a tile that
        // has quietly gone back to being a 40px row fails here rather than being described as one.
        min_width: [["[data-testid='asset-tile']", 110]],
        min_height: [["[data-testid='asset-tile']", 150]],
      },
      render: library,
    },
    {
      id: "asset-library-shelf",
      looking_for:
        "the two collections the catalog cannot supply: Favourites and Recently placed, above the " +
        "buckets. Both are shortcuts into the same grid, so a starred item appears twice — once in " +
        "its collection and once where it is filed — and the star on both instances is lit",
      width: 300,
      setup: () => {
        // Through the store rather than through storage: the shelf reads `localStorage` when its module
        // is evaluated, which has already happened by the time a scene's `setup` runs.
        assetShelfStore.getState().toggleFavourite("marketplace:forge:rusty-sword");
        assetShelfStore.getState().toggleFavourite("local:Light");
        assetShelfStore.getState().recordPlacement("local:Transform");
        assetShelfStore.getState().recordPlacement("marketplace:acme:companion-drone");
      },
      expect: {
        present: [
          ["[data-testid='asset-category'][data-category='favourites']", 1],
          ["[data-testid='asset-category'][data-category='recent']", 1],
        ],
        text_present: ["Favourites", "Recently placed"],
        text_absent: ["std:", "null", "undefined"],
        // Shortcuts belong ABOVE the thing they are a shortcut into, or they are just more catalog.
        stacked: [
          ["[data-testid='asset-category'][data-category='favourites']", "[data-testid='asset-category'][data-category='recent']"],
          ["[data-testid='asset-category'][data-category='recent']", "[data-testid='asset-category'][data-category='std:Props']"],
        ],
      },
      render: library,
    },
    {
      id: "asset-library-search",
      looking_for:
        "a search replaces the collections with ONE ranked Matches grid — the same resolver the " +
        "describe bar uses, not a second search path. The filter chips stay, because a filter over a " +
        "result set is the same question as a filter over the library",
      width: 300,
      expect: {
        present: [["[data-testid='asset-results']", 1], ["[data-testid='asset-tile']", 1]],
        absent: ["[data-testid='asset-collections']", "[data-testid='asset-seam']"],
        text_present: ["Matches", "Rusty Medieval Sword"],
        text_absent: ["std:", "null", "undefined"],
      },
      render: () => <SearchedLibrary query="sword" />,
    },
    {
      id: "asset-library-no-match",
      looking_for:
        "nothing matches, and the panel offers a CONTROL rather than asking a question. The shipped " +
        "version printed a coloured sentence ending in a question mark with nothing to press, which is " +
        "`<ux_quality>` 1 exactly: the decisive step offloaded to a passive line. The empty state also " +
        "says what generating costs before it is pressed",
      width: 300,
      expect: {
        present: [["[data-testid='asset-seam']", 1], ["[data-testid='asset-generate']", 1]],
        absent: ["[data-testid='asset-tile']"],
        text_present: ["zzz", "costs tokens"],
        text_absent: ["std:", "null", "undefined"],
        // The offer is a real target, not a link-sized afterthought inside a sentence.
        min_height: [["[data-testid='asset-generate']", 28]],
      },
      render: () => <SearchedLibrary query="zzz" />,
    },
    {
      id: "asset-library-wide",
      looking_for:
        "the SAME grid given room: the column count is the browser's decision from a minimum tile " +
        "width, so a wider surface gains columns instead of stretching two tiles into billboards. The " +
        "failure this is written against is a preview that grows to half the panel because the grid " +
        "was told how many columns to have instead of how narrow a tile may get",
      width: 560,
      expect: {
        present: [["[data-testid='asset-tile']", 12]],
        text_absent: ["std:", "null", "undefined"],
        // A tile may not simply absorb the extra width: at 560 the grid must have added columns, so no
        // tile is anywhere near half the frame.
        max_width: [["[data-testid='asset-tile']", 200]],
      },
      render: library,
    },
  ];
}

/** Renders only once the same call the panel makes has RESOLVED. Without it, "the panel chose to
 *  render nothing" and "the reply never arrived" are the same two-colour frame — which an
 *  adversarial review turned into a passing capture over a rejecting client. */
function EmptySentinel({ client: c }: { client: EditorClient }) {
  const [ready, setReady] = useState(false);
  useEffect(() => {
    let live = true;
    c.cadReport().then(() => live && setReady(true));
    return () => {
      live = false;
    };
  }, [c]);
  return ready ? <span data-testid="empty-sentinel" style={{ position: "fixed", left: -9999 }} /> : null;
}

// ── the Model workspace ───────────────────────────────────────────────────────────────────────────

/** THE ONLY SUBSYSTEM IN THE EDITOR THAT STILL SHIPPED ITS OWN STYLESHEET (ADR-147).
 *
 *  `src/panels/AssetLabPanel.css` was 374 declarations — more first-party CSS outside `theme/` than
 *  every other panel in the repository put together, and the constitution's flat prohibition is
 *  *"No subsystem is allowed to invent its own styling."* It had never been photographed: no scene
 *  in this registry mounted `AssetLabPanel`, so a stage rail, seven forms, a metric grid, an issue
 *  list and a bake-evidence strip were all shaped by rules nothing but that one file ever read.
 *
 *  It is captured at the size the Model dock actually is — `.mtk-bottom-dock.is-open.is-asset` is
 *  `clamp(320px, 48vh, 520px)` tall and the FULL window wide. That combination is the whole point:
 *  every earlier judgement about this panel was made reading source, where a single stacked column
 *  looks tidy, and it is exactly the shape a wide short band punishes hardest. */
function modelScenes(): Scene[] {
  const asset = {
    id: "gun-7",
    name: "Weld Gun 7",
    source: "Imported STEP",
    revision: "sha256:41c9e0",
  };
  const bakeSources = [
    asset,
    { id: "mdp", name: "Main Distribution Panel MDP-1", source: "Imported STEP" },
    { id: "girder", name: "Overhead Crane Assembly Rev C — Long Travel Girder", source: "Imported STEP" },
  ];
  const inspected: AssetLabReportView = {
    summary: "Audited 1 mesh with 148,204 triangles. Three findings need a decision before optimization.",
    sourceMetrics: [
      { id: "tri", label: "Triangles", value: 148_204, description: "Triangle count of the source mesh." },
      { id: "vert", label: "Vertices", value: 81_930, description: "Unique vertex count after attribute splits." },
      { id: "shells", label: "Connected shells", value: 37, description: "Disconnected surface components." },
      { id: "uv", label: "UV coverage", value: 62.4, unit: "%", description: "Fraction of the UV page occupied by charts." },
      { id: "mat", label: "Materials", value: 4, description: "Distinct bound material slots." },
      { id: "bounds", label: "Bounding box", value: "1.24 × 0.83 × 0.41", unit: "m", description: "Axis-aligned extents in scene units." },
    ],
    issues: [
      { id: "i1", severity: "error", status: "open", title: "Non-manifold edges", detail: "Two faces share an edge with inconsistent winding, which breaks normal repair and baking.", count: 12 },
      { id: "i2", severity: "warning", status: "open", title: "Overlapping UV charts", detail: "Overlapping islands make a projected bake ambiguous over the affected texels.", count: 3 },
      { id: "i3", severity: "info", status: "accepted", title: "Unused vertex colours", detail: "A colour attribute is present but no bound material reads it." },
      { id: "i4", severity: "pass", status: "fixed", title: "Tangent basis", detail: "MikkTSpace tangents are present and consistent with UV0." },
    ],
    warnings: ["Two materials share one texture page; optimizing them separately will duplicate it."],
  };

  const scene = (
    id: string,
    stage: AssetLabStage,
    report: AssetLabReportView | null,
    looking_for: string,
    expect: Expect,
    viewport = { width: 1240, height: 520 },
  ): Scene => ({
    id,
    looking_for,
    expect,
    viewport,
    // The dock modelled as the dock actually is, because the shape is the finding. `.mtk-bottom-dock.
    // is-open.is-asset` is `clamp(320px, 48vh, 520px)` tall with `overflow: hidden auto` on its
    // content — a BOUNDED box the panel must lay itself out inside, not the unbounded column a
    // `minHeight: 100vh` frame would hand it. Photographed unbounded, a panel that overflows its dock
    // by 175px looks like a tidy stack; photographed bounded, the primary action is simply not there.
    render: () => (
      // `.mtk-bottom-dock__content` (bounded, `overflow: hidden auto`) wrapping `.mtk-bottom-workspace`
      // (`height: 100%`, its own scroll) — the real two-element chain, because the panel's header, tab
      // rail and footer are only pinned if the panel FILLS the workspace rather than growing past it.
      <div style={{ height: "100vh", overflow: "hidden auto", background: "var(--mtk-bg-panel)" }}>
        <div className="mtk-bottom-workspace mtk-scroll">
        <AssetLabPanel
          asset={asset}
          report={report}
          activeStage={stage}
          bakeSources={bakeSources}
          availability={report ? undefined : { inspect: { state: "available", reason: "Asset audit is available." } }}
          onRun={() => {}}
        />
        </div>
      </div>
    ),
  });

  return [
    scene(
      "model-inspect-evidence",
      "inspect",
      inspected,
      "the Model workspace at the size its dock really is — 1240 wide, 520 tall — showing the densest " +
      "state it has: a measured source, six metrics, four findings and a warning. What a reader is " +
      "checking is PROPORTION and DENSITY: whether the seven stages, the evidence and the primary " +
      "action can be read at a glance in a wide short band, or whether the panel is one narrow column " +
      "of 10px type stacked past the fold with the width unused",
      {
        present: [
          ["[data-testid='asset-lab']", 1],
          ["[data-testid='asset-lab-metrics']", 1],
          ["[data-testid='asset-lab-inspect']", 1],
        ],
        text_present: ["Weld Gun 7", "Non-manifold edges", "148,204"],
        text_absent: ["null", "undefined", "NaN"],
        // The primary action is the whole point of the stage; a button below the fold of a 520px dock
        // is a workflow with no visible verb.
        unclipped: ["[data-testid='asset-lab-inspect']"],
      },
    ),
    scene(
      "model-bake-controls",
      "bake",
      inspected,
      "the most control-heavy stage in the engine — two selects, a source list of three checkboxes, " +
      "three map checkboxes, an advanced disclosure and a primary action — in a 520px band. Every " +
      "control here should be the same component family the rest of the editor uses, at the same size, " +
      "with the same hit target; the shipped version drew its checkboxes, its fieldsets and its notes " +
      "from a stylesheet no other panel reads",
      {
        present: [
          ["[data-testid='asset-lab']", 1],
          ["[data-testid='asset-lab-bake']", 1],
        ],
        text_present: ["High-detail sources", "Ambient occlusion", "Texture resolution"],
        text_absent: ["null", "undefined", "NaN"],
        unclipped: ["[data-testid='asset-lab-bake']"],
      },
    ),
    scene(
      "model-narrow",
      "inspect",
      inspected,
      "the SAME workspace below the 860px the rail is worth: the stage list turns back into a " +
      "horizontal strip along the top and the evidence takes the whole width, because a 190px column " +
      "is a fifth of a 760px panel. What a reader is checking is that nothing had to be given up for " +
      "it — every stage still reachable, the metrics still readable rather than ellipsised, and the " +
      "primary action still pinned where it was at 1240",
      {
        present: [
          ["[data-testid='asset-lab-stages'] [role='tab']", 7],
          ["[data-testid='asset-lab-metrics']", 1],
        ],
        text_present: ["148,204"],
        text_absent: ["null", "undefined", "NaN"],
        // NOT `text_present: ["Weld Gun 7"]`, and the difference matters: claims read `textContent`,
        // which contains a `display: none` node's words in full. The shared narrow-width rule used to
        // hide `.mtk-workspace-panel__subtitle` — so the asset name was in the DOM, in the claim, and
        // nowhere on the screen. A MEASUREMENT is the claim that cannot be satisfied by hidden text.
        min_height: [[".mtk-workspace-panel__subtitle", 12]],
        // The claim the wide scene makes, at the width where a footer is most likely to lose it.
        unclipped: ["[data-testid='asset-lab-inspect']"],
        // Every stage reachable — a horizontal strip that scrolls past the edge is the pattern this
        // redesign replaced, so it must not come back through the responsive door.
        same_line: [["#asset-lab-stages-inspect-tab", "#asset-lab-stages-export-tab"]],
      },
      { width: 760, height: 560 },
    ),
  ];
}

// ── the gameplay workspace ────────────────────────────────────────────────────────────────────────

/** THE CATALOGS THE PACKAGED EDITOR ACTUALLY SERVES, not the ones the dev mock does. `MockClient`
 *  answers `roleCatalog` with 4 of the engine's 10 roles and `cinemaCatalog`/`vfxCatalog` with `[]`,
 *  because cinematics and effects are `.exe`-only commands — so a capture taken against the mock
 *  would photograph a Gameplay column with two empty sections and call it the panel. That is the C6
 *  failure (green against the mock, different against `/core`) reached through a screenshot.
 *
 *  These rows are copied from `editor-shell/src/role_intent.rs`, `cinema_intent.rs` and
 *  `vfx_intent.rs`. They are a SECOND statement of those catalogs and will drift; what they are for
 *  is proportion — how a ten-card grid lays out in a 300px dock — and a drifted label still answers
 *  that. The behaviour of the cards is tested against the real client in `RolesSection.test.tsx`. */
const GAMEPLAY_ROLES: RoleSpec[] = [
  { kind: "collectible", label: "Collectible", blurb: "Spins; vanishes and scores when something touches it", adds: "spin animation · touch trigger · pickup rule · +1 on the Score counter" },
  { kind: "solid", label: "Solid obstacle", blurb: "An immovable body other things collide with", adds: "fixed physics body · auto-fit collider" },
  { kind: "prop", label: "Physics prop", blurb: "Falls, rolls and collides under gravity", adds: "dynamic physics body · auto-fit collider" },
  { kind: "spinner", label: "Spinner", blurb: "Turns forever — ambient motion", adds: "looping spin animation" },
  { kind: "companion", label: "Companion", blurb: "Follows your props, patrols your waypoints, fights your enemies", adds: "dynamic physics body · auto-fit collider · a live brain (follow / patrol / attack)" },
  { kind: "enemy", label: "Enemy", blurb: "Companions attack it; it falls when struck", adds: "dynamic physics body · auto-fit collider · the defeat rule · +1 Score when beaten" },
  { kind: "waypoint", label: "Waypoint", blurb: "A patrol stop — companions with nothing to follow walk the chain in order", adds: "a numbered patrol marker (no physics)" },
  { kind: "vanishing", label: "Vanishing", blurb: "Disappears a few seconds into the run — a crumbling platform, a fuse, a timed gate", adds: "a countdown · the vanish rule" },
  { kind: "hazard", label: "Hazard", blurb: "Hurts whoever walks into it — spikes, lava, a falling rock", adds: "the hurt rule — it damages the TOUCHER, not itself" },
  { kind: "player", label: "Player", blurb: "YOU, during Play — drive it with the arrow keys or WASD", adds: "dynamic physics body · auto-fit collider · live keyboard control while playing" },
];

const GAMEPLAY_SHOTS: ShotSpec[] = [
  { kind: "establish", label: "Establishing", blurb: "Show where we are before we look at anything closely", adds: "a wide, slowly pulling-out shot from the front" },
  { kind: "hero", label: "Hero shot", blurb: "The workhorse — three-quarters on, pushing in", adds: "a full-body three-quarter shot that creeps closer" },
  { kind: "closeup", label: "Close-up", blurb: "Tight and still — for the moment that matters", adds: "a close, locked-off shot in profile" },
  { kind: "orbit", label: "Show it off", blurb: "Circle the object so every side reads", adds: "a medium shot orbiting a quarter turn" },
  { kind: "reveal", label: "Crane reveal", blurb: "Lift away to show the world around it", adds: "a full shot craning upward" },
  { kind: "looming", label: "Looming", blurb: "From below, so the subject towers", adds: "a low-angle medium shot pushing in" },
];

const GAMEPLAY_EFFECTS: EffectSpec[] = [
  { kind: "fire", label: "Fire", blurb: "It burns — rising, flickering, warm", adds: "a rising flame that cools from white-hot to ember red", burst: false },
  { kind: "smoke", label: "Smoke", blurb: "Dark, slow, expanding — pair it with fire", adds: "a slow dark plume that spreads as it climbs", burst: false },
  { kind: "sparkle", label: "Sparkle", blurb: "Magic, treasure, something worth walking toward", adds: "bright motes orbiting the object", burst: false },
  { kind: "explosion", label: "Explosion", blurb: "One shot — a violent outward burst", adds: "a one-shot blast of hot debris that falls away", burst: true },
  { kind: "sparks", label: "Sparks", blurb: "One shot — metal on metal, a hit landing", adds: "a one-shot spray of fast sparks that arc and drop", burst: true },
  { kind: "pickup", label: "Pick-up pop", blurb: "One shot — the little flourish a collectible deserves", adds: "a one-shot ring of bright motes rising and fading", burst: true },
];

const NO_MATCH: MatchValidation = {
  ok: false,
  is_match_scene: false,
  diagnostics: [],
  cook_digest: null,
  actor_count: 0,
  wave_count: 0,
  lane_length_m: 0,
};

/** The three sections read four endpoints between them and mutate through six more. Only the reads
 *  matter to a capture, so the writes are absent and would throw — which is the point: a scene that
 *  silently mutated would be photographing a state no user gesture produced. */
const gameplayClient = (roster: RoleRow[] = []) =>
  ({
    matchValidate: () => Promise.resolve(NO_MATCH),
    roleCatalog: () => Promise.resolve(GAMEPLAY_ROLES),
    roleStatus: () =>
      Promise.resolve({ roster, score: 0, scoreEntity: null, remaining: 0, companions: [], won: false, health: null, blocked: null }),
    cinemaCatalog: () => Promise.resolve(GAMEPLAY_SHOTS),
    cinemaList: () =>
      Promise.resolve({ entity: null, shots: 0, seconds: 0, mood: "normal", reads: [], problems: [], message: "", reason: null }),
    cameraProbe: () => Promise.resolve({ eye: [0, 0, 0], lookAt: [0, 0, 0], fovDeg: 45, cinematic: false, distance: 0 }),
    vfxCatalog: () => Promise.resolve(GAMEPLAY_EFFECTS),
    vfxList: () => Promise.resolve({ entity: null, layers: 0, particles: 0, reads: [], problems: [], message: "", reason: null }),
    vfxProbe: () => Promise.resolve({ additive: 0, soft: 0, total: 0, bursts: 0, peakRadiance: 0 }),
  }) as unknown as EditorClient;

/** `function`, not `const` — and the whole bundle refuses to load otherwise. `gameplayScenes()` is
 *  hoisted above `SCENES`, but a `setup: selectCrystal` reference is read while the array is being
 *  built, so a `const` declared below it is still in its temporal dead zone. The driver reads its
 *  registry off `window.__MTK_SHOTS__` in the built bundle, so a module that throws on load reports
 *  ZERO scenes — which is exactly what this cost, once, before it was written this way. */
function selectCrystal() {
  const s = projectionStore.getState();
  s.bulkLoad([
    { id: "crystal", name: "Crystal", parentId: null, components: { Transform: {} } },
    { id: "floor", name: "Floor Plate", parentId: null, components: { Transform: {} } },
  ] as never);
  s.select("crystal");
}

function selectNothing() {
  const s = projectionStore.getState();
  s.bulkLoad([{ id: "crystal", name: "Crystal", parentId: null, components: { Transform: {} } }] as never);
  s.select(null);
}

function gameplayScenes(): Scene[] {
  return [
    {
      id: "gameplay-nothing-selected",
      looking_for:
        "THE GAMEPLAY COLUMN BEFORE ANYTHING IS SELECTED, at the 300px the left dock actually gives " +
        "it. What was here was three bare `<h3>`s, three paragraphs each saying 'Select an object, " +
        "then …' with a different noun, and — marooned in the middle of that populated column — a " +
        "full-size empty state with a 40px icon, a five-line centred paragraph, a button floating " +
        "BELOW it and a second centred paragraph below that. Nine blocks of prose and one control. " +
        "What a reader is checking now is that it is three NAMED sections with state in their " +
        "summaries, only the first of them open, and that the one action in the panel sits inside " +
        "the block that explains it rather than under it",
      expect: {
        present: [
          ["[data-testid='roles-section']", 1],
          ["[data-testid='cinema-section']", 1],
          ["[data-testid='vfx-section']", 1],
          // Ten cards, drawn and disabled rather than absent: the vocabulary is visible before the
          // guess that clicking an object would reveal something.
          ["[data-testid='roles-section'] .mtk-choice", 10],
          ["[data-testid='role-collectible'][disabled]", 1],
          // The action is INSIDE the empty state now, not in a row under it.
          [".mtk-empty-panel__actions .mtk-btn--primary", 1],
        ],
        // Two of the three closed by default, so the column is calm at rest. `[data-state]` is the
        // disclosure's own stable token — asserting on a class list would test the styling.
        absent: ["[data-icon-missing]", "[data-testid='cinema-section'][data-state='open']", "[data-testid='vfx-section'][data-state='open']"],
        text_present: ["Roles", "Cinematics", "Effects", "Select an object", "Create a starter match"],
        text_absent: ["undefined", "null", "NaN"],
        // A CLOSED SECTION STILL REPORTS ITS STATE, AND THIS IS A MEASUREMENT BECAUSE `text_present`
        // CANNOT SAY SO. `.mtk-disclosure__summary` was `display: none` under a `max-width: 760px`
        // WINDOW query — a rule about the window deciding what a 340px dock's header may say — and a
        // `text_present: ["6 shots to choose from"]` claim would have been green over three headers
        // showing a caret and a word. `min_width` on all three is the claim that cannot be.
        min_width: [[".mtk-disclosure__summary", 60]],
        // EVERY GEOMETRIC CLAIM HERE IS SCOPED TO THE OPEN SECTION, and the two that were not is how
        // this scene first went red: a closed `DisclosureSection` keeps its content mounted so draft
        // state survives, and collapses it with `grid-template-rows: 0fr` + `overflow: hidden`. Its
        // cards are therefore 242x65 boxes with 0 pixels on screen — correctly so (they are `hidden`
        // and `inert` too), and `unclipped` reported all twelve of them.
        min_height: [["[data-testid='roles-section'] .mtk-choice", 44], ["[data-testid='roles-section'] .mtk-choice__description", 12]],
        unclipped: ["[data-testid='roles-section'] .mtk-choice", "[data-testid='roles-section']", ".mtk-empty-panel__actions"],
      },
      width: 300,
      setup: selectNothing,
      render: () => <MatchPanel client={gameplayClient()} />,
    },
    {
      id: "gameplay-object-selected",
      looking_for:
        "the same column with an object selected, which is when the ten role cards become live. The " +
        "grid is `auto-fit` from a minimum card width rather than a hardcoded `repeat(2, 1fr)`: at " +
        "300px that is ONE column at a readable width instead of two at 113px, where 'Solid " +
        "obstacle' did not fit. Every card carries the sentence that says what it does — it used to " +
        "exist only as a `title`, which is invisible to touch, to the keyboard and to anyone reading " +
        "the panel rather than hunting in it",
      expect: {
        present: [
          ["[data-testid='roles-section'] .mtk-choice", 10],
          ["[data-testid='role-collectible']:not([disabled])", 1],
          ["[data-testid='roles-section'] .mtk-choice__description", 10],
        ],
        absent: ["[data-icon-missing]", "[data-testid='role-collectible'][disabled]"],
        text_present: ["Crystal", "Collectible", "Spins; vanishes and scores"],
        text_absent: ["undefined", "null", "NaN"],
        min_height: [["[data-testid='roles-section'] .mtk-choice", 44]],
        unclipped: ["[data-testid='roles-section'] .mtk-choice", "[data-testid='roles-section'] .mtk-choice__label"],
      },
      width: 300,
      setup: selectCrystal,
      render: () => <MatchPanel client={gameplayClient()} />,
    },
    {
      id: "gameplay-wide",
      looking_for:
        "the SAME grid given room. The column count is the browser's decision from a minimum card " +
        "width, so a wider surface gains columns instead of stretching the same two cards into " +
        "billboards — the failure `asset-library-wide` is written against, one panel over, and the " +
        "one a hardcoded `repeat(2, 1fr)` cannot avoid at either end",
      expect: {
        present: [["[data-testid='roles-section'] .mtk-choice", 10]],
        absent: ["[data-icon-missing]"],
        text_present: ["Collectible", "Waypoint"],
        text_absent: ["undefined", "null", "NaN"],
        unclipped: ["[data-testid='roles-section'] .mtk-choice"],
        // Three cards that fitted one column at 300px share a row here. Named pairs rather than a
        // column count, because the count is the browser's and asserting it would pin the arithmetic
        // rather than the rule.
        same_line: [["[data-testid='role-collectible']", "[data-testid='role-solid']"]],
      },
      viewport: { width: 760, height: 980 },
      setup: selectCrystal,
      render: () => <MatchPanel client={gameplayClient()} />,
    },
  ];
}

// ── the state-machine editor ──────────────────────────────────────────────────────────────────────

/** THE SIGNATURE AUTHORING SURFACE THIS HARNESS HAD NEVER PHOTOGRAPHED.
 *
 *  `state-graph-door` above captures the CANVAS — `StateGraph`, the projection of a machine onto the
 *  shared graph framework. The panel that WRAPS it, where a machine is actually built, had no capture
 *  of any kind: not the states list, not a transition, not a guard, not the empty state, not a
 *  refusal. It was also the last panel in the tree drawing its own controls — 25 raw `<select>`,
 *  `<input>` and `<button>` elements under one hand-written monospace style — and no gate could see
 *  that, because `check-ui-constitution` counts them and a count is not a picture.
 *
 *  Photographed in the box the Logic dock really is: a bounded scroll parent →
 *  `.mtk-bottom-workspace--logic` → its tab strip → the body. The chain is the point — the panel's
 *  header, rail and footer are only pinned if it FILLS that body rather than growing past it, which
 *  is the ADR-124 defect one level down. */
type AuthorReply = { id: string | null; error: string | null; unreachable: string[] };

function stateMachineScenes(): Scene[] {
  const DOOR_REGISTRY: RuleRegistryInfo = {
    events: [
      { name: "badge_accepted", description: "a valid badge was presented at the reader" },
      { name: "open_requested", description: "someone asked the door to open" },
      { name: "travel_complete", description: "the leaf reached the end of its travel" },
      { name: "close_requested", description: "someone asked the door to close" },
      { name: "obstruction_detected", description: "the safety edge was triggered mid-travel" },
      { name: "obstruction_cleared", description: "the safety edge reads clear again" },
    ],
    actions: [{ name: "SetField", description: "set a component field" }],
    components: [
      {
        name: "DoorState",
        fields: [
          { name: "phase", ty: "string" },
          { name: "obstructed", ty: "boolean" },
        ],
      },
      { name: "Interlock", fields: [{ name: "pressure", ty: "number" }] },
    ],
  };

  /** The same door, with a real GUARD on the transition that has one in life: the safety edge only jams
   *  the leaf while something is actually in the way. An empty `conditions` list on every transition
   *  would photograph a guard editor that had never been used. */
  const GUARDED_DOOR: StateMachine = {
    ...DOOR_MACHINE,
    transitions: DOOR_MACHINE.transitions.map((t) =>
      t.id === "t6"
        ? {
            ...t,
            rule: {
              ...t.rule,
              conditions: [
                { entity: "door", component: "DoorState", field: "obstructed", op: "eq" as const, value: { Bool: true } },
              ],
            },
          }
        : t,
    ),
  };

  /** Three states and two transitions — small enough that a TRANSITION CARD, with its guard, fits on
   *  screen at a size the dock really is. The door is the machine that shows what a graph looks like;
   *  this is the one that shows what a transition looks like. */
  const ALARM: StateMachine = {
    name: "Door Alarm",
    entity: "door",
    component: "DoorState",
    field: "phase",
    states: ["Idle", "Ringing", "Silenced"],
    initial: "Idle",
    transitions: [
      {
        id: "a1",
        from: "Idle",
        to: "Ringing",
        rule: {
          name: "Idle -> Ringing",
          enabled: true,
          event: "obstruction_detected",
          conditions: [
            { entity: "door", component: "DoorState", field: "obstructed", op: "eq" as const, value: { Bool: true } },
          ],
          actions: [],
        },
      },
    ],
  };

  const CONVEYOR: StateMachine = {
    name: "Feed Conveyor",
    entity: "conveyor",
    component: "DoorState",
    field: "phase",
    states: ["Idle", "Running"],
    initial: "Idle",
    transitions: [],
  };

  /** `playing` is a SEPARATE fact from `current`: the shell defaults a machine's `current` to its own
   *  `initial`, so a live readout keyed on it would claim a machine that has never run is running. The
   *  wide scene photographs the running case (a `live` chip on the graph, a state in the footer); the
   *  rest photograph the authoring case, which is what a user meets first. */
  const seedDoorScene = (playing = false) => () => {
    playStore.getState().refresh({ playing, paused: false });
    const summaries = {
      door: { id: "door", name: "Airlock Door — Bay 3", parentId: null, kind: "mesh" },
      conveyor: { id: "conveyor", name: "Feed Conveyor", parentId: null, kind: "mesh" },
      reader: { id: "reader", name: "Badge Reader — North", parentId: null, kind: "requirer" },
    };
    projectionStore.setState({
      summaries: { ...summaries } as never,
      edges: {} as never,
      order: Object.keys(summaries),
      selectedId: "door",
      multiSelect: ["door"],
    });
  };

  const stateMachineClient = (machines: StateMachineInfo[], author: () => Promise<AuthorReply>) =>
    ({
      ruleRegistry: () => Promise.resolve(DOOR_REGISTRY),
      stateMachines: () => Promise.resolve(machines),
      authorStateMachine: author,
      deleteStateMachine: () => Promise.resolve(true),
    }) as unknown as EditorClient;

  /** The real chain the panel lays itself out inside, tab strip included — the strip costs the editor
   *  ~40px of a 520px dock, and a capture that leaves it out is a capture of a taller panel. */
  function logicDock(children: ReactNode) {
    return (
      <div style={{ height: "100vh", overflow: "hidden auto", background: "var(--mtk-bg-panel)" }}>
        <div className="mtk-bottom-workspace mtk-bottom-workspace--logic">
          <DockTabs
            ariaLabel="Logic editors"
            activeId="states"
            onChange={() => {}}
            tabs={[
              { id: "rules", label: "Rules" },
              { id: "states", label: "State machines" },
              { id: "graph", label: "Binding graph" },
              { id: "compose", label: "Compose" },
            ]}
          />
          <div className="mtk-bottom-workspace__body is-bleed">{children}</div>
        </div>
      </div>
    );
  }

  const doorInfo: StateMachineInfo = { id: "sm-door", current: "Opening", machine: GUARDED_DOOR };
  // `current` is never null on the wire: the shell defaults it to the machine's own `initial`
  // (`main.rs` `ListStateMachines`), which is exactly why the running/not-running fact comes from the
  // play store instead of from this field.
  const conveyorInfo: StateMachineInfo = { id: "sm-conveyor", current: CONVEYOR.initial, machine: CONVEYOR };
  const openDoor = ["#state-machines-sm-door-tab"];
  const quiet = (): Promise<AuthorReply> => Promise.resolve({ id: "sm-door", error: null, unreachable: [] });

  const scene = (
    id: string,
    looking_for: string,
    expect: Expect,
    author: () => Promise<AuthorReply>,
    click: string[] = openDoor,
    viewport = { width: 1240, height: 520 },
    running = false,
  ): Scene => ({
    id,
    looking_for,
    expect,
    viewport,
    click,
    setup: seedDoorScene(running),
    render: () => logicDock(<StateGraphPanel client={stateMachineClient([doorInfo, conveyorInfo], author)} />),
  });

  return [
    scene(
      "state-machine-editor",
      "THE STATE MACHINE EDITOR, in the dock it ships in. What a reader is checking is that this is " +
        "recognisably the SAME application as the Model workspace: a titled header naming what is " +
        "open, the machines in this scene as a labelled rail on the left, the graph taking the room " +
        "and the controls that shape it beside it — never stacked underneath, which is what put both " +
        "editable lists below a 520px fold. Every control is the shared family: the start state is " +
        "the design system's radio and not the desktop's blue dot, a delete is a ghost icon button " +
        "and not the character times, and the guard on obstruction_detected is the SAME clause row " +
        "the Rules builder one tab over renders. The footer says the thing nobody could find out " +
        "before: there is no Save button because every edit already is one undoable step",
      {
        present: [
          ["[data-testid='sm-list'] [role='tab']", 2],
          ["[data-testid='sm-state']", 6],
          ["[data-testid='sm-transition']", 7],
          ["[data-testid='sm-cond']", 1],
          [".mtk-radio__dot", 6],
          [".react-flow__node", 6],
        ],
        // The OS radio is gone: every single-choice mark in this panel is the shared drawing.
        absent: ["input[type='radio']:not(.mtk-radio__dot)"],
        text_present: ["Airlock Door — Bay 3", "obstruction_detected", "one undoable step", "now in Opening"],
        text_absent: ["null", "undefined", "NaN"],
        // The controls sit BESIDE the canvas, not under it — the whole reason for the split. 300px is
        // the track's floor; a stacked layout measures the full panel width here and fails the ceiling.
        min_width: [[".mtk-canvas-split__side", 300]],
        max_width: [[".mtk-canvas-split__side", 340]],
        // The panel's own chrome must survive the dock: a header that scrolls away and a footer that
        // is 100px below the fold are the two failures this composition exists to prevent.
        // The panel's own chrome must survive the dock, and so must the tab strip above it: a header
        // that scrolls away, a footer 100px below the fold and a tab row squeezed to zero height are
        // three separate ways for a bounded dock to eat the thing it is holding, and the strip is the
        // one that actually happened (24px of every Logic tab, cut to 0, still in the DOM).
        unclipped: [
          ".mtk-workspace-panel__header",
          ".mtk-workspace-panel__footer",
          ".mtk-dock-tab",
          "[data-testid='sm-add-state']",
        ],
        // The controls column scrolls on its own, so reading down it does not drag the graph away.
        // Anything below its fold is REACHED, not lost — which is why `sm-add-transition` is not in
        // the list above and `sm-add-state`, at the top of that column, is.
        min_height: [[".mtk-canvas-split__side", 300]],
        // from and to are one sentence. They wrapped into separate lines the moment the track
        // narrowed, because the selects claimed intrinsic widths instead of sharing the row.
        same_line: [["[data-testid='sm-trans-from']", "[data-testid='sm-trans-to']"]],
      },
      quiet,
      openDoor,
      { width: 1240, height: 520 },
      true,
    ),
    scene(
      "state-machine-unreachable",
      "a state NOTHING LEADS TO, said as a warning rather than a rejection — the machine is saved, " +
        "and the thing that is wrong with it is named at the top of the editor with the fix in the " +
        "same sentence. It used to be a bare coloured line of text with no mark, which is colour " +
        "carrying a meaning on its own",
      {
        present: [
          ["[data-testid='sm-unreachable']", 1],
          ["[data-testid='sm-unreachable'] .mtk-callout__icon", 1],
        ],
        text_present: ["Jammed", "add a transition into it"],
        text_absent: ["null", "undefined", "NaN"],
        unclipped: ["[data-testid='sm-unreachable']", ".mtk-workspace-panel__footer"],
        // The warning is ABOVE the graph it is about, not below the fold under it.
        stacked: [["[data-testid='sm-unreachable']", "[data-testid='state-graph']"]],
      },
      () => Promise.resolve({ id: "sm-door", error: null, unreachable: ["Jammed"] }),
      [...openDoor, "[data-testid='sm-add-transition']"],
    ),
    scene(
      "state-machine-blocked",
      "the REFUSAL, which is the state that has to be legible: the machine the engine would not " +
        "accept, its reason quoted in the user's words, and the editor still holding the draft so the " +
        "edit is one correction away rather than lost (ADR-016). The reason carries a mark and a " +
        "title of its own — a refusal that is only red is a refusal a colour-blind reader meets as an " +
        "ordinary sentence",
      {
        present: [
          ["[data-testid='sm-error']", 1],
          ["[data-testid='sm-error'] .mtk-callout__icon", 1],
        ],
        text_present: ["was not saved", "Nowhere"],
        text_absent: ["null", "undefined", "NaN"],
        unclipped: ["[data-testid='sm-error']"],
        stacked: [["[data-testid='sm-error']", "[data-testid='state-graph']"]],
      },
      () =>
        Promise.resolve({
          id: null,
          error: "a transition points to 'Nowhere', which isn't one of this machine's states",
          unreachable: [],
        }),
      [...openDoor, "[data-testid='sm-add-transition']"],
    ),
    {
      id: "state-machine-empty",
      looking_for:
        "a scene with no state machines in it — an EMPTY STATE with one next step in it, and the " +
        "control that takes it. The shipped panel printed a grey line of prose naming a button that " +
        "lived somewhere else on the panel: the decisive step offloaded to a passive sentence, " +
        "<ux_quality> 1 exactly. The words also have to say what a state machine IS, because the " +
        "reader who meets this screen is by definition the one who has never made one",
      expect: {
        present: [
          ["[data-testid='sm-empty']", 1],
          ["[data-testid='sm-empty'] [data-testid='sm-new']", 1],
        ],
        text_present: ["No state machines yet", "one undoable step"],
        text_absent: ["null", "undefined", "NaN"],
        absent: ["[data-testid='sm-list']"],
        unclipped: ["[data-testid='sm-new']"],
      },
      viewport: { width: 1240, height: 520 },
      setup: seedDoorScene(),
      render: () =>
        logicDock(
          <StateGraphPanel
            client={stateMachineClient([], () => Promise.resolve({ id: "sm-1", error: null, unreachable: [] }))}
          />,
        ),
    },
    scene(
      "state-machine-narrow",
      "the SAME editor at 760px, which is what the dock is worth on a laptop with both side docks " +
        "open. Two shared responsive rules fire here and both have to hold: below 980 the canvas and " +
        "its controls stack, so the graph keeps a readable width instead of being squeezed into a " +
        "third of the panel; below 860 the machine rail turns back into a horizontal strip, because " +
        "a 190px column is a quarter of a 760px panel. What a reader is checking is that NOTHING was " +
        "given up for it — both machines still reachable, both verbs still on screen, the footer " +
        "still pinned. 520px is the dock's own ceiling (`clamp(320px, 48vh, 520px)`), so this is the " +
        "most room this panel is ever given",
      {
        present: [
          ["[data-testid='sm-list'] [role='tab']", 2],
          ["[data-testid='sm-state']", 6],
        ],
        text_present: ["Airlock Door — Bay 3"],
        text_absent: ["null", "undefined", "NaN"],
        // The rail is a strip: its two tabs share a row rather than stacking down a column.
        same_line: [["#state-machines-sm-door-tab", "#state-machines-sm-conveyor-tab"]],
        unclipped: [".mtk-workspace-panel__footer", "[data-testid='sm-add-state']", "#state-machines-sm-conveyor-tab"],
      },
      quiet,
      openDoor,
      { width: 760, height: 520 },
    ),
    {
      id: "state-machine-transition-card",
      looking_for:
        "ONE TRANSITION, READ AS A SENTENCE: from a state, to a state, when an event — and under it " +
        "the guard that makes it conditional, drawn with the SAME clause row the Rules builder uses " +
        "rather than a second copy that had drifted from it. The old version put five monospace " +
        "dropdowns and a times character on one wrapping line with a bare `+ if` button beside them; " +
        "the operator list on a true/false field offered `<` and `>`, which can only ever answer the " +
        "same way. The graph is PUT AWAY here, which is the only way a whole card is on screen at a " +
        "height the dock really reaches: measured, the graph at its 200px floor plus the two section " +
        "headings leave 68px for a 143px card, and no arrangement of the three fixes that — so one " +
        "of them has to be able to go, which is the constitution's `everything collapsible` where it " +
        "actually bites",
      expect: {
        present: [
          ["[data-testid='sm-transition']", 1],
          ["[data-testid='sm-cond']", 1],
          ["[data-testid='sm-add-cond']", 1],
        ],
        text_present: ["obstruction_detected", "Only if every one of these holds"],
        text_absent: ["null", "undefined", "NaN"],
        // The sentence stays on its line, and so does the guard clause beneath it.
        same_line: [
          ["[data-testid='sm-trans-from']", "[data-testid='sm-trans-to']"],
          ["[data-testid='sm-cond-component']", "[data-testid='sm-cond-op']"],
        ],
        unclipped: ["[data-testid='sm-transition']", "[data-testid='sm-cond']", "[data-testid='sm-add-cond']"],
      },
      // The WIDE dock, at the tallest it ever is: `.mtk-bottom-dock` is `clamp(320px, 48vh, 520px)`,
      // so 520 is the ceiling and a scene taller than that photographs a panel nobody has. The
      // editor track gets 148px of it — which is what the graph's 240px ceiling exists to leave.
      viewport: { width: 1240, height: 520 },
      click: ["#state-machines-sm-alarm-tab", "[data-testid='sm-toggle-graph']"],
      setup: seedDoorScene(),
      render: () =>
        logicDock(
          <StateGraphPanel
            client={stateMachineClient([{ id: "sm-alarm", current: ALARM.initial, machine: ALARM }], quiet)}
          />,
        ),
    },
  ];
}


// ── Pipe Forge: the stage's own floating workspace ────────────────────────────────────────────────

/** THE ONLY WORKSPACE THAT FLOATS ON THE STAGE, AND THE LAST ONE NOTHING HAD EVER PHOTOGRAPHED.
 *
 *  Every other sub-engine opens in a dock — a track with a width the shell owns. Pipe Forge is
 *  `position: absolute` INSIDE the viewport, 336px wide, `max-height: calc(100% - 104px)`, floating
 *  beside the tool rail while the user clicks route points into the 3D behind it. That shape is the
 *  whole reason it needs captures rather than a unit test: a panel whose height is a percentage of a
 *  box it does not own is a panel that can run out of room, and nothing in the repository had ever
 *  measured it against that box.
 *
 *  The frame reproduces the real chain — the stage's `position: relative` box with `overflow: hidden`
 *  — and passes the exact `left`/`width` override `App.tsx` passes, so the capture is the panel where
 *  it actually lives rather than a component on a blank page.
 */
function pipeForgeScenes(): Scene[] {
  const noopClient = {} as unknown as EditorClient;

  const status = (over: Partial<PipeForgeStatus> = {}): PipeForgeStatus => ({
    active: true,
    kit: "galvanized",
    diameterCm: 5,
    quality: "production",
    autoFittings: true,
    points: 4,
    lengthM: 12.84,
    previewTriangles: 18_240,
    canBake: true,
    message: "Click again to extend the run.",
    handles: [
      { nodeId: 1, position: [0, 0, 0], connectedEdges: [1], fittingIds: [] },
      { nodeId: 2, position: [3.5, 0, 0], connectedEdges: [1, 2], fittingIds: [1] },
      { nodeId: 3, position: [3.5, 2.25, 0], connectedEdges: [2, 3], fittingIds: [] },
      { nodeId: 4, position: [9.4, 2.25, -1.2], connectedEdges: [3], fittingIds: [] },
    ],
    edges: [
      { id: 1, from: 1, to: 2, diameterM: 0.05 },
      { id: 2, from: 2, to: 3, diameterM: 0.05 },
      { id: 3, from: 3, to: 4, diameterM: 0.05 },
    ],
    fittings: [
      { id: 1, nodeId: 2, kind: "elbow", catalogId: null, automatic: true },
      { id: 2, nodeId: 3, kind: "valve", catalogId: "isolation-valve", automatic: false },
    ],
    fittingCatalog: [
      { id: "isolation-valve", label: "Isolation valve", kind: "valve", assetHandle: "mtkasset:valve-dn50", diameterScale: 1.4, lengthScale: 1 },
      { id: "weld-neck-flange", label: "Weld neck flange DN50", kind: "flange", assetHandle: null, diameterScale: 1.8, lengthScale: 0.4 },
    ],
    branchFrom: null,
    editingEntity: null,
    ...over,
  });

  const bake: PipeBakeReport = {
    entityId: "e-pipe-1",
    handle: "mtkasset:pipe-run-a",
    vertices: 9_612,
    triangles: 18_240,
    lodTriangles: [18_240, 7_100, 2_050],
    textureResolution: 2048,
    collisionHulls: 6,
    collisionKind: "convex",
    collisionTriangles: 640,
    watertight: true,
    warnings: ["Two branches share one collar; the shared collar is exported once."],
    message: "Pipe run baked.",
  };

  /** The stage: a `position: relative` box with `overflow: hidden`, which is what `App.tsx` gives it,
   *  and the ONLY reason `max-height: calc(100% - 104px)` resolves to anything at all. */
  const stage = (node: ReactNode): ReactNode => (
    <div
      data-testid="stage-frame"
      style={{ position: "relative", height: "100vh", overflow: "hidden", background: "var(--mtk-bg-inset)" }}
    >
      {node}
    </div>
  );

  const scene = (
    id: string,
    looking_for: string,
    expect: Expect,
    node: ReactNode,
    click?: string[],
    // The panel's height is `calc(100% - 104px)` of the STAGE, so a scene that has to prove something
    // about a control deep inside an open section needs a stage tall enough to hold it. Anything less
    // photographs the panel's own scrollbar doing its job and calls it a defect.
    viewport = { width: 1180, height: 820 },
  ): Scene => ({
    id,
    looking_for,
    click,
    viewport,
    expect: {
      ...expect,
      present: [["[data-testid='pipe-forge']", 1], ...(expect.present ?? [])],
      // A blank control photographs exactly like a working one: `Icon` draws an empty, still-sized
      // box for a name the set does not have.
      absent: ["[data-icon-missing]", ...(expect.absent ?? [])],
      text_absent: ["undefined", "NaN", ...(expect.text_absent ?? [])],
      // The panel is 336px of a 720px stage and it must be WHOLE — a floating workspace that runs off
      // its own container has no dock to yield into.
      unclipped: ["[data-testid='pipe-forge-title']", ...(expect.unclipped ?? [])],
    },
    render: () => stage(node),
  });

  return [
    scene(
      "pipe-forge-setup",
      "the first thing a user meets after clicking Draw pipe: four recipe choices and one primary " +
      "action, floating beside the tool rail at the 336px the shell gives it. Four `PropertyRow`s " +
      "share ONE set of tracks, so what a reader is checking is a single label column, four control " +
      "cells ending at the same x, and the unit in a column of its own rather than hanging off the " +
      "end of the number. The panel used to draw its own row with a hard 144px control, which is why " +
      "its right edge went ragged and could not narrow when the tool rail is minimised",
      {
        present: [
          ["[data-testid='pipe-forge-setup']", 1],
          ["[data-testid='pipe-forge-start']", 1],
        ],
        text_present: ["Material kit", "Build quality", "Draw new pipe"],
        unclipped: ["[data-testid='pipe-forge-start']", "[data-testid='pipe-forge-kit']"],
      },
      <PipeForge client={noopClient} status={null} onStatus={() => {}} onBaked={() => {}} style={{ left: 138, width: 336 }} />,
    ),
    scene(
      "pipe-forge-editable",
      "the same setup with an editable pipe selected — the state that offers a SECOND primary action " +
      "in a panel that already has one. What a reader is checking is the hierarchy: which of the two " +
      "the eye lands on first, and whether the shared `Callout` reads as an offer rather than as an " +
      "alarm. It is `tone=\"info\"` and carries the tone mark every note in the editor carries, " +
      "because colour alone must not be what separates an offer from a warning",
      {
        present: [["[data-testid='pipe-forge-edit']", 1]],
        text_present: ["This pipe can be reopened", "Edit selected pipe"],
        unclipped: ["[data-testid='pipe-forge-edit']", "[data-testid='pipe-forge-start']"],
      },
      <PipeForge client={noopClient} status={null} editableEntityId="e-pipe-1" onStatus={() => {}} onBaked={() => {}} style={{ left: 138, width: 336 }} />,
    ),
    scene(
      "pipe-forge-drawing",
      "a live route: four points, three runs, the recipe echoed back as badges and the two actions the " +
      "gesture needs. This is the state the user spends the most time in — the panel is beside their " +
      "hand while every click lands in the 3D behind it — so what a reader is checking is whether the " +
      "three numbers can be read WITHOUT stopping, and whether the line under the badges admits that " +
      "the four settings above it are now READ-ONLY. They are: `SetupControls` is unmounted the moment " +
      "drawing starts, and until this pass nothing anywhere said so",
      {
        present: [
          ["[data-testid='pipe-forge-active']", 1],
          ["[data-testid='pipe-forge-bake']", 1],
          ["[data-testid='pipe-forge-undo']", 1],
        ],
        text_present: ["Points", "Length", "Triangles", "12.84 m"],
        same_line: [["[data-testid='pipe-forge-undo']", "[data-testid='pipe-forge-bake']"]],
        unclipped: ["[data-testid='pipe-forge-bake']", "[data-testid='pipe-forge-points']"],
      },
      <PipeForge client={noopClient} status={status()} onStatus={() => {}} onBaked={() => {}} style={{ left: 138, width: 336 }} />,
    ),
    scene(
      "pipe-forge-network",
      "the route-network section opened: a point picker, the three-axis position editor and the three " +
      "actions on the selected point. The axes are the shared `VectorField` — one property, three " +
      "cells, the axis mark drawn INSIDE each cell — where they used to be a `<fieldset>`, a " +
      "`<legend>` and three 11px monospace letters in columns of their own that made the three boxes " +
      "start at three different x positions. A reader is checking that they read as ONE value, and " +
      "that Apply, Draw branch and Remove are the same control family at the same size",
      {
        present: [
          ["[data-testid='pipe-forge-handle']", 1],
          ["[data-testid='pipe-forge-move-handle']", 1],
          ["[data-testid='pipe-forge-remove-handle']", 1],
        ],
        text_present: ["Point on the route", "Position", "Branch diameter"],
        unclipped: [
          "[data-testid='pipe-forge-handle-position-x']",
          "[data-testid='pipe-forge-handle-position-y']",
          "[data-testid='pipe-forge-handle-position-z']",
          "[data-testid='pipe-forge-move-handle']",
        ],
        same_line: [["[data-testid='pipe-forge-handle-position-x']", "[data-testid='pipe-forge-handle-position-z']"]],
      },
      <PipeForge client={noopClient} status={status()} onStatus={() => {}} onBaked={() => {}} style={{ left: 138, width: 336 }} />,
      ["[data-testid='pipe-forge-network'] .mtk-disclosure__toggle"],
    ),
    scene(
      "pipe-forge-catalog",
      "the saved-fittings editor — the deepest the panel goes, and a six-field form nothing in the " +
      "repository had ever seen. It is the SAME form the Model workspace and the inspector use " +
      "(`FieldGrid`/`Field`: label above, control, help below), and its rows are the shared " +
      "`ListRow`, which WRAPS instead of overflowing. That is the measured defect this scene was " +
      "written against: at this exact width every row used to lose 53px of its name and 49px of its " +
      "Remove button off an `overflow-x: hidden` edge, with no scrollbar and no way to reach either",
      {
        present: [
          ["[data-testid='pipe-forge-catalog-label']", 1],
          ["[data-testid='pipe-forge-save-catalog']", 1],
          ["[data-testid='pipe-forge-remove-fitting-2']", 1],
        ],
        text_present: ["Name", "Reference", "Isolation valve", "Weld neck flange DN50"],
        // THE ROWS ARE THE POINT. Every one of these lost pixels off the right edge of an
        // `overflow-x: hidden` panel before `ListRow`; asserting they are whole is asserting the fix.
        unclipped: [
          "[data-testid='pipe-forge-catalog-label']",
          "[data-testid='pipe-forge-save-catalog']",
          "[data-testid='pipe-forge-remove-catalog-isolation-valve']",
          "[data-testid='pipe-forge-remove-catalog-weld-neck-flange']",
        ],
        min_height: [["[data-testid='pipe-forge-catalog-label']", 28]],
      },
      <PipeForge client={noopClient} status={status()} onStatus={() => {}} onBaked={() => {}} style={{ left: 138, width: 336 }} />,
      ["[data-testid='pipe-forge-catalog'] .mtk-disclosure__toggle"],
      { width: 1180, height: 1240 },
    ),
    scene(
      "pipe-forge-baked",
      "the end of the loop: the asset is built and the panel reports what it actually produced — " +
      "triangles, LODs, texture resolution, watertightness, collision — plus a warning and the two " +
      "ways onward. The ux_quality rules 1 and 3 are the subject: the action that started this owns " +
      "its outcome, the outcome says what it BOUGHT, and the next step is a control, not a sentence",
      {
        present: [["[data-testid='pipe-forge-report']", 1]],
        text_present: ["Asset ready", "18,240", "Detail levels", "Watertight", "Draw another pipe"],
        unclipped: ["[data-testid='pipe-forge-report']", "[data-testid='pipe-forge-start']"],
      },
      <PipeForgeBaked report={bake} />,
    ),
  ];
}

/** The baked state needs the panel's own `lastReport`, which only a bake sets — so the scene drives
 *  the real `onBaked` path through a stub client rather than faking a prop the component does not
 *  have. A capture of a state reached differently from how a user reaches it is a capture of a
 *  different state. */
function PipeForgeBaked({ report }: { report: PipeBakeReport }) {
  const [status, setStatus] = useState<PipeForgeStatus | null>(null);
  const [baked, setBaked] = useState(false);
  const client = {
    pipeForgeStart: () => Promise.resolve(startedStatus()),
    pipeForgeBake: () => Promise.resolve(report),
    pipeForgeStatus: () => Promise.resolve(status as PipeForgeStatus),
  } as unknown as EditorClient;
  useEffect(() => {
    if (baked) return;
    setBaked(true);
    // Drive the two real transitions in order: start a session, then bake it. Reaching the report by
    // any other route would photograph a state the component can hold but a user cannot produce.
    const button = document.querySelector<HTMLButtonElement>("[data-testid='pipe-forge-start']");
    button?.click();
  }, [baked]);
  useEffect(() => {
    if (!status?.active) return;
    document.querySelector<HTMLButtonElement>("[data-testid='pipe-forge-bake']")?.click();
  }, [status]);
  return (
    <PipeForge
      client={client}
      status={status}
      editableEntityId="e-pipe-1"
      onStatus={setStatus}
      onBaked={() => {}}
      style={{ left: 138, width: 336 }}
    />
  );
}

function startedStatus(): PipeForgeStatus {
  return {
    active: true,
    kit: "galvanized",
    diameterCm: 5,
    quality: "production",
    autoFittings: true,
    points: 4,
    lengthM: 12.84,
    previewTriangles: 18_240,
    canBake: true,
    message: "Click again to extend the run.",
    handles: [
      { nodeId: 1, position: [0, 0, 0], connectedEdges: [1], fittingIds: [] },
      { nodeId: 2, position: [3.5, 0, 0], connectedEdges: [1], fittingIds: [] },
    ],
    edges: [{ id: 1, from: 1, to: 2, diameterM: 0.05 }],
    fittings: [],
    fittingCatalog: [],
    branchFrom: null,
    editingEntity: null,
  };
}
