//! Generate the checked-in demo `.glb` fixtures the editor shell imports at startup. Run once and
//! commit the output (the fixtures are the provenance-tracked, version-controlled assets; this is how
//! they're produced): `cargo run -p metrocalk-assets --example gen_fixtures -- <out-dir>`.
//! With no argument, writes to `../editor-shell/assets` relative to this crate.

use std::path::PathBuf;

fn main() {
    // Default anchored to this crate's dir (not the invocation CWD), so it always lands inside the
    // repo regardless of where cargo is run from.
    let out = std::env::args().nth(1).map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("editor-shell")
                .join("assets")
        },
        PathBuf::from,
    );
    std::fs::create_dir_all(&out).expect("create assets dir");

    for (name, bytes) in [
        ("healthbar.glb", metrocalk_assets::demo::healthbar_glb()),
        ("prop.glb", metrocalk_assets::demo::prop_glb()),
        ("sphere.glb", metrocalk_assets::demo::sphere_glb()),
        // M11.2 follow-up — a full-PBR demo tile (base + metallic-roughness + normal map) for the
        // positive MR/normal visual check.
        (
            "normal_mapped_quad.glb",
            metrocalk_assets::demo::normal_mapped_quad_glb(),
        ),
        // M11.2 single-texture tile (checker base color) — also the M11.5 near-duplicate counterpart of
        // multi_material_quad.glb: both carry the SAME checker as texture[0] but differ in geometry, so they
        // hash to different content addresses (no exact dedup) yet match perceptually (the dHash hint fires).
        (
            "textured_quad.glb",
            metrocalk_assets::demo::textured_quad_glb(),
        ),
        // M11.2 multi-texture-per-mesh fixture — two materials, two base textures, side by side.
        (
            "multi_material_quad.glb",
            metrocalk_assets::demo::multi_material_quad_glb(),
        ),
        // M11.5 near-duplicate pair (ADR-044) — same structured ripple base texture, different geometry:
        // distinct content addresses (no exact dedup) but an identical perceptual hash (the dHash hint fires).
        ("ripple_quad.glb", metrocalk_assets::demo::ripple_quad_glb()),
        (
            "ripple_quad_wide.glb",
            metrocalk_assets::demo::ripple_quad_wide_glb(),
        ),
        // M11.1 static-collider fixture — a plain cube (flat top a dropped ball rests on).
        ("cube.glb", metrocalk_assets::demo::cube_glb()),
        // M11.1 LOD fixture — a dense sphere (fine enough that distance LOD is visible).
        (
            "dense_sphere.glb",
            metrocalk_assets::demo::dense_sphere_glb(),
        ),
    ] {
        let path = out.join(name);
        std::fs::write(&path, &bytes).expect("write fixture");
        println!("wrote {} ({} bytes)", path.display(), bytes.len());
    }
}
