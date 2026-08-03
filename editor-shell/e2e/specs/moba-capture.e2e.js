// Drive a real MOB match inside the packaged editor and capture the pixels a user would actually see.
//
// The kernel is headless, so the only evidence a match ran used to be a green test count. This spec runs the
// match in the REAL .exe, through the REAL Tauri command surface, drawing REAL imported meshes on the REAL
// wgpu viewport — then captures the OS-composited window at each beat.
//
// It proves a WORKFLOW, not only mechanics. An earlier version started from a match hand-written in Rust, so
// it could show a stun stopping a hero while proving nothing about authoring. Every beat below descends from
// objects authored in the document: the scene is authored, validated, cooked, run, and restored, and the
// cooked digest is carried through so a reader can tie the running match back to what was authored.
//
// The capture is deliberately an OS-level BitBlt of the composited window, not a WebDriver screenshot: the
// viewport is a native wgpu surface beneath a transparent WebView2, so a DOM screenshot would show the React
// panels and a black hole where the 3D is. Every assertion below reads authoritative kernel state via
// `moba_status`; the images are evidence for a human, not the assertion itself.

import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-moba");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
const evidence = [];

mkdirSync(shots, { recursive: true });

/** Capture the composited window and record what the kernel said at that instant. */
async function beat(name, status) {
  const out = path.join(shots, `${name}.png`);
  execFileSync(
    "powershell.exe",
    ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", capture, "-Out", out],
    { stdio: "pipe" },
  );
  evidence.push({
    beat: name,
    tick: status.tick,
    phase: status.phase,
    worldDigest: status.world_digest,
    laneDigest: status.lane_digest,
    // The cook this match was built from. Identical across beats of one run; it is what ties every frame
    // below to the authored scene rather than to a definition written in Rust.
    cookDigest: status.cook_digest,
    cookSchemaVersion: status.cook_schema_version,
    actors: status.actor_count,
    live: status.live_actors,
    heroControls: (status.actors.find((a) => a.owned) || {}).controls || [],
    heroX: (status.actors.find((a) => a.owned) || {}).x_mm,
    heroSpeed: (status.actors.find((a) => a.owned) || {}).speed,
    // The authored entity the hero was cooked from — the traceability link.
    heroSource: (status.actors.find((a) => a.owned) || {}).source,
    shot: path.basename(out),
  });
  return status;
}

/**
 * Invoke a command that is EXPECTED to be refused, returning the refusal as data.
 *
 * A plain `invoke` inside `browser.execute` surfaces a rejection as a WebDriver JS error whose shape is
 * not the command's own error type, so the diagnostics would be lost. Catching inside the page keeps the
 * structured `MobaStartError` — which is the whole point of the assertion.
 */
const invokeExpectingRefusal = (cmd, args = {}) =>
  browser.execute(
    async (c, a) => {
      try {
        await window.__TAURI__.core.invoke(c, a);
        return null;
      } catch (error) {
        return typeof error === "string" ? { message: error, diagnostics: [] } : error;
      }
    },
    cmd,
    args,
  );

/** The authored match, recorded once so later specs can assert against it. */
let authored = null;
let cookDigest = null;

