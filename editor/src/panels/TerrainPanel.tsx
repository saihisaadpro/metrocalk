//! Terrain workspace (M19 / ADR-104) — implemented in accordance with the Engine UI/UX Architecture
//! Constitution.
//!
//! Shared system only: `theme/primitives` for every control, `theme/tokens` for every value, the standard
//! `Panel`/`SectionHeader`/`PropertyRow` grouping, the standard disclosure and validation behaviour. No
//! local styling, no bespoke controls, and nothing here paints the application root — the viewport composite
//! stays transparent (ADR-008).
//!
//! ## Shape of the surface
//!
//! Progressive disclosure, in the order an author actually works:
//!
//! 1. **Start** — a description box and the presets, and nothing else, until a terrain exists. An empty
//!    state that offers the two useful actions beats a panel of disabled controls.
//! 2. **Shape** — the layer stack, sculpt brush and world extent: the controls used constantly.
//! 3. **Describe, Look, Life, Routes, Performance** — collapsed by default, opened when needed.
//!
//! ## Describe-to-build is not a black box
//!
//! The description box shows its **reading** while the text is still editable: every phrase it matched, what
//! that phrase was taken to mean, and which of your words it could not use. What it produces is an ordinary
//! recipe — the layer stack below is the one it wrote, and it is as editable as a hand-authored one. That is
//! the whole reason to route generation through the recipe rather than straight to geometry.
//!
//! Every control commits through one command and is therefore one undo step; the panel never holds authored
//! state of its own. What it renders is what the document returned, so it cannot drift from the truth — and
//! when an edit is refused, the reason and the fix are shown rather than the control silently snapping back.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useStore } from "zustand";
import { projectionStore } from "../store/projection";
import {
  Badge,
  Button,
  NumericField,
  Panel,
  PropertyRow,
  ScrollArea,
  SectionHeader,
  SelectField,
  TextArea,
  TextField,
} from "../theme/primitives";
import { AssetChip, SwatchGrid, SwatchTile } from "../theme/assets";
import { Icon } from "../theme/icons";
import { color, font, fontSize, radius, space, text as textRole } from "../theme/tokens";
import { DisclosureSection } from "../theme/workspace";
import type {
  TerrainIssue,
  TerrainPlan,
  TerrainPathResult,
  TerrainPreset,
  TerrainRecipe,
  TerrainReply,
  TerrainStats,
} from "../transport/protocol";
import type { EditorClient } from "../transport/session";

export interface TerrainPanelProps {
  client: EditorClient;
  /** Poll interval for the profiling readout, in ms. 0 disables polling (tests, and reduced-motion users
   *  who have no reason to watch a counter tick). */
  statsIntervalMs?: number;
  style?: React.CSSProperties;
}

/** Brush kinds, in the order a sculpting toolbar shows them. */
const BRUSH_KINDS = [
  { id: "Raise", label: "Raise / Lower" },
  { id: "Smooth", label: "Smooth" },
  { id: "Flatten", label: "Flatten" },
  { id: "Noise", label: "Roughen" },
] as const;

type BrushKind = (typeof BRUSH_KINDS)[number]["id"];

interface Section {
  id: string;
  title: string;
  defaultOpen: boolean;
}

// The order an author works in, and what is worth seeing without a click.
//
// "Describe a change" leads and opens BY DEFAULT. It used to sit collapsed BELOW the name and seed fields,
// which meant that the moment you finished describing a world, the box you had just used vanished into a
// shut drawer and the panel greeted you with two text inputs. The stated primary workflow was the least
// visible thing on the surface. Identity — name and seed — is real but it is not what you came to do, so it
// moved to the end.
const SECTIONS: Section[] = [
  { id: "describe", title: "Describe a change", defaultOpen: true },
  { id: "shape", title: "Shape", defaultOpen: true },
  { id: "look", title: "Look", defaultOpen: false },
  { id: "life", title: "Life", defaultOpen: false },
  { id: "routes", title: "Routes & Water", defaultOpen: false },
  { id: "perf", title: "Performance", defaultOpen: false },
  { id: "world", title: "This world", defaultOpen: false },
];

/** The ECS component the recipe lives in — `terrain_intent::TERRAIN_COMPONENT` on the engine side.
 *
 * Named here rather than inlined because getting it wrong fails SILENTLY: the lookup simply returns
 * undefined and the panel goes back to trusting only its own replies, which is the exact bug this
 * subscription exists to fix. */
const TERRAIN_COMPONENT = "TerrainRecipe";

/** WHY EVERY CONTROL HERE GOES DARK WHILE A COMMAND IS IN FLIGHT, in the user's words.
 *
 *  One sentence, one constant. Eleven controls in this panel are `disabled={busy}` and, until the
 *  `terrain-authored` capture ran R9 over them, not one of them said so: a control that goes dark for
 *  a reason it will not state is the "every no explained" failure, and it is the same no eleven times.
 *  Writing it eleven times is how ten of them end up saying something slightly different. */
const BUSY_REASON = "The engine is still working on the last change";

const SEVERITY_TONE: Record<TerrainIssue["severity"], "warn" | "accent" | "neutral"> = {
  blocking: "warn",
  warning: "warn",
  info: "neutral",
};

// ── Preset marks ──────────────────────────────────────────────────────────────────────────────────

/** A landform silhouette per preset, drawn in the swatch's well.
 *
 *  WHAT THIS IS AND, MORE IMPORTANTLY, WHAT IT IS NOT. It is a MARK — the same honest position
 *  `AssetTile` already takes when it draws a kind's icon rather than a render: real preview pixels for
 *  a terrain would have to come from the renderer, and the browser build has none. So this never
 *  pretends to be a thumbnail of the world you will get; it says which of six shapes of ground the name
 *  refers to, which is the whole question a picker has to answer, and it answers it before the name is
 *  read. The alternative shipped for two milestones: six headings with a sentence under each, in a
 *  column, with no tile, no image and no affordance — text that happened to be clickable.
 *
 *  Monochrome, two tones, no hue. Six coloured tiles is six competing accents, and the constitution asks
 *  for one disciplined accent rather than a palette per collection (the same argument `AssetTile`'s
 *  `preview` makes). Depth comes from a receding range behind a nearer mass, which survives greyscale,
 *  a colour-blind reader and the high-contrast palette — where both tones are redefined and the drawing
 *  still reads.
 *
 *  Keyed by the preset ID the engine publishes (`terrain/src/preset.rs::all()`). An id with no entry
 *  draws the generic ground plate rather than an empty box, because a preset the engine adds tomorrow
 *  must arrive looking like a preset, not like a bug. */
interface PresetMark {
  /** The far range: lighter, behind. */
  back?: string;
  /** The near mass: the silhouette that carries the landform. */
  front: string;
  /** A third element in the panel tone — snow on a peak, water in a lagoon. */
  highlight?: string;
  /** The highlight is a set of LINES rather than a mass, so it is stroked instead of filled. */
  highlightStroke?: boolean;
}

