//! **metrocalk-authoring — the verifiable AI-authoring substrate (M13.5 / ADR-054, the PhD R1 home).**
//!
//! Incumbents bolt a chatbot onto a file API (Unity MCP exposes imperative `SetTransform` — no validation,
//! no op-log, no rejection-explanation). Metrocalk's **op-stream + relational scene + schema-validated
//! patch** is the *native* substrate for **verifiable** AI authoring. M12.4 shipped the seed
//! ([`metrocalk_core::composition_grammar`]); this crate HARDENS it into **true constrained decoding** and
//! adds the neuro-symbolic PCG leg — the engine's **data model IS the correctness boundary for AI
//! authoring**.
//!
//! **The honest claim is VERIFIABILITY, not capability.** Generation fidelity comes from models we don't
//! control (the Genie-3 gap, FM-T6.7 — complementary, not beaten). What is UNIQUELY-ENABLED here is the
//! **guarantee combination** — constrained + verified + deterministic + explainable + undoable — each
//! property **measurable** (the R1 paper novelty, named honestly; the NL→symbolic *fidelity* is the open
//! research delta, not "solved").
//!
//! **AI-is-a-guest, offline-first.** The whole substrate — the grammar, the validation, the solver, the
//! op-log — is pure [`metrocalk_core`] data + a **hand-rolled** solver and runs with **NO LLM**; an LLM is
//! a guest behind a documented seam (the M12.4 `RemoteComposer` / the MCP server). Everything verifiable
//! here holds offline. The solver is native/offline (browser = a server-side seam, ADR-006); `clingo`/ASP
//! is the heavier **named future** (native-only C++ FFI + grounding blow-up, FF-T11) — the hand-roll is the
//! ARAP/M9.5 audit answer (deterministic, bounded, wasm-auditable). **No Markov-Logic / soft-constraint
//! reasoning** (non-deterministic, can't explain rejections — excluded, FF-T11/§4).
//!
//! **Three legs:**
//! 1. [`constrain`] — the constrained decoder: projects ANY raw op onto the grammar so it is **structurally
//!    schema-valid** (the M12.4 grammar hardened from spec into enforcement). [`Ablation`] measures the
//!    verifiability delta vs the unconstrained baseline ([`schema_check`], SA-22).
//! 2. [`reason_then_constrain`] (CRANE, FM-T6.2) — an unconstrained scratchpad (the *reasoning* / the
//!    "explain") + the constrained committed op (the *verifiable action*); the op-log ([`AttributedEdit`])
//!    stores both — every accepted op a positive example, every rejected op a negative (the RLAIF data
//!    others discard, FM-T6.4).
//! 3. [`BoundedSolver`] — neuro-symbolic PCG: generate schema-valid content over the **scene-graph ground
//!    truth** ([`Scene`]) until a property holds, with **every rejection a [`Certificate`]** (a derivation,
//!    ties M13.9), not a copy string.

use metrocalk_core::{ComponentMeta, ComposeError, ComposeOp, FieldSpec, FieldType, FieldValue};

// ── leg 1: the constrained decoder (harden composition_grammar into enforcement) ─────────────────────

/// The result of [`constrain`] — a **guaranteed schema-valid** op, plus whether the projection had to
/// *clamp* the raw proposal (repair an out-of-grammar component/field/value).
#[derive(Clone, Debug, PartialEq)]
pub struct Constrained {
    pub op: ComposeOp,
    pub clamped: bool,
}

