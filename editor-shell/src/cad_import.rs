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

    let mut ops: Vec<Op> = Vec::with_capacity(report.parts.len() * 8);
    let mut entities = Vec::with_capacity(report.parts.len());
    for p in &report.parts {
        let e = engine.alloc_entity_id();
        ops.push(Op::CreateEntity {
            id: e,
            parent: None,
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
        report,
    })
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
