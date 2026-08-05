//! **M15.11 (ADR-081) — the AI semantic pass, landed on the substrate.** The `interchange` recognizers
//! (AAG symbolic baseline · the learned seam · mesh→primitive recovery) run on ALREADY-DECODED geometry;
//! this module turns their proposals into **validated, typed, queryable, revertible ECS data**:
//!
//! - a recognized feature = a **child entity of its part** (the M15.3 PMI "applies-to = parent" pattern)
//!   carrying the typed [`CAD_FEATURE`] component — durable (component data, reload-safe), queryable
//!   ("select all holes" is a scan of typed fields, never a label parse), one undoable commit;
//! - every proposal passes the **validated-AI gate** before any op is emitted: the schema check
//!   ([`feature_component_meta`]) + the **kernel check** (`metrocalk_interchange::verify_feature`
//!   re-derives the geometric claim from the geometry) + the **confidence band** (below
//!   [`AUTO_COMMIT_CONFIDENCE`] a recognition is HELD for human adjudication, never auto-committed —
//!   the M15.10 discipline applied to labels);
//! - **semantic search** ("hide all fasteners" · "select all holes" · "isolate the brackets") = a typed
//!   verb+noun grammar resolved over the landed components, acting as ONE undoable commit
//!   (`Op::SetActive` batch — deactivate-not-delete, ADR-026), every "no" explained;
//! - the **defeaturing recommender** proposes suppressing sub-tolerance detail with a human-readable
//!   receipt + measured triangle deltas, applied as a REVERTIBLE op (the render mesh handle swaps; the
//!   exact B-rep and the original content-addressed mesh are retained — undo restores);
//! - the **feature-informed collider** planner fits a primitive to the recognized coaxial family and the
//!   kernel VERIFIES it bounds the part before it is ever proposed (V-HACD decomposition is the fallback).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use metrocalk_core::registry::{ComponentMeta, FieldType};
use metrocalk_core::variant::INSTANCE_META;
use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_csg::TriMesh;
use metrocalk_ecs::World;
use metrocalk_interchange::{
    fingerprint, verify_feature, CadFace, FeatureRecognizer, PartClass, PartClassification,
    RecognizedFeature,
};

/// The typed feature component carried by a recognized-feature child entity. String-named registry
/// component (the `CadPart`/`ReimportId`/`Joint` pattern); the schema lives in
/// [`feature_component_meta`] and every landing validates against it.
pub const CAD_FEATURE: &str = "CadFeature";

/// The part-classification component carried by the part entity itself.
pub const PART_CLASS: &str = "PartClass";

/// At/above this confidence a kernel-verified recognition lands automatically; below it the recognition
/// is **surfaced for human adjudication** (held — never auto-committed). The AAG's structurally-certain
/// rules score above this; its weak heuristics (pockets, partial arcs) deliberately below.
pub const AUTO_COMMIT_CONFIDENCE: f64 = 0.8;

/// The [`ComponentMeta`] schema for [`CAD_FEATURE`] — what the validated-AI gate checks proposals
/// against. Kept in the shell (not `/core` stdlib), like the M15.3 FCF.
#[must_use]
pub fn feature_component_meta() -> ComponentMeta {
    ComponentMeta::builder(CAD_FEATURE)
        .category("Props")
        .field("kind", FieldType::String, true)
        .field("confidence", FieldType::Number, true)
        .field("source", FieldType::String, true)
        .field("why", FieldType::String, true)
        .field("faces", FieldType::String, true)
        .field("radius", FieldType::Number, false)
        .field("depth", FieldType::Number, false)
        .field("ax", FieldType::Number, false)
        .field("ay", FieldType::Number, false)
        .field("az", FieldType::Number, false)
        .field("px", FieldType::Number, false)
        .field("py", FieldType::Number, false)
        .field("pz", FieldType::Number, false)
        .field("suppressed", FieldType::Boolean, false)
        .tag("cad")
        .tag("semantic")
        .ui_hint(
            "kind",
            "through-hole|blind-hole|boss|fillet|round|chamfer|pocket",
        )
        .ui_hint(
            "source",
            "aag|learned|recovered — the recognizer provenance, honesty-labeled",
        )
        .build()
}

