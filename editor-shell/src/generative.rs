//! M15.4 (ADR-074) — **intent-driven generative / differentiable design**, the closed, reproducible,
//! auditable loop and the interop-FEA seam. The PhD R1/R2 home applied to CAD.
//!
//! **The thesis (the 18–36-month window the dossier flags).** "Describe the loads in natural language →
//! optimized geometry," closing the loop **AI-authoring (the M12.4 validated-patch contract) → evaluate
//! (a differentiable-sim gradient if FF-T7 is available, else a deterministic optimizer over a
//! DST-validated objective) → deterministic validation (M13.1 DST) → a validated, undoable patch** — and
//! the whole run **reproducible + auditable across ≥2 runs**, which no incumbent can offer because they
//! lack a replayable op-stream + a deterministic core. The substrate is *already ours*: AI authoring is a
//! schema-validated patch ([`crate::ai::apply_ai_patch`], ADR-048/017), validation is deterministic-in-sim
//! (M13.1/ADR-050), the whole loop is reproducible (the op-log + the seed), and the geometry the optimizer
//! shapes is the M15.0 SDF ([`metrocalk_sdf`]).
//!
//! **The honest claim is VERIFIABILITY + REPRODUCIBILITY + AUDITABILITY, not capability.** Two load-bearing
//! gaps are **named, not papered over**:
//! - **FEA accuracy needs validated solvers — integrate, never rebuild (the §4 AVOID line).** Ansys /
//!   Abaqus / CalculiX / OpenFOAM are validated industries; **determinism ≠ accuracy**. Metrocalk owns the
//!   **reproducible, intent-driven orchestration + audit** ABOVE the solver, not the solver. The
//!   reproducible objective here is a **deterministic reduced-order (ROM) analytical proxy**
//!   ([`RomBeamSolver`] — Euler-Bernoulli beam theory), suitable for min-spec/browser and for the
//!   reproducibility guarantee; the **validated FEA** is an external solver behind [`Solver`]
//!   ([`PreciceFmiSolver`] — preCICE + the preCICE-FMI runner, **server-side native**, a NAMED SEAM here),
//!   whose result is captured into the deterministic audit trail and whose raw FP non-determinism is
//!   **OUT** of the reproducibility claim (like the M15.0 OCCT boundary).
//! - **The differentiable-sim leg (FF-T7) is an M13-frontier dependency — gate on it, don't fake it.** A
//!   *differentiable simulator* (an adjoint through the FEA solver) is itself a frontier bet and is **not
//!   available**; the loop runs the deterministic optimizer and USES the **analytic gradient of the ROM
//!   objective** — a real, closed-form gradient of the *proxy*, explicitly **NOT** a differentiable-sim
//!   adjoint. FF-T7 differentiable-gradient is the named gated upgrade ([`GradientSource`]).
//! - **Generation QUALITY comes from models we don't control — the moat is the GUARANTEE, not the raw
//!   generator (the M13.5 Genie gap).** The win is constrained + validated + reproducible + auditable
//!   generation (R1); the NL→symbolic parse ([`parse_spec`]) recognizes a declared subset and is honest
//!   about what it doesn't (the R1 semantic gap, not solved).
//!
//! **Kept in the shell, `/core` untouched** (the M15.1–15.3 precedent): the reproducible ROM + optimizer
//! are pure `f64` (wasm-portable by construction — the browser/min-spec ROM path, native/server for now,
//! owed); the rapier-coupled **M13.1 DST validate-in-sim** is exercised by the spike test as a
//! **dev-dependency** (native), keeping this lib free of the physics backend (the ROM-in-guarantee /
//! heavy-solver-out split at the crate-graph level).

use crate::ai::{apply_ai_patch, AiPatch, PatchOp};
use crate::bridge::{ProjectionDelta, RejectInfo};
use crate::csg_intent::mesh_asset_to_trimesh;
use crate::sdf_intent::{bake as bake_sdf, SdfBakeError};
use metrocalk_assets::AssetStore;
use metrocalk_authoring::Certificate;
use metrocalk_core::registry::{ComponentMeta, FieldType};
use metrocalk_core::{Engine, EntityId, FieldValue, Op, PipelineError};
use metrocalk_csg::validate as validate_mesh;
use metrocalk_ecs::World;
use metrocalk_sdf::{Axis, Grid, Sdf};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::f64::consts::PI;

/// The registered generative-design component name (the [`apply_ai_patch`] schema key + the ECS component
/// the optimized design entity carries). Kept in the shell like the rest of the M15 CAD surfacing.
pub const DESIGN_COMPONENT: &str = "GenerativeDesign";

/// The mesh resolution the chosen design is baked at (off the hot path — an authoring action). The
/// elongated bracket gets fine cross-section resolution from [`Grid::around`]'s per-axis cells.
const BAKE_RES: usize = 40;

// ── the material library (SI: Pa, kg/m³ — a closed set, cited not free-text) ──────────────────────────────

/// An engineering material — a **closed enum** (so the ROM never rests on a made-up property; the numbers
/// are handbook nominals, not a certified datasheet — the honest-material caveat). Serde-friendly (a plain
/// tag) so it round-trips in the bincode audit artifact.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Material {
    /// Structural steel (handbook nominal).
    Steel,
    /// 6061-T6 aluminium (handbook nominal).
    Aluminum,
}

impl Material {
    /// Young's modulus E (Pa).
    #[must_use]
    pub fn youngs_modulus_pa(self) -> f64 {
        match self {
            Material::Steel => 200.0e9,
            Material::Aluminum => 69.0e9,
        }
    }
    /// Yield strength σ_y (Pa) — the allowable stress before the safety factor.
    #[must_use]
    pub fn yield_strength_pa(self) -> f64 {
        match self {
            Material::Steel => 250.0e6,
            Material::Aluminum => 240.0e6,
        }
    }
    /// Density ρ (kg/m³).
    #[must_use]
    pub fn density_kg_m3(self) -> f64 {
        match self {
            Material::Steel => 7850.0,
            Material::Aluminum => 2700.0,
        }
    }
    /// The canonical material name (part of the audit trail).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Material::Steel => "steel",
            Material::Aluminum => "aluminum",
        }
    }

    /// Parse a canonical material name (a closed set — unknown → `None`, an honest "not recognized").
    #[must_use]
    pub fn from_name(s: &str) -> Option<Material> {
        match s.to_ascii_lowercase().as_str() {
            "steel" => Some(Material::Steel),
            "aluminum" | "aluminium" => Some(Material::Aluminum),
            _ => None,
        }
    }
}

