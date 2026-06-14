//! Input-ownership contract (deliverable 6 — stub + documented for M2.6).
//!
//! Invariant 4: the hot path (viewport, gizmos, drag feedback) never crosses the JS boundary. So the
//! React side claims **only UI-chrome pixels**; a pointer over the viewport rect is left for the
//! native wgpu layer (picking/gizmo) and is NOT handled in JS. This is the TS twin of the M2.3
//! `shell-input-routing` Rust crate — same per-pixel UI-vs-viewport split, from the React side.
//!
//! In the browser build the viewport is a native canvas region; in the desktop shell it's the wgpu
//! HWND under the transparent WebView2 (M2.3 single-window). M2.6 wires the real viewport rect +
//! forwards viewport-owned events to the native layer; here we define the boundary so it plugs in.

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export type Owner = "ui" | "viewport";

/** Which layer owns a pointer at `(px,py)` given the viewport rect and the UI-chrome occluders that
 *  float over it (open dropdowns, the inspector popover). UI chrome wins where present; otherwise the
 *  viewport owns the event (and the React handler must NOT call preventDefault / must let it pass to
 *  the native layer). */
export function ownerAt(px: number, py: number, viewport: Rect, uiOverlays: Rect[] = []): Owner {
  const inViewport = px >= viewport.x && px < viewport.x + viewport.w && py >= viewport.y && py < viewport.y + viewport.h;
  if (!inViewport) return "ui"; // outside the viewport = chrome (panels/toolbar)
  for (const o of uiOverlays) {
    if (px >= o.x && px < o.x + o.w && py >= o.y && py < o.y + o.h) return "ui"; // a dropdown over the viewport
  }
  return "viewport";
}

/**
 * React event guard for the viewport host element. Returns `true` if the event belongs to the native
 * layer (the caller should then NOT handle it in JS — leave it for the wgpu picker). M2.6 replaces the
 * body with the real forward-to-native call; the decision logic is final here.
 */
export function shouldDeferToNative(px: number, py: number, viewport: Rect, uiOverlays: Rect[] = []): boolean {
  return ownerAt(px, py, viewport, uiOverlays) === "viewport";
}
