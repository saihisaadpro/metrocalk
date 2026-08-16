//! The semantic world model: named landforms as first-class, editable entities.
//!
//! ## Why this exists
//!
//! A layer stack describes a *texture of terrain* — "hills this big, everywhere". It cannot describe
//! **this mountain**, and so it cannot answer "raise it", "widen it", "flatten the ground there". Every
//! layer is global: changing one changes the whole world, which is also why every layer edit costs a full
//! rebuild. Sculpt strokes are local but they are anonymous dabs of height — you cannot name one, and you
//! cannot ask a stroke how wide it is.
//!
//! A [`WorldFeature`] is the missing middle: a **named landform with bounded support**. It has an identity
//! that survives regeneration, a skeleton (a point, a path or an area), a handful of meaningful parameters,
//! and a footprint outside which it contributes exactly nothing.
//!
//! That last property is the load-bearing one, and it buys three things at once:
//!
//! 1. **Language can address it.** "Raise the north ridge" resolves to a feature id.
//! 2. **Editing is local.** A feature's [`WorldFeature::bounds`] is precisely the region that must be
//!    rebuilt — no heuristics, no whole-world rebuild.
//! 3. **Evaluation stays cheap.** Features are bucketed into a uniform grid, so a point only ever consults
//!    the handful whose footprint covers it, however many the world holds.
//!
//! ## The model
//!
//! Following Galin et al., *Terrain Modeling from Feature Primitives* (Computer Graphics Forum, 2015): the
//! terrain is a construction tree whose leaves are parameterised skeletal primitives — mountains, ridges,
//! valleys, craters — combined by blending, carving and warping operators. This is the same idea, flattened
//! to an ordered list because the document is a CRDT and an ordered list of named things merges and diffs
//! far better than a tree (invariant 1).
//!
//! Each feature contributes, at a point:
//!
//! ```text
//! w(x,z) = falloff(d(x,z) / extent)        // 0 outside the footprint, 1 on the skeleton
//! e(x,z) = amplitude · profile(u) + detail  // the shape it wants to be
//! h      = op(h, w, e)                      // how it combines with what is already there
//! ```
//!
//! ## What is deliberately NOT here
//!
//! Rivers and roads stay [`crate::recipe::SplineDef`]s. They are already modelled as splines with real
//! grading — a road runs a moving average so it crosses dips instead of draping over them, and a river is
//! forced into a monotone descent so it cannot flow uphill. Re-expressing them as features would duplicate
//! working, better code. Features cover what splines cannot: landforms with area.
//!
//! Determinism is preserved exactly as elsewhere in this crate: no RNG, no time, no thread order, and the
//! per-feature detail is hashed from the feature's own id, so adding a feature never disturbs another's.

use crate::noise;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A feature's stable identity.
///
/// Stable is the whole point: it is what a description means by "that mountain" after the world has been
/// regenerated, undone, saved and reopened. Ids are allocated monotonically and never reused, so a stale
/// reference resolves to nothing rather than silently to a different landform.
pub type FeatureId = u32;

/// What a landform *is*, semantically. Decides its defaults and which downstream stages it touches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeatureKind {
    /// A single summit.
    Mountain,
    /// A hill — a mountain's smaller, rounder relative.
    Hill,
    /// A linear crest.
    Ridge,
    /// A linear depression between higher ground.
    Valley,
    /// An enclosed depression.
    Basin,
    /// A raised flat.
    Plateau,
    /// A steep linear break in slope.
    Cliff,
    /// A circular depression with a raised rim.
    Crater,
    /// An area levelled for building on.
    Pad,
    /// An area whose vegetation is denser — no elevation of its own.
    Forest,
    /// An area whose vegetation is cleared — no elevation of its own.
    Clearing,
    /// A named area carrying gameplay meaning only.
    Zone,
}

