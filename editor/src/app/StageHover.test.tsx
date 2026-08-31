//! **The pointer's question, and the cadence it is allowed to ask at** (ADR-191).
//!
//! `viewport_peek` is a per-frame temptation: it is fed from `pointermove`, and invariant 4 says the
//! hot path never crosses the JS boundary. So the assertions here are mostly about WHAT DOES NOT
//! HAPPEN — a move costs no IPC and no render, the probe fires once per pause, a second identical
//! answer does not re-render, and a gesture cancels a probe that is already pending.
//!
//! `tooltipPosition` is tested directly because it is the one part jsdom can judge honestly: a
//! tooltip that opens past the right edge of the window is the same defect class the shots harness's
//! layout invariants exist to catch, and it is completely invisible to a test that only asserts the
//! tooltip rendered.

import { act, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { fakeClient } from "../transport/test-client";
import { HOVER_SETTLE_MS, StageHoverTooltip, tooltipPosition, useStageHover } from "./StageHover";

const DETAILS = { id: "e1", name: "Weld Gun", components: ["Transform"], provides: [], requires: [], boundTo: [] };

/** A host that exposes the hook's surface as DOM, so the test drives it the way App does. */
function Host({ client, enabled }: { client: ReturnType<typeof fakeClient>; enabled: boolean }) {
  const { hover, onMove, clear } = useStageHover(client, enabled);
  return (
    <div>
      <div
        data-testid="stage"
        style={{ width: 400, height: 300 }}
        onPointerMove={(e) => onMove(e.clientX, e.clientY)}
        onPointerLeave={() => clear()}
      />
      <span data-testid="answer">{hover?.id ?? ""}</span>
      <StageHoverTooltip client={client} hover={hover} />
    </div>
  );
}

function move(x: number, y: number) {
  act(() => {
    screen.getByTestId("stage").dispatchEvent(
      new MouseEvent("pointermove", { bubbles: true, cancelable: true, clientX: x, clientY: y }),
    );
  });
}

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("what is under the pointer", () => {
  it("asks NOTHING while the pointer is moving, and once when it stops", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const peek = vi.fn(() => Promise.resolve("e1" as string | null));
    render(<Host client={fakeClient({ viewportPeek: peek })} enabled />);

    // Six moves in one gesture — a slow hand crossing the stage. The old alternative was a probe per
    // move; this is the assertion that keeps it off the hot path.
    for (let i = 0; i < 6; i += 1) move(100 + i * 20, 120);
    expect(peek).not.toHaveBeenCalled();

    await act(async () => {
      vi.advanceTimersByTime(HOVER_SETTLE_MS + 5);
    });
    expect(peek).toHaveBeenCalledTimes(1);
    // The point asked about is the LAST one, normalized against the full surface — the same mapping
    // the click uses, because a hover that highlights one object and a click that selects another is
    // the divergence `viewport_peek`'s own doc comment forbids.
    expect(peek.mock.calls[0]).toEqual([200 / window.innerWidth, 120 / window.innerHeight]);
    await waitFor(() => expect(screen.getByTestId("answer").textContent).toBe("e1"));
  });

  it("names the entity without selecting it", async () => {
    const entityDetails = vi.fn(() => Promise.resolve(DETAILS));
    render(
      <Host
        client={fakeClient({ viewportPeek: () => Promise.resolve("e1"), entityDetails })}
        enabled
      />,
    );
    move(150, 150);
    await waitFor(() => expect(screen.getByTestId("tooltip-name").textContent).toBe("Weld Gun"));
    // HOVER MUST BE INERT. The whole reason this read exists rather than a click is that learning what
    // something is must not change what is selected, bump the revision, or land in the undo stack.
    expect(screen.getByTestId("stage-hover").style.pointerEvents).toBe("none");
  });

  it("says nothing over empty stage — which is most of it", async () => {
    render(<Host client={fakeClient({ viewportPeek: () => Promise.resolve(null) })} enabled />);
    move(150, 150);
    await waitFor(() => expect(screen.getByTestId("answer").textContent).toBe(""));
    expect(screen.queryByTestId("stage-hover")).toBeNull();
  });

  it("a gesture cancels the pending question, and re-enabling does not fire the old one", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const peek = vi.fn(() => Promise.resolve("e1" as string | null));
    const client = fakeClient({ viewportPeek: peek });
    const { rerender } = render(<Host client={client} enabled />);
    move(150, 150);
    // A press, a marquee, an open menu — App flips `enabled` off for all of them.
    rerender(<Host client={client} enabled={false} />);
    await act(async () => {
      vi.advanceTimersByTime(HOVER_SETTLE_MS * 3);
    });
    expect(peek).not.toHaveBeenCalled();
    // A probe left pending across a drag is what would make the tooltip reappear on release,
    // describing an object the pointer left three hundred pixels ago.
    rerender(<Host client={client} enabled />);
    await act(async () => {
      vi.advanceTimersByTime(HOVER_SETTLE_MS * 3);
    });
    expect(peek).not.toHaveBeenCalled();
  });

  it("the pointer leaving takes the tooltip with it", async () => {
    render(<Host client={fakeClient({ viewportPeek: () => Promise.resolve("e1"), entityDetails: () => Promise.resolve(DETAILS) })} enabled />);
    move(150, 150);
    await waitFor(() => expect(screen.getByTestId("stage-hover")).toBeTruthy());
    act(() => {
      // React synthesizes `pointerleave` from a BUBBLING `pointerout` at the root — a non-bubbling
      // `pointerleave` dispatched on the element never reaches the delegated listener, and the test
      // then fails in a way that reads exactly like the handler being absent.
      screen
        .getByTestId("stage")
        .dispatchEvent(new MouseEvent("pointerout", { bubbles: true, cancelable: true, relatedTarget: document.body }));
    });
    expect(screen.queryByTestId("stage-hover")).toBeNull();
  });
});

describe("where the tooltip goes", () => {
  const bounds = { width: 1000, height: 800 };
  const size = { width: 280, height: 120 };

  it("sits below-right of the cursor when there is room", () => {
    expect(tooltipPosition({ x: 100, y: 100 }, size, bounds)).toEqual({ left: 118, top: 118 });
  });

  it("FLIPS rather than clamping at the right edge, so it never covers what it describes", () => {
    const { left } = tooltipPosition({ x: 960, y: 100 }, size, bounds);
    expect(left + size.width).toBeLessThanOrEqual(bounds.width);
    // Clamping alone would put the panel's right edge on the window's and slide it back UNDER the
    // cursor — on top of the object whose name it is printing.
    expect(left).toBeLessThan(960);
  });

  it("flips above the cursor at the bottom edge", () => {
    const { top } = tooltipPosition({ x: 100, y: 780 }, size, bounds);
    expect(top + size.height).toBeLessThanOrEqual(bounds.height);
    expect(top).toBeLessThan(780);
  });

  it("never goes off the top-left, even for a tooltip taller than the window", () => {
    const { left, top } = tooltipPosition({ x: 5, y: 5 }, { width: 2000, height: 2000 }, bounds);
    expect(left).toBeGreaterThanOrEqual(0);
    expect(top).toBeGreaterThanOrEqual(0);
  });
});
