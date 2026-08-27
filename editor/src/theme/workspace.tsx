//! Workspace-level composition primitives for the editor redesign.
//!
//! These components sit one level above the small controls in `primitives.tsx`: they define predictable
//! panel anatomy, dock navigation, progressive disclosure, empty-state guidance, and shortcut notation.
//! Keeping those patterns shared prevents every feature panel from inventing its own header, scrolling,
//! keyboard navigation, and responsive behaviour.

import {
  createElement,
  useEffect,
  useId,
  useRef,
  useState,
  type CSSProperties,
  type HTMLAttributes,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import { Popover, PopoverSurface, type Placement } from "./Popover";
import { Icon } from "./icons";
import { Button, type ButtonProps } from "./primitives";

export interface DockTabItem {
  /** Stable value used for selection and change notifications. */
  id: string;
  label: ReactNode;
  /** Decorative leading visual. It is hidden from assistive technology; `label` remains the tab name. */
  icon?: ReactNode;
  /** Optional compact count/status displayed after the label. */
  badge?: ReactNode;
  disabled?: boolean;
  /** Concise explanation shown on hover, useful for less familiar workspaces. */
  tooltip?: string;
  /** Override the controlled panel id. A deterministic id is generated when the tab list has an `id`. */
  panelId?: string;
}

export interface DockTabsProps {
  tabs: readonly DockTabItem[];
  activeId: string;
  onChange: (id: string) => void;
  ariaLabel: string;
  orientation?: "horizontal" | "vertical";
  /** Supplying an id links tabs to their panels with `aria-controls`. */
  id?: string;
  style?: CSSProperties;
  "data-testid"?: string;
}

/**
 * Compact dock navigation following the WAI-ARIA tabs pattern. Selection uses a roving tab stop and
 * automatic activation because editor panels are local/instant. Arrow keys wrap and skip disabled tabs;
 * Home/End jump to the first/last available panel.
 */
export function DockTabs({
  tabs,
  activeId,
  onChange,
  ariaLabel,
  orientation = "horizontal",
  id,
  style,
  ...rest
}: DockTabsProps) {
  const refs = useRef(new Map<string, HTMLButtonElement>());
  const enabled = tabs.filter((tab) => !tab.disabled);

  function activate(tab: DockTabItem | undefined) {
    if (!tab || tab.disabled) return;
    onChange(tab.id);
    refs.current.get(tab.id)?.focus();
  }

  function onKeyDown(event: KeyboardEvent<HTMLButtonElement>, currentId: string) {
    const current = enabled.findIndex((tab) => tab.id === currentId);
    if (current < 0 || enabled.length === 0) return;

    const previousKey = orientation === "horizontal" ? "ArrowLeft" : "ArrowUp";
    const nextKey = orientation === "horizontal" ? "ArrowRight" : "ArrowDown";
    let next: DockTabItem | undefined;
    if (event.key === previousKey) next = enabled[(current - 1 + enabled.length) % enabled.length];
    else if (event.key === nextKey) next = enabled[(current + 1) % enabled.length];
    else if (event.key === "Home") next = enabled[0];
    else if (event.key === "End") next = enabled[enabled.length - 1];
    else return;

    event.preventDefault();
    activate(next);
  }

  const fallbackId = enabled[0]?.id;
  return (
    <div
      id={id}
      className={`mtk-dock-tabs mtk-dock-tabs--${orientation}`}
      role="tablist"
      aria-label={ariaLabel}
      aria-orientation={orientation}
      style={style}
      {...rest}
    >
      {tabs.map((tab) => {
        const selected = tab.id === activeId;
        const tabId = id ? `${id}-${tab.id}-tab` : undefined;
        const panelId = tab.panelId ?? (id ? `${id}-${tab.id}-panel` : undefined);
        return (
          <button
            key={tab.id}
            ref={(node) => {
              if (node) refs.current.set(tab.id, node);
              else refs.current.delete(tab.id);
            }}
            type="button"
            id={tabId}
            className="mtk-dock-tab"
            role="tab"
            aria-selected={selected}
            aria-controls={panelId}
            disabled={tab.disabled}
            tabIndex={selected || (activeId === "" && tab.id === fallbackId) ? 0 : -1}
            title={tab.tooltip}
            onClick={() => activate(tab)}
            onKeyDown={(event) => onKeyDown(event, tab.id)}
          >
            {tab.icon != null && (
              <span className="mtk-dock-tab__icon" aria-hidden="true">
                {tab.icon}
              </span>
            )}
            <span className="mtk-dock-tab__label">{tab.label}</span>
            {tab.badge != null && <span className="mtk-dock-tab__badge">{tab.badge}</span>}
          </button>
        );
      })}
    </div>
  );
}

export interface DockRailItem {
  id: string;
  label: string;
  icon: ReactNode;
  badge?: ReactNode;
}

export interface DockRailProps {
  side: "left" | "right";
  label: string;
  items: readonly DockRailItem[];
  activeId: string;
  popupOpen?: boolean;
  onActivate: (id: string, anchor: HTMLButtonElement) => void;
  "data-testid"?: string;
}

/** A calm icon rail for collapsed docks; activation opens a drawer or an anchored quick panel. */
export function DockRail({ side, label, items, activeId, popupOpen = false, onActivate, ...rest }: DockRailProps) {
  // No `--${side}` modifier on the shell: the rail is a capsule centred in its track and neither side
  // has ever had a rule, so the class was two dead names in the DOM. `side` still distinguishes the
  // rail's buttons (`data-testid`) and its aria-label.
  return (
    <aside className="mtk-dock-rail-shell" aria-label={`${label} collapsed dock`} {...rest}>
      <div className="mtk-dock-rail" role="toolbar" aria-label={label} aria-orientation="vertical">
        {items.map((item) => {
          const active = item.id === activeId;
          return (
            <Button
              key={item.id}
              data-testid={`rail-${side}-${item.id}`}
              variant="toggle"
              active={active}
              compact
              icon
              aria-label={`Open ${item.label}`}
              aria-haspopup="dialog"
              aria-expanded={active && popupOpen}
              title={item.label}
              onClick={(event) => onActivate(item.id, event.currentTarget)}
            >
              <span aria-hidden="true">{item.icon}</span>
              {item.badge != null && <span className="mtk-dock-rail__badge">{item.badge}</span>}
            </Button>
          );
        })}
      </div>
    </aside>
  );
}

const DISCLOSURE_STORAGE_PREFIX = "metrocalk:disclosure:";

function storedDisclosure(storageKey: string | undefined, fallback: boolean): boolean {
  if (!storageKey || typeof window === "undefined") return fallback;
  try {
    const value = window.localStorage.getItem(`${DISCLOSURE_STORAGE_PREFIX}${storageKey}`);
    return value == null ? fallback : value === "open";
  } catch {
    return fallback;
  }
}

export interface DisclosureSectionProps extends Omit<HTMLAttributes<HTMLElement>, "children" | "title" | "onChange"> {
  title: ReactNode;
  children: ReactNode;
  summary?: ReactNode;
  icon?: ReactNode;
  actions?: ReactNode;
  defaultOpen?: boolean;
  /** Controlled open state. Omit for locally-managed disclosure. */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** Optional localStorage key, automatically namespaced to the editor. */
  storageKey?: string;
  /** Removes closed content from the tree. By default it stays mounted so draft form state is preserved. */
  unmountOnClose?: boolean;
  id?: string;
  headingLevel?: 2 | 3 | 4 | 5 | 6;
  tone?: "plain" | "card";
  density?: "compact" | "default";
  disabled?: boolean;
  /** Expose the content as a named region. Disable for small repeated groups to reduce landmark noise. */
  landmark?: boolean;
}

/** A labelled, keyboard-native progressive-disclosure section with optional persisted open state. */
export function DisclosureSection({
  title,
  children,
  summary,
  icon,
  actions,
  defaultOpen = true,
  open,
  onOpenChange,
  storageKey,
  unmountOnClose = false,
  id,
  headingLevel = 3,
  tone = "plain",
  density = "default",
  disabled = false,
  landmark = true,
  className,
  ...rest
}: DisclosureSectionProps) {
  const generatedId = useId();
  const rootId = id ?? `mtk-disclosure-${generatedId}`;
  const buttonId = `${rootId}-toggle`;
  const contentId = `${rootId}-content`;
  const titleId = `${rootId}-title`;
  const summaryId = `${rootId}-summary`;
  const [localOpen, setLocalOpen] = useState(() => storedDisclosure(storageKey, defaultOpen));
  const expanded = open ?? localOpen;

  useEffect(() => {
    if (!storageKey || typeof window === "undefined") return;
    try {
      window.localStorage.setItem(`${DISCLOSURE_STORAGE_PREFIX}${storageKey}`, expanded ? "open" : "closed");
    } catch {
      // Storage may be unavailable (private mode / locked-down webviews); disclosure still works locally.
    }
  }, [expanded, storageKey]);

  function toggle() {
    if (disabled) return;
    const next = !expanded;
    if (open == null) setLocalOpen(next);
    onOpenChange?.(next);
  }

  const classes = [
    "mtk-disclosure",
    `mtk-disclosure--${tone}`,
    `mtk-disclosure--${density}`,
    className,
  ].filter(Boolean).join(" ");
  const heading = createElement(
    `h${headingLevel}`,
    { className: "mtk-disclosure__heading-wrap" },
    <button
      id={buttonId}
      type="button"
      className="mtk-disclosure__toggle"
      aria-expanded={expanded}
      aria-controls={contentId}
      aria-labelledby={titleId}
      aria-describedby={summary != null ? summaryId : undefined}
      disabled={disabled}
      onClick={toggle}
    >
      <span className={`mtk-disclosure__caret${expanded ? " is-open" : ""}`} aria-hidden="true"><Icon name="chevron-right" size="sm" /></span>
      {icon != null && <span className="mtk-disclosure__icon" aria-hidden="true">{icon}</span>}
      <span className="mtk-disclosure__heading">
        <span id={titleId} className="mtk-disclosure__title">{title}</span>
        {summary != null && <span id={summaryId} className="mtk-disclosure__summary">{summary}</span>}
      </span>
    </button>,
  );

  return (
    <section id={rootId} className={classes} data-state={expanded ? "open" : "closed"} {...rest}>
      <div className="mtk-disclosure__header">
        {heading}
        {actions != null && <div className="mtk-disclosure__actions">{actions}</div>}
      </div>
      {(!unmountOnClose || expanded) && (
        <div
          id={contentId}
          className={`mtk-disclosure__region${expanded ? " is-open" : ""}`}
          role={landmark ? "region" : undefined}
          aria-labelledby={buttonId}
          aria-hidden={!expanded}
          // The collapsed region stays mounted (draft form state survives), so mark it hidden/inert rather
          // than merely invisible. `.mtk-disclosure__region` sets `display: grid`, and author styles beat the
          // UA `[hidden] { display: none }` rule, so the open/close transition still runs.
          hidden={!expanded}
          inert={!expanded ? true : undefined}
        >
          <div className="mtk-disclosure__content">{children}</div>
        </div>
      )}
    </section>
  );
}

export interface MenuPopupProps {
  id: string;
  label: string;
  trigger: ReactNode;
  triggerProps?: Omit<ButtonProps, "children" | "onClick" | "onKeyDown" | "aria-haspopup" | "aria-expanded" | "aria-controls">;
  children: (close: () => void) => ReactNode;
  placement?: Placement;
  onOpenChange?: (open: boolean) => void;
  surfaceClassName?: string;
  surfaceStyle?: CSSProperties;
}

/**
 * Opinionated anchored action menu. It owns trigger semantics, edge-aware placement, keyboard entry,
 * dismissal and focus restoration so feature code only supplies grouped menu content.
 */
export function MenuPopup({
  id,
  label,
  trigger,
  triggerProps,
  children,
  placement = "bottom-start",
  onOpenChange,
  surfaceClassName,
  surfaceStyle,
}: MenuPopupProps) {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuId = `${id}-menu`;

  function changeOpen(next: boolean) {
    setOpen(next);
    onOpenChange?.(next);
  }

  return (
    <>
      <Button
        {...triggerProps}
        ref={triggerRef}
        id={triggerProps?.id ?? `${id}-trigger`}
        active={triggerProps?.variant === "toggle" ? open : triggerProps?.active}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? menuId : undefined}
        onClick={() => changeOpen(!open)}
        onKeyDown={(event) => {
          if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
          event.preventDefault();
          changeOpen(true);
        }}
      >
        {trigger}
      </Button>
      <Popover
        id={menuId}
        open={open}
        anchor={triggerRef}
        returnFocus={triggerRef}
        placement={placement}
        role="menu"
        ariaLabel={label}
        onClose={() => changeOpen(false)}
      >
        <PopoverSurface className={["mtk-popup-menu", surfaceClassName].filter(Boolean).join(" ")} style={surfaceStyle}>
          {children(() => changeOpen(false))}
        </PopoverSurface>
      </Popover>
    </>
  );
}

