//! Mounts ONE scene from `scenes.tsx`, chosen by `?scene=<id>`, with the real theme stylesheet.
//!
//! A static bundle that opens over `file://`, so a panel can be photographed in a real browser on a
//! box with no GPU, no display and no Tauri shell. Deliberately not a route in the editor app: the
//! point is to isolate the panel from the shell, the transport and the rest of the layout, so a
//! capture says something about the panel and nothing about its neighbours.

import { createRoot } from "react-dom/client";
import "../../src/theme/global.css";
import { SCENES } from "./scenes";

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
root.render(
  <div
    data-testid="shot-frame"
    style={{ width: "100%", maxWidth: scene.width ?? 560, background: "var(--mtk-bg-panel)", minHeight: "100vh" }}
  >
    {scene.render()}
  </div>,
);
