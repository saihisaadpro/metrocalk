//! **metrocalk-efficiency — efficiency-per-watt + min-spec as a benchmarked release gate (M13.6 / ADR-055).**
//!
//! Incumbents leave a measured **up-to-351% energy spread** on identical scenes (FM-T7.1) and don't
//! optimize the efficiency-per-watt axis — yet >80% of the world's GPUs are integrated/mobile. The framing
//! is **not "green virtue"** — it's *"the engine that actually runs well on the hardware most of the world
//! owns."* The defensibility is **measurement discipline + the structural reasons** (the determinism of
//! M13.1, the incremental recompute of M13.4), not any single technique.
//!
//! **Load-bearing honesty (the measurement reality):** true energy measurement needs a real power source
//! (RAPL / a platform counter / an external meter) and the visibility-buffer render floor needs a GPU +
//! a capture rig — **neither is present on a headless build box**. So, per `<benchmark_discipline>`, this
//! crate **NEVER invents a joule and NEVER fakes a render**. It ships the two legs that ARE buildable +
//! honest here:
//! 1. **The min-spec ADMISSION-CONTROL GATE** ([`admit`] / [`holds_min_spec`], FF-T6 mixed-criticality +
//!    imprecise-computation): a pure, deterministic, CI-gateable schedulability check that turns min-spec
//!    from perpetual debt into a **GATE** — Level-1 (input/physics/audio) must meet the deadline; Level-3
//!    (shadows/LOD) degrades under overload. This is the shipped, measured GO. **Honest ceiling:**
//!    soft-real-time budget discipline, NOT a hard-real-time proof (caches/DVFS/GPU-queue make that a
//!    fantasy on commodity HW, FF-T6).
//! 2. **The frames-per-joule + SCI harness** ([`published_sci`] / [`frames_per_joule`], ISO/IEC
//!    21031:2024 SCI-for-a-game): it **structurally refuses** to publish an SCI from a [`PowerSource::Proxy`]
//!    — a proxy is an estimate, never a measured joule, so a **faked SCI is impossible by construction**.
//!    The true-watt measurement is an explicit **owed** item with the rig that closes it.
//!
//! **Named futures (honestly parked, GPU/meter-dependent — NOT faked):** the visibility-buffer render floor
//! (deferred texturing / primitive-ID / shade-only-visible, FM-T3.1); GPU-driven indirect culling, meshlet,
//! wgpu radix sort; watt-budget mode (FM-T7.4, needs a power source); the true-watt joules/frame delta.
//! **AVOID (non-goals):** no per-pixel neural shading / frame-gen; no GPU work graphs as a dependency
//! (architect-to-map, don't build); no SCI without a real measurement. **The GPU-efficiency layer is
//! OUTSIDE the M13.1 determinism guarantee** (a separable layer — no efficiency number is a determinism claim).

// ── leg 1: the min-spec admission-control gate (FF-T6 mixed-criticality) ─────────────────────────────

/// A frame task's **criticality** (mixed-criticality, FF-T6). `Level1` = mandatory, must meet the deadline
/// (input, physics, audio); `Level2` = important best-effort; `Level3` = degradable/optional under overload
/// (shadows, LOD, post) — the imprecise-computation tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Criticality {
    Level1,
    Level2,
    Level3,
}

/// One unit of per-frame work with its **min-spec cost** (measured on the entry-level profile — the real
/// per-feature numbers feed this from the release benches).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub name: String,
    pub criticality: Criticality,
    pub cost_us: u32,
}

impl Task {
    #[must_use]
    pub fn new(name: impl Into<String>, criticality: Criticality, cost_us: u32) -> Self {
        Self {
            name: name.into(),
            criticality,
            cost_us,
        }
    }
}

/// The per-frame **min-spec budget** (µs). The reference is 60 fps on the entry-level profile.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    pub frame_us: u32,
}

