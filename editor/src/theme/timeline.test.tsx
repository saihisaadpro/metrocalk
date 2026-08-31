//! The shared timeline framework's own contract.
//!
//! WHAT jsdom CAN AND CANNOT ANSWER HERE, stated once so the next person does not write a test that
//! passes for the wrong reason. jsdom has **no layout** — every rect is 0×0 and no stylesheet is
//! applied — so "the last ruler label is not clipped" and "the playhead does not paint through the
//! label" are questions only `shots` can ask, and they are asked there (`animation-timeline-tracks`,
//! `animation-curve-editor`, `physics-recorded-transport`). What lives here is everything that is a
//! decision rather than a pixel: the geometry arithmetic, the class each state emits, the accessible
//! name each icon-only control carries, and the one structural rule this module was extracted to
//! enforce — that a healthy row renders NO badge.

import type { ReactNode } from "react";
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import {
  CURVE_VIEWBOX,
  CurveCanvas,
  TIMELINE_DIVISIONS,
  TimelineChip,
  TimelineKey,
  TimelineLane,
  TimelinePlayhead,
  TimelineRow,
  TimelineRuler,
  TimelineSurface,
  TimelineTrackHead,
  Transport,
  TransportButtons,
  curvePoint,
  timelineOffset,
  timelineTickAt,
  timelineTicks,
} from "./timeline";

const rect = (width: number, left = 0): DOMRect =>
  ({ width, left, right: left + width, top: 0, bottom: 0, height: 0, x: left, y: 0, toJSON: () => ({}) }) as DOMRect;

const at = (clientX: number, width: number, left = 0) =>
  ({ clientX, currentTarget: { getBoundingClientRect: () => rect(width, left) } });

describe("timeline geometry", () => {
  it("clamps an offset into the lane at both ends and survives a zero duration", () => {
    expect(timelineOffset(45, 180)).toBe("25%");
    // An empty sequence is an ordinary state, not a caller error: every consumer had written its own
    // `Math.max(1, …)` guard, and the one that forgot produced `Infinity%`.
    expect(timelineOffset(0, 0)).toBe("0%");
    expect(timelineOffset(-10, 180)).toBe("0%");
    expect(timelineOffset(9999, 180)).toBe("100%");
  });

  it("clamps a scrub to the sequence rather than letting the last pixel run past the end", () => {
    expect(timelineTickAt(at(50, 100), 180)).toBe(90);
    // The defect this replaces: both consumers divided by the rect width and neither clamped, so a
    // click on the lane's right border scrubbed PAST `duration` and the transport reconciled back.
    expect(timelineTickAt(at(101, 100), 180)).toBe(180);
    expect(timelineTickAt(at(-4, 100), 180)).toBe(0);
  });

  it("reports null rather than NaN when the lane has not been laid out yet", () => {
    expect(timelineTickAt(at(10, 0), 180)).toBeNull();
  });

  it("divides the ruler exactly as many times as the lane graticule is drawn", () => {
    const ticks = timelineTicks(180, (value) => `${value}t`);
    expect(ticks).toHaveLength(TIMELINE_DIVISIONS + 1);
    expect(ticks[0]).toEqual({ value: 0, label: "0t" });
    expect(ticks[ticks.length - 1]).toEqual({ value: 180, label: "180t" });
  });

  it("keeps every division DISTINCT and inside the lane on a duration that is not a round number", () => {
    // The defect this pins: rounding each division to an integer made three pairs of a 6.5s ruler
    // land on the same number — which the ruler used as its React key — and pushed the last one to
    // 7, past the end of the lane it labels. Both are invisible at 60000 ticks per second, which is
    // the only unit this module had ever been used in.
    const ticks = timelineTicks(6.5, (value) => `${value.toFixed(1)}s`);
    expect(new Set(ticks.map((tick) => tick.value)).size).toBe(TIMELINE_DIVISIONS + 1);
    expect(ticks[ticks.length - 1].value).toBe(6.5);
    expect(ticks.every((tick) => tick.value <= 6.5)).toBe(true);
  });

  it("maps a curve point into the plot's inset, not the raw viewBox", () => {
    const bounds = { minTick: 0, tickSpan: 100, minValue: 0, valueSpan: 10 };
    expect(curvePoint({ tick: 0, value: 0 }, bounds)).toEqual({ x: CURVE_VIEWBOX.left, y: CURVE_VIEWBOX.bottom });
    expect(curvePoint({ tick: 100, value: 10 }, bounds)).toEqual({ x: CURVE_VIEWBOX.right, y: CURVE_VIEWBOX.top });
  });
});

