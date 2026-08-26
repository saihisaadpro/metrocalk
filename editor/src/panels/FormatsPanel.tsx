//! **What this build can read and write** — the format registry, shown.
//!
//! The engine's answer to "can it handle my file?" used to require reading source code. Worse, the
//! four places that answered it disagreed: the file dialog offered one list, the sniffer recognised
//! another, and a couple of formats were implemented and reachable from nothing at all.
//!
//! This renders the single declared registry. Three things it deliberately does that a feature matrix
//! usually does not:
//!
//! * **Fidelity is a first-class column.** "Supported" is not a yes/no — an explained seam, a declared
//!   subset and full support are three different promises, and conflating them is how someone
//!   discovers at the wrong moment that their trimmed surfaces did not survive.
//! * **A format this build lacks is still listed**, greyed, with what to do instead. Disappearing from
//!   the list would leave a user holding an `.fbx` with no idea whether the engine knows the format at
//!   all.
//! * **Every row states what it will not carry**, in the same sentence as what it will.

import { useEffect, useState } from "react";
import { Icon } from "../theme/icons";
import { Button, SelectField } from "../theme/primitives";
import { color, font, fontSize, radius, space } from "../theme/tokens";
import type { ColourStatus, FormatSpec } from "../transport/protocol";
import type { EditorClient, ViewportRenderProfile } from "../transport/session";

const FIDELITY_COPY: Record<string, { label: string; hint: string }> = {
  full: { label: "Full", hint: "Everything the engine's scene model holds survives" },
  subset: { label: "Subset", hint: "A stated, tested part of the format — the rest is reported" },
  seam: { label: "Seam", hint: "Recognised and explained here, decoded elsewhere" },
};

const DIRECTION_COPY: Record<string, string> = {
  import: "Read",
  export: "Write",
  both: "Read + write",
};

/** The capability flags, in the order a person actually asks about them. */
const CARRIES: { key: keyof FormatSpec["carries"]; label: string }[] = [
  { key: "geometry", label: "geometry" },
  { key: "hierarchy", label: "hierarchy" },
  { key: "materials", label: "materials" },
  { key: "textures", label: "textures" },
  { key: "skinning", label: "skinning" },
  { key: "animation", label: "animation" },
  { key: "cameras", label: "cameras" },
  { key: "metadata", label: "engineering data" },
  { key: "physics", label: "physics" },
];

/**
 * How colour is handled — per capability, and honest about what is not wired.
 *
 * Two decisions worth stating. First, the view-transform control drives the SAME `set_render_profile`
 * the viewport toolbar already uses, rather than introducing a parallel colour setting: two controls
 * for one piece of renderer state is how they end up disagreeing. Second, the capability list shows
 * the FALSE entries as prominently as the true ones. A colour panel that lists only what works is
 * exactly the panel that lets someone assume ACES is end-to-end when it is not.
 */
const CAPABILITY_COPY: Record<string, string> = {
  sceneLinearWorkingSpace: "Lighting and shading run in scene-linear light, not in display values",
  dataMapsBypassColourTransform: "Roughness, metallic, normal and occlusion maps are never given a transfer function",
  singleToneMapAtResolve: "The tone curve is applied exactly once, at the final resolve",
  colourSpaceDerivedFromOnePolicy: "Every texture's colour space comes from one documented policy, not per-call-site habit",
  colourDecisionCarriesProvenance: "Each decision says whether you chose it, the file declared it, or the engine assumed it",
  environmentColourSpaceOverride: "The environment map's colour space can be declared when the file does not record it",
  perTextureColourSpaceOverride: "A mesh texture's colour space can be overridden per asset and stored with it",
  referenceTestedPrimaryConversions: "Primary matrices are checked against published reference values",
  acesCgWorkingSpaceSelectable: "ACEScg can be chosen as the renderer's working space",
  renderResultsAreGenerationSafe: "Every thumbnail says which request and which state it was rendered for",
  viewTransformPersistedWithProject: "The chosen view is remembered next time you open the project",
  presentationStateSeparateFromDocument: "Changing how you look at a scene never edits the scene",
  ocioConfigLoading: "A studio OCIO config file can be loaded",
  aces2OutputTransform: "The ACES 2.0 Output Transform is available as a view",
  hdrDisplayOutput: "The frame can be presented to an HDR or wide-gamut display",
};

