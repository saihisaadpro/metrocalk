//! **The live scene → STEP AP242**, which closes the one direction this engine could already do and
//! could not reach.
//!
//! `interchange::StepInterchange::export` writes faceted (planar B-rep) AP242 and has been tested
//! since M15.5. Nothing called it: there was no Tauri command, no menu item, and no converter from the
//! engine's render-side meshes to the [`CadScene`] it takes. So an author could bring CAD *in* and
//! could not send anything *back* — the exact half of a "digital thread" that makes the other half
//! worth having.
//!
//! ## Faceted is the honest export, and it is the right one here
//!
//! A triangle mesh has no analytic surfaces. Writing one out as trimmed NURBS would mean inventing
//! surfaces that were never authored, which is how CAD data gets quietly corrupted in translation. AP242
//! has a faceted B-rep representation precisely for this case: every triangle becomes an `ADVANCED_FACE`
//! over a `PLANE`, and the receiving system is told the truth — this is tessellated geometry, at the
//! tolerance the engine actually holds.
//!
//! What that means for the user is stated at the gesture, not discovered downstream: **a part that came
//! in as exact B-rep and goes back out through here comes back FACETED.** For sending a game asset to a
//! machinist, a fabricator or a viewer, that is exactly what is wanted. For round-tripping a precision
//! part, re-export the original instead.

use metrocalk_interchange::{CadFace, CadScene, CadSolid, FaceKind, Units};

/// The most triangles one STEP export may write.
///
/// Each triangle costs roughly eight Part-21 entities (three `CARTESIAN_POINT`s, three `VERTEX_POINT`s,
/// the loop, the face, the plane, the axis placement), so this is a file-size and receiving-system
/// bound rather than a memory one — a million triangles would produce a Part-21 file most CAD systems
/// will refuse to open. Over the cap is REFUSED with the number and a way forward, never silently
/// truncated into a mesh with holes in it.
pub const MAX_EXPORT_TRIANGLES: usize = 250_000;

/// One mesh to write, already in world space.
#[derive(Clone, Debug)]
pub struct StepPart {
    /// The part name, carried into the STEP `PRODUCT`.
    pub name: String,
    /// World-space triangle corners, three per triangle.
    pub triangles: Vec<[[f64; 3]; 3]>,
}

/// Why an export was refused, in the author's language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepExportError {
    /// Nothing to write.
    Empty,
    /// Over [`MAX_EXPORT_TRIANGLES`].
    TooLarge { triangles: usize },
    /// The writer refused.
    Writer(String),
}

impl std::fmt::Display for StepExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(
                f,
                "there is no geometry in this scene to write — add or import something first"
            ),
            Self::TooLarge { triangles } => write!(
                f,
                "this scene is {triangles} triangles, over the {MAX_EXPORT_TRIANGLES} STEP limit — \
                 export a selection, or simplify the meshes in Asset Lab first"
            ),
            Self::Writer(m) => write!(f, "the STEP writer refused: {m}"),
        }
    }
}

impl std::error::Error for StepExportError {}

/// What an export actually wrote — reported back so the cost is legible after the fact, matching how
/// every other command in this editor answers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StepExportReport {
    /// Parts written.
    pub parts: usize,
    /// Triangles written, which is also the `ADVANCED_FACE` count.
    pub triangles: usize,
    /// Fidelity notes, in plain language.
    pub notes: Vec<String>,
}

