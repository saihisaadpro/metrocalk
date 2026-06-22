// The React `/editor` page-object (M10.1 / ADR-030) — deliverable 7 of prompt 40: re-green the
// build-acceptance gate against the React UI by **swapping the page-object, not the specs**.
//
// The React parity components were deliberately authored keeping the vanilla scaffold's STABLE DOM hooks
// (`#describe`/`#describeBtn`, `#reveal .cand`/`.boundrow`, `#status`, `#requirers`, `#topup`, `#rustier`,
// `#ctxmenu .ctxitem[data-action]`/`.disabled`, `#tooltip`, the viewport `#viewport`), so the selector
// layer is (near-)identical — this page-object reuses `scaffold`'s verbs and overrides only where the
// React DOM genuinely differs. Any deltas the FIRST LOCAL RUN against the packaged React `.exe` surfaces
// are pinned here (the gate's adversarial guard: derive + reconcile against the live build, not assume).
//
// Selection: the acceptance run picks this object when `MTK_UI=react` (else the scaffold). Until the local
// React run validates every selector end-to-end, treat the unverified verbs as provisional — the specs +
// acceptance dimensions are unchanged either way (the whole point of the swappable layer).

import { scaffold } from "./scaffold.js";

export const react = {
  ...scaffold,
  name: "react (/editor)",
  // Overrides go here once the local React run identifies any DOM difference, e.g. a relocated control or
  // a React-only wrapper. None known yet — the components mirror the scaffold ids by construction.
};

/** The active page-object for the run — React when `MTK_UI=react`, else the vanilla scaffold. The specs
 *  import `page()` from `scaffold.js`; this is the explicit React entry the acceptance config can select. */
export function reactPage() {
  return react;
}
