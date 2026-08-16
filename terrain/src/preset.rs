//! Presets — the answer to "I opened the terrain tool, now what?".
//!
//! A preset is an ordinary [`TerrainRecipe`], not a special mode: every value it sets is one an author can
//! see and change, and every preset is a legitimate starting point rather than a demo. That matters more than
//! it sounds, because the alternative — a "generate terrain" button whose output cannot be taken apart — is
//! how procedural tools end up used once and then abandoned.
//!
//! Scatter prototypes are declared with **empty mesh keys**. The rules are real and tuned; they simply place
//! nothing until an asset is bound, and [`crate::validate`] says so as information rather than as an error.
//! This is deliberate: a preset should not fail because the project has no pine tree, and it should not
//! silently drop the forest it was designed around either.

use crate::recipe::{
    BiomeRule, Blend, ErosionSettings, HeightMask, Layer, LayerKind, LodPolicy, MaterialLayer,
    ScatterProto, ScatterRule, TerrainRecipe, WaterConfig,
};

/// A preset's identity, for a picker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresetInfo {
    /// Stable id used by commands and by the UI.
    pub id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// One line describing what it produces and what it is good for.
    pub description: &'static str,
}

/// Every preset, in the order a picker should show them.
#[must_use]
pub fn all() -> Vec<PresetInfo> {
    vec![
        PresetInfo {
            id: "flat",
            name: "Flat Ground",
            description: "A level plate with one material — the fastest thing to sculpt on, and the cheapest to render.",
        },
        PresetInfo {
            id: "rolling-hills",
            name: "Rolling Hills",
            description: "Gentle warped hills with grass, dry slopes and rock, plus scattered trees and clutter.",
        },
        PresetInfo {
            id: "alpine",
            name: "Alpine Peaks",
            description: "Eroded ridged mountains with scree, exposed rock and a snow line. The heaviest preset.",
        },
        PresetInfo {
            id: "dunes",
            name: "Desert Dunes",
            description: "Billowed dune fields over a broad basin, with rock outcrops where the sand thins.",
        },
        PresetInfo {
            id: "archipelago",
            name: "Archipelago",
            description: "Islands in a shallow sea with beaches and lagoons — flattened lowlands, steep interiors.",
        },
        PresetInfo {
            id: "canyon",
            name: "Canyon Mesa",
            description: "A high plateau cut by terraced canyons, eroded so the benches read as sedimentary.",
        },
    ]
}

/// Build a preset by id.
#[must_use]
pub fn by_id(id: &str) -> Option<TerrainRecipe> {
    match id {
        "flat" => Some(flat()),
        "rolling-hills" => Some(rolling_hills()),
        "alpine" => Some(alpine()),
        "dunes" => Some(dunes()),
        "archipelago" => Some(archipelago()),
        "canyon" => Some(canyon()),
        _ => None,
    }
}

/// Shared skeleton: a 2 km world of 64 m chunks at 1 m cells.
fn base(name: &str, seed: u64) -> TerrainRecipe {
    TerrainRecipe {
        name: name.into(),
        seed,
        world_size_m: 2048.0,
        chunk_size_m: 64.0,
        chunk_verts: 65,
        layers: Vec::new(),
        lod: LodPolicy::default(),
        ..TerrainRecipe::default()
    }
}

/// A level plate.
#[must_use]
pub fn flat() -> TerrainRecipe {
    let mut r = base("Flat Ground", 0x0F1A_7000);
    r.layers =
        vec![Layer::new("Ground", LayerKind::Constant { height: 0.0 }).with_blend(Blend::Replace)];
    r.water = WaterConfig {
        enabled: false,
        ..WaterConfig::default()
    };
    r.materials = vec![MaterialLayer::new("Ground", [0.30, 0.33, 0.24], 0.88)];
    r.biomes = vec![BiomeRule::by_height(
        "Ground",
        [-1.0e6, -1.0e6, 1.0e6, 1.0e6],
        0,
    )];
    r
}

