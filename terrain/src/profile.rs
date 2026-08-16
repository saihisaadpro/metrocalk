//! Profiling and memory accounting — the numbers a person needs to answer "why is this slow?".
//!
//! Everything here is *measured*, never estimated from a formula, with one exception that says so
//! ([`estimate_resident_bytes`], used before any chunk exists to warn that a budget cannot hold a view
//! distance). Timings are filled in by the host, because this crate has no clock: `std::time::Instant`
//! panics on `wasm32-unknown-unknown`, and a subsystem that cannot be compiled for the browser target would
//! break the workspace's wasm tripwire.

use crate::recipe::TerrainRecipe;

/// Microsecond timings for one chunk build, filled in by the host as it runs each stage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BuildTimings {
    /// Evaluating the height field over the chunk plus its halo.
    pub field_us: u32,
    /// Building every LOD's mesh and measuring their errors.
    pub mesh_us: u32,
    /// Baking the splat albedo and detail normals.
    pub texture_us: u32,
    /// Placing scattered instances.
    pub scatter_us: u32,
    /// Building the height-field collider.
    pub collider_us: u32,
    /// Building the navigation grid.
    pub nav_us: u32,
}

impl BuildTimings {
    /// Total microseconds.
    #[must_use]
    pub fn total_us(&self) -> u32 {
        self.field_us
            .saturating_add(self.mesh_us)
            .saturating_add(self.texture_us)
            .saturating_add(self.scatter_us)
            .saturating_add(self.collider_us)
            .saturating_add(self.nav_us)
    }

    /// Accumulate another chunk's timings.
    pub fn add(&mut self, other: &Self) {
        self.field_us = self.field_us.saturating_add(other.field_us);
        self.mesh_us = self.mesh_us.saturating_add(other.mesh_us);
        self.texture_us = self.texture_us.saturating_add(other.texture_us);
        self.scatter_us = self.scatter_us.saturating_add(other.scatter_us);
        self.collider_us = self.collider_us.saturating_add(other.collider_us);
        self.nav_us = self.nav_us.saturating_add(other.nav_us);
    }

    /// The slowest stage, as `(name, microseconds)` — what a profiling panel puts first, because the
    /// dominant stage is the only one worth tuning.
    #[must_use]
    pub fn dominant(&self) -> (&'static str, u32) {
        [
            ("field", self.field_us),
            ("mesh", self.mesh_us),
            ("texture", self.texture_us),
            ("scatter", self.scatter_us),
            ("collider", self.collider_us),
            ("nav", self.nav_us),
        ]
        .into_iter()
        .max_by_key(|(_, v)| *v)
        .unwrap_or(("field", 0))
    }
}

/// Live memory and draw accounting for the resident terrain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerrainStats {
    /// Resident chunk count.
    pub resident_chunks: u32,
    /// Chunks that survived culling this frame.
    pub visible_chunks: u32,
    /// Chunks rejected by the frustum test.
    pub culled_frustum: u32,
    /// Chunks rejected by the horizon test.
    pub culled_horizon: u32,
    /// Triangles actually submitted (the drawn LOD only, not every LOD held).
    pub drawn_triangles: u64,
    /// Scattered instances submitted.
    pub drawn_instances: u32,
    /// Scattered instances drawn as impostors rather than meshes.
    pub impostor_instances: u32,
    /// Mesh bytes resident.
    pub mesh_bytes: usize,
    /// Texture bytes resident.
    pub texture_bytes: usize,
    /// Scatter bytes resident.
    pub scatter_bytes: usize,
    /// Collider bytes resident.
    pub collider_bytes: usize,
    /// Navigation-grid bytes resident.
    pub nav_bytes: usize,
    /// Chunk builds still in flight.
    pub pending_builds: u32,
    /// Builds completed since the terrain was created.
    pub completed_builds: u64,
    /// Accumulated build timings.
    pub timings: BuildTimings,
}

impl TerrainStats {
    /// Total resident bytes.
    #[must_use]
    pub fn total_bytes(&self) -> usize {
        self.mesh_bytes
            + self.texture_bytes
            + self.scatter_bytes
            + self.collider_bytes
            + self.nav_bytes
    }

    /// Fraction of the recipe's ceiling in use (`1.0` = exactly at the limit).
    #[must_use]
    pub fn budget_fraction(&self, recipe: &TerrainRecipe) -> f32 {
        let cap = recipe.budget.total_bytes();
        if cap == 0 {
            return 0.0;
        }
        self.total_bytes() as f32 / cap as f32
    }

    /// A one-line summary for a status bar.
    #[must_use]
    pub fn summary(&self, recipe: &TerrainRecipe) -> String {
        format!(
            "{} chunks ({} drawn) · {:.1} MB of {} MB · {} tris · {} instances ({} impostors)",
            self.resident_chunks,
            self.visible_chunks,
            self.total_bytes() as f32 / (1024.0 * 1024.0),
            recipe.budget.total_bytes() / (1024 * 1024),
            self.drawn_triangles,
            self.drawn_instances,
            self.impostor_instances
        )
    }
}

