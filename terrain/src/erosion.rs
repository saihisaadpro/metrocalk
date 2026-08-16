//! Erosion — the one stage that cannot be pointwise, so it is a **bake**.
//!
//! A water droplet carries sediment across cells: what happens at a point depends on what happened
//! upstream. That is irreducibly neighbourhood-coupled, so erosion is computed once into a world-aligned
//! [`HeightGrid`] of height *deltas*, and the field then samples that grid pointwise like any other layer.
//! Everything downstream — chunk meshes at any LOD, colliders, nav, scatter — stays seamless and
//! resolution-independent, because they only ever see the sampled result.
//!
//! ## Order independence, by construction
//!
//! The obvious implementation — walk droplets one at a time, each mutating the height the next one reads —
//! is *order dependent*, and order dependence is fatal here: it makes the bake impossible to parallelize,
//! impossible to tile, and impossible to reproduce unless the whole world is processed in one fixed
//! sequence. So this is formulated in **epochs**:
//!
//! 1. Every droplet in an epoch reads the *same frozen* height grid.
//! 2. Each accumulates its erosion and deposition into a separate delta buffer.
//! 3. At the end of the epoch the deltas are applied, and the next epoch reads the new frozen grid.
//!
//! Within an epoch, droplets are independent, so the result does not depend on their order and the work
//! divides across any number of tiles or threads. Across epochs, channels still deepen and feed back on
//! themselves — which is what actually produces drainage networks — because each epoch sees the previous
//! epoch's carved terrain. Eight epochs recovers the visual character of the sequential algorithm while
//! keeping the determinism guarantee that everything else in this crate depends on.
//!
//! Thermal erosion (slumping to a talus angle) is double-buffered for the same reason.
//!
//! ## Cost, honestly
//!
//! The bake grid is `world / cell_m` cells square, and its resolution is a *deliberate* knob: erosion
//! shapes ridge lines and valleys, which are tens of metres across, so 2–4 m per cell is the right scale.
//! Fine detail keeps coming from the noise layers at full resolution, for free, at every LOD. A 4 km world
//! at 3 m per cell is a 1.8 M cell bake — a fraction of a second — where the same world at 0.5 m would be
//! 67 M cells and pointless.
//!
//! References: Musgrave, Kolb & Mace, *The Synthesis and Rendering of Eroded Fractal Terrains* (1989) for
//! the thermal/talus model; the particle-transport formulation follows the widely used droplet scheme
//! (Beyer 2015; Jákó 2011) with the epoch reformulation above.

use crate::grid::HeightGrid;
use crate::noise;
use crate::recipe::{ErosionSettings, TerrainRecipe};

/// Epochs the droplet budget is divided into. More epochs ⇒ more feedback (deeper, more connected
/// channels) at proportionally more synchronization; eight is where the character stops changing much.
pub const EPOCHS: u32 = 8;

/// Largest bake grid this module will allocate, in cells. A recipe asking for more gets a coarser cell size
/// (and [`crate::validate`] says so) instead of a multi-gigabyte allocation.
pub const MAX_BAKE_CELLS: usize = 16_777_216;

/// Floor on the slope term in the carry-capacity formula, so a droplet on perfectly flat ground still has a
/// small capacity and deposits rather than stalling.
const MIN_SLOPE: f32 = 0.02;

/// Ceiling on droplet speed.
///
/// Without it, `speed² += slope · gravity` compounds over a long descent, capacity grows with speed, and a
/// single droplet crossing a 600 m mountain face arrives in the valley carrying an implausible load. This is
/// the difference between erosion that carves and erosion that produces craters.
const MAX_SPEED: f32 = 6.0;

/// The largest fraction of a step's own drop a droplet may erode.
///
/// Eroding the full drop lets a channel dig through its own bed within one step, which reads as a trench of
/// single-cell pits rather than a valley.
const MAX_ERODE_FRACTION: f32 = 0.5;

