//! Scene cameras, from the user's side: save a view, look through it, re-aim it, change its lens — and
//! the two states the surface has to be honest about (a camera with no aim, and the one control that is
//! deliberately off while you are inside the camera it acts on).

import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { CameraSection } from "./CameraSection";
import {
  FOV_MAX_DEG,
  FOV_MIN_DEG,
  focalLengthMm,
  lookThrough,
  recaptureCamera,
  saveCurrentView,
} from "./cameraActions";
import { cameraStore } from "../store/cameras";
import { projectionStore } from "../store/projection";
import { toastStore } from "../store/toasts";
import { fakeClient } from "../transport/test-client";
import type { SceneCameraInfo } from "../transport/protocol";

function camera(over: Partial<SceneCameraInfo> = {}): SceneCameraInfo {
  return {
    id: "cam-1",
    name: "Down the line",
    pos: [30, 6, 0],
    lookAt: [0, 1.5, 0],
    fovDeg: 55,
    near: 3,
    far: 40,
    active: true,
    ...over,
  };
}

beforeEach(() => {
  act(() => {
    cameraStore.getState().reset();
    toastStore.getState().reset();
    projectionStore.getState().select(null);
  });
});

/** Put a camera in the store and select it, the way the engine's refresh + a pick would. */
function selectCamera(cams: SceneCameraInfo[], selected = cams[0]?.id ?? null) {
  act(() => {
    cameraStore.getState().refresh(cams);
    projectionStore.getState().select(selected);
  });
}

