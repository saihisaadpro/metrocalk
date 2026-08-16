//! Behaviour — the Inspector's front door for "make it DO something" (Engine UI/UX Constitution).
//!
//! ADR-107 puts the SELECTION in the right dock — so the moment an asset is clicked, this block
//! offers its behaviour: the full role catalog as one-tap cards (one undoable commit each), the
//! current role held up front, and jump links into the deeper workspaces (Animate for keyframes,
//! Gameplay for the roster + live score). No hunting through engine tabs to give a thing a job.

import { useEffect, useState } from "react";
import { useDisplayedEntity, useSelectedId, useSummary } from "../store/projection";
import { usePlaying } from "../store/play";
import { setStatus } from "../store/ui";
import { pushToast } from "../store/toasts";
import { Button, NumericField } from "../theme/primitives";
import { color, font, fontSize, radius, space } from "../theme/tokens";
import { OnlyIfBlock } from "./OnlyIfBlock";
import type { RoleReply, RoleRow, RoleSpec } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

/** One friendly tuning knob over a `GameRole` field — the plain-language face of the data. */
interface TuneSpec {
  field: string;
  label: string;
  hint: string;
  step: number;
  min: number;
  max: number;
  integer?: boolean;
  fallback: number;
}

/** What each role lets you tune, in the author's language (values live on `GameRole`). */
const TUNING: Record<string, TuneSpec[]> = {
  collectible: [
    { field: "radius", label: "Touch reach", hint: "how close something must come to collect it, metres", step: 0.1, min: 0.2, max: 10, fallback: 0.9 },
    { field: "points", label: "Points", hint: "what collecting it adds to the Score", step: 1, min: 1, max: 100, integer: true, fallback: 1 },
  ],
  companion: [
    { field: "speed", label: "Move speed", hint: "metres per second", step: 0.1, min: 0.5, max: 10, fallback: 2.5 },
    { field: "range", label: "Attack reach", hint: "how close it must be to strike, metres", step: 0.1, min: 0.5, max: 5, fallback: 1.4 },
    { field: "follow", label: "Follow distance", hint: "how far behind you it heels, metres", step: 0.1, min: 0.8, max: 8, fallback: 2 },
    { field: "radius", label: "Aggro reach", hint: "it notices enemies within this range, metres", step: 0.5, min: 1, max: 20, fallback: 6 },
  ],
  enemy: [
    { field: "points", label: "Points", hint: "what beating it adds to the Score", step: 1, min: 1, max: 100, integer: true, fallback: 1 },
  ],
  waypoint: [
    { field: "points", label: "Patrol stop #", hint: "the order companions walk the chain", step: 1, min: 1, max: 50, integer: true, fallback: 1 },
  ],
  player: [
    { field: "speed", label: "Move speed", hint: "metres per second", step: 0.1, min: 0.5, max: 10, fallback: 3.2 },
  ],
};

export interface BehaviourSectionProps {
  client: EditorClient;
  /** Jump to a deeper workspace in the left rail ("animate" | "gameplay"). */
  onJumpTo?: (engine: "animate" | "gameplay") => void;
}

