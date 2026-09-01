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
import { subjectAimStore, useSubjectAim } from "../store/subjectAim";
import { setStatus } from "../store/ui";
import { pushToast } from "../store/toasts";
import { Icon } from "../theme/icons";
import { Button, ReadOut, SelectField, Slider, SliderField, Toolbar, ToolbarGroup, ToolbarSeparator } from "../theme/primitives";
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
import { frameGuideStore, useFrameGuide } from "../store/frameGuide";
import { color, font, fontSize, radius, space } from "../theme/tokens";
import { DEFAULT_RENDER_SETTINGS } from "../transport/protocol";
import type { CinemaPreviewReply, CinemaReply, DeliveryFrame, FramingCatalog, FramingEdit, PathSample, ShotRow, ShotSpec, StandAtReply } from "../transport/protocol";
import type { EditorClient } from "../transport/session";
import { RenderDialog } from "./RenderDialog";
import { ShotCatalogue } from "./ShotCatalogue";
import { SubjectPicker } from "./SubjectPicker";

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

/** Why the pacing control refuses on an object with no cutscene — the same sentence the engine sends
 *  back, so the disabled reason and the refusal cannot drift into two different explanations. */
const EMPTY_PACING = "Pacing scales shot lengths, and this object has no shots yet — add one first.";

/** Why the delivery frame refuses on an object with no cutscene. Same sentence as the engine's. */
const EMPTY_DELIVERY =
  "A delivery frame is what the shots are composed for, and this object has no shots yet — add one first.";

/** Why the frame guide refuses on a cutscene delivered to the stage's own shape. "Match viewport" is
 *  the ABSENCE of a delivery frame, so its guide would be bars around the whole stage — a control
 *  that is enabled and draws nothing is the inert-control failure, so it says why instead. */
const VIEWPORT_GUIDE =
  "\"Match viewport\" already is the stage's shape — pick a delivery frame and the guide will show it.";

/** Why Preview refuses on an object with no cutscene. Same shape as `EMPTY_PACING`, and the same
 *  reason: a control that is enabled and does nothing is worse than one that says why not. */
const EMPTY_PREVIEW = "There is nothing to preview — add a shot first.";

/** Why Render refuses on an object with no cutscene. Same shape, same reason: there is nothing to
 *  film, and the control says so rather than opening a dialog whose every number would be zero. */
const EMPTY_RENDER = "There is nothing to render — add a shot first.";

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
/** ADR-201 — the walk's track, said in words.
 *
 *  The dots ARE the information — five verdicts laid out along the move, so a push-in that is clear
 *  for its first half reads differently from one that is buried throughout — and a picture with no
 *  text behind it is that information withheld from everyone not looking at it. Written as one
 *  sentence rather than five, because five is a list of coordinates and one is a shape.
 */