// ONE GRAMMAR FOR ALL SIX, AND THE FIRST DRAFT DID NOT HAVE IT. Far ground is the lighter tone, near
// ground the darker one, panel-white is the accent on top — that is what makes a set of six read as a
// set rather than as six drawings. The first pass broke it three times and the capture said so at a
// glance: `flat` was a floating outlined diamond among five landscapes anchored to the bottom of their
// well; `archipelago` had painted the SEA in the near tone and the islands in the far one, so the water
// advanced and the land receded; and `canyon`'s notch was a 6px gap in a wall. Depth is the only thing
// these two greys are carrying, so getting the two the wrong way round does not make a worse picture,
// it makes the opposite picture.
const PRESET_MARKS: Record<string, PresetMark> = {
  // Deliberately the calmest of the six: a level far plain over level near ground. There is no shape to
  // draw here and inventing one would misdescribe the preset — a plate is what it is.
  flat: {
    back: "M0 40 H64 V50 H0 Z",
    front: "M0 50 H64 V64 H0 Z",
  },
  "rolling-hills": {
    back: "M0 42 Q12 28 24 40 Q38 53 52 36 Q59 30 64 34 V64 H0 Z",
    front: "M0 52 Q14 39 27 50 Q41 62 64 45 V64 H0 Z",
  },
  alpine: {
    back: "M0 46 L13 21 L24 37 L37 14 L50 39 L64 27 V64 H0 Z",
    front: "M0 54 L16 34 L30 52 L44 31 L58 51 L64 46 V64 H0 Z",
    highlight: "M37 14 L43 25 L37 23 L31 27 Z",
  },
  // Asymmetric on purpose — a long windward back and a short slip face — or it is Rolling Hills again
  // in a set where the two sit side by side.
  dunes: {
    back: "M0 46 C8 36 16 36 24 46 C32 56 40 36 48 42 C55 47 60 44 64 42 V64 H0 Z",
    front: "M0 58 C6 50 12 48 18 54 C24 60 30 48 38 52 C44 55 48 58 54 54 C58 51 61 53 64 55 V64 H0 Z",
  },
  // Land in the near tone, sea in the far one, ripples in white ON the sea. The islands sit on the
  // waterline rather than behind it, which is what makes it an archipelago and not a cliff.
  archipelago: {
    back: "M0 46 H64 V64 H0 Z",
    front: "M4 46 Q16 26 28 46 Z M34 46 Q47 32 60 46 Z",
    highlight: "M6 53 H24 M34 55 H54 M14 59 H44",
    highlightStroke: true,
  },
  // The cut is drawn as ABSENCE: the near mesa simply is not there between x=24 and x=42, so the lighter
  // far wall shows through it, terraced by the step profile at its foot. A notch painted as a line would
  // have been a scratch on a wall — which is what the first version looked like.
  canyon: {
    back: "M0 28 H64 V64 H0 Z",
    front: "M0 28 H16 L22 40 H30 L34 52 H40 L44 40 H52 L58 28 H64 V64 H0 Z",
  },
};

/** The mark for a preset id, with the level-ground plate as the fallback for one we have never seen. */
function presetMark(id: string): PresetMark {
  return PRESET_MARKS[id] ?? PRESET_MARKS.flat;
}

function PresetArt({ id }: { id: string }) {
  const mark = presetMark(id);
  return (
    // Decorative: the tile already states the preset's name, and `SwatchTile` gives the button its
    // accessible name. A second reading of the same fact is noise in a screen reader, not help.
    <svg viewBox="0 0 64 64" role="presentation" aria-hidden focusable="false" preserveAspectRatio="xMidYMid slice">
      {mark.back ? <path d={mark.back} fill={color.border.strong} /> : null}
      <path d={mark.front} fill={color.text.muted} />
      {mark.highlight ? (
        // Filled for a mass (snow), stroked for a line set (the lagoon's water marks) — declared by the
        // mark rather than inferred from its path data, so adding a preset is one table row.
        <path
          d={mark.highlight}
          fill={mark.highlightStroke ? "none" : color.bg.panel}
          stroke={mark.highlightStroke ? color.bg.panel : "none"}
          strokeWidth={2}
          strokeLinecap="round"
        />
      ) : null}
    </svg>
  );
}

/** A layer's kind name, whatever shape serde gave it. */
function layerKind(kind: TerrainLayerKind): string {
  if (typeof kind === "string") return kind;
  const keys = Object.keys(kind ?? {});
  return keys[0] ?? "Layer";
}

type TerrainLayerKind = TerrainRecipe["layers"][number]["kind"];

