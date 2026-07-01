//! M15.5 (ADR-075) — **FMI 3.0 co-simulation, productionized behind the `Solver` trait.** Metrocalk is the
//! **deterministic orchestration master** over a validated-FEA solver: it drives a **deterministic coupling
//! schedule** (a quasi-static load ramp), captures every solver call into a **lossless, replayable audit
//! trail** ("a co-sim run is a file", the M13.1/ADR-050 discipline), and lands the validated result as a
//! **validated, undoable patch** (the M15.4 loop, reusing [`crate::generative::propose_design`]).
//!
//! **Honest scope (stated, not papered over).**
//! - The FMI 3.0 stack is **native-only** (the Rust `fmi` importer loads a native FMU; CalculiX/OpenFOAM are
//!   coupled server-side via preCICE + the preCICE-FMI runner — a C/C++/Python pipeline, not Rust). What we
//!   own + measure headless is the **deterministic master + the audit trail + the validated-patch landing**;
//!   the **live FMU / preCICE run is owed-convergence** (native/server — no C++ toolchain here, the grounded
//!   ADR-070 boundary).
//! - The external solver's result is captured into the audit trail but its **raw FP non-determinism is OUT of
//!   the reproducibility guarantee** ([`Solver::deterministic`] is `false` for it) — the ADR-074 boundary.
//! - **No foreign FMI/preCICE type crosses the [`Solver`] trait** (CI grep-gated, the same discipline as
//!   `rapier::`-in-`/physics`). A seam that isn't wired returns [`SolverError::Unavailable`] — **never a
//!   fabricated stress**.

use crate::generative::{propose_design, Design, LoadSpec, Solver, SolverError, StructuralResult};
use metrocalk_assets::AssetStore;
use metrocalk_core::{Engine, EntityId};
use metrocalk_ecs::World;
use serde::{Deserialize, Serialize};

/// The **FMI 3.0 single-FMU co-simulation seam** — import/run a native FMU (FMI 3.0.2). It is the trait
/// boundary that RESERVES the seam: **native-only**, and until the native FMU runtime is wired it returns
/// [`SolverError::Unavailable`] — it **never fabricates a stress**. When wired, its result is captured into
/// the co-sim audit trail and its raw FP non-determinism stays **OUT** of the reproducibility guarantee
/// ([`Solver::deterministic`] is `false`). Distinct from [`crate::generative::PreciceFmiSolver`] (the
/// multi-solver preCICE pipeline) — this is the single-FMU importer leg.
#[derive(Clone, Copy, Debug, Default)]
pub struct FmiSolver;

impl Solver for FmiSolver {
    fn name(&self) -> &'static str {
        "fmi-3.0"
    }
    fn deterministic(&self) -> bool {
        // A native FMU's co-sim step (its own integrator, threading, FP) is not bit-reproducible across
        // machines; its result is captured into the audit trail, never inside the reproducibility claim.
        false
    }
    fn analyze(&self, _design: &Design, _spec: &LoadSpec) -> Result<StructuralResult, SolverError> {
        Err(SolverError::Unavailable {
            solver: "fmi-3.0",
            reason: "importing/running a native FMU (FMI 3.0.2) is a native-only seam (the Rust FMI stack \
                     loads a native FMU; heavy solvers couple server-side via preCICE + the preCICE-FMI \
                     runner) — not wired in this build. The min-spec/browser path uses the ROM surrogate."
                .to_string(),
        })
    }
}

/// The **deterministic coupling schedule** the orchestration master drives — a quasi-static ramp of the
/// boundary condition (the tip load), `steps` increments from `start_fraction·load` to full `load`. Owned by
/// Metrocalk (not the external solver), so the *schedule* is bit-reproducible even when the solver is not.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoSimSchedule {
    /// The number of quasi-static load increments (≥ 1).
    pub steps: usize,
    /// The first step's load fraction (e.g. `0.25` → ramp from 25% to 100%).
    pub start_fraction: f64,
}

