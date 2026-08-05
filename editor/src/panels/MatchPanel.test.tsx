import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { projectionStore } from "../store/projection";
import type { CookDiagnostic, MatchStatus, MatchValidation } from "../transport/protocol";
import { fakeClient } from "../transport/test-client";
import { MatchPanel } from "./MatchPanel";

const valid: MatchValidation = {
  ok: true,
  is_match_scene: true,
  diagnostics: [],
  cook_digest: "a1b2c3d4e5f60718",
  actor_count: 3,
  wave_count: 1,
  lane_length_m: 12,
};

const brokenDiagnostic: CookDiagnostic = {
  severity: "error",
  code: "out-of-range",
  message: "`health` is 0; health must be between 1 and 4294967295.",
  entity: "1_7",
  component: "MatchActor",
  field: "health",
};

const running: MatchStatus = {
  running: true,
  tick: 48,
  phase: "Active",
  world_digest: "1111222233334444",
  lane_digest: "5555666677778888",
  cook_digest: "a1b2c3d4e5f60718",
  cook_schema_version: 1,
  actor_count: 6,
  live_actors: 6,
  actors: [
    {
      id: 3,
      team: 0,
      kind: "Hero",
      x_mm: 4620,
      y_mm: 0,
      health: 1400,
      max_health: 1400,
      alive: true,
      owned: true,
      controls: ["Stun"],
      speed: 130,
      ability_ready_in: 0,
      attack_order: null,
      source: "1_7",
    },
    // A hostile tower and a hostile minion, so the order buttons are exercised in their ENABLED state.
    // A fixture with only the hero would have let "Attack nearest enemy" ship permanently disabled.
    {
      id: 9,
      team: 1,
      kind: "Minion",
      x_mm: 6000,
      y_mm: 0,
      health: 200,
      max_health: 200,
      alive: true,
      owned: false,
      controls: [],
      speed: 60,
      ability_ready_in: null,
      attack_order: null,
      source: null,
    },
    {
      id: 2,
      team: 1,
      kind: "Structure",
      x_mm: 12000,
      y_mm: 0,
      health: 2000,
      max_health: 2000,
      alive: true,
      owned: false,
      controls: [],
      speed: 0,
      ability_ready_in: null,
      attack_order: null,
      source: "1_9",
    },
  ],
  events: [],
  last_rejection: null,
};

afterEach(() => {
  cleanup();
  projectionStore.getState().reset();
});