export function TerrainPanel({ client, statsIntervalMs = 1000, style }: TerrainPanelProps) {
  const [presets, setPresets] = useState<TerrainPreset[]>([]);
  const [recipe, setRecipe] = useState<TerrainRecipe | null>(null);
  const [issues, setIssues] = useState<TerrainIssue[]>([]);
  const [stats, setStats] = useState<TerrainStats | null>(null);
  const [message, setMessage] = useState<string>("");
  const [busy, setBusy] = useState(false);
  const [open, setOpen] = useState<Record<string, boolean>>(() =>
    Object.fromEntries(SECTIONS.map((s) => [s.id, s.defaultOpen])),
  );
  const [brush, setBrush] = useState<{ kind: BrushKind; radiusM: number; strength: number; hardness: number }>({
    kind: "Raise",
    radiusM: 24,
    strength: 2,
    hardness: 0.75,
  });
  const [tool, setTool] = useState<"none" | "sculpt" | "route">("none");
  const [route, setRoute] = useState<{ kind: string; widthM: number; depthM: number; points: number }>({
    kind: "road",
    widthM: 10,
    depthM: 3,
    points: 0,
  });

  const absorb = useCallback((reply: TerrainReply) => {
    setMessage(reply.ok ? "" : reply.message);
    if (reply.recipe) setRecipe(reply.recipe);
    setIssues(reply.issues ?? []);
    if (reply.stats) setStats(reply.stats);
  }, []);

  // Read the current state once on mount: the scene may already contain a terrain (a reopened project, or
  // an undo that brought one back), and a panel that only knows about terrains it created itself would show
  // an empty state over a live landscape.
  useEffect(() => {
    let cancelled = false;
    void client.terrainPresets().then((p) => {
      if (!cancelled) setPresets(p);
    });
    void client
      .terrainEdit(null)
      .then((r) => {
        if (!cancelled && r.recipe) absorb(r);
      })
      .catch(() => {
        /* no terrain yet — the empty state is the correct answer, not an error */
      });
    return () => {
      cancelled = true;
    };
  }, [client, absorb]);

  // The DOCUMENT is the authority on whether a terrain exists and what it is — not this panel's memory of
  // what it happens to have created. Undo, redo, opening a project, the command palette and any other
  // surface can all add, change or remove a terrain while this panel sits open, and a panel that trusts
  // only its own replies then shows an empty state over a live landscape, or a stale recipe over a changed
  // one. Reading once on mount covered exactly one of those cases.
  //
  // Subscribing to the terrain's own serialized recipe makes the check a string comparison: it changes when
  // and only when the terrain does.
  const docSource = useStore(projectionStore, (st) => {
    for (const id of st.order) {
      const src = st.base[id]?.components?.[TERRAIN_COMPONENT]?.source;
      if (typeof src === "string") return src;
    }
    return null;
  });
  const lastSource = useRef<string | null>(null);
  useEffect(() => {
    if (docSource === lastSource.current) return undefined;
    lastSource.current = docSource;
    if (docSource === null) {
      // Undone or closed: the empty state is now the truthful answer.
      setRecipe(null);
      setIssues([]);
      setStats(null);
      return undefined;
    }
    // Coalesced, because a sculpt gesture commits many small edits in a row and each one lands here.
    const t = setTimeout(() => {
      void client
        .terrainEdit(null)
        .then((r) => {
          if (r.recipe) absorb(r);
        })
        .catch(() => undefined);
    }, 150);
    return () => clearTimeout(t);
  }, [client, docSource, absorb]);

  useEffect(() => {
    if (!statsIntervalMs || !recipe) return undefined;
    const t = setInterval(() => {
      void client.terrainStats().then(setStats).catch(() => undefined);
    }, statsIntervalMs);
    return () => clearInterval(t);
  }, [client, recipe, statsIntervalMs]);

  // Arm the pointer whenever the tool or the brush changes. The viewport then does the aiming, the
  // tracing and the preview natively — the panel's job ends at "here is what the brush is".
  useEffect(() => {
    void client
      .terrainTool(tool, {
        kind: BRUSH_KINDS.findIndex((b) => b.id === brush.kind),
        radiusM: brush.radiusM,
        strength: brush.strength,
        hardness: brush.hardness,
        targetM: 0,
      })
      .catch(() => undefined);
  }, [client, tool, brush]);

  // Leaving the workspace must disarm the pointer, or a stray click keeps sculpting in another panel.
  useEffect(
    () => () => {
      void client.terrainTool("none").catch(() => undefined);
    },
    [client],
  );

  const run = useCallback(
    async (fn: () => Promise<TerrainReply>, failure: string) => {
      setBusy(true);
      try {
        absorb(await fn());
      } catch {
        setMessage(failure);
      } finally {
        setBusy(false);
      }
    },
    [absorb],
  );

  const edit = useCallback(
    (op: Record<string, unknown>, failure: string) => run(() => client.terrainEdit(op), failure),
    [client, run],
  );

  const blocking = useMemo(() => issues.filter((i) => i.severity === "blocking"), [issues]);
  const advisories = useMemo(() => issues.filter((i) => i.severity !== "blocking"), [issues]);

  const toggle = (id: string) => setOpen((o) => ({ ...o, [id]: !o[id] }));

  if (!recipe) {
    return (
      <Panel style={style} data-testid="terrain-panel" data-state="empty">
        {/* No PanelHeader: the dock that hosts this panel already carries the title and subtitle, and a
            second "Terrain" heading directly beneath the first is the kind of duplication that makes an
            interface feel unowned. */}
        {/* TWO WAYS IN, EACH A LABELLED GROUP, AND NO ESSAY.
            This surface used to open with a five-line paragraph explaining what a recipe is, above a
            description box, above six headings-with-a-sentence that were `Button`s and read as body
            copy. Three problems, one cause — the panel was explaining itself in prose where the
            reference sheets show a grid. The prose is gone (a recipe explains itself the moment there
            IS one, and the panel below is that explanation), and the presets are what the constitution's
            asset-browser section already asks for and `SwatchGrid` already implements: large previews on
            a responsive grid. Six choices now fit in the space two of them used, and each one shows the
            shape of the ground it makes before its name is read.

            The headings sit flush with what they label. `SectionHeader` used to carry its own 12px
            inline padding on top of whatever container it was in, which is why the old preset heading
            started 12px to the right of the choices below it; that indent now lives in one place. */}
        <div style={{ padding: space.lg, display: "grid", gap: space.xl, minWidth: 0 }}>
          <div style={{ display: "grid", gap: space.sm, minWidth: 0 }}>
            <SectionHeader>Describe it</SectionHeader>
            <DescribeBox client={client} busy={busy} run={run} />
          </div>

          <div style={{ display: "grid", gap: space.sm, minWidth: 0 }}>
            <SectionHeader>Or start from a preset</SectionHeader>
            <SwatchGrid label="Terrain presets" data-testid="terrain-presets">
              {presets.map((p) => (
                <SwatchTile
                  key={p.id}
                  label={p.name}
                  preview={<PresetArt id={p.id} />}
                  // The description is the tooltip rather than a third line under every tile. It is one
                  // sentence about what the preset produces and what it is good for — worth having, and
                  // not worth six copies of it turning a grid back into the column this replaced.
                  title={p.description}
                  actionLabel={`Create ${p.name}`}
                  disabled={busy}
                  disabledReason={BUSY_REASON}
                  onSelect={() => void run(() => client.terrainCreate(p.id), `Couldn’t create ${p.name}`)}
                  data-testid={`terrain-preset-${p.id}`}
                />
              ))}
            </SwatchGrid>
          </div>

          {message ? (
            <p role="alert" style={{ margin: 0, color: color.warn.text, fontSize: fontSize.meta }}>
              {message}
            </p>
          ) : null}
        </div>
      </Panel>
    );
  }

  return (
    <Panel style={style} data-testid="terrain-panel">
      {/* The memory readout stays — it is genuinely useful at a glance — but as a quiet inline row rather
          than a second panel header competing with the dock's. */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "flex-end",
          padding: `${space.xs}px ${space.md}px 0`,
        }}
      >
        <Badge tone={stats?.overBudget ? "warn" : "neutral"} title="Resident terrain memory against the budget">
          {stats ? `${stats.totalMb.toFixed(0)} / ${stats.budgetMb} MB` : "—"}
        </Badge>
      </div>
      {/* `minWidth: 0` is load-bearing: a CSS grid item defaults to `min-width: auto`, so one long
          unbreakable string (a preset description, a described sentence) widens the track past the
          dock and the overflow is clipped rather than wrapped. */}
      <ScrollArea style={{ padding: space.md, display: "grid", gap: space.md, minWidth: 0 }}>
        {message ? (
          <p
            role="alert"
            data-testid="terrain-message"
            style={{
              margin: 0,
              padding: space.sm,
              borderRadius: radius.sm,
              background: color.warn.bg,
              color: color.warn.text,
              fontSize: fontSize.meta,
            }}
          >
            {message}
          </p>
        ) : null}

        {/* What the ENGINE is currently unhappy about, as opposed to what the recipe validator says. A
            recipe can be perfectly valid and still have a chunk whose build panicked, and the author sees
            that as a hole in the ground with no explanation anywhere. This is the explanation. */}
        {stats?.problem ? (
          <p
            role="alert"
            data-testid="terrain-problem"
            style={{
              margin: 0,
              padding: space.sm,
              borderRadius: radius.sm,
              background: color.danger.bg,
              color: color.danger.text,
              fontSize: fontSize.meta,
            }}
          >
            {stats.problem}
          </p>
        ) : null}

        {/* THE PANEL'S STRUCTURE IS THE SHARED SECTION NOW.
            This strip used to be a `SectionHeader` wrapping a ghost `Button` that carried its own
            `aria-expanded`, its own rotating chevron on the RIGHT, and its own comment explaining that
            the title rendered quieter than the sub-headings inside it. Every one of those is something
            `DisclosureSection` already decides once for the whole editor — caret on the left, a title
            at section weight, a summary slot, the animated open/close, the keyboard semantics — and
            the complaint in that comment was true of the shared component too until the same change
            raised `.mtk-disclosure__title` to body size.
            `toggleTestId` keeps `terrain-section-*` on the CONTROL, which is what `TerrainPanel.test`
            reads `aria-expanded` from and what the packaged-`.exe` terrain spec clicks. */}
        {SECTIONS.map((section) => (
          <DisclosureSection
            key={section.id}
            title={section.title}
            toggleTestId={`terrain-section-${section.id}`}
            open={open[section.id]}
            onOpenChange={() => toggle(section.id)}
            density="compact"
            landmark={false}
            // The contents were only ever mounted while open, and several of them are heavy (a stats
            // poll, a route editor, a materials list). Keeping that is not an optimisation, it is the
            // existing behaviour — the default would have mounted all seven at once.
            unmountOnClose
          >
            {(
              <div style={{ display: "grid", gap: space.sm, minWidth: 0 }}>
                {section.id === "describe" ? (
                  <>
                    <DescribeBox client={client} busy={busy} run={run} compact />
                    {/* ONE LINE, NOT A PARAGRAPH. This was 33 words across five lines in a 276px column,
                        and it said three things: rebuilding is one undo step; the sections below are what
                        the description wrote; they are editable. The second and third are demonstrated by
                        the sections themselves being right there and being ordinary controls — a panel
                        that has to tell you its fields are editable has a different problem. What is left
                        is the one fact the surface cannot show you: what pressing Rebuild costs. */}
                    <span style={{ fontSize: fontSize.meta, color: color.text.secondary, lineHeight: 1.5 }}>
                      Rebuilding replaces this world, in one undo step.
                    </span>
                  </>
                ) : null}
                {section.id === "shape" ? (
                  <ShapeSection
                    recipe={recipe}
                    busy={busy}
                    brush={brush}
                    setBrush={setBrush}
                    edit={edit}
                    tool={tool}
                    setTool={setTool}
                  />
                ) : null}
                {section.id === "look" ? <LookSection recipe={recipe} /> : null}
                {section.id === "life" ? <LifeSection recipe={recipe} busy={busy} edit={edit} /> : null}
                {section.id === "routes" ? (
                  <RoutesSection
                    recipe={recipe}
                    busy={busy}
                    client={client}
                    tool={tool}
                    setTool={setTool}
                    route={route}
                    setRoute={setRoute}
                    absorb={absorb}
                    edit={edit}
                  />
                ) : null}
                {section.id === "perf" ? <PerfSection recipe={recipe} stats={stats} busy={busy} edit={edit} /> : null}
                {section.id === "world" ? (
                  <>
                    <PropertyRow label="Name" htmlFor="terrain-name">
                      <TextField
                        value={recipe.name}
                        data-testid="terrain-name"
                        id="terrain-name"
                        aria-label="Terrain name"
                        onChange={(e) =>
                          void edit({ op: "rename", name: e.target.value }, "Couldn’t rename the terrain")
                        }
                      />
                    </PropertyRow>
                    <PropertyRow
                      label="Seed"
                      help="A different seed is a different world with the same art direction."
                      htmlFor="terrain-seed"
                    >
                      <NumericField
                        value={recipe.seed}
                        step={1}
                        data-testid="terrain-seed"
                        id="terrain-seed"
                        ariaLabel="Terrain seed"
                        onCommit={(v) =>
                          void edit({ op: "setSeed", seed: Math.max(0, Math.round(v)) }, "Couldn’t change the seed")
                        }
                      />
                    </PropertyRow>
                  </>
                ) : null}
              </div>
            )}
          </DisclosureSection>
        ))}

        {issues.length > 0 ? (
          <div data-testid="terrain-issues" style={{ display: "grid", gap: space.xs }}>
            <SectionHeader>
              {blocking.length > 0 ? `${blocking.length} problem${blocking.length === 1 ? "" : "s"}` : "Notes"}
            </SectionHeader>
            {[...blocking, ...advisories].map((issue) => (
              <div
                key={`${issue.field}:${issue.message}`}
                style={{
                  display: "grid",
                  gap: space.xxs,
                  padding: space.sm,
                  borderRadius: radius.sm,
                  background: issue.severity === "info" ? color.bg.raised : color.warn.bg,
                }}
              >
                <span style={{ display: "flex", gap: space.xs, alignItems: "center" }}>
                  <Badge tone={SEVERITY_TONE[issue.severity]}>{issue.severity}</Badge>
                  <code style={{ fontFamily: font.mono, fontSize: fontSize.micro, color: color.text.muted }}>
                    {issue.field}
                  </code>
                </span>
                <span style={{ fontSize: fontSize.meta, color: color.text.primary }}>{issue.message}</span>
                {issue.fix ? (
                  <span style={{ fontSize: fontSize.meta, color: color.text.secondary }}>Fix: {issue.fix}</span>
                ) : null}
              </div>
            ))}
          </div>
        ) : null}
      </ScrollArea>
    </Panel>
  );
}

