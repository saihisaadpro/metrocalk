//! Asset browser (M10.2 / ADR-031, rebuilt for gate 6 in ADR-144) — a searchable, filterable library of
//! **large previews** over the **ONE M3.4 catalog** (registry + marketplace + imported assets).
//!
//! Implement this feature in accordance with the Engine UI/UX Architecture Constitution.
//!
//! IT DOES NOT FORK THE SEARCH. `catalog` (grouped by bucket, ADR-019) and `catalog_search` (the tiered
//! resolver, ranked + a no-match generate seam) stay the only two reads, and **place-into-scene** is
//! still `add_item` → one undoable, persisted, pre-componentized entity. What changed is everything above
//! that line.
//!
//! WHAT THE REBUILD ACTUALLY FIXES, BEYOND LOOKING LIKE THE REFERENCES.
//!
//!  1. **`std:Props` WAS THE GROUP HEADING.** `catalog()` keys its map by the CANONICAL bucket, because
//!     that is the identity two authors' custom categories collapse onto. The panel printed the key. So
//!     the shipped library filed things under `std:Props`, `std:Gameplay`, `std:Other` — engine-internal
//!     namespacing in user copy, `<ux_quality>` 4 exactly. `store/catalog.ts` projects it now, and the
//!     item's OWN `category` (`Companions (acme)`) became the per-tile tag it always should have been.
//!
//!  2. **ONE CLICK SPENT TOKENS.** The whole card was the place action, and `add_item` on a marketplace
//!     source is a metered BUY. A misclick in a browse grid debited the wallet with no price shown first
//!     and no confirmation — `<ux_quality>` 3's "never debit on a single un-confirmed click", in the one
//!     surface that lists priced things. A priced tile now states its price on the tile and opens a
//!     confirm that names the cost and the balance before anything is spent. Free local items still place
//!     on one click, because friction that buys nothing is not safety.
//!
//!  3. **THE NO-MATCH SEAM WAS A RHETORICAL STATUS LINE.** `no catalog match — generate "x"?` was a
//!     coloured sentence with no control in it: the question mark asked the user to answer a question the
//!     panel had given them no way to answer. `<ux_quality>` 1 calls that offloading the decisive step to
//!     a passive gutter. It is an empty state with a real button now, and the button runs the same
//!     `describe` the describe-bar runs.
//!
//!  4. **A STALE SEARCH COULD OVERWRITE A FRESH ONE.** Every keystroke fired a `catalog_search` and
//!     assigned whatever came back. Two in flight, the slower one landing second, and the grid shows the
//!     results for a prefix of what is in the box. A monotonic sequence number is the whole fix.

import { useEffect, useMemo, useRef, useState } from "react";

import { bucketLabel, catalogItemIcon, catalogTier } from "../store/catalog";
import { projectionStore } from "../store/projection";
import { recordPlacement, shelfKey, toggleFavourite, useFavourites, useRecent } from "../store/assetShelf";
import { pushToast } from "../store/toasts";
import { setStatus } from "../store/ui";
import { setBalance, useBalance } from "../store/wallet";
import { AssetChip, AssetGrid, AssetTile } from "../theme/assets";
import { Icon } from "../theme/icons";
import { DialogSurface, Modal } from "../theme/Popover";
import { Button, SearchField } from "../theme/primitives";
import { DisclosureSection, EmptyPanelState } from "../theme/workspace";
import { color, font, fontSize, space } from "../theme/tokens";
import type { CatalogItem } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

/** The one filter that is not a source tier. Kept in the same chip row because to a reader "show me only
 *  my starred ones" and "show me only the marketplace ones" are the same kind of question. */
const FAVOURITES = "__favourites__";

const keyOf = (item: CatalogItem): string => shelfKey(item.source, item.id);

