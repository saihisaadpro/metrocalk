//! ADR-175 — the task dialog that turns an authored cutscene into files on disk.
//!
//! THE CAPABILITY THIS CLOSES. `solve_shot(recipe, subject, t)` has been pure since cutscenes shipped,
//! the renderer has read its own frames back to PNG since M14.2, and until this dialog existed the two
//! had never met: the engine could compose a shot for 2.39:1 scope, letterbox it, preview it at any
//! instant — and there was **no way at all to get a picture out**. Every still of this project's own
//! benchmark film was an operating-system screenshot taken by a script outside the engine.
//!
//! ADR-182 — AND THEN IT BECAME A MOVIE. A sequence is not a deliverable: the person holding 120
//! numbered PNGs still had to find, install and drive `ffmpeg` before anybody could watch the cut, and
//! a cinematic that needs a second program is not one this engine delivered. The dialog now opens on
//! `Movie`, writes one H.264 MP4 through the encoder Windows already has, and still offers the lossless
//! sequence — which is what a compositor wants and the only thing that has no size ceiling.
//!
//! ADR-190 — AND THEN IT REMEMBERED. Every one of the four answers below was a `useState` seeded
//! from a constant on every open: a person who had decided their cut delivers a 1440 scope sequence
//! called `weld-line-master` re-decided it, four controls at a time, on every single render, and none
//! of the four survived closing the dialog — let alone closing the editor. They now live on the
//! cutscene beside its delivery frame, written by `cinema_set_render` as an ordinary undoable commit
//! and saved by the same code path that saves everything else. This component's state is a DRAFT of
//! the document's answer, seeded from it on open and written back as it changes.
//!
//! THE THREE MOMENTS, IN ONE PLACE (`<ux_quality>` 1-3). What will be written, said before the click;
//! how far it has got, while it runs; and what was actually written and where, after. The last one is
//! the part a status-bar toast could never carry: a sequence is 600 files in a folder, and "done" is
//! not an answer to "where are they".

import { useCallback, useEffect, useRef, useState } from "react";
import { Modal } from "../theme/Popover";
import { Button, SelectField, TextField } from "../theme/primitives";
import { Callout, Field, FieldGrid, Metric, MetricGrid, ProgressBar } from "../theme/fields";
import { Icon } from "../theme/icons";
import { color, elevation, font, fontSize, radius, space } from "../theme/tokens";
import type { CinemaReply, RenderFormat, RenderReply, RenderSettings } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

/** The rates the engine renders at, in the order a picker should offer them.
 *
 *  A LIST AND NOT A NUMBER BOX. 24 is cinema, 25 is PAL, 30 is what the web assumes and 60 is what a
 *  screen recording of this editor already runs at; a field someone can type `0` into is a field that
 *  has to refuse, and every one of those refusals is a sentence nobody needed to read. The engine
 *  states the same four in `RENDER_RATES` and refuses anything else, so a rate that reached it from
 *  somewhere other than this list is still answered rather than assumed. */
const RATES = [24, 25, 30, 60] as const;

/** ADR-177 — the output heights the engine offers, in the order a picker should list them.
 *
 *  HEIGHTS AND NOT SIZES. The width is the delivery frame's aspect times the height, worked out by
 *  `render_frame_size` in the engine — so a 2.39:1 cut at 1080 comes out 2582 wide and a vertical one
 *  608, with nobody doing that multiplication here. Duplicating it in TypeScript is exactly the
 *  one-contract-twice `cinemaRenderPlan` exists to prevent; the size the dialog SHOWS is the plan's own
 *  answer, so what it says and what lands in the file's IHDR are the same two numbers by construction.
 *  The engine states the same four in `RENDER_HEIGHTS` and refuses anything else by name. */
const HEIGHTS = [
  { value: "720", label: "720 · HD" },
  { value: "1080", label: "1080 · Full HD" },
  { value: "1440", label: "1440 · QHD" },
  { value: "2160", label: "2160 · 4K UHD" },
] as const;

/** ADR-182 — what a render delivers, in the order a picker should offer them.
 *
 *  TWO AND NOT FIVE. A movie is the thing a person can double-click and a sequence is the thing a
 *  compositor can take; every other container is one of those two wearing a different extension. The
 *  engine states the same two in `RenderFormat` and refuses anything else by name — and refuses the
 *  MOVIE, in its own sentence, on a machine or at a size whose H.264 encoder will not take it, which
 *  is the one refusal a sequence never has. */
const FORMATS = [
  { value: "movie", label: "Movie — one MP4 file" },
  { value: "sequence", label: "PNG sequence — one file per frame" },
] as const;


