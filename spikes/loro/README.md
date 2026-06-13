# M0 Spike ① — Loro as the Metrocalk document layer

Validates/refutes **ADR-002**: that Loro 1.x can replace the planned custom WAL/ARIES undo
system as the document layer (undo/redo, persistence, history) for an ECS-backed scene document.

**Verdict: ADOPT.** All four adopt criteria pass on the realistic 5k-entity scene. Three
implementation-shaping findings (F1–F3) must be carried into M1; none refute ADR-002.

> Throwaway spike. Code-quality bar is "trustworthy measurements", not production. The ECS is not
> integrated here — the scene is modeled directly in Loro to isolate the document-layer questions.

## Run it

```
cargo run --release
```

One binary runs the whole suite serially and prints a Markdown report to stdout (the committed
`results-run2.md` is the verification run; `results-run1.md` is the first run). Single command,
no arguments, fully deterministic from one seed.

## Reproducibility

- **RNG seed:** `0x4D4554524F434131` (ASCII "METROCA1"), fed to a local SplitMix64 (`src/rng.rs`)
  so the synthetic scene is byte-identical across machines/OS/crate versions.
- **Scene:** 5,000 entities, 3–8 components each (Transform always present), ~2,000 binding edges.
- Determinism confirmed: across both runs, every structural number is identical — doc op counts
  (157826 / 186528 / 422883), export sizes (4.51 MB / 9.26 MB / 218.2 KB), alive entities/edges
  (3792 / 978), and the post-merge violation catalog (58 = 17+23+1+17). Only wall-clock timings
  differ run-to-run.

## Environment

| | |
|---|---|
| OS | Windows 11 Home 10.0.26200 |
| CPU | 13th Gen Intel Core i9-13900H (14C / 20T) |
| RAM | 47.6 GB |
| GPU | Intel Iris Xe (irrelevant — no rendering in this spike) |
| rustc / cargo | 1.92.0 (stable-x86_64-pc-windows-msvc) |
| Build profile | `--release` |
| `loro` | 1.13.1 (loro-internal 1.13.1, loro-common 1.13.1, loro-rle 1.6.0, serde_columnar 0.3.14) |
| `windows-sys` | 0.61.2 (peak-RSS query only) |

Latest stable `loro` at time of spike is 1.13.1 — matches the "1.13+" named in the task.

## Methodology

- Median + p99 reported; means are not. Warm-up runs precede every measured loop.
- Measurement runs are serial on an otherwise-idle machine; no parallel builds/benches.
- Per-op mutation latency excludes shadow-model bookkeeping (preparation/validation happen
  outside the timed `execute` + `commit`).
- Undo is measured two ways (this distinction is the headline of Bench 2 — see F2):
  - **single undo of latest**: time the undo, then redo (untimed) to return to the tip, so every
    measured undo reverts the most-recent op — the interactive Ctrl-Z cost.
  - **consecutive undo**: 100 undos in a row with no redo, showing how cost grows as the checkout
    target moves away from the tip.

## Document model

| Concept | Loro representation | Why |
|---|---|---|
| Entity hierarchy | `MovableTree` "hierarchy"; node meta holds the stable `eid` string | the concurrent-reparent CRDT is the thing we're buying Loro for |
| Component data | `components` map: `eid → map(componentName → map(field → value))` | nested maps keyed by stable eid; asset-ref fields are plain strings |
| Binding edges | `bindings` map: `"from\|kind\|to" → map{from,to,kind}` | see list-vs-map note below |

### Binding edges: map keyed by canonical string, not a list

I represent each edge as a **map entry keyed by the canonical triple `from|kind|to`**, not as
elements of a `LoroList`/`LoroMovableList`. Rationale: edges are an unordered *set* with natural
identity (the triple), and the two operations we care about are "does edge X exist?" and "remove
edge X" — both O(1) by key on a map, and idempotent. A list would force linear scans for
existence/removal, has no dedup, and (worse for a CRDT) concurrent inserts of the *same logical
edge* on two peers produce two independent list elements that both survive a merge — a guaranteed
duplicate. A map keyed by the canonical triple makes concurrent "add the same edge" converge to a
single key by construction (the value still conflicts, but the *edge's existence* does not
duplicate). Edge ordering is never semantically meaningful in Metrocalk, so the list's one
advantage (order) is irrelevant.

### Containers are *regular*, not `ensure_mergeable_*` (see F1)

Component/binding sub-containers are created with plain `insert_container` via a get-or-create
helper (`child_map`), **not** Loro's `ensure_mergeable_map`. The mergeable helper is the
"obvious" choice for deterministic concurrent creation, but it does not survive undo/redo (F1).
Using regular containers means concurrent first-writes to the same key fork into two containers;
reconciling that is exactly the job of the merge-validation layer ADR-002 already mandates.

## Results (verification run — `results-run2.md`)

### Bench 1 — 10,000 sequential mutations (70% prop-set / 10% reparent / 5% create / 5% delete / 5% bind-add / 5% bind-remove)

