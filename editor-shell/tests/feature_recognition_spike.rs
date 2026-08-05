//! M15.11 (ADR-081) — THE SPIKE (deliverable #1): the measured go/no-go for **the AI semantic pass**.
//!
//! The claim: on a REAL decoded part (a STEP plate with a through-bore, parsed by the real reader — not a
//! hand-built graph), the AAG recognizer produces **correct, typed, queryable features**: the bore lands as
//! a `CadFeature` child entity (kind `through-hole`, the bore's STEP face id, radius, an explainable `why`)
//! through the **validated-AI gate** (schema check → kernel re-derivation → confidence band), and
//! **"select all holes" WORKS** as a typed query over the landed components — never a label parse.
//!
//! The gate (any of these is a FAIL):
//! - a wrong label lands silently (the kernel check must REJECT a hole claimed on a boss wall, even at 0.99);
//! - a low-confidence recognition is auto-committed (it must be HELD for adjudication);
//! - "select all holes" / "hide all fasteners" re-scans geometry instead of querying the typed components;
//! - a defeature is irreversible or drops the exact B-rep (undo must restore the original render mesh);
//! - recognition is non-deterministic (same file ⇒ same features, bit-for-bit);
//! - AI touches the decode path (recognition input is decoded `CadFace`/`TriMesh` only — CI grep-gated).
//!
//! Real `Engine` + the real STEP reader + real ops, headless, CI-gated (no dark test).

use std::collections::BTreeMap;

use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::FlecsWorld;
use metrocalk_editor_shell::{
    all_cad_parts, class_ops, defeature_ops, parse_semantic, plan_defeature, plan_feature_collider,
    plan_semantic_land, resolve_semantic, semantic_act_ops, SemanticNoun, SemanticVerb,
    CAD_FEATURE, CAD_PART, PART_CLASS,
};
use metrocalk_interchange::{
    classify_part, identities, AagRecognizer, CadReader, FeatureKind, FeatureRecognizer, PartClass,
    RecognizedFeature, StepAssemblyReader,
};

