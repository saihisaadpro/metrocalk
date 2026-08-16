//! Roads, rivers and pads — splines that reshape the terrain they cross.
//!
//! A spline is authored as a handful of clicked points and compiled into a densely sampled Catmull-Rom
//! centre line with a *resolved surface height* per sample. Resolving the height at compile time is what
//! makes the rest pointwise: at run time, carving a river or laying a road is "find the nearest centre-line
//! sample, blend towards its height", which a chunk can evaluate independently of every other chunk.
//!
//! Two resolution rules carry most of the quality:
//!
//! * **Roads are graded, not draped.** The surface height along a road is a running average of the
//!   underlying terrain (or the authored point heights), so a road holds a plausible grade across dips
//!   instead of following every bump like a bad decal.
//! * **Rivers run downhill, by construction.** The resolved height is forced monotonically non-increasing
//!   along the path. A river that flows uphill is the single most obvious failure in procedural hydrology,
//!   and a running minimum makes it impossible rather than unlikely.

use std::collections::BTreeMap;

use crate::mesh::MeshData;
use crate::recipe::{SplineDef, SplineKind};

/// A compiled spline: a sampled centre line with resolved surface heights and a cross-section.
#[derive(Clone, Debug, PartialEq)]
pub struct SplinePath {
    /// Index of the source [`SplineDef`] in the recipe.
    pub def_index: usize,
    /// Road / river / pad.
    pub kind: SplineKind,
    /// Sampled centre line: world `(x, surface_y, z)`.
    pub samples: Vec<[f32; 3]>,
    /// Arc length in metres at each sample, for UVs and for distance-along queries.
    pub arc: Vec<f32>,
    /// Half the flat/carved width, in metres.
    pub half_width: f32,
    /// Blend distance beyond the half width, in metres.
    pub falloff: f32,
    /// River depth below the surface line, in metres.
    pub depth: f32,
    /// Material layer painted over the corridor.
    pub material_layer: Option<usize>,
    /// Scatter suppression distance beyond the corridor, in metres.
    pub clear_scatter: f32,
    /// Take the surface height from the authored control points rather than from the terrain.
    pub use_point_height: bool,
}

impl SplinePath {
    /// Total length in metres.
    #[must_use]
    pub fn length(&self) -> f32 {
        self.arc.last().copied().unwrap_or(0.0)
    }

    /// The distance at which this spline stops influencing the terrain.
    #[must_use]
    pub fn reach(&self) -> f32 {
        self.half_width + self.falloff.max(0.0)
    }
}

/// Where a point sits relative to a spline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplineHit {
    /// Index into [`SplineIndex::paths`].
    pub path: usize,
    /// Perpendicular distance to the centre line, in metres.
    pub distance: f32,
    /// Resolved surface height of the nearest centre-line point, in metres.
    pub surface_y: f32,
    /// Distance along the spline, in metres — the UV/measurement coordinate.
    pub along: f32,
}

/// A uniform-grid index over every spline segment, so a chunk pays for the splines that touch it and not
/// for the hundreds that do not.
#[derive(Clone, Debug, Default)]
pub struct SplineIndex {
    /// The compiled paths.
    pub paths: Vec<SplinePath>,
    cell: f32,
    /// Grid cell → `(path, segment)` pairs whose influence region overlaps the cell.
    buckets: BTreeMap<(i32, i32), Vec<(u32, u32)>>,
    max_reach: f32,
}

/// Uniform Catmull-Rom interpolation of four control values.
fn catmull(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1)
        + (-p0 + p2) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

