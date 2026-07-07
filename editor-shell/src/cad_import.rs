//! M15.7 (ADR-077) — the **universal CAD import** editor-shell seam: land a [`CadImport`] (from the
//! `metrocalk-interchange` pipeline behind the [`CadReader`] trait) onto the substrate as **one undoable
//! transaction** (invariant 3) that is **never-empty + never-silent + substrate-native**.
//!
//! Every part becomes a **renderable entity** carrying a queryable `CadPart` component (name · reference ·
//! fidelity · strategy — the ECS-queryable per-part report, "show tessellation-only parts") + a units-
//! normalized `Transform` (its real assembly position, not the origin-collapse) + a content-addressed mesh
//! handle. **Geometry-hash dedup** means the many instances of one part share one stored mesh (GPU-instanced;
//! `/core` stays mesh-agnostic — geometry by handle, invariant 2). The import is **resumable/revertible** (one
//! Ctrl-Z peels the whole import) and **provenance-tracked** (source hash · format · per-part strategy). A
//! re-import is an **O(1) content-addressed diff** ([`reimport_diff`]) — "which of N parts changed."
//!
//! No proprietary-kernel / `zip` / STEP-lib type crosses this seam — the boundary is the neutral [`CadImport`]
//! behind the `CadReader` trait (invariant 5). The proprietary-CATIA-geometry decode stays the licensed-kernel
//! seam (ADR-070); this seam does the never-empty/never-silent/substrate-native half no incumbent has.

use crate::csg_intent::store_mesh;
use metrocalk_assets::AssetStore;
use metrocalk_core::caps::canonical;
use metrocalk_core::{Engine, EntityId, FieldValue, Op, PipelineError};
use metrocalk_ecs::FlecsWorld;
use metrocalk_interchange::{
    diff, translation_of, CadError, CadImport, CadReader, PartChange, PartDiff, StepAssemblyReader,
    ThreeDxmlReader,
};

use crate::capscene::{CapScene, MESH_FIELD};

/// The queryable per-part component the import writes onto each entity — the never-silent report, ECS-native.
pub const CAD_PART: &str = "CadPart";

/// What a universal CAD import landed: one entity per part + the neutral report (queryable). The report's
/// `parts[i]` aligns with `entities[i]`.
pub struct CadLanding {
    /// One entity per part placement (never-empty: every one has a placed mesh).
    pub entities: Vec<EntityId>,
    /// One geometry-free container entity per [`metrocalk_interchange::GroupNode`] (aligned with
    /// `report.groups`) — the source's named assembly tree, preserved (not flattened), exactly as the live
    /// app path lands it.
    pub group_entities: Vec<EntityId>,
    /// The neutral never-silent report (fidelity counts, notes, per-part diagnosis + fix).
    pub report: CadImport,
    /// The count of UNIQUE meshes stored (the dedup denominator — instances share these).
    pub unique_meshes: usize,
}

/// A universal CAD import that couldn't be landed.
#[derive(Debug)]
pub enum CadImportError {
    /// The file couldn't be read into a neutral scene (container error — explained, never a panic).
    Read(CadError),
    /// The commit was rejected by the pipeline.
    Commit(PipelineError),
}

impl std::fmt::Display for CadImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(e) => write!(f, "{e}"),
            Self::Commit(e) => write!(f, "commit rejected: {e:?}"),
        }
    }
}

impl std::error::Error for CadImportError {}

/// Cheap magic-byte sniff: are these bytes a CAD container this pipeline handles (CATIA 3DXML / STEP AP242)?
/// The live editor uses this to route a dropped/picked file to [`import_cad`] vs the mesh `import_any` path.
#[must_use]
pub fn is_cad_file(bytes: &[u8]) -> bool {
    ThreeDxmlReader.can_read(bytes) || StepAssemblyReader.can_read(bytes)
}