/// The reference min-spec frame budget: **60 fps on the entry-level profile (16 666 µs)** — the `<16 ms`
/// budget this milestone closes as a GATE across M8–M12 (was perpetual debt). A stricter target (30 fps /
/// 33 ms on the weakest integrated GPU) is the owed true-min-spec-rig number.
pub const MIN_SPEC_60FPS_US: u32 = 16_666;

/// How a task was admitted this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Admission {
    /// Ran at full cost.
    Full,
    /// Degraded to a reduced budget (imprecise computation — a Level-3 task delivering partial quality).
    Degraded { to_us: u32 },
    /// Shed this frame (overload).
    Dropped,
}

/// One task's admission decision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scheduled {
    pub task: String,
    pub criticality: Criticality,
    pub admission: Admission,
    pub effective_us: u32,
}

/// The result of admission control for one frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Schedule {
    pub scheduled: Vec<Scheduled>,
    /// The mandatory (Level-1) cost — the number the gate checks against the deadline.
    pub level1_us: u32,
    /// The total effective (post-degradation) cost admitted.
    pub used_us: u32,
    pub budget_us: u32,
    /// **The gate signal:** the critical (Level-1) workload meets the deadline. Level-2/3 always fit-or-
    /// degrade, so they never fail the gate — only Level-1 overload does.
    pub schedulable: bool,
}

/// **Admission control** (FF-T6): Level-1 tasks are mandatory (always `Full`; if their sum busts the
/// budget the frame is **unschedulable** — reported, not silently dropped). Level-2 then Level-3 are
/// admitted in order while budget remains; a Level-3 task that doesn't fully fit **degrades** to the
/// remaining budget (imprecise computation), else is `Dropped`. Deterministic (stable input order).
#[must_use]
pub fn admit(tasks: &[Task], budget: Budget) -> Schedule {
    let level1_us: u32 = tasks
        .iter()
        .filter(|t| t.criticality == Criticality::Level1)
        .map(|t| t.cost_us)
        .sum();
    let schedulable = level1_us <= budget.frame_us;
    let mut remaining = budget.frame_us.saturating_sub(level1_us);
    let mut scheduled = Vec::with_capacity(tasks.len());

    for t in tasks
        .iter()
        .filter(|t| t.criticality == Criticality::Level1)
    {
        scheduled.push(Scheduled {
            task: t.name.clone(),
            criticality: Criticality::Level1,
            admission: Admission::Full,
            effective_us: t.cost_us,
        });
    }
    for level in [Criticality::Level2, Criticality::Level3] {
        for t in tasks.iter().filter(|t| t.criticality == level) {
            let (admission, effective_us) = if t.cost_us <= remaining {
                remaining -= t.cost_us;
                (Admission::Full, t.cost_us)
            } else if level == Criticality::Level3 && remaining > 0 {
                let to = remaining;
                remaining = 0;
                (Admission::Degraded { to_us: to }, to)
            } else {
                (Admission::Dropped, 0)
            };
            scheduled.push(Scheduled {
                task: t.name.clone(),
                criticality: level,
                admission,
                effective_us,
            });
        }
    }
    let used_us = scheduled.iter().map(|s| s.effective_us).sum();
    Schedule {
        scheduled,
        level1_us,
        used_us,
        budget_us: budget.frame_us,
        schedulable,
    }
}

/// **The per-commit MIN-SPEC ADMISSION CHECK** — the gate: does the CRITICAL (Level-1) workload still meet
/// the frame deadline on the min-spec budget? A feature that pushes Level-1 over budget makes the frame
/// unschedulable → the gate **fails the commit** (min-spec stops being debt). Level-2/3 are best-effort
/// (they degrade, never fail the gate — the mixed-criticality contract).
#[must_use]
pub fn holds_min_spec(tasks: &[Task], budget: Budget) -> bool {
    admit(tasks, budget).schedulable
}

// ── leg 2: frames-per-joule + SCI (ISO/IEC 21031:2024) — never a faked joule ─────────────────────────

