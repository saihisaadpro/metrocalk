//! Cinematics — a cutscene is a SHOT LIST, and each shot is a sentence.
//!
//! Select an object, click "Hero shot", and one undoable commit gives you a framed camera move you can
//! watch immediately by pressing Play. Nobody types a distance, a quaternion or a keyframe: the card
//! carries the whole framing decision, the engine solves the pose fresh every tick against the
//! subject's LIVE position, and the shot list reads back as English underneath.
//!
//! Continuity warnings (a jump cut, opening tight, a rushed shot) are shown where the shots are, not
//! hidden in a validator — the "legible cost" clause of the UX-quality contract.

import { useEffect, useState } from "react";
import { useSelectedId, useSummary } from "../store/projection";
import { usePlaying } from "../store/play";
import { setStatus } from "../store/ui";
import { pushToast } from "../store/toasts";
import { Icon } from "../theme/icons";
import { Button } from "../theme/primitives";
import { color, fontSize, radius, space } from "../theme/tokens";
import { DisclosureSection, useSubjectDisclosure } from "../theme/workspace";
import type { CinemaReply, ShotSpec } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

const metaText = {
  fontSize: fontSize.meta,
  color: color.text.muted,
} as const;

const EMPTY: CinemaReply = {
  entity: null,
  shots: 0,
  seconds: 0,
  mood: "normal",
  reads: [],
  problems: [],
  message: "",
  reason: null,
};

const MOODS = [
  { value: "calm", label: "Calm", title: "Measured, cinematic pacing — 2.5× the authored shot length" },
  { value: "normal", label: "Normal", title: "The authored shot length, unchanged" },
  { value: "tense", label: "Tense", title: "Urgent pacing — 0.75× the authored shot length" },
] as const;

