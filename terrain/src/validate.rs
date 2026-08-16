//! Recipe validation — refuse what cannot work, warn about what will disappoint, and always say how to fix
//! it.
//!
//! Two rules shape this module. First, an issue names the **field** it is about, so a UI can take the author
//! straight to the control rather than making them hunt. Second, every issue carries a **fix** in the
//! imperative, because "chunk_verts must be 2^k + 1" is a sentence about the implementation, whereas "use 65
//! — the nearest valid value" is a sentence the author can act on.
//!
//! Blocking issues stop [`crate::field::Terrain::compile`]. Warnings do not: a terrain whose budget is too
//! small for its view distance is still a terrain, and the streamer will degrade it honestly and report so.
//! Silently clamping a bad parameter is the one behaviour deliberately not available here — a world that
//! quietly differs from what was authored is worse than one that refuses to build.

use crate::erosion;
use crate::profile::estimate_resident_bytes;
use crate::recipe::{LayerKind, TerrainRecipe};

/// Seconds per (cell × droplet-per-cell) of erosion bake.
///
/// An ORDER-OF-MAGNITUDE figure, calibrated on one measured point — a 20 km world, which the cell cap
/// coarsens to 6 m (≈11.1 M cells) at the default 0.5 droplets per cell, took ≈83 s. It exists so the
/// author is given a number rather than a frozen window; it is not a benchmark, and a slower or faster
/// machine will differ by a factor, not by an order.
const SECONDS_PER_DROPLET_UNIT: f64 = 1.5e-5;

/// How badly an issue matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Informational — a note, not a problem.
    Info,
    /// The terrain builds but will look or perform worse than the author expects.
    Warning,
    /// The terrain cannot be built.
    Blocking,
}

/// One validation finding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidationIssue {
    /// How badly it matters.
    pub severity: Severity,
    /// Dotted path of the offending field, e.g. `chunk_verts` or `scatter[2].proto`.
    pub field: String,
    /// What is wrong, in plain language.
    pub message: String,
    /// What to do about it, in the imperative. `None` only when there is genuinely nothing to suggest.
    pub fix: Option<String>,
}

/// The full report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ValidationReport {
    /// Every finding, blocking first.
    pub issues: Vec<ValidationIssue>,
}

impl ValidationReport {
    /// Whether anything blocks the build.
    #[must_use]
    pub fn has_blocking(&self) -> bool {
        self.issues.iter().any(|i| i.severity == Severity::Blocking)
    }

    /// Issues at a severity.
    pub fn at(&self, severity: Severity) -> impl Iterator<Item = &ValidationIssue> {
        self.issues.iter().filter(move |i| i.severity == severity)
    }

    /// A one-string summary of the blocking issues, for an error message.
    #[must_use]
    pub fn summary(&self) -> String {
        let blocking: Vec<String> = self
            .at(Severity::Blocking)
            .map(|i| match &i.fix {
                Some(f) => format!("{} ({}) — {f}", i.message, i.field),
                None => format!("{} ({})", i.message, i.field),
            })
            .collect();
        if blocking.is_empty() {
            "no blocking issues".into()
        } else {
            blocking.join("; ")
        }
    }
}

fn issue(
    severity: Severity,
    field: impl Into<String>,
    message: impl Into<String>,
    fix: Option<&str>,
) -> ValidationIssue {
    ValidationIssue {
        severity,
        field: field.into(),
        message: message.into(),
        fix: fix.map(str::to_string),
    }
}

/// The nearest valid `chunk_verts` (a power of two plus one) at or above `n`.
#[must_use]
pub fn nearest_valid_verts(n: u32) -> u32 {
    let quads = n.saturating_sub(1).max(2);
    let mut p = 2u32;
    while p < quads {
        p *= 2;
    }
    p + 1
}

