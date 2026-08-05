import { fireEvent, render, screen } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { FocusBanner } from "./FocusBanner";

test("focus state is announced and cleared through a native labelled button", () => {
  const onClear = vi.fn();
  render(<FocusBanner id="Pipe_Main" dist={8.25} onClear={onClear} />);

  const status = screen.getByRole("status");
  expect(status.getAttribute("aria-live")).toBe("polite");
  const button = screen.getByRole("button", { name: "Focused on Pipe_Main. Exit focus" });
  expect(button.id).toBe("focusbanner");
  expect(button.getAttribute("data-dist")).toBe("8.25");
  expect(button.getAttribute("data-focused")).toBe("true");

  fireEvent.click(button);
  expect(onClear).toHaveBeenCalledTimes(1);
});