export interface PopupMenuItemProps extends Omit<ButtonProps, "children" | "onClick" | "disabled"> {
  label: ReactNode;
  description?: ReactNode;
  leading?: ReactNode;
  meta?: ReactNode;
  disabled?: boolean;
  disabledReason?: string;
  onSelect: () => void | Promise<void>;
  onRequestClose?: () => void;
}

/** A spacious, reference-informed menu row with optional explanation and recovery-aware disabled state. */
export function PopupMenuItem({
  label,
  description,
  leading,
  meta,
  disabled = false,
  disabledReason,
  onSelect,
  onRequestClose,
  className,
  role = "menuitem",
  variant = "ghost",
  ...rest
}: PopupMenuItemProps) {
  return (
    <Button
      {...rest}
      type="button"
      role={role}
      variant={variant}
      className={["mtk-popup-menu__item", leading == null && "is-without-icon", className].filter(Boolean).join(" ")}
      aria-disabled={disabled || undefined}
      title={disabled ? disabledReason : rest.title}
      onClick={() => {
        if (disabled) return;
        try {
          const result: unknown = onSelect();
          // A synchronous action is already done, so dismiss in the same tick — `.finally()` always defers to
          // a microtask, which left the menu covering the change it had just made. Only a genuinely pending
          // action keeps the menu open, where it carries that item's busy state until the work settles.
          const pending = typeof (result as PromiseLike<void> | undefined)?.then === "function";
          if (pending) void Promise.resolve(result).finally(() => onRequestClose?.());
          else onRequestClose?.();
        } catch (error) {
          onRequestClose?.();
          throw error;
        }
      }}
    >
      {leading != null && <span className="mtk-popup-menu__icon" aria-hidden="true">{leading}</span>}
      <span className="mtk-popup-menu__copy">
        <span className="mtk-popup-menu__label">{label}</span>
        {description != null && <span className="mtk-popup-menu__description">{description}</span>}
        {disabled && disabledReason && <span className="mtk-popup-menu__description">{disabledReason}</span>}
      </span>
      {meta != null && <span className="mtk-popup-menu__meta">{meta}</span>}
    </Button>
  );
}

