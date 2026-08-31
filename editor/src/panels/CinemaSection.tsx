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
import { Callout } from "../theme/fields";
import { Icon } from "../theme/icons";
import { Button } from "../theme/primitives";
import { ChoiceCard, ChoiceGrid, DisclosureSection } from "../theme/workspace";
import { color, fontSize, space } from "../theme/tokens";
import { DEFAULT_RENDER_SETTINGS } from "../transport/protocol";
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
  delivery: "viewport",
  render: DEFAULT_RENDER_SETTINGS,
  reads: [],
  rows: [],
  problems: [],
  message: "",
  reason: null,
};

const MOODS = [
  { value: "calm", label: "Calm", title: "Measured, cinematic pacing — 2.5× the authored shot length" },
  { value: "normal", label: "Normal", title: "The authored shot length, unchanged" },
  { value: "tense", label: "Tense", title: "Urgent pacing — 0.75× the authored shot length" },
] as const;

/** Why pacing refuses with nothing to pace — the engine's own sentence, stated once. */
const EMPTY_PACING = "Pacing scales shot lengths, and this object has no shots yet — add one first.";

export interface CinemaSectionProps {
  client: EditorClient;
  /** Open the Cutscene timeline in the Animate dock. Absent in surfaces that have no dock to open
   *  (the shot harness renders this block on its own), and the link is then not offered — an
   *  enabled control that goes nowhere is worse than no control. */
  onOpenTimeline?: () => void;
}

