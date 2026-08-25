//! Pipe Forge — a deterministic, non-LLM procedural asset compiler.
//!
//! The editable source is a small, versioned [`PipeRecipe`].  A bake turns it into an immutable,
//! content-addressed artifact containing the cooked mesh, analytic normals/arc-length UVs, PBR texture
//! set and an honest build report.  Preview state stays outside the scene document; only a successful,
//! persisted artifact is landed by the desktop shell as one undoable transaction.

#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fmt;

use bincode::Options;
use metrocalk_assets::{AssetId, Material, MeshAsset, MeshGpu, Primitive, Texture};
use metrocalk_core::caps::canonical;
use metrocalk_core::variant::INSTANCE_META;
use metrocalk_core::{Engine, EntityId, FieldValue, Op, PipelineError};
use metrocalk_ecs::FlecsWorld;
use serde::{Deserialize, Serialize};

/// Self-describing prefix used by the regular content-addressed blob store.
pub const PIPE_ARTIFACT_MAGIC: &[u8; 8] = b"MTKPIPE1";
/// Current editable/cooked artifact schema.
pub const PIPE_RECIPE_VERSION: u32 = 2;
const LEGACY_PIPE_RECIPE_VERSION: u32 = 1;
const MAX_POINTS: usize = 128;
const MAX_ROUTE_NODES: usize = 256;
const MAX_ROUTE_EDGES: usize = 384;
const MAX_FITTINGS: usize = 192;
const MAX_CATALOG_ENTRIES: usize = 128;
const MIN_SEGMENT_M: f32 = 0.025;
// A valid hero-quality 128-point run is well below this. The up-front byte cap prevents hostile
// length prefixes from turning a small asset load into an unbounded deserialize/allocation request.
const MAX_ARTIFACT_BYTES: usize = 16 * 1024 * 1024;
const MAX_MESH_VERTICES: usize = 250_000;
const MAX_MESH_INDICES: usize = 1_500_000;
const MAX_MESH_PRIMITIVES: usize = 768;
const WELD_EPSILON_M: f32 = 1.0e-5;

/// A first-party visual/geometry kit.  The recipe stores the kit, so a later rebuild remains editable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PipeKit {
    #[default]
    Galvanized,
    Copper,
    Pvc,
    Scifi,
}

impl PipeKit {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Galvanized => "Galvanized steel",
            Self::Copper => "Copper",
            Self::Pvc => "PVC",
            Self::Scifi => "Sci-fi alloy",
        }
    }
}

/// Preview/final mesh density.  Quality affects radial/curve tessellation and texture resolution.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PipeQuality {
    Preview,
    #[default]
    Production,
    Hero,
}

impl PipeQuality {
    const fn radial_segments(self) -> usize {
        match self {
            Self::Preview => 12,
            Self::Production => 24,
            Self::Hero => 36,
        }
    }

    const fn bend_steps(self) -> usize {
        match self {
            Self::Preview => 3,
            Self::Production => 7,
            Self::Hero => 12,
        }
    }

    const fn texture_resolution(self) -> u32 {
        match self {
            Self::Preview => 64,
            Self::Production => 256,
            Self::Hero => 512,
        }
    }
}

/// The approachable controls exposed in the viewport panel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipeForgeOptions {
    pub kit: PipeKit,
    pub diameter_cm: f32,
    pub quality: PipeQuality,
    pub auto_fittings: bool,
}

impl Default for PipeForgeOptions {
    fn default() -> Self {
        Self {
            kit: PipeKit::Galvanized,
            diameter_cm: 10.0,
            quality: PipeQuality::Production,
            auto_fittings: true,
        }
    }
}

/// Stable editable route node. `primary_index` links the compatibility `points` list used by the
/// viewport drawing tool to the graph without making array order an identity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipeRouteNode {
    pub id: u32,
    pub position: [f32; 3],
    #[serde(default)]
    pub primary_index: Option<u32>,
}

/// A topological route edge. Per-edge diameters allow realistic smaller branch runs while preserving
/// the recipe-level diameter as the approachable default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipeRouteEdge {
    pub id: u32,
    pub from: u32,
    pub to: u32,
    pub diameter_m: f32,
}

/// Versioned graph topology kept in every V2 artifact. IDs survive decode/edit/rebake cycles and are
/// therefore suitable viewport route handles rather than transient vector indices.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipeRouteGraph {
    #[serde(default)]
    pub nodes: Vec<PipeRouteNode>,
    #[serde(default)]
    pub edges: Vec<PipeRouteEdge>,
}

/// Semantic fitting categories understood by the procedural compiler and export metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PipeFittingKind {
    Elbow,
    Coupling,
    Tee,
    Valve,
    Flange,
}

/// A reusable project fitting. `asset_handle` records a user-supplied source asset for downstream
/// replacement/export; the native compiler always creates a bounded semantic proxy as a dependable
/// viewport and bake fallback.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserFittingCatalogEntry {
    pub id: String,
    pub label: String,
    pub kind: PipeFittingKind,
    #[serde(default)]
    pub asset_handle: Option<String>,
    pub diameter_scale: f32,
    pub length_scale: f32,
}

/// A fitting attached to a stable route node. It remains attached when that node is moved after bake.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipeFittingPlacement {
    pub id: u32,
    pub node_id: u32,
    pub kind: PipeFittingKind,
    #[serde(default)]
    pub catalog_id: Option<String>,
    /// Inferred elbows/tees are refreshed when handles move; explicit user placements remain stable.
    #[serde(default)]
    pub automatic: bool,
}

/// Read model for direct viewport handle editing after an artifact has been restored.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipeRouteHandle {
    pub node_id: u32,
    pub position: [f32; 3],
    pub connected_edges: Vec<u32>,
    pub fitting_ids: Vec<u32>,
}

/// Editable procedural source. Points are world-space while drawing and are localized to point zero in
/// the artifact, so duplicate shapes deduplicate even when placed in different parts of a level.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipeRecipe {
    pub version: u32,
    pub points: Vec<[f32; 3]>,
    pub diameter_m: f32,
    pub kit: PipeKit,
    pub quality: PipeQuality,
    pub auto_fittings: bool,
    pub grid_m: f32,
    pub bend_radius_m: f32,
    /// Canonical V2 route topology. An empty graph is accepted only while authoring an empty route or
    /// while upgrading a V1 recipe; baking materializes it deterministically.
    #[serde(default)]
    pub graph: PipeRouteGraph,
    #[serde(default)]
    pub fittings: Vec<PipeFittingPlacement>,
    #[serde(default)]
    pub fitting_catalog: Vec<UserFittingCatalogEntry>,
}

impl PipeRecipe {
    /// Start an empty recipe from the UI controls, with production-safe bounds.
    #[must_use]
    pub fn from_options(options: &PipeForgeOptions) -> Self {
        let diameter_m = (options.diameter_cm * 0.01).clamp(0.01, 2.0);
        Self {
            version: PIPE_RECIPE_VERSION,
            points: Vec::new(),
            diameter_m,
            kit: options.kit,
            quality: options.quality,
            auto_fittings: options.auto_fittings,
            grid_m: 0.1,
            bend_radius_m: (diameter_m * 1.75).max(0.075),
            graph: PipeRouteGraph::default(),
            fittings: Vec::new(),
            fitting_catalog: Vec::new(),
        }
    }

    /// Add a point with 10 cm grid + 45°/90° route snapping. Duplicate/too-short clicks are explained.
    pub fn add_point(&mut self, world: [f32; 3]) -> Result<[f32; 3], PipeForgeError> {
        if self.points.len() >= MAX_POINTS {
            return Err(PipeForgeError::TooManyPoints);
        }
        if !finite3(world) {
            return Err(PipeForgeError::NonFinitePoint);
        }
        let mut p = snap_grid(world, self.grid_m.max(0.001));
        if let Some(&last) = self.points.last() {
            p = snap_direction(last, p, self.grid_m.max(0.001));
            if distance(last, p) < MIN_SEGMENT_M {
                return Err(PipeForgeError::SegmentTooShort);
            }
        }
        self.materialize_graph()?;
        let previous_node = self
            .points
            .len()
            .checked_sub(1)
            .and_then(|index| self.primary_node_id(index));
        let primary_index = u32::try_from(self.points.len())
            .map_err(|_| PipeForgeError::InvalidArtifact("route point ID overflow"))?;
        let node_id = self.next_node_id()?;
        self.points.push(p);
        self.graph.nodes.push(PipeRouteNode {
            id: node_id,
            position: p,
            primary_index: Some(primary_index),
        });
        if let Some(from) = previous_node {
            let edge_id = self.next_edge_id()?;
            self.graph.edges.push(PipeRouteEdge {
                id: edge_id,
                from,
                to: node_id,
                diameter_m: self.diameter_m,
            });
        }
        Ok(p)
    }

    /// Add a snapped branch beginning at an existing stable route handle. `points` excludes the
    /// attachment node. The returned node IDs are immediately usable as new viewport handles.
    pub fn add_branch(
        &mut self,
        from_node: u32,
        points: &[[f32; 3]],
        diameter_m: Option<f32>,
    ) -> Result<Vec<u32>, PipeForgeError> {
        let original = self.clone();
        let result = (|| {
            self.materialize_graph()?;
            if points.is_empty() {
                return Err(PipeForgeError::NeedTwoPoints);
            }
            if self.graph.nodes.len().saturating_add(points.len()) > MAX_ROUTE_NODES
                || self.graph.edges.len().saturating_add(points.len()) > MAX_ROUTE_EDGES
            {
                return Err(PipeForgeError::TooManyPoints);
            }
            let diameter = diameter_m.unwrap_or(self.diameter_m);
            validate_diameter(diameter)?;
            let mut last_node = from_node;
            let mut last_position =
                self.node_position(from_node)
                    .ok_or(PipeForgeError::InvalidArtifact(
                        "branch handle does not exist",
                    ))?;
            let mut next_node = self.next_node_id()?;
            let mut next_edge = self.next_edge_id()?;
            let mut added = Vec::with_capacity(points.len());
            for &world in points {
                if !finite3(world) {
                    return Err(PipeForgeError::NonFinitePoint);
                }
                let mut position = snap_grid(world, self.grid_m.max(0.001));
                position = snap_direction(last_position, position, self.grid_m.max(0.001));
                if distance(last_position, position) < MIN_SEGMENT_M {
                    return Err(PipeForgeError::SegmentTooShort);
                }
                self.graph.nodes.push(PipeRouteNode {
                    id: next_node,
                    position,
                    primary_index: None,
                });
                self.graph.edges.push(PipeRouteEdge {
                    id: next_edge,
                    from: last_node,
                    to: next_node,
                    diameter_m: diameter,
                });
                added.push(next_node);
                last_node = next_node;
                last_position = position;
                next_node = next_node
                    .checked_add(1)
                    .ok_or(PipeForgeError::InvalidArtifact("route node ID overflow"))?;
                next_edge = next_edge
                    .checked_add(1)
                    .ok_or(PipeForgeError::InvalidArtifact("route edge ID overflow"))?;
            }
            self.refresh_inferred_fittings()?;
            self.validate(false)?;
            Ok(added)
        })();
        if result.is_err() {
            *self = original;
        }
        result
    }

    /// Move a stable route handle and keep the compatibility primary-point list in sync. This is the
    /// post-bake route-edit operation used after decoding an immutable artifact back to its recipe.
    pub fn move_route_handle(
        &mut self,
        node_id: u32,
        world: [f32; 3],
    ) -> Result<(), PipeForgeError> {
        if !finite3(world) || world.iter().any(|value| value.abs() > 10_000.0) {
            return Err(PipeForgeError::NonFinitePoint);
        }
        self.materialize_graph()?;
        let node_index = self
            .graph
            .nodes
            .iter()
            .position(|node| node.id == node_id)
            .ok_or(PipeForgeError::InvalidArtifact(
                "route handle does not exist",
            ))?;
        let old_position = self.graph.nodes[node_index].position;
        let old_fittings = self.fittings.clone();
        let primary_index = self.graph.nodes[node_index]
            .primary_index
            .and_then(|index| usize::try_from(index).ok())
            .filter(|&index| index < self.points.len());
        self.graph.nodes[node_index].position = world;
        if let Some(index) = primary_index {
            let point = self
                .points
                .get_mut(index)
                .ok_or(PipeForgeError::InvalidArtifact(
                    "primary route handle is out of sync",
                ))?;
            *point = world;
        }
        if let Err(error) = self
            .refresh_inferred_fittings()
            .and_then(|()| self.validate(false))
        {
            self.graph.nodes[node_index].position = old_position;
            self.fittings = old_fittings;
            if let Some(index) = primary_index {
                self.points[index] = old_position;
            }
            return Err(error);
        }
        Ok(())
    }

    /// Remove a leaf handle (the end of a branch or the latest primary point) without destabilizing
    /// the IDs of the remaining network. Interior/junction handles must be rerouted first.
    pub fn remove_route_handle(&mut self, node_id: u32) -> Result<(), PipeForgeError> {
        self.materialize_graph()?;
        let original = self.clone();
        let result = (|| {
            let node = self
                .graph
                .nodes
                .iter()
                .find(|node| node.id == node_id)
                .ok_or(PipeForgeError::InvalidArtifact(
                    "route handle does not exist",
                ))?;
            let connected = self
                .graph
                .edges
                .iter()
                .filter(|edge| edge.from == node_id || edge.to == node_id)
                .count();
            if connected > 1 {
                return Err(PipeForgeError::InvalidArtifact(
                    "only a leaf route handle can be removed",
                ));
            }
            if let Some(primary_index) = node
                .primary_index
                .and_then(|index| usize::try_from(index).ok())
            {
                if primary_index + 1 != self.points.len() {
                    return Err(PipeForgeError::InvalidArtifact(
                        "only the last primary route handle can be removed",
                    ));
                }
                self.points.pop();
            }
            self.graph
                .edges
                .retain(|edge| edge.from != node_id && edge.to != node_id);
            self.graph.nodes.retain(|node| node.id != node_id);
            self.fittings.retain(|fitting| fitting.node_id != node_id);
            self.refresh_inferred_fittings()?;
            self.validate(false)
        })();
        if result.is_err() {
            *self = original;
        }
        result
    }

    /// Attach a semantic or catalog-backed fitting to a route node.
    pub fn place_fitting(
        &mut self,
        node_id: u32,
        kind: PipeFittingKind,
        catalog_id: Option<String>,
    ) -> Result<u32, PipeForgeError> {
        self.materialize_graph()?;
        if self.node_position(node_id).is_none() {
            return Err(PipeForgeError::InvalidArtifact(
                "fitting handle does not exist",
            ));
        }
        if let Some(catalog_id) = catalog_id.as_deref() {
            let entry = self
                .fitting_catalog
                .iter()
                .find(|entry| entry.id == catalog_id)
                .ok_or(PipeForgeError::InvalidArtifact(
                    "fitting catalog entry does not exist",
                ))?;
            if entry.kind != kind {
                return Err(PipeForgeError::InvalidArtifact(
                    "catalog fitting kind does not match placement",
                ));
            }
        }
        let connected = self
            .graph
            .edges
            .iter()
            .filter(|edge| edge.from == node_id || edge.to == node_id)
            .count();
        match kind {
            PipeFittingKind::Tee if connected < 3 => {
                return Err(PipeForgeError::InvalidArtifact(
                    "a tee requires a route junction with at least three ports",
                ));
            }
            PipeFittingKind::Elbow | PipeFittingKind::Coupling if connected != 2 => {
                return Err(PipeForgeError::InvalidArtifact(
                    "an elbow or coupling requires exactly two route ports",
                ));
            }
            _ => {}
        }
        if let Some(existing) = self
            .fittings
            .iter_mut()
            .find(|fitting| fitting.node_id == node_id && fitting.kind == kind)
        {
            if existing.automatic {
                existing.automatic = false;
                existing.catalog_id = catalog_id;
                return Ok(existing.id);
            }
            return Err(PipeForgeError::InvalidArtifact(
                "this semantic fitting already exists on the route handle",
            ));
        }
        if self.fittings.len() >= MAX_FITTINGS {
            return Err(PipeForgeError::InvalidArtifact("too many pipe fittings"));
        }
        let id = self.next_fitting_id()?;
        self.fittings.push(PipeFittingPlacement {
            id,
            node_id,
            kind,
            catalog_id,
            automatic: false,
        });
        Ok(id)
    }

