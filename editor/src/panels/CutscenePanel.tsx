//! The cutscene timeline — a cutscene as a sequence in TIME, not a bulleted list.
//!
//! WHAT WAS MISSING, AND WHERE IT ALREADY EXISTED. `metrocalk_animation::shot::Cutscene` has carried
//! a per-shot `seconds`, an ordered shot list, `effective_shot_seconds`, `playback_at` and a closed
//! framing vocabulary (six sizes × six angles × six moves) since cutscenes shipped. The editor
//! received `reads: string[]` and one total, so every one of those numbers stopped at the boundary:
//! the only edits a user could make were "append a card" and "delete a card", and the only way to
//! change a shot's length or its place in the sequence was to delete everything after it and author
//! it again. The engine knew what time it was; the editor did not.
//!
//! WHY IT IS HERE AND NOT IN THE GAMEPLAY PANEL. `EditorDocks`'s own rule: the wide workspaces open in
//! the bottom dock "because a timeline needs width", and the Cinematics block lives in a 300px left
//! column where the header cell alone would take two-thirds of the lane. The block keeps the gesture
//! that starts a cutscene — pick an object, pick a card — and this is where it is edited against a
//! clock.
//!
//! AND THE PLAYHEAD ANSWERS WITH A PICTURE. `solve_shot` has been pure in `(recipe, subject, t)`
//! since cutscenes shipped, so the engine could always produce the camera at ANY instant — and the
//! only way a user could see one was to press Play and watch the cut from its start. Preview poses
//! the viewport at the playhead through `present_cinematic_moment`, the SAME function Play runs each
//! tick, so what is composed here is the frame that gets filmed rather than an approximation of it.
//! Every framing edit re-poses at the same moment, which is what makes this an authoring loop and
//! not a viewer: change the angle and the viewport is already showing the new angle.
//!
//! ONE EDIT IS ONE UNDO. Every control here commits through the same validated command the card
//! grid does, so a length, a reorder and a re-frame are each one Ctrl-Z. That is also why the two
//! sliders hold a local draft while they are being dragged and commit once on release: a slider that
//! committed per pixel would bury the author's previous state under forty identical undo steps.

import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";
import { useSelectedId, useSummary } from "../store/projection";
import { usePlaying } from "../store/play";
import { cinemaPreviewStore, useCinemaPreview } from "../store/cinemaPreview";
import { setStatus } from "../store/ui";
import { pushToast } from "../store/toasts";
import { Icon } from "../theme/icons";
import { Button, ReadOut, SelectField, Slider, Toolbar, ToolbarGroup, ToolbarSeparator } from "../theme/primitives";
import { Callout, Field, FieldGrid } from "../theme/fields";
import { EmptyPanelState } from "../theme/workspace";
import {
  TimelineClip,
  TimelineEmpty,
  TimelineLane,
  TimelinePlayhead,
  TimelineRow,
  TimelineRuler,
  TimelineSurface,
  TimelineTrackHead,
  timelineTicks,
} from "../theme/timeline";
import { color, font, fontSize, radius, space } from "../theme/tokens";
import type { CinemaPreviewReply, CinemaReply, FramingCatalog, FramingEdit, ShotRow, ShotSpec } from "../transport/protocol";
import type { EditorClient } from "../transport/session";
import { ShotCatalogue } from "./ShotCatalogue";