/// The [`ComponentMeta`] schema for [`PART_CLASS`].
#[must_use]
pub fn part_class_component_meta() -> ComponentMeta {
    ComponentMeta::builder(PART_CLASS)
        .category("Props")
        .field("class", FieldType::String, true)
        .field("confidence", FieldType::Number, true)
        .field("why", FieldType::String, true)
        .field("instanceOf", FieldType::String, false)
        .field("instances", FieldType::Integer, false)
        .tag("cad")
        .tag("semantic")
        .ui_hint(
            "class",
            "fastener|washer|pin|shaft|plate|bracket|housing|unknown",
        )
        .build()
}

/// Schema-validate a field map against a [`ComponentMeta`] — the schema half of the validated-AI gate
/// (the [`crate::ai::apply_ai_patch`] discipline for a field map that isn't JSON).
///
/// # Errors
/// The first violation, human-readable: an unknown field, a type mismatch, or a missing required field.
pub fn validate_fields(
    meta: &ComponentMeta,
    fields: &BTreeMap<String, FieldValue>,
) -> Result<(), String> {
    for (name, value) in fields {
        let Some(spec) = meta.fields.iter().find(|f| &f.name == name) else {
            return Err(format!("unknown field `{name}` on {}", meta.name));
        };
        let ok = matches!(
            (spec.ty, value),
            (FieldType::Integer, FieldValue::Integer(_))
                | (
                    FieldType::Number,
                    FieldValue::Number(_) | FieldValue::Integer(_)
                )
                | (FieldType::Boolean, FieldValue::Bool(_))
                | (FieldType::String, FieldValue::Str(_))
        );
        if !ok {
            return Err(format!(
                "field `{name}` on {} expects {:?}, got {value:?}",
                meta.name, spec.ty
            ));
        }
    }
    for spec in &meta.fields {
        if spec.required && !fields.contains_key(&spec.name) {
            return Err(format!(
                "missing required field `{}` on {}",
                spec.name, meta.name
            ));
        }
    }
    Ok(())
}

/// The typed field map a [`RecognizedFeature`] lands as (the [`CAD_FEATURE`] payload).
#[must_use]
pub fn feature_fields(feature: &RecognizedFeature, source: &str) -> BTreeMap<String, FieldValue> {
    let mut f: BTreeMap<String, FieldValue> = BTreeMap::new();
    f.insert("kind".into(), FieldValue::Str(feature.kind.token().into()));
    f.insert("confidence".into(), FieldValue::Number(feature.confidence));
    f.insert("source".into(), FieldValue::Str(source.into()));
    f.insert("why".into(), FieldValue::Str(feature.why.clone()));
    f.insert(
        "faces".into(),
        FieldValue::Str(
            feature
                .faces
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(","),
        ),
    );
    if let Some(r) = feature.radius {
        f.insert("radius".into(), FieldValue::Number(r));
    }
    if let Some(d) = feature.depth {
        f.insert("depth".into(), FieldValue::Number(d));
    }
    if let Some(a) = feature.axis {
        f.insert("ax".into(), FieldValue::Number(a[0]));
        f.insert("ay".into(), FieldValue::Number(a[1]));
        f.insert("az".into(), FieldValue::Number(a[2]));
    }
    if let Some(o) = feature.origin {
        f.insert("px".into(), FieldValue::Number(o[0]));
        f.insert("py".into(), FieldValue::Number(o[1]));
        f.insert("pz".into(), FieldValue::Number(o[2]));
    }
    f
}

/// A recognition the gate HELD (below [`AUTO_COMMIT_CONFIDENCE`]) — surfaced for a human decision,
/// never auto-committed. The M15.10 adjudication discipline applied to semantic labels.
#[derive(Clone, PartialEq, Debug)]
pub struct HeldRecognition {
    /// The part the recognition is about.
    pub part: EntityId,
    /// The proposal itself (kernel-verified — held only for its confidence, so an accept is safe).
    pub feature: RecognizedFeature,
    /// The recognizer provenance (`"aag"` / `"learned"` / `"recovered"`).
    pub source: String,
}

/// The outcome of gating one part's recognitions: ops to fold into the caller's ONE commit, plus the
/// held + rejected lists (never silent).
#[derive(Default, Debug)]
pub struct SemanticLanding {
    /// Feature child-entity ops (CreateEntity + typed fields + outliner meta) — fold into ONE commit.
    pub ops: Vec<Op>,
    /// The created feature entity ids, aligned with the committed features.
    pub feature_entities: Vec<EntityId>,
    /// Committed (auto-landed) feature count.
    pub committed: usize,
    /// Held for adjudication (low confidence).
    pub held: Vec<HeldRecognition>,
    /// Rejected by the gate: `(kind token, the reason)` — a schema/kernel failure is loud, never dropped.
    pub rejected: Vec<(String, String)>,
}

