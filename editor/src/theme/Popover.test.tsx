//! The floating-overlay primitives (theme/Popover.tsx) — verified headless in jsdom.
//!
//! The load-bearing test is `escapes an overflow:hidden container`: it renders a `Popover` whose trigger
//! lives inside a clipped box and asserts the panel is portaled to `document.body`, NOT nested in the clipped
//! box — i.e. it CANNOT be clipped. That is precisely the File-menu-behind-the-header bug, guarded here so it
//! can never regress. Escape / outside-click dismissal + closed-renders-nothing are covered too.

import { afterEach, expect, test, vi } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import { useRef, useState } from "react";
import { Modal, Popover } from "./Popover";

afterEach(cleanup);

/** A trigger + an anchored Popover, optionally nested inside an `overflow: hidden` box. */
function PopHarness({ onClose = () => {}, clipped = false, open = true }: { onClose?: () => void; clipped?: boolean; open?: boolean }) {
  const anchor = useRef<HTMLButtonElement>(null);
  const [isOpen, setOpen] = useState(open);
  const close = () => {
    setOpen(false);
    onClose();
  };
  const inner = (
    <>
      <button ref={anchor} data-testid="trigger">
        File
      </button>
      <Popover open={isOpen} anchor={anchor} onClose={close}>
        <div data-testid="pop-content">menu items</div>
      </Popover>
    </>
  );
  return clipped ? (
    <div data-testid="clip" style={{ overflow: "hidden", height: 10 }}>
      {inner}
    </div>
  ) : (
    inner
  );
}

test("Popover portals its content to document.body — it ESCAPES an overflow:hidden ancestor (the File-menu bug)", () => {
  render(<PopHarness clipped />);
  const content = screen.getByTestId("pop-content");
  const clip = screen.getByTestId("clip");
  // Present in the document…
  expect(document.body.contains(content)).toBe(true);
  // …but NOT inside the clipped box → cannot be clipped by its `overflow: hidden`.
  expect(clip.contains(content)).toBe(false);
  // Rendered under a top-level portal (a direct-ish child of body, not the test render root).
  expect(content.closest("[data-testid='clip']")).toBeNull();
});

test("Popover renders nothing when closed", () => {
  render(<PopHarness open={false} />);
  expect(screen.queryByTestId("pop-content")).toBeNull();
});

test("Popover dismisses on Escape", () => {
  const onClose = vi.fn();
  render(<PopHarness onClose={onClose} />);
  expect(screen.getByTestId("pop-content")).toBeTruthy();
  fireEvent.keyDown(window, { key: "Escape" });
  expect(onClose).toHaveBeenCalledTimes(1);
});

test("Popover has role=menu for a11y", () => {
  render(<PopHarness />);
  expect(screen.getByRole("menu")).toBeTruthy();
});

test("Modal portals to body, is role=dialog, and dismisses on backdrop-click + Escape", () => {
  const onClose = vi.fn();
  const { rerender } = render(
    <Modal open onClose={onClose}>
      <div data-testid="modal-body">confirm?</div>
    </Modal>,
  );
  const dialog = screen.getByRole("dialog");
  expect(document.body.contains(dialog)).toBe(true);
  expect(screen.getByTestId("modal-body")).toBeTruthy();

  // A click on the dialog content does NOT dismiss…
  fireEvent.mouseDown(screen.getByTestId("modal-body"));
  expect(onClose).not.toHaveBeenCalled();
  // …a click on the backdrop (the dialog element itself) does.
  fireEvent.mouseDown(dialog);
  expect(onClose).toHaveBeenCalledTimes(1);
  // Escape dismisses.
  fireEvent.keyDown(window, { key: "Escape" });
  expect(onClose).toHaveBeenCalledTimes(2);

  // Closed → nothing rendered.
  rerender(
    <Modal open={false} onClose={onClose}>
      <div data-testid="modal-body">confirm?</div>
    </Modal>,
  );
  expect(screen.queryByTestId("modal-body")).toBeNull();
});
