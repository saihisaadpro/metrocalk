//! M15.5 (ADR-075) — **semantic PMI through the STEP AP242 round-trip, measured.** The M15.3 semantic FCFs
//! (typed ECS relationships on imported B-rep faces) are serialized to a STEP AP242 file as **machine-readable
//! `geometric_tolerance` entities** (via the [`metrocalk_interchange`] `CadInterchange` seam), re-imported, and
//! **re-attached as the same typed relationships** — with a **measured fidelity** (what survives *semantic*,
//! not downgraded to graphical). This is the BETTER-INTEGRATED interop leg: the exchanged PMI lands back on the
//! content-addressed, branch/merge, reproducible thread with its semantics intact.
//!
//! **Honest boundary (measured, not badged).** The declared subset (the 10 M15.3 characteristics on a single
//! datum) round-trips 100% semantic **through our pure-Rust Part-21 subset** — the number we publish. Full
//! AP242 ed4 conformance (the complex-instance datum_system algebra, MMC/LMC/composite frames) + fidelity on a
//! wild commercial-CAD file is the **OCCT-backed native/server seam** (ADR-070). A graphical-only callout is
//! **not** silently promoted to semantic — it is an explained downgrade note.

use crate::cad_intent::import_step;
use crate::capscene::CapScene;
use crate::pmi::{attach_fcf, fcfs_on, read_fcf, Characteristic, Fcf, PmiError, Standard};
use metrocalk_assets::AssetStore;
use metrocalk_core::{Engine, EntityId, FieldValue};
use metrocalk_ecs::{FlecsWorld, World};
use metrocalk_interchange::{CadInterchange, CadPmi, CadScene, StepInterchange};

/// A **resolved, typed** feature-control-frame — the "machine-readable structured data" the round-trip must
/// preserve. Keyed by the toleranced face's **position** in the scene's face order (`face_index`), which is
/// stable across the export→re-import cycle even though the STEP `#id`s are renumbered.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SemanticFcf {
    /// The toleranced face's index in the scene's face order.
    pub face_index: usize,
    /// The typed GD&T characteristic (a closed enum, never a label).
    pub characteristic: Characteristic,
    /// The tolerance-zone magnitude (mm).
    pub value_mm: f64,
    /// The datum face's index, if this is an orientation/location tolerance.
    pub datum_index: Option<usize>,
    /// The authoring standard (a closed enum).
    pub standard: Standard,
}

/// One row of the **measured fidelity table** — did this characteristic survive the round-trip as *semantic*
/// machine-readable data, and were its value / datum / standard preserved. Computed **deterministically**
/// (exact `f64` bit comparison, no RNG) so the published fidelity number is itself reproducible.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // the 4 bools ARE the fidelity table's independent survival columns
pub struct FidelityRow {
    /// The characteristic (canonical token).
    pub characteristic: String,
    /// It came back as a queryable typed FCF (not dropped, not a graphical downgrade).
    pub semantic: bool,
    /// The tolerance value is bit-preserved.
    pub value_exact: bool,
    /// The datum reconnected to the **same face** (by index) as the original — not merely "a datum exists".
    pub datum_preserved: bool,
    /// The standard is preserved.
    pub standard_preserved: bool,
}

impl FidelityRow {
    /// A fully-faithful row: semantic + value + datum + standard all preserved.
    #[must_use]
    pub fn faithful(&self) -> bool {
        self.semantic && self.value_exact && self.datum_preserved && self.standard_preserved
    }
}

/// The **measured round-trip fidelity** — the per-characteristic table + the geometry deviation. The honest
/// number the interop claim rests on (not a conformance badge).
#[derive(Clone, Debug, PartialEq)]
pub struct RoundTripFidelity {
    /// One row per original FCF, in a stable order.
    pub rows: Vec<FidelityRow>,
    /// The geometry round-trip deviation (scene units) — the M15.0 budget carried forward.
    pub geometry_deviation: f64,
}

impl RoundTripFidelity {
    /// `(survived_semantic, total)` — the headline number.
    #[must_use]
    pub fn semantic_survival(&self) -> (usize, usize) {
        (
            self.rows.iter().filter(|r| r.semantic).count(),
            self.rows.len(),
        )
    }

    /// Every FCF survived as semantic machine-readable data (not one downgraded to graphical / dropped).
    #[must_use]
    pub fn all_semantic(&self) -> bool {
        !self.rows.is_empty() && self.rows.iter().all(|r| r.semantic)
    }

    /// Every FCF round-trips fully faithfully (semantic + value + datum + standard).
    #[must_use]
    pub fn fully_faithful(&self) -> bool {
        !self.rows.is_empty() && self.rows.iter().all(FidelityRow::faithful)
    }
}