/// Sample a Catmull-Rom spline through `points` at roughly `step_m` spacing. End tangents are mirrored, so
/// a two-point spline is a straight line and a three-point spline curves through the middle.
#[must_use]
pub fn sample_centerline(points: &[[f32; 3]], step_m: f32) -> Vec<[f32; 3]> {
    if points.len() < 2 {
        return points.to_vec();
    }
    let step = step_m.max(0.25);
    let at = |i: isize| -> [f32; 3] {
        let n = points.len() as isize;
        points[i.clamp(0, n - 1) as usize]
    };
    let mut out = Vec::new();
    for i in 0..points.len() - 1 {
        let p0 = at(i as isize - 1);
        let p1 = at(i as isize);
        let p2 = at(i as isize + 1);
        let p3 = at(i as isize + 2);
        let seg_len = {
            let dx = p2[0] - p1[0];
            let dz = p2[2] - p1[2];
            (dx * dx + dz * dz).sqrt()
        };
        // At least two subdivisions per segment so a curve is never a chord; capped so a huge segment
        // cannot allocate without bound.
        let n = ((seg_len / step).ceil() as usize).clamp(2, 4096);
        for k in 0..n {
            let t = k as f32 / n as f32;
            out.push([
                catmull(p0[0], p1[0], p2[0], p3[0], t),
                catmull(p0[1], p1[1], p2[1], p3[1], t),
                catmull(p0[2], p1[2], p2[2], p3[2], t),
            ]);
        }
    }
    out.push(*points.last().expect("non-empty"));
    out
}

/// Distance from a point to a segment, plus the parametric position along it.
fn point_segment(px: f32, pz: f32, a: [f32; 3], b: [f32; 3]) -> (f32, f32) {
    let vx = b[0] - a[0];
    let vz = b[2] - a[2];
    let len2 = vx * vx + vz * vz;
    let t = if len2 <= 1e-12 {
        0.0
    } else {
        (((px - a[0]) * vx + (pz - a[2]) * vz) / len2).clamp(0.0, 1.0)
    };
    let cx = a[0] + vx * t;
    let cz = a[2] + vz * t;
    let dx = px - cx;
    let dz = pz - cz;
    ((dx * dx + dz * dz).sqrt(), t)
}

impl SplineIndex {
    /// Compile every enabled spline's **geometry** — centre line, arc length and the segment buckets.
    ///
    /// Surface heights are left at the authored values and must be filled in by [`Self::resolve`]. The split
    /// exists because the two halves depend on different things: the geometry needs only XZ, while the
    /// heights need the finished layers-plus-strokes field — which in turn is allowed to consult this
    /// index. Building geometry first breaks what would otherwise be a circular dependency.
    pub fn build(defs: &[SplineDef], step_m: f32) -> Self {
        let mut paths = Vec::new();
        for (i, d) in defs.iter().enumerate() {
            if !d.enabled || d.points.len() < 2 {
                continue;
            }
            let samples = sample_centerline(&d.points, step_m);
            let mut arc = Vec::with_capacity(samples.len());
            let mut total = 0.0;
            for (k, s) in samples.iter().enumerate() {
                if k > 0 {
                    let p = samples[k - 1];
                    let dx = s[0] - p[0];
                    let dz = s[2] - p[2];
                    total += (dx * dx + dz * dz).sqrt();
                }
                arc.push(total);
            }
            paths.push(SplinePath {
                def_index: i,
                kind: d.kind,
                samples,
                arc,
                half_width: (d.width_m * 0.5).max(0.1),
                falloff: d.falloff_m.max(0.0),
                depth: d.depth_m.max(0.0),
                material_layer: d.material_layer,
                clear_scatter: d.clear_scatter_m.max(0.0),
                use_point_height: d.use_point_height,
            });
        }
        let max_reach = paths.iter().map(SplinePath::reach).fold(0.0, f32::max);
        // One bucket comfortably larger than the widest corridor keeps a query to a single cell lookup.
        let cell = (max_reach * 2.0).max(16.0);
        let mut buckets: BTreeMap<(i32, i32), Vec<(u32, u32)>> = BTreeMap::new();
        for (pi, p) in paths.iter().enumerate() {
            let reach = p.reach();
            for si in 0..p.samples.len().saturating_sub(1) {
                let a = p.samples[si];
                let b = p.samples[si + 1];
                let lo_x = a[0].min(b[0]) - reach;
                let hi_x = a[0].max(b[0]) + reach;
                let lo_z = a[2].min(b[2]) - reach;
                let hi_z = a[2].max(b[2]) + reach;
                let (cx0, cx1) = ((lo_x / cell).floor() as i32, (hi_x / cell).floor() as i32);
                let (cz0, cz1) = ((lo_z / cell).floor() as i32, (hi_z / cell).floor() as i32);
                for cz in cz0..=cz1 {
                    for cx in cx0..=cx1 {
                        buckets
                            .entry((cx, cz))
                            .or_default()
                            .push((pi as u32, si as u32));
                    }
                }
            }
        }
        Self {
            paths,
            cell,
            buckets,
            max_reach,
        }
    }

