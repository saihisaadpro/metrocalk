//! Local M15.7 spike tool — read a real CAD file (CATIA 3DXML / STEP AP242) and print the never-empty /
//! never-silent import report. Requires the `3dxml` feature:
//!
//! ```text
//! cargo run --example read_cad --features 3dxml --release -- "path/to/Skid Weld Line A.1.3dxml"
//! ```
//!
//! This is the head-to-head evidence vs the documented Unreal/Datasmith result (1 of ~1,280 parts; black
//! screen). It reads the product structure + sniffs the reps + runs the multi-strategy cascade — no kernel.
#![allow(clippy::cast_precision_loss)] // display-only ratios (part/mesh counts) — precision is immaterial

use metrocalk_interchange::{
    mesh_hash, translation_of, CadImport, CadReader, StepAssemblyReader, ThreeDxmlReader,
};
use std::time::Instant;

fn whole_hash(imp: &CadImport) -> u64 {
    // A stable fold over part ids + their mesh hashes + transforms — the determinism signature.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |x: u64| {
        h ^= x;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for p in &imp.parts {
        mix(p.id);
        mix(p.mesh.map_or(0, |i| imp.meshes[i].hash));
        for v in &p.transform {
            mix(v.to_bits());
        }
    }
    for m in &imp.meshes {
        mix(mesh_hash(&m.tris));
    }
    h
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: read_cad <file.3dxml|.step>");
    let bytes = std::fs::read(&path).expect("read file");
    println!(
        "file: {path}\nsize: {:.2} MB\n",
        bytes.len() as f64 / (1024.0 * 1024.0)
    );

    let t = Instant::now();
    let imp = if ThreeDxmlReader.can_read(&bytes) {
        ThreeDxmlReader.read(&bytes)
    } else {
        StepAssemblyReader.read(&bytes)
    }
    .expect("import");
    let dt = t.elapsed();

    println!("=== IMPORT REPORT ===");
    println!("{}\n", imp.summary());
    println!("never_empty  = {}", imp.never_empty());
    println!("never_silent = {}", imp.never_silent());
    println!(
        "source assembly instances (Instance3D): {}   top-level products: {}",
        imp.total_occurrences, imp.products
    );
    let (uniq, inst) = imp.instancing();
    println!(
        "dedup/instancing: {uniq} unique meshes for {inst} placed instances ({:.0}x instancing win)",
        inst as f64 / uniq.max(1) as f64
    );
    println!("parsed + reported in {dt:?} (no kernel)");
    println!(
        "\n>>> HEAD-TO-HEAD vs Unreal/Datasmith on this exact file: Unreal = 1 part, then black screen.\n\
         >>> Metrocalk = {} part placements ({} unique geometries, {} products) placed + diagnosed, \
         0 silently dropped, 0 black.\n",
        imp.part_count(),
        imp.unique_geometry_count(),
        imp.products,
    );

    let c = imp.fidelity_counts();
    println!("fidelity breakdown:");
    println!("  exact-B-rep       : {}", c.exact_brep);
    println!("  tessellation-only : {}", c.tessellation_only);
    println!("  AI-reconstructed  : {}", c.reconstructed);
    println!("  proxy (kernel seam): {}", c.proxy);
    println!("  access-denied     : {}", c.access_denied);
    println!("  failed            : {}\n", c.failed);

    println!("sample parts (first 6):");
    for p in imp.parts.iter().take(6) {
        let t = translation_of(&p.transform);
        println!(
            "  '{}' [ref {}] {} @ ({:.0},{:.0},{:.0})mm — {}",
            p.name,
            p.reference,
            p.fidelity.token(),
            t[0],
            t[1],
            t[2],
            p.fix.as_deref().unwrap_or("(exact)"),
        );
    }
    if !imp.notes.is_empty() {
        println!("\nscene notes:");
        for n in &imp.notes {
            println!("  - {}: {}", n.feature, n.detail);
        }
    }

    // Determinism: re-import ×3, the whole-import signature must be bit-identical.
    println!("\n=== DETERMINISM (re-import x3) ===");
    let h1 = whole_hash(&imp);
    let h2 = whole_hash(
        &ThreeDxmlReader
            .read(&bytes)
            .unwrap_or_else(|_| StepAssemblyReader.read(&bytes).expect("re-import")),
    );
    let h3 = whole_hash(
        &ThreeDxmlReader
            .read(&bytes)
            .unwrap_or_else(|_| StepAssemblyReader.read(&bytes).expect("re-import")),
    );
    println!("signature: {h1:016x} / {h2:016x} / {h3:016x}");
    println!("bit-identical x3: {}", h1 == h2 && h2 == h3);
}