/// The step_id (STEP `#id`) an imported face entity carries (from the M15.0 `import_step`).
fn face_step_id<W: World>(engine: &Engine<W>, face: EntityId) -> Option<u64> {
    match engine.get_field(face, "CadFace", "step_id")? {
        FieldValue::Integer(n) if n > 0 => u64::try_from(n).ok(),
        _ => None,
    }
}

/// Collect the semantic FCFs currently attached to `faces`, resolved as typed [`SemanticFcf`] keyed by face
/// index (the machine-readable read). Deterministic order (faces in the given order, FCFs sorted by
/// [`fcfs_on`]).
#[must_use]
pub fn collect_semantic_fcfs<W: World>(engine: &Engine<W>, faces: &[EntityId]) -> Vec<SemanticFcf> {
    let index_of = |e: EntityId| faces.iter().position(|&f| f == e);
    let mut out = Vec::new();
    for (face_index, &face) in faces.iter().enumerate() {
        for fcf_entity in fcfs_on(engine, face) {
            if let Some(fcf) = read_fcf(engine, fcf_entity) {
                out.push(SemanticFcf {
                    face_index,
                    characteristic: fcf.characteristic,
                    value_mm: fcf.tolerance_mm,
                    datum_index: fcf.datum.and_then(index_of),
                    standard: fcf.standard,
                });
            }
        }
    }
    out
}

/// Serialize the FCFs attached to `faces` into a scene's [`CadScene::pmi`] (neutral tokens, keyed by the
/// face's STEP `#id`) so [`StepInterchange::export`] writes them as semantic `geometric_tolerance` entities.
#[must_use]
pub fn scene_with_pmi<W: World>(
    engine: &Engine<W>,
    faces: &[EntityId],
    mut scene: CadScene,
) -> CadScene {
    let mut pmi = Vec::new();
    for &face in faces {
        let Some(face_id) = face_step_id(engine, face) else {
            continue;
        };
        for fcf_entity in fcfs_on(engine, face) {
            let Some(fcf) = read_fcf(engine, fcf_entity) else {
                continue;
            };
            pmi.push(CadPmi {
                face_id,
                characteristic: fcf.characteristic.canonical().to_string(),
                value_mm: fcf.tolerance_mm,
                datum_face_id: fcf.datum.and_then(|d| face_step_id(engine, d)),
                standard: fcf.standard.canonical().to_string(),
                semantic: true,
            });
        }
    }
    scene.pmi = pmi;
    scene
}

/// Re-import a STEP scene that carries semantic PMI and **re-attach** each FCF to the geometrically-correct
/// re-imported face as a typed ECS relationship (the M15.3 `attach_fcf` — one undoable tx each). Returns the
/// re-imported face entities (scene order) so fidelity can be measured against the original.
///
/// # Errors
/// A [`PmiError`] if a re-attach is rejected (never silently dropped); a PMI whose characteristic/standard
/// token isn't a known typed enum is a **measured downgrade** (skipped + reported), not an error.
pub fn reimport_with_pmi(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    store: &mut AssetStore,
    cad: &CadScene,
) -> Result<Vec<EntityId>, PmiError> {
    let imported =
        import_step(engine, scene, store, cad).map_err(|e| PmiError::Pipeline(e.to_string()))?;
    // Map the re-imported scene's face step_id → its ECS entity (faces are in scene order).
    let scene_face_ids: Vec<u64> = cad
        .solids
        .iter()
        .flat_map(|s| &s.faces)
        .map(|f| f.id)
        .collect();
    let step_to_entity: std::collections::BTreeMap<u64, EntityId> = scene_face_ids
        .iter()
        .copied()
        .zip(imported.faces.iter().copied())
        .collect();

    for p in &cad.pmi {
        if !p.semantic {
            continue; // a graphical-only downgrade is not re-attached as semantic (honest)
        }
        let (Some(characteristic), Some(standard)) = (
            Characteristic::from_canonical(&p.characteristic),
            Standard::from_canonical(&p.standard),
        ) else {
            continue; // an unknown token can't be a typed FCF — a measured downgrade, not a fake
        };
        let Some(&feature) = step_to_entity.get(&p.face_id) else {
            continue;
        };
        let datum = p
            .datum_face_id
            .and_then(|d| step_to_entity.get(&d).copied());
        attach_fcf(
            engine,
            &Fcf {
                feature,
                characteristic,
                tolerance_mm: p.value_mm,
                datum,
                standard,
            },
        )?;
    }
    Ok(imported.faces)
}