function walkTrackReads(path: PathSample[]): string {
  if (path.length === 0) return "The engine has not judged this move.";
  const bad = path.filter((sample) => !sample.acceptable);
  const n = path.length;
  if (bad.length === 0) {
    return `The engine found nothing in the way at any of the ${n} moments it judged along this move.`;
  }
  if (bad.length === n) {
    return `The engine objected at every one of the ${n} moments it judged along this move.`;
  }
  const at = bad.map((sample) => `${Math.round(sample.progress * 100)}%`).join(", ");
  return `Of the ${n} moments the engine judged along this move it objected at ${at}, and found the rest clear.`;
}

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
  /** Whether the render dialog is open. ADR-175. */
  const [rendering, setRendering] = useState(false);
  const [draftAmount, setDraftAmount] = useState<number | null>(null);
  // ADR-201 — WHERE THE AUTHOR IS STANDING, when they are standing in a shot. Set by "Take me
  // there" and by every step of the walk that follows it; `null` whenever the stage camera is not on
  // a shot's path, which is the only state in which a scrub over that path would be a lie.
  const [standing, setStanding] = useState<StandAtReply & { index: number } | null>(null);
  // The slider's own position, kept locally so the control stays under the pointer while the engine
  // answers. The READ-OUT beside it is the engine's, so the two can legitimately differ for a frame:
  // one is where the author has dragged to, the other is where the camera has got to.
  const [walkAt, setWalkAt] = useState(0);
  const walkPending = useRef<number | null>(null);
  const walkInFlight = useRef(false);
  const scroller = useRef<HTMLDivElement | null>(null);
  const [laneRoom, setLaneRoom] = useState(0);
  const previewInfo = useCinemaPreview();
  // The aim in flight, if any. Started here, LIVED on the stage, and committed back here — this
  // panel owns the transaction so a re-aim by pointing and a re-aim by list are the same one-undo
  // edit rather than two paths that can drift apart.
  const aim = useSubjectAim();
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

  // ADR-193 — THE FRAME GUIDE, kept in step with what would actually be delivered.
  //
  // The narrowing is the whole content-awareness of the feature: the author says once that they want
  // to see the frame, and the stage works out whether there IS one — a cutscene with shots, delivered
  // to something other than the stage's own shape, with nothing already holding the camera. So the
  // guide appears when it means something and stays out of the way when it does not, with no second
  // control to remember.
  const guideWanted = useFrameGuide().wanted;
  const guideDelivery: DeliveryFrame | null =
    guideWanted && !previewing && rows.length > 0 && cut.delivery !== "viewport" ? cut.delivery : null;
  // The badge's label is the ENGINE's name for the frame, read out of the framing catalog the picker
  // above is populated from — never a second table in the front end, which would drift silently
  // (a badge reading "16:9" over a scope guide is wrong in a way nothing fails on).
  const guideLabel = labelOf(catalog?.deliveries ?? [], guideDelivery ?? "viewport");
  useEffect(() => {
    frameGuideStore.getState().setDrawn(guideDelivery ? { key: guideDelivery, label: guideLabel } : null);
    void client.setFrameGuide(guideDelivery).catch((e: unknown) => {
      console.error("stage_frame_guide failed", e);
    });
  }, [client, guideDelivery, guideLabel]);

  // Unmount ONLY — deliberately not the cleanup of the effect above, which would clear the guide and
  // re-ask for it on every delivery change and flash the stage between the two. Closing the panel
  // does have to clear it: the toggle would be gone, and the way out would then be the stage badge
  // alone. (It is there, and it works — but a guide with no owner is not a state to leave behind.)
  useEffect(
    () => () => {
      frameGuideStore.getState().setDrawn(null);
      void client.setFrameGuide(null).catch(() => undefined);
    },
    [client],
  );

  const run = useCallback(
    async (
      action: () => Promise<CinemaReply>,
      label: string,
      // ADR-192 — what to do INSTEAD of the ordinary re-pose, for the one edit whose whole point is
      // to change where the camera is: "shoot from this view" has to start a preview the author was
      // not already running, at the moment that shot opens.
      after?: (reply: CinemaReply) => void,
    ) => {
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
          if (after) after(reply);
          else if (previewingRef.current && reply.entity) {
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

  // RE-AIMING IS THE SAME KIND OF EDIT AS RE-FRAMING, and goes through the same one-undo commit —
  // which is what makes "try it on the whole assembly" a decision the author can take back. `run`
  // re-poses the preview at the current playhead afterwards, so the consequence of the choice is on
  // the stage before the picker has finished closing.
  const editSubject = (row: ShotRow, subject: string) => {
    if (!selected || subject === row.subject) return;
    void run(() => client.cinemaSetShotSubject(selected, row.index, subject), "What the shot frames");
  };

  // ADR-192 — SHOOT FROM THIS VIEW. The one gesture in this panel that does not name a value: the
  // author orbits until the frame is the one they want and says *that one*. No pose crosses the
  // boundary — the engine reads its own camera — so the stored shot cannot be a view the renderer
  // was never standing at.
  //
  // AND THE ANSWER COMES BACK ON THE STAGE. The author was orbiting, not previewing, so the
  // ordinary re-pose in `run` would do nothing and the only evidence of the gesture would be a
  // toast. Previewing at the shot's own opening instant puts the DELIVERY-FRAMED result up
  // immediately, inside the bars — which is the difference between "we stored something" and "here
  // is your shot", and the only way the author finds out that a 16:9 stage is not a 2.39:1 frame
  // while there is still something they can do about it.
  const shootFromView = (row: ShotRow) => {
    if (!selected) return;
    void run(
      () => client.cinemaSetShotCamera(selected, row.index),
      "Shoot from this view",
      (reply) => {
        if (!reply.entity) return;
        const at = reply.rows.find((shot) => shot.index === row.index)?.openSeconds ?? row.openSeconds;
        setPlayhead(at);
        void poseAt(reply.entity, at);
      },
    );
  };

  // ADR-200 — TAKE ME THERE. The inverse of "Shoot from this view", and the half that was missing:
  // the engine could say *there is nowhere good to film shot 2 from* and the only fix it could offer
  // was "frame it yourself", starting the author from wherever the viewport happened to be. On a
  // 262 m import that is a manual orbit to a part they cannot see, to redo by hand the fifty-four
  // placement search the engine has just finished. This stands them AT the engine's own best
  // attempt, at the instant the warning is about, so the next click is the one that fixes it.
  //
  // THE PREVIEW IS HANDED BACK FIRST, not refused over. An author reads these warnings while
  // previewing — that is when a bad frame is visible — and the cutscene camera holds the viewport,
  // so standing the orbit camera anywhere would be overwritten on the next tick. Ending the preview
  // is what makes the stage theirs to orbit, which is the whole point of arriving there.
  //
  // AND IT SELECTS THE SHOT. The control the sentence sends them to next ("Shoot from this view")
  // acts on the shot the inspector is editing, so arriving at shot 2's placement with shot 5 still
  // selected would put the author one click from storing this view onto the wrong shot.
  const takeMeThere = async (index: number) => {
    if (!selected || busy) return;
    setBusy(true);
    try {
      if (previewingRef.current) await endPreview(selected);
      const row = rows.find((r) => r.index === index);
      if (row) setActiveId(row.id);
      const reply = await client.cinemaStandAtShot(selected, index);
      if (reply.reason) {
        setStanding(null);
        setRefusal(reply.reason);
        pushToast(reply.reason, "error");
        return;
      }
      setRefusal(null);
      setStanding({ ...reply, index });
      setWalkAt(reply.progress);
      setStatus(reply.message);
      pushToast(reply.message, "info");
    } catch (e) {
      console.error("Take me there failed", e);
      pushToast("Take me there failed — please try again", "error");
    } finally {
      setBusy(false);
    }
  };

  // ADR-201 — WALK THE MOVE. "Take me there" lands at ONE instant of a camera path and says which:
  // *"— 60% through its move"*. The other four fifths of that path were reachable only through a
  // preview, which HOLDS the viewport and is overwritten on the next tick — so the frames an author
  // most needs to judge were the ones they could not stand in and could not orbit from.
  //
  // The engine could always solve any instant. Nothing outside it could ask.
  //
  // LATEST WINS, ONE IN FLIGHT. A drag emits an event per pixel and each one is an IPC round trip
  // that reads an occlusion BVH; queuing them would walk the camera through every intermediate
  // position seconds after the pointer stopped. This keeps only the newest request and sends it as
  // soon as the previous answer lands, which is what makes the camera track the thumb.
  const walkTo = useCallback(
    async (index: number, progress: number) => {
      if (!selected) return;
      walkPending.current = progress;
      if (walkInFlight.current) return;
      walkInFlight.current = true;
      try {
        while (walkPending.current !== null) {
          const at = walkPending.current;
          walkPending.current = null;
          const reply = await client.cinemaStandAtShot(selected, index, at);
          if (reply.reason) {
            // A refusal here is Play or a preview taking the camera mid-walk. The strip goes with
            // it: a scrub whose camera someone else is holding moves nothing.
            setStanding(null);
            setRefusal(reply.reason);
            pushToast(reply.reason, "error");
            return;
          }
          setRefusal(null);
          setStanding({ ...reply, index });
          setStatus(reply.message);
        }
      } catch (e) {
        console.error("Walk the move failed", e);
        pushToast("Walking the move failed — please try again", "error");
      } finally {
        walkInFlight.current = false;
      }
    },
    [client, selected],
  );

  // THE WALK ENDS WHEN THE CAMERA STOPS BEING THE AUTHOR'S. Play and a preview both take the
  // viewport, and a strip still offering to stand somewhere would be a control whose every step is
  // overwritten on the next tick. A framing edit ends it too — for the opposite reason: the camera
  // has not moved, but the PATH under it has, so the track would be describing a shot that no longer
  // exists. Selecting another object ends it because the cutscene on screen is a different one.
  useEffect(() => {
    setStanding(null);
  }, [selected, playing, revision]);
  useEffect(() => {
    if (previewing) setStanding(null);
  }, [previewing]);

  const useTheCardAgain = (row: ShotRow) => {
    if (!selected) return;
    void run(() => client.cinemaClearShotCamera(selected, row.index), "Use the card again");
  };

  // ADR-195 — KEEP THE SUBJECT FRAMED. A placed camera is a tripod: this puts a panning head on it,
  // and the head is the only thing that moves. Nothing is composed on this side — the offset that
  // preserves the author's framing is resolved in the engine against where the subject is standing
  // at the moment they press it, so switching it on while nothing has moved changes nothing at all.
  //
  // Re-posed like `shootFromView`, and for the same reason: the change is invisible on a still stage
  // and completely visible the moment the shot plays.
  const keepFramed = (row: ShotRow, track: boolean) => {
    if (!selected) return;
    void run(
      () => client.cinemaSetShotTracking(selected, row.index, track),
      track ? "Keep the subject framed" : "Lock the camera off",
      (reply) => {
        if (!reply.entity) return;
        const at = reply.rows.find((shot) => shot.index === row.index)?.openSeconds ?? row.openSeconds;
        setPlayhead(at);
        void poseAt(reply.entity, at);
      },
    );
  };

  // AND POINTING AT IT IS THE SAME EDIT. `store/subjectAim` carries only the CHOICE a click on the
  // stage made; the commit is this panel's own `run`, so aiming by pointing and aiming from the list
  // are one transaction, one toast, one re-pose and one Ctrl-Z rather than two paths that drift.
  //
  // The shot is the one the aim STARTED on, not whichever card is open now: an aim can take several
  // seconds of orbiting to line up, and landing it on a different shot because the author clicked
  // another card meanwhile is a re-frame nobody asked for.
  useEffect(() => {
    const picked = aim.picked;
    if (!picked || aim.owner !== selected || aim.shotIndex === null || !selected) return;
    const index = aim.shotIndex;
    subjectAimStore.getState().taken(picked.seq);
    const row = cut.rows.find((shot) => shot.index === index);
    if (row && row.subject === picked.subject) {
      // NOT A NO-OP SILENTLY. Pointing at what the shot already films is a legitimate gesture with a
      // legitimate answer, and a click that produced nothing at all reads as a click that missed.
      pushToast(`Shot ${index + 1} already frames ${picked.name}`, "info");
      setStatus(`Shot ${index + 1} already frames ${picked.name}`);
      return;
    }
    void run(() => client.cinemaSetShotSubject(selected, index, picked.subject), "What the shot frames");
  }, [aim.picked, aim.owner, aim.shotIndex, selected, cut.rows, client, run]);

  // AN AIM BELONGS TO THE PANEL THAT STARTED IT. Selecting something else switches which cutscene is
  // on screen, and Play takes editing away entirely — in both cases the stage would be left
  // intercepting clicks for a shot nobody is looking at any more. Same shape as the preview's own
  // teardown directly above, and for the same reason.
  useEffect(() => {
    if (playing && subjectAimStore.getState().active) subjectAimStore.getState().cancel();
  }, [playing]);
  useEffect(
    () => () => {
      if (subjectAimStore.getState().active) subjectAimStore.getState().cancel();
    },
    [selected],
  );

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
        {/* THE FRAME THE SHOTS ARE COMPOSED FOR. Beside pacing because it is the other property of
            the whole cut, and because it changes every shot at once: the solver fits the subject
            against an aspect ratio, so "Wide" means a different distance in scope than in vertical.
            Until this control existed the only aspect ratio available was the author's stage — so
            opening a dock silently re-composed the film. */}
        <ToolbarSeparator />
        <ToolbarGroup aria-label="Delivery frame">
          <SelectField
            data-testid="cutscene-delivery"
            aria-label="Delivery frame"
            value={cut.delivery}
            disabled={locked || rows.length === 0 || !catalog}
            title={
              rows.length === 0
                ? EMPTY_DELIVERY
                : locked
                  ? lockReason
                  : "The frame this cutscene is delivered in. The stage draws its bars while a shot is on screen."
            }
            onChange={(event) =>
              void run(
                () => client.cinemaSetDelivery(selected, event.currentTarget.value as DeliveryFrame),
                "Delivery frame",
              )
            }
          >
            {(catalog?.deliveries ?? []).map((option) => (
              <option key={option.value} value={option.value} title={option.blurb}>
                {option.label}
              </option>
            ))}
          </SelectField>
          {/* ADR-193 — SEE THE FRAME YOU ARE COMPOSING FOR, while you are composing for it. Beside the
              picker because it is the same fact seen from the stage: the select says what the film
              is, this draws it. Without it the author frames a 2.39:1 shot inside whatever shape the
              docks have left, presses "Shoot from this view", and meets the real frame for the first
              time in the render. */}
          <Button
            data-testid="cutscene-frame-guide"
            variant={guideDelivery ? "primary" : "secondary"}
            compact
            aria-pressed={guideWanted}
            disabled={locked || rows.length === 0 || cut.delivery === "viewport"}
            disabledReason={
              rows.length === 0 ? EMPTY_DELIVERY : cut.delivery === "viewport" ? VIEWPORT_GUIDE : lockReason
            }
            title={
              rows.length === 0
                ? EMPTY_DELIVERY
                : cut.delivery === "viewport"
                  ? VIEWPORT_GUIDE
                  : locked
                    ? lockReason
                    : guideWanted
                      ? "Stop drawing the delivery frame on the stage"
                      : "Draw the delivery frame on the stage, so what you frame is what gets filmed"
            }
            onClick={() => frameGuideStore.getState().setWanted(!guideWanted)}
          >
            <Icon name="frame" size="md" /> Frame guide
          </Button>
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
          {/* RENDER SITS BESIDE PREVIEW because they are the same verb at two commitments: one poses
              the camera for a look, the other keeps what it sees. Putting the render in the File menu
              instead would have separated the decision (which shot, how long, what frame) from the
              only surface that knows any of it. */}
          <Button
            data-testid="cutscene-render"
            variant="secondary"
            compact
            disabled={locked || rows.length === 0}
            disabledReason={rows.length === 0 ? EMPTY_RENDER : lockReason}
            title={
              rows.length === 0
                ? EMPTY_RENDER
                : locked
                  ? lockReason
                  : "Write this cutscene out as a numbered sequence of PNG files"
            }
            onClick={() => setRendering(true)}
          >
            <Icon name="clapper" size="md" /> Render…
          </Button>
        </ToolbarGroup>
      </Toolbar>

      {rendering && (
        <RenderDialog
          open
          onClose={() => setRendering(false)}
          client={client}
          entity={selected}
          name={name}
          cut={cut}
          activeShotIndex={active?.index ?? null}
          deliveryLabel={labelOf(catalog?.deliveries ?? [], cut.delivery)}
          // ADR-190 — the dialog sends the settings change and shows any refusal in place; the cut it
          // changed lives here. Adopting the reply is what keeps the NEXT open seeded from what was
          // actually stored, and the toast is the same "Ctrl-Z to undo" every other cinematics edit
          // raises — these are document edits, and a form control that quietly makes one without
          // saying it is undoable is a form control the author will not trust with a decision.
          onSettingsSaved={(reply, announce) => {
            setCut(reply);
            // ANNOUNCED FOR AN EDIT, SILENT FOR AN ADOPTION. Changing a picker is an authoring
            // gesture and gets the same undo-able toast every other cinematics edit gets; the dialog
            // remembering the folder the author just chose in a picker, as a render starts, is not a
            // second decision and a toast about it would arrive on top of the progress bar.
            if (announce) pushToast(`${reply.message} · Ctrl-Z to undo`, "success");
            setStatus(reply.message);
            setRevision((r) => r + 1);
          }}
        />
      )}

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
                  // ADR-192 — "Placed" where the size would be, for a shot the author framed by eye.
                  // The size is a leftover card there, and a lane reading "Wide pulling out" over a
                  // hand-framed close-up is the same untruth `describe_shot` stopped telling.
                  label={`${row.index + 1} · ${row.camera ? "Placed" : labelOf(catalog?.sizes ?? [], row.size)} ${labelOf(catalog?.motions ?? [], row.motion).toLowerCase()}`}
                  title={row.reads}
                  // ONLY WHEN IT IS NOT THE OWNER. Every clip in an ordinary cutscene films the
                  // object the cutscene hangs on, and captioning all five of them with the same
                  // name is five repetitions of the heading above them. The moment one shot films
                  // something else, the difference is the thing worth reading — and the lane is
                  // where a reader is looking when they ask "which of these is the wide?".
                  meta={
                    row.subject === selected
                      ? `${row.effectiveSeconds.toFixed(1)}s`
                      : `${row.effectiveSeconds.toFixed(1)}s · ${row.subjectName}`
                  }
                  selected={row.id === activeId}
                  live={row.id === live?.id}
                  onClick={() => {
                    // OPENING a shot is not the same instant as where it STARTS. Every shot after
                    // the first opens with a transition, and at its start the transition weight is
                    // zero — the frame there is the END of the shot before. Clicking clip 3 and
                    // being shown shot 2 is not a subtlety: measured on the packaged `.exe`,
                    // re-framing shot 3 from that position moved the camera 0.0000 units, because
                    // shot 3 contributed nothing to the picture. `openSeconds` is ABSOLUTE and comes
                    // from `Cutscene::opens_at` — no arithmetic here, because `start + blend` lands
                    // a hair inside the window in f32 and reports a 99.99999% transition.
                    const at = row.openSeconds;
                    setActiveId(row.id);
                    setPlayhead(at);
                    if (previewing) void poseAt(selected, at);
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
          {/* Only when the frame is not the stage. "Match viewport" is the absence of a delivery
              frame, and printing it would be a fourth measurement of nothing. */}
          {cut.delivery !== "viewport" && (
            <>
              <span aria-hidden style={{ color: color.text.faint }}>{"·"}</span>
              <span title="The frame these numbers were solved for — the bars on the stage are its edges">
                composed for{" "}
                <span style={{ font: font.mono }}>
                  {labelOf(catalog?.deliveries ?? [], cut.delivery)}
                </span>
              </span>
            </>
          )}
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

          {/* ADR-192 — WHERE THIS SHOT FILMS FROM. Above the framing grid, because it decides whether
              two of the controls in it mean anything: a placed camera is not one more axis of the
              card, it is the answer that replaces two of them. */}
          <Toolbar aria-label="Where this shot films from" tight raised={false}>
            <ToolbarGroup attached aria-label="Camera placement">
              <Button
                data-testid="cutscene-shoot-here"
                variant={active.camera ? "secondary" : "primary"}
                compact
                disabled={locked}
                disabledReason={lockReason}
                title={
                  locked
                    ? lockReason
                    : active.camera
                      ? "Replace this shot's camera with the view on the stage right now"
                      : "Film this shot from exactly the view on the stage right now, instead of from its card"
                }
                onClick={() => shootFromView(active)}
              >
                <Icon name="camera" size="md" />{" "}
                {active.camera ? "Re-shoot from this view" : "Shoot from this view"}
              </Button>
              {active.camera && (
                <Button
                  data-testid="cutscene-use-card"
                  variant="secondary"
                  compact
                  disabled={locked}
                  disabledReason={lockReason}
                  title={
                    locked
                      ? lockReason
                      : `Go back to the card: ${labelOf(catalog.sizes, active.size).toLowerCase()}, ${labelOf(catalog.angles, active.angle).toLowerCase()}`
                  }
                  onClick={() => useTheCardAgain(active)}
                >
                  <Icon name="undo" size="md" /> Use the card again
                </Button>
              )}
            </ToolbarGroup>
            <ToolbarSeparator />
            {/* ADR-200 — THE ROUND TRIP, closed. "Shoot from this view" reads the stage into the
                shot; this reads the shot back onto the stage.

                OUTSIDE the attached pair, deliberately. Those two are one decision in two states —
                the shot is placed, or it is on its card — and this is not a third state: it is a
                CAMERA MOVE and changes nothing in the document. Attaching it would draw it as a
                segment of a control it cannot be a segment of.

                And it is offered for every shot, not only a warned one: it is otherwise impossible
                to see where a card shot the ENGINE placed actually films from without previewing,
                which takes the camera away again. */}
            <Button
              data-testid="cutscene-stand-here"
              variant="secondary"
              compact
              disabled={locked}
              disabledReason={lockReason}
              title={
                locked
                  ? lockReason
                  : // ADR-201 — AND THE WALK IS NAMED HERE, because it is the only place an author
                    // who has not pressed this yet can find out it exists: the strip appears once
                    // they are standing on the path, which is after this click. `stillMotions` is
                    // the engine's own list of moves that go nowhere, so the promise is not made
                    // about a shot that cannot keep it.
                    catalog.stillMotions.includes(active.motion)
                    ? active.camera
                      ? "Stand the stage camera at this shot's own camera"
                      : "Stand the stage camera where the engine films this shot from"
                    : active.camera
                      ? "Stand the stage camera at this shot's own camera, then walk its move from there"
                      : "Stand the stage camera where the engine films this shot from, then walk its move from there"
              }
              onClick={() => void takeMeThere(active.index)}
            >
              <Icon name="waypoint" size="md" /> Take me there
            </Button>
            {active.camera && (
              <>
                <ToolbarSeparator />
                {/* ADR-195 — THE ONE THING A PLACED CAMERA COULD NOT DO. Beside the gesture that
                    placed it, because it is the second half of the same decision: where the camera
                    stands, and whether it turns. A toggle and not a checkbox — it belongs to the
                    toolbar that owns the placement, and its pressed state is the answer. */}
                <Button
                  data-testid="cutscene-keep-framed"
                  variant="toggle"
                  compact
                  active={active.camera.track !== null}
                  aria-pressed={active.camera.track !== null}
                  disabled={locked}
                  disabledReason={lockReason}
                  title={
                    locked
                      ? lockReason
                      : active.camera.track !== null
                        ? `The camera stays where you put it and only its aim follows ${active.subjectName}. Press to lock it off.`
                        : `Keep ${active.subjectName} framed as it moves — the camera stays exactly where you put it and only its aim follows.`
                  }
                  onClick={() => keepFramed(active, active.camera?.track === null)}
                >
                  <Icon name="frame" size="md" /> Keep {active.subjectName} framed
                </Button>
                <ToolbarSeparator />
                {/* The NUMBERS, not a badge. "Placed" is a claim; an eye and an aim the author can
                    read against the preview's own read-out above is evidence. */}
                <ReadOut
                  data-testid="cutscene-placed-pose"
                  title={
                    active.camera.track === null
                      ? "Where this shot's camera stands, the point it aims at, and the lens it was framed through"
                      : `Where this shot's camera stands and the lens it was framed through. Its aim is not a fixed point any more — it follows ${active.subjectName}.`
                  }
                >
                  eye <span style={{ font: font.mono }}>{xyz(active.camera.eye)}</span>
                  {" · "}
                  {/* WHEN THE HEAD IS ON, `lookAt` IS NOT WHERE IT LOOKS. It is the framing the
                      offset was taken from, and printing it as "looking at" would be a coordinate
                      the camera stops using the moment the subject moves — the exact class of
                      caption ADR-192 removed from the shot sentence. */}
                  {active.camera.track === null ? (
                    <>looking at <span style={{ font: font.mono }}>{xyz(active.camera.lookAt)}</span></>
                  ) : (
                    <>aim follows {active.subjectName}</>
                  )}
                  {" · "}<span style={{ font: font.mono }}>{active.camera.fovDeg.toFixed(0)}°</span> lens
                </ReadOut>
              </>
            )}
          </Toolbar>

          {/* ADR-201 — WALK THE MOVE. The strip appears once the author is standing on this shot's
              path and that path goes somewhere, which is the only state in which it means anything:
              a scrub over a camera Play is holding moves nothing, and a scrub over a locked-off shot
              moves and changes nothing.

              WHY IT IS NOT ALWAYS ON SCREEN. It is the second half of "Take me there", not a third
              framing control — the author has to BE on the path before walking it is a thing they
              can do. That is also why it sits directly under the toolbar that put them there rather
              than in the framing grid: the grid edits the shot, and this edits nothing at all.

              THE TRACK IS THE ENGINE'S OWN JUDGEMENT, drawn at the five instants the placement
              search actually scored. Before this, the whole of it reached the author as one number
              in one sentence — so a push-in that is clear for its first half and buried for its
              second read exactly like one that is buried throughout. */}
          {standing && standing.index === active.index && standing.moving && (
            <section
              data-testid="cutscene-walk"
              aria-label={`Walk shot ${active.index + 1}'s move`}
              style={{ display: "grid", gap: space.xxs, maxWidth: 1100, minWidth: 0 }}
            >
              <SliderField
                label="Walk the move"
                data-testid="cutscene-walk-slider"
                ariaLabel={`How far through shot ${active.index + 1}'s move to stand`}
                value={walkAt}
                min={0}
                max={1}
                step={0.01}
                disabled={locked}
                title={
                  locked
                    ? lockReason
                    : `Stand the camera anywhere along this shot's ${standing.travel.toFixed(1)} m move — orbit from wherever you stop, and shoot from there.`
                }
                valueLabel={`${Math.round(walkAt * 100)}%`}
                onChange={(event) => {
                  const at = Number(event.currentTarget.value);
                  setWalkAt(at);
                  void walkTo(active.index, at);
                }}
              />
              <div
                data-testid="cutscene-walk-track"
                role="img"
                aria-label={walkTrackReads(standing.path)}
                title={walkTrackReads(standing.path)}
                style={{ position: "relative", height: 10, marginInline: space.xs }}
              >
                <span
                  aria-hidden
                  style={{
                    position: "absolute",
                    insetInline: 0,
                    top: 4,
                    height: 2,
                    borderRadius: radius.sm,
                    background: color.border.subtle,
                  }}
                />
                {standing.path.map((sample) => (
                  <span
                    key={sample.progress}
                    aria-hidden
                    data-testid="cutscene-walk-mark"
                    data-progress={sample.progress}
                    data-clear={sample.acceptable ? "yes" : "no"}
                    style={{
                      position: "absolute",
                      left: `${sample.progress * 100}%`,
                      top: 1,
                      width: 8,
                      height: 8,
                      marginLeft: -4,
                      borderRadius: "50%",
                      background: sample.acceptable ? color.success.solid : color.warn.solid,
                      // The worst instant is ringed rather than recoloured: it is already one of the
                      // objectionable ones, and a third colour would read as a third verdict.
                      outline:
                        Math.abs(sample.progress - standing.worst) < 1e-6
                          ? `2px solid ${color.accent.border}`
                          : undefined,
                    }}
                  />
                ))}
              </div>
              <div style={{ display: "flex", alignItems: "center", gap: space.xs, flexWrap: "wrap" }}>
                {/* THE READ-OUT IS THE ENGINE'S SENTENCE, not a number this panel formatted. It
                    changes as the author walks — "clear here", "78% of the subject is hidden here",
                    "the camera is inside something here" — which is the whole reason a walk beats a
                    single verdict. */}
                <span
                  data-testid="cutscene-walk-reads"
                  role="status"
                  style={{ fontSize: fontSize.meta, color: color.text.secondary, minWidth: 0 }}
                >
                  {standing.message}
                </span>
                <Button
                  data-testid="cutscene-walk-worst"
                  variant="secondary"
                  compact
                  disabled={locked || Math.abs(walkAt - standing.worst) < 1e-6}
                  disabledReason={
                    locked ? lockReason : "You are already standing at the worst frame of this move."
                  }
                  title="Go back to the frame the engine judged this shot on"
                  onClick={() => {
                    setWalkAt(standing.worst);
                    void walkTo(active.index, standing.worst);
                  }}
                >
                  <Icon name="warning" size="md" /> Worst frame
                </Button>
              </div>
            </section>
          )}

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

            {/* WHAT THE SHOT IS OF COMES BEFORE HOW IT IS FRAMED. Size, angle and move are all
                stated RELATIVE to the subject — "how much of the frame the subject fills", "where
                the camera stands relative to the subject's facing" — so the subject is the first
                decision the four of them share, and every one of the other three means something
                different once it changes. */}
            <Field
              label="Frames"
              htmlFor={`${fieldId}-subject`}
              help={
                active.subject === selected
                  ? "The object this cutscene belongs to. Point the shot at the assembly it is part of for an establishing wide, or at one of its parts to cut in."
                  : `This shot films ${active.subjectName}, not ${name}.`
              }
              disabled={locked}
            >
              <SubjectPicker
                id={`${fieldId}-subject`}
                client={client}
                owner={selected}
                shotIndex={active.index}
                value={active.subject}
                valueName={active.subjectName}
                disabled={locked}
                disabledReason={lockReason}
                onPick={(subject) => editSubject(active, subject)}
                onAimInViewport={() => subjectAimStore.getState().begin(selected, active.index, cut.shots)}
              />
            </Field>

            {/* ADR-192 — SIZE AND ANGLE STOP DECIDING ANYTHING once a camera is placed, so they say
                so and stop accepting input. They are not cleared: they are the framing the shot goes
                back to, and "Use the card again" is right there. A control that still turned while
                the picture did not move is `<ux_quality>` 6's inert control, and the reason a
                disabled one has to say WHY in plain words. */}
            <Field
              label="Size"
              htmlFor={`${fieldId}-size`}
              help={
                active.camera
                  ? "Set by the camera you placed. Use the card again to choose a size."
                  : "How much of the frame the subject fills."
              }
              disabled={locked || Boolean(active.camera)}
            >
              <SelectField
                id={`${fieldId}-size`}
                data-testid="cutscene-size"
                value={active.size}
                disabled={locked || Boolean(active.camera)}
                title={
                  active.camera
                    ? "This shot films from a camera you placed, so how much of the frame the subject fills is decided by where that camera stands."
                    : locked
                      ? lockReason
                      : undefined
                }
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
              help={
                active.camera
                  ? "Set by the camera you placed. Use the card again to choose an angle."
                  : "Where the camera stands, relative to the subject's own facing."
              }
              disabled={locked || Boolean(active.camera)}
            >
              <SelectField
                id={`${fieldId}-angle`}
                data-testid="cutscene-angle"
                value={active.angle}
                disabled={locked || Boolean(active.camera)}
                title={
                  active.camera
                    ? "This shot films from a camera you placed, so the angle is wherever you put it."
                    : locked
                      ? lockReason
                      : undefined
                }
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

      {/* ADR-200 — EVERY WARNING IS NOW A PLACE. Each of these sentences names a shot, and until the
          shot number was a FIELD rather than a word inside the prose, that was all the panel had:
          the reader could act on it and the panel could not. The button beside each one takes the
          stage camera to where that shot films from, at the instant the sentence is about.

          The list is also in shot order now. Three producers write it — the document's own
          continuity checks, a placed camera's view of the world, a negotiated placement's — and they
          were appended in the order the functions ran, so a cut whose shot 1 opened tight and whose
          shot 3 was boxed in read "shot 3 ..." first. `in_shot_order` merges them in the engine, so
          there is one order and every surface drawing this list gets it. */}
      {cut.problems.length > 0 && (
        <div role="status" style={{ display: "grid", gap: space.xxs }}>
          {cut.problems.map((problem, i) => (
            // Four identical shots emit three byte-identical jump-cut warnings, so the string alone
            // is not a key.
            // eslint-disable-next-line react/no-array-index-key -- see above
            <Callout key={`${problem.message}-${i}`} tone="warn" data-testid="cutscene-problem">
              <div style={{ display: "grid", gap: space.xxs, justifyItems: "start" }}>
                <span>{problem.message}</span>
                {problem.shot != null && (
                  <Button
                    data-testid="cutscene-take-me-there"
                    variant="secondary"
                    compact
                    disabled={locked}
                    disabledReason={lockReason}
                    title={
                      locked
                        ? lockReason
                        : `Stand the camera where shot ${problem.shot + 1} films from, so you can see this and re-frame it`
                    }
                    onClick={() => void takeMeThere(problem.shot ?? 0)}
                  >
                    <Icon name="waypoint" size="md" /> Take me there
                  </Button>
                )}
              </div>
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
          // THE NEW SHOT OPENS. A card is a whole framing decision, and the first thing an author
          // does with one is aim it — an establishing wide is a wide of the ASSEMBLY, not of the
          // part its cutscene hangs on. Leaving the new shot closed made that a hunt for the clip
          // that had just appeared; opening it puts the Frames picker one click away, which is why
          // "what a shot films" needs no sticky mode above the card grid to be reachable.
          onPick={(kind) =>
            void run(async () => {
              const reply = await client.cinemaAddShot(selected, kind);
              const last = reply.rows[reply.rows.length - 1];
              if (last && !reply.reason) {
                setActiveId(last.id);
                setPlayhead(last.openSeconds);
              }
              return reply;
            }, "Add shot")
          }
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
