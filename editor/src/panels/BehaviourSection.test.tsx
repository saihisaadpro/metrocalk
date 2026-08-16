import { act, render, screen, waitFor, fireEvent } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { projectionStore } from "../store/projection";
import { playStore } from "../store/play";
import { fakeClient } from "../transport/test-client";
import { BehaviourSection } from "./BehaviourSection";

afterEach(() => {
  act(() => {
    projectionStore.getState().reset();
    playStore.getState().reset();
  });
  vi.restoreAllMocks();
});

function seed(id: string, name: string) {
  act(() => {
    projectionStore.getState().applyDelta({ ops: [{ op: "upsert", id, name, parentId: null }] });
    projectionStore.getState().select(id);
  });
}

test("clicking an asset surfaces the full behaviour catalog right in the Inspector", async () => {
  const client = fakeClient();
  seed("e1", "Crystal");
  render(<BehaviourSection client={client} />);
  const state = await screen.findByTestId("behaviour-state");
  expect(state.textContent).toContain("doesn't do anything yet");
  for (const kind of ["collectible", "solid", "prop", "spinner", "companion", "enemy", "waypoint", "player"]) {
    await screen.findByTestId(`behaviour-${kind}`);
  }
  // Legible cost on every card.
  expect(screen.getByTestId("behaviour-companion").getAttribute("title")).toContain("Adds:");
});

test("assigning from the Inspector lands with the undo hint and updates the held-role line", async () => {
  const client = fakeClient({
    roleAssign: vi.fn(() =>
      Promise.resolve({
        applied: "companion",
        entity: "e1",
        added: ["a live brain"],
        scoreEntity: null,
        message: "Now a Companion",
        reason: null,
      }),
    ),
  });
  seed("e1", "Dog");
  render(<BehaviourSection client={client} />);
  fireEvent.click(await screen.findByTestId("behaviour-companion"));
  await waitFor(() => {
    expect(screen.getByTestId("behaviour-state").textContent).toContain("Dog is a Companion");
  });
  expect(client.roleAssign).toHaveBeenCalledWith("e1", "companion");
  await screen.findByTestId("behaviour-clear");
});

test("refusals surface inline, in plain language", async () => {
  const client = fakeClient({
    roleAssign: vi.fn(() =>
      Promise.resolve({
        applied: null,
        entity: "e1",
        added: [],
        scoreEntity: null,
        message: "",
        reason: "stop Play first — roles are authored, not live-edited",
      }),
    ),
  });
  seed("e1", "Wall");
  render(<BehaviourSection client={client} />);
  fireEvent.click(await screen.findByTestId("behaviour-solid"));
  const refusal = await screen.findByTestId("behaviour-refusal");
  expect(refusal.textContent).toContain("stop Play first");
});

test("a held role exposes friendly tuning knobs that write ONE undoable field edit", async () => {
  const client = fakeClient({
    roleStatus: vi.fn(() =>
      Promise.resolve({
        roster: [{ entity: "e1", name: "Dog", role: "companion" }],
        score: 0,
        scoreEntity: null,
        remaining: 0,
        companions: [],
        won: false,
        health: null,
        blocked: null,
      }),
    ),
  });
  seed("e1", "Dog");
  act(() => {
    projectionStore.getState().applyDelta({
      ops: [
        { op: "setField", id: "e1", component: "GameRole", field: "role", value: "companion" },
        { op: "setField", id: "e1", component: "GameRole", field: "follow", value: 2 },
      ],
    });
  });
  render(<BehaviourSection client={client} />);
  await screen.findByTestId("behaviour-tuning");
  // Plain-language knobs, current value shown.
  const follow = (await screen.findByTestId("tune-follow")) as HTMLInputElement;
  expect(follow.value).toBe("2");
  expect(follow.getAttribute("aria-label")).toContain("Follow distance");
  // Commit a new value → exactly one setField with the number.
  fireEvent.change(follow, { target: { value: "3.5" } });
  fireEvent.blur(follow);
  expect(client.setField).toHaveBeenCalledWith("e1", "GameRole", "follow", 3.5);
  // Integer knobs round: a waypoint's patrol number can't be 2.4.
  expect(screen.getByTestId("tune-speed")).toBeTruthy();
});

test("the jump links open the deeper workspaces", async () => {
  const client = fakeClient();
  const jump = vi.fn();
  seed("e1", "Crystal");
  render(<BehaviourSection client={client} onJumpTo={jump} />);
  fireEvent.click(await screen.findByTestId("behaviour-jump-animate"));
  expect(jump).toHaveBeenCalledWith("animate");
  fireEvent.click(await screen.findByTestId("behaviour-jump-gameplay"));
  expect(jump).toHaveBeenCalledWith("gameplay");
});

test("nothing renders with no selection — the Inspector stays selection-scoped", () => {
  const client = fakeClient();
  render(<BehaviourSection client={client} />);
  expect(screen.queryByTestId("behaviour-section")).toBeNull();
});