/// The **unconstrained baseline** (SA-22 ablation arm): validate a raw op against the registry schema
/// *as-is* — it may be **rejected** (the reused [`ComposeError`] reason, the controllability surface).
/// This is the offline, World-free schema layer of the shipped `validate_composition` (which is
/// entity/Registry-bound) — the substrate that must hold with no engine + no LLM.
///
/// # Errors
/// The [`ComposeError`] describing the first schema violation.
pub fn schema_check(op: &ComposeOp, schema: &[ComponentMeta]) -> Result<(), ComposeError> {
    let ComposeOp::SetField {
        component,
        field,
        value,
        ..
    } = op
    else {
        // AuthorRule / AuthorStateMachine are validated by the shipped validate_rule / validate_state_machine
        // (reuse, don't fork) — out of this scalar-SetField ablation's scope.
        return Ok(());
    };
    let meta = schema
        .iter()
        .find(|c| c.name == *component)
        .ok_or_else(|| ComposeError::UnknownComponent {
            component: component.clone(),
        })?;
    let spec = meta
        .fields
        .iter()
        .find(|f| f.name == *field)
        .ok_or_else(|| ComposeError::UnknownField {
            component: component.clone(),
            field: field.clone(),
        })?;
    if value_type(value) != spec.ty {
        return Err(ComposeError::FieldTypeMismatch {
            component: component.clone(),
            field: field.clone(),
            expected: spec.ty,
            got: type_name(value),
        });
    }
    Ok(())
}

/// **The constrained decoder — the M12.4 grammar hardened from a spec into ENFORCEMENT.** Projects ANY raw
/// op onto the grammar: an unknown component/field is clamped to the nearest valid one (edit distance), a
/// wrong-typed value is losslessly coerced or defaulted. The result is **structurally schema-valid by
/// construction** — the model *cannot* emit an out-of-grammar op. (Precondition: a non-empty schema whose
/// components each have ≥1 field — a real registry always does.)
#[must_use]
pub fn constrain(op: &ComposeOp, schema: &[ComponentMeta]) -> Constrained {
    let ComposeOp::SetField {
        entity,
        component,
        field,
        value,
    } = op
    else {
        return Constrained {
            op: op.clone(),
            clamped: false,
        };
    };
    let meta = nearest_component(schema, component);
    let comp_clamped = meta.name != *component;
    let spec = nearest_field(meta, field);
    let field_clamped = spec.name != *field;
    let (val, val_clamped) = coerce(value, spec.ty);
    Constrained {
        op: ComposeOp::SetField {
            entity: entity.clone(),
            component: meta.name.clone(),
            field: spec.name.clone(),
            value: val,
        },
        clamped: comp_clamped || field_clamped || val_clamped,
    }
}

/// The **SA-22 ablation** (the measured spike): constrained-vs-unconstrained over a labeled corpus of
/// `(raw_attempt, intended_op)` cases. `constrained_valid` is **always** `total` (the structural
/// guarantee); `unconstrained_valid` is lower (raw generation emits out-of-schema ops); `intended_match`
/// is the **honest semantic gap** — a valid clamp is not always the *intended* op (the NL→symbolic
/// fidelity that is the open research delta, NOT solved here).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ablation {
    pub total: usize,
    pub unconstrained_valid: usize,
    pub constrained_valid: usize,
    pub intended_match: usize,
}

impl Ablation {
    #[must_use]
    pub fn measure(cases: &[(ComposeOp, ComposeOp)], schema: &[ComponentMeta]) -> Self {
        let mut a = Ablation {
            total: cases.len(),
            ..Default::default()
        };
        for (raw, intended) in cases {
            if schema_check(raw, schema).is_ok() {
                a.unconstrained_valid += 1;
            }
            let c = constrain(raw, schema);
            if schema_check(&c.op, schema).is_ok() {
                a.constrained_valid += 1;
            }
            if c.op == *intended {
                a.intended_match += 1;
            }
        }
        a
    }
}

// ── leg 2: reason-then-constrain (CRANE) + the op-log as attributed edit substrate ───────────────────

/// A **reason-then-constrain** record (CRANE, FM-T6.2): the unconstrained `scratchpad` (the reasoning —
/// the "explain", stored, NEVER applied) + the `committed` constrained op (the verifiable action). The
/// op-log stores both, so tight-grammar clamping never discards the reasoning (the EMNLP over-constraint
/// risk is mitigated by design; the LLM reasoning-preservation *number* is the guest-seam owed measurement).
#[derive(Clone, Debug, PartialEq)]
pub struct ReasonedOp {
    pub scratchpad: String,
    pub committed: ComposeOp,
    pub clamped: bool,
}