impl Default for CoSimSchedule {
    fn default() -> Self {
        Self {
            steps: 4,
            start_fraction: 0.25,
        }
    }
}

impl CoSimSchedule {
    /// The load fraction at step `k` (0-based) — a deterministic linear ramp to 1.0.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    fn fraction(&self, k: usize) -> f64 {
        let steps = self.steps.max(1);
        if steps == 1 {
            return 1.0;
        }
        let t = (k as f64) / ((steps - 1) as f64);
        self.start_fraction + (1.0 - self.start_fraction) * t
    }
}

/// One captured co-sim step — the coupled boundary condition + the solver's structural result. Plain data
/// (bincode-lossless, so the audit round-trips + compares bit-exactly).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoSimStep {
    /// The load fraction applied at this coupling step.
    pub load_fraction: f64,
    /// The tip load (N) at this step.
    pub tip_load_n: f64,
    /// The solver's structural result at this step.
    pub result: StructuralResult,
}

/// A **co-simulation run** — the complete, replayable audit trail: the design, the spec, the schedule, the
/// solver, whether it is deterministic (in the reproducibility guarantee), every coupled step, and a
/// **bit-reproducible audit hash**. Serialized **LOSSLESS via bincode** ("a co-sim run is a file", the
/// M13.1/ADR-050 discipline — a JSON float round-trip would drift a load by 1 ULP). Same `(design, spec,
/// schedule)` + a deterministic solver ⇒ identical bytes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CoSimRun {
    /// The design under load.
    pub design: Design,
    /// The load spec (the audit provenance — what was asked).
    pub spec: LoadSpec,
    /// The deterministic coupling schedule the master drove.
    pub schedule: CoSimSchedule,
    /// The solver's name (the ROM here; a server-side FEA would be captured too).
    pub solver: String,
    /// Whether the solver is deterministic (in the reproducibility guarantee).
    pub deterministic: bool,
    /// Every coupled step, in schedule order (the audit trail).
    pub steps: Vec<CoSimStep>,
    /// Whether the design is feasible at full load (the co-sim verdict).
    pub feasible_at_full: bool,
    /// A hash of the exact step bytes — the reproducible audit witness (meaningful for a deterministic solver).
    pub audit_hash: String,
}

impl CoSimRun {
    /// The **LOSSLESS bincode artifact** — "a co-sim run is a file". Ship it and it replays the exact trail.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        bincode::serialize(self).expect("a co-sim run serializes (pure data)")
    }

    /// Reload a "co-sim run is a file" artifact.
    ///
    /// # Errors
    /// The bincode error if the artifact is malformed / out of format.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(bytes)
    }

    /// **Replay the audit trail** — re-drive every coupling step through `solver` and confirm each captured
    /// result matches bit-for-bit. Meaningful for a deterministic solver (the ROM); an external FEA's raw FP
    /// is out of the guarantee, so `verify_audit` reflects the honest `deterministic` flag.
    #[must_use]
    pub fn verify_audit<S: Solver>(&self, solver: &S) -> bool {
        if !solver.deterministic() {
            return false; // an out-of-guarantee solver is not bit-replayable — stated, not faked
        }
        self.steps.iter().all(|step| {
            let scaled = scale_load(&self.spec, step.load_fraction);
            solver
                .analyze(&self.design, &scaled)
                .is_ok_and(|r| r == step.result)
        })
    }
}

/// A copy of `spec` with the tip load scaled by `fraction` (the coupling boundary condition at a step).
fn scale_load(spec: &LoadSpec, fraction: f64) -> LoadSpec {
    let mut s = spec.clone();
    s.tip_load_n = spec.tip_load_n * fraction;
    s
}