    /// Register or replace a bounded user fitting catalog definition.
    pub fn upsert_catalog_entry(
        &mut self,
        entry: UserFittingCatalogEntry,
    ) -> Result<(), PipeForgeError> {
        validate_catalog_entry(&entry)?;
        if self.fittings.iter().any(|fitting| {
            fitting.catalog_id.as_deref() == Some(entry.id.as_str()) && fitting.kind != entry.kind
        }) {
            return Err(PipeForgeError::InvalidArtifact(
                "a placed fitting uses this catalog ID with a different kind",
            ));
        }
        if let Some(existing) = self
            .fitting_catalog
            .iter_mut()
            .find(|existing| existing.id == entry.id)
        {
            *existing = entry;
        } else {
            if self.fitting_catalog.len() >= MAX_CATALOG_ENTRIES {
                return Err(PipeForgeError::InvalidArtifact(
                    "fitting catalog exceeds the safe entry limit",
                ));
            }
            self.fitting_catalog.push(entry);
            self.fitting_catalog.sort_by(|a, b| a.id.cmp(&b.id));
        }
        Ok(())
    }

    /// Remove one explicit semantic fitting. Automatically inferred elbows/tees are controlled by
    /// `auto_fittings`, so pretending they can be deleted individually would make them reappear on bake.
    pub fn remove_fitting(&mut self, fitting_id: u32) -> Result<(), PipeForgeError> {
        let fitting = self
            .fittings
            .iter()
            .find(|fitting| fitting.id == fitting_id)
            .ok_or(PipeForgeError::InvalidArtifact(
                "pipe fitting does not exist",
            ))?;
        if fitting.automatic {
            return Err(PipeForgeError::InvalidArtifact(
                "automatic fittings are controlled by Auto fittings",
            ));
        }
        self.fittings.retain(|fitting| fitting.id != fitting_id);
        Ok(())
    }

    /// Remove a project catalog entry only when no placement still references it. This explicit failure
    /// avoids silently replacing a user's authored fitting with proxy geometry.
    pub fn remove_catalog_entry(&mut self, id: &str) -> Result<(), PipeForgeError> {
        if !self.fitting_catalog.iter().any(|entry| entry.id == id) {
            return Err(PipeForgeError::InvalidArtifact(
                "fitting catalog entry does not exist",
            ));
        }
        if self
            .fittings
            .iter()
            .any(|fitting| fitting.catalog_id.as_deref() == Some(id))
        {
            return Err(PipeForgeError::InvalidArtifact(
                "remove placed catalog fittings before deleting this entry",
            ));
        }
        self.fitting_catalog.retain(|entry| entry.id != id);
        Ok(())
    }

    /// Resolve an edge into its two stable route handles for native viewport preview.
    #[must_use]
    pub fn route_segments(&self) -> Vec<[[f32; 3]; 2]> {
        let positions: BTreeMap<_, _> = self
            .graph
            .nodes
            .iter()
            .map(|node| (node.id, node.position))
            .collect();
        let mut edges = self.graph.edges.clone();
        edges.sort_by_key(|edge| edge.id);
        edges
            .into_iter()
            .filter_map(|edge| Some([*positions.get(&edge.from)?, *positions.get(&edge.to)?]))
            .collect()
    }

    /// Stable, deterministic handle read-model for the viewport.
    #[must_use]
    pub fn route_handles(&self) -> Vec<PipeRouteHandle> {
        let mut nodes = self.graph.nodes.clone();
        nodes.sort_by_key(|node| node.id);
        nodes
            .into_iter()
            .map(|node| {
                let mut connected_edges: Vec<_> = self
                    .graph
                    .edges
                    .iter()
                    .filter(|edge| edge.from == node.id || edge.to == node.id)
                    .map(|edge| edge.id)
                    .collect();
                connected_edges.sort_unstable();
                let mut fitting_ids: Vec<_> = self
                    .fittings
                    .iter()
                    .filter(|fitting| fitting.node_id == node.id)
                    .map(|fitting| fitting.id)
                    .collect();
                fitting_ids.sort_unstable();
                PipeRouteHandle {
                    node_id: node.id,
                    position: node.position,
                    connected_edges,
                    fitting_ids,
                }
            })
            .collect()
    }

    /// Total routed centerline length, before corner filleting.
    #[must_use]
    pub fn length_m(&self) -> f32 {
        if self.graph.edges.is_empty() {
            return self.points.windows(2).map(|w| distance(w[0], w[1])).sum();
        }
        let positions: BTreeMap<_, _> = self
            .graph
            .nodes
            .iter()
            .map(|node| (node.id, node.position))
            .collect();
        self.graph
            .edges
            .iter()
            .filter_map(|edge| {
                Some(distance(
                    *positions.get(&edge.from)?,
                    *positions.get(&edge.to)?,
                ))
            })
            .sum()
    }

    /// Validate work budgets and all values before any mesh allocation.
    pub fn validate(&self, require_bakeable: bool) -> Result<(), PipeForgeError> {
        if self.version != PIPE_RECIPE_VERSION {
            return Err(PipeForgeError::UnsupportedVersion(self.version));
        }
        if self.points.len() > MAX_POINTS
            || self.graph.nodes.len() > MAX_ROUTE_NODES
            || self.graph.edges.len() > MAX_ROUTE_EDGES
        {
            return Err(PipeForgeError::TooManyPoints);
        }
        validate_diameter(self.diameter_m)?;
        if !(0.001..=10.0).contains(&self.grid_m) || !self.grid_m.is_finite() {
            return Err(PipeForgeError::BadGrid);
        }
        if !(0.005..=20.0).contains(&self.bend_radius_m)
            || !self.bend_radius_m.is_finite()
            || self.bend_radius_m < self.diameter_m * 0.5
        {
            return Err(PipeForgeError::BadBendRadius);
        }
        self.validate_graph(require_bakeable)
    }

    #[allow(clippy::too_many_lines)] // One ordered gate keeps graph cross-reference failures transactional.
    fn validate_graph(&self, require_bakeable: bool) -> Result<(), PipeForgeError> {
        if self.graph.nodes.is_empty() && self.graph.edges.is_empty() {
            return validate_polyline(
                &self.points,
                self.diameter_m,
                self.auto_fittings,
                require_bakeable,
            );
        }
        if self.graph.nodes.is_empty() || (require_bakeable && self.graph.edges.is_empty()) {
            return Err(PipeForgeError::NeedTwoPoints);
        }

        let mut positions = BTreeMap::new();
        let mut primary = BTreeMap::new();
        for node in &self.graph.nodes {
            if positions.insert(node.id, node.position).is_some() {
                return Err(PipeForgeError::InvalidArtifact("duplicate route node ID"));
            }
            if !finite3(node.position) || node.position.iter().any(|value| value.abs() > 10_000.0) {
                return Err(PipeForgeError::NonFinitePoint);
            }
            if let Some(index) = node.primary_index {
                if primary.insert(index, node).is_some() {
                    return Err(PipeForgeError::InvalidArtifact(
                        "duplicate primary route point index",
                    ));
                }
            }
        }
        if !self.points.is_empty() {
            if primary.len() != self.points.len() {
                return Err(PipeForgeError::InvalidArtifact(
                    "primary route and graph are out of sync",
                ));
            }
            for (index, &point) in self.points.iter().enumerate() {
                let index = u32::try_from(index)
                    .map_err(|_| PipeForgeError::InvalidArtifact("primary route index overflow"))?;
                let node = primary.get(&index).ok_or(PipeForgeError::InvalidArtifact(
                    "primary route and graph are out of sync",
                ))?;
                if distance(point, node.position) > WELD_EPSILON_M {
                    return Err(PipeForgeError::InvalidArtifact(
                        "primary route and graph are out of sync",
                    ));
                }
            }
        }

        let mut edge_ids = BTreeSet::new();
        let mut adjacency = BTreeMap::<u32, Vec<(u32, u32)>>::new();
        for edge in &self.graph.edges {
            if !edge_ids.insert(edge.id) {
                return Err(PipeForgeError::InvalidArtifact("duplicate route edge ID"));
            }
            validate_diameter(edge.diameter_m)?;
            let from = positions
                .get(&edge.from)
                .ok_or(PipeForgeError::InvalidArtifact(
                    "route edge references a missing node",
                ))?;
            let to = positions
                .get(&edge.to)
                .ok_or(PipeForgeError::InvalidArtifact(
                    "route edge references a missing node",
                ))?;
            if edge.from == edge.to || distance(*from, *to) < MIN_SEGMENT_M {
                return Err(PipeForgeError::SegmentTooShort);
            }
            adjacency
                .entry(edge.from)
                .or_default()
                .push((edge.to, edge.id));
            adjacency
                .entry(edge.to)
                .or_default()
                .push((edge.from, edge.id));
        }

        // Every cooked graph is one editable network. This catches orphan nodes and accidentally
        // pasted branch islands before geometry work is allocated.
        if let Some((&first, _)) = positions.first_key_value() {
            let mut reached = BTreeSet::new();
            let mut queue = VecDeque::from([first]);
            while let Some(node) = queue.pop_front() {
                if !reached.insert(node) {
                    continue;
                }
                for &(next, _) in adjacency
                    .get(&node)
                    .map_or(&[] as &[(u32, u32)], Vec::as_slice)
                {
                    queue.push_back(next);
                }
            }
            if reached.len() != positions.len() {
                return Err(PipeForgeError::InvalidArtifact(
                    "route graph is disconnected",
                ));
            }
        }

        // Two ports leaving a joint in effectively the same direction represent a doubled-back run.
        for (&node_id, neighbours) in &adjacency {
            let origin = positions[&node_id];
            for (left_index, &(left, _)) in neighbours.iter().enumerate() {
                for &(right, _) in &neighbours[left_index + 1..] {
                    let a = normalize(sub(positions[&left], origin));
                    let b = normalize(sub(positions[&right], origin));
                    if dot(a, b) > 0.98 {
                        return Err(PipeForgeError::InvalidArtifact(
                            "route doubles back on itself at a joint",
                        ));
                    }
                }
            }
        }

        // Non-connected graph edges must maintain their physical outside-diameter clearance.
        for (index, left) in self.graph.edges.iter().enumerate() {
            for right in &self.graph.edges[index + 1..] {
                if left.from == right.from
                    || left.from == right.to
                    || left.to == right.from
                    || left.to == right.to
                {
                    continue;
                }
                let clearance = left.diameter_m.max(right.diameter_m)
                    * if self.auto_fittings { 1.18 } else { 1.0 }
                    * 0.98;
                if segment_segment_distance(
                    positions[&left.from],
                    positions[&left.to],
                    positions[&right.from],
                    positions[&right.to],
                ) < clearance
                {
                    return Err(PipeForgeError::InvalidArtifact(
                        "route intersects itself or passes too close to itself",
                    ));
                }
            }
        }

        if self.fittings.len() > MAX_FITTINGS || self.fitting_catalog.len() > MAX_CATALOG_ENTRIES {
            return Err(PipeForgeError::InvalidArtifact(
                "pipe fitting metadata exceeds the safe budget",
            ));
        }
        let mut catalog_ids = BTreeSet::new();
        for entry in &self.fitting_catalog {
            validate_catalog_entry(entry)?;
            if !catalog_ids.insert(entry.id.as_str()) {
                return Err(PipeForgeError::InvalidArtifact(
                    "duplicate fitting catalog entry ID",
                ));
            }
        }
        let mut fitting_ids = BTreeSet::new();
        for fitting in &self.fittings {
            if !fitting_ids.insert(fitting.id) {
                return Err(PipeForgeError::InvalidArtifact("duplicate pipe fitting ID"));
            }
            if !positions.contains_key(&fitting.node_id) {
                return Err(PipeForgeError::InvalidArtifact(
                    "fitting references a missing route node",
                ));
            }
            if let Some(catalog_id) = fitting.catalog_id.as_deref() {
                let entry = self
                    .fitting_catalog
                    .iter()
                    .find(|entry| entry.id == catalog_id)
                    .ok_or(PipeForgeError::InvalidArtifact(
                        "fitting references a missing catalog entry",
                    ))?;
                if entry.kind != fitting.kind {
                    return Err(PipeForgeError::InvalidArtifact(
                        "catalog fitting kind does not match placement",
                    ));
                }
            }
        }
        Ok(())
    }

    fn upgraded(&self) -> Result<Self, PipeForgeError> {
        if !matches!(
            self.version,
            LEGACY_PIPE_RECIPE_VERSION | PIPE_RECIPE_VERSION
        ) {
            return Err(PipeForgeError::UnsupportedVersion(self.version));
        }
        let mut upgraded = self.clone();
        upgraded.version = PIPE_RECIPE_VERSION;
        upgraded.materialize_graph()?;
        upgraded.refresh_inferred_fittings()?;
        upgraded.graph.nodes.sort_by_key(|node| node.id);
        upgraded.graph.edges.sort_by_key(|edge| edge.id);
        upgraded.fittings.sort_by_key(|fitting| fitting.id);
        upgraded
            .fitting_catalog
            .sort_by(|left, right| left.id.cmp(&right.id));
        upgraded.validate(false)?;
        Ok(upgraded)
    }

    fn materialize_graph(&mut self) -> Result<(), PipeForgeError> {
        if !self.graph.nodes.is_empty() || !self.graph.edges.is_empty() {
            return Ok(());
        }
        if self.points.len() > MAX_POINTS {
            return Err(PipeForgeError::TooManyPoints);
        }
        for (index, &position) in self.points.iter().enumerate() {
            let id = u32::try_from(index + 1)
                .map_err(|_| PipeForgeError::InvalidArtifact("route node ID overflow"))?;
            let primary_index = u32::try_from(index)
                .map_err(|_| PipeForgeError::InvalidArtifact("primary route index overflow"))?;
            self.graph.nodes.push(PipeRouteNode {
                id,
                position,
                primary_index: Some(primary_index),
            });
            if index > 0 {
                self.graph.edges.push(PipeRouteEdge {
                    id: u32::try_from(index)
                        .map_err(|_| PipeForgeError::InvalidArtifact("route edge ID overflow"))?,
                    from: id - 1,
                    to: id,
                    diameter_m: self.diameter_m,
                });
            }
        }
        Ok(())
    }