/** Bits per second, in the unit a person would say out loud. */
function rateLabel(bitsPerSecond: number): string {
  return `${(bitsPerSecond / 1_000_000).toFixed(1)} Mbit/s`;
}

/** ADR-190 — the size picker's value for the height a stored setting names.
 *
 *  A `<select>` carries a string and the document carries `number | null`, so exactly one function
 *  crosses between them, in each direction ([`heightOf`] is the other). `null` is the engine's own
 *  word for "as on screen" and it is a different KIND of answer from a number, which is why it is not
 *  a sentinel `0`: everywhere it is read, it behaves differently rather than numerically.
 *
 *  THE `movie` COERCION IS HERE and not in an effect. A movie has one size for its whole length, so
 *  "as on screen" is not a size it can have — the engine refuses to store the pair. A document that
 *  somehow holds it (hand-edited, or written before this rule) is shown the default height rather
 *  than a `<select>` sitting on a value it does not offer. */
function sizeValueOf(settings: RenderSettings): string {
  if (settings.height === null) return settings.format === "movie" ? "1080" : "viewport";
  return String(settings.height);
}

/** The height a size-picker value means, in the document's vocabulary. */
function heightOf(sizeChoice: string): number | null {
  return sizeChoice === "viewport" ? null : Number(sizeChoice);
}

/** How long a render's own progress is polled for, in ms.
 *
 *  A POLL AND NOT A PUSH, and four times a second and not sixty. The job advances on the engine's
 *  heartbeat with no JavaScript involved at all (invariant 4); this interval is only how often the
 *  bar is allowed to redraw, and a progress bar that updates faster than a person can read is a
 *  progress bar that costs IPC to be illegible. */
const POLL_MS = 250;

/** Bytes, in the unit a person would say out loud. */
function sizeLabel(bytes: number): string {
  if (bytes <= 0) return "0 MB";
  const mb = bytes / (1024 * 1024);
  return mb < 1 ? `${Math.max(1, Math.round(bytes / 1024))} KB` : `${mb.toFixed(1)} MB`;
}

/** Seconds, at the precision a duration is read at. */
function secondsLabel(ms: number): string {
  return `${(ms / 1000).toFixed(1)}s`;
}

export interface RenderDialogProps {
  open: boolean;
  onClose: () => void;
  client: EditorClient;
  /** The object whose cutscene is filmed. */
  entity: string;
  /** Its display name — the dialog's heading and the default file stem. */
  name: string;
  /** The cut as the engine last reported it: its length, its shots and the frame it is delivered in. */
  cut: CinemaReply;
  /** The shot the author has open, offered as the alternative to the whole cut. `null` when none is. */
  activeShotIndex: number | null;
  /** What the delivery frame is CALLED, from the engine's own catalogue — never a word chosen here. */
  deliveryLabel: string;
  /** ADR-190 — a successful `cinema_set_render`, handed up so the panel that owns `cut` can adopt it.
   *
   *  THE DIALOG DOES NOT OWN THE DOCUMENT. It sends the change and shows any refusal in place, next
   *  to the control that caused it; the panel above it holds the cutscene, raises the "Ctrl-Z to
   *  undo" toast every other cinematics edit raises, and is the thing that would otherwise hand this
   *  dialog a stale `cut` the next time it opened. */
  onSettingsSaved: (reply: CinemaReply, announce: boolean) => void;
}

