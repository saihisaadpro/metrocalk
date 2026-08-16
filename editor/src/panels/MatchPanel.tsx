/**
 * The match workspace: author a match, see whether it is valid, run it, and watch it.
 *
 * The match kernel was reachable only from automated tests for a long time — real, deterministic, and
 * completely undiscoverable. This panel is the surface that makes it a feature rather than an internal
 * capability, and it is built around the one thing that was actually missing: the authored scene is the
 * source of truth, so the panel shows the *authoring* state first and the running state second.
 *
 * Three rules it follows deliberately:
 *
 *  - **Validation is continuous, not start-time.** The panel re-validates whenever the document changes, so
 *    a broken scene says so while it is being edited rather than at the moment someone presses Start.
 *  - **A refusal names the object.** Every diagnostic is a button that selects the entity at fault. "Wave
 *    ticks must be positive and future" is useless without knowing which wave.
 *  - **Runtime status is never colour alone.** A stunned hero renders yellow in the viewport, but here the
 *    control is also spelled out by name, so the state survives a greyscale or colour-blind reading.
 */

import { useCallback, useEffect, useState } from "react";
import { projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import { Button } from "../theme/primitives";
import { color, font, fontSize, radius, space } from "../theme/tokens";
import { DisclosureSection, EmptyPanelState } from "../theme/workspace";
import { RolesSection } from "./RolesSection";
import { CinemaSection } from "./CinemaSection";
import { VfxSection } from "./VfxSection";
import type { CookDiagnostic, MatchStatus, MatchValidation } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

const IDLE: MatchValidation = {
  ok: false,
  is_match_scene: false,
  diagnostics: [],
  cook_digest: null,
  actor_count: 0,
  wave_count: 0,
  lane_length_m: 0,
};

const row: React.CSSProperties = {
  display: "flex",
  flexWrap: "wrap",
  alignItems: "center",
  gap: space.sm,
};

const meta: React.CSSProperties = {
  margin: 0,
  color: color.text.muted,
  font: font.ui,
  fontSize: fontSize.meta,
  lineHeight: 1.45,
};

const summaryGrid: React.CSSProperties = {
  display: "grid",
  gridTemplateColumns: "repeat(auto-fit, minmax(120px, 1fr))",
  gap: space.sm,
  margin: `${space.sm} 0`,
};

const statTile: React.CSSProperties = {
  padding: space.sm,
  border: `1px solid ${color.border.subtle}`,
  borderRadius: radius.sm,
  font: font.ui,
};

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div style={statTile}>
      <div style={{ ...meta, fontWeight: 600 }}>{label}</div>
      <div style={{ color: color.text.primary, fontSize: fontSize.body }}>{value}</div>
    </div>
  );
}

/** One cook diagnostic, as a button that takes the author to the object at fault. */
function Diagnostic({ diagnostic }: { diagnostic: CookDiagnostic }) {
  const isError = diagnostic.severity === "error";
  const where = [diagnostic.component, diagnostic.field].filter(Boolean).join(".");
  const select = () => {
    if (diagnostic.entity) projectionStore.getState().select(diagnostic.entity);
  };
  return (
    <li
      style={{
        display: "flex",
        gap: space.sm,
        alignItems: "flex-start",
        padding: space.sm,
        borderLeft: `3px solid ${isError ? color.danger.text : color.warn.text}`,
        background: color.bg.inset,
        borderRadius: radius.sm,
        marginBottom: space.xs,
      }}
    >
      {/* A word, not only a colour — the severity has to survive a greyscale reading. */}
      <span
        style={{ ...meta, fontWeight: 700, color: isError ? color.danger.text : color.warn.text, textTransform: "uppercase", flexShrink: 0 }}
      >
        {isError ? "Fix" : "Note"}
      </span>
      <span style={{ flex: 1, minWidth: 0 }}>
        <span style={{ font: font.ui, fontSize: fontSize.body, color: color.text.primary }}>{diagnostic.message}</span>
        {where ? <span style={{ ...meta, display: "block" }}>{where}</span> : null}
      </span>
      {diagnostic.entity ? (
        <Button variant="ghost" onClick={select} title="Select the object this refers to">
          Show me
        </Button>
      ) : null}
    </li>
  );
}

