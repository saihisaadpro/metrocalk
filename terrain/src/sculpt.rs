//! Sculpting — brush strokes as *data*.
//!
//! A dab is a [`Stroke`]: about forty bytes of centre, radius, strength and shape. Sculpting therefore
//! inherits everything the document already gives ordinary component edits — one undo step per gesture,
//! CRDT merge, replay, diff — and it stays resolution-independent, because a stroke is evaluated as a
//! pointwise function rather than rasterized into a fixed grid. Reopening a project two years later and
//! re-baking at four times the resolution reproduces the same shapes, sharper.
//!
//! ## The one subtlety: what "smooth" smooths
//!
//! `Raise`, `Flatten` and `Noise` are trivially pointwise. `Smooth` is not — smoothing means "average my
//! neighbourhood", and the neighbourhood includes other strokes. Evaluating that naively recurses: each
//! smooth dab would need nine samples of a field that itself contains smooth dabs, so cost would grow as
//! `taps^(smooth count)`.
//!
//! The resolution used here: **a smooth dab low-passes the layer stack plus every preceding
//! non-smooth stroke.** That is well defined, costs a fixed nine taps of a non-recursive function, and does
//! what an author means — smoothing after raising a ridge does flatten the ridge, and smoothing an already
//! smoothed patch is a no-op rather than an exponential bill.

use serde::{Deserialize, Serialize};

use crate::noise;
use crate::recipe::{Stroke, StrokeKind};

/// The interactive brush settings a UI holds; [`stroke_run`] turns a drag over the terrain into the dabs
/// that get recorded in the recipe.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Brush {
    /// What the brush does.
    pub kind: StrokeKind,
    /// Radius in metres.
    pub radius_m: f32,
    /// Metres per dab for `Raise`/`Noise`, blend weight for `Smooth`/`Flatten`.
    pub strength: f32,
    /// Falloff shape, 0 = linear, 1 = smooth.
    pub hardness: f32,
    /// Flatten target height / noise wavelength / smooth radius scale, per [`Stroke::target_m`].
    pub target_m: f32,
    /// Dab spacing as a fraction of the radius. 0.25 gives a continuous-feeling stroke without recording
    /// hundreds of dabs for one drag.
    pub spacing: f32,
}

impl Default for Brush {
    fn default() -> Self {
        Self {
            kind: StrokeKind::Raise,
            radius_m: 24.0,
            strength: 2.0,
            hardness: 0.75,
            target_m: 0.0,
            spacing: 0.25,
        }
    }
}

impl Brush {
    /// One dab at a world position.
    #[must_use]
    pub fn dab(&self, x: f32, z: f32) -> Stroke {
        Stroke {
            kind: self.kind,
            x,
            z,
            radius_m: self.radius_m.max(0.01),
            strength: self.strength,
            hardness: self.hardness.clamp(0.0, 1.0),
            target_m: self.target_m,
        }
    }
}

/// Turn a drag from `from` to `to` into evenly spaced dabs.
///
/// The dab count is bounded by the spacing, not by the input event rate, so a slow careful drag and a fast
/// flick over the same path record the *same* strokes. That makes sculpting reproducible and keeps a long
/// session from accumulating tens of thousands of near-duplicate ops.
#[must_use]
pub fn stroke_run(brush: &Brush, from: [f32; 2], to: [f32; 2]) -> Vec<Stroke> {
    let step = (brush.radius_m * brush.spacing.clamp(0.02, 1.0)).max(0.05);
    let dx = to[0] - from[0];
    let dz = to[1] - from[1];
    let len = (dx * dx + dz * dz).sqrt();
    if len <= step {
        return vec![brush.dab(to[0], to[1])];
    }
    let n = (len / step).floor() as usize;
    // Cap a single gesture so a pathological drag cannot add unbounded document ops.
    let n = n.min(4096);
    let mut out = Vec::with_capacity(n + 1);
    for i in 1..=n {
        let t = i as f32 / n as f32;
        out.push(brush.dab(from[0] + dx * t, from[1] + dz * t));
    }
    out
}

/// Radial falloff: `1` at the centre, `0` at the radius. `hardness` interpolates from a linear cone to a
/// smoothstep dome.
#[must_use]
pub fn falloff(normalized_distance: f32, hardness: f32) -> f32 {
    let d = normalized_distance.clamp(0.0, 1.0);
    let linear = 1.0 - d;
    let smooth = 1.0 - noise::smoothstep(0.0, 1.0, d);
    noise::lerp(linear, smooth, hardness.clamp(0.0, 1.0))
}

/// Apply one non-smooth stroke's contribution to an accumulated height.
fn apply_simple(h: f32, s: &Stroke, x: f32, z: f32, w: f32, seed: u64) -> f32 {
    match s.kind {
        StrokeKind::Raise => h + s.strength * w,
        StrokeKind::Flatten => noise::lerp(h, s.target_m, (s.strength * w).clamp(0.0, 1.0)),
        StrokeKind::Noise => {
            let wl = if s.target_m > 0.01 { s.target_m } else { 8.0 };
            h + noise::value_2d(x / wl, z / wl, seed ^ 0x51E1_D0BB) * s.strength * w
        }
        // Handled by the caller; a smooth dab never contributes through this path.
        StrokeKind::Smooth => h,
    }
}