export function PopupMenuGroup({ label, children }: { label: ReactNode; children: ReactNode }) {
  const generatedId = useId();
  const labelId = `mtk-popup-group-${generatedId}`;
  return (
    <div className="mtk-popup-menu__group" role="group" aria-labelledby={labelId}>
      <div id={labelId} className="mtk-popup-menu__group-label">{label}</div>
      {children}
    </div>
  );
}

export interface EmptyPanelStateProps {
  title: ReactNode;
  description?: ReactNode;
  icon?: ReactNode;
  /** Primary and secondary actions are slots so callers can use the shared Button primitive. */
  primaryAction?: ReactNode;
  secondaryAction?: ReactNode;
  children?: ReactNode;
  compact?: boolean;
  /** Icon beside the words rather than above them — for a WIDE, SHORT band (a timeline lane strip, a
   *  results row) where the stacked form needs 128px of height that the band does not have, and the
   *  title ends up below the fold with only the icon on screen. */
  inline?: boolean;
  style?: CSSProperties;
  "data-testid"?: string;
}

/** Consistent, actionable empty-state guidance for docked panels. */
export function EmptyPanelState({
  title,
  description,
  icon,
  primaryAction,
  secondaryAction,
  children,
  compact = false,
  inline = false,
  style,
  ...rest
}: EmptyPanelStateProps) {
  return (
    <div
      className={[
        "mtk-empty-panel",
        compact && "mtk-empty-panel--compact",
        inline && "mtk-empty-panel--inline",
      ]
        .filter(Boolean)
        .join(" ")}
      role="status"
      style={style}
      {...rest}
    >
      {icon != null && (
        <div className="mtk-empty-panel__icon" aria-hidden="true">
          {icon}
        </div>
      )}
      <div className="mtk-empty-panel__title">{title}</div>
      {description != null && <div className="mtk-empty-panel__description">{description}</div>}
      {children}
      {(primaryAction != null || secondaryAction != null) && (
        <div className="mtk-empty-panel__actions">
          {primaryAction}
          {secondaryAction}
        </div>
      )}
    </div>
  );
}

