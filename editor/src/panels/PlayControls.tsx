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
import { Icon } from "../theme/icons";
import { Button } from "../theme/primitives";
import { color, font, fontSize, radius, space } from "../theme/tokens";
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

  return (
    <div id="playControls" style={{ display: "flex", alignItems: "center", gap: space.sm }}>
      {!playing ? (
        <Button id="play" data-testid="play" variant="primary" compact onClick={() => void act(() => client.play(), () => "playing")}>
          <Icon name="play" size="sm" />
          Play
        </Button>
      ) : (
        <>
          <Button
            id="pause"
            data-testid="pause"
            variant="secondary"
            compact
            onClick={() => void act(() => client.pause(), (i) => (i.paused ? "paused" : "resumed"))}
          >
            <Icon name={paused ? "play" : "pause"} size="sm" />
            {paused ? "Resume" : "Pause"}
          </Button>
          {/* Stop is ALWAYS reachable while playing (the escape hatch). */}
          <Button id="stop" data-testid="stop" variant="danger" compact onClick={() => void act(() => client.stop(), () => "stopped")}>
            <Icon name="stop" size="sm" />
            Stop
          </Button>
          <span
            id="playIndicator"
            data-testid="playIndicator"
            title="the scene is running — edits are disabled until you Stop"
            data-state={paused ? "paused" : "playing"}
            style={{
              display: "inline-flex",
              alignItems: "center",
              gap: space.xs,
              marginLeft: space.xs,
              padding: `2px ${space.md}px`,
              borderRadius: radius.md,
              background: paused ? color.warn.bg : color.success.bg,
              color: paused ? color.warn.text : color.success.text,
              border: `1px solid ${paused ? color.warn.border : color.success.border}`,
              font: font.mono,
              fontSize: fontSize.meta,
            }}
          >
            {/* `data-state` above is what a test asserts — `<test_and_ci_discipline>` 3. The badge used to
                carry U+23F8 in its text and PlayControls.test.tsx matched the literal string, which coupled
                a state assertion to a character the UI font stack does not contain. */}
            {paused ? <Icon name="pause" size={11} /> : <span aria-hidden className="mtk-live-dot" />}
            {paused ? "PAUSED" : "PLAYING"}
          </span>
        </>
      )}
    </div>
  );
}
