//! M15.2 (ADR-072) — **parametric feature-history as the op-stream** + a deterministic, bisectable rebuild.
//!
//! SOLIDWORKS' own value, in its bloggers' words, is that *"designs are not just shapes; they are sequences
//! of decisions"* — the **feature-history tree** that captures design intent and cascades a parameter change
//! through dependent geometry. A CAD vendor **bolts that on beside the kernel**; in Metrocalk **the op-log IS
//! the feature tree** — the [`FeatureOp`] stream is the *primary* source of truth and the geometry is the
//! **derived view**. This module surfaces that stream as a CAD feature tree and makes **rebuild = a
//! deterministic replay** (the M13.1/ADR-050 property applied to feature ops, replayed through the existing
//! [`Engine`] commit pipeline — *not* a forked rebuild engine):
//!
//! - **Rebuild** ([`rebuild`]) replays the feature ops into a fresh engine, deriving the geometry with the
//!   shipped **M13.2 exact-predicate CSG** (ADR-051 — kernel-free) and hashing the canonical logical state
//!   ([`Engine::canonical_state`], ADR-071). [`rebuild_reproduces`] is the ≥2-runs bit-identical gate.
//! - **Bisect** = suppress an upstream feature and replay: a downstream feature that lost its dependency
//!   fails with an **explained** [`FeatureError::BrokenDependency`] (which feature, which dependency, why) —
//!   *not* a silent rebuild error. A failed rebuild ships as a **lossless bincode artifact**
//!   ([`FeatureHistory::to_bytes`]) — "a broken feature is a file" that re-runs the exact failure headlessly.
//! - **Cascade** = edit a parameter (or a variable an [`Expr`] equation derives) and replay — dependent
//!   features recompute deterministically.
//! - **Equations / configurations** are **registry-typed, structured** ops (a typed [`Expr`] AST + named
//!   [`Configuration`] variants), **never a free-text expression DSL** that bypasses the pipeline; a circular
//!   or out-of-domain equation is **Blocked + explained** (ADR-016).
//!
//! **Honest scope — the topological-naming problem is REFRAMED, not solved.** The op-stream gives a
//! **reproducible, bisectable, explained** failure — a genuine improvement — but re-identifying a face/edge
//! after an upstream edit (Kripac 1997; unsolved in OpenCascade/FreeCAD for years) is owned by the geometry
//! kernel's **stable-ID scheme**, gated on the M15.0 geometry decision (a **named future**). The history
//! machinery here is kernel-free; editing *trimmed-NURBS B-rep faces* parametrically is not. Rebuild is
//! native-deterministic (the wasm32 boundary, ADR-020, applies → server-authoritative on the web).

use metrocalk_assets::AssetId;
use metrocalk_core::{Engine, EntityId, FieldValue, Op, PipelineError};
use metrocalk_csg::{box_mesh, validate, Csg, ExactBspCsg, TriMesh};
use metrocalk_ecs::FlecsWorld;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A stable feature-tree node id — the feature's identity in the op-log (peer-stable, the React-Flow node id
/// / the bisect key). Edits/merges key off this, never a position.
pub type FeatureId = u32;

/// A **dimension** — a literal, or a reference to a named global variable (so an equation can drive it). The
/// typed, structured alternative to a free-text expression in a value slot (the AVOID-a-DSL discipline).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Dim {
    /// A literal length.
    Lit(f64),
    /// A reference to a global variable (resolved from the [`FeatureHistory::variables`] equations).
    Ref(String),
}

/// A **typed equation AST** for a global variable — `Const`/`Var`/`Add`/`Sub`/`Mul`/`Div`. This is a
/// *structured*, registry-typed op (the M12.1/M12.4 discipline), **not** an `evalexpr`/`rhai` free-text DSL
/// that would bypass the commit pipeline — so a circular or divide-by-zero equation is caught + explained,
/// never silently evaluated.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    /// A constant.
    Const(f64),
    /// A reference to another variable (the edge of the equation dependency graph).
    Var(String),
    /// Sum.
    Add(Box<Expr>, Box<Expr>),
    /// Difference.
    Sub(Box<Expr>, Box<Expr>),
    /// Product.
    Mul(Box<Expr>, Box<Expr>),
    /// Quotient (a zero divisor is Blocked + explained).
    Div(Box<Expr>, Box<Expr>),
}

