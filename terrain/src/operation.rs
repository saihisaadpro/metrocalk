//! Terrain operations: the inspectable, validated structure that natural language compiles into.
//!
//! ## The contract
//!
//! Nothing may reach the terrain by any other route. A description becomes a list of
//! [`TerrainOperation`]s; each one is *resolved* against the current world (which turns "this valley" into a
//! feature id and a rectangle), *checked* against its constraints, and only then applied. That is what makes
//! the natural-language layer safe to extend with a language model later: the model's whole output surface
//! is this struct, and the struct is validated by code the model cannot reach.
//!
//! ```text
//! "make this valley wider and suitable for vehicles"
//!        │
//!        ▼   plan::compile          (deterministic lexicon, or an LLM emitting the same structs)
//!   [ TerrainOperation { verb: Widen,           target: NearestOfKind(Valley, cursor), .. },
//!     TerrainOperation { verb: MakeTraversable, target: NearestOfKind(Valley, cursor),
//!                        constraints: [MaxSlope(0.25)], .. } ]
//!        │
//!        ▼   TerrainOperation::resolve          → a feature id, a rectangle, refusals with reasons
//!        ▼   ResolvedOperation::apply           → mutates the recipe, returns what it dirtied
//!   Applied { region, stages }                  → the runtime rebuilds exactly that
//! ```
//!
//! ## Why the resolve step is separate from the apply step
//!
//! Because the author must be able to *see the plan before it runs* — which feature it will touch, how much
//! it will move, what it will rebuild — and because a refusal has to be explicable. Resolving without
//! applying is also what lets the panel show "this will widen North Glen from 180 m to 260 m and rebuild
//! ground height, biomes, materials, vegetation" while the sentence is still editable.
//!
//! ## Refusals, not silent damage
//!
//! An impossible request is answered, never approximated into a broken world. Asking to flatten a region
//! that a constraint says must keep its slope, or to address a feature that does not exist, produces a
//! [`Refusal`] carrying the reason and — where there is one — the nearest thing that *would* work.

use crate::feature::{FeatureId, FeatureKind, FeatureOp, Skeleton, WorldFeature};
use crate::recipe::{SplineKind, TerrainRecipe};
use crate::stage::{Stage, StageMask};
use serde::{Deserialize, Serialize};

/// An axis-aligned world rectangle in XZ metres.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    /// Minimum corner `[x, z]`.
    pub min: [f32; 2],
    /// Maximum corner `[x, z]`.
    pub max: [f32; 2],
}

impl Rect {
    /// A rectangle from any two corners.
    #[must_use]
    pub fn new(a: [f32; 2], b: [f32; 2]) -> Self {
        Self {
            min: [a[0].min(b[0]), a[1].min(b[1])],
            max: [a[0].max(b[0]), a[1].max(b[1])],
        }
    }

    /// A square of `radius` metres around a point.
    #[must_use]
    pub fn around(p: [f32; 2], radius: f32) -> Self {
        let r = radius.abs();
        Self {
            min: [p[0] - r, p[1] - r],
            max: [p[0] + r, p[1] + r],
        }
    }

    /// The whole world.
    #[must_use]
    pub fn world(recipe: &TerrainRecipe) -> Self {
        Self {
            min: [0.0, 0.0],
            max: [recipe.world_size_m, recipe.world_size_m],
        }
    }

    /// The smallest rectangle containing both.
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        Self {
            min: [self.min[0].min(other.min[0]), self.min[1].min(other.min[1])],
            max: [self.max[0].max(other.max[0]), self.max[1].max(other.max[1])],
        }
    }

    /// Centre point.
    #[must_use]
    pub fn centre(self) -> [f32; 2] {
        [
            f32::midpoint(self.min[0], self.max[0]),
            f32::midpoint(self.min[1], self.max[1]),
        ]
    }

    /// Whether a point is inside.
    #[must_use]
    pub fn contains(self, p: [f32; 2]) -> bool {
        p[0] >= self.min[0] && p[0] <= self.max[0] && p[1] >= self.min[1] && p[1] <= self.max[1]
    }

    /// Half the shorter side — the radius of the largest circle that fits.
    #[must_use]
    pub fn inradius(self) -> f32 {
        ((self.max[0] - self.min[0]).min(self.max[1] - self.min[1]) * 0.5).max(1.0)
    }
}

/// What an operation acts on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "select", rename_all = "camelCase")]
pub enum Target {
    /// The whole terrain.
    World,
    /// A specific landform, by id.
    Feature {
        /// The id.
        id: FeatureId,
    },
    /// A landform by name — "the north ridge". Matched case-insensitively on a contained substring, so
    /// "north ridge" finds "North Ridge".
    Named {
        /// The name, or part of it.
        name: String,
    },
    /// The nearest landform of a kind to a point — how "this mountain" resolves when the author is looking
    /// at one.
    NearestOfKind {
        /// The kind to look for.
        kind: FeatureKind,
        /// Where the author is pointing.
        near: [f32; 2],
    },
    /// An explicit rectangle.
    Region {
        /// The rectangle.
        rect: Rect,
    },
    /// A disc around where the author is pointing — "here", "this area".
    Here {
        /// Centre.
        at: [f32; 2],
        /// Radius in metres.
        radius_m: f32,
    },
    /// A route (road or river) by index.
    Route {
        /// Index into the recipe's splines.
        index: usize,
    },
    /// The nearest route of a kind — "this river".
    NearestRoute {
        /// Road or river.
        kind: SplineKind,
        /// Where the author is pointing.
        near: [f32; 2],
    },
}

