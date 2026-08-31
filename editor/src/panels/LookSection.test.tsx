//! The scene's look — a capability that existed in the engine and had no way in.
//!
//! The assertions worth having here are not "it renders". They are that the panel reports what the
//! ENGINE said (the label, the size, the measured brightness), that a dismissed file dialog is not
//! drawn as a failure, that a refusal keeps the engine's own sentence rather than inventing one, and
//! that the exposure slider walks stops rather than a linear range. Each of those is a rule that would
//! be silently lost by a refactor and would look fine on screen until someone relied on it.

import { afterEach, expect, test, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { LookSection, brightnessSentence, EXPOSURE_STOPS, nearestStop, stopsFromDefault } from "./LookSection";
import { projectionStore } from "../store/projection";
import { projectStore } from "../store/project";
import { fakeClient } from "../transport/test-client";

afterEach(() => {
  projectionStore.getState().reset();
  projectStore.getState().reset();
});

const STUDIO = {
  applied: false, label: "Studio (built in)", width: 0, height: 0,
  meanRadiance: [0, 0, 0] as [number, number, number],
  message: "", reason: null, path: null, cancelled: false,
};

test("it reports what the ENGINE is lit by, not a default it made up", async () => {
  const environmentState = vi.fn(() =>
    Promise.resolve({ ...STUDIO, applied: true, label: "kloppenheim_06", width: 4096, height: 2048, meanRadiance: [0.5, 0.5, 0.5] as [number, number, number] }),
  );
  render(<LookSection client={fakeClient({ environmentState })} />);

  await waitFor(() => expect(screen.getByTestId("look-env-label").textContent).toBe("kloppenheim_06"));
  // The size is the legible-cost line: a 4k panorama is a real memory decision, and the panel says so.
  expect(screen.getByTestId("look-section").textContent).toContain("4096×2048");
  // A reset is only offered when there is something to reset FROM.
  expect(screen.queryByTestId("look-env-reset")).toBeTruthy();
});

test("with the built-in sky there is nothing to reset, so no reset control is drawn", async () => {
  render(<LookSection client={fakeClient()} />);
  await waitFor(() => expect(screen.getByTestId("look-env-label").textContent).toBe("Studio (built in)"));
  expect(screen.queryByTestId("look-env-reset")).toBeNull();
  // And no brightness reading, because there is no measured panorama to have one.
  expect(screen.queryByTestId("look-brightness")).toBeNull();
});

test("choosing a sky asks the ENGINE to open the picker — the panel never asks for a path", async () => {
  const importEnvironment = vi.fn(() =>
    Promise.resolve({ ...STUDIO, applied: true, label: "sunset_4k", width: 4096, height: 2048, meanRadiance: [0.62, 0.48, 0.36] as [number, number, number], message: 'Lighting from "sunset_4k" (4096x2048)', path: "C:/skies/sunset_4k.hdr" }),
  );
  render(<LookSection client={fakeClient({ importEnvironment })} />);
  await waitFor(() => expect(screen.getByTestId("look-env-choose")).toBeTruthy());

  fireEvent.click(screen.getByTestId("look-env-choose"));

  // No argument: "not given" is precisely what makes `import_environment` open the native dialog. A
  // panel that passed a path here would be a panel that had to have obtained one somehow.
  await waitFor(() => expect(importEnvironment).toHaveBeenCalledWith());
  await waitFor(() => expect(screen.getByTestId("look-env-label").textContent).toBe("sunset_4k"));
});

test("a DISMISSED file dialog is not a failure — nothing is said and nothing changes", async () => {
  const importEnvironment = vi.fn(() =>
    Promise.resolve({ ...STUDIO, cancelled: true, message: "No panorama chosen — the lighting is unchanged." }),
  );
  render(<LookSection client={fakeClient({ importEnvironment })} />);
  await waitFor(() => expect(screen.getByTestId("look-env-choose")).toBeTruthy());

  fireEvent.click(screen.getByTestId("look-env-choose"));
  await waitFor(() => expect(importEnvironment).toHaveBeenCalled());

  // The refusal callout is the thing that must NOT appear: changing your mind is not an error, and a
  // panel that reports it as one teaches people to distrust its errors.
  await waitFor(() => expect(screen.queryByTestId("look-refusal")).toBeNull());
  expect(screen.getByTestId("look-env-label").textContent).toBe("Studio (built in)");
});

test("a refusal shows the ENGINE's own sentence, not a generic one", async () => {
  const importEnvironment = vi.fn(() =>
    Promise.resolve({ ...STUDIO, reason: "that is not a readable Radiance HDR panorama (bad magic)", message: "that is not a readable Radiance HDR panorama (bad magic)" }),
  );
  render(<LookSection client={fakeClient({ importEnvironment })} />);
  await waitFor(() => expect(screen.getByTestId("look-env-choose")).toBeTruthy());

  fireEvent.click(screen.getByTestId("look-env-choose"));

  await waitFor(() => expect(screen.getByTestId("look-refusal").textContent).toContain("Radiance HDR panorama"));
});

test("the exposure slider walks STOPS, and reports the value in stops from the default", async () => {
  const setExposure = vi.fn((e: number) => Promise.resolve(e));
  render(<LookSection client={fakeClient({ setExposure })} />);
  const slider = (await screen.findByTestId("look-exposure")) as HTMLInputElement;

  // The range is an INDEX into the stop table, not a linear 0.05..8 — that is the whole reason the
  // control is usable, and a refactor to a linear range would still look like a slider.
  expect(slider.max).toBe(String(EXPOSURE_STOPS.length - 1));
  expect(slider.value).toBe(String(nearestStop(0.45)));

  fireEvent.change(slider, { target: { value: String(EXPOSURE_STOPS.indexOf(1.8)) } });
  await waitFor(() => expect(setExposure).toHaveBeenCalledWith(1.8));
  expect(screen.getByTestId("look-section").textContent).toContain("+2.0 stops");
});

test("the panel counts the scene's OWN lights, so 'nothing changed' has an answer on screen", async () => {
  projectionStore.getState().bulkLoad([
    { id: "key", name: "Key", parentId: null, components: { Light: { intensity: 60 } } },
    { id: "fill", name: "Fill", parentId: null, components: { Light: { intensity: 20 } } },
    { id: "box", name: "Box", parentId: null, components: { MeshRenderer: { mesh: "cube" } } },
  ]);
  render(<LookSection client={fakeClient()} />);
  await waitFor(() => expect(screen.getByTestId("look-lights").textContent).toContain("2 lights"));
  // Settle the two reads the panel makes on mount before the test ends, so a resolved promise cannot
  // land on an unmounted tree and report itself as an un-acted update.
  await waitFor(() => expect(screen.getByTestId("look-env-label").textContent).toBe("Studio (built in)"));
});

test("a scene with no lights says so — that is the case where the sky does everything", async () => {
  projectionStore.getState().bulkLoad([
    { id: "box", name: "Box", parentId: null, components: { MeshRenderer: { mesh: "cube" } } },
  ]);
  render(<LookSection client={fakeClient()} />);
  await waitFor(() => expect(screen.getByTestId("look-lights").textContent).toContain("No lights"));
  await waitFor(() => expect(screen.getByTestId("look-env-label").textContent).toBe("Studio (built in)"));
});

test("the brightness reading is METERED against mid grey, with the working space's own weights", () => {
  // Rec.709 weights over a flat 0.18 panorama is exactly mid grey — the calibration point, so a sign
  // error or a swapped weight vector cannot pass.
  expect(brightnessSentence([0.18, 0.18, 0.18], [0.2126, 0.7152, 0.0722])).toContain("about mid grey");
  // Twice mid grey is one stop over, and the sentence says which side of it we are on.
  expect(brightnessSentence([0.36, 0.36, 0.36], [0.2126, 0.7152, 0.0722])).toContain("1.0 stops over");
  expect(brightnessSentence([0.09, 0.09, 0.09], [0.2126, 0.7152, 0.0722])).toContain("1.0 stops under");
  // ACEScg weights are genuinely different — a green-heavy sky meters differently in the two spaces,
  // which is the reason the panel reads `luminanceWeights` instead of hard-coding Rec.709.
  const green: [number, number, number] = [0.05, 0.4, 0.05];
  expect(brightnessSentence(green, [0.2126, 0.7152, 0.0722])).not.toBe(
    brightnessSentence(green, [0.2722287, 0.6740818, 0.0536895]),
  );
  // Nothing to report is reported as nothing, never as "0.00 average, 100 stops under".
  expect(brightnessSentence([0, 0, 0], [0.2126, 0.7152, 0.0722])).toBeNull();
});

test("stopsFromDefault names the renderer's default as 'default', not as '0.0 stops'", () => {
  expect(stopsFromDefault(0.45)).toBe("default");
  expect(stopsFromDefault(0.9)).toBe("+1.0 stops");
  expect(stopsFromDefault(0.225)).toBe("−1.0 stops");
});

test("opening a PROJECT re-reads the sky — the panel reports the scene, not the one it mounted for", async () => {
  // The sky is not document state, so no projection delta announces it. `sessionId` is the store's own
  // "this is a different document now" signal, and without keying the read to it the panel keeps
  // reporting the previous project's panorama with the renderer showing the new one.
  let lit = { ...STUDIO };
  const environmentState = vi.fn(() => Promise.resolve(lit));
  render(<LookSection client={fakeClient({ environmentState })} />);
  await waitFor(() => expect(screen.getByTestId("look-env-label").textContent).toBe("Studio (built in)"));

  lit = { ...STUDIO, applied: true, label: "overcast_soho", width: 2048, height: 1024, meanRadiance: [0.3, 0.3, 0.3] };
  act(() => {
    projectStore.getState().switchProject({ path: "C:/work/other.mtk", dirty: false, recents: [], error: null });
  });

  await waitFor(() => expect(screen.getByTestId("look-env-label").textContent).toBe("overcast_soho"));
  expect(environmentState).toHaveBeenCalledTimes(2);
});