type EditFn = (op: Record<string, unknown>, failure: string) => Promise<void>;

/**
 * Ask the engine whether an authored road can actually be driven.
 *
 * The navigation grid was being built for every collider chunk and then read by nothing — the one command
 * that consumed it had no caller outside a test. This is its surface, and it closes the loop on the
 * traversability verb: you say "make this valley traversable", and then you check that something can cross
 * it. The answer comes from the same nav grids an agent would use, so it cannot flatter the terrain.
 */
function RouteCrossingCheck({
  client,
  recipe,
  busy,
}: {
  client: EditorClient;
  recipe: TerrainRecipe;
  busy: boolean;
}) {
  const [index, setIndex] = useState(0);
  const [result, setResult] = useState<TerrainPathResult | null>(null);
  const [checking, setChecking] = useState(false);
  const spline = recipe.splines[Math.min(index, recipe.splines.length - 1)];

  // The route's own two ends. Nav works in world space and the control points already are world space, so
  // this asks exactly the question the author means: "end to end, is this thing passable?"
  const ends = useMemo((): [[number, number, number], [number, number, number]] | null => {
    const pts = spline?.points ?? [];
    if (pts.length < 2) return null;
    return [pts[0], pts[pts.length - 1]];
  }, [spline]);

  return (
    <div style={{ display: "grid", gap: space.xs, minWidth: 0 }}>
      <div style={{ display: "flex", gap: space.xs, alignItems: "center", minWidth: 0 }}>
        <SelectField
          value={String(index)}
          aria-label="Route to check"
          data-testid="terrain-crossing-route"
          onChange={(e) => {
            setIndex(Number(e.target.value));
            setResult(null);
          }}
          style={{ flex: 1, minWidth: 0 }}
        >
          {recipe.splines.map((sp, i) => (
            <option key={`${sp.name}-${i}`} value={String(i)}>
              {sp.name || `Route ${i + 1}`}
            </option>
          ))}
        </SelectField>
        <Button
          variant="secondary"
          compact
          data-testid="terrain-crossing-check"
          disabled={busy || checking || !ends}
          disabledReason={
            busy ? BUSY_REASON : checking ? "Still tracing the last route" : "Pick two ends first — this checks whether a path exists between them"
          }
          onClick={() => {
            if (!ends) return;
            setChecking(true);
            void client
              .terrainPath(ends[0], ends[1])
              .then(setResult)
              .catch(() => setResult(null))
              .finally(() => setChecking(false));
          }}
        >
          {checking ? "Checking…" : "Can it be crossed?"}
        </Button>
      </div>
      {result ? (
        <span
          data-testid="terrain-crossing-result"
          style={{
            fontSize: fontSize.meta,
            lineHeight: 1.5,
            color: result.found ? color.success.text : color.warn.text,
          }}
        >
          {result.found
            ? `Yes — a route exists, ${Math.round(result.lengthM ?? 0)} m end to end.`
            : `No — ${result.reason}`}
        </span>
      ) : null}
    </div>
  );
}


/** Descriptions offered as one-click pills, so the feature is discoverable without a blank-page problem.
 *
 *  A SHORT LABEL AND THE FULL SENTENCE ARE TWO DIFFERENT JOBS, and printing the sentence was doing
 *  neither. Five complete descriptions, stacked full-width in a 276px column, took five wrapped rows —
 *  more vertical space than the description box, the button and the status line together — to say the
 *  same thing a pill says in two words. What a reader needs here is *that there are examples and roughly
 *  what kinds*; what the box needs is the sentence. So the pill shows the kind, the tooltip shows the
 *  sentence, and clicking puts the sentence in the box unchanged. Nothing was shortened — the text
 *  field still receives every word the old row carried. */
const EXAMPLES: { label: string; text: string; mode: "create" | "change" }[] = [
  { label: "Alpine valley", text: "a 4 km eroded alpine valley with a river and dense conifer forest", mode: "create" },
  { label: "Tropical islands", text: "a lush tropical archipelago with beaches and palms", mode: "create" },
  // The second half of the feature: refining the world you already have.
  { label: "Raise a mountain", text: "raise this mountain by 150 m", mode: "change" },
  { label: "Widen the river", text: "widen the river and make this valley traversable", mode: "change" },
  { label: "Plant a forest", text: "plant a dense forest here", mode: "change" },
];

/**
 * Describe-to-build.
 *
 * Two things make this trustworthy rather than a slot machine. First, the reading is shown **before** you
 * commit: you can see that "wizards" was not understood while the text is still editable. Second, what comes
 * out is an ordinary recipe — the layer stack, materials and rules below are the ones it wrote, and they are
 * as editable as if you had typed them yourself.
 */
/** The status line beside `Build it`, which is also its description. One constant, two readers — the
 *  same reason `ANNOTATION_STATUS_ID` is one in `AnimationWorkspace`: a mistyped `aria-describedby`
 *  raises nothing, it just goes quiet. */
const DESCRIBE_STATUS_ID = "terrain-describe-status";