/// A REAL AP242 plate (40×40×5 mm) with a ∅10 through-bore: 6 planar faces (top/bottom carry the bore rim
/// as an INNER `FACE_BOUND` sharing the rim `EDGE_CURVE`s with the cylinder — the adjacency the AAG needs),
/// plus the bore's `CYLINDRICAL_SURFACE` face with `same_sense .F.` (inward — a bore). The cylinder-face
/// topology (two closed rim circles + a seam) mirrors the M15.8 `analytic_quartet` fixture the parser is
/// proven on.
const PLATE_WITH_BORE: &str = "ISO-10303-21;
HEADER;
FILE_DESCRIPTION(('M15.11 spike: plate with a through-bore'),'2;1');
FILE_NAME('plate_bore','2026-07-10T00:00:00',(''),(''),'metrocalk-spike','','');
FILE_SCHEMA(('AUTOMOTIVE_DESIGN { 1 0 10303 442 1 1 4 }'));
ENDSEC;
DATA;
#1 = CARTESIAN_POINT('',(0.,0.,0.));
#2 = DIRECTION('',(0.,0.,1.));
#3 = DIRECTION('',(1.,0.,0.));
#4 = AXIS2_PLACEMENT_3D('',#1,#2,#3);
#5 = CARTESIAN_POINT('',(5.,0.,0.));
#6 = CARTESIAN_POINT('',(5.,0.,5.));
#7 = VERTEX_POINT('',#5);
#8 = VERTEX_POINT('',#6);
#9 = EDGE_CURVE('',#7,#7,$,.T.);
#10 = EDGE_CURVE('',#7,#8,$,.T.);
#11 = EDGE_CURVE('',#8,#8,$,.T.);
#12 = ORIENTED_EDGE('',*,*,#9,.T.);
#13 = ORIENTED_EDGE('',*,*,#10,.T.);
#14 = ORIENTED_EDGE('',*,*,#11,.T.);
#15 = EDGE_LOOP('',(#12,#13,#14));
#16 = FACE_OUTER_BOUND('',#15,.T.);
#17 = CYLINDRICAL_SURFACE('',#4,5.);
#18 = ADVANCED_FACE('',(#16),#17,.F.);
#20 = CARTESIAN_POINT('',(-20.,-20.,5.));
#21 = CARTESIAN_POINT('',(20.,-20.,5.));
#22 = CARTESIAN_POINT('',(20.,20.,5.));
#23 = CARTESIAN_POINT('',(-20.,20.,5.));
#24 = VERTEX_POINT('',#20);
#25 = VERTEX_POINT('',#21);
#26 = VERTEX_POINT('',#22);
#27 = VERTEX_POINT('',#23);
#28 = EDGE_CURVE('',#24,#25,$,.T.);
#29 = EDGE_CURVE('',#25,#26,$,.T.);
#30 = EDGE_CURVE('',#26,#27,$,.T.);
#31 = EDGE_CURVE('',#27,#24,$,.T.);
#32 = ORIENTED_EDGE('',*,*,#28,.T.);
#33 = ORIENTED_EDGE('',*,*,#29,.T.);
#34 = ORIENTED_EDGE('',*,*,#30,.T.);
#35 = ORIENTED_EDGE('',*,*,#31,.T.);
#36 = EDGE_LOOP('',(#32,#33,#34,#35));
#37 = FACE_OUTER_BOUND('',#36,.T.);
#38 = ORIENTED_EDGE('',*,*,#11,.T.);
#39 = EDGE_LOOP('',(#38));
#40 = FACE_BOUND('',#39,.T.);
#41 = AXIS2_PLACEMENT_3D('',#20,#2,#3);
#42 = PLANE('',#41);
#43 = ADVANCED_FACE('',(#37,#40),#42,.T.);
#50 = CARTESIAN_POINT('',(-20.,-20.,0.));
#51 = CARTESIAN_POINT('',(20.,-20.,0.));
#52 = CARTESIAN_POINT('',(20.,20.,0.));
#53 = CARTESIAN_POINT('',(-20.,20.,0.));
#54 = VERTEX_POINT('',#50);
#55 = VERTEX_POINT('',#51);
#56 = VERTEX_POINT('',#52);
#57 = VERTEX_POINT('',#53);
#58 = EDGE_CURVE('',#54,#57,$,.T.);
#59 = EDGE_CURVE('',#57,#56,$,.T.);
#60 = EDGE_CURVE('',#56,#55,$,.T.);
#61 = EDGE_CURVE('',#55,#54,$,.T.);
#62 = ORIENTED_EDGE('',*,*,#58,.T.);
#63 = ORIENTED_EDGE('',*,*,#59,.T.);
#64 = ORIENTED_EDGE('',*,*,#60,.T.);
#65 = ORIENTED_EDGE('',*,*,#61,.T.);
#66 = EDGE_LOOP('',(#62,#63,#64,#65));
#67 = FACE_OUTER_BOUND('',#66,.T.);
#68 = ORIENTED_EDGE('',*,*,#9,.T.);
#69 = EDGE_LOOP('',(#68));
#70 = FACE_BOUND('',#69,.T.);
#71 = AXIS2_PLACEMENT_3D('',#50,#2,#3);
#72 = PLANE('',#71);
#73 = ADVANCED_FACE('',(#67,#70),#72,.F.);
#80 = EDGE_CURVE('',#54,#55,$,.T.);
#81 = EDGE_CURVE('',#55,#25,$,.T.);
#82 = EDGE_CURVE('',#25,#24,$,.T.);
#83 = EDGE_CURVE('',#24,#54,$,.T.);
#84 = ORIENTED_EDGE('',*,*,#80,.T.);
#85 = ORIENTED_EDGE('',*,*,#81,.T.);
#86 = ORIENTED_EDGE('',*,*,#82,.T.);
#87 = ORIENTED_EDGE('',*,*,#83,.T.);
#88 = EDGE_LOOP('',(#84,#85,#86,#87));
#89 = FACE_OUTER_BOUND('',#88,.T.);
#90 = AXIS2_PLACEMENT_3D('',#50,#2,#3);
#91 = PLANE('',#90);
#92 = ADVANCED_FACE('',(#89),#91,.T.);
#100 = EDGE_CURVE('',#55,#56,$,.T.);
#101 = EDGE_CURVE('',#56,#26,$,.T.);
#102 = EDGE_CURVE('',#26,#25,$,.T.);
#103 = ORIENTED_EDGE('',*,*,#100,.T.);
#104 = ORIENTED_EDGE('',*,*,#101,.T.);
#105 = ORIENTED_EDGE('',*,*,#102,.T.);
#106 = ORIENTED_EDGE('',*,*,#81,.F.);
#107 = EDGE_LOOP('',(#103,#104,#105,#106));
#108 = FACE_OUTER_BOUND('',#107,.T.);
#109 = ADVANCED_FACE('',(#108),#91,.T.);
#110 = EDGE_CURVE('',#56,#57,$,.T.);
#111 = EDGE_CURVE('',#57,#27,$,.T.);
#112 = EDGE_CURVE('',#27,#26,$,.T.);
#113 = ORIENTED_EDGE('',*,*,#110,.T.);
#114 = ORIENTED_EDGE('',*,*,#111,.T.);
#115 = ORIENTED_EDGE('',*,*,#112,.T.);
#116 = ORIENTED_EDGE('',*,*,#101,.F.);
#117 = EDGE_LOOP('',(#113,#114,#115,#116));
#118 = FACE_OUTER_BOUND('',#117,.T.);
#119 = ADVANCED_FACE('',(#118),#91,.T.);
#120 = EDGE_CURVE('',#57,#54,$,.T.);
#121 = EDGE_CURVE('',#54,#24,$,.T.);
#122 = EDGE_CURVE('',#24,#27,$,.T.);
#123 = ORIENTED_EDGE('',*,*,#120,.T.);
#124 = ORIENTED_EDGE('',*,*,#121,.T.);
#125 = ORIENTED_EDGE('',*,*,#122,.T.);
#126 = ORIENTED_EDGE('',*,*,#111,.F.);
#127 = EDGE_LOOP('',(#123,#124,#125,#126));
#128 = FACE_OUTER_BOUND('',#127,.T.);
#129 = ADVANCED_FACE('',(#128),#91,.T.);
#140 = CLOSED_SHELL('',(#18,#43,#73,#92,#109,#119,#129));
#141 = MANIFOLD_SOLID_BREP('plate with bore',#140);
ENDSEC;
END-ISO-10303-21;
";

