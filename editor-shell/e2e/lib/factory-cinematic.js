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
  { key: "irbt", pattern: /irbt/i, maximum: 3 },
  { key: "carrier", pattern: /carrier/i, maximum: 2 },
  { key: "skid-cart", pattern: /(?:power[_\s-]*)?skid.*cart|cart.*skid/i, maximum: 3 },
  { key: "skid-fixture", pattern: /power.*skid|skid.*(?:strap|bar|fixture|table|post)/i, maximum: 2 },
  { key: "weld-equipment", pattern: /weld(?:ing)?.*(?:machine|shield|box|equipment)|(?:machine|shield|box|equipment).*weld/i, maximum: 3 },
  { key: "overhead-crane", pattern: /overhead.*crane|crane.*overhead/i, maximum: 2 },
  { key: "trolley", pattern: /tr[eo]lley/i, maximum: 2 },
  { key: "hook", pattern: /hook/i, maximum: 2 },
  { key: "wheel", pattern: /wheel/i, maximum: 3 },
  { key: "fan", pattern: /fan/i, maximum: 2 },
  { key: "robot", pattern: /robot(?!.*(?:support|pedestal))/i, maximum: 2 },
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
    used: 0,
  }));
  const picked = [];
  const pickedIds = new Set();
  const pickedNames = new Set();

  const pick = (part, family) => {
    pickedIds.add(part.id);
    pickedNames.add(normalizedPartName(part));
    picked.push({ ...part, mechanismFamily: family.key });
  };

  // First spend every family's allowance on different readable CAD labels. Production assemblies often
  // instance one named solid hundreds of times; taking the first IDs would satisfy a numeric track count
  // while directing a repetitive, visually ambiguous film.
  let progressed = true;
  while (picked.length < count && progressed) {
    progressed = false;
    for (const pool of pools) {
      if (picked.length >= count || pool.used >= pool.maximum) continue;
      const part = pool.parts.find((candidate) =>
        !pickedIds.has(candidate.id) && !pickedNames.has(normalizedPartName(candidate)));
      if (!part) continue;
      pool.used += 1;
      pick(part, pool);
      progressed = true;
    }
  }

  // If the balanced family allowances did not reach the delivery count, keep preferring new source labels
  // before accepting repeated instances. Family assignment still comes from the same deterministic table.
  for (const part of ordered) {
    if (picked.length >= count) break;
    if (pickedIds.has(part.id) || pickedNames.has(normalizedPartName(part))) continue;
    const family = mechanismFamilies.find((candidate) => candidate.pattern.test(part.name));
    if (!family) continue;
    pick(part, family);
  }

  // Repeated labels are legitimate distinct CAD instances and remain eligible for the remaining animation
  // tracks, but only after the director has exhausted readable diversity.
  for (const part of ordered) {
    if (picked.length >= count) break;
    if (pickedIds.has(part.id)) continue;
    const family = mechanismFamilies.find((candidate) => candidate.pattern.test(part.name));
    if (!family) continue;
    pick(part, family);
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

const TAU = Math.PI * 2;

/**
 * The displacement each industrial mechanism class actually performs, in the scene's canonical metres
 * (the CAD readers normalise millimetre sources through `meters_per_unit`, so 1.0 here is one real metre).
 *
 * Two distinct cycles exist because industrial motion is genuinely of two kinds:
 *
 * - `oscillate` — a stroke that goes out and comes back: clamps, hoists, tool approaches, rail indexing.
 *   The authored value returns to exactly zero at the end of the loop.
 * - `revolve` — continuously rotating machinery: wheels, pulleys, fans, turnwheels. The authored value
 *   ramps monotonically through whole turns, so the mechanism keeps turning at a constant rate and the
 *   pose at the end of the loop is identical to the pose at its start (a whole number of revolutions).
 *
 * Making rotary parts oscillate through a fraction of a turn — the previous behaviour — was not a
 * conservative choice but a wrong one: a roller wheel that rocks 18° and a fan that never completes a
 * revolution both read as broken machinery rather than restrained machinery.
 */
export function motionProfileFor(part) {
  const name = normalizedPartName(part);
  if (/fan.*filter|filter.*fan/.test(name)) {
    return { motion: "prismatic", revolute: false, cycle: "oscillate", axis: [0, 0, 1], amplitude: 0.05, rationale: "a short service-filter extraction rather than an implausible filter-housing spin" };
  }
  if (/turnwheel|\bwheel\b|\bfan\b/.test(name)) {
    const isFan = /\bfan\b/.test(name);
    const isWheel = /\bwheel\b/.test(name);
    // WHOLE turns per 12s loop — a fractional sweep would leave the mechanism in a different pose than it
    // started in, so the loop would visibly jump every time it repeated. A cooling fan runs faster than a
    // rolling wheel but stays far below the capture rate, so rotation reads as rotation, not as a strobe.
    const turns = isFan ? 2 : 1;
    return {
      motion: "revolute",
      revolute: true,
      cycle: "revolve",
      axis: isWheel ? [1, 0, 0] : [0, 0, 1],
      amplitude: TAU * turns,
      turns,
      rationale: `${turns} whole revolution(s) per 12s loop around the named rotary component`,
    };
  }
  if (/hook/.test(name)) {
    return { motion: "prismatic", revolute: false, cycle: "oscillate", axis: [0, 1, 0], amplitude: 0.45, rationale: "a hoist raise and lower well within a gantry hook's real vertical travel" };
  }
  if (/carrier|\bcart\b|tr[eo]lley|irbt/.test(name)) {
    return { motion: "prismatic", revolute: false, cycle: "oscillate", axis: [1, 0, 0], amplitude: 1.2, rationale: "a travel along the factory rail short of the rail's own length" };
  }
  if (/power.*skid|skid.*(?:strap|bar|fixture|table|post)/.test(name)) {
    return { motion: "prismatic", revolute: false, cycle: "oscillate", axis: [1, 0, 0], amplitude: 0.08, rationale: "a restrained skid-fixture indexing move" };
  }
  if (/toggle.*clamp|clamp.*toggle|\bclamp\b/.test(name)) {
    return { motion: "prismatic", revolute: false, cycle: "oscillate", axis: [0, 1, 0], amplitude: 0.05, rationale: "a clamp engage/release stroke, seen in close-up" };
  }
  if (/weld.*boom.*base|boom.*base.*weld/.test(name)) {
    return { motion: "revolute", revolute: true, cycle: "oscillate", axis: [0, 1, 0], amplitude: 0.35, rationale: "a 20° slew around the weld-boom pedestal" };
  }
  if (/boom|moving arm|\barm\b/.test(name)) {
    return { motion: "prismatic", revolute: false, cycle: "oscillate", axis: [0, 0, 1], amplitude: 0.35, rationale: "a measured boom/arm positioning stroke" };
  }
  if (/gun|effector/.test(name)) {
    return { motion: "prismatic", revolute: false, cycle: "oscillate", axis: [0, 0, 1], amplitude: 0.12, rationale: "a tool approach to the work and retract" };
  }
  if (/weld(?:ing)?.*(?:machine|shield|box|equipment)|(?:machine|shield|box|equipment).*weld/.test(name)) {
    return { motion: "prismatic", revolute: false, cycle: "oscillate", axis: [0, 0, 1], amplitude: 0.06, rationale: "a small guarded welding-cell service stroke" };
  }
  if (/robot/.test(name)) {
    return { motion: "revolute", revolute: true, cycle: "oscillate", axis: [0, 1, 0], amplitude: 0.5, rationale: "a 29° robot-axis move between two work positions" };
  }
  if (/crane/.test(name)) {
    return { motion: "prismatic", revolute: false, cycle: "oscillate", axis: [1, 0, 0], amplitude: 1.5, rationale: "a bridge positioning move along the crane runway" };
  }
  if (/motor/.test(name)) {
    // The named CAD solid is the motor BODY, not its shaft: a body that spins would be a fabrication.
    return { motion: "revolute", revolute: true, cycle: "oscillate", axis: [0, 0, 1], amplitude: 0.10, rationale: "a subtle motor torque reaction, not a body spin" };
  }
  return { motion: "prismatic", revolute: false, cycle: "oscillate", axis: [1, 0, 0], amplitude: 0.05, rationale: "a small fallback service motion" };
}

/** The seven sample times of one 12-second mechanism loop; `index` staggers the fleet so the cell breathes. */
const loopKeyTimes = (index) => {
  const phase = (index % 4) * 0.15;
  return [0, 1.45 + phase, 3.35 + phase, 5.55 + phase, 7.65 + phase, 9.55 + phase, 12];
};

/** Seven closed-loop keys; a stroking mechanism returns to its authored zero pose at exactly 12 seconds. */
export function buildMechanismKeys(amplitude, index) {
  const [t0, t1, t2, t3, t4, t5, t6] = loopKeyTimes(index);
  return [
    { t: t0, value: 0 },
    { t: t1, value: amplitude },
    { t: t2, value: amplitude * 0.12 },
    { t: t3, value: -amplitude },
    { t: t4, value: 0 },
    { t: t5, value: amplitude * 0.62 },
    { t: t6, value: 0 },
  ];
}

/**
 * Seven keys of continuous rotation: the value ramps linearly to a whole number of turns, so angular
 * velocity is constant and the pose at t=12 is geometrically identical to the pose at t=0. The loop is
 * therefore still closed — a revolution returns a wheel to itself — without the mechanism ever reversing.
 */
export function buildRevolutionKeys(sweep, index) {
  return loopKeyTimes(index).map((t) => ({ t, value: (sweep * t) / 12 }));
}

/** Dispatch on the profile's cycle so callers never have to know which key shape a family uses. */
export function buildKeysForProfile(profile, index) {
  return profile.cycle === "revolve"
    ? buildRevolutionKeys(profile.amplitude, index)
    : buildMechanismKeys(profile.amplitude, index);
}

/**
 * The authored value a closed loop ends on. A stroking mechanism ends at zero; a revolving one ends a whole
 * number of turns later, which is the same pose. Used to assert the engine's joint readback truthfully.
 */
export function closedLoopEndValue(profile) {
  return profile.cycle === "revolve" ? profile.amplitude : 0;
}

/** True when `value` leaves the mechanism in its authored neutral pose (turns are modulo one revolution). */
export function isNeutralPose(profile, value, tolerance = 1e-6) {
  if (!Number.isFinite(value)) return false;
  if (profile.cycle !== "revolve") return Math.abs(value) <= tolerance;
  const remainder = Math.abs(value) % TAU;
  return remainder <= tolerance || Math.abs(remainder - TAU) <= tolerance;
}

/**
 * Each subject gets a CONTEXT shot and then a shot that says something different about it, in that
 * order: the wider frame establishes where the mechanism sits in the cell before the tighter one shows
 * what it does. Every pair changes both the size and the angle, which is what keeps the cut from
 * reading as a jump cut on a single subject.
 *
 * Five pairs close on `closeup`/`detail`, the two tightest cards in the catalogue. They were absent
 * from this table for a reason that turned out to be an engine defect rather than a directorial choice:
 * the continuity checker compared a shot's AUTHORED length against a per-mood minimum while ignoring
 * the mood's own duration scale, so a 1.8s close-up in a Calm cutscene -- which actually holds for 4.5
 * seconds -- was reported as "short for this mood". Authoring one meant accepting a warning, so the
 * film had no close-ups of its machinery at all. With the check aimed at the effective length, the
 * close vocabulary is usable and the direction can do what an industrial film is for: show the
 * mechanism working, large enough to read.
 */
export const CALM_SHOT_PAIRS = Object.freeze([
  ["hero", "orbit"],
  ["vista", "hero"],
  ["reveal", "closeup"],
  ["birdseye", "dropin"],
  ["sweep", "detail"],
  ["pullback", "hero"],
  ["establish", "orbit"],
  ["overshoulder", "closeup"],
  ["looming", "orbit"],
  ["vista", "sweep"],
  ["hero", "confront"],
  ["birdseye", "detail"],
  ["reveal", "orbit"],
  ["pullback", "closeup"],
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
  confront: 2.0,
  detail: 1.6,
  pullback: 3.2,
  sweep: 3.0,
});

export const calmSecondsForKinds = (kinds) => kinds.reduce((seconds, kind) => {
  const authored = authoredShotSeconds[kind];
  if (!Number.isFinite(authored)) throw new Error(`No duration is recorded for shot kind ${kind}`);
  return seconds + authored * 2.5;
}, 0);

/**
 * Assign each filmed subject its two Calm shots.
 *
 * Cutscenes play in entity-key order, so the FIRST and LAST subjects in that order are the film's
 * opening and closing sequences whether or not anybody treats them that way. Given an `assemblyId` —
 * the imported CAD wrapper that contains the whole factory — the opening shot frames the assembly
 * instead of the part, and the closing shot pulls back out to it again.
 *
 * That is ordinary film grammar (establish the place, work through its detail, leave the place), and
 * without it a film assembled from fifteen part-sized subjects never once shows how big the factory is:
 * every shot is framed on its own subject's bounds, so even a "vista" of a clamp is a shot of a clamp.
 */
export function buildCalmShotAssignments(subjects, { assemblyId = null } = {}) {
  const list = subjects ?? [];
  const lastIndex = list.length - 1;
  return list.map((subject, index) => {
    let kinds = CALM_SHOT_PAIRS[index % CALM_SHOT_PAIRS.length].map((kind) => ({ kind, subject: subject.id }));
    if (assemblyId && list.length > 1) {
      if (index === 0) {
        kinds = [{ kind: "establish", subject: assemblyId }, { kind: "hero", subject: subject.id }];
      } else if (index === lastIndex) {
        kinds = [{ kind: "orbit", subject: subject.id }, { kind: "pullback", subject: assemblyId }];
      }
    }
    return {
      id: subject.id,
      name: subject.name,
      mood: "calm",
      kinds,
      plannedSeconds: calmSecondsForKinds(kinds.map(({ kind }) => kind)),
    };
  });
}