function DescribeBox({
  client,
  busy,
  run,
  compact = false,
}: {
  client: EditorClient;
  busy: boolean;
  /** Sends the command and folds the reply into the panel — the same path every other control uses. */
  run: (fn: () => Promise<TerrainReply>, failure: string) => Promise<void>;
  compact?: boolean;
}) {
  const [text, setText] = useState("");
  const [plan, setPlan] = useState<TerrainPlan | null>(null);

  // Compile as the author types, debounced. This never builds anything and never commits, so it is safe to
  // run on a keystroke — and it is what turns "type and hope" into "type and see". It also resolves spatial
  // targets, so the preview names the landform "this" refers to before anything happens to it.
  useEffect(() => {
    if (!text.trim()) {
      setPlan(null);
      return undefined;
    }
    const t = setTimeout(() => {
      void client
        .terrainPlan(text)
        .then(setPlan)
        .catch(() => setPlan(null));
    }, 220);
    return () => clearTimeout(t);
  }, [client, text]);

  const submit = () =>
    void run(() => client.terrainDescribe(text), "Couldn’t build that description").then(() => {
      // Keep the text: the next thing an author does is refine the sentence, not retype it.
    });

  return (
    <div style={{ display: "grid", gap: space.sm, minWidth: 0 }} data-testid="terrain-describe">
      <TextArea
        value={text}
        rows={compact ? 2 : 3}
        // AN EMPTY BOX THAT LOOKS FULL, BESIDE A LINE SAYING IT IS EMPTY. The placeholder was a bare
        // complete sentence — the same shape, and nearly the same words, as the first `Try one` chip
        // directly below it, which IS a value you can click into the box. So the panel showed what
        // reads as a typed description, greyed out `Build it`, and explained the refusal with "Type a
        // description first": the words are correct and the box contradicts them. `e.g.` is the
        // cheapest thing that makes the field agree with its own status line, and it keeps the
        // example, which is most of what a newcomer needs here. No test asserts this string and none
        // is added — a gate keyed on user-facing prose breaks silently when the prose changes, which
        // is the failure this repository has already paid for twice. The evidence is a before/after
        // capture of the real panel in `progress/visual-acceptance-2026-08-25/`.
        placeholder="e.g. a 4 km eroded alpine valley with a river and dense pine forest"
        aria-label="Describe the terrain"
        data-testid="terrain-describe-text"
        id="terrain-describe-text"
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          // Enter builds; Shift+Enter is a newline, because a description can be more than one line.
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            if (text.trim() && !busy) submit();
          }
        }}
      />
      {/* THE WORDS WERE ALREADY ON SCREEN AND WERE NOT JOINED TO THE CONTROL. The line to the left of
          the button says "Describe a world, or change this one" — which IS the reason `Build it` is
          off, and it is a foot away with nothing tying the two together. A sighted user can infer it;
          a hover gets nothing, a screen reader gets nothing, and a gate reading the DOM can see no
          relationship at all, because there is none (WCAG 2.2 SC 1.3.1 — a relationship conveyed by
          visual arrangement alone). `aria-describedby` states it, and the sentence itself now names
          the refusal rather than only inviting the action. */}
      {/* THE STATUS LINE STOPPED CARRYING THE EXAMPLES, BECAUSE THE EXAMPLES ARE NOW ON SCREEN.
          Two of its four sentences ended in `“raise this mountain”, “widen the river”` — the same two
          descriptions the pills directly below offer as one click each. At the 276px this column really
          is, that made a 79-character sentence wrap to three lines beside the button and shove the "Try
          one" heading into it. The refusal itself is unchanged and still reaches a screen reader through
          `aria-describedby`; the button's `disabledReason` still carries the longer form on hover. */}
      <div style={{ display: "flex", gap: space.sm, alignItems: "center" }}>
        <span id={DESCRIBE_STATUS_ID} data-testid="terrain-describe-status" style={{ flex: 1, minWidth: 0, fontSize: fontSize.meta, color: color.text.secondary }}>
          {plan
            ? plan.kind === "create"
              ? "builds a new world"
              : `${plan.steps.length} change${plan.steps.length === 1 ? "" : "s"} to this world`
            : busy
              ? "Building…"
              : text.trim()
                ? "Builds a new world, or changes this one."
                : "Type a description first."}
        </span>
        <Button
          variant="primary"
          disabled={busy || !text.trim()}
          disabledReason={
            busy
              ? "The world is still being rebuilt from the last description"
              : "Type a description first — this button builds what the box above says"
          }
          aria-describedby={DESCRIBE_STATUS_ID}
          data-testid="terrain-describe-build"
          onClick={submit}
        >
          {compact ? "Rebuild" : "Build it"}
        </Button>
      </div>

      {plan ? (
        <div
          data-testid="terrain-reading"
          style={{
            display: "grid",
            gap: space.xxs,
            padding: space.sm,
            borderRadius: radius.sm,
            background: color.bg.raised,
          }}
        >
          <span style={{ fontSize: fontSize.micro, color: color.text.muted, textTransform: "uppercase", letterSpacing: "0.04em" }}>
            {plan.kind === "create" ? "What this will build" : "What this will change"}
          </span>

          {/* A modification: the concrete steps, each naming the landform it resolved to, what it will do,
              and which derived data it will rebuild. This is the plan, before it runs. */}
          {plan.steps.map((st, i) => (
            <span
              key={`${st.verb}-${i}`}
              data-testid="terrain-plan-step"
              style={{ display: "grid", gap: space.xxs, fontSize: fontSize.meta }}
            >
              {st.refusal ? (
                <>
                  <span style={{ color: color.warn.text }}>{st.refusal}</span>
                  {st.suggestion ? (
                    <span style={{ color: color.text.secondary }}>Try: {st.suggestion}</span>
                  ) : null}
                </>
              ) : (
                <span style={{ color: color.text.primary }}>{st.effect}</span>
              )}
            </span>
          ))}

          {plan.understood.length === 0 && plan.steps.length === 0 ? (
            <span style={{ fontSize: fontSize.meta, color: color.warn.text }}>
              Nothing recognised yet — you’ll get a default temperate hill country.
            </span>
          ) : null}

          {plan.kind === "create"
            ? plan.understood.map((u, i) => (
                <span key={`${u.phrase}-${i}`} style={{ display: "flex", gap: space.xs, fontSize: fontSize.meta }}>
                  <code style={{ fontFamily: font.mono, fontSize: fontSize.micro, color: color.text.muted, minWidth: 96 }}>
                    {u.phrase}
                  </code>
                  <span style={{ color: color.text.primary }}>{u.meaning}</span>
                </span>
              ))
            : null}

          {plan.notes.map((n) => (
            <span key={n} style={{ fontSize: fontSize.meta, color: color.text.secondary }}>
              {n}
            </span>
          ))}

          {plan.unused.length > 0 ? (
            <span data-testid="terrain-reading-unused" style={{ fontSize: fontSize.meta, color: color.warn.text }}>
              Not used: {plan.unused.join(", ")} — these words changed nothing.
            </span>
          ) : null}
        </div>
      ) : null}

      {/* THE EXAMPLES USED TO BE HIDDEN IN THE ONE STATE THEY APPLIED TO.
          All five showed on the empty surface — including "raise this mountain", which needs a mountain
          — and `compact`, the form used once a world exists, showed none at all. So the three examples
          about CHANGING a world were offered only where there was nothing to change, and withheld
          everywhere else. Same list, filtered by which half of the feature this instance is for. */}
      <div style={{ display: "grid", gap: space.xs, minWidth: 0 }} data-testid="terrain-examples">
        <span style={textRole.eyebrow}>Try one</span>
        {/* A WRAPPED ROW OF PILLS, NOT A COLUMN OF SENTENCES. The shared action chip is the editor's
            one spelling of "a small thing you press once"; before it existed this was five ghost
            buttons re-dressed inline with a background, a radius, a leading chevron and a wrap
            override — a pill design invented in this file, which is exactly what the constitution's
            root-cause rule forbids. */}
        <div style={{ display: "flex", flexWrap: "wrap", gap: space.xs, minWidth: 0 }}>
          {EXAMPLES.filter((ex) => ex.mode === (compact ? "change" : "create")).map((ex) => (
            <AssetChip
              key={ex.label}
              tone="neutral"
              title={ex.text}
              data-testid="terrain-example"
              onSelect={() => setText(ex.text)}
            >
              {ex.label}
            </AssetChip>
          ))}
        </div>
      </div>
    </div>
  );
}

