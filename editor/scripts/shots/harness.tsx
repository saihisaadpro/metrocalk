//! Mounts ONE scene from `scenes.tsx`, chosen by `?scene=<id>`, with the real theme stylesheet.
//!
//! A static bundle that opens over `file://`, so a panel can be photographed in a real browser on a
//! box with no GPU, no display and no Tauri shell. Deliberately not a route in the editor app: the
//! point is to isolate the panel from the shell, the transport and the rest of the layout, so a
//! capture says something about the panel and nothing about its neighbours.

import { createRoot } from "react-dom/client";
import "../../src/theme/global.css";
import { SCENES } from "./scenes";

// The driver reads the registry from HERE — the built bundle, the same objects the page renders —
// rather than by regexing `scenes.tsx`. A regex over source is a second statement of the scene list
// that nothing compares to the first, which is the failure this whole repository keeps gating for.
declare global {
  interface Window {
    __MTK_SHOTS__: {
      id: string;
      looking_for: string;
      expect: unknown;
      width?: number;
      viewport?: { width: number; height: number };
      click?: string[];
    }[];
  }
}
window.__MTK_SHOTS__ = SCENES.map((s) => ({
  id: s.id,
  looking_for: s.looking_for,
  expect: s.expect,
  // `width` is forwarded even though the DRIVER never applies it — the harness does, below. It is
  // here so the driver can reject a scene that sets both `width` and `viewport`, and that check was
  // dead the first time it was written precisely because this line was missing: the guard compared a
  // field the registry did not carry, so it agreed with everything. Found by mutating a scene to set
  // both and watching the run pass.
  width: s.width,
  viewport: s.viewport,
  click: s.click,
}));

const id = new URLSearchParams(location.search).get("scene") ?? SCENES[0].id;
const scene = SCENES.find((s) => s.id === id);

const root = createRoot(document.getElementById("root")!);

if (!scene) {
  // Loud, not blank. A mistyped id that rendered an empty page would be photographed as "the panel
  // renders nothing", which is a real state some scenes assert — the two must never look alike.
  document.title = `unknown scene: ${id}`;
  root.render(
    <pre style={{ font: "14px monospace", color: "#b00", padding: 16 }}>
      {`no scene with id "${id}". Known: ${SCENES.map((s) => s.id).join(", ")}`}
    </pre>,
  );
  throw new Error(`unknown scene: ${id}`); // surfaces as a pageerror, which shoot.mjs fails on
}

scene.setup?.();
document.title = scene.id;
// A scene that resized the WINDOW is measured against the window: capping the frame as well would
// state the same number twice, and the second statement is the one that goes stale. `100%` of a body
// whose margin is already 0 IS the viewport width.
root.render(
  <div
    data-testid="shot-frame"
    style={{
      width: "100%",
      maxWidth: scene.viewport ? "none" : (scene.width ?? 560),
      background: "var(--mtk-bg-panel)",
      minHeight: "100vh",
    }}
  >
    {scene.render()}
  </div>,
);