export function MatchPanel({ client }: { client: EditorClient }) {
  const [validation, setValidation] = useState<MatchValidation>(IDLE);
  const [status, setMatchStatus] = useState<MatchStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [startError, setStartError] = useState<CookDiagnostic[] | null>(null);

  const revalidate = useCallback(async () => {
    const next = await client.matchValidate().catch(() => IDLE);
    setValidation(next);
  }, [client]);

  // Re-validate whenever the document changes. This is what makes the feedback continuous: an author
  // dragging a hero off the lane sees the refusal appear as they drag, not when they press Start.
  useEffect(() => {
    void revalidate();
    return projectionStore.subscribe(() => {
      void revalidate();
    });
  }, [revalidate]);

  const running = status?.running ?? false;

  const guard = async (label: string, work: () => Promise<void>) => {
    setBusy(true);
    try {
      await work();
    } catch (error) {
      // A start refusal carries the cook's diagnostics; anything else is reported as-is rather than
      // swallowed. Never a silent no-op on a button the user pressed.
      const diagnostics = (error as { diagnostics?: CookDiagnostic[] })?.diagnostics;
      const message = (error as { message?: string })?.message ?? String(error);
      if (diagnostics?.length) setStartError(diagnostics);
      setStatus(`${label}: ${message}`);
    } finally {
      setBusy(false);
    }
  };

  const create = () =>
    guard("Could not create the match", async () => {
      await client.matchAuthorStarter();
      await revalidate();
      setStatus("Starter match created — edit any object to change how it plays, then press Start.");
    });

  const start = () =>
    guard("Could not start the match", async () => {
      setStartError(null);
      setMatchStatus(await client.matchStart());
      setStatus("Match running — the viewport is showing the simulation, not your scene.");
    });

  const step = (ticks: number) =>
    guard("Could not advance the match", async () => {
      setMatchStatus(await client.matchStep(ticks));
    });

  const stun = () =>
    guard("Could not apply the effect", async () => {
      setMatchStatus(await client.matchStun(40));
    });

  // ── standing orders (GP-08) ────────────────────────────────────────────────────────────────────────
  // One order, then the hero fights on its own. The destination and the target are computed from what is
  // actually on the field rather than typed in, because a coordinate box would make the feature look like
  // a debug tool instead of the thing a player does every few seconds.
  const attackMove = () =>
    guard("Could not give that order", async () => {
      setMatchStatus(await client.matchAttackMove(pushTarget.x, pushTarget.y));
      setStatus("Attack-move given — your hero advances and fights what it meets, on its own.");
    });

  const attackNearest = () =>
    guard("Could not give that order", async () => {
      if (!nearestEnemy) return;
      setMatchStatus(await client.matchAttackTarget(nearestEnemy.id));
      setStatus(`Locked onto ${nearestEnemy.kind.toLowerCase()} #${nearestEnemy.id} — it will not switch targets.`);
    });

  const cast = () =>
    guard("Could not cast", async () => {
      if (!nearestEnemy) return;
      setMatchStatus(await client.matchCast(nearestEnemy.id));
      setStatus(`Cast at ${nearestEnemy.kind.toLowerCase()} #${nearestEnemy.id}.`);
    });

  const hold = () =>
    guard("Could not give that order", async () => {
      setMatchStatus(await client.matchHold());
      setStatus("Holding — your hero stays put and hits whatever comes into range.");
    });

  const halt = () =>
    guard("Could not give that order", async () => {
      setMatchStatus(await client.matchHalt());
      setStatus("Orders cleared — your hero stops moving and stops fighting.");
    });

  const stop = () =>
    guard("Could not stop the match", async () => {
      setMatchStatus(await client.matchStop());
      await revalidate();
      setStatus("Match stopped — your authored scene is back exactly as you left it.");
    });

  // ── not a match scene yet ────────────────────────────────────────────────────────────────────────────
  if (!validation.is_match_scene) {
    return (
      <div style={{ padding: space.md, display: "flex", flexDirection: "column", gap: space.md }}>
        <RolesSection client={client} />
        <CinemaSection client={client} />
        <VfxSection client={client} />
        <EmptyPanelState
          icon="⚔"
          title="This scene doesn't have a match yet"
          description="A match is authored like anything else: a play area, a lane to walk, the actors that fight over it, and the waves that spawn. Create one to get a complete, playable starting point you can then edit."
        />
        <div style={{ ...row, justifyContent: "center", marginTop: space.md }}>
          <Button variant="primary" onClick={create} disabled={busy}>
            Create a starter match
          </Button>
        </div>
        <p style={{ ...meta, textAlign: "center", marginTop: space.sm }}>
          Everything it creates is ordinary scene objects — move them with the gizmo, change their numbers
          in the inspector, and undo it in one step.
        </p>
      </div>
    );
  }

  const errors = validation.diagnostics.filter((d) => d.severity === "error");
  const warnings = validation.diagnostics.filter((d) => d.severity === "warning");
  const hero = status?.actors.find((a) => a.owned) ?? null;
  // Everything hostile to the hero that is still standing, nearest first.
  const enemies = (status?.actors ?? [])
    .filter((a) => a.alive && hero !== null && a.team !== hero.team)
    .sort((a, b) => {
      const da = hero ? (a.x_mm - hero.x_mm) ** 2 + (a.y_mm - hero.y_mm) ** 2 : 0;
      const db = hero ? (b.x_mm - hero.x_mm) ** 2 + (b.y_mm - hero.y_mm) ** 2 : 0;
      return da - db || a.id - b.id;
    });
  const nearestEnemy = enemies[0] ?? null;
  // `null` means the hero was authored with no ability at all, which is a different thing from a
  // cooldown and must not render as one.
  const abilityReady = hero?.ability_ready_in === 0;
  // Push toward the enemy's furthest structure — the thing a lane exists to reach. Falls back to the
  // furthest hostile of any kind, and finally to a step ahead, so the button is never dead.
  const pushTarget = (() => {
    const structures = enemies.filter((a) => a.kind === "Structure" || a.kind === "Objective");
    const goal = structures[structures.length - 1] ?? enemies[enemies.length - 1] ?? null;
    if (goal) return { x: goal.x_mm, y: goal.y_mm };
    return { x: (hero?.x_mm ?? 0) + 10_000, y: hero?.y_mm ?? 0 };
  })();

  return (
    <div style={{ padding: space.md, display: "flex", flexDirection: "column", gap: space.md }}>
      <RolesSection client={client} />
      <CinemaSection client={client} />
      <VfxSection client={client} />
      {/* ── the authored scene ─────────────────────────────────────────────────────────────────────── */}
      <section>
        <h3 style={{ margin: `0 0 ${space.xs}`, font: font.ui, fontSize: fontSize.body, color: color.text.primary }}>
          Your match
        </h3>
        <div style={summaryGrid}>
          <Stat label="Actors" value={String(validation.actor_count)} />
          <Stat label="Waves" value={String(validation.wave_count)} />
          <Stat label="Lane" value={`${validation.lane_length_m.toFixed(1)} m`} />
          <Stat
            label="Ready to run"
            value={validation.ok ? "Yes" : `No — ${errors.length} to fix`}
          />
        </div>
        {validation.cook_digest ? (
          <p style={meta} title="Two runs sharing this fingerprint were built from identical definitions.">
            Definitions fingerprint <code>{validation.cook_digest}</code>
          </p>
        ) : null}
      </section>

      {/* ── what is wrong, and where ───────────────────────────────────────────────────────────────── */}
      {errors.length > 0 || warnings.length > 0 || startError ? (
        <section>
          <h3 style={{ margin: `0 0 ${space.xs}`, font: font.ui, fontSize: fontSize.body, color: color.text.primary }}>
            {errors.length > 0 ? "Fix these before running" : "Worth knowing"}
          </h3>
          <ul style={{ listStyle: "none", margin: 0, padding: 0 }}>
            {(startError ?? validation.diagnostics).map((diagnostic, index) => (
              <Diagnostic key={`${diagnostic.code}-${diagnostic.entity ?? "scene"}-${index}`} diagnostic={diagnostic} />
            ))}
          </ul>
        </section>
      ) : null}

      {/* ── transport ──────────────────────────────────────────────────────────────────────────────── */}
      <section style={row}>
        {running ? (
          <>
            <Button onClick={() => step(1)} disabled={busy}>Step 1</Button>
            <Button onClick={() => step(30)} disabled={busy}>Step 30</Button>
            <Button onClick={() => step(120)} disabled={busy} title="Four seconds of match at 30 ticks per second">
              Step 120
            </Button>
            <Button onClick={stun} disabled={busy} title="Apply a stun to your hero — it halves speed and holds position">
              Stun hero
            </Button>
            <Button variant="danger" onClick={stop} disabled={busy}>Stop</Button>
          </>
        ) : (
          <Button
            variant="primary"
            onClick={start}
            disabled={busy || !validation.ok}
            title={validation.ok ? "Run this scene as a match" : "Fix the problems above first"}
          >
            Start match
          </Button>
        )}
      </section>
      {running ? (
        <p style={meta}>
          The viewport is showing the running match. Your scene is untouched and comes back when you stop.
        </p>
      ) : null}

      {/* ── the running match ──────────────────────────────────────────────────────────────────────── */}
      {status && running ? (
        <section>
          <div style={summaryGrid}>
            <Stat label="Tick" value={String(status.tick)} />
            <Stat label="Phase" value={status.phase} />
            <Stat label="Alive" value={`${status.live_actors} / ${status.actor_count}`} />
            <Stat
              label="Your hero"
              value={hero ? `${(hero.x_mm / 1000).toFixed(2)} m` : "—"}
            />
          </div>
          {hero ? (
            <p style={meta}>
              Speed {hero.speed} mm/tick ·{" "}
              {/* Spelled out, so the viewport's colour is a reinforcement rather than the only signal. */}
              {hero.controls.length > 0 ? `affected by ${hero.controls.join(", ")}` : "no active effects"}
              {hero.source ? " · authored object linked" : ""}
            </p>
          ) : null}
          {/* ── standing orders ─────────────────────────────────────────────────────────────────── */}
          <div style={{ marginTop: space.sm }} data-testid="match-orders">
            <h4 style={{ margin: `0 0 ${space.xs}`, font: font.ui, fontSize: fontSize.body, color: color.text.primary }}>
              Orders
            </h4>
            <p style={meta}>
              An order stays given. Your hero keeps fighting under it until you replace it or clear it —
              you do not press a button per swing.
            </p>
            <div style={{ ...row, marginTop: space.xs }}>
              <Button
                onClick={attackMove}
                disabled={busy}
                data-testid="order-attack-move"
                title={`Advance to ${(pushTarget.x / 1000).toFixed(1)}, ${(pushTarget.y / 1000).toFixed(1)} m and fight what it meets`}
              >
                Attack-move down the lane
              </Button>
              <Button
                onClick={attackNearest}
                disabled={busy || !nearestEnemy}
                data-testid="order-attack-target"
                title={
                  nearestEnemy
                    ? `Lock onto ${nearestEnemy.kind.toLowerCase()} #${nearestEnemy.id} until it is gone`
                    : "Nothing hostile is left to attack"
                }
              >
                Attack nearest enemy
              </Button>
              <Button onClick={hold} disabled={busy} data-testid="order-hold" title="Stay put and hit whatever comes into range">
                Hold position
              </Button>
              <Button onClick={halt} disabled={busy} data-testid="order-halt" title="Stop moving and stop fighting">
                Clear orders
              </Button>
            </div>
            {/* ── the ability ─────────────────────────────────────────────────────────────────── */}
            <div style={{ ...row, marginTop: space.xs }}>
              <Button
                variant="primary"
                onClick={cast}
                disabled={busy || !nearestEnemy || !abilityReady}
                data-testid="order-cast"
                title={
                  hero?.ability_ready_in === null || hero?.ability_ready_in === undefined
                    ? "This hero has no ability authored on it — add one in the inspector"
                    : !nearestEnemy
                      ? "Nothing hostile is left to cast at"
                      : abilityReady
                        ? `Cast at ${nearestEnemy.kind.toLowerCase()} #${nearestEnemy.id}`
                        : `Cooling down — ready in ${hero?.ability_ready_in} ticks`
                }
              >
                {abilityReady ? "Cast ability" : `Cast ability (${hero?.ability_ready_in ?? "—"})`}
              </Button>
              <p style={{ ...meta, margin: 0, alignSelf: "center" }} data-testid="ability-state">
                {hero?.ability_ready_in === null || hero?.ability_ready_in === undefined
                  ? "No ability authored on this hero."
                  : abilityReady
                    ? "Ability ready."
                    : `Ability cooling down: ${hero.ability_ready_in} ticks.`}
              </p>
            </div>
            <p style={{ ...meta, marginTop: space.xs }} data-testid="standing-order">
              {/* Worded by the kernel, so this line can never claim behaviour the runtime does not have. */}
              {hero?.attack_order
                ? `Standing order: ${hero.attack_order}`
                : "Standing order: none — your hero acts only when you tell it to."}
            </p>
          </div>
          {status.last_rejection ? (
            <p style={{ ...meta, color: color.warn.text }}>
              Last order refused: {status.last_rejection}
            </p>
          ) : null}
          <DisclosureSection
            title="Determinism"
            summary="Fingerprints for this run"
            defaultOpen={false}
            storageKey="match-determinism"
          >
            <p style={meta}>
              World <code>{status.world_digest}</code>
              <br />
              Lane <code>{status.lane_digest}</code>
              <br />
              Built from definitions <code>{status.cook_digest}</code> (schema v{status.cook_schema_version})
            </p>
          </DisclosureSection>
        </section>
      ) : null}
    </div>
  );
}