function ShapeSection({
  recipe,
  busy,
  brush,
  setBrush,
  edit,
  tool,
  setTool,
}: {
  recipe: TerrainRecipe;
  busy: boolean;
  brush: { kind: BrushKind; radiusM: number; strength: number; hardness: number };
  setBrush: (b: { kind: BrushKind; radiusM: number; strength: number; hardness: number }) => void;
  edit: EditFn;
  tool: string;
  setTool: (t: "none" | "sculpt" | "route") => void;
}) {
  const strokeCount = recipe.strokes.length;
  return (
    <>
      <PropertyRow label="World size (m)" help="The square the terrain covers." htmlFor="terrain-world-size">
        <NumericField
          value={recipe.world_size_m}
          step={64}
          min={64}
          data-testid="terrain-world-size"
          id="terrain-world-size"
          ariaLabel="World size in metres"
          onCommit={(v) =>
            void edit(
              {
                op: "setExtent",
                world_size_m: v,
                chunk_size_m: recipe.chunk_size_m,
                chunk_verts: recipe.chunk_verts,
              },
              "Couldn’t resize the world",
            )
          }
        />
      </PropertyRow>
      <PropertyRow label="Chunk size (m)" htmlFor="terrain-chunk-size">
        <NumericField
          value={recipe.chunk_size_m}
          step={16}
          min={16}
          data-testid="terrain-chunk-size"
          id="terrain-chunk-size"
          ariaLabel="Chunk size in metres"
          onCommit={(v) =>
            void edit(
              {
                op: "setExtent",
                world_size_m: recipe.world_size_m,
                chunk_size_m: v,
                chunk_verts: recipe.chunk_verts,
              },
              "Couldn’t change the chunk size",
            )
          }
        />
      </PropertyRow>
      <PropertyRow label="Detail (verts)" help="Vertices per chunk edge; must be a power of two plus one." htmlFor="terrain-chunk-verts">
        <NumericField
          value={recipe.chunk_verts}
          step={1}
          min={3}
          data-testid="terrain-chunk-verts"
          id="terrain-chunk-verts"
          ariaLabel="Vertices per chunk edge"
          onCommit={(v) =>
            void edit(
              {
                op: "setExtent",
                world_size_m: recipe.world_size_m,
                chunk_size_m: recipe.chunk_size_m,
                chunk_verts: Math.round(v),
              },
              "Couldn’t change the detail",
            )
          }
        />
      </PropertyRow>

      <SectionHeader>Layers</SectionHeader>
      <div data-testid="terrain-layers" style={{ display: "grid", gap: space.xxs }}>
        {recipe.layers.map((layer, i) => (
          <div
            key={`${layer.name}-${i}`}
            style={{
              display: "flex",
              alignItems: "center",
              gap: space.xs,
              padding: space.xs,
              borderRadius: radius.sm,
              background: color.bg.raised,
              opacity: layer.enabled ? 1 : 0.55,
            }}
          >
            <Button
              variant="toggle"
              compact
              aria-pressed={layer.enabled}
              aria-label={`${layer.enabled ? "Disable" : "Enable"} ${layer.name}`}
              data-testid={`terrain-layer-toggle-${i}`}
              disabled={busy}
              disabledReason={BUSY_REASON}
              onClick={() => void edit({ op: "toggleLayer", index: i, enabled: !layer.enabled }, "Couldn’t toggle that layer")}
            >
              {layer.enabled ? "On" : "Off"}
            </Button>
            <span style={{ flex: 1, fontSize: fontSize.body }}>{layer.name}</span>
            <Badge tone="neutral">{layerKind(layer.kind)}</Badge>
            <Button
              variant="ghost"
              compact
              aria-label={`Move ${layer.name} down the stack`}
              disabled={busy || i === 0}
              disabledReason={busy ? BUSY_REASON : `${layer.name} is already at the bottom of the stack`}
              onClick={() => void edit({ op: "moveLayer", index: i, delta: -1 }, "Couldn’t reorder that layer")}
            >
              <Icon name="arrow-up" size="sm" />
            </Button>
            <Button
              variant="ghost"
              compact
              aria-label={`Move ${layer.name} up the stack`}
              disabled={busy || i === recipe.layers.length - 1}
              disabledReason={busy ? BUSY_REASON : `${layer.name} is already at the top of the stack`}
              onClick={() => void edit({ op: "moveLayer", index: i, delta: 1 }, "Couldn’t reorder that layer")}
            >
              <Icon name="arrow-down" size="sm" />
            </Button>
            <Button
              variant="ghost"
              compact
              aria-label={`Remove ${layer.name}`}
              data-testid={`terrain-layer-remove-${i}`}
              disabled={busy}
              disabledReason={BUSY_REASON}
              onClick={() => void edit({ op: "removeLayer", index: i }, "Couldn’t remove that layer")}
            >
              <Icon name="close" size="sm" />
            </Button>
          </div>
        ))}
      </div>

      <SectionHeader>Sculpt</SectionHeader>
      <Button
        variant={tool === "sculpt" ? "primary" : "secondary"}
        aria-pressed={tool === "sculpt"}
        data-testid="terrain-sculpt-toggle"
        onClick={() => setTool(tool === "sculpt" ? "none" : "sculpt")}
      >
        {tool === "sculpt" ? "Sculpting — click the viewport to paint" : "Sculpt in the viewport"}
      </Button>
      {/* Same cut as the describe box's note. The ring under the cursor explains itself the instant the
          tool is armed; what a reader cannot discover by looking is the undo granularity. */}
      <span style={{ fontSize: fontSize.meta, color: color.text.secondary, lineHeight: 1.5 }}>
        A whole drag lands as one undo step.
      </span>
      <PropertyRow label="Brush" htmlFor="terrain-brush-kind">
        <SelectField
          value={brush.kind}
          data-testid="terrain-brush-kind"
          id="terrain-brush-kind"
          aria-label="Brush kind"
          onChange={(e) => setBrush({ ...brush, kind: e.target.value as BrushKind })}
        >
          {BRUSH_KINDS.map((b) => (
            <option key={b.id} value={b.id}>
              {b.label}
            </option>
          ))}
        </SelectField>
      </PropertyRow>
      <PropertyRow label="Radius (m)" htmlFor="terrain-brush-radius">
        <NumericField
          value={brush.radiusM}
          step={1}
          min={0.5}
          data-testid="terrain-brush-radius"
          id="terrain-brush-radius"
          ariaLabel="Brush radius in metres"
          onCommit={(v) => setBrush({ ...brush, radiusM: v })}
        />
      </PropertyRow>
      <PropertyRow label="Strength" help="Metres per dab for raise, blend amount for smooth and flatten." htmlFor="terrain-brush-strength">
        <NumericField
          value={brush.strength}
          step={0.25}
          data-testid="terrain-brush-strength"
          id="terrain-brush-strength"
          ariaLabel="Brush strength"
          onCommit={(v) => setBrush({ ...brush, strength: v })}
        />
      </PropertyRow>
      <div style={{ display: "flex", gap: space.xs, alignItems: "center" }}>
        <span style={{ flex: 1, fontSize: fontSize.meta, color: color.text.secondary }}>
          {strokeCount === 0 ? "No strokes yet" : `${strokeCount} stroke${strokeCount === 1 ? "" : "s"} recorded`}
        </span>
        <Button
          variant="secondary"
          compact
          data-testid="terrain-sculpt-test"
          disabled={busy}
          // CONDITIONAL, on the same predicate as `disabled`. `Button` resolves `title ?? reason`, so an
          // unconditional title is a permanent block on the refusal ever speaking — the control would
          // keep explaining what it does while going dark for a reason it could not state.
          title={busy ? undefined : "Paint one dab at the world centre — the same operation a viewport drag records."}
          disabledReason={BUSY_REASON}
          onClick={() =>
            void edit(
              {
                op: "sculpt",
                brush: {
                  kind: brush.kind,
                  radiusM: brush.radiusM,
                  strength: brush.strength,
                  hardness: brush.hardness,
                  targetM: 0,
                  spacing: 0.25,
                },
                from: [recipe.world_size_m / 2, recipe.world_size_m / 2],
                to: [recipe.world_size_m / 2, recipe.world_size_m / 2],
              },
              "Couldn’t apply that stroke",
            )
          }
        >
          Apply dab
        </Button>
        <Button
          variant="ghost"
          compact
          data-testid="terrain-clear-strokes"
          disabled={busy || strokeCount === 0}
          disabledReason={busy ? BUSY_REASON : "Nothing has been sculpted yet, so there are no strokes to clear"}
          onClick={() => void edit({ op: "clearStrokes" }, "Couldn’t clear the strokes")}
        >
          Clear
        </Button>
      </div>
    </>
  );
}