/// One **feature** — a typed op in the parametric tree. Each carries a stable [`FeatureId`] and its explicit
/// dependency edges (a `Carve` depends on its target + tool; a `Pattern` on its source). The geometry is the
/// **shipped exact-CSG primitives** (M13.2), so this is kernel-free.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum FeatureOp {
    /// A box primitive solid at `pos` with half-extents `half` (each a [`Dim`], so a variable can drive it).
    Box {
        /// This feature's id.
        id: FeatureId,
        /// Center position.
        pos: [f64; 3],
        /// Half-extents (parametric).
        half: [Dim; 3],
    },
    /// **Carve** `target` by `tool` (an exact-predicate CSG difference) — the body `target` is cut in place;
    /// depends on both `target` and `tool`.
    Carve {
        /// This feature's id.
        id: FeatureId,
        /// The body being cut (a prior feature's output).
        target: FeatureId,
        /// The cutting tool (a prior feature's output).
        tool: FeatureId,
    },
    /// **Linear pattern**: `count` copies of `source` spaced by `spacing`; depends on `source`.
    Pattern {
        /// This feature's id.
        id: FeatureId,
        /// The body to pattern.
        source: FeatureId,
        /// How many instances (the seeded copy keeps the source; `count` total).
        count: u32,
        /// The per-step offset.
        spacing: [f64; 3],
    },
}

impl FeatureOp {
    /// This feature's stable id.
    #[must_use]
    pub fn id(&self) -> FeatureId {
        match self {
            FeatureOp::Box { id, .. }
            | FeatureOp::Carve { id, .. }
            | FeatureOp::Pattern { id, .. } => *id,
        }
    }

    /// The upstream features this one depends on (the dependency edges of the feature tree).
    #[must_use]
    pub fn deps(&self) -> Vec<FeatureId> {
        match self {
            FeatureOp::Box { .. } => vec![],
            FeatureOp::Carve { target, tool, .. } => vec![*target, *tool],
            FeatureOp::Pattern { source, .. } => vec![*source],
        }
    }

    /// A short kind tag (for the feature-tree surface / explanations).
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            FeatureOp::Box { .. } => "Box",
            FeatureOp::Carve { .. } => "Carve",
            FeatureOp::Pattern { .. } => "Pattern",
        }
    }
}

/// A **feature history** — the feature tree *as the op-log*: a typed feature-op stream, the named global
/// variables (equations) the dimensions reference, and the suppressed set (the bisect / CAD "suppress").
/// This is the primary source of truth; geometry is the derived [`rebuild`] view. Serializes **losslessly**
/// (bincode) so a (broken) history is a reproducible file.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct FeatureHistory {
    /// Named global variables / equations (the typed-equation layer). Order-independent; cycle-checked.
    pub variables: BTreeMap<String, Expr>,
    /// The feature ops, in dependency order (a feature may only depend on earlier ones).
    pub features: Vec<FeatureOp>,
    /// Suppressed feature ids — the bisect lever (CAD "suppress a feature"): a suppressed feature is skipped
    /// on rebuild, surfacing any downstream feature that lost its dependency.
    pub suppressed: BTreeSet<FeatureId>,
}

/// A **named configuration** — a set of variable overrides (one model → many variants). Switching a
/// configuration is a deterministic replay to that variant's evaluated dimensions (the "design table" idea,
/// as typed ops).
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Configuration {
    /// The configuration name.
    pub name: String,
    /// Variable value overrides applied before evaluation.
    pub overrides: BTreeMap<String, f64>,
}

/// Why a rebuild was Blocked — every variant a **plain-language, ASCII-safe** explanation (ADR-016): a
/// reproducible, bisectable, *explained* failure, never a silent bad rebuild.
#[derive(Clone, Debug, PartialEq)]
pub enum FeatureError {
    /// Two features share an id (the tree is malformed).
    DuplicateFeatureId(FeatureId),
    /// A feature depends on an id that does not exist in the history.
    UnknownDependency {
        /// The depending feature.
        feature: FeatureId,
        /// The missing dependency id.
        dep: FeatureId,
    },
    /// A feature depends on a feature that is **suppressed** (the bisect result) — the explained downstream
    /// break: which feature lost which dependency, and why.
    BrokenDependency {
        /// The downstream feature that broke.
        feature: FeatureId,
        /// The (suppressed) upstream feature it depended on.
        missing: FeatureId,
        /// The plain-language reason.
        why: String,
    },
    /// A dimension references an undefined variable.
    UnknownVariable {
        /// The variable name.
        name: String,
        /// The feature that referenced it (if any).
        feature: Option<FeatureId>,
    },
    /// An equation forms a cycle (the variable dependency graph is circular).
    CircularEquation {
        /// The cycle path (names), the breaking variable last.
        cycle: Vec<String>,
    },
    /// An equation divides by zero (or is otherwise out of domain).
    InvalidEquation {
        /// The variable being evaluated.
        name: String,
        /// The plain-language reason.
        why: String,
    },
    /// A CSG operation produced a degenerate / non-watertight result (the exact-arithmetic tail, M13.2).
    DegenerateGeometry {
        /// The feature that produced it.
        feature: FeatureId,
        /// The validator's explanation.
        why: String,
    },
    /// The underlying engine/op-log rejected a commit.
    Pipeline(String),
}

