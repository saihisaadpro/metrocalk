//! **The shared asset-browsing framework** (ADR-144) — the one tile, grid and chip every surface that
//! offers content to place inherits, the way `theme/graph.tsx` is the one graph and
//! `PropertyRow` is the one property row.
//!
//! Implement this feature in accordance with the Engine UI/UX Architecture Constitution.
//!
//! WHAT THE CONSTITUTION ASKS FOR AND WHAT WAS THERE. `docs/UI/prompt.txt` § *Asset Browser* is a list of
//! eleven words — large previews, smart search, tags, collections, recent, favourites, filters, drag and
//! drop, animated previews, quick actions — closing with *"No dense file explorer feeling."* What shipped
//! was a single column of 40px rows carrying a 26px mark and two badges: the dense list the section
//! prohibits, and the only asset surface in the engine, so there was nothing for a second one to inherit.
//! Migration gate 6 (§13) is the last of the six and this is its shared root.
//!
//! WHY A TILE IS A COMPOSITION AND NOT ONE BUTTON. The favourite is a control and the tile is a control,
//! and a control drawn on top of another control is `shoot.mjs` R3 — whichever paints last takes the
//! click, decided by paint order rather than by intent. So the tile's own button covers the preview and
//! the name, and everything else — the tier, the star, the chips — sits in sibling rows BELOW it that
//! share no pixels with it. That constraint is why the tile has a footer at all, and the footer is where
//! the tier and the price ended up reading better than they ever did crammed into a 40px row.
//!
//! THE LAYOUT IS DECIDED BY A WIDTH, AND THE WIDTH IS ~124px. That is what a 300px Build dock gives a
//! tile in a two-column grid, and it is not negotiable by wanting more: the first build spent it on four
//! chips in a wrapping row (three stacked rows, no two tiles the same height, the tier truncated to
//! `In this en…`), the second on one meta line (`Local · nee…` at every width the grid can produce).
//! What fits is one word. So the price and the capability moved onto the PREVIEW — 110px of square that
//! was carrying nothing — and the footer kept the tier word alone beside the star. Nothing on a tile
//! ellipsises now, at any width the grid produces.
//!
//! WHAT THIS FILE DELIBERATELY DOES NOT HAVE. No `draggable`. "Drag and drop everywhere" needs a drop
//! target that can place an item at a world position, and the stage is a native wgpu surface the DOM
//! cannot drop onto; an attribute with no target is an affordance that does nothing, which `<ux_quality>`
//! 6 forbids more specifically than the constitution asks for it. It is tracked, not smuggled in.

import type { CSSProperties, ReactNode } from "react";

import { Icon } from "./icons";

/** `data-*` passthrough so a caller can hang its own automation hooks on a tile. */
type DataAttrs = { [dataAttr: `data-${string}`]: unknown };

// ── Chips ─────────────────────────────────────────────────────────────────────────────────────────

export interface AssetChipProps extends DataAttrs {
  children: ReactNode;
  /** A leading mark. Never the only carrier of meaning — the chip always states its word too. */
  icon?: string;
  /** Present ⇒ the chip is a FILTER (a real toggle carrying `aria-pressed`). Absent ⇒ a static TAG. */
  onToggle?: () => void;
  pressed?: boolean;
  title?: string;
  tone?: "neutral" | "accent" | "success" | "warn";
}

/** One chip: a filter when it can be toggled, a tag when it cannot.
 *
 *  Both spellings live in the same component on purpose. A filter chip and a category tag are the same
 *  object visually — that is what makes a filtered view legible, because the chip you pressed looks like
 *  the tag it matched — and splitting them into two components is how the two drift apart. */
export function AssetChip({ children, icon, onToggle, pressed = false, title, tone = "neutral", ...rest }: AssetChipProps) {
  const cls = ["mtk-chip", `mtk-chip--${tone}`, onToggle ? "mtk-chip--filter" : "mtk-chip--tag", pressed && "is-on"]
    .filter(Boolean)
    .join(" ");
  const body = (
    <>
      {icon != null && <Icon name={icon} size="sm" />}
      <span className="mtk-chip__text">{children}</span>
    </>
  );
  if (!onToggle) {
    return (
      <span className={cls} title={title} {...rest}>
        {body}
      </span>
    );
  }
  return (
    <button type="button" className={cls} aria-pressed={pressed} title={title} onClick={onToggle} {...rest}>
      {body}
    </button>
  );
}