/// Warped rolling hills — the general-purpose starting point.
#[must_use]
#[allow(clippy::too_many_lines)] // a preset is a flat data table; breaking it up would scatter one readable recipe
pub fn rolling_hills() -> TerrainRecipe {
    let mut r = base("Rolling Hills", 0x3011_1F67);
    r.layers = vec![
        Layer::new("Sea Floor", LayerKind::Constant { height: -6.0 }).with_blend(Blend::Replace),
        // The silhouette: broad, heavily warped, so ridges bend instead of running straight.
        Layer::new(
            "Hills",
            LayerKind::Fbm {
                amplitude: 38.0,
                wavelength_m: 520.0,
                octaves: 6,
                lacunarity: 2.0,
                gain: 0.5,
                warp_m: 60.0,
                warp_wavelength_m: 900.0,
            },
        ),
        // Mid detail, quieter, so slopes have texture without the silhouette becoming noisy.
        Layer::new(
            "Undulation",
            LayerKind::Fbm {
                amplitude: 7.0,
                wavelength_m: 85.0,
                octaves: 4,
                lacunarity: 2.1,
                gain: 0.5,
                warp_m: 6.0,
                warp_wavelength_m: 120.0,
            },
        )
        .with_seed_offset(11),
        // Rock outcrops only on the high ground.
        Layer::new(
            "Outcrops",
            LayerKind::Cellular {
                amplitude: 9.0,
                wavelength_m: 140.0,
                invert: true,
            },
        )
        .with_seed_offset(29)
        .with_mask(HeightMask {
            height_band: Some([18.0, 30.0, 1.0e6, 1.0e6]),
            noise: Some([300.0, 0.55, 0.12]),
            invert: false,
        }),
    ];
    r.water = WaterConfig {
        enabled: true,
        sea_level_m: 0.0,
        shore_blend_m: 2.5,
        deep_m: 8.0,
    };
    r.materials = vec![
        MaterialLayer::new("Wet Sand", [0.62, 0.56, 0.42], 0.92),
        MaterialLayer::new("Grass", [0.22, 0.34, 0.15], 0.86),
        MaterialLayer::new("Dry Grass", [0.44, 0.42, 0.22], 0.88),
        MaterialLayer::new("Rock", [0.38, 0.37, 0.35], 0.72),
        MaterialLayer::new("Gravel Road", [0.31, 0.29, 0.27], 0.80),
    ];
    r.biomes = vec![
        BiomeRule::by_height("Shore", [-1.0e6, -1.0e6, 1.5, 5.0], 0),
        BiomeRule::by_height("Meadow", [1.5, 5.0, 26.0, 36.0], 1)
            .with_slope([-1.0, -1.0, 0.45, 0.62]),
        BiomeRule::by_height("Upland", [22.0, 34.0, 1.0e6, 1.0e6], 2)
            .with_slope([-1.0, -1.0, 0.5, 0.7]),
        BiomeRule::by_height("Cliff", [-1.0e6, -1.0e6, 1.0e6, 1.0e6], 3)
            .with_slope([0.5, 0.7, 9.0, 9.1]),
    ];
    r.protos = vec![
        ScatterProto {
            radius_m: 2.4,
            height_m: 13.0,
            collide: true,
            ..ScatterProto::new("Broadleaf Tree", "", 2.4, 13.0)
        },
        ScatterProto::new("Shrub", "", 0.8, 1.4),
        ScatterProto::new("Grass Tuft", "", 0.25, 0.4),
        ScatterProto {
            collide: true,
            ..ScatterProto::new("Boulder", "", 1.1, 1.3)
        },
    ];
    r.scatter = vec![
        ScatterRule {
            biomes: vec![1],
            slope_max: 0.42,
            cluster: Some([220.0, 0.48, 0.14]),
            // Capped by the terrain's own view distance: a tree cannot be visible past the chunk it stands
            // on, so asking for more only ever produces trees hovering over nothing.
            view_distance_m: 1000.0,
            ..ScatterRule::new("Woodland", 0, 110.0)
        },
        ScatterRule {
            biomes: vec![1, 2],
            slope_max: 0.55,
            view_distance_m: 400.0,
            ..ScatterRule::new("Shrubs", 1, 600.0)
        },
        ScatterRule {
            biomes: vec![1],
            slope_max: 0.5,
            // Ground clutter belongs close: production open worlds stream it at a fraction of tree range.
            view_distance_m: 70.0,
            scale_range: [0.7, 1.5],
            align_to_normal: 0.55,
            ..ScatterRule::new("Grass", 2, 14_000.0)
        },
        ScatterRule {
            biomes: vec![2, 3],
            slope_max: 0.8,
            cluster: Some([90.0, 0.6, 0.1]),
            view_distance_m: 500.0,
            ..ScatterRule::new("Boulders", 3, 40.0)
        },
    ];
    r
}