function LookSection({ recipe }: { recipe: TerrainRecipe }) {
  return (
    <>
      <SectionHeader>Materials</SectionHeader>
      <div data-testid="terrain-materials" style={{ display: "grid", gap: space.xxs }}>
        {recipe.materials.map((m, i) => (
          <div key={`${m.name}-${i}`} style={{ display: "flex", alignItems: "center", gap: space.xs }}>
            <span
              aria-hidden
              style={{
                width: 16,
                height: 16,
                borderRadius: radius.sm,
                border: `1px solid ${color.border.subtle}`,
                // ui-constitution-allow literal-ui-color: a data-derived material swatch — the colour IS the terrain material's albedo, not a UI colour choice
                background: `rgb(${m.albedo.map((c) => Math.round(Math.min(1, Math.max(0, c)) * 255)).join(",")})`,
              }}
            />
            <span style={{ flex: 1, fontSize: fontSize.body }}>{m.name}</span>
            <span style={{ fontSize: fontSize.micro, color: color.text.muted, fontFamily: font.mono }}>
              rough {m.roughness.toFixed(2)}
            </span>
          </div>
        ))}
      </div>
      <SectionHeader>Biomes</SectionHeader>
      <div data-testid="terrain-biomes" style={{ display: "grid", gap: space.xxs }}>
        {recipe.biomes.map((b, i) => (
          <div key={`${b.name}-${i}`} style={{ display: "flex", alignItems: "center", gap: space.xs }}>
            <span style={{ flex: 1, fontSize: fontSize.body }}>{b.name}</span>
            <Badge tone={b.enabled ? "neutral" : "warn"}>
              {recipe.materials[b.material_layer]?.name ?? `material ${b.material_layer}`}
            </Badge>
          </div>
        ))}
      </div>
      <p style={{ margin: 0, fontSize: fontSize.meta, color: color.text.secondary, lineHeight: 1.5 }}>
        Biomes are weights, not labels: where two overlap, their materials blend. That is why the ground has
        no visible contour lines between them.
      </p>
    </>
  );
}


/** A prototype's author-facing label, numbered when a preset reuses the same name for several variants. */
function protoLabel(recipe: TerrainRecipe, index: number): string {
  const name = recipe.protos[index]?.name ?? "Prop";
  const sameName = recipe.protos.filter((q) => q.name === name);
  if (sameName.length < 2) return name;
  const nth = recipe.protos.slice(0, index + 1).filter((q) => q.name === name).length;
  return `${name} ${nth}`;
}

function LifeSection({ recipe, busy, edit }: { recipe: TerrainRecipe; busy: boolean; edit: EditFn }) {
  const [keys, setKeys] = useState<Record<number, string>>({});
  return (
    <>
      <SectionHeader>Scattered props</SectionHeader>
      <div data-testid="terrain-protos" style={{ display: "grid", gap: space.md }}>
        {recipe.protos.map((p, i) => (
          <div key={`${p.name}-${i}`} style={{ display: "grid", gap: space.xxs, minWidth: 0 }}>
            <span style={{ display: "flex", alignItems: "center", gap: space.xs }}>
              {/* Presets legitimately declare several variants under one name ("Conifers" twice), and two
                  identical rows each fronted by a different hash is unreadable. Number the repeats. */}
              <span style={{ flex: 1, fontSize: fontSize.body, minWidth: 0 }}>{protoLabel(recipe, i)}</span>
              <Badge tone={p.mesh_key ? "success" : "warn"}>{p.mesh_key ? "ready" : "needs a mesh"}</Badge>
            </span>
            {/* The content address is a POWER-USER field, not the headline. It used to be the first and
                largest thing in the row, so the Life section read as a column of hex — the author's actual
                question ("is this prop ready, and what is it?") was answered by a badge they saw second. */}
            <span style={{ display: "flex", gap: space.xs, alignItems: "center", minWidth: 0 }}>
              <span
                style={{
                  ...textRole.eyebrow,
                  padding: 0,
                  flex: "0 0 auto",
                  color: color.text.muted,
                }}
              >
                Asset
              </span>
              <TextField
                mono
                value={keys[i] ?? p.mesh_key}
                placeholder="paste an asset handle to replace it"
                aria-label={`Mesh asset for ${p.name}`}
                data-testid={`terrain-proto-key-${i}`}
                onChange={(e) => setKeys({ ...keys, [i]: e.target.value })}
              />
              <Button
                variant="secondary"
                compact
                disabled={busy}
                disabledReason={BUSY_REASON}
                data-testid={`terrain-proto-bind-${i}`}
                onClick={() =>
                  void edit(
                    {
                      op: "bindProto",
                      index: i,
                      mesh_key: keys[i] ?? p.mesh_key,
                      lod_keys: p.lod_keys,
                      impostor_key: p.impostor_key,
                    },
                    "Couldn’t bind that mesh",
                  )
                }
              >
                Bind
              </Button>
            </span>
          </div>
        ))}
      </div>
      <SectionHeader>Rules</SectionHeader>
      <div data-testid="terrain-scatter" style={{ display: "grid", gap: space.xxs }}>
        {recipe.scatter.map((s, i) => (
          <div key={`${s.name}-${i}`} style={{ display: "flex", alignItems: "center", gap: space.xs }}>
            <span style={{ flex: 1, fontSize: fontSize.body }}>{s.name}</span>
            <span style={{ fontSize: fontSize.micro, color: color.text.muted, fontFamily: font.mono }}>
              {s.density_per_hectare.toFixed(0)}/ha
            </span>
            <Badge tone={recipe.protos[s.proto]?.mesh_key ? "neutral" : "warn"}>
              {recipe.protos[s.proto]?.name ?? "?"}
            </Badge>
          </div>
        ))}
        {recipe.scatter.length === 0 ? (
          <span style={{ fontSize: fontSize.meta, color: color.text.secondary }}>No scatter rules yet.</span>
        ) : null}
      </div>
    </>
  );
}

