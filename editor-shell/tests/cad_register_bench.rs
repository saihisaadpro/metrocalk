//! Headless timing of the CAD land path's mesh-registration cost on the REAL 263 MB bar STEP —
//! read → per-mesh asset conversion → `MeshGpu::from_asset` (the crease-aware normal derivation).
//! Run explicitly (release, real file required):
//!   cargo test --release -p metrocalk-editor-shell --test cad_register_bench -- --ignored --nocapture

use std::time::Instant;

const BAR_STEP: &str =
    "X:\\Work\\Metrocalk\\Games Projects\\Unreal\\Skid Weld Line A.1\\Skid Weld Line A.1_(1).stp";

#[test]
#[ignore = "needs the local 263 MB commercial STEP; run by hand with --ignored"]
fn bench_bar_step_registration() {
    let Ok(bytes) = std::fs::read(BAR_STEP) else {
        eprintln!("SKIP: {BAR_STEP} not present on this box");
        return;
    };
    let t = Instant::now();
    let report = metrocalk_editor_shell::read_cad(&bytes).expect("read_cad");
    eprintln!(
        "read: {:.1}s — {} parts, {} meshes",
        t.elapsed().as_secs_f64(),
        report.parts.len(),
        report.meshes.len()
    );

    let mut total_tris = 0usize;
    let mut times: Vec<(f64, usize, usize)> = Vec::new(); // (secs, tris, mesh index)
    let t_all = Instant::now();
    for (i, m) in report.meshes.iter().enumerate() {
        let t = Instant::now();
        let asset =
            metrocalk_editor_shell::csg_intent::trimesh_to_mesh_asset_colored(&m.tris, "cad", None);
        let _gpu = metrocalk_assets::gpu::MeshGpu::from_asset(&asset);
        let dt = t.elapsed().as_secs_f64();
        let tris = m.tris.triangles.len();
        total_tris += tris;
        times.push((dt, tris, i));
        if dt > 5.0 {
            eprintln!("  mesh {i}: {tris} tris → {dt:.1}s");
        }
    }
    let all = t_all.elapsed().as_secs_f64();
    times.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    eprintln!(
        "register (serial): {:.1}s total over {} meshes / {} tris",
        all,
        report.meshes.len(),
        total_tris
    );
    for (dt, tris, i) in times.iter().take(5) {
        eprintln!("  slowest: mesh {i} — {tris} tris, {dt:.2}s");
    }
}