export function CinemaSection({ client, onOpenTimeline }: CinemaSectionProps) {
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

  return (
    // See `RolesSection` for why this is a `DisclosureSection` and not an `<h3>` over a column of
    // hand-rolled boxes. Closed by default: a cutscene is the second thing an author reaches for,
    // and the summary keeps reporting the shot count while it is shut.
    <DisclosureSection
      data-testid="cinema-section"
      title="Cinematics"
      icon={<Icon name="clapper" size="md" />}
      summary={cut.shots > 0 ? `${cut.shots} shot${cut.shots === 1 ? "" : "s"} · ${cut.seconds.toFixed(1)}s` : specs.length > 0 ? `${specs.length} shots to choose from` : undefined}
      storageKey="gameplay.cinematics"
      tone="card"
      density="compact"
      defaultOpen={false}
      landmark={false}
    >
      <div style={{ display: "grid", gap: space.sm, minWidth: 0 }}>
      {playing && (
        <Callout
          data-testid="cinema-live"
          tone={rolling ? "info" : "neutral"}
          icon={<Icon name="camera" size="sm" />}
        >
          {rolling
            ? "A cutscene has the camera right now"
            : "The camera is free — no cutscene is running"}
        </Callout>
      )}

      {/* The subject line, and — with nothing selected — the one sentence that says what to do. The
          shot cards below stay drawn and disabled, so the vocabulary is visible before the guess. */}
      <div style={{ fontSize: fontSize.meta, color: color.text.secondary }} data-testid="cinema-empty">
        {selected ? (
          <>
            {summary?.name ?? selected}
            {cut.shots > 0 && (
              <span style={{ color: color.accent.base }}>
                {" "}
                · {cut.shots} shot{cut.shots === 1 ? "" : "s"} · {cut.seconds.toFixed(1)}s
              </span>
            )}
          </>
        ) : (
          "Select an object, then pick a shot — the camera frames it for you."
        )}
      </div>

      <div
        role="group"
        aria-label="Cinematic pacing"
        style={{ display: "grid", gridTemplateColumns: "repeat(3, minmax(0, 1fr))", gap: space.xs }}
      >
        {MOODS.map((mood) => (
          <Button
            key={mood.value}
            data-testid={`cinema-mood-${mood.value}`}
            variant={cut.mood === mood.value ? "toggle" : "secondary"}
            active={cut.mood === mood.value}
            compact
            disabled={busy || playing || !selected || cut.shots === 0}
            aria-pressed={cut.mood === mood.value}
            disabledReason={
              playing
                ? "Stop Play first — pacing is authored, not live-edited."
                : !selected
                  ? `Select an object first. ${mood.title}`
                  : cut.shots === 0
                    ? EMPTY_PACING
                    : "Another edit is still in flight — this will be available in a moment."
            }
            title={
              playing
                ? "Stop Play first — pacing is authored, not live-edited"
                : !selected
                  ? `Select an object first. ${mood.title}`
                  : cut.shots === 0
                    ? EMPTY_PACING
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

      {specs.length > 0 && (
      <ChoiceGrid label="Add a shot">
        {specs.map((spec) => (
          <ChoiceCard
            key={spec.kind}
            data-testid={`shot-${spec.kind}`}
            icon={<Icon name={spec.kind} size="md" fallback="camera" />}
            label={spec.label}
            description={spec.blurb}
            disabled={busy || playing || !selected}
            disabledReason={
              playing
                ? "Stop Play first — shots are authored, not live-edited"
                : !selected
                  ? `Select an object first. ${spec.blurb}`
                  : undefined
            }
            title={`${spec.blurb}. Adds: ${spec.adds} — one Ctrl-Z removes it`}
            onSelect={() => selected && void run(() => client.cinemaAddShot(selected, spec.kind), spec.label)}
          />
        ))}
      </ChoiceGrid>
      )}

      {selected && cut.rows.length > 0 && (
        <ol
          data-testid="cinema-shots"
          style={{
            margin: 0,
            padding: `0 0 0 ${space.lg}px`,
            display: "grid",
            gap: space.xxs,
            fontSize: fontSize.meta,
            color: color.text.secondary,
          }}
        >
          {cut.rows.map((row) => (
            <li
              key={row.id}
              data-testid="cinema-shot-row"
              style={{ display: "flex", justifyContent: "space-between", gap: space.xs }}
            >
              <span>{row.reads}</span>
              <Button
                data-testid={`cinema-remove-${row.index}`}
                variant="ghost"
                compact
                icon
                disabled={busy || playing}
                disabledReason={
                  playing
                    ? "Stop Play first — shots are authored, not live-edited."
                    : "Another edit is still in flight — this will be available in a moment."
                }
                aria-label={`Remove shot ${row.index + 1}: ${row.reads}`}
                title={`Remove: ${row.reads}`}
                onClick={() =>
                  selected &&
                  void run(() => client.cinemaRemoveShot(selected, row.index), "Remove shot")
                }
              >
                <Icon name="close" size="sm" />
              </Button>
            </li>
          ))}
        </ol>
      )}

      {selected && cut.problems.length > 0 && (
        <div role="status" style={{ display: "grid", gap: space.xs }}>
          {cut.problems.map((problem, i) => (
            // Four identical shots emit three byte-identical jump-cut warnings, so the string alone
            // is not a key.
            // eslint-disable-next-line react/no-array-index-key -- see above
            <Callout key={`${problem}-${i}`} data-testid="cinema-problem" tone="warn">
              {problem}
            </Callout>
          ))}
        </div>
      )}

      {selected && cut.shots > 0 && (
        <div data-testid="cinema-hint" style={metaText}>
          Press Play to watch it — the camera takes over, then hands back.
        </div>
      )}

      {/* THE LENGTHS AND THE ORDER ARE EDITED ON A CLOCK, and a clock does not fit in a 300px
          column — `EditorDocks`'s own rule is that the wide workspaces open in the bottom dock
          "because a timeline needs width". This block keeps the gesture that starts a cutscene; the
          link is the rest of the loop rather than a sentence telling the user to go and find it. */}
      {selected && cut.shots > 0 && onOpenTimeline && (
        <Button
          data-testid="cinema-open-timeline"
          variant="secondary"
          compact
          title="Set each shot's length, its order and how it is framed, against the cutscene clock"
          onClick={onOpenTimeline}
        >
          <Icon name="clapper" size="md" /> Edit on the cutscene timeline
        </Button>
      )}

      {refusal && (
        <Callout data-testid="cinema-refusal" tone="danger" role="status">
          {refusal}
        </Callout>
      )}
      </div>
    </DisclosureSection>
  );
}

export default CinemaSection;