function RoutesSection({
  recipe,
  busy,
  client,
  tool,
  setTool,
  route,
  setRoute,
  absorb,
  edit,
}: {
  recipe: TerrainRecipe;
  busy: boolean;
  client: EditorClient;
  tool: string;
  setTool: (t: "none" | "sculpt" | "route") => void;
  route: { kind: string; widthM: number; depthM: number; points: number };
  setRoute: (r: { kind: string; widthM: number; depthM: number; points: number }) => void;
  absorb: (r: TerrainReply) => void;
  edit: EditFn;
}) {
  const drawing = tool === "route";
  const water = recipe.water;
  return (
    <>
      <Button
        variant={drawing ? "primary" : "secondary"}
        aria-pressed={drawing}
        data-testid="terrain-route-toggle"
        onClick={() => {
          setTool(drawing ? "none" : "route");
          if (drawing) void client.terrainRouteClear(false).then((n) => setRoute({ ...route, points: n }));
        }}
      >
        {drawing ? "Drawing — click the terrain to add points" : "Draw a route"}
      </Button>
      <span style={{ fontSize: fontSize.meta, color: color.text.secondary, lineHeight: 1.5 }}>
        A road grades across dips instead of draping over them; a river is forced downhill, so it cannot flow
        uphill however you draw it.
      </span>
      <PropertyRow label="Kind" htmlFor="terrain-route-kind">
        <SelectField
          value={route.kind}
          id="terrain-route-kind"
          data-testid="terrain-route-kind"
          aria-label="Route kind"
          onChange={(e) => setRoute({ ...route, kind: e.target.value })}
        >
          <option value="road">Road</option>
          <option value="river">River</option>
          <option value="pad">Pad</option>
        </SelectField>
      </PropertyRow>
      <PropertyRow label="Width (m)" htmlFor="terrain-route-width">
        <NumericField
          value={route.widthM}
          step={1}
          min={1}
          id="terrain-route-width"
          data-testid="terrain-route-width"
          ariaLabel="Route width in metres"
          onCommit={(v) => setRoute({ ...route, widthM: v })}
        />
      </PropertyRow>
      {route.kind === "river" ? (
        <PropertyRow label="Depth (m)" htmlFor="terrain-route-depth">
          <NumericField
            value={route.depthM}
            step={0.5}
            min={0}
            id="terrain-route-depth"
            data-testid="terrain-route-depth"
            ariaLabel="Channel depth in metres"
            onCommit={(v) => setRoute({ ...route, depthM: v })}
          />
        </PropertyRow>
      ) : null}
      <div style={{ display: "flex", gap: space.xs, alignItems: "center" }}>
        <span
          data-testid="terrain-route-points"
          style={{ flex: 1, fontSize: fontSize.meta, color: color.text.secondary }}
        >
          {route.points === 0
            ? "No points yet"
            : `${route.points} point${route.points === 1 ? "" : "s"} placed`}
        </span>
        <Button
          variant="ghost"
          compact
          data-testid="terrain-route-undo"
          disabled={busy || route.points === 0}
          disabledReason={busy ? BUSY_REASON : "No route points placed yet, so there is nothing to undo"}
          onClick={() => void client.terrainRouteClear(true).then((n) => setRoute({ ...route, points: n }))}
        >
          Undo point
        </Button>
        <Button
          variant="primary"
          compact
          data-testid="terrain-route-commit"
          disabled={busy || route.points < 2}
          disabledReason={busy ? BUSY_REASON : "A route needs at least two points — click the ground to place them"}
          onClick={() => {
            const material = recipe.materials.findIndex((m) => /road|gravel|path/i.test(m.name));
            void client
              .terrainRouteCommit(route.kind, route.widthM, route.depthM, material >= 0 ? material : null)
              .then((r) => {
                absorb(r);
                setRoute({ ...route, points: 0 });
              })
              .catch(() => undefined);
          }}
        >
          Commit route
        </Button>
      </div>
      <span style={{ fontSize: fontSize.meta, color: color.text.secondary, lineHeight: 1.5 }}>
        {recipe.splines.length === 0
          ? "No roads or rivers yet."
          : `${recipe.splines.length} route${recipe.splines.length === 1 ? "" : "s"} shaping the terrain.`}
      </span>
      {recipe.splines.length > 0 ? <RouteCrossingCheck client={client} recipe={recipe} busy={busy} /> : null}

      <SectionHeader>Water</SectionHeader>
      <Button
        variant="toggle"
        aria-pressed={water.enabled}
        data-testid="terrain-water-enabled"
        disabled={busy}
        disabledReason={BUSY_REASON}
        onClick={() =>
          void edit(
            { op: "setWater", water: { ...water, enabled: !water.enabled } },
            "Couldn’t change the water",
          )
        }
      >
        {water.enabled ? "Water on" : "Water off"}
      </Button>
      {water.enabled ? (
        <>
          <PropertyRow
            label="Sea level (m)"
            help="Everything below this fills with water, and the shore materials follow it."
            htmlFor="terrain-sea-level"
          >
            <NumericField
              value={water.sea_level_m}
              step={0.5}
              data-testid="terrain-sea-level"
              id="terrain-sea-level"
              ariaLabel="Sea level in metres"
              onCommit={(v) =>
                void edit({ op: "setWater", water: { ...water, sea_level_m: v } }, "Couldn’t move the sea level")
              }
            />
          </PropertyRow>
          <PropertyRow label="Shore blend (m)" htmlFor="terrain-shore-blend">
            <NumericField
              value={water.shore_blend_m}
              step={0.5}
              min={0}
              data-testid="terrain-shore-blend"
              id="terrain-shore-blend"
              ariaLabel="Shore blend in metres"
              onCommit={(v) =>
                void edit({ op: "setWater", water: { ...water, shore_blend_m: v } }, "Couldn’t change the shore")
              }
            />
          </PropertyRow>
        </>
      ) : null}
      <span style={{ fontSize: fontSize.meta, color: color.text.secondary, lineHeight: 1.5 }}>
        Raising the sea level floods the low ground: the surface appears, the beaches move up the slope and
        the flooded ground stops being walkable. Nothing is re-authored — it is the same recipe read against a
        different water line.
      </span>
    </>
  );
}

function PerfSection({
  recipe,
  stats,
  busy,
  edit,
}: {
  recipe: TerrainRecipe;
  stats: TerrainStats | null;
  busy: boolean;
  edit: EditFn;
}) {
  const rows: [string, string][] = stats
    ? [
        ["Chunks resident", `${stats.residentChunks}`],
        ["Chunks drawn", `${stats.visibleChunks}`],
        ["Culled (frustum / horizon)", `${stats.culledFrustum} / ${stats.culledHorizon}`],
        ["Triangles drawn", stats.drawnTriangles.toLocaleString()],
        ["Scattered instances", `${stats.drawnInstances} (${stats.impostorInstances} impostors)`],
        ["Builds in flight", `${stats.pendingBuilds}`],
        ["Slowest build stage", stats.dominantStage],
        [
          "Memory",
          `${stats.totalMb.toFixed(1)} MB — mesh ${stats.meshMb.toFixed(1)}, texture ${stats.textureMb.toFixed(1)}`,
        ],
      ]
    : [];
  return (
    <>
      <PropertyRow label="View distance (m)" htmlFor="terrain-view-distance">
        <NumericField
          value={recipe.lod.max_view_distance_m}
          step={128}
          min={64}
          data-testid="terrain-view-distance"
          id="terrain-view-distance"
          ariaLabel="Terrain view distance in metres"
          onCommit={(v) =>
            void edit({ op: "setLod", lod: { ...recipe.lod, max_view_distance_m: v } }, "Couldn’t change the view distance")
          }
        />
      </PropertyRow>
      <PropertyRow
        label="Detail budget (px)"
        help="A chunk drops to a coarser mesh only once the change would move geometry by less than this many pixels."
        htmlFor="terrain-screen-error"
      >
        <NumericField
          value={recipe.lod.screen_error_px}
          step={0.25}
          min={0.25}
          data-testid="terrain-screen-error"
          id="terrain-screen-error"
          ariaLabel="Screen-space error budget in pixels"
          onCommit={(v) =>
            void edit({ op: "setLod", lod: { ...recipe.lod, screen_error_px: v } }, "Couldn’t change the detail budget")
          }
        />
      </PropertyRow>
      <PropertyRow label="Texture memory (MB)" htmlFor="terrain-screen-error">
        <NumericField
          value={recipe.budget.texture_mb}
          step={32}
          min={16}
          data-testid="terrain-texture-budget"
          id="terrain-texture-budget"
          ariaLabel="Texture memory budget in megabytes"
          onCommit={(v) =>
            void edit(
              { op: "setBudget", budget: { ...recipe.budget, texture_mb: Math.round(v) } },
              "Couldn’t change the budget",
            )
          }
        />
      </PropertyRow>
      <div data-testid="terrain-stats" style={{ display: "grid", gap: space.xxs, paddingTop: space.xs }}>
        {stats ? (
          rows.map(([k, v]) => (
            <div key={k} style={{ display: "flex", gap: space.xs, fontSize: fontSize.meta }}>
              <span style={{ flex: 1, color: color.text.secondary }}>{k}</span>
              <span style={{ fontFamily: font.mono, color: color.text.primary }}>{v}</span>
            </div>
          ))
        ) : (
          <span style={{ fontSize: fontSize.meta, color: color.text.secondary }}>No measurements yet.</span>
        )}
        {stats?.overBudget ? (
          <span data-testid="terrain-over-budget" style={{ fontSize: fontSize.meta, color: color.warn.text }}>
            Over budget: distant chunks are being evicted to stay inside the ceiling. Lower the view distance
            or raise the memory budget.
          </span>
        ) : null}
      </div>
      <span style={{ fontSize: fontSize.micro, color: color.text.muted }}>
        {busy ? "Applying…" : "Measured from the live runtime, not estimated."}
      </span>
    </>
  );
}

export default TerrainPanel;