/// What to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Verb {
    /// Create a landform of a kind.
    Add,
    /// Increase height/depth.
    Raise,
    /// Decrease height/depth.
    Lower,
    /// Increase extent or route width.
    Widen,
    /// Decrease extent or route width.
    Narrow,
    /// Level toward a height.
    Flatten,
    /// Reduce the seeded detail.
    Smooth,
    /// Increase the seeded detail.
    Roughen,
    /// Plant vegetation.
    Plant,
    /// Remove vegetation.
    Clear,
    /// Bring the ground within a slope a vehicle can climb.
    MakeTraversable,
    /// Change the water level.
    Flood,
    /// Delete a landform.
    Remove,
    /// Give a landform a new name.
    Rename,
    /// Move a landform.
    Move,
}

impl Verb {
    /// Whether the verb needs an existing target rather than creating one.
    #[must_use]
    pub fn needs_existing(self) -> bool {
        !matches!(self, Self::Add | Self::Flatten | Self::Plant | Self::Clear)
    }

    /// Author-facing name.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Raise => "raise",
            Self::Lower => "lower",
            Self::Widen => "widen",
            Self::Narrow => "narrow",
            Self::Flatten => "flatten",
            Self::Smooth => "smooth",
            Self::Roughen => "roughen",
            Self::Plant => "plant",
            Self::Clear => "clear",
            Self::MakeTraversable => "make traversable",
            Self::Flood => "flood",
            Self::Remove => "remove",
            Self::Rename => "rename",
            Self::Move => "move",
        }
    }
}

/// A rule the result must satisfy. Checked *after* the operation is applied, against the real field — so a
/// constraint is a measured property of the world, not a promise about the parameters.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "rule", rename_all = "camelCase")]
pub enum Constraint {
    /// Ground gradient (rise/run) must not exceed this anywhere in the affected region.
    MaxSlope {
        /// The limit.
        slope: f32,
    },
    /// The ground must stay above this height.
    KeepAbove {
        /// Metres.
        height_m: f32,
    },
    /// The ground must stay below this height.
    KeepBelow {
        /// Metres.
        height_m: f32,
    },
    /// A vehicle of this class must be able to cross the region.
    Traversable {
        /// The steepest gradient the vehicle can climb.
        max_slope: f32,
    },
}

impl Constraint {
    /// The slope limit this constraint implies, if any.
    #[must_use]
    pub fn slope_limit(self) -> Option<f32> {
        match self {
            Self::MaxSlope { slope } => Some(slope),
            Self::Traversable { max_slope } => Some(max_slope),
            Self::KeepAbove { .. } | Self::KeepBelow { .. } => None,
        }
    }
}

/// Numbers an operation may carry. All optional: a verb uses what it needs and a sensible default
/// otherwise, so a terse sentence still produces a complete operation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Params {
    /// Absolute metres, for [`Verb::Flatten`] and [`Verb::Flood`].
    pub height_m: Option<f32>,
    /// Metres to add, for [`Verb::Raise`]/[`Verb::Lower`].
    pub delta_m: Option<f32>,
    /// Multiplier, for [`Verb::Widen`]/[`Verb::Narrow`] and density changes.
    pub factor: Option<f32>,
    /// Radius/extent in metres.
    pub radius_m: Option<f32>,
    /// The landform kind, for [`Verb::Add`].
    pub kind: Option<FeatureKind>,
    /// A name, for [`Verb::Add`] and [`Verb::Rename`].
    pub name: Option<String>,
    /// A destination, for [`Verb::Move`].
    pub to: Option<[f32; 2]>,
}

/// One authored intent, fully inspectable before it runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerrainOperation {
    /// What to do.
    pub verb: Verb,
    /// What to do it to.
    pub target: Target,
    /// The numbers.
    pub params: Params,
    /// An explicit spatial scope, when the sentence gave one. Otherwise the target decides.
    pub bounds: Option<Rect>,
    /// Rules the result must satisfy.
    pub constraints: Vec<Constraint>,
    /// Seed for anything this operation generates.
    pub seed: u64,
    /// The phrase this came from, so the author can see why an operation exists.
    pub source: String,
}

impl TerrainOperation {
    /// A bare operation with defaults.
    #[must_use]
    pub fn new(verb: Verb, target: Target) -> Self {
        Self {
            verb,
            target,
            params: Params::default(),
            bounds: None,
            constraints: Vec::new(),
            seed: 1,
            source: String::new(),
        }
    }
}

/// Why an operation could not be carried out.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Refusal {
    /// Plain language, addressed to the author.
    pub reason: String,
    /// The nearest thing that would work, when there is one.
    pub suggestion: Option<String>,
}

impl Refusal {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            suggestion: None,
        }
    }

    fn with(reason: impl Into<String>, suggestion: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            suggestion: Some(suggestion.into()),
        }
    }
}

/// An operation bound to a concrete piece of the world.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedOperation {
    /// The operation as authored.
    pub op: TerrainOperation,
    /// The landform it will act on, when it acts on one.
    pub feature: Option<FeatureId>,
    /// The route it will act on, when it acts on one.
    pub route: Option<usize>,
    /// The region it will change.
    pub region: Rect,
    /// What it will rebuild — already the transitive closure.
    pub stages: StageMask,
    /// A sentence describing what will happen, for the panel.
    pub effect: String,
    /// This raise/lower found no landform and will create one where the author is pointing.
    pub implicit_create: bool,
}

/// What actually changed.
#[derive(Clone, Debug, PartialEq)]
pub struct Applied {
    /// The region that changed.
    pub region: Rect,
    /// The stages to rebuild.
    pub stages: StageMask,
    /// A landform this operation created, if it created one.
    pub created: Option<FeatureId>,
    /// A sentence for the history.
    pub effect: String,
}

