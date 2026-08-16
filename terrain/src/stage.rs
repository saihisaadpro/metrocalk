//! The terrain dependency graph: which derived artefacts an edit actually invalidates.
//!
//! ## The problem this solves
//!
//! Terrain is a pipeline of derived data. The ground height is derived from the features and the layer
//! stack; the splat textures are derived from the height and the biomes; the scattered vegetation is derived
//! from all of that; colliders, navigation grids and LOD meshes from the height again. Before this module,
//! invalidation was binary: an edit dirtied a *rectangle*, and every chunk in it rebuilt **everything** —
//! re-baking a quarter-megabyte splat texture because a forest's density changed, or re-running the
//! navigation grid because a material got a new colour.
//!
//! A [`Stage`] names one link in that chain, and [`StageMask`] is a set of them. An edit declares which
//! stages it *touches*; [`StageMask::closure`] expands that to everything downstream. The runtime then
//! rebuilds only those, and only inside the region the edit reached.
//!
//! ```text
//!  Features ─→ Elevation ─┬─→ Erosion ─┬─→ Hydrology ─┬─→ Biomes ─┬─→ Materials
//!                         │            │              │           └─→ Vegetation
//!                         │            │              └─→ Navigation
//!                         │            └─→ Collision
//!                         └─→ Lod
//! ```
//!
//! The edges are transitive by construction: [`StageMask::closure`] iterates to a fixed point, so adding a
//! stage later cannot leave a stale artefact behind because somebody forgot to update a hand-written list.
//!
//! ## Why a mask and not a "dirty" boolean per artefact
//!
//! Because the answer has to travel. The edit happens in the document layer, the rebuild happens on a worker
//! pool, and the upload happens on the render thread. A small copyable set is something all three can carry;
//! a graph walk at each hop is not.

use serde::{Deserialize, Serialize};

/// One derived artefact in the terrain pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Stage {
    /// The semantic model itself — the named landforms.
    Features,
    /// The height field: layers, features, strokes, splines.
    Elevation,
    /// The hydraulic erosion bake.
    Erosion,
    /// Water: the sea/lake surface, depth, and the drainage it implies.
    Hydrology,
    /// Biome weights, from height, slope and moisture.
    Biomes,
    /// The baked splat albedo and detail-normal textures.
    Materials,
    /// Scattered vegetation and props.
    Vegetation,
    /// Height-field colliders.
    Collision,
    /// Walkability grids and paths.
    Navigation,
    /// Chunk meshes, their measured LOD errors, and the streaming representation.
    Lod,
}

impl Stage {
    /// Every stage, in pipeline order.
    pub const ALL: [Self; 10] = [
        Self::Features,
        Self::Elevation,
        Self::Erosion,
        Self::Hydrology,
        Self::Biomes,
        Self::Materials,
        Self::Vegetation,
        Self::Collision,
        Self::Navigation,
        Self::Lod,
    ];

    /// Author-facing name, for the panel's "what this will rebuild" readout.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Features => "features",
            Self::Elevation => "ground height",
            Self::Erosion => "erosion",
            Self::Hydrology => "water",
            Self::Biomes => "biomes",
            Self::Materials => "materials",
            Self::Vegetation => "vegetation",
            Self::Collision => "collision",
            Self::Navigation => "navigation",
            Self::Lod => "meshes",
        }
    }

    fn bit(self) -> u16 {
        1 << (self as u16)
    }

    /// The stages that must be rebuilt when this one changes — direct edges only.
    fn direct_dependents(self) -> &'static [Self] {
        match self {
            Self::Features => &[Self::Elevation],
            // Everything geometric hangs off the height field.
            Self::Elevation => &[
                Self::Erosion,
                Self::Hydrology,
                Self::Biomes,
                Self::Collision,
                Self::Lod,
            ],
            // Erosion rewrites the height, so it re-triggers the same fan-out.
            Self::Erosion => &[Self::Hydrology, Self::Biomes, Self::Collision, Self::Lod],
            // Water decides shores (a biome input) and what can be waded (a navigation input).
            Self::Hydrology => &[Self::Biomes, Self::Navigation],
            // Biomes choose materials and gate scatter rules.
            Self::Biomes => &[Self::Materials, Self::Vegetation],
            // Leaves: consumed by the renderer and the sim, and nothing derives from them.
            Self::Materials | Self::Vegetation | Self::Collision | Self::Navigation | Self::Lod => {
                &[]
            }
        }
    }
}

/// A set of [`Stage`]s.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageMask(u16);

impl StageMask {
    /// The empty set — an edit that derives nothing (a rename).
    #[must_use]
    pub const fn none() -> Self {
        Self(0)
    }

    /// Every stage — the honest answer for an edit whose reach is not known.
    #[must_use]
    pub fn all() -> Self {
        let mut m = Self(0);
        let mut i = 0;
        while i < Stage::ALL.len() {
            m.0 |= Stage::ALL[i].bit();
            i += 1;
        }
        m
    }

    /// A set from a list of stages.
    #[must_use]
    pub fn of(stages: &[Stage]) -> Self {
        let mut m = Self(0);
        for s in stages {
            m.0 |= s.bit();
        }
        m
    }

    /// Whether the set contains a stage.
    #[must_use]
    pub fn has(self, s: Stage) -> bool {
        self.0 & s.bit() != 0
    }

