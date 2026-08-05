//! Shared graph visual contract.
//!
//! Animation, binding, state, material and future graph editors use the same semantic node, edge, canvas
//! and selection language. Domain adapters still own topology and transactions; this module owns chrome.

import type { CSSProperties } from "react";
import { color, elevation, font, fontSize, radius, space } from "./tokens";

export type GraphNodeEmphasis = "default" | "selected" | "initial" | "live" | "blocked";
export type GraphEdgeState = "default" | "selected" | "confirmed" | "pending" | "rejected" | "active" | "disabled";

export const graphTheme = {
  canvas: color.bg.inset,
  grid: color.border.subtle,
  node: color.bg.raised,
  nodeText: color.text.primary,
  edge: color.text.muted,
  selection: color.accent.base,
  initial: color.info.text,
  live: color.success.text,
  warning: color.warn.text,
  danger: color.danger.text,
} as const;

export function graphNodeStyle(emphasis: GraphNodeEmphasis = "default"): CSSProperties {
  const border =
    emphasis === "selected"
      ? `2px solid ${color.accent.base}`
      : emphasis === "initial"
        ? `2px solid ${color.info.text}`
        : emphasis === "live"
          ? `2px solid ${color.success.text}`
          : emphasis === "blocked"
            ? `1px solid ${color.danger.border}`
            : `1px solid ${color.border.default}`;

  return {
    minWidth: 116,
    padding: `${space.sm}px ${space.md}px`,
    border,
    borderRadius: radius.lg,
    background: color.bg.raised,
    color: color.text.primary,
    boxShadow: emphasis === "live" ? `0 0 0 2px ${color.success.border}, ${elevation.e1}` : elevation.e1,
    fontFamily: font.ui,
    fontSize: fontSize.body,
    lineHeight: 1.35,
    opacity: emphasis === "blocked" ? 0.64 : 1,
  };
}

export function graphEdgeStyle(state: GraphEdgeState = "default"): CSSProperties {
  const stroke =
    state === "confirmed" || state === "active"
      ? color.success.text
      : state === "selected"
        ? color.accent.base
        : state === "pending"
          ? color.warn.text
          : state === "rejected"
            ? color.danger.text
            : state === "disabled"
              ? color.text.faint
              : color.text.muted;

  return {
    stroke,
    strokeWidth: state === "active" ? 2 : 1.25,
    strokeDasharray: state === "pending" ? "5 4" : undefined,
    opacity: state === "disabled" ? 0.58 : 1,
  };
}
