//! Visible, accessible feedback for a genuine native OS file drop.
//!
//! Import work remains Rust-owned; this is only its projection. Cancellation is truthful: the action signals
//! the Rust-owned token directly, then remains "Stopping" until the worker confirms a safe checkpoint.

import { useEffect, useRef, useState } from "react";
import { pushToast } from "../store/toasts";
import { setStatus } from "../store/ui";
import { Badge, Button, Surface } from "../theme/primitives";
import { color, elevation, font, fontSize, radius, space, text, z } from "../theme/tokens";
import {
  EMPTY_NATIVE_IMPORT_STATE,
  cancelNativeImport,
  clearNativeImportStopping,
  markNativeImportStopping,
  reduceNativeImportState,
  subscribeNativeImportLifecycle,
  type NativeImportLifecycleEvent,
} from "./nativeImportLifecycle";

export interface ImportDropOverlayProps {
  onEntityImported(rootId: string): void;
  onOpenImportReport(): void;
  onOpenFormats(): void;
  onImportAnother(): void;
}

function progressMessage(event: Extract<NativeImportLifecycleEvent, { phase: "progress" }>): string {
  if (event.stage === "queued") return `Queued ${event.fileName}`;
  if (event.stage === "delayed") {
    const elapsed = Math.max(1, Math.round(event.elapsedMs / 1000));
    return `Still importing ${event.fileName} · ${elapsed}s elapsed`;
  }
  return `Importing ${event.fileName}`;
}

function headingFor(event: NativeImportLifecycleEvent, stopping: boolean): string {
  if (stopping) return "Stopping at a safe checkpoint…";
  switch (event.phase) {
    case "hovered":
      return event.files.some((file) => file.supported) ? "Release to import" : "This drop cannot be imported";
    case "dropped":
      return "Drop received";
    case "progress":
      return progressMessage(event);
    case "succeeded":
      return event.message;
    case "refused":
    case "failed":
    case "cancelled":
      return event.message;
    case "left":
      return "";
  }
}

function badgeFor(event: NativeImportLifecycleEvent, stopping: boolean): { label: string; tone: "neutral" | "accent" | "warn" | "success" } {
  if (stopping) return { label: "Stopping", tone: "warn" };
  switch (event.phase) {
    case "hovered":
      return event.files.some((file) => file.supported)
        ? { label: "Ready", tone: "accent" }
        : { label: "Unsupported", tone: "warn" };
    case "dropped":
      return { label: "Queued", tone: "accent" };
    case "progress":
      return { label: event.stage === "delayed" ? "Still working" : "Importing", tone: event.stage === "delayed" ? "warn" : "accent" };
    case "succeeded":
      return { label: "Imported", tone: "success" };
    case "refused":
      return { label: "Not imported", tone: "warn" };
    case "failed":
      return { label: "Import failed", tone: "warn" };
    case "cancelled":
      return { label: "Cancelled", tone: "neutral" };
    case "left":
      return { label: "", tone: "neutral" };
  }
}