| metric | run 1 | run 2 |
|---|---|---|
| total wall time | 0.563 s | 0.646 s |
| throughput | 17,756 / s | 15,484 / s |
| per-op median | 12.7 µs | 14.2 µs |
| per-op p99 | 192 µs | 230 µs |
| peak RSS (gen + 10.5k mut) | 56.2 MB | 56.1 MB |
| doc ops after run | 186,528 | 186,528 |

Scene generation: ~0.7 s, 157,826 ops, ~52 MB RSS. Sustained throughput over the +90k extension
ranged 18.9k–27.4k mut/s (coarse single-timer figure, load-sensitive — not a gate metric).

### Bench 2 — undo/redo latency

| op | depth 1k median | depth 1k p99 | depth 10k median | depth 10k p99 |
|---|---|---|---|---|
| **single undo of latest** | **69 µs** | **250 µs** | **69 µs** | **131 µs** |
| single redo of latest | 69 µs | 122 µs | 69 µs | 243 µs |
| consecutive undo (1→100 back) | 53 ms | 181 ms | 61 ms | 240 ms |
| 50-op group undo of latest | 52 ms | 231 ms | 61 ms | 143 ms |
| 50-op group redo of latest | 51 ms | 124 ms | 59 ms | 209 ms |

The interactive single-op undo is ~70 µs / sub-millisecond p99 at both depths. Undoing a 50-op
transaction group, or undoing many ops in a row, costs tens of ms because undo computes its
inverse via full-document `checkout`s (see F2).

### Bench 3 — export sizes

| export | at 10k history ops | at 100k history ops |
|---|---|---|
| full snapshot | 4.51 MB | 9.26 MB |
| shallow snapshot (at latest) | 2.01 MB | **218 KB** |
| oplog (all updates) | 2.29 MB | 5.38 MB |

Full snapshot of the 5k-entity scene is 4.51 MB ≪ 20 MB. Shallow snapshot collapses retained
history aggressively (218 KB at 100k ops) — the history-vs-size lever ADR-002 anticipated works.

### Bench 4 — time-travel checkout, 5,000 ops into the past

Checkout-back 94–215 ms, checkout-to-latest 100–321 ms over 5 iterations. Acceptable for a rare
operation; not interactive-grade (consistent with F2 — checkout rebuilds state).

### Bench 5 — merge stress (fork, 500 divergent ops/side + 8 scripted conflicts, bidirectional merge)

- **Converged: true** — after exchanging updates both directions, A and B have byte-identical
  canonical deep-values.
- Merge cost: ~20.7 KB update payload, ~290–400 ms export+import per direction.
- Local-only undo holds after merge: A's first post-merge undo reverted only A's own marker op
  (px 999 → prior) and left B-authored state untouched; unwinding all 465 of A's local steps never
  touched B's `hp=222`. (Each step is a full-checkout undo, so the full unwind took ~52 s — F2.)

#### Scripted conflict outcomes

| # | conflict | outcome |
|---|---|---|
| S1 | A moves X under Y; B moves Y under X | converges to a valid tree (no cycle); one move wins per the MovableTree CRDT |
| S2 | A deletes E; B edits E's component | node stays deleted; component **key not resurrected** (A's map-key delete wins LWW; B's container edits orphaned) |
| S3 | A deletes parent; B creates child under it | child ends up **deleted** (inherits trashed ancestor) |
| S4 | A deletes E; B adds binding referencing E | **dangling edge present** |
| S5 | both set same field different values | **value-level LWW** — one whole value wins (hp=222) |
| S6 | both set same asset-ref path | **value-level LWW** — one path wins, string intact (no corruption) |
| S7 | both add the same canonical edge key | **single map key** (no duplicate edge); losing container orphaned |
| S8 | both reparent same node to different parents | converges to one parent |

## Merge failure-mode catalog (feeds the M1 merge-validation layer)

CRDTs guarantee convergence, not ECS-semantic validity. Every invalid state below is **mechanically
detectable from the document alone** (no shadow model) and **mechanically repairable**. The
validator in `src/validate.rs` already detects all of them; this is the spec for what M1's
merge-validation pass must check after every merge. Counts are from the seeded stress merge (stable
across both runs).

| invalid state | how it arises | detector | count | repair |
|---|---|---|---|---|
| **dangling edge endpoint** | bind added to/at an entity the other side deleted (S4) | edge endpoint eid not in alive-eid set | 17 | delete the edge |
| **orphan component record** | entity deleted on one side, its component map edited/kept on the other (S2) | `components` key with no matching alive tree node | 17 | delete the record |
| **entity missing component record** | node survives but its component map was deleted/lost | alive eid with no `components` key | 1 | recreate empty record or delete node |
| **duplicate eid** | both peers minted the *same* eid string for *different* new nodes (F3) | >1 alive node carries the same meta `eid` | 23 | re-key by peer (see F3) |
| **tree cycle** | concurrent reparents (S1/S8) — *not observed*; MovableTree prevents it | parent-chain walk with step cap | 0 | break lowest-priority edge |
| **alive node under deleted ancestor** | create-under-deleted (S3) leaves child trashed — *handled by Loro as deletion* | parent chain ends in `Deleted`/`Unexist` | 0 | resurrect or cascade-delete |
| **corrupt asset ref** | asset-path string mangled by merge — *not observed* | path fails `assets/…​.{ext}` shape | 0 | flag for user/regeneration |
| **malformed edge / key-value mismatch** | edge map missing fields or key≠value triple — *not observed* | structural check | 0 | rebuild key from value or drop |