fn engine() -> Engine<FlecsWorld> {
    Engine::new(FlecsWorld::new(), 1)
}

/// Land one imported part as the importer would: the part entity + `CadPart` + the semantic pass's
/// validated feature ops — ONE commit.
fn land_part(
    e: &mut Engine<FlecsWorld>,
    name: &str,
    faces: &[metrocalk_interchange::CadFace],
) -> (EntityId, metrocalk_editor_shell::SemanticLanding) {
    let part = e.alloc_entity_id();
    let mut ops = vec![
        Op::CreateEntity {
            id: part,
            parent: None,
        },
        Op::SetField {
            entity: part,
            component: CAD_PART.into(),
            field: "fidelity".into(),
            value: FieldValue::Str("exact-brep".into()),
        },
        Op::SetField {
            entity: part,
            component: CAD_PART.into(),
            field: "name".into(),
            value: FieldValue::Str(name.into()),
        },
    ];
    let landing = plan_semantic_land(e, part, faces, &AagRecognizer);
    ops.extend(landing.ops.clone());
    e.commit("import-cad", ops).expect("land part");
    (part, landing)
}

/// THE SPIKE GATE: a real STEP file → decoded faces survive into the pipeline → the AAG recognizes the
/// bore → it lands as a typed, queryable component through the validated-AI gate → "select all holes"
/// resolves it as a QUERY. Go/no-go for the milestone.
#[test]
fn spike_real_step_bore_lands_typed_and_select_all_holes_works() {
    let import = StepAssemblyReader
        .read(PLATE_WITH_BORE.as_bytes())
        .expect("the plate parses");
    assert!(import.never_empty() && import.never_silent(), "M15.7 holds");

    // The M15.11 face-survival seam: the decoded faces reach the pipeline keyed by part reference.
    let reference = &import.parts[0].reference;
    let faces = import
        .breps
        .get(reference)
        .expect("decoded B-rep faces survive into CadImport.breps");
    assert_eq!(faces.len(), 7, "6 planar + the bore cylinder");

    // The D5-closure regression: the in-pipeline fingerprint now sees the REAL surface histogram
    // (the M15.10 `brep_faces()` stub returned None — all-zero histograms in-pipeline).
    let ids = identities(&import);
    assert_eq!(
        ids[0].fingerprint.surface_hist[1], 1,
        "one cylindrical face counted in the in-pipeline histogram"
    );

    // Land with the AAG through the validated-AI gate — one undoable commit.
    let mut e = engine();
    let (part, landing) = land_part(&mut e, "plate with bore", faces);
    assert_eq!(
        landing.committed, 1,
        "the through-hole lands (rejected: {:?})",
        landing.rejected
    );
    assert!(
        landing.rejected.is_empty(),
        "nothing mislabeled: {:?}",
        landing.rejected
    );

    // The landed feature is TYPED data: kind + the bore's STEP face id + radius + an explainable why.
    let feat = landing.feature_entities[0];
    assert_eq!(
        e.get_field(feat, CAD_FEATURE, "kind"),
        Some(FieldValue::Str("through-hole".into()))
    );
    match e.get_field(feat, CAD_FEATURE, "faces") {
        Some(FieldValue::Str(s)) => assert_eq!(s, "18", "the bore's ADVANCED_FACE #id"),
        other => panic!("faces field: {other:?}"),
    }
    match e.get_field(feat, CAD_FEATURE, "radius") {
        Some(FieldValue::Number(r)) => assert!((r - 5.0).abs() < 1e-9),
        Some(FieldValue::Integer(r)) => assert_eq!(r, 5),
        other => panic!("radius field: {other:?}"),
    }
    match e.get_field(feat, CAD_FEATURE, "why") {
        Some(FieldValue::Str(w)) => {
            assert!(
                w.contains("TOWARD the axis"),
                "the explainable evidence: {w}"
            );
        }
        other => panic!("why field: {other:?}"),
    }
    assert_eq!(
        e.parent_of(feat),
        Some(part),
        "a feature is a child of its part"
    );

    // THE QUERY: "select all holes" is a typed resolution over the landed components — parts + features.
    let intent = parse_semantic("select all holes").expect("parses");
    assert_eq!(intent.verb, SemanticVerb::Select);
    let m = resolve_semantic(&e, &intent.noun);
    assert_eq!(m.features, vec![feat], "the hole feature entity resolves");
    assert_eq!(m.parts, vec![part], "…and the part containing it");

    // One Ctrl-Z peels the whole import INCLUDING the features (one commit).
    assert!(e.undo());
    assert!(
        e.get_field(feat, CAD_FEATURE, "kind").is_none(),
        "undo removed the landed feature"
    );
}