impl std::fmt::Display for FeatureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FeatureError::DuplicateFeatureId(id) => write!(f, "two features share id {id}"),
            FeatureError::UnknownDependency { feature, dep } => {
                write!(
                    f,
                    "feature {feature} depends on feature {dep}, which does not exist"
                )
            }
            FeatureError::BrokenDependency {
                feature,
                missing,
                why,
            } => write!(
                f,
                "feature {feature} cannot rebuild: it depends on feature {missing}, {why}"
            ),
            FeatureError::UnknownVariable { name, feature } => match feature {
                Some(id) => write!(f, "feature {id} references undefined variable '{name}'"),
                None => write!(f, "undefined variable '{name}'"),
            },
            FeatureError::CircularEquation { cycle } => {
                write!(
                    f,
                    "circular equation: {} (a variable cannot depend on itself)",
                    cycle.join(" -> ")
                )
            }
            FeatureError::InvalidEquation { name, why } => {
                write!(f, "equation for '{name}' is invalid: {why}")
            }
            FeatureError::DegenerateGeometry { feature, why } => {
                write!(f, "feature {feature} produced degenerate geometry: {why}")
            }
            FeatureError::Pipeline(e) => write!(f, "engine rejected a feature commit: {e}"),
        }
    }
}

impl std::error::Error for FeatureError {}

impl From<PipelineError> for FeatureError {
    fn from(e: PipelineError) -> Self {
        FeatureError::Pipeline(e.to_string())
    }
}

/// The result of a successful rebuild — the deterministic identity of the derived scene + per-feature
/// geometry handles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rebuilt {
    /// The content-addressed identity of the rebuilt canonical logical state (the rebuild hash; ADR-071).
    pub state_hash: String,
    /// The number of features that were built (suppressed ones excluded).
    pub built_features: usize,
    /// Per-feature derived geometry handle (the content-addressed mesh — the CSG `content_hash`).
    pub geometry: BTreeMap<FeatureId, String>,
}

// ── equation evaluation (typed, cycle-checked) ───────────────────────────────────────────────────────────

/// Evaluate all global variables under an optional configuration's overrides. Topological with **cycle
/// detection**: a circular equation is [`FeatureError::CircularEquation`], a divide-by-zero is
/// [`FeatureError::InvalidEquation`] — Blocked + explained, never silently evaluated.
///
/// # Errors
/// See [`FeatureError`] (circular / invalid / unknown variable).
pub fn eval_variables(
    variables: &BTreeMap<String, Expr>,
    overrides: Option<&BTreeMap<String, f64>>,
) -> Result<BTreeMap<String, f64>, FeatureError> {
    let mut resolved: BTreeMap<String, f64> = BTreeMap::new();
    let mut visiting: Vec<String> = Vec::new();
    for name in variables.keys() {
        resolve_var(name, variables, overrides, &mut resolved, &mut visiting)?;
    }
    // An override for a variable that has no equation is still a usable value.
    if let Some(o) = overrides {
        for (k, v) in o {
            resolved.entry(k.clone()).or_insert(*v);
        }
    }
    Ok(resolved)
}

