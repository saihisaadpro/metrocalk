//! M15.11 real-file evidence: run the AAG semantic pass over a STEP file and print the honest numbers
//! (features by kind · classifications · instance groups · timing). Usage:
//! `cargo run --release -p metrocalk-interchange --example recognize_step -- <file.stp>`

use std::collections::BTreeMap;
use std::time::Instant;

use metrocalk_interchange::{
    classify_part, cluster_instances, identities, AagRecognizer, CadReader, FeatureRecognizer,
    PartClass, StepAssemblyReader,
};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: recognize_step <file.stp>");
    let bytes = std::fs::read(&path).expect("read the file");
    let t0 = Instant::now();
    let import = StepAssemblyReader.read(&bytes).expect("parse");
    let t_parse = t0.elapsed();

    let t1 = Instant::now();
    let mut features_by_kind: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut parts_with_features = 0usize;
    let mut recognized_refs = 0usize;
    for faces in import.breps.values() {
        recognized_refs += 1;
        let feats = AagRecognizer.recognize(faces);
        if !feats.is_empty() {
            parts_with_features += 1;
        }
        for f in feats {
            *features_by_kind.entry(f.kind.token()).or_default() += 1;
        }
    }
    let t_recognize = t1.elapsed();

    let t2 = Instant::now();
    let mut classes: BTreeMap<&'static str, usize> = BTreeMap::new();
    for p in &import.parts {
        let faces = import.breps.get(&p.reference).map(Vec::as_slice);
        let mesh = p.mesh.and_then(|i| import.meshes.get(i));
        let Some(mesh) = mesh else { continue };
        let c = classify_part(faces, &mesh.tris, &[], &p.name);
        if c.class != PartClass::Unknown {
            *classes.entry(c.class.token()).or_default() += 1;
        }
    }
    let t_classify = t2.elapsed();

    let ids = identities(&import);
    let groups = cluster_instances(&ids);
    let repeated = groups.iter().filter(|g| g.members.len() > 1).count();
    let hist_nonzero = ids
        .iter()
        .filter(|i| i.fingerprint.surface_hist.iter().any(|&c| c > 0))
        .count();

    println!("file: {path}");
    println!(
        "parts: {} placements · {} unique geometries · {} with decoded B-rep faces",
        import.part_count(),
        import.unique_geometry_count(),
        recognized_refs
    );
    println!("parse {t_parse:.2?} · recognize {t_recognize:.2?} · classify {t_classify:.2?}");
    println!("features (per unique geometry): {features_by_kind:?} — on {parts_with_features} geometries");
    println!("classes (per placement, Unknown omitted): {classes:?}");
    println!(
        "instance groups: {} total · {repeated} with >1 member · non-zero surface-hist identities: {hist_nonzero}/{}",
        groups.len(),
        ids.len()
    );
}