/// Eroded ridged mountains with a snow line.
#[must_use]
#[allow(clippy::too_many_lines)] // as above
pub fn alpine() -> TerrainRecipe {
    let mut r = base("Alpine Peaks", 0xA19E_C0DE);
    r.height_clamp = [-64.0, 2400.0];
    r.layers = vec![
        Layer::new("Valley Floor", LayerKind::Constant { height: 120.0 })
            .with_blend(Blend::Replace),
        Layer::new(
            "Massif",
            LayerKind::Fbm {
                amplitude: 260.0,
                wavelength_m: 1600.0,
                octaves: 5,
                lacunarity: 2.0,
                gain: 0.55,
                warp_m: 180.0,
                warp_wavelength_m: 2600.0,
            },
        ),
        // Ridges added with Max so they sit on the massif instead of doubling its height.
        Layer::new(
            "Ridges",
            LayerKind::Ridged {
                amplitude: 620.0,
                wavelength_m: 1100.0,
                octaves: 7,
                lacunarity: 2.05,
                gain: 0.52,
                warp_m: 120.0,
                warp_wavelength_m: 1400.0,
            },
        )
        .with_blend(Blend::Max)
        .with_seed_offset(7),
        Layer::new(
            "Rock Detail",
            LayerKind::Fbm {
                amplitude: 14.0,
                wavelength_m: 70.0,
                octaves: 4,
                lacunarity: 2.0,
                gain: 0.5,
                warp_m: 4.0,
                warp_wavelength_m: 90.0,
            },
        )
        .with_seed_offset(19),
        // Erosion is what turns "ridged noise" into "mountains": drainage, scree, softened crests.
        Layer::new(
            "Erosion",
            // 6 m cells: erosion shapes valleys and ridge lines, which are tens to hundreds of metres
            // across, so a finer bake costs time without changing what anyone sees. Fine detail keeps coming
            // from the noise layers at full resolution, at every LOD, for free.
            LayerKind::Erosion(ErosionSettings {
                cell_m: 6.0,
                droplets_per_cell: 0.6,
                thermal_iterations: 5,
                talus_slope: 0.85,
                ..ErosionSettings::default()
            }),
        ),
    ];
    r.water = WaterConfig {
        enabled: true,
        sea_level_m: 100.0,
        shore_blend_m: 3.0,
        deep_m: 10.0,
    };
    r.materials = vec![
        MaterialLayer::new("Alpine Grass", [0.24, 0.31, 0.17], 0.87),
        MaterialLayer::new("Scree", [0.44, 0.42, 0.39], 0.84),
        MaterialLayer::new("Rock", [0.33, 0.32, 0.31], 0.70),
        MaterialLayer {
            detail_wavelength_m: 12.0,
            detail_strength: 0.07,
            ..MaterialLayer::new("Snow", [0.86, 0.89, 0.94], 0.45)
        },
        MaterialLayer::new("Glacial Silt", [0.55, 0.58, 0.56], 0.80),
    ];
    r.biomes = vec![
        BiomeRule::by_height("Lakeshore", [-1.0e6, -1.0e6, 106.0, 118.0], 4),
        BiomeRule::by_height("Meadow", [112.0, 130.0, 520.0, 700.0], 0)
            .with_slope([-1.0, -1.0, 0.5, 0.7]),
        BiomeRule::by_height("Scree", [400.0, 620.0, 1250.0, 1450.0], 1)
            .with_slope([-1.0, -1.0, 0.9, 1.2]),
        BiomeRule::by_height("Cliff", [-1.0e6, -1.0e6, 1.0e6, 1.0e6], 2)
            .with_slope([0.75, 1.0, 9.0, 9.1]),
        BiomeRule {
            patchiness: Some([420.0, 0.42, 0.16]),
            ..BiomeRule::by_height("Snow", [1150.0, 1400.0, 1.0e6, 1.0e6], 3)
                .with_slope([-1.0, -1.0, 1.1, 1.5])
        },
    ];
    r.protos = vec![
        ScatterProto {
            collide: true,
            ..ScatterProto::new("Conifer", "", 1.9, 17.0)
        },
        ScatterProto::new("Alpine Shrub", "", 0.6, 0.9),
        ScatterProto {
            collide: true,
            ..ScatterProto::new("Rock Slab", "", 1.6, 1.1)
        },
    ];
    r.scatter = vec![
        ScatterRule {
            biomes: vec![1],
            slope_max: 0.55,
            height_band: [120.0, 160.0, 900.0, 1100.0],
            cluster: Some([340.0, 0.44, 0.15]),
            // See the note in `rolling_hills`: bounded by the terrain's view distance.
            view_distance_m: 1000.0,
            ..ScatterRule::new("Conifers", 0, 160.0)
        },
        ScatterRule {
            biomes: vec![1, 2],
            slope_max: 0.75,
            view_distance_m: 220.0,
            ..ScatterRule::new("Alpine Scrub", 1, 2200.0)
        },
        ScatterRule {
            biomes: vec![2, 3],
            slope_max: 1.0,
            cluster: Some([70.0, 0.55, 0.1]),
            view_distance_m: 600.0,
            align_to_normal: 0.85,
            ..ScatterRule::new("Scree Rocks", 2, 220.0)
        },
    ];
    r
}