fn resolve_var(
    name: &str,
    variables: &BTreeMap<String, Expr>,
    overrides: Option<&BTreeMap<String, f64>>,
    resolved: &mut BTreeMap<String, f64>,
    visiting: &mut Vec<String>,
) -> Result<f64, FeatureError> {
    if let Some(v) = resolved.get(name) {
        return Ok(*v);
    }
    // A configuration override pins the variable, short-circuiting its equation (the design-table switch).
    if let Some(v) = overrides.and_then(|o| o.get(name)) {
        resolved.insert(name.to_string(), *v);
        return Ok(*v);
    }
    if visiting.iter().any(|n| n == name) {
        let mut cycle = visiting.clone();
        cycle.push(name.to_string());
        return Err(FeatureError::CircularEquation { cycle });
    }
    let expr = variables
        .get(name)
        .ok_or_else(|| FeatureError::UnknownVariable {
            name: name.to_string(),
            feature: None,
        })?;
    visiting.push(name.to_string());
    let val = eval_expr(expr, name, variables, overrides, resolved, visiting)?;
    visiting.pop();
    resolved.insert(name.to_string(), val);
    Ok(val)
}

fn eval_expr(
    expr: &Expr,
    owner: &str,
    variables: &BTreeMap<String, Expr>,
    overrides: Option<&BTreeMap<String, f64>>,
    resolved: &mut BTreeMap<String, f64>,
    visiting: &mut Vec<String>,
) -> Result<f64, FeatureError> {
    match expr {
        Expr::Const(c) => Ok(*c),
        Expr::Var(n) => resolve_var(n, variables, overrides, resolved, visiting),
        Expr::Add(a, b) => Ok(
            eval_expr(a, owner, variables, overrides, resolved, visiting)?
                + eval_expr(b, owner, variables, overrides, resolved, visiting)?,
        ),
        Expr::Sub(a, b) => Ok(
            eval_expr(a, owner, variables, overrides, resolved, visiting)?
                - eval_expr(b, owner, variables, overrides, resolved, visiting)?,
        ),
        Expr::Mul(a, b) => Ok(
            eval_expr(a, owner, variables, overrides, resolved, visiting)?
                * eval_expr(b, owner, variables, overrides, resolved, visiting)?,
        ),
        Expr::Div(a, b) => {
            let denom = eval_expr(b, owner, variables, overrides, resolved, visiting)?;
            if denom == 0.0 {
                return Err(FeatureError::InvalidEquation {
                    name: owner.to_string(),
                    why: "division by zero".to_string(),
                });
            }
            Ok(eval_expr(a, owner, variables, overrides, resolved, visiting)? / denom)
        }
    }
}

fn resolve_dim(
    dim: &Dim,
    feature: FeatureId,
    vars: &BTreeMap<String, f64>,
) -> Result<f64, FeatureError> {
    match dim {
        Dim::Lit(v) => Ok(*v),
        Dim::Ref(name) => vars
            .get(name)
            .copied()
            .ok_or_else(|| FeatureError::UnknownVariable {
                name: name.clone(),
                feature: Some(feature),
            }),
    }
}

// ── validation + rebuild ─────────────────────────────────────────────────────────────────────────────────

/// Validate a single feature op against the history so far — well-formed id + all dependencies exist
/// (earlier in the tree). This is the **typed-feature-op gate** an AI-authored change must pass (the M12.4
/// discipline applied to features — a valid typed op, never a raw mutation).
///
/// # Errors
/// [`FeatureError::DuplicateFeatureId`] / [`FeatureError::UnknownDependency`].
pub fn validate_feature_op(op: &FeatureOp, prior: &[FeatureOp]) -> Result<(), FeatureError> {
    let ids: BTreeSet<FeatureId> = prior.iter().map(FeatureOp::id).collect();
    if ids.contains(&op.id()) {
        return Err(FeatureError::DuplicateFeatureId(op.id()));
    }
    for dep in op.deps() {
        if !ids.contains(&dep) {
            return Err(FeatureError::UnknownDependency {
                feature: op.id(),
                dep,
            });
        }
    }
    Ok(())
}

/// Validate a whole history's structure (unique ids; every dependency declared earlier).
///
/// # Errors
/// See [`validate_feature_op`].
pub fn validate_history(history: &FeatureHistory) -> Result<(), FeatureError> {
    let mut seen: Vec<FeatureOp> = Vec::new();
    for op in &history.features {
        validate_feature_op(op, &seen)?;
        seen.push(op.clone());
    }
    Ok(())
}

struct Built {
    entity: EntityId,
    mesh: TriMesh,
}

fn handle_of(mesh: &TriMesh) -> String {
    format!("mtkasset:{:032x}", mesh.content_hash())
}

