//! **The aiming badge** — what the stage says while a shot is being pointed at something.
//!
//! The rule `PlayBadge` and `PreviewBadge` exist for: a mode that changes what a click MEANS has to
//! be unmistakable where the user is looking, and the way out has to be one gesture from there. This
//! one carries a second job neither of those has — it is the read-out for the gesture in progress.
//!
//! WHY THE RUNGS ARE BUTTONS. A click on an imported assembly lands on a LEAF: one bolt, of one weld
//! gun, of a 262-metre production line. "Film the bolt" is almost never the shot, and the fix cannot
//! be a modifier key nobody discovers. So the object under the cursor arrives with the chain it
//! hangs from — the engine's own hierarchy, each rung carrying the DRAWN-PART count that tells a
//! 378-part assembly apart from the bracket sharing most of its name — and every rung is one click.
//! Clicking the STAGE takes the first rung, which is the thing the cursor is actually over; clicking
//! a rung takes that one. Either way it is one click, and neither is a mode.
//!
//! A PURE PRESENTATION COMPONENT, deliberately: the state lives in `store/subjectAim` and the wiring
//! in `App`, so a test (and the shots gate) can render this at any point of the gesture, including
//! the ones the store cannot be driven to without a native viewport.

import { Button } from "../theme/primitives";
import { Icon } from "../theme/icons";
import { color, elevation, font, fontSize, radius, space, z } from "../theme/tokens";
import type { AimRung } from "../store/subjectAim";

/** `2 parts` / `1 part` / the honest absence — the same sentence the picker's rows use. */
function partsLabel(parts: number): string {
  if (parts === 0) return "nothing drawn";
  return `${parts} part${parts === 1 ? "" : "s"}`;
}

export interface SubjectAimBadgeProps {
  /** Which shot is being aimed, 0-based — `null` only in the degenerate state a test can render. */
  shotIndex: number | null;
  /** How many shots the cutscene holds. */
  shots: number;
  /** The object under the cursor and what it is part of, nearest first. */
  rungs: AimRung[];
  /** A peek is in flight — say so rather than claiming there is nothing under the cursor. */
  looking: boolean;
  /** Take this rung instead of whatever the stage is over. */
  onPick: (rung: AimRung) => void;
  /** Point the STAGE at this rung — `null` when the pointer leaves it. What makes the ladder
   *  legible: `Assembly Hall · 7 parts` is a claim about the picture, and hovering the rung is how
   *  the author checks it before spending a shot on it. */
  onPreview: (rung: AimRung | null) => void;
  onCancel: () => void;
}

export function SubjectAimBadge({ shotIndex, shots, rungs, looking, onPick, onPreview, onCancel }: SubjectAimBadgeProps) {
  return (
    <div
      id="subjectAimBadge"
      data-testid="subjectAimBadge"
      // BOTTOM-CENTRE, not top: the preview badge holds the top of the stage and a shot is very often
      // aimed WHILE it is previewing — that is the whole loop this closes, change what is filmed and
      // watch the frame change. Two pills stacked on the same anchor is one of them hidden.
      //
      // No `stopPropagation` here: `stageInput.ts` decides once, for every overlay, whether a pointer
      // reached the stage surface or a control floating over it.
      style={{
        position: "absolute",
        bottom: space.lg,
        left: "50%",
        transform: "translateX(-50%)",
        zIndex: z.badge,
        display: "flex",
        alignItems: "center",
        flexWrap: "wrap",
        justifyContent: "center",
        gap: space.sm,
        maxWidth: "min(92%, 900px)",
        padding: `${space.xs}px ${space.lg}px`,
        borderRadius: radius.pill,
        background: color.info.bg,
        border: `1px solid ${color.info.border}`,
        color: color.info.text,
        font: font.mono,
        fontSize: fontSize.body,
        boxShadow: elevation.e2,
      }}
    >
      <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
        <Icon name="cursor" size={12} />
        AIMING
      </span>
      <span data-testid="subjectAimShot" style={{ color: color.text.secondary }}>
        {shotIndex === null ? "a shot" : shots > 0 ? `shot ${shotIndex + 1} of ${shots}` : `shot ${shotIndex + 1}`}
      </span>

      {rungs.length === 0 ? (
        // NEVER SILENT, AND NEVER WRONG ABOUT WHICH IT IS. "Looking" and "there is nothing there" are
        // different facts, and a badge that says the second while the first is true teaches the user
        // the gesture is broken.
        <span data-testid="subjectAimHint" style={{ color: color.text.muted }}>
          {looking ? "looking…" : "click what this shot should film"}
        </span>
      ) : (
        <span
          data-testid="subjectAimRungs"
          style={{ display: "inline-flex", alignItems: "center", flexWrap: "wrap", gap: space.xs }}
        >
          {rungs.map((rung, index) => (
            <span key={rung.id} style={{ display: "inline-flex", alignItems: "center", gap: space.xs }}>
              {index > 0 && (
                <span aria-hidden style={{ color: color.text.muted }}>
                  in
                </span>
              )}
              <Button
                data-testid={`subjectAimRung-${rung.id}`}
                data-parts={rung.parts}
                variant={index === 0 ? "primary" : "secondary"}
                compact
                title={
                  rung.parts === 0
                    ? `${rung.name} has no drawn geometry under it, so the camera would be fitted to its origin rather than to anything you can see.`
                    : `Frame ${rung.name} — ${partsLabel(rung.parts)} under it · ${rung.group.toLowerCase()}`
                }
                onClick={() => onPick(rung)}
                // POINTER, not mouse: the same events a touch or a pen produces, so a stylus user
                // gets the cue too. `onFocus`/`onBlur` because these are buttons and Tab reaches
                // them — a keyboard author is asking the same question and deserves the same answer.
                onPointerEnter={() => onPreview(rung)}
                onPointerLeave={() => onPreview(null)}
                onFocus={() => onPreview(rung)}
                onBlur={() => onPreview(null)}
              >
                {rung.name}
                {/* THE MUTED TOKEN IS A CONTRAST CLAIM ABOUT A NEUTRAL BACKGROUND, and the first
                    rung is not on one. `color.text.muted` over the primary fill measures 1.17:1
                    where 4.5 is required — caught by the shots gate on this badge's own capture, and
                    it is text a reader is expected to read: the count is the reason the rung is
                    worth pressing. On the filled rung the count inherits the button's own colour and
                    is set apart by weight instead. */}
                <span
                  style={{
                    color: index === 0 ? "inherit" : color.text.muted,
                    fontWeight: index === 0 ? 400 : undefined,
                    marginLeft: space.xs,
                  }}
                >
                  {partsLabel(rung.parts)}
                </span>
              </Button>
            </span>
          ))}
        </span>
      )}

      <span style={{ color: color.text.muted }}>— Esc or</span>
      <Button data-testid="subjectAimCancel" variant="secondary" compact onClick={onCancel}>
        Cancel
      </Button>
    </div>
  );
}

export default SubjectAimBadge;
