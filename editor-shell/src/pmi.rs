//! M15.3 (ADR-073) — **semantic PMI / MBD / GD&T as relational-ECS-native**, on imported B-rep.
//!
//! "Precision" in CAD ultimately means **tolerances** — GD&T (ASME Y14.5 / ISO GPS: datums,
//! feature-control-frames, true-position). Semantic PMI (machine-readable, vs a graphical annotation) closes
//! the digital thread to CAM/CMM; MBD makes the 3D model the authoritative spec. **Here semantic PMI is the
//! DEFAULT, not an export:** a feature-control-frame is a **typed relationship** on a real imported B-rep
//! feature (the M15.0 [`crate::cad_intent`] `CadFace`/`CadEdge` entities) — `{feature, characteristic, value,
//! datum, standard}` as structured ECS data, **machine-readable by construction**. There is **no
//! graphical-only annotation path**: an [`Fcf`] is built from a closed [`Characteristic`] enum + a numeric
//! zone + a datum entity + a [`Standard`], never a free-text label — you *structurally cannot* store "a
//! picture of a callout".
//!
//! Each PMI element is **one undoable op with provenance** (the op-stream — inv. 1/3); concurrent PMI edits
//! merge clobber-free (Loro, inv. 1) with merge-validation (inv. 3). A tolerance **stack-up** is analysed
//! worst-case + RSS + a **deterministic, seedable Monte-Carlo** (the M13.1/ADR-050 seeded-`splitmix64` +
//! canonical-result discipline, reimplemented locally — not the rapier-coupled `/dst` crate, the M15.2
//! precedent): same seed → the same result, bit-for-bit. A failing stack-up renders as a **derivation
//! certificate** ([`StackupCertificate`]) reusing the shipped [`metrocalk_authoring::Certificate`] (the
//! M13.5/ADR-054 unsat-core seed that "ties M13.9") — *which* features contribute, *which* stage, *the* fix
//! — a trace, not a copy string, off the per-frame hot path. AI-assisted GD&T is a **validated patch** via
//! the shipped [`crate::ai::apply_ai_patch`] contract (ADR-048/017), rejected-as-UX on overreach.
//!
//! **Honest scope (stated, not papered over):**
//! - **PMI gates on the M15.0 imported B-rep** — a datum is a *face/axis*, an FCF references *geometry*; an
//!   [`Fcf`] only attaches to a real `CadFace`/`CadEdge` entity ([`is_cad_feature`]).
//! - **A declared GD&T SUBSET**, not full ASME Y14.5. Shipped: the form + orientation + location
//!   characteristics in [`Characteristic`], each semantic + traced, on a single datum. **Named future**
//!   (not claimed): Rule#1 (envelope), MMC/LMC material-condition modifiers, composite tolerances, the full
//!   datum-reference-frame algebra.
//! - **Deterministic Monte-Carlo is REPRODUCIBILITY, not validated metrology** — a bit-reproducible analysis
//!   (the genuine M13.1 differentiator), *not* a calibrated CMM / certified tolerance study.
//! - **The full M13.9 semiring-provenance theorem (ADR-061) is a NAMED FUTURE** — reserved, not built. This
//!   reuses its shipped *seed* (the M13.5 `Certificate`/unsat-core), not a new explainer.
//! - **Graphical-PMI interop fidelity** (round-tripping semantic PMI through STEP AP242 so it stays
//!   semantic) is the **M15.5** concern — a named seam, measured there, not claimed here.

use crate::ai::{apply_ai_patch, AiPatch, PatchOp};
use crate::bridge::{ProjectionDelta, RejectInfo};
use metrocalk_authoring::Certificate;
use metrocalk_core::registry::{ComponentMeta, FieldType};
use metrocalk_core::{Engine, EntityId, FieldValue, Op, PipelineError};
use metrocalk_ecs::World;
use serde_json::json;

/// The registered PMI component name (the [`crate::ai::apply_ai_patch`] schema key + the ECS component the
/// FCF entity carries). One component; the semantics live in its typed fields.
pub const FCF_COMPONENT: &str = "FeatureControlFrame";

/// The Cpk feasibility epsilon — absorbs f64 rounding so a spec exactly at 3σ reads as Cpk = 1.0 (feasible),
/// not a false infeasible (`3.0 * 0.05` is `0.15000000000000002`, not exactly `0.15`).
const CPK_EPS: f64 = 1e-9;

// ── the typed GD&T vocabulary (a CLOSED enum — no graphical / free-text path is representable) ────────────

/// A GD&T **geometric characteristic** (the ASME Y14.5 / ISO GPS symbol families). A **closed enum**: an FCF
/// is built from one of these + a numeric zone, so a "graphical-only" annotation (an arbitrary label / a
/// drawn callout) is **not representable** — PMI is machine-readable by construction. A **declared subset**
/// (form + orientation + location on a single datum); the material-condition modifiers / composite frames /
/// full datum-reference-frame algebra are the named-future tail (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Characteristic {
    // Form (datumless — reference no datum).
    /// Flatness — a surface within two parallel planes `value` apart.
    Flatness,
    /// Straightness — a line element within a tolerance zone.
    Straightness,
    /// Circularity (roundness).
    Circularity,
    /// Cylindricity.
    Cylindricity,
    // Orientation (datum-referencing).
    /// Parallelism to a datum.
    Parallelism,
    /// Perpendicularity to a datum.
    Perpendicularity,
    /// Angularity to a datum.
    Angularity,
    // Location (datum-referencing).
    /// True position relative to a datum (the `∅value` positional zone).
    Position,
    /// Concentricity to a datum axis.
    Concentricity,
    /// Symmetry about a datum.
    Symmetry,
}