/// Classification + "hide all fasteners" acts as ONE undoable `SetActive` batch; undo restores. The
/// classification itself is honest (the plate classifies from stated evidence; the fastener class comes
/// from the measured `classify_part` on the part's geometry — asserted in interchange's unit tests; here
/// the flow is the target).
#[test]
fn classify_and_hide_all_fasteners_is_one_undoable_commit() {
    let import = StepAssemblyReader
        .read(PLATE_WITH_BORE.as_bytes())
        .expect("parses");
    let reference = import.parts[0].reference.clone();
    let faces = import.breps.get(&reference).unwrap();
    let mesh = &import.meshes[import.parts[0].mesh.unwrap()].tris;

    let mut e = engine();
    let (plate, landing) = land_part(&mut e, "plate", faces);

    // The plate's REAL classification (measured evidence: flat + planar-dominated + a mounting hole).
    let feats: Vec<RecognizedFeature> = landing
        .feature_entities
        .iter()
        .map(|_| RecognizedFeature {
            kind: FeatureKind::ThroughHole,
            faces: vec![18],
            axis: None,
            origin: None,
            radius: Some(5.0),
            depth: Some(5.0),
            confidence: 0.95,
            why: String::new(),
        })
        .collect();
    let c = classify_part(Some(faces), mesh, &feats, "plate");
    assert!(
        matches!(c.class, PartClass::Bracket | PartClass::Plate),
        "a flat holed part classifies as plate/bracket with stated evidence, got {:?}: {}",
        c.class,
        c.why
    );
    e.commit("classify", class_ops(plate, &c, Some(&reference), 1))
        .unwrap();

    // A second part, classified fastener (its geometric classification is unit-tested in interchange;
    // here we stamp the landed form to test the QUERY + ACT flow).
    let bolt = e.alloc_entity_id();
    let mut ops = vec![
        Op::CreateEntity {
            id: bolt,
            parent: None,
        },
        Op::SetField {
            entity: bolt,
            component: CAD_PART.into(),
            field: "fidelity".into(),
            value: FieldValue::Str("exact-brep".into()),
        },
    ];
    ops.extend(class_ops(
        bolt,
        &metrocalk_interchange::PartClassification {
            class: PartClass::Fastener,
            confidence: 0.9,
            why: "rod-shaped, dominated by one coaxial cylinder/cone family".into(),
        },
        Some("pd-77"),
        12,
    ));
    e.commit("import-bolt", ops).unwrap();

    // "hide all fasteners" → ONE batched SetActive commit; the plate is untouched; undo restores.
    let intent = parse_semantic("hide all fasteners").expect("parses");
    assert_eq!(intent.verb, SemanticVerb::Hide);
    assert_eq!(intent.noun, SemanticNoun::Class("fastener"));
    let m = resolve_semantic(&e, &intent.noun);
    assert_eq!(m.parts, vec![bolt]);
    let ops = semantic_act_ops(intent.verb, &m, &all_cad_parts(&e));
    assert_eq!(ops.len(), 1);
    e.commit("hide-fasteners", ops).unwrap();
    assert!(
        !e.is_active(bolt),
        "the fastener is hidden (deactivate-not-delete)"
    );
    assert!(e.is_active(plate), "the plate is untouched");
    assert!(e.undo(), "one Ctrl-Z restores");
    assert!(e.is_active(bolt));

    // ISOLATE scopes to CAD parts: isolating fasteners hides the plate, keeps the bolt.
    let m = resolve_semantic(&e, &SemanticNoun::Class("fastener"));
    let ops = semantic_act_ops(SemanticVerb::Isolate, &m, &all_cad_parts(&e));
    e.commit("isolate-fasteners", ops).unwrap();
    assert!(e.is_active(bolt) && !e.is_active(plate));
    assert!(e.undo());

    // Every "no" explained: an unknown noun and a nonsensical verb+noun both refuse with the vocabulary.
    let err = parse_semantic("select all flanges").unwrap_err();
    assert!(err.contains("part classes"), "{err}");
    let err = parse_semantic("hide all holes").unwrap_err();
    assert!(
        err.contains("defeaturing"),
        "the better path is named: {err}"
    );
}