    /// Fill in every sample's surface height: the graded road line, or the monotone river descent.
    ///
    /// `base_height` must be the terrain height *before* splines are applied (layers, bakes and strokes).
    /// `smoothing_samples` is the half-width of the running average that grades a road.
    pub fn resolve(&mut self, base_height: &impl Fn(f32, f32) -> f32, smoothing_samples: usize) {
        for p in &mut self.paths {
            resolve_surface(
                &mut p.samples,
                p.kind,
                p.use_point_height,
                base_height,
                smoothing_samples,
            );
        }
    }

    /// Whether any spline exists at all — the fast out for terrains with none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }

    /// The widest corridor influence, in metres.
    #[must_use]
    pub fn max_reach(&self) -> f32 {
        self.max_reach
    }

    /// The nearest spline to a world point, or `None` when nothing is within its influence.
    #[must_use]
    pub fn nearest(&self, x: f32, z: f32) -> Option<SplineHit> {
        if self.paths.is_empty() {
            return None;
        }
        let key = (
            (x / self.cell).floor() as i32,
            (z / self.cell).floor() as i32,
        );
        let list = self.buckets.get(&key)?;
        let mut best: Option<SplineHit> = None;
        for &(pi, si) in list {
            let p = &self.paths[pi as usize];
            let a = p.samples[si as usize];
            let b = p.samples[si as usize + 1];
            let (d, t) = point_segment(x, z, a, b);
            if d > p.reach() {
                continue;
            }
            if best.is_none_or(|h| d < h.distance) {
                let base = p.arc[si as usize];
                let next = p.arc[si as usize + 1];
                best = Some(SplineHit {
                    path: pi as usize,
                    distance: d,
                    surface_y: a[1] + (b[1] - a[1]) * t,
                    along: base + (next - base) * t,
                });
            }
        }
        best
    }

    /// Apply every spline's reshaping to a height at `(x, z)`.
    ///
    /// Only the *nearest* spline shapes a point. Blending overlapping corridors would produce a height
    /// between two roads at a crossing — visibly wrong — whereas nearest-wins produces a clean junction.
    #[must_use]
    pub fn apply(&self, h: f32, x: f32, z: f32) -> f32 {
        let Some(hit) = self.nearest(x, z) else {
            return h;
        };
        let p = &self.paths[hit.path];
        // 1 inside the flat width, easing to 0 at the end of the falloff.
        let w = if hit.distance <= p.half_width {
            1.0
        } else {
            let t = ((hit.distance - p.half_width) / p.falloff.max(1e-3)).clamp(0.0, 1.0);
            1.0 - crate::noise::smoothstep(0.0, 1.0, t)
        };
        if w <= 0.0 {
            return h;
        }
        match p.kind {
            SplineKind::Road | SplineKind::Pad => crate::noise::lerp(h, hit.surface_y, w),
            SplineKind::River => {
                // A rounded channel: full depth at the centre line, rising to the banks. The channel only
                // ever cuts down (`min`), so a river crossing a valley does not build a dam across it.
                let across = (hit.distance / p.half_width).clamp(0.0, 1.0);
                let profile = 1.0 - across * across;
                let bed = hit.surface_y - p.depth * profile;
                let carved = h.min(bed);
                crate::noise::lerp(h, carved, w)
            }
        }
    }

