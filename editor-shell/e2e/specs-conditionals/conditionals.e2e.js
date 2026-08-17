// CONDITIONALS on the PACKAGED .exe — "The Locked Vault", the payoff demo the research designed.
//
// BEFORE: a rule either fired or it didn't. A Vault worth 10 points could be walked into on second one,
// and the game was over before it started. There was no way for a user to say "only when…", and when a
// rule silently did not fire, nothing in the system could say why.
// AFTER: select the Vault → Behaviour → Only if… → "The Score is at least 3". The rule now reads back as
// one sentence. Press Play and walk in early: nothing happens AND the panel says
// "⛔ Blocked just now — the Score is 0, needs at least 3". Collect the coins, walk in again: it opens.
// Throughout, the document still holds exactly TWO rules — the per-object condition rides the shared
// rule via the Play-start expansion, so no rule copies accumulate.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, rmSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-conditionals");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
const captureFg = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
mkdirSync(shots, { recursive: true });

let shotIndex = 0;

async function shot(label) {
  await browser.pause(600);
  const out = path.join(shots, `${String(shotIndex).padStart(2, "0")}_${label}.png`);
  shotIndex += 1;
  const good = () => existsSync(out) && statSync(out).size > 20_000;
  const attempt = (script, args) => {
    try {
      execFileSync("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, ...args], { stdio: "pipe" });
    } catch { /* fall through */ }
    if (!good() && existsSync(out)) rmSync(out);
    return good();
  };
  let ok = false;
  for (let round = 0; round < 4 && !ok; round += 1) {
    if (round > 0) await browser.pause(1200);
    ok =
      attempt(capture, ["-Out", out]) ||
      attempt(captureFg, ["-ProcName", "metrocalk-editor-shell", "-Out", out]);
  }
  if (!ok) {
    console.log(`[cond] CAPTURE UNAVAILABLE for ${label} — the desktop refused both paths`);
    return null;
  }
  console.log(`[cond] captured ${path.basename(out)} (${statSync(out).size} bytes)`);
  return out;
}

const click = (selector) =>
  browser.execute((sel) => {
    const el = document.querySelector(sel);
    if (!el) return false;
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return true;
  }, selector);

const setValue = (testid, value) =>
  browser.execute(
    (tid, v) => {
      const el = document.querySelector(`[data-testid="${tid}"]`);
      if (!el) return false;
      const proto = el.tagName === "SELECT" ? window.HTMLSelectElement.prototype : window.HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto, "value").set.call(el, String(v));
      el.dispatchEvent(new Event("change", { bubbles: true }));
      el.dispatchEvent(new Event("input", { bubbles: true }));
      el.dispatchEvent(new FocusEvent("focusout", { bubbles: true }));
      return true;
    },
    testid,
    value,
  );

const selectRow = (id) =>
  browser.execute((key) => {
    const row = document.querySelector(`[data-testid="hrow"][data-id="${key}"]`);
    if (!row) return false;
    row.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return true;
  }, id);

const pos = (id) => invoke("body_sim_position", { id });

async function pressPlay() {
  await click('[data-testid="play"]');
  await browser.waitUntil(
    async () => browser.execute(() => !!document.querySelector('[data-testid="stop"]')),
    { timeout: 10000, timeoutMsg: "Play never engaged" },
  );
}
async function pressStop() {
  await click('[data-testid="stop"]');
  await browser.pause(800);
}
const key = (type, k) =>
  browser.execute((t, kk) => { window.dispatchEvent(new KeyboardEvent(t, { key: kk, bubbles: true })); }, type, k);

/** Drive the hero into something, then let go. */
async function walk(direction, ms) {
  await key("keydown", direction);
  await browser.pause(ms);
  await key("keyup", direction);
  await browser.pause(400);
}

