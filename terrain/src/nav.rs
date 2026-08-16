//! Navigation — a chunked walkability grid with a deterministic A* over it.
//!
//! ## Why a grid and not a navmesh
//!
//! A terrain height field is already a grid, and its walkability is a *local* property: slope, step height,
//! water depth. Extracting a polygon navmesh from it would mean a Recast-style voxelize-and-region pass
//! whose output changes discontinuously when a single sculpt stroke lands — which is exactly the wrong
//! property for a terrain you are editing live. A grid rebuilds per chunk in microseconds, changes locally
//! when the terrain does, and needs no global re-solve.
//!
//! ## Chunked without seams
//!
//! Cells are addressed by **global** integer index, not per chunk, and a chunk's grid stores the cells it
//! owns half-openly. So a path crossing a chunk boundary is not a special case: the search simply looks up
//! the neighbouring cell, finds it in the adjacent chunk's grid, and continues. There is no stitching step
//! to get wrong and no portal graph to maintain.
//!
//! ## Determinism
//!
//! Costs are integers (millimetre-scale fixed point), the frontier is ordered by `(cost, cell key)` so ties
//! break identically everywhere, and every map is a `BTreeMap`. The same query returns the same path on
//! every machine — which it must, because agents following it are part of the simulation.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

use crate::field::Terrain;
use crate::mesh::ChunkSamples;
use crate::recipe::ChunkCoord;

/// What counts as walkable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NavConfig {
    /// Nav cell size as a LOD-style power-of-two step over the terrain's LOD-0 cells. `0` = one nav cell per
    /// terrain cell, `1` = one per 2×2, and so on.
    pub step: u8,
    /// Maximum walkable slope (rise over run).
    pub max_slope: f32,
    /// Maximum height difference an agent can cross between adjacent cells, in metres.
    pub max_step_m: f32,
    /// Deepest water an agent may wade through, in metres. `0` ⇒ water is impassable.
    pub max_wade_m: f32,
}

impl Default for NavConfig {
    fn default() -> Self {
        Self {
            step: 1,
            max_slope: 0.7,
            max_step_m: 0.45,
            max_wade_m: 0.6,
        }
    }
}

/// One chunk's walkability, as a bitset plus a representative height per cell.
#[derive(Clone, Debug, PartialEq)]
pub struct NavGrid {
    /// Which chunk.
    pub coord: ChunkCoord,
    /// Cells per axis.
    pub side: u32,
    /// Cell size in metres.
    pub cell_m: f32,
    /// World XZ of the chunk's minimum corner.
    pub origin: [f32; 2],
    /// One bit per cell, row-major (`side * side` bits, packed into `u64`s).
    pub walkable: Vec<u64>,
    /// Cell-centre heights in metres.
    pub height: Vec<f32>,
}

impl NavGrid {
    /// Heap bytes.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.walkable.len() * 8 + self.height.len() * 4
    }

    /// Whether a local cell is walkable.
    #[must_use]
    pub fn is_walkable(&self, ix: u32, iz: u32) -> bool {
        if ix >= self.side || iz >= self.side {
            return false;
        }
        let bit = (iz * self.side + ix) as usize;
        self.walkable[bit / 64] & (1u64 << (bit % 64)) != 0
    }

    /// Height at a local cell's centre.
    #[must_use]
    pub fn cell_height(&self, ix: u32, iz: u32) -> f32 {
        if ix >= self.side || iz >= self.side {
            return 0.0;
        }
        self.height[(iz * self.side + ix) as usize]
    }

    /// Count of walkable cells — the number a validation panel reports as "walkable area".
    #[must_use]
    pub fn walkable_count(&self) -> u32 {
        self.walkable.iter().map(|w| w.count_ones()).sum()
    }

    fn set_walkable(&mut self, ix: u32, iz: u32) {
        let bit = (iz * self.side + ix) as usize;
        self.walkable[bit / 64] |= 1u64 << (bit % 64);
    }
}

