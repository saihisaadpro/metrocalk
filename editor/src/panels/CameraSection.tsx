//! The selected camera's own controls (M11.4, ADR-043) — its lens, where it stands, what it looks at,
//! and the two verbs that make it worth having: look through it, and point it at what you are looking at.
//!
//! ## Why the Inspector, and why not only the Inspector
//!
//! A scene camera is an ENTITY. It is in the hierarchy, it draws a wireframe glyph in the viewport, it is
//! selectable, and the Inspector is the editor's one answer to "what is selected?". So the deep controls
//! live here, next to the thing they act on.
//!
//! What does NOT live here is CREATING one, because creating a camera means "save where I am standing",
//! and where you are standing is a fact about the viewport, not about the selection. That gesture is in
//! the View menu at the viewport, and both call the same functions in `cameraActions`.
//!
//! ## The read-out is the honesty
//!
//! Eye and target are shown as numbers because the defect this section exists to end is invisible
//! otherwise: a camera with no aim looks, in a picture, exactly like a camera aimed at whatever the
//! editor happened to be orbiting. `Aimed at` reading "the editor's view (unaimed)" is how an author
//! finds a camera saved by an older build and fixes it with the button directly underneath.

import { useCallback, useEffect, useState } from "react";
import { useStore } from "zustand";
import { cameraStore } from "../store/cameras";
import { useSelectedId } from "../store/projection";
import { Icon } from "../theme/icons";
import { Button, ReadOut, SliderField } from "../theme/primitives";
import { color, font, fontSize, space } from "../theme/tokens";
import type { EditorClient } from "../transport/session";
import {
  FOV_MAX_DEG,
  FOV_MIN_DEG,
  activateCamera,
  focalLengthMm,
  freeLook,
  lookThrough,
  metres,
  recaptureCamera,
  refreshCameras,
  setCameraFov,
} from "./cameraActions";

const rowStyle = {
  display: "flex",
  flexWrap: "wrap" as const,
  gap: space.xs,
  alignItems: "center",
};

/**
 * The camera controls, or `null` when the selection is not a camera.
 *
 * Returning `null` rather than an empty-state is deliberate: the Inspector already stacks several
 * sections under one selection, and a "no camera selected" card in each of them would be a column of
 * apologies. The section's own heading disappears with it.
 */
export function CameraSection({ client }: { client: EditorClient }) {
  const selected = useSelectedId();
  const cameras = useStore(cameraStore, (s) => s.cameras);
  const lookingThroughId = useStore(cameraStore, (s) => s.lookingThroughId);
  const camera = cameras.find((c) => c.id === selected) ?? null;
  // The slider tracks the pointer; the commit happens on release, so a drag is ONE undo step rather
  // than one per pixel — the same rule the transform scrub and the joint value slider follow.
  const [draftFov, setDraftFov] = useState<number | null>(null);

  useEffect(() => {
    void refreshCameras(client);
  }, [client, selected]);
  useEffect(() => setDraftFov(null), [camera?.id]);

  const commitFov = useCallback(
    (value: number) => {
      setDraftFov(null);
      if (camera) void setCameraFov(client, camera, value);
    },
    [camera, client],
  );

  if (!camera) return null;

  const inside = lookingThroughId === camera.id;
  const fov = draftFov ?? camera.fovDeg;
  const eye = `${metres(camera.pos[0])}, ${metres(camera.pos[1])}, ${metres(camera.pos[2])}`;
  const aim = camera.lookAt
    ? `${metres(camera.lookAt[0])}, ${metres(camera.lookAt[1])}, ${metres(camera.lookAt[2])}`
    : null;

  return (
    <div data-testid="camera-section" style={{ display: "grid", gap: space.sm }}>
      <div style={rowStyle}>
        <Button
          data-testid="cameraLookThrough"
          variant={inside ? "secondary" : "primary"}
          compact
          onClick={() => void (inside ? freeLook(client) : lookThrough(client, camera))}
          title={
            inside
              ? "Return to the editor's own camera"
              : `Render the viewport from ${camera.name}`
          }
        >
          <Icon name="camera" size="sm" />
          {inside ? "Back to free look" : "Look through"}
        </Button>
        <Button
          data-testid="cameraActivate"
          variant="secondary"
          compact
          disabled={camera.active}
          title={
            camera.active
              ? "This is already the camera Play renders from"
              : "Play and look-through will render from this camera"
          }
          onClick={() => void activateCamera(client, camera)}
        >
          {camera.active ? "Plays from here" : "Play from here"}
        </Button>
        <Button
          data-testid="cameraRecapture"
          variant="secondary"
          compact
          disabled={inside}
          title={
            inside
              ? "You are looking through this camera, so it already shows this view"
              : "Move this camera to where you are standing and point it at what you can see"
          }
          onClick={() => void recaptureCamera(client, camera)}
        >
          <Icon name="view" size="sm" />
          Point at this view
        </Button>
      </div>
      {inside && (
        <p data-testid="cameraRecaptureWhy" style={{ margin: 0, fontSize: fontSize.micro, color: color.text.muted }}>
          Point at this view is off while you are looking through {camera.name} — it already shows this.
        </p>
      )}

      <SliderField
        data-testid="cameraFov"
        label="Lens"
        min={FOV_MIN_DEG}
        max={FOV_MAX_DEG}
        step={1}
        value={Math.round(fov)}
        ariaLabel={`Field of view for ${camera.name}, degrees`}
        valueLabel={`${focalLengthMm(fov)} mm · ${Math.round(fov)}°`}
        onChange={(e) => setDraftFov(Number(e.currentTarget.value))}
        onPointerUp={(e) => commitFov(Number(e.currentTarget.value))}
        onKeyUp={(e) => commitFov(Number(e.currentTarget.value))}
        onBlur={(e) => commitFov(Number(e.currentTarget.value))}
      />

      <dl
        data-testid="cameraPose"
        style={{
          margin: 0,
          display: "grid",
          gridTemplateColumns: "auto 1fr",
          gap: `${space.xs}px ${space.sm}px`,
          alignItems: "baseline",
          fontSize: fontSize.micro,
        }}
      >
        <dt style={{ color: color.text.muted }}>Stands at</dt>
        <dd style={{ margin: 0, font: font.mono }}>
          <ReadOut unit="m">{eye}</ReadOut>
        </dd>
        <dt style={{ color: color.text.muted }}>Aimed at</dt>
        <dd style={{ margin: 0, font: aim ? font.mono : undefined }}>
          {aim ? (
            <ReadOut unit="m">{aim}</ReadOut>
          ) : (
            <span data-testid="cameraUnaimed" style={{ color: color.warn.text }}>
              nothing — it follows the editor&rsquo;s view. Point it at this view to fix that.
            </span>
          )}
        </dd>
      </dl>
    </div>
  );
}