export function AssetBrowser({ client }: { client: EditorClient }) {
  const [groups, setGroups] = useState<Record<string, CatalogItem[]>>({});
  const [loaded, setLoaded] = useState(false);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<CatalogItem[] | null>(null); // null = browse mode
  const [seam, setSeam] = useState<string | null>(null);
  const [tiers, setTiers] = useState<string[]>([]); // active tier filters; empty = everything
  const [onlyFavourites, setOnlyFavourites] = useState(false);
  const [pending, setPending] = useState<CatalogItem | null>(null); // the priced item awaiting confirmation
  const favourites = useFavourites();
  const recent = useRecent();
  const balance = useBalance();
  /** Monotonic search sequence — see the header note (4). A response whose number is not the newest is
   *  dropped, so the grid always shows the results for what is in the box. */
  const searchSeq = useRef(0);

  // Load the one catalog on mount (grouped by bucket).
  useEffect(() => {
    let live = true;
    client
      .catalog()
      .then((g) => {
        if (live) {
          setGroups(g);
          setLoaded(true);
        }
      })
      .catch(() => {
        if (live) {
          setGroups({});
          setLoaded(true);
        }
      });
    return () => {
      live = false;
    };
  }, [client]);

  const all = useMemo(() => Object.values(groups).flat(), [groups]);
  /** The tiers the catalog ACTUALLY contains, in first-seen order. Derived, never a hardcoded list: a
   *  filter for a tier nothing is filed under is a control that can only ever empty the panel. */
  const presentTiers = useMemo(() => {
    const seen: string[] = [];
    for (const item of all) if (!seen.includes(item.source)) seen.push(item.source);
    return seen;
  }, [all]);
  const byKey = useMemo(() => new Map(all.map((i) => [keyOf(i), i])), [all]);

  async function runSearch(q: string) {
    setQuery(q);
    if (!q.trim()) {
      searchSeq.current += 1; // invalidate anything in flight, so a late reply cannot re-open results
      setResults(null);
      setSeam(null);
      return;
    }
    const seq = (searchSeq.current += 1);
    const r = await client.catalogSearch(q);
    if (seq !== searchSeq.current) return;
    setResults(r.items);
    setSeam(r.items.length === 0 ? (r.seam ?? "generate") : null);
  }

  function passesFilters(item: CatalogItem): boolean {
    if (onlyFavourites && !favourites.includes(keyOf(item))) return false;
    return tiers.length === 0 || tiers.includes(item.source);
  }

  function toggleTier(tier: string) {
    if (tier === FAVOURITES) {
      setOnlyFavourites((on) => !on);
      return;
    }
    setTiers((on) => (on.includes(tier) ? on.filter((t) => t !== tier) : [...on, tier]));
  }

  /** Place the item. A priced one has already been through the confirm below by the time it gets here. */
  async function place(item: CatalogItem) {
    const before = balance;
    const r = await client.addItem(item.id, item.source);
    if (r.balance != null) setBalance(r.balance); // a marketplace buy debits — keep the wallet legible
    if (r.created) {
      // place + SELECT the result so it's visible/inspectable (C11 — feedback at the gesture). The cost
      // shown is the ACTUAL debit (balance delta), never a catalog-price guess that could disagree with
      // the charge.
      projectionStore.getState().select(r.created);
      recordPlacement(item.source, item.id);
      const spent = before != null && r.balance != null ? before - r.balance : null;
      const cost = spent && spent > 0 ? ` · −${spent} tokens` : "";
      pushToast(`Added ${item.label} · ${catalogTier(item.source).label}${cost}`, "success");
      setStatus(`added ${item.label} · ${item.source}`);
    } else if (r.seam) {
      pushToast(`${item.label}: ${r.seam}`, "error");
      setStatus(`${item.label}: ${r.seam}`);
    }
  }

  /** The gesture. Free ⇒ place; priced ⇒ ask first, with the price and the balance in the question. */
  function activate(item: CatalogItem) {
    if (item.price != null && item.price > 0) setPending(item);
    else void place(item);
  }

  async function generate() {
    // The same tier the describe-bar escalates to (ADR-012) — one resolution order, reached from the
    // surface where the user just discovered nothing local matches.
    const r = await client.describe(query);
    if (r.created) {
      projectionStore.getState().select(r.created);
      if (r.balance != null) setBalance(r.balance);
      pushToast(`Generated ${query}`, "success");
      setStatus(`generated ${query}`);
      void runSearch(query);
    } else {
      // `seam` is the shell's own word for why it stopped (ADR-012) — quoted rather than paraphrased, so
      // a refusal the engine explained is not replaced here by a guess about it.
      pushToast(`Could not generate “${query}”${r.seam ? ` — ${r.seam}` : ""}`, "error");
      setStatus(`generate failed: ${query}`);
    }
  }

  const tile = (item: CatalogItem, heading: string | null) => {
    const tier = catalogTier(item.source);
    const key = keyOf(item);
    const priced = item.price != null && item.price > 0;
    return (
      <AssetTile
        key={key}
        data-testid="asset-tile"
        activationHooks={{ "data-testid": "asset-item", "data-id": item.id, "data-source": item.source }}
        label={item.label}
        // The mark is the ITEM’s when the icon set draws its name and its COLLECTION’s otherwise — never
        // the tier’s, which is what the first build drew, so every local item in the library showed one
        // identical glyph and a grid of “large previews” previewed nothing.
        kind={catalogItemIcon(item.id, item.bucket)}
        tier={tier.label}
        tierHint={tier.hint}
        // Only when it says something the heading above did not: a `Props` tile inside *Props* repeating
        // its own bucket is noise, `Companions (acme)` inside *Characters* is the reason tags exist.
        tag={heading != null && item.category !== heading ? item.category : undefined}
        price={item.price ?? null}
        provides={item.provides}
        requires={item.requires}
        favourite={favourites.includes(key)}
        onToggleFavourite={() => toggleFavourite(item.source, item.id)}
        onActivate={() => activate(item)}
        actionLabel={priced ? `Buy and add ${item.label} for ${item.price} tokens` : `Add ${item.label} to the scene`}
      />
    );
  };

  // The `data-*` hooks live on a wrapper rather than on `DisclosureSection`: the section's props are a
  // typed `HTMLAttributes` with no `data-*` index signature, and widening a shared primitive's contract to
  // carry one panel's automation hooks is the wrong end to change.
  const collection = (id: string, title: string, items: CatalogItem[], defaultOpen: boolean) => (
    <div key={id} data-testid="asset-category" data-category={id}>
    <DisclosureSection
      title={title}
      summary={`${items.length}`}
      tone="plain"
      density="compact"
      headingLevel={4}
      landmark={false}
      defaultOpen={defaultOpen}
      storageKey={`asset-collection-${id}`}
    >
      <AssetGrid label={title}>{items.map((item) => tile(item, title))}</AssetGrid>
    </DisclosureSection>
    </div>
  );

  const browse = Object.entries(groups)
    .map(([bucket, items]) => [bucket, items.filter(passesFilters)] as const)
    .filter(([, items]) => items.length > 0);
  const filtered = results?.filter(passesFilters) ?? null;
  const favouriteItems = favourites.map((k) => byKey.get(k)).filter((i): i is CatalogItem => i != null);
  const recentItems = recent.map((k) => byKey.get(k)).filter((i): i is CatalogItem => i != null);

  function clearFilters() {
    setTiers([]);
    setOnlyFavourites(false);
  }

  return (
    <div id="assetbrowser" data-testid="assetbrowser" style={{ padding: `0 ${space.lg}px ${space.lg}px` }}>
      <SearchField
        id="assetSearch"
        data-testid="asset-search"
        value={query}
        aria-label="Search the asset library"
        placeholder="Search assets, or describe one…"
        onChange={(e) => void runSearch(e.target.value)}
        style={{ width: "100%", boxSizing: "border-box" }}
      />
      {(presentTiers.length > 1 || favourites.length > 0) && (
        <div
          role="group"
          aria-label="Filter the asset library"
          data-testid="asset-filters"
          style={{ display: "flex", flexWrap: "wrap", gap: space.xs, marginTop: space.md }}
        >
          {presentTiers.map((source) => {
            const tier = catalogTier(source);
            return (
              <AssetChip
                key={source}
                data-testid="asset-filter"
                data-filter={source}
                icon={tier.icon}
                pressed={tiers.includes(source)}
                onToggle={() => toggleTier(source)}
                title={`Show only ${tier.label} — ${tier.hint}`}
              >
                {tier.label}
              </AssetChip>
            );
          })}
          {favourites.length > 0 && (
            <AssetChip
              data-testid="asset-filter"
              data-filter="favourites"
              icon="star"
              pressed={onlyFavourites}
              onToggle={() => toggleTier(FAVOURITES)}
              title="Show only the assets you starred"
            >
              Favourites
            </AssetChip>
          )}
        </div>
      )}

      {filtered !== null ? (
        <div data-testid="asset-results">
          {filtered.length > 0 ? (
            collection("matches", "Matches", filtered, true)
          ) : seam ? (
            <EmptyPanelState
              data-testid="asset-seam"
              compact
              icon={<Icon name="sparkle" size="lg" />}
              title={`Nothing in the library matches “${query}”`}
              description="Everything local and on the marketplace has been searched. Generating makes a new asset from your words and costs tokens."
              primaryAction={
                <Button variant="primary" compact onClick={() => void generate()} data-testid="asset-generate">
                  <Icon name="sparkle" size="md" /> Generate “{query}”
                </Button>
              }
            />
          ) : (
            <EmptyPanelState
              data-testid="asset-filtered-out"
              compact
              icon={<Icon name="search" size="lg" />}
              title="Every match is filtered out"
              description="The library found matches, but none of them passes the filters above."
              primaryAction={
                <Button compact onClick={() => clearFilters()} data-testid="asset-clear-filters">
                  Clear filters
                </Button>
              }
            />
          )}
        </div>
      ) : !loaded ? (
        <div style={{ padding: `${space.lg}px 0`, font: font.ui, fontSize: fontSize.body, color: color.text.muted }}>Loading the library…</div>
      ) : all.length === 0 ? (
        <EmptyPanelState
          data-testid="asset-empty"
          compact
          icon={<Icon name="assets" size="lg" />}
          title="The library is empty"
          description="Nothing is installed and the marketplace is unreachable. Importing a file adds it here."
        />
      ) : (
        <div data-testid="asset-collections">
          {favouriteItems.length > 0 && !onlyFavourites && collection("favourites", "Favourites", favouriteItems, true)}
          {recentItems.length > 0 && collection("recent", "Recently placed", recentItems.filter(passesFilters), true)}
          {browse.map(([bucket, items], index) => collection(bucket, bucketLabel(bucket), items, index < 2))}
          {browse.length === 0 && (
            <EmptyPanelState
              data-testid="asset-filtered-out"
              compact
              icon={<Icon name="search" size="lg" />}
              title="Nothing passes these filters"
              description="The library has assets, but none of them matches the filters above."
              primaryAction={
                <Button compact onClick={() => clearFilters()} data-testid="asset-clear-filters">
                  Clear filters
                </Button>
              }
            />
          )}
        </div>
      )}

      {pending && (
        <Modal open onClose={() => setPending(null)} id="assetBuyGuard" ariaLabelledBy="assetBuyGuardTitle">
          <DialogSurface data-testid="asset-buy-guard" style={{ maxWidth: 360 }}>
            <div id="assetBuyGuardTitle" style={{ font: font.ui, fontSize: fontSize.title, fontWeight: 600, color: color.text.primary }}>
              Buy “{pending.label}”?
            </div>
            <div style={{ marginTop: space.md, font: font.ui, fontSize: fontSize.body, color: color.text.secondary }}>
              {catalogTier(pending.source).label} · <strong>{pending.price} tokens</strong>
              {balance != null && <> · you have {balance}</>}
            </div>
            <div style={{ display: "flex", gap: space.md, justifyContent: "flex-end", marginTop: space.xl }}>
              <Button data-testid="asset-buy-cancel" variant="secondary" compact onClick={() => setPending(null)}>
                Cancel
              </Button>
              <Button
                data-testid="asset-buy-confirm"
                variant="primary"
                compact
                disabled={balance != null && pending.price != null && balance < pending.price}
                disabledReason={`This costs ${pending.price} tokens and you have ${balance}`}
                onClick={() => {
                  const item = pending;
                  setPending(null);
                  void place(item);
                }}
              >
                Buy and add
              </Button>
            </div>
          </DialogSurface>
        </Modal>
      )}
    </div>
  );
}