/// Build a chunk's navigation grid from its already-evaluated height block.
#[must_use]
pub fn build_nav_grid(terrain: &Terrain, samples: &ChunkSamples, cfg: &NavConfig) -> NavGrid {
    let terrain_cells = samples.verts - 1;
    let step = 1u32 << u32::from(cfg.step);
    let side = (terrain_cells / step).max(1);
    let cell_m = samples.cell * step as f32;
    let words = ((side * side) as usize).div_ceil(64);
    let mut grid = NavGrid {
        coord: samples.coord,
        side,
        cell_m,
        origin: samples.origin,
        walkable: vec![0u64; words],
        height: vec![0.0; (side * side) as usize],
    };
    for iz in 0..side {
        for ix in 0..side {
            // Cell centre in terrain-grid coordinates.
            let gx = (ix as f32 + 0.5) * step as f32;
            let gz = (iz as f32 + 0.5) * step as f32;
            let h = samples.height_bilinear(gx, gz);
            grid.height[(iz * side + ix) as usize] = h;
            let slope = samples.slope_at_grid(gx, gz);
            if slope > cfg.max_slope {
                continue;
            }
            let (wx, wz) = (
                samples.origin[0] + gx * samples.cell,
                samples.origin[1] + gz * samples.cell,
            );
            if terrain.water_depth(wx, wz) > cfg.max_wade_m {
                continue;
            }
            grid.set_walkable(ix, iz);
        }
    }
    grid
}

/// A read-only view over a set of chunk nav grids, addressed by global cell index.
#[derive(Clone, Copy, Debug)]
pub struct NavWorld<'a> {
    grids: &'a BTreeMap<ChunkCoord, NavGrid>,
    /// Cells per chunk axis (all grids must share this, i.e. one [`NavConfig`] per world).
    side: u32,
    cell_m: f32,
}

impl<'a> NavWorld<'a> {
    /// Wrap a set of grids. Returns `None` when the set is empty or the grids disagree on resolution — a
    /// mixed-resolution nav set would silently produce paths that skip cells.
    #[must_use]
    pub fn new(grids: &'a BTreeMap<ChunkCoord, NavGrid>) -> Option<Self> {
        let first = grids.values().next()?;
        if grids
            .values()
            .any(|g| g.side != first.side || g.cell_m != first.cell_m)
        {
            return None;
        }
        Some(Self {
            grids,
            side: first.side,
            cell_m: first.cell_m,
        })
    }

    /// Nav cell size in metres.
    #[must_use]
    pub fn cell_m(&self) -> f32 {
        self.cell_m
    }

    /// The global cell containing a world position.
    #[must_use]
    pub fn cell_of(&self, wx: f32, wz: f32) -> (i64, i64) {
        (
            (wx / self.cell_m).floor() as i64,
            (wz / self.cell_m).floor() as i64,
        )
    }

    /// World centre of a global cell.
    #[must_use]
    pub fn cell_center(&self, gx: i64, gz: i64) -> [f32; 2] {
        [
            (gx as f32 + 0.5) * self.cell_m,
            (gz as f32 + 0.5) * self.cell_m,
        ]
    }

    /// Walkability and height of a global cell, or `None` when the cell is blocked or not resident.
    #[must_use]
    pub fn cell(&self, gx: i64, gz: i64) -> Option<f32> {
        let side = i64::from(self.side);
        let coord = ChunkCoord::new(gx.div_euclid(side) as i32, gz.div_euclid(side) as i32);
        let g = self.grids.get(&coord)?;
        let lx = gx.rem_euclid(side) as u32;
        let lz = gz.rem_euclid(side) as u32;
        if !g.is_walkable(lx, lz) {
            return None;
        }
        Some(g.cell_height(lx, lz))
    }
}

/// The outcome of a path query.
#[derive(Clone, Debug, PartialEq)]
pub struct PathResult {
    /// Whether a path was found.
    pub found: bool,
    /// World-space waypoints from start to goal, including both, after line-of-sight smoothing.
    pub waypoints: Vec<[f32; 3]>,
    /// Cells expanded — the cost signal a profiler reports.
    pub visited: usize,
    /// Why the query failed, in plain language. `None` on success.
    pub reason: Option<String>,
}

impl PathResult {
    fn failed(visited: usize, reason: impl Into<String>) -> Self {
        Self {
            found: false,
            waypoints: Vec::new(),
            visited,
            reason: Some(reason.into()),
        }
    }

    /// Total path length in metres (0 when not found).
    #[must_use]
    pub fn length_m(&self) -> f32 {
        self.waypoints
            .windows(2)
            .map(|w| {
                let dx = w[1][0] - w[0][0];
                let dy = w[1][1] - w[0][1];
                let dz = w[1][2] - w[0][2];
                (dx * dx + dy * dy + dz * dz).sqrt()
            })
            .sum()
    }
}

