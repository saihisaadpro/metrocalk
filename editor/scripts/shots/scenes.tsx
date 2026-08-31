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
import { AnimationWorkspace } from "../../src/panels/AnimationWorkspace";
import { CutscenePanel } from "../../src/panels/CutscenePanel";
import { SubjectAimBadge } from "../../src/panels/SubjectAimBadge";
import { Diagnostics } from "../../src/panels/Diagnostics";
import { ImportReport } from "../../src/panels/ImportReport";
import { Reveal } from "../../src/panels/Reveal";
import { RigPanel, type RigDocument } from "../../src/panels/RigPanel";
import RIG_MIXAMO from "../../src/panels/__fixtures__/rig-characterization.json";
import RIG_BLOCKED from "../../src/panels/__fixtures__/rig-not-retargetable.json";
import { MatchPanel } from "../../src/panels/MatchPanel";
import { PhysicsPanel } from "../../src/panels/PhysicsPanel";
import { PosePreview, type PoseDocument } from "../../src/panels/PosePreview";
import POSE_PREVIEW from "../../src/panels/__fixtures__/pose-preview.json";
import { assetShelfStore } from "../../src/store/assetShelf";
import { projectionStore } from "../../src/store/projection";
import type {
  AnimationPropertyInfo,
  AnimationTrackInfo,
  AnimationWorkspaceInfo,
  CadReport,
  CadReportPart,
  CinemaReply,
  EffectSpec,
  FramingCatalog,
  MatchValidation,
  RevealResponse,
  RoleRow,
  RoleSpec,
  ShotRow,
  ShotSpec,
  StateMachine,
  SubjectCatalog,
  TimelineTuple,
} from "../../src/transport/protocol";
import { DEFAULT_RENDER_SETTINGS } from "../../src/transport/protocol";
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
  /** Controls that must be DISABLED, read from the element the browser refuses input on.
   *
   *  Every other claim in this type measures a rectangle or reads text, and both are satisfied by a
   *  control that is present, legible, correctly placed and fully live when it should be dead —
   *  which `<ux_quality>` 6 forbids in exactly those words ("no inert controls") and which a
   *  screenshot cannot distinguish from one that works. The case that needed it: a shot filmed from
   *  a camera the author placed leaves its Size and Angle pickers deciding nothing, and a picker
   *  that still turns while the picture does not move is a control lying about what it edits.
   *
   *  Read from `el.disabled` (or `aria-disabled` for a control that is not a form element), never
   *  from a class name — a styling hook that has drifted from the real state is the drift worth
   *  catching. */
  disabled?: string[];
  /** The dual, and it is not redundant: it pins the controls a feature must NOT have switched off.
   *
   *  Without it, "disable what a placed camera decides" is satisfiable by disabling the whole grid,
   *  and the scene would photograph a shot inspector with more dead in it than the ADR claims. */
  enabled?: string[];
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
  /** Text typed into a control the clicks reached, as `[selector, value]` pairs, after every click.
   *
   *  `page.$` searches the whole document rather than the frame, so this reaches a field inside a
   *  PORTALLED dialog — which is where a command palette's query and a dialog's search have always
   *  lived. The driver selects the field's existing contents first, so two scenes typing into the
   *  same selector cannot depend on which ran before. */
  type?: [selector: string, value: string][];
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

/** A FIVE-SHOT CUT WITH FIVE DIFFERENT LENGTHS, one of which films something else.
 *
 *  The lengths are the point. A cutscene whose shots all run the same time draws five equal bars, and
 *  five equal bars are a LIST — the exact thing this panel replaced, redrawn horizontally. 2.5s, 4.0s,
 *  1.6s, 3.0s and 2.2s over 13.3s is a picture that can only be produced by a surface that reads
 *  `effectiveSeconds`.
 *
 *  Shot 1 frames the hall rather than the gun, because that is ordinary film grammar (establish the
 *  place, then cut in) and because until this session every line in a shot list was captioned with the
 *  cutscene's OWNER — so a wide of the hall read back as a wide of the one part standing in it.
 *
 *  The jump-cut warning is real output, not a fixture string: shots 4 and 5 are framed identically
 *  back to back, which is what `Cutscene::problems` says that about. */
const CUTSCENE_SHOTS = [
  { id: "sh-1", size: "wide", angle: "three_quarter", motion: "pull_out", amount: 0.3, seconds: 2.5, subject: "hall", subjectName: "Assembly Hall", reads: "a wide shot of Assembly Hall from three-quarters, pulling out — 2.5s" },
  { id: "sh-2", size: "full", angle: "three_quarter", motion: "push_in", amount: 0.35, seconds: 4.0, subject: "rig", subjectName: "Weld Gun 7", reads: "a full shot of Weld Gun 7 from three-quarters, pushing in — 4.0s" },
  { id: "sh-3", size: "extreme_close", angle: "profile", motion: "hold", amount: 0, seconds: 1.6, subject: "rig", subjectName: "Weld Gun 7", reads: "a very close shot of Weld Gun 7 in profile, holding still — 1.6s" },
  { id: "sh-4", size: "medium", angle: "low", motion: "orbit", amount: 0.5, seconds: 3.0, subject: "rig", subjectName: "Weld Gun 7", reads: "a medium shot of Weld Gun 7 from below, orbiting — 3.0s" },
  { id: "sh-5", size: "medium", angle: "low", motion: "crane_up", amount: 0.6, seconds: 2.2, subject: "rig", subjectName: "Weld Gun 7", reads: "a medium shot of Weld Gun 7 from below, craning up — 2.2s" },
] as const;

const CUTSCENE: CinemaReply = (() => {
  let start = 0;
  const rows: ShotRow[] = CUTSCENE_SHOTS.map((shot, index) => {
    const row: ShotRow = {
      id: shot.id,
      index,
      reads: shot.reads,
      seconds: shot.seconds,
      effectiveSeconds: shot.seconds,
      startSeconds: start,
      // `Mood::Normal.blend_seconds()` is 0.6s, capped at half the shot; the first never blends,
      // and a shot opens one 60Hz tick past its own window (`Cutscene::opens_at`).
      blendSeconds: index === 0 ? 0 : Math.min(0.6, shot.seconds * 0.5),
      openSeconds: index === 0 ? start : start + Math.min(0.6, shot.seconds * 0.5) + 1 / 60,
      size: shot.size,
      angle: shot.angle,
      motion: shot.motion,
      amount: shot.amount,
      subject: shot.subject,
      subjectName: shot.subjectName,
      camera: null,
    };
    start += shot.seconds;
    return row;
  });
  return {
    entity: "rig",
    shots: rows.length,
    seconds: start,
    mood: "normal",
    delivery: "viewport",
    render: DEFAULT_RENDER_SETTINGS,
    reads: rows.map((row) => row.reads),
    rows,
    problems: [
      'shots on "Weld Gun 7" are framed identically back to back — that reads as a jump cut; change the size or the angle',
    ],
    message: "",
    reason: null,
  };
})();

