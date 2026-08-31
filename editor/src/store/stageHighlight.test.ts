//! The rules the three pointing surfaces share, asserted where they live rather than three times over
//! in three panels.

import { beforeEach, describe, expect, it } from "vitest";
import { highlightKey, stageHighlightStore } from "./stageHighlight";

const state = () => stageHighlightStore.getState();

beforeEach(() => {
  state().reset();
});

describe("what the stage is being asked to light", () => {
  it("starts dark, and pointing at something names who is asking", () => {
    expect(state().ids).toEqual([]);
    expect(state().source).toBeNull();

    state().show("stage", ["e-bracket"]);
    expect(state().ids).toEqual(["e-bracket"]);
    expect(state().source).toBe("stage");
  });

  it("a rung takes over from the stage, and giving it back restores nothing on its own", () => {
    state().show("stage", ["e-bracket"]);
    state().show("rung", ["e-gun"]);
    expect(state().source).toBe("rung");

    // `clear` is the honest end of a `mouseleave`: the rung stops claiming the cue. What the cursor
    // is over is re-asserted by the stage's own next hover — this store does not remember a stack,
    // because a stack would mean a hover restored from a peek that has since gone stale.
    state().clear("rung");
    expect(state().ids).toEqual([]);
    expect(state().source).toBeNull();
  });

  it("A LEAVE FROM A SURFACE THAT NO LONGER OWNS THE CUE IS A NO-OP", () => {
    // The ordering that makes this necessary: the pointer leaves a picker row BECAUSE it arrived on
    // the rung above it, and React fires the arrival first. Without the guard the departing surface
    // would blank the cue the arriving one had just set, and the stage would flicker off exactly
    // while the author was comparing two things.
    state().show("picker", ["e-row"]);
    state().show("rung", ["e-gun"]);
    state().clear("picker");
    expect(state().ids).toEqual(["e-gun"]);
    expect(state().source).toBe("rung");
  });

  it("pointing at nothing turns the cue off, but only for the surface that owns it", () => {
    state().show("stage", ["e-bracket"]);
    // Over empty space is an answer: the stage says so, and the cue goes out with the badge's rungs.
    state().show("stage", []);
    expect(state().source).toBeNull();

    state().show("rung", ["e-gun"]);
    // The stage's peek settling on nothing must not blank a rung the pointer is resting on.
    state().show("stage", []);
    expect(state().ids).toEqual(["e-gun"]);
    expect(state().source).toBe("rung");
  });

  it("the same answer twice does not change the state object", () => {
    // What keeps the one effect that crosses the boundary from re-sending: an identical `show` must
    // not produce a new state, or every mouse move over one object would be an IPC call and a 1 MB
    // instance re-upload.
    state().show("stage", ["e-bracket"]);
    const before = stageHighlightStore.getState();
    state().show("stage", ["e-bracket"]);
    expect(stageHighlightStore.getState()).toBe(before);
  });

  it("the key is the ANSWER, not the array identity", () => {
    state().show("stage", ["a", "b"]);
    const first = highlightKey(stageHighlightStore.getState());
    state().show("rung", ["a", "b"]);
    // Two surfaces, one answer: the picture is the same, so the engine is not asked again.
    expect(highlightKey(stageHighlightStore.getState())).toBe(first);
    state().show("rung", ["b", "a"]);
    // Order is part of the answer only because the engine takes a list; a different list is a
    // different question, and the key must not claim otherwise.
    expect(highlightKey(stageHighlightStore.getState())).not.toBe(first);
  });

  it("reset is unconditional, and idempotent", () => {
    state().show("picker", ["e-row"]);
    state().reset();
    expect(state().source).toBeNull();
    const after = stageHighlightStore.getState();
    state().reset();
    expect(stageHighlightStore.getState()).toBe(after);
  });
});