/// The weight of a stroke at a point, or `None` when the point is outside its radius.
fn weight_at(s: &Stroke, x: f32, z: f32) -> Option<f32> {
    let r = s.radius_m.max(0.01);
    let d2 = s.dist2(x, z);
    if d2 > r * r {
        return None;
    }
    let w = falloff(d2.sqrt() / r, s.hardness);
    if w <= 0.0 {
        None
    } else {
        Some(w)
    }
}

/// Evaluate the stack plus the non-smooth strokes in `prefix` — the field a smooth dab low-passes.
fn field_without_smoothing(
    stack: &impl Fn(f32, f32) -> f32,
    prefix: &[Stroke],
    x: f32,
    z: f32,
    seed: u64,
) -> f32 {
    let mut h = stack(x, z);
    for s in prefix {
        if s.kind == StrokeKind::Smooth {
            continue;
        }
        if let Some(w) = weight_at(s, x, z) {
            h = apply_simple(h, s, x, z, w, seed);
        }
    }
    h
}

/// Nine-tap low pass (centre plus an eight-point ring) of [`field_without_smoothing`].
///
/// Nine taps is enough to remove the octave-scale roughness a smooth brush is aimed at, and it is a fixed
/// cost — the property that keeps a stack of smooth strokes affordable.
fn low_pass(
    stack: &impl Fn(f32, f32) -> f32,
    prefix: &[Stroke],
    x: f32,
    z: f32,
    radius: f32,
    seed: u64,
) -> f32 {
    const RING: [[f32; 2]; 8] = [
        [1.0, 0.0],
        [-1.0, 0.0],
        [0.0, 1.0],
        [0.0, -1.0],
        [0.707_106_77, 0.707_106_77],
        [-0.707_106_77, 0.707_106_77],
        [0.707_106_77, -0.707_106_77],
        [-0.707_106_77, -0.707_106_77],
    ];
    // The centre carries twice a ring tap's weight, which approximates a Gaussian closely enough that the
    // result reads as smoothing rather than as blurring towards a mean.
    let mut sum = field_without_smoothing(stack, prefix, x, z, seed) * 2.0;
    let mut norm = 2.0;
    for r in RING {
        sum += field_without_smoothing(stack, prefix, x + r[0] * radius, z + r[1] * radius, seed);
        norm += 1.0;
    }
    sum / norm
}