/// The optimization objective. A closed enum (only mass-minimization is shipped — the prompt's example);
/// stiffness/compliance objectives are the named future.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Objective {
    /// Minimize the part's mass subject to the stress + deflection budgets.
    MinimizeMass,
}

// ── the NL load-spec + its deterministic parse (the R1 constrained front; honest about the gap) ────────────

/// A structured **load specification** — the symbolic target the NL sentence is compiled to. It carries the
/// load, the cantilever envelope (span + width are the fixed problem geometry; the section height + bore are
/// the design variables), the material, and the budgets. All SI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LoadSpec {
    /// The original NL sentence (the audit trail's provenance — what the user asked for).
    pub description: String,
    /// The downward tip load F (N).
    pub tip_load_n: f64,
    /// The cantilever span L (m) — the beam length, fixed at the base, loaded at the tip.
    pub span_m: f64,
    /// The section width W (m) — a fixed problem dimension.
    pub width_m: f64,
    /// The section-height search range `[min, max]` (m) — a design variable.
    pub height_bounds_m: (f64, f64),
    /// The material.
    pub material: Material,
    /// The design safety factor (allowable stress = yield / safety_factor).
    pub safety_factor: f64,
    /// The tip-deflection budget (m).
    pub deflection_limit_m: f64,
    /// What to optimize.
    pub objective: Objective,
}

impl LoadSpec {
    /// The allowable stress = yield / safety_factor (the design budget the ROM checks against).
    #[must_use]
    pub fn allowable_stress_pa(&self) -> f64 {
        self.yield_over_sf()
    }

    fn yield_over_sf(&self) -> f64 {
        self.material.yield_strength_pa() / self.safety_factor.max(1.0)
    }

    /// A sensible bracket default (a 100×20 mm steel cantilever, section 5–40 mm, SF 2, 0.5 mm deflection
    /// budget) — the fields the NL sentence does not state fall back here (documented, not invented).
    #[must_use]
    fn default_bracket(description: String, tip_load_n: f64, material: Material) -> Self {
        Self {
            description,
            tip_load_n,
            span_m: 0.100,
            width_m: 0.020,
            height_bounds_m: (0.005, 0.040),
            material,
            safety_factor: 2.0,
            deflection_limit_m: 0.0005,
            objective: Objective::MinimizeMass,
        }
    }
}

/// The NL parse couldn't produce a spec — surfaced, never a silent default (the explain discipline).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpecError {
    /// No load magnitude ("… N" / "… newtons") was found.
    NoLoad,
    /// No recognized objective ("minimize mass" / "lightest" / "minimize weight") was found.
    NoObjective,
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpecError::NoLoad => write!(
                f,
                "no load found — state the force, e.g. \"carries 200 N down at the tip\""
            ),
            SpecError::NoObjective => write!(
                f,
                "no objective found — state one, e.g. \"minimize mass\" / \"make it as light as possible\""
            ),
        }
    }
}

impl std::error::Error for SpecError {}

/// **Compile an NL load-spec to the symbolic [`LoadSpec`]** — the R1 constrained front. Recognizes a
/// **declared subset**: a load magnitude in newtons, a mass-minimization objective, and (optionally) a
/// material name; the rest of the problem geometry defaults ([`LoadSpec::default_bracket`]). Deterministic +
/// offline (no LLM — AI-is-a-guest). This is intentionally a *bounded* parser: the NL→symbolic **fidelity
/// gap** (a richer sentence than this grammar covers) is the M13.5/R1 semantic-gap, **named, not solved** —
/// an unrecognized request is a [`SpecError`], never a wrong silent guess.
///
/// # Errors
/// [`SpecError::NoLoad`] / [`SpecError::NoObjective`] when the sentence lacks a recognized load / objective.
pub fn parse_spec(nl: &str) -> Result<LoadSpec, SpecError> {
    let lower = nl.to_ascii_lowercase();

    // Objective (a mass-minimization synonym set).
    let has_objective = [
        "minimize mass",
        "minimise mass",
        "minimize weight",
        "lightest",
        "as light",
    ]
    .iter()
    .any(|k| lower.contains(k));
    if !has_objective {
        return Err(SpecError::NoObjective);
    }

    // Load: the number immediately preceding a newton token ("200 n", "200n", "200 newtons").
    let tip_load_n = parse_newtons(&lower).ok_or(SpecError::NoLoad)?;

    // Material (default steel; the closed set, honest about "not recognized" → falls back to steel).
    let material = ["steel", "aluminum", "aluminium", "titanium"]
        .iter()
        .find(|m| lower.contains(**m))
        .and_then(|m| Material::from_name(m))
        .unwrap_or(Material::Steel);

    Ok(LoadSpec::default_bracket(
        nl.to_string(),
        tip_load_n,
        material,
    ))
}

