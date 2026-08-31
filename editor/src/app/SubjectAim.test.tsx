//! **Aiming a shot at the thing you are looking at**, driven through the stage the way a user drives
//! it.
//!
//! THE ASSERTION THIS FILE EXISTS FOR is the one that reads as a double negative: while an aim is in
//! flight, a click on the viewport must NOT pick. Picking is what the stage does — it is the
//! editor's primary gesture — and here it would be a bug with no visible symptom at the moment it
//! happens: `viewport_pick` changes the SELECTION, the Cutscene panel is bound to the selection, so
//! selecting the object being named switches which cutscene is on screen and throws away the shot
//! the user was in the middle of aiming. The whole reason this mode reads through `viewport_peek` is
//! that peek answers the same question and changes nothing.

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { playStore } from "../store/play";
import { cinemaPreviewStore } from "../store/cinemaPreview";
import { projectionStore } from "../store/projection";
import { subjectAimStore } from "../store/subjectAim";
import { stageHighlightStore } from "../store/stageHighlight";
import { toastStore } from "../store/toasts";
import { walletStore } from "../store/wallet";
import type { SubjectCatalog } from "../transport/protocol";

/** The chain the engine answers with for the bracket: the leaf a click can land on, and the machine
 *  it belongs to. Shaped exactly like `subject_chain`'s reply, headings included. */
const CHAIN: SubjectCatalog = {
  owner: "e-bracket",
  ownerName: "Bracket",
  current: null,
  candidates: [
    { id: "e-bracket", name: "Bracket", group: "This object", parts: 1, framable: true, current: false },
    { id: "e-gun", name: "Weld Gun 7", group: "What it is part of", parts: 42, framable: true, current: false },
  ],
  query: "",
  matches: 2,
  truncated: false,
};

interface Spies {
  pick: ReturnType<typeof vi.fn>;
  peek: ReturnType<typeof vi.fn>;
  chain: ReturnType<typeof vi.fn>;
  hover: ReturnType<typeof vi.fn>;
}
const sessions: Spies[] = [];

vi.mock("../transport/session", async (importOriginal) => {
  const real = await importOriginal<typeof import("../transport/session")>();
  return {
    ...real,
    // The REAL session with three methods watched, for the reason `StageOverlays.test.tsx` states:
    // the thing under test is which of them an event reaches, and a hand-written fake client would
    // still be wired to the same handler while inventing a payload `/core` cannot produce.
    createSession: () => {
      const client = real.createSession();
      const pick = vi.fn(client.viewportPick.bind(client));
      const peek = vi.fn(() => Promise.resolve<string | null>("e-bracket"));
      const chain = vi.fn(() => Promise.resolve(CHAIN));
      const hover = vi.fn((_ids: string[]) => Promise.resolve(0));
      client.viewportPick = pick as typeof client.viewportPick;
      client.viewportPeek = peek as typeof client.viewportPeek;
      client.cinemaSubjectChain = chain as typeof client.cinemaSubjectChain;
      client.viewportHover = hover as typeof client.viewportHover;
      sessions.push({ pick, peek, chain, hover });
      return client;
    },
  };
});

const { App } = await import("./App");

const spies = () => sessions[sessions.length - 1];

beforeEach(() => {
  sessions.length = 0;
  act(() => subjectAimStore.getState().cancel());
  act(() => stageHighlightStore.getState().reset());
});

afterEach(() => {
  act(() => subjectAimStore.getState().cancel());
  act(() => stageHighlightStore.getState().reset());
  projectionStore.getState().reset();
  playStore.getState().reset();
  cinemaPreviewStore.getState().reset();
  walletStore.getState().reset();
  toastStore.getState().reset();
  window.localStorage.clear();
});