/// The validated-AI gate, adversarially: a WRONG high-confidence label is REJECTED by the kernel check
/// (never lands, even at 0.99 — the learned-recognizer guard), and a kernel-VALID low-confidence one is
/// HELD (surfaced), never auto-committed.
#[test]
fn wrong_labels_rejected_and_low_confidence_held() {
    struct MockLearned;
    impl FeatureRecognizer for MockLearned {
        fn source(&self) -> &'static str {
            "learned"
        }
        fn deterministic(&self) -> bool {
            false
        }
        fn recognize(&self, faces: &[metrocalk_interchange::CadFace]) -> Vec<RecognizedFeature> {
            vec![
                // A confidently WRONG claim: "face #43 (a planar plate face) is a through-hole".
                RecognizedFeature {
                    kind: FeatureKind::ThroughHole,
                    faces: vec![43],
                    axis: None,
                    origin: None,
                    radius: Some(5.0),
                    depth: None,
                    confidence: 0.99,
                    why: "a wrong learned label".into(),
                },
                // A kernel-VALID but low-confidence pocket claim on real planar faces.
                RecognizedFeature {
                    kind: FeatureKind::Pocket,
                    faces: faces
                        .iter()
                        .filter(|f| f.id == 43 || f.id == 92)
                        .map(|f| f.id)
                        .collect(),
                    axis: None,
                    origin: None,
                    radius: None,
                    depth: None,
                    confidence: 0.6,
                    why: "a weak heuristic".into(),
                },
            ]
        }
    }

    let import = StepAssemblyReader
        .read(PLATE_WITH_BORE.as_bytes())
        .expect("parses");
    let faces = import.breps.values().next().unwrap().clone();
    let mut e = engine();
    let part = e.alloc_entity_id();
    e.commit(
        "part",
        vec![Op::CreateEntity {
            id: part,
            parent: None,
        }],
    )
    .unwrap();

    let landing = plan_semantic_land(&mut e, part, &faces, &MockLearned);
    assert_eq!(landing.committed, 0, "nothing auto-lands from the mock");
    assert_eq!(landing.rejected.len(), 1, "the wrong label is REJECTED");
    assert!(
        landing.rejected[0].1.contains("not cylindrical"),
        "with the kernel's reason: {}",
        landing.rejected[0].1
    );
    assert_eq!(landing.held.len(), 1, "the low-confidence claim is HELD");
    assert_eq!(landing.held[0].feature.kind, FeatureKind::Pocket);
    assert_eq!(landing.held[0].source, "learned");
}

