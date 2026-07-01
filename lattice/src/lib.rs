//! **metrocalk-lattice — the Lattice Core (M13.7 / ADR-056): CALM + the typed monotone/non-monotone boundary.**
//!
//! The foundational bet: Metrocalk's op-stream is **one mathematical object** — a **join-semilattice** of
//! immutable facts, read by interpreters — from which convergence · determinism · "explain every no" ·
//! safe-AI become theorems. The keystone is **CALM (monotone ⇔ coordination-free, *iff*)** and its single
//! load-bearing decision: **make the monotone/non-monotone boundary EXPLICIT and TYPED.**
//!
//! **The win is the TYPED BOUNDARY, NOT forcing monotonicity.** Games are non-monotone-heavy (delete,
//! `position = x`, "lowest health", rollback) — a "pure-monotone everything-is-a-lattice cathedral" is the
//! named over-reach (FF-T1 §4, VANITY — cut it). The value is *classifying* every op as [`Monotone`]
//! (coordination-free, deterministic, incremental-for-free) or [`NonMonotone`] (retraction → a logged
//! coordination point), so the engine always knows which regime an op is in.
//!
//! **SERVES-THE-AXIS is the gate.** This earns its place only if the typed boundary compounds into
//! simpler/more-correct/faster:
//! - **Monotone ops are coordination-free + incremental-for-free** ([`GSet`]/[`MaxReg`] merge in any order
//!   → same result; the CALM proof) → tells **M13.4** which queries are incrementalizable **for free** (no
//!   retraction path) and tells **M13.1** which ops are **deterministic by construction** (order-independent).
//! - **Non-monotone ops are correctly FLAGGED** and handled via the **ring-in-lattice** ([`OrSet`], FM-T1.2)
//!   — the **same add/remove delta-algebra M13.4's IVM uses** (one algebra for CRDT + IVM, not two). A
//!   non-monotone revoke is order-dependent *without* coordination (flagged) but **converges** through the
//!   OR-Set coordination point.
//!
//! **Crate audit (Hydro/DFIR/`lattices` drift):** the dossier's Hydroflow is renamed to Hydro/DFIR (`dfir_rs`);
//! the lattice types live in the `lattices` crate — pre-1.0 Berkeley-research-grade. We need only the lattice
//! *types* + the monotonicity *classification*, not the Hydro runtime → **hand-roll** (the ARAP/M9.5 rule):
//! zero-dep, wasm-clean, deterministic. `lattices::`/`dfir_rs::`/`hydroflow::` grep-gated OUT.
//!
//! **No per-frame tax:** the classification is a `const` type property, not a runtime cost.

use std::collections::{BTreeMap, BTreeSet};

// ── the one mathematical object: a join-semilattice ──────────────────────────────────────────────────

/// A **join-semilattice**: a set with a least element ([`bottom`](Lattice::bottom)) and a binary
/// [`join`](Lattice::join) (least upper bound) that is **associative, commutative, and idempotent** — the
/// three laws that make merge **order-independent** (⇒ convergent + deterministic, the CRDT/CALM property).
/// The whole op-stream is an instance of this object under four names (join-semilattice / free-structure /
/// immutable-facts-with-time / monotone log).
pub trait Lattice: Clone + PartialEq + Sized {
    /// The least element — the identity for [`join`](Lattice::join).
    fn bottom() -> Self;
    /// The least upper bound of `self` and `other`. Must be associative, commutative, idempotent.
    #[must_use]
    fn join(&self, other: &Self) -> Self;
}

/// Fold a slice of lattice values by [`join`](Lattice::join) — the merge of many replicas. Because join is
/// A/C/I, the result is **independent of the fold order** (the property the tests exercise across
/// permutations).
#[must_use]
pub fn join_all<L: Lattice>(values: &[L]) -> L {
    values.iter().fold(L::bottom(), |acc, v| acc.join(v))
}

// ── monotone lattices (coordination-free, incremental-for-free) ──────────────────────────────────────