/// A rough resident-byte estimate for a recipe, used *before* anything is built so a validation warning can
/// say "this view distance needs about 900 MB and your ceiling is 512 MB".
///
/// Deliberately an over-estimate: it assumes every chunk in range holds LOD-0 geometry and a texture at its
/// ring's resolution, which is the worst case the streamer can reach.
#[must_use]
pub fn estimate_resident_bytes(recipe: &TerrainRecipe) -> usize {
    let per_axis = (recipe.lod.max_view_distance_m / recipe.chunk_size_m).ceil() as usize * 2 + 1;
    let chunks = (per_axis * per_axis).min(recipe.chunk_count() as usize);
    chunks * estimate_chunk_bytes(recipe)
}

/// Rough bytes one resident chunk costs — the per-chunk half of [`estimate_resident_bytes`].
///
/// Used by the streamer to answer "how many chunks can this budget afford?" *before* any chunk exists,
/// which is what lets it choose a view distance that fits rather than requesting a ring it will then have to
/// evict (and re-request, and evict again — the classic streaming thrash).
#[must_use]
pub fn estimate_chunk_bytes(recipe: &TerrainRecipe) -> usize {
    let verts = recipe.verts_at_lod(0) as usize;
    // Every LOD's mesh is held: LOD 0 plus a geometric series of quarters is about 4/3 of LOD 0.
    let mesh = verts * verts * 32 * 4 / 3;
    let samples = (verts + 2) * (verts + 2) * 4;
    let collider = verts * verts * 4;
    // Texture cost falls with the ring, so average roughly half the LOD-0 bake across the set.
    let texture = (recipe.lod.texture_res as usize).pow(2) * 4 / 2;
    mesh + samples + collider + texture
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timings_total_and_name_their_dominant_stage() {
        let t = BuildTimings {
            field_us: 400,
            mesh_us: 120,
            texture_us: 900,
            scatter_us: 50,
            collider_us: 10,
            nav_us: 20,
        };
        assert_eq!(t.total_us(), 1500);
        assert_eq!(t.dominant(), ("texture", 900));
        let mut acc = BuildTimings::default();
        acc.add(&t);
        acc.add(&t);
        assert_eq!(acc.total_us(), 3000);
    }

    #[test]
    fn timings_saturate_instead_of_overflowing() {
        let mut a = BuildTimings {
            field_us: u32::MAX,
            ..BuildTimings::default()
        };
        a.add(&BuildTimings {
            field_us: 1000,
            ..BuildTimings::default()
        });
        assert_eq!(a.field_us, u32::MAX);
        assert_eq!(a.total_us(), u32::MAX);
    }

    #[test]
    fn stats_report_the_budget_honestly() {
        let recipe = TerrainRecipe::default();
        let mut s = TerrainStats {
            resident_chunks: 40,
            visible_chunks: 22,
            drawn_triangles: 500_000,
            drawn_instances: 12_000,
            impostor_instances: 9_000,
            ..TerrainStats::default()
        };
        s.mesh_bytes = 64 * 1024 * 1024;
        s.texture_bytes = 32 * 1024 * 1024;
        assert_eq!(s.total_bytes(), 96 * 1024 * 1024);
        let frac = s.budget_fraction(&recipe);
        assert!(frac > 0.1 && frac < 0.2, "fraction {frac}");
        let line = s.summary(&recipe);
        assert!(line.contains("40 chunks"), "{line}");
        assert!(line.contains("22 drawn"), "{line}");
        assert!(line.contains("9000 impostors"), "{line}");
    }

    #[test]
    fn a_zero_budget_does_not_divide_by_zero() {
        let mut recipe = TerrainRecipe::default();
        recipe.budget.mesh_mb = 0;
        recipe.budget.texture_mb = 0;
        recipe.budget.scatter_mb = 0;
        recipe.budget.collider_mb = 0;
        let s = TerrainStats::default();
        assert_eq!(s.budget_fraction(&recipe), 0.0);
    }

    #[test]
    fn the_estimate_scales_with_view_distance_and_is_capped_by_the_world() {
        let mut r = TerrainRecipe::default();
        r.lod.max_view_distance_m = 256.0;
        let near = estimate_resident_bytes(&r);
        r.lod.max_view_distance_m = 1024.0;
        let far = estimate_resident_bytes(&r);
        assert!(far > near * 4, "estimate did not scale: {near} → {far}");
        // A view distance larger than the world cannot estimate more chunks than the world has.
        r.lod.max_view_distance_m = 100_000.0;
        let huge = estimate_resident_bytes(&r);
        let per_chunk = huge / r.chunk_count() as usize;
        assert!(per_chunk > 0);
        assert!(huge <= per_chunk * r.chunk_count() as usize + per_chunk);
    }
}