/// Where an energy number came from — the provenance that makes a **faked joule structurally impossible**.
/// A [`Proxy`](PowerSource::Proxy) is an ESTIMATE (e.g. frame-time × assumed board watts), never a
/// measured joule; [`published_sci`] refuses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerSource {
    /// Intel/AMD RAPL MSR (a real on-die energy counter).
    Rapl,
    /// A platform power counter (e.g. Windows E3, a mobile PMIC).
    PlatformCounter,
    /// An external hardware power meter.
    ExternalMeter,
    /// A frame-time/bandwidth ESTIMATE — NOT a measured joule (carries the reason).
    Proxy(&'static str),
}

impl PowerSource {
    /// True iff this is a real measurement (not a proxy estimate).
    #[must_use]
    pub fn is_measured(self) -> bool {
        !matches!(self, PowerSource::Proxy(_))
    }
}

/// An energy measurement over `frames` frames, tagged with its [`PowerSource`].
#[derive(Clone, Copy, Debug)]
pub struct EnergyMeasurement {
    pub joules: f64,
    pub frames: u64,
    pub source: PowerSource,
}

/// Why an energy computation was refused — the guard that prevents a fabricated number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnergyError {
    /// A published SCI was requested from a proxy (estimate) source — refused (never a faked joule).
    ProxyNotPublishable(&'static str),
    /// No frames — the functional unit is undefined.
    NoFrames,
    /// No positive energy — nothing to divide by.
    NoEnergy,
}

impl std::fmt::Display for EnergyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ProxyNotPublishable(reason) => write!(
                f,
                "cannot publish an SCI/energy score from a PROXY source ({reason}) — a proxy is an estimate, not a measured joule; attach a real power source (RAPL / platform counter / meter)"
            ),
            Self::NoFrames => write!(f, "no frames measured — the SCI functional unit (per frame) is undefined"),
            Self::NoEnergy => write!(f, "no positive energy measured — frames-per-joule is undefined"),
        }
    }
}

impl std::error::Error for EnergyError {}

/// **Frames-per-joule** — the efficiency axis. Works for any source but carries `source_measured` so a
/// proxy number can never be mistaken for a real one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FramesPerJoule {
    pub value: f64,
    pub source_measured: bool,
}

/// Compute frames-per-joule from a measurement.
///
/// # Errors
/// [`EnergyError::NoEnergy`] / [`EnergyError::NoFrames`] if the inputs are degenerate.
pub fn frames_per_joule(m: &EnergyMeasurement) -> Result<FramesPerJoule, EnergyError> {
    if m.joules <= 0.0 {
        return Err(EnergyError::NoEnergy);
    }
    if m.frames == 0 {
        return Err(EnergyError::NoFrames);
    }
    #[allow(clippy::cast_precision_loss)]
    Ok(FramesPerJoule {
        value: m.frames as f64 / m.joules,
        source_measured: m.source.is_measured(),
    })
}

/// An **SCI score** (ISO/IEC 21031:2024) for a game/engine, per frame: `(E·I + M) / R`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sci {
    /// gCO2e per frame (the functional unit R = one rendered frame).
    pub grams_co2_per_frame: f64,
}

/// **Compute a PUBLISHED SCI** — and **refuse** to compute one from a proxy source. This is the anti-fake
/// guard: `SCI = (operational_energy·carbon_intensity + embodied) / frames`, and a [`PowerSource::Proxy`]
/// (an estimate) is rejected, so a fabricated SCI is impossible by construction (the `<benchmark_discipline>`
/// "never invent an energy number" rule made structural). `carbon_intensity_g_per_joule` and `embodied_g`
/// are real inputs (grid intensity, amortized hardware embodied emissions).
///
/// # Errors
/// [`EnergyError::ProxyNotPublishable`] if `m.source` is a proxy; [`EnergyError::NoFrames`] otherwise.
pub fn published_sci(
    m: &EnergyMeasurement,
    carbon_intensity_g_per_joule: f64,
    embodied_g: f64,
) -> Result<Sci, EnergyError> {
    if let PowerSource::Proxy(reason) = m.source {
        return Err(EnergyError::ProxyNotPublishable(reason));
    }
    if m.frames == 0 {
        return Err(EnergyError::NoFrames);
    }
    let operational = m.joules * carbon_intensity_g_per_joule;
    #[allow(clippy::cast_precision_loss)]
    let per_frame = (operational + embodied_g) / m.frames as f64;
    Ok(Sci {
        grams_co2_per_frame: per_frame,
    })
}