/// Read `bytes` into a neutral [`CadImport`], routing by content: CATIA 3DXML (ZIP) → the native 3DXML reader;
/// STEP AP242 (ISO-10303-21) → the pure-Rust neutral reader. Mesh formats (glTF/OBJ) ride the existing
/// `metrocalk_assets::import_any` path (already shipped) — this router owns the CAD tiers.
///
/// # Errors
/// [`CadError`] for an unrecognized/malformed container (never a panic).
pub fn read_cad(bytes: &[u8]) -> Result<CadImport, CadError> {
    if ThreeDxmlReader.can_read(bytes) {
        ThreeDxmlReader.read(bytes)
    } else if StepAssemblyReader.can_read(bytes) {
        StepAssemblyReader.read(bytes)
    } else {
        Err(CadError::Unrecognized(
            "not a recognized CAD container (CATIA 3DXML / STEP AP242); mesh formats route through \
             assets::import_any"
                .into(),
        ))
    }
}

/// Import a CAD file as ONE undoable transaction: every part → a renderable entity with a queryable `CadPart`
/// component (name/reference/fidelity/strategy) + a units-normalized `Transform` + a content-addressed mesh
/// handle (dedup → instancing). Never-empty (each part has a placed mesh) + never-silent (each carries its
/// diagnosis in the report). One Ctrl-Z peels the whole import.
///
/// # Errors
/// [`CadImportError::Read`] if the container can't be parsed; [`CadImportError::Commit`] if the commit is
/// rejected.
pub fn import_cad(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    store: &mut AssetStore,
    bytes: &[u8],
) -> Result<CadLanding, CadImportError> {
    let report = read_cad(bytes).map_err(CadImportError::Read)?;
    land_import(engine, scene, store, report)
}

/// Land an already-read [`CadImport`] onto the substrate (split out so tests can land a synthetic report).
///
/// # Errors
/// [`CadImportError::Commit`] if the commit is rejected.
pub fn land_import(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    store: &mut AssetStore,
    report: CadImport,
) -> Result<CadLanding, CadImportError> {
    // Store each UNIQUE mesh as a content-addressed asset (the store dedups by content too — belt & braces).
    let handles: Vec<String> = report
        .meshes
        .iter()
        .map(|m| store_mesh(store, &m.tris, "cad"))
        .collect();

    // Units: the reader declares metres-per-unit (STEP/3DXML are mm → 0.001). Normalize positions to the
    // scene's canonical unit so a 15,540 mm crane part lands at 15.54, not 15 km (the 10× trap, handled).
    let m_per_unit = report.units.meters_per_unit;
    let renderable = scene.caps.get(&canonical("Renderable")).copied();

    let mut ops: Vec<Op> = Vec::with_capacity(report.parts.len() * 8 + report.groups.len() * 7);

    // The NAMED structural tree first (report.groups is topological, parent-before-child): one geometry-
    // free identity-transform container per assembly occurrence, marked `__meta__.kind = "group"` — the
    // source's exact hierarchy/grouping/names, never flattened (mirrors the live `land_cad` path).
    let mut group_entities = Vec::with_capacity(report.groups.len());
    let mut src_to_entity: std::collections::BTreeMap<u64, EntityId> =
        std::collections::BTreeMap::new();
    for g in &report.groups {
        let ge = engine.alloc_entity_id();
        src_to_entity.insert(g.id, ge);
        let parent = g.parent.and_then(|pid| src_to_entity.get(&pid).copied());
        ops.push(Op::CreateEntity { id: ge, parent });
        for (f, v) in [("x", 0.0), ("y", 0.0), ("z", 0.0), ("scale", 1.0)] {
            ops.push(Op::SetField {
                entity: ge,
                component: "Transform".into(),
                field: f.into(),
                value: FieldValue::Number(v),
            });
        }
        if !g.name.is_empty() {
            ops.push(Op::SetField {
                entity: ge,
                component: metrocalk_core::variant::INSTANCE_META.into(),
                field: "name".into(),
                value: FieldValue::Str(g.name.clone()),
            });
        }
        ops.push(Op::SetField {
            entity: ge,
            component: metrocalk_core::variant::INSTANCE_META.into(),
            field: "kind".into(),
            value: FieldValue::Str("group".into()),
        });
        group_entities.push(ge);
    }

    let mut entities = Vec::with_capacity(report.parts.len());
    for p in &report.parts {
        let e = engine.alloc_entity_id();
        ops.push(Op::CreateEntity {
            id: e,
            parent: p.parent.and_then(|pid| src_to_entity.get(&pid).copied()),
        });
        // Real placement (units-normalized) — the pivot/position, never the assembly-origin collapse. Both
        // the placement translation AND the mesh geometry live in the source units (mm), so we scale BOTH to
        // the scene's metres: the translation by `m_per_unit`, and the mesh via the entity's uniform `scale`
        // (else a real mm-valued mesh would render ~1000× oversized relative to its metric placement).
        let t = translation_of(&p.transform);
        for (f, v) in [
            ("x", t[0] * m_per_unit),
            ("y", t[1] * m_per_unit),
            ("z", t[2] * m_per_unit),
            ("scale", m_per_unit),
        ] {
            ops.push(Op::SetField {
                entity: e,
                component: "Transform".into(),
                field: f.into(),
                value: FieldValue::Number(v),
            });
        }
        // The content-addressed mesh (real geometry or the shared proxy — never-empty).
        if let Some(mi) = p.mesh {
            ops.push(Op::SetField {
                entity: e,
                component: "MeshRenderer".into(),
                field: MESH_FIELD.into(),
                value: FieldValue::Str(handles[mi].clone()),
            });
        }
        // The queryable per-part report, ECS-native (the never-silent record — "show tessellation-only parts").
        for (field, value) in [
            ("name", p.name.clone()),
            ("reference", p.reference.clone()),
            ("fidelity", p.fidelity.token().to_string()),
            ("strategy", p.strategy.token().to_string()),
        ] {
            ops.push(Op::SetField {
                entity: e,
                component: CAD_PART.into(),
                field: field.into(),
                value: FieldValue::Str(value),
            });
        }
        if let Some(c) = renderable {
            ops.push(Op::AddPair {
                entity: e,
                rel: scene.rels.provides,
                target: c,
            });
        }
        entities.push(e);
    }

    engine
        .commit("import-cad", ops)
        .map_err(CadImportError::Commit)?;

    Ok(CadLanding {
        unique_meshes: handles.len(),
        entities,
        group_entities,
        report,
    })
}

