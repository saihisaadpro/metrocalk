//! The floating-overlay primitives (theme/Popover.tsx) — verified headless in jsdom.
//!
//! The load-bearing test is `escapes an overflow:hidden container`: it renders a `Popover` whose trigger
//! lives inside a clipped box and asserts the panel is portaled to `document.body`, NOT nested in the clipped
//! box — i.e. it CANNOT be clipped. That is precisely the File-menu-behind-the-header bug, guarded here so it
//! can never regress. Escape / outside-click dismissal + closed-renders-nothing are covered too.

import { afterEach, expect, test, vi } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import { useRef, useState } from "react";
import { DialogSurface, Modal, Popover, PopoverSurface } from "./Popover";

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

test("Popover supports non-menu semantics and shared overlay surfaces", () => {
  render(
    <Popover open anchorPoint={{ x: 20, y: 20 }} onClose={() => {}} role="listbox">
      <PopoverSurface data-testid="popover-surface">Options</PopoverSurface>
    </Popover>,
  );
  expect(screen.getByRole("listbox")).toBeTruthy();
  expect(screen.getByTestId("popover-surface").className).toContain("mtk-popover-surface");
});

function KeyboardMenuHarness() {
  const trigger = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  return (
    <>
      <button
        ref={trigger}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen(true)}
      >
        Open actions
      </button>
      <Popover
        open={open}
        anchor={trigger}
        returnFocus={trigger}
        ariaLabel="Actions"
        onClose={() => setOpen(false)}
      >
        <button role="menuitem">First</button>
        <button role="menuitem">Second</button>
      </Popover>
    </>
  );
}

test("menu popovers enter focus, support arrow/Home/End navigation, and restore focus", () => {
  render(<KeyboardMenuHarness />);
  const trigger = screen.getByRole("button", { name: "Open actions" });
  fireEvent.click(trigger);
  const menu = screen.getByRole("menu", { name: "Actions" });
  const first = screen.getByRole("menuitem", { name: "First" });
  const second = screen.getByRole("menuitem", { name: "Second" });
  expect(document.activeElement).toBe(first);
  fireEvent.keyDown(menu, { key: "ArrowDown" });
  expect(document.activeElement).toBe(second);
  fireEvent.keyDown(menu, { key: "Home" });
  expect(document.activeElement).toBe(first);
  fireEvent.keyDown(menu, { key: "End" });
  expect(document.activeElement).toBe(second);
  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.queryByRole("menu", { name: "Actions" })).toBeNull();
  expect(document.activeElement).toBe(trigger);
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

function FocusModalHarness({ preferSecond = false }: { preferSecond?: boolean }) {
  const [open, setOpen] = useState(false);
  const secondRef = useRef<HTMLButtonElement>(null);
  return (
    <>
      <button onClick={() => setOpen(true)}>Open settings</button>
      <button>Outside control</button>
      <Modal
        open={open}
        onClose={() => setOpen(false)}
        initialFocusRef={preferSecond ? secondRef : undefined}
        ariaLabel="Settings"
      >
        <div>
          <button>First action</button>
          <button ref={secondRef}>Second action</button>
        </div>
      </Modal>
    </>
  );
}

test("Modal moves focus inside and restores it to the invoking control on close", () => {
  const { container } = render(<FocusModalHarness />);
  const trigger = screen.getByRole("button", { name: "Open settings" });
  trigger.focus();
  fireEvent.click(trigger);

  expect(screen.getByRole("dialog", { name: "Settings" })).toBeTruthy();
  expect(container.inert).toBe(true);
  expect(container.getAttribute("aria-hidden")).toBe("true");
  expect(document.activeElement).toBe(screen.getByRole("button", { name: "First action" }));
  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull();
  expect(container.inert).toBe(false);
  expect(container.hasAttribute("aria-hidden")).toBe(false);
  expect(document.activeElement).toBe(trigger);
});

test("Modal honours initialFocusRef and traps forward/reverse Tab navigation", () => {
  render(<FocusModalHarness preferSecond />);
  const trigger = screen.getByRole("button", { name: "Open settings" });
  trigger.focus();
  fireEvent.click(trigger);
  const first = screen.getByRole("button", { name: "First action" });
  const second = screen.getByRole("button", { name: "Second action" });
  expect(document.activeElement).toBe(second);

  fireEvent.keyDown(window, { key: "Tab" });
  expect(document.activeElement).toBe(first);
  fireEvent.keyDown(window, { key: "Tab", shiftKey: true });
  expect(document.activeElement).toBe(second);

  // Programmatic focus cannot make the next keyboard Tab escape the modal either.
  screen.getByRole("button", { name: "Outside control", hidden: true }).focus();
  fireEvent.keyDown(window, { key: "Tab" });
  expect(document.activeElement).toBe(first);
});

test("Modal focuses its dialog container when it has no interactive descendants", () => {
  const trigger = document.createElement("button");
  document.body.appendChild(trigger);
  trigger.focus();
  const { unmount } = render(
    <Modal open onClose={() => {}} ariaLabel="Notice">
      <p>Build completed.</p>
    </Modal>,
  );
  expect(document.activeElement).toBe(screen.getByRole("dialog", { name: "Notice" }));
  unmount();
  expect(document.activeElement).toBe(trigger);
  trigger.remove();
});

test("DialogSurface provides the shared modal content contract", () => {
  render(
    <DialogSurface data-testid="dialog-surface">
      Content
    </DialogSurface>,
  );
  expect(screen.getByTestId("dialog-surface").className).toContain("mtk-dialog-surface");
});
