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

import { useEffect, useState } from "react";
import { projectStore, projectName, useProjectInfo, type ProjectInfo } from "../store/project";
import { projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import { pushToast } from "../store/toasts";
import type { EditorClient } from "../transport/session";

/** The file's display name *with* its extension (so a save names the real file, not a stem). */
const fileName = (path: string | null): string | null => (path ? (path.split(/[\\/]/).pop() ?? path) : null);

export function FileMenu({ client }: { client: EditorClient }) {
  const { path, dirty, recents } = useProjectInfo();
  const [open, setOpen] = useState(false);
  // A guarded action awaiting "discard unsaved changes?" confirmation, or `null` when none is pending.
  const [pending, setPending] = useState<{ run: () => Promise<ProjectInfo>; label: string } | null>(null);

  // Escape closes the unsaved-guard (cancel) first, else the open dropdown. Capture-phase + stopPropagation
  // so it runs BEFORE App's window Esc handler — pressing Esc to dismiss a menu must not also Stop Play.
  useEffect(() => {
    if (!open && !pending) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.stopPropagation();
      if (pending) setPending(null);
      else setOpen(false);
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, pending]);

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
    <div id="fileMenuRoot" style={{ position: "relative", font: "12px ui-monospace, monospace" }}>
      <button
        id="fileMenu"
        data-testid="fileMenu"
        onClick={() => setOpen((o) => !o)}
        style={{ padding: "3px 10px", background: "#1b1e26", color: "#e8e8e8", border: "1px solid #2a2d35", borderRadius: 4, cursor: "pointer" }}
      >
        File
        <span style={{ marginLeft: 8, opacity: 0.65 }}>{projectName(path)}</span>
        {dirty && (
          <span id="projectDirty" data-testid="projectDirty" title="unsaved changes" style={{ marginLeft: 4, color: "#fbbf24" }}>
            •
          </span>
        )}
      </button>

      {open && (
        <>
          {/* click-away closes the menu */}
          <div onClick={() => setOpen(false)} style={{ position: "fixed", inset: 0, zIndex: 90 }} />
          <div
            id="fileMenuPanel"
            data-testid="fileMenuPanel"
            style={{ position: "absolute", top: "100%", left: 0, marginTop: 4, minWidth: 220, zIndex: 100, background: "#14161c", border: "1px solid #2a2d35", borderRadius: 6, padding: 4, boxShadow: "0 8px 24px #0008" }}
          >
            <MenuItem id="fileNew" label="New project" onClick={() => guarded(() => client.newProject(), "New", "new project")} />
            <MenuItem id="fileOpen" label="Open…" onClick={() => guarded(() => client.openProject(), "Open", "opened")} />
            <MenuItem id="fileImport" label="Import asset…" onClick={() => void importFile()} />
            <Divider />
            <MenuItem id="fileSave" label="Save" onClick={() => void save(false)} />
            <MenuItem id="fileSaveAs" label="Save As…" onClick={() => void save(true)} />
            <Divider />
            <div style={{ padding: "4px 8px 2px", opacity: 0.5, fontSize: 11 }}>Recent</div>
            <div id="fileRecent" data-testid="fileRecent">
              {recents.length === 0 ? (
                <div style={{ padding: "4px 8px", opacity: 0.4 }}>— none —</div>
              ) : (
                recents.map((p) => (
                  <button
                    key={p}
                    className="fileRecentItem"
                    data-path={p}
                    onClick={() => guarded(() => client.openProject(p), "Open", `opened ${projectName(p)}`)}
                    title={p}
                    style={{ display: "block", width: "100%", textAlign: "left", padding: "4px 8px", background: "transparent", color: "#cdd", border: "none", cursor: "pointer", fontSize: 12, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}
                  >
                    {projectName(p)}
                  </button>
                ))
              )}
            </div>
          </div>
        </>
      )}

      {pending && (
        <div
          id="unsavedGuard"
          data-testid="unsavedGuard"
          onClick={() => setPending(null)}
          style={{ position: "fixed", inset: 0, zIndex: 200, display: "flex", alignItems: "center", justifyContent: "center", background: "#0008" }}
        >
          <div
            onClick={(e) => e.stopPropagation()} // clicks inside the dialog must not cancel
            style={{ background: "#14161c", border: "1px solid #3a3d45", borderRadius: 8, padding: 18, maxWidth: 340 }}
          >
            <div style={{ marginBottom: 12 }}>
              Discard unsaved changes to <strong>{projectName(path)}</strong>?
            </div>
            <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
              <button
                id="guardCancel"
                data-testid="guardCancel"
                onClick={() => setPending(null)}
                style={{ padding: "4px 12px", background: "#1b1e26", color: "#e8e8e8", border: "1px solid #2a2d35", borderRadius: 4, cursor: "pointer" }}
              >
                Cancel
              </button>
              <button
                id="guardDiscard"
                data-testid="guardDiscard"
                onClick={() => void run(pending.run, pending.label === "New" ? "new project" : "opened", true)}
                style={{ padding: "4px 12px", background: "#5a2f1f", color: "#fcd", border: "1px solid #6a3f2f", borderRadius: 4, cursor: "pointer" }}
              >
                Discard &amp; {pending.label}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

function MenuItem({ id, label, onClick }: { id: string; label: string; onClick: () => void }) {
  return (
    <button
      id={id}
      data-testid={id}
      onClick={onClick}
      style={{ display: "block", width: "100%", textAlign: "left", padding: "5px 8px", background: "transparent", color: "#e8e8e8", border: "none", cursor: "pointer", fontSize: 12, borderRadius: 4 }}
    >
      {label}
    </button>
  );
}

function Divider() {
  return <div style={{ height: 1, background: "#2a2d35", margin: "4px 0" }} />;
}