/// A tiny deterministic byte hash (FNV-1a, 64-bit) → hex — the reproducible audit witness.
fn hash_bytes(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// **Orchestrate a co-simulation** — the deterministic master drives `solver` over the coupling `schedule`
/// and captures every step into a replayable [`CoSimRun`]. The *schedule* is bit-reproducible (owned by us);
/// the *solver's* determinism is recorded honestly. If the solver is **[`SolverError::Unavailable`]** (a
/// server-side seam not wired) the co-sim returns that error — **it never fabricates a stress**; a degenerate
/// design likewise propagates.
///
/// # Errors
/// [`SolverError::Unavailable`] if the solver seam isn't wired; [`SolverError::Degenerate`] for an
/// inadmissible section.
pub fn co_simulate<S: Solver>(
    design: &Design,
    spec: &LoadSpec,
    schedule: &CoSimSchedule,
    solver: &S,
) -> Result<CoSimRun, SolverError> {
    let mut steps = Vec::with_capacity(schedule.steps.max(1));
    for k in 0..schedule.steps.max(1) {
        let fraction = schedule.fraction(k);
        let scaled = scale_load(spec, fraction);
        let result = solver.analyze(design, &scaled)?; // Unavailable / Degenerate propagate — never faked
        steps.push(CoSimStep {
            load_fraction: fraction,
            tip_load_n: scaled.tip_load_n,
            result,
        });
    }
    let feasible_at_full = steps.last().is_some_and(|s| s.result.feasible(spec));
    let audit_hash = hash_bytes(&bincode::serialize(&steps).unwrap_or_default());
    Ok(CoSimRun {
        design: *design,
        spec: spec.clone(),
        schedule: *schedule,
        solver: solver.name().to_string(),
        deterministic: solver.deterministic(),
        steps,
        feasible_at_full,
        audit_hash,
    })
}

/// **Land the co-sim's validated design as a validated, undoable, auditable patch** — the closing step of the
/// M15.4 loop applied to a co-simulation. If the design is feasible at full load, apply it through the shipped
/// [`propose_design`] contract (a schema-validated, undoable `apply_ai_patch` — geometry realized as a
/// content-addressed watertight mesh). If it is NOT feasible at full load, it is **rejected-as-UX** (nothing
/// applied) — never a silent bad part.
pub fn land_cosim<W: World>(
    engine: &mut Engine<W>,
    store: &mut AssetStore,
    design_entity: EntityId,
    run: &CoSimRun,
    client_op_id: &str,
) -> crate::ProjectionDelta {
    if !run.feasible_at_full {
        return crate::ProjectionDelta {
            ops: vec![],
            confirms: vec![],
            rejects: vec![crate::bridge::RejectInfo {
                client_op_id: client_op_id.to_string(),
                reason: "the co-simulated design is not feasible at full load — not landed (thicken the \
                         section / reduce the bore, or integrate a validated FEA to certify accuracy)"
                    .to_string(),
            }],
            full: false,
        };
    }
    propose_design(
        engine,
        store,
        design_entity,
        &run.spec,
        &run.design,
        client_op_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generative::{parse_spec, Design, RomBeamSolver};

    fn demo() -> (Design, LoadSpec) {
        let spec = parse_spec(
            "this bracket carries 200 N down at the tip, fixed at the base — minimize mass",
        )
        .expect("parse");
        // A feasible section (a solid-ish 20 mm design comfortably within budget under 200 N).
        let design = Design {
            height_m: 0.020,
            bore_r_m: 0.004,
        };
        (design, spec)
    }

    #[test]
    fn co_sim_over_the_rom_is_deterministic_and_auditable() {
        let (design, spec) = demo();
        let schedule = CoSimSchedule::default();
        let a = co_simulate(&design, &spec, &schedule, &RomBeamSolver).unwrap();
        let b = co_simulate(&design, &spec, &schedule, &RomBeamSolver).unwrap();
        // Bit-reproducible: identical audit bytes across runs (a deterministic master + a deterministic ROM).
        assert_eq!(
            a.to_bytes(),
            b.to_bytes(),
            "co-sim audit is bit-reproducible"
        );
        assert_eq!(a.audit_hash, b.audit_hash);
        assert!(
            a.deterministic,
            "the ROM is in the reproducibility guarantee"
        );
        assert_eq!(
            a.steps.len(),
            schedule.steps,
            "one captured step per increment"
        );
        // The last step is at full load.
        assert!((a.steps.last().unwrap().load_fraction - 1.0).abs() < 1e-12);
        // The audit is REPLAYABLE (re-derive, don't trust the recorded numbers).
        assert!(
            a.verify_audit(&RomBeamSolver),
            "the co-sim audit replays bit-for-bit"
        );
        // "A co-sim run is a file": round-trips losslessly.
        assert_eq!(CoSimRun::from_bytes(&a.to_bytes()).unwrap(), a);
    }

    #[test]
    fn the_fmi_seam_is_unavailable_never_a_fabricated_stress() {
        let (design, spec) = demo();
        // The single-FMU seam is Unavailable (native-only, not wired) — never a fake number.
        assert!(matches!(
            FmiSolver.analyze(&design, &spec),
            Err(SolverError::Unavailable {
                solver: "fmi-3.0",
                ..
            })
        ));
        assert!(
            !FmiSolver.deterministic(),
            "external FEA is OUT of the guarantee"
        );
        // A co-sim over the unwired seam returns Unavailable — the master never fabricates a result.
        assert!(matches!(
            co_simulate(&design, &spec, &CoSimSchedule::default(), &FmiSolver),
            Err(SolverError::Unavailable { .. })
        ));
        // An out-of-guarantee solver is not bit-replayable — verify_audit is honest about that.
        let run = co_simulate(&design, &spec, &CoSimSchedule::default(), &RomBeamSolver).unwrap();
        assert!(
            !run.verify_audit(&FmiSolver),
            "an external solver can't replay the deterministic audit"
        );
    }

    #[test]
    fn a_feasible_co_sim_lands_as_a_validated_undoable_patch() {
        use metrocalk_core::Engine;
        use metrocalk_ecs::FlecsWorld;

        let (design, spec) = demo();
        let run = co_simulate(&design, &spec, &CoSimSchedule::default(), &RomBeamSolver).unwrap();
        assert!(
            run.feasible_at_full,
            "the demo design is feasible at full load"
        );

        let mut engine = Engine::new(FlecsWorld::new(), 1);
        let mut store = AssetStore::new();
        let ent =
            crate::generative::place_design_seed(&mut engine, [0.0, 0.0, 0.0], &spec).unwrap();

        let delta = land_cosim(&mut engine, &mut store, ent, &run, "cosim-1");
        assert_eq!(
            delta.confirms,
            vec!["cosim-1".to_string()],
            "landed as a validated patch"
        );
        assert!(delta.rejects.is_empty());
        // One undoable transaction: Ctrl-Z peels the co-sim result.
        assert!(engine.undo(), "the co-sim landing is undoable");
    }

    #[test]
    fn an_infeasible_co_sim_is_rejected_never_landed() {
        use metrocalk_core::Engine;
        use metrocalk_ecs::FlecsWorld;

        let spec = parse_spec("this bracket carries 200 N down at the tip — minimize mass")
            .expect("parse");
        // A too-thin section over-stresses at full load → infeasible.
        let design = Design {
            height_m: 0.005,
            bore_r_m: 0.0,
        };
        let run = co_simulate(&design, &spec, &CoSimSchedule::default(), &RomBeamSolver).unwrap();
        assert!(
            !run.feasible_at_full,
            "a 5 mm section over-stresses at 200 N"
        );

        let mut engine = Engine::new(FlecsWorld::new(), 1);
        let mut store = AssetStore::new();
        let ent =
            crate::generative::place_design_seed(&mut engine, [0.0, 0.0, 0.0], &spec).unwrap();
        let delta = land_cosim(&mut engine, &mut store, ent, &run, "cosim-2");
        assert!(
            delta.confirms.is_empty() && delta.rejects.len() == 1,
            "rejected-as-UX, nothing landed"
        );
    }
}
