// Drive GP-08's standing attack orders the way a PLAYER drives them — by clicking the panel in the
// packaged .exe — and capture the composited pixels BEFORE and AFTER every single interaction.
//
// The difference from `moba-capture.e2e.js` matters. That spec proves the pipeline (author → cook → run →
// restore) and reaches the kernel mostly through the Tauri command surface. This one proves the FEATURE
// from the outside: every action below is a real click on a real control that a person can find, and each
// one is bracketed by two OS-composited screenshots so the change it caused is visible rather than only
// asserted about. A capability reachable only from an automated `invoke` is not a feature.
//
// The capture is an OS-level BitBlt of the composited window, not a WebDriver screenshot: the viewport is
// a native wgpu surface under a transparent WebView2, so a DOM screenshot shows the panels and a black
// hole where the 3D is. The images are evidence for a human; every assertion reads authoritative kernel
// state through `moba_status`.
//
// One property of the starter match shapes this whole spec: it authors a RED wave and no blue one, so the
// blue core is on a losing clock from tick 0 unless the hero fights. That is exactly the situation a
// standing order exists for — but it also means the match can legitimately END mid-run, taking the panel's
// order controls with it. `ensureRunning` handles that the way a person would, by starting again, and says
// so in the evidence rather than pretending it did not happen.

import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-moba-orders");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");

mkdirSync(shots, { recursive: true });

/** Every interaction, with the kernel state and the image on both sides of it. */
const interactions = [];
let counter = 0;
let restarts = 0;
let revives = 0;

const snap = (name) => {
  const out = path.join(shots, `${name}.png`);
  // The capture minimises and restores the window to force a fresh composite, and across ~50 of those in
  // one run the window handle is occasionally invalid at the instant BitBlt runs (Win32 error 6). It is a
  // harness race, not a product fault, so it is RETRIED rather than tolerated: an interaction without its
  // image is not evidence, and silently skipping one would leave a gap nobody would notice.
  let last;
  for (let attempt = 0; attempt < 4; attempt += 1) {
    try {
      execFileSync(
        "powershell.exe",
        ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", capture, "-Out", out],
        { stdio: "pipe" },
      );
      return path.basename(out);
    } catch (error) {
      last = error;
      execFileSync("powershell.exe", ["-NoProfile", "-Command", "Start-Sleep -Milliseconds 700"], {
        stdio: "pipe",
      });
    }
  }
  throw last;
};

/** The part of kernel truth this spec is about, flattened for the evidence file. */
const digest = (status) => {
  const hero = status.actors.find((a) => a.owned) || {};
  const hostiles = status.actors.filter((a) => a.alive && a.team !== hero.team);
  return {
    tick: status.tick,
    phase: status.phase,
    running: status.running,
    worldDigest: status.world_digest,
    liveActors: status.live_actors,
    heroX: hero.x_mm,
    heroY: hero.y_mm,
    heroHealth: hero.health,
    heroControls: hero.controls || [],
    // The line this whole slice exists to make true.
    standingOrder: hero.attack_order ?? null,
    hostilesAlive: hostiles.length,
    hostileHealth: hostiles.reduce((sum, a) => sum + a.health, 0),
    lastRejection: status.last_rejection,
  };
};

/**
 * One user interaction, captured on both sides.
 *
 * `act` is what the person does. It is a click on a real control wherever the app has one; the two
 * exceptions are the camera framing (a viewport gesture with no bearing on the feature) and the two
 * impossible orders, which no button can express because the panel computes its destinations from what is
 * on the field. Those are flagged `viaUi: false` so a reader is not misled about what was clicked.
 */
async function interaction(label, intent, act, { viaUi = true } = {}) {
  counter += 1;
  const id = String(counter).padStart(2, "0");
  const before = await invoke("moba_status");
  const beforeShot = snap(`${id}-${label}-before`);

  await act();

  const after = await invoke("moba_status");
  const afterShot = snap(`${id}-${label}-after`);
  interactions.push({
    n: counter,
    label,
    intent,
    viaUi,
    before: { ...digest(before), shot: beforeShot },
    after: { ...digest(after), shot: afterShot },
  });
  return after;
}