impl Characteristic {
    /// All shipped characteristics (the declared subset), in a stable order.
    pub const ALL: [Characteristic; 10] = [
        Characteristic::Flatness,
        Characteristic::Straightness,
        Characteristic::Circularity,
        Characteristic::Cylindricity,
        Characteristic::Parallelism,
        Characteristic::Perpendicularity,
        Characteristic::Angularity,
        Characteristic::Position,
        Characteristic::Concentricity,
        Characteristic::Symmetry,
    ];

    /// The canonical, stable storage string (what the ECS field holds — never a display label).
    #[must_use]
    pub fn canonical(self) -> &'static str {
        match self {
            Characteristic::Flatness => "flatness",
            Characteristic::Straightness => "straightness",
            Characteristic::Circularity => "circularity",
            Characteristic::Cylindricity => "cylindricity",
            Characteristic::Parallelism => "parallelism",
            Characteristic::Perpendicularity => "perpendicularity",
            Characteristic::Angularity => "angularity",
            Characteristic::Position => "position",
            Characteristic::Concentricity => "concentricity",
            Characteristic::Symmetry => "symmetry",
        }
    }

    /// Parse a stored canonical string back to the typed characteristic (the machine-readable read path —
    /// an unknown string is **not** a valid characteristic, so a graphical-only annotation is unrepresentable).
    #[must_use]
    pub fn from_canonical(s: &str) -> Option<Characteristic> {
        Characteristic::ALL.into_iter().find(|c| c.canonical() == s)
    }

    /// Whether this characteristic **references a datum** (orientation + location do; form does not). The
    /// structural rule the validator enforces — a datumless form tolerance with a datum, or a location
    /// tolerance without one, is a Blocked+explained authoring error, not a silent accept.
    #[must_use]
    pub fn needs_datum(self) -> bool {
        matches!(
            self,
            Characteristic::Parallelism
                | Characteristic::Perpendicularity
                | Characteristic::Angularity
                | Characteristic::Position
                | Characteristic::Concentricity
                | Characteristic::Symmetry
        )
    }
}

/// The tolerancing **standard** an FCF is authored under (a closed enum — cited, not free text).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standard {
    /// ASME Y14.5 (the US GD&T standard).
    AsmeY14_5,
    /// ISO GPS (the ISO geometrical-product-specification family).
    IsoGps,
}

impl Standard {
    /// The canonical storage string.
    #[must_use]
    pub fn canonical(self) -> &'static str {
        match self {
            Standard::AsmeY14_5 => "ASME_Y14.5",
            Standard::IsoGps => "ISO_GPS",
        }
    }

    /// Parse a stored canonical string back to the typed standard.
    #[must_use]
    pub fn from_canonical(s: &str) -> Option<Standard> {
        match s {
            "ASME_Y14.5" => Some(Standard::AsmeY14_5),
            "ISO_GPS" => Some(Standard::IsoGps),
            _ => None,
        }
    }
}

/// A **semantic feature-control-frame** — the typed relationship attached to an imported B-rep feature. It
/// carries the toleranced `feature` (a `CadFace`/`CadEdge` entity), the `characteristic`, the numeric
/// `tolerance_mm` zone, an optional `datum` (another feature entity), and the `standard`. There is **no
/// `label`/`graphic` field** — the type is machine-readable by construction.
#[derive(Clone, Debug, PartialEq)]
pub struct Fcf {
    /// The toleranced feature — a real imported `CadFace`/`CadEdge` entity (M15.0).
    pub feature: EntityId,
    /// The geometric characteristic (a closed enum, never a label).
    pub characteristic: Characteristic,
    /// The tolerance-zone magnitude in millimetres (a positive numeric value).
    pub tolerance_mm: f64,
    /// The datum reference — another feature entity — for orientation/location characteristics; `None` for
    /// datumless form tolerances.
    pub datum: Option<EntityId>,
    /// The standard this FCF is authored under.
    pub standard: Standard,
}

/// PMI authoring / attach errors — each Blocked+explained (ADR-016), never a silent accept or a panic.
#[derive(Clone, Debug, PartialEq)]
pub enum PmiError {
    /// A location/orientation characteristic was authored without the datum it requires.
    DatumRequired(&'static str),
    /// A datumless form characteristic was authored with a datum reference.
    DatumForbidden(&'static str),
    /// The tolerance zone is not a positive, finite value.
    NonPositiveTolerance(f64),
    /// The toleranced feature is not an imported B-rep feature (M15.0 `CadFace`/`CadEdge`).
    NotABRepFeature(String),
    /// The referenced datum is not an imported B-rep feature.
    DatumNotABRepFeature(String),
    /// The commit was rejected by the pipeline.
    Pipeline(String),
}

impl std::fmt::Display for PmiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PmiError::DatumRequired(c) => write!(
                f,
                "the {c} tolerance references a datum (orientation/location) — attach it to a datum feature"
            ),
            PmiError::DatumForbidden(c) => write!(
                f,
                "the {c} tolerance is a form control (datumless) — it must not reference a datum"
            ),
            PmiError::NonPositiveTolerance(v) => {
                write!(f, "the tolerance zone must be a positive value, got {v}")
            }
            PmiError::NotABRepFeature(e) => write!(
                f,
                "PMI attaches to an imported B-rep face/edge; entity '{e}' is not a CadFace/CadEdge (import a STEP part first — M15.0)"
            ),
            PmiError::DatumNotABRepFeature(e) => write!(
                f,
                "the datum must be an imported B-rep face/edge; entity '{e}' is not a CadFace/CadEdge"
            ),
            PmiError::Pipeline(e) => write!(f, "the FCF commit was rejected: {e}"),
        }
    }
}

impl std::error::Error for PmiError {}

impl From<PipelineError> for PmiError {
    fn from(e: PipelineError) -> Self {
        PmiError::Pipeline(e.to_string())
    }
}