export interface ShortcutBadgeProps {
  keys: string | readonly string[];
  /** Spoken expansion, e.g. "Control plus Shift plus P". Defaults to the visible key sequence. */
  ariaLabel?: string;
  title?: string;
  style?: CSSProperties;
}

/** Platform-neutral keyboard shortcut notation. */
export function ShortcutBadge({ keys, ariaLabel, title, style }: ShortcutBadgeProps) {
  const parts = typeof keys === "string" ? [keys] : keys;
  return (
    <kbd
      className="mtk-shortcut"
      aria-label={ariaLabel ?? parts.join(" plus ")}
      title={title}
      style={style}
    >
      {parts.map((part, index) => (
        <span key={`${part}-${index}`}>
          {index > 0 && (
            <span className="mtk-shortcut__join" aria-hidden="true">
              +
            </span>
          )}
          <span>{part}</span>
        </span>
      ))}
    </kbd>
  );
}

export interface CanvasSplitProps {
  /** The subject — a graph, a preview, a timeline. Takes whatever width is left, and stays put. */
  canvas: ReactNode;
  /** What belongs UNDER the subject rather than beside it: the editor whose rows are wide sentences,
   *  which a 340px track can only wrap. It scrolls; the subject above it does not. */
  below?: ReactNode;
  /** The controls that shape it. A fixed track, because a control column that stretches is a column
   *  of half-empty rows. */
  children: ReactNode;
  /** Put the subject away and give its room to `below`. "Everything collapsible" is a constitution
   *  layout rule, and on a dock with a hard 520px ceiling it is the difference between an editor and
   *  a porthole: measured, a graph at its 200px floor plus two section headings leaves 68px for a
   *  143px card, and no arrangement of the three fixes that — one of them has to be able to go. */
  canvasHidden?: boolean;
  className?: string;
  style?: CSSProperties;
  "data-testid"?: string;
}

