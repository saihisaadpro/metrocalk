//! Import latency — the one-shot heavy op, measured (never frame-budget-gated, never extrapolated).
//! Release is the metric (`cargo test -p metrocalk-assets --release --test import_bench -- --nocapture`);
//! a debug run is not the benchmark. Reports the cold one-shot import of each checked-in fixture plus a
//! warmed median/p99, and packs each to GPU geometry. The fixtures are tiny *own* demo meshes — this
//! measures the real assets we have, it does not extrapolate to a hypothetical MB-scale model.

#![allow(clippy::cast_precision_loss)]

use metrocalk_assets::{GltfImporter, MeshGpu, MeshSource};

const HEALTHBAR_GLB: &[u8] = include_bytes!("../../editor-shell/assets/healthbar.glb");
const PROP_GLB: &[u8] = include_bytes!("../../editor-shell/assets/prop.glb");

fn percentiles(mut us: Vec<f64>) -> (f64, f64) {
    us.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (us[us.len() / 2], us[us.len() * 99 / 100])
}

#[test]
fn import_one_shot_latency() {
    let importer = GltfImporter::new();
    for (name, bytes) in [("healthbar.glb", HEALTHBAR_GLB), ("prop.glb", PROP_GLB)] {
        // Cold one-shot — the number that matters for "import is a one-shot op".
        let t0 = std::time::Instant::now();
        let asset = importer.import(bytes).expect("import");
        let cold_us = t0.elapsed().as_secs_f64() * 1e6;

        // Warmed distribution.
        for _ in 0..50 {
            let _ = importer.import(bytes).unwrap();
        }
        let mut samples = Vec::new();
        for _ in 0..500 {
            let t = std::time::Instant::now();
            let a = importer.import(bytes).unwrap();
            std::hint::black_box(&a);
            samples.push(t.elapsed().as_secs_f64() * 1e6);
        }
        let (p50, p99) = percentiles(samples);

        // Pack to GPU geometry (the render-data prep) — measured separately.
        let tp = std::time::Instant::now();
        let gpu = MeshGpu::from_asset(&asset);
        let pack_us = tp.elapsed().as_secs_f64() * 1e6;

        eprintln!(
            "[M4-import] {name}: {} bytes → {} verts / {} tris · cold {cold_us:.1}us · warm p50 {p50:.1}us p99 {p99:.1}us · pack {pack_us:.1}us ({} gpu verts)",
            bytes.len(),
            asset.vertex_count(),
            asset.triangle_count(),
            gpu.vertex_count(),
        );

        // One-shot, well under any frame budget — this is a load-time op, not a per-frame one.
        assert!(
            p99 < 16_000.0,
            "import is a cheap one-shot, not a frame-budget risk"
        );
    }
}
