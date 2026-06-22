// The React `/editor` page-object (M10.1 / ADR-030; extended M10.10) — deliverable 7 of prompt 40:
// re-green the build-acceptance gate against the React UI by **swapping the page-object, not the specs**.
//
// The React parity components keep the vanilla scaffold's STABLE DOM hooks (`#describe`/`#describeBtn`,
// `#reveal .cand`/`.boundrow`, `#status`, `#requirers`, `#topup`, `#rustier`, `#genBtn`,
// `#ctxmenu .ctxitem[data-action]`/`.disabled`, `#tooltip`, the viewport `#viewport`), so the selector
// layer is (near-)identical — this object reuses `scaffold`'s verbs and overrides only where the M10.10
// UX-hardening pass genuinely changed the *interaction shape* (not the ids):
//   • describe→create now CLOSES THE LOOP in the bar: a no-match shows an explicit `#genBtn` panel
//     (the scaffold contract the React port had dropped — now restored), Generate places + SELECTS.
//   • the AI-edit ("rustier") moved off the wallet into the right-pane panel and is now a TWO-STEP
//     deliberate spend: `#rustier` opens a confirm, `#rustierApply` commits — so `clickRustier` clicks
//     through the confirm.
//   • Play is unmistakable ON THE STAGE: a persistent `#playStageBadge` overlays the viewport while
//     playing (in addition to the toolbar `#playIndicator`); feedback also lands as `#toastHost` toasts.
//
// Selection: the acceptance run picks this object when `MTK_UI=react` (else the scaffold). The new
// observables/overrides are exercised live on the local `.exe` run (the caveated convergence gate).

import { $ } from "@wdio/globals";
import { scaffold } from "./scaffold.js";

const visible = async (sel) => {
  const el = await $(sel);
  if (!(await el.isExisting())) return false;
  return (await el.getCSSProperty("display")).value !== "none";
};

export const react = {
  ...scaffold,
  name: "react (/editor)",

  // ── AI-edit: now a deliberate two-step spend (open confirm → apply) — M10.10 / C3 ──────────────────
  async clickRustier() {
    await (await $("#rustier")).click(); // opens the inline confirm (price + before/after)
    const apply = await $("#rustierApply");
    if (await apply.isExisting()) await apply.click(); // commit the metered spend
  },

  // ── on-stage Play treatment (M10.10 / C2) — the badge overlays the viewport, not only the toolbar ──
  playStageBadgeVisible: () => visible("#playStageBadge"),
  // ── feedback-at-the-gesture toasts (M10.10 / C11) ─────────────────────────────────────────────────
  toastHostVisible: () => visible("#toastHost"),
};

/** The active page-object — React when `MTK_UI=react`, else the vanilla scaffold. */
export function reactPage() {
  return react;
}