/** The framing vocabulary with the WIRE VALUES the Rust catalogue publishes. A scene that invented a
 *  value would photograph a dropdown whose selected option the engine would refuse. */
const FRAMING: FramingCatalog = {
  sizes: [
    { value: "extreme_wide", label: "Distant", blurb: "The subject is a speck in its world" },
    { value: "wide", label: "Wide", blurb: "The whole subject with generous air around it" },
    { value: "full", label: "Full", blurb: "The subject fills most of the height" },
    { value: "medium", label: "Medium", blurb: "Closer — detail starts to read" },
    { value: "close", label: "Close", blurb: "Tight on the subject" },
    { value: "extreme_close", label: "Very close", blurb: "One thing, very large" },
  ],
  angles: [
    { value: "front", label: "Front", blurb: "Facing the subject head-on" },
    { value: "three_quarter", label: "Three-quarter", blurb: "Off to one side, slightly above" },
    { value: "profile", label: "Profile", blurb: "Directly to the side" },
    { value: "behind", label: "Behind", blurb: "Looking where it looks" },
    { value: "low", label: "From below", blurb: "The subject towers" },
    { value: "high", label: "From above", blurb: "The subject is small" },
  ],
  motions: [
    { value: "hold", label: "Hold", blurb: "Locked off — the camera does not move" },
    { value: "push_in", label: "Push in", blurb: "Creep toward the subject" },
    { value: "pull_out", label: "Pull out", blurb: "Drift away" },
    { value: "orbit", label: "Orbit", blurb: "Circle the subject" },
    { value: "crane_up", label: "Crane up", blurb: "Rise while holding the aim" },
    { value: "crane_down", label: "Crane down", blurb: "Descend while holding the aim" },
  ],
  minSeconds: 0.2,
  maxSeconds: 20,
  maxShots: 12,
  stillMotions: ["hold"],
  deliveries: [
    { value: "viewport", label: "Match viewport", blurb: "Compose for the stage as it is now - no bars" },
    { value: "widescreen", label: "16:9 widescreen", blurb: "The broadcast and web default" },
    { value: "scope", label: "2.39:1 scope", blurb: "Anamorphic scope - the widest frame" },
    { value: "academy", label: "4:3 academy", blurb: "Classic" },
    { value: "square", label: "1:1 square", blurb: "Square" },
    { value: "vertical", label: "9:16 vertical", blurb: "Vertical" },
  ],
};

const CUTSCENE_CARDS: ShotSpec[] = [
  { kind: "establish", label: "Establishing", blurb: "Show where we are before we look at anything closely", adds: "a wide, slowly pulling-out shot from the front" },
  { kind: "hero", label: "Hero shot", blurb: "The workhorse — three-quarters on, pushing in", adds: "a full-body three-quarter shot that creeps closer" },
  { kind: "closeup", label: "Close-up", blurb: "Tight and still — for the moment that matters", adds: "a close, locked-off shot in profile" },
  { kind: "orbit", label: "Show it off", blurb: "Circle the object so every side reads", adds: "a medium shot orbiting a quarter turn" },
  { kind: "reveal", label: "Crane reveal", blurb: "Lift away to show the world around it", adds: "a full shot craning upward" },
  { kind: "vista", label: "The vista", blurb: "The subject is a speck in its world", adds: "an extreme-wide, locked-off shot from the front" },
];

/** WHAT A SHOT CAN BE POINTED AT, ranked by the scene's own hierarchy — the engine's own answer,
 *  headings and all.
 *
 *  The `parts` counts are the reason the list is worth reading rather than scrolling: 378 and 1 are
 *  how the whole line and the one bracket inside it tell themselves apart when their names do not.
 *  `Datum A` has none, which is the row that proves the picker warns BEFORE the shot is aimed — a
 *  subject with no drawn geometry is composed by the solver on its own origin, and from outside that
 *  looks like a camera that went somewhere plausible and filmed nothing. */
const SUBJECT_CATALOG: SubjectCatalog = {
  owner: "rig",
  ownerName: "Weld Gun 7",
  current: "hall",
  candidates: [
    { id: "rig", name: "Weld Gun 7", group: "This object", parts: 1, framable: true, current: false },
    { id: "cell", name: "Weld Cell A", group: "What it is part of", parts: 46, framable: true, current: false },
    { id: "hall", name: "Assembly Hall", group: "What it is part of", parts: 378, framable: true, current: true },
    { id: "nozzle", name: "Nozzle", group: "What it is made of", parts: 1, framable: true, current: false },
    { id: "loom", name: "Cable Loom", group: "What it is made of", parts: 3, framable: true, current: false },
    { id: "fixture", name: "Fixture 3", group: "Beside it", parts: 9, framable: true, current: false },
    { id: "datum", name: "Datum A", group: "Beside it", parts: 0, framable: false, current: false },
  ],
  query: "",
  matches: 7,
  truncated: true,
};