/// Extract "<number> N" (newtons) from a lowercased sentence — a combined "200n" token, or a numeric token
/// immediately before an `n` / `newton(s)` unit token. Tokenizes on non-alphanumeric (punctuation is a
/// separator, so "150 n," works). Pure decimal scan, no locale/transcendental dependence.
fn parse_newtons(lower: &str) -> Option<f64> {
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    for c in lower.chars() {
        if c.is_ascii_alphanumeric() || c == '.' {
            cur.push(c);
        } else if !cur.is_empty() {
            tokens.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    // A combined "200n" token (a digit run ending in a lone 'n').
    for t in &tokens {
        if let Some(num) = t.strip_suffix('n') {
            if !num.is_empty() {
                if let Ok(v) = num.parse::<f64>() {
                    return Some(v);
                }
            }
        }
    }
    // "<num> n" / "<num> newton(s)".
    for i in 1..tokens.len() {
        if matches!(tokens[i].as_str(), "n" | "newton" | "newtons") {
            if let Ok(v) = tokens[i - 1].parse::<f64>() {
                return Some(v);
            }
        }
    }
    None
}

// ── the parametric design (the SDF the optimizer shapes) ──────────────────────────────────────────────────

/// The **design vector**: a prismatic cantilever section of `height_m` with an axial lightening bore of
/// `bore_r_m` (a cylinder along the length / neutral axis — it removes the least-stressed material). The
/// geometry is the M15.0 canonical `box − cylinder` SDF op.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Design {
    /// The section height H (m) — the primary bending dimension.
    pub height_m: f64,
    /// The axial bore radius r (m) — 0 for a solid section.
    pub bore_r_m: f64,
}

impl Design {
    /// The M15.0 SDF program: a box `L × H × W` minus an axial cylinder of radius `r` along the length (X).
    /// A through-bore (the cylinder half-length spans the box), so the compiled solid is genus-1 when
    /// `r > 0`. Built in metres.
    #[must_use]
    pub fn to_sdf(&self, spec: &LoadSpec) -> Sdf {
        let half = [spec.span_m / 2.0, self.height_m / 2.0, spec.width_m / 2.0];
        let box_sdf = Sdf::cuboid([0.0, 0.0, 0.0], half);
        if self.bore_r_m > 0.0 {
            let bore = Sdf::cylinder([0.0, 0.0, 0.0], self.bore_r_m, spec.span_m, Axis::X);
            box_sdf.difference(bore)
        } else {
            box_sdf
        }
    }

    /// The bore is geometrically admissible (fits inside the section with a wall).
    fn bore_fits(&self, spec: &LoadSpec) -> bool {
        self.bore_r_m >= 0.0
            && self.bore_r_m < self.height_m / 2.0
            && self.bore_r_m < spec.width_m / 2.0
    }
}

// ── the Solver / Analysis trait (the M8.5 Interchange pattern applied to structural analysis) ──────────────

/// The structural result of analysing a [`Design`] under a [`LoadSpec`] — the objective + the constraint
/// quantities. SI. A `NaN` is never stored (a degenerate section returns `∞` stress/deflection), so the
/// audit trail round-trips + compares bit-exactly (the reproducibility discipline).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct StructuralResult {
    /// The part mass (kg) — the objective.
    pub mass_kg: f64,
    /// The maximum bending stress (Pa) — the strength constraint.
    pub max_stress_pa: f64,
    /// The tip deflection (m) — the stiffness constraint.
    pub tip_deflection_m: f64,
}

impl StructuralResult {
    /// Whether the design meets both budgets (strength + stiffness) — the feasibility test the objective is
    /// minimized subject to.
    #[must_use]
    pub fn feasible(&self, spec: &LoadSpec) -> bool {
        self.max_stress_pa.is_finite()
            && self.tip_deflection_m.is_finite()
            && self.max_stress_pa <= spec.allowable_stress_pa()
            && self.tip_deflection_m <= spec.deflection_limit_m
    }

    /// The canonical integer objective (micrograms) — an exact key for a **bit-stable arg-min** (the
    /// M13.1/M15.3 canonical-result discipline: never compare optima on raw floats). Infeasible / degenerate
    /// → [`u64::MAX`] so it never wins.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    fn objective_key(&self, spec: &LoadSpec) -> u64 {
        if !self.feasible(spec) || !self.mass_kg.is_finite() || self.mass_kg < 0.0 {
            return u64::MAX;
        }
        // kg → micrograms, rounded to the nearest integer (exact, deterministic).
        (self.mass_kg * 1.0e9).round() as u64
    }
}

/// An analysis couldn't be produced — surfaced, never a silent bad number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SolverError {
    /// The design is geometrically degenerate (e.g. the bore doesn't fit the section).
    Degenerate(String),
    /// The solver is a server-side seam that isn't wired in this build (preCICE/FMI) — a NAMED SEAM, never
    /// a fake result.
    Unavailable {
        /// The solver name.
        solver: &'static str,
        /// Why it's unavailable + where it lives.
        reason: String,
    },
}

impl std::fmt::Display for SolverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SolverError::Degenerate(why) => write!(f, "degenerate design: {why}"),
            SolverError::Unavailable { solver, reason } => {
                write!(f, "solver '{solver}' unavailable: {reason}")
            }
        }
    }
}

impl std::error::Error for SolverError {}

/// The project-owned **structural-analysis seam** (invariant 5, the M8.5 [`metrocalk_interchange::Interchange`]
/// pattern). A [`Solver`] evaluates a [`Design`] under a [`LoadSpec`]. Two kinds live behind it:
/// - a **deterministic, in-guarantee** solver ([`RomBeamSolver`]) — pure `f64`, reproducible, the citizen
///   of the browser/min-spec path and of the reproducibility claim;
/// - a **non-deterministic external solver** ([`PreciceFmiSolver`]) — a validated FEA integrated
///   server-side (preCICE + the preCICE-FMI runner over CalculiX/OpenFOAM), whose result is captured into
///   the audit trail but whose raw FP is **OUT** of the reproducibility guarantee.
///
/// **No foreign solver type crosses this trait** (no `precice::`/`fmi::` in a signature) — CI grep-gated,
/// the same discipline as `rapier::`-in-`/physics`, `robust::`-in-`/csg`, OCCT-behind-`CadInterchange`.
pub trait Solver {
    /// The solver's name (provenance / audit).
    fn name(&self) -> &'static str;

    /// Whether this solver is **deterministic + in the reproducibility guarantee**. `true` for the ROM;
    /// `false` for an external FEA (its FP non-determinism is confined behind the trait, ADR-074 audit).
    fn deterministic(&self) -> bool;

    /// Analyse `design` under `spec`.
    ///
    /// # Errors
    /// [`SolverError::Degenerate`] for an inadmissible section; [`SolverError::Unavailable`] for a
    /// server-side seam that isn't wired.
    fn analyze(&self, design: &Design, spec: &LoadSpec) -> Result<StructuralResult, SolverError>;
}

/// The **deterministic reduced-order (ROM) solver** — Euler-Bernoulli cantilever beam theory, closed form,
/// `f64`, using only correctly-rounded ops (`+ - * / sqrt`-free here) so it is **bit-reproducible** and
/// wasm-portable (the browser/min-spec citizen). **It is a proxy, not validated FEA** — its accuracy is
/// beam-theory's (thin-section, small-deflection, linear-elastic), NOT a claim of Ansys-grade fidelity; the
/// milestone claims the reproducible *loop*, not the *physics*.
#[derive(Clone, Copy, Debug, Default)]
pub struct RomBeamSolver;