describe("a shot is aimed by pointing at the object, and the selection never moves", () => {
  it("a click on the stage PEEKS instead of picking, and the choice is the peeked object", async () => {
    render(<App />);
    act(() => subjectAimStore.getState().begin("e1", 1, 4));

    fireEvent.click(screen.getByTestId("viewport"));

    await waitFor(() => expect(subjectAimStore.getState().picked?.subject).toBe("e-bracket"));
    expect(spies().peek).toHaveBeenCalledTimes(1);
    // THE POINT. One `viewport_pick` here and the cutscene on screen would have changed under the
    // author mid-gesture.
    expect(spies().pick).not.toHaveBeenCalled();
    expect(subjectAimStore.getState().active).toBe(false);
  });

  it("and with no aim in flight the same click still picks — the mode is not a blanket guard", () => {
    render(<App />);
    fireEvent.click(screen.getByTestId("viewport"));
    expect(spies().pick).toHaveBeenCalledTimes(1);
    expect(spies().peek).not.toHaveBeenCalled();
  });

  it("the badge names what the cursor is over and offers the assembly it is part of", async () => {
    render(<App />);
    act(() => subjectAimStore.getState().begin("e1", 0, 2));
    // Before any hover the badge asks for the gesture rather than naming nothing.
    expect(screen.getByTestId("subjectAimHint").textContent).toBe("click what this shot should film");

    fireEvent.pointerMove(screen.getByTestId("viewport"), { clientX: 400, clientY: 300 });

    // A click lands on a LEAF — one bracket. The rung beside it is the machine, which is the shot
    // the author almost always meant, and it is one click rather than a search.
    const gun = await screen.findByTestId("subjectAimRung-e-gun");
    expect(gun.textContent).toContain("Weld Gun 7");
    expect(gun.textContent).toContain("42 parts");
    expect(screen.getByTestId("subjectAimRung-e-bracket").textContent).toContain("1 part");

    fireEvent.click(gun);
    expect(subjectAimStore.getState().picked).toEqual(
      expect.objectContaining({ subject: "e-gun", name: "Weld Gun 7" }),
    );
    expect(spies().pick).not.toHaveBeenCalled();
  });

  it("a second hover over the same object does not re-read the chain", async () => {
    render(<App />);
    act(() => subjectAimStore.getState().begin("e1", 0, 2));

    const viewport = screen.getByTestId("viewport");
    fireEvent.pointerMove(viewport, { clientX: 400, clientY: 300 });
    await screen.findByTestId("subjectAimRung-e-gun");
    expect(spies().chain).toHaveBeenCalledTimes(1);

    fireEvent.pointerMove(viewport, { clientX: 410, clientY: 305 });
    // The chain read counts DRAWN PARTS for every rung, which walks every published instance in the
    // scene — 15,711 of them on the imported production line. Sweeping the cursor across one object
    // must not ask for that again.
    await waitFor(() => expect(spies().peek.mock.calls.length).toBeGreaterThan(1));
    expect(spies().chain).toHaveBeenCalledTimes(1);
  });

  it("the first-run card yields to the badge — they share one anchor", () => {
    render(<App />);
    // A pre-assertion, or "the card is gone" would be true of a shell that never drew it.
    expect(screen.getByTestId("onboardSkip")).toBeTruthy();

    act(() => subjectAimStore.getState().begin("e1", 0, 2));

    // Both are `position: absolute; left: 50%; bottom: space.lg` on the stage, so with both up the
    // badge sits on the card's headline — `<ux_quality>` 4's overlap, seen on an `.exe` capture. The
    // card yields to an aim exactly as it already yields to Play and to the dock sheet.
    expect(screen.queryByTestId("onboardSkip")).toBeNull();
    expect(screen.getByTestId("subjectAimBadge")).toBeTruthy();

    act(() => subjectAimStore.getState().cancel());
    expect(screen.getByTestId("onboardSkip")).toBeTruthy();
  });

  it("Escape cancels the aim, and the shot is left framing what it did", async () => {
    render(<App />);
    act(() => subjectAimStore.getState().begin("e1", 0, 2));
    expect(screen.getByTestId("subjectAimBadge")).toBeTruthy();

    act(() => {
      fireEvent.keyDown(window, { key: "Escape" });
    });

    expect(subjectAimStore.getState().active).toBe(false);
    expect(subjectAimStore.getState().picked).toBeNull();
    expect(screen.queryByTestId("subjectAimBadge")).toBeNull();
  });

  it("the badge's own Cancel does not fire a pick underneath it", () => {
    render(<App />);
    act(() => subjectAimStore.getState().begin("e1", 0, 2));

    fireEvent.click(screen.getByTestId("subjectAimCancel"));

    // The badge is INSIDE `#viewport`, whose `onClick` handles the aim — so without `stageInput`'s
    // one gate, pressing Cancel would also aim the shot at whatever is behind the button.
    expect(subjectAimStore.getState().active).toBe(false);
    expect(subjectAimStore.getState().picked).toBeNull();
    expect(spies().peek).not.toHaveBeenCalled();
    expect(spies().pick).not.toHaveBeenCalled();
  });
});