function ColourCard({ client }: { client: EditorClient }) {
  const [colour, setColour] = useState<ColourStatus | null>(null);
  const [busy, setBusy] = useState(false);

  const refresh = () => client.colourStatus().then(setColour).catch(() => setColour(null));
  useEffect(() => {
    let live = true;
    client.colourStatus().then((c) => live && setColour(c)).catch(() => { /* section stays hidden */ });
    return () => { live = false; };
  }, [client]);

  if (!colour) return null;

  const pick = async (profile: ViewportRenderProfile) => {
    setBusy(true);
    try {
      await client.setRenderProfile(profile);
      await refresh();
    } finally {
      setBusy(false);
    }
  };

  // Both of these re-read the status afterwards rather than assuming the write took. The renderer is
  // allowed to refuse — a name it does not know, or a non-linear space for an environment — and a
  // control that showed the requested value regardless would be the exact lie this card exists against.
  const pickWorking = async (arg: string) => {
    setBusy(true);
    try {
      await client.setWorkingSpace(arg);
      await refresh();
    } finally {
      setBusy(false);
    }
  };

  const pickEnv = async (arg: string) => {
    setBusy(true);
    try {
      await client.setEnvironmentColourSpace(arg);
      await refresh();
    } finally {
      setBusy(false);
    }
  };

  const caps = Object.entries(colour.capabilities);
  return (
    <section
      data-testid="colour-card"
      data-active-view={colour.activeView}
      style={{
        border: `1px solid ${color.border.subtle}`,
        borderRadius: radius.md,
        padding: `${space.sm}px ${space.md}px`,
        display: "grid",
        gap: space.xs,
      }}
    >
      <h3 style={{ margin: 0, font: font.ui, fontSize: fontSize.body, color: color.text.primary }}>
        Colour
      </h3>
      <div style={{ fontSize: fontSize.meta, color: color.text.secondary }}>
        Working space <strong data-testid="colour-working">{colour.working.label}</strong>. Everything
        below the tone curve is scene-linear light; the curve runs once, at the end. Brightness here
        means{" "}
        {colour.working.luminanceWeights.map((w) => w.toFixed(4)).join(" / ")} — bloom meters with those.
      </div>

      <div style={{ display: "flex", gap: space.xs, flexWrap: "wrap", alignItems: "center" }}>
        <span style={{ fontSize: fontSize.meta, color: color.text.muted }}>Working space</span>
        {colour.working.options.map((w) => {
          const on = w.id === colour.working.current;
          return (
            <Button
              key={w.id}
              variant={on ? "primary" : "ghost"}
              compact
              disabled={busy}
              aria-pressed={on}
              title={
                on
                  ? "Everything that carries light is converted into this space before it is shaded."
                  : `Shade in ${w.label}. Every colour is converted into it before the BRDF, and the frame returns to Rec.709 once, just before the view transform.`
              }
              data-testid={`colour-working-${w.id}`}
              onClick={() => pickWorking(w.arg)}
            >
              {w.label}
            </Button>
          );
        })}
      </div>

      <div style={{ display: "flex", gap: space.xs, flexWrap: "wrap", alignItems: "center" }}>
        <span style={{ fontSize: fontSize.meta, color: color.text.muted }}>Environment is</span>
        <SelectField
          aria-label="Environment colour space"
          data-testid="colour-env-space"
          disabled={busy}
          value={colour.environment.sourceSpace}
          onChange={(e) => pickEnv(e.target.value)}
          style={{ fontSize: fontSize.meta }}
        >
          {colour.environment.options.map((o) => (
            <option key={o.id} value={o.arg}>
              {o.label}
            </option>
          ))}
        </SelectField>
        {colour.environment.assumed && (
          <span style={{ fontSize: fontSize.meta, color: color.text.muted }}>
            assumed — the file does not say
          </span>
        )}
      </div>

      <div style={{ display: "flex", gap: space.xs, flexWrap: "wrap", alignItems: "center" }}>
        <span style={{ fontSize: fontSize.meta, color: color.text.muted }}>View transform</span>
        {colour.views.map((v) => {
          const on = v.id === colour.activeView;
          return (
            <Button
              key={v.id}
              variant={on ? "primary" : "ghost"}
              compact
              disabled={busy}
              aria-pressed={on}
              title={v.blurb}
              data-testid={`colour-view-${v.id}`}
              onClick={() => pick(v.id === "pbrNeutral" ? "cad" : "cinematic")}
            >
              {v.label}
            </Button>
          );
        })}
      </div>

      <ul style={{ margin: 0, padding: 0, listStyle: "none", display: "grid", gap: 2 }}>
        {caps.map(([key, on]) => (
          <li
            key={key}
            data-testid={`colour-cap-${key}`}
            data-on={on}
            style={{
              fontSize: fontSize.meta,
              color: on ? color.text.secondary : color.text.muted,
              display: "flex",
              gap: space.xs,
            }}
          >
            <span aria-hidden style={{ color: on ? color.accent.base : color.text.muted }}>
              {on ? <Icon name="check" size="sm" /> : <Icon name="minus" size="sm" />}
            </span>
            <span>
              {CAPABILITY_COPY[key] ?? key}
              {!on && <span style={{ color: color.warn.text }}> — not wired</span>}
            </span>
          </li>
        ))}
      </ul>

      {colour.notes.map((n) => (
        <p
          key={n.slice(0, 40)}
          style={{
            margin: 0,
            fontSize: fontSize.meta,
            color: color.text.secondary,
            background: color.bg.inset,
            border: `1px solid ${color.border.subtle}`,
            borderRadius: radius.sm,
            padding: `${space.xxs}px ${space.sm}px`,
          }}
        >
          {n}
        </p>
      ))}
    </section>
  );
}

