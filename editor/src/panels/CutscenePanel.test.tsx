//! The cutscene timeline: a shot is a LENGTH, the sequence is an ORDER, and both are editable.
//!
//! Every assertion here is about a capability the panel did not have before — the engine did, and the
//! reply did not carry it. The one that matters most is the drag: a slider that committed per pixel
//! would be forty undo steps for one decision, so the commit is asserted to happen ONCE, on release.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { CutscenePanel } from "./CutscenePanel";
import { fakeClient } from "../transport/test-client";
import { projectionStore } from "../store/projection";
import { playStore } from "../store/play";
import type { CinemaReply, ShotRow } from "../transport/protocol";

function row(over: Partial<ShotRow> & Pick<ShotRow, "id" | "index" | "startSeconds">): ShotRow {
  return {
    reads: `shot ${over.index + 1}`,
    seconds: 2,
    effectiveSeconds: 2,
    size: "full",
    angle: "three_quarter",
    motion: "push_in",
    amount: 0.35,
    subject: "e1",
    subjectName: "Weld Gun 7",
    ...over,
  };
}

/** Four shots of deliberately DIFFERENT lengths — 2s, 4s, 1s, 3s over a 10s cut — because a timeline
 *  whose clips are all the same width proves nothing about whether it draws durations at all. */
const FOUR_SHOTS: CinemaReply = {
  entity: "e1",
  shots: 4,
  seconds: 10,
  mood: "normal",
  reads: [],
  rows: [
    row({ id: "s1", index: 0, startSeconds: 0, seconds: 2, effectiveSeconds: 2, size: "wide", motion: "pull_out", reads: "a wide shot of Weld Gun 7 from three-quarters, pulling out — 2.0s" }),
    row({ id: "s2", index: 1, startSeconds: 2, seconds: 4, effectiveSeconds: 4, reads: "a full shot of Weld Gun 7 from three-quarters, pushing in — 4.0s" }),
    row({ id: "s3", index: 2, startSeconds: 6, seconds: 1, effectiveSeconds: 1, size: "close", motion: "hold", amount: 0, reads: "a close shot of Weld Gun 7 from three-quarters, holding still — 1.0s" }),
    row({ id: "s4", index: 3, startSeconds: 7, seconds: 3, effectiveSeconds: 3, size: "medium", motion: "orbit", amount: 0.5, reads: "a medium shot of Weld Gun 7 from three-quarters, orbiting — 3.0s" }),
  ],
  problems: [],
  message: "",
  reason: null,
};

function loaded() {
  const client = fakeClient();
  client.cinemaList = vi.fn(() => Promise.resolve(FOUR_SHOTS));
  return client;
}

function selectSomething() {
  act(() => projectionStore.getState().select("e1"));
}

beforeEach(() => {
  act(() => playStore.getState().refresh({ playing: false, paused: false }));
});

