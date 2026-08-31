import { fireEvent, render, screen } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { FocusBanner } from "./FocusBanner";

test("focus state is announced and cleared through a native labelled button", () => {
  const onClear = vi.fn();
  render(<FocusBanner subject="Pipe_Main" dist={8.25} onClear={onClear} />);

  const status = screen.getByRole("status");
  expect(status.getAttribute("aria-live")).toBe("polite");
  const button = screen.getByRole("button", { name: "Focused on Pipe_Main. Exit focus" });
  expect(button.id).toBe("focusbanner");
  expect(button.getAttribute("data-dist")).toBe("8.25");
  expect(button.getAttribute("data-focused")).toBe("true");

  fireEvent.click(button);
  expect(onClear).toHaveBeenCalledTimes(1);
});

test("a focused SET is named as a set, not as one of its members", () => {
  // Focus frames the whole selection (ADR-194), so the banner has to be able to say so. An id-shaped
  // prop could only ever name one member, which is how "framed the selection" stayed true-looking
  // while thirteen of fourteen objects sat outside the frame.
  render(<FocusBanner subject="14 objects" dist={62.5} onClear={() => {}} />);
  expect(screen.getByRole("button", { name: "Focused on 14 objects. Exit focus" })).toBeTruthy();
});
