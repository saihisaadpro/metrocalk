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

/// Read a CAD source with a standards-based geometry fallback for a 3DXML whose `.3DRep` payload is a
/// licensed Dassault binary. Many PLM handoff packages place a STEP AP242 export beside the 3DXML product
/// structure. When the assembly name or normalized file stem matches, use that companion's real geometry,
/// hierarchy, names and colors instead of displaying thousands of placeholder cubes. The resolver is
/// deliberately conservative: it searches only the source directory, accepts only STEP magic, validates the
/// assembly identity, and falls back to the original never-silent 3DXML report if no trustworthy companion
/// exists.
///
/// # Errors
/// The primary source's [`CadError`]. Missing/unreadable/unrelated companion candidates never make an
/// otherwise readable 3DXML fail; they remain documented by its proprietary-geometry report.
pub fn read_cad_with_companion(
    source_path: &std::path::Path,
    bytes: &[u8],
) -> Result<CadImport, CadError> {
    let primary = read_cad(bytes)?;
    if primary.source_format != "CATIA-3DXML"
        || primary.parts.iter().all(|part| part.is_real_geometry())
    {
        return Ok(primary);
    }

    let source_stem = source_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(normalized_cad_label)
        .unwrap_or_default();
    let source_name = normalized_cad_label(&primary.name);
    for candidate_path in step_companion_candidates(source_path) {
        let candidate_stem = candidate_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(normalized_cad_label)
            .unwrap_or_default();
        let stem_matches = !source_stem.is_empty() && source_stem == candidate_stem;
        // A mismatched stem can still be a legitimate PLM export, but parse at most the small, bounded
        // candidate set and require the product name to match before accepting it.
        let Ok(candidate_bytes) = std::fs::read(&candidate_path) else {
            continue;
        };
        if !StepAssemblyReader.can_read(&candidate_bytes) {
            continue;
        }
        let Ok(mut candidate) = StepAssemblyReader.read(&candidate_bytes) else {
            continue;
        };
        let name_matches =
            !source_name.is_empty() && source_name == normalized_cad_label(&candidate.name);
        if (!stem_matches && !name_matches)
            || !candidate.parts.iter().any(|part| part.is_real_geometry())
        {
            continue;
        }

        let unresolved = primary
            .parts
            .iter()
            .filter(|part| !part.is_real_geometry())
            .count();
        let original_occurrences = primary.total_occurrences;
        let companion_display = candidate_path.display().to_string();
        candidate.source_format = "CATIA-3DXML + STEP-AP242 companion".into();
        candidate.source_hash = primary
            .source_hash
            .rotate_left(17)
            .wrapping_add(candidate.source_hash);
        candidate.notes.push(metrocalk_interchange::UnsupportedNote {
            feature: "3DXML geometry resolution".into(),
            detail: format!(
                "the 3DXML product structure referenced {unresolved} proprietary V5_CFV3/CB0001 body \
                 occurrence(s); resolved renderable geometry from the identity-matched STEP AP242 \
                 companion '{companion_display}' (the 3DXML declared {original_occurrences} Instance3D \
                 relationship(s))"
            ),
        });
        return Ok(candidate);
    }
    Ok(primary)
}

fn step_companion_candidates(source_path: &std::path::Path) -> Vec<std::path::PathBuf> {
    const MAX_CANDIDATES: usize = 32;
    let Some(directory) = source_path.parent() else {
        return Vec::new();
    };
    let source_stem = source_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(normalized_cad_label)
        .unwrap_or_default();
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut candidates: Vec<(bool, std::path::PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path == source_path || !path.is_file() {
                return None;
            }
            let extension = path.extension()?.to_str()?.to_ascii_lowercase();
            if !matches!(extension.as_str(), "stp" | "step") {
                return None;
            }
            let within_limit = path.metadata().ok().is_some_and(|metadata| {
                metadata.len() <= metrocalk_interchange::MAX_STEP_ASSEMBLY_BYTES as u64
            });
            if !within_limit {
                return None;
            }
            let stem_matches = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| normalized_cad_label(stem) == source_stem);
            Some((stem_matches, path))
        })
        .collect();
    candidates.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    candidates
        .into_iter()
        .take(MAX_CANDIDATES)
        .map(|(_, path)| path)
        .collect()
}

fn normalized_cad_label(value: &str) -> String {
    let mut value = value.trim().to_ascii_lowercase();
    // Strip common copy/version suffixes (`_(1)`, ` (2)`, `-3`) only when the final token is numeric.
    loop {
        let trimmed = value.trim_end();
        let mut cut = None;
        if trimmed.ends_with(')') {
            if let Some(open) = trimmed.rfind('(') {
                let digits = &trimmed[open + 1..trimmed.len() - 1];
                if !digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit())
                {
                    cut = Some(open);
                }
            }
        }
        let Some(index) = cut else { break };
        value.truncate(index);
        while value
            .chars()
            .next_back()
            .is_some_and(|character| matches!(character, '_' | '-' | ' '))
        {
            value.pop();
        }
    }
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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
        .map(|m| store_mesh(store, &cad_z_up_to_editor_mesh(&m.tris), "cad"))
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
        let editor_transform = cad_z_up_to_editor_transform(&p.transform);
        let t = translation_of(&editor_transform);
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