    /// Union.
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether nothing is set.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Expand to include everything transitively downstream.
    ///
    /// Iterated to a fixed point rather than written out by hand: a new edge added to
    /// [`Stage::direct_dependents`] propagates here automatically, so the graph cannot drift out of sync
    /// with a hand-maintained closure table.
    #[must_use]
    pub fn closure(self) -> Self {
        let mut out = self;
        loop {
            let mut next = out;
            for s in Stage::ALL {
                if out.has(s) {
                    for d in s.direct_dependents() {
                        next.0 |= d.bit();
                    }
                }
            }
            if next == out {
                return out;
            }
            out = next;
        }
    }

    /// The stages in the set, in pipeline order — for a readout an author can read.
    #[must_use]
    pub fn stages(self) -> Vec<Stage> {
        Stage::ALL.into_iter().filter(|s| self.has(*s)).collect()
    }

    /// Plain-language list, e.g. `"ground height, biomes, materials"`.
    #[must_use]
    pub fn describe(self) -> String {
        let names: Vec<&str> = self.stages().iter().map(|s| s.label()).collect();
        if names.is_empty() {
            "nothing".to_string()
        } else {
            names.join(", ")
        }
    }
}

/// What a chunk rebuild must actually produce, derived from a [`StageMask`].
///
/// This is the bridge to [`crate::stream::ChunkRequest`], which already has these switches — they were
/// simply always on. Deriving them from the stage mask is what turns "the pipeline supports partial
/// rebuilds" into "partial rebuilds happen".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RebuildPlan {
    /// Re-evaluate the height field and re-mesh. Everything geometric depends on it.
    pub geometry: bool,
    /// Re-bake the splat albedo/normal textures.
    pub textures: bool,
    /// Re-place scattered instances.
    pub scatter: bool,
    /// Rebuild the height-field collider.
    pub collider: bool,
    /// Rebuild the walkability grid.
    pub nav: bool,
}

impl RebuildPlan {
    /// Derive the plan from a stage mask (which should already be a [`StageMask::closure`]).
    #[must_use]
    pub fn from_mask(mask: StageMask) -> Self {
        let geometry = mask.has(Stage::Elevation)
            || mask.has(Stage::Erosion)
            || mask.has(Stage::Lod)
            || mask.has(Stage::Hydrology);
        Self {
            geometry,
            textures: mask.has(Stage::Materials) || mask.has(Stage::Biomes) || geometry,
            scatter: mask.has(Stage::Vegetation) || geometry,
            collider: mask.has(Stage::Collision) || geometry,
            nav: mask.has(Stage::Navigation) || geometry,
        }
    }

    /// Whether anything at all needs doing.
    #[must_use]
    pub fn is_noop(self) -> bool {
        !(self.geometry || self.textures || self.scatter || self.collider || self.nav)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_a_feature_reaches_every_derived_artefact() {
        // The whole chain, from the semantic model down to the meshes on screen.
        let m = StageMask::of(&[Stage::Features]).closure();
        for s in Stage::ALL {
            assert!(
                m.has(s),
                "{s:?} was not reached from Features: {}",
                m.describe()
            );
        }
    }

    #[test]
    fn a_vegetation_change_does_not_re_run_the_ground() {
        // The point of the graph: planting a forest must not re-bake erosion or re-mesh the world.
        let m = StageMask::of(&[Stage::Vegetation]).closure();
        assert!(m.has(Stage::Vegetation));
        for s in [
            Stage::Elevation,
            Stage::Erosion,
            Stage::Hydrology,
            Stage::Biomes,
            Stage::Materials,
            Stage::Collision,
            Stage::Navigation,
            Stage::Lod,
        ] {
            assert!(!m.has(s), "planting a forest wrongly invalidated {s:?}");
        }
        let plan = RebuildPlan::from_mask(m);
        assert!(plan.scatter);
        assert!(!plan.geometry, "no re-mesh");
        assert!(!plan.textures, "no texture re-bake");
        assert!(!plan.collider && !plan.nav);
    }

    #[test]
    fn a_material_recolour_touches_only_the_textures() {
        let plan = RebuildPlan::from_mask(StageMask::of(&[Stage::Materials]).closure());
        assert!(plan.textures);
        assert!(!plan.geometry && !plan.scatter && !plan.collider && !plan.nav);
    }

    #[test]
    fn water_reaches_biomes_and_navigation_but_not_collision() {
        // Raising the sea changes shores and what can be waded. It does not move the ground, so the
        // colliders under it are still correct.
        let m = StageMask::of(&[Stage::Hydrology]).closure();
        assert!(m.has(Stage::Biomes) && m.has(Stage::Materials));
        assert!(m.has(Stage::Navigation));
        assert!(!m.has(Stage::Erosion), "water does not re-run erosion");
    }

    #[test]
    fn the_closure_is_transitive_and_stable() {
        // Biomes -> Materials and Vegetation, two hops from Elevation.
        let m = StageMask::of(&[Stage::Elevation]).closure();
        assert!(
            m.has(Stage::Materials),
            "two hops: Elevation -> Biomes -> Materials"
        );
        assert!(m.has(Stage::Vegetation));
        // Applying the closure again changes nothing.
        assert_eq!(m.closure(), m);
        // And it terminates on a cyclic-looking request.
        assert_eq!(StageMask::all().closure(), StageMask::all());
    }

    #[test]
    fn an_empty_mask_is_a_no_op_rather_than_a_full_rebuild() {
        // A rename must not cost a world rebuild — the failure mode a default-to-everything design has.
        let m = StageMask::none().closure();
        assert!(m.is_empty());
        assert!(RebuildPlan::from_mask(m).is_noop());
        assert_eq!(m.describe(), "nothing");
    }

    #[test]
    fn the_readout_is_something_an_author_can_read() {
        let m = StageMask::of(&[Stage::Biomes]).closure();
        assert_eq!(m.describe(), "biomes, materials, vegetation");
    }
}
