//! Visual effects - pick a word, pick a moment, press Play.
//!
//! The competition makes an effect a separate asset in a separate graph editor, dropped into the scene
//! and triggered from code. Here it is one click on the object itself: Fire, Sparks, Explosion. The
//! second dropdown is what makes it a GAME effect rather than decoration - "when it's picked up",
//! "when it's hit" - which writes an ordinary field a rule flips, so the engine's closed action
//! vocabulary never grew a verb to gain explosions.
//!
//! Warnings (over budget, every layer glowing, all one-shots) are shown next to the layers, in plain
//! language, and never block the click.

import { useEffect, useState } from "react";
import { useSelectedId, useSummary } from "../store/projection";
import { usePlaying } from "../store/play";
import { setStatus } from "../store/ui";
import { pushToast } from "../store/toasts";
import { Button, SelectField } from "../theme/primitives";
import { color, font, fontSize, radius, space } from "../theme/tokens";
import type { EffectSpec, VfxProbe, VfxReply } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

const sectionH3 = {
  margin: `0 0 ${space.xs}px`,
  font: font.ui,
  fontSize: fontSize.body,
  color: color.text.primary,
} as const;

const metaText = {
  fontSize: fontSize.meta,
  color: color.text.muted,
} as const;

const EMPTY: VfxReply = {
  entity: null,
  layers: 0,
  particles: 0,
  reads: [],
  problems: [],
  message: "",
  reason: null,
};

/** When an effect runs. A one-shot card wants a moment; a continuous one runs the whole time. */
const TRIGGERS = [
  { value: "always", label: "the whole time" },
  { value: "whenCollected", label: "when it's picked up" },
  { value: "whenHit", label: "when it's hit" },
  { value: "onCue", label: "when a rule says so" },
] as const;