impl FeatureKind {
    /// The author-facing name.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Mountain => "mountain",
            Self::Hill => "hill",
            Self::Ridge => "ridge",
            Self::Valley => "valley",
            Self::Basin => "basin",
            Self::Plateau => "plateau",
            Self::Cliff => "cliff",
            Self::Crater => "crater",
            Self::Pad => "pad",
            Self::Forest => "forest",
            Self::Clearing => "clearing",
            Self::Zone => "zone",
        }
    }

    /// Whether this kind changes the ground height at all.
    ///
    /// A forest and a zone are real, addressable features that carry no elevation — asking them to move the
    /// ground would be a bug, and asking the elevation stage to rebuild for them is wasted work.
    #[must_use]
    pub fn shapes_ground(self) -> bool {
        !matches!(self, Self::Forest | Self::Clearing | Self::Zone)
    }

    /// Whether this kind changes where things are scattered.
    #[must_use]
    pub fn shapes_scatter(self) -> bool {
        // Every landform changes scatter indirectly (through slope and height), but only these three are
        // *about* it — the distinction is what lets a forest edit skip the elevation rebuild entirely.
        matches!(self, Self::Forest | Self::Clearing)
    }

    /// The default profile for a newly created feature of this kind.
    #[must_use]
    pub fn default_profile(self) -> Profile {
        match self {
            Self::Mountain => Profile::Peak,
            // The kinds that carry no elevation still need a profile for their scatter falloff; a dome is
            // the natural "fades out from the middle" shape.
            Self::Hill | Self::Basin | Self::Forest | Self::Clearing | Self::Zone => Profile::Dome,
            Self::Ridge | Self::Valley => Profile::Crest,
            Self::Plateau | Self::Pad => Profile::Mesa,
            Self::Cliff => Profile::Scarp,
            Self::Crater => Profile::Rim,
        }
    }

    /// The default operator for a newly created feature of this kind.
    #[must_use]
    pub fn default_op(self) -> FeatureOp {
        match self {
            // Rising ground adds to whatever is there, so a mountain on a slope still reads as a mountain.
            // A cliff rises too — a scarp profile with an Add operator is a step up in the ground.
            Self::Mountain | Self::Hill | Self::Ridge | Self::Crater | Self::Cliff => {
                FeatureOp::Add
            }
            // Depressions carve, so a valley cuts into the hill it crosses rather than floating a negative
            // bump over it.
            Self::Valley | Self::Basin => FeatureOp::Carve,
            // Flats level, and their target height is stored on the feature.
            Self::Plateau | Self::Pad => FeatureOp::Flatten,
            Self::Forest | Self::Clearing | Self::Zone => FeatureOp::None,
        }
    }
}

/// The cross-section a feature wants, as a function of normalised distance from its skeleton.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Profile {
    /// Sharp summit, concave flanks — a mountain.
    Peak,
    /// Rounded — a hill.
    Dome,
    /// Full height along the skeleton, falling to either side — a ridge or a valley floor.
    Crest,
    /// Flat top with a rolled edge — a plateau.
    Mesa,
    /// A step: full on one side, nothing on the other.
    Scarp,
    /// Depressed centre inside a raised ring — a crater.
    Rim,
}

impl Profile {
    /// Height multiplier at normalised distance `u` (0 on the skeleton, 1 at the edge of support).
    #[must_use]
    pub fn at(self, u: f32) -> f32 {
        let u = u.clamp(0.0, 1.0);
        match self {
            // Concave flanks: steep near the top, easing out at the foot.
            Self::Peak => (1.0 - u) * (1.0 - u),
            Self::Dome => 1.0 - noise::smoothstep(0.0, 1.0, u),
            // A crest holds its height for the first fifth, so a ridge reads as a line, not a string of
            // beads.
            Self::Crest => {
                if u < 0.2 {
                    1.0
                } else {
                    1.0 - noise::smoothstep(0.2, 1.0, u)
                }
            }
            // Flat to 60 %, then a rolled edge — the shape that makes a mesa read as a table.
            Self::Mesa => {
                if u < 0.6 {
                    1.0
                } else {
                    1.0 - noise::smoothstep(0.6, 1.0, u)
                }
            }
            Self::Scarp => 1.0 - noise::smoothstep(0.45, 0.55, u),
            // Raised ring at 70 % of the radius, floor below zero in the middle.
            Self::Rim => {
                let ring = 1.0 - ((u - 0.7) / 0.3).abs().min(1.0);
                let floor = -(1.0 - noise::smoothstep(0.0, 0.7, u)) * 0.6;
                ring.mul_add(1.0, floor)
            }
        }
    }
}