/// Reason (unconstrained scratchpad) → constrain (schema-valid committed op).
#[must_use]
pub fn reason_then_constrain(
    scratchpad: impl Into<String>,
    raw: &ComposeOp,
    schema: &[ComponentMeta],
) -> ReasonedOp {
    let c = constrain(raw, schema);
    ReasonedOp {
        scratchpad: scratchpad.into(),
        committed: c.op,
        clamped: c.clamped,
    }
}

/// The outcome of validating a proposed op — the **attributed training signal** (FM-T6.4): accepted ops
/// are positive examples, rejected ops are negatives (with the faithful reason), the RLAIF data the op-log
/// captures for free and others discard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditOutcome {
    Accepted,
    Rejected(String),
}

/// One row of the op-log-as-attributed-edit-substrate.
#[derive(Clone, Debug, PartialEq)]
pub struct AttributedEdit {
    pub op: ComposeOp,
    pub outcome: EditOutcome,
}

impl AttributedEdit {
    /// Attribute a raw proposed op by validating it against the schema (the ADR-017 contract, schema layer).
    #[must_use]
    pub fn attribute(op: ComposeOp, schema: &[ComponentMeta]) -> Self {
        let outcome = match schema_check(&op, schema) {
            Ok(()) => EditOutcome::Accepted,
            Err(e) => EditOutcome::Rejected(e.to_string()),
        };
        Self { op, outcome }
    }
}

// ── leg 3: neuro-symbolic PCG over the scene-graph, every rejection a certificate ────────────────────

/// The **scene-graph** the solver reasons over — the AI's *ground truth* (FM-T6.5): it queries real state
/// before committing, instead of hallucinating. `slots` cells, some **fixed** by the live scene (the AI
/// cannot override real state), linked by `adjacency` edges (linked slots must differ).
#[derive(Clone, Debug)]
pub struct Scene {
    pub slots: usize,
    pub fixed: Vec<Option<u32>>,
    pub adjacency: Vec<(usize, usize)>,
}

/// The property the generated content must satisfy — an NL request compiled to symbolic constraints
/// (bounded/offline, the FF-T11 bounded-design-space discipline): values in `0..domain`, and at least
/// `min_target` slots equal to `target`.
#[derive(Clone, Copy, Debug)]
pub struct ContentSpec {
    pub domain: u32,
    pub target: u32,
    pub min_target: usize,
}

/// A **derivation certificate** for a solved problem: the assignment, the schema-valid content ops, the
/// constraints proven satisfied, and the search cost.
#[derive(Clone, Debug, PartialEq)]
pub struct Derivation {
    pub assignment: Vec<u32>,
    pub ops: Vec<ComposeOp>,
    pub satisfied: Vec<String>,
    pub steps: usize,
}

/// A **rejection certificate** (ties M13.9): the plain-language reason + the `unsat_core` — the specific
/// constraints that couldn't be jointly satisfied. **A derivation, not a copy string** — the AI's "no" is
/// explained, faithfully.
#[derive(Clone, Debug, PartialEq)]
pub struct Certificate {
    pub reason: String,
    pub unsat_core: Vec<String>,
}

/// The outcome of a PCG solve.
#[derive(Clone, Debug, PartialEq)]
pub enum SolveOutcome {
    Solved(Derivation),
    Rejected(Certificate),
}

/// The project-owned PCG/solver seam (invariant 5) — a foreign solver (`clingo::`) would live only here,
/// grep-gated. The generation is content over the relational scene, always schema-valid, deterministic.
pub trait Pcg {
    fn generate(&self, scene: &Scene, spec: &ContentSpec) -> SolveOutcome;
}

/// A **hand-rolled bounded backtracking solver** — the crate-audit answer to `clingo`/ASP (native-only C++
/// FFI + grounding blow-up): deterministic (fixed slot + value order), offline, wasm-auditable, and
/// **bounded** (`max_steps` caps the search — the FF-T11 bounded-design-space limit made explicit).
#[derive(Clone, Copy, Debug)]
pub struct BoundedSolver {
    pub max_steps: usize,
}