    /// Distance in metres from the nearest corridor *edge*: 0 inside a corridor, growing outside it, and
    /// `f32::MAX` when no spline is near. Scatter and masks use this to keep clear of roads.
    #[must_use]
    pub fn clearance(&self, x: f32, z: f32) -> f32 {
        match self.nearest(x, z) {
            None => f32::MAX,
            Some(hit) => {
                let p = &self.paths[hit.path];
                (hit.distance - p.half_width - p.clear_scatter).max(0.0)
            }
        }
    }

    /// The material layer painted at a point by the nearest spline, if any covers it.
    #[must_use]
    pub fn material_at(&self, x: f32, z: f32) -> Option<usize> {
        let hit = self.nearest(x, z)?;
        let p = &self.paths[hit.path];
        if hit.distance <= p.half_width + p.falloff * 0.5 {
            p.material_layer
        } else {
            None
        }
    }

    /// The water surface height a river imposes at a point, if one covers it. Used to fill channels.
    #[must_use]
    pub fn river_surface(&self, x: f32, z: f32) -> Option<f32> {
        let hit = self.nearest(x, z)?;
        let p = &self.paths[hit.path];
        if p.kind == SplineKind::River && hit.distance <= p.half_width {
            // Water sits a little below the bank line so the surface reads as inside the channel.
            Some(hit.surface_y - p.depth * 0.25)
        } else {
            None
        }
    }
}

/// Fill in the `y` of each centre-line sample: the graded road surface, or the monotone river descent.
fn resolve_surface(
    samples: &mut [[f32; 3]],
    kind: SplineKind,
    use_point_height: bool,
    base_height: &impl Fn(f32, f32) -> f32,
    smoothing: usize,
) {
    if samples.is_empty() {
        return;
    }
    // Start from the terrain, or from the authored point heights when the author asked for them.
    let raw: Vec<f32> = if use_point_height {
        samples.iter().map(|s| s[1]).collect()
    } else {
        samples.iter().map(|s| base_height(s[0], s[2])).collect()
    };
    // Running average, which is what gives a road a grade instead of a drape.
    let k = smoothing.min(samples.len());
    let smoothed: Vec<f32> = (0..raw.len())
        .map(|i| {
            let lo = i.saturating_sub(k);
            let hi = (i + k + 1).min(raw.len());
            let n = (hi - lo) as f32;
            raw[lo..hi].iter().sum::<f32>() / n
        })
        .collect();

    match kind {
        SplineKind::River => {
            // Force a downhill run. Descend from whichever end starts higher, so the author does not have
            // to draw rivers in flow order.
            let first = *smoothed.first().expect("non-empty");
            let last = *smoothed.last().expect("non-empty");
            let mut out = smoothed.clone();
            if first >= last {
                let mut cap = first;
                for v in &mut out {
                    cap = cap.min(*v);
                    *v = cap;
                }
            } else {
                let mut cap = last;
                for v in out.iter_mut().rev() {
                    cap = cap.min(*v);
                    *v = cap;
                }
            }
            for (s, y) in samples.iter_mut().zip(out) {
                s[1] = y;
            }
        }
        SplineKind::Road | SplineKind::Pad => {
            for (s, y) in samples.iter_mut().zip(smoothed) {
                s[1] = y;
            }
        }
    }
}