/// Apply every stroke, in order, to the height the layer stack produced at `(x, z)`.
///
/// `stack` must be the stroke-free field (layers, splines and bakes). Callers pass a *spatially filtered*
/// stroke slice — [`crate::field::Terrain`] keeps a grid index so a chunk only ever sees the handful of
/// strokes that touch it, which is what keeps a terrain with ten thousand recorded dabs as fast to evaluate
/// as one with none.
pub fn apply_strokes(
    stack: &impl Fn(f32, f32) -> f32,
    strokes: &[Stroke],
    x: f32,
    z: f32,
    seed: u64,
) -> f32 {
    let mut h = stack(x, z);
    for (i, s) in strokes.iter().enumerate() {
        let Some(w) = weight_at(s, x, z) else {
            continue;
        };
        if s.kind == StrokeKind::Smooth {
            let radius = if s.target_m > 0.01 {
                s.radius_m * s.target_m
            } else {
                s.radius_m * 0.5
            };
            let lp = low_pass(stack, &strokes[..i], x, z, radius, seed);
            h = noise::lerp(h, lp, (s.strength * w).clamp(0.0, 1.0));
        } else {
            h = apply_simple(h, s, x, z, w, seed);
        }
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat() -> impl Fn(f32, f32) -> f32 {
        |_x, _z| 0.0
    }

    #[test]
    fn falloff_is_one_at_the_centre_and_zero_at_the_rim() {
        for hardness in [0.0, 0.5, 1.0] {
            assert!((falloff(0.0, hardness) - 1.0).abs() < 1e-6);
            assert!(falloff(1.0, hardness).abs() < 1e-6);
            assert!(falloff(2.0, hardness).abs() < 1e-6, "clamped past the rim");
        }
    }

    #[test]
    fn raise_peaks_at_the_centre_and_vanishes_outside() {
        let s = vec![Stroke {
            kind: StrokeKind::Raise,
            x: 100.0,
            z: 100.0,
            radius_m: 10.0,
            strength: 5.0,
            hardness: 0.0,
            target_m: 0.0,
        }];
        let f = flat();
        assert!((apply_strokes(&f, &s, 100.0, 100.0, 1) - 5.0).abs() < 1e-5);
        assert_eq!(apply_strokes(&f, &s, 120.0, 100.0, 1), 0.0);
        let mid = apply_strokes(&f, &s, 105.0, 100.0, 1);
        assert!(mid > 2.0 && mid < 3.0, "linear falloff halfway: {mid}");
    }

    #[test]
    fn flatten_pulls_towards_the_target() {
        let stack = |_x: f32, _z: f32| 20.0;
        let s = vec![Stroke {
            kind: StrokeKind::Flatten,
            x: 0.0,
            z: 0.0,
            radius_m: 10.0,
            strength: 1.0,
            hardness: 0.0,
            target_m: 5.0,
        }];
        assert!((apply_strokes(&stack, &s, 0.0, 0.0, 1) - 5.0).abs() < 1e-5);
        // Partway out, partially flattened — never overshooting past the target.
        let mid = apply_strokes(&stack, &s, 5.0, 0.0, 1);
        assert!(mid > 5.0 && mid < 20.0, "{mid}");
    }

    #[test]
    fn smooth_reduces_roughness_of_the_stack_below_it() {
        // A deliberately rough stack; a full-strength smooth dab must reduce the local variation.
        let stack = |x: f32, z: f32| noise::perlin_2d(x * 0.5, z * 0.5, 4) * 10.0;
        let s = vec![Stroke {
            kind: StrokeKind::Smooth,
            x: 0.0,
            z: 0.0,
            radius_m: 30.0,
            strength: 1.0,
            hardness: 0.0,
            target_m: 0.0,
        }];
        let rough: f32 = (0..40)
            .map(|i| {
                let x = i as f32 * 0.25;
                (stack(x + 0.25, 0.0) - stack(x, 0.0)).abs()
            })
            .sum();
        let smoothed: f32 = (0..40)
            .map(|i| {
                let x = i as f32 * 0.25;
                (apply_strokes(&stack, &s, x + 0.25, 0.0, 1) - apply_strokes(&stack, &s, x, 0.0, 1))
                    .abs()
            })
            .sum();
        assert!(
            smoothed < rough * 0.8,
            "smoothing must reduce local variation: {smoothed} vs {rough}"
        );
    }

    #[test]
    fn smooth_flattens_a_previously_raised_ridge() {
        // The behaviour the "what does smooth smooth?" decision exists to deliver.
        let stack = flat();
        let raise = Stroke {
            kind: StrokeKind::Raise,
            x: 0.0,
            z: 0.0,
            radius_m: 12.0,
            strength: 20.0,
            hardness: 0.0,
            target_m: 0.0,
        };
        let smooth = Stroke {
            kind: StrokeKind::Smooth,
            x: 0.0,
            z: 0.0,
            radius_m: 24.0,
            strength: 1.0,
            hardness: 0.0,
            target_m: 1.0,
        };
        let peak_before = apply_strokes(&stack, &[raise], 0.0, 0.0, 1);
        let peak_after = apply_strokes(&stack, &[raise, smooth], 0.0, 0.0, 1);
        assert!(peak_before > 19.0);
        assert!(
            peak_after < peak_before * 0.8,
            "smooth must pull the ridge down: {peak_after} vs {peak_before}"
        );
    }

    #[test]
    fn stroke_run_is_spacing_bounded_not_event_bounded() {
        let b = Brush {
            radius_m: 10.0,
            spacing: 0.25,
            ..Brush::default()
        };
        let one_drag = stroke_run(&b, [0.0, 0.0], [100.0, 0.0]);
        assert_eq!(one_drag.len(), 40, "100 m at 2.5 m spacing");
        // Two half-drags produce the same total dab count as one whole one.
        let mut split = stroke_run(&b, [0.0, 0.0], [50.0, 0.0]);
        split.extend(stroke_run(&b, [50.0, 0.0], [100.0, 0.0]));
        assert_eq!(split.len(), one_drag.len());
        // A tiny nudge still records exactly one dab.
        assert_eq!(stroke_run(&b, [0.0, 0.0], [0.1, 0.0]).len(), 1);
    }

    #[test]
    fn strokes_are_bit_reproducible() {
        let stack = |x: f32, z: f32| noise::fbm(x * 0.01, z * 0.01, 9, 5, 2.0, 0.5) * 30.0;
        let s = vec![
            Stroke {
                kind: StrokeKind::Raise,
                x: 5.0,
                z: 5.0,
                radius_m: 20.0,
                strength: 3.0,
                hardness: 0.7,
                target_m: 0.0,
            },
            Stroke {
                kind: StrokeKind::Smooth,
                x: 8.0,
                z: 4.0,
                radius_m: 15.0,
                strength: 0.6,
                hardness: 1.0,
                target_m: 0.0,
            },
        ];
        let a: Vec<u32> = (0..32)
            .map(|i| apply_strokes(&stack, &s, i as f32, 5.0, 77).to_bits())
            .collect();
        let b: Vec<u32> = (0..32)
            .map(|i| apply_strokes(&stack, &s, i as f32, 5.0, 77).to_bits())
            .collect();
        assert_eq!(a, b);
    }
}