export function CinemaSection({ client }: { client: EditorClient }) {
  const selected = useSelectedId();
  const summary = useSummary(selected ?? "");
  const playing = usePlaying();
  const [specs, setSpecs] = useState<ShotSpec[]>([]);
  const [cut, setCut] = useState<CinemaReply>(EMPTY);
  const [busy, setBusy] = useState(false);
  const [refusal, setRefusal] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  const [rolling, setRolling] = useState(false);

  // With two armed cutscenes exactly one owns the camera, and the panel used to tell BOTH objects
  // "the camera takes over" — true for one of them. Report what the engine actually did.
  useEffect(() => {
    if (!playing) {
      setRolling(false);
      return undefined;
    }
    let alive = true;
    const read = () => {
      void client.cameraProbe().then((p) => alive && setRolling(p.cinematic));
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
      .cinemaCatalog()
      .then((list) => live && setSpecs(list))
      .catch((e: unknown) => console.error("cinema_catalog failed", e));
    return () => {
      live = false;
    };
  }, [client]);

  // See the note in VfxSection: keyed on the entity asked for, cleared on selection change, and
  // re-read across a Play transition and after every local mutation.
  useEffect(() => {
    let live = true;
    if (!selected) {
      setCut(EMPTY);
      setRefusal(null);
      return undefined;
    }
    setRefusal(null);
    void client
      .cinemaList(selected)
      .then((info) => live && setCut(info))
      .catch(() => live && setCut(EMPTY));
    return () => {
      live = false;
    };
  }, [client, selected, playing, revision]);

  async function run(action: () => Promise<CinemaReply>, label: string) {
    if (!selected) return;
    setBusy(true);
    setRefusal(null);
    try {
      const reply = await action();
      if (reply.reason) {
        setRefusal(reply.reason);
        pushToast(reply.reason, "error");
        setStatus(`${label} refused: ${reply.reason}`);
      } else {
        setCut(reply);
        pushToast(`${reply.message} · Ctrl-Z to undo`, "success");
        setStatus(reply.message);
      }
      setRevision((r) => r + 1);
    } catch (e) {
      console.error(`${label} failed`, e);
      pushToast(`${label} failed — please try again`, "error");
    } finally {
      setBusy(false);
    }
  }

  // The group's CONDITION on its own header line — the shared status slot, not a sentence underneath.
  const subject = summary?.name ?? selected;
  const condition = selected
    ? cut.shots > 0
      ? `${subject} · ${cut.shots} shot${cut.shots === 1 ? "" : "s"} · ${cut.seconds.toFixed(1)}s`
      : String(subject)
    : "needs a selection";

  const disclosure = useSubjectDisclosure(selected);

  return (
    <DisclosureSection
      data-testid="cinema-section"
      title="Cinematics"
      summary={condition}
      density="compact"
      landmark={false}
      // Open during Play: `cinema-live` says whether a cutscene has the camera right now, and that
      // read-out answers "why did the view just move" — a question a folded section cannot answer.
      open={playing || disclosure.open}
      onOpenChange={disclosure.onOpenChange}
    >

      {playing && (
        <div
          data-testid="cinema-live"
          style={{
            padding: `${space.xs}px ${space.md}px`,
            background: rolling ? color.accent.subtle : color.bg.inset,
            border: `1px solid ${rolling ? color.accent.border : color.border.subtle}`,
            borderRadius: radius.md,
            marginBottom: space.sm,
            fontSize: fontSize.meta,
            color: rolling ? color.accent.base : color.text.muted,
          }}
        >
          {rolling
            ? "A cutscene has the camera right now"
            : "The camera is free — no cutscene is running"}
        </div>
      )}

      {/* The subject and the shot count are in the section's own header now — printing them again
          here is what made three groups in one column read as three restatements of the selection. */}
      <div style={{ display: "grid", gap: space.xs }}>
          <div
            role="group"
            aria-label="Cinematic pacing"
            style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: space.xs }}
          >
            {MOODS.map((mood) => (
              <Button
                key={mood.value}
                data-testid={`cinema-mood-${mood.value}`}
                variant={cut.mood === mood.value ? "primary" : "secondary"}
                compact
                disabled={busy || playing || !selected}
                aria-pressed={cut.mood === mood.value}
                title={
                  playing
                    ? "Stop Play first — pacing is authored, not live-edited"
                    : !selected
                      ? `Select an object first, then this sets its pacing. ${mood.title}`
                      : mood.title
                }
                onClick={() => selected && void run(
                  () => client.cinemaSetMood(selected, mood.value),
                  `${mood.label} pacing`,
                )}
              >
                {mood.label}
              </Button>
            ))}
          </div>

          {/* AN EMPTY CATALOGUE MUST NOT RENDER AN EMPTY GROUP — the same defect as `VfxSection`'s,
              in the section directly beneath it, because a group that draws its own controls draws
              its own empty state too. `cinema_catalog` answers with nothing wherever the cinematics
              sub-engine is not present (the browser build is exactly that case), and this section
              then showed a pacing row above a labelled group containing no cards: three controls
              that set the pacing of a cutscene there is no way to add. Say so instead. */}
          {specs.length === 0 ? (
            <div data-testid="cinema-unavailable" style={metaText}>
              No shots are available here — the cinematics sub-engine runs in the desktop editor.
            </div>
          ) : (
          <div
            role="group"
            aria-label="Add a shot"
            style={{ display: "grid", gridTemplateColumns: "repeat(2, 1fr)", gap: space.xs }}
          >
            {specs.map((spec) => (
              <Button
                key={spec.kind}
                data-testid={`shot-${spec.kind}`}
                variant="secondary"
                compact
                disabled={busy || playing || !selected}
                title={
                  playing
                    ? "Stop Play first — shots are authored, not live-edited"
                    : !selected
                      ? `Select an object first, then ${spec.label} frames it. ${spec.blurb}`
                      : `${spec.blurb}. Adds: ${spec.adds} — one Ctrl-Z removes it`
                }
                onClick={() => selected && void run(() => client.cinemaAddShot(selected, spec.kind), spec.label)}
              >
                <Icon name={spec.kind} size="md" fallback="camera" /> {spec.label}
              </Button>
            ))}
          </div>
          )}

          {cut.reads.length > 0 && (
            <ol
              data-testid="cinema-shots"
              style={{
                margin: `${space.xs}px 0 0`,
                padding: `0 0 0 ${space.lg}px`,
                display: "grid",
                gap: space.xxs,
                fontSize: fontSize.meta,
                color: color.text.secondary,
              }}
            >
              {cut.reads.map((line, i) => (
                // eslint-disable-next-line react/no-array-index-key -- a shot list IS its order
                <li
                  key={`${line}-${i}`}
                  data-testid="cinema-shot-row"
                  style={{ display: "flex", justifyContent: "space-between", gap: space.xs }}
                >
                  <span>{line}</span>
                  <Button
                    data-testid={`cinema-remove-${i}`}
                    variant="ghost"
                    compact
                    disabled={busy || playing}
                    aria-label={`Remove shot ${i + 1}: ${line}`}
                    title={`Remove: ${line}`}
                    onClick={() =>
                      selected &&
                      void run(() => client.cinemaRemoveShot(selected, i), "Remove shot")
                    }
                  >
                    <Icon name="close" size="sm" />
                  </Button>
                </li>
              ))}
            </ol>
          )}

          {cut.problems.length > 0 && (
            <div role="status" style={{ display: "grid", gap: space.xxs }}>
              {cut.problems.map((problem, i) => (
            <div
              // Four identical shots emit three byte-identical jump-cut warnings, so the string alone
              // is not a key.
              // eslint-disable-next-line react/no-array-index-key -- see above
              key={`${problem}-${i}`}
              data-testid="cinema-problem"
              style={{
                padding: `${space.xs}px ${space.md}px`,
                background: color.warn.bg,
                border: `1px solid ${color.warn.border}`,
                borderRadius: radius.md,
                fontSize: fontSize.meta,
                color: color.warn.text,
              }}
            >
              <Icon name="warning" size="sm" /> {problem}
            </div>
              ))}
            </div>
          )}

          {selected && cut.shots > 0 && (
            <div data-testid="cinema-hint" style={metaText}>
              Press Play to watch it — the camera takes over, then hands back.
            </div>
          )}
          {!selected && (
            <div data-testid="cinema-empty" style={metaText}>
              Select an object, then pick a shot — the camera frames it for you.
            </div>
          )}
      </div>

      {refusal && (
        <div
          data-testid="cinema-refusal"
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
    </DisclosureSection>
  );
}

export default CinemaSection;