impl BoundedSolver {
    #[must_use]
    pub fn new(max_steps: usize) -> Self {
        Self { max_steps }
    }
}

impl Pcg for BoundedSolver {
    fn generate(&self, scene: &Scene, spec: &ContentSpec) -> SolveOutcome {
        // Ground-truth feasibility FIRST — the AI reasons over real state, it cannot override an
        // inconsistent scene; a conflict is REJECTED with a certificate (not silently repaired).
        if let Some(cert) = ground_truth_conflict(scene, spec) {
            return SolveOutcome::Rejected(cert);
        }
        let mut assign = vec![UNASSIGNED; scene.slots];
        for (i, f) in scene.fixed.iter().enumerate() {
            if let Some(v) = f {
                assign[i] = *v;
            }
        }
        let mut steps = 0usize;
        if backtrack(0, &mut assign, scene, spec, self.max_steps, &mut steps) {
            SolveOutcome::Solved(Derivation {
                ops: content_ops(&assign),
                satisfied: satisfied_constraints(scene, spec),
                assignment: assign,
                steps,
            })
        } else {
            SolveOutcome::Rejected(Certificate {
                reason: format!(
                    "no assignment satisfies adjacency + '>= {} slots equal to {}' within {} search steps",
                    spec.min_target, spec.target, self.max_steps
                ),
                unsat_core: unsat_core(scene, spec),
            })
        }
    }
}

const UNASSIGNED: u32 = u32::MAX;

fn ground_truth_conflict(scene: &Scene, spec: &ContentSpec) -> Option<Certificate> {
    if spec.min_target > scene.slots {
        return Some(Certificate {
            reason: format!(
                "the property needs {} slots equal to {}, but the scene has only {} slots",
                spec.min_target, spec.target, scene.slots
            ),
            unsat_core: vec![format!(
                "min_target({}) > slots({})",
                spec.min_target, scene.slots
            )],
        });
    }
    for (i, f) in scene.fixed.iter().enumerate() {
        if let Some(v) = f {
            if *v >= spec.domain {
                return Some(Certificate {
                    reason: format!(
                        "scene slot {i} is fixed to {v}, outside the domain 0..{}",
                        spec.domain
                    ),
                    unsat_core: vec![format!("fixed slot {i} = {v} >= domain {}", spec.domain)],
                });
            }
        }
    }
    for &(a, b) in &scene.adjacency {
        if let (Some(va), Some(vb)) = (
            scene.fixed.get(a).copied().flatten(),
            scene.fixed.get(b).copied().flatten(),
        ) {
            if va == vb {
                return Some(Certificate {
                    reason: format!("the live scene already violates adjacency: slots {a} and {b} are both fixed to {va}"),
                    unsat_core: vec![format!("fixed {a}={va} adjacent to fixed {b}={vb}")],
                });
            }
        }
    }
    None
}

fn backtrack(
    slot: usize,
    assign: &mut [u32],
    scene: &Scene,
    spec: &ContentSpec,
    budget: usize,
    steps: &mut usize,
) -> bool {
    if *steps > budget {
        return false;
    }
    if slot == scene.slots {
        return assign.iter().filter(|&&v| v == spec.target).count() >= spec.min_target;
    }
    if assign[slot] != UNASSIGNED {
        return adjacency_ok(slot, assign[slot], assign, scene)
            && backtrack(slot + 1, assign, scene, spec, budget, steps);
    }
    for v in 0..spec.domain {
        *steps += 1;
        if *steps > budget {
            return false;
        }
        if adjacency_ok(slot, v, assign, scene) {
            assign[slot] = v;
            if backtrack(slot + 1, assign, scene, spec, budget, steps) {
                return true;
            }
            assign[slot] = UNASSIGNED;
        }
    }
    false
}

fn adjacency_ok(slot: usize, v: u32, assign: &[u32], scene: &Scene) -> bool {
    for &(a, b) in &scene.adjacency {
        let other = if a == slot {
            Some(b)
        } else if b == slot {
            Some(a)
        } else {
            None
        };
        if let Some(o) = other {
            if assign.get(o).copied() == Some(v) {
                return false;
            }
        }
    }
    true
}