/// Resolve an operation against the current world.
///
/// # Errors
/// A [`Refusal`] when the target does not exist, the parameters are impossible, or the verb does not apply
/// to the thing it was aimed at.
#[allow(clippy::too_many_lines)] // one flat dispatch over the verb vocabulary; splitting it would scatter the semantics
pub fn resolve(
    op: &TerrainOperation,
    recipe: &TerrainRecipe,
) -> Result<ResolvedOperation, Refusal> {
    let feature = resolve_feature(&op.target, recipe)?;
    let route = resolve_route(&op.target, recipe)?;

    // "raise this" / "lower this" on bare ground CREATES a landform there. Pointing at a hillside and
    // saying "raise this" is the most natural edit there is, and refusing it would make the whole feature
    // feel broken. An unknown NAME still refuses — that is a mistake, not an implicit creation — and so
    // does "this mountain" when there is no mountain, because the author named a kind that is not there.
    let implicit_create = matches!(op.verb, Verb::Raise | Verb::Lower)
        && feature.is_none()
        && route.is_none()
        && matches!(
            op.target,
            Target::Here { .. } | Target::Region { .. } | Target::World
        );

    if op.verb.needs_existing() && feature.is_none() && route.is_none() && !implicit_create {
        return Err(match &op.target {
            Target::Named { name } => Refusal::with(
                format!("there is no landform called “{name}”"),
                nearest_names(recipe),
            ),
            Target::NearestOfKind { kind, .. } => Refusal::with(
                format!("there is no {} here to {}", kind.label(), op.verb.label()),
                format!("create one first — “add a {}”", kind.label()),
            ),
            Target::NearestRoute { kind, .. } => Refusal::new(format!(
                "there is no {} here to {}",
                match kind {
                    SplineKind::River => "river",
                    SplineKind::Road => "road",
                    SplineKind::Pad => "pad",
                },
                op.verb.label()
            )),
            _ => Refusal::new(format!(
                "“{}” needs an existing landform or route to act on",
                op.verb.label()
            )),
        });
    }

    // The region: an explicit scope wins; otherwise the target's own footprint.
    let region = op
        .bounds
        .unwrap_or_else(|| region_of(&op.target, feature, route, recipe));

    // Which stages this verb touches, before the closure.
    let touched = match op.verb {
        Verb::Rename => StageMask::none(),
        Verb::Plant | Verb::Clear => StageMask::of(&[Stage::Vegetation]),
        Verb::Flood => StageMask::of(&[Stage::Hydrology]),
        // Everything else moves ground.
        _ => StageMask::of(&[Stage::Features, Stage::Elevation]),
    };
    let stages = touched.closure();

    let name = feature
        .and_then(|id| recipe.features.iter().find(|f| f.id == id))
        .map_or_else(|| "the ground".to_string(), |f| f.name.clone());
    let effect = describe_effect(op, &name, region, stages);

    Ok(ResolvedOperation {
        op: op.clone(),
        feature,
        route,
        region,
        stages,
        effect,
        implicit_create,
    })
}

fn describe_effect(op: &TerrainOperation, name: &str, region: Rect, stages: StageMask) -> String {
    let what = match op.verb {
        Verb::Add => format!(
            "add a {} at {:.0}, {:.0}",
            op.params.kind.map_or("landform", FeatureKind::label),
            region.centre()[0],
            region.centre()[1]
        ),
        Verb::Raise => format!("raise {name} by {:.0} m", op.params.delta_m.unwrap_or(60.0)),
        Verb::Lower => format!(
            "lower {name} by {:.0} m",
            op.params.delta_m.unwrap_or(60.0).abs()
        ),
        Verb::Widen => format!("widen {name} by ×{:.2}", op.params.factor.unwrap_or(1.4)),
        Verb::Narrow => format!("narrow {name} by ×{:.2}", op.params.factor.unwrap_or(0.7)),
        Verb::Flatten => format!(
            "level the ground around {:.0}, {:.0}",
            region.centre()[0],
            region.centre()[1]
        ),
        Verb::Smooth => format!("smooth {name}"),
        Verb::Roughen => format!("roughen {name}"),
        Verb::Plant => "plant vegetation".to_string(),
        Verb::Clear => "clear vegetation".to_string(),
        Verb::MakeTraversable => format!(
            "bring {name} within a {:.0}% grade",
            op.constraints
                .iter()
                .copied()
                .find_map(Constraint::slope_limit)
                .unwrap_or(0.25)
                * 100.0
        ),
        Verb::Flood => format!(
            "set the water line to {:.1} m",
            op.params.height_m.unwrap_or(0.0)
        ),
        Verb::Remove => format!("remove {name}"),
        Verb::Rename => format!(
            "rename {name} to “{}”",
            op.params.name.clone().unwrap_or_default()
        ),
        Verb::Move => "move it".to_string(),
    };
    format!("{what} — rebuilds {}", stages.describe())
}

fn nearest_names(recipe: &TerrainRecipe) -> String {
    if recipe.features.is_empty() {
        return "this world has no named landforms yet".to_string();
    }
    let names: Vec<&str> = recipe
        .features
        .iter()
        .take(5)
        .map(|f| f.name.as_str())
        .collect();
    format!("this world has: {}", names.join(", "))
}

