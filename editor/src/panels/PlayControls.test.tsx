//! Play/Stop/Pause controls (M10.4 / ADR-034) — verified headless in jsdom: Play enters runtime and
//! shows the indicator + an always-reachable Stop; Pause toggles paused/resumed; Stop returns to
//! authoring (the indicator clears, Play returns). The runtime state is mirrored from the client into
//! the play store (so edit affordances can disable themselves while playing).

import { afterEach, expect, test, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { PlayControls } from "./PlayControls";
import { playStore } from "../store/play";
import { uiStore } from "../store/ui";
import { fakeClient } from "../transport/test-client";
import type { PlayInfo } from "../store/play";

afterEach(() => {
  playStore.getState().reset();
  uiStore.getState().setStatus("");
});

const info = (over: Partial<PlayInfo> = {}): PlayInfo => ({ playing: false, paused: false, ...over });

test("Stopped shows Play; no Stop/indicator until running", async () => {
  render(<PlayControls client={fakeClient()} />);
  await waitFor(() => expect(screen.getByTestId("play")).toBeTruthy());
  expect(screen.queryByTestId("stop")).toBeNull();
  expect(screen.queryByTestId("playIndicator")).toBeNull();
});

test("Play enters runtime: indicator shown + Stop always reachable", async () => {
  const play = vi.fn(() => Promise.resolve(info({ playing: true })));
  render(<PlayControls client={fakeClient({ play })} />);
  await waitFor(() => expect(screen.getByTestId("play")).toBeTruthy());

  fireEvent.click(screen.getByTestId("play"));

  await waitFor(() => expect(play).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(playStore.getState().playing).toBe(true));
  expect(screen.getByTestId("playIndicator").textContent).toBe("● PLAYING");
  expect(screen.getByTestId("stop")).toBeTruthy(); // Stop is reachable while playing
  expect(screen.queryByTestId("play")).toBeNull(); // Play is replaced by Pause/Stop
});

test("Pause freezes (indicator → PAUSED), and toggling resumes", async () => {
  let paused = false;
  const client = fakeClient({
    playState: () => Promise.resolve(info({ playing: true, paused })),
    pause: vi.fn(() => {
      paused = !paused;
      return Promise.resolve(info({ playing: true, paused }));
    }),
  });
  render(<PlayControls client={client} />);
  await waitFor(() => expect(playStore.getState().playing).toBe(true));

  fireEvent.click(screen.getByTestId("pause"));
  await waitFor(() => expect(playStore.getState().paused).toBe(true));
  expect(screen.getByTestId("playIndicator").textContent).toBe("⏸ PAUSED");

  fireEvent.click(screen.getByTestId("pause")); // resume
  await waitFor(() => expect(playStore.getState().paused).toBe(false));
  expect(screen.getByTestId("playIndicator").textContent).toBe("● PLAYING");
});

test("Stop returns to authoring (indicator clears, Play returns)", async () => {
  const stop = vi.fn(() => Promise.resolve(info({ playing: false })));
  const client = fakeClient({ playState: () => Promise.resolve(info({ playing: true })), stop });
  render(<PlayControls client={client} />);
  await waitFor(() => expect(screen.getByTestId("stop")).toBeTruthy());

  fireEvent.click(screen.getByTestId("stop"));

  await waitFor(() => expect(stop).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(playStore.getState().playing).toBe(false));
  expect(screen.queryByTestId("playIndicator")).toBeNull();
  expect(screen.getByTestId("play")).toBeTruthy();
});
