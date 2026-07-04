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
import { Modal, Popover } from "../theme/Popover";
import { Button } from "../theme/primitives";
import { color, elevation, font, fontSize, radius, space } from "../theme/tokens";
import type { EditorClient } from "../transport/session";

/** The file's display name *with* its extension (so a save names the real file, not a stem). */
const fileName = (path: string | null): string | null => (path ? (path.split(/[\\/]/).pop() ?? path) : null);

export function FileMenu({ client }: { client: EditorClient }) {
  const { path, dirty, recents } = useProjectInfo();
  const [open, setOpen] = useState(false);
  // A guarded action awaiting "discard unsaved changes?" confirmation, or `null` when none is pending.
  const [pending, setPending] = useState<{ run: () => Promise<ProjectInfo>; label: string } | null>(null);
  // The trigger the dropdown anchors to (its on-screen rect drives the portaled panel's position).
  const rootRef = useRef<HTMLDivElement>(null);

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
    projectStore.getState().refresh(info);
    if (switchProject && !info.error) projectionStore.getState().select(null);
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

  return (
    <div id="fileMenuRoot" ref={rootRef} style={{ position: "relative", font: font.ui }}>
      <Button id="fileMenu" data-testid="fileMenu" variant="ghost" compact onClick={() => setOpen((o) => !o)}>
        File
        <span style={{ marginLeft: space.xs, color: color.text.muted }}>{projectName(path)}</span>
        {dirty && (
          <span id="projectDirty" data-testid="projectDirty" title="unsaved changes" style={{ marginLeft: space.xxs, color: color.token }}>
            •
          </span>
        )}
      </Button>

      {/* The dropdown is portaled to the body (theme/Popover) so the app header's `overflow: hidden` can never
          clip it and no sibling stacking context can bury it — the fix for "the File menu opens behind the
          suggestion bar / tabs". Edge-aware + Escape/outside-click dismissal are handled by Popover. */}
      <Popover open={open} anchor={rootRef} onClose={() => setOpen(false)}>
        <div
          id="fileMenuPanel"
          data-testid="fileMenuPanel"
          style={{ minWidth: 220, background: color.bg.raised, border: `1px solid ${color.border.default}`, borderRadius: radius.lg, padding: space.xs, boxShadow: elevation.e3 }}
        >
          <MenuItem id="fileNew" label="New project" onClick={() => guarded(() => client.newProject(), "New", "new project")} />
          <MenuItem id="fileOpen" label="Open…" onClick={() => guarded(() => client.openProject(), "Open", "opened")} />
          <MenuItem id="fileImport" label="Import asset…" onClick={() => void importFile()} />
          <Divider />
          <MenuItem id="fileSave" label="Save" onClick={() => void save(false)} />
          <MenuItem id="fileSaveAs" label="Save As…" onClick={() => void save(true)} />
          <Divider />
          <div style={{ color: color.text.muted, fontSize: fontSize.meta, padding: `${space.xs}px ${space.md}px 2px` }}>Recent</div>
          <div id="fileRecent" data-testid="fileRecent">
            {recents.length === 0 ? (
              <div style={{ padding: `${space.xs}px ${space.md}px`, color: color.text.faint, fontSize: fontSize.body }}>— none —</div>
            ) : (
              recents.map((p) => (
                <button
                  key={p}
                  className="fileRecentItem mtk-btn mtk-btn--ghost"
                  data-path={p}
                  onClick={() => guarded(() => client.openProject(p), "Open", `opened ${projectName(p)}`)}
                  title={p}
                  style={{ display: "block", width: "100%", textAlign: "left", padding: `${space.xs}px ${space.md}px`, color: color.text.secondary, fontSize: fontSize.body, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}
                >
                  {projectName(p)}
                </button>
              ))
            )}
          </div>
        </div>
      </Popover>

      {pending && (
        <Modal open onClose={() => setPending(null)} id="unsavedGuard">
          <div
            data-testid="unsavedGuard"
            style={{ background: color.bg.raised, border: `1px solid ${color.border.strong}`, borderRadius: radius.xl, padding: space.xl, maxWidth: 340, boxShadow: elevation.e3, color: color.text.primary }}
          >
            <div style={{ marginBottom: space.lg }}>
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
          </div>
        </Modal>
      )}
    </div>
  );
}

function MenuItem({ id, label, onClick }: { id: string; label: string; onClick: () => void }) {
  return (
    <button
      id={id}
      data-testid={id}
      className="mtk-btn mtk-btn--ghost"
      onClick={onClick}
      style={{ display: "block", width: "100%", textAlign: "left", padding: `${space.sm}px ${space.md}px`, color: color.text.primary, fontSize: fontSize.body }}
    >
      {label}
    </button>
  );
}

function Divider() {
  return <div style={{ height: 1, background: color.border.subtle, margin: `${space.xs}px 0` }} />;
}
