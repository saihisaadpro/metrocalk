//! The Effects block: cards land, layers read back, warnings show, and Play locks authoring.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, fireEvent, act } from "@testing-library/react";
import { VfxSection } from "./VfxSection";
import { CinemaSection } from "./CinemaSection";
import { fakeClient } from "../transport/test-client";
import { projectionStore } from "../store/projection";
import { playStore } from "../store/play";
import type { ShotRow } from "../transport/protocol";

function selectSomething() {
  act(() => projectionStore.getState().select("e1"));
}

// The play store is module-global, so a test that engages Play would otherwise leave every later
// test asserting against a locked panel. Reset it explicitly rather than relying on ordering.
beforeEach(() => {
  act(() => playStore.getState().refresh({ playing: false, paused: false }));
});

describe("VfxSection", () => {
  it("says what to do when nothing is selected, rather than showing dead buttons", () => {
    act(() => projectionStore.getState().select(null));
    render(<VfxSection client={fakeClient()} />);
    expect(screen.getByTestId("vfx-empty").textContent).toMatch(/select an object/i);
  });

  it("offers the catalogue and lands one effect in a single click", async () => {
    const client = fakeClient();
    selectSomething();
    render(<VfxSection client={client} />);
    const fire = await screen.findByTestId("fx-fire");
    // The card spells out what it costs BEFORE the click (the legible-cost clause).
    expect(fire.getAttribute("title")).toMatch(/adds:/i);
    fireEvent.click(fire);
    await waitFor(() => expect(client.vfxAdd).toHaveBeenCalled());
    // and the layer reads back as a sentence, not as a row of numbers
    await waitFor(() => expect(screen.getByTestId("vfx-layers").textContent).toMatch(/Fire/));
  });

  it("sends the chosen MOMENT with the card — an effect attached to gameplay, not decoration", async () => {
    const client = fakeClient();
    selectSomething();
    render(<VfxSection client={client} />);
    const picker = await screen.findByTestId("vfx-trigger");
    fireEvent.change(picker, { target: { value: "whenHit" } });
    fireEvent.click(await screen.findByTestId("fx-sparks"));
    await waitFor(() =>
      expect(client.vfxAdd).toHaveBeenCalledWith("e1", "sparks", "whenHit"),
    );
  });

  it("locks authoring during Play and says why", async () => {
    const client = fakeClient();
    selectSomething();
    act(() => playStore.getState().refresh({ playing: true, paused: false }));
    render(<VfxSection client={client} />);
    const fire = await screen.findByTestId("fx-fire");
    expect(fire.hasAttribute("disabled")).toBe(true);
    expect(fire.getAttribute("title")).toMatch(/stop play first/i);
    act(() => playStore.getState().refresh({ playing: false, paused: false }));
  });

  it("shows a refusal in plain language instead of failing silently", async () => {
    const client = fakeClient();
    client.vfxAdd = vi.fn(() =>
      Promise.resolve({
        entity: null,
        layers: 0,
        particles: 0,
        reads: [],
        problems: [],
        message: "",
        reason: "that object already has 4 effects — remove one first",
      }),
    );
    selectSomething();
    render(<VfxSection client={client} />);
    fireEvent.click(await screen.findByTestId("fx-fire"));
    await waitFor(() =>
      expect(screen.getByTestId("vfx-refusal").textContent).toMatch(/remove one first/),
    );
  });

  it("surfaces continuity warnings without blocking the click", async () => {
    const client = fakeClient();
    // The panel re-reads from the engine after a mutation rather than trusting the reply (so Ctrl-Z
    // and any other out-of-band change stay honest), which means the fake's list has to agree with
    // its add — an inconsistent fake would otherwise assert stale behaviour.
    const landed = {
      entity: "e1",
      layers: 2,
      particles: 120,
      reads: ["Fire", "Sparkle"],
      problems: ["every layer glows — adding a soft layer gives the glow something to read against"],
      message: "Added Sparkle",
      reason: null,
    };
    client.vfxList = vi.fn(() => Promise.resolve(landed));
    client.vfxAdd = vi.fn(() =>
      Promise.resolve({
        entity: "e1",
        layers: 2,
        particles: 120,
        reads: ["Fire", "Sparkle"],
        problems: ["every layer glows — adding a soft layer gives the glow something to read against"],
        message: "Added Sparkle",
        reason: null,
      }),
    );
    selectSomething();
    render(<VfxSection client={client} />);
    fireEvent.click(await screen.findByTestId("fx-fire"));
    await waitFor(() =>
      expect(screen.getByTestId("vfx-problem").textContent).toMatch(/every layer glows/),
    );
    // the layer still landed — advice, not a wall
    expect(screen.getAllByTestId("vfx-layer-row").length).toBe(2);
  });

  it("closes the loop during Play: a LIVE particle count, not just greyed buttons", async () => {
    // The panel's own closing line is "Press Play to see it" — the loop it opens has to close where
    // the author is looking, the way RolesSection closes its own with a live score.
    const client = fakeClient();
    client.vfxProbe = vi.fn(() =>
      Promise.resolve({ additive: 72, soft: 40, total: 112, bursts: 1, peakRadiance: 5.9 }),
    );
    selectSomething();
    act(() => playStore.getState().refresh({ playing: true, paused: false }));
    render(<VfxSection client={client} />);
    const live = await screen.findByTestId("vfx-live");
    await waitFor(() => expect(live.textContent).toMatch(/112 particles/));
    expect(live.textContent).toMatch(/72 glowing/);
    expect(live.textContent).toMatch(/40 hazy/);
    expect(live.textContent).toMatch(/1 one-shot/);
  });

  it("says so plainly when nothing is running, rather than showing a bare zero", async () => {
    const client = fakeClient();
    selectSomething();
    act(() => playStore.getState().refresh({ playing: true, paused: false }));
    render(<VfxSection client={client} />);
    const live = await screen.findByTestId("vfx-live");
    await waitFor(() => expect(live.textContent).toMatch(/nothing is running right now/));
  });
});

