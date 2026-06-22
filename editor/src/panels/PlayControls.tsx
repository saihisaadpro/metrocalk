//! Play / Pause / Stop runtime controls (M10.4 / ADR-034) — the editor's "Press Play". Play runs the
//! deterministic sim on the current scene (the engine thread, off the JS hot path — inv. 4); Pause
//! freezes it; **Stop is always reachable while playing** and restores the exact pre-Play edit state
//! (non-destructive). An unmistakable **Play-mode indicator** shows the runtime is live.
//!
//! Runtime state is authoritative on the shell; this mirrors it into the play store (so edit affordances
//! can disable themselves while playing — the edit↔play boundary). Stable ids (`#play`/`#pause`/`#stop`/
//! `#playIndicator`) for the prompt-40 acceptance page-object.

import { useEffect } from "react";
import { playStore, usePlaying, usePaused } from "../store/play";
import { setStatus } from "../store/ui";
import type { EditorClient } from "../transport/session";
import type { PlayInfo } from "../store/play";

export function PlayControls({ client }: { client: EditorClient }) {
  const playing = usePlaying();
  const paused = usePaused();

  // Mirror the authoritative runtime state on mount (and if the client identity changes).
  useEffect(() => {
    let live = true;
    client
      .playState()
      .then((info) => {
        if (live) playStore.getState().refresh(info);
      })
      .catch(() => {
        /* a failed read leaves Stopped — never crash the chrome */
      });
    return () => {
      live = false;
    };
  }, [client]);

  async function act(action: () => Promise<PlayInfo>, status: (info: PlayInfo) => string) {
    const info = await action();
    playStore.getState().refresh(info);
    setStatus(status(info));
  }

  const btn = (bg: string): React.CSSProperties => ({
    padding: "3px 10px",
    background: bg,
    color: "#e8e8e8",
    border: "1px solid #2a2d35",
    borderRadius: 4,
    cursor: "pointer",
    font: "12px ui-monospace, monospace",
  });

  return (
    <div id="playControls" style={{ display: "flex", alignItems: "center", gap: 6 }}>
      {!playing ? (
        <button id="play" data-testid="play" onClick={() => void act(() => client.play(), () => "▶ playing")} style={btn("#1f4a2f")}>
          ▶ Play
        </button>
      ) : (
        <>
          <button
            id="pause"
            data-testid="pause"
            onClick={() => void act(() => client.pause(), (i) => (i.paused ? "⏸ paused" : "▶ resumed"))}
            style={btn("#3a3416")}
          >
            {paused ? "▶ Resume" : "⏸ Pause"}
          </button>
          {/* Stop is ALWAYS reachable while playing (the escape hatch). */}
          <button id="stop" data-testid="stop" onClick={() => void act(() => client.stop(), () => "⏹ stopped")} style={btn("#5a2f1f")}>
            ⏹ Stop
          </button>
          <span
            id="playIndicator"
            data-testid="playIndicator"
            title="the scene is running — edits are disabled until you Stop"
            style={{ marginLeft: 4, padding: "2px 8px", borderRadius: 4, background: paused ? "#3a3416" : "#1f4a2f", color: paused ? "#fbbf24" : "#7fe39a", font: "11px ui-monospace, monospace" }}
          >
            {paused ? "⏸ PAUSED" : "● PLAYING"}
          </span>
        </>
      )}
    </div>
  );
}
