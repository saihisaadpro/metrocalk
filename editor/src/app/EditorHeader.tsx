//! Production editor header: project identity and file operations on the left, play transport in the
//! centre, undo/redo and the command palette on the right.
//!
//! It deliberately does NOT list workspaces. It used to carry a "Workspaces" menu offering three of the
//! nine sub-engines under different names — a second, disagreeing index. The Engines rail is the one index
//! now, and the header's job is the project and the transport.

import { FileMenu } from "../panels/FileMenu";
import { PlayControls } from "../panels/PlayControls";
import { Wallet } from "../panels/Wallet";
import { Icon } from "../theme/icons";
import { Button } from "../theme/primitives";
import { ShortcutBadge } from "../theme/workspace";
import { color, font, fontSize, space } from "../theme/tokens";
import { setStatus } from "../store/ui";
import type { EditorClient } from "../transport/session";

export interface EditorHeaderProps {
  client: EditorClient;
  onOpenCommands: () => void;
  onOpenLeftDock?: () => void;
  onOpenRightDock?: () => void;
  compact?: boolean;
}


export function EditorHeader({
  client,
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
            <Icon name="menu" size="md" />
          </Button>
        )}
        {compact && onOpenRightDock && (
          <Button data-testid="header-inspector" variant="ghost" compact icon aria-label="Open Inspector" onClick={onOpenRightDock} title="Open Inspector">
            <Icon name="properties" size="md" />
          </Button>
        )}
        <Button data-testid="header-undo" variant="ghost" compact icon aria-label="Undo" onClick={() => void undo()} title="Undo the last scene change (Ctrl/Cmd+Z)">
          <Icon name="undo" size="md" />
        </Button>
        <Button data-testid="header-redo" variant="ghost" compact icon aria-label="Redo" onClick={() => void redo()} title="Redo the last undone scene change (Ctrl/Cmd+Shift+Z)">
          <Icon name="redo" size="md" />
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
            <Icon name="search" size="md" /><span className="mtk-header-label">Commands</span>
          </Button>
        )}
      </div>

      <div className="mtk-editor-header__transport" role="group" aria-label="Play controls">
        <PlayControls client={client} />
      </div>

      <div className="mtk-editor-header__end" role="toolbar" aria-label="Editor workspaces and commands">
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
            <Icon name="search" size="md" />
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
