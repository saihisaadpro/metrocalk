//! ADR-175 — the render dialog: the cost is the ENGINE's, the progress is real, the ledger names the
//! folder.
//!
//! Every assertion here is about a claim the surface makes on the author's behalf. The frame count is
//! the one that matters most: it is stated above a button that writes that many files, so a dialog
//! that computed it locally could be confidently wrong — which is why the number is asserted to come
//! from `cinemaRenderPlan` and to change when the choice does.

import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { RenderDialog } from "./RenderDialog";
import { fakeClient, resetTestClientRender } from "../transport/test-client";
import { DEFAULT_RENDER_SETTINGS } from "../transport/protocol";
import type { CinemaReply, ShotRow } from "../transport/protocol";

function row(index: number, startSeconds: number, seconds: number): ShotRow {
  return {
    id: `s${index}`,
    index,
    reads: `shot ${index + 1}`,
    seconds,
    effectiveSeconds: seconds,
    startSeconds,
    openSeconds: startSeconds,
    blendSeconds: 0,
    size: "full",
    angle: "three_quarter",
    motion: "push_in",
    amount: 0.35,
    subject: "e1",
    subjectName: "Weld Gun 7",
  };
}

const CUT: CinemaReply = {
  entity: "e1",
  shots: 3,
  seconds: 12.5,
  mood: "normal",
  delivery: "scope",
  render: DEFAULT_RENDER_SETTINGS,
  reads: [],
  rows: [row(0, 0, 5), row(1, 5, 5), row(2, 10, 2.5)],
  problems: [],
  message: "",
  reason: null,
};

function open(over: Partial<Parameters<typeof RenderDialog>[0]> = {}) {
  const client = fakeClient();
  const onClose = vi.fn();
  const onSettingsSaved = vi.fn();
  render(
    <RenderDialog
      open
      onClose={onClose}
      client={client}
      entity="e1"
      name="Skid Weld Line"
      cut={CUT}
      activeShotIndex={1}
      deliveryLabel="2.39:1 scope"
      onSettingsSaved={onSettingsSaved}
      {...over}
    />,
  );
  return { client, onClose, onSettingsSaved };
}