/**
 * A CANVAS AND THE CONTROLS THAT SHAPE IT — the constitution's layout rule, as a component.
 *
 * "Viewport in the centre, properties on the right" is stated for the shell and then owed by every
 * editor inside it. A workspace dock is a WIDE, SHORT band (the Logic dock is ~1240x520), so an
 * editor that stacks its subject above its controls puts the controls below the fold and photographs
 * as a graph with nothing to do — which is exactly what the state-machine editor did.
 *
 * Below 980px the two stack, because at that width a 340px control track is a third of the panel.
 */
export function CanvasSplit({ canvas, below, children, canvasHidden = false, className, style, ...rest }: CanvasSplitProps) {
  return (
    <div
      className={["mtk-canvas-split", canvasHidden && "is-canvas-hidden", className].filter(Boolean).join(" ")}
      style={style}
      {...rest}
    >
      {/* Source order is the STACKED order — subject, then the controls beside it, then what sat
          under it — because that is the order a reader wants at one column wide and it is the tab
          order at every width. The two-column arrangement is grid placement, not markup order. */}
      {/* Unmounted rather than `display: none`: a graph canvas measures itself on mount, and a
          hidden one comes back with a viewport it computed for a zero-sized box. */}
      {!canvasHidden && <div className="mtk-canvas-split__subject">{canvas}</div>}
      <div className="mtk-canvas-split__side">{children}</div>
      {below != null && <div className="mtk-canvas-split__below">{below}</div>}
    </div>
  );
}

export interface WorkspacePanelProps extends Omit<HTMLAttributes<HTMLElement>, "title"> {
  title: ReactNode;
  subtitle?: ReactNode;
  icon?: ReactNode;
  actions?: ReactNode;
  actionsLabel?: string;
  /** Optional navigation row pinned beneath the title bar. */
  tabs?: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
  scroll?: boolean;
  padded?: boolean;
  busy?: boolean;
  headingId?: string;
}

/**
 * Canonical editor panel anatomy: fixed header/actions, optional fixed tabs, one flexible scroll region, and
 * an optional fixed footer. The section is labelled by its visible title and exposes busy state to AT.
 */
