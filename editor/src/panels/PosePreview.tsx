//! **See the pose before you press play.**
//!
//! THE PROBLEM THIS SOLVES. In Unreal and Unity the only way to find out whether an animation landed on
//! a character correctly is to run the game and look at it — which is exactly why "the character is
//! T-posing" is Unreal's universal bug report, with at least four unrelated causes and no message
//! attached to any of them. The information needed to answer it is available long before playback: the
//! sampler produces a pose, forward kinematics turns that pose into joint positions, and joint
//! positions are a drawing.
//!
//! So this draws it. Each figure is one skeleton under one pose, rendered as a line per bone in an
//! orthographic front view. Beside each other, they answer questions that no amount of green tests can:
//! *did the clip actually move the character*, *did it move the RIGHT bones*, and *does the same clip
//! produce the same motion on a rig with different proportions and a different rest pose*.
//!
//! WHERE THE NUMBERS COME FROM. `character/tests/pose_preview_fixture.rs` computes every coordinate
//! through the real `bind_sequence → sample → Skeleton::globals` path and fails if the committed JSON
//! and the live computation disagree. Nothing here is drawn from hand-authored coordinates — a preview
//! that could keep painting a convincing figure after the sampler broke would be worse than no preview.

import { Panel, PanelHeader } from "../theme/primitives";
import { color, font, fontSize, radius, space } from "../theme/tokens";

/** One bone: a segment from its parent's origin to its own, in world XY. */
export interface PoseSegment {
  name: string;
  /** Which side of the body the bone is on — `null` for the centre column (spine, neck, head).
   *
   *  COMPUTED IN RUST, FROM THE CHARACTERIZATION, and carried here rather than inferred. The first
   *  version of this component guessed it from the joint name with a regex, which matched Unreal's
   *  `upperarm_l` and could not match Mixamo's `LeftArm` — so one rig rendered untinted under a caption
   *  promising both were tinted. Deciding a bone's side from its spelling is the exact problem
   *  `metrocalk_skeleton::humanoid` exists to solve; the answer belongs upstream, where it is known. */
  side: "left" | "right" | null;
  x1: number;
  y1: number;
  x2: number;
  y2: number;
}

export interface PoseFigure {
  caption: string;
  detail: string;
  segments: PoseSegment[];
}

export interface PoseDocument {
  clip: { name: string; channels: number; keyedBy: string };
  figures: PoseFigure[];
  boundChannels: Record<string, number>;
  diagnostics: number;
  /** Did the two routes to the last two figures land in the same place? **Measured by the sampler,
   *  never assumed** (`character/tests/pose_preview_fixture.rs::routes_agree`).
   *
   *  The last two figures are the same pose reached two ways — bound straight from the humanoid-keyed
   *  clip, and carried across by the retargeter — and they come out pixel-identical, which is the
   *  whole thesis. Undeclared, the panel drew one picture twice under two captions and a reader had no
   *  way to tell the point from a render that had silently failed. It is a boolean rather than a
   *  hard-coded sentence so that the day the two routes separate, the sentence goes with them. */
  routesAgree?: boolean;
}

/** The drawing box, in CSS px. Tall rather than wide: a character is. */
const BOX = { w: 132, h: 190 } as const;

/**
 * Fit every figure into the same scale and baseline so they are COMPARABLE.
 *
 * Normalising each figure independently would be the obvious thing and it would be a lie: two
 * characters of different heights would be drawn the same size, and "the same clip on a taller rig"
 * — the whole claim — would be invisible. So the extent is computed across ALL figures at once, and a
 * 1.45x taller character is drawn 1.45x taller.
 */
function project(figures: PoseFigure[]) {
  const xs: number[] = [];
  const ys: number[] = [];
  for (const f of figures) {
    for (const s of f.segments) {
      xs.push(s.x1, s.x2);
      ys.push(s.y1, s.y2);
    }
  }
  const minX = Math.min(...xs);
  const maxX = Math.max(...xs);
  const minY = Math.min(...ys);
  const maxY = Math.max(...ys);
  const pad = 10;
  const scale = Math.min(
    (BOX.w - pad * 2) / Math.max(maxX - minX, 1e-6),
    (BOX.h - pad * 2) / Math.max(maxY - minY, 1e-6),
  );
  const cx = (minX + maxX) / 2;
  return {
    // SVG's Y axis points down and a skeleton's points up, so Y is flipped here rather than in the
    // fixture — the fixture holds world coordinates, and inverting them there would make every number
    // in it disagree with the engine that produced them.
    x: (v: number) => BOX.w / 2 + (v - cx) * scale,
    y: (v: number) => BOX.h - pad - (v - minY) * scale,
  };
}