describe("the stage answers as well as the badge", () => {
  /** Every subject list the editor has asked the stage to light, oldest first. */
  const asked = () => spies().hover.mock.calls.map((call) => call[0] as string[]);

  it("what the cursor is over lights up on the stage, not only in the badge", async () => {
    render(<App />);
    act(() => subjectAimStore.getState().begin("e1", 0, 2));

    fireEvent.pointerMove(screen.getByTestId("viewport"), { clientX: 400, clientY: 300 });

    // The LEAF, because that is what a click on the stage takes. The badge names the same object in
    // the same breath, so a capture where they disagree is a bug either way round.
    await waitFor(() => expect(asked()).toContainEqual(["e-bracket"]));
    await screen.findByTestId("subjectAimRung-e-bracket");
  });

  it("hovering the assembly rung lights the assembly — the part count stops being a claim", async () => {
    render(<App />);
    act(() => subjectAimStore.getState().begin("e1", 0, 2));
    fireEvent.pointerMove(screen.getByTestId("viewport"), { clientX: 400, clientY: 300 });
    const gun = await screen.findByTestId("subjectAimRung-e-gun");

    fireEvent.pointerEnter(gun);
    await waitFor(() => expect(asked().at(-1)).toEqual(["e-gun"]));

    // And leaving it hands the stage back to what the cursor is actually over, rather than going
    // dark: the author is still pointing at the bracket.
    fireEvent.pointerLeave(gun);
    await waitFor(() => expect(asked().at(-1)).toEqual(["e-bracket"]));
  });

  it("the same answer twice is one crossing of the boundary", async () => {
    render(<App />);
    act(() => subjectAimStore.getState().begin("e1", 0, 2));
    const viewport = screen.getByTestId("viewport");
    fireEvent.pointerMove(viewport, { clientX: 400, clientY: 300 });
    await waitFor(() => expect(asked()).toContainEqual(["e-bracket"]));
    const sofar = spies().hover.mock.calls.length;

    // A sweep across one object re-peeks and gets the same id. Re-sending it would re-walk every
    // drawn instance and re-upload the whole instance buffer for a picture that has not changed.
    fireEvent.pointerMove(viewport, { clientX: 410, clientY: 305 });
    await waitFor(() => expect(spies().peek.mock.calls.length).toBeGreaterThan(1));
    expect(spies().hover.mock.calls.length).toBe(sofar);
  });

  it("the cue dies with the mode", async () => {
    render(<App />);
    act(() => subjectAimStore.getState().begin("e1", 0, 2));
    fireEvent.pointerMove(screen.getByTestId("viewport"), { clientX: 400, clientY: 300 });
    await waitFor(() => expect(asked()).toContainEqual(["e-bracket"]));

    act(() => {
      fireEvent.keyDown(window, { key: "Escape" });
    });

    // A highlight that outlived the aim would leave the stage claiming the cursor is over something
    // in a viewport that has gone back to selecting.
    await waitFor(() => expect(asked().at(-1)).toEqual([]));
    expect(stageHighlightStore.getState().source).toBeNull();
  });
});