describe("CinemaSection", () => {
  it("reports WHICH state the camera is in during Play — there is only one camera", async () => {
    const client = fakeClient();
    client.cameraProbe = vi.fn(() =>
      Promise.resolve({
        eye: [1, 2, 3] as [number, number, number],
        lookAt: [0, 0, 0] as [number, number, number],
        fovDeg: 50,
        cinematic: true,
        distance: 6,
      }),
    );
    selectSomething();
    act(() => playStore.getState().refresh({ playing: true, paused: false }));
    render(<CinemaSection client={client} />);
    await waitFor(() =>
      expect(screen.getByTestId("cinema-live").textContent).toMatch(/has the camera/),
    );
  });

  it("and says the camera is FREE when no cutscene owns it", async () => {
    const client = fakeClient();
    selectSomething();
    act(() => playStore.getState().refresh({ playing: true, paused: false }));
    render(<CinemaSection client={client} />);
    await waitFor(() =>
      expect(screen.getByTestId("cinema-live").textContent).toMatch(/camera is free/),
    );
  });

  it("offers the shot catalogue and lands a hero shot in one click", async () => {
    const client = fakeClient();
    selectSomething();
    render(<CinemaSection client={client} />);
    const hero = await screen.findByTestId("shot-hero");
    expect(hero.getAttribute("title")).toMatch(/adds:/i);
    fireEvent.click(hero);
    await waitFor(() => expect(client.cinemaAddShot).toHaveBeenCalledWith("e1", "hero"));
    await waitFor(() => expect(screen.getByTestId("cinema-shots").textContent).toMatch(/pushing in/));
  });

  it("authors Calm pacing through the shared control and displays the effective duration", async () => {
    const client = fakeClient();
    let mood: "calm" | "normal" | "tense" = "normal";
    const shot = (seconds: number): ShotRow => ({
      id: "shot-1",
      index: 0,
      reads: "a full shot of e1, three-quarters on, pushing in",
      seconds: 2.5,
      effectiveSeconds: seconds,
      openSeconds: 0,
      blendSeconds: 0,
      startSeconds: 0,
      size: "full",
      angle: "three_quarter",
      motion: "push_in",
      amount: 0.35,
      subject: "e1",
      subjectName: "e1",
    });
    client.cinemaList = vi.fn((id: string) => Promise.resolve({
      entity: id,
      shots: 1,
      seconds: mood === "calm" ? 6.25 : 2.5,
      mood,
      reads: [shot(2.5).reads],
      rows: [shot(mood === "calm" ? 6.25 : 2.5)],
      problems: [],
      message: "",
      reason: null,
    }));
    client.cinemaSetMood = vi.fn((id: string, next: "calm" | "normal" | "tense") => {
      mood = next;
      return Promise.resolve({
        entity: id,
        shots: 1,
        seconds: next === "calm" ? 6.25 : 2.5,
        mood: next,
        reads: [shot(2.5).reads],
        rows: [shot(next === "calm" ? 6.25 : 2.5)],
        problems: [],
        message: `Pacing set to ${next}`,
        reason: null,
      });
    });
    selectSomething();
    render(<CinemaSection client={client} />);

    const calm = await screen.findByTestId("cinema-mood-calm");
    expect(calm.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(calm);

    await waitFor(() => expect(client.cinemaSetMood).toHaveBeenCalledWith("e1", "calm"));
    await waitFor(() => expect(calm.getAttribute("aria-pressed")).toBe("true"));
    expect(screen.getByTestId("cinema-section").textContent).toContain("6.3s");
  });

  it("locks shot authoring during Play", async () => {
    const client = fakeClient();
    selectSomething();
    act(() => playStore.getState().refresh({ playing: true, paused: false }));
    render(<CinemaSection client={client} />);
    expect((await screen.findByTestId("shot-hero")).hasAttribute("disabled")).toBe(true);
    act(() => playStore.getState().refresh({ playing: false, paused: false }));
  });
});
