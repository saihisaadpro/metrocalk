// Sub-gate 1b — the React UI that floats over the native wgpu surface.
//
// The whole point: opaque "chrome" (toolbar + side panel) around a fully TRANSPARENT center
// "viewport". If compositing works on Windows WebView2, the native wgpu triangle shows through the
// transparent center; if it doesn't (the Graphite failure), the center is opaque/black/garbage.
// The draggable box + buttons exercise input routing (clicks must hit the webview layer, drags over
// the viewport must still reach the right layer).
import { useState, useCallback } from "react";
import { createRoot } from "react-dom/client";

function Toolbar({ onLog }: { onLog: (s: string) => void }) {
  return (
    <div
      style={{
        height: 44,
        background: "rgba(20,22,28,0.92)",
        color: "#e8e8e8",
        display: "flex",
        alignItems: "center",
        gap: 12,
        padding: "0 12px",
        font: "13px ui-monospace, monospace",
        borderBottom: "1px solid rgba(255,255,255,0.12)",
      }}
    >
      <strong>metrocalk · 1b compositing</strong>
      <button onClick={() => onLog("toolbar button click")}>action</button>
      <span style={{ opacity: 0.7 }}>opaque chrome ↑ · transparent viewport ↓ (triangle should show through)</span>
    </div>
  );
}

function SidePanel() {
  return (
    <div
      style={{
        width: 200,
        background: "rgba(20,22,28,0.85)",
        color: "#cfd2d6",
        padding: 12,
        font: "12px ui-monospace, monospace",
        borderRight: "1px solid rgba(255,255,255,0.12)",
      }}
    >
      <div style={{ marginBottom: 8, fontWeight: 700 }}>inspector</div>
      <div>opaque side panel — proves the UI layer paints.</div>
      <div style={{ marginTop: 8, opacity: 0.7 }}>
        The viewport to the right is transparent. Native wgpu renders beneath the whole window.
      </div>
    </div>
  );
}

function DraggableProbe() {
  const [pos, setPos] = useState({ x: 40, y: 40 });
  const [drag, setDrag] = useState(false);
  const down = useCallback(() => setDrag(true), []);
  const up = useCallback(() => setDrag(false), []);
  const move = useCallback(
    (e: React.MouseEvent) => {
      if (drag) setPos((p) => ({ x: p.x + e.movementX, y: p.y + e.movementY }));
    },
    [drag],
  );
  return (
    <div
      onMouseDown={down}
      onMouseUp={up}
      onMouseMove={move}
      style={{
        position: "absolute",
        left: pos.x,
        top: pos.y,
        width: 120,
        height: 80,
        background: drag ? "rgba(80,180,255,0.55)" : "rgba(80,180,255,0.30)",
        border: "1px solid rgba(160,210,255,0.9)",
        borderRadius: 8,
        color: "#fff",
        font: "12px ui-monospace, monospace",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        cursor: "grab",
        userSelect: "none",
      }}
    >
      drag me over viewport
    </div>
  );
}

function CompositeUI() {
  const [log, setLog] = useState<string[]>([]);
  const onLog = (s: string) => setLog((l) => [s, ...l].slice(0, 6));
  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100vh", background: "transparent" }}>
      <Toolbar onLog={onLog} />
      <div style={{ display: "flex", flex: 1, minHeight: 0 }}>
        <SidePanel />
        {/* the transparent viewport — must show the native wgpu triangle through it */}
        <div
          onClick={(e) => onLog(`viewport click @ ${e.clientX},${e.clientY}`)}
          style={{ position: "relative", flex: 1, background: "transparent", overflow: "hidden" }}
        >
          <div
            style={{
              position: "absolute",
              inset: 0,
              border: "2px dashed rgba(255,255,255,0.25)",
              margin: 8,
              borderRadius: 8,
              pointerEvents: "none",
              color: "rgba(255,255,255,0.5)",
              font: "12px ui-monospace, monospace",
              padding: 6,
            }}
          >
            transparent viewport — wgpu triangle should be visible here
          </div>
          <DraggableProbe />
          <div
            style={{
              position: "absolute",
              right: 8,
              bottom: 8,
              background: "rgba(0,0,0,0.55)",
              color: "#9fe",
              padding: 8,
              borderRadius: 6,
              font: "11px ui-monospace, monospace",
              maxWidth: 280,
            }}
          >
            input-routing log:
            {log.map((l, i) => (
              <div key={i}>· {l}</div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}

export function mountComposite() {
  // make the document itself transparent so the native layer shows through unpainted regions
  document.documentElement.style.background = "transparent";
  document.body.style.background = "transparent";
  const out = document.getElementById("out");
  if (out) out.remove();
  createRoot(document.getElementById("root")!).render(<CompositeUI />);
}