/// The content key for an erosion layer's bake: a digest of the seed, the world extent, and every layer up
/// to and including this one.
///
/// Editing anything *above* the erosion layer — or a stroke, or a road — does not change this key, so the
/// bake is reused and the edit is instant. That is the difference between an erosion-heavy terrain that is
/// pleasant to author and one that stalls for a second on every slider drag.
#[must_use]
pub fn bake_key(recipe: &TerrainRecipe, layer_index: usize) -> String {
    let upto = (layer_index + 1).min(recipe.layers.len());
    let payload = (
        recipe.seed,
        recipe.world_size_m,
        recipe.height_clamp,
        &recipe.layers[..upto],
    );
    let bytes = serde_json::to_vec(&payload).unwrap_or_default();
    format!(
        "erosion:{layer_index}:{:016x}",
        crate::recipe::stable_digest(&bytes)
    )
}

/// The cell size a bake will actually use for this world — the requested one, coarsened if it would exceed
/// [`MAX_BAKE_CELLS`].
#[must_use]
pub fn effective_cell(world_size_m: f32, requested_cell_m: f32) -> f32 {
    let mut cell = requested_cell_m.max(0.1);
    loop {
        let n = (world_size_m / cell).ceil() as usize + 1;
        if n.saturating_mul(n) <= MAX_BAKE_CELLS {
            return cell;
        }
        cell *= 2.0;
    }
}

/// Bake erosion over the whole world.
///
/// `base_height` is the field *below* the erosion layer. The result is a grid of height **deltas** (eroded
/// minus original), so the layer's `amplitude` can dial the effect from full to none without re-baking, and
/// so a delta of exactly zero is recognisably "erosion did nothing here".
pub fn bake(
    recipe: &TerrainRecipe,
    settings: &ErosionSettings,
    base_height: &impl Fn(f32, f32) -> f32,
) -> HeightGrid {
    let cell = effective_cell(recipe.world_size_m, settings.cell_m);
    let n = (recipe.world_size_m / cell).ceil() as u32 + 1;
    let base = HeightGrid::from_fn([0.0, 0.0], cell, n, n, base_height);
    let mut current = base.clone();

    let kernel = DiscKernel::new(settings.radius_cells);
    let droplets_per_epoch = settings.droplets_per_cell / EPOCHS as f32;
    for epoch in 0..EPOCHS {
        let mut delta = vec![0.0f32; current.data.len()];
        for gz in 0..n {
            for gx in 0..n {
                let spawns = spawn_count(gx, gz, epoch, droplets_per_epoch, recipe.seed);
                for k in 0..spawns {
                    // Jitter inside the cell so droplets do not all start on lattice points, which would
                    // print the bake grid into the terrain as a visible cross-hatch.
                    let jx = noise::hash_unit_3(
                        gx as i32,
                        gz as i32,
                        (epoch * 64 + k) as i32,
                        recipe.seed,
                    );
                    let jz = noise::hash_unit_3(
                        gz as i32,
                        gx as i32,
                        (epoch * 64 + k) as i32 + 4096,
                        recipe.seed ^ 0x51ED_2701,
                    );
                    simulate_droplet(
                        &current,
                        &mut delta,
                        gx as f32 + jx,
                        gz as f32 + jz,
                        settings,
                        &kernel,
                    );
                }
            }
        }
        for (h, d) in current.data.iter_mut().zip(&delta) {
            *h += *d;
        }
        thermal_pass(&mut current, settings, cell);
    }

    let mut out = HeightGrid::new([0.0, 0.0], cell, n, n);
    for (o, (eroded, original)) in out
        .data
        .iter_mut()
        .zip(current.data.iter().zip(base.data.iter()))
    {
        *o = eroded - original;
    }
    out
}

/// How many droplets this cell spawns this epoch — a deterministic function of the *world* cell, so the
/// answer is the same whichever tile or thread asks.
fn spawn_count(gx: u32, gz: u32, epoch: u32, per_epoch: f32, seed: u64) -> u32 {
    if per_epoch <= 0.0 {
        return 0;
    }
    let whole = per_epoch.floor() as u32;
    let frac = per_epoch - whole as f32;
    let extra = u32::from(
        noise::hash_unit_3(gx as i32, gz as i32, epoch as i32, seed ^ 0xD1CE_5EED) < frac,
    );
    (whole + extra).min(64)
}