fn resolve_feature(target: &Target, recipe: &TerrainRecipe) -> Result<Option<FeatureId>, Refusal> {
    Ok(match target {
        Target::Feature { id } => {
            if recipe.features.iter().any(|f| f.id == *id) {
                Some(*id)
            } else {
                // A stale id must resolve to nothing rather than to whatever now sits at that slot.
                return Err(Refusal::new(format!(
                    "landform {id} no longer exists — it may have been removed or undone"
                )));
            }
        }
        Target::Named { name } => {
            let needle = name.trim().to_ascii_lowercase();
            recipe
                .features
                .iter()
                .find(|f| f.name.to_ascii_lowercase().contains(&needle))
                .map(|f| f.id)
        }
        // The author NAMED a kind, so search the whole world for one — "the mountain" means the
        // mountain, however far away it is. Unbounded on purpose, unlike a deictic.
        Target::NearestOfKind { kind, near } => nearest_feature(recipe, Some(*kind), *near, None),
        // "Here" and a bare region address the GROUND, and may still land on a landform — which is what
        // makes "raise this" work when the author is pointing at a mountain. But only if they are actually
        // near it: bounded by the pointing radius, so pointing at empty ground half a kilometre away does
        // not silently grab the nearest hill and edit that instead.
        Target::Here { at, radius_m } => nearest_feature(recipe, None, *at, Some(*radius_m)),
        Target::Region { rect } => {
            nearest_feature(recipe, None, rect.centre(), Some(rect.inradius()))
        }
        Target::World | Target::Route { .. } | Target::NearestRoute { .. } => None,
    })
}

/// The nearest enabled landform whose footprint contains the point, else the nearest by anchor.
fn nearest_feature(
    recipe: &TerrainRecipe,
    kind: Option<FeatureKind>,
    near: [f32; 2],
    within_m: Option<f32>,
) -> Option<FeatureId> {
    let matches = |f: &&WorldFeature| kind.is_none_or(|k| f.kind == k) && f.enabled;
    // Prefer one the author is actually standing in — that is what "this mountain" means.
    let covering = recipe
        .features
        .iter()
        .filter(matches)
        .filter(|f| f.weight(near[0], near[1]) > 0.0)
        .min_by(|a, b| {
            let da = a.skeleton.distance(near[0], near[1]);
            let db = b.skeleton.distance(near[0], near[1]);
            da.total_cmp(&db)
        });
    if let Some(f) = covering {
        return Some(f.id);
    }
    recipe
        .features
        .iter()
        .filter(matches)
        .filter(|f| {
            within_m.is_none_or(|r| f.skeleton.distance(near[0], near[1]) <= r + f.extent_m)
        })
        .min_by(|a, b| {
            let da = a.skeleton.distance(near[0], near[1]);
            let db = b.skeleton.distance(near[0], near[1]);
            da.total_cmp(&db)
        })
        .map(|f| f.id)
}

fn resolve_route(target: &Target, recipe: &TerrainRecipe) -> Result<Option<usize>, Refusal> {
    Ok(match target {
        Target::Route { index } => {
            if *index < recipe.splines.len() {
                Some(*index)
            } else {
                return Err(Refusal::new(format!(
                    "route {index} no longer exists — this world has {}",
                    recipe.splines.len()
                )));
            }
        }
        Target::NearestRoute { kind, near } => recipe
            .splines
            .iter()
            .enumerate()
            .filter(|(_, s)| s.kind == *kind && s.enabled)
            .min_by(|(_, a), (_, b)| {
                let da = crate::feature::polyline_distance(&flat(&a.points), near[0], near[1]);
                let db = crate::feature::polyline_distance(&flat(&b.points), near[0], near[1]);
                da.total_cmp(&db)
            })
            .map(|(i, _)| i),
        _ => None,
    })
}

fn flat(points: &[[f32; 3]]) -> Vec<[f32; 2]> {
    points.iter().map(|p| [p[0], p[2]]).collect()
}

fn region_of(
    target: &Target,
    feature: Option<FeatureId>,
    route: Option<usize>,
    recipe: &TerrainRecipe,
) -> Rect {
    if let Some(id) = feature {
        if let Some(f) = recipe.features.iter().find(|f| f.id == id) {
            let (min, max) = f.bounds();
            return Rect { min, max };
        }
    }
    if let Some(i) = route {
        if let Some(s) = recipe.splines.get(i) {
            let pad = s.width_m * 0.5 + s.falloff_m + s.clear_scatter_m + 4.0;
            let pts = flat(&s.points);
            let mut r = Rect::around(pts.first().copied().unwrap_or([0.0, 0.0]), pad);
            for p in &pts {
                r = r.union(Rect::around(*p, pad));
            }
            return r;
        }
    }
    match target {
        Target::Here { at, radius_m } => Rect::around(*at, *radius_m),
        Target::Region { rect } => *rect,
        _ => Rect::world(recipe),
    }
}

