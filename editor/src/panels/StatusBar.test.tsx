//! StatusBar — the transient status line, verified headless in jsdom: an empty status renders the
//! neutral placeholder, and a `setStatus(...)` (from the ephemeral UI store) drives a LIVE update of
//! the same `#status` bar (subscription, not just initial render). Mirrors the scaffold's stable
//! `#status` signal at the component level.

import { afterEach, expect, test } from "vitest";
import { act, render, screen } from "@testing-library/react";
import { StatusBar } from "./StatusBar";
import { setStatus } from "../store/ui";
import { projectionStore } from "../store/projection";

afterEach(() => {
  projectionStore.getState().reset();
  act(() => setStatus(""));
});

test("empty status renders the neutral placeholder, then setStatus drives a live update", () => {
  render(<StatusBar />);

  // empty status → neutral placeholder, NOT a collapsed/blank bar
  const bar = screen.getByTestId("status");
  expect(bar.id).toBe("status");
  expect(bar.textContent).toBe("ready");

  // a real action message arrives on the ephemeral store → the SAME bar updates live (subscription)
  act(() => setStatus("bound HealthBar"));
  expect(screen.getByTestId("status").textContent).toBe("bound HealthBar");

  // and it tracks subsequent changes, including clearing back to the placeholder
  act(() => setStatus("topped up · balance 120"));
  expect(screen.getByTestId("status").textContent).toBe("topped up · balance 120");

  act(() => setStatus(""));
  expect(screen.getByTestId("status").textContent).toBe("ready");
});