/// Run one recognizer's proposals for one part through the **validated-AI gate** and emit the landing
/// ops. Order: (1) schema check → (2) kernel check ([`verify_feature`] re-derives the claim from the
/// geometry — a wrong label is REJECTED with the reason even at confidence 0.99) → (3) the confidence
/// band (kernel-valid but low-confidence ⇒ HELD). Deterministic: proposals are processed in recognizer
/// order (itself deterministic for the AAG).
pub fn plan_semantic_land<W: World>(
    engine: &mut Engine<W>,
    part: EntityId,
    faces: &[CadFace],
    recognizer: &dyn FeatureRecognizer,
) -> SemanticLanding {
    let mut out = SemanticLanding::default();
    let meta = feature_component_meta();
    let source = recognizer.source();
    for feature in recognizer.recognize(faces) {
        // (1) + (2): schema, then the kernel re-derivation. A failure is recorded, never silent.
        let fields = feature_fields(&feature, source);
        if let Err(e) = validate_fields(&meta, &fields) {
            out.rejected.push((feature.kind.token().into(), e));
            continue;
        }
        if let Err(e) = verify_feature(&feature, faces) {
            out.rejected.push((feature.kind.token().into(), e));
            continue;
        }
        // (3): the confidence band — kernel-valid but weak evidence is a QUESTION, not a fact.
        if feature.confidence < AUTO_COMMIT_CONFIDENCE {
            out.held.push(HeldRecognition {
                part,
                feature,
                source: source.into(),
            });
            continue;
        }
        out.feature_entities.push(push_feature_ops(
            engine,
            &mut out.ops,
            part,
            &feature,
            &fields,
        ));
        out.committed += 1;
    }
    out
}

/// The ops for ONE feature child entity (already gate-passed): CreateEntity under the part + the typed
/// [`CAD_FEATURE`] fields + a plain-language outliner name.
fn push_feature_ops<W: World>(
    engine: &mut Engine<W>,
    ops: &mut Vec<Op>,
    part: EntityId,
    feature: &RecognizedFeature,
    fields: &BTreeMap<String, FieldValue>,
) -> EntityId {
    let id = engine.alloc_entity_id();
    ops.push(Op::CreateEntity {
        id,
        parent: Some(part),
    });
    for (field, value) in fields {
        ops.push(Op::SetField {
            entity: id,
            component: CAD_FEATURE.into(),
            field: field.clone(),
            value: value.clone(),
        });
    }
    let label = match feature.radius {
        Some(r) => format!("{} ∅{:.1}", feature.kind.token(), 2.0 * r),
        None => feature.kind.token().to_string(),
    };
    ops.push(Op::SetField {
        entity: id,
        component: INSTANCE_META.into(),
        field: "name".into(),
        value: FieldValue::Str(label),
    });
    ops.push(Op::SetField {
        entity: id,
        component: INSTANCE_META.into(),
        field: "kind".into(),
        value: FieldValue::Str("feature".into()),
    });
    id
}

/// The ops that stamp a part's classification (+ its instance group) onto the part entity. A
/// zero-confidence `Unknown` is NOT stamped — an honest absence, not a fake label.
#[must_use]
pub fn class_ops(
    part: EntityId,
    class: &PartClassification,
    instance_of: Option<&str>,
    instances: usize,
) -> Vec<Op> {
    if class.class == PartClass::Unknown {
        return Vec::new();
    }
    let mut ops = vec![
        Op::SetField {
            entity: part,
            component: PART_CLASS.into(),
            field: "class".into(),
            value: FieldValue::Str(class.class.token().into()),
        },
        Op::SetField {
            entity: part,
            component: PART_CLASS.into(),
            field: "confidence".into(),
            value: FieldValue::Number(class.confidence),
        },
        Op::SetField {
            entity: part,
            component: PART_CLASS.into(),
            field: "why".into(),
            value: FieldValue::Str(class.why.clone()),
        },
    ];
    if let Some(rep) = instance_of {
        ops.push(Op::SetField {
            entity: part,
            component: PART_CLASS.into(),
            field: "instanceOf".into(),
            value: FieldValue::Str(rep.into()),
        });
        #[allow(clippy::cast_possible_wrap)]
        ops.push(Op::SetField {
            entity: part,
            component: PART_CLASS.into(),
            field: "instances".into(),
            value: FieldValue::Integer(instances as i64),
        });
    }
    ops
}

