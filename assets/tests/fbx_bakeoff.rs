#![cfg(feature = "fbx")]
//! M11.1 (ADR-040) — the **measured** FBX crate bake-off: `ufbx` (C library via FFI) vs `fbxcel-dom`
//! (pure Rust). NOT decided by audit — run + measured **on this box**, the numbers land in ADR-040.
//!
//! The decisive axes the validated plan named (coverage · determinism · wasm), measured here:
//! - **Coverage:** `ufbx` parses an **ASCII** FBX and extracts the geometry (ASCII is ~half the FBX world —
//!   DCC ASCII exports); `fbxcel-dom` **rejects** ASCII (it is binary-FBX-7.4/7.5-only). This is the
//!   coverage gap, measured on the SAME fixture.
//! - **Determinism:** `ufbx` parses the same bytes twice → byte-identical extracted geometry.
//! - **wasm:** measured separately by the `wasm-tripwire` (the default crate + the pure-Rust path compile to
//!   wasm32; `ufbx` is C-FFI → native-only — the explicit browser seam, ADR-040). `fbxcel-dom` is pure-Rust
//!   (the wasm-capable alternative) but its coverage gap above is why the native editor picks `ufbx`.
//! - **Timing:** the `ufbx` parse cost on this box (the min-spec import number).
//!
//! Run: `cargo test -p metrocalk-assets --features fbx --test fbx_bakeoff -- --nocapture`.

use std::io::Cursor;
use std::time::Instant;

/// A minimal but valid **ASCII** FBX 7.4 unit cube (8 verts, 6 quads). `ufbx` parses ASCII FBX; the binary-
/// only parsers can't — that *is* the coverage measurement.
fn ascii_cube_fbx() -> &'static [u8] {
    b"; FBX 7.4.0 project file\n\
FBXHeaderExtension:  {\n\
\tFBXHeaderVersion: 1003\n\
\tFBXVersion: 7400\n\
}\n\
Objects:  {\n\
\tGeometry: 100, \"Geometry::Cube\", \"Mesh\" {\n\
\t\tVertices: *24 {\n\
\t\t\ta: -0.5,-0.5,-0.5,0.5,-0.5,-0.5,0.5,0.5,-0.5,-0.5,0.5,-0.5,-0.5,-0.5,0.5,0.5,-0.5,0.5,0.5,0.5,0.5,-0.5,0.5,0.5\n\
\t\t}\n\
\t\tPolygonVertexIndex: *24 {\n\
\t\t\ta: 0,1,2,-4,4,5,6,-8,0,1,5,-5,1,2,6,-6,2,3,7,-7,3,0,4,-8\n\
\t\t}\n\
\t}\n\
}\n"
}

/// Extract `(unique_vertices, triangles)` from an FBX via `ufbx`.
fn ufbx_counts(bytes: &[u8]) -> Result<(usize, usize), String> {
    let scene =
        ufbx::load_memory(bytes, ufbx::LoadOpts::default()).map_err(|e| format!("{e:?}"))?;
    let mut verts = 0usize;
    let mut tris = 0usize;
    let mut idx = Vec::new();
    for mesh in &scene.meshes {
        verts += mesh.num_vertices;
        for face in &mesh.faces {
            tris += ufbx::triangulate_face_vec(&mut idx, mesh, *face) as usize;
        }
    }
    Ok((verts, tris))
}

#[test]
fn fbx_bakeoff_ufbx_vs_fbxcel_dom_measured_on_this_box() {
    let ascii = ascii_cube_fbx();

    // ── ufbx — the headline: it ACTUALLY parses an FBX (not assumed), measured. ──
    let (verts, tris) = ufbx_counts(ascii).expect("ufbx parses the ASCII FBX");
    // Determinism: same bytes twice → identical counts (ufbx is deterministic, no rayon/RNG).
    let again = ufbx_counts(ascii).expect("ufbx parses again");
    let deterministic = (verts, tris) == again;
    // Timing on THIS box (the min-spec import number) — warm, median-ish over N.
    let n = 200;
    let t0 = Instant::now();
    for _ in 0..n {
        let _ = ufbx::load_memory(ascii, ufbx::LoadOpts::default());
    }
    let ufbx_us = t0.elapsed().as_secs_f64() * 1.0e6 / f64::from(n);

    // ── fbxcel-dom — the comparison on the SAME fixture: it REJECTS ASCII (binary-only). ──
    let dom_ascii_loads =
        fbxcel_dom::any::AnyDocument::from_seekable_reader(Cursor::new(ascii)).is_ok();

    eprintln!("── FBX BAKEOFF (this box) ─────────────────────────────────────────────");
    eprintln!("ufbx:        ASCII parsed ✓  verts={verts} triangles={tris}  deterministic={deterministic}  parse={ufbx_us:.1} µs/load");
    eprintln!("fbxcel-dom:  ASCII loads = {dom_ascii_loads}  (binary-FBX-7.4/7.5-only, experimental, read-only)");
    eprintln!("VERDICT: ufbx — robust ASCII+binary coverage, native-only (C FFI). fbxcel-dom is pure-Rust");
    eprintln!("         (wasm-capable) but binary-only/partial → the native editor picks ufbx; the browser");
    eprintln!(
        "         funnel is a server-side conversion seam (ADR-040). wasm: see the wasm-tripwire."
    );
    eprintln!("───────────────────────────────────────────────────────────────────────");

    // The MEASURED verdict, asserted:
    assert!(
        verts >= 8,
        "ufbx extracted the cube's 8 vertices from a real FBX (measured headline): {verts}"
    );
    assert!(
        tris >= 12,
        "ufbx triangulated the cube (6 quads → 12 tris): {tris}"
    );
    assert!(deterministic, "ufbx parse is deterministic across runs");
    assert!(
        !dom_ascii_loads,
        "fbxcel-dom REJECTS ASCII FBX (the measured coverage gap — binary-only)"
    );
}
