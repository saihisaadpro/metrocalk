import assert from "node:assert/strict";
import test from "node:test";
import {
  buildCalmShotAssignments,
  buildKeysForProfile,
  buildRevolutionKeys,
  chooseFilmedSubjects,
  closedLoopEndValue,
  FACTORY_ACCEPTANCE,
  isNeutralPose,
  motionProfileFor,
  selectMechanismParts,
} from "./factory-cinematic.js";

const names = [
  "Toggle_Clamp_Pos_block A",
  "Toggle_Clamp_Pos_block B",
  "TurnWheel_SupportClamp_Plate A",
  "TurnWheel_SupportClamp_Plate B",
  "Weld boom moving arm A",
  "Weld boom moving arm B",
  "Hand Welding Gun A",
  "Robot weld end effector A",
  "IRBT track carrier A",
  "IRBT track carrier B",
  "Power skid Cart A",
  "Skid Cart B",
  "Overhead Crane 10 Ton A",
  "Overhead Crane 10 Ton B",
  "Trolley motor A",
  "GORBEL hook A",
  "Drive wheel A",
  "Drive wheel B",
  "Cooling fan A",
  "Robot pedestal A",
  "Motor A",
  "Motor B",
  "Support clamp A",
  "Support clamp B",
  "Support clamp C",
  "Weld boom base C",
  "Power skid Cart C",
];

const candidates = names.map((name, index) => ({
  id: `part-${String(index).padStart(2, "0")}`,
  name,
  fidelity: index % 8 === 0 ? "tessellation-only" : "exact-brep",
}));

test("semantic mechanism selection is deterministic, diverse, and entity-distinct", () => {
  const selected = selectMechanismParts([...candidates, candidates[0]], FACTORY_ACCEPTANCE.animationTracks);
  const repeated = selectMechanismParts([...candidates].reverse(), FACTORY_ACCEPTANCE.animationTracks);
  assert.equal(selected.length, FACTORY_ACCEPTANCE.animationTracks);
  assert.equal(new Set(selected.map(({ id }) => id)).size, FACTORY_ACCEPTANCE.animationTracks);
  assert.deepEqual(selected.map(({ id }) => id), repeated.map(({ id }) => id));
  assert.ok(new Set(selected.map(({ mechanismFamily }) => mechanismFamily)).size >= 10);
  assert.equal(chooseFilmedSubjects(selected).length, FACTORY_ACCEPTANCE.filmedSubjects);
});

test("instance-heavy factory names cannot crowd readable mechanism diversity out of the film", () => {
  const productionLikeNames = [
    ...Array(102).fill("Copy of Weld boom base"),
    ...Array(102).fill("Weld boom base"),
    "Weld boom moving arm_1",
    "Hand Welding Gun",
    "IRBT_Left_Section_WithCableGuideL",
    "IRBT_Mid_Section_WithCableGuideL",
    "IRBT_Right_Section_WithCableGuideL",
    "GORBEL Overhead GantryLoad hook GN 5862-M36-540-1",
    "Load hook GN 5862-M36-54 (0)-2",
    "eye HookP700_Default",
    "PULLEY WHEEL",
    "Small Roller Wheel",
    "Drive wheel",
    "Cooling fan",
    "Robot Rail support",
    "Robot weld end effector",
    "Motor",
    "Motor_1",
    "Trelley motor",
    "Support clamp",
    "Toggle clamp",
    "Turnwheel clamp",
  ];
  const instanceHeavy = productionLikeNames.map((name, index) => ({
    id: `factory-${String(index).padStart(3, "0")}`,
    name,
    fidelity: "exact-brep",
  }));
  const selected = selectMechanismParts(instanceHeavy, FACTORY_ACCEPTANCE.animationTracks);
  assert.equal(selected.length, FACTORY_ACCEPTANCE.animationTracks);
  assert.equal(new Set(selected.map(({ id }) => id)).size, FACTORY_ACCEPTANCE.animationTracks);
  assert.ok(new Set(selected.map((part) => part.name.trim().toLocaleLowerCase().replace(/[_\s-]+/g, " "))).size >= 15);
});

test("every mechanism profile produces a finite seven-key loop that closes on a neutral pose", () => {
  for (const [index, part] of candidates.entries()) {
    const profile = motionProfileFor(part);
    assert.equal(Math.hypot(...profile.axis), 1);
    assert.ok(profile.amplitude > 0);
    assert.ok(profile.cycle === "oscillate" || profile.cycle === "revolve");
    // Prismatic travel stays inside what the named mechanism can physically do; a crane bridge and a rail
    // carrier move metres, a clamp moves centimetres, and nothing is allowed to fling a part across the cell.
    if (!profile.revolute) assert.ok(profile.amplitude <= 1.5, `${part.name} travels ${profile.amplitude}m`);

    const keys = buildKeysForProfile(profile, index);
    assert.equal(keys.length, 7);
    assert.equal(keys[0].t, 0);
    assert.equal(keys[0].value, 0);
    assert.equal(keys.at(-1).t, 12);
    assert.ok(keys.every(({ t, value }) => Number.isFinite(t) && Number.isFinite(value)));
    assert.ok(keys.every((key, at) => at === 0 || key.t > keys[at - 1].t));
    assert.ok(Math.abs(keys.at(-1).value - closedLoopEndValue(profile)) <= 1e-12);
    assert.ok(isNeutralPose(profile, keys.at(-1).value),
      `${part.name} does not end its loop in the pose it started in`);
    // Every authored sample must survive the joint's own travel limits unclamped.
    const limit = Math.abs(profile.amplitude) * 1.15;
    assert.ok(keys.every(({ value }) => Math.abs(value) <= limit + 1e-12));
  }
});

