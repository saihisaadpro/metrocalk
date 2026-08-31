//! **The Inspector when there is nothing to inspect (ADR-170).**
//!
//! The Inspector is the tallest surface in the editor — a 300px track running the full height of the
//! window, roughly a quarter of a 1440px desktop — and until this file existed the state it is in most
//! often said one grey sentence, `Select an entity to inspect.`, pinned to its top-left corner with a
//! thousand pixels of nothing under it. The panel next door (`Relations`) answered the *same* absence
//! with a composed `EmptyPanelState`: two tabs of one dock, disagreeing about what an empty panel looks
//! like, which is the exact class of drift the UI constitution exists to end.
//!
//! **What this deliberately does NOT do is fill the space.** The constitution's own opening complaint is
//! that the editor had become *overwhelming* — so answering dead pixels with a second copy of the
//! Hierarchy would be moving in the direction the brief was written against. The column stays quiet. It
//! gains a centred, properly-composed statement of what will appear here and how to get there, and ONE
//! list, which is usually absent:
//!
//! **`Requirers` — the objects waiting for a binding.** It is the only thing in this editor that is both
//! genuinely useful with no selection and not a duplicate of something already on screen: a rare,
//! filtered set (a needle in a 5k-entity scene) that today is reachable only through an icon-only
//! popover in the *other* dock's header. It answers "what is waiting for me?", a question with no
//! selection in it, and clicking a row makes the selection this panel is missing. When nothing is
//! waiting it renders nothing at all, so the calm state stays calm.

import { Requirers, useRequirers } from "../panels/Requirers";
import { Icon } from "../theme/icons";
import { EmptyPanelState } from "../theme/workspace";
import { color, space } from "../theme/tokens";

export function InspectorEmpty() {
  const waiting = useRequirers();
  return (
    <div
      id="inspector"
      data-testid="inspectorNoSelection"
      style={{
        // `.mtk-dock-panel.mtk-scroll > *` sets `flex: none` on every direct child — a deliberate rule
        // (in a scroller nothing shrinks), and it is also what would leave this stack at the top of a
        // 700px column. `1 0 auto` takes the growth back WITHOUT taking the shrink: the composition
        // centres itself in whatever height it is given and never gets squashed below its content.
        flex: "1 0 auto",
        display: "flex",
        flexDirection: "column",
        justifyContent: "center",
        gap: space.sm,
        paddingBottom: space.xl,
      }}
    >
      <EmptyPanelState
        data-testid="inspectorEmpty"
        icon={<Icon name="properties" size="xl" />}
        title="Select an object to edit its properties"
        description="Its components, behaviour and material appear here. Click anything in the viewport, or a row in the Scene list."
        // The panel is centred as ONE composition. Left at its stylesheet `flex: 1 1 auto` it would grow
        // to eat the column and push the list below it onto the bottom edge, which reads as two unrelated
        // things that happen to share a track rather than as one calm answer to one absence.
        style={waiting.length > 0 ? { flex: "0 0 auto", minHeight: 0, paddingBottom: space.lg } : { flex: "0 0 auto" }}
      />
      {waiting.length > 0 && (
        <div style={{ borderTop: `1px solid ${color.border.subtle}` }}>
          <Requirers hideWhenEmpty />
        </div>
      )}
    </div>
  );
}