/// How a feature combines with the ground already under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FeatureOp {
    /// Add its elevation. The default for anything that rises.
    Add,
    /// Subtract its elevation, so it cuts in. The default for anything that sinks.
    Carve,
    /// Take whichever is higher — a floor that never lowers existing ground.
    Max,
    /// Take whichever is lower — a ceiling.
    Min,
    /// Level toward `target_m`, weighted by the falloff. Building sites and plateaus.
    Flatten,
    /// Contributes no elevation at all (forests, zones).
    None,
}

/// The geometry a feature is built around.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Skeleton {
    /// A single place. Mountains, hills, craters.
    Point {
        /// World X.
        x: f32,
        /// World Z.
        z: f32,
    },
    /// A polyline. Ridges, valleys, cliffs.
    Path {
        /// World XZ vertices, in order.
        points: Vec<[f32; 2]>,
    },
    /// A closed polygon. Plateaus, basins, forests, zones.
    Area {
        /// World XZ vertices, in order; the closing edge is implied.
        points: Vec<[f32; 2]>,
    },
}

impl Skeleton {
    /// Distance from `(x, z)` to the skeleton, in metres. Zero on it; zero anywhere *inside* an area, so an
    /// area feature is full-strength across its whole interior rather than only along its outline.
    #[must_use]
    pub fn distance(&self, x: f32, z: f32) -> f32 {
        match self {
            Self::Point { x: px, z: pz } => ((x - px).powi(2) + (z - pz).powi(2)).sqrt(),
            Self::Path { points } => polyline_distance(points, x, z),
            Self::Area { points } => {
                if point_in_polygon(points, x, z) {
                    0.0
                } else {
                    polyline_distance_closed(points, x, z)
                }
            }
        }
    }

    /// The skeleton's own XZ bounding box, before the support radius is added.
    #[must_use]
    pub fn bounds(&self) -> Footprint {
        match self {
            Self::Point { x, z } => ([*x, *z], [*x, *z]),
            Self::Path { points } | Self::Area { points } => {
                let mut min = [f32::MAX; 2];
                let mut max = [f32::MIN; 2];
                for p in points {
                    min[0] = min[0].min(p[0]);
                    min[1] = min[1].min(p[1]);
                    max[0] = max[0].max(p[0]);
                    max[1] = max[1].max(p[1]);
                }
                if points.is_empty() {
                    ([0.0, 0.0], [0.0, 0.0])
                } else {
                    (min, max)
                }
            }
        }
    }

    /// A representative point — where a label goes, and where "the height of this feature" is sampled.
    #[must_use]
    pub fn anchor(&self) -> [f32; 2] {
        match self {
            Self::Point { x, z } => [*x, *z],
            Self::Path { points } | Self::Area { points } => {
                if points.is_empty() {
                    return [0.0, 0.0];
                }
                let n = points.len() as f32;
                let sx: f32 = points.iter().map(|p| p[0]).sum();
                let sz: f32 = points.iter().map(|p| p[1]).sum();
                [sx / n, sz / n]
            }
        }
    }

    /// Move every vertex by `(dx, dz)`.
    pub fn translate(&mut self, dx: f32, dz: f32) {
        match self {
            Self::Point { x, z } => {
                *x += dx;
                *z += dz;
            }
            Self::Path { points } | Self::Area { points } => {
                for p in points.iter_mut() {
                    p[0] += dx;
                    p[1] += dz;
                }
            }
        }
    }