    fn refresh_inferred_fittings(&mut self) -> Result<(), PipeForgeError> {
        let positions: BTreeMap<_, _> = self
            .graph
            .nodes
            .iter()
            .map(|node| (node.id, node.position))
            .collect();
        let mut adjacency = BTreeMap::<u32, Vec<u32>>::new();
        for edge in &self.graph.edges {
            adjacency.entry(edge.from).or_default().push(edge.to);
            adjacency.entry(edge.to).or_default().push(edge.from);
        }
        let mut desired = Vec::new();
        if self.auto_fittings {
            for (&node_id, neighbours) in &adjacency {
                let kind = if neighbours.len() >= 3 {
                    Some(PipeFittingKind::Tee)
                } else if neighbours.len() == 2 {
                    let origin = positions[&node_id];
                    let left = normalize(sub(positions[&neighbours[0]], origin));
                    let right = normalize(sub(positions[&neighbours[1]], origin));
                    (dot(left, right) > -0.995).then_some(PipeFittingKind::Elbow)
                } else {
                    None
                };
                if let Some(kind) = kind {
                    desired.push((node_id, kind));
                }
            }
        }
        desired.sort_unstable();
        self.fittings.retain(|fitting| {
            !fitting.automatic || desired.contains(&(fitting.node_id, fitting.kind))
        });
        for (node_id, kind) in desired {
            if self
                .fittings
                .iter()
                .any(|fitting| fitting.node_id == node_id && fitting.kind == kind)
            {
                continue;
            }
            if self.fittings.len() >= MAX_FITTINGS {
                return Err(PipeForgeError::InvalidArtifact("too many pipe fittings"));
            }
            let id = self.next_fitting_id()?;
            self.fittings.push(PipeFittingPlacement {
                id,
                node_id,
                kind,
                catalog_id: None,
                automatic: true,
            });
        }
        self.fittings.sort_by_key(|fitting| fitting.id);
        Ok(())
    }

    fn primary_node_id(&self, index: usize) -> Option<u32> {
        let index = u32::try_from(index).ok()?;
        self.graph
            .nodes
            .iter()
            .find(|node| node.primary_index == Some(index))
            .map(|node| node.id)
    }

    fn node_position(&self, node_id: u32) -> Option<[f32; 3]> {
        self.graph
            .nodes
            .iter()
            .find(|node| node.id == node_id)
            .map(|node| node.position)
    }

    fn next_node_id(&self) -> Result<u32, PipeForgeError> {
        next_stable_id(
            self.graph.nodes.iter().map(|node| node.id),
            "route node ID overflow",
        )
    }

    fn next_edge_id(&self) -> Result<u32, PipeForgeError> {
        next_stable_id(
            self.graph.edges.iter().map(|edge| edge.id),
            "route edge ID overflow",
        )
    }

    fn next_fitting_id(&self) -> Result<u32, PipeForgeError> {
        next_stable_id(
            self.fittings.iter().map(|fitting| fitting.id),
            "pipe fitting ID overflow",
        )
    }

    fn localized(&self) -> (Self, [f32; 3]) {
        let anchor = self
            .points
            .first()
            .copied()
            .or_else(|| {
                self.graph
                    .nodes
                    .iter()
                    .min_by_key(|node| node.id)
                    .map(|node| node.position)
            })
            .unwrap_or([0.0; 3]);
        let mut local = self.clone();
        for p in &mut local.points {
            *p = sub(*p, anchor);
        }
        for node in &mut local.graph.nodes {
            node.position = sub(node.position, anchor);
        }
        (local, anchor)
    }
}

fn validate_diameter(diameter_m: f32) -> Result<(), PipeForgeError> {
    if !(0.01..=2.0).contains(&diameter_m) || !diameter_m.is_finite() {
        return Err(PipeForgeError::BadDiameter);
    }
    Ok(())
}

fn validate_catalog_entry(entry: &UserFittingCatalogEntry) -> Result<(), PipeForgeError> {
    if entry.id.is_empty()
        || entry.id.len() > 96
        || entry.label.is_empty()
        || entry.label.len() > 160
        || entry
            .asset_handle
            .as_ref()
            .is_some_and(|handle| handle.is_empty() || handle.len() > 256)
        || !entry.diameter_scale.is_finite()
        || !(0.5..=4.0).contains(&entry.diameter_scale)
        || !entry.length_scale.is_finite()
        || !(0.25..=8.0).contains(&entry.length_scale)
    {
        return Err(PipeForgeError::InvalidArtifact(
            "fitting catalog entry is outside safe bounds",
        ));
    }
    Ok(())
}

fn next_stable_id(
    values: impl Iterator<Item = u32>,
    overflow_message: &'static str,
) -> Result<u32, PipeForgeError> {
    values
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or(PipeForgeError::InvalidArtifact(overflow_message))
}

fn validate_polyline(
    points: &[[f32; 3]],
    diameter_m: f32,
    auto_fittings: bool,
    require_bakeable: bool,
) -> Result<(), PipeForgeError> {
    if require_bakeable && points.len() < 2 {
        return Err(PipeForgeError::NeedTwoPoints);
    }
    if points
        .iter()
        .any(|point| !finite3(*point) || point.iter().any(|value| value.abs() > 10_000.0))
    {
        return Err(PipeForgeError::NonFinitePoint);
    }
    if points
        .windows(2)
        .any(|segment| distance(segment[0], segment[1]) < MIN_SEGMENT_M)
    {
        return Err(PipeForgeError::SegmentTooShort);
    }
    if points.windows(3).any(|points| {
        dot(
            normalize(sub(points[1], points[0])),
            normalize(sub(points[2], points[1])),
        ) < -0.98
    }) {
        return Err(PipeForgeError::InvalidArtifact(
            "route doubles back on itself at a joint",
        ));
    }
    let clearance = diameter_m * if auto_fittings { 1.18 } else { 1.0 } * 0.98;
    for left in 0..points.len().saturating_sub(1) {
        for right in left + 2..points.len().saturating_sub(1) {
            if segment_segment_distance(
                points[left],
                points[left + 1],
                points[right],
                points[right + 1],
            ) < clearance
            {
                return Err(PipeForgeError::InvalidArtifact(
                    "route intersects itself or passes too close to itself",
                ));
            }
        }
    }
    Ok(())
}

/// Live tool read-model used by React. Preview numbers are computed from the actual current recipe.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipeForgeStatus {
    pub active: bool,
    pub points: usize,
    pub length_m: f32,
    pub preview_triangles: usize,
    pub can_bake: bool,
    pub message: String,
    /// Authoritative controls for the currently active native session. Keeping these in the status
    /// prevents a panel remount from presenting defaults that differ from the recipe being edited.
    pub kit: PipeKit,
    pub diameter_cm: f32,
    pub quality: PipeQuality,
    pub auto_fittings: bool,
    /// World-space stable handles used by both the panel and direct viewport manipulation.
    pub handles: Vec<PipeRouteHandle>,
    /// Stable topology. Endpoint positions resolve through `handles`, avoiding duplicated mutable state.
    #[serde(rename = "edges")]
    pub graph_edges: Vec<PipeRouteEdge>,
    pub fittings: Vec<PipeFittingPlacement>,
    #[serde(rename = "fittingCatalog")]
    pub catalog: Vec<UserFittingCatalogEntry>,
    /// Current stable handle from which the next viewport click extends a branch.
    pub branch_from: Option<u32>,
    /// Existing scene identity when this session is a post-bake edit rather than a new asset.
    pub editing_entity: Option<String>,
}

impl Default for PipeForgeStatus {
    fn default() -> Self {
        let options = PipeForgeOptions::default();
        Self {
            active: false,
            points: 0,
            length_m: 0.0,
            preview_triangles: 0,
            can_bake: false,
            message: String::new(),
            kit: options.kit,
            diameter_cm: options.diameter_cm,
            quality: options.quality,
            auto_fittings: options.auto_fittings,
            handles: Vec::new(),
            graph_edges: Vec::new(),
            fittings: Vec::new(),
            catalog: Vec::new(),
            branch_from: None,
            editing_entity: None,
        }
    }
}

/// Honest result of finalization. LOD counts are the actual deterministic renderer LOD meshes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PipeBakeReport {
    pub entity_id: Option<String>,
    pub handle: Option<String>,
    pub vertices: usize,
    pub triangles: usize,
    pub lod_triangles: Vec<usize>,
    pub texture_resolution: u32,
    pub collision_hulls: usize,
    pub collision_kind: String,
    pub collision_triangles: usize,
    pub watertight: bool,
    pub warnings: Vec<String>,
    pub message: String,
}

/// Render-only editing session. Immutable cooked assets are produced only by [`bake_pipe`].
#[derive(Clone, Debug)]
pub struct PipeToolSession {
    pub recipe: PipeRecipe,
    pub message: String,
    /// Existing entity-space origin. New drawing sessions author directly in world space with zero here;
    /// restored recipes stay local and are projected back to world space through this anchor.
    pub anchor: [f32; 3],
    pub editing_entity: Option<String>,
    branch_from: Option<u32>,
    branch_diameter_m: Option<f32>,
    branch_path: Vec<u32>,
}

impl PipeToolSession {
    #[must_use]
    pub fn new(options: &PipeForgeOptions) -> Self {
        Self {
            recipe: PipeRecipe::from_options(options),
            message: "Click in the viewport to place the first point".into(),
            anchor: [0.0; 3],
            editing_entity: None,
            branch_from: None,
            branch_diameter_m: None,
            branch_path: Vec::new(),
        }
    }

    /// Restore an editable local recipe behind an existing scene entity.
    pub fn edit(
        recipe: &PipeRecipe,
        entity: String,
        anchor: [f32; 3],
    ) -> Result<Self, PipeForgeError> {
        let recipe = recipe.upgraded()?;
        recipe.validate(true)?;
        if !finite3(anchor) {
            return Err(PipeForgeError::NonFinitePoint);
        }
        Ok(Self {
            recipe,
            message: "Pipe restored — drag handles, add a branch or rebake in place".into(),
            anchor,
            editing_entity: Some(entity),
            branch_from: None,
            branch_diameter_m: None,
            branch_path: Vec::new(),
        })
    }

    #[must_use]
    pub fn status(&self) -> PipeForgeStatus {
        let can_bake = !self.recipe.graph.edges.is_empty() || self.recipe.points.len() >= 2;
        let preview_triangles = if can_bake {
            preview_triangle_count(&self.recipe)
        } else {
            0
        };
        let mut handles = self.recipe.route_handles();
        for handle in &mut handles {
            handle.position = add(handle.position, self.anchor);
        }
        let mut graph_edges = self.recipe.graph.edges.clone();
        graph_edges.sort_by_key(|edge| edge.id);
        let mut fittings = self.recipe.fittings.clone();
        fittings.sort_by_key(|fitting| fitting.id);
        let mut catalog = self.recipe.fitting_catalog.clone();
        catalog.sort_by(|left, right| left.id.cmp(&right.id));
        PipeForgeStatus {
            active: true,
            points: self.recipe.points.len(),
            length_m: self.recipe.length_m(),
            preview_triangles,
            can_bake,
            message: self.message.clone(),
            kit: self.recipe.kit,
            diameter_cm: self.recipe.diameter_m * 100.0,
            quality: self.recipe.quality,
            auto_fittings: self.recipe.auto_fittings,
            handles,
            graph_edges,
            fittings,
            catalog,
            branch_from: self.branch_from,
            editing_entity: self.editing_entity.clone(),
        }
    }

    pub fn point(&mut self, point: [f32; 3]) -> PipeForgeStatus {
        let local = sub(point, self.anchor);
        self.message = if let Some(from) = self.branch_from {
            match self
                .recipe
                .add_branch(from, &[local], self.branch_diameter_m)
            {
                Ok(nodes) => {
                    let tip = nodes[0];
                    self.branch_from = Some(tip);
                    self.branch_path.push(tip);
                    "Branch extended — keep drawing or end the branch".into()
                }
                Err(error) => error.to_string(),
            }
        } else {
            match self.recipe.add_point(local) {
                Ok(_) if self.recipe.points.len() == 1 => {
                    "First point placed — click to extend the run".into()
                }
                Ok(_) => "Run updated — keep drawing or bake the asset".into(),
                Err(e) => e.to_string(),
            }
        };
        self.status()
    }

    pub fn begin_branch(&mut self, node_id: u32, diameter_cm: f32) -> PipeForgeStatus {
        let result = (|| {
            self.recipe.materialize_graph()?;
            if self.recipe.node_position(node_id).is_none() {
                return Err(PipeForgeError::InvalidArtifact(
                    "branch handle does not exist",
                ));
            }
            let branch_width_m = diameter_cm * 0.01;
            validate_diameter(branch_width_m)?;
            self.branch_from = Some(node_id);
            self.branch_diameter_m = Some(branch_width_m);
            self.branch_path = vec![node_id];
            Ok(())
        })();
        self.message = result.map_or_else(
            |error| error.to_string(),
            |()| "Branch mode active — click the viewport to extend it".into(),
        );
        self.status()
    }

    pub fn end_branch(&mut self) -> PipeForgeStatus {
        self.branch_from = None;
        self.branch_diameter_m = None;
        self.branch_path.clear();
        self.message = "Branch finished — edit another handle or rebake".into();
        self.status()
    }

    pub fn move_handle(&mut self, node_id: u32, world: [f32; 3]) -> PipeForgeStatus {
        self.message = self
            .recipe
            .move_route_handle(node_id, sub(world, self.anchor))
            .map_or_else(
                |error| error.to_string(),
                |()| "Route handle moved — rebake to update the scene asset".into(),
            );
        self.status()
    }

    pub fn remove_handle(&mut self, node_id: u32) -> PipeForgeStatus {
        self.message = self.recipe.remove_route_handle(node_id).map_or_else(
            |error| error.to_string(),
            |()| {
                if self.branch_from == Some(node_id) {
                    self.branch_from = None;
                    self.branch_diameter_m = None;
                    self.branch_path.clear();
                }
                "Route handle removed — stable IDs on the remaining network were preserved".into()
            },
        );
        self.status()
    }

    pub fn place_fitting(
        &mut self,
        node_id: u32,
        kind: PipeFittingKind,
        catalog_id: Option<String>,
    ) -> PipeForgeStatus {
        self.message = self
            .recipe
            .place_fitting(node_id, kind, catalog_id)
            .map_or_else(
                |error| error.to_string(),
                |id| format!("Semantic fitting {id} placed — it will follow this handle"),
            );
        self.status()
    }

    pub fn remove_fitting(&mut self, fitting_id: u32) -> PipeForgeStatus {
        self.message = self.recipe.remove_fitting(fitting_id).map_or_else(
            |error| error.to_string(),
            |()| "Semantic fitting removed".into(),
        );
        self.status()
    }

    pub fn upsert_catalog(&mut self, entry: UserFittingCatalogEntry) -> PipeForgeStatus {
        self.message = self.recipe.upsert_catalog_entry(entry).map_or_else(
            |error| error.to_string(),
            |()| "Project fitting catalog updated".into(),
        );
        self.status()
    }

    pub fn remove_catalog(&mut self, id: &str) -> PipeForgeStatus {
        self.message = self.recipe.remove_catalog_entry(id).map_or_else(
            |error| error.to_string(),
            |()| "Project fitting catalog entry removed".into(),
        );
        self.status()
    }

    #[must_use]
    pub fn preview_points(&self) -> Vec<[f32; 3]> {
        self.recipe
            .points
            .iter()
            .map(|&point| add(point, self.anchor))
            .collect()
    }

    #[must_use]
    pub fn preview_handles(&self) -> Vec<[f32; 3]> {
        self.recipe
            .route_handles()
            .into_iter()
            .map(|handle| add(handle.position, self.anchor))
            .collect()
    }

    #[must_use]
    pub fn preview_edges(&self) -> Vec<[[f32; 3]; 2]> {
        self.recipe
            .route_segments()
            .into_iter()
            .map(|[from, to]| [add(from, self.anchor), add(to, self.anchor)])
            .collect()
    }