/// Whether a column-major 4×4's 3×3 basis is a **proper rigid rotation** (unit-length, mutually
/// orthogonal columns, det > 0) — the precondition for the exact trace quaternion conversion. STEP
/// `AXIS2_PLACEMENT_3D` frames always are; a CATIA 3DXML instance chain can carry a **mirror** (symmetry
/// instances, det < 0) or scale in the basis, which NO quaternion represents — feeding one to the trace
/// formulas emits a plausible-looking but silently wrong rotation.
#[must_use]
pub fn basis_is_rigid(m: &[f64; 16]) -> bool {
    let col = |i: usize| [m[i * 4], m[i * 4 + 1], m[i * 4 + 2]];
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let (x, y, z) = (col(0), col(1), col(2));
    let eps = 1e-4;
    let det = x[0] * (y[1] * z[2] - y[2] * z[1]) - y[0] * (x[1] * z[2] - x[2] * z[1])
        + z[0] * (x[1] * y[2] - x[2] * y[1]);
    (dot(x, x) - 1.0).abs() < eps
        && (dot(y, y) - 1.0).abs() < eps
        && (dot(z, z) - 1.0).abs() < eps
        && dot(x, y).abs() < eps
        && dot(y, z).abs() < eps
        && dot(x, z).abs() < eps
        && det > 0.0
}

/// Bake a transform's 3×3 basis (rotation **including** any mirror/scale a quaternion can't carry) into
/// the mesh's vertices — the placement fallback for a non-rigid instance basis. The translation is NOT
/// applied (the entity still carries it), so instances of the same mirrored geometry still dedup.
#[must_use]
pub fn bake_basis_into_mesh(
    m: &[f64; 16],
    mesh: &metrocalk_csg::TriMesh,
) -> metrocalk_csg::TriMesh {
    let positions = mesh
        .positions
        .iter()
        .map(|p| {
            [
                m[0] * p[0] + m[4] * p[1] + m[8] * p[2],
                m[1] * p[0] + m[5] * p[1] + m[9] * p[2],
                m[2] * p[0] + m[6] * p[1] + m[10] * p[2],
            ]
        })
        .collect();
    metrocalk_csg::TriMesh::new(positions, mesh.triangles.clone())
}

