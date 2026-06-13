//! # `metrocalk-ecs` — the engine's relational query surface
//!
//! The ONE crate where `flecs_ecs` and `unsafe` are permitted (the workspace forbids `unsafe`; this
//! crate is the documented exception — ADR-001). Everything else in the engine depends on the
//! [`World`] trait, never on Flecs: no `flecs_ecs` type appears in any trait signature, and a CI
//! check forbids `flecs_ecs` outside this crate.
//!
//! ## Anchored to the product's queries, not Flecs's feature set
//! The contract is exactly the compatibility queries the product is built on (ADR-006): pair-match
//! `(Provides, X)`, wildcard `(R, *)`, negation ("lacks `(BindsTo, *)`"), read-target, and the
//! minimal entity / pair / tag mutations they need. Flecs-only powers — transitive traversal,
//! `$var` joins, hierarchy up/down — are **deliberately not exposed**: the Phase-2 browser backend
//! could not mirror them and the product does not need them. If a future product query needs more,
//! widen [`Clause`]/[`Term`] deliberately and re-check both backends — do not reach behind the trait.
//!
//! ## Two backends (ADR-006), one trait
//! Native: [`flecs_backend::FlecsWorld`] (built here). Browser (Phase 2): a pure-Rust index over the
//! Loro projection — **not** built here, but the trait is shaped to admit it. Concretely, per method:
//! - [`World::create_entity`] / [`World::delete_entity`] → allocate a `u64` id / create+delete a
//!   Loro `MovableTree` node.
//! - [`World::add_tag`] / [`World::add_pair`] / `remove_*` → insert/remove in forward maps
//!   (entity→tags, entity→pairs) plus a reverse index `(rel, target) → entities`; on the Loro
//!   backend these mirror the document's component/binding maps and are maintained from it.
//! - [`World::has_tag`] / [`World::has_pair`] (incl. [`Target::Any`]) → membership test on those maps.
//! - [`World::targets`] → forward-index lookup of an entity's `(rel, *)` targets.
//! - [`World::for_each_edge`] → iterate the reverse index for `rel` (the bindings map).
//! - [`World::build_query`] / [`World::for_each_match`] → evaluate the clauses as set intersection
//!   (required terms) and difference (negated terms) over the indexes. Flecs maintains the match set
//!   incrementally; a Loro backend maintains the same indexes incrementally. Same matches, both ways.
//! - [`World::defer`] → bracket structural mutations (Flecs deferred mode; Loro: batch into one commit).
//! - [`World::set_sparse`] → a storage *hint*: Flecs marks the kind `DontFragment` (sparse, no
//!   archetype fragmentation — spike ② F1); a Loro backend ignores it (it never fragments).
//!
//! None of these requires a capability only Flecs has.
//!
//! ## The strongest case this trait can't be implemented over a Loro projection
//! The strongest case is **performance, not expressibility**. Flecs answers
//! [`World::for_each_match`] from an incrementally-cached, archetype-partitioned match set — matched
//! tables are pre-grouped, so iteration is the near-linear scan the spike measured at 12 µs p99. A
//! Loro-projection backend has no archetypes; it must intersect/difference id-sets, and a naive
//! rebuild-per-query would be `O(scene)`, not `O(matches)`. The trait is still *expressible* — set
//! algebra over indexes yields identical results — but matching Flecs's latency requires the Loro
//! backend to (a) maintain its indexes incrementally on every mutation rather than rebuild per
//! query, and (b) keep per-relationship reverse indexes so wildcard/negation terms are `O(result)`,
//! not `O(scene)`. That is real Phase-2 engineering, and ADR-006 already flags browser query latency
//! as the thing to benchmark — but it does **not** change the trait: no method demands a Flecs-only
//! capability, only a performance target the Loro backend must independently hit. If that benchmark
//! fails in Phase 2, the fallback is in ADR-006 (reduced browser feature set, or `emscripten`-Flecs),
//! not a trait change. The abstraction holds; the risk is contained to Phase-2 perf, where it's tracked.

pub mod flecs_backend;
pub mod rng;
pub mod scene;

pub use flecs_backend::FlecsWorld;

/// Opaque handle to an identity in the world — a scene entity, a relationship kind (e.g. `Provides`,
/// `BindsTo`), or a tag/capability target (e.g. `Health`, `Player`). Backends map it to their own
/// representation; a `u64` id space serves both Flecs (entity ids are `u64`) and a Loro-projection
/// backend (which assigns `u64` keys). Construction is the backend's job — callers obtain handles
/// from [`World::create_entity`] (and, later, the registry) and never fabricate them.
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd)]
pub struct Entity(pub u64);