export function RenderDialog({
  open,
  onClose,
  client,
  entity,
  name,
  cut,
  activeShotIndex,
  deliveryLabel,
  onSettingsSaved,
}: RenderDialogProps) {
  const [fps, setFps] = useState<number>(cut.render.fps);
  /** `"viewport"`, or one of `HEIGHTS`. A string because that is what a `<select>` carries; the number
   *  it means is `height` below, and `null` is the engine's own word for "as on screen". */
  const [sizeChoice, setSizeChoice] = useState<string>(() => sizeValueOf(cut.render));
  const [scope, setScope] = useState<"cut" | "shot">("cut");
  const [format, setFormat] = useState<RenderFormat>(cut.render.format);
  /** The file name as the author is typing it. EMPTY IS A REAL ANSWER — it means "call it after the
   *  object" — so this is seeded from the stored name and not from `name`, and the object's name is
   *  the field's PLACEHOLDER rather than its value. Seeding it with the object's name instead would
   *  freeze that name into the document the first time anybody touched the field. */
  const [stem, setStem] = useState<string>(cut.render.name);
  const [plan, setPlan] = useState<RenderReply | null>(null);
  const [job, setJob] = useState<RenderReply | null>(null);
  const [starting, setStarting] = useState(false);
  /** ADR-190 — why the last settings write did not land, said beside the controls that made it.
   *
   *  IN THE DIALOG AND NOT IN A TOAST BEHIND IT. A modal covers the status bar, and "stop Play first"
   *  is an answer about the control the reader is looking at. */
  const [settingsRefusal, setSettingsRefusal] = useState<string | null>(null);
  const startButton = useRef<HTMLButtonElement>(null);

  // The shot index this render would film, or `null` for the whole cut. One expression, because three
  // places need the same answer and a second copy of "…and only when a shot is actually open" is how
  // a dialog ends up offering to render shot `null`.
  const shotIndex = scope === "shot" && activeShotIndex !== null ? activeShotIndex : null;
  const height = sizeChoice === "viewport" ? null : Number(sizeChoice);
  const running = job?.running === true;
  const finished = job?.done === true;

  // Reopening is a fresh dialog — UNLESS a render is still going, in which case it is that render.
  //
  // The job lives on the engine, not in this component, so closing the dialog does not stop it. A
  // reopen that always started fresh would therefore offer a "Render 319 frames" button over a render
  // already in flight, and the engine would refuse the click — a control that is enabled and cannot
  // act, which is exactly what `<ux_quality>` 6 forbids. A FINISHED job is not adopted: its ledger
  // belongs to the moment it was read, and a dialog opened an hour later reporting it as news is a
  // different lie.
  useEffect(() => {
    if (!open) return;
    setStarting(false);
    setSettingsRefusal(null);
    // ALWAYS the whole cut, even with a shot open. The headline offer is the film; rendering one shot
    // is the narrower second choice, and a dialog that opened on it would quietly make "Render" mean
    // two seconds of a thirteen-second cut for anybody who had clicked a clip first.
    //
    // AND IT IS THE ONE ANSWER THAT IS STILL NOT REMEMBERED (ADR-190), deliberately: the other four
    // describe the DELIVERABLE and belong to the cut, while this one describes what the author is
    // looking at right now. A dialog that reopened on "shot 3 only" a week later would render two
    // seconds of a film and say it had rendered the film.
    setScope("cut");
    // ADR-190 — THE DOCUMENT'S ANSWERS, not four constants. `cut` is the reply the panel above is
    // already holding, so these are settled before the first paint rather than arriving after it.
    setFps(cut.render.fps);
    setSizeChoice(sizeValueOf(cut.render));
    setFormat(cut.render.format);
    setStem(cut.render.name);
    let live = true;
    void client
      .cinemaRenderStatus()
      .then((reply) => {
        if (live) setJob(reply.running ? reply : null);
      })
      .catch(() => {
        if (live) setJob(null);
      });
    return () => {
      live = false;
    };
    // `cut.render` and not `cut`: this effect SEEDS the draft, and re-running it on every reply would
    // reset the author's half-made choice each time an unrelated cinematics edit refreshed the cut.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, client, cut.render]);

  // THE COST, FROM THE ENGINE. Asked again whenever the choice changes, because the frame count is
  // `plan_render`'s answer and not this component's arithmetic — the same function the job runs, so
  // the number above the button is the number of files that will appear.
  useEffect(() => {
    if (!open) return;
    let live = true;
    void client
      .cinemaRenderPlan(entity, fps, shotIndex, height, format)
      .then((reply) => {
        if (live) setPlan(reply);
      })
      .catch(() => {
        if (live) setPlan(null);
      });
    return () => {
      live = false;
    };
  }, [open, client, entity, fps, shotIndex, height, format]);

  // Progress. Stops the moment the job does, so a finished render is not polled forever — and the
  // ledger it left is exactly what the last poll returned.
  useEffect(() => {
    if (!open || !running) return;
    const timer = window.setInterval(() => {
      void client
        .cinemaRenderStatus()
        .then(setJob)
        .catch(() => undefined);
    }, POLL_MS);
    return () => window.clearInterval(timer);
  }, [open, running, client]);

  // ADR-190 — WRITE THE FOUR ANSWERS TO THE DOCUMENT.
  //
  // THE WHOLE BLOCK, EVERY TIME, and never one field. The four are validated together on the engine —
  // `(movie, "as on screen")` is the one pair it refuses — so sending one of them alone would ask it
  // to judge a combination half of which it has to read out of the document. `cinema_set_render`
  // writes the same serialised cutscene whatever changed, so a four-field write costs exactly what a
  // one-field write costs.
  //
  // AND THE CONTROLS SNAP BACK ON A REFUSAL. The document is the truth; a picker left showing 1440
  // after the engine declined to store 1440 is a control lying about the state of the thing it edits.
  const commit = useCallback(
    async (
      next: {
        format: RenderFormat;
        fps: number;
        sizeChoice: string;
        stem: string;
        folder?: string;
      },
      announce = true,
    ) => {
      setFormat(next.format);
      setFps(next.fps);
      setSizeChoice(next.sizeChoice);
      setStem(next.stem);
      try {
        const reply = await client.cinemaSetRender(
          entity,
          next.format,
          next.fps,
          heightOf(next.sizeChoice),
          next.stem,
          // NEVER TYPED, AND NEVER FROM A DRAFT. A destination only ever arrives from a picker, so
          // the default here is the one the document already holds — changing a rate cannot clear a
          // folder — and the only caller that overrides it is the one holding a path a picker just
          // returned.
          next.folder ?? cut.render.folder,
        );
        if (reply.reason) {
          setSettingsRefusal(reply.reason);
          setFormat(cut.render.format);
          setFps(cut.render.fps);
          setSizeChoice(sizeValueOf(cut.render));
          setStem(cut.render.name);
          return;
        }
        setSettingsRefusal(null);
        onSettingsSaved(reply, announce);
      } catch (e) {
        console.error("cinema_set_render failed", e);
        setSettingsRefusal("The render settings could not be saved — please try again.");
      }
    },
    [client, entity, cut.render, onSettingsSaved],
  );

  // ADR-190 — ASK FOR A FOLDER, AND REMEMBER IT.
  //
  // A CANCELLED PICKER IS NOT A REFUSAL. It comes back with no entity and no reason, because a
  // decision not to decide is not an error and nothing changed; treating it as one is how a surface
  // ends up scolding somebody for pressing Escape.
  const chooseFolder = useCallback(async () => {
    try {
      const reply = await client.cinemaPickRenderFolder(entity);
      if (reply.entity === null) return;
      if (reply.reason) {
        setSettingsRefusal(reply.reason);
        return;
      }
      setSettingsRefusal(null);
      onSettingsSaved(reply, true);
    } catch (e) {
      console.error("cinema_pick_render_folder failed", e);
      setSettingsRefusal("The destination could not be saved — please try again.");
    }
  }, [client, entity, onSettingsSaved]);

  // The draft as it stands, so each handler names only the one answer it changes.
  const draft = { format, fps, sizeChoice, stem };

  const start = useCallback(async () => {
    setStarting(true);
    try {
      // AN EMPTY NAME MEANS THE OBJECT'S OWN, resolved here the same way `RenderSettings::stem_for`
      // resolves it on the engine — the document stores what was typed, and blank is a real answer.
      const reply = await client.cinemaRenderStart(
        entity,
        fps,
        shotIndex,
        stem.trim() || name,
        // THE REMEMBERED DESTINATION, or `null` to be asked. The engine treats a folder that is no
        // longer on this machine exactly like `null`, so a project that travelled asks rather than
        // failing.
        cut.render.folder || null,
        height,
        format,
      );
      setJob(reply);
      // AND WHAT THE PICKER RETURNED IS REMEMBERED. Clicking Render with nothing stored opens a
      // picker titled "Choose a folder for the rendered frames"; the author has just answered the
      // question, and asking it again on the next take would be forgetting an answer given ten
      // seconds ago. Announced quietly — the author came here to render, not to edit settings, so
      // this rides in without a toast of its own.
      if (!reply.reason && reply.folder && reply.folder !== cut.render.folder) {
        await commit({ format, fps, sizeChoice, stem, folder: reply.folder }, false);
      }
    } finally {
      setStarting(false);
    }
  }, [client, entity, fps, shotIndex, stem, name, height, format, sizeChoice, cut.render.folder, commit]);

  const stop = useCallback(async () => {
    setJob(await client.cinemaRenderCancel());
  }, [client]);

  if (!open) return null;

  // A SETTINGS REFUSAL IS NOT A PLAN REFUSAL, so it is not folded into `refusal`: `refusal` disables
  // the Render button and captions it, and "stop Play first — render settings are authored" is a
  // reason the SETTING did not land, not a reason there is nothing to render.
  const refusal = job?.reason ?? plan?.reason ?? null;
  const frames = plan?.frames ?? 0;
  // The size the files will be, from the plan — never multiplied here. `0` while the first plan is in
  // flight, and while a refusal stands there is no size because there is no render.
  const outWidth = plan?.width ?? 0;
  const outHeight = plan?.height ?? 0;
  // THE MOVIE'S PRICE, FROM THE ENGINE. `bitrate_for` is the engine's function and this never
  // multiplies anything: the number said above the button is the number the encoder is handed.
  const bitrate = plan?.bitrate ?? 0;
  // What the FINISHED job was, not what the picker currently says — a reopened dialog is looking at a
  // render it did not start, and a ledger captioned by this component's own state would describe the
  // wrong delivery for it.
  const jobIsMovie = job?.format === "movie";
  // ENABLED WHILE THE COUNT IS STILL COMING.
  //
  // A plan that has not arrived yet is not a refusal, and painting it as one is not free: the primary
  // button spent the first frames of every open in the DISABLED palette and then animated out of it,
  // which is a control that looks broken for about as long as a person needs to notice. It was caught
  // by the shots gate reading the capture's own pixels — 3.36:1 grey-on-grey on a control the gate
  // could see was not disabled — and by nothing else, because every selector assertion in this file
  // and in `specs-render` reads the settled state.
  //
  // Safe because `cinema_render_start` PLANS AGAIN on the engine before it writes anything, with the
  // same `plan_render` this dialog is waiting on: an author who hits Render in that first moment gets
  // a render, and a cut with nothing to render is still refused — in a sentence, by the engine. This
  // is not an enabled control that cannot act; it is one whose price tag is still loading.
  const canRender = refusal === null && !running && !starting;
  const headingId = "render-dialog-heading";
  const describedId = "render-dialog-description";

  return (
    <Modal
      open
      // Closable while it runs. The job is the engine's, not this component's — it keeps going, and
      // reopening finds it again — so trapping the author in a progress bar for five minutes would be
      // a modal holding them hostage to a thing it does not own.
      onClose={onClose}
      initialFocusRef={startButton}
      ariaLabelledBy={headingId}
      ariaDescribedBy={describedId}
    >
      <div
        data-testid="render-dialog"
        style={{
          width: "min(680px, calc(100vw - 32px))",
          maxHeight: "calc(100vh - 32px)",
          overflow: "auto",
          padding: space.lg,
          display: "grid",
          gap: space.md,
          border: `1px solid ${color.border.strong}`,
          borderRadius: radius.xl,
          background: color.bg.raised,
          color: color.text.primary,
          // `elevation.e4`, not a literal shadow. The dialog beside it in `AnimationWorkspace` carries
          // a hand-written `rgba(0, 0, 0, 0.48)` that predates the scale; copying it would have made
          // this the second statement of a value the design system already owns.
          boxShadow: elevation.e4,
        }}
      >
        <header style={{ display: "grid", gap: space.xxs }}>
          <h2 id={headingId} style={{ margin: 0, font: font.ui, fontSize: fontSize.heading }}>
            <Icon name="clapper" size="md" /> Render {name}
          </h2>
          <p
            id={describedId}
            style={{ margin: 0, fontSize: fontSize.meta, color: color.text.secondary }}
          >
            Every frame is drawn by the renderer the viewport draws with, composed for {deliveryLabel} —
            the same picture the preview shows, without the editor over it.
          </p>
        </header>

        {/* THE OPTIONS DISAPPEAR ONCE THE ANSWER EXISTS. A finished render replaces them with its own
            ledger in place, rather than leaving a form above a result — the reader's question has
            changed from "what shall I render" to "where did it go". */}
        {finished ? (
          <section data-testid="render-ledger" style={{ display: "grid", gap: space.sm }}>
            {/* THE HEADLINE SAYS WHAT THE TILES CANNOT. Its first draft repeated the engine's own
                sentence — "Rendered 319 frames at 1920x803 in 41.8s" — forty pixels above three tiles
                carrying exactly those three numbers: the same stutter ADR-162 found in the preview
                read-out, arriving from the other direction. What the tiles do NOT carry is what the
                sequence IS — how much cutscene, at what rate. Found by READING the capture; no
                selector assertion can see a fact stated twice. */}
            <Callout
              tone={job.failures.length > 0 ? "warn" : "success"}
              title={job.failures.length > 0 ? "Rendered, with losses" : "Rendered"}
            >
              {job.failures.length > 0
                ? job.message
                : `${job.seconds.toFixed(1)}s of cutscene at ${job.fps} frames per second, ${
                    jobIsMovie ? "encoded as one H.264 movie" : "as a lossless PNG sequence"
                  }.`}
            </Callout>
            <MetricGrid minColumn={140}>
              <Metric
                data-testid="render-ledger-frames"
                label="Frames written"
                value={job.written}
                description="Files that exist on disk"
              />
              <Metric
                label="Frame size"
                value={job.width > 0 ? `${job.width}×${job.height}` : "—"}
                description="The pixel size each frame was written at"
              />
              <Metric label="On disk" value={sizeLabel(job.bytes)} />
              <Metric label="Took" value={secondsLabel(job.elapsedMs)} />
            </MetricGrid>
            {/* THE FOLDER, THEN WHAT IS IN IT. Two lines and no joined path: the separator belongs to
                whichever platform the engine is running on, and the one written here appeared in the
                first capture as `C:/renders/line\Line.0000.png` — one path carrying both. The range
                also answers the question the folder alone does not: which of these files are mine. */}
            <div style={{ display: "grid", gap: space.xxs }}>
              <span style={{ fontSize: fontSize.meta, color: color.text.muted }}>
                {job.written > 0 ? "Written to" : "Nothing was written to"}
              </span>
              <code
                data-testid="render-ledger-folder"
                style={{
                  font: font.mono,
                  fontSize: fontSize.meta,
                  color: color.text.secondary,
                  wordBreak: "break-all",
                }}
              >
                {job.folder}
              </code>
              {/* ONE NAME FOR A MOVIE, A RANGE FOR A SEQUENCE. The range answers the question the
                  folder alone does not — which of these files are mine — and a movie has no such
                  question: there is exactly one file and this is what it is called. Printing
                  `take.0000.png … take.0119.png` over a folder holding one `take.mp4` would be the
                  dialog describing a delivery it did not make. */}
              {job.written > 0 && (
                <code
                  data-testid="render-ledger-files"
                  style={{ font: font.mono, fontSize: fontSize.meta, color: color.text.muted }}
                >
                  {jobIsMovie ? (
                    `${job.stem}.mp4`
                  ) : (
                    <>
                      {job.stem}.0000.png … {job.stem}.
                      {String(job.written - 1).padStart(4, "0")}.png
                    </>
                  )}
                </code>
              )}
            </div>
            {job.failures.length > 0 && (
              <ul
                data-testid="render-failures"
                style={{
                  margin: 0,
                  paddingLeft: space.md,
                  fontSize: fontSize.meta,
                  color: color.warn.text,
                }}
              >
                {job.failures.slice(0, 6).map((why) => (
                  <li key={why}>{why}</li>
                ))}
              </ul>
            )}
          </section>
        ) : running ? (
          <section data-testid="render-progress" style={{ display: "grid", gap: space.sm }}>
            <ProgressBar
              value={job.frames > 0 ? job.written / job.frames : undefined}
              label="Render progress"
            />
            <p style={{ margin: 0, fontSize: fontSize.meta, color: color.text.secondary }}>
              {job.message}
              {job.width > 0 && (
                <>
                  {" · "}
                  <span style={{ font: font.mono }}>
                    {job.width}
                    {"×"}
                    {job.height}
                  </span>
                </>
              )}
              {" · "}
              {secondsLabel(job.elapsedMs)}
            </p>
            {/* WHAT IS TRUE OF *THIS* RENDER. The warning below is real on the swapchain path — a
                minimised window stops vending textures and the job stalls, which is why it is bounded
                at 30s with a sentence about it — and false on the offscreen one, where the frames are
                drawn into targets the window has nothing to do with. `job.offscreen` is the engine's
                own answer rather than this component remembering which button was clicked, because a
                reopened dialog is looking at a render it did not start. */}
            <Callout tone="neutral" title="The viewport is filming">
              {job.offscreen
                ? "The stage is showing each frame as it is written, fitted to the window. The render is drawn separately, so it keeps going if the editor is covered or minimised."
                : "The stage is showing each frame as it is written. Leave it in front — a minimised window produces no frames to save."}
            </Callout>
          </section>
        ) : (
          <>
            <FieldGrid minColumn={190} aria-label="Render settings">
              <Field
                label="Render"
                htmlFor="render-scope"
                help={
                  activeShotIndex === null
                    ? "Open a shot on the timeline to render just that one."
                    : "A shot is rendered with the transition that opens it, so it arrives the way it does in the cut."
                }
              >
                <SelectField
                  id="render-scope"
                  data-testid="render-scope"
                  value={scope}
                  onChange={(event) => setScope(event.currentTarget.value as "cut" | "shot")}
                >
                  <option value="cut">
                    The whole cut {"—"} {cut.shots} shot{cut.shots === 1 ? "" : "s"}
                  </option>
                  {activeShotIndex !== null && (
                    <option value="shot">Shot {activeShotIndex + 1} only</option>
                  )}
                </SelectField>
              </Field>
              {/* FIRST, BECAUSE IT CHANGES WHAT THE OTHERS MEAN. The size picker's ceiling, the cost
                  line's second sentence and the ledger's file name all depend on this answer, so a
                  control placed after them would be a control the reader meets after the sentences it
                  governs. */}
              <Field
                label="Deliver as"
                htmlFor="render-format"
                help={
                  format === "movie"
                    ? "One file you can play anywhere. Encoded by this machine's own H.264 encoder."
                    : "One lossless PNG per frame — bigger, and what a compositor or an editor wants."
                }
              >
                <SelectField
                  id="render-format"
                  data-testid="render-format"
                  value={format}
                  onChange={(event) => {
                    // ADR-182's coercion, moved out of an effect and into the gesture that needs it.
                    // A movie has one size for its whole length, so "as on screen" is not a size it
                    // can have; choosing a movie while on it moves to the default height in the SAME
                    // commit rather than writing a pair the engine refuses and then correcting it.
                    const next = event.currentTarget.value as RenderFormat;
                    const sizeChoice =
                      next === "movie" && draft.sizeChoice === "viewport" ? "1080" : draft.sizeChoice;
                    void commit({ ...draft, format: next, sizeChoice });
                  }}
                >
                  {FORMATS.map((f) => (
                    <option key={f.value} value={f.value}>
                      {f.label}
                    </option>
                  ))}
                </SelectField>
              </Field>
              <Field
                label="Frame rate"
                htmlFor="render-fps"
                unit="fps"
                help="24 is cinema. 30 and 60 are smoother and write more files."
              >
                <SelectField
                  id="render-fps"
                  data-testid="render-fps"
                  value={String(fps)}
                  onChange={(event) =>
                    void commit({ ...draft, fps: Number(event.currentTarget.value) })
                  }
                >
                  {RATES.map((rate) => (
                    <option key={rate} value={rate}>
                      {rate}
                    </option>
                  ))}
                </SelectField>
              </Field>
              <Field
                label="Frame size"
                htmlFor="render-size"
                help={
                  // WHAT THE CHOICE IS FOR, not what the numbers are — the numbers are in the cost
                  // block below, stated once, by the engine.
                  format === "movie"
                    ? "The width follows the frame the shots are composed for. A movie is one size for its whole length."
                    : "The width follows the frame the shots are composed for. Taller is sharper and slower."
                }
              >
                <SelectField
                  id="render-size"
                  data-testid="render-size"
                  value={sizeChoice}
                  onChange={(event) =>
                    void commit({ ...draft, sizeChoice: event.currentTarget.value })
                  }
                >
                  {/* THE OLD BEHAVIOUR, STILL OFFERED AND NO LONGER THE DEFAULT. Rendering the stage
                      at whatever size the docks have left it is the right answer for a quick look at
                      a cut, and the wrong one for anything that leaves the editor.

                      ADR-182 — and it is not offered AT ALL for a movie, because a movie cannot have
                      it: the stream's frame size is written once, before the first sample, and the
                      stage's is a measurement that moves. Absent rather than present-and-refused. */}
                  {format === "sequence" && <option value="viewport">As on screen</option>}
                  {HEIGHTS.map((h) => (
                    <option key={h.value} value={h.value}>
                      {h.label}
                    </option>
                  ))}
                </SelectField>
              </Field>
              <Field
                label="Name"
                htmlFor="render-stem"
                help={
                  format === "movie"
                    ? "The movie is called after it — name.mp4."
                    : "Frames are numbered after it — name.0000.png, name.0001.png."
                }
              >
                {/* ADR-190 — TYPED LOCALLY, COMMITTED ON BLUR OR ENTER. The three pickers above
                    write on change because a `<select>` change IS the decision; a text field's every
                    keystroke is not, and committing each one would put fourteen undoable entries and
                    fourteen round trips behind one nine-letter name. Same shape as the rename session
                    in ADR-153, for the same reason. */}
                <TextField
                  id="render-stem"
                  data-testid="render-stem"
                  value={stem}
                  placeholder={name}
                  onChange={(event) => setStem(event.currentTarget.value)}
                  onBlur={(event) => {
                    if (event.currentTarget.value !== cut.render.name) {
                      void commit({ ...draft, stem: event.currentTarget.value });
                    }
                  }}
                  onKeyDown={(event) => {
                    if (event.key === "Enter") event.currentTarget.blur();
                  }}
                />
              </Field>
            </FieldGrid>

            {/* ADR-190 — WHERE, SAID BEFORE THE CLICK. This dialog's whole argument is that what will
                be written is stated before the button that writes it (`<ux_quality>` 1), and until
                now the one thing it never said was the destination: the folder was asked for AFTER
                the click, by the operating system, and the only surface that ever named it was the
                ledger at the end. It is a field like the other five now, and the answer is
                remembered on the cut. */}
            <Field
              label="Where"
              htmlFor="render-folder"
              help={
                cut.render.folder
                  ? "Remembered on this cut. If the folder is gone, the next render asks again."
                  : "Choose one now, or be asked when you click Render — either way it is remembered."
              }
            >
              <div style={{ display: "flex", gap: space.sm, alignItems: "center" }}>
                <span
                  id="render-folder"
                  data-testid="render-folder"
                  style={{
                    flex: 1,
                    minWidth: 0,
                    font: font.mono,
                    fontSize: fontSize.meta,
                    color: cut.render.folder ? color.text.primary : color.text.muted,
                    // A PATH IS READ FROM ITS END. `direction: rtl` keeps the folder — the part that
                    // identifies it — visible when the path is longer than the row, instead of eight
                    // characters of drive letter and an ellipsis.
                    direction: "rtl",
                    textAlign: "left",
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    whiteSpace: "nowrap",
                  }}
                  title={cut.render.folder || undefined}
                >
                  {cut.render.folder || "You'll be asked when you render"}
                </span>
                <Button
                  data-testid="render-folder-choose"
                  variant="secondary"
                  onClick={() => void chooseFolder()}
                >
                  Choose…
                </Button>
              </div>
            </Field>

            {settingsRefusal !== null && (
              <Callout tone="warn" title="That setting was not saved">
                {settingsRefusal}
              </Callout>
            )}

            {/* THE COST, ABOVE THE BUTTON THAT PAYS IT. The frame count is the engine's own plan, not
                a multiplication done here — so what this says and what appears in the folder are the
                same number by construction. */}
            <div
              data-testid="render-cost"
              style={{
                display: "grid",
                gap: space.xxs,
                padding: `${space.sm}px ${space.md}px`,
                borderRadius: radius.md,
                background: color.info.bg,
                border: `1px solid ${color.info.border}`,
              }}
            >
              <strong style={{ fontSize: fontSize.body, color: color.info.text }}>
                {refusal ?? plan?.message ?? "Counting the frames…"}
              </strong>
              {/* NOT THE FRAME COUNT AGAIN. The strong line above is the engine's own sentence and
                  already carries the count, the span, the rate and the size; a second line repeating
                  any of them is the stutter ADR-175 found in the ledger and ADR-162 in the preview
                  read-out. What is NOT above it is what the files are — lossless PNG — and that a
                  folder is the next question. */}
              {refusal === null && frames > 0 && (
                <span style={{ fontSize: fontSize.meta, color: color.text.secondary }}>
                  {format === "movie"
                    ? `One H.264 MP4 at about ${rateLabel(bitrate)}, into a folder you choose next.`
                    : "Lossless PNG, one file per frame, into a folder you choose next."}
                </span>
              )}
            </div>

            {/* WHAT A CHOSEN SIZE ACTUALLY COSTS, said before the click. Until ADR-177 this callout
                said the opposite — that the size was the window's and could not be chosen — because
                it could not be. It can now, and the honest thing left to say is that a render bigger
                than the stage is drawn rather than upscaled, which is why it is slower. */}
            {refusal === null && outHeight > 0 && (
              <Callout
                tone="neutral"
                title={
                  height === null
                    ? "Rendering the stage"
                    : `Rendering at ${outWidth} × ${outHeight}`
                }
              >
                {height === null
                  ? "Each frame is the composed picture at the size the window is making it, so opening a dock makes the next render smaller. Choose a height for a size that does not move."
                  : "Each frame is drawn at this size rather than stretched up to it, so it is sharper than the stage and takes longer per frame."}
              </Callout>
            )}
          </>
        )}

        <footer style={{ display: "flex", gap: space.sm, justifyContent: "flex-end" }}>
          {running ? (
            <>
              <Button data-testid="render-hide" variant="secondary" onClick={onClose}>
                Keep rendering
              </Button>
              <Button data-testid="render-stop" variant="danger" onClick={() => void stop()}>
                <Icon name="stop" size="sm" /> Stop
              </Button>
            </>
          ) : finished ? (
            <>
              <Button data-testid="render-again" variant="secondary" onClick={() => setJob(null)}>
                Render again
              </Button>
              <Button data-testid="render-done" variant="primary" onClick={onClose}>
                Done
              </Button>
            </>
          ) : (
            <>
              <Button data-testid="render-cancel" variant="secondary" onClick={onClose}>
                Cancel
              </Button>
              <Button
                ref={startButton}
                data-testid="render-start"
                variant="primary"
                disabled={!canRender}
                disabledReason={refusal ?? "There is nothing to render yet."}
                onClick={() => void start()}
              >
                <Icon name="clapper" size="sm" />{" "}
                {frames > 0 ? `Render ${frames} frames` : "Render"}
              </Button>
            </>
          )}
        </footer>
      </div>
    </Modal>
  );
}
