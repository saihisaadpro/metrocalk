# ADR-016: Viewport action model + direct-manipulation interaction (right-click context actions · hover details)

**Date:** 2026-06-20 · **Status:** Accepted — M3.3 (`editor-shell/src/actions.rs` + `capscene.rs`;
`src-tauri/src/main.rs` + `render.rs`; `web/index.html`) · **Builds on:** [ADR-011](011-intent-ranking.md)
(the reveal/rank/explain engine + the explain-every-"no" pattern), [ADR-013](013-live-persistence-replay-log.md)
(the replay-log the new actions ride), M1.6 (entity-resurrection undo). · **Reuses:** north-star test #1's
≤2-interaction / every-"no"-explained bar.

## Context

M3.1 made binding discoverable through the side panel; M3.2 made it describable. M3.3 makes the
interaction **direct**: right-click an entity in the viewport → a context menu of exactly the actions
valid for it (Bind… / Remove / Duplicate / Focus / Inspect), every unavailable one explained; hover an
entity → a details tooltip so you can read the scene without selecting. This is the "context reveal" the
M3 roadmap always named, and the interaction the dogfood verdict will judge. The live shell runs the
minimal `web/index.html` scaffold (not the real React `/editor`), so the work is split by durability:
the reusable substance in `/core`-adjacent crates (the bridge), a thin probe in the scaffold.

## Decision

**1. A registry-driven action model — UI-agnostic data, not DOM.** `actions::actions_for(entity)` returns
each action + whether it's available + a **specific reason** when not (the M3.1 `why_not` discipline).
`Bind…` is the only conditional action — available iff the entity is a requirer with an unbound required
capability (reusing the reveal's `required_caps`), else greyed "requires no capabilities" / "already
bound"; Remove/Duplicate/Focus/Inspect are always available for a live entity. Deterministic, offline,
O(1)/action, no side effects. It lives in the **bridge** so it survives the eventual React port —
the scaffold and the future `/editor` both call the same query.

**2. Every mutating action is one undoable pipeline transaction (invariant 3).**
- **Remove** deletes the entity *and* cleans every binding it's in — a removed provider's edge is freed
  (the dependent re-opens), and removing a requirer clears the provider's consumed-marker `(BindsTo,…)`
  pair so the provider re-enters the candidate set. Undo restores the entity (M1.6 resurrection) + edges
  + pairs **atomically**.
- **Duplicate** clones components + provides/requires caps under a **fresh deterministic id** at a fixed
  offset; `BindsTo`/edges are **not** cloned (the clone is independently bindable).
- Both persist as replay-log records (`Remove`/`Duplicate`) and survive reload (ADR-013). Bind… reuses
  the M3.1 `bind`; **Focus** is a viewport-camera op (a settable look-at `cam_target`) with no mutation.

**3. Right-click vs orbit — a movement-threshold disambiguation (invariant 4 preserved).** A right-drag
still orbits natively (`drag_start`/`drag_end`, zero-per-frame-IPC, unchanged). A right-press whose
release moved **< 6 px** is a *click* → it opens the menu and `viewport_peek`s the entity under the
cursor (DPI-safe normalized cursor). The browser context menu is suppressed.

**4. Hover details — debounced, pick-on-change, no per-frame IPC (invariant 4).** Hover uses a new
**non-mutating** `viewport_peek` (identifies the entity without changing selection or bumping the
revision). The JS debounces mouse-move (~130 ms settle); `entity_details` is fetched **only when the
hovered entity changes** — so the boundary is crossed on change, never per frame. The render hot path
stays 0 IPC/frame (orbit unchanged).

## Consequences

- **A second, more direct binding route** (right-click → Bind…) without violating test #1's
  ≤2-interaction / every-"no"-explained bar.
- **Consistent + persistent:** Remove re-projects with a **targeted** delta (`Remove(id)` + `RemoveEdge`
  per freed binding — not a full reload, inv. 2); Duplicate echoes the clone; both survive close→reopen.
- **Measured (release, i9-13900H):** `actions_for` on the 5k scene **p99 ~2.7–3.7 µs** (≈4000× under the
  16 ms budget). Hover crosses JS only on entity change (debounced); orbit stays 0 IPC/frame.
- **wasm parity:** the action model is pure metadata (wasm-portable by design, like the reveal); the
  commands ride the existing pipeline; the menu/tooltip are DOM. No new wasm-incompatible code — the
  `wasm32` tripwire is unaffected.
- **The scaffold is a probe, not the product:** the production menu/tooltip is the React `/editor`
  follow-up; the durable action model + transactional commands move over unchanged.

## Revisit when

The React `/editor` is wired into the shell (the menu/tooltip get a real component over the same
`actions_for` + commands), or a richer action set (group/align/parent) is added (extend `actions_for`),
or hover wants richer data than the bridge projection carries.
