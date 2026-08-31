//! **Finding objects by what they ARE, not only by what they are called.**
//!
//! THE GAP THIS CLOSES. The outliner's search box matched one thing: a substring of a name, or a
//! substring of an id. On the assemblies this engine exists for that is close to useless — an imported
//! 15,711-part model names its parts whatever the CAD system named them, and "every light", "every
//! physics body", "everything still waiting for a binding" are not spellings of anything. The editor
//! was already holding the answer: `EntitySummary.kind` is the salient type the engine classified
//! (`bridge.rs::classify_kind`), and `rel.needsBinding` is the live requirer truth off the real
//! `/core` — both computed on the projection path, both drawn on every row as an icon, and neither
//! reachable by the box directly above them.
//!
//! ADR-176 made the same argument for the stage: "select everything like this one" asks what a thing
//! IS, and no rectangle can ask it. This is that question typed rather than pointed at, over the whole
//! scene rather than outward from one object.
//!
//! ## Three decisions worth stating
//!
//! **An unknown key is TEXT, never a failed filter.** `bolt:M8` is a part number, not a query for the
//! `bolt` facet, so only the keys named here take a token out of the free text. The cost of the rule is
//! that a typo (`kinds:light`) searches for the literal string and finds nothing — which is why the
//! empty state names the keys that exist, and why the chips exist at all: the vocabulary is taught by
//! the scene, not by documentation nobody opens.
//!
//! **The facets are counted from the scene, never listed here.** `facetsOf` enumerates the kinds
//! actually present. Only the WORD for a kind is written below, and a kind with no word still appears
//! under its own name — so a kind added to the engine arrives in the picker on its own, which is the
//! opposite of how KTX2 became importable and unofferable.
//!
//! **A facet that matches everything is not offered.** A chip reading "Meshes 15,711" on a scene of
//! 15,711 meshes narrows nothing and teaches nothing; `Select all` is already one row away in the
//! palette. Facets are offered strictly between one and all.

import type { EntityProjection, EntitySummary } from "../transport/protocol";

/** A parsed query. Every field is already lowercased, so matching is a comparison and not a re-parse. */
export interface SceneQuery {
  /** Free text — a substring of a name or an id, exactly as the box has always behaved. */
  text: string;
  /** `kind:` / `type:` values. ALTERNATIVES: "lights or cameras" is a question a person asks, and
   *  "a light that is also a camera" is not a thing an entity can be. */
  kinds: string[];
  /** `has:` component names. Every one must be present — adding a word narrows, as a search should. */
  components: string[];
  /** `needs:binding` — the requirer signal the Requirers panel lists and the row already draws. */
  needsBinding: boolean;
}

/** The keys that take a token out of the free text. Anything else is part of the name being searched. */
const FACET_KEYS = new Set(["kind", "type", "has", "needs"]);

/** The filter keys, in the order the empty state names them. Exported so the copy cannot drift from
 *  the parser: a list of keys written out inside a panel goes stale the day one is added here. */
export const FILTER_KEYS: readonly string[] = ["kind:", "has:", "needs:binding"];

/** Parse what the user typed. Never throws and never refuses — the worst input is a query that matches
 *  nothing, and the panel explains that case rather than this one. */
export function parseQuery(raw: string): SceneQuery {
  const kinds: string[] = [];
  const components: string[] = [];
  const free: string[] = [];
  let needsBinding = false;

  for (const token of raw.trim().split(/\s+/)) {
    if (!token) continue;
    const colon = token.indexOf(":");
    const key = colon > 0 ? token.slice(0, colon).toLocaleLowerCase() : "";
    const value = colon > 0 ? token.slice(colon + 1).toLocaleLowerCase() : "";
    if (!value || !FACET_KEYS.has(key)) {
      free.push(token);
      continue;
    }
    if (key === "kind" || key === "type") kinds.push(value);
    else if (key === "has") components.push(value);
    else if (value === "binding" || value === "bindings") needsBinding = true;
    // `needs:` with anything else is not a filter this engine has, so it stays text and is searched
    // for — never silently dropped, which would answer a question nobody asked.
    else free.push(token);
  }

  return { text: free.join(" ").toLocaleLowerCase(), kinds, components, needsBinding };
}

/** Whether a query asks for nothing — the unfiltered list, and the cheap path the panel keeps. */
export function queryIsEmpty(query: SceneQuery): boolean {
  return !query.text && !query.kinds.length && !query.components.length && !query.needsBinding;
}

/** Whether answering this query needs the COMPONENT map rather than just the summaries. Only `has:`
 *  does, and the panel subscribes to that map only when this is true — the summaries are what a row
 *  renders from (M2.5), and subscribing the whole tree to every field edit to answer a query nobody
 *  typed is how a 15,711-row list stops holding its frame budget. */