/// The registered [`ComponentMeta`] for the FCF component — the schema the AI-GD&T patch
/// ([`ai_adjust_tolerance`]) validates against (reuse the ADR-048/017 contract). Kept in the shell (not
/// `/core`'s stdlib) like the rest of the M15 CAD-domain surfacing; the pipeline itself accepts the fields
/// as data regardless (as [`crate::cad_intent`] writes `CadFace`).
#[must_use]
pub fn fcf_component_meta() -> ComponentMeta {
    ComponentMeta::builder(FCF_COMPONENT)
        .category("Props")
        .field("characteristic", FieldType::String, true)
        .field("tolerance", FieldType::Number, true)
        .field_fmt("datum", FieldType::String, false, Some("entity-ref"))
        .field("standard", FieldType::String, true)
        .tag("cad")
        .tag("gdt")
        .tag("pmi")
        .ui_hint(
            "characteristic",
            "GD&T characteristic: flatness|straightness|circularity|cylindricity|parallelism|perpendicularity|angularity|position|concentricity|symmetry",
        )
        .ui_hint("tolerance", "the tolerance-zone magnitude in millimetres (> 0)")
        .ui_hint("standard", "ASME_Y14.5 | ISO_GPS")
        .build()
}

/// Whether `id` is an imported B-rep feature — carries a `CadFace.step_id` **or** a `CadEdge.step_id`
/// (produced by the M15.0 STEP import, [`crate::cad_intent::import_step`]). PMI **gates** on this: a
/// tolerance attaches to real geometry, never floats free.
pub fn is_cad_feature<W: World>(engine: &Engine<W>, id: EntityId) -> bool {
    engine.get_field(id, "CadFace", "step_id").is_some()
        || engine.get_field(id, "CadEdge", "step_id").is_some()
}

/// Validate an FCF's typed invariants (datum rule + positive zone) — Blocked+explained, no World needed.
///
/// # Errors
/// The specific [`PmiError`] (datum required/forbidden, or a non-positive tolerance).
pub fn validate_fcf(fcf: &Fcf) -> Result<(), PmiError> {
    if !(fcf.tolerance_mm.is_finite() && fcf.tolerance_mm > 0.0) {
        return Err(PmiError::NonPositiveTolerance(fcf.tolerance_mm));
    }
    let name = fcf.characteristic.canonical();
    match (fcf.characteristic.needs_datum(), fcf.datum.is_some()) {
        (true, false) => Err(PmiError::DatumRequired(name)),
        (false, true) => Err(PmiError::DatumForbidden(name)),
        _ => Ok(()),
    }
}

/// **Attach a semantic FCF to an imported B-rep feature as ONE undoable transaction** (inv. 3). The FCF is a
/// child entity of the toleranced feature (the parent relationship = "applies to") carrying the typed
/// [`FCF_COMPONENT`] fields; one Ctrl-Z peels it. The op-log entry is its provenance. Gates on the M15.0
/// geometry: the feature (and any datum) must be a real `CadFace`/`CadEdge` ([`is_cad_feature`]).
///
/// # Errors
/// A [`PmiError`] if the FCF is invalid, the feature/datum isn't imported B-rep, or the commit is rejected.
pub fn attach_fcf<W: World>(engine: &mut Engine<W>, fcf: &Fcf) -> Result<EntityId, PmiError> {
    validate_fcf(fcf)?;
    if !is_cad_feature(engine, fcf.feature) {
        return Err(PmiError::NotABRepFeature(fcf.feature.to_loro_key()));
    }
    if let Some(d) = fcf.datum {
        if !is_cad_feature(engine, d) {
            return Err(PmiError::DatumNotABRepFeature(d.to_loro_key()));
        }
    }

    let id = engine.alloc_entity_id();
    let datum_key = fcf.datum.map(|d| d.to_loro_key()).unwrap_or_default();
    engine.commit(
        "attach-fcf",
        vec![
            Op::CreateEntity {
                id,
                parent: Some(fcf.feature),
            },
            Op::SetField {
                entity: id,
                component: FCF_COMPONENT.into(),
                field: "characteristic".into(),
                value: FieldValue::Str(fcf.characteristic.canonical().into()),
            },
            Op::SetField {
                entity: id,
                component: FCF_COMPONENT.into(),
                field: "tolerance".into(),
                value: FieldValue::Number(fcf.tolerance_mm),
            },
            Op::SetField {
                entity: id,
                component: FCF_COMPONENT.into(),
                field: "datum".into(),
                value: FieldValue::Str(datum_key),
            },
            Op::SetField {
                entity: id,
                component: FCF_COMPONENT.into(),
                field: "standard".into(),
                value: FieldValue::Str(fcf.standard.canonical().into()),
            },
        ],
    )?;
    Ok(id)
}

/// **Read a semantic FCF back as structured data** — the machine-readable-by-construction query: reconstruct
/// `{feature, characteristic, value, datum, standard}` from the typed fields, **not** by parsing a label.
/// Returns `None` if `fcf_entity` doesn't carry a valid FCF (unknown characteristic/standard → not an FCF).
#[must_use]
#[allow(clippy::cast_precision_loss)] // a tolerance stored as a whole-number Integer → f64 is exact at this scale
pub fn read_fcf<W: World>(engine: &Engine<W>, fcf_entity: EntityId) -> Option<Fcf> {
    let feature = engine.parent_of(fcf_entity)?;
    let characteristic = match engine.get_field(fcf_entity, FCF_COMPONENT, "characteristic")? {
        FieldValue::Str(s) => Characteristic::from_canonical(&s)?,
        _ => return None,
    };
    let tolerance_mm = match engine.get_field(fcf_entity, FCF_COMPONENT, "tolerance")? {
        FieldValue::Number(n) => n,
        FieldValue::Integer(i) => i as f64,
        _ => return None,
    };
    let standard = match engine.get_field(fcf_entity, FCF_COMPONENT, "standard")? {
        FieldValue::Str(s) => Standard::from_canonical(&s)?,
        _ => return None,
    };
    let datum = match engine.get_field(fcf_entity, FCF_COMPONENT, "datum") {
        Some(FieldValue::Str(s)) if !s.is_empty() => EntityId::from_loro_key(&s),
        _ => None,
    };
    Some(Fcf {
        feature,
        characteristic,
        tolerance_mm,
        datum,
        standard,
    })
}