    pub fn undo_point(&mut self) -> PipeForgeStatus {
        if self.branch_path.len() > 1 {
            let node_id = self.branch_path.pop().expect("branch path is non-empty");
            let previous = *self.branch_path.last().expect("branch origin remains");
            self.message = self.recipe.remove_route_handle(node_id).map_or_else(
                |error| error.to_string(),
                |()| {
                    self.branch_from = Some(previous);
                    "Last branch handle removed".into()
                },
            );
            return self.status();
        }
        self.message = match self.recipe.points.len().checked_sub(1) {
            Some(index) => {
                let node_id = self.recipe.primary_node_id(index);
                let connected = node_id.map_or(0, |node_id| {
                    self.recipe
                        .graph
                        .edges
                        .iter()
                        .filter(|edge| edge.from == node_id || edge.to == node_id)
                        .count()
                });
                if connected > 1 {
                    "That route point has branches; move or remove those branch handles first"
                        .into()
                } else {
                    self.recipe.points.pop();
                    if let Some(node_id) = node_id {
                        self.recipe
                            .graph
                            .edges
                            .retain(|edge| edge.from != node_id && edge.to != node_id);
                        self.recipe.graph.nodes.retain(|node| node.id != node_id);
                        self.recipe
                            .fittings
                            .retain(|fitting| fitting.node_id != node_id);
                    }
                    "Last point removed".into()
                }
            }
            None => "No points to undo".into(),
        };
        self.status()
    }
}

fn preview_triangle_count(recipe: &PipeRecipe) -> usize {
    let radial = recipe.quality.radial_segments();
    let routes: usize = route_polylines(recipe)
        .into_iter()
        .map(|route| {
            let mut segment = recipe.clone();
            segment.points = route.points;
            segment.diameter_m = route.diameter_m;
            let (centers, _) = profiled_centerline(&segment);
            centers.len().saturating_sub(1) * radial * 2 + radial * 2
        })
        .sum();
    let fittings = recipe
        .fittings
        .iter()
        .map(|fitting| match fitting.kind {
            PipeFittingKind::Tee => {
                let ports = recipe
                    .graph
                    .edges
                    .iter()
                    .filter(|edge| edge.from == fitting.node_id || edge.to == fitting.node_id)
                    .count();
                ports * radial.max(6) * 4
            }
            PipeFittingKind::Valve => radial.max(6) * 4 + (radial.max(12) / 2).max(6) * 4,
            PipeFittingKind::Elbow | PipeFittingKind::Coupling | PipeFittingKind::Flange => {
                radial.max(6) * 4
            }
        })
        .sum::<usize>();
    routes.saturating_add(fittings)
}

/// Fully cooked result, ready to persist and then register/land.
#[derive(Debug)]
pub struct PipeBuild {
    pub recipe: PipeRecipe,
    pub anchor: [f32; 3],
    pub asset: MeshAsset,
    pub artifact_bytes: Vec<u8>,
    pub handle: String,
    pub report: PipeBakeReport,
}

/// Validation/build failures are always user-explainable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PipeForgeError {
    NeedTwoPoints,
    TooManyPoints,
    SegmentTooShort,
    NonFinitePoint,
    BadDiameter,
    BadGrid,
    BadBendRadius,
    UnsupportedVersion(u32),
    InvalidArtifact(&'static str),
    Encode(String),
}

impl fmt::Display for PipeForgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NeedTwoPoints => f.write_str("Place at least two points before baking"),
            Self::TooManyPoints => f.write_str("This run reached the 128-point safety limit"),
            Self::SegmentTooShort => f.write_str("That point is too close to the previous point"),
            Self::NonFinitePoint => {
                f.write_str("That viewport position is outside the usable work area")
            }
            Self::BadDiameter => f.write_str("Diameter must be between 1 cm and 200 cm"),
            Self::BadGrid => f.write_str("The snap grid is outside its safe range"),
            Self::BadBendRadius => f.write_str("The bend radius is outside its safe range"),
            Self::UnsupportedVersion(v) => write!(f, "Pipe recipe version {v} is not supported"),
            Self::InvalidArtifact(reason) => write!(f, "Pipe asset is invalid: {reason}"),
            Self::Encode(reason) => write!(f, "Could not encode the pipe asset: {reason}"),
        }
    }
}

impl std::error::Error for PipeForgeError {}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ArtifactTexture {
    width: u32,
    height: u32,
    rgba8: Vec<u8>,
}

/// Exact V1 recipe layout. Keeping this separate is what makes old `MTKPIPE1` blobs decodable after
/// the public recipe grows a graph and catalog fields.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct LegacyPipeRecipeV1 {
    version: u32,
    points: Vec<[f32; 3]>,
    diameter_m: f32,
    kit: PipeKit,
    quality: PipeQuality,
    auto_fittings: bool,
    grid_m: f32,
    bend_radius_m: f32,
}

