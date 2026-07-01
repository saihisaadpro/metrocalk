//! M15.0 (ADR-070, Leg B) — the SDF-determinism harness. Compiles the canonical `box − cylinder` SDF op to
//! a mesh and prints its content hash as `FINAL_SDF_HASH = <hex>`, so the `sdf-determinism` CI workflow can
//! read it as a `::notice::` across native (x86_64/arm64) AND wasm32-wasip1 (wasmtime) — the cross-platform
//! bit-determinism check. Native `f64` is bit-identical (the ADR-020 property, re-confirmed); because this
//! path uses only IEEE-754 correctly-rounded ops (no `fma`/transcendentals), the wasm run is *designed* to
//! match — the CI notice is where that is observed. Runs the compile twice and asserts self-consistency.

use metrocalk_sdf::{compile, validate, Axis, Grid, Sdf};

fn main() {
    let sdf = Sdf::cuboid([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]).difference(Sdf::cylinder(
        [0.0, 0.0, 0.0],
        0.5,
        2.0,
        Axis::Y,
    ));
    let grid = Grid::around(&sdf, 48, 0.06);
    let mesh = compile(&sdf, &grid);
    let again = compile(&sdf, &grid);
    let r = validate(&mesh);

    assert!(
        r.watertight && r.manifold,
        "compiled mesh is watertight+manifold"
    );
    assert_eq!(
        mesh.content_hash(),
        again.content_hash(),
        "SDF compile is self-consistent across runs"
    );

    println!("SDF_TRIS = {}", mesh.triangle_count());
    println!("SDF_GENUS = {:?}", r.genus);
    println!("FINAL_SDF_HASH = {:032x}", mesh.content_hash());
}
