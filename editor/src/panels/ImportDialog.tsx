//! **Import into the scene** — the read side of the task dialog ADR-174 built for the write side.
//!
//! What was there: `File ▸ Import asset…`, one menu item straight into a native file dialog. Nothing
//! said which files this build can open until the OS dropdown was already up; nothing said what a
//! STEP assembly loses on the way in; and afterwards the answer was `imported · e-42` in a status bar
//! that overwrites itself, with the real account — the per-part fidelity breakdown — in a panel in a
//! different dock that the person had no reason to know about. That is the same before/gesture/after
//! split the export dialog closed, mirrored.
//!
//! So the whole workflow is one dialog, and it holds all three moments:
//!
//! * **Before** — a rail of every format this build can READ (ten, against export's four), and for
//!   the chosen one: its declared fidelity, the registry's honest note, the extensions it accepts,
//!   the nine-capability checklist, and the sentence that is the actual point — *what this reader
//!   will not bring in*, stated before a file is chosen rather than discovered afterwards.
//! * **The gesture** — one primary action, which opens the native dialog **filtered to the chosen
//!   format**. The rail is not decoration: picking STEP AP242 means the file picker shows STEP files.
//! * **After** — the importer's own account, in the same place: which outcome it was in its own
//!   words, and when the scene holds CAD, the per-part honesty breakdown that until now lived only in
//!   the bottom dock.
//!
//! **The rail is the catalogue, not a list.** Membership is `readsScenes` over `format_catalog()`,
//! and the extensions each row sends are the format's own, so an eleventh reader appears here with no
//! edit to this file. `Any supported file` leads it and is the default, because the ordinary case is
//! "I have a file" and not "I have decided on a format" — the dialog must not make the common path
//! the longer one.