// ── Grid ──────────────────────────────────────────────────────────────────────────────────────────

export interface AssetGridProps extends DataAttrs {
  children: ReactNode;
  /** Accessible name for the group of tiles (the collection this grid is showing). */
  label: string;
  style?: CSSProperties;
}

/** The responsive tile grid. Column COUNT is the browser's decision from a minimum tile width, so the
 *  same grid fills a 300px dock with two columns and a wide drawer with five without the caller doing
 *  arithmetic against a width it cannot measure. */
export function AssetGrid({ children, label, style, ...rest }: AssetGridProps) {
  return (
    <div className="mtk-asset-grid" role="group" aria-label={label} style={style} {...rest}>
      {children}
    </div>
  );
}

// ── Tile ──────────────────────────────────────────────────────────────────────────────────────────

export interface AssetTileProps extends DataAttrs {
  /** The name a reader is shown. Wraps to two lines, then ellipsises — an asset name is content. */
  label: string;
  /** The `theme/icons.tsx` name for the mark drawn when there is no rendered preview. Unresolvable
   *  names fall back to the generic shape rather than drawing an empty box. */
  kind: string;
  /** The preview. Omit and the tile draws `kind`'s mark, which is the honest state in a build with no
   *  renderer behind it (ADR-058: real RTT pixels are the `.exe`). Monochrome on purpose — a hue per
   *  item turns a grid of twelve into twelve competing colours, and the constitution asks for one
   *  disciplined accent, not a palette per collection. */
  preview?: ReactNode;
  /** The source tier, in the user's words — "Marketplace", not `marketplace`. */
  tier: string;
  /** The sentence the tier word is short for, for the meta line's tooltip. */
  tierHint?: string;
  /** The item's own collection, when it differs from the group it is filed under (`Companions (acme)`
   *  inside *Characters*). Omit when it would only repeat the heading above it. */
  tag?: string;
  /** Token price. `null`/omitted = free. A priced tile states its price BEFORE the action that spends. */
  price?: number | null;
  /** What this brings and what it needs, in the core's display vocabulary. */
  provides?: readonly string[];
  requires?: readonly string[];
  favourite?: boolean;
  onToggleFavourite?: () => void;
  onActivate: () => void;
  /** The accessible name of the tile's own button — the ACTION, so a screen reader hears what pressing
   *  it does rather than only what the thing is called. */
  actionLabel: string;
  selected?: boolean;
  disabled?: boolean;
  disabledReason?: string;
  /** `data-*` for the tile's own BUTTON rather than its frame.
   *
   *  Automation drives the element that PERFORMS the action, and after this refactor that is no longer
   *  the outermost node: `[data-testid="asset-item"]` used to be the whole card and is now the button
   *  inside it, so the E2E page object and the Vitest suite keep clicking the thing that places. Putting
   *  the hook anywhere else would leave `fireEvent.click` landing on a `div` that does nothing. */
  activationHooks?: DataAttrs;
}

/** One asset, as a large preview.
 *
 *  THE ANATOMY IS THREE ROWS, AND IT WAS FIVE. The first build put the tier, a capability chip, a price
 *  chip and a category tag on every tile: at the ~124px a 300px dock gives a tile, that wrapped into
 *  three stacked chip rows, made no two tiles the same height, and truncated the tier to `In this en…`.
 *
 *  WHERE EACH THING ENDED UP, AND THE WIDTH THAT DECIDED IT. A tile's footer has ~110px, and a star and
 *  its gap take 30 of them; a 1:1 preview well above it has the whole 110 and, until now, nothing in it.
 *  So the two things that ellipsised in a footer — the price and the capability — moved ONTO the preview,
 *  where they fit outright, and the footer kept the one thing that has to be a word in the flow of the
 *  tile: where this came from. Both overlays are labels, not controls, and they live inside the tile's own
 *  button, which is why they may sit over the preview at all: `shoot.mjs` R3 is about two CONTROLS sharing
 *  pixels, and the only other control on the tile is the star, which is a sibling below.
 *
 *  WHAT IS LEFT IN THE TOOLTIP is the FULL `provides`/`requires` lists and the sentence the tier word is
 *  short for. A list belongs in a tooltip; a tooltip is where nothing wraps. */