/// A **grow-only set** (G-Set) — the canonical monotone lattice: `join` = union. Every op ([`insert`]) only
/// **adds**, so state only climbs the order → coordination-free + deterministic (CALM). The `caps` /
/// granted-capabilities subsystem in its add-only regime.
///
/// [`insert`]: GSet::insert
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct GSet<T: Ord + Clone> {
    elems: BTreeSet<T>,
}

impl<T: Ord + Clone> GSet<T> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            elems: BTreeSet::new(),
        }
    }
    /// The one op — **monotone** (add-only).
    pub fn insert(&mut self, v: T) {
        self.elems.insert(v);
    }
    #[must_use]
    pub fn contains(&self, v: &T) -> bool {
        self.elems.contains(v)
    }
    #[must_use]
    pub fn value(&self) -> &BTreeSet<T> {
        &self.elems
    }
}

impl<T: Ord + Clone> Lattice for GSet<T> {
    fn bottom() -> Self {
        Self::new()
    }
    fn join(&self, other: &Self) -> Self {
        Self {
            elems: self.elems.union(&other.elems).cloned().collect(),
        }
    }
}

/// A **max-register** — a monotone lattice on integers: `join` = max ("highest score wins"). Its op
/// ([`raise`](MaxReg::raise)) is **monotone**; note its dual **`min` ("lowest health") is NON-monotone** —
/// the boundary the classification draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaxReg(pub i64);

impl MaxReg {
    /// Monotone: the value only climbs.
    pub fn raise(&mut self, v: i64) {
        self.0 = self.0.max(v);
    }
}

impl Lattice for MaxReg {
    fn bottom() -> Self {
        MaxReg(i64::MIN)
    }
    fn join(&self, other: &Self) -> Self {
        MaxReg(self.0.max(other.0))
    }
}

// ── the ring-in-lattice: an OR-Set (retraction handled as a lattice) ─────────────────────────────────

/// A causal tag — `(actor, seq)`, unique per add — the "dot" that turns a non-monotone remove into a
/// monotone tombstone.
pub type Dot = (u32, u64);

/// An **observed-remove set** (OR-Set) — the **ring in a lattice** (FM-T1.2): adds are tagged with unique
/// [`Dot`]s; a remove **tombstones the observed dots** (a monotone `removes` set), so a non-monotone
/// *retraction* becomes a **join-semilattice** (both `adds` and `removes` only grow) that **converges**.
/// This is the **same add/remove delta-algebra M13.4's IVM maintained multiset uses** — one mechanism for
/// CRDT convergence and incremental maintenance. Concurrent add-vs-remove resolves **add-wins** (the
/// concurrent add's dot isn't in the observed tombstones).
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct OrSet<T: Ord + Clone> {
    adds: BTreeMap<T, BTreeSet<Dot>>,
    removes: BTreeSet<Dot>,
}

impl<T: Ord + Clone> OrSet<T> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            adds: BTreeMap::new(),
            removes: BTreeSet::new(),
        }
    }
    /// Add `v` with a unique causal tag (monotone: grows `adds`).
    pub fn add(&mut self, v: T, dot: Dot) {
        self.adds.entry(v).or_default().insert(dot);
    }
    /// Remove `v` — tombstones the currently-**observed** add-dots (the retraction, made monotone: grows
    /// `removes`; a concurrent add with an unobserved dot survives → add-wins).
    pub fn remove(&mut self, v: &T) {
        let observed: Vec<Dot> = self
            .adds
            .get(v)
            .map(|dots| dots.iter().copied().collect())
            .unwrap_or_default();
        for d in observed {
            self.removes.insert(d);
        }
    }
    #[must_use]
    pub fn contains(&self, v: &T) -> bool {
        self.adds
            .get(v)
            .is_some_and(|dots| dots.iter().any(|d| !self.removes.contains(d)))
    }
    #[must_use]
    pub fn value(&self) -> BTreeSet<T> {
        self.adds
            .iter()
            .filter(|(_, dots)| dots.iter().any(|d| !self.removes.contains(d)))
            .map(|(v, _)| v.clone())
            .collect()
    }
}