/// Query every FCF **attached to** `feature` — the relational read (its child entities carrying
/// [`FCF_COMPONENT`]), sorted for a deterministic order.
#[must_use]
pub fn fcfs_on<W: World>(engine: &Engine<W>, feature: EntityId) -> Vec<EntityId> {
    let mut out: Vec<EntityId> = engine
        .entity_ids()
        .into_iter()
        .filter(|&e| {
            engine.parent_of(e) == Some(feature)
                && engine
                    .get_field(e, FCF_COMPONENT, "characteristic")
                    .is_some()
        })
        .collect();
    out.sort();
    out
}

// ── AI-assisted GD&T = a validated patch (reuse the ADR-048/017 contract, no raw annotation) ──────────────

/// **AI-assisted GD&T as a validated patch.** An AI-proposed tolerance change (e.g. "loosen the position to
/// hit 99.7% yield") is applied to an existing FCF through the shipped [`apply_ai_patch`] contract — a
/// schema-validated, undoable, single transaction — **never a raw annotation**. Overreach is
/// rejected-as-UX: a non-existent FCF, a wrong-typed value, or a non-positive tolerance returns a rejection
/// (nothing applied). The domain rule (tolerance > 0) is pre-checked so a bad AI value is rejected before it
/// reaches the pipeline.
pub fn ai_adjust_tolerance<W: World>(
    engine: &mut Engine<W>,
    fcf_entity: EntityId,
    new_tolerance_mm: f64,
    client_op_id: &str,
) -> ProjectionDelta {
    // Domain guard: an AI must not set a non-positive/NaN tolerance (rejected-as-UX, nothing applied).
    if !(new_tolerance_mm.is_finite() && new_tolerance_mm > 0.0) {
        return ProjectionDelta {
            ops: vec![],
            confirms: vec![],
            rejects: vec![RejectInfo {
                client_op_id: client_op_id.to_string(),
                reason: format!(
                    "a GD&T tolerance must be a positive value, got {new_tolerance_mm}"
                ),
            }],
            full: false,
        };
    }
    let schema = [fcf_component_meta()];
    let patch = AiPatch {
        client_op_id: client_op_id.to_string(),
        ops: vec![PatchOp::SetField {
            id: fcf_entity.to_loro_key(),
            component: FCF_COMPONENT.to_string(),
            field: "tolerance".to_string(),
            value: json!(new_tolerance_mm),
        }],
    };
    apply_ai_patch(engine, &schema, "ai-gdt-tolerance", &patch)
}

// ── deterministic, seedable Monte-Carlo tolerance stack-up ───────────────────────────────────────────────

/// A seed-driven deterministic RNG (`splitmix64`) — the **injected** randomness source for the Monte-Carlo,
/// reimplemented locally (the M13.1/ADR-050 discipline; not the rapier-coupled `/dst` crate — the M15.2
/// precedent). Pure integer math ⇒ no FP/ISA variance in the draws ⇒ same seed → same samples, bit-for-bit.
#[derive(Clone, Debug)]
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

    /// A reproducible f64 in [0, 1) — the 53 high bits (pure integer math + one exact division).
    #[allow(clippy::cast_precision_loss)]
    fn next_f64(&mut self) -> f64 {
        let num = (self.next_u64() >> 11) as f64;
        num / 9_007_199_254_740_992.0 // 2^53
    }

    /// A standard-normal draw via the **Irwin-Hall** sum-of-12-uniforms approximation — deliberately using
    /// **only** IEEE-754 add/div (no `ln`/`cos`/`sin` transcendentals, unlike Box-Muller): the draw stays
    /// exactly reproducible and cross-platform-clean (the M15.0 no-transcendentals discipline). Mean 0,
    /// variance 1 (sum of 12 U(0,1) has mean 6, variance 1).
    fn next_normal(&mut self) -> f64 {
        let mut s = 0.0;
        for _ in 0..12 {
            s += self.next_f64();
        }
        s - 6.0
    }
}

/// One contributor to a tolerance stack-up chain — a toleranced feature with its **spec** tolerance and the
/// **process capability** (`process_sigma_mm`, the achievable manufacturing σ — distinct from the spec).
/// `direction` is its ±1 sign in the assembly chain (some dimensions add to the gap, some subtract).
#[derive(Clone, Debug)]
pub struct Contributor {
    /// A human/trace label for the feature (e.g. the STEP `#id` or the part name).
    pub feature: String,
    /// The controlled characteristic.
    pub characteristic: Characteristic,
    /// The nominal dimension (mm).
    pub nominal_mm: f64,
    /// The spec tolerance ± (mm) — the design allowance.
    pub tolerance_mm: f64,
    /// The achievable process standard deviation (mm) — the manufacturing reality (Cpk = tol / 3σ).
    pub process_sigma_mm: f64,
    /// The ±1 direction of this contributor in the assembly chain.
    pub direction: f64,
}

impl Contributor {
    /// The **process capability index** Cpk = spec / 3σ. `< 1.0` means the spec is tighter than the process
    /// can hold at ±3σ (99.73%) — an infeasible-to-manufacture tolerance.
    #[must_use]
    pub fn cpk(&self) -> f64 {
        if self.process_sigma_mm <= 0.0 {
            return f64::INFINITY;
        }
        self.tolerance_mm / (3.0 * self.process_sigma_mm)
    }