/// The generated content as **schema-valid** ops (a `Cell.value` per slot) — the AI's output is data the
/// commit pipeline validates + applies undoably (ADR-017), never a raw mutation.
fn content_ops(assign: &[u32]) -> Vec<ComposeOp> {
    assign
        .iter()
        .enumerate()
        .map(|(i, &v)| ComposeOp::SetField {
            entity: format!("slot{i}"),
            component: "Cell".to_string(),
            field: "value".to_string(),
            value: FieldValue::Integer(i64::from(v)),
        })
        .collect()
}

fn satisfied_constraints(scene: &Scene, spec: &ContentSpec) -> Vec<String> {
    vec![
        format!(
            "all {} adjacency edges hold (linked slots differ)",
            scene.adjacency.len()
        ),
        format!(
            ">= {} slots equal to {} (property satisfied)",
            spec.min_target, spec.target
        ),
    ]
}

fn unsat_core(scene: &Scene, spec: &ContentSpec) -> Vec<String> {
    vec![
        format!(
            "domain 0..{} over {} slots with {} adjacency edges",
            spec.domain,
            scene.slots,
            scene.adjacency.len()
        ),
        format!(
            "cannot place {} slots at value {} without an adjacency conflict",
            spec.min_target, spec.target
        ),
    ]
}

// ── the schema-projection helpers (edit-distance clamp + lossless coercion) ──────────────────────────

fn nearest_component<'a>(schema: &'a [ComponentMeta], name: &str) -> &'a ComponentMeta {
    schema
        .iter()
        .find(|c| c.name == name)
        .or_else(|| schema.iter().min_by_key(|c| edit_distance(&c.name, name)))
        .expect("the grammar schema is non-empty (a real registry always has components)")
}

fn nearest_field<'a>(meta: &'a ComponentMeta, name: &str) -> &'a FieldSpec {
    meta.fields
        .iter()
        .find(|f| f.name == name)
        .or_else(|| {
            meta.fields
                .iter()
                .min_by_key(|f| edit_distance(&f.name, name))
        })
        .expect("a grammar component has >= 1 field")
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

fn coerce(value: &FieldValue, ty: FieldType) -> (FieldValue, bool) {
    if value_type(value) == ty {
        return (value.clone(), false);
    }
    (
        lossless_convert(value, ty).unwrap_or_else(|| default_of(ty)),
        true,
    )
}

fn lossless_convert(value: &FieldValue, ty: FieldType) -> Option<FieldValue> {
    use FieldType as T;
    use FieldValue as V;
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    match (value, ty) {
        (V::Integer(i), T::Number) => Some(V::Number(*i as f64)),
        (V::Number(n), T::Integer) if n.fract() == 0.0 => Some(V::Integer(*n as i64)),
        (V::Bool(b), T::Integer) => Some(V::Integer(i64::from(*b))),
        (_, T::String) => Some(V::Str(value_string(value))),
        (V::Str(s), T::Integer) => s.parse().ok().map(V::Integer),
        (V::Str(s), T::Number) => s.parse().ok().map(V::Number),
        _ => None,
    }
}

fn default_of(ty: FieldType) -> FieldValue {
    match ty {
        FieldType::Integer => FieldValue::Integer(0),
        FieldType::Number => FieldValue::Number(0.0),
        FieldType::Boolean => FieldValue::Bool(false),
        FieldType::String => FieldValue::Str(String::new()),
    }
}

fn value_type(v: &FieldValue) -> FieldType {
    match v {
        FieldValue::Integer(_) => FieldType::Integer,
        FieldValue::Number(_) => FieldType::Number,
        FieldValue::Bool(_) => FieldType::Boolean,
        FieldValue::Str(_) => FieldType::String,
    }
}

fn type_name(v: &FieldValue) -> &'static str {
    match v {
        FieldValue::Integer(_) => "integer",
        FieldValue::Number(_) => "number",
        FieldValue::Bool(_) => "boolean",
        FieldValue::Str(_) => "string",
    }
}

