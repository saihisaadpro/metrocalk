//! Editor shell — wires the projection store to a (mock) core over the in-process transport and lays
//! out the panels. The viewport is a placeholder rect that demonstrates the **input-ownership
//! contract**: pointer events over it are deferred to the native wgpu layer (invariant 4), wired for
//! real in M2.6. Everything else here is UI chrome owned by React.

import { useMemo, useRef } from "react";
import { createSession, type EditorClient } from "../transport/session";
import { shouldDeferToNative, type Rect } from "../input/ownership";
import { Hierarchy } from "../panels/Hierarchy";
import { Rejections } from "../panels/Rejections";
import { Inspector } from "../inspector/Inspector";
import { BindingGraph } from "../graph/BindingGraph";

/** Build the editor session once: the REAL Tauri shell transport inside the packaged `.exe` (the live
 *  `/core` over the `connect` Channel), else the in-process MockCore for `npm run dev` / tests. */
function useEditorSession(): EditorClient {
  const ref = useRef<EditorClient | null>(null);
  if (!ref.current) {
    ref.current = createSession();
  }
  return ref.current;
}

export function App() {
  const client = useEditorSession();
  // Placeholder viewport rect; M2.6 supplies the real wgpu region.
  const viewport: Rect = useMemo(() => ({ x: 280, y: 56, w: 600, h: 620 }), []);

  return (
    <div style={{ height: "100vh", display: "flex", flexDirection: "column", background: "#0a0a0f", color: "#e8e8e8" }}>
      <div style={{ height: 40, display: "flex", alignItems: "center", gap: 12, padding: "0 12px", background: "#14161c", borderBottom: "1px solid #2a2d35", font: "13px ui-monospace, monospace" }}>
        <strong>metrocalk</strong> <span style={{ opacity: 0.6 }}>editor — projection of the core (M2.5)</span>
      </div>
      <div style={{ flex: 1, display: "grid", gridTemplateColumns: "260px 1fr 320px", minHeight: 0 }}>
        <div style={{ borderRight: "1px solid #2a2d35", overflow: "hidden" }}>
          <Hierarchy />
        </div>
        {/* viewport: native-owned. React must NOT handle hot input here (invariant 4). */}
        <div
          onPointerDown={(e) => {
            if (shouldDeferToNative(e.clientX, e.clientY, viewport)) {
              // left for the native wgpu picker — wired in M2.6. Do nothing in JS.
              return;
            }
          }}
          style={{ position: "relative", background: "#0d0f15", display: "flex", alignItems: "center", justifyContent: "center", color: "#444", font: "12px ui-monospace, monospace" }}
        >
          native wgpu viewport (M2.6) — hot input owned by Rust, not JS
        </div>
        <div style={{ borderLeft: "1px solid #2a2d35", overflowY: "auto", display: "flex", flexDirection: "column" }}>
          <Inspector client={client} />
          <div style={{ borderTop: "1px solid #2a2d35", flex: 1, minHeight: 260 }}>
            <BindingGraph />
          </div>
        </div>
      </div>
      <Rejections />
    </div>
  );
}