describe("Conditionals — the Key and the Door", () => {
  let hero;
  let door;
  let keyItem;

  before(async () => {
    await browser.waitUntil(
      async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)),
      { timeout: 30000 },
    );
    await pressStop();
    await invoke("new_project");
    await browser.pause(700);
    await browser.execute(() => {
      document.querySelector('[data-testid="onboardSkip"]')?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
  });

  it("builds the set: a hero, a Key to the right, a Door to the left worth 10", async () => {
    hero = (await invoke("shape_spawn", { kind: "capsule", pos: [0, 0, 0] })).created;
    await invoke("role_assign", { id: hero, role: "player" });
    keyItem = (await invoke("shape_spawn", { kind: "torus", pos: [4.5, 0, 0] })).created;
    door = (await invoke("shape_spawn", { kind: "box", pos: [-4.5, 0, 0] })).created;
    for (const c of [keyItem, door]) {
      const r = await invoke("role_assign", { id: c, role: "collectible" });
      expect(r.reason).toBe(null);
    }
    // The door is worth 10 — an ordinary Inspector field edit. Before this round that number was inert.
    await invoke("submit_edit", {
      tx: {
        clientOpId: "door-points",
        label: "door points",
        patches: [{ op: "replace", path: `/entities/${door}/GameRole/points`, value: 10 }],
        intent: { kind: "setField", id: door, component: "GameRole", field: "points", value: 10 },
      },
    });
    await invoke("frame_all");
    expect(hero && door && keyItem).toBeTruthy();
  });

  it("gates the Door behind the Key — one condition, read back as a sentence", async () => {
    await click('[data-testid="engine-scene"]');
    await browser.pause(300);
    expect(await selectRow(door)).toBe(true);
    await (await $('[data-testid="onlyif-block"]')).waitForExist({ timeout: 10000 });
    const before = await invoke("condition_list", { id: door });
    console.log(`[cond] BEFORE: "${before.sentence}"`);
    await shot("before_no_condition");

    // "Only if the Key is gone" — pick the card, pick the object, add.
    expect(await setValue("onlyif-pick", "other_gone")).toBe(true);
    await browser.pause(300);
    expect(await setValue("onlyif-object", keyItem)).toBe(true);
    await browser.pause(200);
    expect(await click('[data-testid="onlyif-add"]')).toBe(true);
    await browser.waitUntil(
      async () => (await invoke("condition_list", { id: door })).all.length === 1,
      { timeout: 10000, timeoutMsg: "the clause never landed" },
    );
    const after = await invoke("condition_list", { id: door });
    console.log(`[cond] AFTER:  "${after.sentence}"`);
    expect(after.sentence).toContain("only if");
    expect(after.all[0].reads).toContain("is gone");
    await shot("after_condition_added");
  });

  it("THE INVARIANT: two gated objects, still ONE rule in the document — no copies", async () => {
    const rules = await invoke("list_rules");
    console.log(`[cond] rules in the document: ${rules.map((r) => r.rule.name).join(", ")} (${rules.length})`);
    expect(rules.length).toBe(1);
  });

  it("PLAY: the Door refuses while the Key is still there — and says exactly why", async () => {
    await pressPlay();
    await browser.pause(600);
    await walk("ArrowLeft", 2200); // straight into the door, key untouched

    let status = null;
    await browser.waitUntil(
      async () => {
        status = await invoke("role_status");
        return !!status.blocked;
      },
      { timeout: 15000, interval: 300, timeoutMsg: "no near-miss was recorded" },
    );
    console.log(`[cond] BLOCKED: "${status.blocked.why}"`);
    expect(status.score).toBe(0);
    expect(status.blocked.why.toLowerCase()).toContain("waiting on");
    expect(status.blocked.why.toLowerCase()).toContain("gone");
    // textContent, not getText(): the Inspector dock scrolls, and WebDriver reports "" for an
    // element that is present but below the fold.
    const line = await browser.execute(
      () => document.querySelector('[data-testid="onlyif-blocked"]')?.textContent ?? "",
    );
    console.log(`[cond] the panel reads: ${line}`);
    expect(line).toContain("Blocked just now");
    // Show it where a PLAYER is looking: the Gameplay panel, beside the live score.
    await click('[data-testid="engine-gameplay"]');
    await browser.pause(700);
    const inPanel = await browser.execute(
      () => document.querySelector('[data-testid="roles-blocked"]')?.textContent ?? "",
    );
    console.log(`[cond] gameplay panel announces: ${inPanel}`);
    expect(inPanel).toContain("waiting on");
    await shot("play_door_blocked_with_reason");
  });

  it("take the Key, and the same touch now opens the Door — scoring its OWN 10 points", async () => {
    // Walk in bursts until the key is taken — robust to acceleration and panel focus, instead of
    // betting the whole test on one hard-coded duration.
    let status = null;
    for (let attempt = 0; attempt < 6; attempt += 1) {
      await walk("ArrowRight", 1600);
      status = await invoke("role_status");
      if (status.score >= 1) break;
    }
    expect(status.score).toBeGreaterThanOrEqual(1);
    console.log(`[cond] key taken — score ${status.score}`);
    expect(status.score).toBe(1);

    for (let attempt = 0; attempt < 8; attempt += 1) {
      await walk("ArrowLeft", 1600);
      status = await invoke("role_status");
      if (status.remaining === 0) break;
    }
    expect(status.remaining).toBe(0);
    // 1 (key) + 10 (door) — proof the per-object Points field is REAL, not a hardcoded 1.
    console.log(`[cond] DOOR OPEN — score ${status.score} (1 key + 10 door)`);
    expect(status.score).toBe(11);
    await shot("play_door_opened_scored_ten");
    await pressStop();
    await browser.pause(700);
  });

  it("Stop restores everything, and one Ctrl-Z removes the condition", async () => {
    const status = await invoke("role_status");
    expect(status.score).toBe(0);
    expect(status.blocked).toBe(null);
    const before = (await invoke("condition_list", { id: door })).all.length;
    await invoke("condition_add", {
      id: door,
      request: { kind: "score_under", number: 99, object: null, any: false },
    });
    const added = (await invoke("condition_list", { id: door })).all.length;
    expect(added).toBe(before + 1);
    await invoke("undo");
    await browser.pause(600);
    const after = (await invoke("condition_list", { id: door })).all.length;
    expect(after).toBe(before);
    console.log(`[cond] one Ctrl-Z removed the clause (${before} -> ${added} -> ${after})`);
    await shot("undone");
  });

  it("refusals are explained: an impossible clause is named, not silently authored", async () => {
    await pressStop(); // a prior failure must not leave Play running and mask the real refusal
    const rock = (await invoke("shape_spawn", { kind: "wedge", pos: [0, 0, -6] })).created;
    const reply = await invoke("condition_add", {
      id: door,
      request: { kind: "other_gone", object: rock, number: null, any: false },
    });
    console.log(`[cond] refusal: ${reply.reason}`);
    expect(reply.reason).toContain("no role yet");
    expect((await invoke("condition_list", { id: door })).all.length).toBe(1);
  });

  it("any captures taken are real pixels", async () => {
    const files = readdirSync(shots);
    if (files.length === 0) {
      console.log("[cond] no captures this run — the desktop refused OS capture");
      return;
    }
    for (const f of files) {
      expect(statSync(path.join(shots, f)).size).toBeGreaterThan(20_000);
    }
    console.log(`[cond] evidence: ${files.join(", ")}`);
  });
});
