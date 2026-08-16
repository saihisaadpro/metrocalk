import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { projectionStore } from "../store/projection";
import { playStore } from "../store/play";
import { toastStore } from "../store/toasts";
import { fakeClient } from "../transport/test-client";
import { RolesSection } from "./RolesSection";

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
  });
}

test("with nothing selected the section teaches the gesture; selecting shows the four role cards", async () => {
  const client = fakeClient();
  render(<RolesSection client={client} />);
  expect((await screen.findByTestId("roles-hint")).textContent).toContain("Select an object");

  seed("e1", "Crystal");
  act(() => projectionStore.getState().select("e1"));
  for (const kind of ["collectible", "solid", "prop", "spinner"]) {
    await screen.findByTestId(`role-${kind}`);
  }
  // Legible cost: the card's title says exactly what assigning adds.
  const card = screen.getByTestId("role-collectible");
  expect(card.getAttribute("title")).toContain("Adds:");
  expect(card.getAttribute("title")).toContain("Ctrl-Z");
});

test("assigning a role toasts with the undo hint and refreshes the roster", async () => {
  const client = fakeClient({
    roleStatus: vi
      .fn()
      .mockResolvedValueOnce({ roster: [], score: 0, scoreEntity: null, remaining: 0, companions: [], won: false, blocked: null })
      .mockResolvedValue({
        roster: [{ entity: "e1", name: "Crystal", role: "collectible" }],
        score: 0,
        scoreEntity: "s1",
        remaining: 1,
        companions: [],
        won: false,
        health: null,
        blocked: null,
      }),
  });
  seed("e1", "Crystal");
  act(() => projectionStore.getState().select("e1"));
  render(<RolesSection client={client} />);

  fireEvent.click(await screen.findByTestId("role-collectible"));
  await waitFor(() => expect(client.roleAssign).toHaveBeenCalledWith("e1", "collectible"));
  expect(toastStore.getState().toasts.some((t) => t.text.includes("Ctrl-Z to undo"))).toBe(true);
  // The roster now lists it, marked with its role.
  const row = await screen.findByTestId("role-row");
  expect(row.textContent).toContain("Crystal");
  expect(row.textContent).toContain("collectible");
});

test("a refusal is explained inline and the roster stays unchanged", async () => {
  const client = fakeClient({
    roleAssign: vi.fn(() =>
      Promise.resolve({
        applied: null,
        entity: null,
        added: [],
        scoreEntity: null,
        message: "no",
        reason: "stop Play first — roles are authored, not live-edited",
      }),
    ),
  });
  seed("e1", "Crystal");
  act(() => projectionStore.getState().select("e1"));
  render(<RolesSection client={client} />);
  fireEvent.click(await screen.findByTestId("role-collectible"));
  expect((await screen.findByTestId("role-refusal")).textContent).toContain("stop Play first");
  expect(screen.queryByTestId("role-row")).toBeNull();
});

test("during Play each companion reports what it is doing, live", async () => {
  const client = fakeClient({
    roleStatus: vi.fn(() =>
      Promise.resolve({
        roster: [{ entity: "dog", name: "Dog", role: "companion" }],
        score: 0,
        scoreEntity: null,
        remaining: 0,
        companions: [{ entity: "dog", name: "Dog", doing: "hunting Skeleton" }],
        won: false,
        health: null,
        blocked: null,
      }),
    ),
  });
  act(() => {
    playStore.getState().refresh({ playing: true, paused: false });
  });
  render(<RolesSection client={client} />);
  const status = await screen.findByTestId("companion-status");
  expect(status.textContent).toContain("Dog");
  expect(status.textContent).toContain("hunting Skeleton");
});

test("a blocked rule is announced where the player is looking, not buried", async () => {
  const client = fakeClient({
    roleStatus: vi.fn(() =>
      Promise.resolve({
        roster: [{ entity: "door", name: "Door", role: "collectible" }],
        score: 0,
        scoreEntity: "s1",
        remaining: 1,
        companions: [],
        won: false,
        health: null,
        blocked: { name: "Door", rule: "Collect on touch", why: "waiting on: Key is gone" },
      }),
    ),
  });
  act(() => {
    playStore.getState().refresh({ playing: true, paused: false });
  });
  render(<RolesSection client={client} />);
  const line = await screen.findByTestId("roles-blocked");
  expect(line.textContent).toContain("Door");
  expect(line.textContent).toContain("waiting on: Key is gone");
});

test("victory shows the banner the moment every objective is done", async () => {
  const client = fakeClient({
    roleStatus: vi.fn(() =>
      Promise.resolve({
        roster: [{ entity: "sk", name: "Skeleton", role: "enemy" }],
        score: 1,
        scoreEntity: "s1",
        remaining: 0,
        companions: [],
        won: true,
        health: null,
        blocked: null,
      }),
    ),
  });
  act(() => {
    playStore.getState().refresh({ playing: true, paused: false });
  });
  render(<RolesSection client={client} />);
  const banner = await screen.findByTestId("role-victory");
  expect(banner.textContent).toContain("You won");
});

test("during Play the section shows the LIVE score and disables authoring with a reason", async () => {
  const client = fakeClient({
    roleStatus: vi.fn(() =>
      Promise.resolve({
        roster: [{ entity: "e1", name: "Crystal", role: "collectible" }],
        score: 2,
        scoreEntity: "s1",
        remaining: 1,
        companions: [],
        won: false,
        health: null,
        blocked: null,
      }),
    ),
  });
  seed("e1", "Crystal");
  act(() => {
    projectionStore.getState().select("e1");
    playStore.getState().refresh({ playing: true, paused: false });
  });
  render(<RolesSection client={client} />);

  const score = await screen.findByTestId("role-score");
  expect(score.textContent).toContain("2");
  expect(score.textContent).toContain("1 collectible left");
  const card = (await screen.findByTestId("role-collectible")) as HTMLButtonElement;
  expect(card.disabled).toBe(true);
  expect(card.title).toContain("Stop Play first");
});

test("clicking a roster row selects that entity (the panel writes selection, ADR-107 style)", async () => {
  const client = fakeClient({
    roleStatus: vi.fn(() =>
      Promise.resolve({
        roster: [{ entity: "e9", name: "Gate", role: "solid" }],
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
  seed("e9", "Gate");
  render(<RolesSection client={client} />);
  fireEvent.click(await screen.findByTestId("role-row"));
  expect(projectionStore.getState().selectedId).toBe("e9");
});