describe("the ruler", () => {
  it("marks the first and last labels, which are the two that would otherwise be cut in half", () => {
    render(
      <TimelineSurface laneWidth={600}>
        <TimelineRuler label="Tracks" duration={180} currentTick={90} ticks={timelineTicks(180, (v) => `${v}t`)} />
      </TimelineSurface>,
    );
    const ticks = document.querySelectorAll(".mtk-timeline__tick");
    expect(ticks).toHaveLength(TIMELINE_DIVISIONS + 1);
    expect(ticks[0].className).toContain("mtk-timeline__tick--first");
    expect(ticks[ticks.length - 1].className).toContain("mtk-timeline__tick--last");
    // Nothing in between may claim an edge rule — a mid-ruler label centres on its own division.
    expect(ticks[5].className).toBe("mtk-timeline__tick");
  });

  it("does not mark a lone tick as both edges", () => {
    render(<TimelineRuler label="Tracks" duration={0} currentTick={0} ticks={[{ value: 0, label: "0t" }]} />);
    const only = document.querySelector(".mtk-timeline__tick")!;
    expect(only.className).toContain("--first");
    expect(only.className).not.toContain("--last");
  });

  it("scrubs to the clamped time the pointer landed on", () => {
    const onScrub = vi.fn();
    render(<TimelineRuler data-testid="ruler" label="Tracks" duration={180} currentTick={0} ticks={[]} onScrub={onScrub} />);
    const track = screen.getByTestId("ruler");
    // jsdom gives every element a zero-width rect, which is precisely the case the helper returns
    // null for — so the assertion is that a scrub on an unlaid-out ruler emits NOTHING rather than
    // `NaN`, which is the value the old inline arithmetic would have committed as a tick.
    fireEvent.click(track);
    expect(onScrub).not.toHaveBeenCalled();

    track.getBoundingClientRect = () => rect(200);
    fireEvent.click(track, { clientX: 100 });
    expect(onScrub).toHaveBeenCalledWith(90);

    // ...and the last pixel of the ruler is the END of the sequence, not one past it.
    fireEvent.click(track, { clientX: 260 });
    expect(onScrub).toHaveBeenLastCalledWith(180);
  });
});

