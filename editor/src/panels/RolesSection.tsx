//! Roles — the Gameplay panel's asset→gameplay bridge (implemented per the Engine UI/UX Constitution).
//!
//! Select any object, click a role card, and ONE undoable commit makes it a live gameplay
//! participant: a Collectible gets its spin, its touch trigger, the shared pickup rule and a visible
//! binding to the Score; Solid/Prop get real physics; Spinner gets ambient motion. During Play the
//! score readout here is LIVE from the rules runtime — the loop the roles exist to close.

import { useEffect, useState } from "react";
import { projectionStore, useSelectedId, useSummary } from "../store/projection";
import { usePlaying } from "../store/play";
import { setStatus } from "../store/ui";
import { pushToast } from "../store/toasts";
import { Button } from "../theme/primitives";
import { color, font, fontSize, radius, space } from "../theme/tokens";
import type { RoleReply, RoleSpec, RoleStatusInfo } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

const sectionH3 = {
  margin: `0 0 ${space.xs}px`,
  font: font.ui,
  fontSize: fontSize.body,
  color: color.text.primary,
} as const;

const metaText = {
  fontSize: fontSize.meta,
  color: color.text.muted,
} as const;

export function RolesSection({ client }: { client: EditorClient }) {
  const selected = useSelectedId();
  const summary = useSummary(selected ?? "");
  const playing = usePlaying();
  const [specs, setSpecs] = useState<RoleSpec[]>([]);
  const [status, setRoles] = useState<RoleStatusInfo>({ roster: [], score: 0, scoreEntity: null, remaining: 0, companions: [], won: false, health: null, blocked: null });
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

  // Roster on mount + selection change; a 500 ms score poll ONLY during Play (the chrome cadence
  // discipline — never a per-frame read).
  useEffect(() => {
    let live = true;
    const read = () => {
      void client
        .roleStatus()
        .then((info) => live && setRoles(info))
        .catch(() => {});
    };
    read();
    if (!playing) return () => {
      live = false;
    };
    const timer = setInterval(read, 500);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [client, playing, selected]);

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
        const info = await client.roleStatus();
        setRoles(info);
      }
    } catch (e) {
      console.error(`${verb} failed`, e);
      pushToast(`${verb} failed — please try again`, "error");
    } finally {
      setBusy(false);
    }
  }

  const currentRole = selected ? status.roster.find((r) => r.entity === selected)?.role ?? null : null;

  return (
    <section data-testid="roles-section">
      <h3 style={sectionH3}>Roles</h3>

      {playing && (
        <div
          data-testid="role-score"
          style={{
            display: "flex",
            gap: space.md,
            alignItems: "baseline",
            padding: `${space.sm}px ${space.md}px`,
            background: color.accent.subtle,
            border: `1px solid ${color.accent.border}`,
            borderRadius: radius.md,
            marginBottom: space.sm,
          }}
        >
          <span style={{ fontSize: fontSize.title, fontWeight: 700, color: color.accent.base }}>
            ★ {status.score}
          </span>
          <span style={metaText}>
            {/* "all collected!" is only true if there was ever anything to collect. A scene with no
                collectibles at all was congratulating the player for doing nothing — seen in a live
                capture of a cutscene scene, which has a statue and no coins. */}
            {status.remaining > 0
              ? `${status.remaining} collectible${status.remaining === 1 ? "" : "s"} left`
              : status.roster.some((r) => r.role === "collectible")
                ? "all collected!"
                : "nothing to collect in this scene yet"}
          </span>
        </div>
      )}

      {playing && status.won && (
        <div
          data-testid="role-victory"
          style={{
            padding: `${space.sm}px ${space.md}px`,
            background: color.accent.subtle,
            border: `1px solid ${color.accent.border}`,
            borderRadius: radius.md,
            marginBottom: space.sm,
            fontWeight: 700,
            color: color.accent.base,
          }}
        >
          🏆 You won! Every enemy beaten, every crystal collected · Stop to keep building
        </div>
      )}

      {playing && status.health && (
        <div
          data-testid="roles-health"
          style={{
            display: "flex",
            gap: space.sm,
            alignItems: "baseline",
            padding: `${space.xs}px ${space.md}px`,
            marginBottom: space.sm,
            fontSize: fontSize.meta,
            color: color.text.secondary,
          }}
        >
          <span style={{ fontSize: fontSize.body }}>
            {"♥".repeat(Math.max(0, status.health.hp))}
            <span style={{ opacity: 0.25 }}>
              {"♥".repeat(Math.max(0, status.health.maxHp - status.health.hp))}
            </span>
          </span>
          <span>
            {status.health.name} · {status.health.hp}/{status.health.maxHp}
            {status.health.hp <= 0 ? " — out of the game" : ""}
          </span>
        </div>
      )}

      {playing && status.blocked && (
        <div
          data-testid="roles-blocked"
          role="status"
          style={{
            padding: `${space.sm}px ${space.md}px`,
            background: color.warn.bg,
            border: `1px solid ${color.warn.border}`,
            borderRadius: radius.md,
            marginBottom: space.sm,
            fontSize: fontSize.meta,
            color: color.warn.text,
          }}
        >
          ⛔ {status.blocked.name || "Something"} didn&apos;t respond — {status.blocked.why}
        </div>
      )}

      {playing && status.companions.length > 0 && (
        <div data-testid="companion-status" style={{ display: "grid", gap: 2, marginBottom: space.sm }}>
          {status.companions.map((c) => (
            <div key={c.entity} style={metaText}>
              ♥ {c.name} — {c.doing || "waking up"}
            </div>
          ))}
        </div>
      )}

      {selected ? (
        <div style={{ display: "grid", gap: space.xs }}>
          <div style={{ fontSize: fontSize.meta, color: color.text.secondary }}>
            {summary?.name ?? selected}
            {currentRole && (
              <span style={{ color: color.accent.base }}> · {currentRole}</span>
            )}
          </div>
          <div role="group" aria-label="Assign a role" style={{ display: "grid", gridTemplateColumns: "repeat(2, 1fr)", gap: space.xs }}>
            {specs.map((spec) => (
              <Button
                key={spec.kind}
                data-testid={`role-${spec.kind}`}
                variant={currentRole === spec.kind ? "toggle" : "secondary"}
                active={currentRole === spec.kind}
                compact
                disabled={busy || playing}
                title={
                  playing
                    ? "Stop Play first — roles are authored, not live-edited"
                    : `${spec.blurb}. Adds: ${spec.adds} — one Ctrl-Z removes it all`
                }
                onClick={() => selected && void run(() => client.roleAssign(selected, spec.kind), `Make it a ${spec.label}`)}
              >
                {spec.icon} {spec.label}
              </Button>
            ))}
          </div>
          {currentRole && (
            <Button
              data-testid="role-clear"
              variant="ghost"
              compact
              disabled={busy || playing}
              title={playing ? "Stop Play first" : "Remove the role and what it added (mesh and transform stay)"}
              onClick={() => selected && void run(() => client.roleClear(selected), "Clear role")}
            >
              Clear role
            </Button>
          )}
        </div>
      ) : (
        <div data-testid="roles-hint" style={metaText}>
          Select an object, then give it a role — it becomes part of the game in one click.
        </div>
      )}

      {status.roster.length > 0 && (
        <div data-testid="roles-roster" style={{ marginTop: space.sm, display: "grid", gap: 2 }}>
          {status.roster.map((row) => (
            <Button
              key={row.entity}
              variant="ghost"
              compact
              data-testid="role-row"
              data-id={row.entity}
              title="Select this object"
              onClick={() => projectionStore.getState().select(row.entity)}
              style={{ justifyContent: "space-between", display: "flex", width: "100%" }}
            >
              <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{row.name}</span>
              <span style={{ ...metaText, color: color.accent.base }}>{row.role}</span>
            </Button>
          ))}
        </div>
      )}

      {refusal && (
        <div
          data-testid="role-refusal"
          role="status"
          style={{
            marginTop: space.sm,
            padding: `${space.sm}px ${space.md}px`,
            background: color.danger.bg,
            border: `1px solid ${color.danger.border}`,
            borderRadius: radius.md,
            fontSize: fontSize.meta,
            color: color.danger.text,
          }}
        >
          {refusal}
        </div>
      )}
    </section>
  );
}

export default RolesSection;
