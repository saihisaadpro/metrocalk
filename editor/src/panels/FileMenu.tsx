//! The **File menu** (M10.3 / ADR-033) — New / Open / Save / Save As / Recent, with an
//! **unsaved-changes guard**. The headline "save your work and reopen it", as the production shell's
//! menu (deliverable 3). Every action goes through the `EditorClient` project verbs, which wrap the
//! shell's `.mtk` save/open (atomic, versioned); the menu is UI chrome only (invariant 1).
//!
//! The unsaved-changes guard refreshes the **authoritative** dirty flag from the shell
//! (`projectState`) when the menu opens, so New / Open / a Recent-open ask before discarding unsaved
//! work; Save / Save As never lose data and aren't guarded. Stable ids (`#fileMenu`, `#fileNew`,
//! `#fileOpen`, `#fileSave`, `#fileSaveAs`, `.fileRecentItem`, `#unsavedGuard`) so the prompt-40
//! acceptance page-object keys off them.

import { useEffect, useRef, useState } from "react";
import { projectStore, projectName, useProjectInfo, type ProjectInfo } from "../store/project";
import { projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import { pushToast } from "../store/toasts";
import { DialogSurface, Modal, Popover, PopoverSurface } from "../theme/Popover";
import { Button } from "../theme/primitives";
import { PopupMenuGroup, PopupMenuItem } from "../theme/workspace";
import { color, font, fontSize, space } from "../theme/tokens";
import type { EditorClient } from "../transport/session";

/** The file's display name *with* its extension (so a save names the real file, not a stem). */
const fileName = (path: string | null): string | null => (path ? (path.split(/[\\/]/).pop() ?? path) : null);

export interface FileMenuProps {
  client: EditorClient;
  /** Open the export task dialog. The dialog is owned by `App` so the command palette can reach the
   *  same one surface — a second instance behind the menu would be a second answer to one question. */
  onExport: () => void;
}

export function FileMenu({ client, onExport }: FileMenuProps) {
  const { path, dirty, recents } = useProjectInfo();
  const [open, setOpen] = useState(false);
  // A guarded action awaiting "discard unsaved changes?" confirmation, or `null` when none is pending.
  const [pending, setPending] = useState<{ run: () => Promise<ProjectInfo>; label: string } | null>(null);
  // The trigger the dropdown anchors to (its on-screen rect drives the portaled panel's position).
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);

  // (Escape-to-dismiss for both the dropdown and the guard is handled by `Popover`/`Modal` — capture-phase +
  // stopPropagation, so pressing Esc to close a menu never also triggers App's Stop-Play.)

  // Refresh the authoritative project state (path · dirty · recents) whenever the menu opens, so the
  // guard reads the shell's truth, not just the optimistic indicator.
  useEffect(() => {
    if (!open) return;
    let live = true;
    client
      .projectState()
      .then((info) => {
        if (live) projectStore.getState().refresh(info);
      })
      .catch(() => {
        /* a failed read leaves the last-known state — never crash the chrome */
      });
    return () => {
      live = false;
    };
  }, [open, client]);

  /** Run a project op, mirror the result, surface success/error, and close the menus. A project SWITCH
   *  (New/Open) clears the selection so selection-bound modules (Inspector · Reveal · Wallet's AI-edit)
   *  don't keep pointing at an entity from the old scene — the new scene streams in over the Channel. */
  async function run(action: () => Promise<ProjectInfo>, ok: string, switchProject = false) {
    const info = await action();
    if (switchProject && !info.error) {
      projectStore.getState().switchProject(info);
      projectionStore.getState().select(null);
    } else {
      projectStore.getState().refresh(info);
    }
    setStatus(info.error ? info.error : ok);
    setOpen(false);
    setPending(null);
  }

  /** New / Open lose the current scene — guard on unsaved changes first; they switch projects. */
  function guarded(action: () => Promise<ProjectInfo>, label: string, ok: string) {
    if (dirty) {
      setPending({ run: () => action(), label });
    } else {
      void run(action, ok, true);
    }
  }

  /** Honest Save (C9): an UNTITLED project has no name, so Save → **Save As** (the dialog assigns one);
   *  afterward the menu title reflects the real filename and the status/toast names the file —
   *  never "saved" on an unnamed doc. `forceDialog` is the explicit Save As… item. */
  async function save(forceDialog: boolean) {
    const info = await (forceDialog || !path ? client.saveProjectAs() : client.saveProject());
    projectStore.getState().refresh(info);
    const name = fileName(info.path);
    if (info.error) {
      setStatus(info.error);
      pushToast(info.error, "error");
    } else if (!name) {
      // a CANCELLED Save-As dialog returns the unchanged (still-unnamed) state — an honest no-op, never
      // "saved" on an unnamed doc (C9). (The native dialog is the `.exe` path; the mock always names it.)
      setStatus("save cancelled");
    } else {
      setStatus(`Saved to ${name}`);
      pushToast(`Saved to ${name}`, "success");
    }
    setOpen(false);
  }

  /** M11.1 (ADR-040) — File→Import: open the native file dialog, import the chosen file through the MAGIC
   *  router (FBX/glTF/OBJ/PNG), select the placed entity. "Drop any file → a working asset." */
  async function importFile() {
    setOpen(false);
    const id = await client.importAssetDialog();
    if (id) {
      projectionStore.getState().select(id);
      projectStore.getState().markDirty();
      setStatus(`imported · ${id}`);
      pushToast("imported an asset", "success");
    } else {
      setStatus("import cancelled or unsupported");
    }
  }

  /** ADR-175 — keep the picture currently on the stage as a PNG.
   *
   *  Under Export and not under Project, because that is what it is: the scene leaving the engine in a
   *  form something else can read. It needs no selection and no cutscene — a still of an imported
   *  assembly is the most ordinary thing an engineer wants out of a viewer, and until this existed the
   *  answer was the operating system's screenshot key and a crop.
   *
   *  WHAT YOU SEE IS WHAT IT SAVES, grid and gizmo included. Nothing is posed and nothing is hidden,
   *  so the file is the frame — which the first `.exe` capture proved and this menu item's own first
   *  tooltip denied ("with none of the editor over it"). A cutscene RENDER is clean, because the
   *  cutscene camera puts the viewport into `ViewportVisibility::Cinematic`; a stage capture is not,
   *  and saying so is cheaper than a flicker of chrome disappearing under the author's cursor. */
  async function saveFrame() {
    setOpen(false);
    setStatus("Saving the viewport frame…");
    try {
      const result = await client.viewportCapture();
      setStatus(result.message);
      if (result.reason === null) pushToast(result.message, "success");
      else if (!/cancel/i.test(result.message)) pushToast(result.message, "error");
    } catch (cause) {
      const message = cause instanceof Error && cause.message ? cause.message : "Saving the frame failed";
      setStatus(message);
      pushToast(message, "error");
    }
  }

  return (
    <div id="fileMenuRoot" ref={rootRef} style={{ position: "relative", font: font.ui }}>
      <Button
        ref={triggerRef}
        id="fileMenu"
        data-testid="fileMenu"
        variant="ghost"
        compact
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? "fileMenuPopover" : undefined}
        onClick={() => setOpen((o) => !o)}
      >
        File
        <span className="mtk-file-menu__project" style={{ marginLeft: space.xs, color: color.text.muted }}>{projectName(path)}</span>
        {dirty && (
          <span id="projectDirty" data-testid="projectDirty" title="unsaved changes" style={{ marginLeft: space.xxs, color: color.token }}>
            •
          </span>
        )}
      </Button>

      {/* The dropdown is portaled to the body (theme/Popover) so the app header's `overflow: hidden` can never
          clip it and no sibling stacking context can bury it — the fix for "the File menu opens behind the
          suggestion bar / tabs". Edge-aware + Escape/outside-click dismissal are handled by Popover. */}
      <Popover
        open={open}
        anchor={rootRef}
        returnFocus={triggerRef}
        id="fileMenuPopover"
        ariaLabelledBy="fileMenu"
        onClose={() => setOpen(false)}
      >
        <PopoverSurface
          id="fileMenuPanel"
          data-testid="fileMenuPanel"
          className="mtk-popup-menu"
        >
          <PopupMenuGroup label="Project">
            <MenuItem id="fileNew" label="New project" onClick={() => guarded(() => client.newProject(), "New", "new project")} />
            <MenuItem id="fileOpen" label="Open…" onClick={() => guarded(() => client.openProject(), "Open", "opened")} />
            <MenuItem id="fileImport" label="Import asset…" onClick={() => void importFile()} />
            <MenuItem id="fileSave" label="Save" onClick={() => void save(false)} />
            <MenuItem id="fileSaveAs" label="Save As…" onClick={() => void save(true)} />
          </PopupMenuGroup>
          <PopupMenuGroup label="Export">
            {/* One item, not three. The format choice used to BE the menu, which meant the character of
                each writer — what it carries, what it drops — had nowhere to live but a `title`, and the
                write fired before the user had seen any of it. It is a decision with consequences, so it
                gets the surface a decision needs (ADR-174). */}
            <MenuItem
              id="fileExport"
              label="Export scene…"
              title="Choose a format, see what it carries and what this scene would lose, then write it"
              onClick={() => {
                setOpen(false);
                onExport();
              }}
            />
            <MenuItem id="fileSaveFrame" label="Viewport frame (PNG)…" title="Save the picture on the stage right now, exactly as you see it — including the grid and the gizmo, because nothing is posed and nothing is hidden. A cutscene render has neither." onClick={() => void saveFrame()} />
          </PopupMenuGroup>
          <PopupMenuGroup label={<span id="fileRecentHeading">Recent</span>}>
            <div id="fileRecent" data-testid="fileRecent" role="group" aria-labelledby="fileRecentHeading">
              {recents.length === 0 ? (
                <div style={{ padding: `${space.xs}px ${space.md}px`, color: color.text.muted, fontSize: fontSize.body }}>— none —</div>
              ) : (
                recents.map((p) => (
                  <PopupMenuItem
                    key={p}
                    className="fileRecentItem"
                    data-path={p}
                    label={projectName(p)}
                    onSelect={() => guarded(() => client.openProject(p), "Open", `opened ${projectName(p)}`)}
                    title={p}
                  />
                ))
              )}
            </div>
          </PopupMenuGroup>
        </PopoverSurface>
      </Popover>

      {pending && (
        <Modal
          open
          onClose={() => setPending(null)}
          id="unsavedGuard"
          ariaLabelledBy="unsavedGuardTitle"
        >
          <DialogSurface
            data-testid="unsavedGuard"
            style={{ maxWidth: 340 }}
          >
            <div id="unsavedGuardTitle" style={{ marginBottom: space.lg }}>
              Discard unsaved changes to <strong>{projectName(path)}</strong>?
            </div>
            <div style={{ display: "flex", gap: space.md, justifyContent: "flex-end" }}>
              <Button id="guardCancel" data-testid="guardCancel" variant="secondary" compact onClick={() => setPending(null)}>
                Cancel
              </Button>
              <Button
                id="guardDiscard"
                data-testid="guardDiscard"
                variant="danger"
                compact
                onClick={() => void run(pending.run, pending.label === "New" ? "new project" : "opened", true)}
              >
                Discard &amp; {pending.label}
              </Button>
            </div>
          </DialogSurface>
        </Modal>
      )}
    </div>
  );
}

function MenuItem({ id, label, title, onClick }: { id: string; label: string; title?: string; onClick: () => void }) {
  return (
    <PopupMenuItem
      id={id}
      data-testid={id}
      label={label}
      onSelect={onClick}
      title={title}
    />
  );
}