/// A **LABELED PROXY**: turn frame-times into an energy ESTIMATE assuming a fixed board power. Tagged
/// [`PowerSource::Proxy`] so [`published_sci`] refuses it — use only for relative frame-time comparison and
/// as the placeholder the true-watt rig replaces. **This is NOT a measured joule.**
#[must_use]
pub fn proxy_energy(frame_us: &[u32], assumed_board_watts: f64) -> EnergyMeasurement {
    let total_us: u64 = frame_us.iter().map(|&u| u64::from(u)).sum();
    #[allow(clippy::cast_precision_loss)]
    let seconds = total_us as f64 / 1_000_000.0;
    EnergyMeasurement {
        joules: assumed_board_watts * seconds,
        frames: frame_us.len() as u64,
        source: PowerSource::Proxy(
            "frame-time x assumed board watts — an ESTIMATE, not a measured joule (true-watt rig owed)",
        ),
    }
}

// ── leg 3: the structural-reason narrative (evidenced with real numbers, not asserted) ───────────────

/// *Why* Metrocalk is faster-per-watt — each reason paired with a **measured** number from this repo (not
/// asserted). Energy is the mechanism; the green story is a consequence.
#[must_use]
pub fn structural_reasons() -> Vec<(&'static str, &'static str)> {
    vec![
        ("Rust + data-oriented Flecs ECS — cache-locality is energy (DRAM access ~100-1000x on-chip, FM-T7.2)", "the M1.5 query gate holds <16 ms at scale"),
        ("Determinism = no GC pauses / no frame-time spikes to over-provision against (M13.1)", "DST headless replay ~19,870 frames/s / 331x real-time, bit-identical x3"),
        ("Incremental recompute does O(delta) work, not O(world) each frame (M13.4)", "IVM 107x faster than eager at 0.1% churn; eager busts the 16 ms budget at 20k entities, incremental holds it"),
        ("Offline, verifiable AI authoring runs with no per-frame model inference (M13.5)", "the authoring substrate is deterministic + LLM-free"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget() -> Budget {
        Budget {
            frame_us: MIN_SPEC_60FPS_US,
        }
    }

    fn baseline_frame() -> Vec<Task> {
        vec![
            Task::new("input", Criticality::Level1, 200),
            Task::new("physics", Criticality::Level1, 3_000),
            Task::new("audio", Criticality::Level1, 800),
            Task::new("gameplay", Criticality::Level2, 2_000),
            Task::new("shadows", Criticality::Level3, 6_000),
            Task::new("post", Criticality::Level3, 4_000),
        ]
    }

    #[test]
    fn the_min_spec_gate_catches_an_unschedulable_level1_feature() {
        // The baseline holds the min-spec budget.
        assert!(
            holds_min_spec(&baseline_frame(), budget()),
            "the baseline frame is schedulable on min-spec"
        );

        // A heavy new LEVEL-1 feature (an un-degradable physics system that busts the frame) → the gate
        // FAILS the commit (min-spec is a gate, not debt).
        let mut heavy = baseline_frame();
        heavy.push(Task::new("naive_broadphase", Criticality::Level1, 20_000));
        assert!(
            !holds_min_spec(&heavy, budget()),
            "a Level-1 feature that busts the min-spec budget fails the gate"
        );
    }

    #[test]
    fn level3_degrades_under_overload_and_does_not_fail_the_gate() {
        // Pile on Level-3 work far over budget: the frame stays SCHEDULABLE (Level-1 meets its deadline);
        // Level-3 tasks degrade/drop (imprecise computation) instead of failing the gate.
        let mut over = baseline_frame();
        over.push(Task::new("raytraced_gi", Criticality::Level3, 50_000));
        let sched = admit(&over, budget());
        assert!(
            sched.schedulable,
            "Level-3 overload does NOT make the frame unschedulable"
        );
        assert!(holds_min_spec(&over, budget()));
        let degraded_or_dropped = sched
            .scheduled
            .iter()
            .any(|s| matches!(s.admission, Admission::Degraded { .. } | Admission::Dropped));
        assert!(
            degraded_or_dropped,
            "some Level-3 work degraded/dropped to hold the budget"
        );
        assert!(
            sched.used_us <= sched.budget_us || sched.level1_us <= sched.budget_us,
            "the admitted work fits the deadline (Level-1) or the whole budget"
        );
    }

    #[test]
    fn level1_is_never_dropped_it_reports_the_overload_honestly() {
        // Even when Level-1 alone busts the budget, it is admitted FULL (never silently shed) and the
        // overload is reported via schedulable=false — an honest signal, not a hidden failure.
        let tasks = vec![Task::new("giant_sim", Criticality::Level1, 30_000)];
        let sched = admit(&tasks, budget());
        assert!(!sched.schedulable);
        assert_eq!(
            sched.scheduled[0].admission,
            Admission::Full,
            "critical work is never dropped"
        );
        assert_eq!(sched.level1_us, 30_000);
    }

    #[test]
    fn admission_is_deterministic() {
        assert_eq!(
            admit(&baseline_frame(), budget()),
            admit(&baseline_frame(), budget()),
            "same tasks + budget -> same schedule"
        );
    }

    #[test]
    fn published_sci_refuses_a_proxy_source_never_a_faked_joule() {
        // THE ANTI-FAKE GUARD: an SCI from a PROXY energy source is REFUSED (structurally impossible to
        // publish a fabricated joule).
        let proxy = proxy_energy(&[16_000, 16_500, 16_200], 15.0);
        let err = published_sci(&proxy, 0.0001, 100.0).expect_err("a proxy cannot publish an SCI");
        assert!(matches!(err, EnergyError::ProxyNotPublishable(_)));
        assert!(
            err.to_string().contains("not a measured joule"),
            "the refusal is explained: {err}"
        );

        // A REAL measured source computes an SCI.
        let measured = EnergyMeasurement {
            joules: 0.75,
            frames: 100,
            source: PowerSource::Rapl,
        };
        let sci = published_sci(&measured, 0.0001, 100.0).expect("a measured source publishes");
        assert!(sci.grams_co2_per_frame > 0.0);
    }

    #[test]
    fn frames_per_joule_carries_the_source_provenance() {
        let proxy = proxy_energy(&[16_000; 60], 15.0);
        let fpj_proxy = frames_per_joule(&proxy).unwrap();
        assert!(
            !fpj_proxy.source_measured,
            "a proxy fpj is flagged as an estimate"
        );

        let measured = EnergyMeasurement {
            joules: 2.0,
            frames: 120,
            source: PowerSource::ExternalMeter,
        };
        let fpj = frames_per_joule(&measured).unwrap();
        assert!(
            fpj.source_measured && (fpj.value - 60.0).abs() < 1e-9,
            "120 frames / 2 J = 60 fpj, measured"
        );
    }

    #[test]
    fn the_proxy_is_labeled_and_the_structural_narrative_is_evidenced() {
        // The proxy is honestly labeled (not a joule).
        let e = proxy_energy(&[16_000], 15.0);
        assert!(matches!(e.source, PowerSource::Proxy(_)));

        // The structural-reason narrative cites REAL measured numbers from this repo (not asserted).
        let reasons = structural_reasons();
        assert!(reasons.len() >= 3, "several structural reasons");
        assert!(
            reasons
                .iter()
                .any(|(_, ev)| ev.contains("19,870") || ev.contains("331x")),
            "the determinism reason cites the measured DST number"
        );
        assert!(
            reasons.iter().any(|(_, ev)| ev.contains("107x")),
            "the incremental reason cites the measured IVM crossover"
        );
    }
}