    /// The loosest spec that still holds Cpk = 1.0 (= 3σ) — the "loosen to" recommendation.
    #[must_use]
    pub fn cpk1_tolerance_mm(&self) -> f64 {
        3.0 * self.process_sigma_mm
    }
}

/// A tolerance **stack-up**: a chain of contributors and the assembly-gap requirement window
/// `[gap_min_mm, gap_max_mm]` around `gap_nominal_mm`, plus the `target_yield` (e.g. 0.997 = ±3σ).
#[derive(Clone, Debug)]
pub struct Stackup {
    /// A name for the stack-up (the trace title).
    pub name: String,
    /// The contributing features.
    pub contributors: Vec<Contributor>,
    /// The nominal assembly gap (mm).
    pub gap_nominal_mm: f64,
    /// The minimum acceptable gap (mm).
    pub gap_min_mm: f64,
    /// The maximum acceptable gap (mm).
    pub gap_max_mm: f64,
    /// The required assembly yield fraction (e.g. 0.997).
    pub target_yield: f64,
}

/// The result of a Monte-Carlo run — **integer** counts so the reproducibility assertion is exact (no float
/// round-trip; the M13.1 canonical-result discipline). `pass`/`samples` fully determine the yield.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct McResult {
    /// The seed the run used (part of the canonical, reproducible result).
    pub seed: u64,
    /// The number of Monte-Carlo samples.
    pub samples: usize,
    /// How many samples fell within the assembly-gap window.
    pub pass: usize,
}

impl McResult {
    /// The measured yield fraction (`pass / samples`) — deterministic (integer ratio).
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn yield_fraction(&self) -> f64 {
        if self.samples == 0 {
            return 0.0;
        }
        self.pass as f64 / self.samples as f64
    }
}

impl Stackup {
    /// The **worst-case** stack — the arithmetic sum of the spec tolerances (the guaranteed bound).
    #[must_use]
    pub fn worst_case_mm(&self) -> f64 {
        self.contributors.iter().map(|c| c.tolerance_mm.abs()).sum()
    }

    /// The **RSS** (root-sum-square) stack — the statistical bound (√Σ tol²). Uses `sqrt` only (an
    /// IEEE-754 correctly-rounded op — deterministic, unlike the transcendentals).
    #[must_use]
    pub fn rss_mm(&self) -> f64 {
        self.contributors
            .iter()
            .map(|c| c.tolerance_mm * c.tolerance_mm)
            .sum::<f64>()
            .sqrt()
    }

    /// A **deterministic, seedable Monte-Carlo** of the assembly gap: each contributor's actual deviation is
    /// drawn `N(0, process_sigma)` (the manufacturing reality), the gap is `gap_nominal + Σ dir·dev`, and a
    /// sample **passes** if the gap is within `[gap_min, gap_max]`. Same `seed` + `samples` → the same
    /// `McResult`, bit-for-bit (reproducible analysis — **not** metrology).
    #[must_use]
    pub fn monte_carlo(&self, seed: u64, samples: usize) -> McResult {
        let mut rng = Rng::new(seed);
        let mut pass = 0usize;
        for _ in 0..samples {
            let mut gap = self.gap_nominal_mm;
            for c in &self.contributors {
                gap += c.direction * c.process_sigma_mm * rng.next_normal();
            }
            if gap >= self.gap_min_mm && gap <= self.gap_max_mm {
                pass += 1;
            }
        }
        McResult {
            seed,
            samples,
            pass,
        }
    }
}

/// One row of the derivation-certificate trace — a contributor's share of the stack + its manufacturability.
#[derive(Clone, Debug, PartialEq)]
pub struct Contribution {
    /// The 1-based stage in the chain (the "at which stage" the certificate names).
    pub stage: usize,
    /// The feature label.
    pub feature: String,
    /// The characteristic canonical name.
    pub characteristic: &'static str,
    /// The spec tolerance (mm).
    pub tolerance_mm: f64,
    /// The process σ (mm).
    pub process_sigma_mm: f64,
    /// The process-capability index (spec / 3σ).
    pub cpk: f64,
}

/// The suggested fix a failing stack-up's certificate carries (the "the fix" the M13.9-style trace names).
#[derive(Clone, Debug, PartialEq)]
pub struct Fix {
    /// Which feature to change.
    pub feature: String,
    /// Its current tolerance (mm).
    pub from_mm: f64,
    /// The recommended tolerance (mm).
    pub to_mm: f64,
    /// Whether the fix loosens (`true`) or tightens (`false`).
    pub loosen: bool,
    /// The plain-language rationale.
    pub rationale: String,
}

/// A **stack-up derivation certificate** — the tolerance-violation "explain every no", reusing the shipped
/// [`metrocalk_authoring::Certificate`] (the M13.5/ADR-054 unsat-core seed that "ties M13.9") as its base:
/// `base.reason` (plain language) + `base.unsat_core` (the specific failing facts). Adds the domain trace:
/// every contributor's share ([`Contribution`]), the recommended [`Fix`], and the Monte-Carlo evidence. A
/// **trace, not a copy string** — computed off the hot path (an authoring-mode action, inv. 4). The full
/// M13.9 semiring-provenance theorem (ADR-061) is a named future; this reuses its seed.
#[derive(Clone, Debug, PartialEq)]
pub struct StackupCertificate {
    /// The reused every-no base (reason + unsat-core).
    pub base: Certificate,
    /// The per-contributor trace (which features, which stage).
    pub contributions: Vec<Contribution>,
    /// The recommended fix (which feature, from → to).
    pub fix: Option<Fix>,
    /// The Monte-Carlo evidence backing the analysis (reproducible).
    pub mc: McResult,
}

