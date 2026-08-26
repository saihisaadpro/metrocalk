//! **The catalog's vocabulary, projected for a reader** (ADR-144) — the pure functions that turn the
//! namespaced strings `metrocalk_core::catalog` puts on the wire into the words a user is shown.
//!
//! WHY THIS FILE EXISTS. `catalog()` returns `BTreeMap<bucket, Vec<CatalogItem>>` where the KEY is the
//! canonical bucket — `"std:Props"`, `"std:Characters"` — because that is the identity two authors'
//! custom categories collapse onto (`core/src/taxonomy.rs`). The asset browser printed that key as its
//! group heading, so the shipped UI said **`std:Props`** where it meant *Props*: engine-internal
//! namespacing in user copy, which `<ux_quality>` 4 names by hand. The item's own `category` field is
//! already display-formatted by the core (`Companions (acme)`), but a BUCKET has no such field — it is
//! a map key and nothing more — so the projection has to happen here.
//!
//! IT MIRRORS `core/src/caps.rs::display_name` AND SAYS SO. `display_name` drops a `std:` namespace and
//! parenthesises any other (`acme:Companions` → `Companions (acme)`). That is one rule stated twice, in
//! two languages — the shape `<test_and_ci_discipline>` 6 is about — so the mirror is kept deliberately
//! total and trivial (a split on the first colon, no table, no vocabulary to drift) and the cases the
//! Rust tests pin are pinned again here against the same strings.

import { resolveIcon } from "../theme/icons";

/** One catalog source tier, projected: the icon that draws it and the word a reader is shown.
 *
 *  `metrocalk_core::catalog::Source` serialises exactly two of these (`local`, `marketplace`). The other
 *  two are the tiers the resolver escalates to (ADR-012) and the importer produces, and they arrive on
 *  the same field. An UNKNOWN source is never renamed into one of the four — it keeps its own word and
 *  takes the neutral mark, because inventing a tier for a string we do not recognise is exactly how a
 *  future source would be reported to the user as something it is not. */
export interface CatalogTier {
  /** `theme/icons.tsx` name — also the `TypeIcon` hue key, which is why the two vocabularies share it. */
  icon: string;
  /** The word shown to a reader. One word: it shares a ~110px line with the item's collection. */
  label: string;
  /** The sentence the word is short for. Shown as a tooltip, never relied on alone. */
  hint: string;
  /** Whether this tier can cost tokens. Drives the confirm-before-spend path, never a colour alone. */
  metered: boolean;
}

const TIERS: Record<string, CatalogTier> = {
  // One word each, and that is a size constraint as much as an editorial one: the tier shares a ~110px
  // line with the item's collection, and "In this engine" — the first, friendlier wording — reached a
  // reader as `In this en…`. The sentence that wording was trying to say lives in `hint`, which is
  // the tooltip, where there is room for it.
  local: { icon: "local", label: "Local", hint: "already in this engine — free to place", metered: false },
  marketplace: { icon: "marketplace", label: "Marketplace", hint: "published by someone else — costs tokens", metered: true },
  generated: { icon: "generated", label: "Generated", hint: "made from a description — costs tokens", metered: true },
  imported: { icon: "imported", label: "Imported", hint: "brought in from a file you opened", metered: false },
};

/** Project a catalog item's `source` into the tier a reader is shown. Unknown sources keep their own
 *  word rather than being folded into a neighbouring tier. */
export function catalogTier(source: string): CatalogTier {
  return TIERS[source] ?? { icon: "default", label: source, hint: `a source this build does not recognise: ${source}`, metered: false };
}

/** A namespaced core name (`std:Props`, `acme:Companions`) → the words a reader is shown.
 *
 *  The mirror of `core/src/caps.rs::display_name`: a `std:` namespace is dropped, any other namespace is
 *  parenthesised after the local name, and an un-namespaced name is already display form. */
export function displayName(name: string): string {
  const colon = name.indexOf(":");
  if (colon < 0) return name;
  const namespace = name.slice(0, colon);
  const local = name.slice(colon + 1);
  if (local.length === 0) return name;
  return namespace === "std" ? local : `${local} (${namespace})`;
}

/** The heading for one browse group. The map key is a canonical bucket, so this is `displayName` — named
 *  separately because the call site is about a COLLECTION and the next reader should not have to know
 *  that a bucket happens to be spelled like a capability. */
export const bucketLabel = (bucket: string): string => displayName(bucket);

/** The mark for one browse collection, keyed on the canonical bucket `metrocalk_core::taxonomy` defines
 *  (`STD_CATEGORIES` plus `std:Other`). Seven entries and a fallback, because a bucket set that grows
 *  needs a mark added deliberately rather than a blank box appearing in a grid.
 *
 *  It is a MAP AND NOT A GUESS. The alternative considered was deriving the mark from the item — its id,
 *  its first capability — which reads well for `Camera` and `Light` and produces nothing at all for
 *  `Collider`, `Behavior` or any marketplace entry, so most of a real catalog would fall through to the
 *  generic shape anyway. A collection's mark is shared by everything filed under it, which is what makes
 *  a grid scannable: the eye sorts by group, not by twelve unrelated glyphs. */
const BUCKET_ICONS: Record<string, string> = {
  "std:UI": "properties",
  "std:Gameplay": "gameplay",
  "std:Props": "prop",
  "std:Characters": "character",
  "std:Audio": "audio",
  "std:Logic": "logic",
  "std:Other": "shape",
};

/** The `theme/icons.tsx` name for a bucket's mark. Unknown buckets take the generic shape. */
export function bucketIcon(bucket: string): string {
  return BUCKET_ICONS[bucket] ?? "shape";
}

/** The mark for ONE item: its own name when the icon set already draws it, else its collection's.
 *
 *  Both halves are needed and neither is enough. Drawing the collection's mark alone gave a four-across
 *  grid in which `Camera`, `Light`, `Mesh`, `Sprite`, `Transform` and a marketplace sword were six copies
 *  of the same glyph — a wall of previews that previews nothing. Deriving from the id alone leaves
 *  `Collider`, `Behavior` and every `forge:`-namespaced entry with no mark at all.
 *
 *  The id half is an EXACT LOOKUP in a set that already exists, never a new vocabulary: `resolveIcon`
 *  answers only for names the icon set draws or has declared an alias for, so this cannot invent a mark
 *  and cannot go stale — an icon that is deleted stops matching and the item falls back to its
 *  collection, which is the correct behaviour rather than a blank box. */
export function catalogItemIcon(id: string, bucket: string): string {
  return resolveIcon(id.toLowerCase()) ?? bucketIcon(bucket);
}

/** Every mark this module can ask the icon set for — so a test can prove all of them resolve. `Icon`'s
 *  `name` is a `string` (the Rust catalogs feed it at runtime), so `tsc` cannot spell-check these and
 *  `check-icon-vocab.mjs` only reads LITERAL `<Icon name>` sites, which a table lookup is not. */
export const CATALOG_ICONS: readonly string[] = [
  ...Object.values(BUCKET_ICONS),
  ...Object.values(TIERS).map((t) => t.icon),
  "default",
];