import { useEffect, useMemo, useRef, useState } from "react";
import { useStore } from "zustand";
import { projectName, projectStore, useProjectInfo } from "../store/project";
import { projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import { pushToast } from "../store/toasts";
import { DialogSurface, Modal } from "../theme/Popover";
import { Icon } from "../theme/icons";
import { Badge, Button, DialogTitle, SectionHeader } from "../theme/primitives";
import { EmptyPanelState, NavRail } from "../theme/workspace";
import type { CadReport, FormatSpec, ImportDialogResponse } from "../transport/protocol";
import type { EditorClient } from "../transport/session";
import { CARRIES, FIDELITY_COPY, extensionList, readableExtensions, readsScenes } from "./formatVocabulary";

/** The mark each readable domain gets in the rail — the export dialog's table, which is the point of
 *  having one: the same format wears the same mark on both sides of the engine. */
const DOMAIN_ICON: Record<string, string> = {
  "Real-time": "model",
  CAD: "measure",
  Simulation: "physics",
  Textures: "assets",
};

const FIDELITY_TONE: Record<string, "success" | "accent" | "warn"> = {
  full: "success",
  subset: "accent",
  seam: "warn",
};

/** The rail's first row, and the state the dialog opens in. Not a format — the absence of a filter. */
export const ANY_FORMAT = "any";

/** The per-part honesty classes, in the order the CAD report itself counts them. The labels are
 *  `ImportReport`'s, deliberately: two surfaces describing one breakdown in two vocabularies is the
 *  twin-panel drift this repository has already paid for. */
const FIDELITY_ROWS: readonly { key: keyof CadReport; label: string; tone: "success" | "accent" | "warn" }[] = [
  { key: "exactBrep", label: "Exact B-rep", tone: "success" },
  { key: "tessellationOnly", label: "Tessellation-only", tone: "accent" },
  { key: "aiReconstructed", label: "AI-reconstructed", tone: "accent" },
  { key: "proxy", label: "Unresolved", tone: "warn" },
  { key: "accessDenied", label: "Access-denied", tone: "warn" },
  { key: "failed", label: "Failed", tone: "warn" },
];

/**
 * What a reader will NOT bring in, from the capabilities it declares it does not carry.
 *
 * The export side counts this from the scene, because the scene is on hand and the file is not. Here
 * it is the mirror image and the honesty is the same shape: this is what the FORMAT declares, before
 * anything has been opened, so it can name the capability and never a count. The copy beside it says
 * exactly that, and the importer's own report — which has seen the file — fills the gap afterwards,
 * in this same dialog.
 */
export function missingCapabilities(carries: FormatSpec["carries"]): string[] {
  return CARRIES.filter((c) => !carries[c.key]).map((c) => c.label);
}

/** "geometry, hierarchy and materials" — an Oxford-free list a sentence can end on. */
export function joinWords(words: readonly string[]): string {
  if (words.length <= 1) return words[0] ?? "";
  return `${words.slice(0, -1).join(", ")} and ${words[words.length - 1]}`;
}

export interface ImportDialogProps {
  open: boolean;
  onClose: () => void;
  client: EditorClient;
}

export function ImportDialog({ open, onClose, client }: ImportDialogProps) {
  const { path } = useProjectInfo();
  const base = useStore(projectionStore, (s) => s.base);
  const deactivated = useStore(projectionStore, (s) => s.deactivated);
  const [catalog, setCatalog] = useState<FormatSpec[] | null>(null);
  const [activeId, setActiveId] = useState<string>(ANY_FORMAT);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<ImportDialogResponse | null>(null);
  const [report, setReport] = useState<CadReport | null>(null);
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

  // A re-open is a fresh question. Leaving the previous run's account on screen would answer it with
  // a report about an import that has already been undone — the stale-status-line bug, one surface
  // over. The FORMAT survives, because "the last thing I imported" is a good guess at the next one.
  useEffect(() => {
    if (!open) {
      setResult(null);
      setReport(null);
    }
  }, [open]);

  const readable = useMemo(() => (catalog ?? []).filter(readsScenes), [catalog]);
  const active = readable.find((f) => f.id === activeId) ?? null;
  const everything = useMemo(() => readableExtensions(catalog ?? []), [catalog]);

  const entities = useMemo(
    () => Object.values(base).filter((e) => deactivated[e.id] == null),
    [base, deactivated],
  );
  const missing = useMemo(() => (active ? missingCapabilities(active.carries) : []), [active]);
  const subject = projectName(path);

  async function runImport() {
    setBusy(true);
    setResult(null);
    setReport(null);
    try {
      // `undefined` — not an empty array — is "every readable extension": the shell reads an empty
      // filter as "this caller asked for nothing I have" and widens it anyway, but saying it here
      // costs nothing and keeps the two ends agreeing about which value means "unfiltered".
      const reply = await client.importAssetDialog(active ? active.extensions : undefined);
      setResult(reply);
      setStatus(reply.message);
      if (reply.outcome === "imported" && reply.entityId) {
        projectionStore.getState().select(reply.entityId);
        projectStore.getState().markDirty();
        pushToast(reply.message, "success");
        // The per-part breakdown, in the dialog that caused it. `cadReport` answers 0 for a scene
        // with no CAD, and the block below simply does not render then — an OBJ has no fidelity
        // classes and a table of six zeroes would be noise dressed as an account.
        await client
          .cadReport()
          .then(setReport)
          .catch(() => setReport(null));
      } else if (reply.outcome === "failed") {
        pushToast(reply.message, "error");
      }
    } catch (cause) {
      const message = cause instanceof Error && cause.message ? cause.message : "The import could not be started.";
      setResult({ entityId: null, outcome: "failed", message });
      setStatus(message);
      pushToast(message, "error");
    } finally {
      setBusy(false);
    }
  }

  if (!open) return null;

  const railItems = [
    {
      id: ANY_FORMAT,
      label: "Any supported file",
      icon: <Icon name="import" size="md" />,
      tooltip: `Every format this build can read — ${everything.length} extensions`,
    },
    ...readable.map((f) => ({
      id: f.id,
      label: f.label,
      icon: <Icon name={DOMAIN_ICON[f.domain] ?? "import"} size="md" />,
      tooltip: `${f.domain} · reads ${extensionList(f).join(" · ")}`,
    })),
  ];

  return (
    <Modal open onClose={onClose} id="importDialog" ariaLabelledBy="importDialogTitle" initialFocusRef={confirmRef}>
      <DialogSurface flush data-testid="importDialog" className="mtk-taskdialog" style={{ ["--mtk-dialog-width" as string]: "820px" }}>
        <div className="mtk-taskdialog__rail">
          <DialogTitle id="importDialogTitle" className="mtk-taskdialog__title">Import a file</DialogTitle>
          {catalog != null && (
            <NavRail
              id="importFormats"
              label="Import formats"
              panelIdPrefix="importPane-"
              activeId={activeId}
              onChange={(id) => {
                setActiveId(id);
                setResult(null);
                setReport(null);
              }}
              items={railItems}
            />
          )}
          <div className="mtk-taskdialog__subject" data-testid="importSubject">
            <span className="mtk-taskdialog__subject-name" title={path ?? undefined}>{subject}</span>
            <span className="mtk-taskdialog__subject-count">
              {entities.length === 1 ? "1 object in this scene" : `${entities.length} objects in this scene`}
            </span>
          </div>
        </div>

        <div className="mtk-taskdialog__pane" id={`importPane-${activeId}`} role="tabpanel">
          {catalog == null ? (
            <div className="mtk-taskdialog__body" data-testid="importLoading">
              <p className="mtk-taskdialog__note">Reading what this build can open…</p>
            </div>
          ) : readable.length === 0 ? (
            <div className="mtk-taskdialog__body">
              <EmptyPanelState
                data-testid="importEmpty"
                icon={<Icon name="import" size="xl" />}
                title="This build declares no readable format"
                description="The format registry is compiled in, so an empty list is a build problem rather than a missing file. Nothing can be brought in until a reader is present."
              />
            </div>
          ) : (
            <div className="mtk-taskdialog__body" data-testid={`importPane-${activeId}`}>
              {result != null ? (
                <ImportAccount result={result} report={report} />
              ) : active == null ? (
                <AnyFormatPane extensions={everything} formats={readable.length} />
              ) : (
                <>
                  <header className="mtk-taskdialog__head">
                    <SectionHeader variant="panel" className="mtk-taskdialog__format">{active.label}</SectionHeader>
                    <Badge tone={FIDELITY_TONE[active.fidelity] ?? "neutral"} title={FIDELITY_COPY[active.fidelity]?.hint}>
                      {FIDELITY_COPY[active.fidelity]?.label ?? active.fidelity}
                    </Badge>
                    <span className="mtk-taskdialog__ext">{extensionList(active).join(" · ")}</span>
                  </header>
                  <p className="mtk-taskdialog__note">{active.note}</p>

                  <section className="mtk-taskdialog__block">
                    <SectionHeader variant="eyebrow" className="mtk-taskdialog__legend" id="importCarriesLegend">
                      What this format brings in
                    </SectionHeader>
                    <ul className="mtk-taskdialog__carries" aria-labelledby="importCarriesLegend" data-testid="importCarries">
                      {CARRIES.map((c) => {
                        const carried = active.carries[c.key];
                        return (
                          <li
                            key={c.key}
                            className="mtk-taskdialog__carry"
                            data-carried={carried}
                            data-testid={`importCarry-${c.key}`}
                          >
                            {/* The mark carries the meaning and NAMES it: `Icon`'s `title` renders an
                                SVG `<title>`, which is the accessible name and is in the text of the
                                page, with no box of its own. */}
                            <Icon name={carried ? "check" : "minus"} size="sm" title={carried ? "read" : "not read"} />
                            <span>{c.title}</span>
                          </li>
                        );
                      })}
                    </ul>
                  </section>

                  {missing.length > 0 && (
                    <div className="mtk-taskdialog__cost" data-testid="importCost">
                      <Icon name="warning" size="sm" />
                      <div>
                        <strong>Not brought in: {joinWords(missing)}.</strong>
                        <span>
                          {" "}
                          Declared by the reader, before a file is opened — so it names the capability
                          and never a count. What one particular file holds is reported here once it
                          has been read.
                        </span>
                      </div>
                    </div>
                  )}
                </>
              )}
            </div>
          )}

          <footer className="mtk-taskdialog__foot">
            {result?.outcome === "imported" ? (
              <>
                <Button data-testid="importAgain" variant="secondary" onClick={() => { setResult(null); setReport(null); }}>
                  Import another
                </Button>
                <Button data-testid="importDone" variant="primary" size="comfortable" onClick={onClose} style={{ flex: 1 }}>
                  Done
                </Button>
              </>
            ) : (
              <>
                <Button data-testid="importCancel" variant="ghost" onClick={onClose}>
                  {result == null ? "Cancel" : "Close"}
                </Button>
                <Button
                  ref={confirmRef}
                  data-testid="importConfirm"
                  variant="primary"
                  size="comfortable"
                  style={{ flex: 1 }}
                  disabled={catalog == null || readable.length === 0 || busy}
                  // THREE REASONS, NOT ONE. "This build declares no format it can read" is true of an
                  // empty catalogue and false of one that has not arrived yet, and the footer renders
                  // before the read resolves — so a single reason would state a build problem during
                  // an ordinary half-second.
                  disabledReason={
                    catalog == null
                      ? "Still reading what this build can open."
                      : readable.length === 0
                        ? "This build declares no format it can read."
                        : busy
                          ? "The file dialog is open."
                          : undefined
                  }
                  onClick={() => void runImport()}
                >
                  {busy ? "Choosing…" : active == null ? "Choose a file…" : `Choose a ${active.label} file…`}
                </Button>
              </>
            )}
          </footer>
        </div>
      </DialogSurface>
    </Modal>
  );
}

/** The default pane: no filter chosen, so the subject is the whole readable set. */
function AnyFormatPane({ extensions, formats }: { extensions: readonly string[]; formats: number }) {
  return (
    <>
      <header className="mtk-taskdialog__head">
        <SectionHeader variant="panel" className="mtk-taskdialog__format">Any supported file</SectionHeader>
        <Badge tone="accent">{formats} formats</Badge>
      </header>
      <p className="mtk-taskdialog__note">
        The file dialog will show every file this build can open, and the reader is recognised from the
        file itself. Choose a format on the left to narrow it and to see what that reader carries.
      </p>
      <section className="mtk-taskdialog__block">
        <SectionHeader variant="eyebrow" className="mtk-taskdialog__legend" id="importExtLegend">
          Extensions this build opens
        </SectionHeader>
        <ul className="mtk-taskdialog__exts" aria-labelledby="importExtLegend" data-testid="importExtensions">
          {extensions.map((e) => (
            <li key={e}>.{e}</li>
          ))}
        </ul>
      </section>
    </>
  );
}

/** The importer's own account of the run — which outcome it was, and the per-part breakdown when the
 *  scene holds CAD. Replaces the options rather than flashing past them. */
function ImportAccount({ result, report }: { result: ImportDialogResponse; report: CadReport | null }) {
  if (result.outcome !== "imported") {
    // Cancelled and failed are DIFFERENT, and this is the whole reason the reply names the outcome:
    // a dismissed file dialog is not a refusal and must not be dressed as one.
    const failed = result.outcome === "failed";
    return (
      <div
        className={failed ? "mtk-taskdialog__failure" : "mtk-taskdialog__cost"}
        data-testid="importRefused"
        data-outcome={result.outcome}
        role="status"
      >
        <Icon name={failed ? "warning" : "info"} size="md" />
        <p>{result.message}</p>
      </div>
    );
  }
  const counted = report != null && report.total > 0;
  return (
    <div className="mtk-taskdialog__report" data-testid="importResult" role="status">
      <section className="mtk-taskdialog__block">
        <SectionHeader variant="eyebrow" className="mtk-taskdialog__legend">What came in</SectionHeader>
        <p className="mtk-taskdialog__note" data-testid="importMessage">{result.message}</p>
        {result.entityId != null && (
          <p className="mtk-taskdialog__path" data-testid="importEntity" title={result.entityId}>
            {result.entityId} · selected in the scene
          </p>
        )}
      </section>
      {counted && (
        <section className="mtk-taskdialog__block">
          <SectionHeader variant="eyebrow" className="mtk-taskdialog__legend">How exactly it came in</SectionHeader>
          <ul className="mtk-taskdialog__ledger mtk-taskdialog__ledger--counts" data-testid="importFidelity">
            {FIDELITY_ROWS.filter((row) => (report[row.key] as number) > 0).map((row) => (
              <li key={row.key} data-fidelity={row.key}>
                <Badge tone={row.tone}>{report[row.key] as number}</Badge>
                <div>
                  <strong>{row.label}</strong>
                </div>
              </li>
            ))}
          </ul>
          <p className="mtk-taskdialog__note">
            {report.total} part{report.total === 1 ? "" : "s"} accounted for. Every class is explained,
            with its one-click fix, in the Import report.
          </p>
        </section>
      )}
    </div>
  );
}

export default ImportDialog;
