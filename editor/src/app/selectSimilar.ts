//! **"Select everything like this one."** What the editor already knows about an object, turned into
//! a selection — and a sentence saying what it matched on.
//!
//! THE CASE A RECTANGLE CANNOT REACH. ADR-158 gave the stage its marquee, which selects what is
//! inside a box on screen. That is the wrong shape for the selection that actually costs a person
//! their afternoon: an imported assembly where 378 of the 15,711 parts are the same bolt, scattered
//! through the model and most of them behind something. No box contains them, no drag reaches them,
//! and ctrl-clicking 378 times is not a workflow. The editor is already holding the fact that makes
//! them one set — they draw the same mesh — and had no way to say so.
//!
//! MATCH ON THE STRONGEST THING PRESENT, AND SAY WHICH. A rule that falls back silently is a rule
//! that surprises: "Select similar" on a light and on a bolt cannot mean the same thing, so the
//! answer carries the reason it found, in plain language, for the sentence at the gesture
//! (`<ux_quality>` 2 and 4). No hidden heuristic, no ranked guess, no model — this is a query over a
//! projection the editor already has, which is why it costs nothing and works offline.

import { entityLabel } from "../store/selectionText";
import type { EntityProjection } from "../transport/protocol";

/** A similarity answer: the matched ids (the object asked about included) and why they match. */
export interface SimilarSelection {
  ids: string[];
  /** Plain-language phrase completing "Selected N objects …". */
  reason: string;
}

/** The mesh handle an entity draws with, when it has one. */
function meshOf(entity: EntityProjection | undefined): string | null {
  const mesh = entity?.components?.MeshRenderer?.mesh;
  return typeof mesh === "string" && mesh.length > 0 ? mesh : null;
}

/** The sorted names of an entity's components — its "kind" in an ECS, where what a thing IS is the
 *  set of components it carries. */
function signatureOf(entity: EntityProjection | undefined): string {
  return Object.keys(entity?.components ?? {})
    .slice()
    .sort()
    .join("+");
}

/** Name a component list the way a person would read it: at most three, then "and N more". */
function nameSignature(signature: string): string {
  const parts = signature.split("+").filter(Boolean);
  if (parts.length <= 3) return parts.join(", ");
  return `${parts.slice(0, 3).join(", ")} and ${parts.length - 3} more`;
}

/**
 * Everything in `order` that is the same kind of thing as `primary`.
 *
 * Returns `null` when there is nothing to match on at all — an entity with no mesh and no components
 * is not "similar" to anything, and answering with every other blank entity in the scene would be
 * worse than answering nothing. The caller turns that into a refusal with a reason rather than a
 * selection nobody asked for.
 */
export function similarTo(
  displayed: Record<string, EntityProjection>,
  order: string[],
  primary: string,
): SimilarSelection | null {
  const entity = displayed[primary];
  if (!entity) return null;

  // 1. THE SAME PART. A mesh handle is content-addressed — two entities carrying it draw identical
  //    geometry — so this is an identity, not a resemblance, and it is the answer the assembly case
  //    needs. It survives a different material, a different transform and a different name, because
  //    none of those change which part it is.
  const mesh = meshOf(entity);
  if (mesh) {
    return {
      ids: order.filter((id) => meshOf(displayed[id]) === mesh),
      reason: `sharing the geometry of ${entityLabel(primary)}`,
    };
  }

  // 2. THE SAME KIND. No mesh (a light, a camera, an empty, a rule holder): match the component
  //    signature, which is what "the same kind of thing" means in an ECS.
  const signature = signatureOf(entity);
  if (!signature) return null;
  return {
    ids: order.filter((id) => signatureOf(displayed[id]) === signature),
    reason: `with the same make-up as ${entityLabel(primary)} (${nameSignature(signature)})`,
  };
}