/// **Rebuild** the feature history into a fresh engine and return the derived scene's deterministic identity
/// — the CAD rebuild as a replay through the existing commit pipeline (each feature = one undoable commit).
/// Geometry is derived with the exact-predicate CSG (M13.2); suppressed features are skipped (bisect).
///
/// Pass an optional [`Configuration`] to rebuild a named variant (its overrides drive the equations).
///
/// # Errors
/// [`FeatureError`] — a circular/invalid equation, a broken (suppressed) dependency, or degenerate geometry,
/// each Blocked + explained.
pub fn rebuild(
    history: &FeatureHistory,
    config: Option<&Configuration>,
) -> Result<Rebuilt, FeatureError> {
    validate_history(history)?;
    let vars = eval_variables(&history.variables, config.map(|c| &c.overrides))?;

    let csg = ExactBspCsg::new();
    let mut engine = Engine::new(FlecsWorld::new(), 1);
    let mut built: BTreeMap<FeatureId, Built> = BTreeMap::new();

    for op in &history.features {
        if history.suppressed.contains(&op.id()) {
            continue;
        }
        // Every dependency must be built (not suppressed / not missing) — the bisectable explained break.
        for dep in op.deps() {
            if !built.contains_key(&dep) {
                let why = if history.suppressed.contains(&dep) {
                    format!("which is suppressed (re-enable feature {dep} to rebuild)")
                } else {
                    format!("whose own rebuild did not produce geometry (feature {dep})")
                };
                return Err(FeatureError::BrokenDependency {
                    feature: op.id(),
                    missing: dep,
                    why,
                });
            }
        }

        build_one_feature(op, &mut engine, &mut built, &vars, csg)?;
    }

    let state_hash = AssetId::of_bytes(engine.canonical_state().as_bytes())
        .as_str()
        .to_string();
    let geometry = built
        .iter()
        .map(|(k, v)| (*k, handle_of(&v.mesh)))
        .collect();
    Ok(Rebuilt {
        state_hash,
        built_features: built.len(),
        geometry,
    })
}

/// Lower one feature op to engine commits + derived geometry (dependencies are pre-checked by [`rebuild`]).
/// A `Box` builds a primitive; a `Carve` is an exact-CSG difference cut in place; a `Pattern` emits the
/// further offset instances.
fn build_one_feature(
    op: &FeatureOp,
    engine: &mut Engine<FlecsWorld>,
    built: &mut BTreeMap<FeatureId, Built>,
    vars: &BTreeMap<String, f64>,
    csg: ExactBspCsg,
) -> Result<(), FeatureError> {
    match op {
        FeatureOp::Box { id, pos, half } => {
            let h = [
                resolve_dim(&half[0], *id, vars)?,
                resolve_dim(&half[1], *id, vars)?,
                resolve_dim(&half[2], *id, vars)?,
            ];
            let mesh = box_mesh(*pos, h);
            let entity = commit_solid(engine, *pos, &mesh)?;
            built.insert(*id, Built { entity, mesh });
        }
        FeatureOp::Carve { id, target, tool } => {
            let result = csg
                .difference(&built[target].mesh, &built[tool].mesh)
                .map_err(|e| FeatureError::DegenerateGeometry {
                    feature: *id,
                    why: e.to_string(),
                })?;
            let report = validate(&result);
            if !report.is_clean() {
                return Err(FeatureError::DegenerateGeometry {
                    feature: *id,
                    why: report.explain(),
                });
            }
            // The cut modifies the target body in place (the CAD "cut feature"): update its mesh handle.
            let entity = built[target].entity;
            engine.commit(
                "feature:carve",
                vec![Op::SetField {
                    entity,
                    component: "MeshRenderer".into(),
                    field: "mesh".into(),
                    value: FieldValue::Str(handle_of(&result)),
                }],
            )?;
            built.insert(
                *id,
                Built {
                    entity,
                    mesh: result,
                },
            );
        }
        FeatureOp::Pattern {
            id,
            source,
            count,
            spacing,
        } => {
            let (src_mesh, src_handle) = {
                let s = &built[source];
                (s.mesh.clone(), handle_of(&s.mesh))
            };
            let mut first: Option<EntityId> = None;
            // The seeded copy is the source; emit `count - 1` further instances offset by `spacing`.
            for i in 1..*count {
                let step = f64::from(i);
                let pos = [spacing[0] * step, spacing[1] * step, spacing[2] * step];
                let e = commit_solid_with_handle(engine, pos, &src_handle)?;
                first.get_or_insert(e);
            }
            let entity = first.unwrap_or(built[source].entity);
            built.insert(
                *id,
                Built {
                    entity,
                    mesh: src_mesh,
                },
            );
        }
    }
    Ok(())
}