/// Convert world-space triangle soup into the neutral [`CadScene`] the STEP writer takes.
///
/// Degenerate triangles are dropped and counted rather than written: a zero-area `ADVANCED_FACE` has no
/// well-defined `PLANE` normal, and emitting one produces a file that opens and then fails validation
/// somewhere else entirely — the worst kind of interchange bug.
#[must_use]
pub fn scene_from_parts(parts: &[StepPart]) -> (CadScene, usize) {
    let mut solids = Vec::with_capacity(parts.len());
    let mut next_id: u64 = 1;
    let mut degenerate = 0usize;

    for part in parts {
        let mut faces = Vec::with_capacity(part.triangles.len());
        for tri in &part.triangles {
            if triangle_is_degenerate(tri) {
                degenerate += 1;
                continue;
            }
            faces.push(CadFace {
                id: next_id,
                kind: FaceKind::Planar,
                outer: vec![tri[0], tri[1], tri[2]],
                inner: Vec::new(),
                edges: Vec::new(),
                surface: None,
                recognized: None,
                same_sense: true,
                outer_sense: true,
            });
            next_id += 1;
        }
        if faces.is_empty() {
            continue;
        }
        solids.push(CadSolid { id: next_id, faces });
        next_id += 1;
    }

    let scene = CadScene {
        name: "Metrocalk scene".into(),
        format: "STEP-AP242".into(),
        // The engine works in SI metres and says so, rather than leaving the receiving system to
        // assume STEP's millimetre convention and land the part 1000x too small.
        units: Units::SI,
        solids,
        pmi: Vec::new(),
        notes: Vec::new(),
    };
    (scene, degenerate)
}

/// A triangle with no area — collapsed to a line or a point.
fn triangle_is_degenerate(t: &[[f64; 3]; 3]) -> bool {
    let e1 = [t[1][0] - t[0][0], t[1][1] - t[0][1], t[1][2] - t[0][2]];
    let e2 = [t[2][0] - t[0][0], t[2][1] - t[0][1], t[2][2] - t[0][2]];
    let cross = [
        e1[1] * e2[2] - e1[2] * e2[1],
        e1[2] * e2[0] - e1[0] * e2[2],
        e1[0] * e2[1] - e1[1] * e2[0],
    ];
    let area2 = cross[0].mul_add(cross[0], cross[1].mul_add(cross[1], cross[2] * cross[2]));
    !area2.is_finite() || area2 <= 1.0e-20
}

