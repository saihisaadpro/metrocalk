import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, expect, test } from "vitest";
import { projectionStore } from "../store/projection";
import { Rejections } from "./Rejections";

afterEach(() => {
  act(() => projectionStore.getState().reset());
});

test("a rejected edit is assertively announced and can be dismissed with a labelled button", () => {
  act(() => {
    projectionStore.getState().applyDelta({
      ops: [],
      rejects: [{ clientOpId: "op-7", reason: "Collider generation needs a closed mesh" }],
    });
  });
  render(<Rejections />);

  const alert = screen.getByRole("alert");
  expect(alert.getAttribute("aria-live")).toBe("assertive");
  expect(alert.getAttribute("aria-atomic")).toBe("true");
  expect(screen.getByTestId("reject").getAttribute("aria-label")).toBe("Rejected changes");

  fireEvent.click(screen.getByRole("button", { name: /Dismiss rejection: Collider generation/i }));
  expect(screen.queryByTestId("reject")).toBeNull();
});