/// Apply a resolved operation to a recipe.
///
/// # Errors
/// A [`Refusal`] when the operation cannot be carried out on what it resolved to.
#[allow(clippy::too_many_lines)] // the verb dispatch, deliberately in one place
pub fn apply(res: &ResolvedOperation, recipe: &mut TerrainRecipe) -> Result<Applied, Refusal> {
    let op = &res.op;
    let mut created = None;

    // A raise with nothing to raise builds the landform it describes.
    let verb = if res.implicit_create {
        Verb::Add
    } else {
        op.verb
    };
    match verb {
        Verb::Add | Verb::Plant | Verb::Clear | Verb::Flatten => {
            let kind = op.params.kind.unwrap_or(match op.verb {
                Verb::Plant => FeatureKind::Forest,
                Verb::Clear => FeatureKind::Clearing,
                Verb::Flatten => FeatureKind::Pad,
                _ => FeatureKind::Hill,
            });
            let centre = res.region.centre();
            let extent = op
                .params
                .radius_m
                .unwrap_or_else(|| res.region.inradius())
                .max(1.0);
            let id = recipe.next_feature_id.max(1);
            recipe.next_feature_id = id + 1;
            let name = op
                .params
                .name
                .clone()
                .unwrap_or_else(|| default_name(kind, recipe));
            let mut f = WorldFeature::new(
                id,
                name,
                kind,
                Skeleton::Point {
                    x: centre[0],
                    z: centre[1],
                },
            );
            f.extent_m = extent;
            if let Some(a) = op.params.delta_m {
                // A "lower this" creates a depression, so the sign of the verb survives into the landform.
                f.amplitude_m = if op.verb == Verb::Lower { -a.abs() } else { a };
                if op.verb == Verb::Lower {
                    f.op = FeatureOp::Carve;
                }
            }
            if kind == FeatureKind::Pad || op.verb == Verb::Flatten {
                f.op = FeatureOp::Flatten;
                // The level to flatten to: what the sentence said, or the ground where the author pointed.
                f.target_m = op.params.height_m.unwrap_or(0.0);
                f.amplitude_m = 0.0;
                f.roughness = 0.0;
            }
            if let Some(fac) = op.params.factor {
                f.scatter_scale = fac.max(0.0);
            }
            created = Some(id);
            recipe.features.push(f);
        }
        Verb::Raise | Verb::Lower => {
            let f = feature_mut(recipe, res)?;
            let sign = if op.verb == Verb::Raise { 1.0 } else { -1.0 };
            let d = op.params.delta_m.unwrap_or(60.0).abs() * sign;
            if f.op == FeatureOp::Flatten {
                f.target_m += d;
            } else if f.op == FeatureOp::Carve {
                // Carving deepens downward, so "lower this valley" makes it deeper, not shallower.
                f.amplitude_m = (f.amplitude_m.abs() - d).max(0.0);
            } else {
                f.amplitude_m += d;
            }
        }
        Verb::Widen | Verb::Narrow => {
            let factor = op
                .params
                .factor
                .unwrap_or(if op.verb == Verb::Widen { 1.4 } else { 0.7 })
                .clamp(0.05, 20.0);
            if let Some(i) = res.route {
                let s = recipe.splines.get_mut(i).ok_or_else(|| {
                    Refusal::new("that route disappeared while the edit was pending")
                })?;
                s.width_m = (s.width_m * factor).clamp(1.0, 400.0);
                s.falloff_m = (s.falloff_m * factor).clamp(0.0, 400.0);
            } else {
                let f = feature_mut(recipe, res)?;
                f.extent_m = (f.extent_m * factor).clamp(1.0, 20_000.0);
            }
        }
        Verb::Smooth | Verb::Roughen => {
            let f = feature_mut(recipe, res)?;
            let factor =
                op.params
                    .factor
                    .unwrap_or(if op.verb == Verb::Smooth { 0.4 } else { 1.8 });
            f.roughness = (f.roughness * factor).clamp(0.0, 2.0);
            if op.verb == Verb::Roughen && f.roughness < 0.05 {
                // Roughening something perfectly smooth must do *something*, or the verb is a no-op that
                // looks like a bug.
                f.roughness = 0.2;
            }
        }
        Verb::MakeTraversable => {
            // Traversability is a slope problem, and the deterministic answer to a slope problem is to
            // reduce the relief that causes it. Flattening toward the region's own mean height keeps the
            // landform recognisable — "preserve geological plausibility" — instead of stamping a plate.
            let limit = op
                .constraints
                .iter()
                .copied()
                .find_map(Constraint::slope_limit)
                .unwrap_or(0.25);
            // `Flatten` is ABSOLUTE: it lerps the ground toward `target_m`. Defaulting that to 0.0 —
            // which is what this did — levelled the region to sea level rather than reducing its grade, so
            // "make this valley traversable" cut a crater through an alpine world down to y = 0 and called
            // it a corridor. There is no honest default for an absolute height, so refuse instead.
            //
            // Resolved BEFORE any part of the recipe is touched: a refusal must leave the world exactly as
            // it found it, and allocating the id first left `next_feature_id` advanced by a operation that
            // never happened.
            let target_m = op.params.height_m.ok_or_else(|| {
                Refusal::with(
                    "I do not know how high the ground is there, so I cannot level it",
                    "point at the ground you want to make traversable, or say a height",
                )
            })?;
            let centre = res.region.centre();
            let id = recipe.next_feature_id.max(1);
            recipe.next_feature_id = id + 1;
            let mut f = WorldFeature::new(
                id,
                format!("Traversable Corridor {id}"),
                FeatureKind::Pad,
                Skeleton::Point {
                    x: centre[0],
                    z: centre[1],
                },
            );
            f.extent_m = res.region.inradius();
            f.op = FeatureOp::Flatten;
            f.target_m = target_m;
            f.amplitude_m = 0.0;
            f.roughness = 0.0;
            // A partial level: enough to bring the grade down, not so much that the valley stops being one.
            f.edge = (1.0 / limit.max(0.02)).clamp(1.0, 6.0);
            created = Some(id);
            recipe.features.push(f);
        }
        Verb::Flood => {
            let level = op.params.height_m.ok_or_else(|| {
                Refusal::with("no water level was given", "say how high — “flood to 40 m”")
            })?;
            recipe.water.enabled = true;
            recipe.water.sea_level_m = level;
        }
        Verb::Remove => {
            let id = res
                .feature
                .ok_or_else(|| Refusal::new("nothing to remove"))?;
            recipe.features.retain(|f| f.id != id);
        }
        Verb::Rename => {
            let name = op
                .params
                .name
                .clone()
                .ok_or_else(|| Refusal::new("no new name was given"))?;
            feature_mut(recipe, res)?.name = name;
        }
        Verb::Move => {
            let to = op
                .params
                .to
                .ok_or_else(|| Refusal::new("no destination was given"))?;
            let f = feature_mut(recipe, res)?;
            let a = f.skeleton.anchor();
            f.skeleton.translate(to[0] - a[0], to[1] - a[1]);
        }
    }

    // The region an edit dirties is the union of where the target WAS and where it now IS — moving or
    // shrinking a landform must rebuild the ground it vacated as well as the ground it now covers.
    let mut region = res.region;
    if let Some(id) = created.or(res.feature) {
        if let Some(f) = recipe.features.iter().find(|f| f.id == id) {
            let (min, max) = f.bounds();
            region = region.union(Rect { min, max });
        }
    }
    if let Some(i) = res.route {
        if let Some(s) = recipe.splines.get(i) {
            let pad = s.width_m * 0.5 + s.falloff_m + s.clear_scatter_m + 4.0;
            for p in &s.points {
                region = region.union(Rect::around([p[0], p[2]], pad));
            }
        }
    }

    Ok(Applied {
        region,
        stages: res.stages,
        created,
        effect: res.effect.clone(),
    })
}

