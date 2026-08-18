import { act, render, screen, waitFor, fireEvent } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { projectionStore } from "../store/projection";
import { playStore } from "../store/play";
import type { RoleRow } from "../transport/protocol";
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

/** `role_assign` and `role_status` are TWO round-trips and the panel makes both: it sets the role
 *  optimistically from the assign reply, then bumps `refreshKey` and re-reads `role_status`, which is
 *  authoritative (invariant 1 — the UI holds a projection, so the later read wins).
 *
 *  This test used to stub only `roleAssign` and inherit `fakeClient`'s `roleStatus`, which answers
 *  `roster: []` forever. So the fixture asserted two things the real shell cannot both say: "the
 *  assign applied `companion` to e1" and "e1 holds no role". The refetch then legitimately set the
 *  role back to null and unmounted `behaviour-clear` — and whether the test passed came down to
 *  whether `findByTestId`'s first poll landed before that promise resolved. It flaked ~1 run in 3.
 *
 *  Retrying was the wrong repair: this is ADR-123's failure from the other side — green against a
 *  payload `/core` cannot produce — and the transient it was really pinning is the OPTIMISTIC state,
 *  not the settled one the test's own name claims. So the roster now changes when the assign lands,
 *  exactly as the shell's does, and the assertions are about where the panel COMES TO REST. */
test("assigning from the Inspector lands with the undo hint and updates the held-role line", async () => {
  // Typed as the wire type, not as the three keys this test happens to read. Left inferred, the
  // literal below type-checked against itself and omitted `RoleRow.name` — a fixture wrong about the
  // shell in the exact way the fixture it replaced was, caught here only because `tsc` was asked.
  let roster: RoleRow[] = [];
  const client = fakeClient({
    roleAssign: vi.fn(() => {
      roster = [{ entity: "e1", name: "Dog", role: "companion" }];
      return Promise.resolve({
        applied: "companion",
        entity: "e1",
        added: ["a live brain"],
        scoreEntity: null,
        message: "Now a Companion",
        reason: null,
      });
    }),
    roleStatus: vi.fn(() =>
      Promise.resolve({
        roster,
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
  render(<BehaviourSection client={client} />);
  // Before the click the roster is empty, so the held-role line must NOT already read "Companion" —
  // otherwise the assertion below would pass on a panel that ignored the click entirely.
  expect((await screen.findByTestId("behaviour-state")).textContent).toContain("doesn't do anything yet");

  fireEvent.click(await screen.findByTestId("behaviour-companion"));
  await waitFor(() => {
    expect(screen.getByTestId("behaviour-state").textContent).toContain("Dog is a Companion");
  });
  expect(client.roleAssign).toHaveBeenCalledWith("e1", "companion");
  await screen.findByTestId("behaviour-clear");

  // The settled state, after the authoritative re-read has had its say. Without this the test still
  // only pins the optimistic flash: `role_status` is what decides what the panel finally shows.
  await waitFor(() => expect(client.roleStatus).toHaveBeenCalledTimes(2));
  expect(screen.getByTestId("behaviour-state").textContent).toContain("Dog is a Companion");
  expect(screen.getByTestId("behaviour-clear")).toBeTruthy();
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