const cutsceneClient = () =>
  ({
    cinemaCatalog: () => Promise.resolve(CUTSCENE_CARDS),
    cinemaFramingCatalog: () => Promise.resolve(FRAMING),
    cinemaSubjectCatalog: (_id: string, _index: number | null, query: string) =>
      Promise.resolve(
        query
          ? {
              ...SUBJECT_CATALOG,
              query,
              truncated: false,
              candidates: SUBJECT_CATALOG.candidates
                .filter((c) => c.name.toLowerCase().includes(query.toLowerCase()))
                .map((c) => ({ ...c, group: "Matches" })),
            }
          : SUBJECT_CATALOG,
      ),
    cinemaList: () => Promise.resolve(CUTSCENE),
    // ADR-190 - the render settings are a DOCUMENT EDIT, so a fixture that answers `cinemaList` and
    // not this one is a client the dialog can read from and not write to. It reached the harness as a
    // `console.error` on the LEDGER scene, which never touches a picker: starting a render adopts the
    // folder the picker returned, and adopting is a commit. Echoing the request keeps the reply
    // consistent with what the caller just asked for, which is what the engine does.
    cinemaSetRender: (
      entity: string,
      format: "movie" | "sequence",
      fps: number,
      height: number | null,
      name: string,
      folder: string,
    ) =>
      Promise.resolve({
        ...CUTSCENE,
        entity,
        render: { format, fps, height, name, folder },
        message: `Delivering ${format === "movie" ? "Movie (MP4)" : "PNG sequence"} at ${height ?? "as on screen"}, ${fps} fps`,
      }),
    // A cancelled picker: no entity, no reason, nothing changed. The harness cannot open a native
    // dialog, and a stub that pretended one had returned a path would photograph a state no click in
    // this scene produced.
    cinemaPickRenderFolder: () => Promise.resolve({ ...CUTSCENE, entity: null, message: "" }),
    // The pose a shot solver would answer with. A capture cannot show the wgpu frame — the harness
    // runs on a box with no GPU — so what is photographed here is the AUTHORING surface around it:
    // the toggle in its pressed state and the read-out naming the moment. The live composite is the
    // `.exe` run's job, and these two pieces of evidence answer different questions.
    cinemaPreview: (id: string, seconds: number, active: boolean) =>
      Promise.resolve({
        active,
        entity: active ? id : null,
        seconds,
        shotIndex: active ? 1 : null,
        shots: CUTSCENE.rows.length,
        reads: CUTSCENE.rows[1].reads,
        subjectName: CUTSCENE.rows[1].subjectName,
        progress: 0.25,
        blending: false,
        eye: [6.2, 3.1, 9.4] as [number, number, number],
        lookAt: [0, 1.4, 0] as [number, number, number],
        fovDeg: 50,
        message: active ? `Previewing shot 2 of 5 at ${seconds.toFixed(1)}s` : "Preview off",
        reason: null,
      }),
  }) as unknown as EditorClient;

/** ADR-192 — the same five-shot cut with its OPENING shot filmed from a camera the author placed.
 *
 *  The opener on purpose, and not only because it is the clip this scene's one click lands on: it is
 *  the establishing wide of a 262 m hall, which is the shot the card vocabulary is least able to
 *  give — every size solves a stand-off from a bounding sphere, and there is no distance at which a
 *  262 m assembly shot broadside fills a 16:9 frame. Standing the camera where the author can see
 *  the line receding is the answer, and it is not expressible as a size crossed with an angle.
 *
 *  Its card (`wide` / `three_quarter`) stays on the row, disabled, because that is the framing "Use
 *  the card again" restores — and the sentence it used to read is what the scene asserts is gone. */
const PLACED_CAMERA = {
  eye: [7.4, 2.9, -5.1] as [number, number, number],
  lookAt: [0.2, 1.35, 0.4] as [number, number, number],
  // `CAMERA_FOV_DEG` — the lens the viewport actually draws through, and what `camera_probe` now
  // reports. It answered a bare 45 until this session, for a projection that has never used one.
  fovDeg: 55,
};

const placedCameraClient = () => {
  const rows = CUTSCENE.rows.map((row) =>
    row.index === 0
      ? {
          ...row,
          camera: PLACED_CAMERA,
          reads: "a placed shot of Assembly Hall, pulling out — 2.5s",
        }
      : row,
  );
  const cut: CinemaReply = {
    ...CUTSCENE,
    rows,
    reads: rows.map((row) => row.reads),
    // The jump cut between shots 4 and 5 is still real; the placed shot simply is not part of it.
    problems: CUTSCENE.problems,
  };
  return {
    ...cutsceneClient(),
    cinemaList: () => Promise.resolve(cut),
    cinemaSetShotCamera: (id: string, index: number) =>
      Promise.resolve({ ...cut, entity: id, message: `Shot ${index + 1} is now ${rows[index]?.reads ?? "placed"}` }),
    cinemaClearShotCamera: (id: string, index: number) =>
      Promise.resolve({ ...CUTSCENE, entity: id, message: `Shot ${index + 1} is now ${CUTSCENE.rows[index]?.reads ?? "back on its card"}` }),
  } as unknown as EditorClient;
};

/** Nothing rendered yet - the zero row every render answer in this file is built from. */
const RENDER_IDLE = {
  running: false,
  done: false,
  entity: "rig",
  frames: 0,
  written: 0,
  width: 0,
  height: 0,
  offscreen: false,
  // ADR-182 — a render delivers a MOVIE unless the author asks for the frames, so this is the state
  // the dialog opens in and the one a capture has to be taken of.
  format: "movie" as const,
  bitrate: 0,
  fps: 24,
  seconds: 0,
  folder: "",
  stem: "Skid Weld Line",
  bytes: 0,
  elapsedMs: 0,
  failures: [] as string[],
  message: "",
  reason: null as string | null,
};

/** A finished render, with the numbers a real one produces. ADR-177: 1080 lines of a 2.39:1 delivery
 *  is 2582x1080 - a DELIVERY size and not the stage's, which is the whole difference this pass made.
 *  The stage a capture of this dialog is taken on is 1400x900, and the frames are taller than it. */
const RENDER_DONE = {
  ...RENDER_IDLE,
  done: true,
  frames: 319,
  written: 319,
  width: 2582,
  height: 1080,
  offscreen: true,
  seconds: 13.3,
  folder: "C:/renders/skid-weld-line",
  // ADR-182 — a movie is ONE file, so what it weighs is that file rather than 319 lossless frames.
  // 13.3s of 2582x1080 at 8.0 Mbit/s is about 13 MB, against the ~155 MB the same cut costs as 319
  // lossless PNGs (the number this fixture carried before ADR-182).
  bytes: 13_337_000,
  bitrate: 8_022_528,
  elapsedMs: 41_800,
  message: "Rendered 319 frames at 2582x1080 in 41.8s into Skid Weld Line.mp4",
};

/** ADR-175 - the same cutscene with a render PLAN behind it. Delivered in scope, because the frame a
 *  cut is composed for decides the shape of every file the render writes, and a capture of the dialog
 *  over a viewport-shaped cut could not show that.
 *
 *  The plan's numbers are the fixture's, and computing them is the ENGINE's job in the product: 13.3s
 *  at 24 fps is 319 frames. A stub answering a different count would photograph a dialog whose cost
 *  sentence and whose button disagreed - the defect the plan command exists to make impossible. */