/// Convert a CAD Z-up affine transform into the editor's right-handed Y-up basis. Both the local mesh and
/// its occurrence transform use this basis change, so the conversion is `C * M * C^-1`, not a translation-
/// only swizzle. Source `(x, y, z)` becomes editor `(x, z, -y)`.
#[must_use]
pub fn cad_z_up_to_editor_transform(m: &[f64; 16]) -> [f64; 16] {
    [
        m[0], m[2], -m[1], m[3], // C * source X
        m[8], m[10], -m[9], m[11], // C * source Z (editor Y)
        -m[4], -m[6], m[5], -m[7], // -C * source Y (editor Z)
        m[12], m[14], -m[13], m[15], // converted translation
    ]
}

/// Rotate CAD-local mesh coordinates from Z-up into the editor's Y-up basis. This is a proper rotation
/// (determinant +1), so triangle winding and authored face orientation remain valid.
#[must_use]
pub fn cad_z_up_to_editor_mesh(mesh: &metrocalk_csg::TriMesh) -> metrocalk_csg::TriMesh {
    let positions = mesh
        .positions
        .iter()
        .map(|position| [position[0], position[2], -position[1]])
        .collect();
    metrocalk_csg::TriMesh::new(positions, mesh.triangles.clone())
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

const MAX_PERSISTED_CAD_MESH_BYTES: u64 = 512 * 1024 * 1024;

fn cad_mesh_file_name(handle: &str) -> std::io::Result<String> {
    // Project documents are user-controlled input. A mesh handle must never be able to turn a cache lookup
    // into a path traversal; generated handles contain only ASCII alphanumerics and ':' separators.
    if !handle.starts_with("mtkcad:")
        || handle.len() > 160
        || !handle
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == ':')
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "invalid persisted CAD mesh handle",
        ));
    }
    Ok(format!("{}.bin", handle.replace(':', "-")))
}

#[allow(clippy::manual_let_else, clippy::single_match_else)] // Both read/decode failures log path-specific recovery before returning.
fn load_persisted_cad_mesh(
    path: &std::path::Path,
    expected_handle: Option<&str>,
) -> Option<(String, metrocalk_assets::MeshAsset)> {
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > MAX_PERSISTED_CAD_MESH_BYTES {
        eprintln!(
            "[shell] cad-mesh sidecar {} exceeds the per-mesh safety limit — skipped",
            path.display()
        );
        return None;
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => {
            eprintln!(
                "[shell] cad-mesh sidecar {} unreadable — skipped",
                path.display()
            );
            return None;
        }
    };
    let rec = match bincode::deserialize::<PersistedCadMesh>(&bytes) {
        Ok(rec) => rec,
        Err(error) => {
            eprintln!(
                "[shell] cad-mesh sidecar {} corrupt — skipped: {error}",
                path.display()
            );
            return None;
        }
    };
    if cad_mesh_file_name(&rec.handle).is_err()
        || expected_handle.is_some_and(|expected| expected != rec.handle)
        || rec
            .positions
            .iter()
            .flatten()
            .any(|coordinate| !coordinate.is_finite())
        || rec
            .triangles
            .iter()
            .flatten()
            .any(|index| usize::try_from(*index).map_or(true, |index| index >= rec.positions.len()))
        || rec
            .color
            .is_some_and(|color| color.iter().any(|channel| !channel.is_finite()))
    {
        eprintln!(
            "[shell] cad-mesh sidecar {} failed identity or geometry validation — skipped",
            path.display()
        );
        return None;
    }
    let mesh = metrocalk_csg::TriMesh::new(rec.positions, rec.triangles);
    let asset = crate::csg_intent::trimesh_to_mesh_asset_colored(&mesh, "cad", rec.color);
    Some((rec.handle, asset))
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
    std::fs::write(dir.join(cad_mesh_file_name(handle)?), bytes)
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
        if let Some(restored) = load_persisted_cad_mesh(&path, None) {
            out.push(restored);
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Load only the persisted CAD meshes referenced by the active document. Lookup is direct by the
/// content-derived filename, so an unrelated global cache does not make editor startup progressively
/// slower. Missing, corrupt, identity-mismatched, or path-like handles are skipped; the caller compares
/// the returned count with `handles.len()` and surfaces unresolved document assets.
#[must_use]
pub fn load_persisted_cad_meshes_for(
    dir: &std::path::Path,
    handles: &std::collections::BTreeSet<String>,
) -> Vec<(String, metrocalk_assets::MeshAsset)> {
    handles
        .iter()
        .filter_map(|handle| {
            let file_name = cad_mesh_file_name(handle).ok()?;
            load_persisted_cad_mesh(&dir.join(file_name), Some(handle))
        })
        .collect()
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
#[allow(clippy::float_cmp)] // exact normalized fixture values are part of the import contract
mod tests;