/// Bilinear height and gradient of a grid at a fractional cell position.
fn height_and_gradient(g: &HeightGrid, px: f32, pz: f32) -> (f32, f32, f32) {
    let ix = px.floor() as i64;
    let iz = pz.floor() as i64;
    let tx = px - ix as f32;
    let tz = pz - iz as f32;
    let h00 = g.get(ix, iz);
    let h10 = g.get(ix + 1, iz);
    let h01 = g.get(ix, iz + 1);
    let h11 = g.get(ix + 1, iz + 1);
    let h = h00 * (1.0 - tx) * (1.0 - tz)
        + h10 * tx * (1.0 - tz)
        + h01 * (1.0 - tx) * tz
        + h11 * tx * tz;
    // Gradient in *cell* units, which is what the droplet step works in.
    let gx = (h10 - h00) * (1.0 - tz) + (h11 - h01) * tz;
    let gz = (h01 - h00) * (1.0 - tx) + (h11 - h10) * tx;
    (h, gx, gz)
}

/// Add a value at a fractional position with bilinear weights.
fn deposit_bilinear(delta: &mut [f32], w: u32, h: u32, px: f32, pz: f32, amount: f32) {
    let ix = px.floor() as i64;
    let iz = pz.floor() as i64;
    let tx = px - ix as f32;
    let tz = pz - iz as f32;
    let mut put = |x: i64, z: i64, k: f32| {
        if x < 0 || z < 0 || x >= i64::from(w) || z >= i64::from(h) {
            return;
        }
        delta[z as usize * w as usize + x as usize] += amount * k;
    };
    put(ix, iz, (1.0 - tx) * (1.0 - tz));
    put(ix + 1, iz, tx * (1.0 - tz));
    put(ix, iz + 1, (1.0 - tx) * tz);
    put(ix + 1, iz + 1, tx * tz);
}

/// The erosion brush: cell offsets and their weights, built once per bake.
///
/// This used to be recomputed inside every erosion step — two passes over a 5×5 disc with a `sqrt` per cell,
/// millions of times per bake. Hoisting it out is the single largest cost in the bake and buys most of the
/// difference between an eighteen-second alpine world and a four-second one.
struct DiscKernel {
    /// `(dx, dz, weight)`, weights summing to 1 for a fully interior disc.
    taps: Vec<(i64, i64, f32)>,
    /// The disc radius in cells, so the interior fast path can be taken without scanning the taps.
    radius: u32,
}

impl DiscKernel {
    fn new(radius: u32) -> Self {
        if radius == 0 {
            return Self {
                taps: vec![(0, 0, 1.0)],
                radius: 0,
            };
        }
        let r = i64::from(radius);
        let mut taps = Vec::with_capacity(((2 * r + 1) * (2 * r + 1)) as usize);
        let mut total = 0.0f32;
        for dz in -r..=r {
            for dx in -r..=r {
                let d = ((dx * dx + dz * dz) as f32).sqrt();
                let k = (1.0 - d / (radius as f32 + 1.0)).max(0.0);
                if k > 0.0 {
                    taps.push((dx, dz, k));
                    total += k;
                }
            }
        }
        if total > 0.0 {
            for t in &mut taps {
                t.2 /= total;
            }
        }
        Self { taps, radius }
    }
}