/// Write `parts` as faceted STEP AP242 text.
///
/// # Errors
/// [`StepExportError`] — nothing to write, over the triangle cap, or the writer refused. Every one
/// carries what to do about it.
pub fn export_parts(parts: &[StepPart]) -> Result<(String, StepExportReport), StepExportError> {
    let triangles: usize = parts.iter().map(|p| p.triangles.len()).sum();
    if triangles == 0 {
        return Err(StepExportError::Empty);
    }
    if triangles > MAX_EXPORT_TRIANGLES {
        return Err(StepExportError::TooLarge { triangles });
    }

    let (scene, degenerate) = scene_from_parts(parts);
    if scene.solids.is_empty() {
        return Err(StepExportError::Empty);
    }
    let written: usize = scene.solids.iter().map(|s| s.faces.len()).sum();
    let text =
        crate::pmi_step::export_step(&scene).map_err(|e| StepExportError::Writer(e.to_string()))?;

    let mut notes = vec![format!(
        "Written as FACETED B-rep — {written} triangles became planar faces. A receiving CAD system \
         gets exact tessellated geometry, not analytic surfaces."
    )];
    if degenerate > 0 {
        notes.push(format!(
            "{degenerate} zero-area triangle(s) were left out — they have no surface normal, and a CAD \
             system would reject the file for them."
        ));
    }
    notes.push(
        "Appearance is not carried: STEP is a geometry and engineering-data format, so materials, \
         textures and animation stay behind. Export glTF for those."
            .into(),
    );

    Ok((
        text,
        StepExportReport {
            parts: scene.solids.len(),
            triangles: written,
            notes,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> [[f64; 3]; 3] {
        [a, b, c]
    }

    fn unit_tetra() -> StepPart {
        StepPart {
            name: "Tetra".into(),
            triangles: vec![
                tri([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
                tri([0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
                tri([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
                tri([1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
            ],
        }
    }

    #[test]
    fn a_mesh_becomes_a_step_file_that_says_it_is_step() {
        let (text, report) = export_parts(&[unit_tetra()]).expect("exports");
        assert!(text.starts_with("ISO-10303-21;"), "not a Part-21 file");
        assert!(text.contains("ENDSEC;"), "truncated");
        assert!(
            text.trim_end().ends_with("END-ISO-10303-21;"),
            "unterminated"
        );
        assert_eq!(report.parts, 1);
        assert_eq!(report.triangles, 4);
    }

    #[test]
    fn what_it_wrote_and_what_it_left_behind_are_both_stated() {
        let (_, report) = export_parts(&[unit_tetra()]).expect("exports");
        let all = report.notes.join(" ");
        assert!(
            all.contains("FACETED"),
            "must say the geometry is faceted: {all}"
        );
        assert!(
            all.contains("materials") || all.contains("Appearance"),
            "must say appearance is dropped: {all}"
        );
    }

    #[test]
    fn zero_area_triangles_are_dropped_and_counted_not_written() {
        // A collapsed triangle has no plane normal. Writing one produces a file that opens and then
        // fails validation somewhere else — so it is refused here, visibly.
        let mut part = unit_tetra();
        part.triangles
            .push(tri([1.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 1.0, 1.0]));
        part.triangles
            .push(tri([0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [1.0, 0.0, 0.0])); // colinear
        let (_, report) = export_parts(&[part]).expect("exports");
        assert_eq!(report.triangles, 4, "degenerates must not be written");
        assert!(
            report.notes.iter().any(|n| n.contains("zero-area")),
            "the drop must be reported: {:?}",
            report.notes
        );
    }

    #[test]
    fn an_empty_scene_is_refused_with_a_way_forward() {
        let err = export_parts(&[]).expect_err("refuses");
        assert!(err.to_string().contains("import something"), "{err}");
        let all_degenerate = StepPart {
            name: "Flat".into(),
            triangles: vec![tri([0.0; 3], [0.0; 3], [0.0; 3])],
        };
        let err = export_parts(&[all_degenerate]).expect_err("refuses");
        assert!(matches!(err, StepExportError::Empty), "{err:?}");
    }

    #[test]
    fn an_oversized_scene_is_refused_with_the_number_and_the_remedy() {
        let huge = StepPart {
            name: "Huge".into(),
            triangles: vec![
                tri([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
                MAX_EXPORT_TRIANGLES + 1
            ],
        };
        let err = export_parts(&[huge]).expect_err("refuses");
        let msg = err.to_string();
        assert!(msg.contains(&MAX_EXPORT_TRIANGLES.to_string()), "{msg}");
        assert!(
            msg.contains("Asset Lab") || msg.contains("simplify"),
            "{msg}"
        );
    }

    #[test]
    fn several_parts_stay_several_solids() {
        let mut b = unit_tetra();
        b.name = "Second".into();
        let (_, report) = export_parts(&[unit_tetra(), b]).expect("exports");
        assert_eq!(report.parts, 2, "parts must not be merged into one blob");
        assert_eq!(report.triangles, 8);
    }

    #[test]
    fn the_written_file_reads_back_as_the_same_geometry() {
        // The claim that matters: this is not "text that looks like STEP", it round-trips through the
        // engine's own reader with the face count intact.
        let (text, report) = export_parts(&[unit_tetra()]).expect("exports");
        let back = crate::pmi_step::import_step_text(&text).expect("re-imports");
        assert_eq!(
            back.face_count(),
            report.triangles,
            "the file did not read back as what was written"
        );
    }

    #[test]
    fn the_same_scene_writes_the_same_bytes_twice() {
        let (a, _) = export_parts(&[unit_tetra()]).expect("exports");
        let (b, _) = export_parts(&[unit_tetra()]).expect("exports");
        // Byte-identical apart from any timestamp the header carries — compare the DATA section, which
        // is the part a downstream diff or a content hash actually cares about.
        let data = |s: &str| {
            s.split("DATA;")
                .nth(1)
                .map(str::to_string)
                .unwrap_or_default()
        };
        assert_eq!(data(&a), data(&b), "export is not reproducible");
    }
}