    /// Scale about the anchor — how "make it bigger" moves geometry rather than only widening the falloff.
    pub fn scale(&mut self, factor: f32) {
        let a = self.anchor();
        match self {
            Self::Point { .. } => {}
            Self::Path { points } | Self::Area { points } => {
                for p in points.iter_mut() {
                    p[0] = a[0] + (p[0] - a[0]) * factor;
                    p[1] = a[1] + (p[1] - a[1]) * factor;
                }
            }
        }
    }
}

/// A named landform.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldFeature {
    /// Stable identity. Never reused.
    pub id: FeatureId,
    /// Author-facing name — what a description says to address it ("the north ridge").
    pub name: String,
    /// What it is.
    pub kind: FeatureKind,
    /// Where it is.
    pub skeleton: Skeleton,
    /// How far its influence reaches beyond the skeleton, in metres. Outside this it contributes nothing —
    /// which is what makes editing local.
    pub extent_m: f32,
    /// Signed height in metres. Positive rises, negative sinks, whatever the operator.
    pub amplitude_m: f32,
    /// Cross-section.
    pub profile: Profile,
    /// How it combines with the ground under it.
    pub op: FeatureOp,
    /// Level to this height, for [`FeatureOp::Flatten`]. Ignored by every other operator.
    pub target_m: f32,
    /// Shapes the falloff: 1 is a smooth shoulder, higher is a tighter edge.
    pub edge: f32,
    /// How much seeded detail rides on the feature, as a fraction of its amplitude.
    pub roughness: f32,
    /// Detail wavelength in metres.
    pub detail_m: f32,
    /// Scatter density multiplier inside the footprint. `1.0` changes nothing; `0.0` clears it.
    pub scatter_scale: f32,
    /// Off keeps it in the document, addressable and restorable, without any effect.
    pub enabled: bool,
}