// ── Semantic search (D6): a typed verb+noun grammar over the landed components ────────────────────────────

/// The action verb of a semantic query.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SemanticVerb {
    Select,
    Hide,
    Show,
    Isolate,
    Count,
}

/// What the query targets: part classes resolve to part entities; feature kinds resolve to feature
/// entities (+ their parent parts).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SemanticNoun {
    /// One or more [`CAD_FEATURE`] `kind` tokens ("holes" → through-hole + blind-hole).
    Features(Vec<&'static str>),
    /// A [`PART_CLASS`] `class` token.
    Class(&'static str),
}

/// A parsed semantic query.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SemanticIntent {
    pub verb: SemanticVerb,
    pub noun: SemanticNoun,
}

const CLASS_VOCAB: &str =
    "bolts/screws/nuts/fasteners · washers · pins · shafts · plates/panels · \
                           brackets · housings";
const FEATURE_VOCAB: &str = "holes/bores · bosses · fillets · rounds · chamfers · pockets";

/// Parse a natural-language semantic query into a typed intent — deterministic token matching (the M3.2
/// resolver discipline: offline, no LLM in the path; an LLM tier can PRODUCE these sentences, never
/// bypass the grammar).
///
/// # Errors
/// An explained refusal naming what IS understood (every "no" explained — ADR-016).
pub fn parse_semantic(query: &str) -> Result<SemanticIntent, String> {
    let lc = query.to_lowercase();
    let tokens: Vec<&str> = lc
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    let has = |t: &str| tokens.contains(&t);

    let verb = if has("hide") || has("conceal") {
        SemanticVerb::Hide
    } else if has("show") || has("unhide") || has("reveal") {
        SemanticVerb::Show
    } else if has("isolate") || has("only") {
        SemanticVerb::Isolate
    } else if has("count") || has("many") {
        SemanticVerb::Count
    } else if has("select") || has("pick") || has("highlight") || has("find") {
        SemanticVerb::Select
    } else {
        return Err(format!(
            "no verb understood in \"{query}\" — I know: select · hide · show · isolate · count"
        ));
    };

    let noun = if [
        "bolt",
        "bolts",
        "screw",
        "screws",
        "nut",
        "nuts",
        "fastener",
        "fasteners",
        "rivet",
        "rivets",
    ]
    .iter()
    .any(|t| has(t))
    {
        SemanticNoun::Class("fastener")
    } else if has("washer") || has("washers") {
        SemanticNoun::Class("washer")
    } else if has("pin") || has("pins") {
        SemanticNoun::Class("pin")
    } else if has("shaft") || has("shafts") {
        SemanticNoun::Class("shaft")
    } else if ["plate", "plates", "panel", "panels"]
        .iter()
        .any(|t| has(t))
    {
        SemanticNoun::Class("plate")
    } else if has("bracket") || has("brackets") {
        SemanticNoun::Class("bracket")
    } else if has("housing") || has("housings") {
        SemanticNoun::Class("housing")
    } else if ["hole", "holes", "bore", "bores"].iter().any(|t| has(t)) {
        SemanticNoun::Features(vec!["through-hole", "blind-hole"])
    } else if has("boss") || has("bosses") {
        SemanticNoun::Features(vec!["boss"])
    } else if has("fillet") || has("fillets") {
        SemanticNoun::Features(vec!["fillet"])
    } else if has("round") || has("rounds") {
        SemanticNoun::Features(vec!["round"])
    } else if has("chamfer") || has("chamfers") {
        SemanticNoun::Features(vec!["chamfer"])
    } else if has("pocket") || has("pockets") {
        SemanticNoun::Features(vec!["pocket"])
    } else {
        return Err(format!(
            "no target understood in \"{query}\" — I know part classes ({CLASS_VOCAB}) and feature \
             kinds ({FEATURE_VOCAB})"
        ));
    };

    // A feature is geometry OF a part — hiding it isn't a visibility toggle, it's defeaturing. Refuse
    // with the better path rather than doing something surprising (ux_quality: honest, explained).
    if matches!(noun, SemanticNoun::Features(_))
        && matches!(
            verb,
            SemanticVerb::Hide | SemanticVerb::Show | SemanticVerb::Isolate
        )
    {
        return Err(
            "a feature (a hole/fillet/chamfer) is part of its part's geometry — it can't be hidden \
             on its own. Use select/count for features, or the defeaturing recommender to suppress \
             sub-tolerance detail."
                .into(),
        );
    }
    Ok(SemanticIntent { verb, noun })
}

