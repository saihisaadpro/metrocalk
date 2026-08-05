import { render, screen } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { LazyWorkspace } from "./LazyWorkspace";

afterEach(() => {
  vi.restoreAllMocks();
});

test("announces a lazy workspace as a polite, named loading status", () => {
  const pending = new Promise(() => {});
  function PendingWorkspace(): never {
    throw pending;
  }

  render(
    <LazyWorkspace label="Asset Lab">
      <PendingWorkspace />
    </LazyWorkspace>,
  );

  const status = screen.getByRole("status");
  expect(status.getAttribute("aria-live")).toBe("polite");
  expect(status.textContent).toContain("Loading asset lab");
  expect(status.querySelector(".mtk-spinner")?.getAttribute("aria-hidden")).toBe("true");
});

test("contains a failed lazy workspace, announces the error, and leaves a keyboard-operable recovery action", () => {
  vi.spyOn(console, "error").mockImplementation(() => {});
  function BrokenWorkspace(): never {
    throw new Error("dynamic import failed");
  }

  render(
    <LazyWorkspace label="Logic workspace">
      <BrokenWorkspace />
    </LazyWorkspace>,
  );

  const alert = screen.getByRole("alert");
  expect(alert.textContent).toContain("Logic workspace could not be loaded");
  expect(alert.textContent).toContain("still safe to use");
  const recovery = screen.getByRole("button", { name: "Reload editor" });
  recovery.focus();
  expect(document.activeElement).toBe(recovery);
});
