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
import type { CinemaReply, RenderFormat, RenderReply } from "../transport/protocol";
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

/** What a render starts as.
 *
 *  THE MOVIE, for the reason 1080 is the default height and not "as on screen": a render is the thing
 *  that LEAVES the editor, and what leaves it should be watchable. The sequence is still one click
 *  away and is still the right answer for a compositor — it is lossless, and it is the only one of
 *  the two with no encoder ceiling over it. */
const DEFAULT_FORMAT: RenderFormat = "movie";

/** Bits per second, in the unit a person would say out loud. */
function rateLabel(bitsPerSecond: number): string {
  return `${(bitsPerSecond / 1_000_000).toFixed(1)} Mbit/s`;
}

/** The height a render starts on.
 *
 *  1080 AND NOT "AS ON SCREEN", which is what this dialog shipped with. A render is a DELIVERY — the
 *  thing that leaves the editor — and the size a stage happens to be after the author opened a dock is
 *  not a delivery format. The old default silently made every sequence as tall as the window, which on
 *  a laptop with both docks open is around 400 lines: a film nobody can use, produced by a dialog that
 *  never asked. */
const DEFAULT_HEIGHT = "1080";

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
}: RenderDialogProps) {
  const [fps, setFps] = useState<number>(24);
  /** `"viewport"`, or one of `HEIGHTS`. A string because that is what a `<select>` carries; the number
   *  it means is `height` below, and `null` is the engine's own word for "as on screen". */
  const [sizeChoice, setSizeChoice] = useState<string>(DEFAULT_HEIGHT);
  const [scope, setScope] = useState<"cut" | "shot">("cut");
  const [format, setFormat] = useState<RenderFormat>(DEFAULT_FORMAT);
  const [stem, setStem] = useState(() => name);
  const [plan, setPlan] = useState<RenderReply | null>(null);
  const [job, setJob] = useState<RenderReply | null>(null);
  const [starting, setStarting] = useState(false);
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
    setStem(name);
    // ALWAYS the whole cut, even with a shot open. The headline offer is the film; rendering one shot
    // is the narrower second choice, and a dialog that opened on it would quietly make "Render" mean
    // two seconds of a thirteen-second cut for anybody who had clicked a clip first.
    setScope("cut");
    setSizeChoice(DEFAULT_HEIGHT);
    setFormat(DEFAULT_FORMAT);
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
  }, [open, name, client]);

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

  // ADR-182 — A MOVIE HAS ONE SIZE FOR ITS WHOLE LENGTH, so "as on screen" is not one it can have: a
  // stream declares its frame size in a header written before the first sample, and the stage's size
  // is a measurement that changes while you work. The engine refuses the pair in a sentence — but
  // LEAVING the author on it and then explaining is worse than not offering it: the option disappears
  // from the picker while a movie is selected, and choosing a movie while on it moves to the default
  // height rather than to a refusal the author has to read and then undo.
  useEffect(() => {
    if (format === "movie" && sizeChoice === "viewport") setSizeChoice(DEFAULT_HEIGHT);
  }, [format, sizeChoice]);

  const start = useCallback(async () => {
    setStarting(true);
    try {
      const reply = await client.cinemaRenderStart(entity, fps, shotIndex, stem, null, height, format);
      setJob(reply);
    } finally {
      setStarting(false);
    }
  }, [client, entity, fps, shotIndex, stem, height, format]);

  const stop = useCallback(async () => {
    setJob(await client.cinemaRenderCancel());
  }, [client]);

  if (!open) return null;

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
                  onChange={(event) => setFormat(event.currentTarget.value as RenderFormat)}
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
                  onChange={(event) => setFps(Number(event.currentTarget.value))}
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
                  onChange={(event) => setSizeChoice(event.currentTarget.value)}
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
                <TextField
                  id="render-stem"
                  data-testid="render-stem"
                  value={stem}
                  onChange={(event) => setStem(event.currentTarget.value)}
                />
              </Field>
            </FieldGrid>

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
