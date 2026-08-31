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
  reads: [],
  rows: [row(0, 0, 5), row(1, 5, 5), row(2, 10, 2.5)],
  problems: [],
  message: "",
  reason: null,
};

function open(over: Partial<Parameters<typeof RenderDialog>[0]> = {}) {
  const client = fakeClient();
  const onClose = vi.fn();
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
      {...over}
    />,
  );
  return { client, onClose };
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
    await waitFor(() => expect(client.cinemaRenderPlan).toHaveBeenCalledWith("e1", 24, null, 1080));
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
    await waitFor(() => expect(client.cinemaRenderPlan).toHaveBeenCalledWith("e1", 60, null, 1080));
    await waitFor(() =>
      expect(screen.getByTestId("render-start").textContent).toMatch(/Render 750 frames/),
    );
  });

  it("narrows to one shot when the scope says so, and renders THAT one", async () => {
    const { client } = open();
    await waitFor(() => expect(client.cinemaRenderPlan).toHaveBeenCalledWith("e1", 24, null, 1080));
    fireEvent.change(screen.getByTestId("render-scope"), { target: { value: "shot" } });
    await waitFor(() => expect(client.cinemaRenderPlan).toHaveBeenCalledWith("e1", 24, 1, 1080));
    fireEvent.click(await screen.findByTestId("render-start"));
    await waitFor(() =>
      expect(client.cinemaRenderStart).toHaveBeenCalledWith("e1", 24, 1, "Skid Weld Line", null, 1080),
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
      />,
    );
    await waitFor(() => expect(screen.getByLabelText("Render progress")).toBeTruthy());
    expect(screen.queryByTestId("render-start")).toBeNull();
    // ...and the author is not held hostage by it: the dialog can be put away while it runs.
    expect(screen.getByTestId("render-hide")).toBeTruthy();
  });

  // ── ADR-177: the size the files are written at ────────────────────────────────────────────────

  it("opens on a delivery size rather than on whatever the window happens to be", async () => {
    // THE DEFAULT IS THE POINT. This dialog shipped writing every sequence at the size of the stage,
    // which on a laptop with both docks open is around 400 lines — a film nobody can use, produced by
    // a dialog that never asked. The offer it opens on is now a delivery format.
    const { client } = open();
    const size = (await screen.findByTestId("render-size")) as HTMLSelectElement;
    expect(size.value).toBe("1080");
    // ...and "the stage" is still there, because it is the right answer for a quick look.
    expect(Array.from(size.options).map((o) => o.value)).toEqual([
      "viewport",
      "720",
      "1080",
      "1440",
      "2160",
    ]);
    await waitFor(() => expect(client.cinemaRenderPlan).toHaveBeenCalledWith("e1", 24, null, 1080));
  });

  it("re-asks the engine when the size changes, and states the pixels it answered", async () => {
    // The width is NEVER computed here. 2160 x 2.39 = 5162, and the only way this string can appear
    // is the engine having been asked and its answer rendered — which is what makes the size above the
    // button the size in the file.
    const { client } = open();
    await waitFor(() =>
      expect(screen.getByTestId("render-cost").textContent).toMatch(/2582x1080/),
    );
    fireEvent.change(screen.getByTestId("render-size"), { target: { value: "2160" } });
    await waitFor(() => expect(client.cinemaRenderPlan).toHaveBeenCalledWith("e1", 24, null, 2160));
    await waitFor(() =>
      expect(screen.getByTestId("render-cost").textContent).toMatch(/5162x2160/),
    );
    // The frame COUNT is unchanged by a size — a render is the same film at a different resolution,
    // and a dialog that re-planned the frames would be saying otherwise.
    expect(screen.getByTestId("render-start").textContent).toMatch(/Render 300 frames/);
  });

  it("renders at the size that was chosen, and says so while it runs", async () => {
    const { client } = open();
    fireEvent.change(await screen.findByTestId("render-size"), { target: { value: "720" } });
    await waitFor(() => expect(client.cinemaRenderPlan).toHaveBeenCalledWith("e1", 24, null, 720));
    fireEvent.click(screen.getByTestId("render-start"));
    await waitFor(() =>
      expect(client.cinemaRenderStart).toHaveBeenCalledWith(
        "e1",
        24,
        null,
        "Skid Weld Line",
        null,
        720,
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
    // window genuinely stops producing frames, and the render genuinely stalls on it.
    const { client } = open();
    fireEvent.change(await screen.findByTestId("render-size"), { target: { value: "viewport" } });
    await waitFor(() => expect(client.cinemaRenderPlan).toHaveBeenCalledWith("e1", 24, null, null));
    fireEvent.click(screen.getByTestId("render-start"));
    const progress = await screen.findByTestId("render-progress");
    await waitFor(() => expect(progress.textContent).toContain("minimised window produces no frames"));
    expect(progress.textContent).not.toContain("covered or minimised");
  });

  it("names the frame it is composed for, because that is what decides the shape of the files", async () => {
    open({ deliveryLabel: "2.39:1 scope" });
    expect((await screen.findByTestId("render-dialog")).textContent).toContain("2.39:1 scope");
  });
});