/// **Measure the fidelity** — match each original FCF to a re-imported one (by face index + characteristic)
/// and record what survived. Deterministic (exact comparisons). This is the honest number, computed — not a
/// badge.
#[must_use]
pub fn measure_fidelity(
    original: &[SemanticFcf],
    reimported: &[SemanticFcf],
    geometry_deviation: f64,
) -> RoundTripFidelity {
    let rows = original
        .iter()
        .map(|orig| {
            let found = reimported.iter().find(|r| {
                r.face_index == orig.face_index && r.characteristic == orig.characteristic
            });
            match found {
                Some(r) => FidelityRow {
                    characteristic: orig.characteristic.canonical().to_string(),
                    semantic: true,
                    value_exact: r.value_mm.to_bits() == orig.value_mm.to_bits(),
                    // Strict: the datum reconnected to the SAME face (index), not merely "a datum exists"
                    // — so a round-trip that reattached the datum to the wrong face fails the fidelity table,
                    // not just property_b's single hand-picked FCF.
                    datum_preserved: r.datum_index == orig.datum_index,
                    standard_preserved: r.standard == orig.standard,
                },
                None => FidelityRow {
                    characteristic: orig.characteristic.canonical().to_string(),
                    semantic: false,
                    value_exact: false,
                    datum_preserved: false,
                    standard_preserved: false,
                },
            }
        })
        .collect();
    RoundTripFidelity {
        rows,
        geometry_deviation,
    }
}

/// The re-export step for the round-trip: write a scene (with PMI) to STEP AP242 text.
///
/// # Errors
/// A [`PmiError`] wrapping the STEP export error (e.g. an all-curved scene — the OCCT seam).
pub fn export_step(scene: &CadScene) -> Result<String, PmiError> {
    StepInterchange
        .export(scene)
        .map_err(|e| PmiError::Pipeline(e.to_string()))
}

/// Re-import STEP AP242 text (with PMI) to a [`CadScene`].
///
/// # Errors
/// A [`PmiError`] wrapping the STEP import error (malformed/oversized → explained, never a panic).
pub fn import_step_text(text: &str) -> Result<CadScene, PmiError> {
    StepInterchange
        .import(text.as_bytes())
        .map_err(|e| PmiError::Pipeline(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fcf(
        face: usize,
        c: Characteristic,
        v: f64,
        datum: Option<usize>,
        s: Standard,
    ) -> SemanticFcf {
        SemanticFcf {
            face_index: face,
            characteristic: c,
            value_mm: v,
            datum_index: datum,
            standard: s,
        }
    }

    #[test]
    fn identical_sets_are_fully_faithful() {
        let orig = vec![
            fcf(
                0,
                Characteristic::Position,
                0.10,
                Some(1),
                Standard::AsmeY14_5,
            ),
            fcf(2, Characteristic::Flatness, 0.02, None, Standard::IsoGps),
        ];
        let fid = measure_fidelity(&orig, &orig, 0.0);
        assert!(fid.fully_faithful());
        assert_eq!(fid.semantic_survival(), (2, 2));
    }

    #[test]
    fn a_dropped_fcf_is_a_non_semantic_row() {
        let orig = vec![fcf(
            0,
            Characteristic::Position,
            0.10,
            Some(1),
            Standard::AsmeY14_5,
        )];
        let fid = measure_fidelity(&orig, &[], 0.0);
        assert!(!fid.all_semantic());
        assert_eq!(fid.semantic_survival(), (0, 1));
        assert!(!fid.rows[0].semantic);
    }

    #[test]
    fn a_perturbed_value_or_lost_datum_is_recorded_not_hidden() {
        let orig = vec![fcf(
            0,
            Characteristic::Position,
            0.10,
            Some(1),
            Standard::AsmeY14_5,
        )];
        // Came back semantic but with a perturbed value + lost datum.
        let re = vec![fcf(
            0,
            Characteristic::Position,
            0.11,
            None,
            Standard::AsmeY14_5,
        )];
        let fid = measure_fidelity(&orig, &re, 1e-9);
        assert!(fid.all_semantic(), "it did come back as a typed FCF");
        assert!(!fid.fully_faithful(), "but not fully faithful");
        assert!(!fid.rows[0].value_exact);
        assert!(!fid.rows[0].datum_preserved);
    }

    #[test]
    fn fidelity_is_deterministic() {
        let orig = vec![
            fcf(
                0,
                Characteristic::Position,
                0.10,
                Some(1),
                Standard::AsmeY14_5,
            ),
            fcf(
                2,
                Characteristic::Cylindricity,
                0.05,
                None,
                Standard::IsoGps,
            ),
        ];
        let a = measure_fidelity(&orig, &orig, 0.0);
        let b = measure_fidelity(&orig, &orig, 0.0);
        assert_eq!(a, b, "the fidelity number is reproducible");
    }
}
