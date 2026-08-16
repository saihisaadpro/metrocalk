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
import { color, font, fontSize, motion, radius, space, text as textRole } from "../theme/tokens";
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

const SEVERITY_TONE: Record<TerrainIssue["severity"], "warn" | "accent" | "neutral"> = {
  blocking: "warn",
  warning: "warn",
  info: "neutral",
};

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
        <div style={{ padding: space.lg, display: "grid", gap: space.md, minWidth: 0 }}>
          <p style={{ margin: 0, color: color.text.secondary, fontSize: fontSize.body, lineHeight: 1.5 }}>
            Describe the landscape you want, or pick a starting point. Either way you get a{" "}
            <em>recipe</em> — a seed, a stack of layers and the strokes you paint on top — and every value
            stays editable afterwards. Nothing is baked in.
          </p>
          <DescribeBox client={client} busy={busy} run={run} />
          <div style={{ height: 1, background: color.border.subtle, margin: `${space.xs} 0` }} aria-hidden />
          <SectionHeader>Or start from a preset</SectionHeader>
          <div style={{ display: "grid", gap: space.sm }} data-testid="terrain-presets">
            {presets.map((p) => (
              <Button
                key={p.id}
                variant="ghost"
                disabled={busy}
                data-testid={`terrain-preset-${p.id}`}
                onClick={() => void run(() => client.terrainCreate(p.id), `Couldn’t create ${p.name}`)}
                // Ghost, not bordered: six outlined cards in a column is more chrome than a list of six
                // choices needs. The hover state carries the affordance.
                style={{ justifyContent: "flex-start", textAlign: "left", height: "auto", padding: `${space.sm} ${space.md}`, minWidth: 0, whiteSpace: "normal" }}
              >
                <span style={{ display: "grid", gap: space.xxs, minWidth: 0 }}>
                  <span style={{ fontSize: fontSize.label }}>{p.name}</span>
                  <span style={{ fontSize: fontSize.meta, color: color.text.secondary, fontWeight: 400 }}>
                    {p.description}
                  </span>
                </span>
              </Button>
            ))}
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
          padding: `${space.xs} ${space.md} 0`,
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

        {SECTIONS.map((section) => (
          <div key={section.id}>
            <SectionHeader>
              <Button
                variant="ghost"
                compact
                aria-expanded={open[section.id]}
                data-testid={`terrain-section-${section.id}`}
                onClick={() => toggle(section.id)}
                style={{ width: "100%", justifyContent: "space-between" }}
              >
                {/* The section's own title, at section weight. As bare ghost-button text it rendered
                    QUIETER than the plain sub-headings inside it ("Layers", "Sculpt") — so the thing that
                    contained the group looked less important than the group, and the strip read as a row of
                    disabled controls rather than the panel's structure. */}
                <span style={{ ...textRole.sectionTitle, padding: 0 }}>{section.title}</span>
                {/* A chevron that ROTATES, not a +/− glyph. "+" and "−" are the vocabulary of add and
                    remove, which this strip sits directly above rows that really do add and remove. */}
                <span
                  aria-hidden
                  style={{
                    color: color.text.muted,
                    fontSize: fontSize.micro,
                    lineHeight: 1,
                    transition: `transform ${motion.fast}`,
                    transform: open[section.id] ? "rotate(90deg)" : "rotate(0deg)",
                  }}
                >
                  ▶
                </span>
              </Button>
            </SectionHeader>
            {open[section.id] ? (
              <div style={{ display: "grid", gap: space.sm, paddingTop: space.xs, minWidth: 0 }}>
                {section.id === "describe" ? (
                  <>
                    <DescribeBox client={client} busy={busy} run={run} compact />
                    <span style={{ fontSize: fontSize.meta, color: color.text.secondary, lineHeight: 1.5 }}>
                      Rebuilding replaces this world in one undo step. Your layers, materials and rules below
                      are whatever the last description wrote — edit them freely; describing again starts over.
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
            ) : null}
          </div>
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


/** Descriptions offered as buttons, so the feature is discoverable without a blank-page problem. */
const EXAMPLES = [
  "a 4 km eroded alpine valley with a river and dense conifer forest",
  "a lush tropical archipelago with beaches and palms",
  // The second half of the feature: refining the world you already have.
  "raise this mountain by 150 m",
  "widen the river and make this valley traversable",
  "plant a dense forest here",
];

/**
 * Describe-to-build.
 *
 * Two things make this trustworthy rather than a slot machine. First, the reading is shown **before** you
 * commit: you can see that "wizards" was not understood while the text is still editable. Second, what comes
 * out is an ordinary recipe — the layer stack, materials and rules below are the ones it wrote, and they are
 * as editable as if you had typed them yourself.
 */
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
        placeholder="a 4 km eroded alpine valley with a river and dense pine forest"
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
      <div style={{ display: "flex", gap: space.xs, alignItems: "center" }}>
        <span style={{ flex: 1, fontSize: fontSize.meta, color: color.text.secondary }}>
          {plan
            ? plan.kind === "create"
              ? "builds a new world"
              : `${plan.steps.length} change${plan.steps.length === 1 ? "" : "s"} to this world`
            : "Describe a world, or change this one — “raise this mountain”, “widen the river”."}
        </span>
        <Button
          variant="primary"
          disabled={busy || !text.trim()}
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

      {compact ? null : (
        <div style={{ display: "grid", gap: space.xxs, minWidth: 0 }} data-testid="terrain-examples">
          <span style={{ ...textRole.eyebrow, paddingBottom: space.xxs }}>Try one</span>
          {EXAMPLES.map((ex) => (
            <Button
              key={ex}
              variant="ghost"
              compact
              disabled={busy}
              onClick={() => setText(ex)}
              // A resting surface and a leading mark. As bare ghost buttons these read as body copy, so the
              // quickest way into the whole feature looked like a paragraph nobody could click.
              style={{
                justifyContent: "flex-start",
                textAlign: "left",
                height: "auto",
                padding: `${space.sm} ${space.md}`,
                minWidth: 0,
                whiteSpace: "normal",
                background: color.bg.inset,
                borderRadius: radius.md,
                gap: space.sm,
                alignItems: "flex-start",
              }}
            >
              <span aria-hidden style={{ color: color.accent.base, fontSize: fontSize.meta, lineHeight: 1.5 }}>
                ›
              </span>
              <span style={{ fontSize: fontSize.meta, color: color.text.secondary, fontWeight: 400, lineHeight: 1.5 }}>
                {ex}
              </span>
            </Button>
          ))}
        </div>
      )}
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
              onClick={() => void edit({ op: "moveLayer", index: i, delta: -1 }, "Couldn’t reorder that layer")}
            >
              ↑
            </Button>
            <Button
              variant="ghost"
              compact
              aria-label={`Move ${layer.name} up the stack`}
              disabled={busy || i === recipe.layers.length - 1}
              onClick={() => void edit({ op: "moveLayer", index: i, delta: 1 }, "Couldn’t reorder that layer")}
            >
              ↓
            </Button>
            <Button
              variant="ghost"
              compact
              aria-label={`Remove ${layer.name}`}
              data-testid={`terrain-layer-remove-${i}`}
              disabled={busy}
              onClick={() => void edit({ op: "removeLayer", index: i }, "Couldn’t remove that layer")}
            >
              ✕
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
      <span style={{ fontSize: fontSize.meta, color: color.text.secondary, lineHeight: 1.5 }}>
        Drag on the terrain to paint. The ring under the cursor is the brush; the ground follows it live and
        the whole drag lands as one undo step.
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
          title="Paint one dab at the world centre — the same operation a viewport drag records."
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
                  color: color.text.faint,
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
          onClick={() => void client.terrainRouteClear(true).then((n) => setRoute({ ...route, points: n }))}
        >
          Undo point
        </Button>
        <Button
          variant="primary"
          compact
          data-testid="terrain-route-commit"
          disabled={busy || route.points < 2}
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
