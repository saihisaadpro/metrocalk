//! JointPanel (M15.9 / ADR-079) — verified headless: a selected part offers the two plain-language joint
//! verbs; a jointed part shows the honesty-labeled source + drives value/key/scrub through the client.
//! Asserts the structured data-* signals + client calls, never prose.

import { afterEach, expect, test, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { JointPanel } from "./JointPanel";
import { projectionStore } from "../store/projection";
import { fakeClient } from "../transport/test-client";
import type { JointInfo } from "../transport/protocol";

afterEach(() => projectionStore.getState().reset());

function seed() {
  projectionStore.getState().bulkLoad([
    { id: "part1", name: "Trolley", parentId: null, components: { Transform: { x: 5, y: 0, z: 2 } } },
  ]);
  projectionStore.getState().select("part1");
}

test("an unjointed selection offers the two joint verbs and authors with the part's own pivot", async () => {
  seed();
  const setJoint = vi.fn(() => Promise.resolve(true));
  render(<JointPanel client={fakeClient({ setJoint, jointInfo: () => Promise.resolve(null) })} />);
  const make = await screen.findByTestId("make-revolute");
  fireEvent.click(make);
  await waitFor(() => expect(setJoint).toHaveBeenCalled());
  const [id, revolute, _axis, pivot, , , source] = setJoint.mock.calls[0] as unknown as [string, boolean, number[], number[], number, number, string];
  expect(id).toBe("part1");
  expect(revolute).toBe(true);
  expect(pivot).toEqual([5, 0, 2]); // the part's own position — never the scene origin
  expect(source).toBe("manual"); // honesty-labeled
});

test("a jointed selection shows the labeled source and scrubs the timeline through the client", async () => {
  seed();
  const info: JointInfo = { jointType: "revolute", axis: [0, 0, 1], pivot: [5, 0, 2], source: "manual", value: 0, min: -10, max: 10, trackEnd: 2, keys: 2 };
  const jointScrub = vi.fn(() => Promise.resolve(1));
  const jointKey = vi.fn(() => Promise.resolve(true));
  render(<JointPanel client={fakeClient({ jointInfo: () => Promise.resolve(info), jointScrub, jointKey })} />);
  const panel = await screen.findByTestId("joint-panel");
  await waitFor(() => expect(panel.getAttribute("data-joint-type")).toBe("revolute"));
  expect(panel.getAttribute("data-source")).toBe("manual");
  // Scrub drives the deterministic playback command.
  fireEvent.change(screen.getByTestId("joint-scrub"), { target: { value: "1" } });
  await waitFor(() => expect(jointScrub).toHaveBeenCalledWith(1));
  // Keying records the pose at the typed time.
  fireEvent.change(screen.getByTestId("joint-key-t"), { target: { value: "2" } });
  fireEvent.click(screen.getByTestId("joint-key"));
  await waitFor(() => expect(jointKey).toHaveBeenCalledWith("part1", 2));
});
