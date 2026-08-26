//! Search-first access to editor actions. The palette deliberately owns only discovery, focus, and feedback;
//! the caller supplies the commands and their real engine/UI callbacks. This keeps it useful across authoring
//! workspaces without creating a second command-dispatch system.

import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { DialogSurface, Modal } from "../theme/Popover";
import { Icon } from "../theme/icons";
import { Button } from "../theme/primitives";
import { ShortcutBadge } from "../theme/workspace";
import { color, fontSize, space, text } from "../theme/tokens";

interface EditorCommandBase {
  /** Stable unique id used for selection and tests. */
  id: string;
  label: string;
  category: string;
  /** One-line explanation shown below the command name. */
  description?: string;
  /** Additional search terms that do not need to appear in the visible label. */
  keywords?: readonly string[];
  /** Display-only shortcut. Global shortcut handling remains owned by the editor shell. */
  shortcut?: string | readonly string[];
  /** The real action. The palette closes only after this resolves successfully. */
  execute: () => void | Promise<void>;
}

/** A command, and — **if it can ever be off — the plain-language reason, required by the compiler**.
 *
 *  WHY A UNION RATHER THAN TWO OPTIONAL FIELDS. It was two optional fields, and the row below rendered
 *  `command.disabledReason ?? "This command is currently unavailable"` — a fallback that repeats the
 *  state back at the user and calls it an explanation. Every command in the shell happens to supply a
 *  real reason today, so that string has never been on screen; what it actually did was make the next
 *  silent refusal look like a handled case, in the one surface where a user goes precisely BECAUSE
 *  they cannot find the control. R9 in the shots gate catches a wordless refusal that a scene
 *  photographs. This catches it in `tsc`, for every command, photographed or not — and the fallback is
 *  gone, so there is nothing left to say the untrue thing. */
export type EditorCommand = EditorCommandBase &
  ({ disabled?: false | undefined; disabledReason?: undefined } | { disabled: boolean; disabledReason: string });

export interface CommandPaletteProps {
  open: boolean;
  onClose: () => void;
  commands: readonly EditorCommand[];
  title?: string;
  placeholder?: string;
  /** Receives command failures after the palette has presented them to the user. */
  onCommandError?: (error: unknown, command: EditorCommand) => void;
}

interface RankedCommand {
  command: EditorCommand;
  sourceIndex: number;
  rank: number;
}

function normalized(value: string): string {
  return value.trim().toLocaleLowerCase();
}

/** A small relevance pass keeps exact/prefix matches above metadata-only matches while remaining stable. */
function commandRank(command: EditorCommand, query: string): number | null {
  if (!query) return 0;
  const label = normalized(command.label);
  if (label === query) return 0;
  if (label.startsWith(query)) return 1;
  if (label.split(/\s+/).some((word) => word.startsWith(query))) return 2;
  if (label.includes(query)) return 3;
  if (command.keywords?.some((keyword) => normalized(keyword).includes(query))) return 4;
  if (normalized(command.category).includes(query)) return 5;
  if (command.description && normalized(command.description).includes(query)) return 6;
  return null;
}

function safeId(value: string): string {
  return value.replace(/[^a-zA-Z0-9_-]/g, "-");
}

function errorMessage(error: unknown): string {
  if (error instanceof Error && error.message.trim()) return error.message;
  return "The command could not be completed. Try again.";
}