const renderingCutsceneClient = () =>
  ({
    ...cutsceneClient(),
    cinemaList: () => Promise.resolve({ ...CUTSCENE, delivery: "scope" as const }),
    cinemaRenderPlan: (
      id: string,
      fps: number,
      shot: number | null,
      height: number | null = null,
      format: "movie" | "sequence" | null = null,
    ) => {
      const seconds = shot === null ? CUTSCENE.seconds : (CUTSCENE.rows[shot]?.effectiveSeconds ?? 0);
      const frames = Math.max(1, Math.round(seconds * fps));
      // ADR-177 - the size is the ENGINE's answer in the product; here it is the same arithmetic over
      // this fixture's scope delivery, so the sentence photographed is the one a real plan produces.
      const width = height === null ? 1920 : Math.round((height * 2.39) / 2) * 2;
      const tall = height ?? 803;
      // ADR-182 — the bit rate is the ENGINE's `bitrate_for` in the product; here it is the same
      // arithmetic over this fixture, so the sentence photographed is one a real plan produces.
      const chosen = format ?? "movie";
      const bitrate =
        chosen === "movie"
          ? Math.min(120_000_000, Math.max(2_000_000, Math.floor((width * tall * fps * 12) / 100)))
          : 0;
      return Promise.resolve({
        ...RENDER_IDLE,
        entity: id,
        fps,
        frames,
        seconds,
        width,
        height: tall,
        format: chosen,
        bitrate,
        message: `${frames} frames \u00b7 ${seconds.toFixed(1)}s at ${fps} fps \u00b7 ${width}x${tall}`,
      });
    },
    cinemaRenderStatus: () => Promise.resolve(RENDER_DONE),
    cinemaRenderCancel: () => Promise.resolve(RENDER_DONE),
  }) as unknown as EditorClient;

/** ADR-190 — the same cutscene with the five render answers ALREADY AUTHORED.
 *
 *  NOT ONE OF THEM IS A DEFAULT, and that is the entire point of the scene it feeds: before this pass
 *  the dialog's four pickers were `useState` initialisers, so a fixture holding a stored answer and a
 *  fixture holding nothing photographed the same dialog. The folder is the fifth, and it was never a
 *  field at all — the operating system asked for it after the click. */
const REMEMBERED_RENDER = {
  format: "sequence" as const,
  fps: 60,
  height: 1440,
  name: "weld-line-master",
  folder: "D:/Deliveries/Skid Weld Line/2026-08-31 master",
};

const rememberedRenderClient = () =>
  ({
    ...renderingCutsceneClient(),
    cinemaList: () =>
      Promise.resolve({ ...CUTSCENE, delivery: "scope" as const, render: REMEMBERED_RENDER }),
  }) as unknown as EditorClient;

/** The same client, one click further on: the render has finished and the dialog is its ledger. */
const renderedCutsceneClient = () =>
  ({
    ...renderingCutsceneClient(),
    cinemaRenderStart: () => Promise.resolve(RENDER_DONE),
  }) as unknown as EditorClient;

/** The same cutscene, delivered in scope. One field differs, and it changes what every shot in the
 *  list is composed for — which is why it belongs to the CUT and not to a shot. */
