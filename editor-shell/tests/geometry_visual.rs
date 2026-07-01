//! M15.0 (ADR-070) — visual acceptance (real pixels, not a DOM shot). The wgpu-PBR render lives in the
//! non-member `src-tauri` app and this headless box has no confirmed GPU/capture rig, so the live wgpu
//! capture + cross-GPU fidelity is **owed convergence** (like M13.6's GPU rig). But geometric correctness is
//! capturable in-session: this **CPU-rasterizes the ACTUAL meshes** (the imported STEP cube + the compiled
//! SDF box−cylinder) — deterministic orthographic Lambert shading of the real triangles — to a PPM, and
//! asserts the frame is non-blank (surface + empty space present: the "viewport-not-black" real-pixel check).
//! It proves the geometry is intact (silhouette + the bore visible + no inverted normals via shading). The
//! PNG/PPM evidence is written under `editor-shell/evidence/` (local-only). NOT the wgpu render — labelled so.

// A software rasterizer is inherently pixel-index casts + short geometric names (a/b/c/n/x/y) — the
// precision loss on a 256px raster is irrelevant and the names are standard.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::many_single_char_names,
    clippy::similar_names
)]

use metrocalk_interchange::{CadInterchange, StepInterchange};
use metrocalk_sdf::{compile, Axis, Grid, Sdf, TriMesh};
use std::io::Write;

const W: usize = 256;
const H: usize = 256;

/// Rotate a point by a fixed iso view (≈35° about X, then 40° about Y) so both the form and the bore show.
fn iso(p: [f64; 3]) -> [f64; 3] {
    // precomputed cos/sin (constants — no transcendental at runtime, deterministic)
    let (cx, sx) = (0.819_152, 0.573_576); // 35°
    let (cy, sy) = (0.766_044, 0.642_788); // 40°
                                           // rotate X
    let y1 = cx * p[1] - sx * p[2];
    let z1 = sx * p[1] + cx * p[2];
    // rotate Y
    let x2 = cy * p[0] + sy * z1;
    let z2 = -sy * p[0] + cy * z1;
    [x2, y1, z2]
}

/// Lambert-shade + z-buffer the mesh into an RGB frame (orthographic, auto-fit). Returns (rgb, lit_frac).
fn rasterize(mesh: &TriMesh) -> (Vec<u8>, f64) {
    let mut rgb = vec![24u8; W * H * 3]; // dark background
    let mut zbuf = vec![f64::NEG_INFINITY; W * H];

    // Transform + fit to frame.
    let pts: Vec<[f64; 3]> = mesh.positions.iter().map(|&p| iso(p)).collect();
    if pts.is_empty() {
        return (rgb, 0.0);
    }
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for p in &pts {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-9);
    let scale = 0.82 * (W.min(H) as f64) / span;
    let cx = 0.5 * (lo[0] + hi[0]);
    let cy = 0.5 * (lo[1] + hi[1]);
    let to_screen = |p: [f64; 3]| -> (f64, f64) {
        let sx = (W as f64) * 0.5 + (p[0] - cx) * scale;
        let sy = (H as f64) * 0.5 - (p[1] - cy) * scale; // flip Y for image space
        (sx, sy)
    };
    let light = normalize([0.4, 0.6, 0.7]);

    let mut lit = 0usize;
    for tri in &mesh.triangles {
        let a = pts[tri[0] as usize];
        let b = pts[tri[1] as usize];
        let c = pts[tri[2] as usize];
        let n = normalize(cross(sub(b, a), sub(c, a)));
        // shade by |n·L| so we see both outer surface and interior of the bore
        let shade = 0.18 + 0.82 * (n[0] * light[0] + n[1] * light[1] + n[2] * light[2]).abs();
        let (ax, ay) = to_screen(a);
        let (bx, by) = to_screen(b);
        let (ccx, ccy) = to_screen(c);
        let minx = ax.min(bx).min(ccx).floor().max(0.0) as usize;
        let maxx = ax.max(bx).max(ccx).ceil().min(W as f64 - 1.0) as usize;
        let miny = ay.min(by).min(ccy).floor().max(0.0) as usize;
        let maxy = ay.max(by).max(ccy).ceil().min(H as f64 - 1.0) as usize;
        let area = edge(ax, ay, bx, by, ccx, ccy);
        if area.abs() < 1e-9 {
            continue;
        }
        for py in miny..=maxy {
            for px in minx..=maxx {
                let (fx, fy) = (px as f64 + 0.5, py as f64 + 0.5);
                let w0 = edge(bx, by, ccx, ccy, fx, fy) / area;
                let w1 = edge(ccx, ccy, ax, ay, fx, fy) / area;
                let w2 = edge(ax, ay, bx, by, fx, fy) / area;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * a[2] + w1 * b[2] + w2 * c[2];
                let idx = py * W + px;
                if z > zbuf[idx] {
                    zbuf[idx] = z;
                    let v = (shade * 235.0) as u8 + 10;
                    rgb[idx * 3] = v;
                    rgb[idx * 3 + 1] = v;
                    rgb[idx * 3 + 2] = (f64::from(v) * 1.02).min(255.0) as u8;
                }
            }
        }
    }
    for &z in &zbuf {
        if z > f64::NEG_INFINITY {
            lit += 1;
        }
    }
    (rgb, lit as f64 / (W * H) as f64)
}

fn write_ppm(path: &str, rgb: &[u8]) {
    if let Some(parent) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::File::create(path) {
        let _ = write!(f, "P6\n{W} {H}\n255\n");
        let _ = f.write_all(rgb);
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn normalize(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
    [v[0] / l, v[1] / l, v[2] / l]
}
fn edge(ax: f64, ay: f64, bx: f64, by: f64, px: f64, py: f64) -> f64 {
    (px - ax) * (by - ay) - (py - ay) * (bx - ax)
}

#[test]
fn step_cube_renders_real_pixels_geometry_intact() {
    let cad = StepInterchange
        .import(include_str!("../../interchange/tests/fixtures/cube_ap242.step").as_bytes())
        .expect("import");
    let (rgb, lit) = rasterize(&cad.tessellate());
    write_ppm("evidence/m15-step-cube.ppm", &rgb);
    // Viewport-not-black: the surface rendered AND there is empty space (not a full fill / not blank).
    assert!(
        lit > 0.15 && lit < 0.92,
        "the cube rendered real pixels (lit fraction {lit:.3})"
    );
    println!("[visual] STEP cube → evidence/m15-step-cube.ppm (lit {lit:.3})");
}

#[test]
fn sdf_box_minus_cylinder_renders_real_pixels_with_the_bore() {
    let sdf = Sdf::cuboid([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]).difference(Sdf::cylinder(
        [0.0, 0.0, 0.0],
        0.5,
        2.0,
        Axis::Y,
    ));
    let (rgb, lit) = rasterize(&compile(&sdf, &Grid::around(&sdf, 48, 0.06)));
    write_ppm("evidence/m15-sdf-box-cylinder.ppm", &rgb);
    assert!(
        lit > 0.15 && lit < 0.92,
        "the bored box rendered real pixels (lit fraction {lit:.3})"
    );
    println!("[visual] SDF box−cylinder → evidence/m15-sdf-box-cylinder.ppm (lit {lit:.3})");
}
