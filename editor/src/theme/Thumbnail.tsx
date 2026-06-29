//! The live per-entity thumbnail (M14.2 / ADR-058) — the flagship. Renders the entity's **real** viewport
//! pixels (a `data:` PNG the native renderer produced via an off-frame RTT) when ready, else the styled
//! type-icon fallback. It only READS the thumbnail store (selective subscription — a row re-renders only when
//! ITS own thumbnail changes); the REQUEST policy (visible-only · dirty-only · budget) lives in the store and
//! is driven by the panel reporting its visible window. `data-thumb-status` is the structured signal tests
//! key on (`ready` vs `fallback`), never a styled string.

import { useThumb } from "../store/thumbnails";
import { TypeIcon } from "./primitives";

export function Thumbnail({
  id,
  kind,
  size = 40,
  selected = false,
  title,
}: {
  id: string;
  kind: string;
  size?: number;
  selected?: boolean;
  title?: string;
}) {
  const entry = useThumb(id);
  const ready = entry?.status === "ready" && !!entry.url;
  return (
    <span
      className={"mtk-thumb" + (selected ? " is-selected" : "")}
      data-testid="thumb"
      data-thumb-status={ready ? "ready" : "fallback"}
      data-kind={kind}
      title={title}
      style={{ width: size, height: size }}
    >
      {ready ? (
        <img src={entry!.url!} alt="" draggable={false} />
      ) : (
        <TypeIcon kind={kind} size={size} style={{ border: "none", borderRadius: 0 }} />
      )}
    </span>
  );
}
