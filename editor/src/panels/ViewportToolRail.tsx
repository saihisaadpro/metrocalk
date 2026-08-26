//! Primary viewport tool selection. The rail is intentionally controlled: it presents and navigates tools,
//! while App decides what selecting each tool means and remains the single owner of engine state.

import { useId, useRef, useState, type CSSProperties, type KeyboardEvent, type SyntheticEvent } from "react";
import { Icon } from "../theme/icons";
import { Button } from "../theme/primitives";
import { ShortcutBadge } from "../theme/workspace";
import { color, elevation, motion, radius, space, z } from "../theme/tokens";

export type ViewportTool = "select" | "move" | "rotate" | "scale" | "pipe";

export interface ViewportToolAvailability {
  disabled?: boolean;
  /** Why the tool is unavailable. Displayed in its native tooltip. */
  reason?: string;
  hidden?: boolean;
}

export interface ViewportToolRailProps {
  activeTool: ViewportTool;
  onToolChange: (tool: ViewportTool) => void;
  availability?: Partial<Record<ViewportTool, ViewportToolAvailability>>;
  /** Controlled minimized state. Omit to let the rail manage it locally. */
  minimized?: boolean;
  defaultMinimized?: boolean;
  onMinimizedChange?: (minimized: boolean) => void;
  style?: CSSProperties;
  className?: string;
  "data-testid"?: string;
}

interface ToolDefinition {
  id: ViewportTool;
  label: string;
  description: string;
  shortcut?: string;
}

const TOOLS: readonly ToolDefinition[] = [
  { id: "select", label: "Select", description: "Select objects in the scene" },
  { id: "move", label: "Move", description: "Translate the active selection", shortcut: "W" },
  { id: "rotate", label: "Rotate", description: "Rotate the active selection", shortcut: "E" },
  { id: "scale", label: "Scale", description: "Resize the active selection", shortcut: "R" },
  { id: "pipe", label: "Pipe", description: "Draw and bake a routed pipe asset" },
] as const;

/** Stable ids retained from the original viewport toolbar for automation, deep links, and help content. */
const TOOL_ELEMENT_IDS: Partial<Record<ViewportTool, string>> = {
  select: "vpSelect",
  move: "vpMove",
  rotate: "vpRotate",
  scale: "vpScale",
  pipe: "vpPipe",
};

/** THE TOOL RAIL DOES NOT DRAW ITS OWN ICONS ANY MORE.
 *
 *  It used to carry five hand-authored `<path>`s in a local `ToolIcon` — at viewBox 24, stroke 1.7,
 *  round caps, which is to say it had independently arrived at the house conventions and there was no
 *  way to know that except by reading both files. That is the shape the Constitution forbids: a
 *  subsystem with its own icon component is one restyle away from being the odd one out, and the five
 *  marks could not be reused by a command palette or a context menu that wanted the same tool. They
 *  live in [`theme/icons.tsx`] now under `cursor`/`move`/`rotate`/`scale`/`pipe`, with `select` a
 *  declared alias for `cursor`, so one tool means one picture everywhere it appears. */
function ToolIcon({ tool }: { tool: ViewportTool }) {
  return <Icon name={tool} size="lg" className="mtk-tool-rail__icon" />;
}

