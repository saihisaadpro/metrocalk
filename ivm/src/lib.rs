//! **metrocalk-ivm — the Incremental Engine SPIKE (M13.4 / ADR-053).**
//!
//! The deep efficiency bet, **de-risked as a MEASURED go/no-go — not an engine-wide rewrite.** The
//! frontier (DBSP/IVM, VLDB-2023 best paper) turns a relational query into an incremental operator that
//! processes only the **changed rows** (O(Δ)/frame) instead of re-evaluating the world each frame. This
//! crate measures the **crossover** (at what entity-count × churn-rate does incremental beat eager?) on
//! **ONE** expensive query — the signature **capability compat-query** — and the whole deliverable is
//! that number + a clear generalize / don't-generalize verdict.
//!
//! **The query (the M1.5 / ADR-001 intent query):** the relational join
//! `{ (requirer, provider, cap) : requires(r, c) ∧ provides(p, c) ∧ r ≠ p }` — "which providers can bind
//! to which requirers." Eager recomputes it each frame (O(N + Σ_c R_c·P_c)); the maintained view
//! processes only the op-stream deltas (O(Δ · affected)).
//!
//! **The FM-T1.2 keystone (why this is UNIQUELY-ENABLED):** the op-CRDT log and the IVM input stream are
//! **one delta-algebra** — [`Delta`] is the serializable projection of the shipped
//! `Op::{AddPair, RemovePair, DeleteEntity}` capability Provides/Requires pairs (ADR-001), not a second
//! mechanism. Incumbents (eager re-eval over a mutable graph) structurally cannot retrofit this.
//!
//! **Honest scope (load-bearing):**
//! - **The deliverable is a crossover number + a go/no-go, NOT a shipped incremental engine.** The dossier
//!   is emphatic: fine-grained incremental bookkeeping can cost *more* than recompute on hot loops; the win
//!   is on **expensive derived queries at low churn**. We measure it; generalizing is a later, per-query
//!   decision gated on each query's own number.
//! - **Monotonicity decides safety (ties M13.7).** The compat view is **NON-monotone**: `Unprovide` /
//!   `Unrequire` / `Remove` **retract** pairs. Adding is monotone (coordination-free, CALM); retraction
//!   needs the signed-delta / ring-in-lattice handling (FM-T1.2) — which the maintained multiset here
//!   provides. A general non-monotone query needs the M13.7 lattice-boundary coordination point.
//! - **Crate audit = hand-roll (the ARAP/M9.5 precedent).** `dbsp` (Feldera) is real but heavy (~106K
//!   SLoC, wasm32 unconfirmed); `differential-dataflow` is lighter but still a research dep. This crate is
//!   a **zero-dependency, wasm-portable, deterministic** hand-rolled operator behind a project-owned
//!   [`IncrementalView`] trait — own-it-for-determinism/wasm/min-spec. `dbsp`/`differential_dataflow` are
//!   grep-gated OUT (reserved to `/ivm` if ever adopted for a heavier query family).
//! - **Determinism is a feature (keep it).** `BTreeMap`/`BTreeSet` ordering ⇒ the output is canonical and
//!   **bit-identical** to eager (asserted); no RNG in the operator.

use std::collections::{BTreeMap, BTreeSet};

/// An entity id (the loro-key / op-stream entity — a plain integer here, decoupled from the native ECS).
pub type Entity = u64;
/// An interned capability id (the registry interns each capability as an [`Entity`] pair target; here a
/// small integer, the same relational role).
pub type Cap = u32;

/// One element of the compat result: requirer `r` can bind provider `p` on capability `cap`
/// (`requires(r, cap) ∧ provides(p, cap) ∧ r ≠ p`). Field order = the canonical sort key (deterministic).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Match {
    pub requirer: Entity,
    pub provider: Entity,
    pub cap: Cap,
}