impl Solver for RomBeamSolver {
    fn name(&self) -> &'static str {
        "rom-beam"
    }
    fn deterministic(&self) -> bool {
        true
    }
    #[allow(clippy::many_single_char_names)] // w/h/r/l/e/f/c are the standard beam-theory symbols
    fn analyze(&self, design: &Design, spec: &LoadSpec) -> Result<StructuralResult, SolverError> {
        if !design.bore_fits(spec) {
            return Err(SolverError::Degenerate(format!(
                "bore r={:.4} m does not fit a section H={:.4} m, W={:.4} m",
                design.bore_r_m, design.height_m, spec.width_m
            )));
        }
        let (w, h, r) = (spec.width_m, design.height_m, design.bore_r_m);
        let (l, e, f, rho) = (
            spec.span_m,
            spec.material.youngs_modulus_pa(),
            spec.tip_load_n,
            spec.material.density_kg_m3(),
        );

        // Section area (rectangle minus the axial circular bore) and mass (prismatic).
        let area = w * h - PI * r * r;
        let mass_kg = rho * area * l;

        // Second moment of area about the horizontal (Z) centroidal axis: rectangle minus the centred
        // circular bore. A vertical tip load bends about Z; the bore sits on the neutral axis.
        let i_z = w * h * h * h / 12.0 - PI * r * r * r * r / 4.0;
        if i_z <= 0.0 || area <= 0.0 {
            // A hollowed-out degenerate section: report ∞ (never NaN), so it is infeasible, never chosen.
            return Ok(StructuralResult {
                mass_kg: mass_kg.max(0.0),
                max_stress_pa: f64::INFINITY,
                tip_deflection_m: f64::INFINITY,
            });
        }

        // Bending: M = F·L at the fixed base; σ = M·c / I, c = H/2. Deflection: δ = F·L³ / (3·E·I).
        let moment = f * l;
        let c = h / 2.0;
        let max_stress_pa = moment * c / i_z;
        let tip_deflection_m = f * l * l * l / (3.0 * e * i_z);

        Ok(StructuralResult {
            mass_kg,
            max_stress_pa,
            tip_deflection_m,
        })
    }
}

/// The **external validated-FEA seam** — preCICE + the preCICE-FMI runner over CalculiX / Gmsh-meshing /
/// OpenFOAM, **server-side native** (the crate-reality: CalculiX/OpenFOAM are **not** native FMUs; the
/// established couple is preCICE adapters + the preCICE-FMI runner, a Python/C++ pipeline). Metrocalk is the
/// **deterministic orchestration + intent + audit master** over it. **Integrate, never rebuild Ansys** (the
/// §4 AVOID line). This impl is the trait boundary that RESERVES the seam — it returns
/// [`SolverError::Unavailable`] until the server pipeline is wired; it never fabricates a stress. When it is
/// wired, its result is captured into the audit trail and its raw FP non-determinism stays **OUT** of the
/// reproducibility guarantee ([`Solver::deterministic`] is `false`).
#[derive(Clone, Copy, Debug, Default)]
pub struct PreciceFmiSolver;

impl Solver for PreciceFmiSolver {
    fn name(&self) -> &'static str {
        "precice-fmi"
    }
    fn deterministic(&self) -> bool {
        // A validated external FEA is NOT bit-reproducible (mesh/solver FP + threading vary); its result is
        // captured into the audit trail, never inside the reproducibility claim (the ADR-074 audit boundary).
        false
    }
    fn analyze(&self, _design: &Design, _spec: &LoadSpec) -> Result<StructuralResult, SolverError> {
        Err(SolverError::Unavailable {
            solver: "precice-fmi",
            reason: "validated FEA is a server-side seam (preCICE + the preCICE-FMI runner over \
                     CalculiX/OpenFOAM, native/server-only — integrate, never rebuild Ansys); \
                     not wired in this build. The min-spec/browser path uses the ROM surrogate."
                .to_string(),
        })
    }
}

// ── the differentiable-sim leg (FF-T7): used-or-named-gated, never faked ───────────────────────────────────

/// Where the optimizer's descent gradient comes from. The loop **uses** [`GradientSource::AnalyticRom`] (a
/// real, closed-form gradient of the ROM objective); [`GradientSource::DifferentiableSim`] (FF-T7 — an
/// adjoint through a *differentiable simulator*) is the **named gated upgrade**, an M13-frontier dependency
/// that is **not available** — never faked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GradientSource {
    /// The analytic gradient of the reduced-order (beam-theory) objective — real + deterministic. **NOT** a
    /// differentiable-simulation adjoint.
    AnalyticRom,
    /// FF-T7: an adjoint through a differentiable simulator — the gated upgrade (an M13-frontier dependency).
    DifferentiableSim,
}

impl GradientSource {
    /// Whether the FF-T7 differentiable-sim gradient is available in this build (it is not — a frontier
    /// dependency; the loop never fakes it).
    #[must_use]
    pub fn ff_t7_available() -> bool {
        false
    }
}

/// The analytic gradient of the ROM **mass** objective w.r.t. the design vector `(height, bore_r)`. Closed
/// form: `mass = ρ·L·(W·H − π·r²)` ⇒ `∂mass/∂H = ρ·L·W`, `∂mass/∂r = −ρ·L·2π·r`. To *reduce* mass we descend
/// this gradient (shrink H, grow r) — subject to feasibility. **A gradient of the proxy, not of a simulator**
/// (FF-T7).
#[must_use]
fn mass_gradient(design: &Design, spec: &LoadSpec) -> [f64; 2] {
    let k = spec.material.density_kg_m3() * spec.span_m;
    [k * spec.width_m, -k * 2.0 * PI * design.bore_r_m]
}

// ── the deterministic optimizer + the reproducible, auditable run ──────────────────────────────────────────

/// One evaluated design in the audit trail — the design vector, its analysis, whether it was feasible, and
/// the canonical integer objective (the bit-stable arg-min key). Plain data (bincode-lossless).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DesignCandidate {
    /// The design vector evaluated.
    pub design: Design,
    /// Its structural analysis.
    pub result: StructuralResult,
    /// Whether it met both budgets.
    pub feasible: bool,
    /// The canonical integer objective (micrograms; [`u64::MAX`] if infeasible/degenerate).
    pub objective_ug: u64,
    /// How this candidate was produced (sweep vs a seeded gradient restart) — the audit provenance.
    pub origin: CandidateOrigin,
}

/// How a candidate entered the search — the audit provenance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CandidateOrigin {
    /// A point on the deterministic parametric sweep.
    Sweep,
    /// A seeded gradient-descent restart (the seed-driven leg — same seed → same restarts).
    GradientRestart,
}

