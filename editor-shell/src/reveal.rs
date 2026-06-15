//! M3.1 reveal engine — the north-star differentiator, deterministic and fully offline (no LLM).
//!
//! On entity select it answers "what can this connect to?" by running the **existing M1.5 `World`
//! compatibility query** (providers of a required capability that aren't yet bound — pair-match +
//! negation, ~12 µs) and ranks the compatible candidates by intent (proximity · affinity · recency).
//! The part that makes it feel categorically easier than inspect-and-wire — explaining **every "no"**
//! — is computed **per target on demand** ([`why_not`]): the UI greys the bounded set it actually
//! shows and asks the reason for each, so the hot path stays scene-size-independent (an eager
//! all-entities scan blew the 16 ms budget at 5k — measured 33 ms — and isn't the real UX anyway).
//!
//! Pure functions over the `World` + read-only maps, so they're deterministic (same scene → same
//! order) and headless-testable independent of the live shell.

// The map params are app-owned, default-hasher maps (registry names / positions); generalizing this
// internal API over the hasher `S` adds noise for no caller benefit.
#![allow(clippy::implicit_hasher)]

use std::collections::{HashMap, HashSet};

use metrocalk_ecs::{Clause, Entity, Target, Term, World};

/// The interned capability relationships (created by the registry / scene builder).
#[derive(Clone, Copy)]
pub struct Rels {
    pub provides: Entity,
    pub requires: Entity,
    pub binds_to: Entity,
}

/// Why a target can't be bound — derived from the registry, made specific and helpful.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhyNot {
    /// It provides capabilities, but none the selection requires (names the missing one).
    MissingCapability(String),
    /// It already has an outgoing binding (excluded by the not-yet-bound query).
    AlreadyBound,
    /// It provides no capabilities at all — nothing to bind to.
    NoCapability,
}

impl WhyNot {
    /// A human-facing, specific reason string ("every 'no' explained").
    #[must_use]
    pub fn explain(&self) -> String {
        match self {
            WhyNot::MissingCapability(cap) => format!("doesn't provide {cap}"),
            WhyNot::AlreadyBound => "already bound to something else".to_string(),
            WhyNot::NoCapability => "provides no capabilities".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub entity: Entity,
    pub distance: f32,
    /// How many of the selection's required capabilities this candidate provides (≥1 to be compatible).
    pub affinity: u32,
}

/// The reveal result: what the selection requires + the ranked compatible targets. Incompatible
/// reasons are computed on demand via [`why_not`] for whatever bounded set the UI greys.
#[derive(Debug, Clone, PartialEq)]
pub struct Reveal {
    pub required: Vec<String>,
    pub compatible: Vec<Candidate>,
}

/// Read-only context the ranking needs beyond the `World`.
pub struct Context<'a> {
    /// capability entity → name (from the registry), for the "doesn't provide X" reason.
    pub cap_name: &'a HashMap<Entity, String>,
    /// entity → world position, for proximity ranking (from `engine.components_of(..).Transform`).
    pub position: &'a HashMap<Entity, [f32; 3]>,
    /// entity → last-touched sequence (higher = more recent); absent ⇒ 0.
    pub recency: &'a HashMap<Entity, u64>,
}

/// The selection's required capabilities (entity handles).
#[must_use]
pub fn required_caps<W: World>(world: &W, selected: Entity, rels: Rels) -> Vec<Entity> {
    world.targets(selected, rels.requires)
}

/// Run the reveal for `selected` — ranked compatible targets via the **exact M1.5 indexed query**
/// (`with(Provides, C)` + `without(BindsTo, *)`). Scene-size-independent on the hot path.
#[must_use]
pub fn reveal<W: World>(world: &W, selected: Entity, rels: Rels, ctx: &Context<'_>) -> Reveal {
    let required = required_caps(world, selected, rels);
    let required_names: Vec<String> = required
        .iter()
        .filter_map(|c| ctx.cap_name.get(c).cloned())
        .collect();

    // Compatible: providers of any required cap with no outgoing binding (the proven query). Affinity
    // = how many of the required caps the entity provides, derived by counting which per-cap queries
    // it matched — NOT a per-entity `targets()` read (that O(matches) Flecs read blew the budget at
    // 5k). The hot path is then just the indexed query (per cap) + a hashmap tally + the rank.
    let mut hits: HashMap<Entity, u32> = HashMap::new();
    for &cap in &required {
        let q = world.build_query(&[
            Clause::with(Term::Pair {
                rel: rels.provides,
                target: Target::Exact(cap),
            }),
            Clause::without(Term::Pair {
                rel: rels.binds_to,
                target: Target::Any,
            }),
        ]);
        world.for_each_match(&q, &mut |e| {
            if e != selected {
                *hits.entry(e).or_insert(0) += 1;
            }
        });
    }
    let sel_pos = ctx.position.get(&selected).copied().unwrap_or([0.0; 3]);
    let mut compatible: Vec<Candidate> = hits
        .into_iter()
        .map(|(entity, affinity)| Candidate {
            entity,
            distance: dist(
                sel_pos,
                ctx.position.get(&entity).copied().unwrap_or([0.0; 3]),
            ),
            affinity,
        })
        .collect();

    // Deterministic intent ranking: nearer first, then better capability fit, then more-recently
    // touched, then a stable entity-id tiebreak (so the same scene always yields the same order).
    compatible.sort_by(|a, b| {
        a.distance
            .partial_cmp(&b.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.affinity.cmp(&a.affinity))
            .then_with(|| {
                let ra = ctx.recency.get(&a.entity).copied().unwrap_or(0);
                let rb = ctx.recency.get(&b.entity).copied().unwrap_or(0);
                rb.cmp(&ra)
            })
            .then(a.entity.0.cmp(&b.entity.0))
    });

    Reveal {
        required: required_names,
        compatible,
    }
}

/// Explain why `candidate` can't be bound to `selected`, or `None` if it actually IS compatible.
/// O(1) per target — the UI calls this for the bounded set of targets it greys (hover/inline), so
/// "every 'no' explained" never costs an all-entities scan.
#[must_use]
pub fn why_not<W: World>(
    world: &W,
    selected: Entity,
    rels: Rels,
    candidate: Entity,
    cap_name: &HashMap<Entity, String>,
) -> Option<WhyNot> {
    if candidate == selected {
        return None;
    }
    let required = required_caps(world, selected, rels);
    let req_set: HashSet<Entity> = required.iter().copied().collect();
    let provided = world.targets(candidate, rels.provides);
    let provides_required = provided.iter().any(|c| req_set.contains(c));
    let bound = world.has_pair(candidate, rels.binds_to, Target::Any);

    if provides_required && !bound {
        None // compatible
    } else if provides_required && bound {
        Some(WhyNot::AlreadyBound)
    } else if provided.is_empty() {
        Some(WhyNot::NoCapability)
    } else {
        // Name a required capability the candidate is *actually* missing (not merely the first one
        // required) — so a multi-capability requirer's greyed reason is specific, not arbitrary.
        let missing = required
            .iter()
            .find(|c| !provided.contains(c))
            .and_then(|c| cap_name.get(c).cloned())
            .unwrap_or_else(|| "the required capability".to_string());
        Some(WhyNot::MissingCapability(missing))
    }
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    (dx * dx + dy * dy + dz * dz).sqrt()
}
