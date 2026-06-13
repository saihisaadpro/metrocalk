//! Native [`World`] backend over Flecs v4.1 via `flecs_ecs` (ADR-001). This is the only module that
//! names `flecs_ecs`; the trait it implements is Flecs-free, and a CI check forbids `flecs_ecs`
//! outside this crate.
//!
//! Design notes:
//! - **Everything is a runtime id.** Relationships (`Provides`, `BindsTo`), tags (`Player`), and
//!   capability targets (`Health`) are all entities created via [`World::create_entity`]; this crate
//!   never uses `#[derive(Component)]` types, matching how the registry (M1.3) will hand out ids and
//!   keeping the surface identical to what a Loro-projection backend can offer.
//! - **One clear pair form.** The spike found `with`/`without` ambiguous between a value pair and a
//!   turbofish type pair. We expose exactly one: a [`Clause`] over runtime ids, translated here to
//!   the value form `with((rel, target))`; the wildcard target is the documented `flecs::Wildcard::ID`.
//! - **Safety locks ON** (`flecs_ecs` default features) — the ~0–10 % cost is the price of soundness
//!   (spike ②). Structural mutation is done inside [`World::defer`] (Flecs deferred mode).

use crate::{Clause, Entity, Target, Term, World};
use flecs_ecs::core::Entity as FlecsId;
use flecs_ecs::prelude::*;

/// The native Flecs-backed [`World`].
pub struct FlecsWorld {
    world: flecs_ecs::core::World,
}

impl Default for FlecsWorld {
    fn default() -> Self {
        Self::new()
    }
}

impl FlecsWorld {
    /// Create an empty world (safety locks ON via `flecs_ecs` default features).
    pub fn new() -> Self {
        Self {
            world: flecs_ecs::core::World::new(),
        }
    }
}

/// our id -> flecs id.
#[inline]
fn fid(e: Entity) -> FlecsId {
    FlecsId(e.0)
}

impl World for FlecsWorld {
    type Query = Query<()>;

    fn create_entity(&mut self) -> Entity {
        Entity(self.world.entity().id().0)
    }

    fn delete_entity(&mut self, e: Entity) {
        self.world.entity_from_id(e.0).destruct();
    }

    fn add_tag(&mut self, e: Entity, tag: Entity) {
        self.world.entity_from_id(e.0).add(fid(tag));
    }

    fn remove_tag(&mut self, e: Entity, tag: Entity) {
        self.world.entity_from_id(e.0).remove(fid(tag));
    }

    fn add_pair(&mut self, e: Entity, rel: Entity, target: Entity) {
        self.world.entity_from_id(e.0).add((fid(rel), fid(target)));
    }

    fn remove_pair(&mut self, e: Entity, rel: Entity, target: Entity) {
        self.world
            .entity_from_id(e.0)
            .remove((fid(rel), fid(target)));
    }

    fn has_tag(&self, e: Entity, tag: Entity) -> bool {
        self.world.entity_from_id(e.0).has(fid(tag))
    }

    fn has_pair(&self, e: Entity, rel: Entity, target: Target) -> bool {
        let e = self.world.entity_from_id(e.0);
        match target {
            Target::Exact(t) => e.has((fid(rel), fid(t))),
            Target::Any => e.has((fid(rel), flecs::Wildcard::ID)),
        }
    }

    fn targets(&self, e: Entity, rel: Entity) -> Vec<Entity> {
        let mut out = Vec::new();
        self.world
            .entity_from_id(e.0)
            .each_target(fid(rel), |t| out.push(Entity(t.id().0)));
        out
    }

    fn for_each_edge(&self, rel: Entity, f: &mut dyn FnMut(Entity, Entity)) {
        // One-shot uncached query over (rel, *); read each matched pair's target.
        let q = self
            .world
            .query::<()>()
            .with((fid(rel), flecs::Wildcard::ID))
            .build();
        q.each_iter(|it, i, ()| {
            let src = it.entity(i).id().0;
            let tgt = it.pair(0).second_id().id().0;
            f(Entity(src), Entity(tgt));
        });
    }

    fn build_query(&self, clauses: &[Clause]) -> Self::Query {
        let mut b = self.world.query::<()>();
        for c in clauses {
            match (c.term, c.present) {
                (Term::Tag(t), true) => {
                    b.with(fid(t));
                }
                (Term::Tag(t), false) => {
                    b.without(fid(t));
                }
                (
                    Term::Pair {
                        rel,
                        target: Target::Exact(t),
                    },
                    true,
                ) => {
                    b.with((fid(rel), fid(t)));
                }
                (
                    Term::Pair {
                        rel,
                        target: Target::Exact(t),
                    },
                    false,
                ) => {
                    b.without((fid(rel), fid(t)));
                }
                (
                    Term::Pair {
                        rel,
                        target: Target::Any,
                    },
                    true,
                ) => {
                    b.with((fid(rel), flecs::Wildcard::ID));
                }
                (
                    Term::Pair {
                        rel,
                        target: Target::Any,
                    },
                    false,
                ) => {
                    b.without((fid(rel), flecs::Wildcard::ID));
                }
            }
        }
        b.set_cached();
        b.build()
    }

    fn for_each_match(&self, query: &Self::Query, f: &mut dyn FnMut(Entity)) {
        query.each_entity(|e, ()| f(Entity(e.id().0)));
    }

    fn defer(&mut self, f: &mut dyn FnMut(&mut Self)) {
        let _ = self.world.defer_begin();
        f(self);
        let _ = self.world.defer_end();
    }
}