/// What a semantic query resolved to.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct SemanticMatches {
    /// Matched part entities (class queries; for feature queries: the parts CONTAINING the features).
    pub parts: Vec<EntityId>,
    /// Matched feature entities (feature queries only).
    pub features: Vec<EntityId>,
}

/// Resolve a parsed intent over the landed components — a deterministic scan of the typed fields (the
/// M15.3 `fcfs_on` idiom; at CAD-scene scale a scan is instant, and a pre-indexed pair is the named
/// scale-up). Sorted by entity id.
#[must_use]
pub fn resolve_semantic<W: World>(engine: &Engine<W>, noun: &SemanticNoun) -> SemanticMatches {
    let mut m = SemanticMatches::default();
    match noun {
        SemanticNoun::Class(token) => {
            for id in engine.entity_ids() {
                if let Some(FieldValue::Str(c)) = engine.get_field(id, PART_CLASS, "class") {
                    if c == *token {
                        m.parts.push(id);
                    }
                }
            }
        }
        SemanticNoun::Features(kinds) => {
            for id in engine.entity_ids() {
                if let Some(FieldValue::Str(k)) = engine.get_field(id, CAD_FEATURE, "kind") {
                    if kinds.contains(&k.as_str()) {
                        m.features.push(id);
                        if let Some(p) = engine.parent_of(id) {
                            if !m.parts.contains(&p) {
                                m.parts.push(p);
                            }
                        }
                    }
                }
            }
        }
    }
    m.parts.sort_unstable();
    m.features.sort_unstable();
    m
}

/// The ops for an acting verb (hide / show / isolate) over resolved CLASS matches — one batched,
/// undoable `SetActive` commit (deactivate-not-delete, ADR-026). `all_cad_parts` scopes ISOLATE to CAD
/// parts (isolating brackets hides the other CAD parts, not the user's non-CAD scene).
#[must_use]
pub fn semantic_act_ops(
    verb: SemanticVerb,
    matches: &SemanticMatches,
    all_cad_parts: &[EntityId],
) -> Vec<Op> {
    match verb {
        SemanticVerb::Hide => matches
            .parts
            .iter()
            .map(|&entity| Op::SetActive {
                entity,
                active: false,
            })
            .collect(),
        SemanticVerb::Show => matches
            .parts
            .iter()
            .map(|&entity| Op::SetActive {
                entity,
                active: true,
            })
            .collect(),
        SemanticVerb::Isolate => all_cad_parts
            .iter()
            .map(|&entity| Op::SetActive {
                entity,
                active: matches.parts.contains(&entity),
            })
            .collect(),
        SemanticVerb::Select | SemanticVerb::Count => Vec::new(),
    }
}

/// Every CAD part entity in the scene (carries `CadPart`) — the isolate scope.
#[must_use]
pub fn all_cad_parts<W: World>(engine: &Engine<W>) -> Vec<EntityId> {
    let mut out: Vec<EntityId> = engine
        .entity_ids()
        .into_iter()
        .filter(|&id| {
            engine
                .get_field(id, crate::cad_import::CAD_PART, "fidelity")
                .is_some()
        })
        .collect();
    out.sort_unstable();
    out
}

// ── The defeaturing recommender (D5) ───────────────────────────────────────────────────────────────────────

/// One proposed suppression: a landed sub-tolerance feature and the measured effect of removing its
/// faces from the render tessellation.
#[derive(Clone, PartialEq, Debug)]
pub struct DefeatureRow {
    pub part: EntityId,
    pub feature_entity: EntityId,
    /// The feature kind token + its size (radius for blends/holes, depth for chamfers), for the receipt.
    pub kind: String,
    pub size: f64,
    /// Triangles removed by suppressing this feature's faces (measured on the re-tessellation).
    pub tris_removed: usize,
}

/// The recommender's proposal: rows + a human-readable receipt + the measured totals + the lighter
/// meshes (per part). NOTHING is applied — the caller shows the receipt and applies on accept.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct DefeaturePlan {
    pub rows: Vec<DefeatureRow>,
    pub receipt: String,
    pub tris_before: usize,
    pub tris_after: usize,
    /// Per part: the re-tessellated lighter mesh + its content-address handle.
    pub meshes: Vec<(EntityId, TriMesh, String)>,
}

/// Which landed feature kinds are defeaturing candidates (visual detail, not load-bearing form).
const DEFEATURE_KINDS: [&str; 4] = ["fillet", "round", "chamfer", "blind-hole"];