/// Dune fields over a broad basin.
#[must_use]
#[allow(clippy::too_many_lines)] // as above
pub fn dunes() -> TerrainRecipe {
    let mut r = base("Desert Dunes", 0xD0_5E_47_00);
    r.layers = vec![
        Layer::new("Basin", LayerKind::Constant { height: 4.0 }).with_blend(Blend::Replace),
        Layer::new(
            "Basin Relief",
            LayerKind::Fbm {
                amplitude: 26.0,
                wavelength_m: 900.0,
                octaves: 4,
                lacunarity: 2.0,
                gain: 0.5,
                warp_m: 90.0,
                warp_wavelength_m: 1500.0,
            },
        ),
        // Billow gives the rounded lobes that read as sand; the warp makes the crests sinuous.
        Layer::new(
            "Dunes",
            LayerKind::Billow {
                amplitude: 17.0,
                wavelength_m: 130.0,
                octaves: 3,
                lacunarity: 2.2,
                gain: 0.42,
            },
        )
        .with_seed_offset(5),
        Layer::new(
            "Ripples",
            LayerKind::Fbm {
                amplitude: 0.7,
                wavelength_m: 9.0,
                octaves: 2,
                lacunarity: 2.0,
                gain: 0.5,
                warp_m: 0.0,
                warp_wavelength_m: 1.0,
            },
        )
        .with_seed_offset(23),
        Layer::new(
            "Bedrock",
            LayerKind::Ridged {
                amplitude: 70.0,
                wavelength_m: 700.0,
                octaves: 5,
                lacunarity: 2.0,
                gain: 0.5,
                warp_m: 40.0,
                warp_wavelength_m: 900.0,
            },
        )
        .with_blend(Blend::Max)
        .with_seed_offset(31)
        .with_mask(HeightMask {
            height_band: None,
            noise: Some([800.0, 0.62, 0.1]),
            invert: false,
        }),
    ];
    r.water = WaterConfig {
        enabled: false,
        ..WaterConfig::default()
    };
    r.materials = vec![
        MaterialLayer {
            detail_wavelength_m: 3.0,
            detail_strength: 0.10,
            ..MaterialLayer::new("Sand", [0.74, 0.63, 0.42], 0.93)
        },
        MaterialLayer::new("Coarse Sand", [0.62, 0.51, 0.34], 0.90),
        MaterialLayer::new("Desert Rock", [0.46, 0.36, 0.28], 0.75),
    ];
    r.biomes = vec![
        BiomeRule::by_height("Sand Sea", [-1.0e6, -1.0e6, 1.0e6, 1.0e6], 0)
            .with_slope([-1.0, -1.0, 0.35, 0.5]),
        BiomeRule::by_height("Dune Face", [-1.0e6, -1.0e6, 1.0e6, 1.0e6], 1)
            .with_slope([0.3, 0.45, 0.7, 0.9]),
        BiomeRule::by_height("Escarpment", [-1.0e6, -1.0e6, 1.0e6, 1.0e6], 2)
            .with_slope([0.65, 0.85, 9.0, 9.1]),
    ];
    r.protos = vec![
        ScatterProto::new("Desert Shrub", "", 0.7, 0.8),
        ScatterProto {
            collide: true,
            ..ScatterProto::new("Sandstone Block", "", 1.4, 1.8)
        },
    ];
    r.scatter = vec![
        ScatterRule {
            biomes: vec![0],
            slope_max: 0.3,
            cluster: Some([260.0, 0.6, 0.12]),
            view_distance_m: 300.0,
            ..ScatterRule::new("Scrub", 0, 90.0)
        },
        ScatterRule {
            biomes: vec![2],
            slope_max: 0.9,
            view_distance_m: 500.0,
            align_to_normal: 0.7,
            ..ScatterRule::new("Rubble", 1, 120.0)
        },
    ];
    r
}

