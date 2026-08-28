//! **Export the scene** — the focused task dialog the three File-menu items used to be.
//!
//! What was there: `Export complete scene ▸ GLB… / USDA… / STEP AP242…`. Picking one fired the write
//! immediately, with the format's whole character compressed into a `title` tooltip, and the answer —
//! what was written, where, and what did not survive — arrived in the status bar and a toast that
//! overwrites itself. Three of the constitution's rules land on that at once: an action whose outcome
//! is offloaded to a passive gutter (`<ux_quality>` 1), feedback nowhere near the gesture (2), and a
//! cost that is neither legible before nor accounted for after (3).
//!
//! So the whole workflow is one dialog, and it holds all three moments:
//!
//! * **Before** — a rail of every format this build can WRITE, and for the chosen one: its declared
//!   fidelity, the honest note from the registry, and a checklist of the nine capabilities it carries.
//!   Plus the sentence that is the actual point — *what is in THIS scene that this format will not
//!   write* — because a fidelity tier is an abstraction and "3 cameras will not be written" is not.
//! * **The gesture** — one primary action, naming the project it will write.
//! * **After** — the exporter's own fidelity ledger, in the same place, replacing the options rather
//!   than flashing past them. The path is on screen, in mono, and the dialog stays open until the
//!   person is done reading it.
//!
//! **The rail is the catalogue, not a list of three.** Membership is `writesScenes` over
//! `format_catalog()`, and the argument each row sends is derived from the format's canonical
//! extension (see `formatVocabulary.exportArgFor`). A fourth writer therefore appears here with no
//! edit to this file — and if its canonical extension is one `scene_export` would refuse, the row
//! says so instead of failing at the click.