/// Commit a solid body (a content-addressed mesh + a transform) as one undoable feature commit.
fn commit_solid(
    engine: &mut Engine<FlecsWorld>,
    pos: [f64; 3],
    mesh: &TriMesh,
) -> Result<EntityId, PipelineError> {
    commit_solid_with_handle(engine, pos, &handle_of(mesh))
}

fn commit_solid_with_handle(
    engine: &mut Engine<FlecsWorld>,
    pos: [f64; 3],
    handle: &str,
) -> Result<EntityId, PipelineError> {
    let id = engine.alloc_entity_id();
    engine.commit(
        "feature:solid",
        vec![
            Op::CreateEntity { id, parent: None },
            Op::SetField {
                entity: id,
                component: "Transform".into(),
                field: "px".into(),
                value: FieldValue::Number(pos[0]),
            },
            Op::SetField {
                entity: id,
                component: "Transform".into(),
                field: "py".into(),
                value: FieldValue::Number(pos[1]),
            },
            Op::SetField {
                entity: id,
                component: "Transform".into(),
                field: "pz".into(),
                value: FieldValue::Number(pos[2]),
            },
            Op::SetField {
                entity: id,
                component: "MeshRenderer".into(),
                field: "mesh".into(),
                value: FieldValue::Str(handle.to_string()),
            },
        ],
    )?;
    Ok(id)
}

/// Rebuild `runs.max(2)` times and assert the rebuild identity is bit-identical every run — the ≥2-runs
/// deterministic-rebuild gate (the M13.1/ADR-050 `reproduces_at` discipline applied to feature ops).
///
/// # Errors
/// Propagates the first rebuild error.
pub fn rebuild_reproduces(
    history: &FeatureHistory,
    config: Option<&Configuration>,
    runs: usize,
) -> Result<bool, FeatureError> {
    let first = rebuild(history, config)?.state_hash;
    for _ in 1..runs.max(2) {
        if rebuild(history, config)?.state_hash != first {
            return Ok(false);
        }
    }
    Ok(true)
}

impl FeatureHistory {
    /// Serialize **losslessly** (bincode — bit-exact f64) so a (broken) history is a reproducible file: the
    /// M13.1/ADR-050 "a bug is a file" discipline applied to feature ops. JSON is NOT used (a 1-ULP
    /// shortest-float round-trip on a parameter would change the rebuild hash).
    ///
    /// # Errors
    /// `bincode::Error` on a serialization failure (pure data — should not fire).
    pub fn to_bytes(&self) -> Result<Vec<u8>, bincode::Error> {
        bincode::serialize(self)
    }