/// The target side of a relationship pair in a query/predicate: a concrete entity, or the wildcard
/// `*` matching any target.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum Target {
    /// Match exactly this target entity, e.g. `(Provides, Health)`.
    Exact(Entity),
    /// Match any target, e.g. `(BindsTo, *)`.
    Any,
}

/// A single thing an entity can have: a tag/component, or a relationship pair.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum Term {
    /// Has the tag/component `0` (e.g. the role `Player`).
    Tag(Entity),
    /// Has the pair `(rel, target)` (e.g. `(Provides, Health)` or `(BindsTo, *)`).
    Pair {
        /// The relationship kind, e.g. `Provides` / `BindsTo`.
        rel: Entity,
        /// The target side (concrete or wildcard).
        target: Target,
    },
}

/// One clause of a query: a [`Term`] that must be present (`present = true`) or absent
/// (`present = false`, i.e. negation — "lacks …").
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct Clause {
    /// The term being constrained.
    pub term: Term,
    /// `true` = the entity must have the term; `false` = must not (negation).
    pub present: bool,
}

impl Clause {
    /// Convenience: a required term.
    pub fn with(term: Term) -> Self {
        Self {
            term,
            present: true,
        }
    }
    /// Convenience: a negated term ("lacks …").
    pub fn without(term: Term) -> Self {
        Self {
            term,
            present: false,
        }
    }
}

/// The engine's relational query surface. Implemented by [`FlecsWorld`] (native) and, in Phase 2, by
/// a pure-Rust index over the Loro projection (ADR-006) — see the crate-root docs for the per-method
/// mapping that keeps the second backend implementable.
///
/// Built-once / iterated-every-selection-change is the editor's steady state: [`build_query`] makes
/// a cached query, [`for_each_match`] re-reads its current matches.
///
/// [`build_query`]: World::build_query
/// [`for_each_match`]: World::for_each_match
pub trait World {
    /// A compiled, cached query. Opaque to callers — they hold it and pass it back to
    /// [`for_each_match`](World::for_each_match) / [`matches`](World::matches).
    type Query;

    // --- structure (mutations) ---

    /// Create a new entity and return its handle.
    fn create_entity(&mut self) -> Entity;
    /// Delete an entity (and, per backend semantics, its owned pairs/tags).
    fn delete_entity(&mut self, e: Entity);
    /// Add the tag/component `tag` to `e`.
    fn add_tag(&mut self, e: Entity, tag: Entity);
    /// Remove the tag/component `tag` from `e`.
    fn remove_tag(&mut self, e: Entity, tag: Entity);
    /// Add the pair `(rel, target)` to `e`.
    fn add_pair(&mut self, e: Entity, rel: Entity, target: Entity);
    /// Remove the pair `(rel, target)` from `e`.
    fn remove_pair(&mut self, e: Entity, rel: Entity, target: Entity);

    // --- predicates / reads ---

    /// Whether `e` has the tag/component `tag`.
    fn has_tag(&self, e: Entity, tag: Entity) -> bool;
    /// Whether `e` has a matching pair. [`Target::Any`] asks "has any `(rel, *)`".
    fn has_pair(&self, e: Entity, rel: Entity, target: Target) -> bool;
    /// The targets of `(rel, *)` on `e`, in a deterministic order.
    fn targets(&self, e: Entity, rel: Entity) -> Vec<Entity>;
    /// Visit every `(rel, *)` edge in the world as `(source, target)` — powers the relationship
    /// visualizer.
    fn for_each_edge(&self, rel: Entity, f: &mut dyn FnMut(Entity, Entity));

    // --- queries (built once, iterated every selection change) ---

    /// Compile + cache a query from `clauses` (required terms intersected, negated terms differenced).
    fn build_query(&self, clauses: &[Clause]) -> Self::Query;
    /// Visit the query's current matches (re-evaluated against current world state).
    fn for_each_match(&self, query: &Self::Query, f: &mut dyn FnMut(Entity));
    /// Collect the query's current matches.
    fn matches(&self, query: &Self::Query) -> Vec<Entity> {
        let mut out = Vec::new();
        self.for_each_match(query, &mut |e| out.push(e));
        out
    }

    // --- deferred mutation ---

    /// Run `f` with structural mutations deferred, so they are safe to issue during/around query
    /// iteration (Flecs deferred mode; a Loro backend batches into one commit).
    fn defer(&mut self, f: &mut dyn FnMut(&mut Self));

    // --- storage hint ---

    /// Hint that pairs/components of `kind` should use sparse, non-fragmenting storage. Call BEFORE
    /// adding any pair/tag of `kind`. This is a storage hint, not a query feature: native Flecs marks
    /// `kind` `DontFragment` (no archetype fragmentation — spike ② F1); a Loro-projection backend may
    /// ignore it (it doesn't fragment). Query results are identical either way.
    fn set_sparse(&mut self, kind: Entity);
}