const EMPTY: CinemaReply = {
  entity: null,
  shots: 0,
  seconds: 0,
  mood: "normal",
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

/** Why the pacing control refuses on an object with no cutscene — the same sentence the engine sends
 *  back, so the disabled reason and the refusal cannot drift into two different explanations. */
const EMPTY_PACING = "Pacing scales shot lengths, and this object has no shots yet — add one first.";

/** Why Preview refuses on an object with no cutscene. Same shape as `EMPTY_PACING`, and the same
 *  reason: a control that is enabled and does nothing is worse than one that says why not. */
const EMPTY_PREVIEW = "There is nothing to preview — add a shot first.";

/** The floor, in px per second. Below this a shot stops being a bar and becomes a line.
 *
 *  A CUT IS READ WHOLE. The property timeline is scrubbed one keyframe at a time and so is laid out
 *  at a fixed 160px per second and panned; a cutscene is a SHAPE — five lengths in a row — and the
 *  question it exists to answer ("is the third shot too long?") cannot be asked about a shot that is
 *  off screen. So this lane fills whatever it is given, and only starts scrolling once the sequence
 *  is long enough that fitting it would draw twelve hairlines. At 40px/s the twelve-shot, twenty-
 *  second-a-shot ceiling is a 9,600px strip, which scrolls; anything a person is likely to author
 *  fits. */
const MIN_PX_PER_SECOND = 40;

/** The lane width for a cut of `seconds` inside a scroller `available` px wide.
 *
 *  `available` is the scroller's own content box minus the sticky header column, read from the same
 *  `--mtk-track-header` the rows lay themselves out with — the number is declared once, in the
 *  stylesheet, and this is a reader of it rather than a second copy. `0` means "not laid out yet",
 *  and the fallback is the framework's own floor rather than a guess. */
function laneWidthFor(seconds: number, available: number): number {
  const fitted = available > 0 ? available : 620;
  return Math.max(fitted, Math.min(12_000, seconds * MIN_PX_PER_SECOND));
}

/** Three world coordinates, at the precision a person can hold in their head against an
 *  inspector's Transform row. Centimetres: a shot camera stands metres away, and a fourth decimal
 *  is noise dressed as precision. */
function xyz(v: readonly [number, number, number]): string {
  return v.map((n) => n.toFixed(2)).join(", ");
}

/** The word a framing value reads as, from the catalogue that also validates it. Falls back to the
 *  wire value rather than to nothing: an unlabelled option is still an option a user can pick. */
function labelOf(options: { value: string; label: string }[], value: string): string {
  return options.find((option) => option.value === value)?.label ?? value;
}

export function CutscenePanel({ client }: { client: EditorClient }) {
  const selected = useSelectedId();
  const summary = useSummary(selected ?? "");
  const playing = usePlaying();
  const fieldId = useId();

  const [specs, setSpecs] = useState<ShotSpec[]>([]);
  const [catalog, setCatalog] = useState<FramingCatalog | null>(null);
  const [cut, setCut] = useState<CinemaReply>(EMPTY);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [playhead, setPlayhead] = useState(0);
  const [busy, setBusy] = useState(false);
  const [refusal, setRefusal] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  // The value a slider currently reads while the pointer is down, before it becomes a commit.
  const [draftSeconds, setDraftSeconds] = useState<number | null>(null);
  const [draftAmount, setDraftAmount] = useState<number | null>(null);
  const scroller = useRef<HTMLDivElement | null>(null);
  const [laneRoom, setLaneRoom] = useState(0);
  const previewInfo = useCinemaPreview();
  // The solved pose, kept locally rather than in the store: the stage badge names the SHOT, and
  // three coordinates are a reading for the person editing the shot, not for someone glancing at
  // the viewport. Cleared with the preview so a stale camera cannot outlive the frame it describes.
  const [pose, setPose] = useState<CinemaPreviewReply | null>(null);
  // Previewing THIS object. The store is one state for two surfaces (this toggle and the stage
  // badge), so it can legitimately be holding a different object's cutscene while this panel is
  // looking at something else — in which case this panel's control is off, which is the truth.
  const previewing = previewInfo.active && previewInfo.entity === selected;
  // Read by `run`'s success path, which fires long after the click that started it. A ref rather
  // than a dependency because re-posing must use where the playhead IS, not where it was when the
  // edit's callback closed over it.
  const previewingRef = useRef(false);
  const playheadRef = useRef(0);
  previewingRef.current = previewing;
  playheadRef.current = playhead;

  // How much lane the panel actually has, re-read whenever the dock is resized. Without this the
  // strip is a fixed width and the last shots of an ordinary cut sit off the right-hand edge of a
  // scroller — present in the DOM, absent from the picture.
  useLayoutEffect(() => {
    const element = scroller.current;
    if (!element || typeof ResizeObserver === "undefined") return undefined;
    const measure = () => {
      const header = Number.parseFloat(
        getComputedStyle(element).getPropertyValue("--mtk-track-header"),
      );
      setLaneRoom(Math.max(0, element.clientWidth - (Number.isFinite(header) ? header : 178)));
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
    // Keyed on the shot count because that is what decides whether the timeline is MOUNTED at all —
    // the observer handles every width change after that, so re-attaching on each render would be
    // churn. It cannot be `[]`: the element does not exist on the first render of an empty cutscene.
  }, [cut.rows.length]);

  useEffect(() => {
    let live = true;
    void client
      .cinemaCatalog()
      .then((list) => live && setSpecs(list))
      .catch((e: unknown) => console.error("cinema_catalog failed", e));
    void client
      .cinemaFramingCatalog()
      .then((c) => live && setCatalog(c))
      .catch((e: unknown) => console.error("cinema_framing_catalog failed", e));
    return () => {
      live = false;
    };
  }, [client]);

  // Keyed on the entity asked for, cleared on a selection change, and re-read across a Play
  // transition and after every local mutation — the same discipline the Cinematics block uses.
  useEffect(() => {
    let live = true;
    if (!selected) {
      setCut(EMPTY);
      setActiveId(null);
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

  const rows = cut.rows;
  const duration = cut.seconds;

  // The shot the inspector is editing, found by ID rather than by index: a reorder moves a shot and
  // an index would silently follow whatever slid into its place.
  const active = useMemo(
    () => rows.find((row) => row.id === activeId) ?? null,
    [rows, activeId],
  );
  // The shot the playhead is over — a different fact from the selected one, and the reason the
  // ruler is worth scrubbing at all: it answers "what is on screen at 4.2 seconds".
  const live = useMemo(
    () =>
      rows.find(
        (row) => playhead >= row.startSeconds && playhead < row.startSeconds + row.effectiveSeconds,
      ) ?? rows[rows.length - 1] ?? null,
    [rows, playhead],
  );

  /** Stand the viewport camera at `t` on this cutscene's clock. The one place a preview starts or
   *  moves, so the store, the status line and the refusal cannot be updated by three callers in
   *  three different ways. */
  const poseAt = useCallback(
    async (id: string, t: number) => {
      const reply = await client.cinemaPreview(id, t, true).catch((e: unknown) => {
        console.error("cinema_preview failed", e);
        return null;
      });
      if (!reply) return;
      cinemaPreviewStore.getState().from(reply);
      setPose(reply.reason ? null : reply);
      if (reply.reason) {
        setRefusal(reply.reason);
        pushToast(reply.reason, "error");
      }
      setStatus(reply.message);
    },
    [client],
  );

  /** Hand the viewport back. The store is cleared FIRST and unconditionally: the badge offering the
   *  only way out must not survive a command that failed to send. */
  const endPreview = useCallback(
    async (id: string | null) => {
      const target = id ?? cinemaPreviewStore.getState().entity;
      cinemaPreviewStore.getState().reset();
      setPose(null);
      if (!target) return;
      await client.cinemaPreview(target, 0, false).catch((e: unknown) => {
        console.error("cinema_preview failed", e);
        return null;
      });
    },
    [client],
  );

  // Play takes the camera itself, and the shell hands a held preview back before it does — so the
  // store must stop claiming otherwise, or the stage carries a badge for a preview that has ended.
  useEffect(() => {
    if (playing) cinemaPreviewStore.getState().reset();
  }, [playing]);

  // A preview belongs to the object it was started on. Changing selection — or closing this panel —
  // ends it, because the alternative is a viewport locked into a shot of something the author is no
  // longer editing, with the control that would release it now unmounted.
  useEffect(() => () => void endPreview(null), [selected, endPreview]);

  const run = useCallback(
    async (action: () => Promise<CinemaReply>, label: string) => {
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
          // THE LOOP. An edit that does not move the picture is an edit the author has to imagine.
          // Re-solving at the same moment is what makes changing an angle feel like turning a
          // camera rather than filling in a form and pressing Play to find out.
          if (previewingRef.current && reply.entity) {
            void poseAt(reply.entity, playheadRef.current);
          }
        }
        setRevision((r) => r + 1);
      } catch (e) {
        console.error(`${label} failed`, e);
        pushToast(`${label} failed — please try again`, "error");
      } finally {
        setBusy(false);
        setDraftSeconds(null);
        setDraftAmount(null);
      }
    },
    [poseAt],
  );

  const editFraming = (row: ShotRow, edit: FramingEdit, label: string) => {
    if (!selected) return;
    void run(() => client.cinemaSetShotFraming(selected, row.index, edit), label);
  };

  if (!selected) {
    return (
      <div data-testid="cutscene-panel" style={{ padding: space.md }}>
        <EmptyPanelState
          compact
          icon={<Icon name="clapper" size="xl" />}
          title="No object selected"
          description="A cutscene belongs to an object. Select one, then add a shot — the camera frames it for you and the shots line up here in the order they play."
        />
      </div>
    );
  }

  const name = summary?.name ?? selected;
  const locked = busy || playing;
  const lockReason = playing
    ? "Stop Play first — a cutscene is authored, not live-edited."
    : "Another edit is still in flight — this will be available in a moment.";

  return (
    <div
      data-testid="cutscene-panel"
      style={{ display: "flex", flexDirection: "column", gap: space.sm, padding: space.sm, minWidth: 0 }}
    >
      <Toolbar aria-label="Cutscene" tight>
        <ToolbarGroup aria-label="Cutscene subject">
          <ReadOut title="The object this cutscene belongs to">{name}</ReadOut>
        </ToolbarGroup>
        {/* A CLOCK WITH NOTHING ON IT SAYS NOTHING. An empty cutscene used to read "0 shots · 0.0s",
            which states in two measurements the absence the empty state below states in a sentence —
            and `0.0s` in particular reads as a measurement OF something.

            ONE READ-OUT, NOT TWO. Three adjacent read-outs in one group rendered as
            "Weld Gun 7 5 shots 13.3 s": each is correct, and together they are a phrase nobody can
            parse. A separator between groups is what tells the eye where one fact ends. */}
        {rows.length > 0 && (
          <>
            <ToolbarSeparator />
            <ToolbarGroup aria-label="Cutscene length">
              <ReadOut title="Shots in the sequence, and how long it runs at the current pacing">
                {`${cut.shots} shot${cut.shots === 1 ? "" : "s"} · ${duration.toFixed(1)}s`}
              </ReadOut>
            </ToolbarGroup>
          </>
        )}
        <ToolbarSeparator />
        <ToolbarGroup attached aria-label="Cinematic pacing">
          {MOODS.map((mood) => (
            <Button
              key={mood.value}
              data-testid={`cutscene-mood-${mood.value}`}
              variant={cut.mood === mood.value ? "primary" : "secondary"}
              compact
              disabled={locked || rows.length === 0}
              disabledReason={rows.length === 0 ? EMPTY_PACING : lockReason}
              aria-pressed={cut.mood === mood.value}
              title={rows.length === 0 ? EMPTY_PACING : locked ? lockReason : mood.title}
              onClick={() => void run(() => client.cinemaSetMood(selected, mood.value), `${mood.label} pacing`)}
            >
              {mood.label}
            </Button>
          ))}
        </ToolbarGroup>
        {live && (
          <>
            <ToolbarSeparator />
            <ToolbarGroup aria-label="Playhead">
              <ReadOut title="Where the playhead is, and which shot is on screen there">
                {`${playhead.toFixed(1)}s · shot ${live.index + 1} of ${rows.length}`}
              </ReadOut>
            </ToolbarGroup>
          </>
        )}
        <ToolbarSeparator />
        <ToolbarGroup aria-label="Shot preview">
          <Button
            data-testid="cutscene-preview"
            variant={previewing ? "primary" : "secondary"}
            compact
            aria-pressed={previewing}
            disabled={locked || rows.length === 0}
            disabledReason={rows.length === 0 ? EMPTY_PREVIEW : lockReason}
            title={
              rows.length === 0
                ? EMPTY_PREVIEW
                : locked
                  ? lockReason
                  : previewing
                    ? "Give the viewport back to the editor camera"
                    : "Stand the viewport camera where the playhead is — the same frame Play films"
            }
            onClick={() => {
              if (previewing) void endPreview(selected);
              else void poseAt(selected, playhead);
            }}
          >
            <Icon name="camera" size="md" /> Preview
          </Button>
        </ToolbarGroup>
      </Toolbar>

      {rows.length === 0 ? (
        <EmptyPanelState
          compact
          icon={<Icon name="camera" size="xl" />}
          title={`${name} has no cutscene yet`}
          description="Pick a shot below. Each card is a whole framing decision, so the first click already looks good — then set its length, its order and its framing here."
        />
      ) : (
        <TimelineSurface
          laneWidth={laneWidthFor(duration, laneRoom)}
          scrollRef={scroller}
          data-testid="cutscene-timeline"
        >
          <TimelineRuler
            label="Cutscene"
            duration={duration}
            currentTick={playhead}
            // The ruler divides the cut into ten EQUAL parts, and a 13.3s cut divides at 1.33s. Rounded
            // to whole seconds those divisions read "0 1 3 4 5 7 8 9 11 12 13" — a ruler with three
            // numbers missing, which is what it looked like in the first capture. One decimal, with a
            // trailing zero dropped, says where the marks really are.
            ticks={timelineTicks(duration, (value) => `${Number(value.toFixed(1))}s`)}
            onScrub={(tick) => {
              setPlayhead(tick);
              const under = rows.find(
                (row) => tick >= row.startSeconds && tick < row.startSeconds + row.effectiveSeconds,
              );
              if (under) setActiveId(under.id);
              // Only while previewing. A ruler click is a reading gesture as often as a camera one,
              // and taking the viewport from an author who did not ask for it is the surprise this
              // toggle exists to prevent.
              if (previewing) void poseAt(selected, tick);
            }}
            data-testid="cutscene-ruler"
          />
          <TimelineRow variant="track" data-testid="cutscene-track">
            <TimelineTrackHead
              name="Shots"
              title={`${rows.length} shots, in the order they play`}
              meta={`${rows.length} · ${duration.toFixed(1)}s`}
            />
            <TimelineLane>
              {rows.map((row) => (
                <TimelineClip
                  key={row.id}
                  data-testid="cutscene-clip"
                  start={row.startSeconds}
                  length={row.effectiveSeconds}
                  duration={duration}
                  label={`${row.index + 1} · ${labelOf(catalog?.sizes ?? [], row.size)} ${labelOf(catalog?.motions ?? [], row.motion).toLowerCase()}`}
                  title={row.reads}
                  meta={`${row.effectiveSeconds.toFixed(1)}s`}
                  selected={row.id === activeId}
                  live={row.id === live?.id}
                  onClick={() => {
                    setActiveId(row.id);
                    setPlayhead(row.startSeconds);
                    if (previewing) void poseAt(selected, row.startSeconds);
                  }}
                />
              ))}
              <TimelinePlayhead tick={playhead} duration={duration} />
            </TimelineLane>
          </TimelineRow>
        </TimelineSurface>
      )}

      {/* WHERE THE CAMERA ACTUALLY IS, and only while something is standing there.
          Progressive disclosure in its plainest form: authoring a shot needs five words, and
          checking one sometimes needs three coordinates — a lens height that reads wrong, a target
          that is not on the part, a near plane about to clip. The numbers exist (the solver hands
          them back with every pose) and until now they crossed the boundary and stopped. They are
          `font.mono` because they are read digit by digit against an inspector's Transform, and
          they are not editable: the pose is SOLVED from the five framing words, and a nudgeable eye
          would be a second, contradictory way to say where the camera goes. */}
      {previewing && pose && (
        <div
          data-testid="cutscene-preview-pose"
          style={{
            display: "flex",
            flexWrap: "wrap",
            alignItems: "baseline",
            gap: `${space.xxs}px ${space.md}px`,
            padding: `${space.xxs}px ${space.sm}px`,
            borderRadius: radius.sm,
            background: color.info.bg,
            border: `1px solid ${color.info.border}`,
            fontSize: fontSize.meta,
            color: color.text.secondary,
          }}
        >
          {/* THE SENTENCE ONLY WHEN IT SAYS SOMETHING NEW. What the playhead is over and what the
              inspector is editing are different facts, and clicking a clip makes them the same one —
              so repeating the shot's sentence here printed it twice, forty pixels apart, under a
              heading that already names the shot. It comes back the moment they diverge, which is
              exactly when a reader needs to be told the picture is not the shot they are editing.
              Found by READING the capture; no selector assertion can see a stutter. */}
          <span style={{ color: color.info.text }}>
            {pose.blending
              ? `Transition into shot ${(pose.shotIndex ?? 0) + 1}`
              : pose.shotIndex === active?.index
                ? "On screen"
                : `On screen: ${pose.reads}`}
          </span>
          <span title="Where the camera stands, in world units">
            eye <span style={{ font: font.mono }}>{xyz(pose.eye)}</span>
          </span>
          <span aria-hidden style={{ color: color.text.faint }}>{"·"}</span>
          <span title="The point it is aimed at, in world units">
            looking at <span style={{ font: font.mono }}>{xyz(pose.lookAt)}</span>
          </span>
          <span aria-hidden style={{ color: color.text.faint }}>{"·"}</span>
          <span title="Vertical field of view">
            <span style={{ font: font.mono }}>{pose.fovDeg.toFixed(0)}°</span> lens
          </span>
        </div>
      )}

      {rows.length > 0 && !active && (
        <TimelineEmpty>
          <span style={{ fontSize: fontSize.meta, color: color.text.muted }}>
            Click a shot on the timeline to set its length, its order and how it is framed.
          </span>
        </TimelineEmpty>
      )}

      {active && catalog && (
        <section data-testid="cutscene-shot-editor" style={{ display: "grid", gap: space.xs, minWidth: 0 }}>
          <h3
            style={{
              margin: 0,
              font: font.ui,
              fontSize: fontSize.body,
              color: color.text.primary,
            }}
          >
            Shot {active.index + 1} of {rows.length}
          </h3>
          <p data-testid="cutscene-shot-reads" style={{ margin: 0, fontSize: fontSize.meta, color: color.text.secondary }}>
            {active.reads}
          </p>

          {/* Capped rather than spanning the dock. `FieldGrid` is `auto-fit` + `1fr`, so five fields in
              a 2000px dock become five 400px tracks holding a select that reads "Wide" — the same
              stretched-control shape ADR-147 found in the Model workspace, arriving from the other
              direction. The cap is a line-length decision this panel owns; the grid's behaviour is
              the design system's and stays untouched. */}
          <FieldGrid minColumn={190} aria-label="Shot framing" style={{ maxWidth: 1100 }}>
            <Field
              label="Length"
              htmlFor={`${fieldId}-seconds`}
              unit="s"
              help={
                cut.mood === "normal"
                  ? `Between ${catalog.minSeconds}s and ${catalog.maxSeconds}s.`
                  : `Authored length. ${MOODS.find((m) => m.value === cut.mood)?.label} pacing plays it for ${active.effectiveSeconds.toFixed(1)}s.`
              }
              disabled={locked}
            >
              <Slider
                id={`${fieldId}-seconds`}
                data-testid="cutscene-seconds"
                value={draftSeconds ?? active.seconds}
                min={catalog.minSeconds}
                max={catalog.maxSeconds}
                step={0.1}
                disabled={locked}
                aria-label="Shot length in seconds"
                title={locked ? lockReason : undefined}
                onChange={(event) => setDraftSeconds(Number(event.currentTarget.value))}
                onPointerUp={() => {
                  if (draftSeconds !== null && draftSeconds !== active.seconds) {
                    void run(
                      () => client.cinemaSetShotSeconds(selected, active.index, draftSeconds),
                      "Shot length",
                    );
                  } else {
                    setDraftSeconds(null);
                  }
                }}
                onKeyUp={() => {
                  if (draftSeconds !== null && draftSeconds !== active.seconds) {
                    void run(
                      () => client.cinemaSetShotSeconds(selected, active.index, draftSeconds),
                      "Shot length",
                    );
                  }
                }}
              />
              <output
                htmlFor={`${fieldId}-seconds`}
                data-testid="cutscene-seconds-value"
                style={{ fontSize: fontSize.meta, color: color.text.secondary, minWidth: 40, textAlign: "right" }}
              >
                {(draftSeconds ?? active.seconds).toFixed(1)}
              </output>
            </Field>

            <Field
              label="Size"
              htmlFor={`${fieldId}-size`}
              help="How much of the frame the subject fills."
              disabled={locked}
            >
              <SelectField
                id={`${fieldId}-size`}
                data-testid="cutscene-size"
                value={active.size}
                disabled={locked}
                title={locked ? lockReason : undefined}
                onChange={(event) => editFraming(active, { size: event.currentTarget.value }, "Shot size")}
              >
                {catalog.sizes.map((option) => (
                  <option key={option.value} value={option.value} title={option.blurb}>
                    {option.label}
                  </option>
                ))}
              </SelectField>
            </Field>

            <Field
              label="Angle"
              htmlFor={`${fieldId}-angle`}
              help="Where the camera stands, relative to the subject's own facing."
              disabled={locked}
            >
              <SelectField
                id={`${fieldId}-angle`}
                data-testid="cutscene-angle"
                value={active.angle}
                disabled={locked}
                title={locked ? lockReason : undefined}
                onChange={(event) => editFraming(active, { angle: event.currentTarget.value }, "Camera angle")}
              >
                {catalog.angles.map((option) => (
                  <option key={option.value} value={option.value} title={option.blurb}>
                    {option.label}
                  </option>
                ))}
              </SelectField>
            </Field>

            <Field
              label="Move"
              htmlFor={`${fieldId}-motion`}
              help="What the camera does over the shot."
              disabled={locked}
            >
              <SelectField
                id={`${fieldId}-motion`}
                data-testid="cutscene-motion"
                value={active.motion}
                disabled={locked}
                title={locked ? lockReason : undefined}
                onChange={(event) => editFraming(active, { motion: event.currentTarget.value }, "Camera move")}
              >
                {catalog.motions.map((option) => (
                  <option key={option.value} value={option.value} title={option.blurb}>
                    {option.label}
                  </option>
                ))}
              </SelectField>
            </Field>

            {(() => {
              // A locked-off shot has no strength to set — the dial would move and change nothing,
              // which `<ux_quality>` 6 calls an inert control. The list of moves this is true for
              // comes from the engine, not from a string compared here.
              const still = catalog.stillMotions.includes(active.motion);
              return (
                <Field
                  label="Move strength"
                  htmlFor={`${fieldId}-amount`}
                  help={
                    still
                      ? `A ${labelOf(catalog.motions, active.motion).toLowerCase()} shot does not move, so it has no strength to set. Choose another move first.`
                      : "How far the move travels — a push-in of 0.35 closes 35% of the distance."
                  }
                  disabled={locked || still}
                >
                  <Slider
                    id={`${fieldId}-amount`}
                    data-testid="cutscene-amount"
                    value={draftAmount ?? active.amount}
                    min={0}
                    max={1}
                    step={0.05}
                    disabled={locked || still}
                    aria-label="Move strength"
                    title={
                      still
                        ? `A ${labelOf(catalog.motions, active.motion).toLowerCase()} shot does not move, so it has no strength to set.`
                        : locked
                          ? lockReason
                          : undefined
                    }
                    onChange={(event) => setDraftAmount(Number(event.currentTarget.value))}
                    onPointerUp={() => {
                      if (draftAmount !== null && draftAmount !== active.amount) {
                        editFraming(active, { amount: draftAmount }, "Move strength");
                      } else {
                        setDraftAmount(null);
                      }
                    }}
                    onKeyUp={() => {
                      if (draftAmount !== null && draftAmount !== active.amount) {
                        editFraming(active, { amount: draftAmount }, "Move strength");
                      }
                    }}
                  />
                  <output
                    htmlFor={`${fieldId}-amount`}
                    data-testid="cutscene-amount-value"
                    style={{ fontSize: fontSize.meta, color: color.text.secondary, minWidth: 40, textAlign: "right" }}
                  >
                    {(draftAmount ?? active.amount).toFixed(2)}
                  </output>
                </Field>
              );
            })()}
          </FieldGrid>

          <Toolbar aria-label="Shot order" tight raised={false}>
            <ToolbarGroup attached aria-label="Move this shot">
              <Button
                data-testid="cutscene-earlier"
                variant="secondary"
                compact
                disabled={locked || active.index === 0}
                disabledReason={
                  active.index === 0 ? "This is already the first shot." : lockReason
                }
                title={active.index === 0 ? "This is already the first shot." : "Play this shot one place earlier"}
                onClick={() =>
                  void run(
                    () => client.cinemaMoveShot(selected, active.index, active.index - 1),
                    "Move shot earlier",
                  )
                }
              >
                <Icon name="prev" size="md" /> Earlier
              </Button>
              <Button
                data-testid="cutscene-later"
                variant="secondary"
                compact
                disabled={locked || active.index === rows.length - 1}
                disabledReason={
                  active.index === rows.length - 1 ? "This is already the last shot." : lockReason
                }
                title={
                  active.index === rows.length - 1
                    ? "This is already the last shot."
                    : "Play this shot one place later"
                }
                onClick={() =>
                  void run(
                    () => client.cinemaMoveShot(selected, active.index, active.index + 1),
                    "Move shot later",
                  )
                }
              >
                Later <Icon name="next" size="md" />
              </Button>
            </ToolbarGroup>
            <ToolbarSeparator />
            <Button
              data-testid="cutscene-remove"
              variant="danger"
              compact
              disabled={locked}
              disabledReason={lockReason}
              title={locked ? lockReason : `Remove: ${active.reads}`}
              onClick={() => {
                setActiveId(null);
                void run(() => client.cinemaRemoveShot(selected, active.index), "Remove shot");
              }}
            >
              <Icon name="close" size="md" /> Remove shot
            </Button>
          </Toolbar>
        </section>
      )}

      {cut.problems.length > 0 && (
        <div role="status" style={{ display: "grid", gap: space.xxs }}>
          {cut.problems.map((problem, i) => (
            // Four identical shots emit three byte-identical jump-cut warnings, so the string alone
            // is not a key.
            // eslint-disable-next-line react/no-array-index-key -- see above
            <Callout key={`${problem}-${i}`} tone="warn" data-testid="cutscene-problem">
              {problem}
            </Callout>
          ))}
        </div>
      )}

      {refusal && (
        <Callout tone="danger" role="status" data-testid="cutscene-refusal">
          {refusal}
        </Callout>
      )}

      <section style={{ display: "grid", gap: space.xs, minWidth: 0 }}>
        <h3 style={{ margin: 0, font: font.ui, fontSize: fontSize.body, color: color.text.primary }}>
          Add a shot
        </h3>
        <ShotCatalogue
          specs={specs}
          minColumn={150}
          disabled={locked || cut.shots >= (catalog?.maxShots ?? Infinity)}
          disabledReason={
            catalog && cut.shots >= catalog.maxShots
              ? `A cutscene holds at most ${catalog.maxShots} shots — remove one first.`
              : lockReason
          }
          onPick={(kind) => void run(() => client.cinemaAddShot(selected, kind), "Add shot")}
        />
        <p style={{ margin: 0, fontSize: fontSize.meta, color: color.text.muted }}>
          Turn on Preview to stand the viewport camera on the playhead as you edit, or press Play to
          watch the whole cut — either way the camera hands back when you are done.
        </p>
      </section>
    </div>
  );
}

export default CutscenePanel;