    /// Reload a history from its lossless bincode bytes.
    ///
    /// # Errors
    /// `bincode::Error` on malformed bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(bytes)
    }

    /// Append a feature op after validating it (unique id + dependencies exist) — the typed-feature-op gate
    /// every authored change (human or AI) passes; never a raw mutation.
    ///
    /// # Errors
    /// See [`validate_feature_op`].
    pub fn push_validated(&mut self, op: FeatureOp) -> Result<(), FeatureError> {
        validate_feature_op(&op, &self.features)?;
        self.features.push(op);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real parametric chain: a `width`-driven base box, a smaller tool box, a carve (base minus tool),
    /// and a 3-up linear pattern of the carved body — create -> parametrize -> CSG-carve -> pattern.
    fn parametric_chain() -> FeatureHistory {
        let mut variables = BTreeMap::new();
        // width = 2.0; the base half-extent is driven by it (the equation layer).
        variables.insert("width".to_string(), Expr::Const(2.0));
        FeatureHistory {
            variables,
            features: vec![
                FeatureOp::Box {
                    id: 1,
                    pos: [0.0, 0.0, 0.0],
                    half: [Dim::Ref("width".into()), Dim::Lit(1.0), Dim::Lit(1.0)],
                },
                FeatureOp::Box {
                    id: 2,
                    pos: [0.0, 1.0, 0.0],
                    half: [Dim::Lit(0.5), Dim::Lit(0.5), Dim::Lit(0.5)],
                },
                FeatureOp::Carve {
                    id: 3,
                    target: 1,
                    tool: 2,
                },
                FeatureOp::Pattern {
                    id: 4,
                    source: 3,
                    count: 3,
                    spacing: [5.0, 0.0, 0.0],
                },
            ],
            suppressed: BTreeSet::new(),
        }
    }

    #[test]
    fn equations_evaluate_and_drive_dimensions() {
        let mut vars = BTreeMap::new();
        vars.insert("a".to_string(), Expr::Const(10.0));
        vars.insert(
            "b".to_string(),
            Expr::Div(Box::new(Expr::Var("a".into())), Box::new(Expr::Const(4.0))),
        );
        let resolved = eval_variables(&vars, None).unwrap();
        assert!((resolved["a"] - 10.0).abs() < 1e-12);
        assert!(
            (resolved["b"] - 2.5).abs() < 1e-12,
            "b = a / 4 (a typed equation, not a free-text DSL)"
        );
    }

    #[test]
    fn a_circular_equation_is_blocked_and_explained() {
        let mut vars = BTreeMap::new();
        vars.insert("x".to_string(), Expr::Var("y".into()));
        vars.insert("y".to_string(), Expr::Var("x".into()));
        let err = eval_variables(&vars, None).unwrap_err();
        match &err {
            FeatureError::CircularEquation { cycle } => {
                assert!(cycle.len() >= 2, "the cycle path is reported");
            }
            other => panic!("expected CircularEquation, got {other:?}"),
        }
        let msg = err.to_string();
        assert!(
            msg.contains("circular") && msg.is_ascii(),
            "Blocked + explained, ASCII"
        );
    }

    #[test]
    fn a_divide_by_zero_equation_is_blocked() {
        let mut vars = BTreeMap::new();
        vars.insert(
            "q".to_string(),
            Expr::Div(Box::new(Expr::Const(1.0)), Box::new(Expr::Const(0.0))),
        );
        assert!(matches!(
            eval_variables(&vars, None),
            Err(FeatureError::InvalidEquation { .. })
        ));
    }

    #[test]
    fn an_ai_emitted_op_with_an_unknown_dependency_is_rejected() {
        // The typed-feature-op gate (M12.4 discipline): an op referencing a non-existent feature is Blocked,
        // never applied as a raw mutation.
        let prior = vec![FeatureOp::Box {
            id: 1,
            pos: [0.0; 3],
            half: [Dim::Lit(1.0), Dim::Lit(1.0), Dim::Lit(1.0)],
        }];
        let bad = FeatureOp::Carve {
            id: 2,
            target: 1,
            tool: 99,
        };
        assert!(matches!(
            validate_feature_op(&bad, &prior),
            Err(FeatureError::UnknownDependency {
                feature: 2,
                dep: 99
            })
        ));
        // A duplicate id is also rejected.
        let dup = FeatureOp::Box {
            id: 1,
            pos: [0.0; 3],
            half: [Dim::Lit(1.0), Dim::Lit(1.0), Dim::Lit(1.0)],
        };
        assert!(matches!(
            validate_feature_op(&dup, &prior),
            Err(FeatureError::DuplicateFeatureId(1))
        ));
    }

    #[test]
    fn a_configuration_switch_is_a_deterministic_variant() {
        let history = parametric_chain();
        let base = rebuild(&history, None).unwrap();
        // A "wide" configuration overrides the width variable → a different, but deterministic, variant.
        let wide = Configuration {
            name: "wide".into(),
            overrides: BTreeMap::from([("width".to_string(), 4.0)]),
        };
        let v1 = rebuild(&history, Some(&wide)).unwrap();
        let v2 = rebuild(&history, Some(&wide)).unwrap();
        assert_ne!(
            base.state_hash, v1.state_hash,
            "the variant differs from the base"
        );
        assert_eq!(
            v1.state_hash, v2.state_hash,
            "the same configuration replays deterministically"
        );
    }

    #[test]
    fn the_artifact_round_trips_losslessly() {
        let history = parametric_chain();
        let bytes = history.to_bytes().unwrap();
        let reloaded = FeatureHistory::from_bytes(&bytes).unwrap();
        assert_eq!(
            history, reloaded,
            "a feature history is a lossless bincode file"
        );
        // And the reloaded history rebuilds to the SAME identity (the reproduction artifact works).
        assert_eq!(
            rebuild(&history, None).unwrap().state_hash,
            rebuild(&reloaded, None).unwrap().state_hash
        );
    }
}
