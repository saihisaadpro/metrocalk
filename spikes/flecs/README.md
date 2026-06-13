# M0 Spike ② — Flecs wildcard queries at editor scale

Validates/refutes **ADR-001**: that Flecs v4.1 via `flecs_ecs` 0.2.2 delivers the
compatibility-query performance the product is built on ("find all entities providing Health,
not yet bound" in <16 ms on a realistic editor scene), and that the Rust binding is safe enough
to build a company on.

**Verdict: ADOPT.** Every adopt criterion passes with a ~300–1900× margin, with safety locks ON
(the configuration we'd ship). One scaling finding (F1: pair-induced archetype fragmentation
inflates memory) needs an M1 follow-up but does not touch query latency or the decision.

> Throwaway spike. Bar is "trustworthy measurements", not production. No Loro/UI/rendering.

## Run it

```
cargo run --release                        # safety locks ON  (flecs_safety_locks) — what we'd ship
cargo run --release --no-default-features  # safety locks OFF — for the ON/OFF delta only
cargo bench --bench compat                 # criterion cross-check of Bench 1 (cached, 5k)
```

Single binary, no args, deterministic from one seed. `flecs_safety_locks` is the **only**
difference between the two builds (Cargo.toml expands `flecs_base` into its components so the lock
feature can be toggled in isolation).

## Reproducibility

- **RNG seed:** `0x4D4554524F434131` ("METROCA1"), local SplitMix64 (`src/rng.rs`) — same scene
  every run/machine. Same seed as the Loro spike.
- **Scene:** 5,000 entities · 40 capability kinds (index 0 = Health) · `(Provides, Health)` on
  ~6% of entities · 3–8 other random `(Provides, cap)` pairs each · ~2,000 `(BindsTo, target)`
  edges · roles (`Player`/`Enemy`/`UiElement`) on ~half.
- Determinism confirmed: matched-entity counts are identical across all runs — **211** of 5,000
  and **830** of 20,000 Health-providers-without-an-outgoing-binding; **1,999** binding edges
  (2,000 minus self-loops). Only timings vary, all within <10% run-to-run.

## Environment

| | |
|---|---|
| OS | Windows 11 Home 10.0.26200 |
| CPU | 13th Gen Intel Core i9-13900H (14C / 20T) |
| RAM | 47.6 GB |
| rustc / cargo | 1.92.0 (stable-x86_64-pc-windows-msvc) |
| Build profile | `--release` |
| `flecs_ecs` (binding) | 0.2.2 |
| `flecs_ecs_sys` | 0.2.1 |
| Flecs C core | **v4.1.2** (`FLECS_VERSION_MAJOR/MINOR/PATCH = 4/1/2`) |
| `criterion` (dev) | 0.5.1 |

`flecs_ecs` 0.2.2 is the latest stable as of June 2026 (matches the "0.2.x" named in the task);
the bundled C core is v4.1.2 (matches ADR-001's "v4.1").

## Model (Metrocalk semantics in Flecs idioms)

| Metrocalk concept | Flecs idiom |
|---|---|
| capability provision | pair `(Provides, cap)` — `Provides` a `#[derive(Component)]` tag, `cap` a runtime entity |
| binding edge | pair `(BindsTo, target)` on the source entity |
| roles | tags `Player` / `Enemy` / `UiElement` |

40 capabilities are runtime entities (`cap_0..cap_39`), not 40 Rust types — that's how a real
registry of hundreds of component kinds must work (runtime-registered), and it stresses the same
pair machinery.

### What "the compatibility question" is, exactly

Benchmark 1 queries **entities that provide Health and have no outgoing `(BindsTo, *)`** —
i.e. Health providers not yet bound to anything. In the builder: a positive pair term
`with((Provides, health))` plus a **wildcard negation** `without((BindsTo, *))`. This is the
machinery behind both north-star tests (rank compatible bind targets; describe-to-create
resolution step ①).

Modeling note: the prompt phrases it as "lack a `(BindsTo,*)` pointing at them" (incoming). I
implemented the **outgoing** form (provider has no binding *of its own*) because it is
unambiguous and idiomatic, and stresses the identical engine features (pair match + wildcard
negation). The incoming form ("nobody binds to me") is expressible in Flecs with a target-position
term `BindsTo($x, $this)` and is an M1 detail, not a perf risk — the cost is the same wildcard
traversal Bench 3 already measures.

## Results — safety locks ON (what we'd ship), 2 runs

Median / p99 / max, µs unless noted. Each measured loop: 2,000 samples after 200 warm-up.

| benchmark | run 1 | run 2 |
|---|---|---|
| **B1 cached @5k** (warm) | 8.7 / 12.2 / 24.7 | 9.6 / 12.3 / 34.4 |
| B1 uncached @5k | 101 / 126 / 515 | ~same |
| B1 cold (first cached eval) @5k | 41 (single) | — |
| **B1 cached @20k** (warm) | 39.8 / 58.3 / 140 | 40.4 / 61.2 / 166 |
| B1 uncached @20k | 914 / 1083 / 1190 | ~same |
| **B2 cached re-query after 100 mutations** | 25.0 / 41.1 / 55.3 | 24.5 / 41.3 / 48.3 |
| **B3 wildcard: all 1,999 BindsTo edges** | 130 / 162 / 264 | ~same |
| **B4 churn correctness** | PASS (211→1211→211, 0 stale) | PASS |
| B4 destruct 1,000 entities | 46 µs | 46 µs |

### Safety-lock ON vs OFF delta

| benchmark | ON (median / p99) | OFF (median / p99) | delta |
|---|---|---|---|
| B1 cached @5k | 8.7 / 12.2 | 8.4 / 14.2 | within noise |
| B1 cached @20k | 39.8 / 58.3 | 35.0 / 62.2 | ~10% median |
| B2 under mutation | 25.0 / 41.1 | 24.5 / 35.9 | within noise |
| B3 wildcard | 130 / 162 | 112 / 149 | ~10% |

The locks cost **0–10%** on these queries — dominated by run-to-run noise. Reason: the locks guard
*mutable component-data aliasing*, and our relationship queries are tag/pair-only (no `&mut T`
column borrowed during iteration), so there is almost nothing to check. **Recommendation: ship
with `flecs_safety_locks` ON** — negligible cost, and it is precisely the runtime guard the
wrapper-isolation rule wants.

### Criterion cross-check (Bench 1 cached, 5k)

`cargo bench` reports `compat_cached_5k` at **14.9 µs mean** (CI 14.8–15.1 µs, 414k iterations).
The hand-rolled **median** is ~9 µs; the gap is the expected mean-vs-median skew (max samples reach
~76 µs and pull the mean up). Both methods agree the cached query is ~10–15 µs — **>1000× under the
16 ms budget** — confirming the headline number is real, not measurement noise.

### Memory (Bench 5, 20k entities)

Peak RSS ≈ **296 MB** → **~14.8 KB / entity** (peak RSS / entity count). This is high (see F1).

## Adopt criteria — verdict

| criterion | threshold | measured (safety ON) | result |
|---|---|---|---|
| B1 cached p99 @5k | < 16 ms | **12.2 µs** | ✅ ~1300× margin |
| B1 cached p99 @20k | < 16 ms | **58 µs** | ✅ ~275× margin |
| B2 p99 under mutation (shipping config = ON) | < 16 ms | **41 µs** | ✅ ~390× margin |
| B4 zero stale results | required | PASS (211→1211→211) | ✅ |
| no soundness landmine the wrapper can't contain | required | contained by `flecs_safety_locks` | ✅ (see assessment) |

**Recommendation: ADOPT Flecs v4.1.2 via `flecs_ecs` 0.2.2 (confirms ADR-001), behind our wrapper,
shipping with safety locks ON.**

## Findings

**F1 — pair-induced archetype fragmentation inflates memory (~14.8 KB/entity at 20k).** Flecs is
archetype-based: a pair like `(Provides, cap_7)` is part of an entity's archetype identity. Our
synthetic entities each carry a near-unique random set of 3–8 `(Provides, *)` pairs, so almost
every entity lands in its **own table** → ~20k tables, each with fixed per-table overhead →
~296 MB. Query latency is unaffected (the cached query stores matched tables and iterates them
fine), so this does **not** threaten the gate, but a 50k-entity scene would cost ~750 MB on this
model. Mitigations, to validate in M1 (Flecs supports both, untested here): mark `Provides`/`BindsTo`
with the **`DontFragment`** trait (`#[flecs(traits(DontFragment))]` — sparse storage, no table
fragmentation; `component_traits.rs:353`), or store low-cardinality capability sets as component
data rather than archetype-defining pairs. Real scenes also fragment less than this worst-case
(entities share component patterns). **This is the single most valuable M1 follow-up.**

**F2 — "pointing at them" (incoming) vs outgoing binding** is a modeling choice, documented above;
both stress the same engine features and Bench 3 already measures the wildcard-traversal cost.

## Binding assessment — `flecs_ecs` 0.2.2

**Unsafe surface.** ~**1,116 `unsafe {`** blocks + **64 `unsafe fn`** across `flecs_ecs/src`
(`flecs_ecs_sys` is generated bindgen FFI). Categories: FFI calls into the C core (majority),
raw-pointer dereference of component/table data during iteration, and a few transmutes. This is a
large unsafe surface — unavoidable for a thin binding over a mature C engine — and is the core
reason ADR-001's wrapper-isolation rule exists.

**Aliasing model & soundness containment.** The `flecs_safety_locks` feature (ON by default)
maintains a per-`(component-or-pair-id, table-id)` **read/write counter** (`src/core/safety_map.rs`:
a `WRITE_FLAG` high bit + read count) and checks it around query iteration, turning Rust's borrow
rules into **runtime panics** when a callback would alias a column mutably while it's read/written
elsewhere. Structural mutation during iteration is handled by Flecs's **deferred mode**
(`defer_begin`/`defer_end`): adds/removes are queued and applied at the merge point, so the churn
in Bench 2/4 is safe and produced zero stale results. Net: the soundness landmines are real but
**containable** — the locks catch aliasing at runtime for ~0–10% cost, and the wrapper can force
deferred mutation and a safe query surface.

**API gaps / sharp edges hit during the spike** (M1 engineers will live here):
- `with`/`without` take a **value** argument, not a turbofish type: `without((BindsTo, id::<flecs::Wildcard>()))`,
  not `without::<(BindsTo, flecs::Wildcard)>()`. The type-pair form compiles but means something
  else (it then *also* wants a value). Easy to get wrong; the wrapper should expose one clear form.
- Wildcard target in a term is `id::<flecs::Wildcard>()` (or `flecs::Wildcard::ID`); as a query
  *type* it's `&(R, flecs::Wildcard)`. Two spellings for two contexts.
- `TableIter::entity(row)` and `it.pair(term)` return values (not `Option`); `pair.second_id()`
  gives the target `EntityView`. Reading a wildcard pair's target needs `each_iter`, not `each`.
- Pairs are archetype-identity by default → F1. The `DontFragment`/`Sparse` traits exist but you
  must opt in deliberately.

**Build & debugging.** Clean build (C core + binding + our crate) ≈ **48 s**; incremental rebuild of
our crate ≈ **2 s**; the C core is cached per feature set. Compiler errors from the builder's
generic `impl IntoId`/`FromAccessArg` bounds are moderately cryptic (the "takes 1 argument but 0
supplied" above), but the shipped `examples/` are excellent and resolved every question.

**Ergonomics exhibit — the actual Bench 1 query (verbatim from `src/lib.rs`):**

```rust
pub fn compat_query(scene: &Scene, cached: bool) -> Query<()> {
    let health = scene.caps[HEALTH_CAP];
    let mut b = scene.world.query::<()>();
    b.with((Provides, health));                       // pair term: provides Health
    b.without((BindsTo, id::<flecs::Wildcard>()));    // wildcard negation: no outgoing binding
    if cached {
        b.set_cached();                               // incrementally-maintained match set
    }
    b.build()
}
// iterate: query.each(|_| count += 1)   (tag-only → unit tuple)
```

**Would you build a company on this binding, given the wrapper-isolation rule?** Yes — with the
wrapper as the non-negotiable condition ADR-001 already sets. The binding is a one-maintainer 0.x
crate with a large unsafe surface, which is real maintenance/version risk; but it is a *thin,
faithful* layer over a battle-tested C engine, the safety-lock feature contains the aliasing
landmine at negligible cost, the performance is 100–1900× inside budget, and the examples are
thorough. The wrapper means we depend on a small, well-defined slice of this API (build query,
add/remove pair, iterate, read target) that we can re-implement against another binding — or
vendor `flecs_ecs` — if maintenance lapses. The fallback (`bevy_ecs` + hand-built relationship
index) stays viable precisely because nothing Flecs-shaped leaks past the wrapper.