describe("MatchPanel", () => {
  it("offers to create a match when the scene has none, instead of showing dead controls", async () => {
    const authorStarter = vi.fn(() =>
      Promise.resolve({ settings: "s", lane: "l", waypoints: ["w0", "w1"], actors: ["a0", "a1", "a2"], waves: ["v0"] }),
    );
    // Not a match scene at first; after authoring, it is.
    const validate = vi
      .fn<() => Promise<MatchValidation>>()
      .mockResolvedValueOnce({ ...valid, ok: false, is_match_scene: false, actor_count: 0, wave_count: 0, lane_length_m: 0, cook_digest: null })
      .mockResolvedValue(valid);
    const client = fakeClient({ matchValidate: validate, matchAuthorStarter: authorStarter });

    render(<MatchPanel client={client} />);
    const create = await screen.findByRole("button", { name: /create a starter match/i });
    // The empty state explains what a match is rather than showing a disabled Start button.
    expect(screen.queryByRole("button", { name: /start match/i })).toBeNull();

    fireEvent.click(create);
    await waitFor(() => expect(authorStarter).toHaveBeenCalled());
    expect(await screen.findByRole("button", { name: /start match/i })).toBeTruthy();
  });

  it("summarises the authored match, not just whether it is valid", async () => {
    render(<MatchPanel client={fakeClient({ matchValidate: () => Promise.resolve(valid) })} />);
    // Actors, waves and lane length come from the cook — the author can sanity-check them at a glance.
    expect(await screen.findByText("3")).toBeTruthy();
    expect(screen.getByText("12.0 m")).toBeTruthy();
    expect(screen.getByText("Yes")).toBeTruthy();
    // The definitions fingerprint is shown, so two runs can be compared.
    expect(screen.getByText("a1b2c3d4e5f60718")).toBeTruthy();
  });

  it("blocks Start and names the object at fault when the scene will not cook", async () => {
    const client = fakeClient({
      matchValidate: () => Promise.resolve({ ...valid, ok: false, diagnostics: [brokenDiagnostic] }),
    });
    render(<MatchPanel client={client} />);

    const start = await screen.findByRole("button", { name: /start match/i });
    expect((start as HTMLButtonElement).disabled).toBe(true);
    // The message is the actionable one, not an enum name.
    expect(screen.getByText(/health.*is 0/i)).toBeTruthy();
    expect(screen.getByText("MatchActor.health")).toBeTruthy();
  });

  it("selects the offending object when a diagnostic is acted on", async () => {
    const client = fakeClient({
      matchValidate: () => Promise.resolve({ ...valid, ok: false, diagnostics: [brokenDiagnostic] }),
    });
    render(<MatchPanel client={client} />);
    fireEvent.click(await screen.findByRole("button", { name: /show me/i }));
    // A refusal that cannot be located is not actionable; this is what makes it one.
    expect(projectionStore.getState().selectedId).toBe("1_7");
  });

  it("reports a start refusal from the shell rather than failing silently", async () => {
    const client = fakeClient({
      matchValidate: () => Promise.resolve(valid),
      matchStart: () =>
        Promise.reject({ message: "This scene cannot run a match yet — one problem needs fixing.", diagnostics: [brokenDiagnostic] }),
    });
    render(<MatchPanel client={client} />);
    fireEvent.click(await screen.findByRole("button", { name: /start match/i }));
    expect(await screen.findByText(/health.*is 0/i)).toBeTruthy();
  });

  it("shows an active status effect by name, not by colour alone", async () => {
    const client = fakeClient({
      matchValidate: () => Promise.resolve(valid),
      matchStart: () => Promise.resolve(running),
    });
    render(<MatchPanel client={client} />);
    fireEvent.click(await screen.findByRole("button", { name: /start match/i }));

    // The viewport tints a stunned hero yellow. Colour cannot be the only channel, so the control is
    // spelled out here — this is the assertion that keeps that true.
    expect(await screen.findByText(/affected by Stun/)).toBeTruthy();
    expect(screen.getByText("48")).toBeTruthy();
    expect(screen.getByText("4.62 m")).toBeTruthy();
  });

  it("swaps to running transport controls and restores the scene on stop", async () => {
    const stop = vi.fn(() => Promise.resolve({ ...running, running: false, tick: 0, actors: [] }));
    const client = fakeClient({
      matchValidate: () => Promise.resolve(valid),
      matchStart: () => Promise.resolve(running),
      matchStop: stop,
    });
    render(<MatchPanel client={client} />);
    fireEvent.click(await screen.findByRole("button", { name: /start match/i }));

    // While running, the panel says plainly that the viewport is not showing the user's scene.
    expect(await screen.findByText(/your scene is untouched and comes back when you stop/i)).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: /^stop$/i }));
    await waitFor(() => expect(stop).toHaveBeenCalled());
    expect(await screen.findByRole("button", { name: /start match/i })).toBeTruthy();
  });

  // ── GP-08: standing orders ───────────────────────────────────────────────────────────────────────
  it("attack-move aims at the enemy's furthest structure, not at a coordinate the user has to type", async () => {
    const attackMove = vi.fn(() => Promise.resolve(running));
    const client = fakeClient({
      matchValidate: () => Promise.resolve(valid),
      matchStart: () => Promise.resolve(running),
      matchAttackMove: attackMove,
    });
    render(<MatchPanel client={client} />);
    fireEvent.click(await screen.findByRole("button", { name: /start match/i }));
    fireEvent.click(await screen.findByTestId("order-attack-move"));

    // The hostile STRUCTURE at 12 m, not the nearer minion at 6 m: a lane exists to reach the tower.
    await waitFor(() => expect(attackMove).toHaveBeenCalledWith(12000, 0));
  });

  it("locks onto the NEAREST hostile, so the button means what its label says", async () => {
    const attackTarget = vi.fn(() => Promise.resolve(running));
    const client = fakeClient({
      matchValidate: () => Promise.resolve(valid),
      matchStart: () => Promise.resolve(running),
      matchAttackTarget: attackTarget,
    });
    render(<MatchPanel client={client} />);
    fireEvent.click(await screen.findByRole("button", { name: /start match/i }));
    fireEvent.click(await screen.findByTestId("order-attack-target"));

    // The minion at 6 m, not the tower at 12 m.
    await waitFor(() => expect(attackTarget).toHaveBeenCalledWith(9));
  });

  it("shows the standing order in the kernel's own words, and says plainly when there is none", async () => {
    const held: MatchStatus = {
      ...running,
      actors: running.actors.map((actor) =>
        actor.owned ? { ...actor, attack_order: "hold position" } : actor,
      ),
    };
    const client = fakeClient({
      matchValidate: () => Promise.resolve(valid),
      matchStart: () => Promise.resolve(running),
      matchHold: () => Promise.resolve(held),
    });
    render(<MatchPanel client={client} />);
    fireEvent.click(await screen.findByRole("button", { name: /start match/i }));

    // Before: the panel must not imply an order the hero does not have.
    expect((await screen.findByTestId("standing-order")).textContent).toMatch(/none/i);
    fireEvent.click(screen.getByTestId("order-hold"));
    await waitFor(() =>
      expect(screen.getByTestId("standing-order").textContent).toMatch(/hold position/i),
    );
  });

  it("casts the hero's authored ability at the nearest hostile", async () => {
    // The gap this closes: until now there was no cast button at all, and the only crowd control in the
    // build was a developer cheat. The kernel has carried casts since MOB-1.
    const cast = vi.fn(() => Promise.resolve(running));
    const client = fakeClient({
      matchValidate: () => Promise.resolve(valid),
      matchStart: () => Promise.resolve(running),
      matchCast: cast,
    });
    render(<MatchPanel client={client} />);
    fireEvent.click(await screen.findByRole("button", { name: /start match/i }));
    fireEvent.click(await screen.findByTestId("order-cast"));
    await waitFor(() => expect(cast).toHaveBeenCalledWith(9));
  });

  it("distinguishes a cooling-down ability from a hero that has none at all", async () => {
    const cooling: MatchStatus = {
      ...running,
      actors: running.actors.map((a) => (a.owned ? { ...a, ability_ready_in: 17 } : a)),
    };
    const none: MatchStatus = {
      ...running,
      actors: running.actors.map((a) => (a.owned ? { ...a, ability_ready_in: null } : a)),
    };

    const cool = fakeClient({
      matchValidate: () => Promise.resolve(valid),
      matchStart: () => Promise.resolve(cooling),
    });
    render(<MatchPanel client={cool} />);
    fireEvent.click(await screen.findByRole("button", { name: /start match/i }));
    expect((await screen.findByTestId("ability-state")).textContent).toMatch(/cooling down: 17/i);
    expect(((await screen.findByTestId("order-cast")) as HTMLButtonElement).disabled).toBe(true);
    cleanup();

    // The two states must not read the same. A hero with no ability is an AUTHORING problem, and telling
    // the author "cooling down" would send them to wait for something that is never coming.
    const bare = fakeClient({
      matchValidate: () => Promise.resolve(valid),
      matchStart: () => Promise.resolve(none),
    });
    render(<MatchPanel client={bare} />);
    fireEvent.click(await screen.findByRole("button", { name: /start match/i }));
    expect((await screen.findByTestId("ability-state")).textContent).toMatch(/no ability authored/i);
    expect(((await screen.findByTestId("order-cast")) as HTMLButtonElement).title).toMatch(
      /no ability authored/i,
    );
  });

  it("disables attack-target when nothing hostile is left, rather than failing on press", async () => {
    const lonely: MatchStatus = { ...running, actors: running.actors.filter((a) => a.owned) };
    const client = fakeClient({
      matchValidate: () => Promise.resolve(valid),
      matchStart: () => Promise.resolve(lonely),
    });
    render(<MatchPanel client={client} />);
    fireEvent.click(await screen.findByRole("button", { name: /start match/i }));

    const button = (await screen.findByTestId("order-attack-target")) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
    // ...and the reason is readable, not just a greyed-out control.
    expect(button.title).toMatch(/nothing hostile/i);
  });
});
