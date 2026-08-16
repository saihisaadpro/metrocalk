// The Formats panel, and specifically the colour card's honesty.
//
// The interesting assertions here are not "it renders". They are that the panel refuses to flatter
// itself: a capability that is NOT wired must appear, labelled, next to the ones that are. A colour
// panel listing only its successes is precisely the panel that lets somebody assume ACES runs
// end-to-end when only the view transform was hooked up — and then grade a shot against it.

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { FormatsPanel } from "./FormatsPanel";
import { fakeClient } from "../transport/test-client";

test("the colour card names the working space and the LIVE view transform", async () => {
  const client = fakeClient();
  render(<FormatsPanel client={client} />);

  await waitFor(() => expect(screen.getByTestId("colour-card")).toBeTruthy());
  expect(screen.getByTestId("colour-working").textContent).toBe("Linear Rec.709");
  // Not merely "a filmic transform exists" — the card says which one is running.
  expect(screen.getByTestId("colour-card").getAttribute("data-active-view")).toBe("acesFit");
  expect(screen.getByTestId("colour-view-acesFit").getAttribute("aria-pressed")).toBe("true");
  expect(screen.getByTestId("colour-view-pbrNeutral").getAttribute("aria-pressed")).toBe("false");
});

test("a capability that is NOT wired is shown and labelled, not omitted", async () => {
  const client = fakeClient();
  render(<FormatsPanel client={client} />);

  await waitFor(() => expect(screen.getByTestId("colour-card")).toBeTruthy());

  const wired = screen.getByTestId("colour-cap-sceneLinearWorkingSpace");
  expect(wired.getAttribute("data-on")).toBe("true");

  // The one that matters: present in the DOM, marked off, and carrying the words that stop a reader
  // assuming otherwise. It is OCIO now rather than ACEScg — ACEScg IS the working space since this
  // work, and the honest unwired row moved to the thing that genuinely is not there.
  const notWired = screen.getByTestId("colour-cap-ocioConfigLoading");
  expect(notWired.getAttribute("data-on")).toBe("false");
  expect(notWired.textContent).toContain("not wired");
});

test("choosing a view transform drives the renderer's own profile, not a parallel setting", async () => {
  const client = fakeClient();
  const setProfile = vi.fn((p: "cinematic" | "cad") => Promise.resolve(p));
  client.setRenderProfile = setProfile;
  render(<FormatsPanel client={client} />);
  await waitFor(() => expect(screen.getByTestId("colour-card")).toBeTruthy());

  fireEvent.click(screen.getByTestId("colour-view-pbrNeutral"));

  // `set_render_profile` is the switch the viewport toolbar already uses. Two controls for one piece
  // of renderer state is how they end up disagreeing, so the colour card reuses it rather than
  // introducing a second source of truth.
  await waitFor(() => expect(setProfile).toHaveBeenCalledWith("cad"));
});

test("the scope note is shown, because a limit nobody reads is not a limit", async () => {
  const client = fakeClient();
  render(<FormatsPanel client={client} />);
  await waitFor(() => expect(screen.getByTestId("colour-card")).toBeTruthy());
  expect(screen.getByTestId("colour-card").textContent).toContain("not available in this build");
});

test("choosing a working space asks the renderer, and re-reads rather than assuming", async () => {
  const client = fakeClient();
  const setWorking = vi.fn((s: string) => Promise.resolve(s));
  client.setWorkingSpace = setWorking;
  render(<FormatsPanel client={client} />);
  await waitFor(() => expect(screen.getByTestId("colour-card")).toBeTruthy());

  expect(screen.getByTestId("colour-working-linearRec709").getAttribute("aria-pressed")).toBe("true");
  fireEvent.click(screen.getByTestId("colour-working-acesCg"));

  await waitFor(() => expect(setWorking).toHaveBeenCalledWith("acesCg"));
  // The renderer is allowed to refuse. The card therefore re-reads the status instead of showing the
  // value it just asked for — a control that reported its own request would be unable to show a refusal.
  await waitFor(() => expect(client.colourStatus).toHaveBeenCalledTimes(2));
});

test("the environment's colour space is declarable, and says when it is only assumed", async () => {
  const client = fakeClient();
  const setEnv = vi.fn((s: string) => Promise.resolve(s));
  client.setEnvironmentColourSpace = setEnv;
  render(<FormatsPanel client={client} />);
  await waitFor(() => expect(screen.getByTestId("colour-card")).toBeTruthy());

  // The default is an ASSUMPTION — Radiance .hdr has no required primaries header — and the card has
  // to say so, or a person cannot tell a declaration from a guess.
  expect(screen.getByTestId("colour-card").textContent).toContain("assumed");
  fireEvent.change(screen.getByTestId("colour-env-space"), { target: { value: "acesCg" } });
  await waitFor(() => expect(setEnv).toHaveBeenCalledWith("acesCg"));
});

test("the panel still renders when the engine has no colour answer", async () => {
  // The colour card is additive: an engine that does not answer `colour_status` must leave the format
  // list working rather than blanking the tab.
  const client = fakeClient();
  client.colourStatus = vi.fn(() => Promise.reject(new Error("no such command")));
  client.formatCatalog = vi.fn(() =>
    Promise.resolve([
      {
        id: "gltf",
        label: "glTF 2.0",
        extensions: ["gltf", "glb"],
        domain: "Game & real-time",
        direction: "both" as const,
        fidelity: "full" as const,
        carries: {
          geometry: true,
          hierarchy: true,
          materials: true,
          textures: true,
          skinning: true,
          animation: true,
          cameras: true,
          metadata: false,
          physics: false,
        },
        note: "The engine's native interchange.",
        available: true,
      },
    ]),
  );

  render(<FormatsPanel client={client} />);
  await waitFor(() => expect(screen.getByTestId("format-gltf")).toBeTruthy());
  expect(screen.queryByTestId("colour-card")).toBeNull();
});
