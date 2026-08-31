//! Scene look — what the scene is lit by, and how bright the camera takes it in.
//!
//! ## Why this panel exists
//!
//! All of it was already built and none of it was reachable. `import_environment` decodes a Radiance
//! panorama and lights the whole scene with it — its own doc comment says it "is the command that makes
//! it a capability a person can actually use" — and it was wired to nothing, so the only ways to use it
//! were the `MTK_ENV_HDR` environment variable read once at process start, or driving the app from a
//! script. `set_exposure` was in the same state: reported by `colour_status`, settable by no one.
//! The command-reachability audit has re-found this cluster on every run of this lane since 2026-08-26.
//!
//! ## Four decisions worth stating
//!
//! **The engine opens the file dialog, not the panel.** `import_environment` takes an optional path and
//! opens the native picker when it is absent, so the UI never has to ask a person to type an absolute
//! path into a text box — the arrangement `import_asset_dialog` already uses for meshes. A dismissed
//! dialog comes back as `cancelled`, which is a no-op and is drawn as neither a success nor a failure.
//!
//! **Nothing here enters the document.** The sky and the exposure change the picture and not the
//! project — no undo entry, no dirty flag, no CRDT operation (ADR-021). They are saved beside the
//! project in its `.view.json` sidecar and restored when it reopens, which is the part that used to be
//! missing: before this, a panorama was a thing you chose again every session, if you could choose it
//! at all.
//!
//! **The brightness is REPORTED, not implied by a thumbnail.** `mean_radiance` is what the diffuse
//! image-based lighting actually lights with — the box-filtered mip chain converges to exactly it — and
//! it was already in the reply and displayed nowhere. Metered against mid grey with the working space's
//! own luminance weights, it answers "will this sky make my scene brighter" before the frame is drawn.
//!
//! **The lights the scene already has are counted here.** "I loaded a panorama and nothing changed" is
//! what happens when authored lights dominate, so the one line that prevents it is the count — read off
//! the projection the outliner already holds, at the cost of a scan and no engine call.

import { useCallback, useEffect, useMemo, useState } from "react";
import { setStatus } from "../store/ui";
import { pushToast } from "../store/toasts";
import { projectionStore } from "../store/projection";
import { useStore } from "zustand";
import { environmentOutcome } from "../app/environmentOutcome";
import { Icon } from "../theme/icons";
import { Button, SliderField } from "../theme/primitives";
import { Callout, Field, FieldGrid } from "../theme/fields";
import { color, fontSize, space } from "../theme/tokens";
import type { EnvironmentReply } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

/**
 * The exposure stops the slider walks.
 *
 * Stops rather than a continuous range because exposure is perceptually logarithmic: a linear slider
 * from 0.05 to 8 spends four fifths of its travel above "blown out" and squeezes the entire usable
 * range into its first centimetre. These are the engine's own clamp bounds (`set_exposure` clamps to
 * `0.05..=8.0`) walked in roughly thirds of a stop, so every step is the same visible change.
 */
export const EXPOSURE_STOPS = [0.05, 0.1, 0.15, 0.22, 0.3, 0.45, 0.65, 0.9, 1.3, 1.8, 2.5, 3.5, 5, 8];

/** The renderer's default exposure, which is also the stop the slider reads as "default". */
const DEFAULT_EXPOSURE = 0.45;

/** Mid grey — the reference every photographic meter is calibrated to, and what a stop is counted from. */
const MID_GREY = 0.18;

/** Rec.709 luminance, the fallback when the colour status has not answered yet. */
const REC709_WEIGHTS: [number, number, number] = [0.2126, 0.7152, 0.0722];

/** The stop nearest a value, so a restored exposure lands the handle somewhere honest. */
export function nearestStop(value: number): number {
  let best = 0;
  for (let i = 1; i < EXPOSURE_STOPS.length; i += 1) {
    if (Math.abs(EXPOSURE_STOPS[i] - value) < Math.abs(EXPOSURE_STOPS[best] - value)) best = i;
  }
  return best;
}

/**
 * How much brighter or darker than the renderer's default this is, in stops.
 *
 * "0.45" means nothing to anyone. "−1.0 stops" is the unit every photographer, lighting artist and
 * camera on earth already uses, and it is the same number.
 */
export function stopsFromDefault(value: number): string {
  const stops = Math.log2(value / DEFAULT_EXPOSURE);
  if (Math.abs(stops) < 0.05) return "default";
  return `${stops > 0 ? "+" : "−"}${Math.abs(stops).toFixed(1)} stops`;
}