/// Defeaturing: an explained receipt with MEASURED triangle deltas; applying is a revertible op (the
/// render handle swaps, the feature flags `suppressed`); ONE undo restores the original handle. The
/// exact B-rep is untouched by construction (faces live in the import, not the op).
#[test]
fn defeature_receipt_is_measured_and_revertible() {
    let import = StepAssemblyReader
        .read(PLATE_WITH_BORE.as_bytes())
        .expect("parses");
    let reference = import.parts[0].reference.clone();
    let faces = import.breps.get(&reference).unwrap();

    let mut e = engine();
    let part = e.alloc_entity_id();
    let feat = e.alloc_entity_id();
    let original_handle = "mtkcad:originalhash0000";
    e.commit(
        "seed",
        vec![
            Op::CreateEntity {
                id: part,
                parent: None,
            },
            Op::SetField {
                entity: part,
                component: "MeshRenderer".into(),
                field: "mesh".into(),
                value: FieldValue::Str(original_handle.into()),
            },
            Op::CreateEntity {
                id: feat,
                parent: Some(part),
            },
            // A landed sub-tolerance blend naming the bore's face (the flow under test is the
            // receipt + revertible swap; recognition correctness is the spike test above).
            Op::SetField {
                entity: feat,
                component: CAD_FEATURE.into(),
                field: "kind".into(),
                value: FieldValue::Str("blind-hole".into()),
            },
            Op::SetField {
                entity: feat,
                component: CAD_FEATURE.into(),
                field: "radius".into(),
                value: FieldValue::Number(5.0),
            },
            Op::SetField {
                entity: feat,
                component: CAD_FEATURE.into(),
                field: "faces".into(),
                value: FieldValue::Str("18".into()),
            },
        ],
    )
    .unwrap();

    // Below-tolerance (tol 6 > r 5) → proposed; the receipt carries MEASURED deltas.
    let plan = plan_defeature(&e, part, faces, 6.0);
    assert_eq!(plan.rows.len(), 1);
    assert!(
        plan.tris_after < plan.tris_before,
        "suppressing the bore removes its wall triangles: {} → {}",
        plan.tris_before,
        plan.tris_after
    );
    assert!(
        plan.receipt.contains("→"),
        "a human-readable receipt: {}",
        plan.receipt
    );
    assert!(
        plan.rows[0].tris_removed > 0,
        "the per-feature delta is measured"
    );

    // Above-tolerance → NOT proposed (never a silent size-blind sweep).
    assert!(plan_defeature(&e, part, faces, 1.0).rows.is_empty());

    // Apply = ONE revertible commit: the handle swaps + the feature flags; undo restores BOTH.
    e.commit("defeature", defeature_ops(&plan)).unwrap();
    match e.get_field(part, "MeshRenderer", "mesh") {
        Some(FieldValue::Str(h)) => assert!(h.ends_with(":defeat"), "{h}"),
        other => panic!("mesh handle: {other:?}"),
    }
    assert_eq!(
        e.get_field(feat, CAD_FEATURE, "suppressed"),
        Some(FieldValue::Bool(true))
    );
    assert!(e.undo());
    assert_eq!(
        e.get_field(part, "MeshRenderer", "mesh"),
        Some(FieldValue::Str(original_handle.into())),
        "undo restores the ORIGINAL content-addressed mesh — nothing was destroyed"
    );
}

