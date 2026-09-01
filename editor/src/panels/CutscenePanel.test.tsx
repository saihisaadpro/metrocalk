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
import { cinemaPreviewStore } from "../store/cinemaPreview";
import { subjectAimStore } from "../store/subjectAim";
import { stageHighlightStore } from "../store/stageHighlight";
import { frameGuideStore } from "../store/frameGuide";
import { toastStore } from "../store/toasts";
import { DEFAULT_RENDER_SETTINGS } from "../transport/protocol";
import type { CinemaReply, ShotRow } from "../transport/protocol";

function row(over: Partial<ShotRow> & Pick<ShotRow, "id" | "index" | "startSeconds">): ShotRow {
  return {
    reads: `shot ${over.index + 1}`,
    seconds: 2,
    effectiveSeconds: 2,
    // The engine's own `Mood::Normal` window (0.6s, capped at half the shot), and 0 on the first —
    // a fixture that always said 0 would make "opening a shot seeks past its blend" untestable.
    blendSeconds: over.index === 0 ? 0 : 0.5,
    openSeconds: over.index === 0 ? over.startSeconds : over.startSeconds + 0.5,
    size: "full",
    angle: "three_quarter",
    motion: "push_in",
    amount: 0.35,
    camera: null,
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
  delivery: "viewport", render: DEFAULT_RENDER_SETTINGS,
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
  // Shared state between two surfaces is shared state between two TESTS. Without this a run that
  // left the preview on decides whether the next one's scrub poses the camera.
  act(() => cinemaPreviewStore.getState().reset());
  // An aim left in flight by one test decides whether the next one's click aims or selects.
  act(() => subjectAimStore.getState().cancel());
  // The frame-guide preference PERSISTS by design, so one test turning it off would decide whether
  // the next one draws a guide at all — the shared-state trap, in the one store that has storage.
  act(() => frameGuideStore.getState().setWanted(true));
  act(() => frameGuideStore.getState().setDrawn(null));
  act(() => toastStore.getState().reset());
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

  // ── ADR-197 — a warning about the world, refreshed when the world settles ────────────────────

  it("re-reads the cut when the SCENE changes, not only when the panel edits it", async () => {
    // The warning list now carries what the world says about each shot's camera, so an author who
    // pushes a machine into a shot's line of sight must see it. Before this the panel re-read only
    // after its own mutations, and the engine's reply and the panel disagreed on the packaged .exe.
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const client = loaded();
      selectSomething();
      render(<CutscenePanel client={client} />);
      await screen.findAllByTestId("cutscene-clip");
      const before = (client.cinemaList as ReturnType<typeof vi.fn>).mock.calls.length;

      act(() => {
        projectionStore.getState().applyDelta({
          ops: [{ op: "setField", id: "e2", component: "Transform", field: "x", value: 4 }],
        });
      });
      await act(async () => {
        vi.advanceTimersByTime(400);
      });
      await waitFor(() =>
        expect((client.cinemaList as ReturnType<typeof vi.fn>).mock.calls.length).toBeGreaterThan(before),
      );
    } finally {
      vi.useRealTimers();
    }
  });

  it("costs ONE re-read for a burst of deltas, not one per frame", async () => {
    // `applyDelta` is the streaming channel handler and its own comment calls a pure-transform delta
    // "the 60 Hz common case" — a drag, a played animation and a previewed cutscene each push one per
    // frame. Sixty `cinema_list` round trips a second, each running the placement search, for a
    // sentence nobody can read at that rate, is the regression this debounce exists to prevent.
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const client = loaded();
      selectSomething();
      render(<CutscenePanel client={client} />);
      await screen.findAllByTestId("cutscene-clip");
      const before = (client.cinemaList as ReturnType<typeof vi.fn>).mock.calls.length;

      // A second of a 60 Hz stream, with no gap long enough to settle.
      for (let frame = 0; frame < 60; frame += 1) {
        act(() => {
          projectionStore.getState().applyDelta({
            ops: [{ op: "setField", id: "e2", component: "Transform", field: "x", value: frame }],
          });
        });
        act(() => {
          vi.advanceTimersByTime(16);
        });
      }
      expect((client.cinemaList as ReturnType<typeof vi.fn>).mock.calls.length).toBe(before);

      // ...and exactly one once it stops.
      await act(async () => {
        vi.advanceTimersByTime(400);
      });
      await waitFor(() =>
        expect((client.cinemaList as ReturnType<typeof vi.fn>).mock.calls.length).toBe(before + 1),
      );
    } finally {
      vi.useRealTimers();
    }
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
    // Clicking the third clip opens THAT shot — which is its start PLUS its opening blend, because
    // at the start itself the transition weight is zero and the frame is the end of shot 2.
    fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[2]);
    await waitFor(() => expect(panel.textContent).toMatch(/6\.5s · shot 3 of 4/));
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

  it("previews the shot at the playhead, and hands the viewport back when told to", async () => {
    const client = loaded();
    selectSomething();
    render(<CutscenePanel client={client} />);
    const toggle = await screen.findByTestId("cutscene-preview");
    expect(toggle.getAttribute("aria-pressed")).toBe("false");

    fireEvent.click(toggle);
    // The playhead starts at 0, so this is the FIRST shot's opening frame - the same moment Play
    // would draw at t=0.
    await waitFor(() => expect(client.cinemaPreview).toHaveBeenCalledWith("e1", 0, true));
    await waitFor(() => expect(cinemaPreviewStore.getState().active).toBe(true));
    expect(cinemaPreviewStore.getState().entity).toBe("e1");
    await waitFor(() =>
      expect(screen.getByTestId("cutscene-preview").getAttribute("aria-pressed")).toBe("true"),
    );

    fireEvent.click(screen.getByTestId("cutscene-preview"));
    await waitFor(() => expect(client.cinemaPreview).toHaveBeenCalledWith("e1", 0, false));
    expect(cinemaPreviewStore.getState().active).toBe(false);
  });

  it("moves the camera with the playhead - but only once the author has asked for it", async () => {
    const client = loaded();
    selectSomething();
    render(<CutscenePanel client={client} />);
    const clips = await screen.findAllByTestId("cutscene-clip");

    // NOT previewing: clicking a clip is a reading gesture, and taking someone's viewport because
    // they looked at a shot is exactly the surprise the toggle exists to prevent.
    fireEvent.click(clips[2]);
    await waitFor(() => expect(screen.getByTestId("cutscene-shot-editor")).toBeTruthy());
    expect(client.cinemaPreview).not.toHaveBeenCalled();

    fireEvent.click(await screen.findByTestId("cutscene-preview"));
    await waitFor(() => expect(cinemaPreviewStore.getState().active).toBe(true));
    // The playhead is at 6.5s - inside shot 3, past its opening blend - so turning preview on stands
    // there, not at 0 and not at the 6.0s instant where shot 3 is not yet what you see.
    expect(client.cinemaPreview).toHaveBeenCalledWith("e1", 6.5, true);

    // ...and now a clip click DOES move the camera, to the moment that shot becomes itself.
    fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[3]);
    await waitFor(() => expect(client.cinemaPreview).toHaveBeenCalledWith("e1", 7.5, true));
  });

  it("re-poses at the same moment after an edit, so changing a framing shows the new framing", async () => {
    const client = loaded();
    selectSomething();
    render(<CutscenePanel client={client} />);
    fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[1]);
    fireEvent.click(await screen.findByTestId("cutscene-preview"));
    await waitFor(() => expect(cinemaPreviewStore.getState().active).toBe(true));
    const beforeEdit = (client.cinemaPreview as ReturnType<typeof vi.fn>).mock.calls.length;

    fireEvent.change(await screen.findByTestId("cutscene-angle"), { target: { value: "low" } });

    await waitFor(() =>
      expect(client.cinemaSetShotFraming).toHaveBeenCalledWith("e1", 1, { angle: "low" }),
    );
    // The whole point: the edit lands AND the viewport is re-solved at the moment already on screen.
    // Without this the author changes an angle and the picture in front of them is the old one.
    await waitFor(() =>
      expect((client.cinemaPreview as ReturnType<typeof vi.fn>).mock.calls.length).toBe(
        beforeEdit + 1,
      ),
    );
    expect((client.cinemaPreview as ReturnType<typeof vi.fn>).mock.calls.at(-1)).toEqual([
      "e1",
      2.5,
      true,
    ]);
  });

  it("opening a shot seeks past its blend, because at its start it is not what you see", async () => {
    const client = loaded();
    selectSomething();
    render(<CutscenePanel client={client} />);
    const panel = await screen.findByTestId("cutscene-panel");

    // Shot 1 never blends: opening it is its start, exactly.
    fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[0]);
    await waitFor(() => expect(panel.textContent).toMatch(/0\.0s · shot 1 of 4/));

    // Shot 2 starts at 2.0s and opens over 0.5s. Landing on 2.0 would show the END OF SHOT 1 —
    // measured on the packaged .exe, where re-framing from that position moved the camera 0.0000
    // units and the panel's own read-out said "Transition into shot 3".
    fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[1]);
    await waitFor(() => expect(panel.textContent).toMatch(/2\.5s · shot 2 of 4/));
  });

  it("refuses to preview a cutscene with no shots, and says what to do instead", async () => {
    const client = loaded();
    client.cinemaList = vi.fn(() =>
      Promise.resolve({ ...FOUR_SHOTS, shots: 0, seconds: 0, rows: [] }),
    );
    selectSomething();
    render(<CutscenePanel client={client} />);
    const toggle = await screen.findByTestId("cutscene-preview");
    expect(toggle.hasAttribute("disabled")).toBe(true);
    expect(toggle.getAttribute("title")).toMatch(/nothing to preview/i);
    fireEvent.click(toggle);
    expect(client.cinemaPreview).not.toHaveBeenCalled();
  });

  it("stops claiming the viewport the moment Play takes it", async () => {
    const client = loaded();
    selectSomething();
    render(<CutscenePanel client={client} />);
    fireEvent.click(await screen.findByTestId("cutscene-preview"));
    await waitFor(() => expect(cinemaPreviewStore.getState().active).toBe(true));

    // The shell hands a held preview back before Play captures the pre-Play camera. If the store
    // kept claiming otherwise the stage would carry a PREVIEW badge over a running game.
    act(() => playStore.getState().refresh({ playing: true, paused: false }));
    await waitFor(() => expect(cinemaPreviewStore.getState().active).toBe(false));
    act(() => playStore.getState().refresh({ playing: false, paused: false }));
  });

  it("offers the delivery frames the ENGINE publishes, and sends the one that was picked", async () => {
    // The picker is filled from `FramingCatalog.deliveries`, not from a list in this file. A second
    // list here would be free to offer a word the engine refuses — the whole reason the catalogue is
    // published by the side that validates it.
    const client = loaded();
    selectSomething();
    render(<CutscenePanel client={client} />);
    const picker = (await screen.findByTestId("cutscene-delivery")) as HTMLSelectElement;
    const catalogue = await client.cinemaFramingCatalog();
    await waitFor(() =>
      expect([...picker.options].map((o) => o.value)).toEqual(
        catalogue.deliveries.map((d) => d.value),
      ),
    );
    expect(picker.value).toBe("viewport");

    fireEvent.change(picker, { target: { value: "scope" } });
    await waitFor(() => expect(client.cinemaSetDelivery).toHaveBeenCalledWith("e1", "scope"));
  });

  describe("the frame guide", () => {
    /** A cut delivered to scope — the case a guide exists for. `viewport` is the absence of one. */
    function scopeClient() {
      const client = loaded();
      client.cinemaList = vi.fn(() => Promise.resolve({ ...FOUR_SHOTS, delivery: "scope" as const }));
      return client;
    }

    it("asks the stage to draw the frame this cut is delivered in, and names it the ENGINE's way", async () => {
      const client = scopeClient();
      selectSomething();
      render(<CutscenePanel client={client} />);
      await waitFor(() => expect(client.setFrameGuide).toHaveBeenCalledWith("scope"));
      // The badge on the stage reads the engine's own label, carried with the key — never a second
      // table of names in the front end, which would drift silently.
      const catalogue = await client.cinemaFramingCatalog();
      const label = catalogue.deliveries.find((d) => d.value === "scope")?.label;
      await waitFor(() => expect(frameGuideStore.getState().drawn).toEqual({ key: "scope", label }));
    });

    it("draws nothing for a cut delivered to the stage's own shape, and says why the control is dead", async () => {
      // NEGATIVE CONTROL for the test above: "Match viewport" IS a delivery frame in the picker, and
      // it is the one with no bars — so an implementation that guided to everything would pass there
      // and fail here.
      const client = loaded(); // FOUR_SHOTS delivers "viewport"
      selectSomething();
      render(<CutscenePanel client={client} />);
      const toggle = await screen.findByTestId("cutscene-frame-guide");
      await waitFor(() => expect(toggle.hasAttribute("disabled")).toBe(true));
      expect(toggle.getAttribute("title")).toMatch(/already is the stage/i);
      expect(client.setFrameGuide).not.toHaveBeenCalledWith("viewport");
      expect(frameGuideStore.getState().drawn).toBeNull();
    });

    it("turns the guide off from the panel, and tells the stage to stop drawing it", async () => {
      const client = scopeClient();
      selectSomething();
      render(<CutscenePanel client={client} />);
      const toggle = await screen.findByTestId("cutscene-frame-guide");
      await waitFor(() => expect(toggle.getAttribute("aria-pressed")).toBe("true"));

      fireEvent.click(toggle);
      await waitFor(() => expect(client.setFrameGuide).toHaveBeenLastCalledWith(null));
      expect(frameGuideStore.getState().drawn).toBeNull();
      expect(toggle.getAttribute("aria-pressed")).toBe("false");

      // And back on again, which is the half that proves the toggle is a toggle and not an off switch.
      fireEvent.click(toggle);
      await waitFor(() => expect(client.setFrameGuide).toHaveBeenLastCalledWith("scope"));
    });

    it("stops guiding while a preview holds the camera, because the shot itself is the frame then", async () => {
      const client = scopeClient();
      selectSomething();
      render(<CutscenePanel client={client} />);
      await waitFor(() => expect(client.setFrameGuide).toHaveBeenLastCalledWith("scope"));
      fireEvent.click(await screen.findByTestId("cutscene-preview"));
      await waitFor(() => expect(client.setFrameGuide).toHaveBeenLastCalledWith(null));
      expect(frameGuideStore.getState().drawn).toBeNull();
    });

    it("clears the guide when the panel closes, so the stage is never left letterboxed by an unmounted control", async () => {
      const client = scopeClient();
      selectSomething();
      const view = render(<CutscenePanel client={client} />);
      await waitFor(() => expect(client.setFrameGuide).toHaveBeenLastCalledWith("scope"));
      view.unmount();
      await waitFor(() => expect(client.setFrameGuide).toHaveBeenLastCalledWith(null));
      expect(frameGuideStore.getState().drawn).toBeNull();
    });
  });

  it("names the frame the previewed pose was solved for, and only when it is not the stage", async () => {
    // "Match viewport" is the ABSENCE of a delivery frame; printing it beside three coordinates would
    // be a fourth measurement of nothing.
    const client = loaded();
    selectSomething();
    render(<CutscenePanel client={client} />);
    fireEvent.click(await screen.findByTestId("cutscene-preview"));
    const pose = await screen.findByTestId("cutscene-preview-pose");
    expect(pose.textContent).not.toMatch(/composed for/i);

    client.cinemaList = vi.fn(() => Promise.resolve({ ...FOUR_SHOTS, delivery: "scope" as const }));
    act(() => projectionStore.getState().select(null));
    act(() => projectionStore.getState().select("e1"));
    fireEvent.click(await screen.findByTestId("cutscene-preview"));
    await waitFor(() =>
      expect(screen.getByTestId("cutscene-preview-pose").textContent).toMatch(/composed for.*scope/i),
    );
  });

  describe("what the shot frames", () => {
    it("offers the scene's own hierarchy, headed by the engine's words, with the parts count that tells two similar names apart", async () => {
      const client = loaded();
      selectSomething();
      render(<CutscenePanel client={client} />);
      fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[0]);

      // Closed, it reads what the shot films — no fetch needed for that, it is on the row.
      expect((await screen.findByTestId("cutscene-subject-name")).textContent).toBe("Weld Gun 7");
      fireEvent.click(screen.getByTestId("cutscene-subject"));

      // The picker asks about the shot BEING EDITED, so the engine can tick its current subject.
      await waitFor(() => expect(client.cinemaSubjectCatalog).toHaveBeenCalledWith("e1", 0, ""));
      const list = await screen.findByTestId("cutscene-subject-list");
      // The headings are the ENGINE's ranking, read back in its order — not a re-sort here.
      expect(list.textContent).toContain("This object");
      expect(list.textContent).toContain("What it is part of");
      expect(list.textContent).toContain("What it is made of");
      // The number is the decision: 46 parts is an assembly, 1 part is the thing inside it.
      expect(screen.getByTestId("cutscene-subject-option-e9").getAttribute("data-parts")).toBe("46");
      expect(screen.getByTestId("cutscene-subject-option-e9").textContent).toContain("46 parts");
      expect(screen.getByTestId("cutscene-subject-option-e2").textContent).toContain("1 part");
    });

    it("says a subject has nothing drawn under it BEFORE the shot is aimed at it", async () => {
      selectSomething();
      render(<CutscenePanel client={loaded()} />);
      fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[0]);
      fireEvent.click(await screen.findByTestId("cutscene-subject"));

      // THE SILENT FAILURE THIS EXISTS TO STOP: the solver frames a subject with no drawn geometry
      // at its own origin inside a metre-ish box, so the camera goes somewhere plausible and points
      // at nothing. It is invisible from outside — unless the picker says so first.
      const empty = await screen.findByTestId("cutscene-subject-option-e3");
      expect(empty.textContent).toContain("nothing drawn");
      expect(empty.textContent).toMatch(/composed on its origin/i);
      // And it is still OFFERED, not hidden: a marker is a legitimate thing to point a camera at,
      // and a picker that silently omitted objects would be a scene the user cannot fully reach.
      expect(empty).toBeTruthy();
    });

    it("resting on a row lights it on the stage, so a list of 15,711 names is navigable", async () => {
      vi.useFakeTimers();
      try {
        selectSomething();
        render(<CutscenePanel client={loaded()} />);
        fireEvent.click((await vi.waitFor(() => screen.findAllByTestId("cutscene-clip")))[0]);
        fireEvent.click(await vi.waitFor(() => screen.findByTestId("cutscene-subject")));
        const row = await vi.waitFor(() => screen.findByTestId("cutscene-subject-option-e9"));

        // SETTLED, not per event: sweeping the pointer down a list is one gesture, and asking the
        // engine to walk every drawn instance once per row is the stall this delay exists to stop.
        fireEvent.pointerEnter(row);
        expect(stageHighlightStore.getState().ids).toEqual([]);
        act(() => void vi.advanceTimersByTime(120));
        expect(stageHighlightStore.getState().ids).toEqual(["e9"]);
        expect(stageHighlightStore.getState().source).toBe("picker");

        // And closing the picker takes the cue with it — a closed popover still lighting a row is
        // the stage answering a question nobody is asking.
        fireEvent.keyDown(window, { key: "Escape" });
        act(() => void vi.advanceTimersByTime(120));
        expect(stageHighlightStore.getState().source).toBeNull();
      } finally {
        vi.useRealTimers();
      }
    });

    it("searches the whole scene when the ranked list does not have it", async () => {
      const client = loaded();
      selectSomething();
      render(<CutscenePanel client={client} />);
      fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[0]);
      fireEvent.click(await screen.findByTestId("cutscene-subject"));
      await screen.findByTestId("cutscene-subject-option-e1");

      fireEvent.change(screen.getByTestId("cutscene-subject-search"), { target: { value: "hall" } });
      await waitFor(() => expect(client.cinemaSubjectCatalog).toHaveBeenCalledWith("e1", 0, "hall"));
      // The search ANSWERS: one row, under the engine's "Matches" heading, and the four ranked rows
      // are gone. A picker that ignored its own box would still show all four here.
      await waitFor(() =>
        expect(screen.queryByTestId("cutscene-subject-option-e1")).toBeNull(),
      );
      expect(screen.getByTestId("cutscene-subject-option-e9")).toBeTruthy();
      expect(screen.getByTestId("cutscene-subject-list").textContent).toContain("Matches");
    });

    it("re-aims the shot through the one validated command, and only when the choice changed", async () => {
      const client = loaded();
      selectSomething();
      render(<CutscenePanel client={client} />);
      fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[0]);
      fireEvent.click(await screen.findByTestId("cutscene-subject"));

      // Picking the object it ALREADY films commits nothing. Re-writing the same subject would be an
      // undo step that changes nothing a user can see — the failure `set_mood_ops` refuses for the
      // same reason.
      fireEvent.click(await screen.findByTestId("cutscene-subject-option-e1"));
      expect(client.cinemaSetShotSubject).not.toHaveBeenCalled();

      fireEvent.click(screen.getByTestId("cutscene-subject"));
      fireEvent.click(await screen.findByTestId("cutscene-subject-option-e9"));
      await waitFor(() => expect(client.cinemaSetShotSubject).toHaveBeenCalledWith("e1", 0, "e9"));
      // One command, one undo — not a framing edit carrying a subject in a spare field.
      expect(client.cinemaSetShotFraming).not.toHaveBeenCalled();
    });

    it("offers pointing at it before naming it, and the aim names the shot that is open", async () => {
      selectSomething();
      render(<CutscenePanel client={loaded()} />);
      fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[1]);
      fireEvent.click(await screen.findByTestId("cutscene-subject"));

      // FIRST in the picker, above the search. In a 15,711-part import the list is how you reach
      // something you can NAME; the most common thing an author wants to film is the one they are
      // already looking at.
      const aim = await screen.findByTestId("cutscene-subject-aim");
      expect(aim.textContent).toContain("Click it in the viewport");

      fireEvent.click(aim);
      const state = subjectAimStore.getState();
      expect(state.active).toBe(true);
      expect(state.owner).toBe("e1");
      expect(state.shotIndex).toBe(1);
      expect(state.shots).toBe(4);
    });

    it("commits a choice made on the stage through the SAME command the list uses", async () => {
      const client = loaded();
      selectSomething();
      render(<CutscenePanel client={client} />);
      fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[2]);
      fireEvent.click(await screen.findByTestId("cutscene-subject"));
      fireEvent.click(await screen.findByTestId("cutscene-subject-aim"));

      // What the stage does when the user clicks the object: the choice, and nothing else.
      act(() => subjectAimStore.getState().pick("e9", "Assembly Hall"));

      // One command, one undo, one toast — the same path the list commits through, so pointing at
      // an object and choosing it from a list cannot drift into two different edits.
      await waitFor(() => expect(client.cinemaSetShotSubject).toHaveBeenCalledWith("e1", 2, "e9"));
      expect(client.cinemaSetShotFraming).not.toHaveBeenCalled();
      // And the choice is consumed exactly once — a second commit would be a second undo step.
      expect(subjectAimStore.getState().picked).toBeNull();
      expect(client.cinemaSetShotSubject).toHaveBeenCalledTimes(1);
    });

    it("lands on the shot the aim STARTED on, not on whichever card is open when it finishes", async () => {
      const client = loaded();
      selectSomething();
      render(<CutscenePanel client={client} />);
      const clips = await screen.findAllByTestId("cutscene-clip");
      fireEvent.click(clips[0]);
      fireEvent.click(await screen.findByTestId("cutscene-subject"));
      fireEvent.click(await screen.findByTestId("cutscene-subject-aim"));

      // Lining a camera up takes orbiting, and the shot list is right there. Opening another card
      // mid-aim must not re-frame THAT one.
      fireEvent.click(clips[3]);
      act(() => subjectAimStore.getState().pick("e9", "Assembly Hall"));

      await waitFor(() => expect(client.cinemaSetShotSubject).toHaveBeenCalledWith("e1", 0, "e9"));
    });

    it("pointing at what the shot already films says so instead of writing an empty undo step", async () => {
      const client = loaded();
      selectSomething();
      render(<CutscenePanel client={client} />);
      fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[0]);
      fireEvent.click(await screen.findByTestId("cutscene-subject"));
      fireEvent.click(await screen.findByTestId("cutscene-subject-aim"));

      act(() => subjectAimStore.getState().pick("e1", "Weld Gun 7"));

      await waitFor(() => expect(subjectAimStore.getState().picked).toBeNull());
      expect(client.cinemaSetShotSubject).not.toHaveBeenCalled();
      // NOT SILENT, THOUGH. A click that produced nothing at all reads as a click that missed.
      expect(toastStore.getState().toasts.map((t) => t.text).join(" ")).toContain(
        "already frames Weld Gun 7",
      );
    });

    it("Play takes the stage back, and an aim in flight goes with it", async () => {
      selectSomething();
      render(<CutscenePanel client={loaded()} />);
      fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[0]);
      fireEvent.click(await screen.findByTestId("cutscene-subject"));
      fireEvent.click(await screen.findByTestId("cutscene-subject-aim"));
      expect(subjectAimStore.getState().active).toBe(true);

      act(() => playStore.getState().refresh({ playing: true, paused: false }));

      // Otherwise the stage keeps intercepting clicks for an edit the engine refuses anyway, and
      // the badge stands over a running game promising a re-frame that cannot happen.
      expect(subjectAimStore.getState().active).toBe(false);
    });

    it("names what a clip films on the lane, and only when it is not the cutscene's own object", async () => {
      const client = loaded();
      client.cinemaList = vi.fn(() =>
        Promise.resolve({
          ...FOUR_SHOTS,
          rows: [
            // An establishing wide of the whole assembly, then the ordinary shots of the owner.
            { ...FOUR_SHOTS.rows[0], subject: "e9", subjectName: "Assembly Hall" },
            ...FOUR_SHOTS.rows.slice(1),
          ],
        }),
      );
      selectSomething();
      render(<CutscenePanel client={client} />);
      const clips = await screen.findAllByTestId("cutscene-clip");
      // The one that differs says so; the three that do not are not captioned four times with the
      // name in the heading above them.
      expect(clips[0].textContent).toContain("Assembly Hall");
      expect(clips[1].textContent).not.toContain("Weld Gun 7");
      expect(clips[2].textContent).not.toContain("Weld Gun 7");
    });

    it("opens the shot a card just added, so aiming it is the next click and not a hunt", async () => {
      const client = fakeClient();
      // The list AGREES with what the add returns, because it is the same cutscene read twice — the
      // panel re-reads after every commit, and a fixture whose two answers disagree would close the
      // editor the add had just opened for a reason no engine produces.
      const added = await client.cinemaAddShot("e1", "establish");
      client.cinemaList = vi.fn(() => Promise.resolve(added));
      selectSomething();
      render(<CutscenePanel client={client} />);
      await screen.findAllByTestId("cutscene-clip");
      // Nothing is open until a shot is chosen — the panel says so rather than guessing.
      expect(screen.queryByTestId("cutscene-shot-editor")).toBeNull();

      fireEvent.click(screen.getByTestId("shot-establish"));
      const editor = await screen.findByTestId("cutscene-shot-editor");
      expect(editor.textContent).toMatch(/Shot 1 of 1/);
      // …and the aiming control is right there, which is the whole reason the shot opens.
      expect(screen.getByTestId("cutscene-subject")).toBeTruthy();
    });

    it("refuses to re-aim during Play, and says why", async () => {
      selectSomething();
      render(<CutscenePanel client={loaded()} />);
      fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[0]);
      act(() => playStore.getState().refresh({ playing: true, paused: false }));
      const trigger = await screen.findByTestId("cutscene-subject");
      await waitFor(() => expect(trigger.hasAttribute("disabled")).toBe(true));
      expect(trigger.getAttribute("title")).toMatch(/Stop Play first/i);
    });
  });

  // -----------------------------------------------------------------------------------------------
  // ADR-192 — a camera the author placed. The card vocabulary could not express "shoot from here",
  // which is the most basic gesture there is.
  // -----------------------------------------------------------------------------------------------
  describe("shooting from the view on the stage", () => {
    const PLACED = {
      eye: [7.4, 2.9, -5.1] as [number, number, number],
      lookAt: [0.2, 1.35, 0.4] as [number, number, number],
      fovDeg: 55,
      track: null,
    };

    /** The same four shots with the opener placed — what the engine sends back after the gesture. */
    const withPlacedOpener: CinemaReply = {
      ...FOUR_SHOTS,
      rows: FOUR_SHOTS.rows.map((r) =>
        r.index === 0
          ? { ...r, camera: PLACED, reads: "a placed shot of Weld Gun 7, pulling out — 2.0s" }
          : r,
      ),
    };

    it("sends NO POSE — the engine reads its own camera, so the stored shot cannot be a view it never stood at", async () => {
      const client = loaded();
      client.cinemaSetShotCamera = vi.fn(() => Promise.resolve({ ...withPlacedOpener, entity: "e1", message: "Shot 1 is now a placed shot of Weld Gun 7, pulling out — 2.0s" }));
      selectSomething();
      render(<CutscenePanel client={client} />);
      fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[0]);
      fireEvent.click(await screen.findByTestId("cutscene-shoot-here"));
      await waitFor(() => expect(client.cinemaSetShotCamera).toHaveBeenCalledWith("e1", 0));
      // Two arguments and no third: an editor that sent coordinates would be a second opinion about
      // where the camera is.
      expect(vi.mocked(client.cinemaSetShotCamera).mock.calls[0]).toHaveLength(2);
    });

    it("puts the DELIVERY-FRAMED result on the stage at the gesture, not a toast about it", async () => {
      const client = loaded();
      client.cinemaSetShotCamera = vi.fn(() => Promise.resolve({ ...withPlacedOpener, entity: "e1", message: "placed" }));
      selectSomething();
      render(<CutscenePanel client={client} />);
      fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[0]);
      fireEvent.click(await screen.findByTestId("cutscene-shoot-here"));
      // The author was ORBITING, not previewing, so nothing would have moved without this: the panel
      // starts the preview itself, at the instant that shot is on screen alone.
      await waitFor(() =>
        expect(client.cinemaPreview).toHaveBeenCalledWith("e1", withPlacedOpener.rows[0].openSeconds, true),
      );
    });

    it("stops the size and the angle deciding anything, and says why in plain words", async () => {
      const client = loaded();
      client.cinemaList = vi.fn(() => Promise.resolve(withPlacedOpener));
      selectSomething();
      render(<CutscenePanel client={client} />);
      fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[0]);
      const size = (await screen.findByTestId("cutscene-size")) as HTMLSelectElement;
      const angle = screen.getByTestId("cutscene-angle") as HTMLSelectElement;
      expect(size.disabled).toBe(true);
      expect(angle.disabled).toBe(true);
      expect(size.getAttribute("title")).toMatch(/camera you placed/i);
      // NOT CLEARED. They are the framing "Use the card again" restores.
      expect(size.value).toBe("wide");
      expect(angle.value).toBe("three_quarter");
      // ...and the two controls a placed camera does NOT take over stay live, which is the whole
      // difference between a first-class shot and an escape hatch.
      expect((screen.getByTestId("cutscene-motion") as HTMLSelectElement).disabled).toBe(false);
      expect((screen.getByTestId("cutscene-amount") as HTMLInputElement).disabled).toBe(false);
    });

    it("captions the shot from the pose, never from the card it no longer uses", async () => {
      const client = loaded();
      client.cinemaList = vi.fn(() => Promise.resolve(withPlacedOpener));
      selectSomething();
      render(<CutscenePanel client={client} />);
      fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[0]);
      expect((await screen.findByTestId("cutscene-shot-reads")).textContent).toMatch(/a placed shot of/);
      // The lane too: "Wide pulling out" over a hand-framed shot is the same untruth one line up.
      expect((await screen.findAllByTestId("cutscene-clip"))[0].textContent).toMatch(/Placed/);
      // The numbers, not a badge — the author reads them against the preview's own read-out.
      expect(screen.getByTestId("cutscene-placed-pose").textContent).toMatch(/7\.40, 2\.90, -5\.10/);
      expect(screen.getByTestId("cutscene-placed-pose").textContent).toMatch(/55°/);
    });

    it("gives the shot back to its card through the one command, and offers that only when there is one", async () => {
      const client = loaded();
      client.cinemaList = vi.fn(() => Promise.resolve(withPlacedOpener));
      client.cinemaClearShotCamera = vi.fn(() => Promise.resolve({ ...FOUR_SHOTS, entity: "e1", message: "back on its card" }));
      selectSomething();
      render(<CutscenePanel client={client} />);
      const clips = await screen.findAllByTestId("cutscene-clip");
      fireEvent.click(clips[0]);
      fireEvent.click(await screen.findByTestId("cutscene-use-card"));
      await waitFor(() => expect(client.cinemaClearShotCamera).toHaveBeenCalledWith("e1", 0));
      // Shot 2 is an ordinary card shot: there is nothing to give back, so the control is not there
      // at all rather than sitting disabled with nothing to say.
      fireEvent.click(clips[1]);
      await waitFor(() => expect(screen.getByTestId("cutscene-shot-editor").textContent).toMatch(/Shot 2 of 4/));
      expect(screen.queryByTestId("cutscene-use-card")).toBeNull();
      expect(screen.queryByTestId("cutscene-placed-pose")).toBeNull();
    });

    it("refuses to re-place a camera during Play, and says why", async () => {
      selectSomething();
      render(<CutscenePanel client={loaded()} />);
      fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[0]);
      act(() => playStore.getState().refresh({ playing: true, paused: false }));
      const shoot = await screen.findByTestId("cutscene-shoot-here");
      await waitFor(() => expect(shoot.hasAttribute("disabled")).toBe(true));
      expect(shoot.getAttribute("title")).toMatch(/Stop Play first/i);
    });

    // ---------------------------------------------------------------------------------------------
    // ADR-195 — KEEP THE SUBJECT FRAMED. The one thing a placed camera could not do.
    // ---------------------------------------------------------------------------------------------

    const withFollowingOpener: CinemaReply = {
      ...FOUR_SHOTS,
      rows: FOUR_SHOTS.rows.map((r) =>
        r.index === 0
          ? {
              ...r,
              camera: { ...PLACED, track: [0.2, 1.35, 0.4] as [number, number, number] },
              reads: "a placed shot of Weld Gun 7, pulling out, keeping it framed — 2.0s",
            }
          : r,
      ),
    };

    it("sends a BOOLEAN, not an offset — the engine is the only side that knows where the subject is", async () => {
      const client = loaded();
      client.cinemaList = vi.fn(() => Promise.resolve(withPlacedOpener));
      client.cinemaSetShotTracking = vi.fn(() => Promise.resolve({ ...withFollowingOpener, entity: "e1", message: "keeps it framed" }));
      selectSomething();
      render(<CutscenePanel client={client} />);
      fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[0]);
      fireEvent.click(await screen.findByTestId("cutscene-keep-framed"));
      await waitFor(() => expect(client.cinemaSetShotTracking).toHaveBeenCalledWith("e1", 0, true));
      expect(vi.mocked(client.cinemaSetShotTracking).mock.calls[0]).toHaveLength(3);
    });

    it("is a toggle: pressed while the head is on, and pressing it again locks the camera off", async () => {
      const client = loaded();
      client.cinemaList = vi.fn(() => Promise.resolve(withFollowingOpener));
      client.cinemaSetShotTracking = vi.fn(() => Promise.resolve({ ...withPlacedOpener, entity: "e1", message: "locked off" }));
      selectSomething();
      render(<CutscenePanel client={client} />);
      fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[0]);
      const toggle = await screen.findByTestId("cutscene-keep-framed");
      expect(toggle.getAttribute("aria-pressed")).toBe("true");
      fireEvent.click(toggle);
      await waitFor(() => expect(client.cinemaSetShotTracking).toHaveBeenCalledWith("e1", 0, false));
    });

    it("names the object it follows, on the control and in the read-out", async () => {
      const client = loaded();
      client.cinemaList = vi.fn(() => Promise.resolve(withFollowingOpener));
      selectSomething();
      render(<CutscenePanel client={client} />);
      fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[0]);
      const subject = withFollowingOpener.rows[0].subjectName;
      expect((await screen.findByTestId("cutscene-keep-framed")).textContent).toContain(subject);
      // AND `lookAt` IS GONE from the read-out. It is the framing the offset was taken from, not a
      // point the camera still uses, and printing it as "looking at" would be a coordinate that
      // stops being true the moment the subject moves.
      const pose = screen.getByTestId("cutscene-placed-pose");
      expect(pose.textContent).toMatch(/aim follows/);
      expect(pose.textContent).not.toMatch(/looking at/);
      expect(pose.textContent).toMatch(/7\.40, 2\.90, -5\.10/);
    });

    it("offers the head only where there is a camera to put it on", async () => {
      const client = loaded();
      client.cinemaList = vi.fn(() => Promise.resolve(withPlacedOpener));
      selectSomething();
      render(<CutscenePanel client={client} />);
      const clips = await screen.findAllByTestId("cutscene-clip");
      fireEvent.click(clips[0]);
      expect(await screen.findByTestId("cutscene-keep-framed")).toBeTruthy();
      // Shot 2 films from its card, which already re-solves around its subject every tick.
      fireEvent.click(clips[1]);
      await waitFor(() => expect(screen.getByTestId("cutscene-shot-editor").textContent).toMatch(/Shot 2 of 4/));
      expect(screen.queryByTestId("cutscene-keep-framed")).toBeNull();
    });

    it("puts the result on the stage, because a head that turns is invisible on a still frame", async () => {
      const client = loaded();
      client.cinemaList = vi.fn(() => Promise.resolve(withPlacedOpener));
      client.cinemaSetShotTracking = vi.fn(() => Promise.resolve({ ...withFollowingOpener, entity: "e1", message: "keeps it framed" }));
      selectSomething();
      render(<CutscenePanel client={client} />);
      fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[0]);
      fireEvent.click(await screen.findByTestId("cutscene-keep-framed"));
      await waitFor(() =>
        expect(client.cinemaPreview).toHaveBeenCalledWith("e1", withFollowingOpener.rows[0].openSeconds, true),
      );
    });

    it("refuses during Play, with the same reason every other authoring control gives", async () => {
      const client = loaded();
      client.cinemaList = vi.fn(() => Promise.resolve(withPlacedOpener));
      selectSomething();
      render(<CutscenePanel client={client} />);
      fireEvent.click((await screen.findAllByTestId("cutscene-clip"))[0]);
      act(() => playStore.getState().refresh({ playing: true, paused: false }));
      const toggle = await screen.findByTestId("cutscene-keep-framed");
      await waitFor(() => expect(toggle.hasAttribute("disabled")).toBe(true));
      expect(toggle.getAttribute("title")).toMatch(/Stop Play first/i);
    });
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