describe("CameraSection", () => {
  it("renders nothing at all when the selection is not a camera", () => {
    const client = fakeClient();
    client.sceneCameras = vi.fn(() => Promise.resolve([camera()]));
    selectCamera([camera()], "some-other-object");
    const { container } = render(<CameraSection client={client} />);
    expect(container.querySelector("[data-testid='camera-section']")).toBeNull();
  });

  it("shows where the camera stands and what it looks at", async () => {
    const client = fakeClient();
    client.sceneCameras = vi.fn(() => Promise.resolve([camera()]));
    selectCamera([camera()]);
    render(<CameraSection client={client} />);
    const pose = await screen.findByTestId("cameraPose");
    expect(pose.textContent).toMatch(/30, 6\.0, 0\.0/);
    expect(pose.textContent).toMatch(/0\.0, 1\.5, 0\.0/);
  });

  /** The defect the whole capability exists to end, made visible: a camera with no aim follows the
   *  editor's view, and in a picture that is indistinguishable from a camera that is aimed. */
  it("says out loud when a camera has no aim, instead of showing plausible numbers", async () => {
    const client = fakeClient();
    const unaimed = camera({ lookAt: null });
    client.sceneCameras = vi.fn(() => Promise.resolve([unaimed]));
    selectCamera([unaimed]);
    render(<CameraSection client={client} />);
    const why = await screen.findByTestId("cameraUnaimed");
    expect(why.textContent).toMatch(/follows the editor/i);
  });

  it("looks through the camera, and offers the way back out", async () => {
    const client = fakeClient();
    const cam = camera();
    client.sceneCameras = vi.fn(() => Promise.resolve([cam]));
    selectCamera([cam]);
    render(<CameraSection client={client} />);

    fireEvent.click(await screen.findByTestId("cameraLookThrough"));
    await waitFor(() => expect(client.lookThroughCamera).toHaveBeenCalledWith(true));
    await waitFor(() =>
      expect(screen.getByTestId("cameraLookThrough").textContent).toMatch(/free look/i),
    );

    fireEvent.click(screen.getByTestId("cameraLookThrough"));
    await waitFor(() => expect(client.lookThroughCamera).toHaveBeenCalledWith(false));
  });

  /** `<ux_quality>` 6 — a disabled control explains WHY, in plain words. Re-aiming a camera at its own
   *  picture is a commit that changes nothing. */
  it("turns Point at this view off while you are inside that camera, and says why", async () => {
    const client = fakeClient();
    const cam = camera();
    client.sceneCameras = vi.fn(() => Promise.resolve([cam]));
    selectCamera([cam]);
    render(<CameraSection client={client} />);

    expect((screen.getByTestId("cameraRecapture") as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(await screen.findByTestId("cameraLookThrough"));
    await waitFor(() =>
      expect((screen.getByTestId("cameraRecapture") as HTMLButtonElement).disabled).toBe(true),
    );
    expect(screen.getByTestId("cameraRecaptureWhy").textContent).toMatch(/already shows this/i);
  });

  /** One undo step per drag, not one per pixel — the slider tracks the pointer and commits on release. */
  it("commits the lens once, on release, not on every step of the drag", async () => {
    const client = fakeClient();
    const cam = camera();
    client.sceneCameras = vi.fn(() => Promise.resolve([cam]));
    selectCamera([cam]);
    render(<CameraSection client={client} />);
    const slider = await screen.findByTestId("cameraFov");

    fireEvent.change(slider, { target: { value: "40" } });
    fireEvent.change(slider, { target: { value: "30" } });
    fireEvent.change(slider, { target: { value: "24" } });
    expect(client.setCameraFov).not.toHaveBeenCalled();

    fireEvent.pointerUp(slider, { target: { value: "24" } });
    await waitFor(() => expect(client.setCameraFov).toHaveBeenCalledTimes(1));
    expect(client.setCameraFov).toHaveBeenCalledWith("cam-1", 24);
  });

  it("reads the lens in millimetres as well as degrees, because that is the vocabulary", async () => {
    const client = fakeClient();
    const cam = camera({ fovDeg: 55 });
    client.sceneCameras = vi.fn(() => Promise.resolve([cam]));
    selectCamera([cam]);
    const { container } = render(<CameraSection client={client} />);
    await screen.findByTestId("cameraFov");
    expect(container.textContent).toMatch(/23 mm · 55°/);
  });
});

describe("focalLengthMm", () => {
  /** The angle the engine stores is the VERTICAL one — `Mat4::perspective_rh` takes `fov_y_radians` —
   *  so the equivalent is solved against the 36 x 24 mm frame's 12 mm half-HEIGHT. Reading it off the
   *  half-WIDTH would report every lens as roughly 1.5x wider than it is. */
  it("solves the full-frame equivalent from the vertical field of view", () => {
    expect(focalLengthMm(55)).toBe(23);
    expect(focalLengthMm(28)).toBe(48);
    // A wide angle is a short lens and a narrow one is long — the direction, asserted so an inverted
    // formula cannot pass on one convenient value.
    expect(focalLengthMm(FOV_MIN_DEG)).toBeGreaterThan(focalLengthMm(FOV_MAX_DEG));
  });
});

describe("cameraActions", () => {
  it("makes the FIRST saved camera the active one and later ones not, then selects nothing for itself", async () => {
    const client = fakeClient();
    const saved: SceneCameraInfo[] = [];
    client.addCameraHere = vi.fn((name: string | null, active: boolean) => {
      saved.push(camera({ id: `cam-${saved.length + 1}`, name: name ?? `Camera ${saved.length + 1}`, active }));
      return Promise.resolve(saved[saved.length - 1].id);
    });
    client.sceneCameras = vi.fn(() => Promise.resolve(saved.map((c) => ({ ...c }))));

    await act(async () => {
      await saveCurrentView(client);
    });
    expect(client.addCameraHere).toHaveBeenLastCalledWith(null, true);

    await act(async () => {
      await saveCurrentView(client);
    });
    // The second save must NOT take the active slot from a camera the author chose.
    expect(client.addCameraHere).toHaveBeenLastCalledWith(null, false);
  });

  /** `look_through_camera` renders the ACTIVE camera. Clicking a row that is not active and only
   *  calling look-through would put a DIFFERENT camera's picture on screen under that row's name. */
  it("activates a camera before looking through it, so the row shows what it says", async () => {
    const client = fakeClient();
    const inactive = camera({ id: "cam-2", name: "Overhead", active: false });
    client.sceneCameras = vi.fn(() => Promise.resolve([camera({ active: false }), { ...inactive, active: true }]));
    await act(async () => {
      await lookThrough(client, inactive);
    });
    expect(client.setActiveCamera).toHaveBeenCalledWith("cam-2");
    expect(client.lookThroughCamera).toHaveBeenCalledWith(true);
  });

  it("does not claim to be looking through a camera the engine refused", async () => {
    const client = fakeClient();
    client.lookThroughCamera = vi.fn(() => Promise.resolve(false));
    client.sceneCameras = vi.fn(() => Promise.resolve([]));
    await act(async () => {
      await lookThrough(client, camera());
    });
    expect(cameraStore.getState().lookingThroughId).toBeNull();
    expect(toastStore.getState().toasts.some((t) => t.kind === "error")).toBe(true);
  });

  it("refuses to re-aim the camera you are already inside, rather than committing a no-op", async () => {
    const client = fakeClient();
    const cam = camera();
    client.sceneCameras = vi.fn(() => Promise.resolve([cam]));
    act(() => {
      cameraStore.getState().refresh([cam]);
      cameraStore.getState().setLookingThrough(cam.id);
    });
    await act(async () => {
      await recaptureCamera(client, cam);
    });
    expect(client.recaptureCamera).not.toHaveBeenCalled();
  });
});

describe("cameraStore", () => {
  /** A badge naming a camera nobody is inside is worse than no badge. The list is the only place that
   *  knows a camera was deleted or stood down, so it is the place that clears the flag. */
  it("drops the look-through flag when that camera stops being active", () => {
    const cam = camera();
    act(() => {
      cameraStore.getState().refresh([cam]);
      cameraStore.getState().setLookingThrough(cam.id);
    });
    expect(cameraStore.getState().lookingThroughId).toBe("cam-1");

    act(() => cameraStore.getState().refresh([{ ...cam, active: false }]));
    expect(cameraStore.getState().lookingThroughId).toBeNull();
  });

  it("and when that camera is gone entirely", () => {
    const cam = camera();
    act(() => {
      cameraStore.getState().refresh([cam]);
      cameraStore.getState().setLookingThrough(cam.id);
    });
    act(() => cameraStore.getState().refresh([]));
    expect(cameraStore.getState().lookingThroughId).toBeNull();
  });
});
