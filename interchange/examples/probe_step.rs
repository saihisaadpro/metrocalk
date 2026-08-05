//! Throwaway probe: read a STEP file via the assembly reader and report part count + spatial spread,
//! so the B-rep placement can be validated without the full app rebuild. `cargo run --example probe_step -- <path>`.
use metrocalk_interchange::{CadReader, StepAssemblyReader};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: probe_step <path.stp>");
    let bytes = std::fs::read(&path).expect("read file");
    eprintln!("read {} bytes from {path}", bytes.len());
    let t0 = std::time::Instant::now();
    let imp = StepAssemblyReader.read(&bytes).expect("parse STEP");
    eprintln!("parsed in {:?}", t0.elapsed());
    eprintln!("{}", imp.summary());

    let parts = &imp.parts;
    eprintln!("parts: {}", parts.len());
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    let mut tri_total = 0usize;
    let mut with_geom = 0usize;
    let mut with_color = 0usize;
    for p in parts {
        let t = metrocalk_interchange::translation_of(&p.transform);
        for k in 0..3 {
            if t[k].is_finite() {
                lo[k] = lo[k].min(t[k]);
                hi[k] = hi[k].max(t[k]);
            }
        }
        if let Some(mi) = p.mesh {
            let m = &imp.meshes[mi];
            let n = m.tris.triangle_count();
            tri_total += n;
            if n > 0 {
                with_geom += 1;
            }
        }
        if p.color.is_some() {
            with_color += 1;
        }
    }
    eprintln!(
        "placement bbox (mm): X[{:.0},{:.0}] Y[{:.0},{:.0}] Z[{:.0},{:.0}]",
        lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]
    );
    eprintln!(
        "span: {:.0} x {:.0} x {:.0} mm",
        hi[0] - lo[0],
        hi[1] - lo[1],
        hi[2] - lo[2]
    );
    eprintln!(
        "parts with geometry: {with_geom}, with colour: {with_color}, total triangles: {tri_total}"
    );
    // sample a few part positions
    for p in parts.iter().take(6) {
        let t = metrocalk_interchange::translation_of(&p.transform);
        eprintln!("  '{}' @ ({:.0},{:.0},{:.0})", p.name, t[0], t[1], t[2]);
    }
}