fn feature_mut<'a>(
    recipe: &'a mut TerrainRecipe,
    res: &ResolvedOperation,
) -> Result<&'a mut WorldFeature, Refusal> {
    let id = res
        .feature
        .ok_or_else(|| Refusal::new("that operation needs a landform, and none was found here"))?;
    recipe
        .features
        .iter_mut()
        .find(|f| f.id == id)
        .ok_or_else(|| Refusal::new("that landform disappeared while the edit was pending"))
}

fn default_name(kind: FeatureKind, recipe: &TerrainRecipe) -> String {
    let n = recipe.features.iter().filter(|f| f.kind == kind).count() + 1;
    let label = kind.label();
    let mut c = label.chars();
    let title = c.next().map_or_else(String::new, |f| {
        f.to_uppercase().collect::<String>() + c.as_str()
    });
    format!("{title} {n}")
}

/// Check a constraint against the terrain that resulted, by sampling.
///
/// The point of measuring rather than trusting the parameters: "make this traversable" is a claim about the
/// *world*, and the only honest way to know is to walk it. Returns the worst violation found, or `None`.
#[must_use]
pub fn check_constraints(
    constraints: &[Constraint],
    region: Rect,
    height: &dyn Fn(f32, f32) -> f32,
    samples: u32,
) -> Option<Refusal> {
    if constraints.is_empty() {
        return None;
    }
    let n = samples.max(2);
    let step_x = (region.max[0] - region.min[0]) / f32::from(u16::try_from(n).unwrap_or(u16::MAX));
    let step_z = (region.max[1] - region.min[1]) / f32::from(u16::try_from(n).unwrap_or(u16::MAX));
    let cell = step_x.max(step_z).max(0.5);
    let mut worst_slope = 0.0f32;
    let mut lowest = f32::MAX;
    let mut highest = f32::MIN;
    for iz in 0..n {
        for ix in 0..n {
            let x = step_x.mul_add(ix as f32, region.min[0]);
            let z = step_z.mul_add(iz as f32, region.min[1]);
            let h = height(x, z);
            lowest = lowest.min(h);
            highest = highest.max(h);
            let dx = (height(x + cell, z) - h) / cell;
            let dz = (height(x, z + cell) - h) / cell;
            worst_slope = worst_slope.max(dz.mul_add(dz, dx * dx).sqrt());
        }
    }
    for c in constraints {
        match *c {
            Constraint::MaxSlope { slope } | Constraint::Traversable { max_slope: slope } => {
                if worst_slope > slope {
                    return Some(Refusal::with(
                        format!(
                            "the steepest grade here is {:.0}%, over the {:.0}% limit",
                            worst_slope * 100.0,
                            slope * 100.0
                        ),
                        "widen the area, or allow a steeper grade",
                    ));
                }
            }
            Constraint::KeepAbove { height_m } => {
                if lowest < height_m {
                    return Some(Refusal::new(format!(
                        "the ground drops to {lowest:.1} m, below the {height_m:.1} m floor"
                    )));
                }
            }
            Constraint::KeepBelow { height_m } => {
                if highest > height_m {
                    return Some(Refusal::new(format!(
                        "the ground rises to {highest:.1} m, above the {height_m:.1} m ceiling"
                    )));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::Profile;

    fn world_with_a_mountain() -> TerrainRecipe {
        let mut r = TerrainRecipe {
            world_size_m: 2048.0,
            ..TerrainRecipe::default()
        };
        let mut m = WorldFeature::new(
            1,
            "North Ridge",
            FeatureKind::Mountain,
            Skeleton::Point { x: 600.0, z: 600.0 },
        );
        m.extent_m = 300.0;
        m.amplitude_m = 200.0;
        m.profile = Profile::Peak;
        r.features.push(m);
        r.next_feature_id = 2;
        r
    }

    fn run(op: &TerrainOperation, recipe: &mut TerrainRecipe) -> Result<Applied, Refusal> {
        let res = resolve(op, recipe)?;
        apply(&res, recipe)
    }

    #[test]
    fn a_landform_can_be_addressed_by_name_and_raised() {
        let mut r = world_with_a_mountain();
        let mut op = TerrainOperation::new(
            Verb::Raise,
            Target::Named {
                name: "north ridge".into(),
            },
        );
        op.params.delta_m = Some(80.0);
        let applied = run(&op, &mut r).expect("raise");
        assert!((r.features[0].amplitude_m - 280.0).abs() < 1e-3);
        // The region it dirtied is the landform's footprint, not the world.
        assert!(applied.region.max[0] - applied.region.min[0] < r.world_size_m);
        assert!(applied.region.contains([600.0, 600.0]));
        assert!(applied.stages.has(Stage::Elevation) && applied.stages.has(Stage::Lod));
    }

    #[test]
    fn this_mountain_resolves_to_the_one_the_author_is_pointing_at() {
        let mut r = world_with_a_mountain();
        let mut second = WorldFeature::new(
            2,
            "South Peak",
            FeatureKind::Mountain,
            Skeleton::Point {
                x: 1600.0,
                z: 1600.0,
            },
        );
        second.extent_m = 300.0;
        second.amplitude_m = 150.0;
        r.features.push(second);
        r.next_feature_id = 3;

        let op = TerrainOperation::new(
            Verb::Raise,
            Target::NearestOfKind {
                kind: FeatureKind::Mountain,
                near: [1550.0, 1620.0],
            },
        );
        let res = resolve(&op, &r).expect("resolve");
        assert_eq!(res.feature, Some(2), "must pick the one under the cursor");
        run(&op, &mut r).expect("raise");
        assert!(
            (r.features[0].amplitude_m - 200.0).abs() < 1e-3,
            "the other one is untouched"
        );
        assert!(r.features[1].amplitude_m > 150.0);
    }

    #[test]
    fn addressing_something_that_does_not_exist_is_refused_with_help() {
        let r = world_with_a_mountain();
        let op = TerrainOperation::new(
            Verb::Raise,
            Target::Named {
                name: "the volcano".into(),
            },
        );
        let err = resolve(&op, &r).expect_err("must refuse");
        assert!(err.reason.contains("volcano"), "{}", err.reason);
        // And it says what IS there, so the author can correct it.
        assert!(err.suggestion.unwrap_or_default().contains("North Ridge"));
    }

    #[test]
    fn a_stale_id_refuses_rather_than_hitting_a_different_landform() {
        // The reason ids are never reused: an operation queued against a removed landform must fail, not
        // silently retarget.
        let mut r = world_with_a_mountain();
        run(
            &TerrainOperation::new(Verb::Remove, Target::Feature { id: 1 }),
            &mut r,
        )
        .expect("remove");
        let err = resolve(
            &TerrainOperation::new(Verb::Raise, Target::Feature { id: 1 }),
            &r,
        )
        .expect_err("must refuse");
        assert!(err.reason.contains("no longer exists"), "{}", err.reason);
    }

    #[test]
    fn widening_grows_the_footprint_and_dirties_both_the_old_and_new_ground() {
        let mut r = world_with_a_mountain();
        let before = r.features[0].bounds();
        let mut op = TerrainOperation::new(Verb::Widen, Target::Feature { id: 1 });
        op.params.factor = Some(2.0);
        let applied = run(&op, &mut r).expect("widen");
        assert!((r.features[0].extent_m - 600.0).abs() < 1e-3);
        let after = r.features[0].bounds();
        // The dirty region must cover the NEW footprint...
        assert!(applied.region.contains(after.0) && applied.region.contains(after.1));
        // ...and the old one, or the ground it used to touch keeps a stale mesh.
        assert!(applied.region.contains(before.0) && applied.region.contains(before.1));
    }

    #[test]
    fn narrowing_still_rebuilds_the_ground_the_landform_vacated() {
        // The subtle direction: shrinking leaves stale terrain OUTSIDE the new footprint.
        let mut r = world_with_a_mountain();
        let before = r.features[0].bounds();
        let mut op = TerrainOperation::new(Verb::Narrow, Target::Feature { id: 1 });
        op.params.factor = Some(0.25);
        let applied = run(&op, &mut r).expect("narrow");
        assert!(
            applied.region.contains(before.0) && applied.region.contains(before.1),
            "the vacated ground must be rebuilt too"
        );
    }

    #[test]
    fn planting_a_forest_rebuilds_vegetation_and_nothing_geometric() {
        let mut r = world_with_a_mountain();
        let mut op = TerrainOperation::new(
            Verb::Plant,
            Target::Here {
                at: [400.0, 400.0],
                radius_m: 150.0,
            },
        );
        op.params.factor = Some(4.0);
        let applied = run(&op, &mut r).expect("plant");
        assert!(applied.stages.has(Stage::Vegetation));
        assert!(
            !applied.stages.has(Stage::Elevation),
            "a forest is not a hill"
        );
        assert!(!applied.stages.has(Stage::Collision));
        let f = r.features.last().expect("created");
        assert_eq!(f.kind, FeatureKind::Forest);
        assert!(f.scatter_scale > 3.0);
    }

    #[test]
    fn raising_bare_ground_creates_the_landform_it_describes() {
        // Pointing at a hillside and saying "raise this" must raise it — the live run refused, which made
        // the headline gesture of the whole feature look broken.
        let mut r = TerrainRecipe {
            world_size_m: 2048.0,
            ..TerrainRecipe::default()
        };
        let mut op = TerrainOperation::new(
            Verb::Raise,
            Target::Here {
                at: [900.0, 900.0],
                radius_m: 150.0,
            },
        );
        op.params.delta_m = Some(120.0);
        let res = resolve(&op, &r).expect("must not refuse bare ground");
        assert!(res.implicit_create);
        let applied = apply(&res, &mut r).expect("apply");
        let f = r.features.last().expect("a landform was created");
        assert!((f.amplitude_m - 120.0).abs() < 1e-3);
        assert!(
            f.apply(0.0, 900.0, 900.0, r.seed) > 100.0,
            "the ground actually rose"
        );
        assert!(applied.created.is_some());
        // Still bounded — creating from a deictic must not dirty the world.
        assert!(applied.region.max[0] - applied.region.min[0] < r.world_size_m);

        // "lower this" makes a hollow, not a hill.
        let mut down = TerrainOperation::new(
            Verb::Lower,
            Target::Here {
                at: [200.0, 200.0],
                radius_m: 100.0,
            },
        );
        down.params.delta_m = Some(50.0);
        let res2 = resolve(&down, &r).expect("resolve");
        apply(&res2, &mut r).expect("apply");
        let hollow = r.features.last().expect("created");
        assert!(hollow.apply(0.0, 200.0, 200.0, r.seed) < 0.0, "it dug down");
    }

    #[test]
    fn an_unknown_name_still_refuses_rather_than_quietly_creating() {
        // The line between "implicit create" and "you made a mistake": a name the world does not have is a
        // mistake, and inventing a landform for it would hide the typo.
        let r = world_with_a_mountain();
        let mut op = TerrainOperation::new(
            Verb::Raise,
            Target::Named {
                name: "the volcano".into(),
            },
        );
        op.params.delta_m = Some(100.0);
        let err = resolve(&op, &r).expect_err("must refuse");
        assert!(err.reason.contains("volcano"), "{}", err.reason);
    }

    #[test]
    fn a_rename_rebuilds_nothing_at_all() {
        let mut r = world_with_a_mountain();
        let mut op = TerrainOperation::new(Verb::Rename, Target::Feature { id: 1 });
        op.params.name = Some("Ben Nevis".into());
        let applied = run(&op, &mut r).expect("rename");
        assert_eq!(r.features[0].name, "Ben Nevis");
        assert!(applied.stages.is_empty(), "a rename is not a rebuild");
    }

    #[test]
    fn flattening_creates_a_levelled_pad_at_the_asked_height() {
        let mut r = world_with_a_mountain();
        let mut op = TerrainOperation::new(
            Verb::Flatten,
            Target::Here {
                at: [600.0, 600.0],
                radius_m: 120.0,
            },
        );
        op.params.height_m = Some(50.0);
        run(&op, &mut r).expect("flatten");
        let pad = r.features.last().expect("pad");
        assert_eq!(pad.op, FeatureOp::Flatten);
        assert!((pad.target_m - 50.0).abs() < 1e-3);
        // And it levels: at the centre the pad wins over the mountain under it.
        let h = pad.apply(500.0, 600.0, 600.0, r.seed);
        assert!((h - 50.0).abs() < 1e-3, "levelled to {h}");
    }

    #[test]
    fn constraints_are_measured_against_the_real_field_not_the_parameters() {
        let region = Rect::new([0.0, 0.0], [100.0, 100.0]);
        // A 100 % grade — a 45° ramp.
        let steep = |x: f32, _z: f32| x;
        let refusal = check_constraints(
            &[Constraint::Traversable { max_slope: 0.25 }],
            region,
            &steep,
            16,
        )
        .expect("must refuse a cliff");
        assert!(refusal.reason.contains('%'), "{}", refusal.reason);
        assert!(refusal.suggestion.is_some());
        // Flat ground passes the same check.
        let flat = |_x: f32, _z: f32| 12.0;
        assert!(check_constraints(
            &[Constraint::Traversable { max_slope: 0.25 }],
            region,
            &flat,
            16
        )
        .is_none());
    }

    #[test]
    fn height_floors_and_ceilings_are_checked_too() {
        let region = Rect::new([0.0, 0.0], [50.0, 50.0]);
        let ground = |_x: f32, _z: f32| -8.0;
        assert!(check_constraints(
            &[Constraint::KeepAbove { height_m: 0.0 }],
            region,
            &ground,
            8
        )
        .is_some());
        assert!(check_constraints(
            &[Constraint::KeepBelow { height_m: 0.0 }],
            region,
            &ground,
            8
        )
        .is_none());
    }

    #[test]
    fn widening_a_river_changes_the_route_not_a_landform() {
        let mut r = world_with_a_mountain();
        r.splines.push(crate::recipe::SplineDef {
            name: "Glen Water".into(),
            kind: SplineKind::River,
            points: vec![[100.0, 0.0, 100.0], [800.0, 0.0, 900.0]],
            width_m: 12.0,
            falloff_m: 10.0,
            depth_m: 3.0,
            use_point_height: false,
            material_layer: None,
            clear_scatter_m: 6.0,
            enabled: true,
        });
        let mut op = TerrainOperation::new(
            Verb::Widen,
            Target::NearestRoute {
                kind: SplineKind::River,
                near: [400.0, 500.0],
            },
        );
        op.params.factor = Some(2.0);
        let res = resolve(&op, &r).expect("resolve");
        assert_eq!(res.route, Some(0));
        run(&op, &mut r).expect("widen");
        assert!((r.splines[0].width_m - 24.0).abs() < 1e-3);
        // The mountain is untouched — "widen this river" must not resize a landform.
        assert!((r.features[0].extent_m - 300.0).abs() < 1e-3);
    }

    #[test]
    fn every_operation_says_what_it_will_do_before_it_does_it() {
        let r = world_with_a_mountain();
        let mut op = TerrainOperation::new(Verb::Raise, Target::Feature { id: 1 });
        op.params.delta_m = Some(120.0);
        let res = resolve(&op, &r).expect("resolve");
        assert!(res.effect.contains("North Ridge"), "{}", res.effect);
        assert!(res.effect.contains("120"), "{}", res.effect);
        assert!(res.effect.contains("rebuilds"), "{}", res.effect);
        // Resolving must not have changed anything.
        assert_eq!(r.features[0].amplitude_m, 200.0);
    }
}
