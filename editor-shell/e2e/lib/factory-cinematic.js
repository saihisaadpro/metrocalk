export const FACTORY_ACCEPTANCE = Object.freeze({
  fixtureBasename: "Skid Weld Line A.1_(1).stp",
  minimumFixtureBytes: 200_000_000,
  report: Object.freeze({
    total: 15_711,
    exactBrep: 15_476,
    tessellationOnly: 235,
    aiReconstructed: 0,
    proxy: 0,
    accessDenied: 0,
    failed: 0,
  }),
  animationTracks: 24,
  filmedSubjects: 15,
  cinematicShots: 30,
  minimumCinematicSeconds: 180,
});

export const SEMANTIC_SEARCHES = Object.freeze([
  "toggle",
  "turnwheel",
  "boom",
  "welding gun",
  "effector",
  "irbt",
  "carrier",
  "cart",
  "skid",
  "crane",
  "trolley",
  "hook",
  "wheel",
  "fan",
  "robot",
  "motor",
  "clamp",
  "weld",
]);

const mechanismFamilies = Object.freeze([
  { key: "toggle-clamp", pattern: /toggle.*clamp|clamp.*toggle/i, maximum: 3 },
  { key: "turnwheel", pattern: /turnwheel/i, maximum: 2 },
  { key: "weld-boom", pattern: /weld.*boom|boom.*weld/i, maximum: 3 },
  { key: "welding-gun", pattern: /weld(?:ing)?.*gun|gun.*weld/i, maximum: 2 },
  { key: "end-effector", pattern: /end[_\s-]*effector/i, maximum: 2 },
  { key: "irbt", pattern: /irbt/i, maximum: 2 },
  { key: "carrier", pattern: /carrier/i, maximum: 2 },
  { key: "skid-cart", pattern: /(?:power[_\s-]*)?skid.*cart|cart.*skid/i, maximum: 3 },
  { key: "overhead-crane", pattern: /overhead.*crane|crane.*overhead/i, maximum: 2 },
  { key: "trolley", pattern: /trolley/i, maximum: 2 },
  { key: "hook", pattern: /hook/i, maximum: 2 },
  { key: "wheel", pattern: /wheel/i, maximum: 3 },
  { key: "fan", pattern: /fan/i, maximum: 2 },
  { key: "robot", pattern: /robot/i, maximum: 2 },
  { key: "motor", pattern: /motor/i, maximum: 3 },
  { key: "clamp", pattern: /clamp/i, maximum: 3 },
]);

const byNameThenId = (left, right) => left.name.localeCompare(right.name, undefined, {
  sensitivity: "base",
  numeric: true,
}) || left.id.localeCompare(right.id);

export const normalizedPartName = (part) => String(part?.name ?? "")
  .trim()
  .toLocaleLowerCase()
  .replace(/[_\s-]+/g, " ");

/**
 * Pick a reproducible, mechanism-diverse set rather than taking the first 24 rows from a 15k-part report.
 * One round-robin pass through every semantic family happens before a family receives its second part.
 */
export function selectMechanismParts(candidates, count = FACTORY_ACCEPTANCE.animationTracks) {
  const unique = new Map();
  for (const candidate of candidates ?? []) {
    if (!candidate || typeof candidate.id !== "string" || !candidate.id || !normalizedPartName(candidate)) continue;
    if (candidate.fidelity !== "exact-brep" && candidate.fidelity !== "tessellation-only") continue;
    unique.set(candidate.id, candidate);
  }
  const ordered = [...unique.values()].sort(byNameThenId);
  const pools = mechanismFamilies.map((family) => ({
    ...family,
    parts: ordered.filter((part) => family.pattern.test(part.name)),
    cursor: 0,
    used: 0,
  }));
  const picked = [];
  const pickedIds = new Set();

  let progressed = true;
  while (picked.length < count && progressed) {
    progressed = false;
    for (const pool of pools) {
      if (picked.length >= count || pool.used >= pool.maximum) continue;
      while (pool.cursor < pool.parts.length && pickedIds.has(pool.parts[pool.cursor].id)) pool.cursor += 1;
      const part = pool.parts[pool.cursor];
      if (!part) continue;
      pool.cursor += 1;
      pool.used += 1;
      pickedIds.add(part.id);
      picked.push({ ...part, mechanismFamily: pool.key });
      progressed = true;
    }
  }

  for (const part of ordered) {
    if (picked.length >= count) break;
    if (pickedIds.has(part.id)) continue;
    const family = mechanismFamilies.find((candidate) => candidate.pattern.test(part.name));
    if (!family) continue;
    pickedIds.add(part.id);
    picked.push({ ...part, mechanismFamily: family.key });
  }
  return picked;
}

