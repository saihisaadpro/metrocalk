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
import { Diagnostics } from "../../src/panels/Diagnostics";
import { ImportReport } from "../../src/panels/ImportReport";
import { Reveal } from "../../src/panels/Reveal";
import { projectionStore } from "../../src/store/projection";
import type { CadReport, CadReportPart, RevealResponse } from "../../src/transport/protocol";
import type { EditorClient } from "../../src/transport/session";

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
};

export type Scene = {
  /** Stable id — the `?scene=` parameter and the PNG's filename. Never derived from a title. */
  id: string;
  /** What a reader should be checking in the capture. Printed by `shoot.mjs` beside the filename. */
  looking_for: string;
  /** The machine-checkable part of `looking_for`. A scene without one is rejected by the driver. */
  expect: Expect;
  width?: number;
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

export const SCENES: Scene[] = [
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
];

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