test("rotary machinery turns whole revolutions at a constant rate instead of rocking in place", () => {
  const rotary = ["PULLEY WHEEL", "Small Roller Wheel", "Cooling fan", "TurnWheel_SupportClamp_Plate A"];
  for (const [index, name] of rotary.entries()) {
    const profile = motionProfileFor({ name });
    assert.equal(profile.cycle, "revolve", `${name} should keep turning`);
    assert.equal(profile.revolute, true);
    // A whole number of turns is what makes the loop closed AND the motion continuous. A fractional sweep
    // would park the wheel somewhere else than it started and jump on every repeat.
    assert.ok(Number.isInteger(profile.turns), `${name} turns ${profile.turns} times`);
    assert.ok(Math.abs(profile.amplitude - Math.PI * 2 * profile.turns) <= 1e-12);

    const keys = buildRevolutionKeys(profile.amplitude, index);
    assert.ok(keys.every((key, at) => at === 0 || key.value > keys[at - 1].value),
      `${name} must never reverse mid-loop`);
    // Constant angular velocity: every sample sits on the same straight ramp.
    const rate = profile.amplitude / 12;
    assert.ok(keys.every(({ t, value }) => Math.abs(value - rate * t) <= 1e-12));
    assert.ok(isNeutralPose(profile, keys.at(-1).value));
    // The motion has to be big enough to read on camera — the defect being fixed here was an 18° wobble.
    assert.ok(profile.amplitude >= Math.PI, `${name} only sweeps ${profile.amplitude} rad`);
  }
});

test("a motor body reacts to torque rather than spinning, which would be a fabrication", () => {
  const motor = motionProfileFor({ name: "Motor" });
  assert.equal(motor.cycle, "oscillate");
  assert.ok(motor.amplitude <= 0.2);
  // A trolley motor travels with the trolley it drives; the rail family owns that name before the
  // motor family sees it, and that ordering is deliberate.
  assert.equal(motionProfileFor({ name: "Trelley motor" }).cycle, "oscillate");
  assert.equal(motionProfileFor({ name: "Trelley motor" }).revolute, false);
  assert.equal(isNeutralPose(motor, 0), true);
  assert.equal(isNeutralPose(motor, 0.1), false);
  // A revolving profile is neutral at any whole turn, and only at a whole turn.
  const wheel = motionProfileFor({ name: "Drive wheel" });
  assert.equal(isNeutralPose(wheel, Math.PI * 2), true);
  assert.equal(isNeutralPose(wheel, Math.PI * 4), true);
  assert.equal(isNeutralPose(wheel, Math.PI), false);
});

test("the 15-subject Calm direction contains 30 shots and clears three minutes with margin", () => {
  const subjects = chooseFilmedSubjects(selectMechanismParts(candidates));
  const assignments = buildCalmShotAssignments(subjects);
  assert.equal(assignments.length, FACTORY_ACCEPTANCE.filmedSubjects);
  assert.equal(assignments.flatMap(({ kinds }) => kinds).length, FACTORY_ACCEPTANCE.cinematicShots);
  const seconds = assignments.reduce((total, assignment) => total + assignment.plannedSeconds, 0);
  // 193.5s: five pairs now end on `closeup`/`detail`, the catalogue's two shortest cards, which is
  // 15.25s less screen time than the all-wide table it replaced -- and still a comfortable margin over
  // the three-minute delivery floor. Close-ups are the point of the trade, not a cost to minimise.
  assert.equal(seconds, 193.5);
  assert.ok(seconds > FACTORY_ACCEPTANCE.minimumCinematicSeconds);
  // Every shot frames its own subject when no assembly is named.
  assert.ok(assignments.every(({ id, kinds }) => kinds.every(({ subject }) => subject === id)));
});

test("the film opens on the whole assembly and pulls back out to it at the end", () => {
  const subjects = chooseFilmedSubjects(selectMechanismParts(candidates));
  const ordered = [...subjects].sort((left, right) => left.id.localeCompare(right.id));
  const assemblyId = "1_0";
  const assignments = buildCalmShotAssignments(ordered, { assemblyId });

  assert.equal(assignments.length, FACTORY_ACCEPTANCE.filmedSubjects);
  assert.equal(assignments.flatMap(({ kinds }) => kinds).length, FACTORY_ACCEPTANCE.cinematicShots,
    "establishing the place must not cost the film any of its authored shots");

  const opening = assignments[0];
  const closing = assignments.at(-1);
  assert.deepEqual(opening.kinds, [
    { kind: "establish", subject: assemblyId },
    { kind: "hero", subject: opening.id },
  ], "open wide on the factory, then push in to the first mechanism");
  assert.deepEqual(closing.kinds, [
    { kind: "orbit", subject: closing.id },
    { kind: "pullback", subject: assemblyId },
  ], "leave the last mechanism by pulling back out to the whole factory");

  // Exactly two shots may frame the assembly; the middle of the film stays on its mechanisms.
  const onAssembly = assignments.flatMap(({ kinds }) => kinds).filter(({ subject }) => subject === assemblyId);
  assert.equal(onAssembly.length, 2);
  assert.ok(assignments.slice(1, -1).every(({ id, kinds }) => kinds.every(({ subject }) => subject === id)));

  const seconds = assignments.reduce((total, assignment) => total + assignment.plannedSeconds, 0);
  assert.ok(seconds > FACTORY_ACCEPTANCE.minimumCinematicSeconds, `only ${seconds}s`);
});

test("a single-subject direction is left alone rather than losing its only shots to the assembly", () => {
  const solo = buildCalmShotAssignments([{ id: "1_7", name: "Lone motor" }], { assemblyId: "1_0" });
  assert.equal(solo.length, 1);
  assert.ok(solo[0].kinds.every(({ subject }) => subject === "1_7"));
});