export function queryNeedsComponents(query: SceneQuery): boolean {
  return query.components.length > 0;
}

/** Does this entity answer the query? `entity` may be absent whenever `queryNeedsComponents` is false. */
export function matchesQuery(
  query: SceneQuery,
  id: string,
  summary: EntitySummary | undefined,
  entity: EntityProjection | undefined,
): boolean {
  if (query.text) {
    const name = summary?.name ?? "";
    if (!id.toLocaleLowerCase().includes(query.text) && !name.toLocaleLowerCase().includes(query.text)) {
      return false;
    }
  }
  if (query.kinds.length && !query.kinds.includes((summary?.kind ?? "").toLocaleLowerCase())) return false;
  if (query.needsBinding && !summary?.rel?.needsBinding) return false;
  if (query.components.length) {
    const present = Object.keys(entity?.components ?? {}).map((c) => c.toLocaleLowerCase());
    for (const wanted of query.components) if (!present.includes(wanted)) return false;
  }
  return true;
}

/** One offerable way to narrow this scene. */
export interface SceneFacet {
  /** The token it adds to (or removes from) the query — `kind:light`. Also its stable test hook. */
  token: string;
  /** What to call it, plural, in the words a person uses. */
  label: string;
  /** How many objects it selects. Shown, because a filter whose size you cannot see is a guess. */
  count: number;
}

/** The word for each kind the engine classifies (`bridge.rs::classify_kind` + `store/relSummary.ts`).
 *  A kind absent from this map still appears — under its own name — so the picker cannot fall behind
 *  the classifier. */
const KIND_LABELS: Record<string, string> = {
  mesh: "Meshes",
  light: "Lights",
  camera: "Cameras",
  group: "Groups",
  physics: "Physics bodies",
  audio: "Audio sources",
  character: "Characters",
  requirer: "Requirers",
  default: "Other objects",
};

function kindLabel(kind: string): string {
  return KIND_LABELS[kind] ?? kind.charAt(0).toLocaleUpperCase() + kind.slice(1);
}

/**
 * The ways this particular scene can be narrowed, largest first.
 *
 * One pass over the summaries — the same map the rows already render from — so this costs a scan and
 * no engine call, works offline, and is exact rather than sampled.
 */
export function facetsOf(order: string[], summaries: Record<string, EntitySummary>): SceneFacet[] {
  const total = order.length;
  if (total < 2) return [];

  const byKind = new Map<string, number>();
  let needsBinding = 0;
  for (const id of order) {
    const summary = summaries[id];
    const kind = summary?.kind;
    if (kind) byKind.set(kind, (byKind.get(kind) ?? 0) + 1);
    if (summary?.rel?.needsBinding) needsBinding += 1;
  }

  const facets: SceneFacet[] = [];
  for (const [kind, count] of byKind) {
    if (count > 0 && count < total) facets.push({ token: `kind:${kind}`, label: kindLabel(kind), count });
  }
  if (needsBinding > 0 && needsBinding < total) {
    facets.push({ token: "needs:binding", label: "Needs a binding", count: needsBinding });
  }

  facets.sort((a, b) => b.count - a.count || a.label.localeCompare(b.label));
  return facets;
}

/** Whether a query already carries a facet's token — what makes a chip a TOGGLE rather than a control
 *  that can only ever add. Compared against the PARSED query, not the raw string, so `KIND:Light` and
 *  `kind:light` are the one state they describe. */
export function queryHasFacet(query: SceneQuery, facet: SceneFacet): boolean {
  const colon = facet.token.indexOf(":");
  const key = facet.token.slice(0, colon);
  const value = facet.token.slice(colon + 1);
  if (key === "kind") return query.kinds.includes(value);
  if (key === "needs") return query.needsBinding;
  return false;
}

/** Add or remove a facet's token in the RAW query text, preserving everything else the user typed —
 *  a chip narrows what is already there rather than replacing it, which is the whole reason a person
 *  types a name and then clicks "Lights". */
export function toggleFacet(raw: string, facet: SceneFacet): string {
  const parsed = parseQuery(raw);
  if (!queryHasFacet(parsed, facet)) return raw.trim() ? `${raw.trim()} ${facet.token}` : facet.token;

  const colon = facet.token.indexOf(":");
  const key = facet.token.slice(0, colon);
  const value = facet.token.slice(colon + 1);
  const kept = raw
    .trim()
    .split(/\s+/)
    .filter((token) => {
      const c = token.indexOf(":");
      if (c <= 0) return true;
      const k = token.slice(0, c).toLocaleLowerCase();
      const v = token.slice(c + 1).toLocaleLowerCase();
      if (key === "kind") return !((k === "kind" || k === "type") && v === value);
      return !(k === "needs" && (v === "binding" || v === "bindings"));
    });
  return kept.join(" ");
}