/** Accessible, categorized command finder with combobox/listbox semantics and complete keyboard operation. */
export function CommandPalette({
  open,
  onClose,
  commands,
  title = "Command palette",
  placeholder = "Search tools and actions…",
  onCommandError,
}: CommandPaletteProps) {
  const generatedId = useId();
  const paletteId = `command-palette-${safeId(generatedId)}`;
  const titleId = `${paletteId}-title`;
  const hintId = `${paletteId}-hint`;
  const listId = `${paletteId}-results`;
  const inputRef = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState("");
  const [activeId, setActiveId] = useState<string | null>(null);
  const [runningId, setRunningId] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  const ranked = useMemo<RankedCommand[]>(() => {
    const needle = normalized(query);
    return commands
      .map((command, sourceIndex) => ({ command, sourceIndex, rank: commandRank(command, needle) }))
      .filter((item): item is RankedCommand => item.rank != null)
      .sort((a, b) => a.rank - b.rank || a.sourceIndex - b.sourceIndex);
  }, [commands, query]);

  const groups = useMemo(() => {
    const result = new Map<string, EditorCommand[]>();
    for (const { command } of ranked) {
      const category = command.category.trim() || "Other";
      const group = result.get(category);
      if (group) group.push(command);
      else result.set(category, [command]);
    }
    return Array.from(result.entries());
  }, [ranked]);

  // Follow the rendered (category-grouped) order so Arrow navigation always matches the user's eye path.
  const available = useMemo(
    () => groups.flatMap(([, group]) => group).filter((command) => !command.disabled),
    [groups],
  );

  useEffect(() => {
    if (!open) return;
    setQuery("");
    setFailure(null);
    setRunningId(null);
    setActiveId(commands.find((command) => !command.disabled)?.id ?? null);
    // Command arrays are commonly assembled by the parent during render. Reset only for a new open cycle,
    // never because that array received a new identity while the user was typing.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  useEffect(() => {
    if (!open) return;
    if (!available.some((command) => command.id === activeId)) {
      setActiveId(available[0]?.id ?? null);
    }
  }, [activeId, available, open]);

  function moveSelection(where: "next" | "previous" | "first" | "last") {
    if (available.length === 0) return;
    const current = available.findIndex((command) => command.id === activeId);
    if (where === "first") setActiveId(available[0].id);
    else if (where === "last") setActiveId(available[available.length - 1].id);
    else if (where === "next") setActiveId(available[(Math.max(current, -1) + 1) % available.length].id);
    else setActiveId(available[(current <= 0 ? available.length : current) - 1].id);
  }

  async function run(command: EditorCommand) {
    if (command.disabled || runningId != null) return;
    setFailure(null);
    setRunningId(command.id);
    try {
      await command.execute();
      onClose();
    } catch (error) {
      setFailure(errorMessage(error));
      onCommandError?.(error, command);
      inputRef.current?.focus();
    } finally {
      setRunningId(null);
    }
  }

  function onInputKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "ArrowDown" || event.key === "ArrowUp" || event.key === "Home" || event.key === "End") {
      event.preventDefault();
      if (event.key === "ArrowDown") moveSelection("next");
      else if (event.key === "ArrowUp") moveSelection("previous");
      else if (event.key === "Home") moveSelection("first");
      else moveSelection("last");
      return;
    }
    if (event.key === "Enter") {
      event.preventDefault();
      const command = available.find((item) => item.id === activeId);
      if (command) void run(command);
    }
  }

  const activeOptionId = activeId ? `${paletteId}-option-${safeId(activeId)}` : undefined;

  return (
    <Modal
      open={open}
      onClose={() => {
        if (runningId == null) onClose();
      }}
      initialFocusRef={inputRef}
      ariaLabelledBy={titleId}
      ariaDescribedBy={hintId}
      id={paletteId}
    >
      <DialogSurface
        className="mtk-command-palette"
        data-testid="command-palette"
        aria-busy={runningId != null || undefined}
        onClick={(event) => event.stopPropagation()}
        onPointerDown={(event) => event.stopPropagation()}
        style={{
          display: "flex",
          flexDirection: "column",
          width: "min(680px, calc(100vw - 24px))",
          maxHeight: "min(620px, calc(100vh - 32px))",
          overflow: "hidden",
        }}
      >
        <header
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            gap: space.md,
            padding: `${space.lg}px ${space.lg}px ${space.sm}px`,
          }}
        >
          <div style={{ minWidth: 0 }}>
            <h2 id={titleId} style={{ margin: 0, fontSize: fontSize.title, color: color.text.primary }}>
              {title}
            </h2>
            <div id={hintId} style={{ ...text.metadata, marginTop: space.xxs }}>
              Find any editor action without leaving the viewport
            </div>
          </div>
          <Button type="button" variant="ghost" icon compact aria-label="Close command palette" title="Close (Escape)" onClick={onClose} disabled={runningId != null}>
            <Icon name="close" size="sm" />
          </Button>
        </header>

        <div style={{ padding: `${space.xs}px ${space.lg}px ${space.md}px` }}>
          <input
            ref={inputRef}
            type="text"
            className="mtk-input"
            role="combobox"
            aria-label="Search commands"
            aria-autocomplete="list"
            aria-expanded={ranked.length > 0}
            aria-controls={ranked.length > 0 ? listId : undefined}
            aria-activedescendant={activeOptionId}
            autoComplete="off"
            spellCheck={false}
            placeholder={placeholder}
            value={query}
            disabled={runningId != null}
            onChange={(event) => {
              setQuery(event.target.value);
              setFailure(null);
            }}
            onKeyDown={onInputKeyDown}
            style={{ width: "100%", minHeight: 40, boxSizing: "border-box", fontSize: fontSize.label }}
          />
        </div>

        <div
          id={listId}
          role={ranked.length > 0 ? "listbox" : "status"}
          aria-label={ranked.length > 0 ? "Command results" : undefined}
          className="mtk-scroll mtk-command-palette__results"
          style={{ flex: "1 1 auto", minHeight: 0, overflow: "auto", padding: `0 ${space.sm}px ${space.sm}px` }}
        >
          {ranked.length === 0 ? (
            <div className="mtk-command-palette__empty">
              <strong>No matching commands</strong>
              <span>Try a tool name, task, or category.</span>
            </div>
          ) : (
            groups.map(([category, categoryCommands], groupIndex) => {
              const headingId = `${paletteId}-group-${groupIndex}`;
              return (
                <div key={category} role="group" aria-labelledby={headingId} className="mtk-command-palette__group">
                  <div id={headingId} className="mtk-command-palette__category">
                    {category}
                  </div>
                  {categoryCommands.map((command) => {
                    const active = activeId === command.id;
                    const running = runningId === command.id;
                    // A THIRD REFUSAL NOBODY HAD NAMED. The row below is disabled by `command.disabled
                    // || runningId != null` — so while ANY command is running, EVERY OTHER command in
                    // the palette goes dark, `command.disabled` is false for all of them, and the
                    // title fell through to `command.description`, which is optional. A palette full
                    // of grey rows and no sentence anywhere is the exact failure this session is
                    // about, in the surface a user opens *because* they could not find the control.
                    // Found by reading the line the type change made me look at, not by the probe: no
                    // scene photographs a running command.
                    const refusal = command.disabled
                      ? command.disabledReason
                      : running
                        ? `${command.label} is running`
                        : runningId != null
                          ? "Another command is still running — it finishes before the palette takes another"
                          : undefined;
                    return (
                      <button
                        key={command.id}
                        type="button"
                        role="option"
                        id={`${paletteId}-option-${safeId(command.id)}`}
                        className="mtk-command-palette__command"
                        aria-selected={active}
                        aria-disabled={command.disabled || undefined}
                        disabled={command.disabled || runningId != null}
                        title={refusal ?? command.description}
                        data-command-id={command.id}
                        data-active={active ? "true" : "false"}
                        onMouseEnter={() => {
                          if (!command.disabled && runningId == null) setActiveId(command.id);
                        }}
                        onMouseDown={(event) => event.preventDefault()}
                        onClick={() => void run(command)}
                      >
                        <span className="mtk-command-palette__command-copy">
                          <span className="mtk-command-palette__command-label">
                            {running && <span className="mtk-spinner" aria-hidden="true" />}
                            {command.label}
                          </span>
                          {(command.description || (command.disabled && command.disabledReason)) && (
                            <span className="mtk-command-palette__command-description">
                              {command.disabled && command.disabledReason ? command.disabledReason : command.description}
                            </span>
                          )}
                        </span>
                        {command.shortcut != null && <ShortcutBadge keys={command.shortcut} />}
                      </button>
                    );
                  })}
                </div>
              );
            })
          )}
        </div>

        <div className="mtk-command-palette__footer">
          <span role={failure ? "alert" : undefined} aria-live={failure ? "assertive" : "polite"}>
            {failure ?? `${ranked.length} command${ranked.length === 1 ? "" : "s"}`}
          </span>
          <span className="mtk-command-palette__help" aria-hidden="true">
            <Icon name="arrow-up" size="sm" /><Icon name="arrow-down" size="sm" /> navigate · Enter run · Esc close
          </span>
        </div>
      </DialogSurface>
    </Modal>
  );
}
