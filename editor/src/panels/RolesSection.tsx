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
import { Callout } from "../theme/fields";
import { Icon } from "../theme/icons";
import { Button } from "../theme/primitives";
import { ChoiceCard, ChoiceGrid, DisclosureSection } from "../theme/workspace";
import { color, fontSize, space } from "../theme/tokens";
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
  const assigned = status.roster.length;

  return (
    // ONE SECTION, NOT AN `<h3>` AND A COLUMN OF DIVS. The three Gameplay sections each drew their own
    // heading, their own accent box, their own warning box and their own refusal box — four private
    // opinions per section, twelve across the panel, all of them shapes the design system already
    // owns. What that cost was legible in a capture: with nothing selected the whole workspace was
    // three headings, three paragraphs each saying "Select an object, then …", and one button.
    //
    // A disclosure carries the state in its summary, so a CLOSED section still reports what it holds —
    // which is what makes closing two of the three the calm default rather than hiding the panel.
    <DisclosureSection
      data-testid="roles-section"
      title="Roles"
      icon={<Icon name="gameplay" size="md" />}
      summary={assigned > 0 ? `${assigned} assigned` : specs.length > 0 ? `${specs.length} to choose from` : undefined}
      storageKey="gameplay.roles"
      tone="card"
      density="compact"
      landmark={false}
    >
      <div style={{ display: "grid", gap: space.sm, minWidth: 0 }}>
      {playing && (
        <Callout
          data-testid="role-score"
          tone="info"
          icon={<Icon name="star" size="sm" />}
          title={String(status.score)}
        >
          {/* "all collected!" is only true if there was ever anything to collect. A scene with no
              collectibles at all was congratulating the player for doing nothing — seen in a live
              capture of a cutscene scene, which has a statue and no coins. */}
          {status.remaining > 0
            ? `${status.remaining} collectible${status.remaining === 1 ? "" : "s"} left`
            : status.roster.some((r) => r.role === "collectible")
              ? "all collected!"
              : "nothing to collect in this scene yet"}
        </Callout>
      )}

      {playing && status.won && (
        <Callout data-testid="role-victory" tone="success" icon={<Icon name="trophy" size="sm" />}>
          You won! Every enemy beaten, every crystal collected · Stop to keep building
        </Callout>
      )}

      {playing && status.health != null && (
        <div
          data-testid="roles-health"
          style={{
            display: "flex",
            gap: space.sm,
            alignItems: "baseline",
            padding: `0 ${space.xs}px`,
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
        <Callout data-testid="roles-blocked" tone="warn" role="status">
          {status.blocked.name || "Something"} didn&apos;t respond — {status.blocked.why}
        </Callout>
      )}

      {playing && status.companions.length > 0 && (
        <div data-testid="companion-status" style={{ display: "grid", gap: 2 }}>
          {status.companions.map((c) => (
            <div key={c.entity} style={metaText}>
              <Icon name="heart" size="sm" /> {c.name} — {c.doing || "waking up"}
            </div>
          ))}
        </div>
      )}

      {/* THE CARDS ARE ALWAYS DRAWN, AND WITH NOTHING SELECTED THEY ARE DISABLED RATHER THAN ABSENT.
          What was here instead was one sentence — "Select an object, then give it a role" — and the
          ten roles the engine offers were invisible until you had already guessed that clicking an
          object would reveal something. A disabled card says the same sentence AND shows the
          vocabulary, which is the difference between a prompt and a menu. */}
      <div style={{ fontSize: fontSize.meta, color: color.text.secondary }} data-testid="roles-hint">
        {selected ? (
          <>
            {summary?.name ?? selected}
            {currentRole && <span style={{ color: color.accent.base }}> · {currentRole}</span>}
          </>
        ) : (
          "Select an object, then give it a role — it becomes part of the game in one click."
        )}
      </div>

      {specs.length > 0 && (
      <ChoiceGrid label="Assign a role">
        {specs.map((spec) => (
          <ChoiceCard
            key={spec.kind}
            data-testid={`role-${spec.kind}`}
            icon={<Icon name={spec.kind} size="md" fallback="shape" />}
            label={spec.label}
            description={spec.blurb}
            selected={currentRole === spec.kind}
            disabled={busy || playing || !selected}
            disabledReason={
              playing
                ? "Stop Play first — roles are authored, not live-edited"
                : !selected
                  ? `Select an object first. ${spec.blurb}`
                  : undefined
            }
            title={`${spec.blurb}. Adds: ${spec.adds} — one Ctrl-Z removes it all`}
            onSelect={() => selected && void run(() => client.roleAssign(selected, spec.kind), `Make it a ${spec.label}`)}
          />
        ))}
      </ChoiceGrid>
      )}

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

      {status.roster.length > 0 && (
        <div data-testid="roles-roster" style={{ display: "grid", gap: 2 }}>
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
        <Callout data-testid="role-refusal" tone="danger" role="status">
          {refusal}
        </Callout>
      )}
      </div>
    </DisclosureSection>
  );
}

export default RolesSection;