export function VfxSection({ client }: { client: EditorClient }) {
  const selected = useSelectedId();
  const summary = useSummary(selected ?? "");
  const playing = usePlaying();
  const [specs, setSpecs] = useState<EffectSpec[]>([]);
  const [fx, setFx] = useState<VfxReply>(EMPTY);
  const [trigger, setTrigger] = useState<string>("always");
  const [busy, setBusy] = useState(false);
  const [refusal, setRefusal] = useState<string | null>(null);
  // Bumped by every mutation, so the list re-reads from the engine rather than trusting the reply.
  // That also makes Ctrl-Z honest: undo happens outside this panel, and the panel's own toast is what
  // tells the user to press it.
  const [revision, setRevision] = useState(0);
  const [live, setLive] = useState<VfxProbe | null>(null);

  // The loop this panel opens is "press Play to see it" — so it has to CLOSE here, where the author is
  // looking, the way RolesSection closes its own with a live score. 500 ms is the house chrome cadence:
  // never a per-frame read, and only while Play is running.
  useEffect(() => {
    if (!playing) {
      setLive(null);
      return undefined;
    }
    let alive = true;
    const read = () => {
      void client.vfxProbe().then((p) => alive && setLive(p));
    };
    read();
    const timer = setInterval(read, 500);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, [client, playing]);

  useEffect(() => {
    let live = true;
    client
      .vfxCatalog()
      .then((list) => live && setSpecs(list))
      .catch((e: unknown) => console.error("vfx_catalog failed", e));
    return () => {
      live = false;
    };
  }, [client]);

  // Keyed on the entity the reply was ASKED for. Without this, selecting A then B quickly lets A's
  // slower reply land under B's name — the panel would confidently show the wrong object's effects.
  // `playing` is a dependency because a rule flips `Vfx.playing` during Play, so the panel must
  // re-read on both edges rather than showing pre-Play state for the whole run.
  useEffect(() => {
    let live = true;
    if (!selected) {
      setFx(EMPTY);
      setRefusal(null);
      return undefined;
    }
    // A refusal is about the object it was refused for; carrying it to the next selection libels it.
    setRefusal(null);
    void client
      .vfxList(selected)
      .then((info) => live && setFx(info))
      .catch(() => live && setFx(EMPTY));
    return () => {
      live = false;
    };
  }, [client, selected, playing, revision]);

  async function run(action: () => Promise<VfxReply>, verb: string) {
    if (!selected) return;
    const forEntity = selected;
    setBusy(true);
    setRefusal(null);
    try {
      const reply = await action();
      // The selection can change while this is in flight; a reply about the previous object must not
      // be painted under the new one's name.
      if (forEntity !== selected) return;
      if (reply.reason) {
        setRefusal(reply.reason);
        pushToast(reply.reason, "error");
        setStatus(`${verb} refused: ${reply.reason}`);
      } else {
        setFx(reply);
        pushToast(`${reply.message} · Ctrl-Z to undo`, "success");
        setStatus(reply.message);
      }
      if (forEntity === selected) setRevision((r) => r + 1);
    } catch (e) {
      console.error(`${verb} failed`, e);
      pushToast(`${verb} failed — please try again`, "error");
    } finally {
      setBusy(false);
    }
  }

  return (
    <section data-testid="vfx-section">
      <h3 style={sectionH3}>Effects</h3>

      {playing && live && (
        <div
          data-testid="vfx-live"
          style={{
            display: "flex",
            gap: space.md,
            alignItems: "baseline",
            padding: `${space.xs}px ${space.md}px`,
            background: color.accent.subtle,
            border: `1px solid ${color.accent.border}`,
            borderRadius: radius.md,
            marginBottom: space.sm,
            fontSize: fontSize.meta,
            color: color.text.secondary,
          }}
        >
          <span style={{ fontWeight: 700, color: color.accent.base }}>
            {live.total} particle{live.total === 1 ? "" : "s"}
          </span>
          <span>
            {live.total === 0
              ? "nothing is running right now"
              : `${live.additive} glowing${live.soft > 0 ? `, ${live.soft} hazy` : ""}${
                  live.bursts > 0 ? ` · ${live.bursts} one-shot${live.bursts === 1 ? "" : "s"}` : ""
                }`}
          </span>
        </div>
      )}

      {selected ? (
        <div style={{ display: "grid", gap: space.xs }}>
          <div style={{ fontSize: fontSize.meta, color: color.text.secondary }}>
            {summary?.name ?? selected}
            {fx.layers > 0 && (
              <span style={{ color: color.accent.base }}>
                {" · "}
                {fx.layers} effect{fx.layers === 1 ? "" : "s"} {"·"} {fx.particles} particles
              </span>
            )}
          </div>

          <label style={{ ...metaText, display: "grid", gap: space.xxs }}>
            Run the next effect
            <SelectField
              data-testid="vfx-trigger"
              aria-label="When the next effect you add should run"
              value={trigger}
              disabled={busy || playing}
              onChange={(e) => setTrigger(e.target.value)}
            >
              {TRIGGERS.map((t) => (
                <option key={t.value} value={t.value}>
                  {t.label}
                </option>
              ))}
            </SelectField>
          </label>

          <div
            role="group"
            aria-label="Add an effect"
            style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: space.xs }}
          >
            {specs.map((spec) => (
              <Button
                key={spec.kind}
                data-testid={`fx-${spec.kind}`}
                variant="secondary"
                compact
                disabled={busy || playing}
                title={
                  playing
                    ? "Stop Play first — effects are authored, not live-edited"
                    : `${spec.blurb}. Adds: ${spec.adds} — one Ctrl-Z removes it`
                }
                onClick={() =>
                  selected &&
                  void run(() => client.vfxAdd(selected, spec.kind, trigger), `Add ${spec.label}`)
                }
              >
                {spec.icon} {spec.label}
              </Button>
            ))}
          </div>

          {fx.reads.length > 0 && (
            <ul
              data-testid="vfx-layers"
              style={{
                margin: `${space.xs}px 0 0`,
                padding: 0,
                listStyle: "none",
                display: "grid",
                gap: space.xxs,
                fontSize: fontSize.meta,
                color: color.text.secondary,
              }}
            >
              {fx.reads.map((line, i) => (
                // eslint-disable-next-line react/no-array-index-key -- a layer stack IS its order
                <li
                  key={`${line}-${i}`}
                  data-testid="vfx-layer-row"
                  style={{ display: "flex", justifyContent: "space-between", gap: space.xs }}
                >
                  <span>{line}</span>
                  <Button
                    data-testid={`vfx-remove-${i}`}
                    variant="ghost"
                    compact
                    disabled={busy || playing}
                    // A row of six buttons all announced as "✕" tells a screen-reader user nothing
                    // about WHICH effect they are about to delete. The name carries the layer.
                    aria-label={`Remove effect: ${line}`}
                    title={`Remove: ${line}`}
                    onClick={() =>
                      selected && void run(() => client.vfxRemove(selected, i), "Remove effect")
                    }
                  >
                    {"✕"}
                  </Button>
                </li>
              ))}
            </ul>
          )}

          {/* ONE live region containing the list. N regions inserted already-populated are commonly
              not announced at all, and several at once announce in an unpredictable order. */}
          {fx.problems.length > 0 && (
            <div role="status" style={{ display: "grid", gap: space.xxs }}>
              {fx.problems.map((problem) => (
            <div
              key={problem}
              data-testid="vfx-problem"
              style={{
                padding: `${space.xs}px ${space.md}px`,
                background: color.warn.bg,
                border: `1px solid ${color.warn.border}`,
                borderRadius: radius.md,
                fontSize: fontSize.meta,
                color: color.warn.text,
              }}
            >
              {"⚠"} {problem}
            </div>
              ))}
            </div>
          )}

          {fx.layers > 0 && (
            <div data-testid="vfx-hint" style={metaText}>
              Press Play to see it {"—"} effects run during Play and leave nothing behind on
              Stop.
            </div>
          )}
        </div>
      ) : (
        <div data-testid="vfx-empty" style={metaText}>
          Select an object, then give it an effect {"—"} it catches fire, sparks when hit, or
          pops when taken.
        </div>
      )}

      {refusal && (
        <div
          data-testid="vfx-refusal"
          role="status"
          style={{
            marginTop: space.sm,
            padding: `${space.sm}px ${space.md}px`,
            background: color.danger.bg,
            border: `1px solid ${color.danger.border}`,
            borderRadius: radius.md,
            fontSize: fontSize.meta,
            color: color.danger.text,
          }}
        >
          {refusal}
        </div>
      )}
    </section>
  );
}

export default VfxSection;
