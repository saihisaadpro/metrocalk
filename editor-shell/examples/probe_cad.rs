//! Headless CAD import diagnostic for validating a real source file without starting the desktop UI.

use metrocalk_editor_shell::read_cad_with_companion;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: cargo run -p metrocalk-editor-shell --example probe_cad -- <file>");
    let source_path = std::path::Path::new(&path);
    let bytes = std::fs::read(source_path).expect("read CAD source");
    let started = std::time::Instant::now();
    let report = read_cad_with_companion(source_path, &bytes).expect("import CAD source");

    let unique_triangles: usize = report
        .meshes
        .iter()
        .map(|mesh| mesh.tris.triangle_count())
        .sum();
    println!("{}", report.summary());
    println!("elapsed_ms={}", started.elapsed().as_millis());
    println!("groups={}", report.groups.len());
    println!("unique_meshes={}", report.meshes.len());
    println!("unique_triangles={unique_triangles}");
    println!("source_hash={}", report.source_hash);
    for note in &report.notes {
        println!("note={note:?}");
    }
}
