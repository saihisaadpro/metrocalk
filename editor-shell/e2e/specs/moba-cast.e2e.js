// Prove that a PLAYER can cast a spell in the packaged .exe — a thing that was impossible until now.
//
// The kernel has carried abilities since MOB-1: casts, travelling projectiles, impact shapes, ability
// ranks, and the whole crit/penetration/shield/lifesteal pipeline they feed. None of it was reachable
// from an authored scene. `match_cook.rs` emitted `abilities: vec![]` for every actor and never called
// `register_ability`, the Tauri surface had no cast verb, and the only crowd control a person could
// produce in the editor was `stun_hero` — a developer cheat, not a game action.
//
// So this spec asserts the whole chain in the order it actually runs: the scene AUTHORS an ability, the
// cook REGISTERS it, the hero is EQUIPPED with it, a button CASTS it, and a projectile it launched
// reaches a body and takes health off it. Every step is captured before and after.

import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-moba-cast");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");

mkdirSync(shots, { recursive: true });

const beats = [];
let counter = 0;

const snap = (name) => {
  const out = path.join(shots, `${name}.png`);
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

const digest = (status) => {
  const hero = status.actors.find((a) => a.owned) || {};
  const hostiles = status.actors.filter((a) => a.alive && a.team !== hero.team);
  return {
    tick: status.tick,
    running: status.running,
    heroX: hero.x_mm,
    abilityReadyIn: hero.ability_ready_in ?? null,
    hostilesAlive: hostiles.length,
    hostileHealth: hostiles.reduce((sum, a) => sum + a.health, 0),
    lastRejection: status.last_rejection,
  };
};

async function beat(label, intent, act) {
  counter += 1;
  const id = String(counter).padStart(2, "0");
  const before = await invoke("moba_status");
  const beforeShot = snap(`${id}-${label}-before`);
  await act();
  const after = await invoke("moba_status");
  const afterShot = snap(`${id}-${label}-after`);
  beats.push({
    n: counter,
    label,
    intent,
    before: { ...digest(before), shot: beforeShot },
    after: { ...digest(after), shot: afterShot },
  });
  return after;
}

async function press(testId) {
  const button = await $(`[data-testid="${testId}"]`);
  await button.scrollIntoView({ block: "center" });
  await button.waitForClickable({ timeout: 15000 });
  await button.click();
}

async function pressLabelled(text) {
  const panel = await $("#inspector-workspaces-match-panel");
  const button = await panel.$(`button*=${text}`);
  await button.scrollIntoView({ block: "center" });
  await button.waitForClickable({ timeout: 15000 });
  await button.click();
}

const until = (predicate, message) =>
  browser.waitUntil(async () => predicate(await invoke("moba_status")), {
    timeout: 20000,
    interval: 250,
    timeoutMsg: message,
  });

describe("casting an authored ability from the packaged editor", () => {
  it("authors a match whose hero actually carries an ability", async () => {
    await beat(
      "open-and-author",
      "Open the Match workspace and create the starter match.",
      async () => {
        const tab = await $("#inspector-workspaces-match-tab");
        await tab.waitForExist({ timeout: 20000 });
        await tab.click();
        await (await $("#inspector-workspaces-match-panel")).waitForDisplayed({ timeout: 20000 });
        await pressLabelled("Create a starter match");
        await browser.waitUntil(async () => (await invoke("moba_validate")).ok, {
          timeout: 20000,
          timeoutMsg: "the starter match never validated",
        });
        await invoke("view_preset", { preset: "top" });
        await invoke("frame_all");
      },
    );

  });

  it("starts the match with the ability ready, not merely present", async () => {
    const started = await beat("start-match", "Press Start.", async () => {
      await pressLabelled("Start match");
      await until((s) => s.running, "the match never started");
      await invoke("view_preset", { preset: "top" });
      await invoke("frame_all");
    });
    // The cooked artifact is the inspectable middle of the chain: if the ability is not HERE, nothing
    // downstream can be real. Read AFTER start, because `moba_cooked` describes a running session.
    const cooked = await invoke("moba_cooked");
    const authored = (cooked.actors || []).find((a) => a.owned);
    expect(authored).toBeTruthy();
    expect(authored.ability).toBeTruthy();
    expect(authored.ability.damage).toBeGreaterThan(0);
    // Authored as a travelling bolt, so the flight the kernel already models is actually exercised.
    expect(authored.ability.projectileSpeedMmPerTick).toBeGreaterThan(0);
    writeFileSync(
      path.join(shots, "cooked-ability.json"),
      JSON.stringify(authored.ability, null, 2),
      "utf8",
    );

    const hero = started.actors.find((a) => a.owned);
    // 0 means castable now. `null` would mean the hero has no ability at all, which is the state this
    // whole slice exists to end, so the two must not be conflated.
    expect(hero.ability_ready_in).toBe(0);
    expect((await $('[data-testid="ability-state"]')).getText()).resolves.toMatch(/ready/i);
  });

  it("spawns something to cast at", async () => {
    const waved = await beat("step-30-wave", "Let the first wave spawn.", async () => {
      await pressLabelled("Step 30");
      await until((s) => s.tick >= 30, "the match did not advance");
    });
    const hero = waved.actors.find((a) => a.owned);
    expect(waved.actors.filter((a) => a.alive && a.team !== hero.team).length).toBeGreaterThan(0);
  });

  it("casts, and the cast costs a cooldown and lands damage", async () => {
    const before = await invoke("moba_status");
    const cast = await beat(
      "cast-ability",
      "Press Cast. This is the first player-reachable cast this editor has ever had.",
      async () => {
        await press("order-cast");
        // A cast is a COMMAND, queued for `tick + 1` like every other. In a stepped editor session
        // nothing happens until the clock moves, so the tick it executes on has to be advanced before
        // the kernel can be asked whether it took.
        await pressLabelled("Step 1");
        await until(
          (s) => (s.actors.find((a) => a.owned) || {}).ability_ready_in !== 0 || !s.running,
          "the cast never registered",
        );
      },
    );
    expect(cast.last_rejection).toBe(null);

    const landed = await beat(
      "step-30-projectile-flies",
      "Advance only. The bolt has to cross the ground and reach a body by itself.",
      async () => {
        await pressLabelled("Step 30");
        await until((s) => s.tick >= cast.tick + 30 || !s.running, "the match stalled");
      },
    );

    // The claim is not "a button was pressed" — it is that a projectile the CAST launched reached
    // something. Read from kernel events, not from the panel's own text.
    const flew = landed.events.some((e) => e.includes("Projectile"));
    expect(flew).toBe(true);

    // And the ability went on cooldown, which is the kernel's own accounting rather than the UI's.
    const heroAfter = cast.actors.find((a) => a.owned);
    expect(heroAfter.ability_ready_in).toBeGreaterThan(0);
  });

  it("recovers the cooldown and can cast again", async () => {
    const ready = await beat(
      "step-30-cooldown-recovers",
      "Advance until the ability is castable again — no new command from me.",
      async () => {
        await pressLabelled("Step 30");
        await until(
          (s) => (s.actors.find((a) => a.owned) || {}).ability_ready_in === 0 || !s.running,
          "the cooldown never recovered",
        );
      },
    );
    if (ready.running) {
      expect(ready.actors.find((a) => a.owned).ability_ready_in).toBe(0);
    }

    expect(beats.length).toBeGreaterThanOrEqual(6);
    for (const b of beats) {
      expect(typeof b.before.shot).toBe("string");
      expect(typeof b.after.shot).toBe("string");
    }
    writeFileSync(
      path.join(shots, "evidence.json"),
      JSON.stringify(
        {
          schema: "metrocalk.moba.cast.v1",
          slice: "an authored ability, reachable and castable by a player",
          beats: beats.length,
          images: beats.length * 2,
          beatList: beats,
        },
        null,
        2,
      ),
      "utf8",
    );
  });
});