/** Responsive vertical toolbar with roving keyboard focus and optional compact presentation. */
export function ViewportToolRail({
  activeTool,
  onToolChange,
  availability = {},
  minimized,
  defaultMinimized = false,
  onMinimizedChange,
  style,
  className,
  ...rest
}: ViewportToolRailProps) {
  const [localMinimized, setLocalMinimized] = useState(defaultMinimized);
  const toolbarId = `viewport-primary-tools-${useId().replace(/[^a-zA-Z0-9_-]/g, "-")}`;
  const compact = minimized ?? localMinimized;
  const railRef = useRef<HTMLElement>(null);
  const visibleTools = TOOLS.filter((tool) => !availability[tool.id]?.hidden);
  const enabledTools = visibleTools.filter((tool) => !availability[tool.id]?.disabled);
  const fallbackTool = enabledTools.some((tool) => tool.id === activeTool) ? activeTool : enabledTools[0]?.id;

  function toggleMinimized() {
    const next = !compact;
    if (minimized == null) setLocalMinimized(next);
    onMinimizedChange?.(next);
  }

  function moveFocus(event: KeyboardEvent<HTMLButtonElement>, current: ViewportTool) {
    const index = enabledTools.findIndex((tool) => tool.id === current);
    if (index < 0 || enabledTools.length === 0) return;
    let next: ToolDefinition | undefined;
    if (event.key === "ArrowDown" || event.key === "ArrowRight") next = enabledTools[(index + 1) % enabledTools.length];
    else if (event.key === "ArrowUp" || event.key === "ArrowLeft") next = enabledTools[(index - 1 + enabledTools.length) % enabledTools.length];
    else if (event.key === "Home") next = enabledTools[0];
    else if (event.key === "End") next = enabledTools[enabledTools.length - 1];
    else return;
    event.preventDefault();
    railRef.current?.querySelector<HTMLButtonElement>(`button[data-tool="${next.id}"]`)?.focus();
  }

  const stop = (event: SyntheticEvent) => event.stopPropagation();
  const classes = ["mtk-viewport-tool-rail", className].filter(Boolean).join(" ");

  return (
    <nav
      ref={railRef}
      className={classes}
      aria-label="Viewport tools"
      data-minimized={compact ? "true" : "false"}
      onPointerDown={stop}
      onClick={stop}
      onDoubleClick={stop}
      onContextMenu={stop}
      style={{
        position: "absolute",
        top: space.xxl * 2,
        left: space.sm,
        display: "flex",
        flexDirection: "column",
        width: compact ? 42 : 126,
        minWidth: 42,
        padding: space.xs,
        border: `1px solid ${color.border.default}`,
        borderRadius: radius.lg,
        background: color.bg.raised,
        boxShadow: elevation.e2,
        zIndex: z.chrome,
        pointerEvents: "auto",
        transition: `width ${motion.base}`,
        ...style,
      }}
      {...rest}
    >
      <div
        id={toolbarId}
        role="toolbar"
        aria-label="Primary viewport tools"
        aria-orientation="vertical"
        style={{ display: "flex", flexDirection: "column", gap: space.xxs }}
      >
        {visibleTools.map((tool) => {
          const state = availability[tool.id];
          const disabled = !!state?.disabled;
          const active = activeTool === tool.id;
          const title = `${tool.description}${tool.shortcut ? ` (${tool.shortcut})` : ""}${disabled && state?.reason ? ` — ${state.reason}` : ""}`;
          return (
            <Button
              key={tool.id}
              id={TOOL_ELEMENT_IDS[tool.id]}
              data-testid={TOOL_ELEMENT_IDS[tool.id]}
              type="button"
              variant="toggle"
              active={active}
              compact
              disabled={disabled}
              aria-label={tool.label}
              aria-pressed={active}
              aria-describedby={disabled && state?.reason ? `viewport-tool-${tool.id}-reason` : undefined}
              data-tool={tool.id}
              tabIndex={tool.id === fallbackTool ? 0 : -1}
              title={title}
              onClick={() => onToolChange(tool.id)}
              onKeyDown={(event) => moveFocus(event, tool.id)}
              style={{
                width: "100%",
                minHeight: 36,
                justifyContent: compact ? "center" : "flex-start",
                gap: space.sm,
                paddingInline: compact ? space.sm : space.md,
                overflow: "hidden",
              }}
            >
              <ToolIcon tool={tool.id} />
              {!compact && <span className="mtk-tool-rail__label">{tool.label}</span>}
              {!compact && tool.shortcut && <ShortcutBadge keys={tool.shortcut} style={{ marginLeft: "auto" }} />}
              {disabled && state?.reason && (
                <span id={`viewport-tool-${tool.id}-reason`} hidden>
                  {state.reason}
                </span>
              )}
            </Button>
          );
        })}
      </div>

      <div className="mtk-tool-rail__divider" aria-hidden="true" />
      <Button
        type="button"
        variant="ghost"
        compact
        icon
        aria-label={compact ? "Expand viewport tool names" : "Collapse viewport tool names"}
        aria-expanded={!compact}
        aria-controls={toolbarId}
        title={compact ? "Expand tool names" : "Collapse tool names"}
        onClick={toggleMinimized}
        style={{ alignSelf: compact ? "center" : "flex-end" }}
      >
        <span aria-hidden="true">{compact ? "›" : "‹"}</span>
      </Button>
    </nav>
  );
}