/// The feature-informed collider: a coax-dominated part gets a KERNEL-VERIFIED bounding cylinder (with
/// the fit evidence in `why`); a planar plate is an explained refusal (the V-HACD/hull fallback path).
#[test]
fn feature_collider_is_kernel_verified_or_refused_with_reason() {
    // A bolt-ish part: shank + head cylinders, coaxial (built as decoded faces — recognition input).
    let import = StepAssemblyReader
        .read(PLATE_WITH_BORE.as_bytes())
        .expect("parses");
    let plate_faces = import.breps.values().next().unwrap();
    let plate_mesh = &import.meshes[import.parts[0].mesh.unwrap()].tris;

    // The plate: cylinder family is 1 of 7 faces → an EXPLAINED refusal naming the fallback.
    let err = plan_feature_collider(plate_faces, plate_mesh).unwrap_err();
    assert!(err.contains("V-HACD"), "the fallback is named: {err}");

    // A shaft: one long cylinder face + 2 cap planes; mesh = a matching closed tube (from the import's
    // own tessellation of a bore-less cylinder body would need a second fixture — build the mesh
    // directly; the planner's kernel check runs on THIS mesh).
    let n = 24usize;
    let (mut positions, mut triangles) = (Vec::new(), Vec::new());
    for i in 0..n {
        #[allow(clippy::cast_precision_loss)]
        let a = std::f64::consts::TAU * i as f64 / n as f64;
        positions.push([3.0 * a.cos(), 3.0 * a.sin(), 0.0]);
        positions.push([3.0 * a.cos(), 3.0 * a.sin(), 30.0]);
    }
    positions.push([0.0, 0.0, 0.0]);
    positions.push([0.0, 0.0, 30.0]);
    #[allow(clippy::cast_possible_truncation)]
    let (c0, c1) = ((2 * n) as u32, (2 * n + 1) as u32);
    for i in 0..n {
        #[allow(clippy::cast_possible_truncation)]
        let (a0, a1) = ((2 * i) as u32, (2 * i + 1) as u32);
        #[allow(clippy::cast_possible_truncation)]
        let (b0, b1) = ((2 * ((i + 1) % n)) as u32, (2 * ((i + 1) % n) + 1) as u32);
        triangles.push([a0, b0, b1]);
        triangles.push([a0, b1, a1]);
        triangles.push([a0, c0, b0]); // bottom cap fan
        triangles.push([a1, b1, c1]); // top cap fan
    }
    let shaft_mesh = metrocalk_csg::TriMesh {
        positions,
        triangles,
    };
    // The decoded faces: one full cylinder (recognized) + we reuse the bore face's surface with the
    // right frame by borrowing the plate's cylinder face and flipping it to a wall.
    let mut shaft_faces: Vec<metrocalk_interchange::CadFace> =
        plate_faces.iter().filter(|f| f.id == 18).cloned().collect();
    shaft_faces[0].same_sense = true; // outward — a wall, not a bore
    let plan = plan_feature_collider(&shaft_faces, &shaft_mesh).expect("a cylinder is proposed");
    assert_eq!(plan.shape, "cylinder");
    assert!(plan.why.contains("bounds every vertex"), "{}", plan.why);
    match (plan.fields.get("radius"), plan.fields.get("halfHeight")) {
        (Some(FieldValue::Number(r)), Some(FieldValue::Number(h))) => {
            assert!((r - 3.0).abs() < 1e-6, "measured bounding radius {r}");
            assert!((h - 15.0).abs() < 1e-6, "measured half height {h}");
        }
        other => panic!("cylinder fields: {other:?}"),
    }
}

