//! Production editor header: project identity and file operations on the left, play transport in the
//! centre, and high-frequency workspace/command access on the right. Creation and optional AI are no
//! longer allowed to consume a permanent full-width row above the viewport.

import { FileMenu } from "../panels/FileMenu";
import { PlayControls } from "../panels/PlayControls";
import { Wallet } from "../panels/Wallet";
import { Button } from "../theme/primitives";
import { MenuPopup, PopupMenuGroup, PopupMenuItem, ShortcutBadge } from "../theme/workspace";
import { color, font, fontSize, space } from "../theme/tokens";
import { setStatus } from "../store/ui";
import type { EditorClient } from "../transport/session";

export interface EditorHeaderProps {
  client: EditorClient;
  onOpenCreate: () => void;
  onOpenLogic: () => void;
  onOpenReview: () => void;
  onOpenCommands: () => void;
  onOpenLeftDock?: () => void;
  onOpenRightDock?: () => void;
  compact?: boolean;
}

function WorkspaceMenu({
  compact,
  onOpenCreate,
  onOpenLogic,
  onOpenReview,
}: Pick<EditorHeaderProps, "compact" | "onOpenCreate" | "onOpenLogic" | "onOpenReview">) {
  return (
    <MenuPopup
      id="header-workspaces-popup"
      label="Open engine workspace"
      placement="bottom-end"
      trigger={compact ? <span aria-hidden="true">◇</span> : <>Workspaces <span aria-hidden="true">⌄</span></>}
      triggerProps={{
        "data-testid": "header-workspaces",
        variant: "ghost",
        compact: true,
        icon: compact,
        "aria-label": compact ? "Open engine workspace" : undefined,
        title: "Open creation, logic, and review workspaces",
      }}
    >
      {(close) => (
        <PopupMenuGroup label="Workspaces">
          <PopupMenuItem
            data-testid="header-create"
            label="Create"
            description="Assets, imports and procedural tools"
            leading="＋"
            onSelect={onOpenCreate}
            onRequestClose={close}
          />
          <PopupMenuItem
            data-testid="header-logic"
            label="Logic"
            description="Rules, state machines and bindings"
            leading="◇"
            onSelect={onOpenLogic}
            onRequestClose={close}
          />
          <PopupMenuItem
            data-testid="header-review"
            label="Review"
            description="Import fidelity and diagnostics"
            leading="✓"
            onSelect={onOpenReview}
            onRequestClose={close}
          />
        </PopupMenuGroup>
      )}
    </MenuPopup>
  );
}

export function EditorHeader({
  client,
  onOpenCreate,
  onOpenLogic,
  onOpenReview,
  onOpenCommands,
  onOpenLeftDock,
  onOpenRightDock,
  compact = false,
}: EditorHeaderProps) {
  async function undo() {
    const changed = await client.undo().catch(() => false);
    setStatus(changed ? "Undo complete" : "Nothing to undo");
  }

  async function redo() {
    const changed = await client.redo().catch(() => false);
    setStatus(changed ? "Redo complete" : "Nothing to redo");
  }

  return (
    <header className={`mtk-editor-header${compact ? " is-compact" : ""}`} data-testid="editor-header">
      <a className="mtk-skip-link" href="#viewport">Skip to viewport</a>
      <div className="mtk-editor-header__start" role="toolbar" aria-label="Project and workspaces">
        <div className="mtk-editor-brand">
          <span className="mtk-editor-brand__mark" aria-hidden="true">M</span>
          <span className="mtk-editor-brand__name">metrocalk<span aria-hidden="true">.</span></span>
        </div>
        <FileMenu client={client} />
        {compact && onOpenLeftDock && (
          <Button data-testid="header-scene" variant="ghost" compact icon aria-label="Open Scene and Assets" onClick={onOpenLeftDock} title="Open Scene and Assets">
            <span aria-hidden="true">☰</span>
          </Button>
        )}
        {compact && onOpenRightDock && (
          <Button data-testid="header-inspector" variant="ghost" compact icon aria-label="Open Inspector" onClick={onOpenRightDock} title="Open Inspector">
            <span aria-hidden="true">⚙</span>
          </Button>
        )}
        {compact && <WorkspaceMenu compact onOpenCreate={onOpenCreate} onOpenLogic={onOpenLogic} onOpenReview={onOpenReview} />}
        <Button data-testid="header-undo" variant="ghost" compact icon aria-label="Undo" onClick={() => void undo()} title="Undo the last scene change (Ctrl/Cmd+Z)">
          <span aria-hidden="true">↶</span>
        </Button>
        <Button data-testid="header-redo" variant="ghost" compact icon aria-label="Redo" onClick={() => void redo()} title="Redo the last undone scene change (Ctrl/Cmd+Shift+Z)">
          <span aria-hidden="true">↷</span>
        </Button>
        {compact && (
          <Button
            data-testid="command-palette-trigger"
            variant="ghost"
            compact
            icon
            onClick={onOpenCommands}
            aria-haspopup="dialog"
            aria-label="Search editor commands"
            title="Search every editor command (Ctrl/Cmd+K)"
          >
            <span aria-hidden="true">⌕</span><span className="mtk-header-label"> Commands</span>
          </Button>
        )}
      </div>

      <div className="mtk-editor-header__transport" role="group" aria-label="Play controls">
        <PlayControls client={client} />
      </div>

      <div className="mtk-editor-header__end" role="toolbar" aria-label="Editor workspaces and commands">
        {!compact && <WorkspaceMenu onOpenCreate={onOpenCreate} onOpenLogic={onOpenLogic} onOpenReview={onOpenReview} />}
        {!compact && (
          <Button
            data-testid="command-palette-trigger"
            variant="secondary"
            compact
            onClick={onOpenCommands}
            aria-haspopup="dialog"
            title="Search every editor command"
            style={{ gap: space.sm }}
          >
            <span aria-hidden="true">⌕</span>
            <span className="mtk-command-trigger__label">Commands</span>
            <ShortcutBadge keys={["Ctrl", "K"]} ariaLabel="Control plus K" />
          </Button>
        )}
        <div className="mtk-editor-header__credits" role="group" aria-label="Optional AI credits">
          <Wallet client={client} compact />
        </div>
      </div>
    </header>
  );
}

export const editorHeaderMetrics = {
  height: 56,
  border: `1px solid ${color.border.subtle}`,
  font: font.ui,
  fontSize: fontSize.body,
} as const;