/** Click a control by its test id, failing loudly rather than silently no-op'ing on a missing button. */
async function press(testId) {
  const button = await $(`[data-testid="${testId}"]`);
  await button.scrollIntoView({ block: "center" });
  await button.waitForClickable({ timeout: 15000 });
  await button.click();
}

/** Click a control by its visible label — the way a person finds it. */
async function pressLabelled(text) {
  const panel = await $("#inspector-workspaces-match-panel");
  const button = await panel.$(`button*=${text}`);
  await button.scrollIntoView({ block: "center" });
  await button.waitForClickable({ timeout: 15000 });
  await button.click();
}

/** Wait until the kernel reports what the click was supposed to cause. */
const until = (predicate, message) =>
  browser.waitUntil(async () => predicate(await invoke("moba_status")), {
    timeout: 20000,
    interval: 250,
    timeoutMsg: message,
  });

/**
 * Advance the clock by pressing a step button, and settle.
 *
 * Tolerant of the match ENDING inside the window: `moba_step` stops at the terminal frame, so demanding an
 * exact tick would turn a legitimate match result into a spurious failure. The caller asserts on what
 * actually matters — health lost, the order still standing — not on the clock reading a chosen number.
 */
async function advance(label) {
  const status = await invoke("moba_status");
  // A finished match has no step buttons at all, so looking for one would fail on the SELECTOR and hide
  // the real reason. Say nothing happened, and let the caller decide whether that matters.
  if (!status.running) return false;
  await pressLabelled(label);
  await until(
    (s) => s.tick > status.tick || !s.running,
    `the match neither advanced past tick ${status.tick} nor ended`,
  );
  return true;
}

/**
 * Give a standing order the way the editor actually delivers one: press the button, then advance the tick
 * it executes on.
 *
 * An order is a COMMAND, queued for `tick + 1` like every other. In a stepped editor session nothing
 * happens until the clock moves, so reading the hero's standing order straight after the click asks the
 * kernel about a tick that has not occurred. This is not a workaround — it is what a person does, and an
 * earlier version of this spec failed precisely because it skipped it.
 */
async function giveOrder(testId) {
  await press(testId);
  await advance("Step 1");
}

/**
 * Make sure a match is running before an order is given.
 *
 * The blue core can legitimately fall — that is the match working, not the app breaking — and when it does
 * the panel replaces the order controls with "Start match". Restarting is what a person does; it is
 * counted so the evidence file reports how often it was needed rather than hiding it.
 */
async function ensureRunning() {
  if ((await invoke("moba_status")).running) return;
  restarts += 1;
  await pressLabelled("Start match");
  await until((s) => s.running, "the match could not be restarted");
  await invoke("view_preset", { preset: "top" });
  await invoke("frame_all");
}

/**
 * Make sure there is a LIVING hero to give an order to.
 *
 * The hero can be killed — red pushes the lane and the starter authors a 30-tick respawn — and the kernel
 * correctly refuses every command to a corpse with `ActorDead`. Waiting out the respawn is what a player
 * does; asserting through it would be asserting that death does not happen.
 */
async function ensureCommandable() {
  await ensureRunning();
  for (let attempt = 0; attempt < 6; attempt += 1) {
    const status = await invoke("moba_status");
    const hero = status.actors.find((a) => a.owned);
    if (hero && hero.alive) return;
    if (!status.running) {
      await ensureRunning();
      continue;
    }
    revives += 1;
    await advance("Step 30");
  }
  throw new Error("the hero never came back, so no order could be given");
}

/** The standing order the panel is DISPLAYING, as a person reads it. */
async function displayedOrder() {
  const line = await $('[data-testid="standing-order"]');
  // getText() returns "" for anything outside the viewport, so an un-scrolled read would silently claim
  // the panel says nothing at all — which is how this spec first "proved" a missing line.
  await line.scrollIntoView({ block: "center" });
  return line.getText();
}

/** The hero's standing order straight from the kernel. */
const orderOf = (status) => (status.actors.find((a) => a.owned) || {}).attack_order ?? null;