/// Recognition is deterministic END-TO-END through the real reader: same bytes ⇒ same features ⇒ the
/// same landed field maps, bit-for-bit (ADR-020 quantized decisions).
#[test]
fn end_to_end_recognition_is_deterministic() {
    let read = || {
        let import = StepAssemblyReader.read(PLATE_WITH_BORE.as_bytes()).unwrap();
        let faces = import.breps.values().next().unwrap().clone();
        AagRecognizer.recognize(&faces)
    };
    let (a, b) = (read(), read());
    assert_eq!(a, b, "same file ⇒ same recognition, bit-for-bit");
    assert!(!a.is_empty());
    let fields_a: Vec<BTreeMap<String, FieldValue>> = a
        .iter()
        .map(|f| metrocalk_editor_shell::feature_fields(f, "aag"))
        .collect();
    let fields_b: Vec<BTreeMap<String, FieldValue>> = b
        .iter()
        .map(|f| metrocalk_editor_shell::feature_fields(f, "aag"))
        .collect();
    assert_eq!(fields_a, fields_b);
}

/// The REAL M15.8 evidence fixture (4 curved solids — cylinder / sphere band / torus ring / cone
/// frustum): the AAG recognizes the free-standing cylinder as a BOSS (outward wall, full sweep) and
/// stays HONEST about the rest (no guessed labels on a lone sphere band / torus ring with no adjacency).
#[test]
fn quartet_fixture_recognizes_the_boss_and_never_guesses() {
    let bytes = include_bytes!("../e2e/samples/analytic_quartet.stp");
    let import = StepAssemblyReader.read(bytes).expect("the quartet parses");
    let mut all: Vec<RecognizedFeature> = Vec::new();
    for faces in import.breps.values() {
        all.extend(AagRecognizer.recognize(faces));
    }
    assert!(
        all.iter().any(|f| f.kind == FeatureKind::Boss),
        "the free-standing cylinder reads as a boss: {all:?}"
    );
    for f in &all {
        assert!(f.confidence >= 0.5, "no scraping-the-barrel labels: {f:?}");
    }
}

/// PART_CLASS is honestly absent for an Unknown classification — never a fake label.
#[test]
fn unknown_class_is_not_stamped() {
    let mut e = engine();
    let part = e.alloc_entity_id();
    e.commit(
        "part",
        vec![Op::CreateEntity {
            id: part,
            parent: None,
        }],
    )
    .unwrap();
    let ops = class_ops(
        part,
        &metrocalk_interchange::PartClassification {
            class: PartClass::Unknown,
            confidence: 0.0,
            why: "no rule matched".into(),
        },
        None,
        1,
    );
    assert!(ops.is_empty(), "Unknown is an honest absence, not a stamp");
    assert!(e.get_field(part, PART_CLASS, "class").is_none());
}