/// Fixed-point cost scale: costs are in millimetres, so the frontier orders on integers and the search is
/// bit-deterministic.
const COST_SCALE: f32 = 1000.0;
const DIAGONAL: f32 = std::f32::consts::SQRT_2;

/// A* between two world positions over the resident nav grids.
///
/// `max_visited` bounds the search so an unreachable goal costs a known amount rather than scanning the
/// whole world; exceeding it is reported as a reason, never as a silent "no path".
#[must_use]
pub fn find_path(
    world: &NavWorld<'_>,
    from: [f32; 3],
    to: [f32; 3],
    cfg: &NavConfig,
    max_visited: usize,
) -> PathResult {
    let start = world.cell_of(from[0], from[2]);
    let goal = world.cell_of(to[0], to[2]);
    if world.cell(start.0, start.1).is_none() {
        return PathResult::failed(0, "the start position is not on walkable terrain");
    }
    if world.cell(goal.0, goal.1).is_none() {
        return PathResult::failed(0, "the destination is not on walkable terrain");
    }
    if start == goal {
        let c = world.cell_center(start.0, start.1);
        let h = world.cell(start.0, start.1).unwrap_or(0.0);
        return PathResult {
            found: true,
            waypoints: vec![from, [c[0], h, c[1]]],
            visited: 1,
            reason: None,
        };
    }

    let heuristic = |gx: i64, gz: i64| -> u32 {
        let dx = (gx - goal.0).abs() as f32;
        let dz = (gz - goal.1).abs() as f32;
        let straight = (dx - dz).abs();
        let diag = dx.min(dz);
        ((straight + diag * DIAGONAL) * world.cell_m * COST_SCALE) as u32
    };

    let mut g_score: BTreeMap<(i64, i64), u32> = BTreeMap::new();
    let mut came_from: BTreeMap<(i64, i64), (i64, i64)> = BTreeMap::new();
    // `(f, cell)` — the cell in the key makes ties break identically on every machine.
    let mut open: BinaryHeap<Reverse<(u32, (i64, i64))>> = BinaryHeap::new();
    g_score.insert(start, 0);
    open.push(Reverse((heuristic(start.0, start.1), start)));
    let mut visited = 0usize;

    while let Some(Reverse((_, current))) = open.pop() {
        if current == goal {
            return finish(world, from, to, &came_from, goal, visited);
        }
        visited += 1;
        if visited > max_visited {
            return PathResult::failed(
                visited,
                format!("no path found within {max_visited} cells of search — the destination may be enclosed"),
            );
        }
        let here_g = *g_score.get(&current).unwrap_or(&u32::MAX);
        let here_h = world.cell(current.0, current.1).unwrap_or(0.0);
        for (dx, dz) in [
            (1i64, 0i64),
            (-1, 0),
            (0, 1),
            (0, -1),
            (1, 1),
            (1, -1),
            (-1, 1),
            (-1, -1),
        ] {
            let next = (current.0 + dx, current.1 + dz);
            let Some(next_h) = world.cell(next.0, next.1) else {
                continue;
            };
            let rise = (next_h - here_h).abs();
            if rise > cfg.max_step_m {
                continue;
            }
            // Diagonal moves may not cut a corner between two blocked cells.
            if dx != 0
                && dz != 0
                && (world.cell(current.0 + dx, current.1).is_none()
                    || world.cell(current.0, current.1 + dz).is_none())
            {
                continue;
            }
            let flat = if dx != 0 && dz != 0 {
                world.cell_m * DIAGONAL
            } else {
                world.cell_m
            };
            // Climbing costs more than walking, so paths prefer contours — the behaviour that makes
            // generated routes look deliberate rather than drawn with a ruler.
            let step_cost = ((flat + rise * 2.0) * COST_SCALE) as u32;
            let tentative = here_g.saturating_add(step_cost);
            if tentative < *g_score.get(&next).unwrap_or(&u32::MAX) {
                g_score.insert(next, tentative);
                came_from.insert(next, current);
                open.push(Reverse((
                    tentative.saturating_add(heuristic(next.0, next.1)),
                    next,
                )));
            }
        }
    }
    PathResult::failed(visited, "the destination is not reachable from the start")
}

