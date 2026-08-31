//! The aim session — the state a click on the stage turns into a shot's subject.
//!
//! Every case here is a way the gesture can go wrong in a manner nothing else would catch: a choice
//! that lands after the mode was cancelled, the same object aimed at twice, and a commit consumed by
//! the panel that did not start the aim.

import { beforeEach, describe, expect, it } from "vitest";
import { subjectAimStore, type AimRung } from "./subjectAim";

const RUNGS: AimRung[] = [
  { id: "e2", name: "Bracket", parts: 1, group: "This object" },
  { id: "e1", name: "Weld Gun 7", parts: 42, group: "What it is part of" },
];

beforeEach(() => subjectAimStore.getState().cancel());

describe("aiming a shot by pointing at the thing", () => {
  it("begins on one shot of one cutscene and carries what the badge has to say", () => {
    subjectAimStore.getState().begin("e1", 1, 5);
    const state = subjectAimStore.getState();
    expect(state.active).toBe(true);
    expect(state.owner).toBe("e1");
    expect(state.shotIndex).toBe(1);
    expect(state.shots).toBe(5);
    expect(state.rungs).toEqual([]);
    expect(state.picked).toBeNull();
  });

  it("a pick ends the mode and leaves the choice for the panel", () => {
    subjectAimStore.getState().begin("e1", 0, 2);
    subjectAimStore.getState().hover(RUNGS);
    subjectAimStore.getState().pick("e1", "Weld Gun 7");

    const state = subjectAimStore.getState();
    // The stage stops intercepting clicks at the CHOICE — the commit is the panel's, and its own
    // busy state carries the rest. A mode still on while a commit is in flight is a second click
    // aiming a shot the user has already aimed.
    expect(state.active).toBe(false);
    expect(state.rungs).toEqual([]);
    expect(state.picked).toMatchObject({ subject: "e1", name: "Weld Gun 7" });
  });

  it("aiming twice at the SAME object is two events, not one swallowed by the first", () => {
    subjectAimStore.getState().begin("e1", 0, 2);
    subjectAimStore.getState().pick("e9", "Assembly Hall");
    const first = subjectAimStore.getState().picked!;
    subjectAimStore.getState().taken(first.seq);

    subjectAimStore.getState().begin("e1", 1, 2);
    subjectAimStore.getState().pick("e9", "Assembly Hall");
    const second = subjectAimStore.getState().picked!;

    // Without the sequence number an effect keyed on the subject id would see the same value and do
    // nothing — so the second shot would silently keep filming whatever it filmed before. Asserted
    // as a DIFFERENCE, not an absolute: the counter is deliberately session-lifetime, which is the
    // whole reason a stale `taken` from the previous aim cannot consume this one.
    expect(second.subject).toBe("e9");
    expect(second.seq).toBeGreaterThan(first.seq);
  });

  it("a choice that arrives after a cancel is refused, not applied late", () => {
    subjectAimStore.getState().begin("e1", 0, 2);
    subjectAimStore.getState().cancel();

    subjectAimStore.getState().pick("e9", "Assembly Hall");

    // The real shape of this: Esc, then the in-flight peek from the click before it resolves. A shot
    // re-aimed a beat after the user pressed Cancel is an edit they explicitly declined.
    expect(subjectAimStore.getState().picked).toBeNull();
    expect(subjectAimStore.getState().active).toBe(false);
  });

  it("only the panel holding THIS choice can consume it", () => {
    subjectAimStore.getState().begin("e1", 0, 2);
    subjectAimStore.getState().pick("e9", "Assembly Hall");

    const seq = subjectAimStore.getState().picked!.seq;
    subjectAimStore.getState().taken(seq - 1);
    expect(subjectAimStore.getState().picked).not.toBeNull();

    subjectAimStore.getState().taken(seq);
    expect(subjectAimStore.getState().picked).toBeNull();
  });

  it("a hover clears the looking flag, and a cancel clears everything", () => {
    subjectAimStore.getState().begin("e1", 0, 2);
    subjectAimStore.getState().look(true);
    expect(subjectAimStore.getState().looking).toBe(true);

    subjectAimStore.getState().hover(RUNGS);
    expect(subjectAimStore.getState().looking).toBe(false);
    expect(subjectAimStore.getState().rungs).toHaveLength(2);

    subjectAimStore.getState().cancel();
    expect(subjectAimStore.getState()).toMatchObject({
      active: false,
      owner: null,
      shotIndex: null,
      rungs: [],
      looking: false,
      picked: null,
    });
  });
});
