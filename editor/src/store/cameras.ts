//! The **scene cameras** store (M11.4, ADR-043) — the authored camera list and whether the viewport is
//! currently rendering through one.
//!
//! ## Why a store and not three components each asking
//!
//! Three surfaces need this at once: the View menu at the viewport (save a camera, jump back to one),
//! the Inspector section for a selected camera (its lens, its aim, look through it), and the badge ON
//! the stage that says which camera you are inside. Each polling `scene_cameras` for itself would put
//! three answers on screen that disagree for one frame after every edit — and the disagreement would be
//! about *which camera is active*, which is the one fact the whole feature turns on.
//!
//! ## What is mirrored and what is not
//!
//! `cameras` mirrors the document: an authored camera is an ENTITY, and the engine is authoritative for
//! it (invariant 1). `lookingThroughId` is **not** in the document — looking through a camera is a render
//! projection (ADR-021), never a commit — so it lives only here and resets on reload, which is correct:
//! reopening a project puts you back at your own view, not inside a camera you left on.

import { createStore } from "zustand/vanilla";
import { useStore } from "zustand";
import type { SceneCameraInfo } from "../transport/protocol";

interface CameraState {
  cameras: SceneCameraInfo[];
  /** The camera the viewport is rendering through, or `null` for free look. */
  lookingThroughId: string | null;
  refresh(cameras: SceneCameraInfo[]): void;
  setLookingThrough(id: string | null): void;
  reset(): void;
}

export const cameraStore = createStore<CameraState>((set) => ({
  cameras: [],
  lookingThroughId: null,
  refresh: (cameras) =>
    set((s) => ({
      cameras,
      // A camera that was deleted, or one that stopped being the active one, cannot still be the
      // camera you are looking through — the badge would name a picture nobody is inside. Cleared
      // here rather than at each call site, because the list is the only place that knows.
      lookingThroughId: cameras.some((c) => c.id === s.lookingThroughId && c.active)
        ? s.lookingThroughId
        : null,
    })),
  setLookingThrough: (id) => set({ lookingThroughId: id }),
  reset: () => set({ cameras: [], lookingThroughId: null }),
}));

/** Every authored scene camera, oldest first. */
export const useCameras = (): SceneCameraInfo[] => useStore(cameraStore, (s) => s.cameras);

/** The camera the viewport is rendering through, or `null` for free look. */
export const useLookingThrough = (): SceneCameraInfo | null =>
  useStore(cameraStore, (s) => s.cameras.find((c) => c.id === s.lookingThroughId) ?? null);

/** The one camera look-through and Play render from, or `null` if the scene has none. */
export const useActiveCamera = (): SceneCameraInfo | null =>
  useStore(cameraStore, (s) => s.cameras.find((c) => c.active) ?? null);