export function BehaviourSection({ client, onJumpTo }: BehaviourSectionProps) {
  const selected = useSelectedId();
  const summary = useSummary(selected ?? "");
  const entity = useDisplayedEntity(selected ?? "");
  const playing = usePlaying();
  const [specs, setSpecs] = useState<RoleSpec[]>([]);
  const [currentRole, setCurrentRole] = useState<string | null>(null);
  const [roster, setRoster] = useState<RoleRow[]>([]);
  const [blockedWhy, setBlockedWhy] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const [busy, setBusy] = useState(false);
  const [refusal, setRefusal] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    client
      .roleCatalog()
      .then((list) => live && setSpecs(list))
      .catch((e: unknown) => console.error("role_catalog failed", e));
    return () => {
      live = false;
    };
  }, [client]);

  useEffect(() => {
    let live = true;
    setRefusal(null);
    if (!selected) {
      setCurrentRole(null);
      return undefined;
    }
    client
      .roleStatus()
      .then((info) => {
        if (!live) return;
        setCurrentRole(info.roster.find((r) => r.entity === selected)?.role ?? null);
        setRoster(info.roster);
      })
      .catch((e: unknown) => console.error("role_status failed", e));
    return () => {
      live = false;
    };
  }, [client, selected, refreshKey]);

  // While playing, poll for the near-miss line — the answer to "nothing happened". The chrome cadence
  // (500 ms), never a per-frame read, and only while a run is actually going.
  useEffect(() => {
    if (!playing) {
      setBlockedWhy(null);
      return undefined;
    }
    let live = true;
    const tick = () => {
      client
        .roleStatus()
        .then((info) => live && setBlockedWhy(info.blocked?.why ?? null))
        .catch(() => {});
    };
    tick();
    const timer = setInterval(tick, 500);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [client, playing]);

  if (!selected) return null;

  async function run(action: () => Promise<RoleReply>, verb: string) {
    setBusy(true);
    setRefusal(null);
    try {
      const reply = await action();
      if (reply.reason) {
        setRefusal(reply.reason);
        pushToast(reply.reason, "error");
        setStatus(`${verb} refused: ${reply.reason}`);
      } else {
        pushToast(`${reply.message} · Ctrl-Z to undo`, "success");
        setStatus(reply.message);
        setCurrentRole(reply.applied ?? null);
        setRefreshKey((k) => k + 1);
      }
    } catch (e) {
      console.error(`${verb} failed`, e);
      pushToast(`${verb} failed — please try again`, "error");
    } finally {
      setBusy(false);
    }
  }

  const held = specs.find((s) => s.kind === currentRole) ?? null;
  // "doesn't do anything yet" was a lie for a burning crate or an object with a cutscene: this block
  // only knew about ROLES, so an object carrying effects or shots was told it was inert while the
  // panels directly below it listed exactly what it does. Seen in a live capture.
  const alsoDoes = [
    entity?.components?.Vfx ? "an effect" : null,
    entity?.components?.Cinematic ? "a cutscene" : null,
  ].filter(Boolean) as string[];

  return (
    <section
      data-testid="behaviour-section"
      style={{
        margin: `${space.sm}px 0`,
        padding: space.md,
        background: color.accent.subtle,
        border: `1px solid ${color.accent.border}`,
        borderRadius: radius.md,
        display: "grid",
        gap: space.sm,
      }}
    >
      <div style={{ display: "flex", alignItems: "baseline", gap: space.sm }}>
        <h3 style={{ margin: 0, font: font.ui, fontSize: fontSize.body, color: color.text.primary }}>
          ✨ Behaviour
        </h3>
        <span data-testid="behaviour-state" style={{ fontSize: fontSize.meta, color: color.text.muted }}>
          {held
            ? `${held.icon} ${summary?.name ?? "This object"} is a ${held.label}`
            : alsoDoes.length > 0
              ? `${summary?.name ?? "This object"} has ${alsoDoes.join(" and ")}, but no gameplay job yet:`
              : `${summary?.name ?? "This object"} doesn't do anything yet — give it a job:`}
        </span>
      </div>

      <div
        role="group"
        aria-label="Assign a behaviour"
        style={{ display: "grid", gridTemplateColumns: "repeat(2, 1fr)", gap: space.xs }}
      >
        {specs.map((spec) => (
          <Button
            key={spec.kind}
            data-testid={`behaviour-${spec.kind}`}
            variant={currentRole === spec.kind ? "toggle" : "secondary"}
            active={currentRole === spec.kind}
            compact
            disabled={busy || playing}
            title={
              playing
                ? "Stop Play first — behaviour is authored, not live-edited"
                : `${spec.blurb}. Adds: ${spec.adds}. One step — Ctrl-Z undoes it.`
            }
            onClick={() =>
              selected && void run(() => client.roleAssign(selected, spec.kind), spec.label)
            }
          >
            {spec.icon} {spec.label}
          </Button>
        ))}
      </div>

      {held && (TUNING[held.kind]?.length ?? 0) > 0 && (
        <div data-testid="behaviour-tuning" style={{ display: "grid", gap: space.xs }}>
          <div style={{ fontSize: fontSize.meta, color: color.text.muted }}>
            Tune it — every change is one step, Ctrl-Z undoes it:
          </div>
          <div style={{ display: "grid", gridTemplateColumns: "repeat(2, 1fr)", gap: space.xs }}>
            {(TUNING[held.kind] ?? []).map((knob) => {
              const raw = (entity?.components as Record<string, Record<string, unknown>> | undefined)?.GameRole?.[knob.field];
              const value = typeof raw === "number" && raw > 0 ? raw : knob.fallback;
              return (
                <label key={knob.field} style={{ display: "grid", gap: 2, fontSize: fontSize.meta, color: color.text.secondary }}>
                  {knob.label}
                  <NumericField
                    data-testid={`tune-${knob.field}`}
                    ariaLabel={`${knob.label} — ${knob.hint}`}
                    title={knob.hint}
                    value={value}
                    step={knob.step}
                    integer={knob.integer}
                    min={knob.min}
                    max={knob.max}
                    disabled={playing}
                    onCommit={(next) => {
                      if (!selected || next === value) return;
                      client.setField(selected, "GameRole", knob.field, knob.integer ? Math.round(next) : next);
                      pushToast(`${knob.label} → ${knob.integer ? Math.round(next) : next} · Ctrl-Z to undo`, "success");
                    }}
                  />
                </label>
              );
            })}
          </div>
        </div>
      )}

      {held && (
        <OnlyIfBlock
          client={client}
          roster={roster}
          blockedWhy={blockedWhy}
          refreshKey={refreshKey}
        />
      )}

      <div style={{ display: "flex", gap: space.sm, alignItems: "center" }}>
        {held && (
          <Button
            data-testid="behaviour-clear"
            variant="ghost"
            compact
            disabled={busy || playing}
            title="Remove the role and everything it added — one step, Ctrl-Z undoes it"
            onClick={() => selected && void run(() => client.roleClear(selected), "Clear")}
          >
            Clear role
          </Button>
        )}
        {onJumpTo && (
          <>
            <Button
              data-testid="behaviour-jump-animate"
              variant="ghost"
              compact
              title="Open the Animate workspace for keyframe timelines on this object"
              onClick={() => onJumpTo("animate")}
            >
              Animate ▸
            </Button>
            <Button
              data-testid="behaviour-jump-gameplay"
              variant="ghost"
              compact
              title="Open the Gameplay workspace — the full roster, live score and match tools"
              onClick={() => onJumpTo("gameplay")}
            >
              Gameplay ▸
            </Button>
          </>
        )}
      </div>

      {refusal && (
        <div
          data-testid="behaviour-refusal"
          role="status"
          style={{
            padding: `${space.xs}px ${space.sm}px`,
            fontSize: fontSize.meta,
            color: color.danger.text,
            background: color.danger.bg,
            border: `1px solid ${color.danger.border}`,
            borderRadius: radius.sm,
          }}
        >
          {refusal}
        </div>
      )}
    </section>
  );
}