/// The outcome of analysing a stack-up.
#[derive(Clone, Debug, PartialEq)]
pub enum StackupAnalysis {
    /// The stack-up holds: every tolerance is manufacturable (Cpk ≥ 1) and the assembly meets the target
    /// yield. Carries the reproducible Monte-Carlo + the worst-case/RSS bounds.
    Pass {
        /// The reproducible Monte-Carlo result.
        mc: McResult,
        /// The worst-case stack (mm).
        worst_case_mm: f64,
        /// The RSS stack (mm).
        rss_mm: f64,
    },
    /// The stack-up fails — a traced, reproducible derivation certificate.
    Fail(Box<StackupCertificate>),
}

impl Stackup {
    fn contributions(&self) -> Vec<Contribution> {
        self.contributors
            .iter()
            .enumerate()
            .map(|(i, c)| Contribution {
                stage: i + 1,
                feature: c.feature.clone(),
                characteristic: c.characteristic.canonical(),
                tolerance_mm: c.tolerance_mm,
                process_sigma_mm: c.process_sigma_mm,
                cpk: c.cpk(),
            })
            .collect()
    }

    /// **Analyse the stack-up** with a seeded Monte-Carlo — the deterministic, off-hot-path authoring action.
    /// Two honest failure modes, each a traced [`StackupCertificate`]:
    /// 1. **an infeasible tolerance** (a feature's spec is tighter than its process, Cpk < 1) → the fix is to
    ///    **loosen** it to 3σ (Cpk 1.0 → 99.73% within-spec) — the assembly yield is σ-driven, so the loosen
    ///    is free; this is the "∅0.1 → loosen to ∅0.15 = 99.7% yield" case;
    /// 2. **the assembly yield misses the target** (too much variation) → the fix is to **tighten** the
    ///    dominant contributor.
    #[must_use]
    #[allow(clippy::too_many_lines)] // one linear analysis with two explained failure modes; splitting scatters shared state
    pub fn analyze(&self, seed: u64, samples: usize) -> StackupAnalysis {
        let mc = self.monte_carlo(seed, samples);
        let contributions = self.contributions();

        // Failure mode 1: the tightest-relative-to-process (lowest-Cpk) feature, if any is infeasible.
        // CPK_EPS absorbs f64 rounding (`3.0 * 0.05` is `0.15000000000000002`, not exactly `0.15`), so a
        // spec exactly at 3σ reads as Cpk = 1.0 (feasible), not a false infeasible.
        let worst = self
            .contributors
            .iter()
            .enumerate()
            .filter(|(_, c)| c.cpk() < 1.0 - CPK_EPS)
            .min_by(|(_, a), (_, b)| {
                a.cpk()
                    .partial_cmp(&b.cpk())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        if let Some((i, c)) = worst {
            let to = c.cpk1_tolerance_mm();
            let reason = format!(
                "tolerance stack-up '{}' is not manufacturable: feature '{}' (stage {}) specifies a \u{2300}{:.3} mm {} zone, but its process (\u{3c3} = {:.3} mm) needs \u{2300}{:.3} mm for Cpk \u{2265} 1.0 (its Cpk is {:.2} < 1.0 \u{2014} scrap-heavy at target confidence)",
                self.name,
                c.feature,
                i + 1,
                c.tolerance_mm,
                c.characteristic.canonical(),
                c.process_sigma_mm,
                to,
                c.cpk(),
            );
            let unsat_core = vec![
                format!(
                    "Cpk('{}' @ stage {}) = {:.2} < 1.0",
                    c.feature,
                    i + 1,
                    c.cpk()
                ),
                format!(
                    "spec \u{2300}{:.3} < 3\u{3c3} = \u{2300}{:.3}",
                    c.tolerance_mm, to
                ),
            ];
            let fix = Fix {
                feature: c.feature.clone(),
                from_mm: c.tolerance_mm,
                to_mm: to,
                loosen: to > c.tolerance_mm,
                rationale: format!(
                    "loosen '{}' from \u{2300}{:.3} to \u{2300}{:.3} mm (3\u{3c3}, Cpk 1.0 \u{2192} 99.73% within-spec); the assembly gap still yields {:.2}% (\u{3c3}-driven, unchanged by the spec loosening)",
                    c.feature,
                    c.tolerance_mm,
                    to,
                    100.0 * mc.yield_fraction()
                ),
            };
            return StackupAnalysis::Fail(Box::new(StackupCertificate {
                base: Certificate { reason, unsat_core },
                contributions,
                fix: Some(fix),
                mc,
            }));
        }

        // Failure mode 2: manufacturable, but the assembly yield misses the target.
        if mc.yield_fraction() < self.target_yield {
            // The dominant contributor to the gap variance (largest σ² share) — the one to tighten.
            let dom = self
                .contributors
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| {
                    let va = a.process_sigma_mm * a.process_sigma_mm;
                    let vb = b.process_sigma_mm * b.process_sigma_mm;
                    va.partial_cmp(&vb).unwrap_or(std::cmp::Ordering::Equal)
                });
            let (fix, extra) = if let Some((i, c)) = dom {
                let to = c.tolerance_mm * 0.5;
                (
                    Some(Fix {
                        feature: c.feature.clone(),
                        from_mm: c.tolerance_mm,
                        to_mm: to,
                        loosen: false,
                        rationale: format!(
                            "tighten '{}' (stage {}, the dominant \u{3c3} = {:.3} mm) from \u{2300}{:.3} to \u{2300}{:.3} mm to raise the assembly yield toward {:.1}%",
                            c.feature,
                            i + 1,
                            c.process_sigma_mm,
                            c.tolerance_mm,
                            to,
                            100.0 * self.target_yield
                        ),
                    }),
                    format!(
                        "dominant contributor '{}' @ stage {} (\u{3c3}\u{b2} share)",
                        c.feature,
                        i + 1
                    ),
                )
            } else {
                (None, "no contributors".to_string())
            };
            let reason = format!(
                "tolerance stack-up '{}' misses the yield target: the assembly gap yields {:.1}% (< {:.1}% required) \u{2014} too much accumulated variation",
                self.name,
                100.0 * mc.yield_fraction(),
                100.0 * self.target_yield
            );
            let unsat_core = vec![
                format!(
                    "assembly yield {:.1}% < target {:.1}%",
                    100.0 * mc.yield_fraction(),
                    100.0 * self.target_yield
                ),
                extra,
            ];
            return StackupAnalysis::Fail(Box::new(StackupCertificate {
                base: Certificate { reason, unsat_core },
                contributions,
                fix,
                mc,
            }));
        }

        StackupAnalysis::Pass {
            mc,
            worst_case_mm: self.worst_case_mm(),
            rss_mm: self.rss_mm(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cad_intent::import_step;
    use crate::capscene::{CapResolver, CapScene};
    use metrocalk_assets::AssetStore;
    use metrocalk_ecs::FlecsWorld;
    use metrocalk_interchange::{CadInterchange, StepInterchange};

    const CUBE_STEP: &str = include_str!("../../interchange/tests/fixtures/cube_ap242.step");

    fn engine_with_import() -> (Engine<FlecsWorld>, super::super::cad_intent::StepImport) {
        let mut world = FlecsWorld::new();
        let scene = CapScene::intern(&mut world);
        let mut engine = Engine::new(world, 1);
        engine.set_capability_resolver(Box::new(CapResolver::from_scene(&scene)));
        let mut store = AssetStore::new();
        let cad = StepInterchange
            .import(CUBE_STEP.as_bytes())
            .expect("import");
        let imported = import_step(&mut engine, &scene, &mut store, &cad).expect("map");
        (engine, imported)
    }

    #[test]
    fn characteristic_round_trips_and_the_datum_rule_is_typed() {
        for c in Characteristic::ALL {
            assert_eq!(Characteristic::from_canonical(c.canonical()), Some(c));
        }
        assert!(Characteristic::from_canonical("a-drawn-callout").is_none());
        assert!(Characteristic::Position.needs_datum());
        assert!(!Characteristic::Flatness.needs_datum());
    }

    #[test]
    fn validate_fcf_enforces_datum_and_positive_zone() {
        let f = |ch, tol, datum| Fcf {
            feature: EntityId {
                peer: 1,
                counter: 0,
            },
            characteristic: ch,
            tolerance_mm: tol,
            datum,
            standard: Standard::AsmeY14_5,
        };
        let d = Some(EntityId {
            peer: 1,
            counter: 9,
        });
        assert_eq!(
            validate_fcf(&f(Characteristic::Flatness, 0.02, None)),
            Ok(())
        );
        assert_eq!(validate_fcf(&f(Characteristic::Position, 0.1, d)), Ok(()));
        assert_eq!(
            validate_fcf(&f(Characteristic::Position, 0.1, None)),
            Err(PmiError::DatumRequired("position"))
        );
        assert_eq!(
            validate_fcf(&f(Characteristic::Flatness, 0.02, d)),
            Err(PmiError::DatumForbidden("flatness"))
        );
        assert_eq!(
            validate_fcf(&f(Characteristic::Flatness, 0.0, None)),
            Err(PmiError::NonPositiveTolerance(0.0))
        );
    }

    #[test]
    fn attach_read_query_and_undo_on_a_real_brep_face() {
        let (mut engine, imported) = engine_with_import();
        let face = imported.faces[0];
        let datum = imported.faces[1];

        let fcf = Fcf {
            feature: face,
            characteristic: Characteristic::Position,
            tolerance_mm: 0.10,
            datum: Some(datum),
            standard: Standard::AsmeY14_5,
        };
        let before = engine.entity_count();
        let id = attach_fcf(&mut engine, &fcf).expect("attach");

        // Machine-readable by construction: read the structured tuple back (not a parsed label).
        let read = read_fcf(&engine, id).expect("read");
        assert_eq!(read, fcf);
        assert_eq!(read.characteristic, Characteristic::Position);
        assert_eq!(read.datum, Some(datum));

        // Relational query: the FCF is attached to the face.
        assert_eq!(fcfs_on(&engine, face), vec![id]);
        assert!(fcfs_on(&engine, datum).is_empty());

        // One undoable transaction.
        assert!(engine.undo());
        assert_eq!(engine.entity_count(), before);
        assert!(read_fcf(&engine, id).is_none());
    }

    #[test]
    fn pmi_gates_on_imported_brep_geometry() {
        // A plain (non-CAD) entity cannot carry an FCF — PMI attaches to real B-rep.
        let mut world = FlecsWorld::new();
        let scene = CapScene::intern(&mut world);
        let mut engine = Engine::new(world, 1);
        engine.set_capability_resolver(Box::new(CapResolver::from_scene(&scene)));
        let plain = engine.alloc_entity_id();
        engine
            .commit(
                "plain",
                vec![Op::CreateEntity {
                    id: plain,
                    parent: None,
                }],
            )
            .unwrap();
        let fcf = Fcf {
            feature: plain,
            characteristic: Characteristic::Flatness,
            tolerance_mm: 0.02,
            datum: None,
            standard: Standard::IsoGps,
        };
        assert!(matches!(
            attach_fcf(&mut engine, &fcf),
            Err(PmiError::NotABRepFeature(_))
        ));
    }

    #[test]
    fn worst_case_and_rss_bounds() {
        let s = demo_stackup();
        // worst-case = Σ|tol| = 0.15+0.12+0.10+0.09 = 0.46
        assert!((s.worst_case_mm() - 0.46).abs() < 1e-9);
        // rss = √(0.15²+0.12²+0.10²+0.09²) = √0.055 ≈ 0.234520...
        assert!((s.rss_mm() - 0.055_f64.sqrt()).abs() < 1e-12);
        assert!(s.rss_mm() < s.worst_case_mm());
    }

    #[test]
    fn monte_carlo_is_deterministic_and_seedable() {
        let s = demo_stackup();
        let a = s.monte_carlo(0x00C0_FFEE, 20_000);
        let b = s.monte_carlo(0x00C0_FFEE, 20_000);
        assert_eq!(a, b, "same seed → identical result, bit-for-bit");
        // A different seed keeps the same integer contract (may differ in pass count, still exact).
        let c = s.monte_carlo(0x1234, 20_000);
        assert_eq!(c.samples, 20_000);
    }

    #[test]
    fn analyze_flags_the_infeasible_tolerance_with_a_loosening_fix() {
        let s = demo_stackup();
        match s.analyze(0x00C0_FFEE, 20_000) {
            StackupAnalysis::Fail(cert) => {
                // The trace is a certificate (unsat-core), not a copy string.
                assert!(!cert.base.unsat_core.is_empty());
                assert!(cert.base.reason.contains("stage 3"));
                let fix = cert.fix.expect("a fix");
                assert_eq!(fix.feature, "face #56 (spacer)");
                assert!(fix.loosen);
                assert!((fix.from_mm - 0.10).abs() < 1e-9);
                assert!((fix.to_mm - 0.15).abs() < 1e-9);
                // The trace names all four stages.
                assert_eq!(cert.contributions.len(), 4);
                assert_eq!(cert.contributions[2].stage, 3);
            }
            StackupAnalysis::Pass { .. } => panic!("stage-3 ∅0.10 is infeasible (Cpk 0.67)"),
        }
    }

    #[test]
    fn a_manufacturable_stackup_passes() {
        let mut s = demo_stackup();
        // Loosen the infeasible feature to 3σ (Cpk 1.0) → manufacturable; the assembly yield is high.
        s.contributors[2].tolerance_mm = 0.15;
        match s.analyze(0x00C0_FFEE, 20_000) {
            StackupAnalysis::Pass {
                mc,
                worst_case_mm,
                rss_mm,
            } => {
                assert!(mc.yield_fraction() >= s.target_yield);
                assert!(rss_mm < worst_case_mm);
            }
            StackupAnalysis::Fail(c) => panic!("should pass now: {}", c.base.reason),
        }
    }

    #[test]
    fn ai_gdt_is_a_validated_patch_and_rejects_overreach() {
        let (mut engine, imported) = engine_with_import();
        let fcf = Fcf {
            feature: imported.faces[0],
            characteristic: Characteristic::Position,
            tolerance_mm: 0.10,
            datum: Some(imported.faces[1]),
            standard: Standard::AsmeY14_5,
        };
        let id = attach_fcf(&mut engine, &fcf).unwrap();

        // A valid AI tolerance adjustment goes through apply_ai_patch as one undoable tx.
        let ok = ai_adjust_tolerance(&mut engine, id, 0.15, "op-1");
        assert_eq!(ok.confirms, vec!["op-1".to_string()]);
        assert!(ok.rejects.is_empty());
        assert!(
            (read_fcf(&engine, id).unwrap().tolerance_mm - 0.15).abs() < 1e-12,
            "the tolerance was updated"
        );

        // Overreach 1: a non-positive tolerance is rejected-as-UX (nothing applied).
        let bad = ai_adjust_tolerance(&mut engine, id, -1.0, "op-2");
        assert!(bad.confirms.is_empty());
        assert_eq!(bad.rejects.len(), 1);
        assert!((read_fcf(&engine, id).unwrap().tolerance_mm - 0.15).abs() < 1e-12);

        // Overreach 2: a non-existent FCF entity is rejected by the contract.
        let ghost = EntityId {
            peer: 99,
            counter: 99,
        };
        let bad2 = ai_adjust_tolerance(&mut engine, ghost, 0.2, "op-3");
        assert_eq!(bad2.rejects.len(), 1);
    }

    /// A 4-feature linear stack-up (a shaft-in-bore clearance). Stage 3's ∅0.10 spec is tighter than its
    /// process (σ 0.05 → Cpk 0.67) — infeasible, the "loosen to ∅0.15" case. The assembly gap (σ-driven)
    /// yields ~100%, so the loosen is free.
    fn demo_stackup() -> Stackup {
        Stackup {
            name: "shaft-in-bore clearance".to_string(),
            contributors: vec![
                Contributor {
                    feature: "face #12 (bracket seat)".to_string(),
                    characteristic: Characteristic::Position,
                    nominal_mm: 10.0,
                    tolerance_mm: 0.15,
                    process_sigma_mm: 0.05,
                    direction: -1.0,
                },
                Contributor {
                    feature: "face #34 (bolt boss)".to_string(),
                    characteristic: Characteristic::Perpendicularity,
                    nominal_mm: 3.0,
                    tolerance_mm: 0.12,
                    process_sigma_mm: 0.04,
                    direction: 1.0,
                },
                Contributor {
                    feature: "face #56 (spacer)".to_string(),
                    characteristic: Characteristic::Position,
                    nominal_mm: 5.0,
                    tolerance_mm: 0.10,
                    process_sigma_mm: 0.05,
                    direction: 1.0,
                },
                Contributor {
                    feature: "face #78 (cover)".to_string(),
                    characteristic: Characteristic::Flatness,
                    nominal_mm: 2.5,
                    tolerance_mm: 0.09,
                    process_sigma_mm: 0.03,
                    direction: -1.0,
                },
            ],
            gap_nominal_mm: 0.50,
            gap_min_mm: 0.20,
            gap_max_mm: 0.80,
            target_yield: 0.997,
        }
    }
}
