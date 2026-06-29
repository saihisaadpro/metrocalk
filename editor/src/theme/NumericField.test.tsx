//! The scrub-to-edit NumericField (M14.3 / ADR-059) — verified headless: a scrub-drag COALESCES into ONE
//! commit at pointer-up (one undo step, not N), with live `onScrub` feedback during the drag; keyboard nudge
//! and type-to-set each commit one transaction; invalid input reverts (no silent zeroing). Keys off the
//! commit count + values (the structured signal), never a styled string.

import { expect, test, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { NumericField } from "./primitives";

test("scrub-drag coalesces into ONE commit at mouse-up (not per move), with live onScrub feedback", () => {
  const onCommit = vi.fn();
  const onScrub = vi.fn();
  render(<NumericField value={5} step={1} onCommit={onCommit} onScrub={onScrub} data-testid="nf" />);
  const el = screen.getByTestId("nf");
  fireEvent.mouseDown(el, { clientX: 100, button: 0 });
  fireEvent.mouseMove(window, { clientX: 120 }); // dx 20 × speed(step=1) → +20
  fireEvent.mouseMove(window, { clientX: 140 }); // dx 40 → +40 → 45
  fireEvent.mouseUp(window);
  expect(onScrub).toHaveBeenCalled(); // live during the drag (local feedback, no IPC)
  expect(onCommit).toHaveBeenCalledTimes(1); // ONE undo step for the whole scrub
  expect(onCommit).toHaveBeenCalledWith(45);
});

test("a click (no movement) does NOT commit — it leaves the field for typing", () => {
  const onCommit = vi.fn();
  render(<NumericField value={5} step={1} onCommit={onCommit} data-testid="nf" />);
  const el = screen.getByTestId("nf");
  fireEvent.mouseDown(el, { clientX: 100, button: 0 });
  fireEvent.mouseUp(window); // no movement → not a scrub
  expect(onCommit).not.toHaveBeenCalled();
});

test("keyboard nudge commits one step (Shift = ×10)", () => {
  const onCommit = vi.fn();
  render(<NumericField value={5} step={1} onCommit={onCommit} data-testid="nf" />);
  const el = screen.getByTestId("nf");
  fireEvent.keyDown(el, { key: "ArrowUp" });
  expect(onCommit).toHaveBeenLastCalledWith(6);
  fireEvent.keyDown(el, { key: "ArrowDown", shiftKey: true });
  expect(onCommit).toHaveBeenLastCalledWith(-5); // 5 − 10
});

test("type-to-set commits on blur; invalid input reverts (no silent zeroing)", () => {
  const onCommit = vi.fn();
  render(<NumericField value={5} onCommit={onCommit} data-testid="nf" />);
  const el = screen.getByTestId("nf") as HTMLInputElement;
  fireEvent.focus(el);
  fireEvent.change(el, { target: { value: "12.5" } });
  fireEvent.blur(el);
  expect(onCommit).toHaveBeenCalledWith(12.5);

  onCommit.mockClear();
  fireEvent.focus(el);
  fireEvent.change(el, { target: { value: "abc" } });
  fireEvent.blur(el);
  expect(onCommit).not.toHaveBeenCalled(); // invalid → no emit
  expect(el.value).toBe("5"); // reverted to the committed value, NOT silently zeroed
});

test("integer field rounds + nudges by 1; data-scrubbing tracks the live drag state", () => {
  const onCommit = vi.fn();
  render(<NumericField value={3} integer onCommit={onCommit} data-testid="nf" />);
  const el = screen.getByTestId("nf");
  expect(el.getAttribute("data-scrubbing")).toBe("0");
  fireEvent.mouseDown(el, { clientX: 0, button: 0 });
  fireEvent.mouseMove(window, { clientX: 10 }); // integer step 1 → +10 → 13
  expect(el.getAttribute("data-scrubbing")).toBe("1");
  fireEvent.mouseUp(window);
  expect(onCommit).toHaveBeenCalledWith(13);
  expect(el.getAttribute("data-scrubbing")).toBe("0");
});