import { useEffect, useMemo, useRef, useState } from "react";
import { useStore } from "zustand";
import { projectName, useProjectInfo } from "../store/project";
import { projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import { pushToast } from "../store/toasts";
import { DialogSurface, Modal } from "../theme/Popover";
import { Icon } from "../theme/icons";
import { Badge, Button } from "../theme/primitives";
import { EmptyPanelState, NavRail } from "../theme/workspace";
import type { EntityProjection, FormatSpec, SceneExportFormat, SceneExportResponse } from "../transport/protocol";
import type { EditorClient } from "../transport/session";
import { CARRIES, FIDELITY_COPY, exportArgAccepted, exportArgFor, writesScenes } from "./formatVocabulary";

/** The mark each writable domain gets in the rail. A domain with no entry falls back to the generic
 *  export mark — a new domain is then unremarkable rather than invisible. */
const DOMAIN_ICON: Record<string, string> = {
  "Real-time": "model",
  CAD: "measure",
  Simulation: "physics",
  Textures: "assets",
};

/**
 * What this scene holds that a format will not write, counted from the projection.
 *
 * **This deliberately does not attempt all nine capabilities.** Three of them are legible from the
 * scene outline the editor already holds — a component name is a fact — and the rest (materials,
 * textures, skinning) are properties of assets the outline does not carry. A probe that guessed at
 * those would produce the one output worse than silence here: "nothing will be lost", confidently,
 * about a scene that is about to lose its textures. So the sentence reports only what it can see and
 * the copy beside it says exactly that, and the exporter's own ledger — which does see everything —
 * is what fills the gap, in this same dialog, one click later.
 *
 * The component names are the core's registered vocabulary (`core/src/stdlib.rs`), which is why both
 * spellings of the rigid-body component appear: the registry declares both.
 */
export const OMISSION_PROBES: readonly { key: keyof FormatSpec["carries"]; components: readonly string[]; noun: [one: string, many: string] }[] = [
  { key: "cameras", components: ["Camera"], noun: ["camera", "cameras"] },
  { key: "animation", components: ["Animation", "Animator", "Cutscene"], noun: ["animated object", "animated objects"] },
  { key: "physics", components: ["RigidBody", "Rigidbody", "Collider", "Joint"], noun: ["physics body", "physics bodies"] },
];

export interface Omission {
  key: keyof FormatSpec["carries"];
  count: number;
  /** Already pluralised against `count` — the caller joins, it does not decide. */
  text: string;
}

/** Count, per probe, the entities carrying something `carries` says this format cannot write. */
export function sceneOmissions(
  entities: readonly EntityProjection[],
  carries: FormatSpec["carries"],
): Omission[] {
  const out: Omission[] = [];
  for (const probe of OMISSION_PROBES) {
    if (carries[probe.key]) continue;
    const count = entities.filter((e) => probe.components.some((c) => e.components[c] != null)).length;
    if (count === 0) continue;
    out.push({ key: probe.key, count, text: `${count} ${probe.noun[count === 1 ? 0 : 1]}` });
  }
  return out;
}

const FIDELITY_TONE: Record<string, "success" | "accent" | "warn"> = {
  full: "success",
  subset: "accent",
  seam: "warn",
};

const STATUS_TONE: Record<string, "success" | "accent" | "warn"> = {
  preserved: "success",
  converted: "accent",
  omitted: "warn",
};

export interface ExportDialogProps {
  open: boolean;
  onClose: () => void;
  client: EditorClient;
}

export function ExportDialog({ open, onClose, client }: ExportDialogProps) {
  const { path } = useProjectInfo();
  // Two narrow selectors rather than the whole state: the dialog re-renders when the scene's
  // membership changes, not on every field edit inside it.
  const base = useStore(projectionStore, (s) => s.base);
  const deactivated = useStore(projectionStore, (s) => s.deactivated);
  const [catalog, setCatalog] = useState<FormatSpec[] | null>(null);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<SceneExportResponse | null>(null);
  const confirmRef = useRef<HTMLButtonElement>(null);

  // Read the catalogue when the dialog opens, not at mount: it is a build fact, but a dialog that is
  // never opened should not have cost an IPC round trip to say so.
  useEffect(() => {
    if (!open) return;
    let live = true;
    client
      .formatCatalog()
      .then((specs) => live && setCatalog(specs))
      .catch(() => live && setCatalog([]));
    return () => {
      live = false;
    };
  }, [open, client]);

  // A re-open is a fresh question. Leaving the previous run's ledger on screen would answer it with a
  // report about a scene that has since been edited — the stale-status-line bug, one surface over.
  useEffect(() => {
    if (!open) setResult(null);
  }, [open]);

  const writable = useMemo(() => (catalog ?? []).filter(writesScenes), [catalog]);
  const active = writable.find((f) => f.id === activeId) ?? writable[0] ?? null;

  const entities = useMemo(
    () => Object.values(base).filter((e) => deactivated[e.id] == null),
    [base, deactivated],
  );
  const omissions = useMemo(
    () => (active ? sceneOmissions(entities, active.carries) : []),
    [entities, active],
  );

  const arg = active ? exportArgFor(active) : "";
  const addressable = arg !== "" && exportArgAccepted(arg);
  const subject = projectName(path);

  async function runExport() {
    if (!active || !addressable) return;
    setBusy(true);
    setResult(null);
    try {
      const reply = await client.sceneExport(arg as SceneExportFormat);
      setResult(reply);
      setStatus(reply.message);
      // The toast is a NOTIFICATION, not the answer — the answer is the ledger below, which is why
      // this one carries no fidelity arithmetic. A person who dismissed the toast has lost nothing.
      if (reply.ok) pushToast(reply.message, "success");
      else if (!/cancel/i.test(reply.message)) pushToast(reply.message, "error");
    } catch (cause) {
      const message = cause instanceof Error && cause.message ? cause.message : "The export could not be completed.";
      setResult({
        ok: false,
        message,
        format: arg,
        exportedPath: null,
        nodes: 0,
        meshes: 0,
        skins: 0,
        animations: 0,
        fidelity: [],
      });
      setStatus(message);
      pushToast(message, "error");
    } finally {
      setBusy(false);
    }
  }

  if (!open) return null;

  return (
    <Modal open onClose={onClose} id="exportDialog" ariaLabelledBy="exportDialogTitle" initialFocusRef={confirmRef}>
      <DialogSurface flush data-testid="exportDialog" className="mtk-export" style={{ ["--mtk-dialog-width" as string]: "820px" }}>
        <div className="mtk-export__rail">
          <h2 id="exportDialogTitle" className="mtk-export__title">Export scene</h2>
          {writable.length > 0 && (
            <NavRail
              id="exportFormats"
              label="Export formats"
              panelIdPrefix="exportPane-"
              activeId={active?.id ?? ""}
              onChange={(id) => {
                setActiveId(id);
                setResult(null);
              }}
              items={writable.map((f) => ({
                id: f.id,
                label: f.label,
                icon: <Icon name={DOMAIN_ICON[f.domain] ?? "export"} size="md" />,
                tooltip: `${f.domain} · writes .${exportArgFor(f)}`,
              }))}
            />
          )}
          <div className="mtk-export__subject" data-testid="exportSubject">
            <span className="mtk-export__subject-name" title={path ?? undefined}>{subject}</span>
            <span className="mtk-export__subject-count">
              {entities.length === 1 ? "1 object in this scene" : `${entities.length} objects in this scene`}
            </span>
          </div>
        </div>

        <div className="mtk-export__pane" id={active ? `exportPane-${active.id}` : undefined} role={active ? "tabpanel" : undefined}>
          {catalog == null ? (
            <div className="mtk-export__body" data-testid="exportLoading">
              <p className="mtk-export__note">Reading what this build can write…</p>
            </div>
          ) : active == null ? (
            <div className="mtk-export__body">
              <EmptyPanelState
                data-testid="exportEmpty"
                icon={<Icon name="export" size="xl" />}
                title="This build declares no export format"
                description="The format registry is compiled in, so an empty list is a build problem rather than an empty project. Nothing here can be written until a writer is present."
              />
            </div>
          ) : (
            <div className="mtk-export__body" data-testid={`exportPane-${active.id}`}>
              <header className="mtk-export__head">
                <h3 className="mtk-export__format">{active.label}</h3>
                <Badge tone={FIDELITY_TONE[active.fidelity] ?? "neutral"} title={FIDELITY_COPY[active.fidelity]?.hint}>
                  {FIDELITY_COPY[active.fidelity]?.label ?? active.fidelity}
                </Badge>
                <span className="mtk-export__ext">.{arg}</span>
              </header>
              <p className="mtk-export__note">{active.note}</p>

              {result == null ? (
                <>
                  <section className="mtk-export__block">
                  <h4 className="mtk-export__legend" id="exportCarriesLegend">What this format writes</h4>
                  <ul className="mtk-export__carries" aria-labelledby="exportCarriesLegend" data-testid="exportCarries">
                    {CARRIES.map((c) => {
                      const carried = active.carries[c.key];
                      return (
                        <li
                          key={c.key}
                          className="mtk-export__carry"
                          data-carried={carried}
                          data-testid={`exportCarry-${c.key}`}
                        >
                          {/* The mark carries the meaning and NAMES it: `Icon`'s `title` renders an
                              SVG `<title>`, which is the accessible name and is in the text of the
                              page, with no box of its own. A screen-reader-only span here would be a
                              1px element holding a clipped sentence — real to a capture gate, and
                              the wrong shape for a mark that is already the semantics. */}
                          <Icon name={carried ? "check" : "minus"} size="sm" title={carried ? "written" : "not written"} />
                          <span>{c.title}</span>
                        </li>
                      );
                    })}
                  </ul>
                  </section>

                  {omissions.length > 0 && (
                    <div className="mtk-export__cost" data-testid="exportCost">
                      <Icon name="warning" size="sm" />
                      <div>
                        <strong>
                          Not written from this scene: {omissions.map((o) => o.text).join(" · ")}.
                        </strong>
                        <span>
                          {" "}
                          Counted from the scene outline. Material, texture and geometry detail is
                          reported by the exporter itself, below, once it runs.
                        </span>
                      </div>
                    </div>
                  )}
                </>
              ) : (
                <ExportLedger result={result} />
              )}
            </div>
          )}

          <footer className="mtk-export__foot">
            {result?.ok ? (
              <>
                <Button data-testid="exportAgain" variant="secondary" onClick={() => setResult(null)}>
                  Export again
                </Button>
                <Button data-testid="exportDone" variant="primary" size="comfortable" onClick={onClose} style={{ flex: 1 }}>
                  Done
                </Button>
              </>
            ) : (
              <>
                <Button data-testid="exportCancel" variant="ghost" onClick={onClose}>
                  Cancel
                </Button>
                <Button
                  ref={confirmRef}
                  data-testid="exportConfirm"
                  variant="primary"
                  size="comfortable"
                  style={{ flex: 1 }}
                  disabled={active == null || !addressable || busy}
                  disabledReason={
                    active == null
                      ? "This build declares no format it can write."
                      : !addressable
                        ? `This build offers ${active.label} but cannot address it: the scene exporter does not accept ".${arg}".`
                        : busy
                          ? "The export is running."
                          : undefined
                  }
                  onClick={() => void runExport()}
                >
                  {busy ? "Exporting…" : `Export ${subject}`}
                </Button>
              </>
            )}
          </footer>
        </div>
      </DialogSurface>
    </Modal>
  );
}

/** The exporter's own account of the run — counts, destination, and every fidelity decision it made. */
function ExportLedger({ result }: { result: SceneExportResponse }) {
  if (!result.ok) {
    return (
      <div className="mtk-export__failure" data-testid="exportFailure" role="status">
        <Icon name="warning" size="md" />
        <p>{result.message}</p>
      </div>
    );
  }
  const counts: [string, number][] = [
    ["objects", result.nodes],
    ["meshes", result.meshes],
    ["skins", result.skins],
    ["animations", result.animations],
  ];
  return (
    <div className="mtk-export__report" data-testid="exportResult" role="status">
      <section className="mtk-export__block">
        <h4 className="mtk-export__legend">What was written</h4>
        <ul className="mtk-export__counts">
          {counts.map(([label, value]) => (
            <li key={label} data-testid={`exportCount-${label}`}>
              <strong>{value}</strong>
              <span>{label}</span>
            </li>
          ))}
        </ul>
        {result.exportedPath != null && (
          <p className="mtk-export__path" data-testid="exportPath" title={result.exportedPath}>
            {result.exportedPath}
          </p>
        )}
      </section>
      <section className="mtk-export__block">
      <h4 className="mtk-export__legend">What it cost</h4>
      {result.fidelity.length === 0 ? (
        <p className="mtk-export__note" data-testid="exportNoFidelity">
          The exporter reported nothing changed or omitted.
        </p>
      ) : (
        <ul className="mtk-export__ledger" data-testid="exportFidelity">
          {result.fidelity.map((entry, index) => (
            <li key={`${entry.feature}-${index}`} data-status={entry.status}>
              <Badge tone={STATUS_TONE[entry.status] ?? "neutral"}>{entry.status}</Badge>
              <div>
                <strong>
                  {entry.feature}
                  {entry.count > 0 ? ` · ${entry.count}` : ""}
                </strong>
                <span>{entry.detail}</span>
              </div>
            </li>
          ))}
        </ul>
      )}
      </section>
    </div>
  );
}

export default ExportDialog;
