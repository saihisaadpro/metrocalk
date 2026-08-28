//! ADR-175 — the task dialog that turns an authored cutscene into files on disk.
//!
//! THE CAPABILITY THIS CLOSES. `solve_shot(recipe, subject, t)` has been pure since cutscenes shipped,
//! the renderer has read its own frames back to PNG since M14.2, and until this dialog existed the two
//! had never met: the engine could compose a shot for 2.39:1 scope, letterbox it, preview it at any
//! instant — and there was **no way at all to get a picture out**. Every still of this project's own
//! benchmark film was an operating-system screenshot taken by a script outside the engine.
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
import type { CinemaReply, RenderReply } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

/** The rates the engine renders at, in the order a picker should offer them.
 *
 *  A LIST AND NOT A NUMBER BOX. 24 is cinema, 25 is PAL, 30 is what the web assumes and 60 is what a
 *  screen recording of this editor already runs at; a field someone can type `0` into is a field that
 *  has to refuse, and every one of those refusals is a sentence nobody needed to read. The engine
 *  states the same four in `RENDER_RATES` and refuses anything else, so a rate that reached it from
 *  somewhere other than this list is still answered rather than assumed. */
const RATES = [24, 25, 30, 60] as const;

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
  const [scope, setScope] = useState<"cut" | "shot">("cut");
  const [stem, setStem] = useState(() => name);
  const [plan, setPlan] = useState<RenderReply | null>(null);
  const [job, setJob] = useState<RenderReply | null>(null);
  const [starting, setStarting] = useState(false);
  const startButton = useRef<HTMLButtonElement>(null);

  // The shot index this render would film, or `null` for the whole cut. One expression, because three
  // places need the same answer and a second copy of "…and only when a shot is actually open" is how
  // a dialog ends up offering to render shot `null`.
  const shotIndex = scope === "shot" && activeShotIndex !== null ? activeShotIndex : null;
  const running = job?.running === true;
  const finished = job?.done === true;

  // Reopening is a fresh dialog. Without this, closing a finished render and opening it again shows
  // the previous ledger under a button that says "Render" — the surface claiming a result for work
  // that has not started.
  useEffect(() => {
    if (!open) return;
    setJob(null);
    setStarting(false);
    setStem(name);
    // ALWAYS the whole cut, even with a shot open. The headline offer is the film; rendering one shot
    // is the narrower second choice, and a dialog that opened on it would quietly make "Render" mean
    // two seconds of a thirteen-second cut for anybody who had clicked a clip first.
    setScope("cut");
  }, [open, name]);

  // THE COST, FROM THE ENGINE. Asked again whenever the choice changes, because the frame count is
  // `plan_render`'s answer and not this component's arithmetic — the same function the job runs, so
  // the number above the button is the number of files that will appear.
  useEffect(() => {
    if (!open) return;
    let live = true;
    void client
      .cinemaRenderPlan(entity, fps, shotIndex)
      .then((reply) => {
        if (live) setPlan(reply);
      })
      .catch(() => {
        if (live) setPlan(null);
      });
    return () => {
      live = false;
    };
  }, [open, client, entity, fps, shotIndex]);

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

  const start = useCallback(async () => {
    setStarting(true);
    try {
      const reply = await client.cinemaRenderStart(entity, fps, shotIndex, stem);
      setJob(reply);
    } finally {
      setStarting(false);
    }
  }, [client, entity, fps, shotIndex, stem]);

  const stop = useCallback(async () => {
    setJob(await client.cinemaRenderCancel());
  }, [client]);

  if (!open) return null;

  const refusal = job?.reason ?? plan?.reason ?? null;
  const frames = plan?.frames ?? 0;
  const canRender = refusal === null && frames > 0 && !running && !starting;
  const headingId = "render-dialog-heading";
  const describedId = "render-dialog-description";

  return (
    <Modal
      open
      onClose={running ? () => undefined : onClose}
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
            Every frame is drawn by the viewport itself and written as a PNG, composed for{" "}
            {deliveryLabel} — the same picture the preview shows, without the editor over it.
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
                : `${job.seconds.toFixed(1)}s of cutscene at ${job.fps} frames per second.`}
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
              {job.written > 0 && (
                <code
                  data-testid="render-ledger-files"
                  style={{ font: font.mono, fontSize: fontSize.meta, color: color.text.muted }}
                >
                  {job.stem}.0000.png … {job.stem}.
                  {String(job.written - 1).padStart(4, "0")}.png
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
            <Callout tone="neutral" title="The viewport is filming">
              The stage is showing each frame as it is written. Leave it in front — a minimised window
              produces no frames to save.
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
                label="Name"
                htmlFor="render-stem"
                help="Frames are numbered after it — name.0000.png, name.0001.png."
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
              {refusal === null && frames > 0 && (
                <span style={{ fontSize: fontSize.meta, color: color.text.secondary }}>
                  Written as {frames} PNG files into a folder you choose next.
                </span>
              )}
            </div>

            {/* THE ONE THING THIS CANNOT DO, SAID BEFORE THE CLICK. Frames come off the viewport's own
                swapchain, so their size is the size of the composed frame on screen — a fact the
                dialog cannot change and must not hide behind an output-resolution box that would
                have to lie. */}
            <Callout tone="neutral" title="Frame size">
              Each frame is written at the size of the composed picture on screen. Make the window
              bigger, or collapse a dock, for a larger render.
            </Callout>
          </>
        )}

        <footer style={{ display: "flex", gap: space.sm, justifyContent: "flex-end" }}>
          {running ? (
            <>
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