export function FormatsPanel({ client }: { client: EditorClient }) {
  const [formats, setFormats] = useState<FormatSpec[]>([]);
  const [failed, setFailed] = useState(false);
  // Distinct from `formats.length`: a registry that legitimately came back empty is a different state
  // from one still in flight, and conflating them leaves the tab reading "Reading what this build
  // supports…" forever with nothing on the way.
  const [loaded, setLoaded] = useState(false);
  const [open, setOpen] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    client
      .formatCatalog()
      .then((list) => {
        if (!live) return;
        setFormats(list);
        setLoaded(true);
      })
      .catch(() => live && setFailed(true));
    return () => {
      live = false;
    };
  }, [client]);

  if (failed) {
    return (
      <div data-testid="formats-failed" style={{ padding: space.md, fontSize: fontSize.meta, color: color.danger.text }}>
        The format list could not be read from the engine.
      </div>
    );
  }
  if (!loaded) {
    return (
      <div data-testid="formats-loading" style={{ padding: space.md, fontSize: fontSize.meta, color: color.text.muted }}>
        Reading what this build supports…
      </div>
    );
  }
  if (formats.length === 0) {
    return (
      <div data-testid="formats-empty" style={{ padding: space.md, display: "grid", gap: space.md }}>
        <div style={{ fontSize: fontSize.meta, color: color.warn.text }}>
          This build declares no import or export formats. That is a build problem, not an empty
          project — the registry is compiled in.
        </div>
        <ColourCard client={client} />
      </div>
    );
  }

  // Grouped by domain so a CAD user and a game user each find their own section.
  const domains = [...new Set(formats.map((f) => f.domain))];

  return (
    <div data-testid="formats-panel" style={{ padding: space.md, display: "grid", gap: space.md }}>
      <div style={{ fontSize: fontSize.meta, color: color.text.secondary }}>
        {formats.filter((f) => f.available).length} of {formats.length} formats are available in this
        build. Every entry says what it carries and what it leaves behind.
      </div>

      <ColourCard client={client} />

      {domains.map((domain) => (
        <section key={domain} style={{ display: "grid", gap: space.xs }}>
          <h3
            style={{
              margin: 0,
              font: font.ui,
              fontSize: fontSize.body,
              color: color.text.primary,
            }}
          >
            {domain}
          </h3>

          {formats
            .filter((f) => f.domain === domain)
            .map((f) => {
              const fid = FIDELITY_COPY[f.fidelity] ?? { label: f.fidelity, hint: "" };
              const isOpen = open === f.id;
              const carried = CARRIES.filter((c) => f.carries[c.key]).map((c) => c.label);
              return (
                <div
                  key={f.id}
                  data-testid={`format-${f.id}`}
                  data-available={f.available}
                  style={{
                    border: `1px solid ${color.border.subtle}`,
                    borderRadius: radius.md,
                    padding: `${space.sm}px ${space.md}px`,
                    // Unavailable is dimmed, NOT hidden: the user still needs to learn the engine
                    // knows the format.
                    opacity: f.available ? 1 : 0.55,
                    display: "grid",
                    gap: space.xxs,
                  }}
                >
                  <div style={{ display: "flex", alignItems: "baseline", gap: space.sm, flexWrap: "wrap" }}>
                    <span style={{ fontSize: fontSize.body, color: color.text.primary }}>{f.label}</span>
                    <span style={{ fontSize: fontSize.meta, color: color.text.muted }}>
                      {f.extensions.map((e) => `.${e}`).join(" ")}
                    </span>
                    <span
                      data-testid={`format-${f.id}-direction`}
                      style={{ fontSize: fontSize.meta, color: color.accent.base }}
                    >
                      {DIRECTION_COPY[f.direction] ?? f.direction}
                    </span>
                    <span
                      data-testid={`format-${f.id}-fidelity`}
                      title={fid.hint}
                      style={{
                        fontSize: fontSize.meta,
                        padding: "0 6px",
                        borderRadius: radius.sm,
                        border: `1px solid ${color.border.subtle}`,
                        color: color.text.secondary,
                      }}
                    >
                      {fid.label}
                    </span>
                    {!f.available && (
                      <span style={{ fontSize: fontSize.meta, color: color.warn.text }}>
                        not in this build
                      </span>
                    )}
                    <Button
                      variant="ghost"
                      compact
                      aria-expanded={isOpen}
                      aria-label={`${isOpen ? "Hide" : "Show"} what ${f.label} carries`}
                      onClick={() => setOpen(isOpen ? null : f.id)}
                      style={{ marginLeft: "auto" }}
                    >
                      {isOpen ? "Less" : "Details"}
                    </Button>
                  </div>

                  <div style={{ fontSize: fontSize.meta, color: color.text.secondary }}>
                    {carried.length > 0 ? `Carries ${carried.join(", ")}.` : "Carries no scene data."}
                  </div>

                  {isOpen && (
                    <div
                      data-testid={`format-${f.id}-note`}
                      style={{
                        fontSize: fontSize.meta,
                        color: color.text.secondary,
                        background: color.bg.inset,
                        border: `1px solid ${color.border.subtle}`,
                        borderRadius: radius.sm,
                        padding: `${space.xs}px ${space.sm}px`,
                      }}
                    >
                      {f.note}
                    </div>
                  )}
                </div>
              );
            })}
        </section>
      ))}
    </div>
  );
}

export default FormatsPanel;