describe("a track head", () => {
  const head = (attention?: ReactNode) =>
    render(
      <TimelineRow variant="track">
        <TimelineTrackHead name="Transform · Position Y" title="Weld Gun 7 / Transform.position.y · ready" meta="4 keys" attention={attention} />
      </TimelineRow>,
    );

  it("renders NO badge when the row does not need attention", () => {
    head();
    // The whole reason `attention` exists. Three green `ready` pills on three healthy rows is four
    // accent colours inside 178px saying that nothing is wrong.
    expect(document.querySelector(".mtk-badge")).toBeNull();
    // ...and the readiness word is still reachable, on the control that names the track.
    expect(screen.getByRole("button", { name: "Select track Transform · Position Y" }).title).toContain("ready");
  });

  it("renders the badge when it does", () => {
    head(<span className="mtk-badge">preview</span>);
    expect(document.querySelector(".mtk-badge")?.textContent).toBe("preview");
  });

  it("puts the name on a control with an accessible name, not a bare clickable div", () => {
    const onSelect = vi.fn();
    render(
      <TimelineRow variant="track">
        <TimelineTrackHead name="Emitter · Rate" selected onSelect={onSelect} />
      </TimelineRow>,
    );
    const button = screen.getByRole("button", { name: "Select track Emitter · Rate" });
    expect(button.getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(button);
    expect(onSelect).toHaveBeenCalledTimes(1);
  });
});

describe("things on a lane", () => {
  it("says selected, invalid and locked with classes rather than with colour alone", () => {
    render(
      <TimelineLane>
        <TimelineKey tick={45} duration={180} selected aria-label="a" data-testid="k1" />
        <TimelineKey tick={90} duration={180} invalid locked aria-label="b" data-testid="k2" />
      </TimelineLane>,
    );
    const first = screen.getByTestId("k1");
    expect(first.getAttribute("data-selected")).toBe("true");
    expect(first.querySelector(".mtk-timeline__key-mark")?.className).toContain("is-selected");

    const second = screen.getByTestId("k2");
    // An unselected key must not claim the attribute at all — `data-selected="false"` is a value a
    // CSS attribute selector matches just as happily as `"true"`.
    expect(second.hasAttribute("data-selected")).toBe(false);
    const mark = second.querySelector(".mtk-timeline__key-mark")!.className;
    expect(mark).toContain("is-invalid");
    expect(mark).toContain("is-locked");
    expect(mark).not.toContain("is-selected");
  });

  it("gives a chip's remove control its own accessible name and omits it when there is nothing to remove", () => {
    const onRemove = vi.fn();
    const { rerender } = render(
      <TimelineLane>
        <TimelineChip tick={45} duration={180} label="Contact marker at 45 ticks" removeLabel="Delete marker Contact" onRemove={onRemove}>
          M
        </TimelineChip>
      </TimelineLane>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Delete marker Contact" }));
    expect(onRemove).toHaveBeenCalledTimes(1);

    rerender(
      <TimelineLane>
        <TimelineChip tick={45} duration={180} label="Contact marker at 45 ticks">
          M
        </TimelineChip>
      </TimelineLane>,
    );
    expect(screen.queryByRole("button", { name: "Delete marker Contact" })).toBeNull();
    expect(screen.getByRole("button", { name: "Contact marker at 45 ticks" })).toBeTruthy();
  });

  it("reads the playhead from the custom property when it is not given a tick", () => {
    render(<TimelineLane><TimelinePlayhead /></TimelineLane>);
    const head = document.querySelector<HTMLElement>(".mtk-timeline__playhead")!;
    // Invariant 4: during playback the head moves by writing one custom property on an ancestor, so
    // the lanes are NOT re-rendered per frame. A component that always took a prop would defeat it.
    expect(head.style.left).toContain("--animation-playhead");
    expect(head.className).not.toContain("--handled");
  });
});

describe("the transport", () => {
  it("is a labelled group carrying the shared pill class, whichever subsystem built it", () => {
    render(
      <Transport aria-label="Recorded simulation transport" data-testid="t">
        <TransportButtons aria-label="Playback"><button type="button">go</button></TransportButtons>
      </Transport>,
    );
    const t = screen.getByTestId("t");
    expect(t.className).toContain("mtk-transport");
    expect(t.getAttribute("role")).toBe("group");
    expect(screen.getByRole("group", { name: "Recorded simulation transport" })).toBe(t);
    // `attached` is what makes the run read as ONE control rather than four loose squares.
    expect(document.querySelector(".mtk-toolbar__group--attached")).toBeTruthy();
  });
});

describe("the curve canvas", () => {
  it("draws a graticule at the ruler's own divisions and names both axes", () => {
    render(
      <CurveCanvas aria-label="Position Y animation curve editor" valueAxis="value" timeAxis="time" caption="Position Y" hint="Edit in the inspector.">
        <path d="M 0 0" />
      </CurveCanvas>,
    );
    // Eleven columns (the ruler's ten divisions) and five rows. Without this the curve is a picture
    // of a shape: there is no way to see that two keys are level.
    expect(document.querySelectorAll(".mtk-curve__grid line")).toHaveLength(TIMELINE_DIVISIONS + 1 + 5);
    expect(screen.getByText("value").className).toContain("mtk-curve__axis--value");
    expect(screen.getByText("time").className).toContain("mtk-curve__axis--time");
    expect(screen.getByRole("group", { name: "Position Y animation curve editor" })).toBeTruthy();
  });
});
