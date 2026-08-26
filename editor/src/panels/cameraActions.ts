//! Everything a user can do to a scene camera, written once.
//!
//! Three surfaces perform these — the View menu at the viewport, the Inspector section for a selected
//! camera, and the badge on the stage — and every one of them has to leave the same state behind: the
//! store refreshed from the engine, the look-through flag agreeing with what the engine actually did,
//! and a confirmation at the gesture. Three copies of that sequence is three chances for one surface to
//! forget the refresh and show a list that is one edit behind.
//!
//! Each of these is a thin composition over `EditorClient`. None of them holds state; the store is the
//! only mirror and the engine is authoritative for it (invariant 1).

import { cameraStore } from "../store/cameras";
import { pushToast } from "../store/toasts";
import { setStatus } from "../store/ui";
import type { SceneCameraInfo } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

/** The lens range the engine clamps a scene camera to (`FOV_MIN_DEG` / `FOV_MAX_DEG` in `main.rs`). */
export const FOV_MIN_DEG = 5;
export const FOV_MAX_DEG = 120;

/** Re-read the authored camera list from the engine into the store. Every mutation ends with one. */
export async function refreshCameras(client: EditorClient): Promise<SceneCameraInfo[]> {
  const cameras = await client.sceneCameras().catch(() => [] as SceneCameraInfo[]);
  cameraStore.getState().refresh(cameras);
  return cameras;
}

/**
 * Save what is on screen as a new camera — the gesture the whole capability exists for.
 *
 * `active` when the scene has no camera yet, so the very first save is immediately the one Play and
 * look-through use; later saves do not silently take over the active slot from a camera the author
 * chose. Returns the new camera, or `null` if the engine refused.
 */
export async function saveCurrentView(
  client: EditorClient,
  name?: string,
): Promise<SceneCameraInfo | null> {
  const first = cameraStore.getState().cameras.length === 0;
  const id = await client.addCameraHere(name?.trim() || null, first).catch(() => null);
  if (!id) {
    pushToast("The camera could not be saved", "error");
    return null;
  }
  const created = (await refreshCameras(client)).find((c) => c.id === id) ?? null;
  const label = created?.name ?? "Camera";
  pushToast(`Saved this view as ${label}`, "success");
  setStatus(`Saved this view as ${label}`);
  return created;
}

/**
 * Render the viewport through `camera` — making it the active one first if it is not already.
 *
 * Activating before looking through is what makes the row do what its label says: `look_through_camera`
 * renders the ACTIVE camera, so clicking a row that is not active and only calling look-through would
 * put a different camera's picture on screen under that row's name.
 */
export async function lookThrough(client: EditorClient, camera: SceneCameraInfo): Promise<boolean> {
  if (!camera.active && !(await client.setActiveCamera(camera.id).catch(() => false))) {
    pushToast(`${camera.name} could not be made the active camera`, "error");
    return false;
  }
  const found = await client.lookThroughCamera(true).catch(() => false);
  await refreshCameras(client);
  if (!found) {
    pushToast(`${camera.name} could not be looked through`, "error");
    return false;
  }
  cameraStore.getState().setLookingThrough(camera.id);
  setStatus(`Looking through ${camera.name}`);
  return true;
}

/** Back to the editor's own camera. Always succeeds; the engine simply drops the render override. */
export async function freeLook(client: EditorClient): Promise<void> {
  await client.lookThroughCamera(false).catch(() => false);
  cameraStore.getState().setLookingThrough(null);
  setStatus("Free look");
}

/** Make `camera` the one Play and look-through render from, without moving the viewport. */
export async function activateCamera(
  client: EditorClient,
  camera: SceneCameraInfo,
): Promise<boolean> {
  const ok = await client.setActiveCamera(camera.id).catch(() => false);
  await refreshCameras(client);
  if (ok) pushToast(`${camera.name} is the active camera`, "success");
  else pushToast(`${camera.name} could not be made the active camera`, "error");
  return ok;
}

/**
 * Point an existing camera at what is on screen now.
 *
 * Deliberately NOT offered while looking through that same camera: re-aiming a camera at its own
 * picture is a commit that changes nothing, and an undo step that undoes nothing is worse than a
 * disabled control. The caller disables it; this refuses it too, so the two cannot disagree.
 */
export async function recaptureCamera(
  client: EditorClient,
  camera: SceneCameraInfo,
): Promise<boolean> {
  if (cameraStore.getState().lookingThroughId === camera.id) {
    pushToast(`${camera.name} already shows this view`, "info");
    return false;
  }
  const ok = await client.recaptureCamera(camera.id).catch(() => false);
  await refreshCameras(client);
  if (ok) pushToast(`${camera.name} now shows this view`, "success");
  else pushToast(`${camera.name} could not be re-aimed`, "error");
  return ok;
}

/** Change one camera's lens. One undoable commit per call — the caller commits on release, not per pixel. */
export async function setCameraFov(
  client: EditorClient,
  camera: SceneCameraInfo,
  fov: number,
): Promise<boolean> {
  const clamped = Math.min(FOV_MAX_DEG, Math.max(FOV_MIN_DEG, Math.round(fov)));
  const ok = await client.setCameraFov(camera.id, clamped).catch(() => false);
  await refreshCameras(client);
  if (ok) setStatus(`${camera.name} · ${clamped}° lens`);
  return ok;
}

/**
 * How a lens reads to someone who thinks in millimetres rather than degrees.
 *
 * The full-frame equivalent, because "35 mm" is the vocabulary of every reference a person composing a
 * shot has ever read and "63°" is not. `f = 12 / tan(fov / 2)` — the 36 x 24 mm frame's half-HEIGHT,
 * because this angle is the VERTICAL one: the engine hands it to `Mat4::perspective_rh`, whose first
 * argument is `fov_y_radians`, and the viewport's own solver names its half-angle `half_v`. Using the
 * half-width would report every lens as ~1.5x wider than it is.
 */
export function focalLengthMm(fovDeg: number): number {
  const half = (Math.max(FOV_MIN_DEG, Math.min(FOV_MAX_DEG, fovDeg)) * Math.PI) / 360;
  return Math.round(12 / Math.tan(half));
}

/** A world point as a person reads a plant drawing: whole metres, one decimal below ten. */
export function metres(value: number): string {
  return Math.abs(value) >= 10 ? `${Math.round(value)}` : value.toFixed(1);
}