/// Islands in a shallow sea.
#[must_use]
#[allow(clippy::too_many_lines)] // as above
pub fn archipelago() -> TerrainRecipe {
    let mut r = base("Archipelago", 0xA2_C4_15_1A);
    r.layers = vec![
        Layer::new("Sea Bed", LayerKind::Constant { height: -34.0 }).with_blend(Blend::Replace),
        Layer::new(
            "Land Mass",
            LayerKind::Fbm {
                amplitude: 120.0,
                wavelength_m: 760.0,
                octaves: 6,
                lacunarity: 2.0,
                gain: 0.5,
                warp_m: 90.0,
                warp_wavelength_m: 1300.0,
            },
        ),
        // A power curve about the water line sinks the shallows and steepens the interiors, which is what
        // turns fBm blobs into islands with real coastlines.
        Layer::new(
            "Coastal Curve",
            LayerKind::Curve {
                exponent: 2.0,
                pivot_m: 0.0,
            },
        ),
        Layer::new(
            "Headland Detail",
            LayerKind::Fbm {
                amplitude: 9.0,
                wavelength_m: 70.0,
                octaves: 4,
                lacunarity: 2.0,
                gain: 0.5,
                warp_m: 5.0,
                warp_wavelength_m: 90.0,
            },
        )
        .with_seed_offset(13)
        .with_mask(HeightMask {
            height_band: Some([-6.0, 2.0, 1.0e6, 1.0e6]),
            noise: None,
            invert: false,
        }),
    ];
    r.height_clamp = [-64.0, 900.0];
    r.water = WaterConfig {
        enabled: true,
        sea_level_m: 0.0,
        shore_blend_m: 1.5,
        deep_m: 12.0,
    };
    r.materials = vec![
        MaterialLayer::new("Sea Bed", [0.38, 0.40, 0.34], 0.92),
        MaterialLayer {
            detail_wavelength_m: 2.5,
            detail_strength: 0.12,
            ..MaterialLayer::new("Beach Sand", [0.80, 0.74, 0.56], 0.94)
        },
        MaterialLayer::new("Island Grass", [0.20, 0.36, 0.16], 0.86),
        MaterialLayer::new("Sea Cliff", [0.40, 0.38, 0.35], 0.72),
    ];
    r.biomes = vec![
        BiomeRule::by_height("Sea Bed", [-1.0e6, -1.0e6, -1.0, 0.4], 0),
        BiomeRule::by_height("Beach", [-1.2, 0.2, 2.5, 5.5], 1),
        BiomeRule::by_height("Interior", [3.0, 6.0, 1.0e6, 1.0e6], 2)
            .with_slope([-1.0, -1.0, 0.55, 0.75]),
        BiomeRule::by_height("Cliff", [1.0, 3.0, 1.0e6, 1.0e6], 3)
            .with_slope([0.55, 0.75, 9.0, 9.1]),
    ];
    r.protos = vec![
        ScatterProto {
            collide: true,
            ..ScatterProto::new("Palm", "", 1.6, 9.0)
        },
        ScatterProto::new("Coastal Grass", "", 0.3, 0.5),
        ScatterProto::new("Driftwood", "", 0.9, 0.4),
    ];
    r.scatter = vec![
        ScatterRule {
            biomes: vec![2],
            slope_max: 0.45,
            height_band: [3.0, 6.0, 60.0, 90.0],
            cluster: Some([180.0, 0.45, 0.15]),
            view_distance_m: 900.0,
            ..ScatterRule::new("Palms", 0, 140.0)
        },
        ScatterRule {
            biomes: vec![1, 2],
            slope_max: 0.4,
            view_distance_m: 80.0,
            align_to_normal: 0.5,
            ..ScatterRule::new("Coastal Grass", 1, 9000.0)
        },
        ScatterRule {
            biomes: vec![1],
            slope_max: 0.25,
            view_distance_m: 200.0,
            ..ScatterRule::new("Driftwood", 2, 30.0)
        },
    ];
    r
}

