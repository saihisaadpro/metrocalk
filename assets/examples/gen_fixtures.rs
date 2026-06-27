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
        // M11.2 multi-texture-per-mesh fixture — two materials, two base textures, side by side.
        (
            "multi_material_quad.glb",
            metrocalk_assets::demo::multi_material_quad_glb(),
        ),
    ] {
        let path = out.join(name);
        std::fs::write(&path, &bytes).expect("write fixture");
        println!("wrote {} ({} bytes)", path.display(), bytes.len());
    }
}