/// Build a defeaturing proposal for one part: its landed sub-`tol` features (radius/depth < `tol`, in
/// scene units) are candidates; the lighter mesh is the part's faces re-tessellated WITHOUT the
/// suppressed feature faces — measured, deterministic, and honest (the exact B-rep + the original mesh
/// stay retained; only the render handle would swap).
#[must_use]
#[allow(clippy::cast_precision_loss)] // Triangle budgets are far below f64's exact-integer range.
pub fn plan_defeature<W: World>(
    engine: &Engine<W>,
    part: EntityId,
    faces: &[CadFace],
    tol: f64,
) -> DefeaturePlan {
    let mut plan = DefeaturePlan::default();
    // The landed features on this part, smallest first (deterministic).
    let mut candidates: Vec<(EntityId, String, f64, Vec<u64>)> = Vec::new();
    for id in engine.entity_ids() {
        if engine.parent_of(id) != Some(part) {
            continue;
        }
        let Some(FieldValue::Str(kind)) = engine.get_field(id, CAD_FEATURE, "kind") else {
            continue;
        };
        if !DEFEATURE_KINDS.contains(&kind.as_str()) {
            continue;
        }
        if matches!(
            engine.get_field(id, CAD_FEATURE, "suppressed"),
            Some(FieldValue::Bool(true))
        ) {
            continue; // already suppressed
        }
        // Both value arms (a whole-number Number round-trips as Integer — the M11.2 scale bug's lesson).
        #[allow(clippy::cast_precision_loss)]
        let as_f64 = |v: Option<FieldValue>| match v {
            Some(FieldValue::Number(x)) => Some(x),
            Some(FieldValue::Integer(i)) => Some(i as f64),
            _ => None,
        };
        let size = match (
            as_f64(engine.get_field(id, CAD_FEATURE, "radius")),
            as_f64(engine.get_field(id, CAD_FEATURE, "depth")),
        ) {
            (Some(r), _) => r,
            (_, Some(d)) => d,
            _ => continue,
        };
        if size >= tol {
            continue;
        }
        let face_ids: Vec<u64> = match engine.get_field(id, CAD_FEATURE, "faces") {
            Some(FieldValue::Str(s)) => s.split(',').filter_map(|t| t.parse().ok()).collect(),
            _ => continue,
        };
        candidates.push((id, kind, size, face_ids));
    }
    candidates.sort_by_key(|candidate| candidate.0);
    if candidates.is_empty() {
        return plan;
    }

    let before = metrocalk_interchange::tessellate_faces(faces);
    plan.tris_before = before.triangle_count();
    let mut suppressed_faces: Vec<u64> = Vec::new();
    let mut last_count = plan.tris_before;
    for (feature_entity, kind, size, face_ids) in candidates {
        suppressed_faces.extend(&face_ids);
        let kept: Vec<CadFace> = faces
            .iter()
            .filter(|f| !suppressed_faces.contains(&f.id))
            .cloned()
            .collect();
        let mesh = metrocalk_interchange::tessellate_faces(&kept);
        let count = mesh.triangle_count();
        plan.rows.push(DefeatureRow {
            part,
            feature_entity,
            kind,
            size,
            tris_removed: last_count.saturating_sub(count),
        });
        last_count = count;
    }
    let kept: Vec<CadFace> = faces
        .iter()
        .filter(|f| !suppressed_faces.contains(&f.id))
        .cloned()
        .collect();
    let lighter = metrocalk_interchange::tessellate_faces(&kept);
    plan.tris_after = lighter.triangle_count();
    let handle = format!(
        "mtkcad:{:016x}:defeat",
        metrocalk_interchange::mesh_hash(&lighter)
    );
    plan.meshes.push((part, lighter, handle));

    let mut receipt = format!(
        "Suppress {} sub-{tol:.1} feature(s) on this part? {} → {} triangles (−{:.0}%). Keeps the \
         exact B-rep + the original mesh; one undo restores.",
        plan.rows.len(),
        plan.tris_before,
        plan.tris_after,
        100.0 * (plan.tris_before.saturating_sub(plan.tris_after)) as f64
            / (plan.tris_before.max(1)) as f64
    );
    for r in &plan.rows {
        let _ = write!(
            receipt,
            "\n  · {} ({:.2}) — −{} tris",
            r.kind, r.size, r.tris_removed
        );
    }
    plan.receipt = receipt;
    plan
}