describe("CutscenePanel", () => {
  it("says what to do when nothing is selected rather than showing an empty clock", () => {
    act(() => projectionStore.getState().select(null));
    render(<CutscenePanel client={fakeClient()} />);
    expect(screen.getByTestId("cutscene-panel").textContent).toMatch(/no object selected/i);
  });

  it("draws one clip per shot, sized by how long it runs — the number that did not cross the boundary", async () => {
    selectSomething();
    render(<CutscenePanel client={loaded()} />);
    const clips = await screen.findAllByTestId("cutscene-clip");
    expect(clips).toHaveLength(4);
    // 2s, 4s, 1s and 3s of a 10s cut. Read off the style rather than a class, because the WIDTH is
    // the whole claim: a timeline that lays four equal bars has drawn a list, not a duration.
    expect(clips.map((clip) => clip.style.width)).toEqual(["20%", "40%", "10%", "30%"]);
    expect(clips.map((clip) => clip.style.left)).toEqual(["0%", "20%", "60%", "70%"]);
  });

  it("opens the shot on its own numbers when a clip is clicked", async () => {
    selectSomething();
    render(<CutscenePanel client={loaded()} />);
    const clips = await screen.findAllByTestId("cutscene-clip");
    fireEvent.click(clips[1]);
    const editor = await screen.findByTestId("cutscene-shot-editor");
    expect(editor.textContent).toMatch(/Shot 2 of 4/);
    expect(screen.getByTestId("cutscene-shot-reads").textContent).toMatch(/pushing in/);
    expect((screen.getByTestId("cutscene-seconds") as HTMLInputElement).value).toBe("4");
    expect((screen.getByTestId("cutscene-size") as HTMLSelectElement).value).toBe("full");
    expect((screen.getByTestId("cutscene-motion") as HTMLSelectElement).value).toBe("push_in");
  });

  it("commits a new length ONCE, on release — not once per pixel of the drag", async () => {
    const client = loaded();
    selectSomething();
    render(<CutscenePanel client={client} />);
    fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[1]);
    const slider = await screen.findByTestId("cutscene-seconds");

    // The drag itself.
    fireEvent.change(slider, { target: { value: "5" } });
    fireEvent.change(slider, { target: { value: "6" } });
    fireEvent.change(slider, { target: { value: "6.5" } });
    expect(client.cinemaSetShotSeconds).not.toHaveBeenCalled();
    // ...and the read-out follows the pointer, so the control is aimable while it is being dragged.
    expect(screen.getByTestId("cutscene-seconds-value").textContent).toBe("6.5");

    fireEvent.pointerUp(slider);
    await waitFor(() => expect(client.cinemaSetShotSeconds).toHaveBeenCalledTimes(1));
    expect(client.cinemaSetShotSeconds).toHaveBeenCalledWith("e1", 1, 6.5);
  });

  it("re-frames a shot in place, on the axis that changed and no other", async () => {
    const client = loaded();
    selectSomething();
    render(<CutscenePanel client={client} />);
    fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[0]);
    fireEvent.change(await screen.findByTestId("cutscene-size"), { target: { value: "close" } });
    await waitFor(() =>
      expect(client.cinemaSetShotFraming).toHaveBeenCalledWith("e1", 0, { size: "close" }),
    );
  });

  it("moves a shot along the sequence", async () => {
    const client = loaded();
    selectSomething();
    render(<CutscenePanel client={client} />);
    fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[0]);
    fireEvent.click(await screen.findByTestId("cutscene-later"));
    await waitFor(() => expect(client.cinemaMoveShot).toHaveBeenCalledWith("e1", 0, 1));
  });

  it("refuses at both ends of the sequence WITH the reason, rather than going dark", async () => {
    selectSomething();
    render(<CutscenePanel client={loaded()} />);

    fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[0]);
    const earlier = await screen.findByTestId("cutscene-earlier");
    expect(earlier.hasAttribute("disabled")).toBe(true);
    expect(earlier.getAttribute("title")).toMatch(/already the first shot/i);

    fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[3]);
    const later = await screen.findByTestId("cutscene-later");
    expect(later.hasAttribute("disabled")).toBe(true);
    expect(later.getAttribute("title")).toMatch(/already the last shot/i);
  });

  it("disables move strength on a locked-off shot and says why, instead of offering a dial that does nothing", async () => {
    selectSomething();
    render(<CutscenePanel client={loaded()} />);
    // Shot 3 holds still.
    fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[2]);
    const amount = await screen.findByTestId("cutscene-amount");
    expect(amount.hasAttribute("disabled")).toBe(true);
    expect(amount.getAttribute("title")).toMatch(/does not move/i);
    // ...and the same sentence is in the field's help, where a reader looking at the label finds it.
    expect(screen.getByTestId("cutscene-shot-editor").textContent).toMatch(/does not move/i);
  });

  it("scrubbing the ruler says which shot is live at that moment", async () => {
    selectSomething();
    render(<CutscenePanel client={loaded()} />);
    const panel = await screen.findByTestId("cutscene-panel");
    // 0s is inside shot 1 before anything is touched.
    expect(panel.textContent).toMatch(/0\.0s · shot 1 of 4/);
    // Clicking the third clip moves the playhead to where that shot starts.
    fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[2]);
    await waitFor(() => expect(panel.textContent).toMatch(/6\.0s · shot 3 of 4/));
  });

  it("locks every authoring control during Play, each with the same plain reason", async () => {
    selectSomething();
    render(<CutscenePanel client={loaded()} />);
    fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[1]);
    act(() => playStore.getState().refresh({ playing: true, paused: false }));

    for (const id of ["cutscene-seconds", "cutscene-size", "cutscene-angle", "cutscene-motion", "cutscene-remove", "shot-hero"]) {
      const control = await screen.findByTestId(id);
      expect(control.hasAttribute("disabled")).toBe(true);
      expect(control.getAttribute("title")).toMatch(/stop play first/i);
    }
    act(() => playStore.getState().refresh({ playing: false, paused: false }));
  });

  it("refuses a thirteenth shot with the ceiling and the way out, from the engine's own number", async () => {
    const client = loaded();
    client.cinemaList = vi.fn(() =>
      Promise.resolve({ ...FOUR_SHOTS, shots: 12, rows: FOUR_SHOTS.rows }),
    );
    selectSomething();
    render(<CutscenePanel client={client} />);
    const hero = await screen.findByTestId("shot-hero");
    expect(hero.hasAttribute("disabled")).toBe(true);
    expect(hero.getAttribute("title")).toMatch(/at most 12 shots/i);
  });
});