export function ImportDropOverlay(props: ImportDropOverlayProps) {
  const [view, setView] = useState(EMPTY_NATIVE_IMPORT_STATE);
  const viewRef = useRef(EMPTY_NATIVE_IMPORT_STATE);
  const callbacks = useRef(props);
  callbacks.current = props;

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    const receive = (event: NativeImportLifecycleEvent) => {
      if (disposed) return;
      const current = viewRef.current;
      const next = reduceNativeImportState(current, event);
      // Paint and side effects must make the same ordering decision. In particular, an old batch
      // completing after a newer drop must not select its entity or replace the newer progress view.
      if (next === current) return;
      viewRef.current = next;
      setView(next);
      switch (event.phase) {
        case "dropped": {
          const count = event.files.length;
          setStatus(`${count} ${count === 1 ? "file" : "files"} queued for import`);
          break;
        }
        case "progress":
          setStatus(next.stoppingBatchId === event.batchId ? "Stopping import at a safe checkpoint" : progressMessage(event));
          break;
        case "succeeded":
          if (event.subject.kind === "entity") callbacks.current.onEntityImported(event.subject.rootId);
          setStatus(event.message);
          pushToast(event.message, "success");
          break;
        case "refused":
          setStatus(event.message);
          pushToast(event.message, "error");
          break;
        case "failed":
          setStatus(event.message);
          pushToast(event.message, "error");
          break;
        case "cancelled":
          setStatus(event.message);
          pushToast(event.message, "info");
          break;
        default:
          break;
      }
    };

    void subscribeNativeImportLifecycle(receive)
      .then((stop) => {
        if (disposed) stop();
        else unlisten = stop;
      })
      .catch((error) => {
        if (!disposed) console.error("native import lifecycle listener failed", error);
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const dismiss = () => {
    // Keep the batch watermark so a delayed event from an older batch cannot resurrect a dismissed card.
    const next = { ...viewRef.current, event: null };
    viewRef.current = next;
    setView(next);
  };

  const requestCancellation = () => {
    const current = viewRef.current;
    const event = current.event;
    if (!event || (event.phase !== "dropped" && event.phase !== "progress")) return;
    const batchId = event.batchId;
    const next = markNativeImportStopping(current, batchId);
    if (next === current) return;
    viewRef.current = next;
    setView(next);
    setStatus("Stopping import at a safe checkpoint");

    void cancelNativeImport(batchId)
      .then((accepted) => {
        if (accepted) return;
        const latest = viewRef.current;
        const latestEvent = latest.event;
        const stillBusy = latestEvent
          && (latestEvent.phase === "dropped" || latestEvent.phase === "progress")
          && latestEvent.batchId === batchId;
        if (!stillBusy) return;
        const reopened = clearNativeImportStopping(latest, batchId);
        viewRef.current = reopened;
        setView(reopened);
        setStatus("This import has already crossed its final commit checkpoint");
      })
      .catch((error) => {
        const latest = viewRef.current;
        const latestEvent = latest.event;
        const stillBusy = latestEvent
          && (latestEvent.phase === "dropped" || latestEvent.phase === "progress")
          && latestEvent.batchId === batchId;
        if (!stillBusy) return;
        const reopened = clearNativeImportStopping(latest, batchId);
        viewRef.current = reopened;
        setView(reopened);
        setStatus("Could not send the cancellation request; the import is still running");
        console.error("native import cancellation failed", error);
      });
  };

  const event = view.event;
  if (!event || event.phase === "left") return null;
  const alert = event.phase === "refused" || event.phase === "failed";
  const stopping = (event.phase === "dropped" || event.phase === "progress")
    && view.stoppingBatchId === event.batchId;
  const badge = badgeFor(event, stopping);
  const busy = event.phase === "dropped" || event.phase === "progress";
  const files = event.phase === "hovered" || event.phase === "dropped" ? event.files : null;

  return (
    <div
      data-testid="native-import-overlay"
      data-phase={event.phase}
      role={alert ? "alert" : "status"}
      aria-live={alert ? "assertive" : "polite"}
      aria-atomic="true"
      aria-busy={busy || undefined}
      style={{
        position: "absolute",
        top: space.xxl,
        left: "50%",
        zIndex: z.badge,
        width: `min(520px, calc(100% - ${space.xxl * 2}px))`,
        transform: "translateX(-50%)",
        pointerEvents: "none",
      }}
    >
      <Surface
        tone="floating"
        style={{
          display: "grid",
          gap: space.md,
          padding: space.lg,
          borderRadius: radius.lg,
          border: `1px solid ${alert ? color.danger.border : color.border.strong}`,
          boxShadow: elevation.e3,
          color: color.text.primary,
          background: color.bg.raised,
        }}
      >
        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: space.md }}>
          <strong style={text.panelTitle}>{headingFor(event, stopping)}</strong>
          <Badge tone={badge.tone}>{badge.label}</Badge>
        </div>

        {files && (
          <div style={{ display: "grid", gap: space.xs }}>
            {files.map((file, index) => (
              <div
                key={`${file.name}-${index}`}
                style={{
                  display: "flex",
                  alignItems: "baseline",
                  justifyContent: "space-between",
                  gap: space.md,
                  padding: `${space.xs}px ${space.sm}px`,
                  borderRadius: radius.sm,
                  background: color.bg.inset,
                  border: `1px solid ${color.border.subtle}`,
                }}
              >
                <span style={{ minWidth: 0, overflowWrap: "anywhere", font: font.mono, fontSize: fontSize.body }}>{file.name}</span>
                <span style={{ flex: "none", color: file.supported ? color.success.text : color.warn.text, fontSize: fontSize.meta }}>
                  {file.supported ? "Supported" : file.reason ?? "Unavailable"}
                </span>
              </div>
            ))}
          </div>
        )}

        {event.phase === "progress" && (
          <div style={{ display: "grid", gap: space.sm }}>
            <progress aria-label={`Import progress for ${event.fileName}`} style={{ width: "100%" }} />
            {event.total > 1 && (
              <span style={{ color: color.text.secondary, fontSize: fontSize.meta }}>
                File {event.index} of {event.total} · {event.completed} complete
              </span>
            )}
            {event.stage === "delayed" && (
              <span style={{ color: color.warn.text, fontSize: fontSize.meta }}>
                Large CAD assemblies can take several minutes. The importer is still working in order; no retry is needed.
              </span>
            )}
          </div>
        )}

        {stopping && (
          <span style={{ color: color.warn.text, fontSize: fontSize.meta }}>
            Cancellation has been requested. Parsing may finish before the importer reaches a safe checkpoint; no scene changes will be committed.
          </span>
        )}

        {event.phase === "refused" && (
          <span style={{ color: color.text.secondary, fontSize: fontSize.body }}>
            No scene changes were made. Check the formats available in this build or choose another file.
          </span>
        )}
        {event.phase === "failed" && (
          <span style={{ color: color.text.secondary, fontSize: fontSize.body }}>
            The editor remains usable. Review the import diagnostics before retrying to avoid duplicate geometry.
          </span>
        )}
        {event.phase === "cancelled" && (
          <span style={{ color: color.text.secondary, fontSize: fontSize.body }}>
            The cancelled batch made no scene commit. You can safely retry this file.
          </span>
        )}

        {busy && (
          <div style={{ display: "flex", justifyContent: "flex-end", pointerEvents: "auto" }}>
            <Button
              compact
              variant="secondary"
              disabled={stopping}
              aria-label={stopping ? "Stopping import" : "Cancel import"}
              onClick={requestCancellation}
            >
              {stopping ? "Stopping…" : "Cancel"}
            </Button>
          </div>
        )}

        {(event.phase === "succeeded" || event.phase === "refused" || event.phase === "failed" || event.phase === "cancelled") && (
          <div style={{ display: "flex", justifyContent: "flex-end", flexWrap: "wrap", gap: space.sm, pointerEvents: "auto" }}>
            {event.phase === "refused" ? (
              <Button compact onClick={() => callbacks.current.onOpenFormats()}>
                Supported formats
              </Button>
            ) : event.phase !== "cancelled" ? (
              <Button compact onClick={() => callbacks.current.onOpenImportReport()}>
                Import report
              </Button>
            ) : null}
            {event.phase !== "succeeded" && (
              <Button compact variant="primary" onClick={() => callbacks.current.onImportAnother()}>
                Import another…
              </Button>
            )}
            <Button compact variant="ghost" onClick={dismiss}>
              Dismiss
            </Button>
          </div>
        )}
      </Surface>
    </div>
  );
}
