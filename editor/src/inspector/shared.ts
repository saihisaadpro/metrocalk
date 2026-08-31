//! What a MULTI-SELECTION has in common — the content-aware half of editing more than one object.
//!
//! The engine has been able to set one field on N entities as one atomic, undoable transaction since
//! M10.6 (`capscene::multi_edit` → one `engine.commit` with N `SetField` ops). The editor's only route
//! to it was a hard-coded "Move up 5 m" button in a popup menu, and the Inspector — the surface a user
//! actually changes a property on — read `selectedId` and ignored the selection entirely. So selecting
//! twelve lights and dimming them was twelve selections, twelve edits and twelve undo steps.
//!
//! **The panel cannot offer a field it is not safe to write.** `engine.commit` is all-or-nothing
//! (M1.6): one entity missing the component rejects the whole batch, and the user would see a refusal
//! for an object they did not know was in the selection. So the form is built from the INTERSECTION —
//! a field is offered only when every selected entity carries it — and the fields that did not make it
//! are COUNTED and named rather than silently dropped (`<ux_quality>` 6: honest state).
//!
//! **Agreement is separate from presence.** Twelve lights all have `Light.intensity`; they need not
//! agree about it. A field the selection disagrees about is `mixed`: the control shows "Mixed" instead
//! of one object's value dressed up as everyone's, and setting it writes the new value to all N.

import type { Json } from "../transport/protocol";

/** One entity's components, in the shape the projection store holds them. */
export type Components = Record<string, Record<string, Json>>;

export interface SharedShape {
  /** component → field → the value every entity agrees on, or `undefined` where they disagree.
   *
   *  `undefined` for a mixed field is deliberate and load-bearing: the form's data is diffed against
   *  itself to decide what to emit, so seeding a mixed field with the primary's value would make
   *  "set them all to the primary's 1.2" a no-op — the one edit a user reaches for most. */
  components: Record<string, Record<string, Json | undefined>>;
  /** `"Component.field"` for every OFFERED field the selection disagrees about. */
  mixed: ReadonlySet<string>;
  /** Fields carried by some of the selection but not all — offered by nothing, counted by this. */
  partialFields: number;
  /** The components only some of the selection carries, named, so the omission can be explained. */
  partialComponents: string[];
}

const EMPTY: SharedShape = {
  components: {},
  mixed: new Set(),
  partialFields: 0,
  partialComponents: [],
};

/** Scalar equality over the wire's `Json`. Component fields are scalars in `/core`'s registry, but a
 *  value that arrives as an object or array must compare by content rather than by reference — two
 *  entities projected separately never share one. */
function sameValue(a: Json | undefined, b: Json | undefined): boolean {
  if (a === b) return true;
  if (a === null || b === null || a === undefined || b === undefined) return false;
  if (typeof a === "object" || typeof b === "object") return JSON.stringify(a) === JSON.stringify(b);
  return false;
}

/**
 * The form a selection can be edited through: the components and fields EVERY entity carries, each
 * marked agreed (a value) or mixed, plus an honest count of what was left out.
 *
 * `entities[0]` is the primary/anchor, and it fixes the ORDER of components and fields — the panel
 * must not re-order itself because a different object happens to be first in the store.
 */
export function sharedShape(entities: readonly Components[]): SharedShape {
  if (entities.length === 0) return EMPTY;
  const [primary, ...rest] = entities;
  if (rest.length === 0) {
    // A "selection" of one is its own intersection — nothing is mixed and nothing is left out.
    const components: Record<string, Record<string, Json | undefined>> = {};
    for (const [c, fields] of Object.entries(primary)) components[c] = { ...fields };
    return { components, mixed: new Set(), partialFields: 0, partialComponents: [] };
  }

  const components: Record<string, Record<string, Json | undefined>> = {};
  const mixed = new Set<string>();
  const partialComponents: string[] = [];
  let partialFields = 0;

  for (const [component, fields] of Object.entries(primary)) {
    const others = rest.map((e) => e[component]);
    if (others.some((o) => o === undefined)) {
      // The component itself is not shared: every one of its fields is a field this panel cannot
      // offer, and the component is named so the sentence can say WHICH capability was dropped.
      partialComponents.push(component);
      partialFields += Object.keys(fields).length;
      continue;
    }
    const shared: Record<string, Json | undefined> = {};
    for (const [field, value] of Object.entries(fields)) {
      if (others.some((o) => !(field in (o as Record<string, Json>)))) {
        partialFields += 1;
        continue;
      }
      const agreed = others.every((o) => sameValue((o as Record<string, Json>)[field], value));
      shared[field] = agreed ? value : undefined;
      if (!agreed) mixed.add(`${component}.${field}`);
    }
    if (Object.keys(shared).length > 0) components[component] = shared;
  }

  // Fields the OTHERS carry that the primary does not are equally "on some but not all". Counted from
  // the union rather than from the primary, because a panel that reports 0 while hiding four rows is
  // the same untruth in the other direction.
  const seen = new Set<string>();
  for (const [component, fields] of Object.entries(primary)) {
    for (const field of Object.keys(fields)) seen.add(`${component}.${field}`);
  }
  for (const entity of rest) {
    for (const [component, fields] of Object.entries(entity)) {
      for (const field of Object.keys(fields)) {
        const key = `${component}.${field}`;
        if (seen.has(key)) continue;
        seen.add(key);
        partialFields += 1;
        if (!(component in primary) && !partialComponents.includes(component)) {
          partialComponents.push(component);
        }
      }
    }
  }

  return { components, mixed, partialFields, partialComponents };
}

/** One entry per distinct `kind` in the selection, most-common first — the makeup line's data.
 *  Ties break on the kind name so the sentence is stable across renders and across runs. */
export function selectionMakeup(kinds: readonly (string | undefined)[]): { kind: string; count: number }[] {
  const counts = new Map<string, number>();
  for (const k of kinds) {
    const kind = k ?? "default";
    counts.set(kind, (counts.get(kind) ?? 0) + 1);
  }
  return [...counts.entries()]
    .map(([kind, count]) => ({ kind, count }))
    .sort((a, b) => b.count - a.count || a.kind.localeCompare(b.kind, "en-GB"));
}

/** Plural-aware English for a kind and a count — "3 meshes", "1 light", "2 objects" for the unnamed
 *  `default` kind. Kept beside the counting so the panel never has to build the sentence itself.
 *  `mesh` is the reason the sibilant rule is here and not a bare `+ "s"`: "3 meshs" is the kind of
 *  copy defect a green test suite ships. */
export function describeKind(kind: string, count: number): string {
  const noun = kind === "default" ? "object" : kind;
  if (count === 1) return `1 ${noun}`;
  const sibilant = /(s|x|z|ch|sh)$/i.test(noun);
  return `${count} ${noun}${sibilant ? "es" : "s"}`;
}
