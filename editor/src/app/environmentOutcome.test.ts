//! The three outcomes of an environment reply, which two surfaces now read.
//!
//! The rule under test is that CANCELLED and REFUSED are not the same thing. Collapsing them is the
//! easy mistake — both leave the lighting unchanged — and it produces an editor that shouts "failed"
//! at somebody who pressed Escape.

import { expect, test } from "vitest";
import { environmentOutcome } from "./environmentOutcome";
import type { EnvironmentReply } from "../transport/protocol";

const BASE: EnvironmentReply = {
  applied: false, label: "", width: 0, height: 0, meanRadiance: [0, 0, 0],
  message: "", reason: null, path: null, cancelled: false,
};

test("a dismissed dialog says NOTHING — not an error, not a success", () => {
  expect(environmentOutcome({ ...BASE, cancelled: true, message: "No panorama chosen — the lighting is unchanged." })).toBeNull();
});

test("a refusal carries the engine's own reason, verbatim", () => {
  const reason = "that panorama is 900 MB, over the 512 MB limit - downsample it first";
  expect(environmentOutcome({ ...BASE, reason, message: reason })).toEqual({ tone: "error", message: reason });
});

test("a success reports what it bought", () => {
  const message = 'Lighting from "sunset_4k" (4096x2048) - it lights the scene and shows in reflections';
  expect(environmentOutcome({ ...BASE, applied: true, label: "sunset_4k", message })).toEqual({ tone: "success", message });
});

test("a success with no sentence still names what happened, never an empty toast", () => {
  expect(environmentOutcome({ ...BASE, applied: true, label: "Studio (built in)" })).toEqual({
    tone: "success",
    message: "Lighting from Studio (built in)",
  });
});

test("cancelled wins over a reason, because the engine sets both on a dismissal it explains", () => {
  // `import_environment` writes a message on cancel so a log line reads sensibly. If a future change
  // also set `reason` there, a reader that checked `reason` first would report an error for Escape.
  expect(environmentOutcome({ ...BASE, cancelled: true, reason: "no file" })).toBeNull();
});