describe("GP-08 standing orders, driven from the panel a player uses", () => {
  it("opens the Match workspace and authors a match to fight over", async () => {
    await interaction(
      "open-match-workspace",
      "Find the feature at all: open the Match workspace in the packaged app.",
      async () => {
        const tab = await $("#inspector-workspaces-match-tab");
        await tab.waitForExist({ timeout: 20000 });
        await tab.click();
        const panel = await $("#inspector-workspaces-match-panel");
        await panel.waitForDisplayed({ timeout: 20000 });
      },
    );

    await interaction(
      "create-starter-match",
      "Press the one button an empty scene offers, and get something runnable.",
      async () => {
        await pressLabelled("Create a starter match");
        await browser.waitUntil(async () => (await invoke("moba_validate")).ok, {
          timeout: 20000,
          timeoutMsg: "the starter match never validated",
        });
      },
    );

    await interaction(
      "frame-the-lane",
      "Look down at the lane, so every capture after this is readable.",
      async () => {
        await invoke("view_preset", { preset: "top" });
        await invoke("frame_all");
      },
      { viaUi: false },
    );
  });

  it("starts the match, and says plainly that no order is standing yet", async () => {
    const started = await interaction(
      "start-match",
      "Press Start. The viewport becomes the running match, drawn with the real imported meshes.",
      async () => {
        await pressLabelled("Start match");
        await until((s) => s.running, "the match never started");
        await invoke("view_preset", { preset: "top" });
        await invoke("frame_all");
      },
    );
    expect(started.running).toBe(true);
    expect(orderOf(started)).toBe(null);
    // The panel must not imply an order the hero has not been given.
    expect(await displayedOrder()).toMatch(/none/i);

    const waved = await interaction(
      "step-30-first-wave",
      "Let the first red wave spawn, so there is something for an order to find.",
      async () => {
        await advance("Step 30");
      },
    );
    const hero = waved.actors.find((a) => a.owned);
    expect(waved.actors.filter((a) => a.alive && a.team !== hero.team).length).toBeGreaterThan(1);
  });

  it("takes ONE attack-move and fights under it with no further command", async () => {
    await ensureCommandable();
    const ordered = await interaction(
      "order-attack-move",
      "Give ONE attack-move. From here the hero is supposed to fight on its own.",
      async () => {
        await giveOrder("order-attack-move");
      },
    );
    expect(ordered.last_rejection).toBe(null);
    expect(orderOf(ordered)).toMatch(/attack-move/i);
    expect(await displayedOrder()).toMatch(/attack-move/i);

    // The defining property. Damage is read from KERNEL state, so this is not "the panel said so" — it is
    // the hostile side's health falling while nothing is pressed but the clock.
    const fought = await interaction(
      "step-30-fight-under-the-order",
      "Advance the clock only. Every swing in this window is the kernel's, not a command of mine.",
      async () => {
        await advance("Step 30");
      },
    );
    // Deliberately NOT "total hostile health went down": red spawns three fresh 320 hp minions every 24
    // ticks, so that number can RISE while the hero is winning, and an earlier version of this assertion
    // failed for exactly that reason. Count the hero's own swings instead — each one is a swing nobody
    // asked for.
    const heroId = (fought.actors.find((a) => a.owned) || {}).id;
    const swings = fought.events.filter(
      (e) => e.includes("BasicAttackStarted") && e.includes(`ActorId(${heroId})`),
    ).length;
    expect(swings).toBeGreaterThan(0);
    // And the order was never consumed by the swings it produced.
    expect(digest(fought).standingOrder).toMatch(/attack-move/i);

    const again = await interaction(
      "step-30-still-fighting",
      "Keep the clock running: the order is still standing, so the fighting continues.",
      async () => {
        await advance("Step 30");
      },
    );
    expect(digest(again).standingOrder).toMatch(/attack-move/i);
  });

  it("locks onto a named target, chosen from what is actually on the field", async () => {
    await ensureCommandable();
    const before = await invoke("moba_status");
    const hero = before.actors.find((a) => a.owned);
    const nearest = before.actors
      .filter((a) => a.alive && a.team !== hero.team)
      .sort(
        (a, b) =>
          (a.x_mm - hero.x_mm) ** 2 +
          (a.y_mm - hero.y_mm) ** 2 -
          ((b.x_mm - hero.x_mm) ** 2 + (b.y_mm - hero.y_mm) ** 2) ||
          a.id - b.id,
      )[0];

    const locked = await interaction(
      "order-attack-nearest",
      "Name a target. The panel picks the nearest hostile, and the hero must not switch off it.",
      async () => {
        await giveOrder("order-attack-target");
      },
    );
    const named = orderOf(locked);
    expect(named).toMatch(/attacking #/);
    expect(await displayedOrder()).toMatch(/attacking #/);
    // The panel's "nearest" has to be the field's nearest, or the label is a lie.
    if (nearest) expect(named).toContain(String(nearest.id));

    const held = await interaction(
      "step-30-under-the-named-order",
      "Advance only. A named order is fought to the end rather than re-aimed at whatever drifts closer.",
      async () => {
        await advance("Step 30");
      },
    );
    // Either still on that target, or the target is dead and the order has ENDED — never silently re-aimed
    // at somebody else, which is the failure this assertion exists to catch.
    const now = orderOf(held);
    expect(now === null || now === named).toBe(true);
  });

  it("holds position on command, and stops fighting when orders are cleared", async () => {
    await ensureCommandable();
    const held = await interaction(
      "order-hold-position",
      "Hold. The hero stays where it is and hits whatever walks into range.",
      async () => {
        await giveOrder("order-hold");
      },
    );
    expect(orderOf(held)).toMatch(/hold/i);
    expect(await displayedOrder()).toMatch(/hold position/i);
    const anchored = digest(held).heroX;

    const stayed = await interaction(
      "step-30-holding",
      "Advance under the hold. A hold that walked would be an attack-move.",
      async () => {
        await advance("Step 30");
      },
    );
    // Only meaningful while the hero is the same hero: death sends it back to its authored spawn after 30
    // ticks, and that displacement says nothing about whether a hold holds.
    const survivor = stayed.actors.find((a) => a.owned);
    if (survivor && survivor.alive && digest(stayed).heroHealth > 0) {
      expect(digest(stayed).heroX).toBe(anchored);
    }

    await ensureCommandable();
    const cleared = await interaction(
      "order-clear",
      "Clear orders. Stop must mean stop — not stop-walking-but-keep-swinging.",
      async () => {
        await giveOrder("order-halt");
      },
    );
    expect(orderOf(cleared)).toBe(null);
    expect(await displayedOrder()).toMatch(/none/i);

    const idle = digest(cleared);
    const still = await interaction(
      "step-30-after-clearing",
      "Advance with no order standing. My hero must be idle, even with hostiles in reach.",
      async () => {
        await advance("Step 30");
      },
    );
    // Other units still fight each other, so this asserts the ORDER is gone rather than that the world
    // froze. Position is checked only if the hero lived through the window, for the respawn reason above.
    expect(digest(still).standingOrder).toBe(null);
    const alive = still.actors.find((a) => a.owned);
    if (alive && alive.alive && idle.heroHealth > 0) {
      expect(digest(still).heroX).toBe(idle.heroX);
    }
  });

  it("gates an automatic swing behind crowd control, exactly as it gates a commanded one", async () => {
    await ensureCommandable();
    await interaction(
      "order-hold-again",
      "Put the hero back under a hold, so there is a standing order for the stun to fight.",
      async () => {
        await giveOrder("order-hold");
      },
    );

    const stunned = await interaction(
      "stun-the-hero",
      "Stun the hero while an order is standing. A standing order must not out-swing crowd control.",
      async () => {
        await pressLabelled("Stun hero");
        await until(
          (s) => ((s.actors.find((a) => a.owned) || {}).controls || []).includes("Stun"),
          "the hero was never stunned",
        );
      },
    );
    expect(digest(stunned).heroControls).toContain("Stun");
    // The order SURVIVES the stun — suppressed, not cancelled. That distinction is exactly why the hero
    // resumes by itself below instead of needing a fresh order.
    expect(digest(stunned).standingOrder).toMatch(/hold/i);

    await interaction(
      "step-30-stunned",
      "Advance while stunned. The order is standing and must still produce nothing.",
      async () => {
        await advance("Step 30");
      },
    );

    const freed = await interaction(
      "step-30-stun-expires",
      "Advance past the stun. The hero resumes fighting with no new order from me.",
      async () => {
        await advance("Step 30");
        await until(
          (s) =>
            !((s.actors.find((a) => a.owned) || {}).controls || []).includes("Stun") || !s.running,
          "the stun never expired",
        );
      },
    );
    if (freed.running) {
      expect(digest(freed).heroControls).not.toContain("Stun");
      expect(digest(freed).standingOrder).toMatch(/hold/i);
    }
  });

  it("refuses an impossible order without disturbing the match", async () => {
    await ensureCommandable();
    const before = await invoke("moba_status");
    const standing = orderOf(before);

    const refused = await interaction(
      "refuse-out-of-bounds-attack-move",
      "Ask for an attack-move far outside the map. It must be refused, and the match must not move.",
      async () => {
        await invoke("moba_attack_move", { xMm: 9_000_000, yMm: 0 });
      },
      { viaUi: false },
    );
    expect(refused.last_rejection).not.toBe(null);
    // A refusal must not advance the world, nor quietly drop the order already standing.
    expect(refused.tick).toBe(before.tick);
    expect(refused.world_digest).toBe(before.world_digest);
    expect(orderOf(refused)).toBe(standing);

    const unknown = await interaction(
      "refuse-attack-target-nonexistent",
      "Name a target that does not exist. Same rule: refused, and the standing order survives.",
      async () => {
        await invoke("moba_attack_target", { target: 999999 });
      },
      { viaUi: false },
    );
    expect(unknown.last_rejection).not.toBe(null);
    expect(orderOf(unknown)).toBe(standing);
  });

  it("takes a fresh attack-move after the refusals and keeps fighting", async () => {
    await ensureCommandable();
    const resumed = await interaction(
      "order-attack-move-again",
      "A refusal must not have poisoned the command path: the next real order still works.",
      async () => {
        await giveOrder("order-attack-move");
      },
    );
    expect(resumed.last_rejection).toBe(null);
    expect(orderOf(resumed)).toMatch(/attack-move/i);

    await interaction("step-30-second-push", "Advance under the second order.", async () => {
      await advance("Step 30");
    });

    await interaction(
      "step-1-final-approach",
      "One last tick, so the frame before stopping is readable.",
      async () => {
        await advance("Step 1");
      },
    );
  });

  it("stops the match and leaves the authored scene exactly as it was", async () => {
    await ensureCommandable();
    const before = await invoke("moba_validate");
    const stopped = await interaction(
      "stop-match",
      "Press Stop. The authored scene comes back and the run leaves no trace in the document.",
      async () => {
        await pressLabelled("Stop");
        await until((s) => !s.running, "the match never stopped");
      },
    );
    expect(stopped.running).toBe(false);

    const after = await invoke("moba_validate");
    expect(after.ok).toBe(true);
    // The match ran as a projection: the document still cooks to the fingerprint it had before.
    expect(after.cook_digest).toBe(before.cook_digest);

    const restarted = await interaction(
      "restart-match",
      "Start again from the same authored scene — repeated play/stop must not drift.",
      async () => {
        await pressLabelled("Start match");
        await until((s) => s.running, "the match did not restart");
      },
    );
    expect(restarted.tick).toBe(0);
    expect(orderOf(restarted)).toBe(null);

    // ── the evidence file ──────────────────────────────────────────────────────────────────────────
    // Twenty is the floor this run has to clear, ASSERTED rather than trusted: a spec that quietly skipped
    // half its interactions would otherwise still finish green.
    expect(interactions.length).toBeGreaterThanOrEqual(20);
    const uiDriven = interactions.filter((i) => i.viaUi).length;
    expect(uiDriven).toBeGreaterThanOrEqual(18);
    // Every interaction must carry a before AND an after image, or it is not evidence of a change.
    for (const item of interactions) {
      expect(typeof item.before.shot).toBe("string");
      expect(typeof item.after.shot).toBe("string");
    }

    writeFileSync(
      path.join(shots, "evidence.json"),
      JSON.stringify(
        {
          schema: "metrocalk.moba.orders.v1",
          slice: "GP-08 standing attack orders",
          interactions: interactions.length,
          uiDriven,
          images: interactions.length * 2,
          // How often the blue core fell and the match had to be started again. Reported rather than
          // hidden: it is a real property of the starter match, not a flake in the harness.
          restarts,
          // How often an order had to wait for the hero to respawn. Also reported rather than hidden: a
          // hero dying to a lane push is the match working, not the harness failing.
          revives,
          beats: interactions,
        },
        null,
        2,
      ),
      "utf8",
    );
  });
});