impl LegacyPipeRecipeV1 {
    fn into_current(self) -> Result<PipeRecipe, PipeForgeError> {
        if self.version != LEGACY_PIPE_RECIPE_VERSION {
            return Err(PipeForgeError::UnsupportedVersion(self.version));
        }
        PipeRecipe {
            version: PIPE_RECIPE_VERSION,
            points: self.points,
            diameter_m: self.diameter_m,
            kit: self.kit,
            quality: self.quality,
            auto_fittings: self.auto_fittings,
            grid_m: self.grid_m,
            bend_radius_m: self.bend_radius_m,
            graph: PipeRouteGraph::default(),
            fittings: Vec::new(),
            fitting_catalog: Vec::new(),
        }
        .upgraded()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PipeArtifactV1 {
    version: u32,
    recipe: LegacyPipeRecipeV1,
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
    textures: Vec<ArtifactTexture>,
    metallic: f32,
    roughness: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ArtifactPrimitive {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    tangents: Vec<[f32; 4]>,
    indices: Vec<u32>,
    material: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PipeArtifactV2 {
    version: u32,
    recipe: PipeRecipe,
    primitives: Vec<ArtifactPrimitive>,
    textures: Vec<ArtifactTexture>,
    metallic: f32,
    roughness: f32,
}

/// Compile, validate and encode an immutable production asset. The handle hashes the exact artifact bytes.
pub fn bake_pipe(recipe: &PipeRecipe) -> Result<PipeBuild, PipeForgeError> {
    bake_pipe_impl(recipe, true)
}

/// Recompile an existing entity-local recipe without choosing a new first-point anchor. The caller keeps
/// the entity's authored `Transform` unchanged, so moving any route handle (including node one) has the
/// exact world-space result shown by the preview.
pub fn rebake_pipe_in_place(recipe: &PipeRecipe) -> Result<PipeBuild, PipeForgeError> {
    bake_pipe_impl(recipe, false)
}

fn bake_pipe_impl(
    recipe: &PipeRecipe,
    localize_to_first_handle: bool,
) -> Result<PipeBuild, PipeForgeError> {
    let upgraded = recipe.upgraded()?;
    upgraded.validate(true)?;
    let (local, anchor) = if localize_to_first_handle {
        upgraded.localized()
    } else {
        (upgraded, [0.0; 3])
    };
    let asset = mesh_from_recipe(&local)?;
    let watertight = !asset.primitives.is_empty() && asset.primitives.iter().all(is_watertight);
    if !watertight {
        return Err(PipeForgeError::InvalidArtifact(
            "compiled mesh is not watertight",
        ));
    }
    let gpu = MeshGpu::from_asset(&asset);
    let mut lod_triangles = vec![asset.triangle_count()];
    // Keep this identical to the native renderer's `m.lods(2)` upload path: report only levels that
    // actually exist at runtime, never an aspirational offline LOD.
    lod_triangles.extend(gpu.lods(2).iter().map(|lod| lod.index_count() / 3));
    let mut warnings = Vec::new();
    if lod_triangles.len() < 3 {
        warnings
            .push("This very small run only produced the LOD levels that reduced cleanly".into());
    }
    if local
        .fitting_catalog
        .iter()
        .any(|entry| entry.asset_handle.is_some())
    {
        warnings.push(
            "User fitting asset handles are preserved; this native bake uses their bounded semantic proxy geometry"
                .into(),
        );
    }
    let mat = &asset.materials[0];
    let artifact = PipeArtifactV2 {
        version: PIPE_RECIPE_VERSION,
        recipe: local.clone(),
        primitives: asset
            .primitives
            .iter()
            .map(|primitive| ArtifactPrimitive {
                positions: primitive.positions.clone(),
                normals: primitive.normals.clone(),
                uvs: primitive.uvs.clone(),
                tangents: primitive.tangents.clone(),
                indices: primitive.indices.clone(),
                material: primitive.material,
            })
            .collect(),
        textures: asset
            .textures
            .iter()
            .map(|t| ArtifactTexture {
                width: t.width,
                height: t.height,
                rgba8: t.rgba8.clone(),
            })
            .collect(),
        metallic: mat.metallic,
        roughness: mat.roughness,
    };
    let mut artifact_bytes = PIPE_ARTIFACT_MAGIC.to_vec();
    artifact_bytes
        .extend(bincode::serialize(&artifact).map_err(|e| PipeForgeError::Encode(e.to_string()))?);
    if artifact_bytes.len() > MAX_ARTIFACT_BYTES {
        return Err(PipeForgeError::InvalidArtifact(
            "compiled payload exceeds the safe size limit",
        ));
    }
    let handle = AssetId::of_bytes(&artifact_bytes).as_str().to_string();
    let report = PipeBakeReport {
        entity_id: None,
        handle: Some(handle.clone()),
        vertices: asset.vertex_count(),
        triangles: asset.triangle_count(),
        lod_triangles,
        texture_resolution: local.quality.texture_resolution(),
        collision_hulls: 0,
        collision_kind: "triangle mesh".into(),
        collision_triangles: asset.triangle_count(),
        watertight,
        warnings,
        message: format!(
            "{} pipe network baked with {} route handles and {} semantic fittings",
            local.kit.label(),
            local.graph.nodes.len(),
            local.fittings.len()
        ),
    };
    Ok(PipeBuild {
        recipe: local,
        anchor,
        asset,
        artifact_bytes,
        handle,
        report,
    })
}

/// Decode a trusted blob into its editable recipe + cooked mesh. `Ok(None)` means it is a different asset.
#[allow(clippy::too_many_lines)] // Migration and hostile-input validation share one ordered audit path.
pub fn decode_pipe_artifact(
    bytes: &[u8],
) -> Result<Option<(PipeRecipe, MeshAsset, PipeBakeReport)>, PipeForgeError> {
    if !bytes.starts_with(PIPE_ARTIFACT_MAGIC) {
        return Ok(None);
    }
    if bytes.len() > MAX_ARTIFACT_BYTES || bytes.len() == PIPE_ARTIFACT_MAGIC.len() {
        return Err(PipeForgeError::InvalidArtifact(
            "payload exceeds the safe size limit",
        ));
    }
    let payload = &bytes[PIPE_ARTIFACT_MAGIC.len()..];
    let version = payload
        .get(..4)
        .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
        .map(u32::from_le_bytes)
        .ok_or(PipeForgeError::InvalidArtifact("corrupt payload"))?;
    let (recipe, primitives, textures, metallic, roughness) = match version {
        LEGACY_PIPE_RECIPE_VERSION => {
            let artifact: PipeArtifactV1 = decode_artifact_payload(payload)?;
            let recipe = artifact.recipe.into_current()?;
            let primitive = ArtifactPrimitive {
                positions: artifact.positions,
                normals: artifact.normals,
                uvs: artifact.uvs,
                tangents: Vec::new(),
                indices: artifact.indices,
                material: 0,
            };
            (
                recipe,
                vec![primitive],
                artifact.textures,
                artifact.metallic,
                artifact.roughness,
            )
        }
        PIPE_RECIPE_VERSION => {
            let artifact: PipeArtifactV2 = decode_artifact_payload(payload)?;
            (
                artifact.recipe,
                artifact.primitives,
                artifact.textures,
                artifact.metallic,
                artifact.roughness,
            )
        }
        unsupported => return Err(PipeForgeError::UnsupportedVersion(unsupported)),
    };
    recipe.validate(true)?;
    validate_material_payload(&recipe, &textures, metallic, roughness)?;
    if primitives.is_empty() || primitives.len() > MAX_MESH_PRIMITIVES {
        return Err(PipeForgeError::InvalidArtifact(
            "artifact has no mesh primitives",
        ));
    }
    let total_vertices = primitives.iter().try_fold(0usize, |total, primitive| {
        validate_artifact_primitive(primitive)?;
        total
            .checked_add(primitive.positions.len())
            .ok_or(PipeForgeError::InvalidArtifact(
                "mesh vertex budget overflow",
            ))
    })?;
    let total_indices = primitives.iter().try_fold(0usize, |total, primitive| {
        total
            .checked_add(primitive.indices.len())
            .ok_or(PipeForgeError::InvalidArtifact(
                "mesh index budget overflow",
            ))
    })?;
    if total_vertices > MAX_MESH_VERTICES || total_indices > MAX_MESH_INDICES {
        return Err(PipeForgeError::InvalidArtifact(
            "compiled mesh exceeds the safe geometry budget",
        ));
    }
    let asset = MeshAsset {
        name: format!("{} pipe", recipe.kit.label()),
        primitives: primitives
            .into_iter()
            .map(|primitive| Primitive {
                positions: primitive.positions,
                normals: primitive.normals,
                uvs: primitive.uvs,
                tangents: primitive.tangents,
                indices: primitive.indices,
                material: primitive.material,
                ..Primitive::default()
            })
            .collect(),
        materials: vec![pipe_material(metallic, roughness)],
        textures: textures
            .into_iter()
            .map(|texture| Texture {
                width: texture.width,
                height: texture.height,
                rgba8: texture.rgba8,
            })
            .collect(),
        skeleton: None,
    };
    if !asset.primitives.iter().all(is_watertight) {
        return Err(PipeForgeError::InvalidArtifact("mesh is not watertight"));
    }
    let report = restored_report(bytes, &recipe, &asset);
    Ok(Some((recipe, asset, report)))
}

fn decode_artifact_payload<T: serde::de::DeserializeOwned>(
    payload: &[u8],
) -> Result<T, PipeForgeError> {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_limit(MAX_ARTIFACT_BYTES as u64)
        .reject_trailing_bytes()
        .deserialize(payload)
        .map_err(|_| PipeForgeError::InvalidArtifact("corrupt payload"))
}

fn validate_artifact_primitive(primitive: &ArtifactPrimitive) -> Result<(), PipeForgeError> {
    if primitive.positions.is_empty()
        || primitive.indices.is_empty()
        || primitive.positions.len() != primitive.normals.len()
        || primitive.positions.len() != primitive.uvs.len()
        || (!primitive.tangents.is_empty() && primitive.positions.len() != primitive.tangents.len())
        || !primitive.indices.len().is_multiple_of(3)
        || primitive
            .indices
            .iter()
            .any(|&index| index as usize >= primitive.positions.len())
        || primitive.material != 0
        || primitive
            .positions
            .iter()
            .any(|position| !finite3(*position))
        || primitive
            .positions
            .iter()
            .flatten()
            .any(|value| value.abs() > 20_010.0)
        || primitive
            .normals
            .iter()
            .any(|normal| !finite3(*normal) || !(0.5..=1.5).contains(&length(*normal)))
        || primitive
            .uvs
            .iter()
            .flatten()
            .any(|value| !value.is_finite() || value.abs() > 1_000_000.0)
        || primitive.tangents.iter().any(|tangent| {
            tangent.iter().any(|value| !value.is_finite())
                || !(0.5..=1.5).contains(&length([tangent[0], tangent[1], tangent[2]]))
                || !(0.5..=1.5).contains(&tangent[3].abs())
        })
    {
        return Err(PipeForgeError::InvalidArtifact("bad mesh buffers"));
    }
    Ok(())
}

fn validate_material_payload(
    recipe: &PipeRecipe,
    textures: &[ArtifactTexture],
    metallic: f32,
    roughness: f32,
) -> Result<(), PipeForgeError> {
    let expected = recipe.quality.texture_resolution();
    if !metallic.is_finite()
        || !roughness.is_finite()
        || !(0.0..=1.0).contains(&metallic)
        || !(0.0..=1.0).contains(&roughness)
        || textures.len() != 3
        || textures.iter().any(|texture| {
            texture.width == 0
                || texture.height == 0
                || texture.width != expected
                || texture.height != expected
                || texture
                    .width
                    .checked_mul(texture.height)
                    .and_then(|pixels| pixels.checked_mul(4))
                    .is_none_or(|bytes| texture.rgba8.len() != bytes as usize)
        })
    {
        return Err(PipeForgeError::InvalidArtifact("bad PBR texture set"));
    }
    Ok(())
}

fn pipe_material(metallic: f32, roughness: f32) -> Material {
    Material {
        base_color: [1.0; 4],
        metallic,
        roughness,
        base_color_texture: Some(0),
        metallic_roughness_texture: Some(1),
        normal_texture: Some(2),
        occlusion_texture: None,
        curvature_texture: None,
    }
}

fn restored_report(bytes: &[u8], recipe: &PipeRecipe, asset: &MeshAsset) -> PipeBakeReport {
    let gpu = MeshGpu::from_asset(asset);
    let mut lod_triangles = vec![asset.triangle_count()];
    lod_triangles.extend(gpu.lods(2).iter().map(|lod| lod.index_count() / 3));
    PipeBakeReport {
        handle: Some(AssetId::of_bytes(bytes).as_str().to_string()),
        vertices: asset.vertex_count(),
        triangles: asset.triangle_count(),
        lod_triangles,
        texture_resolution: recipe.quality.texture_resolution(),
        collision_hulls: 0,
        collision_kind: "triangle mesh".into(),
        collision_triangles: asset.triangle_count(),
        watertight: true,
        message: format!(
            "Pipe network restored with {} editable route handles",
            recipe.graph.nodes.len()
        ),
        ..PipeBakeReport::default()
    }
}

/// Land a persisted build as ONE undoable scene transaction: named renderable entity, editable source
/// recipe, exact local-to-world anchor, fixed rigid body and exact cooked triangle-mesh collider. The artifact
/// must already be durable before this is called; a failed commit therefore never leaves a dangling handle.
#[allow(clippy::too_many_lines)] // The scene transaction is assembled and committed atomically.
pub fn land_pipe_asset(
    engine: &mut Engine<FlecsWorld>,
    scene: &crate::capscene::CapScene,
    source: &PipeRecipe,
    handle: &str,
    anchor: [f32; 3],
    report: &PipeBakeReport,
) -> Result<EntityId, PipelineError> {
    let id = engine.alloc_entity_id();
    let recipe_json = serde_json::to_string(source).unwrap_or_default();
    let branch_edges = source
        .graph
        .edges
        .len()
        .saturating_sub(source.points.len().saturating_sub(1));
    let mut ops = vec![Op::CreateEntity { id, parent: None }];
    for (field, value) in [
        ("x", anchor[0]),
        ("y", anchor[1]),
        ("z", anchor[2]),
        ("scale", 1.0),
    ] {
        ops.push(Op::SetField {
            entity: id,
            component: "Transform".into(),
            field: field.into(),
            value: FieldValue::Number(f64::from(value)),
        });
    }
    for (component, field, value) in [
        (
            INSTANCE_META,
            crate::capscene::NAME_FIELD,
            FieldValue::Str(format!("{} pipe", source.kit.label())),
        ),
        (
            "MeshRenderer",
            crate::capscene::MESH_FIELD,
            FieldValue::Str(handle.into()),
        ),
        ("MeshRenderer", "castShadows", FieldValue::Bool(true)),
        ("PipeRecipe", "source", FieldValue::Str(recipe_json)),
        (
            "PipeRecipe",
            "version",
            FieldValue::Integer(i64::from(source.version)),
        ),
        (
            "PipeRecipe",
            "kit",
            FieldValue::Str(source.kit.label().into()),
        ),
        (
            "PipeRecipe",
            "diameterCm",
            FieldValue::Number(f64::from(source.diameter_m * 100.0)),
        ),
        (
            "PipeRecipe",
            "lengthM",
            FieldValue::Number(f64::from(source.length_m())),
        ),
        (
            "PipeRecipe",
            "routeHandles",
            FieldValue::Integer(i64::try_from(source.graph.nodes.len()).unwrap_or(i64::MAX)),
        ),
        (
            "PipeRecipe",
            "branchEdges",
            FieldValue::Integer(i64::try_from(branch_edges).unwrap_or(i64::MAX)),
        ),
        (
            "PipeRecipe",
            "fittings",
            FieldValue::Integer(i64::try_from(source.fittings.len()).unwrap_or(i64::MAX)),
        ),
        (
            "PipeRecipe",
            "triangles",
            FieldValue::Integer(i64::try_from(report.triangles).unwrap_or(i64::MAX)),
        ),
        (
            "PipeRecipe",
            "textureResolution",
            FieldValue::Integer(i64::from(report.texture_resolution)),
        ),
        ("RigidBody", "kind", FieldValue::Str("fixed".into())),
        ("Collider", "shape", FieldValue::Str("trimesh".into())),
        ("Collider", "friction", FieldValue::Number(0.65)),
        ("Collider", "restitution", FieldValue::Number(0.05)),
    ] {
        ops.push(Op::SetField {
            entity: id,
            component: component.into(),
            field: field.into(),
            value,
        });
    }
    for (cap, rel) in [
        ("Renderable", scene.rels.provides),
        ("Physics", scene.rels.provides),
        ("Collision", scene.rels.provides),
    ] {
        if let Some(&target) = scene.caps.get(&canonical(cap)) {
            ops.push(Op::AddPair {
                entity: id,
                rel,
                target,
            });
        }
    }
    engine.commit("bake-pipe-asset", ops)?;
    Ok(id)
}

/// Replace the cooked payload and editable source of an existing pipe in one undoable document commit.
/// Deliberately no `Transform` operation is emitted: the world anchor, hierarchy and scene identity remain
/// exactly as authored, while undo restores the prior handle/source pair atomically.
pub fn replace_pipe_asset(
    engine: &mut Engine<FlecsWorld>,
    id: EntityId,
    source: &PipeRecipe,
    handle: &str,
    report: &PipeBakeReport,
) -> Result<(), PipelineError> {
    let recipe_json = serde_json::to_string(source).unwrap_or_default();
    let branch_edges = source
        .graph
        .edges
        .len()
        .saturating_sub(source.points.len().saturating_sub(1));
    let values = [
        (
            "MeshRenderer",
            crate::capscene::MESH_FIELD,
            FieldValue::Str(handle.into()),
        ),
        ("PipeRecipe", "source", FieldValue::Str(recipe_json)),
        (
            "PipeRecipe",
            "version",
            FieldValue::Integer(i64::from(source.version)),
        ),
        (
            "PipeRecipe",
            "kit",
            FieldValue::Str(source.kit.label().into()),
        ),
        (
            "PipeRecipe",
            "diameterCm",
            FieldValue::Number(f64::from(source.diameter_m * 100.0)),
        ),
        (
            "PipeRecipe",
            "lengthM",
            FieldValue::Number(f64::from(source.length_m())),
        ),
        (
            "PipeRecipe",
            "routeHandles",
            FieldValue::Integer(i64::try_from(source.graph.nodes.len()).unwrap_or(i64::MAX)),
        ),
        (
            "PipeRecipe",
            "branchEdges",
            FieldValue::Integer(i64::try_from(branch_edges).unwrap_or(i64::MAX)),
        ),
        (
            "PipeRecipe",
            "fittings",
            FieldValue::Integer(i64::try_from(source.fittings.len()).unwrap_or(i64::MAX)),
        ),
        (
            "PipeRecipe",
            "triangles",
            FieldValue::Integer(i64::try_from(report.triangles).unwrap_or(i64::MAX)),
        ),
        (
            "PipeRecipe",
            "textureResolution",
            FieldValue::Integer(i64::from(report.texture_resolution)),
        ),
    ];
    let ops = values
        .into_iter()
        .map(|(component, field, value)| Op::SetField {
            entity: id,
            component: component.into(),
            field: field.into(),
            value,
        })
        .collect();
    engine.commit("rebake-pipe-asset", ops)?;
    Ok(())
}

fn mesh_from_recipe(recipe: &PipeRecipe) -> Result<MeshAsset, PipeForgeError> {
    recipe.validate(true)?;
    let radial = recipe.quality.radial_segments();
    let mut primitives = Vec::new();
    for route in route_polylines(recipe) {
        let mut route_recipe = recipe.clone();
        route_recipe.points = route.points;
        route_recipe.diameter_m = route.diameter_m;
        route_recipe.graph = PipeRouteGraph::default();
        route_recipe.fittings.clear();
        let (centers, radii) = profiled_centerline(&route_recipe);
        let mut primitive = Primitive::default();
        append_sweep(&mut primitive, &centers, &radii, radial);
        primitive.material = 0;
        primitives.push(primitive);
    }
    for fitting in &recipe.fittings {
        append_fitting_geometry(&mut primitives, recipe, fitting, radial)?;
    }
    let vertices = primitives.iter().try_fold(0usize, |total, primitive| {
        total
            .checked_add(primitive.positions.len())
            .ok_or(PipeForgeError::InvalidArtifact(
                "mesh vertex budget overflow",
            ))
    })?;
    let indices = primitives.iter().try_fold(0usize, |total, primitive| {
        total
            .checked_add(primitive.indices.len())
            .ok_or(PipeForgeError::InvalidArtifact(
                "mesh index budget overflow",
            ))
    })?;
    if primitives.is_empty()
        || primitives.len() > MAX_MESH_PRIMITIVES
        || vertices > MAX_MESH_VERTICES
        || indices > MAX_MESH_INDICES
    {
        return Err(PipeForgeError::InvalidArtifact(
            "compiled mesh exceeds the safe geometry budget",
        ));
    }
    let res = recipe.quality.texture_resolution();
    let (base, mr, normal, metallic, roughness) = material_textures(recipe.kit, res);
    Ok(MeshAsset {
        name: format!("{} pipe", recipe.kit.label()),
        primitives,
        materials: vec![pipe_material(metallic, roughness)],
        textures: vec![base, mr, normal],
        skeleton: None,
    })
}

#[derive(Clone, Debug)]
struct RoutePolyline {
    points: Vec<[f32; 3]>,
    diameter_m: f32,
}

/// Deterministically decompose a graph into maximal same-diameter chains. Junctions terminate chains,
/// which keeps each output shell watertight; semantic tee geometry bridges the visual junction.
fn route_polylines(recipe: &PipeRecipe) -> Vec<RoutePolyline> {
    if recipe.graph.edges.is_empty() {
        return (recipe.points.len() >= 2)
            .then(|| RoutePolyline {
                points: recipe.points.clone(),
                diameter_m: recipe.diameter_m,
            })
            .into_iter()
            .collect();
    }
    let positions: BTreeMap<_, _> = recipe
        .graph
        .nodes
        .iter()
        .map(|node| (node.id, node.position))
        .collect();
    let edges: BTreeMap<_, _> = recipe
        .graph
        .edges
        .iter()
        .map(|edge| (edge.id, edge))
        .collect();
    let mut adjacency = BTreeMap::<u32, Vec<u32>>::new();
    for edge in &recipe.graph.edges {
        adjacency.entry(edge.from).or_default().push(edge.id);
        adjacency.entry(edge.to).or_default().push(edge.id);
    }
    for edge_ids in adjacency.values_mut() {
        edge_ids.sort_unstable();
    }
    let mut visited = BTreeSet::new();
    let mut routes = Vec::new();
    let starts: Vec<_> = adjacency
        .iter()
        .filter(|(_, edge_ids)| edge_ids.len() != 2)
        .map(|(&node_id, _)| node_id)
        .collect();
    for start in starts {
        for &first_edge in adjacency.get(&start).map_or(&[] as &[u32], Vec::as_slice) {
            if visited.contains(&first_edge) {
                continue;
            }
            routes.push(walk_route_chain(
                start,
                first_edge,
                &positions,
                &edges,
                &adjacency,
                &mut visited,
            ));
        }
    }
    // Closed circuits have no endpoint/junction start. Compile their edges as bounded independent
    // shells instead of silently dropping them or introducing coincident caps in one primitive.
    let remaining_edges: Vec<_> = recipe
        .graph
        .edges
        .iter()
        .filter(|edge| !visited.contains(&edge.id))
        .collect();
    for edge in remaining_edges {
        visited.insert(edge.id);
        routes.push(RoutePolyline {
            points: vec![positions[&edge.from], positions[&edge.to]],
            diameter_m: edge.diameter_m,
        });
    }
    routes
}

fn walk_route_chain(
    start: u32,
    first_edge: u32,
    positions: &BTreeMap<u32, [f32; 3]>,
    edges: &BTreeMap<u32, &PipeRouteEdge>,
    adjacency: &BTreeMap<u32, Vec<u32>>,
    visited: &mut BTreeSet<u32>,
) -> RoutePolyline {
    let diameter_m = edges[&first_edge].diameter_m;
    let mut points = vec![positions[&start]];
    let mut current_node = start;
    let mut current_edge = first_edge;
    loop {
        visited.insert(current_edge);
        let edge = edges[&current_edge];
        let next_node = if edge.from == current_node {
            edge.to
        } else {
            edge.from
        };
        points.push(positions[&next_node]);
        let incident = adjacency
            .get(&next_node)
            .map_or(&[] as &[u32], Vec::as_slice);
        if incident.len() != 2 {
            break;
        }
        let Some(next_edge) = incident
            .iter()
            .copied()
            .find(|edge_id| !visited.contains(edge_id))
        else {
            break;
        };
        if (edges[&next_edge].diameter_m - diameter_m).abs() > 1.0e-6 {
            break;
        }
        current_node = next_node;
        current_edge = next_edge;
    }
    RoutePolyline { points, diameter_m }
}

#[allow(clippy::too_many_lines)] // Each semantic fitting's bounded construction is audited together.
fn append_fitting_geometry(
    primitives: &mut Vec<Primitive>,
    recipe: &PipeRecipe,
    fitting: &PipeFittingPlacement,
    radial: usize,
) -> Result<(), PipeForgeError> {
    let center = recipe
        .node_position(fitting.node_id)
        .ok_or(PipeForgeError::InvalidArtifact(
            "fitting references a missing route node",
        ))?;
    let mut ports: Vec<([f32; 3], f32)> = recipe
        .graph
        .edges
        .iter()
        .filter_map(|edge| {
            let other = if edge.from == fitting.node_id {
                edge.to
            } else if edge.to == fitting.node_id {
                edge.from
            } else {
                return None;
            };
            let other = recipe.node_position(other)?;
            Some((normalize(sub(other, center)), edge.diameter_m))
        })
        .collect();
    ports.sort_by(|left, right| {
        left.0[0]
            .total_cmp(&right.0[0])
            .then(left.0[1].total_cmp(&right.0[1]))
            .then(left.0[2].total_cmp(&right.0[2]))
    });
    if ports.is_empty() {
        return Err(PipeForgeError::InvalidArtifact(
            "fitting has no route ports",
        ));
    }
    let catalog = fitting.catalog_id.as_deref().and_then(|catalog_id| {
        recipe
            .fitting_catalog
            .iter()
            .find(|entry| entry.id == catalog_id)
    });
    let diameter_scale = catalog.map_or(1.0, |entry| entry.diameter_scale);
    let length_scale = catalog.map_or(1.0, |entry| entry.length_scale);
    let diameter = ports
        .iter()
        .map(|(_, diameter)| *diameter)
        .fold(recipe.diameter_m, f32::max);
    let base_radius = diameter * 0.5;
    let primary_axis = fitting_axis(&ports);
    match fitting.kind {
        PipeFittingKind::Tee => {
            for &(direction, port_diameter) in &ports {
                push_cylinder(
                    primitives,
                    center,
                    direction,
                    port_diameter * 0.72 * length_scale,
                    port_diameter * 0.62 * diameter_scale,
                    radial,
                );
            }
        }
        PipeFittingKind::Valve => {
            push_cylinder(
                primitives,
                center,
                primary_axis,
                diameter * 1.45 * length_scale,
                base_radius * 1.42 * diameter_scale,
                radial,
            );
            let stem_axis = transported_normal(primary_axis, [0.0, 1.0, 0.0]);
            let stem_center = add(center, mul(stem_axis, diameter * 0.56));
            push_cylinder(
                primitives,
                stem_center,
                stem_axis,
                diameter * 0.9 * length_scale,
                base_radius * 0.24 * diameter_scale,
                radial.max(12) / 2,
            );
        }
        PipeFittingKind::Flange => push_cylinder(
            primitives,
            center,
            primary_axis,
            diameter * 0.32 * length_scale,
            base_radius * 1.72 * diameter_scale,
            radial,
        ),
        PipeFittingKind::Elbow => {
            // The route sweep supplies the curved elbow. This raised compact hub makes the fitting
            // semantic and readable at production distance without adding an intersecting open mesh.
            push_cylinder(
                primitives,
                center,
                primary_axis,
                diameter * 0.52 * length_scale,
                base_radius * 1.18 * diameter_scale,
                radial,
            );
        }
        PipeFittingKind::Coupling => push_cylinder(
            primitives,
            center,
            primary_axis,
            diameter * 0.7 * length_scale,
            base_radius * 1.2 * diameter_scale,
            radial,
        ),
    }
    Ok(())
}

fn fitting_axis(ports: &[([f32; 3], f32)]) -> [f32; 3] {
    if ports.len() >= 2 {
        let span = sub(ports[0].0, ports[1].0);
        if length(span) > 0.25 {
            return normalize(span);
        }
    }
    ports[0].0
}

fn push_cylinder(
    primitives: &mut Vec<Primitive>,
    center: [f32; 3],
    axis: [f32; 3],
    axial_length: f32,
    radius: f32,
    radial: usize,
) {
    let half = mul(normalize(axis), axial_length.max(MIN_SEGMENT_M) * 0.5);
    let centers = [sub(center, half), add(center, half)];
    let mut primitive = Primitive::default();
    append_sweep(
        &mut primitive,
        &centers,
        &[radius.max(0.005), radius.max(0.005)],
        radial.max(6),
    );
    primitive.material = 0;
    primitives.push(primitive);
}

/// Samples the rounded route at every joint-band boundary and returns a continuous radius profile.
/// Fittings therefore remain part of the pipe's single indexed sweep instead of becoming sealed,
/// intersecting shells.
fn profiled_centerline(recipe: &PipeRecipe) -> (Vec<[f32; 3]>, Vec<f32>) {
    let rounded = rounded_centerline(
        &recipe.points,
        recipe.bend_radius_m,
        recipe.quality.bend_steps(),
    );
    let base_radius = recipe.diameter_m * 0.5;
    if !recipe.auto_fittings || rounded.len() < 2 {
        return (rounded.clone(), vec![base_radius; rounded.len()]);
    }

    let cumulative = cumulative_lengths(&rounded);
    let total = *cumulative.last().unwrap_or(&0.0);
    let joint_distances: Vec<f32> = recipe
        .points
        .iter()
        .map(|&point| nearest_arclength(&rounded, &cumulative, point))
        .collect();
    let band_half = base_radius * 0.58;
    let transition = (base_radius * 0.35).max(0.001);
    let mut samples = cumulative.clone();
    for &joint in &joint_distances {
        for offset in [
            -(band_half + transition),
            -band_half,
            0.0,
            band_half,
            band_half + transition,
        ] {
            samples.push((joint + offset).clamp(0.0, total));
        }
    }
    samples.sort_by(f32::total_cmp);
    samples.dedup_by(|a, b| (*a - *b).abs() <= 1.0e-6);

    let centers = samples
        .iter()
        .map(|&s| sample_polyline(&rounded, &cumulative, s))
        .collect();
    let collar_radius = base_radius * 1.18;
    let radii = samples
        .iter()
        .map(|&s| {
            joint_distances.iter().fold(base_radius, |radius, &joint| {
                let d = (s - joint).abs();
                let candidate = if d <= band_half {
                    collar_radius
                } else if d < band_half + transition {
                    let t = (d - band_half) / transition;
                    collar_radius + (base_radius - collar_radius) * t
                } else {
                    base_radius
                };
                radius.max(candidate)
            })
        })
        .collect();
    (centers, radii)
}

fn cumulative_lengths(points: &[[f32; 3]]) -> Vec<f32> {
    let mut cumulative = Vec::with_capacity(points.len());
    cumulative.push(0.0);
    for segment in points.windows(2) {
        let next = cumulative.last().copied().unwrap_or(0.0) + distance(segment[0], segment[1]);
        cumulative.push(next);
    }
    cumulative
}

fn nearest_arclength(points: &[[f32; 3]], cumulative: &[f32], point: [f32; 3]) -> f32 {
    let mut best_distance_sq = f32::INFINITY;
    let mut best_s = 0.0;
    for (i, segment) in points.windows(2).enumerate() {
        let delta = sub(segment[1], segment[0]);
        let length_sq = dot(delta, delta);
        if length_sq <= 1.0e-12 {
            continue;
        }
        let t = (dot(sub(point, segment[0]), delta) / length_sq).clamp(0.0, 1.0);
        let projection = add(segment[0], mul(delta, t));
        let d = sub(point, projection);
        let d_sq = dot(d, d);
        if d_sq < best_distance_sq {
            best_distance_sq = d_sq;
            best_s = cumulative[i] + length_sq.sqrt() * t;
        }
    }
    best_s
}

fn sample_polyline(points: &[[f32; 3]], cumulative: &[f32], s: f32) -> [f32; 3] {
    if s <= 0.0 {
        return points[0];
    }
    let total = *cumulative.last().unwrap_or(&0.0);
    if s >= total {
        return *points.last().unwrap();
    }
    let upper = cumulative.partition_point(|&distance| distance < s);
    let index = upper.saturating_sub(1).min(points.len() - 2);
    let span = cumulative[index + 1] - cumulative[index];
    let t = if span > 1.0e-8 {
        (s - cumulative[index]) / span
    } else {
        0.0
    };
    add(points[index], mul(sub(points[index + 1], points[index]), t))
}

#[allow(clippy::many_single_char_names)] // a/b/c/t/q are standard quadratic Bezier notation.
fn rounded_centerline(points: &[[f32; 3]], bend_radius: f32, steps: usize) -> Vec<[f32; 3]> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let mut out = vec![points[0]];
    for i in 1..points.len() - 1 {
        let (a, b, c) = (points[i - 1], points[i], points[i + 1]);
        let toward_a = normalize(sub(a, b));
        let toward_c = normalize(sub(c, b));
        let angle = dot(toward_a, toward_c).clamp(-1.0, 1.0).acos();
        if angle < 0.08 || (std::f32::consts::PI - angle).abs() < 0.04 {
            out.push(b);
            continue;
        }
        let max_offset = (distance(a, b).min(distance(b, c)) * 0.42).max(0.0);
        let offset = (bend_radius / (angle * 0.5).tan().max(0.05)).min(max_offset);
        if offset < MIN_SEGMENT_M {
            out.push(b);
            continue;
        }
        let entry = add(b, mul(toward_a, offset));
        let exit = add(b, mul(toward_c, offset));
        push_unique(&mut out, entry);
        for s in 1..=steps.max(1) {
            let t = s as f32 / steps.max(1) as f32;
            let omt = 1.0 - t;
            let q = add(
                add(mul(entry, omt * omt), mul(b, 2.0 * omt * t)),
                mul(exit, t * t),
            );
            push_unique(&mut out, q);
        }
    }
    push_unique(&mut out, *points.last().unwrap());
    out
}

fn append_sweep(prim: &mut Primitive, centers: &[[f32; 3]], radii: &[f32], radial: usize) {
    debug_assert_eq!(centers.len(), radii.len());
    let base = prim.positions.len() as u32;
    let stride = radial + 1; // duplicate the circumference seam so V can be exactly 0 and 1
    let mut traveled = 0.0f32;
    let mut previous_normal = [0.0; 3];
    let mut tangents = Vec::with_capacity(centers.len());
    let cumulative = cumulative_lengths(centers);
    let texture_scale_radius = radii.iter().copied().fold(f32::INFINITY, f32::min);
    for i in 0..centers.len() {
        if i > 0 {
            traveled += distance(centers[i - 1], centers[i]);
        }
        let tangent = if i == 0 {
            normalize(sub(centers[1], centers[0]))
        } else if i + 1 == centers.len() {
            normalize(sub(centers[i], centers[i - 1]))
        } else {
            normalize(sub(centers[i + 1], centers[i - 1]))
        };
        let normal = transported_normal(tangent, previous_normal);
        previous_normal = normal;
        let binormal = normalize(cross(normal, tangent));
        tangents.push(tangent);
        let dr_ds = if i == 0 {
            (radii[1] - radii[0]) / (cumulative[1] - cumulative[0]).max(1.0e-6)
        } else if i + 1 == centers.len() {
            (radii[i] - radii[i - 1]) / (cumulative[i] - cumulative[i - 1]).max(1.0e-6)
        } else {
            (radii[i + 1] - radii[i - 1]) / (cumulative[i + 1] - cumulative[i - 1]).max(1.0e-6)
        };
        for j in 0..=radial {
            let v = j as f32 / radial as f32;
            let theta = v * std::f32::consts::TAU;
            let radial_normal =
                normalize(add(mul(normal, theta.cos()), mul(binormal, theta.sin())));
            let surface_normal = normalize(sub(radial_normal, mul(tangent, dr_ds)));
            prim.positions
                .push(add(centers[i], mul(radial_normal, radii[i])));
            prim.normals.push(surface_normal);
            prim.uvs.push([
                traveled / (std::f32::consts::TAU * texture_scale_radius).max(0.001),
                v,
            ]);
        }
    }
    for i in 0..centers.len() - 1 {
        for j in 0..radial {
            let a = base + (i * stride + j) as u32;
            let b = base + ((i + 1) * stride + j) as u32;
            let c = base + (i * stride + j + 1) as u32;
            let d = base + ((i + 1) * stride + j + 1) as u32;
            prim.indices.extend_from_slice(&[a, b, d, a, d, c]);
        }
    }
    // Caps duplicate the boundary vertices for flat shading and planar UVs. The topology validator
    // welds coincident positions, so the render seam/hard edge remains one closed manifold.
    append_cap(prim, centers[0], mul(tangents[0], -1.0), base, radial, true);
    let last_ring = base + ((centers.len() - 1) * stride) as u32;
    append_cap(
        prim,
        *centers.last().unwrap(),
        *tangents.last().unwrap(),
        last_ring,
        radial,
        false,
    );
}

fn append_cap(
    prim: &mut Primitive,
    center: [f32; 3],
    outward: [f32; 3],
    side_ring: u32,
    radial: usize,
    start: bool,
) {
    let cap_center = prim.positions.len() as u32;
    prim.positions.push(center);
    prim.normals.push(outward);
    prim.uvs.push([0.5, 0.5]);
    let cap_ring = prim.positions.len() as u32;
    for j in 0..radial {
        let position = prim.positions[side_ring as usize + j];
        prim.positions.push(position);
        prim.normals.push(outward);
        let theta = j as f32 / radial as f32 * std::f32::consts::TAU;
        prim.uvs
            .push([0.5 + theta.cos() * 0.5, 0.5 + theta.sin() * 0.5]);
    }
    for j in 0..radial {
        let k = (j + 1) % radial;
        let j = cap_ring + j as u32;
        let k = cap_ring + k as u32;
        if start {
            prim.indices.extend_from_slice(&[cap_center, j, k]);
        } else {
            prim.indices.extend_from_slice(&[cap_center, k, j]);
        }
    }
}

fn material_textures(kit: PipeKit, res: u32) -> (Texture, Texture, Texture, f32, f32) {
    let (base_rgb, metal, rough, seed) = match kit {
        PipeKit::Galvanized => ([154u8, 164, 170], 245u8, 92u8, 0x51a2_u32),
        PipeKit::Copper => ([190u8, 89, 48], 250u8, 72u8, 0xc022_u32),
        PipeKit::Pvc => ([218u8, 226, 229], 0u8, 82u8, 0x9c31_u32),
        PipeKit::Scifi => ([35u8, 63, 70], 220u8, 54u8, 0x5c1f_u32),
    };
    let mut base = Vec::with_capacity(res as usize * res as usize * 4);
    let mut mr = Vec::with_capacity(base.capacity());
    let mut normal = Vec::with_capacity(base.capacity());
    for y in 0..res {
        for x in 0..res {
            let noise = i16::from(hash_noise(x, y, seed)) - 128;
            let scratch = if hash_noise(x / 2, y * 7, seed ^ 0x9911) > 247 {
                -22
            } else {
                0
            };
            let stripe = matches!(kit, PipeKit::Scifi) && (x * 8 / res.max(1)).is_multiple_of(8);
            for (channel, source) in base_rgb.into_iter().enumerate() {
                let tint = if stripe {
                    [190i16, 82, 18][channel]
                } else {
                    i16::from(source) + noise / 13 + scratch
                };
                base.push(clamped_u8(tint));
            }
            base.push(255);
            let rough_px = clamped_u8((i16::from(rough) + noise / 10).clamp(8, 250));
            mr.extend_from_slice(&[235, rough_px, metal, 255]); // glTF: roughness=G, metalness=B
            let hx = i16::from(hash_noise(x.wrapping_add(1), y, seed))
                - i16::from(hash_noise(x.wrapping_sub(1), y, seed));
            let hy = i16::from(hash_noise(x, y.wrapping_add(1), seed))
                - i16::from(hash_noise(x, y.wrapping_sub(1), seed));
            normal.extend_from_slice(&[
                clamped_u8(128 + hx / 16),
                clamped_u8(128 + hy / 16),
                252,
                255,
            ]);
        }
    }
    let texture = |rgba8| Texture {
        width: res,
        height: res,
        rgba8,
    };
    (texture(base), texture(mr), texture(normal), 1.0, 1.0)
}

fn hash_noise(x: u32, y: u32, seed: u32) -> u8 {
    let mut h = x
        .wrapping_mul(0x9e37_79b1)
        .wrapping_add(y.wrapping_mul(0x85eb_ca77))
        ^ seed;
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    (h ^ (h >> 16)) as u8
}

fn clamped_u8(value: i16) -> u8 {
    u8::try_from(value.clamp(0, 255)).expect("value was clamped to the u8 range")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct WeldKey(i64, i64, i64);

fn weld_key(position: [f32; 3]) -> Option<WeldKey> {
    finite3(position).then(|| {
        WeldKey(
            (position[0] / WELD_EPSILON_M).round() as i64,
            (position[1] / WELD_EPSILON_M).round() as i64,
            (position[2] / WELD_EPSILON_M).round() as i64,
        )
    })
}

/// Validates the rendered topology after welding deliberate UV seams and hard-normal cap vertices.
/// Besides two uses per edge, orientations must cancel and every triangle must belong to one component.
fn is_watertight(prim: &Primitive) -> bool {
    if prim.positions.is_empty()
        || prim.positions.len() != prim.normals.len()
        || prim.positions.len() != prim.uvs.len()
        || prim.indices.is_empty()
        || !prim.indices.len().is_multiple_of(3)
        || prim.positions.iter().any(|p| !finite3(*p))
        || prim
            .normals
            .iter()
            .any(|n| !finite3(*n) || !(0.5..=1.5).contains(&length(*n)))
        || prim.uvs.iter().flatten().any(|v| !v.is_finite())
    {
        return false;
    }

    let mut key_ids = HashMap::<WeldKey, usize>::new();
    let mut vertex_keys = Vec::with_capacity(prim.positions.len());
    for &position in &prim.positions {
        let Some(key) = weld_key(position) else {
            return false;
        };
        let next_id = key_ids.len();
        let id = *key_ids.entry(key).or_insert(next_id);
        vertex_keys.push((key, id));
    }
    let mut parent: Vec<usize> = (0..key_ids.len()).collect();
    let mut edges = HashMap::<(WeldKey, WeldKey), (u8, i8)>::new();
    let mut used_vertices = vec![false; prim.positions.len()];
    let mut used_welds = vec![false; key_ids.len()];

    for tri in prim.indices.as_chunks::<3>().0 {
        let [ia, ib, ic] = [tri[0] as usize, tri[1] as usize, tri[2] as usize];
        if ia >= prim.positions.len() || ib >= prim.positions.len() || ic >= prim.positions.len() {
            return false;
        }
        used_vertices[ia] = true;
        used_vertices[ib] = true;
        used_vertices[ic] = true;
        let [(ka, wa), (kb, wb), (kc, wc)] = [vertex_keys[ia], vertex_keys[ib], vertex_keys[ic]];
        if ka == kb || kb == kc || kc == ka {
            return false;
        }
        let area_twice = length(cross(
            sub(prim.positions[ib], prim.positions[ia]),
            sub(prim.positions[ic], prim.positions[ia]),
        ));
        if !area_twice.is_finite() || area_twice <= 1.0e-10 {
            return false;
        }
        used_welds[wa] = true;
        used_welds[wb] = true;
        used_welds[wc] = true;
        union_sets(&mut parent, wa, wb);
        union_sets(&mut parent, wb, wc);
        for (from, to) in [(ka, kb), (kb, kc), (kc, ka)] {
            let (key, direction) = if from < to {
                ((from, to), 1)
            } else {
                ((to, from), -1)
            };
            let entry = edges.entry(key).or_default();
            entry.0 = entry.0.saturating_add(1);
            entry.1 = entry.1.saturating_add(direction);
        }
    }

    if used_vertices.iter().any(|used| !used)
        || edges
            .values()
            .any(|&(uses, orientation)| uses != 2 || orientation != 0)
    {
        return false;
    }
    let Some(first) = used_welds.iter().position(|&used| used) else {
        return false;
    };
    let root = find_set(&mut parent, first);
    for (id, used) in used_welds.into_iter().enumerate() {
        if used && find_set(&mut parent, id) != root {
            return false;
        }
    }
    true
}

fn find_set(parent: &mut [usize], value: usize) -> usize {
    let mut root = value;
    while parent[root] != root {
        root = parent[root];
    }
    let mut current = value;
    while parent[current] != current {
        let next = parent[current];
        parent[current] = root;
        current = next;
    }
    root
}

fn union_sets(parent: &mut [usize], a: usize, b: usize) {
    let root_a = find_set(parent, a);
    let root_b = find_set(parent, b);
    if root_a != root_b {
        parent[root_b] = root_a;
    }
}

fn snap_grid(p: [f32; 3], grid: f32) -> [f32; 3] {
    p.map(|v| (v / grid).round() * grid)
}

fn snap_direction(origin: [f32; 3], point: [f32; 3], grid: f32) -> [f32; 3] {
    let d = sub(point, origin);
    let horizontal = (d[0] * d[0] + d[2] * d[2]).sqrt();
    if d[1].abs() > horizontal * 1.25 {
        return [origin[0], point[1], origin[2]];
    }
    if horizontal < grid * 0.5 {
        return point;
    }
    let step = std::f32::consts::FRAC_PI_4;
    let angle = d[2].atan2(d[0]);
    let snapped = (angle / step).round() * step;
    let length = (horizontal / grid).round().max(1.0) * grid;
    [
        origin[0] + snapped.cos() * length,
        point[1],
        origin[2] + snapped.sin() * length,
    ]
}

fn transported_normal(tangent: [f32; 3], previous: [f32; 3]) -> [f32; 3] {
    let projected = sub(previous, mul(tangent, dot(previous, tangent)));
    if length(projected) > 1.0e-4 {
        normalize(projected)
    } else {
        let reference = if tangent[1].abs() < 0.9 {
            [0.0, 1.0, 0.0]
        } else {
            [1.0, 0.0, 0.0]
        };
        normalize(cross(tangent, reference))
    }
}

fn push_unique(out: &mut Vec<[f32; 3]>, p: [f32; 3]) {
    if out.last().is_none_or(|&last| distance(last, p) > 1.0e-5) {
        out.push(p);
    }
}

fn finite3(p: [f32; 3]) -> bool {
    p.into_iter().all(f32::is_finite)
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}
fn normalize(a: [f32; 3]) -> [f32; 3] {
    let l = length(a);
    if l > 1.0e-8 {
        mul(a, 1.0 / l)
    } else {
        [1.0, 0.0, 0.0]
    }
}
fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    length(sub(a, b))
}

/// Minimum distance between two finite 3D line segments (Real-Time Collision Detection, §5.1.9).
#[allow(clippy::many_single_char_names)] // Notation follows Real-Time Collision Detection section 5.1.9.
fn segment_segment_distance(p1: [f32; 3], q1: [f32; 3], p2: [f32; 3], q2: [f32; 3]) -> f32 {
    let d1 = sub(q1, p1);
    let d2 = sub(q2, p2);
    let r = sub(p1, p2);
    let a = dot(d1, d1);
    let e = dot(d2, d2);
    let f = dot(d2, r);
    let epsilon = 1.0e-12;
    let (mut s, t);
    if a <= epsilon && e <= epsilon {
        return distance(p1, p2);
    }
    if a <= epsilon {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = dot(d1, r);
        if e <= epsilon {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = dot(d1, d2);
            let denominator = a * e - b * b;
            s = if denominator.abs() > epsilon {
                ((b * f - c * e) / denominator).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let projected = b * s + f;
            if projected < 0.0 {
                t = 0.0;
                s = (-c / a).clamp(0.0, 1.0);
            } else if projected > e {
                t = 1.0;
                s = ((b - c) / a).clamp(0.0, 1.0);
            } else {
                t = projected / e;
            }
        }
    }
    distance(add(p1, mul(d1, s)), add(p2, mul(d2, t)))
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // deterministic recipe and mesh fixtures intentionally assert exact values
mod tests {
    use super::*;

    fn recipe() -> PipeRecipe {
        let mut r = PipeRecipe::from_options(&PipeForgeOptions::default());
        r.points = vec![[2.0, 0.0, 3.0], [4.0, 0.0, 3.0], [4.0, 1.5, 3.0]];
        r
    }

    #[test]
    fn bake_is_deterministic_content_addressed_and_local() {
        let a = bake_pipe(&recipe()).expect("bake");
        let b = bake_pipe(&recipe()).expect("bake again");
        assert_eq!(a.artifact_bytes, b.artifact_bytes);
        assert_eq!(a.handle, b.handle);
        assert_eq!(a.anchor, [2.0, 0.0, 3.0]);
        assert_eq!(a.recipe.points[0], [0.0; 3]);
        assert_eq!(a.handle, AssetId::of_bytes(&a.artifact_bytes).as_str());
    }

    #[test]
    fn translated_identical_shape_deduplicates() {
        let a = recipe();
        let mut b = a.clone();
        for p in &mut b.points {
            *p = add(*p, [100.0, 12.0, -40.0]);
        }
        assert_eq!(bake_pipe(&a).unwrap().handle, bake_pipe(&b).unwrap().handle);
    }

    #[test]
    fn mesh_is_watertight_finite_textured_and_lod_ready() {
        let built = bake_pipe(&recipe()).expect("bake");
        let p = &built.asset.primitives[0];
        assert!(is_watertight(p));
        assert!(p.positions.iter().flatten().all(|v| v.is_finite()));
        assert!(p.normals.iter().all(|n| (length(*n) - 1.0).abs() < 1.0e-3));
        assert_eq!(p.positions.len(), p.uvs.len());
        assert!(p.uvs.iter().flatten().all(|v| v.is_finite()));
        assert_eq!(built.asset.textures.len(), 3);
        assert_eq!(built.asset.materials[0].base_color_texture, Some(0));
        assert_eq!(built.asset.materials[0].metallic_roughness_texture, Some(1));
        assert_eq!(built.asset.materials[0].normal_texture, Some(2));
        assert!(built.report.lod_triangles.windows(2).all(|w| w[1] <= w[0]));
        assert_eq!(built.report.collision_hulls, 0);
        assert_eq!(built.report.collision_kind, "triangle mesh");
        assert_eq!(built.report.collision_triangles, built.report.triangles);
    }

    #[test]
    fn joint_bands_are_one_connected_manifold_not_overlapping_shells() {
        let fitted = bake_pipe(&recipe()).expect("fitted pipe");
        let mut plain_recipe = recipe();
        plain_recipe.auto_fittings = false;
        let plain = bake_pipe(&plain_recipe).expect("plain pipe");
        assert!(fitted.report.triangles > plain.report.triangles);
        assert!(is_watertight(&fitted.asset.primitives[0]));

        // A second individually closed shell used to pass the old edge-count validator. The welded
        // connectivity check must reject it even though every edge in both components has two uses.
        let mut disconnected = fitted.asset.primitives[0].clone();
        let original_vertices = disconnected.positions.len();
        let original_indices = disconnected.indices.clone();
        let extra_positions: Vec<_> = disconnected.positions[..original_vertices]
            .iter()
            .map(|&p| add(p, [50.0, 0.0, 0.0]))
            .collect();
        disconnected.positions.extend(extra_positions);
        disconnected.normals.extend_from_within(..original_vertices);
        disconnected.uvs.extend_from_within(..original_vertices);
        disconnected.indices.extend(
            original_indices
                .into_iter()
                .map(|index| index + original_vertices as u32),
        );
        assert!(!is_watertight(&disconnected));
    }

    #[test]
    fn circumference_seams_and_caps_have_production_uvs_and_normals() {
        let r = recipe();
        let (centers, radii) = profiled_centerline(&r);
        let radial = r.quality.radial_segments();
        let stride = radial + 1;
        let mut prim = Primitive::default();
        append_sweep(&mut prim, &centers, &radii, radial);
        for ring in 0..centers.len() {
            let first = ring * stride;
            let seam = first + radial;
            assert!(distance(prim.positions[first], prim.positions[seam]) < 1.0e-5);
            assert_eq!(prim.uvs[first][1], 0.0);
            assert_eq!(prim.uvs[seam][1], 1.0);
        }
        let side_vertices = centers.len() * stride;
        let start_outward = normalize(sub(centers[0], centers[1]));
        assert_eq!(prim.positions[side_vertices], centers[0]);
        assert!(dot(prim.normals[side_vertices], start_outward) > 0.999);
        for normal in &prim.normals[side_vertices..=side_vertices + radial] {
            assert!(dot(*normal, start_outward) > 0.999);
        }
        assert!(is_watertight(&prim));
    }

    #[test]
    fn artifact_round_trip_rebuilds_exact_mesh_and_rejects_corruption() {
        let built = bake_pipe(&recipe()).expect("bake");
        let (restored_recipe, restored, report) = decode_pipe_artifact(&built.artifact_bytes)
            .expect("decode")
            .expect("pipe magic");
        assert_eq!(restored_recipe, built.recipe);
        assert_eq!(restored, built.asset);
        assert_eq!(report.handle.as_deref(), Some(built.handle.as_str()));
        let mut corrupt = built.artifact_bytes.clone();
        corrupt.truncate(24);
        assert!(decode_pipe_artifact(&corrupt).is_err());
        assert!(decode_pipe_artifact(b"glTF bytes").unwrap().is_none());

        let mut trailing = built.artifact_bytes.clone();
        trailing.push(0);
        assert!(decode_pipe_artifact(&trailing).is_err());
    }

    #[test]
    fn artifact_decoder_enforces_byte_buffer_and_finite_value_budgets() {
        let built = bake_pipe(&recipe()).expect("bake");
        let mut artifact: PipeArtifactV2 =
            bincode::deserialize(&built.artifact_bytes[PIPE_ARTIFACT_MAGIC.len()..])
                .expect("test artifact");
        artifact.primitives[0].normals[0][0] = f32::NAN;
        let mut non_finite = PIPE_ARTIFACT_MAGIC.to_vec();
        non_finite.extend(bincode::serialize(&artifact).unwrap());
        assert!(decode_pipe_artifact(&non_finite).is_err());

        let mut bad_material: PipeArtifactV2 =
            bincode::deserialize(&built.artifact_bytes[PIPE_ARTIFACT_MAGIC.len()..])
                .expect("test artifact");
        bad_material.primitives[0].material = 99;
        let mut bad_material_bytes = PIPE_ARTIFACT_MAGIC.to_vec();
        bad_material_bytes.extend(bincode::serialize(&bad_material).unwrap());
        assert!(decode_pipe_artifact(&bad_material_bytes).is_err());

        let mut over_budget = vec![0_u8; MAX_ARTIFACT_BYTES + 1];
        over_budget[..PIPE_ARTIFACT_MAGIC.len()].copy_from_slice(PIPE_ARTIFACT_MAGIC);
        assert!(decode_pipe_artifact(&over_budget).is_err());
    }

    #[test]
    fn legacy_v1_artifact_migrates_without_losing_source_or_cooked_mesh() {
        let built = bake_pipe(&recipe()).expect("current bake");
        let recipe = LegacyPipeRecipeV1 {
            version: LEGACY_PIPE_RECIPE_VERSION,
            points: built.recipe.points.clone(),
            diameter_m: built.recipe.diameter_m,
            kit: built.recipe.kit,
            quality: built.recipe.quality,
            auto_fittings: built.recipe.auto_fittings,
            grid_m: built.recipe.grid_m,
            bend_radius_m: built.recipe.bend_radius_m,
        };
        let primitive = &built.asset.primitives[0];
        let material = &built.asset.materials[0];
        let legacy = PipeArtifactV1 {
            version: LEGACY_PIPE_RECIPE_VERSION,
            recipe: recipe.clone(),
            positions: primitive.positions.clone(),
            normals: primitive.normals.clone(),
            uvs: primitive.uvs.clone(),
            indices: primitive.indices.clone(),
            textures: built
                .asset
                .textures
                .iter()
                .map(|texture| ArtifactTexture {
                    width: texture.width,
                    height: texture.height,
                    rgba8: texture.rgba8.clone(),
                })
                .collect(),
            metallic: material.metallic,
            roughness: material.roughness,
        };
        let mut bytes = PIPE_ARTIFACT_MAGIC.to_vec();
        bytes.extend(bincode::serialize(&legacy).expect("encode legacy fixture"));

        let (migrated, restored, report) = decode_pipe_artifact(&bytes)
            .expect("decode legacy")
            .expect("pipe artifact");
        assert_eq!(migrated.version, PIPE_RECIPE_VERSION);
        assert_eq!(migrated.points, recipe.points);
        assert_eq!(migrated.diameter_m, recipe.diameter_m);
        assert_eq!(migrated.kit, recipe.kit);
        assert_eq!(migrated.quality, recipe.quality);
        assert_eq!(migrated.graph.nodes.len(), recipe.points.len());
        assert_eq!(migrated.graph.edges.len(), recipe.points.len() - 1);
        assert_eq!(restored.primitives.len(), 1);
        assert_eq!(restored.primitives[0].positions, primitive.positions);
        assert_eq!(restored.primitives[0].indices, primitive.indices);
        assert!(report.watertight);
        assert_eq!(
            report.handle.as_deref(),
            Some(AssetId::of_bytes(&bytes).as_str())
        );
    }

    #[test]
    fn graph_branches_semantic_fittings_catalog_and_post_bake_handles_round_trip() {
        let mut source = PipeRecipe::from_options(&PipeForgeOptions::default());
        source.add_point([0.0, 0.0, 0.0]).unwrap();
        source.add_point([2.0, 0.0, 0.0]).unwrap();
        source.add_point([4.0, 0.0, 0.0]).unwrap();
        let branch_nodes = source
            .add_branch(2, &[[2.0, 0.0, 2.0]], Some(0.06))
            .expect("branch");
        let branch_tip = branch_nodes[0];
        assert!(source
            .fittings
            .iter()
            .any(|fitting| fitting.node_id == 2 && fitting.kind == PipeFittingKind::Tee));

        source
            .upsert_catalog_entry(UserFittingCatalogEntry {
                id: "project-valve-dn60".into(),
                label: "Project isolation valve DN60".into(),
                kind: PipeFittingKind::Valve,
                asset_handle: Some("asset:project-valve-dn60".into()),
                diameter_scale: 1.1,
                length_scale: 1.25,
            })
            .unwrap();
        let valve_id = source
            .place_fitting(
                branch_tip,
                PipeFittingKind::Valve,
                Some("project-valve-dn60".into()),
            )
            .unwrap();
        source
            .place_fitting(1, PipeFittingKind::Flange, None)
            .unwrap();

        let first = bake_pipe(&source).expect("graph bake");
        assert!(first.asset.primitives.len() >= 8);
        assert!(first.asset.primitives.iter().all(is_watertight));
        assert!(first
            .report
            .warnings
            .iter()
            .any(|warning| warning.contains("proxy")));
        let (mut restored, restored_asset, _) = decode_pipe_artifact(&first.artifact_bytes)
            .unwrap()
            .unwrap();
        assert_eq!(restored, first.recipe);
        assert_eq!(restored_asset, first.asset);
        let handle = restored
            .route_handles()
            .into_iter()
            .find(|handle| handle.node_id == branch_tip)
            .expect("branch handle");
        assert!(handle.fitting_ids.contains(&valve_id));
        assert_eq!(handle.connected_edges.len(), 1);

        restored
            .move_route_handle(branch_tip, [2.0, 1.0, 2.0])
            .expect("edit restored route");
        assert!(restored
            .fittings
            .iter()
            .any(|fitting| fitting.id == valve_id && fitting.node_id == branch_tip));
        let edited = bake_pipe(&restored).expect("rebake edited route");
        assert_ne!(edited.handle, first.handle);
        assert_eq!(
            edited.recipe.node_position(branch_tip),
            Some([2.0, 1.0, 2.0])
        );

        let before_invalid = restored.clone();
        assert!(restored
            .move_route_handle(branch_tip, [2.0, 0.0, 0.0])
            .is_err());
        assert_eq!(
            restored, before_invalid,
            "invalid handle moves are transactional"
        );
    }

    #[test]
    fn malformed_graph_catalog_and_branch_budget_fail_transactionally() {
        let mut source = PipeRecipe::from_options(&PipeForgeOptions::default());
        source.add_point([0.0, 0.0, 0.0]).unwrap();
        source.add_point([2.0, 0.0, 0.0]).unwrap();

        let before = source.clone();
        assert!(source
            .add_branch(2, &[[2.0, 0.0, 2.0], [2.0, 0.0, 2.0]], None)
            .is_err());
        assert_eq!(
            source, before,
            "a failed branch never leaves partial graph edits"
        );

        let mut duplicate = before.clone();
        duplicate.graph.nodes[1].id = duplicate.graph.nodes[0].id;
        assert!(matches!(
            bake_pipe(&duplicate),
            Err(PipeForgeError::InvalidArtifact("duplicate route node ID"))
        ));

        let mut missing_node = before.clone();
        missing_node.graph.edges[0].to = 99_999;
        assert!(matches!(
            bake_pipe(&missing_node),
            Err(PipeForgeError::InvalidArtifact(
                "route edge references a missing node"
            ))
        ));

        let mut over_budget = PipeRecipe::from_options(&PipeForgeOptions::default());
        over_budget.graph.nodes = (0..=MAX_ROUTE_NODES)
            .map(|id| PipeRouteNode {
                id: id as u32,
                position: [id as f32, 0.0, 0.0],
                primary_index: None,
            })
            .collect();
        assert!(matches!(
            bake_pipe(&over_budget),
            Err(PipeForgeError::TooManyPoints)
        ));

        assert!(source
            .upsert_catalog_entry(UserFittingCatalogEntry {
                id: "bad".into(),
                label: "Invalid dimensions".into(),
                kind: PipeFittingKind::Flange,
                asset_handle: None,
                diameter_scale: f32::NAN,
                length_scale: 1.0,
            })
            .is_err());
        assert!(source.fitting_catalog.is_empty());
    }

    #[test]
    fn semantic_fittings_enforce_port_topology_and_catalog_references() {
        let mut source = PipeRecipe::from_options(&PipeForgeOptions::default());
        source.add_point([0.0, 0.0, 0.0]).unwrap();
        source.add_point([2.0, 0.0, 0.0]).unwrap();
        source.add_point([4.0, 0.0, 0.0]).unwrap();
        assert!(source.place_fitting(2, PipeFittingKind::Tee, None).is_err());
        source
            .place_fitting(2, PipeFittingKind::Coupling, None)
            .expect("two-port coupling");

        source
            .add_branch(2, &[[2.0, 0.0, 2.0]], Some(0.05))
            .unwrap();
        let automatic_tee = source
            .fittings
            .iter()
            .find(|fitting| {
                fitting.node_id == 2 && fitting.kind == PipeFittingKind::Tee && fitting.automatic
            })
            .expect("inferred tee")
            .id;
        source
            .upsert_catalog_entry(UserFittingCatalogEntry {
                id: "project-tee".into(),
                label: "Project tee".into(),
                kind: PipeFittingKind::Tee,
                asset_handle: Some("asset:project-tee".into()),
                diameter_scale: 1.0,
                length_scale: 1.0,
            })
            .unwrap();
        assert_eq!(
            source
                .place_fitting(2, PipeFittingKind::Tee, Some("project-tee".into()))
                .unwrap(),
            automatic_tee,
            "catalog placement upgrades the inferred fitting without overlapping geometry"
        );
        assert!(
            !source
                .fittings
                .iter()
                .find(|fitting| fitting.id == automatic_tee)
                .unwrap()
                .automatic
        );
        assert!(source
            .upsert_catalog_entry(UserFittingCatalogEntry {
                id: "project-tee".into(),
                label: "Invalid kind change".into(),
                kind: PipeFittingKind::Valve,
                asset_handle: None,
                diameter_scale: 1.0,
                length_scale: 1.0,
            })
            .is_err());
        assert!(source.remove_catalog_entry("project-tee").is_err());
        source.remove_fitting(automatic_tee).unwrap();
        source.remove_catalog_entry("project-tee").unwrap();
    }

    #[test]
    fn every_kit_and_quality_produces_distinct_valid_pbr_asset() {
        let mut handles = std::collections::BTreeSet::new();
        for kit in [
            PipeKit::Galvanized,
            PipeKit::Copper,
            PipeKit::Pvc,
            PipeKit::Scifi,
        ] {
            for quality in [
                PipeQuality::Preview,
                PipeQuality::Production,
                PipeQuality::Hero,
            ] {
                let mut r = recipe();
                r.kit = kit;
                r.quality = quality;
                let b = bake_pipe(&r).expect("kit bake");
                assert!(b.report.watertight);
                assert!(b.report.triangles > 0);
                assert_eq!(b.report.texture_resolution, quality.texture_resolution());
                handles.insert(b.handle);
            }
        }
        assert_eq!(handles.len(), 12);
    }

    #[test]
    fn snapped_authoring_is_grid_and_angle_constrained() {
        let mut r = PipeRecipe::from_options(&PipeForgeOptions::default());
        assert_eq!(r.add_point([0.03, 0.02, 0.04]).unwrap(), [0.0, 0.0, 0.0]);
        let p = r.add_point([1.03, 0.04, 0.31]).unwrap();
        assert!((p[0] - p[2]).abs() < 1.0e-4 || p[2].abs() < 1.0e-4);
        assert!(r.add_point(p).is_err(), "duplicate point rejected");
    }

    #[test]
    fn invalid_and_oversized_recipes_fail_before_allocating() {
        let mut r = PipeRecipe::from_options(&PipeForgeOptions::default());
        assert_eq!(
            bake_pipe(&r).unwrap_err().to_string(),
            "Place at least two points before baking"
        );
        r.points = vec![[0.0; 3], [f32::NAN, 0.0, 0.0]];
        assert!(matches!(bake_pipe(&r), Err(PipeForgeError::NonFinitePoint)));
        r.points = (0..=MAX_POINTS).map(|i| [i as f32, 0.0, 0.0]).collect();
        assert!(matches!(bake_pipe(&r), Err(PipeForgeError::TooManyPoints)));

        let mut crossing = PipeRecipe::from_options(&PipeForgeOptions::default());
        crossing.points = vec![
            [-1.0, 0.0, -1.0],
            [1.0, 0.0, 1.0],
            [-1.0, 0.0, 1.0],
            [1.0, 0.0, -1.0],
        ];
        assert!(matches!(
            bake_pipe(&crossing),
            Err(PipeForgeError::InvalidArtifact(
                "route intersects itself or passes too close to itself"
            ))
        ));

        let mut reversal = PipeRecipe::from_options(&PipeForgeOptions::default());
        reversal.points = vec![[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.5, 0.0, 0.0]];
        assert!(matches!(
            bake_pipe(&reversal),
            Err(PipeForgeError::InvalidArtifact(
                "route doubles back on itself at a joint"
            ))
        ));

        let mut impossible_bend = recipe();
        impossible_bend.bend_radius_m = impossible_bend.diameter_m * 0.25;
        assert!(matches!(
            bake_pipe(&impossible_bend),
            Err(PipeForgeError::BadBendRadius)
        ));
    }

    #[test]
    fn tool_undo_and_status_are_truthful() {
        let options = PipeForgeOptions {
            kit: PipeKit::Copper,
            diameter_cm: 27.5,
            quality: PipeQuality::Hero,
            auto_fittings: false,
        };
        let mut tool = PipeToolSession::new(&options);
        assert!(!tool.status().can_bake);
        assert_eq!(tool.status().kit, PipeKit::Copper);
        assert_eq!(tool.status().diameter_cm, 27.5);
        assert_eq!(tool.status().quality, PipeQuality::Hero);
        assert!(!tool.status().auto_fittings);
        tool.point([0.0, 0.0, 0.0]);
        tool.point([2.0, 0.0, 0.0]);
        assert!(tool.status().can_bake);
        assert!(tool.status().preview_triangles > 0);
        tool.undo_point();
        assert_eq!(tool.status().points, 1);
        assert!(!tool.status().can_bake);
    }

    #[test]
    fn restored_session_projects_world_handles_and_extends_a_stable_branch_tip() {
        let baked = bake_pipe(&recipe()).expect("initial bake");
        let mut tool =
            PipeToolSession::edit(&baked.recipe, "entity:pipe-7".into(), [10.0, 2.0, -3.0])
                .expect("restore session");
        let status = tool.status();
        assert_eq!(status.editing_entity.as_deref(), Some("entity:pipe-7"));
        assert_eq!(status.handles[0].position, [10.0, 2.0, -3.0]);
        assert_eq!(status.graph_edges.len(), 2);

        let status = tool.begin_branch(2, 6.0);
        assert_eq!(status.branch_from, Some(2));
        let status = tool.point([12.0, 2.0, -1.0]);
        let tip = status.branch_from.expect("stable branch tip");
        assert_ne!(tip, 2);
        assert_eq!(status.graph_edges.len(), 3);
        assert_eq!(status.handles.len(), 4);
        assert_eq!(
            status
                .handles
                .iter()
                .find(|handle| handle.node_id == tip)
                .expect("tip handle")
                .position,
            [12.0, 2.0, -1.0]
        );
        assert!(status
            .fittings
            .iter()
            .any(|fitting| fitting.node_id == 2 && fitting.kind == PipeFittingKind::Tee));
        let status = tool.end_branch();
        assert_eq!(status.branch_from, None);
    }

    #[test]
    fn status_json_matches_the_viewport_transport_contract() {
        let status = PipeToolSession::edit(
            &bake_pipe(&recipe()).unwrap().recipe,
            "pipe-transport".into(),
            [4.0, 0.0, 1.0],
        )
        .unwrap()
        .status();
        let json = serde_json::to_value(status).expect("serialize status");
        for key in [
            "lengthM",
            "previewTriangles",
            "canBake",
            "autoFittings",
            "handles",
            "edges",
            "fittings",
            "fittingCatalog",
            "branchFrom",
            "editingEntity",
        ] {
            assert!(json.get(key).is_some(), "missing camelCase field {key}");
        }
        assert!(json.get("graphEdges").is_none());
        assert!(json.get("catalog").is_none());
    }

    #[test]
    fn in_place_rebake_preserves_entity_and_anchor_and_undo_restores_old_source() {
        let mut world = FlecsWorld::new();
        let scene = crate::capscene::CapScene::intern(&mut world);
        let mut engine = Engine::new(world, 91);
        let first = bake_pipe(&recipe()).expect("initial bake");
        let id = land_pipe_asset(
            &mut engine,
            &scene,
            &first.recipe,
            &first.handle,
            first.anchor,
            &first.report,
        )
        .expect("land pipe");
        let original_count = engine.entity_count();
        let original = engine.components_of(id);
        let original_transform = original.get("Transform").cloned();
        let original_source = original
            .get("PipeRecipe")
            .and_then(|fields| fields.get("source"))
            .cloned();
        let original_handle = original
            .get("MeshRenderer")
            .and_then(|fields| fields.get(crate::capscene::MESH_FIELD))
            .cloned();

        let mut edited_source = first.recipe.clone();
        edited_source
            .move_route_handle(2, [3.0, 0.0, 0.0])
            .expect("move middle handle");
        let edited = rebake_pipe_in_place(&edited_source).expect("in-place compile");
        replace_pipe_asset(
            &mut engine,
            id,
            &edited.recipe,
            &edited.handle,
            &edited.report,
        )
        .expect("single replacement commit");

        assert_eq!(engine.entity_count(), original_count, "identity was reused");
        let replaced = engine.components_of(id);
        assert_eq!(replaced.get("Transform").cloned(), original_transform);
        assert_ne!(
            replaced
                .get("MeshRenderer")
                .and_then(|fields| fields.get(crate::capscene::MESH_FIELD)),
            original_handle.as_ref()
        );
        assert!(engine.undo(), "one undo peels the complete replacement");
        let undone = engine.components_of(id);
        assert_eq!(undone.get("Transform").cloned(), original_transform);
        assert_eq!(
            undone
                .get("MeshRenderer")
                .and_then(|fields| fields.get(crate::capscene::MESH_FIELD)),
            original_handle.as_ref()
        );
        assert_eq!(
            undone
                .get("PipeRecipe")
                .and_then(|fields| fields.get("source")),
            original_source.as_ref()
        );
    }
}
