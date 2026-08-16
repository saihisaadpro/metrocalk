import { useEffect } from "react";
import { usePlaying } from "../store/play";
import type { EditorClient } from "../transport/session";

const DRIVE_KEYS = ["ArrowRight", "ArrowLeft", "ArrowUp", "ArrowDown", "w", "a", "s", "d"];

/** Normalize a keyboard event to a drive key (letters case-folded), or null if not one. */
function driveKey(e: KeyboardEvent): string | null {
  const k = e.key.length === 1 ? e.key.toLowerCase() : e.key;
  return DRIVE_KEYS.includes(k) ? k : null;
}

/**
 * WASD / arrow keys drive the Player role while Play runs — world x/z axes, sent to the
 * engine ONLY when the axis changes (a held key costs zero IPC). Typing into any field is
 * never hijacked, and releasing Play (or unmount) always parks the axis at rest.
 */
export function usePlayerDrive(client: EditorClient): void {
  const playing = usePlaying();
  useEffect(() => {
    if (!playing) return undefined;
    const held = new Set<string>();
    let last: [number, number] = [0, 0];
    const send = () => {
      const x =
        (held.has("ArrowRight") || held.has("d") ? 1 : 0) -
        (held.has("ArrowLeft") || held.has("a") ? 1 : 0);
      const z =
        (held.has("ArrowDown") || held.has("s") ? 1 : 0) -
        (held.has("ArrowUp") || held.has("w") ? 1 : 0);
      if (x !== last[0] || z !== last[1]) {
        last = [x, z];
        void client.playerInput(x, z);
      }
    };
    const typing = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null;
      return !!t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable);
    };
    const down = (e: KeyboardEvent) => {
      if (typing(e)) return;
      const k = driveKey(e);
      if (!k) return;
      held.add(k);
      send();
      // Arrows must drive the player, not scroll a panel.
      if (k.startsWith("Arrow")) e.preventDefault();
    };
    const up = (e: KeyboardEvent) => {
      const k = driveKey(e);
      if (!k) return;
      held.delete(k);
      send();
    };
    window.addEventListener("keydown", down);
    window.addEventListener("keyup", up);
    return () => {
      window.removeEventListener("keydown", down);
      window.removeEventListener("keyup", up);
      void client.playerInput(0, 0);
    };
  }, [playing, client]);
}