function Figure({ figure, at }: { figure: PoseFigure; at: ReturnType<typeof project> }) {
  return (
    <figure data-testid="pose-figure" style={{ margin: 0, minWidth: 0 }}>
      <svg
        width={BOX.w}
        height={BOX.h}
        viewBox={`0 0 ${BOX.w} ${BOX.h}`}
        role="img"
        aria-label={`${figure.caption}: ${figure.detail}`}
        style={{
          display: "block",
          background: color.bg.inset,
          border: `1px solid ${color.border.subtle}`,
          borderRadius: radius.sm,
        }}
      >
        {figure.segments.map((s) => (
          <line
            key={s.name}
            data-testid="pose-bone"
            x1={at.x(s.x1)}
            y1={at.y(s.y1)}
            x2={at.x(s.x2)}
            y2={at.y(s.y2)}
            data-side={s.side ?? "centre"}
            stroke={s.side === "left" ? color.accent.base : color.text.secondary}
            strokeWidth={2}
            strokeLinecap="round"
          />
        ))}
      </svg>
      <figcaption
        style={{
          marginTop: space.xs,
          fontSize: fontSize.micro,
          color: color.text.primary,
          lineHeight: 1.35,
          maxWidth: BOX.w,
        }}
      >
        <span data-testid="pose-caption" style={{ display: "block" }}>
          {figure.caption}
        </span>
        <span data-testid="pose-detail" style={{ color: color.text.secondary }}>
          {figure.detail}
        </span>
      </figcaption>
    </figure>
  );
}

export function PosePreview({ doc }: { doc: PoseDocument }) {
  const at = project(doc.figures);
  const rigs = Object.keys(doc.boundChannels);

  return (
    <Panel data-testid="pose-preview" scroll>
      <PanelHeader title={<span data-testid="pose-title">Pose preview</span>} />

      <div
        data-testid="pose-headline"
        style={{
          padding: space.md,
          margin: `0 0 ${space.sm}px 0`,
          borderRadius: radius.md,
          background: color.success.bg,
          border: `1px solid ${color.success.border}`,
          color: color.text.primary,
          fontSize: fontSize.meta,
          lineHeight: 1.45,
        }}
      >
        <strong>{doc.clip.name}</strong> is keyed to {doc.clip.keyedBy} bones, so it belongs to no rig:{" "}
        {rigs.map((r) => `${doc.boundChannels[r]}/${doc.clip.channels} channels bound on ${r}`).join(", ")}
        {doc.diagnostics === 0 ? ", with nothing to report." : `, with ${doc.diagnostics} to report.`}
      </div>

      <div
        style={{
          display: "flex",
          gap: space.md,
          padding: `0 ${space.md}px ${space.md}px`,
          overflowX: "auto",
        }}
      >
        {doc.figures.map((f) => (
          <Figure key={f.caption} figure={f} at={at} />
        ))}
      </div>

      <div
        style={{
          padding: `0 ${space.md}px ${space.md}px`,
          fontSize: fontSize.micro,
          color: color.text.secondary,
          fontFamily: font.ui,
          lineHeight: 1.45,
        }}
      >
        Every figure is drawn at the same scale, so a taller character is drawn taller. Bones on the
        character&rsquo;s left are tinted — a mirrored rig is the most common retarget error and the one a
        monochrome figure hides.
        {doc.routesAgree === true && (
          // THE LAST TWO PICTURES ARE THE SAME PICTURE, AND SAYING SO IS THE POINT. Two identical
          // frames under two different captions read as a bug unless the reader already believes the
          // thesis they are the evidence for. Rendered only when the sampler measured it — see
          // `routesAgree`.
          <span data-testid="pose-routes-agree">
            {" "}
            The last two figures are identical on purpose: the clip is addressed to the human body, so
            binding it directly and retargeting it across arrive at the same pose. A separation here
            would be a real difference between the two routes, and this sentence would disappear.
          </span>
        )}
      </div>
    </Panel>
  );
}