describe("RenderDialog", () => {
  beforeEach(() => {
    resetTestClientRender();
  });

  it("states the frame count the ENGINE planned, not one it computed itself", async () => {
    const { client } = open();
    // The dialog asks; it does not multiply. The arguments matter as much as the answer: a dialog
    // that asked about the whole cut and then rendered one shot would show an honest number about
    // the wrong thing. `null` is the whole cut, which is what it opens on even with a shot selected.
    await waitFor(() => expect(client.cinemaRenderPlan).toHaveBeenCalledWith("e1", 24, null, 1080, "movie"));
    // 12.5s of cut at 24 fps — the fixture's own arithmetic, echoed onto the button that pays it.
    await waitFor(() =>
      expect(screen.getByTestId("render-start").textContent).toMatch(/Render 300 frames/),
    );
    expect(screen.getByTestId("render-cost").textContent).toMatch(/300 frames/);
  });

  it("re-asks the engine when the rate changes, and the button says the new number", async () => {
    const { client } = open();
    await waitFor(() =>
      expect(screen.getByTestId("render-start").textContent).toMatch(/Render 300 frames/),
    );
    fireEvent.change(screen.getByTestId("render-fps"), { target: { value: "60" } });
    await waitFor(() => expect(client.cinemaRenderPlan).toHaveBeenCalledWith("e1", 60, null, 1080, "movie"));
    await waitFor(() =>
      expect(screen.getByTestId("render-start").textContent).toMatch(/Render 750 frames/),
    );
  });

  it("narrows to one shot when the scope says so, and renders THAT one", async () => {
    const { client } = open();
    await waitFor(() => expect(client.cinemaRenderPlan).toHaveBeenCalledWith("e1", 24, null, 1080, "movie"));
    fireEvent.change(screen.getByTestId("render-scope"), { target: { value: "shot" } });
    await waitFor(() => expect(client.cinemaRenderPlan).toHaveBeenCalledWith("e1", 24, 1, 1080, "movie"));
    fireEvent.click(await screen.findByTestId("render-start"));
    await waitFor(() =>
      expect(client.cinemaRenderStart).toHaveBeenCalledWith("e1", 24, 1, "Skid Weld Line", null, 1080, "movie"),
    );
  });

  it("offers only the whole cut when no shot is open", async () => {
    open({ activeShotIndex: null });
    const scope = (await screen.findByTestId("render-scope")) as HTMLSelectElement;
    expect(Array.from(scope.options).map((o) => o.value)).toEqual(["cut"]);
  });

  it("shows real progress while it runs, and stopping keeps what was written", async () => {
    const { client } = open();
    // The fixture plans 24 frames for one shot and writes 20 per status call, so the first poll is a
    // partial bar — which is the only state that proves the bar is a REPORT and not an animation.
    fireEvent.change(await screen.findByTestId("render-scope"), { target: { value: "shot" } });
    fireEvent.click(await screen.findByTestId("render-start"));
    const bar = () => screen.getByLabelText("Render progress") as HTMLProgressElement;
    await waitFor(() => expect(bar().value).toBe(0));
    await waitFor(() => expect(client.cinemaRenderStatus).toHaveBeenCalled());
    await waitFor(() => expect(bar().value).toBeCloseTo(20 / 24, 3));
    fireEvent.click(screen.getByTestId("render-stop"));
    await waitFor(() => expect(client.cinemaRenderCancel).toHaveBeenCalled());
    // The frames already written are KEPT, and the ledger says how many. Deleting an author's files
    // because they changed their mind is a bigger surprise than leaving them.
    expect(screen.getByTestId("render-ledger-frames").textContent).toContain("20");
  });

  it("ends on a ledger that names the folder and the size, not a toast", async () => {
    open();
    fireEvent.click(await screen.findByTestId("render-start"));
    const ledger = await screen.findByTestId("render-ledger", undefined, { timeout: 4000 });
    await waitFor(() => expect(screen.getByTestId("render-ledger-frames").textContent).toContain("60"));
    // 1080 x 2.39 = 2582 wide — the size the dialog ASKED for, not the size of the stage. Before
    // ADR-177 this assertion read 1920x803, which was the window.
    expect(ledger.textContent).toMatch(/2582×1080/);
    expect(screen.getByTestId("render-ledger-folder").textContent).toContain("C:/renders/skid-weld-line");
    // The options are GONE. A finished render that left the form above its result would be asking a
    // question the reader has stopped having.
    expect(screen.queryByTestId("render-fps")).toBeNull();
  });

  it("refuses in a sentence, on the control, rather than opening a dialog that cannot act", async () => {
    const client = fakeClient();
    client.cinemaRenderPlan = vi.fn(() =>
      Promise.resolve({
        running: false,
        done: false,
        entity: "e1",
        frames: 0,
        written: 0,
        width: 0,
        height: 0,
        offscreen: false,
        format: "movie" as const,
        bitrate: 0,
        fps: 24,
        seconds: 0,
        folder: "",
        stem: "",
        bytes: 0,
        elapsedMs: 0,
        failures: [],
        message: "There is nothing to render — this object has no shots yet.",
        reason: "There is nothing to render — this object has no shots yet.",
      }),
    );
    render(
      <RenderDialog
        open
        onClose={vi.fn()}
        client={client}
        entity="e1"
        name="Skid Weld Line"
        cut={{ ...CUT, shots: 0, seconds: 0, rows: [] }}
        activeShotIndex={null}
        deliveryLabel="Match viewport"
        onSettingsSaved={vi.fn()}
      />,
    );
    const start = await screen.findByTestId("render-start");
    await waitFor(() => expect(start.hasAttribute("disabled")).toBe(true));
    expect(screen.getByTestId("render-cost").textContent).toContain("no shots yet");
  });

  it("reopening onto a render already in flight shows THAT render, not a button it would refuse", async () => {
    // The job lives on the engine, so closing the dialog does not stop it. A reopen that always
    // started fresh would offer to start a second one and be refused — an enabled control that cannot
    // act. A FINISHED job is not adopted: its ledger belongs to the moment it was read.
    const client = fakeClient();
    await client.cinemaRenderStart("e1", 24, null, "Skid Weld Line");
    render(
      <RenderDialog
        open
        onClose={vi.fn()}
        client={client}
        entity="e1"
        name="Skid Weld Line"
        cut={CUT}
        activeShotIndex={null}
        deliveryLabel="2.39:1 scope"
        onSettingsSaved={vi.fn()}
      />,
    );
    await waitFor(() => expect(screen.getByLabelText("Render progress")).toBeTruthy());
    expect(screen.queryByTestId("render-start")).toBeNull();
    // ...and the author is not held hostage by it: the dialog can be put away while it runs.
    expect(screen.getByTestId("render-hide")).toBeTruthy();
  });

  // ── ADR-177: the size the files are written at ────────────────────────────────────────────────

  it("does not offer a movie a size a movie cannot have, and moves off it rather than refusing", async () => {
    // ADR-182 — the pair the engine refuses is not a pair this dialog can be left holding. Leaving it
    // there and explaining afterwards is worse than not offering it: the author would have to read a
    // sentence and then undo the thing they just did.
    const { client } = open();
    fireEvent.change(await screen.findByTestId("render-format"), { target: { value: "sequence" } });
    const size = () => screen.getByTestId("render-size") as HTMLSelectElement;
    expect(Array.from(size().options).map((o) => o.value)).toContain("viewport");
    fireEvent.change(size(), { target: { value: "viewport" } });
    await waitFor(() => expect(size().value).toBe("viewport"));
    fireEvent.change(screen.getByTestId("render-format"), { target: { value: "movie" } });
    // The option is GONE, and the picker has landed on the default height rather than on nothing.
    await waitFor(() => expect(size().value).toBe("1080"));
    expect(Array.from(size().options).map((o) => o.value)).not.toContain("viewport");
    await waitFor(() => expect(client.cinemaRenderPlan).toHaveBeenCalledWith("e1", 24, null, 1080, "movie"));
  });

  it("opens on a delivery size rather than on whatever the window happens to be", async () => {
    // THE DEFAULT IS THE POINT. This dialog shipped writing every sequence at the size of the stage,
    // which on a laptop with both docks open is around 400 lines — a film nobody can use, produced by
    // a dialog that never asked. The offer it opens on is now a delivery format.
    const { client } = open();
    const size = (await screen.findByTestId("render-size")) as HTMLSelectElement;
    expect(size.value).toBe("1080");
    // ...and "the stage" is still there for a sequence, because it is the right answer for a quick
    // look. It is not offered for a movie — see the test above.
    expect(Array.from(size.options).map((o) => o.value)).toEqual(["720", "1080", "1440", "2160"]);
    fireEvent.change(screen.getByTestId("render-format"), { target: { value: "sequence" } });
    await waitFor(() =>
      expect(
        Array.from((screen.getByTestId("render-size") as HTMLSelectElement).options).map((o) => o.value),
      ).toEqual(["viewport", "720", "1080", "1440", "2160"]),
    );
    await waitFor(() => expect(client.cinemaRenderPlan).toHaveBeenCalledWith("e1", 24, null, 1080, "movie"));
  });

  it("re-asks the engine when the size changes, and states the pixels it answered", async () => {
    // The width is NEVER computed here. 1440 x 2.39 = 3442, and the only way this string can appear
    // is the engine having been asked and its answer rendered — which is what makes the size above the
    // button the size in the file. (2160 is the case ADR-182 turned into a refusal for a movie; it has
    // its own test below, and this one stays about the ARITHMETIC.)
    const { client } = open();
    await waitFor(() =>
      expect(screen.getByTestId("render-cost").textContent).toMatch(/2582x1080/),
    );
    fireEvent.change(screen.getByTestId("render-size"), { target: { value: "1440" } });
    await waitFor(() => expect(client.cinemaRenderPlan).toHaveBeenCalledWith("e1", 24, null, 1440, "movie"));
    await waitFor(() =>
      expect(screen.getByTestId("render-cost").textContent).toMatch(/3442x1440/),
    );
    // The frame COUNT is unchanged by a size — a render is the same film at a different resolution,
    // and a dialog that re-planned the frames would be saying otherwise.
    expect(screen.getByTestId("render-start").textContent).toMatch(/Render 300 frames/);
  });

  it("renders at the size that was chosen, and says so while it runs", async () => {
    const { client } = open();
    fireEvent.change(await screen.findByTestId("render-size"), { target: { value: "720" } });
    await waitFor(() => expect(client.cinemaRenderPlan).toHaveBeenCalledWith("e1", 24, null, 720, "movie"));
    fireEvent.click(screen.getByTestId("render-start"));
    await waitFor(() =>
      expect(client.cinemaRenderStart).toHaveBeenCalledWith(
        "e1",
        24,
        null,
        "Skid Weld Line",
        null,
        720,
        "movie",
      ),
    );
    // A size the renderer was GIVEN is known before the first frame, so the progress line carries it
    // from the start rather than reading `0x0` until a capture returns.
    const progress = await screen.findByTestId("render-progress");
    await waitFor(() => expect(progress.textContent).toMatch(/1722×720/));
    // ...and the advice matches what is true of THIS render: an offscreen one does not need the
    // window in front, and telling the author otherwise is a false warning.
    expect(progress.textContent).toContain("covered or minimised");
  });

  it("keeps the old warning for a render that really does read the window", async () => {
    // NEGATIVE CONTROL for the sentence above. "As on screen" is a swapchain capture: a minimised
    // window genuinely stops producing frames, and the render genuinely stalls on it. It is a
    // SEQUENCE-only size since ADR-182 — a movie's frame size is written once, before the first
    // sample, and the stage's is a measurement that moves — so the delivery is chosen first.
    const { client } = open();
    fireEvent.change(await screen.findByTestId("render-format"), { target: { value: "sequence" } });
    fireEvent.change(await screen.findByTestId("render-size"), { target: { value: "viewport" } });
    await waitFor(() => expect(client.cinemaRenderPlan).toHaveBeenCalledWith("e1", 24, null, null, "sequence"));
    fireEvent.click(screen.getByTestId("render-start"));
    const progress = await screen.findByTestId("render-progress");
    await waitFor(() => expect(progress.textContent).toContain("minimised window produces no frames"));
    expect(progress.textContent).not.toContain("covered or minimised");
  });

  // ── ADR-182: the sequence becomes a movie ─────────────────────────────────────────────────────

  it("opens on a MOVIE, and the price of one is stated above the button that pays it", async () => {
    // THE DEFAULT IS THE DELIVERABLE. Before this the answer to "render my cut" was 300 numbered PNGs
    // and an unstated instruction to go and find `ffmpeg`.
    const { client } = open();
    const format = (await screen.findByTestId("render-format")) as HTMLSelectElement;
    expect(format.value).toBe("movie");
    expect(Array.from(format.options).map((o) => o.value)).toEqual(["movie", "sequence"]);
    await waitFor(() => expect(client.cinemaRenderPlan).toHaveBeenCalledWith("e1", 24, null, 1080, "movie"));
    // 2582 x 1080 x 24 x 0.12 bits = 8.0 Mbit/s. The number is the ENGINE's — the dialog multiplies
    // nothing — and it is beside the frame count, not after the render.
    await waitFor(() =>
      expect(screen.getByTestId("render-cost").textContent).toMatch(/One H\.264 MP4 at about 8\.0 Mbit\/s/),
    );
    // …and the name field says what the file will be called, because that changed with the delivery.
    expect(screen.getByTestId("render-dialog").textContent).toContain("name.mp4");
  });

  it("switches to the lossless sequence, and every sentence about the output changes with it", async () => {
    const { client } = open();
    await waitFor(() => expect(screen.getByTestId("render-cost").textContent).toMatch(/MP4/));
    fireEvent.change(screen.getByTestId("render-format"), { target: { value: "sequence" } });
    // RE-ASKED, not re-labelled: a movie and a sequence are the same frames, but only one of them has
    // a bit rate and only one of them has an encoder ceiling, so the engine has to answer again.
    await waitFor(() =>
      expect(client.cinemaRenderPlan).toHaveBeenCalledWith("e1", 24, null, 1080, "sequence"),
    );
    await waitFor(() =>
      expect(screen.getByTestId("render-cost").textContent).toMatch(/Lossless PNG, one file per frame/),
    );
    expect(screen.getByTestId("render-cost").textContent).not.toMatch(/Mbit/);
    expect(screen.getByTestId("render-dialog").textContent).toContain("name.0000.png");
    fireEvent.click(screen.getByTestId("render-start"));
    await waitFor(() =>
      expect(client.cinemaRenderStart).toHaveBeenCalledWith(
        "e1",
        24,
        null,
        "Skid Weld Line",
        null,
        1080,
        "sequence",
      ),
    );
  });

  it("refuses a movie the encoder cannot make, naming both controls that can change it", async () => {
    // 2160 lines at 2.39:1 is 5162 wide, which no H.264 encoder takes. The refusal has to arrive
    // BEFORE the click — a render that drew 300 correct frames and then could not close its container
    // would have spent four minutes to say this — and it has to name a way out, because "unsupported"
    // leaves the author with two pickers and no idea which one is the problem.
    const { client } = open();
    await waitFor(() => expect(screen.getByTestId("render-start").textContent).toMatch(/300 frames/));
    fireEvent.change(screen.getByTestId("render-size"), { target: { value: "2160" } });
    await waitFor(() => expect(client.cinemaRenderPlan).toHaveBeenCalledWith("e1", 24, null, 2160, "movie"));
    const cost = await screen.findByTestId("render-cost");
    await waitFor(() => expect(cost.textContent).toMatch(/5162 x 2160/));
    expect(cost.textContent).toMatch(/shorter height/);
    expect(cost.textContent).toMatch(/PNG sequence/);
    // …and the button that cannot act SAYS it cannot, rather than starting a render that fails.
    expect((screen.getByTestId("render-start") as HTMLButtonElement).disabled).toBe(true);

    // THE WAY OUT WORKS, and it is one control away: the same size as a sequence is not refused,
    // because the ceiling belongs to the encoder and a lossless frame has none over it.
    fireEvent.change(screen.getByTestId("render-format"), { target: { value: "sequence" } });
    await waitFor(() =>
      expect(screen.getByTestId("render-cost").textContent).toMatch(/5162x2160/),
    );
    expect((screen.getByTestId("render-start") as HTMLButtonElement).disabled).toBe(false);
  });

  it("ends on a ledger that names ONE file, because that is what a movie is", async () => {
    open();
    fireEvent.click(await screen.findByTestId("render-start"));
    await screen.findByTestId("render-ledger", undefined, { timeout: 4000 });
    await waitFor(() =>
      expect(screen.getByTestId("render-ledger-files").textContent).toBe("Skid Weld Line.mp4"),
    );
    // NOT the numbered range. `take.0000.png … take.0059.png` printed over a folder holding one
    // `take.mp4` is the dialog describing a delivery it did not make.
    expect(screen.getByTestId("render-ledger-files").textContent).not.toMatch(/0000/);
    expect(screen.getByTestId("render-ledger").textContent).toMatch(/encoded as one H\.264 movie/);
  });

  it("names the frame it is composed for, because that is what decides the shape of the files", async () => {
    open({ deliveryLabel: "2.39:1 scope" });
    expect((await screen.findByTestId("render-dialog")).textContent).toContain("2.39:1 scope");
  });
  // ── ADR-190: the four answers live on the cutscene ────────────────────────────────────────────

  it("opens on what the CUTSCENE says it delivers, not on four constants", async () => {
    // The whole capability, from the reading side. Before this pass these four controls were seeded
    // from `useState(24)`, `useState("1080")`, `useState("movie")` and `useState(name)` on every open
    // — so a person who had decided their cut delivers a 720 sequence called `weld-master` was shown
    // somebody else's answer every single time.
    open({
      cut: {
        ...CUT,
        render: { format: "sequence", fps: 60, height: 720, name: "weld-master", folder: "" },
      },
    });
    await screen.findByTestId("render-format");
    expect((screen.getByTestId("render-format") as HTMLSelectElement).value).toBe("sequence");
    expect((screen.getByTestId("render-fps") as HTMLSelectElement).value).toBe("60");
    expect((screen.getByTestId("render-size") as HTMLSelectElement).value).toBe("720");
    expect((screen.getByTestId("render-stem") as HTMLInputElement).value).toBe("weld-master");
  });

  it("plans and renders with the stored answers, so the file is the one the cut asked for", async () => {
    // A dialog that DISPLAYED the stored settings and then planned with the defaults would pass the
    // test above and deliver the wrong film. The engine's own plan call is the assertion.
    const { client } = open({
      cut: {
        ...CUT,
        render: { format: "sequence", fps: 30, height: 1440, name: "weld-master", folder: "" },
      },
    });
    await waitFor(() =>
      expect(client.cinemaRenderPlan).toHaveBeenCalledWith("e1", 30, null, 1440, "sequence"),
    );
    fireEvent.click(await screen.findByTestId("render-start"));
    await waitFor(() =>
      expect(client.cinemaRenderStart).toHaveBeenCalledWith("e1", 30, null, "weld-master", null, 1440, "sequence"),
    );
  });

  it("writes a changed picker straight to the document, as the whole block", async () => {
    // THE WHOLE BLOCK AND NOT ONE FIELD: the engine validates `(format, height)` together — a movie
    // cannot be "as on screen" — so a setter that took one answer would ask it to judge a pair half
    // of which it had to read back out of the document.
    const { client, onSettingsSaved } = open();
    await screen.findByTestId("render-fps");
    fireEvent.change(screen.getByTestId("render-fps"), { target: { value: "30" } });
    await waitFor(() =>
      expect(client.cinemaSetRender).toHaveBeenCalledWith("e1", "movie", 30, 1080, "", ""),
    );
    // ...and the panel that OWNS the cut is handed the reply, which is what makes the next open show
    // it rather than the value it was opened on.
    await waitFor(() => expect(onSettingsSaved).toHaveBeenCalled());
    expect(onSettingsSaved.mock.calls[0][0].render.fps).toBe(30);
  });

  it("commits the name on blur rather than on every keystroke", async () => {
    // A `<select>` change IS the decision. A text field's every keystroke is not — committing each
    // one would put nine undoable entries and nine round trips behind one nine-letter name.
    const { client } = open();
    const stem = (await screen.findByTestId("render-stem")) as HTMLInputElement;
    fireEvent.change(stem, { target: { value: "weld" } });
    fireEvent.change(stem, { target: { value: "weld-master" } });
    expect(client.cinemaSetRender).not.toHaveBeenCalled();
    fireEvent.blur(stem);
    await waitFor(() =>
      expect(client.cinemaSetRender).toHaveBeenCalledWith("e1", "movie", 24, 1080, "weld-master", ""),
    );
    expect(client.cinemaSetRender).toHaveBeenCalledTimes(1);
  });

  it("leaves the name field empty and shows the object's name as the placeholder", async () => {
    // EMPTY IS A REAL ANSWER — it means "call it after the object" — so the field is seeded from the
    // stored name and the object's name is the PLACEHOLDER. Seeding the value with the object's name
    // would freeze that name into the document the first time anybody touched the field, and renaming
    // the assembly would then leave last month's name on every future file.
    const { client } = open();
    const stem = (await screen.findByTestId("render-stem")) as HTMLInputElement;
    expect(stem.value).toBe("");
    expect(stem.placeholder).toBe("Skid Weld Line");
    // ...and the render still lands on the object's name, resolved the way the engine resolves it.
    fireEvent.click(await screen.findByTestId("render-start"));
    await waitFor(() =>
      expect(client.cinemaRenderStart).toHaveBeenCalledWith("e1", 24, null, "Skid Weld Line", null, 1080, "movie"),
    );
  });

  it("moves off 'as on screen' in the SAME commit that chooses a movie", async () => {
    // ADR-182's coercion, now that the pair is stored. A movie declares its frame size once, before
    // its first sample, so "as on screen" is not a size it can have and the engine refuses the pair.
    // Writing it and then correcting it would leave one refusal in the author's way for no reason.
    const { client } = open({
      cut: { ...CUT, render: { format: "sequence", fps: 24, height: null, name: "", folder: "" } },
    });
    expect((await screen.findByTestId("render-size") as HTMLSelectElement).value).toBe("viewport");
    fireEvent.change(screen.getByTestId("render-format"), { target: { value: "movie" } });
    await waitFor(() =>
      expect(client.cinemaSetRender).toHaveBeenCalledWith("e1", "movie", 24, 1080, "", ""),
    );
    expect(client.cinemaSetRender).toHaveBeenCalledTimes(1);
  });

  it("snaps the control back and says why when the engine refuses the change", async () => {
    // THE DOCUMENT IS THE TRUTH. A picker left showing 60 after the engine declined to store 60 is a
    // control lying about the state of the thing it edits — and the sentence belongs beside it,
    // because a modal covers the status bar the toast would land in.
    const refused = vi.fn(() =>
      Promise.resolve({
        ...CUT,
        reason: "stop Play first — render settings are authored, not live-edited",
      }),
    );
    const onSettingsSaved = vi.fn();
    render(
      <RenderDialog
        open
        onClose={vi.fn()}
        client={fakeClient({ cinemaSetRender: refused })}
        entity="e1"
        name="Skid Weld Line"
        cut={CUT}
        activeShotIndex={null}
        deliveryLabel="2.39:1 scope"
        onSettingsSaved={onSettingsSaved}
      />,
    );
    await screen.findByTestId("render-fps");
    fireEvent.change(screen.getByTestId("render-fps"), { target: { value: "60" } });
    await waitFor(() => expect(screen.getByText(/stop Play first/)).toBeTruthy());
    expect((screen.getByTestId("render-fps") as HTMLSelectElement).value).toBe("24");
    expect(onSettingsSaved).not.toHaveBeenCalled();
  });

  it("does not remember which SHOT was open — only what the cut delivers", async () => {
    // The one answer that is deliberately still not stored. The other four describe the deliverable;
    // this one describes what the author is looking at right now, and a dialog reopening on
    // "shot 2 only" a week later would render two seconds of a film and call it the film.
    const { client } = open();
    fireEvent.change(await screen.findByTestId("render-scope"), { target: { value: "shot" } });
    await waitFor(() => expect(client.cinemaRenderPlan).toHaveBeenCalledWith("e1", 24, 1, 1080, "movie"));
    expect(client.cinemaSetRender).not.toHaveBeenCalled();
  });

  it("says WHERE the files go before the click, not only in the ledger after it", async () => {
    // The one thing this dialog never stated before the button that pays for it. The folder was
    // asked for AFTER the click, by the operating system, and the only surface that ever named it
    // was the ledger at the end.
    open({ cut: { ...CUT, render: { ...CUT.render, folder: "X:\\Renders\\Skid" } } });
    expect((await screen.findByTestId("render-folder")).textContent).toBe("X:\\Renders\\Skid");
  });

  it("says it will ask when no folder is remembered", async () => {
    open();
    expect((await screen.findByTestId("render-folder")).textContent).toMatch(/asked when you render/);
  });

  it("renders into the remembered folder without asking", async () => {
    // THE FRICTION THIS REMOVES. Before this, every take walked the operating system's folder tree
    // again for a folder chosen ten minutes earlier. `null` is what asks; a path is what does not.
    const { client } = open({ cut: { ...CUT, render: { ...CUT.render, folder: "X:\\Renders" } } });
    fireEvent.click(await screen.findByTestId("render-start"));
    await waitFor(() =>
      expect(client.cinemaRenderStart).toHaveBeenCalledWith("e1", 24, null, "Skid Weld Line", "X:\\Renders", 1080, "movie"),
    );
  });

  it("remembers the folder the render's own picker returned", async () => {
    // Clicking Render with nothing stored opens a picker titled "Choose a folder for the rendered
    // frames". The author has just answered the question; asking it again on the next take would be
    // forgetting an answer given ten seconds ago.
    const { client, onSettingsSaved } = open();
    fireEvent.click(await screen.findByTestId("render-start"));
    await waitFor(() =>
      expect(client.cinemaSetRender).toHaveBeenCalledWith("e1", "movie", 24, 1080, "", "C:/renders/skid-weld-line"),
    );
    // ...and QUIETLY. The author came here to render, not to edit settings, so this rides in without
    // a toast landing on top of the progress bar.
    await waitFor(() => expect(onSettingsSaved).toHaveBeenCalled());
    expect(onSettingsSaved.mock.calls.at(-1)?.[1]).toBe(false);
  });

  it("stores what the folder picker returned, as one undoable change", async () => {
    const { client, onSettingsSaved } = open();
    fireEvent.click(await screen.findByTestId("render-folder-choose"));
    await waitFor(() => expect(client.cinemaPickRenderFolder).toHaveBeenCalledWith("e1"));
    await waitFor(() => expect(onSettingsSaved).toHaveBeenCalled());
    // An explicit choice IS announced — it is an authoring gesture like any other.
    expect(onSettingsSaved.mock.calls[0][1]).toBe(true);
    expect(onSettingsSaved.mock.calls[0][0].render.folder).toBe("X:\\Renders");
  });

  it("treats a cancelled folder picker as nothing at all", async () => {
    // A decision not to decide is not an error. Nothing changed, so nothing is said — scolding
    // somebody for pressing Escape is the failure this asserts against.
    const cancelled = vi.fn(() =>
      Promise.resolve({ ...CUT, entity: null, message: "", reason: null }),
    );
    const onSettingsSaved = vi.fn();
    render(
      <RenderDialog
        open
        onClose={vi.fn()}
        client={fakeClient({ cinemaPickRenderFolder: cancelled })}
        entity="e1"
        name="Skid Weld Line"
        cut={CUT}
        activeShotIndex={null}
        deliveryLabel="2.39:1 scope"
        onSettingsSaved={onSettingsSaved}
      />,
    );
    fireEvent.click(await screen.findByTestId("render-folder-choose"));
    await waitFor(() => expect(cancelled).toHaveBeenCalled());
    expect(onSettingsSaved).not.toHaveBeenCalled();
    expect(screen.queryByText(/was not saved/)).toBeNull();
  });

  it("changing a rate cannot clear the folder", async () => {
    // The setter takes the whole block, so every write states all five — and a draft that carried
    // its own idea of the destination would blank a remembered one the first time anybody touched
    // a picker. The destination only ever comes from the document or from a picker.
    const { client } = open({ cut: { ...CUT, render: { ...CUT.render, folder: "X:\\Renders" } } });
    await screen.findByTestId("render-fps");
    fireEvent.change(screen.getByTestId("render-fps"), { target: { value: "30" } });
    await waitFor(() =>
      expect(client.cinemaSetRender).toHaveBeenCalledWith("e1", "movie", 30, 1080, "", "X:\\Renders"),
    );
  });
});