export function WorkspacePanel({
  title,
  subtitle,
  icon,
  actions,
  actionsLabel = "Panel actions",
  tabs,
  children,
  footer,
  scroll = true,
  padded = false,
  busy = false,
  headingId,
  className,
  ...rest
}: WorkspacePanelProps) {
  const generatedId = useId();
  const titleId = headingId ?? `mtk-panel-title-${generatedId}`;
  const classes = ["mtk-workspace-panel", className].filter(Boolean).join(" ");
  const bodyClasses = [
    "mtk-workspace-panel__body",
    scroll && "mtk-scroll",
    padded && "is-padded",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <section className={classes} aria-labelledby={titleId} aria-busy={busy || undefined} {...rest}>
      <header className="mtk-workspace-panel__header">
        <div className="mtk-workspace-panel__identity">
          {icon != null && (
            <span className="mtk-workspace-panel__icon" aria-hidden="true">
              {icon}
            </span>
          )}
          <div className="mtk-workspace-panel__titles">
            <h2 id={titleId} className="mtk-workspace-panel__title">
              {title}
            </h2>
            {subtitle != null && <div className="mtk-workspace-panel__subtitle">{subtitle}</div>}
          </div>
        </div>
        {actions != null && (
          <div className="mtk-workspace-panel__actions" role="toolbar" aria-label={actionsLabel}>
            {actions}
          </div>
        )}
      </header>
      {tabs != null && <div className="mtk-workspace-panel__tabs">{tabs}</div>}
      <div className={bodyClasses}>{children}</div>
      {footer != null && <footer className="mtk-workspace-panel__footer">{footer}</footer>}
    </section>
  );
}

export interface NavRailItem {
  id: string;
  label: string;
  icon?: ReactNode;
  /** Plain-language hint — what this section is for. Becomes the item's `title`. */
  tooltip?: string;
  badge?: ReactNode;
  disabled?: boolean;
  /** Why it is refusing, in the user's words. Shown as the `title` while disabled. */
  disabledReason?: string;
}

export interface NavRailProps {
  id: string;
  label: string;
  items: readonly NavRailItem[];
  activeId: string;
  onChange: (id: string) => void;
  /** Prefix of the id each item's panel carries, so the tab and its panel name each other. */
  panelIdPrefix?: string;
  className?: string;
  style?: CSSProperties;
  "data-testid"?: string;
}

/**
 * A LABELLED VERTICAL RAIL — the shape a workspace with more sections than a tab strip can hold.
 *
 * `DockRail` above is the icon-only capsule a COLLAPSED dock shows; there was no labelled form, so a
 * panel with seven stages had exactly one answer available to it: `DockTabs` with `overflow-x: auto`,
 * which is the pattern `Toolbar`'s own comment calls out for putting controls off screen with nothing
 * on screen to say they exist. On a wide, short surface the rail is the right shape twice over — every
 * section is visible at once, and the width it costs is the axis that surface has to spare.
 *
 * It is a real tablist with roving arrow-key navigation, so the seven stages are one stop in the tab
 * order rather than seven, and the stylesheet turns it back into a horizontal strip below 860px.
 */
export function NavRail({
  id,
  label,
  items,
  activeId,
  onChange,
  panelIdPrefix,
  className,
  style,
  ...rest
}: NavRailProps) {
  const enabled = items.filter((item) => !item.disabled);

  function move(delta: number) {
    if (enabled.length === 0) return;
    const at = enabled.findIndex((item) => item.id === activeId);
    const next = enabled[(((at < 0 ? 0 : at) + delta) % enabled.length + enabled.length) % enabled.length];
    onChange(next.id);
    document.getElementById(`${id}-${next.id}-tab`)?.focus();
  }

  return (
    <div
      className={["mtk-nav-rail", className].filter(Boolean).join(" ")}
      role="tablist"
      aria-label={label}
      aria-orientation="vertical"
      style={style}
      {...rest}
    >
      {items.map((item) => {
        const active = item.id === activeId;
        return (
          <button
            key={item.id}
            id={`${id}-${item.id}-tab`}
            type="button"
            role="tab"
            className="mtk-nav-rail__item"
            aria-selected={active}
            aria-controls={panelIdPrefix ? `${panelIdPrefix}${item.id}` : undefined}
            // One stop in the tab order for the whole rail; the arrows move within it.
            tabIndex={active ? 0 : -1}
            disabled={item.disabled}
            title={item.disabled ? item.disabledReason ?? item.tooltip : item.tooltip}
            onClick={() => onChange(item.id)}
            onKeyDown={(event) => {
              if (event.key === "ArrowDown" || event.key === "ArrowRight") { event.preventDefault(); move(1); }
              else if (event.key === "ArrowUp" || event.key === "ArrowLeft") { event.preventDefault(); move(-1); }
            }}
          >
            {item.icon != null && <span className="mtk-nav-rail__icon" aria-hidden="true">{item.icon}</span>}
            <span className="mtk-nav-rail__label">{item.label}</span>
            {item.badge != null && <span className="mtk-nav-rail__badge">{item.badge}</span>}
          </button>
        );
      })}
    </div>
  );
}