The "0 observed" rows matter as much as the non-zero ones: **MovableTree produced no cycles** under
adversarial conflicting reparents, and **asset-reference strings survived every merge intact** —
two risks ADR-002 explicitly flagged, now retired.

## Findings that shape M1 (none refute ADR-002)

**F1 — `ensure_mergeable_*` does not survive undo/redo.** A container created via
`ensure_mergeable_map` is, after one undo + redo cycle, recreated as a *regular* container; the
deterministic mergeable marker is lost, and a later `ensure_mergeable_map` on that key returns
`ArgErr("…the key already holds a non-mergeable value")`. Confirmed by source
(`loro-internal/src/handler.rs::ensure_mergeable_container` guards against a non-marker occupant)
and by a reproducing run. **Decision:** the document layer uses regular containers + the
merge-validation layer, not the mergeable helper. Worth an upstream issue.

**F2 — undo computes its inverse via full-document `checkout`s, so undo cost tracks checkout
*distance from the tip*, not change size.** `LoroDoc::undo_internal` checks out `from`→`to` and back
(`loro-internal/src/loro.rs`). There are **two independent cost axes**, both visible in Bench 2 —
do not conflate them:

- *Group size* — undoing a single 50-op transaction group is tens of ms (52–61 ms median) because
  one undo spans a 50-op checkout. **Mitigant:** keep transaction groups small (one user action =
  one small commit) so a single undo stays one short step from the tip.
- *Walk-back depth* — the headline ~70 µs is undo-of-the-latest-op *while sitting at the tip* (the
  benchmark redoes back to the tip after each measured undo, so every sample reverts one step). Real
  multi-level Ctrl-Z never sits at the tip: it walks backward, each step further away, so
  *consecutive* undo reaches **~53 ms median / 240 ms p99 by ~50 steps in a row** (the
  `consecutive undo (1→100 back)` row, and the ~52 s full unwind of 465 steps in Bench 5). Small
  transaction groups do **not** help this axis — N small commits still cost N increasingly-distant
  checkouts.

The walk-back figure (~53 ms) is the *same order of magnitude* as the 16 ms interactive frame
budget, so undo is **not** "solved at 70 µs" — that number is the best case only. **Decision for
M1:** interactive multi-level undo needs an engine-side fast path — an ECS-authoritative in-memory
inverse-op stack for recent undo (consistent with invariant 1: ECS authoritative, Loro is its
durable mirror) — with Loro `checkout`/`UndoManager` reserved for deep history and time-travel. Do
not ship an interactive undo that calls Loro `checkout` once per Ctrl-Z.

**F3 — concurrent entity creation collides on eids.** Two forked peers minted the *same* `eid`
strings (`e005000…`) for *different* tree nodes, producing 23 duplicate-eid violations after merge.
Loro's own `TreeID` is `(peer, counter)` and never collides; our application-level `eid` must do
the same. **Decision for M1:** entity IDs must embed the peer/replica id (or derive from `TreeID`),
not a peer-local monotonic counter.

**F4 — peak RSS** over the whole process (which holds a 100k-history doc plus two 5k-entity merge
forks simultaneously) was ~221 MB; the isolated 5k-scene + 10.5k-mutation working set was ~56 MB.

## Adopt criteria — verdict

| criterion | threshold | measured | result |
|---|---|---|---|
| single-op undo p99 @ 10k history — **latest op, at tip** | < 5 ms | 0.13 ms | ✅ PASS\* |
| 10k-mutation run | < 10 s | 0.56–0.65 s | ✅ PASS |
| full snapshot of 5k-entity scene | < 20 MB | 4.51 MB | ✅ PASS |
| post-merge invalid states detectable & repairable | all | all 8 classes; 0 undetectable | ✅ PASS |

\* The undo criterion measures undo-of-the-latest-op while sitting at the tip — the right metric for
"is the CRDT undo path viable" (it is, decisively). It is **not** the cost of interactive multi-level
Ctrl-Z, which walks back from the tip at ~53 ms median / 240 ms p99 (see **F2**). ADOPT stands; the
walk-back cost is an M1 *implementation* directive (engine-side undo fast path), not a Loro defect.

**Recommendation: ADOPT Loro 1.13.1 as the document layer (confirms ADR-002).** Carry F1–F3 into
M1: regular containers + a merge-validation layer (already planned); peer-namespaced entity IDs; and
for undo, small transaction groups **plus** an engine-side in-memory inverse-op stack for interactive
multi-level undo (F2) — Loro `checkout` is for deep history / time-travel, not per-Ctrl-Z.
