//! A DOM overlay ON the stage must not also DRIVE the stage.
//!
//! The viewport is a `position: relative` region with pointer handlers on it — a left click picks, a
//! right drag orbits, a press can grab a gizmo handle — and the shell overlays real controls inside
//! that region: the "● PLAYING" badge with its ⏹ Stop, and the first-run card with Skip / Start. A
//! click on any of them bubbles to the viewport's own handler, so pressing a button that is *on* the
//! stage also fires a pick *at* the stage.
//!
//! THIS WAS LIVE, and it was measured before it was fixed rather than argued. In a real Chromium at
//! 1440 px, clicking the onboarding card's Skip button moved the status line from
//! `connected · 5 entities` to `nothing here` — the pick had run, missed, and reported. The user
//! pressed Skip and the editor also deselected for them.
//!
//! It surfaced while moving the onboarding card ONTO the stage to stop it overlapping the left dock
//! (`<ux_quality>` 4). The card was previously `position: fixed` over the window, so it had the same
//! exposure only where it happened to sit above the viewport; `PlayBadge` has always been inside it.
//! Both are fixed, and both are pinned here, because a guard nothing exercises is a guard that comes
//! back off in the next refactor.

import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { playStore } from "../store/play";
import { projectionStore } from "../store/projection";
import { toastStore } from "../store/toasts";
import { walletStore } from "../store/wallet";

/** Every session this file's `App` builds, so the spy can be read after the render. */
const picks: ReturnType<typeof vi.fn>[] = [];

vi.mock("../transport/session", async (importOriginal) => {
  const real = await importOriginal<typeof import("../transport/session")>();
  return {
    ...real,
    // The REAL session, with one method watched. Not a hand-written fake: the thing under test is
    // whether an event reaches the viewport's handler, and a fake client would still be wired to the
    // same handler — but a fake shell would be a payload `/core` cannot produce, which is the
    // failure ADR-123 is about. Only the observation is added.
    createSession: () => {
      const client = real.createSession();
      const spy = vi.fn(client.viewportPick.bind(client));
      client.viewportPick = spy as typeof client.viewportPick;
      picks.push(spy);
      return client;
    },
  };
});

// Imported AFTER the mock is declared so `App` closes over the wrapped `createSession`.
const { App } = await import("./App");

/** The pick spy belonging to the App instance rendered by the current test. */
const lastPick = () => picks[picks.length - 1];

beforeEach(() => {
  picks.length = 0;
});

afterEach(() => {
  projectionStore.getState().reset();
  playStore.getState().reset();
  walletStore.getState().reset();
  toastStore.getState().reset();
  window.localStorage.clear();
});

describe("controls overlaid on the stage do not also click the stage", () => {
  it("the first-run card's Skip does not fire a viewport pick underneath it", () => {
    render(<App />);
    // A pre-assertion, so this cannot pass against a shell that never rendered the card at all —
    // `show={!sceneEmpty && !playing}`, and both of those could change without anyone noticing here.
    const skip = screen.getByTestId("onboardSkip");
    expect(lastPick()).not.toHaveBeenCalled();

    fireEvent.click(skip);

    // The card is INSIDE `#viewport`, whose `onClick` picks. Without `stopPropagation` this is 1 —
    // verified in a real browser, where the status line read "nothing here" after pressing Skip.
    expect(lastPick()).not.toHaveBeenCalled();
  });

  it("the PLAYING badge's Stop does not fire a viewport pick underneath it", () => {
    render(<App />);
    act(() => playStore.getState().refresh({ playing: true, paused: false }));
    const stop = screen.getByTestId("stageStop");
    expect(lastPick()).not.toHaveBeenCalled();

    fireEvent.click(stop);

    expect(lastPick()).not.toHaveBeenCalled();
  });

  it("and the guard is not a blanket one: a click on the bare viewport still picks", () => {
    render(<App />);
    // The control that matters. A fix that stopped every click reaching the viewport would pass both
    // tests above and break the editor's primary gesture — select by clicking the thing.
    fireEvent.click(screen.getByTestId("viewport"));
    expect(lastPick()).toHaveBeenCalledTimes(1);
  });
});
