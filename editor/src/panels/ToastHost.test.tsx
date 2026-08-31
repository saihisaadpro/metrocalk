import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { pushToast, toastStore, TOAST_TTL_MS } from "../store/toasts";
import { ToastHost } from "./ToastHost";

afterEach(() => {
  act(() => toastStore.getState().reset());
  vi.useRealTimers();
});

test("notifications use the right live-region urgency and expose labelled dismiss buttons", () => {
  act(() => {
    pushToast("Saved project", "success");
    pushToast("Bake failed", "error");
  });
  render(<ToastHost />);

  const [success, error] = screen.getAllByTestId("toast");
  expect(success.getAttribute("role")).toBe("status");
  expect(success.getAttribute("aria-live")).toBe("polite");
  expect(error.getAttribute("role")).toBe("alert");
  expect(error.getAttribute("aria-live")).toBe("assertive");
  expect(screen.getByTestId("toastHost").getAttribute("aria-label")).toBe("Notifications");

  const dismiss = screen.getByRole("button", { name: "Dismiss notification: Saved project" });
  expect(success.style.pointerEvents).toBe("none");
  expect(dismiss.style.pointerEvents).toBe("auto");
  fireEvent.click(dismiss);
  expect(screen.queryByText("Saved project")).toBeNull();
  expect(screen.getByText("Bake failed")).toBeTruthy();
});

test("auto-dismiss pauses while a notification is being inspected", () => {
  vi.useFakeTimers();
  act(() => pushToast("Imported mesh", "info"));
  render(<ToastHost />);

  const toast = screen.getByTestId("toast");
  fireEvent.mouseEnter(toast);
  act(() => vi.advanceTimersByTime(TOAST_TTL_MS * 2));
  expect(screen.getByText("Imported mesh")).toBeTruthy();

  fireEvent.mouseLeave(toast);
  act(() => vi.advanceTimersByTime(TOAST_TTL_MS));
  expect(screen.queryByText("Imported mesh")).toBeNull();
});

test("a notification is capped against the stage it is centred on, never against the window", () => {
  act(() =>
    pushToast(
      "Shot 1 is now a wide shot of Assembly Hall from three-quarters, pulling out — 2.5s · Ctrl-Z to undo",
      "success",
    ),
  );
  render(<ToastHost />);

  // jsdom performs no layout, so this pins the DECLARED cap — which is where the defect was. The host
  // is absolutely positioned INSIDE `#viewport` and centred on it, and the row was capped at
  // `100vw - 96px`: at a 1296px window with two docks open the stage is ~508px, so a long message was
  // sized for 1200 and clipped at BOTH edges by the stage's own `overflow: hidden`. Seen on an `.exe`
  // capture, where a cutscene re-aim toast read "s now a wide shot of Assembly Hall…".
  expect(screen.getByTestId("toastHost").style.maxWidth).toBe("calc(100% - 24px)");
  expect(screen.getByTestId("toast").style.maxWidth).toBe("100%");
});