/// The ops that APPLY an accepted defeature plan — a revertible transaction: the part's render mesh
/// handle swaps to the lighter mesh and each suppressed feature is flagged. The original mesh stays in
/// the content-addressed store and the exact B-rep is untouched — one Ctrl-Z restores everything.
#[must_use]
pub fn defeature_ops(plan: &DefeaturePlan) -> Vec<Op> {
    let mut ops = Vec::new();
    for (part, _mesh, handle) in &plan.meshes {
        ops.push(Op::SetField {
            entity: *part,
            component: "MeshRenderer".into(),
            field: "mesh".into(),
            value: FieldValue::Str(handle.clone()),
        });
    }
    for row in &plan.rows {
        ops.push(Op::SetField {
            entity: row.feature_entity,
            component: CAD_FEATURE.into(),
            field: "suppressed".into(),
            value: FieldValue::Bool(true),
        });
    }
    ops
}

// ── The feature-informed collider planner (D4) ─────────────────────────────────────────────────────────────

/// A kernel-verified primitive collider proposal for a part. `fields` are the `Collider` component's
/// flat scalar fields (the stdlib vocabulary + the cylinder frame extension).
#[derive(Clone, PartialEq, Debug)]
pub struct ColliderPlan {
    /// `"cylinder"` — the proposed `Collider.shape` token.
    pub shape: String,
    pub fields: BTreeMap<String, FieldValue>,
    /// The evidence: which recognized family, the fit ratio, the bound check.
    pub why: String,
}

