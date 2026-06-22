//! toasts store (M10.10 / C11) — verified headless: a pushed toast appends with a stable monotonic id
//! (no Math.random), in order; dismiss removes exactly one. The auto-dismiss timer lives in the host, so
//! the store stays pure + synchronously testable.

import { afterEach, expect, test } from "vitest";
import { toastStore, pushToast } from "./toasts";

afterEach(() => toastStore.getState().reset());

test("push appends a toast with a stable monotonic id, in order; dismiss removes exactly one", () => {
  const a = pushToast("created HealthBar", "success");
  const b = pushToast("−2 tokens · 98 left", "cost");
  expect(b).toBeGreaterThan(a); // monotonic ids, stable across runs

  expect(toastStore.getState().toasts.map((t) => t.text)).toEqual(["created HealthBar", "−2 tokens · 98 left"]);
  expect(toastStore.getState().toasts.map((t) => t.kind)).toEqual(["success", "cost"]);

  toastStore.getState().dismiss(a);
  expect(toastStore.getState().toasts.map((t) => t.text)).toEqual(["−2 tokens · 98 left"]);
});