impl WorldFeature {
    /// A feature of `kind` at a skeleton, with that kind's defaults.
    #[must_use]
    pub fn new(
        id: FeatureId,
        name: impl Into<String>,
        kind: FeatureKind,
        skeleton: Skeleton,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            kind,
            skeleton,
            extent_m: 200.0,
            amplitude_m: if kind.shapes_ground() { 120.0 } else { 0.0 },
            profile: kind.default_profile(),
            op: kind.default_op(),
            target_m: 0.0,
            edge: 1.0,
            roughness: 0.25,
            detail_m: 60.0,
            scatter_scale: match kind {
                FeatureKind::Forest => 3.0,
                FeatureKind::Clearing => 0.0,
                _ => 1.0,
            },
            enabled: true,
        }
    }

    /// The world rectangle this feature can possibly affect.
    ///
    /// Exact, not conservative-by-a-lot: it is the skeleton's box grown by the support radius. Every
    /// invalidation decision downstream is this rectangle, so being tight here is what keeps an edit cheap.
    #[must_use]
    pub fn bounds(&self) -> Footprint {
        let (min, max) = self.skeleton.bounds();
        let r = self.extent_m.max(0.0);
        ([min[0] - r, min[1] - r], [max[0] + r, max[1] + r])
    }

    /// Influence at a point: 1 on the skeleton, 0 at and beyond the support radius.
    #[must_use]
    pub fn weight(&self, x: f32, z: f32) -> f32 {
        if !self.enabled {
            return 0.0;
        }
        let r = self.extent_m.max(0.01);
        let d = self.skeleton.distance(x, z);
        if d >= r {
            return 0.0;
        }
        let u = d / r;
        // Falling from 1 on the skeleton to 0 at the support radius.
        let w = 1.0 - noise::smoothstep(0.0, 1.0, u);
        // `edge` sharpens the shoulder without moving the support radius, so tightening an edge never
        // changes which chunks the feature touches.
        if (self.edge - 1.0).abs() < f32::EPSILON {
            w
        } else {
            w.powf(self.edge.clamp(0.05, 8.0))
        }
    }

    /// The elevation this feature wants at a point, before the operator combines it.
    #[must_use]
    fn elevation(&self, x: f32, z: f32, seed: u64) -> f32 {
        let r = self.extent_m.max(0.01);
        let u = (self.skeleton.distance(x, z) / r).min(1.0);
        let shape = self.profile.at(u);
        let mut e = self.amplitude_m * shape;
        if self.roughness > 0.0 {
            // Detail is hashed from the feature's OWN id, so adding, removing or reordering features never
            // disturbs the grain of the others.
            let fseed = seed ^ (u64::from(self.id).wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let wl = self.detail_m.max(1.0);
            let n = noise::fbm(x / wl, z / wl, fseed, 4, 2.0, 0.5);
            // Scaled by the shape so the detail fades out with the feature instead of leaving a rough
            // disc on flat ground at the edge of support.
            e += n * self.roughness * self.amplitude_m.abs() * shape;
        }
        e
    }

    /// Apply this feature to the height already accumulated at a point.
    #[must_use]
    pub fn apply(&self, h: f32, x: f32, z: f32, seed: u64) -> f32 {
        if !self.enabled || !self.kind.shapes_ground() || self.op == FeatureOp::None {
            return h;
        }
        let w = self.weight(x, z);
        if w <= 0.0 {
            return h;
        }
        let e = self.elevation(x, z, seed);
        match self.op {
            FeatureOp::Add => w.mul_add(e, h),
            FeatureOp::Carve => (-w).mul_add(e.abs(), h),
            // Weighted so the operator still fades out at the edge of support: a hard max would leave a
            // visible disc wherever the feature's surface crossed the ground.
            FeatureOp::Max => h + w * ((h + e).max(h) - h),
            FeatureOp::Min => h + w * ((h + e).min(h) - h),
            FeatureOp::Flatten => w.mul_add(self.target_m - h, h),
            FeatureOp::None => h,
        }
    }

    /// Scatter density multiplier at a point: 1 outside the footprint, blending to `scatter_scale` on the
    /// skeleton.
    #[must_use]
    pub fn scatter_at(&self, x: f32, z: f32) -> f32 {
        if !self.enabled || !self.kind.shapes_scatter() {
            return 1.0;
        }
        let w = self.weight(x, z);
        w.mul_add(self.scatter_scale - 1.0, 1.0).max(0.0)
    }
}

/// Distance from a point to a polyline, in metres. `f32::MAX` for an empty line.
#[must_use]
pub fn polyline_distance(points: &[[f32; 2]], x: f32, z: f32) -> f32 {
    match points.len() {
        0 => f32::MAX,
        1 => ((x - points[0][0]).powi(2) + (z - points[0][1]).powi(2)).sqrt(),
        _ => points
            .windows(2)
            .map(|s| segment_distance(s[0], s[1], x, z))
            .fold(f32::MAX, f32::min),
    }
}

/// As [`polyline_distance`], plus the closing edge — the boundary of a polygon.
#[must_use]
fn polyline_distance_closed(points: &[[f32; 2]], x: f32, z: f32) -> f32 {
    let open = polyline_distance(points, x, z);
    if points.len() < 3 {
        return open;
    }
    open.min(segment_distance(points[points.len() - 1], points[0], x, z))
}

fn segment_distance(a: [f32; 2], b: [f32; 2], x: f32, z: f32) -> f32 {
    let (dx, dz) = (b[0] - a[0], b[1] - a[1]);
    let len2 = dz.mul_add(dz, dx * dx);
    let t = if len2 <= f32::EPSILON {
        0.0
    } else {
        (((x - a[0]) * dx + (z - a[1]) * dz) / len2).clamp(0.0, 1.0)
    };
    let (px, pz) = (t.mul_add(dx, a[0]), t.mul_add(dz, a[1]));
    ((x - px).powi(2) + (z - pz).powi(2)).sqrt()
}