export function AssetTile({
  label,
  kind,
  preview,
  tier,
  tierHint,
  tag,
  price = null,
  provides = [],
  requires = [],
  favourite = false,
  onToggleFavourite,
  onActivate,
  actionLabel,
  selected = false,
  disabled = false,
  disabledReason,
  activationHooks,
  ...rest
}: AssetTileProps) {
  const cls = ["mtk-asset-tile", selected && "is-selected", disabled && "is-unavailable"].filter(Boolean).join(" ");
  // The constraint first, the benefit when there is no constraint — see the header note.
  const capability =
    requires.length > 0 ? `needs ${requires[0]}` : provides.length > 0 ? `adds ${provides[0]}` : null;
  // A CUSTOM collection out-ranks a capability on the one line there is room for. `Companions (acme)`
  // says something none of its siblings say; `needs Spatial` is true of every Prop in the library.
  const note = tag ?? capability;
  const tierTitle = tierHint != null ? `${tier} — ${tierHint}` : tier;
  // The NOTE's tooltip is the full picture, because the note is the abbreviation: one of two capability
  // lists, or a collection, standing in for all of it. The footer's tooltip is only about the tier.
  const noteTitle = [
    tag != null ? `filed under ${tag}` : null,
    requires.length > 0 ? `attaches to something providing ${requires.join(", ")}` : null,
    provides.length > 0 ? `brings ${provides.join(", ")}` : null,
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <div className={cls} {...rest}>
      <button
        type="button"
        className="mtk-asset-tile__main"
        aria-label={actionLabel}
        title={disabled ? disabledReason : actionLabel}
        disabled={disabled}
        onClick={disabled ? undefined : onActivate}
        {...activationHooks}
      >
        <span className="mtk-asset-tile__preview">
          {preview ?? <Icon name={kind} fallback="shape" size={44} />}
          {price != null && (
            <span className="mtk-asset-tile__badge">
              <AssetChip tone="warn" icon="tokens" title={`costs ${price} tokens`} data-testid="asset-price">
                {price}
              </AssetChip>
            </span>
          )}
          {note != null && (
            <span className="mtk-asset-tile__note">
              {/* NEUTRAL, including for a `requires`. Amber means caution, and "needs Spatial" is not a
                  caution — it is the ordinary fact that a prop attaches to something. Five of the six
                  Props in the library carry it, so painting it as a warning made the collection look
                  like a wall of problems. The words `needs` and `adds` carry the distinction, which is
                  what the constitution asks for anyway: never colour alone. The one warn tone on a tile
                  is the price, where caution is the point. */}
              <AssetChip tone="neutral" title={noteTitle} data-testid="asset-note">
                {note}
              </AssetChip>
            </span>
          )}
        </span>
        <span className="mtk-asset-tile__label">{label}</span>
      </button>
      <div className="mtk-asset-tile__foot">
        <span className="mtk-asset-tile__meta" title={tierTitle}>
          {tier}
        </span>
        {onToggleFavourite != null && (
          <button
            type="button"
            className={`mtk-asset-tile__fav${favourite ? " is-on" : ""}`}
            aria-pressed={favourite}
            aria-label={favourite ? `Remove ${label} from favourites` : `Add ${label} to favourites`}
            title={favourite ? "Remove from favourites" : "Add to favourites"}
            data-testid="asset-favourite"
            onClick={onToggleFavourite}
          >
            <Icon name={favourite ? "star" : "star-outline"} size="sm" />
          </button>
        )}
      </div>
    </div>
  );
}
