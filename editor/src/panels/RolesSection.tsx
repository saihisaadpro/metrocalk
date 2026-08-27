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
import { Icon } from "../theme/icons";
import { Button } from "../theme/primitives";
import { color, fontSize, radius, space } from "../theme/tokens";
import { DisclosureSection, useSubjectDisclosure } from "../theme/workspace";
import type { RoleReply, RoleSpec, RoleStatusInfo } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

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

  // The group's CONDITION, on the group's own header line — the shared status slot every other
  // workspace section already uses ("Ready", "Selection required", "Advanced"). This is what the three
  // stacked sentences became: a reader learns the same thing from the strip without opening anything.
  const subject = summary?.name ?? selected;
  const condition = playing
    ? `score ${status.score}`
    : selected
      ? currentRole
        ? `${subject} · ${currentRole}`
        : String(subject)
      : status.roster.length > 0
        ? `${status.roster.length} assigned`
        : "needs a selection";

  const disclosure = useSubjectDisclosure(selected);

  return (
    <DisclosureSection
      data-testid="roles-section"
      title="Roles"
      summary={condition}
      density="compact"
      landmark={false}
      // Forced open during Play, because the live read-outs below (score, health, blocked, companions)
      // are the loop this whole feature exists to close, and a folded section is a scoreboard nobody
      // can see. The user's own fold is still remembered for when Play stops.
      open={playing || disclosure.open}
      onOpenChange={disclosure.onOpenChange}
    >
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
            <Icon name="star" size="sm" /> {status.score}
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
          <Icon name="trophy" size="md" /> You won! Every enemy beaten, every crystal collected · Stop to keep building
        </div>
      )}

      {playing && status.health != null && (
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
          {/* A row of hearts, drawn rather than typed — the same reason the rest of the editor stopped
              using characters: U+2665 BLACK HEART SUIT escapes to Segoe UI Emoji on Windows and paints a red
              pictograph into a monochrome read-out, and `String.repeat` on a glyph cannot be styled
              per-heart anyway. Spent hearts are the same mark at low opacity, so the bar reads as
              "three of five" and not as two unrelated symbols. */}
          <span style={{ display: "inline-flex", gap: 2, alignItems: "center" }} aria-hidden>
            {Array.from({ length: Math.max(0, status.health!.maxHp) }, (_, i) => (
              <Icon key={i} name="heart" size="sm" style={{ opacity: i < status.health!.hp ? 1 : 0.25 }} />
            ))}
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
          <Icon name="blocked" size="sm" /> {status.blocked.name || "Something"} didn&apos;t respond — {status.blocked.why}
        </div>
      )}

      {playing && status.companions.length > 0 && (
        <div data-testid="companion-status" style={{ display: "grid", gap: 2, marginBottom: space.sm }}>
          {status.companions.map((c) => (
            <div key={c.entity} style={metaText}>
              <Icon name="heart" size="sm" /> {c.name} — {c.doing || "waking up"}
            </div>
          ))}
        </div>
      )}

      {/* The subject is named in the section's own HEADER, not again here: three groups each
          re-printing "Crystal" under its own title is what a 340px column looked like before. */}
      <div style={{ display: "grid", gap: space.xs }}>
        {/* THE CARDS ARE ALWAYS HERE — disabled, with the reason, when there is nothing to apply them
            to. `<ux_quality>` 1 and 6: the control that starts the action owns it, and a disabled
            control explains itself in plain words. The version this replaces removed the four cards
            entirely and left a sentence telling the reader to go and select something, so the one
            question this group exists to answer — *what can a role even be?* — could not be asked
            without first guessing that the answer was here. */}
        <div role="group" aria-label="Assign a role" style={{ display: "grid", gridTemplateColumns: "repeat(2, 1fr)", gap: space.xs }}>
          {specs.map((spec) => (
            <Button
              key={spec.kind}
              data-testid={`role-${spec.kind}`}
              variant={currentRole === spec.kind ? "toggle" : "secondary"}
              active={currentRole === spec.kind}
              compact
              disabled={busy || playing || !selected}
              title={
                playing
                  ? "Stop Play first — roles are authored, not live-edited"
                  : !selected
                    ? `Select an object first, then ${spec.label} applies to it. ${spec.blurb}`
                    : `${spec.blurb}. Adds: ${spec.adds} — one Ctrl-Z removes it all`
              }
              onClick={() => selected && void run(() => client.roleAssign(selected, spec.kind), `Make it a ${spec.label}`)}
            >
              <Icon name={spec.kind} size="md" fallback="shape" /> {spec.label}
            </Button>
          ))}
        </div>
        {selected && currentRole && (
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
        {!selected && (
          <div data-testid="roles-hint" style={metaText}>
            Select an object, then give it a role — it becomes part of the game in one click.
          </div>
        )}
      </div>

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
    </DisclosureSection>
  );
}

export default RolesSection;