/// Ray-crossing inside test. Deterministic and allocation-free.
#[must_use]
pub fn point_in_polygon(points: &[[f32; 2]], x: f32, z: f32) -> bool {
    if points.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let (a, b) = (points[i], points[j]);
        if (a[1] > z) != (b[1] > z) {
            let t = (z - a[1]) / (b[1] - a[1]);
            if t.mul_add(b[0] - a[0], a[0]) > x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

/// A feature's XZ footprint: `(min, max)` corners in world metres.
pub type Footprint = ([f32; 2], [f32; 2]);

/// A uniform-grid index over features, so a point consults only the ones whose footprint covers it.
///
/// The same shape as the stroke and spline indexes in [`crate::field`] — one more spatial bucket table
/// rather than a new kind of structure.
#[derive(Clone, Debug, Default)]
pub struct FeatureIndex {
    cell: f32,
    /// Feature positions in the recipe's list, grouped by bucket.
    flat: Vec<u32>,
    buckets: BTreeMap<(i32, i32), (u32, u32)>,
}

impl FeatureIndex {
    /// Bucket every ground-shaping feature by its footprint.
    #[must_use]
    pub fn build(features: &[WorldFeature]) -> Self {
        let live: Vec<(u32, Footprint)> = features
            .iter()
            .enumerate()
            .filter(|(_, f)| f.enabled)
            .map(|(i, f)| (u32::try_from(i).unwrap_or(u32::MAX), f.bounds()))
            .collect();
        if live.is_empty() {
            return Self::default();
        }
        // A bucket at least as wide as the widest feature keeps each one in a handful of buckets.
        let widest = live
            .iter()
            .map(|(_, (min, max))| (max[0] - min[0]).max(max[1] - min[1]))
            .fold(0.0f32, f32::max);
        let cell = widest.max(64.0);
        let mut per_bucket: BTreeMap<(i32, i32), Vec<u32>> = BTreeMap::new();
        for (i, (min, max)) in &live {
            let x0 = (min[0] / cell).floor() as i32;
            let x1 = (max[0] / cell).floor() as i32;
            let z0 = (min[1] / cell).floor() as i32;
            let z1 = (max[1] / cell).floor() as i32;
            for cz in z0..=z1 {
                for cx in x0..=x1 {
                    per_bucket.entry((cx, cz)).or_default().push(*i);
                }
            }
        }
        let mut flat = Vec::new();
        let mut buckets = BTreeMap::new();
        for (k, v) in per_bucket {
            let start = u32::try_from(flat.len()).unwrap_or(u32::MAX);
            flat.extend(v);
            let len = u32::try_from(flat.len()).unwrap_or(u32::MAX) - start;
            buckets.insert(k, (start, len));
        }
        Self {
            cell,
            flat,
            buckets,
        }
    }

    /// The features whose bucket covers this point. Indices into the recipe's feature list.
    #[must_use]
    pub fn near(&self, x: f32, z: f32) -> &[u32] {
        if self.flat.is_empty() {
            return &[];
        }
        let key = (
            (x / self.cell).floor() as i32,
            (z / self.cell).floor() as i32,
        );
        match self.buckets.get(&key) {
            Some(&(start, len)) => &self.flat[start as usize..(start + len) as usize],
            None => &[],
        }
    }

    /// Whether the index holds anything at all — the fast path for a world with no features.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.flat.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mountain(id: FeatureId, x: f32, z: f32, amp: f32, extent: f32) -> WorldFeature {
        WorldFeature {
            extent_m: extent,
            amplitude_m: amp,
            roughness: 0.0,
            ..WorldFeature::new(id, "Peak", FeatureKind::Mountain, Skeleton::Point { x, z })
        }
    }

    #[test]
    fn a_feature_contributes_nothing_outside_its_footprint() {
        // The property everything else rests on: bounded support is what makes editing local.
        let m = mountain(1, 500.0, 500.0, 200.0, 100.0);
        assert_eq!(m.apply(10.0, 700.0, 500.0, 7), 10.0);
        assert_eq!(m.apply(10.0, 500.0, 380.0, 7), 10.0);
        assert_eq!(m.weight(601.0, 500.0), 0.0);
        // And on the skeleton it is at full strength.
        assert!((m.weight(500.0, 500.0) - 1.0).abs() < 1e-6);
        assert!(m.apply(10.0, 500.0, 500.0, 7) > 200.0);
    }

    #[test]
    fn the_bounds_really_do_contain_every_point_it_moves() {
        // If this were ever false, invalidation would miss chunks and leave a seam.
        for skeleton in [
            Skeleton::Point { x: 300.0, z: 400.0 },
            Skeleton::Path {
                points: vec![[100.0, 100.0], [400.0, 250.0], [600.0, 120.0]],
            },
            Skeleton::Area {
                points: vec![
                    [200.0, 200.0],
                    [500.0, 220.0],
                    [480.0, 460.0],
                    [180.0, 430.0],
                ],
            },
        ] {
            let f = WorldFeature {
                extent_m: 90.0,
                amplitude_m: 60.0,
                roughness: 0.5,
                ..WorldFeature::new(3, "F", FeatureKind::Hill, skeleton)
            };
            let (min, max) = f.bounds();
            for i in 0..90 {
                for k in 0..90 {
                    let (x, z) = (i as f32 * 10.0, k as f32 * 10.0);
                    let moved = (f.apply(0.0, x, z, 11) - 0.0).abs() > 1e-6;
                    if moved {
                        assert!(
                            x >= min[0] && x <= max[0] && z >= min[1] && z <= max[1],
                            "({x},{z}) moved but is outside {min:?}..{max:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn an_area_feature_is_full_strength_across_its_whole_interior() {
        // Not just along its outline — otherwise a plateau would be a ring.
        let f = WorldFeature {
            extent_m: 50.0,
            target_m: 100.0,
            ..WorldFeature::new(
                4,
                "Mesa",
                FeatureKind::Plateau,
                Skeleton::Area {
                    points: vec![[0.0, 0.0], [400.0, 0.0], [400.0, 400.0], [0.0, 400.0]],
                },
            )
        };
        for (x, z) in [(200.0, 200.0), (20.0, 380.0), (399.0, 1.0)] {
            assert!((f.weight(x, z) - 1.0).abs() < 1e-6, "({x},{z}) is inside");
            assert!((f.apply(0.0, x, z, 5) - 100.0).abs() < 1e-3, "levelled");
        }
        // Outside, untouched.
        assert_eq!(f.apply(7.0, 600.0, 200.0, 5), 7.0);
    }

    #[test]
    fn carving_cuts_down_and_adding_builds_up_whatever_the_sign() {
        let base = 50.0;
        let up = mountain(5, 0.0, 0.0, 80.0, 100.0);
        assert!(up.apply(base, 0.0, 0.0, 1) > base);
        let mut down = up.clone();
        down.op = FeatureOp::Carve;
        assert!(down.apply(base, 0.0, 0.0, 1) < base);
        // Carve with a NEGATIVE amplitude must still carve — the operator decides direction, not the sign,
        // or "make this valley deeper" would invert it.
        let mut negative = down.clone();
        negative.amplitude_m = -80.0;
        assert!(negative.apply(base, 0.0, 0.0, 1) < base);
    }

    #[test]
    fn detail_belongs_to_the_feature_that_owns_it() {
        // Adding a second mountain must not change the first one's grain — otherwise every edit visibly
        // reshuffles the whole world and "raise that peak" becomes "regenerate everything".
        let a = WorldFeature {
            roughness: 0.6,
            ..mountain(1, 0.0, 0.0, 100.0, 300.0)
        };
        let b = WorldFeature {
            roughness: 0.6,
            ..mountain(2, 2000.0, 0.0, 100.0, 300.0)
        };
        let alone: Vec<u32> = (0..40)
            .map(|i| a.apply(0.0, i as f32 * 5.0, 0.0, 9).to_bits())
            .collect();
        let together: Vec<u32> = (0..40)
            .map(|i| {
                b.apply(a.apply(0.0, i as f32 * 5.0, 0.0, 9), i as f32 * 5.0, 0.0, 9)
                    .to_bits()
            })
            .collect();
        assert_eq!(alone, together, "a distant feature perturbed this one");
    }

    #[test]
    fn the_index_returns_every_feature_covering_a_point() {
        let fs = vec![
            mountain(1, 100.0, 100.0, 50.0, 80.0),
            mountain(2, 120.0, 110.0, 50.0, 80.0),
            mountain(3, 5000.0, 5000.0, 50.0, 80.0),
        ];
        let idx = FeatureIndex::build(&fs);
        let near = idx.near(110.0, 105.0);
        assert!(near.contains(&0) && near.contains(&1), "{near:?}");
        assert!(!near.contains(&2), "a feature 5 km away is not near");
        // Every feature that actually moves the point must be in its bucket, or evaluation would silently
        // drop landforms.
        for (i, f) in fs.iter().enumerate() {
            for (x, z) in [(110.0, 105.0), (5000.0, 5000.0)] {
                if f.weight(x, z) > 0.0 {
                    assert!(
                        idx.near(x, z).contains(&u32::try_from(i).unwrap()),
                        "feature {i} affects ({x},{z}) but is not in its bucket"
                    );
                }
            }
        }
    }

    #[test]
    fn a_disabled_feature_stays_addressable_but_does_nothing() {
        let mut m = mountain(1, 0.0, 0.0, 100.0, 100.0);
        m.enabled = false;
        assert_eq!(m.apply(5.0, 0.0, 0.0, 1), 5.0);
        assert_eq!(m.weight(0.0, 0.0), 0.0);
        assert!(FeatureIndex::build(&[m.clone()]).is_empty());
        // Still named, still has an id — it can be turned back on by description.
        assert_eq!(m.id, 1);
    }

    #[test]
    fn forests_move_scatter_and_never_the_ground() {
        let f = WorldFeature {
            extent_m: 200.0,
            scatter_scale: 4.0,
            ..WorldFeature::new(
                9,
                "Pinewood",
                FeatureKind::Forest,
                Skeleton::Point { x: 0.0, z: 0.0 },
            )
        };
        assert_eq!(f.apply(12.0, 0.0, 0.0, 3), 12.0, "a forest is not a hill");
        assert!(f.scatter_at(0.0, 0.0) > 3.9);
        assert!((f.scatter_at(1000.0, 0.0) - 1.0).abs() < 1e-6);
        // And a clearing removes what is there.
        let c = WorldFeature {
            extent_m: 100.0,
            ..WorldFeature::new(
                10,
                "Glade",
                FeatureKind::Clearing,
                Skeleton::Point { x: 0.0, z: 0.0 },
            )
        };
        assert!(c.scatter_at(0.0, 0.0) < 0.01);
    }

    #[test]
    fn moving_and_scaling_a_skeleton_moves_its_footprint_with_it() {
        let mut f = WorldFeature::new(
            1,
            "Ridge",
            FeatureKind::Ridge,
            Skeleton::Path {
                points: vec![[0.0, 0.0], [100.0, 0.0]],
            },
        );
        f.extent_m = 50.0;
        let (min0, _) = f.bounds();
        f.skeleton.translate(500.0, 250.0);
        let (min1, _) = f.bounds();
        assert!((min1[0] - min0[0] - 500.0).abs() < 1e-3);
        assert!((min1[1] - min0[1] - 250.0).abs() < 1e-3);
        // Scaling grows the skeleton about its own anchor, so "make it twice as long" is expressible.
        let before = f.skeleton.bounds();
        f.skeleton.scale(2.0);
        let after = f.skeleton.bounds();
        let (w0, w1) = (before.1[0] - before.0[0], after.1[0] - after.0[0]);
        assert!((w1 - w0 * 2.0).abs() < 1e-3, "{w0} -> {w1}");
    }
}
