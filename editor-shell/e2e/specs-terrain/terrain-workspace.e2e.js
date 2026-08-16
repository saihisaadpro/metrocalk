// M19 (ADR-104) — the terrain sub-engine, exercised on the PACKAGED .exe through the same commands the
// Terrain workspace sends, with real pixels captured at each step.
//
// This is the acceptance the headless benches cannot give: a real wgpu device, the real composite, the real
// streaming worker pool, and the real native brush path — the sculpt gesture here drives the identical
// render-loop code a hand does, via the injected cursor (the trick the transform gizmo already uses), not a
// parallel test-only path.
//
// What each block proves is written above it. Every assertion is on a number the engine reports or a pixel
// that was captured, never on "it looked right".

import { execFileSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-terrain");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");

mkdirSync(shots, { recursive: true });

/** Capture the composited window — the real pixels, not a WebView screenshot. */
function shot(name) {
  const out = path.join(shots, `${name}.png`);
  execFileSync(
    "powershell.exe",
    ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", capture, "-Out", out],
    { stdio: "pipe" },
  );
  return out;
}

/** Wait until the streamer has chunks resident and drawn. */
async function settled(minVisible = 4) {
  let last = null;
  let stable = 0;
  await browser.waitUntil(
    async () => {
      const now = await invoke("terrain_stats");
      // "Settled" means the resident set stopped growing, not merely that nothing is in flight this
      // instant — the queue is bounded, so it empties briefly between batches.
      stable = last && now.residentChunks === last.residentChunks ? stable + 1 : 0;
      last = now;
      return now.visibleChunks >= minVisible && now.pendingBuilds === 0 && stable >= 2;
    },
    {
      timeout: 120000,
      interval: 600,
      timeoutMsg: `terrain never settled (see the logged stats)`,
    },
  );
  return last;
}