fn finish(
    world: &NavWorld<'_>,
    from: [f32; 3],
    to: [f32; 3],
    came_from: &BTreeMap<(i64, i64), (i64, i64)>,
    goal: (i64, i64),
    visited: usize,
) -> PathResult {
    let mut cells = vec![goal];
    let mut cur = goal;
    while let Some(&prev) = came_from.get(&cur) {
        cells.push(prev);
        cur = prev;
        if cells.len() > 1_000_000 {
            break;
        }
    }
    cells.reverse();
    let mut pts: Vec<[f32; 3]> = Vec::with_capacity(cells.len() + 2);
    pts.push(from);
    for c in &cells {
        let ctr = world.cell_center(c.0, c.1);
        let h = world.cell(c.0, c.1).unwrap_or(from[1]);
        pts.push([ctr[0], h, ctr[1]]);
    }
    pts.push(to);
    let waypoints = smooth(world, &pts);
    PathResult {
        found: true,
        waypoints,
        visited,
        reason: None,
    }
}

/// String-pulling: drop any waypoint that can be skipped by a straight walkable line.
///
/// Grid A* produces staircases; agents following them look like they are on rails. This is the cheap
/// standard fix and it costs a handful of walkability lookups per retained point.
fn smooth(world: &NavWorld<'_>, pts: &[[f32; 3]]) -> Vec<[f32; 3]> {
    if pts.len() <= 2 {
        return pts.to_vec();
    }
    let mut out = vec![pts[0]];
    let mut anchor = 0usize;
    let mut probe = 1usize;
    while probe < pts.len() {
        if probe == pts.len() - 1 {
            out.push(pts[probe]);
            break;
        }
        if line_walkable(world, pts[anchor], pts[probe + 1]) {
            probe += 1;
        } else {
            out.push(pts[probe]);
            anchor = probe;
            probe += 1;
        }
    }
    out
}