/// Fit a **cylinder collider** to a part dominated by one recognized coaxial family (a boss/shaft/
/// fastener) and KERNEL-VERIFY it: (a) the axis comes from real recognized geometry, (b) the cylinder
/// BOUNDS every mesh vertex by construction (radius/half-height are measured maxima), and (c) the fit is
/// tight enough to be worth it (mesh volume ≥ [`CYLINDER_FIT_MIN`] of the cylinder's) — a poor fit is an
/// explained refusal, and the caller falls back to convex decomposition (V-HACD) / hull.
///
/// # Errors
/// The explained reason no cylinder is proposed (no coaxial family / degenerate axis / poor fit).
#[allow(
    clippy::cast_possible_truncation,
    clippy::many_single_char_names,
    clippy::too_many_lines
)] // Quantized axis math uses canonical analytic notation and one auditable fit gate.
pub fn plan_feature_collider(faces: &[CadFace], mesh: &TriMesh) -> Result<ColliderPlan, String> {
    use metrocalk_interchange::AnalyticSurface;
    if mesh.triangles.is_empty() {
        return Err("no mesh to fit".into());
    }
    // The dominant recognized coaxial cylinder/cone family (by face count, ties by axis key order).
    let mut by_axis: BTreeMap<[i64; 3], (usize, [f64; 3])> = BTreeMap::new();
    for f in faces {
        let Some(AnalyticSurface::Cylinder { frame, .. } | AnalyticSurface::Cone { frame, .. }) =
            &f.recognized
        else {
            continue;
        };
        let z = [frame[8], frame[9], frame[10]];
        let n = (z[0] * z[0] + z[1] * z[1] + z[2] * z[2]).sqrt();
        if n < 1e-12 {
            continue;
        }
        let mut a = [z[0] / n, z[1] / n, z[2] / n];
        // Canonical sign (an axis is undirected).
        let k = [
            (a[0] * 1e6).round() as i64,
            (a[1] * 1e6).round() as i64,
            (a[2] * 1e6).round() as i64,
        ];
        let nk = [-k[0], -k[1], -k[2]];
        if k < nk {
            a = [-a[0], -a[1], -a[2]];
        }
        let key = [
            (a[0] * 1e6).round() as i64,
            (a[1] * 1e6).round() as i64,
            (a[2] * 1e6).round() as i64,
        ];
        let e = by_axis.entry(key).or_insert((0, a));
        e.0 += 1;
    }
    let mut families: Vec<([i64; 3], usize, [f64; 3])> =
        by_axis.into_iter().map(|(k, (n, a))| (k, n, a)).collect();
    // Most faces first; ties break on the axis key (deterministic).
    families.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
    let Some(&(_key, count, axis)) = families.first() else {
        return Err(
            "no recognized cylindrical/conical family on this part — nothing to inform a primitive \
             collider; use convex decomposition (V-HACD) or a hull"
                .into(),
        );
    };
    if count * 2 < faces.len().max(1) {
        return Err(format!(
            "the dominant coaxial family covers only {count} of {} faces — a cylinder would not \
             represent this part; use convex decomposition (V-HACD) or a hull",
            faces.len()
        ));
    }
    // Measured bounding cylinder around the axis through the mesh centroid: r = max radial distance,
    // half-height = half the axial span (bounds every vertex BY CONSTRUCTION — the kernel guarantee).
    let n = mesh.positions.len().max(1);
    let mut c = [0.0f64; 3];
    for p in &mesh.positions {
        c = [c[0] + p[0], c[1] + p[1], c[2] + p[2]];
    }
    #[allow(clippy::cast_precision_loss)]
    let c = [c[0] / n as f64, c[1] / n as f64, c[2] / n as f64];
    let (mut r_max, mut v_min, mut v_max) = (0.0f64, f64::INFINITY, f64::NEG_INFINITY);
    for p in &mesh.positions {
        let d = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
        let v = d[0] * axis[0] + d[1] * axis[1] + d[2] * axis[2];
        let radial = [d[0] - v * axis[0], d[1] - v * axis[1], d[2] - v * axis[2]];
        r_max = r_max
            .max((radial[0] * radial[0] + radial[1] * radial[1] + radial[2] * radial[2]).sqrt());
        v_min = v_min.min(v);
        v_max = v_max.max(v);
    }
    let half_height = 0.5 * (v_max - v_min);
    if !(r_max > 0.0 && half_height > 0.0) {
        return Err("degenerate part extent — no cylinder".into());
    }
    // Fit check: the part must FILL the cylinder well enough that the primitive is better than a hull.
    let mesh_volume = fingerprint(mesh, None).volume.abs();
    let cyl_volume = std::f64::consts::PI * r_max * r_max * (v_max - v_min);
    let fit = mesh_volume / cyl_volume.max(1e-12);
    if fit < CYLINDER_FIT_MIN {
        return Err(format!(
            "the bounding cylinder fits poorly (the part fills {:.0}% of it < the {:.0}% gate) — a \
             cylinder would overclaim collision space; use convex decomposition (V-HACD) or a hull",
            fit * 100.0,
            CYLINDER_FIT_MIN * 100.0
        ));
    }
    // The axial mid-point offset of the cylinder centre from the mesh centroid, along the axis.
    let mid = f64::midpoint(v_min, v_max);
    let centre = [
        c[0] + mid * axis[0],
        c[1] + mid * axis[1],
        c[2] + mid * axis[2],
    ];
    let mut fields: BTreeMap<String, FieldValue> = BTreeMap::new();
    fields.insert("shape".into(), FieldValue::Str("cylinder".into()));
    fields.insert("radius".into(), FieldValue::Number(r_max));
    fields.insert("halfHeight".into(), FieldValue::Number(half_height));
    fields.insert("axX".into(), FieldValue::Number(axis[0]));
    fields.insert("axY".into(), FieldValue::Number(axis[1]));
    fields.insert("axZ".into(), FieldValue::Number(axis[2]));
    fields.insert("cX".into(), FieldValue::Number(centre[0]));
    fields.insert("cY".into(), FieldValue::Number(centre[1]));
    fields.insert("cZ".into(), FieldValue::Number(centre[2]));
    Ok(ColliderPlan {
        shape: "cylinder".into(),
        why: format!(
            "the dominant recognized coaxial family ({count} of {} faces) → a cylinder r={r_max:.3} \
             × h={:.3} along ({:.3}, {:.3}, {:.3}); bounds every vertex by construction; the part \
             fills {:.0}% of it (≥ the {:.0}% gate). Physically stabler + cheaper than a mesh \
             decomposition.",
            faces.len(),
            v_max - v_min,
            axis[0],
            axis[1],
            axis[2],
            fit * 100.0,
            CYLINDER_FIT_MIN * 100.0
        ),
        fields,
    })
}

/// The minimum mesh-volume / cylinder-volume ratio for a cylinder collider to be worth proposing (below
/// it, the primitive overclaims space a hull/decomposition would not).
pub const CYLINDER_FIT_MIN: f64 = 0.5;

/// The `Collider` component ops for an accepted plan (+ a `RigidBody` is the caller's/user's decision —
/// this only authors the collider, the intent path stays `physics_intent`).
#[must_use]
pub fn collider_ops(part: EntityId, plan: &ColliderPlan) -> Vec<Op> {
    plan.fields
        .iter()
        .map(|(field, value)| Op::SetField {
            entity: part,
            component: "Collider".into(),
            field: field.clone(),
            value: value.clone(),
        })
        .collect()
}