/**
 * What a panorama will actually light the scene with, as a sentence.
 *
 * The mean radiance is metered against mid grey in stops, because that is the comparison a lighting
 * artist already carries in their head — and it is a real measurement of the loaded file rather than a
 * guess from its name. Weighted with the working space's own luminance weights, which `colour_status`
 * reports and, until this panel, nothing read: metering ACEScg radiance with Rec.709 weights is the
 * kind of quiet error that only shows up as "the numbers disagree with the picture".
 */
export function brightnessSentence(
  mean: [number, number, number],
  weights: [number, number, number],
): string | null {
  const luminance = mean[0] * weights[0] + mean[1] * weights[1] + mean[2] * weights[2];
  if (!Number.isFinite(luminance) || luminance <= 0) return null;
  const stops = Math.log2(luminance / MID_GREY);
  const relative =
    Math.abs(stops) < 0.1
      ? "about mid grey"
      : `${Math.abs(stops).toFixed(1)} stops ${stops > 0 ? "over" : "under"} mid grey`;
  return `lights at ${luminance.toFixed(2)} average, ${relative}`;
}

const EMPTY_ENV: EnvironmentReply = {
  applied: false,
  label: "Studio (built in)",
  width: 0,
  height: 0,
  meanRadiance: [0, 0, 0],
  message: "",
  reason: null,
  path: null,
  cancelled: false,
};