/// One persisted **derived** CAD render mesh (bincode, in the app's `metrocalk-cad-meshes` sidecar): the
/// unique (geometry, colour) mesh the live import registered on the GPU, keyed by its handle — so a saved
/// scene's `MeshRenderer.mesh = "mtkcad:…"` re-resolves after restart + open **without re-parsing the
/// multi-hundred-MB source container** (the boot cost is deserialize + GPU upload, proportional to the
/// ~dozens of unique meshes, not the 262 MB file). Without this, every imported CAD part silently degraded
/// to a placeholder cube on reload — the never-silent violation the adversarial review flagged.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct PersistedCadMesh {
    /// The exact handle the doc's `MeshRenderer.mesh` field carries (`mtkcad:<geom-hash>[:<rgb>]`).
    pub handle: String,
    pub positions: Vec<[f64; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub color: Option<[f32; 3]>,
}

/// Persist one derived CAD render mesh into `dir`, keyed by its handle (`:` → `-` for a valid filename).
/// The caller logs a failure (never-silent) — a part that can't persist still renders this session.
///
/// # Errors
/// Any I/O error creating the dir or writing the record.
pub fn persist_cad_mesh(
    dir: &std::path::Path,
    handle: &str,
    mesh: &metrocalk_csg::TriMesh,
    color: Option<[f32; 3]>,
) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let rec = PersistedCadMesh {
        handle: handle.to_string(),
        positions: mesh.positions.clone(),
        triangles: mesh.triangles.clone(),
        color,
    };
    let bytes = bincode::serialize(&rec).map_err(std::io::Error::other)?;
    std::fs::write(dir.join(format!("{}.bin", handle.replace(':', "-"))), bytes)
}

/// Load every persisted CAD render mesh from `dir` (boot-time restore): each record becomes the same
/// colour-baked [`metrocalk_assets::MeshAsset`] the live import built, under the SAME handle. A corrupt
/// record is skipped with a log line (never trusted, never a boot abort); order is deterministic (sorted
/// by handle, not OS dir order).
#[must_use]
pub fn load_persisted_cad_meshes(
    dir: &std::path::Path,
) -> Vec<(String, metrocalk_assets::MeshAsset)> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out; // no sidecar yet — no CAD was ever imported
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("bin") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!(
                "[shell] cad-mesh sidecar {} unreadable — skipped",
                path.display()
            );
            continue;
        };
        match bincode::deserialize::<PersistedCadMesh>(&bytes) {
            Ok(rec) => {
                let mesh = metrocalk_csg::TriMesh::new(rec.positions, rec.triangles);
                let asset =
                    crate::csg_intent::trimesh_to_mesh_asset_colored(&mesh, "cad", rec.color);
                out.push((rec.handle, asset));
            }
            Err(e) => eprintln!(
                "[shell] cad-mesh sidecar {} corrupt — skipped: {e}",
                path.display()
            ),
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The **O(1) content-addressed re-import diff**: which of the N parts changed between two imports of the same
/// assembly (unchanged / moved / geometry-changed / added / removed). Re-importing an unchanged file is all-
/// `Unchanged` — never a full re-tessellation of every part (the substrate advantage no CAD tool ships).
#[must_use]
pub fn reimport_diff(before: &CadImport, after: &CadImport) -> Vec<PartDiff> {
    diff(before, after)
}

/// Count the parts that actually changed in a re-import (the "12 of 1,280 parts changed" headline).
#[must_use]
pub fn changed_count(diff: &[PartDiff]) -> usize {
    diff.iter()
        .filter(|d| d.change != PartChange::Unchanged)
        .count()
}

#[cfg(test)]
mod tests;