/** Prefer 15 different readable labels, then fall back to distinct entity ids only if the source repeats labels. */
export function chooseFilmedSubjects(parts, count = FACTORY_ACCEPTANCE.filmedSubjects) {
  const selected = [];
  const ids = new Set();
  const names = new Set();
  for (const part of parts ?? []) {
    const name = normalizedPartName(part);
    if (!name || names.has(name) || ids.has(part.id)) continue;
    selected.push(part);
    ids.add(part.id);
    names.add(name);
    if (selected.length === count) return selected;
  }
  for (const part of parts ?? []) {
    if (ids.has(part.id)) continue;
    selected.push(part);
    ids.add(part.id);
    if (selected.length === count) break;
  }
  return selected;
}

/** A deliberately small, plausible displacement for each industrial mechanism class. */
export function motionProfileFor(part) {
  const name = normalizedPartName(part);
  if (/turnwheel|\bwheel\b|\bfan\b/.test(name)) {
    return {
      motion: "revolute",
      revolute: true,
      axis: /\bwheel\b/.test(name) ? [1, 0, 0] : [0, 0, 1],
      amplitude: /\bfan\b/.test(name) ? 0.45 : 0.32,
      rationale: "a restrained service rotation around the named rotary component",
    };
  }
  if (/hook/.test(name)) {
    return { motion: "prismatic", revolute: false, axis: [0, 1, 0], amplitude: 0.18, rationale: "a short vertical hoist travel" };
  }
  if (/carrier|\bcart\b|trolley|irbt/.test(name)) {
    return { motion: "prismatic", revolute: false, axis: [1, 0, 0], amplitude: 0.28, rationale: "a conservative travel along the factory rail" };
  }
  if (/toggle.*clamp|clamp.*toggle|\bclamp\b/.test(name)) {
    return { motion: "prismatic", revolute: false, axis: [0, 1, 0], amplitude: 0.04, rationale: "a short clamp engage/release stroke" };
  }
  if (/boom|moving arm|\barm\b/.test(name)) {
    return { motion: "prismatic", revolute: false, axis: [0, 0, 1], amplitude: 0.12, rationale: "a measured boom/arm positioning stroke" };
  }
  if (/gun|effector/.test(name)) {
    return { motion: "prismatic", revolute: false, axis: [0, 0, 1], amplitude: 0.06, rationale: "a precise tool approach and retract" };
  }
  if (/robot/.test(name)) {
    return { motion: "revolute", revolute: true, axis: [0, 1, 0], amplitude: 0.14, rationale: "a modest robot-axis settle" };
  }
  if (/crane/.test(name)) {
    return { motion: "prismatic", revolute: false, axis: [1, 0, 0], amplitude: 0.20, rationale: "a short bridge positioning move" };
  }
  if (/motor/.test(name)) {
    return { motion: "revolute", revolute: true, axis: [0, 0, 1], amplitude: 0.10, rationale: "a subtle motor torque reaction, not a body spin" };
  }
  return { motion: "prismatic", revolute: false, axis: [1, 0, 0], amplitude: 0.035, rationale: "a small fallback service motion" };
}

/** Seven closed-loop keys; every track returns to its authored zero pose at exactly 12 seconds. */
export function buildMechanismKeys(amplitude, index) {
  const phase = (index % 4) * 0.15;
  return [
    { t: 0, value: 0 },
    { t: 1.45 + phase, value: amplitude },
    { t: 3.35 + phase, value: amplitude * 0.12 },
    { t: 5.55 + phase, value: -amplitude },
    { t: 7.65 + phase, value: 0 },
    { t: 9.55 + phase, value: amplitude * 0.62 },
    { t: 12, value: 0 },
  ];
}

export const CALM_SHOT_PAIRS = Object.freeze([
  ["hero", "orbit"],
  ["vista", "hero"],
  ["reveal", "detail"],
  ["birdseye", "dropin"],
  ["sweep", "closeup"],
  ["pullback", "hero"],
  ["establish", "orbit"],
  ["overshoulder", "reveal"],
  ["looming", "orbit"],
  ["vista", "sweep"],
  ["hero", "closeup"],
  ["birdseye", "hero"],
  ["reveal", "orbit"],
  ["pullback", "detail"],
  ["establish", "sweep"],
]);

const authoredShotSeconds = Object.freeze({
  establish: 2.5,
  hero: 2.5,
  closeup: 1.8,
  orbit: 3.0,
  reveal: 3.0,
  looming: 2.0,
  vista: 3.0,
  overshoulder: 2.2,
  birdseye: 3.0,
  dropin: 2.2,
  detail: 1.6,
  pullback: 3.2,
  sweep: 3.0,
});

export const calmSecondsForKinds = (kinds) => kinds.reduce((seconds, kind) => {
  const authored = authoredShotSeconds[kind];
  if (!Number.isFinite(authored)) throw new Error(`No duration is recorded for shot kind ${kind}`);
  return seconds + authored * 2.5;
}, 0);

export function buildCalmShotAssignments(subjects) {
  return (subjects ?? []).map((subject, index) => {
    const kinds = CALM_SHOT_PAIRS[index % CALM_SHOT_PAIRS.length];
    return {
      id: subject.id,
      name: subject.name,
      mood: "calm",
      kinds: [...kinds],
      plannedSeconds: calmSecondsForKinds(kinds),
    };
  });
}