/// A **generative-design run** — the complete, replayable audit trail: the spec, the seed, the solver, the
/// candidates (spec → candidates → objective), the chosen index, the gradient source, and a
/// **bit-reproducible objective hash**. Serialized **LOSSLESS via bincode** ("a design run is a file", the
/// M13.1/ADR-050 "a bug is a file" discipline — a JSON float round-trip would drift a parameter by 1 ULP and
/// break the bit-identical reproduction). Same `(spec, seed)` ⇒ identical bytes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GenerativeRun {
    /// The NL sentence + its parsed spec (the audit provenance — what the user asked for).
    pub spec: LoadSpec,
    /// The injected seed (any stochastic search choice is reproducible from it — the DST/VOPR shape).
    pub seed: u64,
    /// The solver's name (the reproducible ROM here; a server-side FEA would be captured too).
    pub solver: String,
    /// Whether the solver is deterministic (in the reproducibility guarantee).
    pub deterministic_objective: bool,
    /// The sweep resolution `(height_steps, bore_steps)`.
    pub grid: (usize, usize),
    /// Every design evaluated, in deterministic order (the audit trail).
    pub candidates: Vec<DesignCandidate>,
    /// The chosen candidate index (the min-mass feasible design), or `None` if the loop found none (parked).
    pub chosen: Option<usize>,
    /// The gradient source used (analytic ROM) — FF-T7 differentiable-sim is the named gated upgrade.
    pub gradient_source: GradientSource,
    /// A hash of the chosen candidate's exact bytes — the **bit-reproducible objective** witness.
    pub objective_hash: String,
}

impl GenerativeRun {
    /// The chosen design (min-mass feasible), if the loop converged.
    #[must_use]
    pub fn chosen_candidate(&self) -> Option<&DesignCandidate> {
        self.chosen.and_then(|i| self.candidates.get(i))
    }

    /// The **LOSSLESS bincode artifact** — "a design run is a file" (the M13.1 discipline). Ship it and it
    /// re-runs the exact audit trail bit-for-bit.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        bincode::serialize(self).expect("a generative run serializes (pure data)")
    }

    /// Reload a "design run is a file" artifact (bincode).
    ///
    /// # Errors
    /// The bincode error if the artifact is malformed / out of format.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(bytes)
    }

    /// **Replay the audit trail** — re-evaluate every candidate through `solver` and confirm the stored
    /// result matches bit-for-bit. The audit is *replayable*: an auditor re-derives the run, they don't
    /// trust the recorded numbers. `true` iff every candidate reproduces.
    #[must_use]
    pub fn verify_audit<S: Solver>(&self, solver: &S) -> bool {
        self.candidates.iter().all(|cand| {
            solver
                .analyze(&cand.design, &self.spec)
                .is_ok_and(|r| r == cand.result)
        })
    }
}

/// A seed-driven deterministic RNG (`splitmix64`) — the **injected** randomness for the gradient restarts
/// (the M13.1/ADR-050 discipline: inject all non-determinism, so `(spec, seed)` fully determines the run).
/// Pure integer math ⇒ no FP/ISA variance in the draws.
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    #[allow(clippy::cast_precision_loss)]
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / 9_007_199_254_740_992.0
    }
}

/// **THE GENERATIVE LOOP** (deliverable #1/#2): a deterministic optimizer over the ROM objective. Runs a
/// **parametric sweep** (`res × res` over the section-height × bore-radius design space) plus a small number
/// of **seeded gradient-descent restarts** (the analytic ROM gradient — the differentiable leg used
/// honestly), keeps every evaluated design in the audit trail, and picks the **feasible min-mass** design by
/// its canonical integer objective (a bit-stable arg-min, ties broken by first index). Fully deterministic +
/// reproducible: same `(spec, seed, res)` ⇒ identical [`GenerativeRun`] (bit-for-bit, `verify_audit`-able).
///
/// The objective is [`Solver::deterministic`]; a non-deterministic external solver's result would be
/// captured into the audit trail but is **out** of the reproducibility claim (ADR-074).
#[must_use]
#[allow(clippy::cast_precision_loss)] // res + step indices are small; the usize→f64 cast is exact here
pub fn optimize<S: Solver>(spec: &LoadSpec, seed: u64, res: usize, solver: &S) -> GenerativeRun {
    /// The number of seeded gradient-descent restarts (the seed-driven leg — same seed → same restarts).
    const RESTARTS: usize = 4;
    let res = res.max(2);
    let mut candidates: Vec<DesignCandidate> = Vec::new();

    let eval = |design: Design, origin: CandidateOrigin| -> Option<DesignCandidate> {
        match solver.analyze(&design, spec) {
            Ok(result) => {
                let feasible = result.feasible(spec);
                Some(DesignCandidate {
                    design,
                    result,
                    feasible,
                    objective_ug: result.objective_key(spec),
                    origin,
                })
            }
            Err(_) => None, // degenerate designs are not part of the trail (they don't exist as parts)
        }
    };

    // 1) the deterministic parametric sweep over (height, bore).
    let (h_lo, h_hi) = spec.height_bounds_m;
    for hi in 0..res {
        let h = h_lo + (h_hi - h_lo) * (hi as f64) / ((res - 1) as f64);
        // bore in [0, min(H/2, W/2)) with a wall margin (last step stops short of the wall).
        let r_max = (h / 2.0).min(spec.width_m / 2.0) * 0.95;
        for ri in 0..res {
            let r = r_max * (ri as f64) / (res as f64); // 0 .. r_max·(res-1)/res (never touches the wall)
            if let Some(c) = eval(
                Design {
                    height_m: h,
                    bore_r_m: r,
                },
                CandidateOrigin::Sweep,
            ) {
                candidates.push(c);
            }
        }
    }

    // 2) seeded gradient-descent restarts (the analytic ROM gradient, the differentiable leg used honestly).
    let mut rng = Rng::new(seed);
    for _ in 0..RESTARTS {
        let h = h_lo + (h_hi - h_lo) * rng.unit();
        let r = (h / 2.0).min(spec.width_m / 2.0) * 0.9 * rng.unit();
        let polished = gradient_descend(
            Design {
                height_m: h,
                bore_r_m: r,
            },
            spec,
            solver,
        );
        if let Some(c) = eval(polished, CandidateOrigin::GradientRestart) {
            candidates.push(c);
        }
    }

    // 3) the bit-stable arg-min over the canonical integer objective (feasible only; ties → first index).
    let chosen = candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.feasible)
        .min_by_key(|(_, c)| c.objective_ug)
        .map(|(i, _)| i);

    let objective_hash = chosen.map_or_else(
        || "none".to_string(),
        |i| hash_bytes(&bincode::serialize(&candidates[i]).unwrap_or_default()),
    );

    GenerativeRun {
        spec: spec.clone(),
        seed,
        solver: solver.name().to_string(),
        deterministic_objective: solver.deterministic(),
        grid: (res, res),
        candidates,
        chosen,
        gradient_source: GradientSource::AnalyticRom,
        objective_hash,
    }
}