fn line_walkable(world: &NavWorld<'_>, a: [f32; 3], b: [f32; 3]) -> bool {
    let dx = b[0] - a[0];
    let dz = b[2] - a[2];
    let len = (dx * dx + dz * dz).sqrt();
    let steps = ((len / (world.cell_m * 0.5)).ceil() as usize).clamp(1, 4096);
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let (x, z) = (a[0] + dx * t, a[2] + dz * t);
        let (gx, gz) = world.cell_of(x, z);
        if world.cell(gx, gz).is_none() {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::{Layer, LayerKind, Stroke, StrokeKind, TerrainRecipe};
    use std::collections::BTreeMap;

    fn terrain_with(layers: Vec<Layer>, strokes: Vec<Stroke>) -> Terrain {
        let mut r = TerrainRecipe {
            world_size_m: 256.0,
            chunk_size_m: 64.0,
            chunk_verts: 33,
            ..TerrainRecipe::default()
        };
        r.water.sea_level_m = -1000.0;
        r.layers.extend(layers);
        r.strokes = strokes;
        Terrain::compile(r, BTreeMap::new()).expect("compile")
    }

    fn grids(t: &Terrain, cfg: &NavConfig, n: i32) -> BTreeMap<ChunkCoord, NavGrid> {
        let mut m = BTreeMap::new();
        for cz in 0..n {
            for cx in 0..n {
                let c = ChunkCoord::new(cx, cz);
                let s = t.sample_chunk(c).expect("chunk");
                m.insert(c, build_nav_grid(t, &s, cfg));
            }
        }
        m
    }

    #[test]
    fn flat_terrain_is_entirely_walkable() {
        let t = terrain_with(vec![], vec![]);
        let cfg = NavConfig::default();
        let g = build_nav_grid(&t, &t.sample_chunk(ChunkCoord::new(0, 0)).unwrap(), &cfg);
        assert_eq!(g.side, 16, "33 verts at step 1 = 16 cells");
        assert_eq!(g.walkable_count(), 16 * 16);
    }

    #[test]
    fn steep_ground_is_not_walkable() {
        let t = terrain_with(
            vec![Layer::new(
                "Cliffs",
                LayerKind::Ridged {
                    amplitude: 200.0,
                    wavelength_m: 40.0,
                    octaves: 5,
                    lacunarity: 2.0,
                    gain: 0.6,
                    warp_m: 0.0,
                    warp_wavelength_m: 1.0,
                },
            )],
            vec![],
        );
        let cfg = NavConfig::default();
        let g = build_nav_grid(&t, &t.sample_chunk(ChunkCoord::new(1, 1)).unwrap(), &cfg);
        let total = g.side * g.side;
        assert!(
            g.walkable_count() < total,
            "a cliff field cannot be fully walkable"
        );
        assert!(g.walkable_count() > 0, "and cannot be fully blocked either");
    }

    #[test]
    fn deep_water_blocks_and_shallow_water_does_not() {
        let mut r = TerrainRecipe {
            world_size_m: 128.0,
            chunk_size_m: 64.0,
            chunk_verts: 33,
            ..TerrainRecipe::default()
        };
        r.water.sea_level_m = 0.3;
        let shallow = Terrain::compile(r.clone(), BTreeMap::new()).expect("compile");
        let cfg = NavConfig::default();
        let g = build_nav_grid(
            &shallow,
            &shallow.sample_chunk(ChunkCoord::new(0, 0)).unwrap(),
            &cfg,
        );
        assert_eq!(
            g.walkable_count(),
            g.side * g.side,
            "0.3 m is waist-deep at most"
        );

        r.water.sea_level_m = 4.0;
        let deep = Terrain::compile(r, BTreeMap::new()).expect("compile");
        let g2 = build_nav_grid(
            &deep,
            &deep.sample_chunk(ChunkCoord::new(0, 0)).unwrap(),
            &cfg,
        );
        assert_eq!(g2.walkable_count(), 0, "4 m of water is not waded");
    }

    #[test]
    fn a_path_crosses_chunk_boundaries_without_stitching() {
        let t = terrain_with(vec![], vec![]);
        let cfg = NavConfig::default();
        let gs = grids(&t, &cfg, 4);
        let world = NavWorld::new(&gs).expect("uniform grids");
        let r = find_path(&world, [8.0, 0.0, 8.0], [240.0, 0.0, 240.0], &cfg, 200_000);
        assert!(r.found, "no path: {:?}", r.reason);
        // On flat ground, smoothing should reduce a 200 m diagonal to almost a straight line.
        assert!(
            r.waypoints.len() <= 6,
            "not smoothed: {} points",
            r.waypoints.len()
        );
        let direct = ((240.0f32 - 8.0).powi(2) * 2.0).sqrt();
        assert!(
            r.length_m() < direct * 1.15,
            "path is {} m against a {direct} m straight line",
            r.length_m()
        );
    }

    #[test]
    fn a_path_routes_around_an_impassable_ridge() {
        // A wall across most of the world, with a gap: the path must find the gap.
        let mut strokes = Vec::new();
        for i in 0..40 {
            let z = 100.0;
            let x = 4.0 + i as f32 * 4.0;
            if x > 150.0 && x < 175.0 {
                continue; // the gap
            }
            strokes.push(Stroke {
                kind: StrokeKind::Raise,
                x,
                z,
                radius_m: 6.0,
                strength: 60.0,
                hardness: 0.0,
                target_m: 0.0,
            });
        }
        let t = terrain_with(vec![], strokes);
        let cfg = NavConfig::default();
        let gs = grids(&t, &cfg, 4);
        let world = NavWorld::new(&gs).expect("grids");
        let r = find_path(&world, [20.0, 0.0, 40.0], [20.0, 0.0, 200.0], &cfg, 400_000);
        assert!(r.found, "no route through the gap: {:?}", r.reason);
        // It must have gone via the gap, so the path is much longer than the straight line.
        assert!(
            r.length_m() > 200.0,
            "path suspiciously short ({} m) — did it walk through the wall?",
            r.length_m()
        );
        assert!(
            r.waypoints.iter().any(|p| p[0] > 140.0),
            "the route never reached the gap"
        );
    }

    #[test]
    fn an_unreachable_goal_is_explained_not_silently_empty() {
        let t = terrain_with(vec![], vec![]);
        let cfg = NavConfig::default();
        let gs = grids(&t, &cfg, 2);
        let world = NavWorld::new(&gs).expect("grids");
        // Off the resident set entirely.
        let r = find_path(&world, [8.0, 0.0, 8.0], [900.0, 0.0, 900.0], &cfg, 10_000);
        assert!(!r.found);
        let reason = r.reason.expect("must explain");
        assert!(reason.contains("destination"), "unhelpful reason: {reason}");
    }

    #[test]
    fn the_search_is_bounded_and_says_so() {
        let t = terrain_with(vec![], vec![]);
        let cfg = NavConfig::default();
        let gs = grids(&t, &cfg, 4);
        let world = NavWorld::new(&gs).expect("grids");
        let r = find_path(&world, [8.0, 0.0, 8.0], [240.0, 0.0, 240.0], &cfg, 10);
        assert!(!r.found);
        assert!(r.visited <= 12);
        assert!(r.reason.unwrap().contains("within 10 cells"));
    }

    #[test]
    fn paths_are_bit_reproducible() {
        let t = terrain_with(
            vec![Layer::new(
                "Rolling",
                LayerKind::Fbm {
                    amplitude: 12.0,
                    wavelength_m: 120.0,
                    octaves: 4,
                    lacunarity: 2.0,
                    gain: 0.5,
                    warp_m: 0.0,
                    warp_wavelength_m: 1.0,
                },
            )],
            vec![],
        );
        let cfg = NavConfig::default();
        let gs = grids(&t, &cfg, 4);
        let world = NavWorld::new(&gs).expect("grids");
        let a = find_path(
            &world,
            [10.0, 0.0, 10.0],
            [230.0, 0.0, 190.0],
            &cfg,
            200_000,
        );
        let b = find_path(
            &world,
            [10.0, 0.0, 10.0],
            [230.0, 0.0, 190.0],
            &cfg,
            200_000,
        );
        assert_eq!(a, b);
    }

    #[test]
    fn mixed_resolution_grids_are_refused_rather_than_silently_wrong() {
        let t = terrain_with(vec![], vec![]);
        let fine = NavConfig {
            step: 0,
            ..NavConfig::default()
        };
        let coarse = NavConfig {
            step: 2,
            ..NavConfig::default()
        };
        let mut m = BTreeMap::new();
        let a = ChunkCoord::new(0, 0);
        let b = ChunkCoord::new(1, 0);
        m.insert(a, build_nav_grid(&t, &t.sample_chunk(a).unwrap(), &fine));
        m.insert(b, build_nav_grid(&t, &t.sample_chunk(b).unwrap(), &coarse));
        assert!(NavWorld::new(&m).is_none());
    }

    #[test]
    fn diagonal_moves_cannot_squeeze_between_two_blocked_cells() {
        // Built by hand rather than sculpted, so the geometry is exactly the case under test: two diagonally
        // adjacent walkable cells whose shared orthogonal neighbours are both blocked. Cutting that corner
        // is the classic grid-pathing bug — an agent slipping through the gap between two boulders.
        let side = 4u32;
        let mut g = NavGrid {
            coord: ChunkCoord::new(0, 0),
            side,
            cell_m: 1.0,
            origin: [0.0, 0.0],
            walkable: vec![0u64; 1],
            height: vec![0.0; (side * side) as usize],
        };
        g.set_walkable(0, 0);
        g.set_walkable(1, 1);
        let mut gs = BTreeMap::new();
        gs.insert(ChunkCoord::new(0, 0), g);
        let world = NavWorld::new(&gs).expect("grid");
        let cfg = NavConfig::default();
        let r = find_path(&world, [0.5, 0.0, 0.5], [1.5, 0.0, 1.5], &cfg, 1_000);
        assert!(
            !r.found,
            "the only route is a corner cut between two blocked cells, which must be refused"
        );

        // Opening one of the two orthogonal neighbours makes the move legal again, which proves the refusal
        // above came from the corner rule and not from something else being wrong.
        let mut open = gs.remove(&ChunkCoord::new(0, 0)).expect("grid");
        open.set_walkable(1, 0);
        let mut gs2 = BTreeMap::new();
        gs2.insert(ChunkCoord::new(0, 0), open);
        let world2 = NavWorld::new(&gs2).expect("grid");
        let r2 = find_path(&world2, [0.5, 0.0, 0.5], [1.5, 0.0, 1.5], &cfg, 1_000);
        assert!(
            r2.found,
            "should route via the opened cell: {:?}",
            r2.reason
        );
    }
}