/// One op-stream **delta** — the serializable projection of the shipped `Op::{AddPair, RemovePair,
/// DeleteEntity}` capability pairs (ADR-001; `AddPair(e, Provides, cap)` ↔ [`Delta::Provide`]). Feeding a
/// stream of these into an [`IncrementalView`] is the FM-T1.2 op-log-IS-the-IVM-input-stream identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delta {
    /// Entity begins providing a capability (monotone — only adds pairs).
    Provide { entity: Entity, cap: Cap },
    /// Entity stops providing a capability (NON-monotone — retracts pairs).
    Unprovide { entity: Entity, cap: Cap },
    /// Entity begins requiring a capability (monotone).
    Require { entity: Entity, cap: Cap },
    /// Entity stops requiring a capability (NON-monotone — retracts pairs).
    Unrequire { entity: Entity, cap: Cap },
    /// Entity is deleted — retracts all its provides + requires (NON-monotone; the `DeleteEntity` op).
    Remove { entity: Entity },
}

/// The project-owned incremental-view seam (invariant 5). A view consumes op-stream [`Delta`]s and can
/// produce the current compat result; the maintained ([`IncrementalCompat`]) and recompute-each-frame
/// ([`EagerCompat`]) implementations are compared bit-for-bit in the crossover study. No foreign IVM type
/// (`dbsp::` / `differential_dataflow::`) leaks this trait (a hand-rolled operator; grep-gated).
pub trait IncrementalView {
    /// Feed one op-stream delta into the view.
    fn apply(&mut self, delta: Delta);
    /// The current compat result set, **sorted canonically** (the bit-identical equality key).
    fn result(&self) -> Vec<Match>;
    /// The bookkeeping memory the view holds (bytes, approximate) — the IVM overhead the crossover weighs
    /// against the recompute it saves.
    fn memory_bytes(&self) -> usize;
}

// ── the EAGER baseline: recompute the whole query each frame ───────────────────────────────────────

/// The **eager** baseline: it records the raw relation and **recomputes the entire join on every
/// [`result`](IncrementalView::result)** — the incumbent shape (eager re-eval over the mutable graph).
/// `apply` is trivial; the cost is the per-frame recompute.
#[derive(Default, Clone)]
pub struct EagerCompat {
    provides: BTreeMap<Entity, BTreeSet<Cap>>,
    requires: BTreeMap<Entity, BTreeSet<Cap>>,
}

impl EagerCompat {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl IncrementalView for EagerCompat {
    fn apply(&mut self, delta: Delta) {
        match delta {
            Delta::Provide { entity, cap } => {
                self.provides.entry(entity).or_default().insert(cap);
            }
            Delta::Require { entity, cap } => {
                self.requires.entry(entity).or_default().insert(cap);
            }
            Delta::Unprovide { entity, cap } => {
                if let Some(s) = self.provides.get_mut(&entity) {
                    s.remove(&cap);
                }
                if self.provides.get(&entity).is_some_and(BTreeSet::is_empty) {
                    self.provides.remove(&entity);
                }
            }
            Delta::Unrequire { entity, cap } => {
                if let Some(s) = self.requires.get_mut(&entity) {
                    s.remove(&cap);
                }
                if self.requires.get(&entity).is_some_and(BTreeSet::is_empty) {
                    self.requires.remove(&entity);
                }
            }
            Delta::Remove { entity } => {
                self.provides.remove(&entity);
                self.requires.remove(&entity);
            }
        }
    }

    fn result(&self) -> Vec<Match> {
        // Recompute the whole join from scratch (the eager cost): invert provides to cap → providers,
        // then for every (requirer, cap) emit the cross product with providers of that cap.
        let mut providers_by_cap: BTreeMap<Cap, Vec<Entity>> = BTreeMap::new();
        for (&entity, caps) in &self.provides {
            for &cap in caps {
                providers_by_cap.entry(cap).or_default().push(entity);
            }
        }
        let mut out = Vec::new();
        for (&requirer, caps) in &self.requires {
            for &cap in caps {
                if let Some(providers) = providers_by_cap.get(&cap) {
                    for &provider in providers {
                        if provider != requirer {
                            out.push(Match {
                                requirer,
                                provider,
                                cap,
                            });
                        }
                    }
                }
            }
        }
        out.sort_unstable();
        out
    }