impl<T: Ord + Clone> Lattice for OrSet<T> {
    fn bottom() -> Self {
        Self::new()
    }
    fn join(&self, other: &Self) -> Self {
        let mut adds = self.adds.clone();
        for (v, dots) in &other.adds {
            adds.entry(v.clone())
                .or_default()
                .extend(dots.iter().copied());
        }
        Self {
            adds,
            removes: self.removes.union(&other.removes).copied().collect(),
        }
    }
}

// ── the typed monotone/non-monotone boundary (the keystone; const, no per-frame tax) ─────────────────

/// The regime an op is in — the **typed boundary**. `Monotone` ops are coordination-free, deterministic,
/// and incremental-for-free (CALM). `NonMonotone` ops (delete / min / overwrite / rollback) need a
/// **coordination point** (the [`OrSet`] ring-in-lattice / the shipped merge-validation, invariant 3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Monotonicity {
    Monotone,
    NonMonotone,
}

/// One op on the capability subsystem (the ADR-026 override/caps model, in miniature) — each carries its
/// **typed** monotonicity. `Grant` is add-only (monotone); `Revoke` is a retraction (non-monotone).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapOp {
    Grant(u32),
    Revoke(u32),
}

impl CapOp {
    /// The **typed classification** — a `const` property (compile-time, **no per-frame tax**): the engine
    /// always knows the regime without a runtime probe.
    #[must_use]
    pub const fn monotonicity(self) -> Monotonicity {
        match self {
            CapOp::Grant(_) => Monotonicity::Monotone,
            CapOp::Revoke(_) => Monotonicity::NonMonotone,
        }
    }
}