describe("MOB match running inside the packaged editor", () => {
  it("refuses to run a scene that has no match, and says what to add", async () => {
    // This is the guard against the gap this whole feature closed. If a match could start without anything
    // authored, the demonstration would again be proving mechanics rather than a pipeline.
    const before = await invoke("moba_validate");
    expect(before.is_match_scene).toBe(false);
    expect(before.ok).toBe(false);

    const refused = await invokeExpectingRefusal("moba_start");
    expect(refused).not.toBe(null);
    const text = JSON.stringify(refused);
    // Not a stack trace or an enum name — a sentence naming the object to create.
    expect(text).toContain("Match Settings");
    await beat("00-no-match-refused", await invoke("moba_status"));
  });

  it("authors a starter match into the document and validates it", async () => {
    authored = await invoke("moba_author_starter");
    expect(authored.actors.length).toBe(3);
    expect(authored.waypoints.length).toBe(2);
    expect(authored.waves.length).toBe(1);

    const validated = await invoke("moba_validate");
    expect(validated.is_match_scene).toBe(true);
    expect(validated.ok).toBe(true);
    expect(validated.diagnostics.length).toBe(0);
    expect(validated.actor_count).toBe(3);
    expect(validated.wave_count).toBe(1);
    // A 12 m lane, cooked from the two authored waypoints.
    expect(Math.round(validated.lane_length_m)).toBe(12);
    expect(typeof validated.cook_digest).toBe("string");
    cookDigest = validated.cook_digest;

    await invoke("view_preset", { preset: "top" });
    await invoke("frame_all");
    await beat("01a-match-authored", await invoke("moba_status"));
  });

  it("surfaces the whole loop in the Match workspace, not only on the command surface", async () => {
    // A capability reachable only from automated tests is not a feature. This drives the REAL panel in the
    // REAL packaged app: open the workspace, and check it is showing the authored match rather than the
    // "no match here" empty state it showed before anything was authored.
    const tab = await $("#inspector-workspaces-match-tab");
    await tab.waitForExist({ timeout: 10000 });
    await tab.click();
    const panel = await $("#inspector-workspaces-match-panel");
    await panel.waitForDisplayed({ timeout: 10000 });
    await browser.waitUntil(
      async () => (await panel.getText()).includes("Your match"),
      { timeout: 10000, timeoutMsg: "the Match workspace never showed the authored match" },
    );
    const text = await panel.getText();
    // The summary is the authored numbers, and the fingerprint ties it to the cook.
    expect(text).toContain("Ready to run");
    expect(text).toContain(cookDigest);
    expect(text).toContain("12.0 m");
    await beat("01b-match-workspace", await invoke("moba_status"));
  });

  it("starts the match from the cooked authored scene and draws it with real imported meshes", async () => {
    const started = await invoke("moba_start");
    // Frame the lane before capturing. The default editor camera sits nearly end-on to it, which compresses
    // twelve metres of match into a few pixels - a capture nobody can read is not evidence.
    await invoke("view_preset", { preset: "top" });
    await invoke("frame_all");
    expect(started.running).toBe(true);
    expect(started.actor_count).toBeGreaterThan(0);
    // The running match was built from the SAME cook the validator reported — not from a second definition.
    expect(started.cook_digest).toBe(cookDigest);
    // The hero is the player-owned actor; the two cores are the objectives.
    expect(started.actors.filter((a) => a.owned).length).toBe(1);

    // Every standing actor traces back to an entity the author can select in the outliner.
    const hero = started.actors.find((a) => a.owned);
    expect(authored.actors).toContain(hero.source);

    // And the cooked artifact itself is readable — the inspectable middle of the evidence chain.
    const cooked = await invoke("moba_cooked");
    expect(cooked.digest).toBe(cookDigest);
    expect(cooked.actors.length).toBe(3);
    expect(cooked.lane.centerline.length).toBe(2);
    writeFileSync(
      path.join(shots, "cooked-match.json"),
      JSON.stringify(cooked, null, 2),
      "utf8",
    );
    await beat("01-match-started", started);
  });

  it("advances the authoritative simulation and spawns waves", async () => {
    const early = await invoke("moba_step", { ticks: 6 });
    expect(early.tick).toBeGreaterThanOrEqual(6);
    await beat("02-ticked-6", early);

    const waved = await invoke("moba_step", { ticks: 30 });
    // The wave scheduler must have produced minions by now - proof MOB-2's scheduler runs live, from the
    // authored wave's own firstTick/interval/unitCount rather than from a wave written in Rust.
    expect(waved.actor_count).toBeGreaterThan(3);
    // Spawned units are runtime-only: they have no authored source, and must never claim one.
    const spawned = waved.actors.filter((a) => a.source === null || a.source === undefined);
    expect(spawned.length).toBeGreaterThan(0);
    await beat("03-waves-marching", waved);
  });

  it("accepts a lane-anchored player order and moves the hero", async () => {
    const before = await invoke("moba_status");
    const hero = before.actors.find((a) => a.owned);
    const ordered = await invoke("moba_order_move", { xMm: 8000, yMm: 0 });
    expect(ordered.last_rejection).toBe(null);
    const moved = await invoke("moba_step", { ticks: 12 });
    const after = moved.actors.find((a) => a.owned);
    expect(after.x_mm).toBeGreaterThan(hero.x_mm);
    await beat("04-hero-ordered-forward", moved);
  });

  it("refuses an off-corridor order without disturbing the match", async () => {
    const before = await invoke("moba_status");
    const refused = await invoke("moba_order_move", { xMm: 6000, yMm: 40000 });
    expect(refused.last_rejection).not.toBe(null);
    // The refusal must leave the simulation untouched.
    expect(refused.tick).toBe(before.tick);
    expect(refused.world_digest).toBe(before.world_digest);
    await beat("05-off-corridor-refused", refused);
  });

  it("shows GP-02 crowd control stopping the hero in the live viewport", async () => {
    const stunned = await invoke("moba_stun", { ticks: 40 });
    const hero = stunned.actors.find((a) => a.owned);
    expect(hero.controls).toContain("Stun");
    await beat("06-hero-stunned", stunned);

    // A stunned hero refuses orders and holds position - the whole point of GP-02.
    const held = hero.x_mm;
    const refused = await invoke("moba_order_move", { xMm: 11000, yMm: 0 });
    expect(refused.last_rejection).not.toBe(null);
    const later = await invoke("moba_step", { ticks: 8 });
    const stillHero = later.actors.find((a) => a.owned);
    expect(stillHero.x_mm).toBe(held);
    expect(stillHero.controls).toContain("Stun");
    await beat("07-stunned-holds-position", later);
  });

  it("frees the hero when the stun expires and resumes the match", async () => {
    const freed = await invoke("moba_step", { ticks: 45 });
    const hero = freed.actors.find((a) => a.owned);
    expect(hero.controls).not.toContain("Stun");
    await beat("08-stun-expired", freed);

    const advanced = await invoke("moba_step", { ticks: 60 });
    await beat("09-match-advanced", advanced);
    expect(advanced.tick).toBeGreaterThan(freed.tick);
  });

  it("restores the editor scene when the match stops, leaving the authored match intact", async () => {
    const stopped = await invoke("moba_stop");
    expect(stopped.running).toBe(false);
    await beat("10-stopped-scene-restored", stopped);

    // The match ran as a projection: the authored document is untouched, so it still validates, and cooks
    // to the SAME digest it did before the match ran. A run that had leaked into the document would move it.
    const after = await invoke("moba_validate");
    expect(after.is_match_scene).toBe(true);
    expect(after.ok).toBe(true);
    expect(after.cook_digest).toBe(cookDigest);
    expect(after.actor_count).toBe(3);

    // And it can be started again from the same authored scene — repeated play/stop cycles do not drift.
    const restarted = await invoke("moba_start");
    expect(restarted.cook_digest).toBe(cookDigest);
    expect(restarted.tick).toBe(0);
    await invoke("moba_stop");

    writeFileSync(
      path.join(shots, "evidence.json"),
      JSON.stringify(
        {
          schema: "metrocalk.moba.capture.v2",
          cookDigest,
          authored,
          beats: evidence,
        },
        null,
        2,
      ),
      "utf8",
    );
  });
});