    fn memory_bytes(&self) -> usize {
        entity_map_bytes(&self.provides) + entity_map_bytes(&self.requires)
    }
}

// ── the INCREMENTAL maintained view: process only the deltas ────────────────────────────────────────

/// The **incremental** maintained view: it keeps per-cap provider/requirer indexes + the materialized
/// match set, and each [`apply`](IncrementalView::apply) touches **only the pairs the delta changed**
/// (O(Δ · affected)). [`result`](IncrementalView::result) is a cheap read of the maintained set — no
/// recompute. This is the operator the crossover measures against eager.
#[derive(Default, Clone)]
pub struct IncrementalCompat {
    providers: BTreeMap<Cap, BTreeSet<Entity>>,
    requirers: BTreeMap<Cap, BTreeSet<Entity>>,
    entity_provides: BTreeMap<Entity, BTreeSet<Cap>>,
    entity_requires: BTreeMap<Entity, BTreeSet<Cap>>,
    matches: BTreeSet<Match>,
}

impl IncrementalCompat {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of maintained match pairs (a cheap O(1) read — the kind of query the maintained view
    /// answers without materializing, the real IVM win over recompute).
    #[must_use]
    pub fn match_count(&self) -> usize {
        self.matches.len()
    }
}

impl IncrementalView for IncrementalCompat {
    #[allow(clippy::too_many_lines)] // one match over the five delta kinds; splitting it hurts clarity
    fn apply(&mut self, delta: Delta) {
        match delta {
            Delta::Provide { entity, cap } => {
                if self.entity_provides.entry(entity).or_default().insert(cap) {
                    self.providers.entry(cap).or_default().insert(entity);
                    if let Some(reqs) = self.requirers.get(&cap) {
                        for &requirer in reqs {
                            if requirer != entity {
                                self.matches.insert(Match {
                                    requirer,
                                    provider: entity,
                                    cap,
                                });
                            }
                        }
                    }
                }
            }
            Delta::Require { entity, cap } => {
                if self.entity_requires.entry(entity).or_default().insert(cap) {
                    self.requirers.entry(cap).or_default().insert(entity);
                    if let Some(provs) = self.providers.get(&cap) {
                        for &provider in provs {
                            if provider != entity {
                                self.matches.insert(Match {
                                    requirer: entity,
                                    provider,
                                    cap,
                                });
                            }
                        }
                    }
                }
            }
            Delta::Unprovide { entity, cap } => {
                let removed = self
                    .entity_provides
                    .get_mut(&entity)
                    .is_some_and(|caps| caps.remove(&cap));
                if removed {
                    if self
                        .entity_provides
                        .get(&entity)
                        .is_some_and(BTreeSet::is_empty)
                    {
                        self.entity_provides.remove(&entity);
                    }
                    if let Some(ps) = self.providers.get_mut(&cap) {
                        ps.remove(&entity);
                    }
                    if self.providers.get(&cap).is_some_and(BTreeSet::is_empty) {
                        self.providers.remove(&cap);
                    }
                    if let Some(reqs) = self.requirers.get(&cap) {
                        for &requirer in reqs {
                            if requirer != entity {
                                self.matches.remove(&Match {
                                    requirer,
                                    provider: entity,
                                    cap,
                                });
                            }
                        }
                    }
                }
            }
            Delta::Unrequire { entity, cap } => {
                let removed = self
                    .entity_requires
                    .get_mut(&entity)
                    .is_some_and(|caps| caps.remove(&cap));
                if removed {
                    if self
                        .entity_requires
                        .get(&entity)
                        .is_some_and(BTreeSet::is_empty)
                    {
                        self.entity_requires.remove(&entity);
                    }
                    if let Some(rs) = self.requirers.get_mut(&cap) {
                        rs.remove(&entity);
                    }
                    if self.requirers.get(&cap).is_some_and(BTreeSet::is_empty) {
                        self.requirers.remove(&cap);
                    }
                    if let Some(provs) = self.providers.get(&cap) {
                        for &provider in provs {
                            if provider != entity {
                                self.matches.remove(&Match {
                                    requirer: entity,
                                    provider,
                                    cap,
                                });
                            }
                        }
                    }
                }
            }
            Delta::Remove { entity } => {
                let provided: Vec<Cap> = self
                    .entity_provides
                    .get(&entity)
                    .map(|s| s.iter().copied().collect())
                    .unwrap_or_default();
                for cap in provided {
                    self.apply(Delta::Unprovide { entity, cap });
                }
                let required: Vec<Cap> = self
                    .entity_requires
                    .get(&entity)
                    .map(|s| s.iter().copied().collect())
                    .unwrap_or_default();
                for cap in required {
                    self.apply(Delta::Unrequire { entity, cap });
                }
            }
        }
    }