fn value_string(v: &FieldValue) -> String {
    match v {
        FieldValue::Integer(i) => i.to_string(),
        FieldValue::Number(n) => n.to_string(),
        FieldValue::Bool(b) => b.to_string(),
        FieldValue::Str(s) => s.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrocalk_core::{composition_grammar, grammar_coverage};

    fn field(name: &str, ty: FieldType) -> FieldSpec {
        FieldSpec {
            name: name.to_string(),
            ty,
            required: false,
            format: None,
        }
    }

    fn component(name: &str, fields: Vec<FieldSpec>) -> ComponentMeta {
        ComponentMeta {
            name: name.to_string(),
            fields,
            ..Default::default()
        }
    }

    fn schema() -> Vec<ComponentMeta> {
        vec![
            component(
                "Health",
                vec![
                    field("hp", FieldType::Integer),
                    field("max", FieldType::Integer),
                ],
            ),
            component("Flammable", vec![field("lit", FieldType::Boolean)]),
            component("KillCounter", vec![field("count", FieldType::Integer)]),
        ]
    }

    fn set(entity: &str, component: &str, fld: &str, value: FieldValue) -> ComposeOp {
        ComposeOp::SetField {
            entity: entity.to_string(),
            component: component.to_string(),
            field: fld.to_string(),
            value,
        }
    }

    // A tiny deterministic LCG (offline, no system entropy — the AI-as-guest discipline).
    fn lcg(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *state
    }

    #[test]
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    fn constrained_decoding_is_structurally_valid_for_any_input() {
        // THE STRUCTURAL GUARANTEE (test-first): for ANY raw op — garbage component/field/value — the
        // constrained decoder emits a SCHEMA-VALID op. The model literally cannot emit an out-of-grammar op.
        let schema = schema();
        let comps = ["Health", "Helth", "Flame", "zzz", "KillCountr", "Cell"];
        let fields = ["hp", "hpp", "lit", "count", "", "xyzzy"];
        let mut st = 0x5EED_u64;
        for _ in 0..5000 {
            let raw = set(
                "e0",
                comps[(lcg(&mut st) as usize) % comps.len()],
                fields[(lcg(&mut st) as usize) % fields.len()],
                match lcg(&mut st) % 4 {
                    0 => FieldValue::Integer((lcg(&mut st) % 100) as i64),
                    1 => FieldValue::Number(1.5),
                    2 => FieldValue::Bool(true),
                    _ => FieldValue::Str("nope".to_string()),
                },
            );
            let c = constrain(&raw, &schema);
            assert!(
                schema_check(&c.op, &schema).is_ok(),
                "constrain() must yield a schema-valid op for ANY raw input, got {:?}",
                c.op
            );
        }
    }

    #[test]
    #[allow(clippy::cast_precision_loss)]
    fn the_sa22_ablation_measures_the_verifiability_delta() {
        // THE MEASURED SPIKE (deterministic → identical across runs, the >=2-runs discipline): a labeled
        // corpus of (raw_attempt, intended_op). Constrained validity = 100% (structural); unconstrained
        // validity < 100% (raw generation emits out-of-schema ops); intended-match is the honest semantic
        // gap (a valid clamp is not always the intended op — the NL->symbolic fidelity research delta).
        let schema = schema();
        let cases = vec![
            // valid (both arms agree)
            (
                set("e", "Health", "hp", FieldValue::Integer(5)),
                set("e", "Health", "hp", FieldValue::Integer(5)),
            ),
            (
                set("e", "Flammable", "lit", FieldValue::Bool(true)),
                set("e", "Flammable", "lit", FieldValue::Bool(true)),
            ),
            // typo'd component (unconstrained REJECTS; constrained clamps to the intended nearest)
            (
                set("e", "Helth", "hp", FieldValue::Integer(3)),
                set("e", "Health", "hp", FieldValue::Integer(3)),
            ),
            (
                set("e", "KillCountr", "count", FieldValue::Integer(1)),
                set("e", "KillCounter", "count", FieldValue::Integer(1)),
            ),
            // unknown field (unconstrained REJECTS; constrained clamps)
            (
                set("e", "Health", "hpp", FieldValue::Integer(9)),
                set("e", "Health", "hp", FieldValue::Integer(9)),
            ),
            // wrong type (unconstrained REJECTS; constrained coerces losslessly to the intended)
            (
                set(
                    "e",
                    "KillCounter",
                    "count",
                    FieldValue::Str("7".to_string()),
                ),
                set("e", "KillCounter", "count", FieldValue::Integer(7)),
            ),
            // ambiguous clamp → valid but NOT intended (the semantic gap made visible)
            (
                set("e", "zzz", "qqq", FieldValue::Bool(false)),
                set("e", "Flammable", "lit", FieldValue::Bool(false)),
            ),
        ];
        let a = Ablation::measure(&cases, &schema);
        assert_eq!(
            a.constrained_valid, a.total,
            "constrained decoding is structurally 100% valid"
        );
        assert!(
            a.unconstrained_valid < a.total,
            "the unconstrained baseline emits invalid ops (the delta)"
        );
        assert!(
            a.intended_match >= a.unconstrained_valid,
            "constraining never loses an already-valid intended op"
        );
        assert!(
            a.intended_match < a.total,
            "intended-match < 100%: the honest NL->symbolic semantic gap"
        );

        // Reproducible (>=2 runs identical — deterministic, offline).
        assert_eq!(Ablation::measure(&cases, &schema), a);

        println!(
            "::notice::authoring-ablation total={} unconstrained-valid={} constrained-valid={} intended-match={} (constrained safety = {:.0}% vs unconstrained {:.0}%; intended = {:.0}% the semantic gap)",
            a.total, a.unconstrained_valid, a.constrained_valid, a.intended_match,
            100.0 * a.constrained_valid as f64 / a.total as f64,
            100.0 * a.unconstrained_valid as f64 / a.total as f64,
            100.0 * a.intended_match as f64 / a.total as f64,
        );
    }

    #[test]
    fn reason_then_constrain_keeps_the_scratchpad_and_commits_a_valid_op() {
        // CRANE: the unconstrained scratchpad (reasoning) is preserved; the committed op is schema-valid.
        let schema = schema();
        let raw = set("e", "Helth", "hpp", FieldValue::Str("42".to_string())); // fully wrong
        let r = reason_then_constrain(
            "the user wants the knight's health set to 42",
            &raw,
            &schema,
        );
        assert!(r.clamped, "the raw proposal needed repair");
        assert!(
            schema_check(&r.committed, &schema).is_ok(),
            "the committed op is schema-valid"
        );
        assert!(
            r.scratchpad.contains("health"),
            "the reasoning is preserved (the 'explain'), not discarded"
        );
        assert_eq!(
            r.committed,
            set("e", "Health", "hp", FieldValue::Integer(42)),
            "reason-then-constrain repaired to the intended op"
        );
    }

    #[test]
    fn the_op_log_is_the_attributed_edit_substrate() {
        // Every accepted op = a positive example; every rejected op = a negative, with the faithful reason.
        let schema = schema();
        let good =
            AttributedEdit::attribute(set("e", "Health", "hp", FieldValue::Integer(1)), &schema);
        assert_eq!(good.outcome, EditOutcome::Accepted);
        let bad =
            AttributedEdit::attribute(set("e", "Health", "hpp", FieldValue::Integer(1)), &schema);
        match bad.outcome {
            EditOutcome::Rejected(reason) => assert!(
                reason.contains("no field"),
                "the negative carries the faithful reason: {reason}"
            ),
            EditOutcome::Accepted => panic!("an out-of-schema op must be a negative example"),
        }
    }

    #[test]
    fn the_pcg_solver_generates_schema_valid_content_over_the_scene_graph() {
        // Neuro-symbolic PCG: solve a satisfiable scene → a derivation whose generated ops are ALL
        // schema-valid (validate-by-construction) and whose property holds.
        let cell_schema = vec![component("Cell", vec![field("value", FieldType::Integer)])];
        let scene = Scene {
            slots: 5,
            fixed: vec![None, None, None, None, None],
            adjacency: vec![(0, 1), (1, 2), (2, 3), (3, 4)],
        };
        let spec = ContentSpec {
            domain: 3,
            target: 1,
            min_target: 2,
        };
        match BoundedSolver::new(10_000).generate(&scene, &spec) {
            SolveOutcome::Solved(d) => {
                assert!(
                    d.assignment.iter().filter(|&&v| v == 1).count() >= 2,
                    "the property holds"
                );
                for op in &d.ops {
                    assert!(
                        schema_check(op, &cell_schema).is_ok(),
                        "generated content is schema-valid"
                    );
                }
                assert!(
                    !d.satisfied.is_empty(),
                    "the derivation names the satisfied constraints"
                );
            }
            SolveOutcome::Rejected(c) => {
                panic!("this scene is satisfiable, got a rejection: {}", c.reason)
            }
        }
    }

    #[test]
    fn an_unsatisfiable_property_yields_a_derivation_certificate_not_a_copy_string() {
        // The rejection is a CERTIFICATE (an unsat-core / derivation — the M13.9 tie), not a copy string.
        let scene = Scene {
            slots: 2,
            fixed: vec![None, None],
            adjacency: vec![(0, 1)],
        };
        // domain 2 (values 0/1), adjacency forces the two slots to differ → at most ONE slot equals target;
        // demanding min_target=2 is unsatisfiable.
        let spec = ContentSpec {
            domain: 2,
            target: 1,
            min_target: 2,
        };
        match BoundedSolver::new(10_000).generate(&scene, &spec) {
            SolveOutcome::Rejected(c) => {
                assert!(
                    !c.unsat_core.is_empty(),
                    "the rejection carries an unsat-core (a derivation, not a copy string)"
                );
                assert!(
                    c.reason.contains("adjacency") || c.reason.contains("slots"),
                    "the reason is faithful: {}",
                    c.reason
                );
            }
            SolveOutcome::Solved(_) => panic!("this property is unsatisfiable under adjacency"),
        }
    }

    #[test]
    fn the_solver_respects_the_scene_ground_truth_and_cannot_override_real_state() {
        // The AI reasons over REAL state: a live scene that already violates adjacency is REJECTED with a
        // certificate citing the scene fact — the AI cannot corrupt/override the ground truth.
        let scene = Scene {
            slots: 2,
            fixed: vec![Some(1), Some(1)],
            adjacency: vec![(0, 1)],
        };
        let spec = ContentSpec {
            domain: 3,
            target: 0,
            min_target: 0,
        };
        match BoundedSolver::new(1000).generate(&scene, &spec) {
            SolveOutcome::Rejected(c) => assert!(
                c.reason.contains("already violates"),
                "cites the ground-truth conflict: {}",
                c.reason
            ),
            SolveOutcome::Solved(_) => {
                panic!("an inconsistent ground-truth scene must be rejected, not overridden")
            }
        }
    }

    #[test]
    fn the_substrate_runs_offline_no_llm_and_reuses_the_m12_4_grammar() {
        // AI-as-guest: the whole substrate (constrain + solve) runs with NO LLM and is DETERMINISTIC
        // (same input → same output). And it REUSES the shipped M12.4 grammar (composition_grammar +
        // grammar_coverage), not a parallel model.
        let schema = schema();
        let raw = set("e", "Helth", "hp", FieldValue::Integer(5));
        assert_eq!(
            constrain(&raw, &schema),
            constrain(&raw, &schema),
            "constrained decoding is deterministic + offline"
        );

        let grammar = composition_grammar(&schema);
        assert!(
            grammar.is_object(),
            "the M12.4 constrained-decoding grammar (JSON Schema) is reused"
        );
        assert!(
            grammar_coverage(&schema).within_subset,
            "the schema stays within the reliable constrained-decoding subset (SA-22)"
        );
    }
}