/// Build a ribbon mesh along a spline — the visible road/river surface.
///
/// The ribbon is generated in *world* space and lifted by `lift_m` so it sits just above the flattened
/// terrain rather than z-fighting with it. UVs run `0..1` across the width and metres-per-`uv_scale_m`
/// along the length, so a tiling road texture keeps a constant real-world scale regardless of spline length.
#[must_use]
pub fn ribbon_mesh(path: &SplinePath, lift_m: f32, uv_scale_m: f32) -> MeshData {
    let mut m = MeshData::default();
    if path.samples.len() < 2 {
        return m;
    }
    let hw = path.half_width;
    let scale = if uv_scale_m > 0.01 { uv_scale_m } else { 1.0 };
    for (i, s) in path.samples.iter().enumerate() {
        // Forward direction by central difference, so the ribbon does not kink at the joints.
        let prev = path.samples[i.saturating_sub(1)];
        let next = path.samples[(i + 1).min(path.samples.len() - 1)];
        let mut fx = next[0] - prev[0];
        let mut fz = next[2] - prev[2];
        let len = (fx * fx + fz * fz).sqrt();
        if len > 1e-6 {
            fx /= len;
            fz /= len;
        } else {
            fx = 1.0;
            fz = 0.0;
        }
        // Left normal in the XZ plane.
        let (nx, nz) = (-fz, fx);
        let v = path.arc[i] / scale;
        let y = s[1] + lift_m;
        m.positions.push([s[0] + nx * hw, y, s[2] + nz * hw]);
        m.normals.push([0.0, 1.0, 0.0]);
        m.uvs.push([0.0, v]);
        m.positions.push([s[0] - nx * hw, y, s[2] - nz * hw]);
        m.normals.push([0.0, 1.0, 0.0]);
        m.uvs.push([1.0, v]);
    }
    for i in 0..path.samples.len() - 1 {
        let b = (i * 2) as u32;
        m.indices
            .extend_from_slice(&[b, b + 2, b + 1, b + 1, b + 2, b + 3]);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::SplineDef;

    /// Build and resolve in one step — what [`crate::field::Terrain::compile`] does in two.
    fn compiled(
        defs: &[SplineDef],
        base: &impl Fn(f32, f32) -> f32,
        step: f32,
        smoothing: usize,
    ) -> SplineIndex {
        let mut idx = SplineIndex::build(defs, step);
        idx.resolve(base, smoothing);
        idx
    }

    fn straight_road() -> SplineDef {
        SplineDef {
            name: "R".into(),
            kind: SplineKind::Road,
            points: vec![[0.0, 0.0, 0.0], [100.0, 0.0, 0.0]],
            width_m: 10.0,
            falloff_m: 5.0,
            ..SplineDef::default()
        }
    }

    #[test]
    fn centerline_passes_through_its_control_points() {
        let pts = vec![[0.0, 0.0, 0.0], [10.0, 0.0, 10.0], [20.0, 0.0, 0.0]];
        let s = sample_centerline(&pts, 1.0);
        assert!(s.len() > 10);
        assert_eq!(s[0], pts[0]);
        assert_eq!(*s.last().unwrap(), pts[2]);
        // The middle control point is hit (Catmull-Rom interpolates its controls).
        assert!(
            s.iter()
                .any(|p| (p[0] - 10.0).abs() < 1e-3 && (p[2] - 10.0).abs() < 1e-3),
            "must interpolate the middle point"
        );
    }

    #[test]
    fn road_flattens_across_its_width_and_releases_outside() {
        let hilly = |x: f32, _z: f32| x * 0.2;
        let idx = compiled(&[straight_road()], &hilly, 2.0, 8);
        // On the centre line at x = 50, the graded surface is the local average of a linear ramp = 10 m.
        let on = idx.apply(hilly(50.0, 0.0), 50.0, 0.0);
        assert!((on - 10.0).abs() < 0.6, "graded road surface: {on}");
        // Across the flat width, the height is the road's, not the terrain's.
        let across = idx.apply(hilly(50.0, 4.0), 50.0, 4.0);
        assert!((across - on).abs() < 0.1, "flat across the width: {across}");
        // Well outside the falloff, the terrain is untouched.
        let out = idx.apply(hilly(50.0, 40.0), 50.0, 40.0);
        assert_eq!(out, hilly(50.0, 40.0));
    }

    #[test]
    fn river_only_ever_runs_downhill() {
        // A terrain with a bump in the middle: a draped river would climb it.
        let bumpy = |x: f32, _z: f32| {
            let t = (x - 50.0) / 25.0;
            20.0 - x * 0.1 + 15.0 * (1.0 - t * t).max(0.0)
        };
        let def = SplineDef {
            name: "River".into(),
            kind: SplineKind::River,
            points: vec![[0.0, 0.0, 0.0], [100.0, 0.0, 0.0]],
            width_m: 12.0,
            falloff_m: 6.0,
            depth_m: 4.0,
            ..SplineDef::default()
        };
        let idx = compiled(&[def], &bumpy, 2.0, 4);
        let ys: Vec<f32> = idx.paths[0].samples.iter().map(|s| s[1]).collect();
        for w in ys.windows(2) {
            assert!(
                w[1] <= w[0] + 1e-4,
                "river surface climbed: {:?} -> {:?}",
                w[0],
                w[1]
            );
        }
        // And the channel actually cuts into the bump.
        let mid = idx.apply(bumpy(50.0, 0.0), 50.0, 0.0);
        assert!(mid < bumpy(50.0, 0.0) - 3.0, "channel not carved: {mid}");
    }

    #[test]
    fn river_carve_never_raises_terrain() {
        let flat = |_x: f32, _z: f32| 0.0;
        let def = SplineDef {
            kind: SplineKind::River,
            points: vec![[0.0, 0.0, 0.0], [50.0, 0.0, 0.0]],
            width_m: 10.0,
            depth_m: 5.0,
            ..SplineDef::default()
        };
        let idx = compiled(&[def], &flat, 2.0, 4);
        for i in 0..40 {
            let x = i as f32 * 1.25;
            for j in -8..8 {
                let z = j as f32;
                let h = idx.apply(0.0, x, z);
                assert!(h <= 1e-5, "river raised the ground at ({x}, {z}): {h}");
            }
        }
    }

    #[test]
    fn clearance_is_zero_on_the_road_and_grows_away_from_it() {
        let flat = |_x: f32, _z: f32| 0.0;
        let idx = compiled(&[straight_road()], &flat, 2.0, 4);
        assert_eq!(idx.clearance(50.0, 0.0), 0.0);
        // Half-width 5 m plus the default 3 m scatter margin: 7 m is still inside, 10 m is clear.
        assert_eq!(idx.clearance(50.0, 7.0), 0.0);
        assert!(idx.clearance(50.0, 10.0) > 0.0);
        assert_eq!(idx.clearance(50.0, 500.0), f32::MAX, "no spline in range");
    }

    #[test]
    fn ribbon_is_a_well_formed_strip() {
        let flat = |_x: f32, _z: f32| 0.0;
        let idx = compiled(&[straight_road()], &flat, 5.0, 4);
        let m = ribbon_mesh(&idx.paths[0], 0.05, 8.0);
        assert_eq!(m.positions.len(), idx.paths[0].samples.len() * 2);
        assert_eq!(m.indices.len() % 3, 0);
        assert_eq!(m.indices.len() / 3, (idx.paths[0].samples.len() - 1) * 2);
        for i in &m.indices {
            assert!((*i as usize) < m.positions.len(), "index out of range");
        }
        // The strip spans the full width.
        let width = (m.positions[0][2] - m.positions[1][2]).abs();
        assert!((width - 10.0).abs() < 1e-3, "ribbon width: {width}");
    }

    #[test]
    fn an_empty_index_is_a_pure_pass_through() {
        let idx = SplineIndex::default();
        assert!(idx.is_empty());
        assert_eq!(idx.apply(12.0, 5.0, 5.0), 12.0);
        assert_eq!(idx.nearest(0.0, 0.0), None);
    }
}