    fn result(&self) -> Vec<Match> {
        // A cheap read of the maintained set (already canonically ordered by BTreeSet) — no recompute.
        self.matches.iter().copied().collect()
    }

    fn memory_bytes(&self) -> usize {
        cap_map_bytes(&self.providers)
            + cap_map_bytes(&self.requirers)
            + entity_map_bytes(&self.entity_provides)
            + entity_map_bytes(&self.entity_requires)
            + self.matches.len() * std::mem::size_of::<Match>()
    }
}

// ── memory accounting (approximate: entry count × element size, ignoring BTree node overhead) ─────────

fn entity_map_bytes(m: &BTreeMap<Entity, BTreeSet<Cap>>) -> usize {
    m.values()
        .map(|s| std::mem::size_of::<Entity>() + s.len() * std::mem::size_of::<Cap>())
        .sum()
}

fn cap_map_bytes(m: &BTreeMap<Cap, BTreeSet<Entity>>) -> usize {
    m.values()
        .map(|s| std::mem::size_of::<Cap>() + s.len() * std::mem::size_of::<Entity>())
        .sum()
}

// ── a deterministic workload generator (shared by tests + the release bench) ─────────────────────────

/// A tiny seeded LCG — a **deterministic** pseudo-random source for reproducible workloads (no
/// wall-clock, no system entropy; the same discipline as the DST seeded RNG). Same seed → same stream.
#[derive(Clone, Debug)]
pub struct Lcg(u64);

impl Lcg {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        // Numerical Recipes LCG constants — pure integer math, ISA-independent.
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    /// A reproducible value in `0..n`.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }
}

/// Build a deterministic initial relation of `entities` entities over `caps` capabilities, each entity
/// providing one cap and requiring one cap. Returns the delta stream that builds it (feed to any
/// [`IncrementalView`]). `caps ≈ entities / density` sets the join density (providers/requirers per cap).
#[must_use]
pub fn build_scene(entities: u64, caps: u64, seed: u64) -> Vec<Delta> {
    let mut rng = Lcg::new(seed);
    let caps = caps.max(1);
    let mut out = Vec::with_capacity(usize::try_from(entities).unwrap_or(0) * 2);
    for e in 0..entities {
        out.push(Delta::Provide {
            entity: e,
            cap: u32::try_from(rng.below(caps)).unwrap_or(0),
        });
        out.push(Delta::Require {
            entity: e,
            cap: u32::try_from(rng.below(caps)).unwrap_or(0),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply_all(view: &mut impl IncrementalView, deltas: &[Delta]) {
        for &d in deltas {
            view.apply(d);
        }
    }

    #[test]
    fn a_hand_computed_join_is_correct() {
        // Ground-truth semantics (not just self-consistency): entities 1,2 provide cap 7; entity 3
        // requires cap 7 → matches {(3,1,7), (3,2,7)}. Entity 1 also requires cap 7 → (1,2,7),(1,1,7 is
        // excluded by r≠p). So expected = {(1,2,7),(3,1,7),(3,2,7)}.
        let deltas = [
            Delta::Provide { entity: 1, cap: 7 },
            Delta::Provide { entity: 2, cap: 7 },
            Delta::Require { entity: 3, cap: 7 },
            Delta::Require { entity: 1, cap: 7 },
        ];
        let mut inc = IncrementalCompat::new();
        apply_all(&mut inc, &deltas);
        let expected = vec![
            Match {
                requirer: 1,
                provider: 2,
                cap: 7,
            },
            Match {
                requirer: 3,
                provider: 1,
                cap: 7,
            },
            Match {
                requirer: 3,
                provider: 2,
                cap: 7,
            },
        ];
        assert_eq!(
            inc.result(),
            expected,
            "the hand-computed compat join is exact (self-pair excluded)"
        );
    }

    #[test]
    fn eager_and_incremental_agree_bit_for_bit_including_retractions_and_delete() {
        // THE CORRECTNESS GATE (bit-identical, test-first) — a sequence that exercises the NON-monotone
        // cases (Unprovide/Unrequire/Remove retract pairs). After EVERY delta the maintained view must
        // equal the full recompute, exactly.
        let seq = [
            Delta::Provide { entity: 1, cap: 5 },
            Delta::Require { entity: 2, cap: 5 },
            Delta::Provide { entity: 3, cap: 5 },
            Delta::Require { entity: 4, cap: 5 },
            Delta::Unprovide { entity: 1, cap: 5 }, // retraction
            Delta::Provide { entity: 2, cap: 9 },
            Delta::Require { entity: 3, cap: 9 },
            Delta::Unrequire { entity: 2, cap: 5 }, // retraction
            Delta::Remove { entity: 3 },            // delete: retracts 3's provides AND requires
        ];
        let (mut eager, mut inc) = (EagerCompat::new(), IncrementalCompat::new());
        for (i, &d) in seq.iter().enumerate() {
            eager.apply(d);
            inc.apply(d);
            assert_eq!(
                inc.result(),
                eager.result(),
                "bit-identical after delta {i} ({d:?}) — incl. the non-monotone retraction/delete"
            );
        }
        assert_eq!(inc.match_count(), inc.result().len());
    }

    #[test]
    fn a_randomized_delta_stream_stays_bit_identical() {
        // Fuzz-style: a long deterministic pseudo-random stream of mixed deltas over a small universe.
        // The maintained view tracks the full recompute EXACTLY throughout (the IVM equality guarantee).
        let mut rng = Lcg::new(0x1234_5678);
        let (mut eager, mut inc) = (EagerCompat::new(), IncrementalCompat::new());
        for step in 0..4000u32 {
            let e = rng.below(12);
            let c = u32::try_from(rng.below(5)).unwrap();
            let d = match rng.below(6) {
                0 => Delta::Provide { entity: e, cap: c },
                1 => Delta::Unprovide { entity: e, cap: c },
                2 => Delta::Require { entity: e, cap: c },
                3 => Delta::Unrequire { entity: e, cap: c },
                _ => Delta::Remove { entity: e },
            };
            eager.apply(d);
            inc.apply(d);
            if step % 97 == 0 {
                assert_eq!(inc.result(), eager.result(), "bit-identical at step {step}");
            }
        }
        assert_eq!(
            inc.result(),
            eager.result(),
            "bit-identical at the end of the stream"
        );
    }

    #[test]
    fn the_compat_query_is_non_monotone_retractions_shrink_the_view() {
        // The monotonicity classification (ties M13.7): adding is MONOTONE (the view only grows); a
        // retraction (Unprovide) makes it SHRINK — so the query is NON-monotone and needs retraction
        // handling (the maintained multiset provides it; a general case needs the M13.7 coordination pt).
        let mut inc = IncrementalCompat::new();
        apply_all(
            &mut inc,
            &[
                Delta::Provide { entity: 1, cap: 3 },
                Delta::Require { entity: 2, cap: 3 },
            ],
        );
        let grown = inc.result().len();
        assert_eq!(grown, 1, "monotone add produced one pair");
        inc.apply(Delta::Unprovide { entity: 1, cap: 3 }); // the non-monotone step
        assert_eq!(
            inc.result().len(),
            0,
            "a retraction SHRINKS the view — the query is non-monotone"
        );
    }

    #[test]
    fn the_operator_is_deterministic() {
        // Same deltas → same result, twice (no RNG / wall-clock in the operator — the determinism the
        // DST/M13.1 discipline requires; the output is the bit-identical equality key).
        let deltas = build_scene(300, 40, 0xABCD);
        let run = || {
            let mut inc = IncrementalCompat::new();
            apply_all(&mut inc, &deltas);
            inc.result()
        };
        assert_eq!(
            run(),
            run(),
            "the maintained view is deterministic across runs"
        );
    }

    #[test]
    fn incremental_bookkeeping_costs_memory_that_eager_does_not() {
        // The HONEST cost the crossover weighs: the maintained view holds indexes + the materialized
        // match set, so it uses MORE memory than the eager baseline (which keeps only the raw relation).
        let deltas = build_scene(500, 64, 0x55);
        let (mut eager, mut inc) = (EagerCompat::new(), IncrementalCompat::new());
        apply_all(&mut eager, &deltas);
        apply_all(&mut inc, &deltas);
        assert_eq!(inc.result(), eager.result(), "same result");
        assert!(
            inc.memory_bytes() > eager.memory_bytes(),
            "the IVM bookkeeping is a real memory overhead ({} vs {} bytes)",
            inc.memory_bytes(),
            eager.memory_bytes()
        );
    }
}
