import assert from "node:assert/strict";
import test from "node:test";
import {
  buildCalmShotAssignments,
  buildMechanismKeys,
  chooseFilmedSubjects,
  FACTORY_ACCEPTANCE,
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

test("every conservative mechanism profile produces a finite seven-key closed loop", () => {
  for (const [index, part] of candidates.entries()) {
    const profile = motionProfileFor(part);
    assert.equal(Math.hypot(...profile.axis), 1);
    assert.ok(profile.amplitude > 0 && profile.amplitude <= 0.45);
    const keys = buildMechanismKeys(profile.amplitude, index);
    assert.equal(keys.length, 7);
    assert.equal(keys[0].t, 0);
    assert.equal(keys[0].value, 0);
    assert.equal(keys.at(-1).t, 12);
    assert.equal(keys.at(-1).value, 0);
    assert.ok(keys.every(({ t, value }) => Number.isFinite(t) && Number.isFinite(value)));
    assert.ok(keys.every((key, at) => at === 0 || key.t > keys[at - 1].t));
  }
});

test("the 15-subject Calm direction contains 30 shots and clears three minutes with margin", () => {
  const subjects = chooseFilmedSubjects(selectMechanismParts(candidates));
  const assignments = buildCalmShotAssignments(subjects);
  assert.equal(assignments.length, FACTORY_ACCEPTANCE.filmedSubjects);
  assert.equal(assignments.flatMap(({ kinds }) => kinds).length, FACTORY_ACCEPTANCE.cinematicShots);
  const seconds = assignments.reduce((total, assignment) => total + assignment.plannedSeconds, 0);
  assert.equal(seconds, 208.75);
  assert.ok(seconds > FACTORY_ACCEPTANCE.minimumCinematicSeconds);
});