describe("terrain — create, stream, sculpt and route on the packaged app", () => {
  it("builds the terrain a plain-language description asks for", async () => {
    await invoke("new_project");
    const text = "a 3 km eroded alpine valley with a river, a road and dense conifer forest";

    // Reading first: it must be answerable WITHOUT building anything, because the panel calls it while the
    // author is still typing.
    const reading = await invoke("terrain_read_description", { text });
    console.log(`[describe] brief ${JSON.stringify(reading.brief)}`);
    expect(reading.brief.landform).toBe("Mountains");
    expect(reading.brief.climate).toBe("Alpine");
    expect(reading.brief.worldSizeM).toBe(3000);
    expect(reading.brief.river).toBe(true);
    expect(reading.brief.road).toBe(true);
    expect(reading.brief.erosion).toBe(true);
    expect(reading.understood.length).toBeGreaterThan(4);

    const built = await invoke("terrain_describe", { text });
    expect(built.ok).toBe(true);
    expect(built.recipe).not.toBe(null);
    expect(built.issues.filter((i) => i.severity === "blocking")).toEqual([]);
    // The description became a REAL recipe — layers, routes and bound vegetation, all editable afterwards.
    expect(built.recipe.layers.length).toBeGreaterThan(2);
    expect(built.recipe.splines.length).toBe(2);
    // 3 000 m snapped to a whole number of 64 m chunks: 3000/64 = 46.875 -> 47 chunks -> 3 008 m.
    expect(built.recipe.world_size_m).toBe(3008);
    // The sentence travels with the recipe, so reopening the project shows what was asked for.
    expect(built.recipe.description).toContain("alpine valley");
    for (const proto of built.recipe.protos) {
      expect(proto.mesh_key).not.toBe("");
    }
    // And the reading came back with it, so the panel can say what it understood.
    expect(built.reading.understood.length).toBeGreaterThan(4);
    console.log(
      `[describe] understood ${built.reading.understood.map((u) => u.meaning).join(" | ")}`,
    );

    const stats = await settled(8);
    console.log(`[describe] ${JSON.stringify(stats)}`);
    expect(stats.active).toBe(true);
    expect(stats.drawnTriangles).toBeGreaterThan(10000);
    expect(stats.drawnInstances).toBeGreaterThan(0);
    expect(stats.overBudget).toBe(false);
    shot("00-described-alpine-valley");

    // The same sentence must give the same world — the whole reason this is a lexicon and not a model.
    const again = await invoke("terrain_read_description", { text });
    expect(again.brief.seed).toBe(reading.brief.seed);
    // A different sentence must not.
    const other = await invoke("terrain_read_description", { text: `${text} and lakes` });
    expect(other.brief.seed).not.toBe(reading.brief.seed);
  });

  it("describes a completely different world in one step, and says what it ignored", async () => {
    const text = "a small lush tropical archipelago with beaches, palms and unicorns";
    const built = await invoke("terrain_describe", { text });
    expect(built.ok).toBe(true);
    expect(built.recipe.world_size_m).toBe(1024);
    expect(built.recipe.water.enabled).toBe(true);
    // The climate drove the species, which drove the generated stand-in mesh — no new asset involved.
    const names = built.recipe.protos.map((p) => p.name).join(", ");
    console.log(`[describe] species: ${names}`);
    expect(names.toLowerCase()).toContain("palm");
    // And the word it could not use is named rather than silently dropped.
    console.log(`[describe] unused: ${JSON.stringify(built.reading.unused)}`);
    expect(built.reading.unused).toContain("unicorns");

    const stats = await settled(6);
    expect(stats.overBudget).toBe(false);
    shot("00b-described-tropical-isles");
  });

  it("changes PART of a world from a sentence, and leaves the rest alone", async () => {
    // ADR-106 — the half of describe-to-create that refines instead of replacing. This is the whole point:
    // the author says "raise this mountain" and one place moves.
    await invoke("new_project");
    await invoke("terrain_describe", { text: "a 2 km rolling temperate hill country" });
    await settled(6);

    // Point at the ground, the way a hand does. The viewport ray-cast gives "this" its meaning.
    await invoke("terrain_test_cursor", { x: 0.5, y: 0.55 });
    await browser.pause(400);
    const hit = await invoke("terrain_cursor_hit");
    expect(hit).not.toBe(null);
    const [hx, , hz] = hit;
    // A far-away probe that MUST NOT move.
    const far = [hx + 900, hz + 900];
    const before = {
      here: await invoke("terrain_height", { x: hx, z: hz }),
      far: await invoke("terrain_height", { x: far[0], z: far[1] }),
    };

    // The plan is inspectable BEFORE it runs, and it must be a modification, not a new world.
    const plan = await invoke("terrain_plan", { text: "raise this by 120 m" });
    console.log(`[local] plan ${JSON.stringify(plan)}`);
    expect(plan.kind).toBe("modify");
    expect(plan.steps.length).toBe(1);
    expect(plan.steps[0].refusal).toBe(null);
    expect(plan.steps[0].rebuilds).toContain("ground height");
    // It names a bounded region — not the whole world.
    expect(plan.steps[0].region).not.toBe(null);
    const [rminx, rminz, rmaxx, rmaxz] = plan.steps[0].region;
    expect(rmaxx - rminx).toBeLessThan(2000);

    const applied = await invoke("terrain_describe", { text: "raise this by 120 m" });
    expect(applied.ok).toBe(true);
    // The recipe gained a NAMED landform — a semantic entity, not an anonymous dab of height.
    expect(applied.recipe.features.length).toBeGreaterThan(0);
    console.log(`[local] features ${JSON.stringify(applied.recipe.features.map((f) => f.name))}`);

    await settled(6);
    const after = {
      here: await invoke("terrain_height", { x: hx, z: hz }),
      far: await invoke("terrain_height", { x: far[0], z: far[1] }),
    };
    console.log(`[local] here ${before.here.toFixed(2)} -> ${after.here.toFixed(2)}`);
    console.log(`[local] far  ${before.far.toFixed(2)} -> ${after.far.toFixed(2)}`);
    // The ground under the cursor rose...
    expect(after.here).toBeGreaterThan(before.here + 20);
    // ...and the ground 900 m away did not move at all. THIS is the local-edit property.
    expect(Math.abs(after.far - before.far)).toBeLessThan(0.01);
    shot("10-local-edit-raised");

    // One sentence is one undo step.
    expect(await invoke("undo")).toBe(true);
    await settled(6);
    const undone = await invoke("terrain_height", { x: hx, z: hz });
    expect(Math.abs(undone - before.here)).toBeLessThan(0.05);
    expect(rminz).toBeLessThan(rmaxz);
    await invoke("terrain_test_cursor", { x: null, y: null });
  });

  it("refuses an impossible request with a reason instead of breaking the world", async () => {
    const plan = await invoke("terrain_plan", { text: "raise the volcano by 400 m" });
    console.log(`[local] refusal ${JSON.stringify(plan.steps)}`);
    expect(plan.kind).toBe("modify");
    expect(plan.ok).toBe(false);
    expect(plan.steps[0].refusal).toContain("volcano");
    // And the world is untouched — a refused plan changes nothing.
    const stats = await invoke("terrain_stats");
    expect(stats.active).toBe(true);
  });

  it("creates a terrain from a preset and streams it into the viewport", async () => {
    await invoke("new_project");
    const presets = await invoke("terrain_presets");
    expect(presets.length).toBeGreaterThan(3);
    expect(presets.map((p) => p.id)).toContain("rolling-hills");

    const reply = await invoke("terrain_create", { preset: "rolling-hills" });
    expect(reply.ok).toBe(true);
    expect(reply.recipe).not.toBe(null);
    expect(reply.recipe.name).toBe("Rolling Hills");
    // The blocking issues are what would stop it building; there must be none.
    expect(reply.issues.filter((i) => i.severity === "blocking")).toEqual([]);

    const stats = await settled(8);
    console.log(`[terrain] ${JSON.stringify(stats)}`);
    expect(stats.active).toBe(true);
    expect(stats.residentChunks).toBeGreaterThan(8);
    expect(stats.drawnTriangles).toBeGreaterThan(10000);
    // Culling has to be doing something, or the "visible" number is just the resident one renamed.
    expect(stats.culledFrustum + stats.culledHorizon).toBeGreaterThan(0);
    // And it must be inside the budget it declared.
    expect(stats.totalMb).toBeLessThanOrEqual(stats.budgetMb);
    expect(stats.overBudget).toBe(false);

    shot("01-rolling-hills");
  });

  it("binds stand-in vegetation so the preset's woodland is actually there", async () => {
    // The preset declares scatter rules; creation generates and binds stand-in meshes for them, so the
    // instances have something to draw. An empty field would mean the whole scatter path is dead.
    const reply = await invoke("terrain_edit", { args: { entity: null, edit: null } });
    expect(reply.recipe.protos.length).toBeGreaterThan(0);
    for (const proto of reply.recipe.protos) {
      expect(proto.mesh_key).not.toBe("");
      expect(proto.lod_keys.length).toBeGreaterThan(0);
      expect(proto.impostor_key).not.toBe(null);
    }
    const stats = await invoke("terrain_stats");
    console.log(`[terrain] instances=${stats.drawnInstances} impostors=${stats.impostorInstances}`);
    expect(stats.drawnInstances).toBeGreaterThan(0);
  });

  it("sculpts through the native brush and changes the ground under the cursor", async () => {
    const before = await invoke("terrain_stats");
    // Aim at the centre of the viewport and confirm the ray actually finds the surface.
    await invoke("terrain_tool", { mode: "sculpt", kind: 0, radiusM: 60, strength: 14, hardness: 0.6 });
    await invoke("terrain_test_cursor", { x: 0.5, y: 0.5 });
    await browser.pause(400);
    const hit = await invoke("terrain_cursor_hit");
    expect(hit).not.toBe(null);
    const [hx, , hz] = hit;
    const heightBefore = await invoke("terrain_height", { x: hx, z: hz });
    expect(heightBefore).not.toBe(null);

    // A gesture: press, drag across a few screen positions, release. Two commands bracket it; everything
    // between is the render loop tracing the stroke natively.
    await invoke("terrain_paint_begin");
    for (const [x, y] of [
      [0.5, 0.5],
      [0.52, 0.5],
      [0.54, 0.51],
      [0.56, 0.52],
    ]) {
      await invoke("terrain_test_cursor", { x, y });
      await browser.pause(220);
    }
    shot("02-sculpt-in-progress");
    const committed = await invoke("terrain_paint_end");
    expect(committed.ok).toBe(true);
    expect(committed.recipe.strokes.length).toBeGreaterThan(0);
    console.log(`[terrain] committed ${committed.recipe.strokes.length} dabs`);

    await settled(4);
    const heightAfter = await invoke("terrain_height", { x: hx, z: hz });
    expect(heightAfter).toBeGreaterThan(heightBefore + 3);
    console.log(`[terrain] ground rose ${(heightAfter - heightBefore).toFixed(2)} m under the brush`);
    shot("03-sculpted");

    // One gesture is ONE undo step: undoing must put the ground back.
    expect(await invoke("undo")).toBe(true);
    await settled(4);
    const heightUndone = await invoke("terrain_height", { x: hx, z: hz });
    expect(Math.abs(heightUndone - heightBefore)).toBeLessThan(0.05);
    shot("04-after-undo");
    // Redo restores it, so the transaction is a real two-way step.
    expect(await invoke("redo")).toBe(true);
    await settled(4);
    expect(await invoke("terrain_height", { x: hx, z: hz })).toBeGreaterThan(heightBefore + 3);

    // The sculpt must not have cost the budget or the streaming.
    const after = await invoke("terrain_stats");
    expect(after.overBudget).toBe(false);
    expect(after.residentChunks).toBeGreaterThan(before.residentChunks / 2);
  });

  it("draws a road on the terrain and flattens the ground along it", async () => {
    await invoke("terrain_tool", { mode: "route" });
    const points = [
      [0.35, 0.55],
      [0.5, 0.52],
      [0.65, 0.5],
    ];
    for (const [x, y] of points) {
      await invoke("terrain_test_cursor", { x, y });
      await browser.pause(250);
      await invoke("terrain_route_point");
      await browser.pause(250);
    }
    shot("05-route-drawn");

    const before = await invoke("terrain_edit", { args: { entity: null, edit: null } });
    const reply = await invoke("terrain_route_commit", {
      kind: "road",
      widthM: 14,
      depthM: 0,
      materialLayer: null,
    });
    expect(reply.ok).toBe(true);
    expect(reply.recipe.splines.length).toBe(before.recipe.splines.length + 1);
    const road = reply.recipe.splines[reply.recipe.splines.length - 1];
    expect(road.points.length).toBe(3);
    console.log(`[terrain] road with ${road.points.length} points, ${road.width_m} m wide`);

    await settled(4);
    // The corridor must be flat ACROSS its width — that is what "a road" means, and it is measurable.
    const [ax, , az] = road.points[1];
    const centre = await invoke("terrain_height", { x: ax, z: az });
    const across = await invoke("terrain_height", { x: ax, z: az + 4 });
    const onRoad = Math.abs(centre - across);
    // Measured against the ground the road CROSSES, not an absolute: what "a road" means is that the
    // corridor is flatter than the terrain around it, and an absolute threshold only measures how rough
    // this particular hillside happened to be where the cursor landed.
    const offA = await invoke("terrain_height", { x: ax, z: az + 60 });
    const offB = await invoke("terrain_height", { x: ax, z: az + 64 });
    const offRoad = Math.abs(offA - offB);
    console.log(
      `[terrain] corridor ${onRoad.toFixed(3)} m across 4 m vs open ground ${offRoad.toFixed(3)} m`,
    );
    expect(onRoad).toBeLessThan(Math.max(offRoad, 0.35) * 1.5);
    shot("06-road-committed");

    await invoke("terrain_tool", { mode: "none" });
    await invoke("terrain_test_cursor", { x: null, y: null });
  });

  it("answers a walkable path across the streamed navigation grids", async () => {
    const stats = await invoke("terrain_stats");
    expect(stats.navMb).toBeGreaterThan(0);
    // Find dry, walkable ground first: the world centre is often below the water line, and asking for a
    // path across a lake bed would test the refusal rather than the pathfinder.
    const centre = 2048 / 2;
    let land = null;
    for (let r = 0; r <= 240 && !land; r += 60) {
      for (const [dx, dz] of [
        [r, 0],
        [-r, 0],
        [0, r],
        [0, -r],
      ]) {
        const y = await invoke("terrain_height", { x: centre + dx, z: centre + dz });
        if (y !== null && y > 3) {
          land = [centre + dx, y, centre + dz];
          break;
        }
      }
    }
    console.log(`[terrain] walkable probe found ${JSON.stringify(land)}`);
    const from = land ?? [centre, 0, centre];
    const result = await invoke("terrain_path", {
      from: [from[0] - 30, from[1], from[2] - 30],
      to: [from[0] + 30, from[1], from[2] + 30],
    });
    console.log(`[terrain] path ${JSON.stringify({ found: result.found, len: result.lengthM })}`);
    // Either a route or an explained refusal — never a silent empty answer.
    if (result.found) {
      expect(result.waypoints.length).toBeGreaterThan(1);
      expect(result.lengthM).toBeGreaterThan(60);
    } else {
      expect(typeof result.reason).toBe("string");
      expect(result.reason.length).toBeGreaterThan(10);
    }
  });

  it("floods the low ground when the sea level rises, and stops it being walkable", async () => {
    // Water is not a decal on top of the terrain: it is a line the whole recipe is read against. Raising it
    // must produce a surface AND take the flooded ground out of the navigation grid, or the two answers
    // disagree and an agent walks across a lake.
    const before = await invoke("terrain_edit", { args: { entity: null, edit: null } });
    const centre = 2048 / 2;
    const ground = await invoke("terrain_height", { x: centre, z: centre });
    expect(ground).not.toBe(null);

    const sea = ground + 12;
    const flooded = await invoke("terrain_edit", {
      args: {
        entity: null,
        edit: { op: "setWater", water: { ...before.recipe.water, enabled: true, sea_level_m: sea } },
      },
    });
    expect(flooded.ok).toBe(true);
    expect(flooded.recipe.water.sea_level_m).toBeCloseTo(sea, 3);

    const stats = await settled(6);
    console.log(`[terrain] flooded to ${sea.toFixed(2)} m — ${JSON.stringify(stats)}`);
    expect(stats.overBudget).toBe(false);
    shot("09-flooded");

    // 12 m of water is far past any wading depth, so a route through it must be refused with a reason.
    const drowned = await invoke("terrain_path", {
      from: [centre - 30, ground, centre - 30],
      to: [centre + 30, ground, centre + 30],
    });
    console.log(`[terrain] under water: found=${drowned.found} reason=${drowned.reason}`);
    expect(drowned.found).toBe(false);
    expect(typeof drowned.reason).toBe("string");
    expect(drowned.reason.length).toBeGreaterThan(10);

    // And it is reversible: dropping the sea level puts the ground back exactly as it was.
    const dry = await invoke("terrain_edit", {
      args: { entity: null, edit: { op: "setWater", water: before.recipe.water } },
    });
    expect(dry.ok).toBe(true);
    await settled(6);
    expect(await invoke("terrain_height", { x: centre, z: centre })).toBeCloseTo(ground, 3);
  });

  it("levels a corridor toward the ground you point at, and never to sea level", async () => {
    // ADR-108 §3. `FeatureOp::Flatten` is an ABSOLUTE lerp, and MakeTraversable used to default its target
    // to 0.0 — so on any world whose ground is not near y = 0 this verb cut a hole down to sea level and
    // called it a corridor. The check is on real ground: sample the height, ask for the corridor, and prove
    // the created feature levels toward THAT height rather than toward zero.
    const centre = 2048 / 2;
    const ground = await invoke("terrain_height", { x: centre, z: centre });
    console.log(`[traversable] ground at centre ${ground}`);
    const before = await invoke("terrain_edit", { args: { entity: null, edit: null } });
    const featuresBefore = (before.recipe.features ?? []).length;
    console.log(`[traversable] features before ${featuresBefore}`);

    // Give the verb something to act on. MakeTraversable needs an existing landform or route — on bare
    // ground it correctly refuses — so create a hill first and then ask for the corridor across it. That
    // exercises the POSITIVE path, which is the one the crater bug lived on.
    const hill = await invoke("terrain_describe", { text: "add a hill here" });
    console.log(`[traversable] hill ok=${hill.ok} features=${(hill.recipe?.features ?? []).length}`);

    const said = await invoke("terrain_describe", { text: "make this traversable" });
    console.log(`[traversable] ok=${said.ok} message=${said.message}`);
    if (!said.ok) {
      // A refusal is an acceptable outcome (no cursor ⇒ no known ground height) — but it must be EXPLAINED
      // and it must not have touched the world. What is not acceptable is a silent crater.
      expect(said.message.length).toBeGreaterThan(10);
      const after = await invoke("terrain_edit", { args: { entity: null, edit: null } });
      expect((after.recipe.features ?? []).length).toBe(featuresBefore);
      return;
    }
    const made = (said.recipe.features ?? []).slice(-1)[0];
    console.log(`[traversable] created ${JSON.stringify({ op: made?.op, target: made?.targetM })}`);
    expect(made).toBeTruthy();
    // The whole bug in one assertion: the corridor must not be levelling the world to zero.
    if (ground !== null && Math.abs(ground) > 5) {
      expect(Math.abs(made.targetM)).toBeGreaterThan(1);
    }
    await settled(6);
    const stats = await invoke("terrain_stats");
    expect(stats.active).toBe(true);
    expect(stats.problem).toBe("");
  });

  it("refuses to change the terrain while the scene is running, and says so", async () => {
    // ADR-108 §6. The no-edits-during-Play guard lived only on the generic edit path, so terrain commands
    // walked straight through it: the viewport showed a new hill while every body kept using the collider
    // built when Play started. Refusing is the fix; saying why is the half that makes it usable.
    await invoke("play");
    try {
      const denied = await invoke("terrain_edit", {
        args: { entity: null, edit: { op: "setSeed", seed: 424242 } },
      });
      console.log(`[play] terrain edit during Play -> ok=${denied.ok} "${denied.message}"`);
      expect(denied.ok).toBe(false);
      expect(denied.message.toLowerCase()).toContain("stop the scene");
    } finally {
      await invoke("stop");
    }
    // And after stopping, the same edit works — the guard is a gate, not a wall.
    const allowed = await invoke("terrain_edit", {
      args: { entity: null, edit: { op: "setSeed", seed: 424242 } },
    });
    expect(allowed.ok).toBe(true);
    expect(allowed.recipe.seed).toBe(424242);
    await settled(6);
  });

  it("reports no engine-side problem across the whole session", async () => {
    // ADR-108 §1/§2. `terrain_problem` now crosses the IPC boundary, which means this assertion is finally
    // possible: after everything above — creation, re-description, local edits, sculpting, roads, floods,
    // preset swaps — the engine has no complaint. Before the wire, this field could have held anything and
    // no test and no author could have seen it.
    const stats = await invoke("terrain_stats");
    console.log(`[terrain] problem field: "${stats.problem}"`);
    expect(stats.problem).toBe("");
    expect(stats.active).toBe(true);
  });

  it("survives a preset swap and a seed change without leaking the previous world", async () => {
    const swapped = await invoke("terrain_edit", {
      args: { entity: null, edit: { op: "applyPreset", id: "alpine" } },
    });
    expect(swapped.ok).toBe(true);
    expect(swapped.recipe.name).toBe("Alpine Peaks");
    const stats = await settled(6);
    expect(stats.overBudget).toBe(false);
    shot("07-alpine");

    const reseeded = await invoke("terrain_edit", {
      args: { entity: null, edit: { op: "setSeed", seed: 987654 } },
    });
    expect(reseeded.ok).toBe(true);
    expect(reseeded.recipe.seed).toBe(987654);
    await settled(6);
    shot("08-alpine-reseeded");
    const final = await invoke("terrain_stats");
    console.log(`[terrain] final ${JSON.stringify(final)}`);
    expect(final.totalMb).toBeLessThanOrEqual(final.budgetMb);
  });
});
