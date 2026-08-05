import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, expect, test, vi } from "vitest";
import { Onboarding } from "./Onboarding";

beforeEach(() => localStorage.clear());

test("workspace visibility controls rendering while the compatibility default remains visible", () => {
  const { rerender } = render(<Onboarding show={false} />);
  expect(screen.queryByTestId("onboarding")).toBeNull();

  rerender(<Onboarding />);
  const region = screen.getByRole("region", { name: "Make your first thing" });
  expect(region.getAttribute("aria-live")).toBe("polite");
  const disclosure = screen.getByRole("button", { name: "Make your first thing" });
  expect(disclosure.getAttribute("aria-expanded")).toBe("false");
  expect(disclosure.textContent).toContain("Place · bind · Play · Save");
  fireEvent.click(disclosure);
  expect(disclosure.getAttribute("aria-expanded")).toBe("true");
  expect(screen.getByText(/one-minute, skippable path/i)).toBeTruthy();
});

test("the primary CTA executes its callback, records completion, and closes", () => {
  const onStart = vi.fn();
  render(<Onboarding show onStart={onStart} />);

  fireEvent.click(screen.getByTestId("onboardStart"));
  expect(onStart).toHaveBeenCalledTimes(1);
  expect(localStorage.getItem("mtk.onboarded.v1")).toBe("1");
  expect(screen.queryByTestId("onboarding")).toBeNull();
});

test("Skip is an executable secondary CTA with its stable test hook", () => {
  const onSkip = vi.fn();
  render(<Onboarding show onSkip={onSkip} />);

  fireEvent.click(screen.getByTestId("onboardSkip"));
  expect(onSkip).toHaveBeenCalledTimes(1);
  expect(screen.queryByTestId("onboarding")).toBeNull();
});