/// Remove a value spread over the disc, so erosion cuts a channel rather than punching single-cell pits.
///
/// Taps that fall off the grid are dropped and the rest renormalized, so the removed volume stays equal to
/// `amount` at the world edge instead of quietly shrinking there.
fn erode_disc(
    delta: &mut [f32],
    w: u32,
    h: u32,
    px: f32,
    pz: f32,
    amount: f32,
    kernel: &DiscKernel,
) {
    let cx = px.floor() as i64;
    let cz = pz.floor() as i64;
    let (wi, hi) = (i64::from(w), i64::from(h));
    let inside = |x: i64, z: i64| x >= 0 && z >= 0 && x < wi && z < hi;
    // Fast path: a disc wholly inside the grid keeps every tap, so the pre-normalized weights are already
    // correct and the bounds sweep can be skipped entirely. This is the overwhelmingly common case and it is
    // the inner loop of the whole bake.
    let r = i64::from(kernel.radius);
    if cx - r >= 0 && cz - r >= 0 && cx + r < wi && cz + r < hi {
        for (dx, dz, k) in &kernel.taps {
            let idx = (cz + dz) as usize * w as usize + (cx + dx) as usize;
            delta[idx] -= amount * k;
        }
        return;
    }
    let mut kept = 0.0f32;
    for (dx, dz, k) in &kernel.taps {
        if inside(cx + dx, cz + dz) {
            kept += *k;
        }
    }
    if kept <= 0.0 {
        return;
    }
    let scale = amount / kept;
    for (dx, dz, k) in &kernel.taps {
        let (x, z) = (cx + dx, cz + dz);
        if inside(x, z) {
            delta[z as usize * w as usize + x as usize] -= scale * k;
        }
    }
}

/// Walk one droplet downhill across the frozen grid, accumulating into `delta`.
fn simulate_droplet(
    grid: &HeightGrid,
    delta: &mut [f32],
    start_x: f32,
    start_z: f32,
    s: &ErosionSettings,
    kernel: &DiscKernel,
) {
    let (w, h) = (grid.width, grid.height);
    let mut px = start_x;
    let mut pz = start_z;
    let mut dx = 0.0f32;
    let mut dz = 0.0f32;
    let mut speed = 1.0f32;
    let mut water = 1.0f32;
    let mut sediment = 0.0f32;
    let inertia = s.inertia.clamp(0.0, 0.99);

    for _ in 0..s.max_steps {
        let (height, gx, gz) = height_and_gradient(grid, px, pz);
        // Steer: keep some of the previous direction, add the downhill gradient.
        dx = dx * inertia - gx * (1.0 - inertia);
        dz = dz * inertia - gz * (1.0 - inertia);
        let len = (dx * dx + dz * dz).sqrt();
        if len < 1e-6 {
            // Stalled in a pit: drop what is carried and stop. This is what fills basins with sediment.
            deposit_bilinear(delta, w, h, px, pz, sediment);
            return;
        }
        dx /= len;
        dz /= len;
        let nx = px + dx;
        let nz = pz + dz;
        if nx < 0.0 || nz < 0.0 || nx > (w - 1) as f32 || nz > (h - 1) as f32 {
            // Ran off the world: the sediment leaves with it (a river mouth), which is correct and also
            // prevents a rim of deposition around the world edge.
            return;
        }
        let (new_height, _, _) = height_and_gradient(grid, nx, nz);
        let dh = new_height - height;
        // The DIMENSIONLESS slope, not the raw metre drop. This distinction is load-bearing: the published
        // droplet formulations assume a heightmap normalized to roughly [0, 1] over its grid, where one
        // step's drop is a few thousandths. Feeding metres straight into a capacity of
        // `factor · drop · speed · water` scales the carried load by the world's height range, which on a
        // 600 m mountain produces deposition hundreds of metres deep. Dividing by the cell size makes
        // capacity depend on the *shape* of the terrain rather than on the units it is measured in, so one
        // set of settings behaves the same on a 20 m dune field and a 2 km massif.
        let slope = -dh / grid.cell.max(1e-3);

        if dh > 0.0 {
            // Uphill: fill the dip in front, up to what is carried.
            let drop = sediment.min(dh);
            deposit_bilinear(delta, w, h, px, pz, drop);
            sediment -= drop;
        } else {
            // Capacity in metres of sediment.
            let capacity = slope.max(MIN_SLOPE) * speed * water * s.capacity;
            if sediment > capacity {
                let drop = (sediment - capacity) * s.deposition;
                deposit_bilinear(delta, w, h, px, pz, drop);
                sediment -= drop;
            } else {
                let take = ((capacity - sediment) * s.erosion_rate).min(-dh * MAX_ERODE_FRACTION);
                erode_disc(delta, w, h, px, pz, take, kernel);
                sediment += take;
            }
        }

        // Downhill speeds the droplet up; uphill slows it. Capped, or a long descent compounds.
        speed = (speed * speed + slope * s.gravity)
            .max(0.0)
            .sqrt()
            .min(MAX_SPEED);
        water *= 1.0 - s.evaporation.clamp(0.0, 1.0);
        if water < 0.01 {
            deposit_bilinear(delta, w, h, nx, nz, sediment);
            return;
        }
        px = nx;
        pz = nz;
    }
    // Out of lifetime: whatever is still carried settles here.
    deposit_bilinear(delta, w, h, px, pz, sediment);
}