/// **SERVES-THE-AXIS**: an op-set is **coordination-free** (all-monotone) ⇒ its derived query is
/// incrementalizable **for free** (M13.4, no retraction path) and deterministic without a coordination
/// point (M13.1). A single non-monotone op ⇒ the subsystem needs the ring-in-lattice / merge-validation
/// coordination point. This is the concrete payoff the classification buys.
#[must_use]
pub fn is_coordination_free(ops: &[CapOp]) -> bool {
    ops.iter()
        .all(|op| op.monotonicity() == Monotonicity::Monotone)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every permutation of `0..n` (n small) — to prove order-independence exhaustively.
    fn permutations(n: usize) -> Vec<Vec<usize>> {
        if n <= 1 {
            return vec![(0..n).collect()];
        }
        let mut out = Vec::new();
        for i in 0..n {
            for mut sub in permutations(n - 1) {
                for x in &mut sub {
                    if *x >= i {
                        *x += 1;
                    }
                }
                let mut p = vec![i];
                p.extend(sub);
                out.push(p);
            }
        }
        out
    }

    #[test]
    fn monotone_grants_merge_order_independent_the_calm_proof() {
        // CALM (monotone ⇒ coordination-free): three replicas each grant a cap; merging in EVERY order
        // yields the SAME set — no coordination needed. (The ADR-026 convergence method, at the typed level.)
        let replicas: Vec<GSet<u32>> = [1u32, 2, 3]
            .iter()
            .map(|&c| {
                let mut g = GSet::new();
                g.insert(c);
                g
            })
            .collect();
        let expected = join_all(&replicas);
        for perm in permutations(replicas.len()) {
            let ordered: Vec<GSet<u32>> = perm.iter().map(|&i| replicas[i].clone()).collect();
            assert_eq!(
                join_all(&ordered),
                expected,
                "monotone joins are order-independent (coordination-free)"
            );
        }
        assert_eq!(expected.value(), &BTreeSet::from([1, 2, 3]));
    }

    #[test]
    fn a_max_register_is_monotone_and_order_independent() {
        // A max-register ("highest score") is monotone; min ("lowest health") is its non-monotone dual.
        let regs = [MaxReg(10), MaxReg(30), MaxReg(20)];
        let expected = join_all(&regs);
        for perm in permutations(regs.len()) {
            let ordered: Vec<MaxReg> = perm.iter().map(|&i| regs[i]).collect();
            assert_eq!(join_all(&ordered), expected);
        }
        assert_eq!(expected, MaxReg(30), "join = max, order-independent");
    }

    #[test]
    fn a_non_monotone_revoke_is_order_dependent_without_coordination() {
        // WITHOUT a coordination point (a plain set with grant=insert / revoke=remove), concurrent
        // grant+revoke of the SAME cap does NOT converge — the reason it's classified NON-monotone.
        let grant_then_revoke = {
            let mut s = BTreeSet::new();
            s.insert(7u32);
            s.remove(&7);
            s
        }; // {} (empty)
        let revoke_then_grant = {
            let mut s = BTreeSet::<u32>::new();
            s.remove(&7);
            s.insert(7);
            s
        }; // {7}
        assert_ne!(
            grant_then_revoke, revoke_then_grant,
            "a naive revoke is order-dependent (non-monotone) — it needs coordination"
        );
        assert_eq!(
            CapOp::Revoke(7).monotonicity(),
            Monotonicity::NonMonotone,
            "the op is correctly FLAGGED non-monotone"
        );
    }

    #[test]
    fn the_ring_in_lattice_orset_makes_the_retraction_converge() {
        // WITH the OR-Set coordination point, the SAME concurrent grant+revoke CONVERGES regardless of
        // merge order (the ring-in-lattice): replica A grants+revokes cap 7 (observing its own add);
        // replica B concurrently grants cap 7 (a different dot) — the concurrent add SURVIVES (add-wins).
        let mut a: OrSet<u32> = OrSet::new();
        a.add(7, (0, 0)); // A's grant
        a.remove(&7); // A's revoke (tombstones dot (0,0) only)

        let mut b: OrSet<u32> = OrSet::new();
        b.add(7, (1, 0)); // B's concurrent grant (dot (1,0), unobserved by A's revoke)

        // Merge in BOTH orders → identical, convergent, and cap 7 is PRESENT (add-wins).
        let ab = a.join(&b);
        let ba = b.join(&a);
        assert_eq!(
            ab, ba,
            "the OR-Set join is order-independent (a lattice) — the retraction converges"
        );
        assert!(
            ab.contains(&7),
            "the concurrent add survives the observed-remove (add-wins) — deterministic"
        );
        assert_eq!(ab.value(), BTreeSet::from([7]));
    }

    #[test]
    fn the_semilattice_laws_hold_idempotent_commutative_associative() {
        // The three laws that make merge order-independent (the "one mathematical object" property).
        let mk = |c: u32, dot: Dot| {
            let mut s: OrSet<u32> = OrSet::new();
            s.add(c, dot);
            s
        };
        let (x, y, z) = (mk(1, (0, 0)), mk(2, (1, 0)), mk(3, (2, 0)));
        assert_eq!(x.join(&x), x, "idempotent");
        assert_eq!(x.join(&y), y.join(&x), "commutative");
        assert_eq!(x.join(&y).join(&z), x.join(&y.join(&z)), "associative");
    }

    #[test]
    fn the_classification_is_typed_and_const_no_per_frame_tax() {
        // The boundary is a `const` type property — usable in a const context, so it is a compile-time
        // discipline, NOT a runtime per-frame cost.
        const G: Monotonicity = CapOp::Grant(1).monotonicity();
        const R: Monotonicity = CapOp::Revoke(1).monotonicity();
        assert_eq!(G, Monotonicity::Monotone);
        assert_eq!(R, Monotonicity::NonMonotone);
    }

    #[test]
    fn serves_the_axis_coordination_free_iff_all_monotone() {
        // The concrete payoff: an all-monotone op-set is coordination-free (feeds M13.4 incremental-for-free
        // + M13.1 determinism); a single non-monotone op flips it to "needs the coordination point".
        assert!(
            is_coordination_free(&[CapOp::Grant(1), CapOp::Grant(2), CapOp::Grant(3)]),
            "all grants → coordination-free (M13.4 incrementalizable for free)"
        );
        assert!(
            !is_coordination_free(&[CapOp::Grant(1), CapOp::Revoke(1)]),
            "a revoke → needs the ring-in-lattice / merge-validation coordination point"
        );
    }
}