/// A **deterministic projected gradient descent** on the ROM mass objective from a start design: repeatedly
/// step in the mass-decreasing direction (shrink height along `−∂mass/∂H`, grow bore along `−∂mass/∂r`) with
/// a geometric step schedule, **projecting back to feasibility** (never accept an over-budget step). Fixed
/// iteration count + step schedule ⇒ bit-reproducible. Uses [`mass_gradient`] — the analytic ROM gradient,
/// **not** an FF-T7 adjoint.
fn gradient_descend<S: Solver>(start: Design, spec: &LoadSpec, solver: &S) -> Design {
    let (h_lo, h_hi) = spec.height_bounds_m;
    let mut best = start;
    let mut step = (h_hi - h_lo) * 0.25; // an absolute step in metres, shrunk each round
    for _ in 0..24 {
        let g = mass_gradient(&best, spec);
        // Descend the NEGATIVE gradient, MAGNITUDE-WEIGHTED per dimension (unit-normalized) — so the gradient
        // is genuinely load-bearing: H moves ∝ ∂mass/∂H and the bore moves ∝ ∂mass/∂r (which grows with r),
        // the search direction tilting as the relative sensitivities change. A real analytic-ROM-gradient
        // descent, projected to feasibility (`sqrt` is a correctly-rounded op — deterministic). NOT a fixed
        // direction, and NOT an FF-T7 differentiable-simulator adjoint.
        let norm = (g[0] * g[0] + g[1] * g[1]).sqrt();
        if norm > 0.0 {
            let cand = Design {
                height_m: (best.height_m - step * g[0] / norm).clamp(h_lo, h_hi),
                bore_r_m: (best.bore_r_m - step * g[1] / norm).max(0.0),
            };
            if solver.analyze(&cand, spec).is_ok_and(|r| r.feasible(spec)) {
                best = cand; // only accept a feasible step (projected descent to the constraint boundary)
            }
        }
        // Always shrink the step (a deterministic schedule) so the search converges.
        step *= 0.6;
    }
    best
}