/// One thermal-erosion sweep: material on slopes steeper than the talus angle slumps to lower neighbours.
///
/// Double-buffered, so the result is independent of traversal order — the same property the droplet epochs
/// provide, for the same reason.
fn thermal_pass(grid: &mut HeightGrid, s: &ErosionSettings, cell: f32) {
    if s.thermal_iterations == 0 || s.talus_slope <= 0.0 {
        return;
    }
    let (w, h) = (i64::from(grid.width), i64::from(grid.height));
    // Maximum stable height difference between 4-neighbours at this cell size.
    let max_step = s.talus_slope * cell;
    for _ in 0..s.thermal_iterations {
        let mut delta = vec![0.0f32; grid.data.len()];
        for z in 0..h {
            for x in 0..w {
                let here = grid.get(x, z);
                // Total excess towards the four neighbours, then move a stable fraction of it.
                let mut excess = [0.0f32; 4];
                let mut total = 0.0f32;
                for (k, (nx, nz)) in [(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)]
                    .into_iter()
                    .enumerate()
                {
                    if nx < 0 || nz < 0 || nx >= w || nz >= h {
                        continue;
                    }
                    let d = here - grid.get(nx, nz) - max_step;
                    if d > 0.0 {
                        excess[k] = d;
                        total += d;
                    }
                }
                if total <= 0.0 {
                    continue;
                }
                // Half the excess per sweep keeps the explicit scheme stable (a full move oscillates).
                let move_total = total * 0.25;
                let idx = (z * w + x) as usize;
                delta[idx] -= move_total;
                for (k, (nx, nz)) in [(x + 1, z), (x - 1, z), (x, z + 1), (x, z - 1)]
                    .into_iter()
                    .enumerate()
                {
                    if excess[k] <= 0.0 || nx < 0 || nz < 0 || nx >= w || nz >= h {
                        continue;
                    }
                    delta[(nz * w + nx) as usize] += move_total * excess[k] / total;
                }
            }
        }
        for (v, d) in grid.data.iter_mut().zip(&delta) {
            *v += *d;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::{Layer, LayerKind, TerrainRecipe};

    fn small_world() -> TerrainRecipe {
        TerrainRecipe {
            world_size_m: 256.0,
            chunk_size_m: 64.0,
            chunk_verts: 33,
            ..TerrainRecipe::default()
        }
    }

    fn mountain() -> impl Fn(f32, f32) -> f32 {
        // A ridged field with real slopes for water to run down.
        |x: f32, z: f32| noise::ridged(x / 120.0, z / 120.0, 4242, 5, 2.0, 0.5) * 90.0
    }

    fn fast_settings() -> ErosionSettings {
        ErosionSettings {
            cell_m: 4.0,
            droplets_per_cell: 2.0,
            max_steps: 24,
            thermal_iterations: 2,
            ..ErosionSettings::default()
        }
    }

    #[test]
    fn bake_is_bit_reproducible() {
        let r = small_world();
        let s = fast_settings();
        let a = bake(&r, &s, &mountain());
        let b = bake(&r, &s, &mountain());
        assert_eq!(
            a.content_bytes(),
            b.content_bytes(),
            "the same recipe must bake to the same bytes"
        );
    }

    #[test]
    fn erosion_actually_moves_material_and_stays_bounded() {
        let r = small_world();
        let g = bake(&r, &fast_settings(), &mountain());
        let (lo, hi) = g.min_max();
        assert!(lo < -0.05, "nothing was eroded: min delta {lo}");
        assert!(hi > 0.05, "nothing was deposited: max delta {hi}");
        // Bounded against the source terrain's own relief, not an absolute number, so the check holds at any
        // world scale. This test deliberately runs four times the default droplet budget on a deliberately
        // spiky ridged field, and most of the incision is thermal slumping — which is the point of it — so
        // the bar is "erosion did not reshape the landscape wholesale", not "erosion was subtle".
        let relief = source_relief(&mountain());
        assert!(
            lo > -relief * 0.5 && hi < relief * 0.5,
            "erosion moved {lo}..{hi} m against a {relief} m relief — the parameters are unstable"
        );
    }

    #[test]
    fn erosion_saturates_instead_of_running_away() {
        // The real stability property, and the one that catches a mis-scaled capacity term: quadrupling the
        // droplet budget must NOT quadruple the displacement. Transport is self-limiting — a droplet can only
        // carry its capacity — so the effect saturates. Superlinear growth is the signature of a feedback
        // loop with no ceiling, which is exactly the bug that shipped in the first version of this module.
        let r = small_world();
        let magnitude = |droplets: f32| {
            let s = ErosionSettings {
                droplets_per_cell: droplets,
                ..fast_settings()
            };
            let g = bake(&r, &s, &mountain());
            let (lo, hi) = g.min_max();
            lo.abs().max(hi.abs())
        };
        let one = magnitude(1.0);
        let four = magnitude(4.0);
        assert!(
            one > 0.1,
            "the low-budget bake did nothing measurable: {one}"
        );
        assert!(
            four < one * 3.0,
            "4x the droplets produced {four} m against {one} m — that is a runaway, not erosion"
        );
    }

    /// Peak-to-trough of the un-eroded field over the bake grid.
    fn source_relief(f: &impl Fn(f32, f32) -> f32) -> f32 {
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        for i in 0..64 {
            for j in 0..64 {
                let h = f(i as f32 * 4.0, j as f32 * 4.0);
                lo = lo.min(h);
                hi = hi.max(h);
            }
        }
        (hi - lo).max(1.0)
    }

    #[test]
    fn erosion_roughly_conserves_mass() {
        // Sediment leaves at the world edge, so this is "no runaway", not strict conservation.
        let r = small_world();
        let g = bake(&r, &fast_settings(), &mountain());
        let sum: f32 = g.data.iter().sum();
        let scale: f32 = g.data.iter().map(|v| v.abs()).sum();
        assert!(
            sum.abs() < scale * 0.5,
            "net material change {sum} is large against the total movement {scale}"
        );
    }

    #[test]
    fn the_delta_field_is_continuous_so_it_cannot_print_a_seam() {
        // Any visible artefact from the bake would show up as a large cell-to-cell jump in the delta.
        let r = small_world();
        let g = bake(&r, &fast_settings(), &mountain());
        let (lo, hi) = g.min_max();
        let range = (hi - lo).max(1e-3);
        let mut worst = 0.0f32;
        for z in 0..i64::from(g.height) {
            for x in 0..i64::from(g.width) - 1 {
                worst = worst.max((g.get(x + 1, z) - g.get(x, z)).abs());
            }
        }
        assert!(
            worst < range * 0.6,
            "delta discontinuity {worst} against range {range}"
        );
    }

    #[test]
    fn thermal_erosion_reduces_slopes_past_the_talus_angle() {
        // A cone far steeper than the talus angle must slump.
        let cell = 2.0;
        let n = 41;
        let mut g = HeightGrid::from_fn([0.0, 0.0], cell, n, n, |x, z| {
            let dx = x - 40.0;
            let dz = z - 40.0;
            (60.0 - (dx * dx + dz * dz).sqrt() * 3.0).max(0.0)
        });
        let s = ErosionSettings {
            thermal_iterations: 40,
            talus_slope: 0.5,
            ..ErosionSettings::default()
        };
        let steepest_before = steepest_slope(&g, cell);
        thermal_pass(&mut g, &s, cell);
        let steepest_after = steepest_slope(&g, cell);
        assert!(
            steepest_after < steepest_before * 0.8,
            "thermal erosion did not slump the cone: {steepest_after} vs {steepest_before}"
        );
    }

    fn steepest_slope(g: &HeightGrid, cell: f32) -> f32 {
        let mut worst = 0.0f32;
        for z in 0..i64::from(g.height) {
            for x in 0..i64::from(g.width) - 1 {
                worst = worst.max(((g.get(x + 1, z) - g.get(x, z)) / cell).abs());
            }
        }
        worst
    }

    #[test]
    fn droplet_order_does_not_change_an_epoch() {
        // The property the epoch formulation exists for: inside one epoch, droplets are independent, so
        // accumulating them in reverse order changes nothing but float summation order.
        let g = HeightGrid::from_fn([0.0, 0.0], 4.0, 33, 33, |x, z| {
            noise::ridged(x / 60.0, z / 60.0, 7, 4, 2.0, 0.5) * 40.0
        });
        let s = fast_settings();
        let starts: Vec<(f32, f32)> = (0..64)
            .map(|i| (4.0 + (i % 8) as f32 * 3.1, 4.0 + (i / 8) as f32 * 3.1))
            .collect();
        let mut fwd = vec![0.0f32; g.data.len()];
        for (x, z) in &starts {
            simulate_droplet(&g, &mut fwd, *x, *z, &s, &DiscKernel::new(s.radius_cells));
        }
        let mut rev = vec![0.0f32; g.data.len()];
        for (x, z) in starts.iter().rev() {
            simulate_droplet(&g, &mut rev, *x, *z, &s, &DiscKernel::new(s.radius_cells));
        }
        let worst = fwd
            .iter()
            .zip(&rev)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        let scale = fwd.iter().map(|v| v.abs()).fold(0.0f32, f32::max).max(1e-6);
        assert!(
            worst < scale * 1e-3,
            "order changed the epoch result by {worst} against a scale of {scale}"
        );
    }

    #[test]
    fn a_huge_world_coarsens_instead_of_allocating_forever() {
        // 32 km at 0.5 m would be 64 000² cells; the guard must back off to a sane grid.
        let cell = effective_cell(32_768.0, 0.5);
        assert!(cell > 0.5, "cell size was not coarsened: {cell}");
        let n = (32_768.0 / cell).ceil() as usize + 1;
        assert!(n * n <= MAX_BAKE_CELLS, "guard failed: {n}²");
        // A reasonable world is left exactly as requested.
        assert_eq!(effective_cell(2048.0, 3.0), 3.0);
    }

    #[test]
    fn bake_key_ignores_layers_above_it() {
        let mut r = small_world();
        r.layers.push(Layer::new(
            "Hills",
            LayerKind::Fbm {
                amplitude: 30.0,
                wavelength_m: 200.0,
                octaves: 4,
                lacunarity: 2.0,
                gain: 0.5,
                warp_m: 0.0,
                warp_wavelength_m: 1.0,
            },
        ));
        r.layers
            .push(Layer::new("Erode", LayerKind::Erosion(fast_settings())));
        let key_before = bake_key(&r, 2);
        // Something added ABOVE the erosion layer must not invalidate the bake.
        r.layers
            .push(Layer::new("Snow", LayerKind::Constant { height: 1.0 }));
        assert_eq!(key_before, bake_key(&r, 2), "bake wrongly invalidated");
        // Something changed BELOW it must.
        if let LayerKind::Fbm { amplitude, .. } = &mut r.layers[1].kind {
            *amplitude = 31.0;
        }
        assert_ne!(key_before, bake_key(&r, 2), "stale bake would be reused");
    }
}