/// A plateau cut by terraced canyons.
#[must_use]
#[allow(clippy::too_many_lines)] // as above
pub fn canyon() -> TerrainRecipe {
    let mut r = base("Canyon Mesa", 0xCA49_0000);
    r.height_clamp = [-32.0, 900.0];
    r.layers = vec![
        Layer::new("Plateau", LayerKind::Constant { height: 320.0 }).with_blend(Blend::Replace),
        Layer::new(
            "Plateau Relief",
            LayerKind::Fbm {
                amplitude: 34.0,
                wavelength_m: 800.0,
                octaves: 4,
                lacunarity: 2.0,
                gain: 0.5,
                warp_m: 70.0,
                warp_wavelength_m: 1200.0,
            },
        ),
        // Canyons are cut with Min so the channels go *down* into the plateau rather than adding relief.
        Layer::new(
            "Canyons",
            LayerKind::Ridged {
                amplitude: 300.0,
                wavelength_m: 900.0,
                octaves: 6,
                lacunarity: 2.1,
                gain: 0.5,
                warp_m: 110.0,
                warp_wavelength_m: 1500.0,
            },
        )
        .with_blend(Blend::Min)
        .with_seed_offset(3),
        // Sedimentary benches: the single most recognisable feature of this landscape.
        Layer::new(
            "Benches",
            LayerKind::Terrace {
                step_m: 14.0,
                sharpness: 0.8,
            },
        ),
        Layer::new(
            "Rock Grain",
            LayerKind::Fbm {
                amplitude: 3.5,
                wavelength_m: 26.0,
                octaves: 3,
                lacunarity: 2.0,
                gain: 0.5,
                warp_m: 0.0,
                warp_wavelength_m: 1.0,
            },
        )
        .with_seed_offset(41),
        Layer::new(
            "Erosion",
            LayerKind::Erosion(ErosionSettings {
                cell_m: 4.0,
                droplets_per_cell: 0.6,
                thermal_iterations: 3,
                talus_slope: 1.1,
                amplitude: 0.8,
                ..ErosionSettings::default()
            }),
        ),
    ];
    r.water = WaterConfig {
        enabled: true,
        sea_level_m: 30.0,
        shore_blend_m: 2.0,
        deep_m: 6.0,
    };
    r.materials = vec![
        MaterialLayer::new("Canyon Floor", [0.56, 0.44, 0.31], 0.90),
        MaterialLayer::new("Red Rock", [0.52, 0.30, 0.20], 0.78),
        MaterialLayer::new("Pale Strata", [0.66, 0.56, 0.42], 0.80),
        MaterialLayer::new("Mesa Top", [0.44, 0.40, 0.28], 0.86),
    ];
    r.biomes = vec![
        BiomeRule::by_height("Floor", [-1.0e6, -1.0e6, 90.0, 140.0], 0),
        BiomeRule::by_height("Walls", [80.0, 130.0, 300.0, 340.0], 1)
            .with_slope([0.35, 0.6, 9.0, 9.1]),
        BiomeRule {
            patchiness: Some([120.0, 0.5, 0.12]),
            ..BiomeRule::by_height("Strata", [110.0, 160.0, 320.0, 360.0], 2)
                .with_slope([0.3, 0.5, 9.0, 9.1])
        },
        BiomeRule::by_height("Mesa", [300.0, 340.0, 1.0e6, 1.0e6], 3)
            .with_slope([-1.0, -1.0, 0.35, 0.5]),
    ];
    r.protos = vec![
        ScatterProto::new("Desert Brush", "", 0.6, 0.7),
        ScatterProto {
            collide: true,
            ..ScatterProto::new("Fallen Boulder", "", 1.8, 2.0)
        },
    ];
    r.scatter = vec![
        ScatterRule {
            biomes: vec![0, 3],
            slope_max: 0.35,
            cluster: Some([200.0, 0.55, 0.14]),
            view_distance_m: 350.0,
            ..ScatterRule::new("Brush", 0, 260.0)
        },
        ScatterRule {
            biomes: vec![0],
            slope_max: 0.5,
            view_distance_m: 500.0,
            align_to_normal: 0.4,
            ..ScatterRule::new("Rockfall", 1, 60.0)
        },
    ];
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::Terrain;
    use crate::recipe::ChunkCoord;
    use crate::validate;
    use std::collections::BTreeMap;

    #[test]
    fn every_listed_preset_resolves() {
        for info in all() {
            assert!(
                by_id(info.id).is_some(),
                "{} is listed but missing",
                info.id
            );
            assert!(!info.name.is_empty());
            assert!(
                info.description.len() > 20,
                "{} needs a real description",
                info.id
            );
        }
        assert_eq!(by_id("nope"), None);
    }

    #[test]
    fn every_preset_validates_without_blocking_issues() {
        for info in all() {
            let r = by_id(info.id).expect("preset");
            let report = validate::validate(&r);
            assert!(
                !report.has_blocking(),
                "{} is not buildable: {}",
                info.id,
                report.summary()
            );
        }
    }

    #[test]
    fn every_preset_compiles_and_produces_terrain_in_range() {
        // Erosion presets are the slow ones, so this samples rather than sweeping the whole world.
        for info in all() {
            let r = by_id(info.id).expect("preset");
            let clamp = r.height_clamp;
            let name = info.id;
            let t = Terrain::compile(r, BTreeMap::new())
                .unwrap_or_else(|e| panic!("{name} failed to compile: {e}"));
            let mut lo = f32::MAX;
            let mut hi = f32::MIN;
            for i in 0..40 {
                for j in 0..40 {
                    let h = t.height(i as f32 * 51.0, j as f32 * 51.0);
                    assert!(h.is_finite(), "{name} produced a non-finite height");
                    assert!(
                        h >= clamp[0] - 1e-3 && h <= clamp[1] + 1e-3,
                        "{name} escaped its clamp: {h}"
                    );
                    lo = lo.min(h);
                    hi = hi.max(h);
                }
            }
            if name == "flat" {
                assert!(
                    (hi - lo).abs() < 1e-6,
                    "flat preset is not flat: {lo}..{hi}"
                );
            } else {
                assert!(hi - lo > 5.0, "{name} is suspiciously flat: {lo}..{hi}");
            }
        }
    }

    #[test]
    fn presets_are_bit_reproducible_across_compiles() {
        // The one preset with erosion in it, since that is where reproducibility is hardest.
        let a = Terrain::compile(canyon(), BTreeMap::new()).expect("compile");
        let b = Terrain::compile(canyon(), BTreeMap::new()).expect("compile");
        for i in 0..64 {
            let (x, z) = (i as f32 * 29.0, 640.0);
            assert_eq!(a.height(x, z).to_bits(), b.height(x, z).to_bits());
        }
    }

    #[test]
    fn scatter_rules_reference_real_prototypes_and_biomes() {
        for info in all() {
            let r = by_id(info.id).expect("preset");
            for (i, s) in r.scatter.iter().enumerate() {
                assert!(s.proto < r.protos.len(), "{} scatter[{i}] proto", info.id);
                for b in &s.biomes {
                    assert!(*b < r.biomes.len(), "{} scatter[{i}] biome {b}", info.id);
                }
                assert!(
                    s.view_distance_m <= r.lod.max_view_distance_m,
                    "{} scatter[{i}] outruns the streaming distance",
                    info.id
                );
            }
            for b in &r.biomes {
                assert!(
                    b.material_layer < r.materials.len(),
                    "{} biome material",
                    info.id
                );
            }
        }
    }

    #[test]
    fn a_preset_chunk_meshes_cleanly() {
        let t = Terrain::compile(rolling_hills(), BTreeMap::new()).expect("compile");
        let s = t.sample_chunk(ChunkCoord::new(8, 8)).expect("chunk");
        let m = crate::mesh::build_chunk_mesh(&s, 0, &t.recipe().lod);
        assert!(m.data.tri_count() > 1000);
        assert!(m.bounds_max[1] >= m.bounds_min[1]);
        for p in &m.data.positions {
            assert!(p[1].is_finite());
        }
    }
}