export function LookSection({ client }: { client: EditorClient }) {
  const [env, setEnv] = useState<EnvironmentReply>(EMPTY_ENV);
  const [exposure, setExposureValue] = useState(DEFAULT_EXPOSURE);
  const [weights, setWeights] = useState<[number, number, number]>(REC709_WEIGHTS);
  const [busy, setBusy] = useState(false);
  const [refusal, setRefusal] = useState<string | null>(null);

  // The scene's own lights, off the projection the outliner already holds. `kind` is the salient type
  // the engine classified (`bridge.rs::classify_kind`) and the same field `sceneQuery`'s facets count,
  // so this line and the outliner's "Lights 6" chip can never disagree.
  const order = useStore(projectionStore, (s) => s.order);
  const summaries = useStore(projectionStore, (s) => s.summaries);
  const lightCount = useMemo(
    () => order.reduce((n, id) => (summaries[id]?.kind === "light" ? n + 1 : n), 0),
    [order, summaries],
  );

  const readState = useCallback(() => {
    void client
      .environmentState()
      .then(setEnv)
      .catch((e: unknown) => console.error("environment_state failed", e));
    // The exposure and the luminance weights both live in the colour status, which is the one place
    // that already reports them — a second reader would be a second opinion about the same numbers.
    void client
      .colourStatus()
      .then((status) => {
        setExposureValue(status.exposure);
        setWeights(status.working.luminanceWeights);
      })
      .catch((e: unknown) => console.error("colour_status failed", e));
  }, [client]);

  useEffect(readState, [readState]);

  async function chooseEnvironment() {
    setBusy(true);
    setRefusal(null);
    try {
      const reply = await client.importEnvironment();
      // Three outcomes, read in ONE place (`environmentOutcome`) because the palette reads the same
      // reply: a dismissed dialog is silent, a refusal keeps the engine's own sentence, a success
      // reports what it bought.
      const outcome = environmentOutcome(reply);
      if (!outcome) return;
      if (outcome.tone === "error") {
        setRefusal(outcome.message);
        pushToast(outcome.message, "error");
        return;
      }
      setEnv(reply);
      pushToast(outcome.message, "success");
      setStatus(outcome.message);
    } catch (e) {
      console.error("import_environment failed", e);
      pushToast("That panorama could not be loaded — please try again", "error");
    } finally {
      setBusy(false);
    }
  }

  async function clearEnvironment() {
    setBusy(true);
    setRefusal(null);
    try {
      const reply = await client.resetEnvironment();
      // `applied` on a RESET means "the change landed"; on a read it means "a panorama is in force".
      // The panel stores the second sense, so it is normalised here rather than at every use.
      setEnv({ ...reply, applied: false, meanRadiance: [0, 0, 0], path: null });
      pushToast(reply.message, "success");
      setStatus(reply.message);
    } catch (e) {
      console.error("reset_environment failed", e);
      pushToast("The lighting could not be reset — please try again", "error");
    } finally {
      setBusy(false);
    }
  }

  function slideExposure(stopIndex: number) {
    const value = EXPOSURE_STOPS[stopIndex] ?? DEFAULT_EXPOSURE;
    setExposureValue(value);
    // Fire-and-forget on drag: this is render-only state with no engine round trip (ADR-021), and
    // awaiting each step would make the handle lag the pointer.
    void client.setExposure(value).catch((e: unknown) => console.error("set_exposure failed", e));
  }

  const brightness = env.applied ? brightnessSentence(env.meanRadiance, weights) : null;

  return (
    <section data-testid="look-section" style={{ display: "grid", gap: space.sm }}>
      <FieldGrid minColumn={200}>
        <Field
          label="Lit by"
          htmlFor="look-env"
          span="full"
          help={
            env.applied
              ? "It lights the scene and shows in reflections."
              : "The built-in studio sky. Load a panorama to light the scene with a real place."
          }
        >
          <div id="look-env" style={{ display: "grid", gap: space.xs }}>
            <div data-testid="look-env-label" style={{ fontSize: fontSize.body, color: color.text.primary }}>
              {env.label}
            </div>
            <div style={{ display: "flex", gap: space.xs, flexWrap: "wrap" }}>
              <Button
                data-testid="look-env-choose"
                variant="secondary"
                compact
                disabled={busy}
                title="Choose a Radiance .hdr panorama to light the scene with"
                onClick={() => void chooseEnvironment()}
              >
                <Icon name="sunrise" size="md" /> {env.applied ? "Change sky…" : "Use a sky image…"}
              </Button>
              {env.applied && (
                <Button
                  data-testid="look-env-reset"
                  variant="ghost"
                  compact
                  disabled={busy}
                  title="Go back to the built-in studio lighting"
                  onClick={() => void clearEnvironment()}
                >
                  <Icon name="close" size="sm" /> Studio sky
                </Button>
              )}
            </div>
            {/* ONE MEASUREMENT LINE, not a second paragraph of help. The size and the brightness are
                both facts about the FILE — measured, in the renderer's own terms — and the first
                capture of this panel had them as two more grey sentences indistinguishable from the
                field's help and from the scene note below, so three registers read as one wall of
                prose. `mean_radiance` is the mean the diffuse IBL converges to: the difference
                between "I chose a sunset" and "this sky is 1.5 stops hot". */}
            {env.applied && (
              <div
                data-testid="look-brightness"
                style={{ fontSize: fontSize.meta, color: color.text.muted, fontVariantNumeric: "tabular-nums" }}
              >
                {env.width}×{env.height}
                {brightness ? ` · ${brightness}` : ""}
              </div>
            )}
          </div>
        </Field>
      </FieldGrid>

      <FieldGrid minColumn={200}>
        <Field
          label="Exposure"
          htmlFor="look-exposure"
          span="full"
          help="How much light the camera takes in, in stops. It changes the picture, never the lights."
        >
          <SliderField
            id="look-exposure"
            data-testid="look-exposure"
            label={null}
            ariaLabel="Exposure"
            min={0}
            max={EXPOSURE_STOPS.length - 1}
            step={1}
            value={nearestStop(exposure)}
            valueLabel={stopsFromDefault(exposure)}
            disabled={busy}
            onChange={(e) => slideExposure(Number((e.target as HTMLInputElement).value))}
          />
        </Field>
      </FieldGrid>

      {/* THE SCENE'S OWN CONTRIBUTION, and it is a footnote about the whole panel rather than help for
          either field above — so it is ruled off from them. "I loaded a panorama and nothing changed"
          is what happens when authored lights dominate, and this is the one line that answers it on
          screen. Counted from the projection the outliner already holds: no engine call. */}
      <div
        data-testid="look-lights"
        style={{
          fontSize: fontSize.meta,
          color: color.text.secondary,
          borderTop: `1px solid ${color.border.subtle}`,
          paddingTop: space.xs,
        }}
      >
        {lightCount === 0
          ? "No lights in this scene — the sky and the renderer's default key light are doing all of it."
          : `${lightCount} ${lightCount === 1 ? "light" : "lights"} in this scene, lighting it alongside the sky.`}
      </div>

      {refusal && (
        <Callout tone="danger" role="status" data-testid="look-refusal">
          {refusal}
        </Callout>
      )}
    </section>
  );
}

export default LookSection;