/// Validate a recipe.
#[allow(clippy::too_many_lines)] // a flat checklist; grouping it into helpers would only scatter it
#[must_use]
pub fn validate(recipe: &TerrainRecipe) -> ValidationReport {
    let mut out = Vec::new();

    // --- Geometry, the things that make LODs line up at all ---
    let quads = recipe.chunk_verts.saturating_sub(1);
    if recipe.chunk_verts < 3 {
        out.push(issue(
            Severity::Blocking,
            "chunk_verts",
            format!(
                "a chunk needs at least 3 vertices per edge to have any surface; this is {}",
                recipe.chunk_verts
            ),
            Some("use 65 for 1 m cells on a 64 m chunk"),
        ));
    } else if !quads.is_power_of_two() {
        out.push(issue(
            Severity::Blocking,
            "chunk_verts",
            format!(
                "{} vertices per edge means {quads} cells, which is not a power of two — coarser LODs \
                 could not subsample it exactly and every chunk boundary would crack",
                recipe.chunk_verts
            ),
            Some(&format!("use {}", nearest_valid_verts(recipe.chunk_verts))),
        ));
    }
    if recipe.chunk_size_m <= 0.0 {
        out.push(issue(
            Severity::Blocking,
            "chunk_size_m",
            "chunk size must be positive".to_string(),
            Some("use 64"),
        ));
    }
    if recipe.world_size_m < recipe.chunk_size_m {
        out.push(issue(
            Severity::Blocking,
            "world_size_m",
            format!(
                "the world ({} m) is smaller than one chunk ({} m)",
                recipe.world_size_m, recipe.chunk_size_m
            ),
            Some("raise the world size to at least one chunk, or lower the chunk size"),
        ));
    }
    if recipe.height_clamp[0] >= recipe.height_clamp[1] {
        out.push(issue(
            Severity::Blocking,
            "height_clamp",
            format!(
                "the height clamp is empty or inverted: [{}, {}]",
                recipe.height_clamp[0], recipe.height_clamp[1]
            ),
            Some("set the minimum below the maximum, e.g. [-256, 2048]"),
        ));
    }
    if recipe.lod.levels == 0 {
        out.push(issue(
            Severity::Blocking,
            "lod.levels",
            "a terrain needs at least one LOD".to_string(),
            Some("use 4"),
        ));
    } else {
        let deepest = 1u32 << u32::from(recipe.lod.levels - 1);
        if quads > 0 && deepest > quads {
            out.push(issue(
                Severity::Warning,
                "lod.levels",
                format!(
                    "{} LOD levels would coarsen a {quads}-cell chunk past a single quad",
                    recipe.lod.levels
                ),
                Some(&format!(
                    "use {} levels, or raise chunk_verts",
                    quads.trailing_zeros().max(1)
                )),
            ));
        }
    }

    // --- References, the things that would panic or silently draw nothing ---
    if recipe.materials.is_empty() {
        out.push(issue(
            Severity::Blocking,
            "materials",
            "a terrain needs at least one ground material or it has nothing to paint with"
                .to_string(),
            Some("add a material, e.g. Ground"),
        ));
    }
    if recipe.biomes.is_empty() {
        out.push(issue(
            Severity::Blocking,
            "biomes",
            "a terrain needs at least one biome rule to choose materials".to_string(),
            Some("add a biome covering all heights and point it at material 0"),
        ));
    }
    for (i, b) in recipe.biomes.iter().enumerate() {
        if b.material_layer >= recipe.materials.len() {
            out.push(issue(
                Severity::Blocking,
                format!("biomes[{i}].material_layer"),
                format!(
                    "biome '{}' points at material {} but only {} exist",
                    b.name,
                    b.material_layer,
                    recipe.materials.len()
                ),
                Some("pick an existing material"),
            ));
        }
        for (name, band) in [
            ("height_band", b.height_band),
            ("slope_band", b.slope_band),
            ("moisture_band", b.moisture_band),
        ] {
            if !(band[0] <= band[1] && band[1] <= band[2] && band[2] <= band[3]) {
                out.push(issue(
                    Severity::Warning,
                    format!("biomes[{i}].{name}"),
                    format!(
                        "biome '{}' has an out-of-order {name} {band:?}, so it will never match",
                        b.name
                    ),
                    Some("order the four stops from lowest to highest"),
                ));
            }
        }
    }
    for (i, s) in recipe.scatter.iter().enumerate() {
        if s.proto >= recipe.protos.len() {
            out.push(issue(
                Severity::Blocking,
                format!("scatter[{i}].proto"),
                format!(
                    "scatter rule '{}' points at prototype {} but only {} exist",
                    s.name,
                    s.proto,
                    recipe.protos.len()
                ),
                Some("pick an existing prototype"),
            ));
            continue;
        }
        if recipe.protos[s.proto].mesh_key.is_empty() {
            out.push(issue(
                Severity::Info,
                format!("protos[{}].mesh_key", s.proto),
                format!(
                    "prototype '{}' has no mesh bound yet, so rule '{}' places nothing",
                    recipe.protos[s.proto].name, s.name
                ),
                Some("assign a mesh asset to the prototype"),
            ));
        }
        for bi in &s.biomes {
            if *bi >= recipe.biomes.len() {
                out.push(issue(
                    Severity::Warning,
                    format!("scatter[{i}].biomes"),
                    format!(
                        "rule '{}' references biome {bi}, which does not exist",
                        s.name
                    ),
                    Some("remove the reference or add the biome"),
                ));
            }
        }
        if s.view_distance_m > recipe.lod.max_view_distance_m {
            out.push(issue(
                Severity::Warning,
                format!("scatter[{i}].view_distance_m"),
                format!(
                    "rule '{}' wants instances out to {} m but chunks stop streaming at {} m, so they \
                     cannot appear",
                    s.name, s.view_distance_m, recipe.lod.max_view_distance_m
                ),
                Some("lower the rule's view distance, or raise lod.max_view_distance_m"),
            ));
        }
        if s.density_per_hectare > 100_000.0 {
            out.push(issue(
                Severity::Warning,
                format!("scatter[{i}].density_per_hectare"),
                format!(
                    "{} instances per hectare is about one every 10 cm; the per-chunk cap will truncate it",
                    s.density_per_hectare
                ),
                Some("use 20 000 or less for ground clutter"),
            ));
        }
    }
    for (i, sp) in recipe.splines.iter().enumerate() {
        if sp.enabled && sp.points.len() < 2 {
            out.push(issue(
                Severity::Warning,
                format!("splines[{i}].points"),
                format!("'{}' has fewer than two points and is ignored", sp.name),
                Some("add at least two control points"),
            ));
        }
        if let Some(m) = sp.material_layer {
            if m >= recipe.materials.len() {
                out.push(issue(
                    Severity::Warning,
                    format!("splines[{i}].material_layer"),
                    format!(
                        "'{}' paints with material {m}, which does not exist",
                        sp.name
                    ),
                    Some("pick an existing material"),
                ));
            }
        }
    }
    for (i, l) in recipe.layers.iter().enumerate() {
        match &l.kind {
            LayerKind::Erosion(s) => {
                let effective = erosion::effective_cell(recipe.world_size_m, s.cell_m);
                if effective > s.cell_m {
                    out.push(issue(
                        Severity::Warning,
                        format!("layers[{i}].cell_m"),
                        format!(
                            "an erosion bake at {} m over a {} m world exceeds the {} M cell cap, so it \
                             will run at {effective} m instead",
                            s.cell_m,
                            recipe.world_size_m,
                            erosion::MAX_BAKE_CELLS / 1_000_000
                        ),
                        Some("raise cell_m, or reduce world_size_m"),
                    ));
                }
                // What the bake will COST, before it is paid. `erosion::bake` is a single serial sweep on
                // the caller's thread — the editor compiles terrain on its engine thread — so a large
                // eroded world stops the application dead for the duration with no progress and no way
                // back. An author who is told "about 80 seconds" can choose; one who is not simply watches
                // a frozen window and concludes the editor crashed.
                let n = f64::from((recipe.world_size_m / effective).ceil()) + 1.0;
                let seconds = n * n * f64::from(s.droplets_per_cell) * SECONDS_PER_DROPLET_UNIT;
                if seconds > 10.0 {
                    out.push(issue(
                        Severity::Warning,
                        format!("layers[{i}].cell_m"),
                        format!(
                            "eroding a {} m world at {effective} m takes roughly {seconds:.0} s, and \
                             nothing else can happen while it runs",
                            recipe.world_size_m
                        ),
                        Some("raise cell_m, lower droplets_per_cell, or shrink the world"),
                    ));
                }
                if s.droplets_per_cell > 8.0 {
                    out.push(issue(
                        Severity::Warning,
                        format!("layers[{i}].droplets_per_cell"),
                        format!(
                            "{} droplets per cell is a long bake for little extra effect",
                            s.droplets_per_cell
                        ),
                        Some("0.5 to 2 is the useful range"),
                    ));
                }
            }
            LayerKind::Fbm {
                octaves,
                wavelength_m,
                ..
            }
            | LayerKind::Ridged {
                octaves,
                wavelength_m,
                ..
            } => {
                if *octaves > 12 {
                    out.push(issue(
                        Severity::Warning,
                        format!("layers[{i}].octaves"),
                        format!("{octaves} octaves costs field time for detail below one cell"),
                        Some("8 or fewer is plenty"),
                    ));
                }
                let finest =
                    wavelength_m / 2f32.powi(i32::from(*octaves as u16 as i16) - 1).max(1.0);
                if finest < recipe.cell_size_at_lod(0) * 0.5 {
                    out.push(issue(
                        Severity::Info,
                        format!("layers[{i}].octaves"),
                        format!(
                            "'{}' generates detail at about {finest:.2} m, finer than the {} m mesh can \
                             show, so it will alias rather than add detail",
                            l.name,
                            recipe.cell_size_at_lod(0)
                        ),
                        Some("drop an octave, or raise chunk_verts"),
                    ));
                }
            }
            _ => {}
        }
    }

    // --- Budget, the thing that decides whether it holds up ---
    let estimate = estimate_resident_bytes(recipe);
    let cap = recipe.budget.total_bytes();
    if cap > 0 && estimate > cap {
        out.push(issue(
            Severity::Warning,
            "budget",
            format!(
                "a {} m view distance needs roughly {} MB resident but the ceiling is {} MB, so distant \
                 chunks will be evicted and may pop in",
                recipe.lod.max_view_distance_m,
                estimate / (1024 * 1024),
                cap / (1024 * 1024)
            ),
            Some("raise the budget, lower the view distance, or lower lod.texture_res"),
        ));
    }
    if recipe.chunk_count() > 65_536 {
        out.push(issue(
            Severity::Warning,
            "world_size_m",
            format!(
                "{} chunks is a lot of bookkeeping; streaming is fine but authoring tools will feel it",
                recipe.chunk_count()
            ),
            Some("raise chunk_size_m to reduce the count"),
        ));
    }

    out.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.field.cmp(&b.field)));
    ValidationReport { issues: out }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::{
        BiomeRule, ErosionSettings, Layer, MaterialLayer, ScatterProto, ScatterRule,
    };

    #[test]
    fn the_default_recipe_is_clean() {
        let r = validate(&TerrainRecipe::default());
        assert!(!r.has_blocking(), "default recipe reported {:?}", r.issues);
    }

    #[test]
    fn a_non_power_of_two_chunk_is_blocked_with_the_valid_value() {
        let r = TerrainRecipe {
            chunk_verts: 50,
            ..TerrainRecipe::default()
        };
        let report = validate(&r);
        assert!(report.has_blocking());
        let i = report
            .at(Severity::Blocking)
            .find(|i| i.field == "chunk_verts")
            .expect("must flag chunk_verts");
        assert!(i.message.contains("power of two"), "{}", i.message);
        assert_eq!(i.fix.as_deref(), Some("use 65"));
        // And the suggestion is itself valid.
        let mut fixed = r.clone();
        fixed.chunk_verts = nearest_valid_verts(50);
        assert!(!validate(&fixed).has_blocking());
    }

    #[test]
    fn nearest_valid_verts_rounds_up_to_a_power_of_two_plus_one() {
        assert_eq!(nearest_valid_verts(0), 3);
        assert_eq!(nearest_valid_verts(3), 3);
        assert_eq!(nearest_valid_verts(4), 5);
        assert_eq!(nearest_valid_verts(33), 33);
        assert_eq!(nearest_valid_verts(50), 65);
        assert_eq!(nearest_valid_verts(129), 129);
    }

    #[test]
    fn dangling_references_are_blocking_and_name_the_index() {
        let mut r = TerrainRecipe::default();
        r.biomes
            .push(BiomeRule::by_height("Bad", [0.0, 1.0, 2.0, 3.0], 7));
        r.protos.push(ScatterProto::new("Tree", "mesh:t", 1.0, 5.0));
        r.scatter.push(ScatterRule::new("Bad", 9, 100.0));
        let report = validate(&r);
        assert!(report.has_blocking());
        let fields: Vec<&str> = report
            .at(Severity::Blocking)
            .map(|i| i.field.as_str())
            .collect();
        assert!(fields.contains(&"biomes[1].material_layer"), "{fields:?}");
        assert!(fields.contains(&"scatter[0].proto"), "{fields:?}");
        assert!(report.summary().contains("only 1 exist"));
    }

    #[test]
    fn an_empty_material_list_is_blocking() {
        let mut r = TerrainRecipe::default();
        r.materials.clear();
        assert!(validate(&r).has_blocking());
    }

    #[test]
    fn an_unbound_prototype_is_information_not_an_error() {
        let mut r = TerrainRecipe::default();
        r.protos.push(ScatterProto::new("Tree", "", 1.0, 5.0));
        r.scatter.push(ScatterRule::new("Trees", 0, 100.0));
        let report = validate(&r);
        assert!(!report.has_blocking());
        assert!(
            report
                .at(Severity::Info)
                .any(|i| i.message.contains("no mesh bound")),
            "{:?}",
            report.issues
        );
    }

    #[test]
    fn an_over_budget_view_distance_warns_with_real_numbers() {
        let mut r = TerrainRecipe::default();
        r.lod.max_view_distance_m = 4096.0;
        r.budget.mesh_mb = 8;
        r.budget.texture_mb = 8;
        r.budget.scatter_mb = 1;
        r.budget.collider_mb = 1;
        let report = validate(&r);
        assert!(!report.has_blocking(), "an over-budget world still builds");
        let w = report
            .at(Severity::Warning)
            .find(|i| i.field == "budget")
            .expect("must warn about the budget");
        assert!(w.message.contains("MB resident"), "{}", w.message);
        assert!(w.fix.is_some());
    }

    #[test]
    fn a_coarsened_erosion_bake_is_disclosed() {
        let mut r = TerrainRecipe {
            world_size_m: 40_000.0,
            ..TerrainRecipe::default()
        };
        r.layers.push(Layer::new(
            "Erode",
            LayerKind::Erosion(ErosionSettings {
                cell_m: 0.5,
                ..ErosionSettings::default()
            }),
        ));
        let report = validate(&r);
        assert!(
            report
                .at(Severity::Warning)
                .any(|i| i.message.contains("cell cap")),
            "a silently coarsened bake is exactly what this must not do: {:?}",
            report.issues
        );
    }

    #[test]
    fn scatter_beyond_the_streaming_distance_is_flagged() {
        let mut r = TerrainRecipe::default();
        r.protos.push(ScatterProto::new("Tree", "mesh:t", 1.0, 8.0));
        let mut rule = ScatterRule::new("Trees", 0, 80.0);
        rule.view_distance_m = r.lod.max_view_distance_m + 1000.0;
        r.scatter.push(rule);
        let report = validate(&r);
        assert!(report
            .at(Severity::Warning)
            .any(|i| i.field == "scatter[0].view_distance_m"));
    }

    #[test]
    fn every_issue_carries_a_field_and_a_message() {
        // The contract the UI depends on: an issue you cannot navigate to is not actionable.
        let mut r = TerrainRecipe {
            chunk_verts: 40,
            height_clamp: [10.0, -10.0],
            materials: vec![MaterialLayer::new("M", [0.5; 3], 0.8)],
            ..TerrainRecipe::default()
        };
        r.lod.levels = 0;
        for i in validate(&r).issues {
            assert!(!i.field.is_empty(), "issue with no field: {i:?}");
            assert!(!i.message.is_empty(), "issue with no message: {i:?}");
        }
    }

    #[test]
    fn issues_are_sorted_worst_first() {
        let mut r = TerrainRecipe {
            chunk_verts: 40,
            ..TerrainRecipe::default()
        };
        r.lod.max_view_distance_m = 8192.0;
        r.budget.texture_mb = 1;
        let report = validate(&r);
        let severities: Vec<Severity> = report.issues.iter().map(|i| i.severity).collect();
        for w in severities.windows(2) {
            assert!(w[0] >= w[1], "not sorted: {severities:?}");
        }
    }
}
