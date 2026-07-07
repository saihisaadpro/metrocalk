//! Headless CPU rasterizer for the whole STEP assembly — renders the imported parts (real geometry +
//! per-part colour) to PPM images with NO GPU/desktop (works while the screen is locked). This is a
//! geometry+placement proof (deterministic iso/top/front Lambert shading of the real triangles), NOT the
//! wgpu-PBR render. `cargo run --release --example render_step -- <path.stp> <out_dir>`.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    // Rasterizer math reads best in its conventional notation: triangle verts a/b/c, normal n,
    // per-axis min/max bounds — the "similar/single-char names" pedantic lints fight that idiom.
    clippy::similar_names,
    clippy::many_single_char_names
)]
use metrocalk_interchange::{CadReader, StepAssemblyReader};
use std::io::Write;

const W: usize = 1100;
const H: usize = 720;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: render_step <path.stp> <out_dir>");
    let out_dir = std::env::args().nth(2).unwrap_or_else(|| ".".into());
    let bytes = std::fs::read(&path).expect("read");
    let imp = StepAssemblyReader.read(&bytes).expect("parse STEP");
    eprintln!("{}", imp.summary());

    // Gather world-space triangles + a per-triangle colour (from each part's authored STEP colour).
    let mut verts: Vec<[f64; 3]> = Vec::new();
    let mut tris: Vec<[u32; 3]> = Vec::new();
    let mut tri_col: Vec<[f32; 3]> = Vec::new();
    for p in &imp.parts {
        let Some(mi) = p.mesh else { continue };
        let m = &imp.meshes[mi];
        let base = verts.len() as u32;
        for v in &m.tris.positions {
            verts.push(apply(&p.transform, *v));
        }
        let col = p.color.unwrap_or([0.60, 0.61, 0.63]);
        for t in &m.tris.triangles {
            tris.push([t[0] + base, t[1] + base, t[2] + base]);
            tri_col.push(col);
        }
    }
    eprintln!("world tris: {}, verts: {}", tris.len(), verts.len());
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for v in &verts {
        for k in 0..3 {
            lo[k] = lo[k].min(v[k]);
            hi[k] = hi[k].max(v[k]);
        }
    }
    eprintln!(
        "world bbox (mm): X[{:.0},{:.0}] Y[{:.0},{:.0}] Z[{:.0},{:.0}]",
        lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]
    );

    for (name, rot) in [
        ("iso", iso as fn([f64; 3]) -> [f64; 3]),
        ("top", top),
        ("front", front),
    ] {
        let rgb = rasterize(&verts, &tris, &tri_col, rot);
        let out = format!("{out_dir}/step_{name}.ppm");
        write_ppm(&out, &rgb);
        eprintln!("wrote {out}");
    }
}

fn apply(m: &[f64; 16], p: [f64; 3]) -> [f64; 3] {
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

fn iso(p: [f64; 3]) -> [f64; 3] {
    let (cx, sx) = (0.819_152, 0.573_576);
    let (cy, sy) = (0.766_044, 0.642_788);
    let y1 = cx * p[1] - sx * p[2];
    let z1 = sx * p[1] + cx * p[2];
    let x2 = cy * p[0] + sy * z1;
    let z2 = -sy * p[0] + cy * z1;
    [x2, y1, z2]
}
// Top view: look down −Z (X right, Y up in image).
fn top(p: [f64; 3]) -> [f64; 3] {
    [p[0], p[1], p[2]]
}
// Front view: look along −Y (X right, Z up).
fn front(p: [f64; 3]) -> [f64; 3] {
    [p[0], p[2], -p[1]]
}

fn rasterize(
    verts: &[[f64; 3]],
    tris: &[[u32; 3]],
    tri_col: &[[f32; 3]],
    rot: fn([f64; 3]) -> [f64; 3],
) -> Vec<u8> {
    let mut rgb = vec![26u8; W * H * 3];
    let mut zbuf = vec![f64::NEG_INFINITY; W * H];
    let pts: Vec<[f64; 3]> = verts.iter().map(|&p| rot(p)).collect();
    if pts.is_empty() {
        return rgb;
    }
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for p in &pts {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1e-9);
    let scale = 0.90 * (W.min(H) as f64) / span;
    let cx = 0.5 * (lo[0] + hi[0]);
    let cy = 0.5 * (lo[1] + hi[1]);
    let to_screen = |p: [f64; 3]| -> (f64, f64) {
        (
            (W as f64) * 0.5 + (p[0] - cx) * scale,
            (H as f64) * 0.5 - (p[1] - cy) * scale,
        )
    };
    let light = normalize([0.35, 0.55, 0.75]);
    for (ti, tri) in tris.iter().enumerate() {
        let a = pts[tri[0] as usize];
        let b = pts[tri[1] as usize];
        let c = pts[tri[2] as usize];
        let n = normalize(cross(sub(b, a), sub(c, a)));
        let shade = 0.22 + 0.78 * (n[0] * light[0] + n[1] * light[1] + n[2] * light[2]).abs();
        let col = tri_col[ti];
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
                    // linear->approx sRGB gamma + Lambert
                    let g = |c: f32| ((c * shade as f32).powf(0.4545) * 255.0).min(255.0) as u8;
                    rgb[idx * 3] = g(col[0]);
                    rgb[idx * 3 + 1] = g(col[1]);
                    rgb[idx * 3 + 2] = g(col[2]);
                }
            }
        }
    }
    rgb
}

fn write_ppm(path: &str, rgb: &[u8]) {
    if let Some(p) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let mut f = std::fs::File::create(path).expect("create");
    let _ = write!(f, "P6\n{W} {H}\n255\n");
    let _ = f.write_all(rgb);
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