const deliveredCutsceneClient = () =>
  ({
    ...cutsceneClient(),
    cinemaList: () => Promise.resolve({ ...CUTSCENE, delivery: "scope" as const }),
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
    id: "cutscene-timeline",
    looking_for:
      "A CUTSCENE AS A SEQUENCE IN TIME. What this panel replaced was a bulleted list of five " +
      "sentences with a × beside each: no length on screen anywhere, no way to reorder, and no way " +
      "to change a shot without deleting it and re-authoring everything after it — while the engine " +
      "had carried a per-shot `seconds`, an ordered list and a six-by-six-by-six framing vocabulary " +
      "the whole time. Check that the five bars are five DIFFERENT widths in the ratio 2.5 : 4.0 : " +
      "1.6 : 3.0 : 2.2 — equal bars would mean the panel is drawing a list again — that each carries " +
      "its own duration, and that the ruler above them is labelled in seconds with a playhead that " +
      "has a handle. The first bar reads Assembly Hall, not Weld Gun 7: a shot may film something " +
      "other than the object its cutscene hangs on, and every line in the old list was captioned " +
      "with the owner. Below, the shot inspector: length, size, angle, move and strength, each one " +
      "a control from the shared field family and each one edit landing as a single undoable commit. " +
      "The jump-cut warning is the engine's own continuity check, shown where the shots are",
    // The WINDOW, not a frame cap. `width` leaves the window at 620px and merely caps the frame, so
    // a panel that measures its own container to decide how wide to draw a lane gets photographed
    // fitting a 620px box under a caption about a full-window dock. Found by this scene: the last
    // two shots of a five-shot cut were scrolled off the right-hand edge of the capture.
    viewport: { width: 1400, height: 900 },
    setup: selectAnimatedEntity,
    // Without the click this photographs the timeline with no shot inspector under it — a capture of
    // half the panel under a caption describing all of it.
    click: ["[data-testid='cutscene-clip']"],
    expect: {
      present: [
        ["[data-testid='cutscene-clip']", 5],
        ["[data-testid='cutscene-shot-editor']", 1],
        ["[data-testid='cutscene-problem']", 1],
        // The framing vocabulary is on screen as three real selects, not as prose.
        ["[data-testid='cutscene-size']", 1],
        ["[data-testid='cutscene-angle']", 1],
        ["[data-testid='cutscene-motion']", 1],
        // ...and the catalogue is still one click away from the timeline it feeds.
        ["[data-testid='shot-catalogue'] .mtk-btn", 6],
      ],
      text_present: [
        "Assembly Hall",
        "Weld Gun 7",
        "Shot 1 of 5",
        // The total, and the one number the old list could not show at all.
        "13.3",
        "jump cut",
      ],
      // The empty states this scene must NOT be photographing.
      text_absent: ["No object selected", "has no cutscene yet", "null", "undefined", "NaN"],
      // A bar you cannot see is a shot you cannot select, and the shortest one here is 1.6s of 13.3.
      min_width: [["[data-testid='cutscene-clip']", 24]],
      unclipped: [
        "[data-testid='cutscene-clip']",
        "[data-testid='cutscene-shot-editor'] .mtk-select",
        "[data-testid='cutscene-shot-editor'] .mtk-btn",
        "[data-testid='cutscene-panel'] > .mtk-toolbar .mtk-btn",
      ],
      // The ruler labels sit ABOVE the lane they measure, and the inspector under the timeline.
      stacked: [["[data-testid='cutscene-timeline']", "[data-testid='cutscene-shot-editor']"]],
      // Earlier/Later/Remove are one row, not three: an order control that wrapped onto its own line
      // is the toolbar having run out of width.
      same_line: [["[data-testid='cutscene-earlier']", "[data-testid='cutscene-remove']"]],
    },
    render: () => <CutscenePanel client={cutsceneClient()} />,
  },
  {
    id: "cutscene-placed-camera",
    looking_for:
      "A SHOT THE AUTHOR PLACED BY EYE. Every shot before this one was a CARD — one of six sizes " +
      "crossed with one of six angles, solved relative to the subject's own facing — and the " +
      "vocabulary is why the first click looks good, but it could not express the most basic " +
      "gesture in cinematography: orbit until the frame is the one you want, then shoot THAT. The " +
      "renderer has taken an arbitrary eye/look-at/lens since ADR-168; nothing in the editor could " +
      "hand it one. Check three things, and they are the whole decision. First, the camera row sits " +
      "ABOVE the framing grid and says 'Re-shoot from this view' beside 'Use the card again', " +
      "because a placed camera is not one more axis of the card — it REPLACES two of them. Second, " +
      "Size and Angle are visibly DISABLED and their help lines say why in plain words ('Set by the " +
      "camera you placed'), rather than sitting there enabled and deciding nothing; the values are " +
      "not cleared, because they are the framing 'Use the card again' restores. Third, the eye, the " +
      "aim and the lens are on screen as NUMBERS the author can read against the preview's own " +
      "read-out — a 'Placed' badge would be a claim, and three coordinates are evidence. The " +
      "sentence under the heading reads 'a placed shot of ...', not 'a very close shot ... in " +
      "profile': captioning a hand-framed wide with the leftover card is this list's one job done " +
      "wrong. Move and Move strength stay LIVE, which is the difference between an escape hatch and " +
      "a first-class shot — 'put the camera here and push in' is a sentence this engine can film",
    viewport: { width: 1400, height: 900 },
    setup: selectAnimatedEntity,
    click: ["[data-testid='cutscene-clip']"],
    expect: {
      present: [
        ["[data-testid='cutscene-shot-editor']", 1],
        ["[data-testid='cutscene-shoot-here']", 1],
        ["[data-testid='cutscene-use-card']", 1],
        ["[data-testid='cutscene-placed-pose']", 1],
      ],
      text_present: [
        "Re-shoot from this view",
        "Use the card again",
        "a placed shot of",
        "Set by the camera you placed",
        // The lens the viewport actually draws through — `CAMERA_FOV_DEG`, not the probe's old 45.
        "55",
      ],
      // The caption a placed shot must NOT be wearing — its own leftover card, read back as if it
      // still decided the frame.
      text_absent: ["a wide shot of Assembly Hall", "null", "undefined", "NaN"],
      disabled: ["[data-testid='cutscene-size']", "[data-testid='cutscene-angle']"],
      // ...while the two controls a placed camera does NOT take over stay usable.
      enabled: ["[data-testid='cutscene-motion']", "[data-testid='cutscene-amount']"],
      unclipped: [
        "[data-testid='cutscene-shoot-here']",
        "[data-testid='cutscene-use-card']",
        "[data-testid='cutscene-placed-pose']",
      ],
      // The camera row is above the grid whose two controls it disables.
      stacked: [
        ["[data-testid='cutscene-shoot-here']", "[data-testid='cutscene-size']"],
      ],
      same_line: [["[data-testid='cutscene-shoot-here']", "[data-testid='cutscene-use-card']"]],
    },
    render: () => <CutscenePanel client={placedCameraClient()} />,
  },
  {
    id: "cutscene-render",
    looking_for:
      "THE WAY A PICTURE GETS OUT OF THIS ENGINE. Until this dialog existed there was none: the " +
      "shell wrote no image file anywhere, and every still of this project's own benchmark film was " +
      "an operating-system screenshot taken by a script outside the engine - while the renderer had " +
      "been reading its own frames back to PNG since M14.2 and the shot solver could pose the " +
      "camera at any instant. Check that all three moments of the task are here and in this order: " +
      "WHAT will be written (the scope, the FRAME SIZE, the rate, the name), WHAT IT COSTS stated " +
      "above the button that pays it - a frame count AND a pixel size that are the ENGINE's own " +
      "plan, not this dialog's arithmetic - and what that choice means, said before the click " +
      "rather than discovered after it. ADR-177: the size used to be the one thing this dialog " +
      "could not change, because every frame came off the window's own swapchain and was therefore " +
      "as tall as whatever the docks had left; it is now a delivery format, and the dialog opens " +
      "on 1080 rather than on the stage. Check that the size control offers 'As on screen' as well, " +
      "because the stage is still the right answer for a quick look. The description names the " +
      "delivery frame the cut is composed for, because that is what decides the SHAPE of every " +
      "file - and the width follows from it, which is why the picker asks for a HEIGHT and not a " +
      "resolution. ADR-182: the dialog now opens on a MOVIE - one H.264 MP4 - because a render is the "
      + "thing that leaves the editor and 319 numbered PNGs are not something anybody can watch. "
      + "Check that the delivery sits beside WHAT is being filmed and ahead of the rate, the size "
      + "and the name, because it changes what all three of those mean; that the cost line states "
      + "the bit rate the encoder is handed (about 8.0 Mbit/s) "
      + "beside the frame count; and that 'As on screen' is NOT offered here - a movie declares its "
      + "frame size once, before the first sample, and the stage's size is a measurement that moves. "
      + "The lossless sequence is one control away and is still the right answer for a compositor. "
      + "The primary button says the number: 'Render 319 frames', never a bare 'Render'",
    viewport: { width: 1400, height: 900 },
    setup: selectAnimatedEntity,
    click: ["[data-testid='cutscene-render']"],
    expect: {
      present: [
        ["[data-testid='render-dialog']", 1],
        ["[data-testid='render-format']", 1],
        ["[data-testid='render-scope']", 1],
        ["[data-testid='render-fps']", 1],
        ["[data-testid='render-size']", 1],
        ["[data-testid='render-stem']", 1],
        ["[data-testid='render-cost']", 1],
        ["[data-testid='render-start']", 1],
      ],
      text_present: [
        // The frame the cut is composed for - the fixture delivers in scope, and the shape of every
        // written file follows from it.
        "2.39:1 scope",
        // The cost, and the fact that it is a count of FRAMES.
        "319 frames",
        // ADR-182 - what the render DELIVERS, and what one costs, stated above the button that pays
        // for it. The bit rate is the engine's `bitrate_for`, never a multiplication done here.
        "Deliver as",
        "MP4",
        "8.0 Mbit/s",
        // ADR-177 - the size is a choice, and the pixels it comes to are stated before the click.
        "Frame size",
        "2582 × 1080",
      ],
      text_absent: [
        "null",
        "undefined",
        "NaN",
        "0 frames",
        // The sentence this dialog used to end on, before a size could be chosen.
        "the size of the composed picture on screen",
        // ADR-182 - a movie has ONE size for its whole length, so the stage is not one of its
        // options. Present-and-refused would be worse than absent: the author would pick it, read a
        // sentence, and undo what they just did.
        "As on screen",
      ],
      unclipped: [
        "[data-testid='render-start']",
        "[data-testid='render-cost']",
        "[data-testid='render-format']",
        "[data-testid='render-scope']",
        "[data-testid='render-size']",
      ],
      // The cost sits ABOVE the button that pays it. Below it, it is a receipt.
      stacked: [["[data-testid='render-cost']", "[data-testid='render-start']"]],
      // Cancel and Render share the footer row rather than stacking into two full-width bars.
      same_line: [["[data-testid='render-cancel']", "[data-testid='render-start']"]],
    },
    render: () => <CutscenePanel client={renderingCutsceneClient()} />,
  },
  {
    id: "cutscene-render-remembered",
    looking_for:
      "THE DIALOG THAT REMEMBERS WHAT THIS CUT DELIVERS. ADR-190: every one of these answers used " +
      "to be a `useState` seeded from a constant, re-asked on every single render - an author who " +
      "had decided their cut delivers a 1440 scope sequence called 'weld-line-master' re-decided " +
      "it, four controls at a time, every time, and none of the four survived closing the dialog. " +
      "They live on the CUTSCENE now, beside its delivery frame, written as ordinary undoable " +
      "commits and saved with the project. Compare this capture against 'cutscene-render', which " +
      "is the same dialog over a cut nobody has answered for: THAT one opens on a movie at 1080 " +
      "with an empty name, and this one opens on the sequence, 60 fps, 1440 and the stored name - " +
      "because the document said so, not because this component did. " +
      "The fifth answer is the one that was never a field at all: 'Where' was asked for by the " +
      "operating system AFTER the click, and the only surface that ever named it was the ledger " +
      "at the end - so a dialog whose whole argument is that the cost is stated before the button " +
      "that pays it never stated the destination. Check that the row is there, that it names a " +
      "real folder read from its END (a path identifies itself by its last segment, not its drive " +
      "letter), and that 'Choose...' sits beside it. Check too that 'As on screen' IS offered " +
      "here, because this cut delivers a SEQUENCE - the option is absent only while a movie is " +
      "selected, which is ADR-182's refusal turned into a picker that cannot express it",
    viewport: { width: 1400, height: 900 },
    setup: selectAnimatedEntity,
    click: ["[data-testid='cutscene-render']"],
    expect: {
      present: [
        ["[data-testid='render-dialog']", 1],
        ["[data-testid='render-format']", 1],
        ["[data-testid='render-fps']", 1],
        ["[data-testid='render-size']", 1],
        ["[data-testid='render-stem']", 1],
        // ADR-190 - the destination, said before the click, for the first time.
        ["[data-testid='render-folder']", 1],
        ["[data-testid='render-folder-choose']", 1],
        ["[data-testid='render-cost']", 1],
        ["[data-testid='render-start']", 1],
      ],
      text_present: [
        // The stored answers, on screen. The name is the one a person typed, not the object's.
        "Where",
        "2026-08-31 master",
        "Remembered on this cut",
        // ...and the cost sentence is computed FROM them: 13.3s at 60 fps is 798 frames, and 1440
        // lines of a 2.39:1 delivery is 3442 wide. A dialog that painted the stored answers and then
        // planned against the defaults would show 319 and 2582.
        "798 frames",
        "3442x1440",
        "60 fps",
        // ADR-182's refusal, expressed as a picker rather than a sentence: the stage IS offered while
        // a sequence is selected, and disappears the moment a movie is. `cutscene-render` is the
        // other half of that pair and asserts its ABSENCE over the same control.
        "As on screen",
      ],
      text_absent: [
        "null",
        "undefined",
        "NaN",
        "0 frames",
        // The defaults this cut is NOT on - the whole claim in three absences.
        "319 frames",
        "You'll be asked when you render",
      ],
      unclipped: [
        "[data-testid='render-folder']",
        "[data-testid='render-folder-choose']",
        "[data-testid='render-start']",
        "[data-testid='render-cost']",
      ],
      // The destination is a field like the others, above the cost that is above the button.
      stacked: [
        ["[data-testid='render-stem']", "[data-testid='render-folder']"],
        ["[data-testid='render-folder']", "[data-testid='render-cost']"],
      ],
      // The path and the button that changes it are one row: a "Choose..." that wrapped below the
      // path is the field having run out of width.
      same_line: [["[data-testid='render-folder']", "[data-testid='render-folder-choose']"]],
    },
    render: () => <CutscenePanel client={rememberedRenderClient()} />,
  },
  {
    id: "cutscene-render-ledger",
    looking_for:
      "WHERE THE MOVIE WENT. 'Done' is not an answer to 'where is it' - which is exactly what a " +
      "status-bar toast could say and no more. ADR-182: the delivery is one H.264 MP4, so this " +
      "ledger names ONE file rather than a numbered range; check that it does, and that a reader " +
      "can answer 'where is my film' without leaving this dialog. " +
      "A render is 319 frames, and 'done' is not an answer to " +
      "'where are they' - which is exactly what a status-bar toast could say and no more. The " +
      "options are GONE and their space is the ledger: how many frames exist, the pixel size they " +
      "were actually written at (2582x1080 - the delivery the author chose, not the shape of the " +
      "1400x900 stage this capture was taken on), what they weigh, how long it took, and the " +
      "destination path in mono, whole, " +
      "wrapping rather than truncating. Check that the reader can answer 'where are my files' " +
      "without leaving this dialog, and that no settings control is still on screen asking a " +
      "question the reader has stopped having",
    viewport: { width: 1400, height: 900 },
    setup: selectAnimatedEntity,
    click: ["[data-testid='cutscene-render']", "[data-testid='render-start']"],
    expect: {
      present: [
        ["[data-testid='render-ledger']", 1],
        ["[data-testid='render-ledger-frames']", 1],
        ["[data-testid='render-ledger-folder']", 1],
        ["[data-testid='render-done']", 1],
      ],
      // The settings are gone, not merely disabled.
      absent: ["[data-testid='render-fps']", "[data-testid='render-scope']"],
      text_present: [
        "319",
        "2582",
        "1080",
        "Rendered",
        "renders",
        // ADR-182 - ONE file, named, rather than `take.0000.png … take.0318.png` over a folder
        // holding a single movie.
        "Skid Weld Line.mp4",
        "encoded as one H.264 movie",
      ],
      text_absent: ["null", "undefined", "NaN"],
      unclipped: ["[data-testid='render-ledger-folder']", "[data-testid='render-done']"],
    },
    render: () => <CutscenePanel client={renderedCutsceneClient()} />,
  },
  {
    id: "cutscene-shot-subject",
    looking_for:
      "WHAT THIS SHOT FRAMES — the control that was missing while the engine already had the " +
      "capability. A `ShotRecipe` has carried its own `subject` since cutscenes shipped, and the " +
      "runtime resolves it as the union of every rendered instance in that object's HIERARCHY " +
      "SUBTREE — so 'film the whole assembly' was always solvable, the editor simply sent no " +
      "subject and offered no way to change one, which made the most ordinary cinematic sequence " +
      "there is (hold on the whole line, then cut in to one machine) impossible to author. Shot 1 " +
      "here films the hall the gun stands in. Check that the LANE says so — clip 1 carries " +
      "'Assembly Hall' beside its duration and the other four carry only a duration, because " +
      "captioning all five with the same name is the heading repeated five times — and that the " +
      "shot inspector's first framing control is 'Frames', reading back Assembly Hall with a help " +
      "line naming the difference. Frames sits before Size, Angle and Move because all three of " +
      "those are stated RELATIVE to the subject, so every one of them means something else once it " +
      "changes. THE OPEN LIST IS NOT ASSERTED HERE: it is a `theme/Popover`, portalled to " +
      "`document.body` by design, and this gate evaluates claims inside the scene's own frame. Its " +
      "contents — the ranked groups, the parts counts, the nothing-drawn warning — are measured on " +
      "the packaged .exe by `specs-subjectpicker`, against the real engine's own ranking",
    viewport: { width: 1400, height: 900 },
    setup: selectAnimatedEntity,
    // Open shot 1 — the one that films something other than the object its cutscene hangs on.
    click: ["[data-testid='cutscene-clip']"],
    expect: {
      present: [
        ["[data-testid='cutscene-shot-editor']", 1],
        ["[data-testid='cutscene-subject']", 1],
        ["[data-testid='cutscene-subject-name']", 1],
        ["[data-testid='cutscene-clip']", 5],
      ],
      text_present: [
        "Frames",
        "Assembly Hall",
        // The help line under the control, which is where the difference is explained.
        "This shot films Assembly Hall, not Weld Gun 7",
      ],
      text_absent: ["No object selected", "has no cutscene yet", "null", "undefined", "NaN"],
      // A control whose value is ellipsised is a control that does not answer its own question.
      unclipped: ["[data-testid='cutscene-subject']", "[data-testid='cutscene-subject-name']"],
      // The sentence names the subject, and the control that changes it is under the sentence.
      stacked: [["[data-testid='cutscene-shot-reads']", "[data-testid='cutscene-subject']"]],
      // Frames and Size are one row of the framing grid: what a shot is OF and how it is framed
      // belong to the same decision and are read together.
      same_line: [["[data-testid='cutscene-subject']", "[data-testid='cutscene-size']"]],
    },
    render: () => <CutscenePanel client={cutsceneClient()} />,
  },
  {
    id: "cutscene-aim-badge",
    looking_for:
      "AIMING A SHOT BY POINTING AT THE THING. The stage while an aim is in flight. Three engine " +
      "capabilities existed and had never met: `viewport_peek` names what is under the cursor " +
      "WITHOUT changing the selection (and was called by nothing in the editor), " +
      "`cinema_subject_chain` answers with the object and every assembly it belongs to, and " +
      "`cinema_set_shot_subject` re-aims a shot as one undoable edit. What a user could reach was a " +
      "search box, so in a 15,711-part import 'film THAT one' meant knowing its name. Check that " +
      "the badge says which shot is being aimed (shot 2 of 5), that the cursor's object and the " +
      "machine it belongs to are BOTH offered as buttons with their drawn-part counts — 1 part vs " +
      "42 vs 378 is the whole reason a click on one bolt does not have to become a shot of one " +
      "bolt — that the first rung is the emphasised one because it is what the stage click itself " +
      "would take, and that the way out is named on the badge (Esc, and a Cancel beside it). It " +
      "sits at the BOTTOM of the stage on purpose: the preview badge holds the top, and re-aiming a " +
      "shot while previewing it is the loop this closes",
    viewport: { width: 1400, height: 620 },
    expect: {
      present: [
        ["[data-testid='subjectAimBadge']", 1],
        ["[data-testid='subjectAimRungs']", 1],
        ["[data-testid='subjectAimRung-bolt']", 1],
        ["[data-testid='subjectAimRung-rig']", 1],
        ["[data-testid='subjectAimRung-hall']", 1],
        ["[data-testid='subjectAimCancel']", 1],
      ],
      text_present: [
        "AIMING",
        "shot 2 of 5",
        "Bolt M8",
        "1 part",
        "Weld Gun 7",
        "42 parts",
        "Assembly Hall",
        "378 parts",
        "Esc",
      ],
      // The badge is the read-out for a gesture in progress; a hint left standing beside a named
      // ladder would be two answers to the same question.
      text_absent: ["click what this shot should film", "looking", "null", "undefined", "NaN"],
      unclipped: [
        "[data-testid='subjectAimRung-bolt']",
        "[data-testid='subjectAimRung-rig']",
        "[data-testid='subjectAimRung-hall']",
        "[data-testid='subjectAimCancel']",
      ],
      // One pill. A ladder that wrapped its rungs onto a second line under the word AIMING is a
      // badge that has run out of width, and the widen-to-the-assembly choice is the half that goes.
      same_line: [
        ["[data-testid='subjectAimShot']", "[data-testid='subjectAimRung-bolt']"],
        ["[data-testid='subjectAimRung-bolt']", "[data-testid='subjectAimRung-hall']"],
        ["[data-testid='subjectAimRung-hall']", "[data-testid='subjectAimCancel']"],
      ],
    },
    render: () => (
      // The stage, at the size the badge actually stands in: absolutely positioned against the
      // viewport region, bottom-centre.
      <div style={{ position: "relative", height: 560, background: "var(--mtk-bg-inset)" }}>
        <SubjectAimBadge
          shotIndex={1}
          shots={5}
          looking={false}
          rungs={[
            { id: "bolt", name: "Bolt M8", parts: 1, group: "This object" },
            { id: "rig", name: "Weld Gun 7", parts: 42, group: "What it is part of" },
            { id: "hall", name: "Assembly Hall", parts: 378, group: "What it is part of" },
          ]}
          onPick={() => {}}
          onPreview={() => {}}
          onCancel={() => {}}
        />
      </div>
    ),
  },
  {
    id: "cutscene-preview",
    looking_for:
      "THE PLAYHEAD ANSWERING WITH A PICTURE. `solve_shot` has been pure in (recipe, subject, t) " +
      "since cutscenes shipped, so the engine could always produce the camera at any instant — and " +
      "the only way to see one was to press Play and watch the cut from its start. Here the second " +
      "clip has been clicked and Preview turned on. Check that the Preview control reads as PRESSED " +
      "(filled, not outlined — an accent border alone would be a toggle whose state you have to " +
      "know already), that it sits in its own toolbar group rather than crowding the pacing run, " +
      "and that the playhead read-out beside it names the same shot the timeline is highlighting: " +
      "3.1s, shot 2 of 5 — where shot 2 BECOMES ITSELF (its 2.5s start plus its 0.6s opening blend), not where it starts and not how long it runs. This is the toggle's whole job — the author says WHEN, and the viewport " +
      "answers with the frame Play would film at that moment",
    viewport: { width: 1400, height: 900 },
    setup: selectAnimatedEntity,
    // In order: open the second shot, then take the camera. Clicking the toggle first would preview
    // 0.0s and photograph a caption that disagrees with its own picture.
    click: ["[data-testid='cutscene-clip']:nth-of-type(2)", "[data-testid='cutscene-preview']"],
    expect: {
      present: [
        ["[data-testid='cutscene-clip']", 5],
        ["[data-testid='cutscene-preview']", 1],
        ["[data-testid='cutscene-shot-editor']", 1],
        // The pose read-out is the expert half of this control and appears ONLY while a preview is
        // standing somewhere - a scene that did not assert it would photograph the beginner half.
        ["[data-testid='cutscene-preview-pose']", 1],
      ],
      text_present: [
        "Preview",
        "3.1s · shot 2 of 5",
        "Weld Gun 7",
        // Three world coordinates, not a promise of them.
        "6.20, 3.10, 9.40",
        "0.00, 1.40, 0.00",
        "50° lens",
      ],
      text_absent: ["No object selected", "has no cutscene yet", "null", "undefined", "NaN"],
      unclipped: [
        "[data-testid='cutscene-preview']",
        "[data-testid='cutscene-preview-pose']",
        "[data-testid='cutscene-panel'] > .mtk-toolbar .mtk-btn",
      ],
      // The read-out sits between the lane it describes and the inspector for the selected shot.
      stacked: [
        ["[data-testid='cutscene-timeline']", "[data-testid='cutscene-preview-pose']"],
        ["[data-testid='cutscene-preview-pose']", "[data-testid='cutscene-shot-editor']"],
      ],
      // The control that takes the viewport is on the same row as the pacing it sits beside; a
      // Preview button that wrapped onto its own line is the toolbar having run out of width.
      same_line: [["[data-testid='cutscene-mood-calm']", "[data-testid='cutscene-preview']"]],
    },
    render: () => <CutscenePanel client={cutsceneClient()} />,
  },
  {
    id: "cutscene-delivery-frame",
    looking_for:
      "THE FRAME THE SHOTS ARE COMPOSED FOR. A shot solver fits a subject against an ASPECT RATIO — " +
      "it is how far back the camera stands — and until this control the only ratio available was " +
      "whatever shape the author's stage happened to be, so opening a dock silently re-composed the " +
      "film. Check that the delivery picker sits in the toolbar beside pacing (both are properties " +
      "of the whole cut, not of one shot), that it reads '2.39:1 scope' rather than a number, and " +
      "that the pose read-out under the lane now ends with the frame those three coordinates were " +
      "solved for. Match viewport is the ABSENCE of a delivery frame and prints nothing there: this " +
      "capture is the one where it is on",
    viewport: { width: 1400, height: 900 },
    setup: selectAnimatedEntity,
    click: ["[data-testid='cutscene-clip']:nth-of-type(2)", "[data-testid='cutscene-preview']"],
    expect: {
      present: [
        ["[data-testid='cutscene-delivery']", 1],
        ["[data-testid='cutscene-preview-pose']", 1],
      ],
      text_present: ["2.39:1 scope", "composed for"],
      text_absent: ["No object selected", "has no cutscene yet", "null", "undefined", "NaN"],
      unclipped: [
        "[data-testid='cutscene-delivery']",
        "[data-testid='cutscene-preview-pose']",
      ],
      // The frame is chosen in the toolbar and reported under the lane - a control and its
      // consequence, in that order down the panel.
      stacked: [["[data-testid='cutscene-delivery']", "[data-testid='cutscene-preview-pose']"]],
      // ...and it is on the pacing row, not on a line of its own.
      same_line: [["[data-testid='cutscene-mood-calm']", "[data-testid='cutscene-delivery']"]],
    },
    render: () => <CutscenePanel client={deliveredCutsceneClient()} />,
  },
  {
    id: "cutscene-empty",
    looking_for:
      "THE STATE EVERY NEW CUTSCENE STARTS IN, and the one a capture is most likely to skip. An " +
      "object is selected and has no shots: the panel says so in the object's own name, says what " +
      "the first click will do, and puts the whole card catalogue right there. No timeline is drawn " +
      "— a ruler over an empty lane is a clock with nothing on it — and no shot inspector, because " +
      "there is no shot. Nothing here is a dark control waiting to be understood",
    viewport: { width: 1000, height: 700 },
    setup: selectAnimatedEntity,
    expect: {
      present: [["[data-testid='shot-catalogue'] .mtk-btn", 6]],
      absent: ["[data-testid='cutscene-clip']", "[data-testid='cutscene-shot-editor']"],
      text_present: ["Weld Gun 7 has no cutscene yet", "Add a shot"],
      text_absent: ["null", "undefined", "NaN", "0.0s"],
      unclipped: ["[data-testid='shot-catalogue'] .mtk-btn"],
    },
    render: () => (
      <CutscenePanel
        client={
          ({
            cinemaCatalog: () => Promise.resolve(CUTSCENE_CARDS),
            cinemaFramingCatalog: () => Promise.resolve(FRAMING),
            cinemaList: () =>
              Promise.resolve({ ...CUTSCENE, shots: 0, seconds: 0, reads: [], rows: [], problems: [] }),
          }) as unknown as EditorClient
        }
      />
    ),
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
  ...inspectorScenes(),
  ...assetScenes(),
  ...modelScenes(),
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
    // THE WHOLE REPLY SHAPE, and not the fields this scene happens to read. It is cast through
    // `unknown`, so nothing type-checks it, and a stub short of a field the panel reads is a
    // `TypeError` in the page rather than a compile error here: `rows` and `render` were both absent
    // and `cut.rows.length` took the whole harness down with `undefined`.
    cinemaList: () =>
      Promise.resolve({
        entity: null,
        shots: 0,
        seconds: 0,
        mood: "normal",
        delivery: "viewport",
        render: DEFAULT_RENDER_SETTINGS,
        reads: [],
        rows: [],
        problems: [],
        message: "",
        reason: null,
      } satisfies CinemaReply),
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