/// A tiny deterministic byte hash (FNV-1a, 64-bit) → hex — the reproducible objective witness. Not
/// cryptographic; a stable, offline, dependency-free content hash of the chosen candidate's exact bytes.
fn hash_bytes(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

// ── the every-no certificate for an over-budget proposal (reuse the M13.5 seed) ────────────────────────────

/// A **rejection certificate** for an over-budget / infeasible design — the "explain every no", reusing the
/// shipped [`metrocalk_authoring::Certificate`] (the M13.5/ADR-054 unsat-core seed that "ties M13.9"), NOT a
/// copy string: *which* budget the design busts + by how much. `None` when the design is feasible. The full
/// M13.9 semiring-provenance theorem (ADR-061) is a named future; this reuses its seed (the M15.3 precedent).
#[must_use]
pub fn design_certificate<S: Solver>(
    design: &Design,
    spec: &LoadSpec,
    solver: &S,
) -> Option<Certificate> {
    let result = solver.analyze(design, spec).ok()?;
    if result.feasible(spec) {
        return None;
    }
    let mut unsat_core = Vec::new();
    if result.max_stress_pa > spec.allowable_stress_pa() {
        unsat_core.push(format!(
            "bending stress {:.1} MPa > allowable {:.1} MPa (yield {:.0} MPa / SF {:.1})",
            result.max_stress_pa / 1.0e6,
            spec.allowable_stress_pa() / 1.0e6,
            spec.material.yield_strength_pa() / 1.0e6,
            spec.safety_factor,
        ));
    }
    if result.tip_deflection_m > spec.deflection_limit_m {
        unsat_core.push(format!(
            "tip deflection {:.3} mm > budget {:.3} mm",
            result.tip_deflection_m * 1000.0,
            spec.deflection_limit_m * 1000.0,
        ));
    }
    let reason = format!(
        "the proposed section (H {:.1} mm, bore \u{2300}{:.1} mm) is over budget under the {:.0} N tip load: \
         {} \u{2014} thicken the section or reduce the bore (integrate a validated FEA to certify accuracy)",
        design.height_m * 1000.0,
        design.bore_r_m * 2000.0,
        spec.tip_load_n,
        unsat_core.join("; "),
    );
    Some(Certificate { reason, unsat_core })
}

// ── the validated, undoable patch (M12.4 apply_ai_patch) + geometry realization ────────────────────────────

/// The registered [`ComponentMeta`] for the generative-design component — the schema the optimized-design
/// patch ([`apply_optimized_design`]) validates against (reuse the ADR-048/017 contract). Kept in the shell.
#[must_use]
pub fn design_component_meta() -> ComponentMeta {
    ComponentMeta::builder(DESIGN_COMPONENT)
        .category("Props")
        .field("height_mm", FieldType::Number, true)
        .field("bore_dia_mm", FieldType::Number, true)
        .field("mass_g", FieldType::Number, true)
        .field("stress_mpa", FieldType::Number, true)
        .field("deflection_mm", FieldType::Number, true)
        .field_fmt("mesh", FieldType::String, false, Some("mesh-handle"))
        .field("spec", FieldType::String, false)
        .tag("cad")
        .tag("generative")
        .ui_hint("height_mm", "the optimized section height (mm)")
        .ui_hint("bore_dia_mm", "the optimized lightening-bore diameter (mm)")
        .ui_hint(
            "mass_g",
            "the optimized part mass (g) — the minimized objective",
        )
        .build()
}

/// Create the design entity that the optimized patch targets — a renderable entity carrying a baseline
/// [`DESIGN_COMPONENT`] + a `Transform` at `pos`, in one undoable commit. The optimizer's chosen design is
/// then applied as a **validated patch** ([`apply_optimized_design`]) — the M15.3 pattern (create via
/// commit, optimize via `apply_ai_patch`).
///
/// # Errors
/// Propagates a [`PipelineError`] if the commit is rejected.
pub fn place_design_seed<W: World>(
    engine: &mut Engine<W>,
    pos: [f64; 3],
    spec: &LoadSpec,
) -> Result<EntityId, PipelineError> {
    let id = engine.alloc_entity_id();
    engine.commit(
        "place-generative-design",
        vec![
            Op::CreateEntity { id, parent: None },
            Op::SetField {
                entity: id,
                component: "Transform".into(),
                field: "x".into(),
                value: FieldValue::Number(pos[0]),
            },
            Op::SetField {
                entity: id,
                component: "Transform".into(),
                field: "y".into(),
                value: FieldValue::Number(pos[1]),
            },
            Op::SetField {
                entity: id,
                component: "Transform".into(),
                field: "z".into(),
                value: FieldValue::Number(pos[2]),
            },
            Op::SetField {
                entity: id,
                component: DESIGN_COMPONENT.into(),
                field: "spec".into(),
                value: FieldValue::Str(spec.description.clone()),
            },
        ],
    )?;
    Ok(id)
}

/// **AI proposes a geometry change as a validated, undoable patch** (deliverable #2, the M12.4 contract).
/// The proposed `design`'s parameters (height, bore, mass, stress, deflection) + its baked mesh handle are
/// applied to `design_entity`'s [`DESIGN_COMPONENT`] through the shipped [`apply_ai_patch`] contract — a
/// schema-validated, single, undoable transaction (**never a raw mutation**). The geometry is realized by
/// baking the design's M15.0 SDF to a **content-addressed, watertight** mesh (geometry by handle, invariant
/// 2). **Over budget → rejected-as-UX** with the [`design_certificate`] reason (nothing applied); a
/// degenerate section or an invalid patch is likewise rejected. Returns the `ProjectionDelta` (confirms on
/// success, rejects with the reason otherwise).
pub fn propose_design<W: World>(
    engine: &mut Engine<W>,
    store: &mut AssetStore,
    design_entity: EntityId,
    spec: &LoadSpec,
    design: &Design,
    client_op_id: &str,
) -> ProjectionDelta {
    // Rejected-as-UX: an over-budget design does not get applied (the every-no certificate reason).
    if let Some(cert) = design_certificate(design, spec, &RomBeamSolver) {
        return reject(client_op_id, cert.reason);
    }
    let result = match RomBeamSolver.analyze(design, spec) {
        Ok(r) => r,
        Err(e) => return reject(client_op_id, e.to_string()),
    };
    // Realize the geometry: bake the SDF to a content-addressed, watertight mesh (geometry by handle).
    let handle = match bake_design(store, design, spec) {
        Ok(h) => h,
        Err(e) => return reject(client_op_id, format!("geometry bake failed: {e}")),
    };

    let schema = [design_component_meta()];
    let patch = AiPatch {
        client_op_id: client_op_id.to_string(),
        ops: vec![
            set(design_entity, "height_mm", json!(design.height_m * 1000.0)),
            set(
                design_entity,
                "bore_dia_mm",
                json!(design.bore_r_m * 2000.0),
            ),
            set(design_entity, "mass_g", json!(result.mass_kg * 1000.0)),
            set(
                design_entity,
                "stress_mpa",
                json!(result.max_stress_pa / 1.0e6),
            ),
            set(
                design_entity,
                "deflection_mm",
                json!(result.tip_deflection_m * 1000.0),
            ),
            set(design_entity, "mesh", json!(handle)),
        ],
    };
    apply_ai_patch(engine, &schema, "generative-optimize", &patch)
}

/// **Apply the optimizer's chosen design as a validated, undoable patch** — the loop's closing step:
/// [`propose_design`] with the [`GenerativeRun`]'s chosen (min-mass feasible) design. If the loop found no
/// feasible design (parked), it is rejected-as-UX with a reason.
pub fn apply_optimized_design<W: World>(
    engine: &mut Engine<W>,
    store: &mut AssetStore,
    design_entity: EntityId,
    spec: &LoadSpec,
    run: &GenerativeRun,
    client_op_id: &str,
) -> ProjectionDelta {
    let Some(cand) = run.chosen_candidate() else {
        return reject(
            client_op_id,
            "the generative loop found no feasible design (parked) — loosen a budget or the geometry envelope"
                .to_string(),
        );
    };
    let design = cand.design;
    propose_design(engine, store, design_entity, spec, &design, client_op_id)
}

/// Bake a design's M15.0 SDF program to a **content-addressed, watertight** mesh (the geometry realization,
/// off the hot path). Deterministic — same design ⇒ same content address (a nice reproducibility corollary).
///
/// # Errors
/// [`SdfBakeError`] if the field doesn't compile to a watertight, manifold mesh (Blocked-explained, never a
/// silent crack).
pub fn bake_design(
    store: &mut AssetStore,
    design: &Design,
    spec: &LoadSpec,
) -> Result<String, SdfBakeError> {
    let sdf = design.to_sdf(spec);
    let grid = Grid::around(&sdf, BAKE_RES, 0.06);
    bake_sdf(store, &sdf, &grid)
}

/// Confirm a baked handle resolves to a clean watertight solid (used by the spike to assert the geometry
/// realization is crack-free, reusing the always-on M13.2 validator).
#[must_use]
pub fn baked_mesh_is_watertight(store: &AssetStore, handle: &str) -> bool {
    store.get_str(handle).is_some_and(|asset| {
        let r = validate_mesh(&mesh_asset_to_trimesh(asset));
        r.watertight && r.manifold
    })
}

fn set(entity: EntityId, field: &str, value: serde_json::Value) -> PatchOp {
    PatchOp::SetField {
        id: entity.to_loro_key(),
        component: DESIGN_COMPONENT.to_string(),
        field: field.to_string(),
        value,
    }
}

fn reject(client_op_id: &str, reason: String) -> ProjectionDelta {
    ProjectionDelta {
        ops: vec![],
        confirms: vec![],
        rejects: vec![RejectInfo {
            client_op_id: client_op_id.to_string(),
            reason,
        }],
        full: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steel_bracket() -> LoadSpec {
        parse_spec("this bracket carries 200 N down at the tip, fixed at the base — minimize mass")
            .expect("parse the canonical spec")
    }

    #[test]
    fn parse_recognizes_the_load_and_objective_and_is_honest_about_the_rest() {
        let s = steel_bracket();
        assert!((s.tip_load_n - 200.0).abs() < 1e-9);
        assert_eq!(s.material, Material::Steel);
        assert_eq!(s.objective, Objective::MinimizeMass);
        // Honest NL gap: no objective → an explained error, never a wrong silent guess.
        assert_eq!(
            parse_spec("make a 200 N bracket"),
            Err(SpecError::NoObjective)
        );
        assert_eq!(parse_spec("minimize mass please"), Err(SpecError::NoLoad));
        // A material is recognized when named.
        let al = parse_spec("carries 150 N, minimize mass, aluminum").unwrap();
        assert_eq!(al.material, Material::Aluminum);
    }

    #[test]
    fn rom_beam_solver_is_sane_and_a_solid_section_is_feasible() {
        let spec = steel_bracket();
        // A 20 mm solid section is overbuilt for a 200 N / 100 mm cantilever → feasible with margin.
        let solid = Design {
            height_m: 0.020,
            bore_r_m: 0.0,
        };
        let r = RomBeamSolver.analyze(&solid, &spec).unwrap();
        assert!(r.feasible(&spec), "a solid 20 mm section holds the load");
        assert!(r.max_stress_pa < spec.allowable_stress_pa());
        // A 5 mm solid section over-stresses (the strength constraint binds) → infeasible.
        let thin = Design {
            height_m: 0.005,
            bore_r_m: 0.0,
        };
        let rt = RomBeamSolver.analyze(&thin, &spec).unwrap();
        assert!(!rt.feasible(&spec), "a 5 mm section is over budget");
    }

    #[test]
    fn the_loop_finds_a_feasible_min_mass_design_and_is_reproducible() {
        let spec = steel_bracket();
        let a = optimize(&spec, 0xD00D, 12, &RomBeamSolver);
        let b = optimize(&spec, 0xD00D, 12, &RomBeamSolver);
        assert_eq!(a, b, "same spec + seed ⇒ identical run (bit-for-bit)");
        let chosen = a.chosen_candidate().expect("a feasible design");
        assert!(chosen.feasible);
        // The chosen design is lighter than the overbuilt solid 20 mm section (real optimization happened).
        let baseline = RomBeamSolver
            .analyze(
                &Design {
                    height_m: 0.020,
                    bore_r_m: 0.0,
                },
                &spec,
            )
            .unwrap();
        assert!(
            chosen.result.mass_kg < baseline.mass_kg,
            "the optimizer removed mass: {} < {}",
            chosen.result.mass_kg,
            baseline.mass_kg
        );
    }

    #[test]
    fn a_different_seed_keeps_the_deterministic_contract() {
        let spec = steel_bracket();
        let a = optimize(&spec, 1, 10, &RomBeamSolver);
        let b = optimize(&spec, 1, 10, &RomBeamSolver);
        assert_eq!(a.objective_hash, b.objective_hash, "seed 1 is reproducible");
        // The audit trail replays (an auditor re-derives every candidate).
        assert!(a.verify_audit(&RomBeamSolver));
    }

    #[test]
    fn the_run_is_a_file_round_trips_bit_for_bit() {
        let spec = steel_bracket();
        let run = optimize(&spec, 42, 10, &RomBeamSolver);
        let bytes = run.to_bytes();
        let loaded = GenerativeRun::from_bytes(&bytes).expect("reload the artifact");
        assert_eq!(loaded, run, "the bincode artifact round-trips bit-exactly");
        assert_eq!(
            loaded.to_bytes(),
            bytes,
            "re-serialization is byte-identical"
        );
    }

    #[test]
    fn the_precice_fmi_solver_is_a_named_seam_never_a_fake_number() {
        let spec = steel_bracket();
        let design = Design {
            height_m: 0.02,
            bore_r_m: 0.0,
        };
        match PreciceFmiSolver.analyze(&design, &spec) {
            Err(SolverError::Unavailable { solver, reason }) => {
                assert_eq!(solver, "precice-fmi");
                assert!(reason.contains("server-side"));
            }
            other => panic!("the FEA seam must be Unavailable, not a fabricated result: {other:?}"),
        }
        assert!(
            !PreciceFmiSolver.deterministic(),
            "external FEA is OUT of the guarantee"
        );
        assert!(RomBeamSolver.deterministic(), "the ROM is IN the guarantee");
    }

    #[test]
    fn ff_t7_differentiable_is_gated_not_faked() {
        assert!(
            !GradientSource::ff_t7_available(),
            "FF-T7 is a gated frontier dependency"
        );
        let spec = steel_bracket();
        let run = optimize(&spec, 7, 8, &RomBeamSolver);
        assert_eq!(
            run.gradient_source,
            GradientSource::AnalyticRom,
            "the loop uses the analytic ROM gradient, not a faked adjoint"
        );
    }

    #[test]
    fn an_over_budget_design_yields_a_certificate_not_a_copy_string() {
        let spec = steel_bracket();
        // A 5 mm solid section over-stresses → a certificate naming the busted budget.
        let bad = Design {
            height_m: 0.005,
            bore_r_m: 0.0,
        };
        let cert = design_certificate(&bad, &spec, &RomBeamSolver).expect("an over-budget cert");
        assert!(
            !cert.unsat_core.is_empty(),
            "carries an unsat-core (a derivation)"
        );
        assert!(cert.reason.contains("over budget"));
        // A feasible design → no certificate.
        let ok = Design {
            height_m: 0.020,
            bore_r_m: 0.0,
        };
        assert!(design_certificate(&ok, &spec, &RomBeamSolver).is_none());
    }

    #[test]
    fn the_chosen_design_bakes_to_a_watertight_mesh() {
        let spec = steel_bracket();
        let run = optimize(&spec, 3, 10, &RomBeamSolver);
        let cand = run.chosen_candidate().unwrap();
        let mut store = AssetStore::new();
        let h1 = bake_design(&mut store, &cand.design, &spec).expect("clean bake");
        assert!(baked_mesh_is_watertight(&store, &h1));
        // Deterministic geometry: the same design re-bakes to the same content address.
        let mut store2 = AssetStore::new();
        let h2 = bake_design(&mut store2, &cand.design, &spec).unwrap();
        assert_eq!(h1, h2, "same design ⇒ same content-addressed handle");
    }
}
