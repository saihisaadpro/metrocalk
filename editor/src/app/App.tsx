//! Editor shell — wires the projection store to a (mock) core over the in-process transport and lays
//! out the panels. The viewport is a placeholder rect that demonstrates the **input-ownership
//! contract**: pointer events over it are deferred to the native wgpu layer (invariant 4), wired for
//! real in M2.6. Everything else here is UI chrome owned by React.

import { useMemo, useRef } from "react";
import { DeltaClient } from "../transport/client";
import { MockCore } from "../transport/mock-core";
import type { EntityProjection } from "../transport/protocol";
import { inProcessPair } from "../transport/transport";
import { shouldDeferToNative, type Rect } from "../input/ownership";
import { Hierarchy } from "../panels/Hierarchy";
import { Rejections } from "../panels/Rejections";
import { Inspector } from "../inspector/Inspector";
import { BindingGraph } from "../graph/BindingGraph";

const CAPS = ["Health", "Shield", "Click", "Damage", "Light"];

/** A seeded 5k scene for the core (authoritative). Deterministic so the dev view is reproducible. */
function buildWorld(n: number): EntityProjection[] {
  const out: EntityProjection[] = [];
  let seed = 0x9e3779b9;
  const rnd = () => ((seed = (seed * 1664525 + 1013904223) >>> 0) / 0xffffffff);
  for (let i = 0; i < n; i++) {
    const components: EntityProjection["components"] = {
      Transform: { x: Math.round(rnd() * 100), y: Math.round(rnd() * 100), z: 0 },
    };
    if (i % 7 === 0) components.Material = { color: "#88ccff", metalness: 0.2 };
    if (i % 5 === 0) components.Provides = { capability: CAPS[i % CAPS.length] };
    if (i % 11 === 0) components.Socket = { accepts: CAPS[(i + 1) % CAPS.length] };
    if (i % 13 === 0) components.Targeting = { target: "" };
    out.push({ id: `e${i}`, name: `Entity ${i}`, parentId: i === 0 ? null : "e0", components });
  }
  return out;
}

function useEditorSession(): DeltaClient {
  const ref = useRef<DeltaClient | null>(null);
  if (!ref.current) {
    const [uiT, coreT] = inProcessPair();
    const world = buildWorld(5000);
    const core = new MockCore(coreT, world);
    const client = new DeltaClient(uiT);
    core.emitScene(); // initial committed delta → the UI projects it
    ref.current = client;
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
