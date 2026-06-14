//! Deterministic render-scene generation — the spatial counterpart to M1.4's `ecs/src/scene.rs`.
//!
//! M1.4's `scene-gen` is a *semantic/relational* scene (capabilities + `BindsTo` edges); it has no
//! spatial component. This spike needs **entity transforms** to render, so they're generated here
//! from the **same SplitMix64 seed** ("METROCA1") and entity counts (5k / 20k presets) as M1.4 —
//! same seed ⇒ same cloud across runs and machines (benchmark-discipline: reproducible inputs).

use crate::rng::Rng;
use bytemuck::{Pod, Zeroable};

/// Same seed as M1.4 / the M0 spikes ("METROCA1").
pub const SEED: u64 = 0x4D45_5452_4F43_4131;

/// One renderable entity. Layout matches the WGSL `Instance` storage struct exactly:
/// `center` (vec3, offset 0) · `radius` (f32, offset 12) · `color` (vec3, offset 16) · `scale`
/// (f32, offset 28) = 32 bytes, a multiple of 16 (std430-clean for the vec3 align-16 rule).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct Instance {
    /// World-space center of the entity's bounding sphere (also the cube center).
    pub center: [f32; 3],
    /// Bounding-sphere radius for frustum culling.
    pub radius: f32,
    /// Flat color (placeholder material).
    pub color: [f32; 3],
    /// Uniform cube half-extent.
    pub scale: f32,
}

/// A generated render scene: the instance cloud and its overall extent.
pub struct Scene {
    pub instances: Vec<Instance>,
    /// Half-extent of the cube the cloud is distributed in (world units, centered at origin).
    pub extent: f32,
}

/// Build the entity cloud for `n` entities (5_000 or 20_000 to match M1.4's presets).
///
/// The cloud half-extent scales with `n^(1/3)` so per-entity *density* is held roughly constant
/// between the 5k and 20k presets — the 20k scene is a bigger volume, not a denser one, so the
/// frame-cost delta reflects entity count, not overdraw.
#[must_use]
pub fn build_scene(n: usize) -> Scene {
    let mut rng = Rng::new(SEED);
    // ~constant density: extent grows with the cube root of the count.
    let extent = 18.0 * (n as f32 / 5_000.0).cbrt();
    let mut instances = Vec::with_capacity(n);
    for _ in 0..n {
        let center = [
            rng.range(-extent, extent),
            rng.range(-extent, extent),
            rng.range(-extent, extent),
        ];
        let scale = rng.range(0.18, 0.55);
        // bounding sphere of a cube with half-extent `scale` is scale*sqrt(3).
        let radius = scale * 1.732_05;
        let color = [
            rng.range(0.25, 1.0),
            rng.range(0.25, 1.0),
            rng.range(0.25, 1.0),
        ];
        instances.push(Instance {
            center,
            radius,
            color,
            scale,
        });
    }
    Scene { instances, extent }
}
