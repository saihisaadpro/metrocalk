use std::error::Error;
use std::fmt::{self, Display, Formatter};

/// The authoritative fixed-step index.
pub type Tick = u64;

/// One whole in basis-point arithmetic.
pub const BASIS_POINTS: u16 = 10_000;

/// Hard safety ceiling for an in-memory MOB-1 match/replay. Longer evidence moves to checkpointed MOB-2
/// artifacts instead of allocating an unbounded frame history.
pub const MAX_MATCH_TICKS: Tick = 250_000;

/// Absolute allocation ceiling for one core-runtime checkpoint, independent of authored target profiles.
pub const MAX_RUNTIME_CHECKPOINT_BYTES: u32 = 16 * 1024 * 1024;

/// Hard decoded-state ceilings. Target profiles are normally much smaller; these prevent a valid-sized
/// checkpoint from amplifying attacker-controlled counts into unbounded heap structures.
pub const MAX_PLAYER_BUDGET: u32 = 64;
pub const MAX_ACTOR_BUDGET: u32 = 4_096;
pub const MAX_ABILITY_DEFINITIONS: u32 = 4_096;
pub const MAX_ABILITIES_PER_ACTOR_BUDGET: u32 = 64;
pub const MAX_COMMANDS_PER_TICK_BUDGET: u32 = 2_048;
pub const MAX_COMMANDS_PER_PLAYER_PER_TICK_BUDGET: u32 = 128;
pub const MAX_REJECTION_EVENTS_PER_FRAME_BUDGET: u32 = 2_048;
pub const MAX_COMMAND_LEAD_TICKS: u16 = 128;
pub const MAX_LANE_BUDGET: u16 = 16;
pub const MAX_LANE_POINTS_PER_LANE_BUDGET: u16 = 1_024;
pub const MAX_WAVE_SCHEDULE_BUDGET: u16 = 256;
pub const MAX_UNITS_PER_WAVE_BUDGET: u16 = 256;
pub const MAX_MODIFIERS_PER_ACTOR_BUDGET: u16 = 64;
pub const MAX_STATUS_EFFECTS_PER_ACTOR_BUDGET: u16 = 32;
pub const MAX_PROJECTILE_BUDGET: u16 = 512;

/// Hard ceilings on projectile flight speed and hit radius.
///
/// These are arithmetic safety bounds, not balance knobs. Swept collision compares
/// `cross²` against `radius² × |step|²` in `i128`; with map coordinates bounded to ±500,000 mm these
/// ceilings keep every intermediate three orders of magnitude below `i128::MAX`, so the collision test can
/// never wrap. A speed above 100 m per tick would also outrun the map in a single tick, making the swept
/// test the only thing standing between a projectile and every actor on the board.
pub const MAX_PROJECTILE_SPEED_MM_PER_TICK: u32 = 100_000;
pub const MAX_PROJECTILE_RADIUS_MM: u32 = 100_000;
/// Same reasoning for an area impact: the overlap test squares this against a `u128` distance.
pub const MAX_IMPACT_RADIUS_MM: u32 = 1_000_000;

pub const MAX_SCORE_ENTRY_BUDGET: u16 = 64;
pub const MAX_DAMAGE_CREDITS_PER_ACTOR_BUDGET: u16 = 32;
pub const MAX_ASSIST_WINDOW_TICKS: u32 = 3_600;
pub const MAX_PASSIVE_GOLD_PER_100S: u32 = 10_000;
/// Arithmetic-safety ceiling, not a balance knob — the share test squares this against the `u128`
/// [`Vec2Mm::squared_distance`], exactly like [`MAX_IMPACT_RADIUS_MM`].
pub const MAX_XP_SHARE_RADIUS_MM: u32 = 1_000_000;
pub const MAX_BOUNTY_GOLD: u32 = 10_000;
pub const MAX_BOUNTY_EXPERIENCE: u32 = 10_000;
pub const MAX_STAT_GROWTH_PER_LEVEL: u32 = 10_000;
pub const MAX_ABILITY_PER_RANK_AMOUNT: u32 = 1_000_000;
pub const MAX_HERO_LEVEL: u8 = 18;
pub const MAX_ABILITY_RANK: u8 = 5;
/// `experience_for_level(MAX_HERO_LEVEL)`. Experience past the cap is **not banked**, which is what turns
/// an otherwise unbounded counter into a quantity a restore audit can actually check.
pub const EXPERIENCE_CAP: u32 = 18_360;
/// `streak_gold(10)`. Named because the earned-gold restore relation is derived from it.
pub const MAX_STREAK_GOLD: u32 = 550;

/// Cumulative experience required to *be* `level`: `10 × (L−1) × (5L + 18)`.
///
/// Level 1 costs nothing and level 18 costs 18,360. This is League of Legends' shipped curve, copied
/// rather than invented so the pacing is a known quantity, and kept closed-form rather than a table so it
/// needs no authored content and cannot drift from its inverse.
#[must_use]
pub const fn experience_for_level(level: u8) -> u32 {
    let level = if level > MAX_HERO_LEVEL {
        MAX_HERO_LEVEL
    } else if level < 1 {
        1
    } else {
        level
    } as u32;
    10 * (level - 1) * (5 * level + 18)
}

/// The exact inverse of [`experience_for_level`]: `L = (isqrt(2x + 529) − 13) / 10`, clamped to `1..=18`.
///
/// Integer square root only — no float, no lookup table, no per-tick loop. `level` is therefore **derived**
/// state: it is never encoded in a checkpoint, so a payload cannot assert a level its own experience does
/// not produce.
#[must_use]
pub fn level_for_experience(experience: u32) -> u8 {
    let capped = if experience > EXPERIENCE_CAP {
        EXPERIENCE_CAP
    } else {
        experience
    };
    let root = integer_sqrt(u128::from(capped) * 2 + 529);
    let level = u8::try_from((root.saturating_sub(13)) / 10).unwrap_or(MAX_HERO_LEVEL);
    level.clamp(1, MAX_HERO_LEVEL)
}

/// Shutdown gold for ending a `streak`-long kill streak: `5x(x + 1)`, zero below three, clamped at ten.
///
/// Dota 2's shipped polynomial. Pure multiply/add — no division, so no rounding rule to get wrong.
#[must_use]
pub const fn streak_gold(streak: u16) -> u32 {
    if streak < 3 {
        return 0;
    }
    let streak = if streak > 10 { 10 } else { streak } as u32;
    5 * streak * (streak + 1)
}

/// Largest-remainder apportionment of `pool` across `recipients`.
///
/// Returns `(base, remainder)`; the caller awards one extra to the first `remainder` recipients **in
/// ascending `ActorId` order**. Conserving rather than truncating on purpose: it makes "every unit a death
/// mints is awarded" a statable invariant, where truncation leaves the shortfall invisible.
#[must_use]
pub const fn apportion(pool: u32, recipients: u32) -> (u32, u32) {
    if recipients == 0 {
        return (0, 0);
    }
    (pool / recipients, pool % recipients)
}

/// Passive gold owed after `elapsed_ticks`, as a closed form of the tick rather than an accumulator.
///
/// Being a pure function of the tick is the whole point: there is no running total to drift, to desync
/// between two runtimes, or to tamper with in a checkpoint — the value is re-derived and compared exactly
/// on every frame boundary.
#[must_use]
pub const fn passive_gold_owed(elapsed_ticks: Tick, config: MatchConfig) -> u32 {
    let denominator = 100 * config.tick_rate_hz as u64;
    if denominator == 0 {
        return 0;
    }
    let owed = elapsed_ticks.saturating_mul(config.passive_gold_per_100s as u64) / denominator;
    // Clamped first, so the narrowing below is exact rather than a truncation.
    if owed > u32::MAX as u64 {
        u32::MAX
    } else {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the branch above proves the value fits"
        )]
        {
            owed as u32
        }
    }
}

/// Stable identity for one source of randomness.
///
/// Every roll mixes its domain into the key, so two domains cannot correlate and — the property that
/// actually matters — **a domain that skips a roll cannot shift any other domain's sequence**. A shared
/// stateful stream does not have that property: two runs of the same match may legitimately evaluate a
/// different number of crit rolls before a given spawn, and with one cursor the second run's numbers all
/// shift. That is the desync class this design removes by construction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RngDomain {
    CriticalStrike,
    /// Reserved so later slices claim a stable constant rather than renumbering an existing one.
    /// A shipped domain constant is never reused.
    NeutralSpawn,
    ProcEffect,
    AiTieBreak,
}

impl RngDomain {
    /// Append-only, and part of simulation truth: changing one silently re-rolls every historical match
    /// that replays through it.
    const fn key(self) -> u64 {
        match self {
            Self::CriticalStrike => 0x4352_4954_5f31,
            Self::NeutralSpawn => 0x4e45_5554_5f31,
            Self::ProcEffect => 0x5052_4f43_5f31,
            Self::AiTieBreak => 0x4149_5442_5f31,
        }
    }
}

/// The integer mixer every roll is built from — SplitMix64's finalizer.
///
/// Chosen over the crate's FNV-1a `StableHash` deliberately: FNV is a *hash*, not a generator, and its
/// avalanche in the low bits is poor — which is exactly where a `% BASIS_POINTS` comparison reads. This
/// finalizer is a handful of integer operations, carries no state, and is bit-identical on every target
/// the kernel compiles for.
const fn mix64(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// One deterministic roll, **keyed rather than streamed**.
///
/// The result is a pure function of `(seed, domain, tick, subject, object, counter)`. Nothing is consumed
/// and no cursor advances, so the same logical event yields the same number regardless of the order the
/// simulation evaluated things in — which is precisely why iteration order can never leak into a roll.
#[must_use]
pub fn roll_u64(
    seed: u64,
    domain: RngDomain,
    tick: Tick,
    subject: u64,
    object: u64,
    counter: u64,
) -> u64 {
    let mut value = mix64(seed ^ domain.key());
    value = mix64(value ^ mix64(tick));
    value = mix64(value ^ mix64(subject));
    value = mix64(value ^ mix64(object));
    mix64(value ^ mix64(counter))
}

/// Does a keyed roll land under `chance_bps`?
///
/// Modulo bias is negligible: the reduction is from `2^64` to 10,000 buckets, so the worst-case bias is
/// on the order of `10^-16` — far below anything a match could observe.
#[must_use]
pub fn roll_under_bps(
    seed: u64,
    domain: RngDomain,
    tick: Tick,
    subject: u64,
    object: u64,
    counter: u64,
    chance_bps: u16,
) -> bool {
    if chance_bps == 0 {
        return false;
    }
    if chance_bps >= BASIS_POINTS {
        return true;
    }
    let value = roll_u64(seed, domain, tick, subject, object, counter);
    (value % u64::from(BASIS_POINTS)) < u64::from(chance_bps)
}

/// Pseudo-random-distribution constants, in basis points, indexed by whole percent `0..=100`.
///
/// PRD (Warcraft III, inherited by Dota 2) replaces a flat roll with an escalating one: on the `N`-th
/// attempt since the last success the probability is `C x N`. That removes both the long dry streak and
/// the improbable double-proc a flat roll produces, which is why players read it as fairer than true
/// randomness even though it is less random.
///
/// **`C` is not the nominal chance** — it is the constant whose resulting distribution *averages* to it,
/// and it has no closed form. This table is the solution of `1 / E[N](C) = p`, and
/// `the_prd_table_matches_its_own_derivation` re-derives every row and asserts it, so the table is
/// verified rather than copied. The 10 % row is the published cross-check: community-derived sources give
/// `C ~= 0.01475`, and this independently-solved table lands on **147** basis points.
const PRD_CONSTANTS_BPS: [u16; 101] = [
    0, 2, 6, 14, 24, 38, 54, 74, 96, 120, 147, 177, 210, 245, 282, 322, 365, 409, 456, 505, 557,
    611, 667, 725, 785, 847, 912, 978, 1047, 1117, 1189, 1264, 1340, 1418, 1498, 1580, 1663, 1749,
    1836, 1925, 2015, 2109, 2204, 2299, 2395, 2493, 2599, 2705, 2810, 2916, 3021, 3127, 3233, 3341,
    3474, 3604, 3732, 3858, 3983, 4105, 4226, 4346, 4464, 4581, 4697, 4811, 4925, 5075, 5294, 5507,
    5714, 5915, 6111, 6301, 6486, 6667, 6842, 7013, 7179, 7342, 7500, 7654, 7805, 7952, 8095, 8235,
    8372, 8506, 8636, 8764, 8889, 9011, 9130, 9247, 9362, 9474, 9583, 9691, 9796, 9899, 10000,
];

/// The PRD constant for a nominal chance, rounded to the nearest whole percent.
///
/// Whole-percent resolution is deliberate: the table is regenerated by test, and a per-basis-point table
/// would be 10,001 entries to buy a difference no player can perceive.
#[must_use]
pub fn prd_constant_bps(chance_bps: u16) -> u16 {
    let percent = usize::from(chance_bps.min(BASIS_POINTS)).div_euclid(100);
    let remainder = usize::from(chance_bps.min(BASIS_POINTS)).rem_euclid(100);
    let index = (percent + usize::from(remainder >= 50)).min(100);
    PRD_CONSTANTS_BPS[index]
}

/// The escalating probability for the `attempts`-th try since the last success, in basis points.
///
/// `attempts` is 1-based: the first attempt after a success is attempt 1 and sees exactly `C`.
#[must_use]
pub fn prd_chance_bps(chance_bps: u16, attempts: u16) -> u16 {
    let constant = u32::from(prd_constant_bps(chance_bps));
    let escalated = constant.saturating_mul(u32::from(attempts.max(1)));
    u16::try_from(escalated.min(u32::from(BASIS_POINTS))).unwrap_or(BASIS_POINTS)
}

pub(crate) fn integer_sqrt(value: u128) -> u128 {
    if value < 2 {
        return value;
    }
    let mut estimate = value;
    let mut next = estimate.div_ceil(2);
    while next < estimate {
        estimate = next;
        next = u128::midpoint(estimate, value / estimate);
    }
    estimate
}

pub(crate) fn integer_sqrt_ceil(value: u128) -> u128 {
    let floor = integer_sqrt(value);
    if floor * floor == value {
        floor
    } else {
        floor + 1
    }
}

macro_rules! id_type {
    ($name:ident, $inner:ty, $description:literal) => {
        #[doc = $description]
        #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub $inner);

        impl $name {
            #[must_use]
            pub const fn get(self) -> $inner {
                self.0
            }
        }
    };
}

id_type!(
    ActorId,
    u64,
    "Stable runtime identity for one replicated actor."
);
id_type!(
    PlayerId,
    u64,
    "Authenticated player identity inside one match."
);
id_type!(TeamId, u8, "Team identity inside one match.");
id_type!(
    AbilityId,
    u32,
    "Dense-build-compatible identity for an ability definition."
);
id_type!(
    ProjectileId,
    u64,
    "Monotonic never-reused identity for one in-flight projectile."
);

/// A deterministic two-dimensional gameplay position in integer millimetres.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Vec2Mm {
    pub x: i32,
    pub y: i32,
}

impl Vec2Mm {
    #[must_use]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    pub(crate) fn squared_distance(self, other: Self) -> u128 {
        let dx = i64::from(other.x) - i64::from(self.x);
        let dy = i64::from(other.y) - i64::from(self.y);
        u128::from(dx.unsigned_abs()).pow(2) + u128::from(dy.unsigned_abs()).pow(2)
    }
}

/// Inclusive map bounds in integer millimetres.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RectMm {
    pub min: Vec2Mm,
    pub max: Vec2Mm,
}

impl RectMm {
    #[must_use]
    pub const fn new(min: Vec2Mm, max: Vec2Mm) -> Self {
        Self { min, max }
    }

    #[must_use]
    pub const fn contains(self, point: Vec2Mm) -> bool {
        point.x >= self.min.x
            && point.x <= self.max.x
            && point.y >= self.min.y
            && point.y <= self.max.y
    }

    pub(crate) const fn is_valid(self) -> bool {
        self.min.x < self.max.x && self.min.y < self.max.y
    }
}

/// Broad runtime category. Domain-specific behavior is data, not an inheritance hierarchy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActorKind {
    Hero,
    Minion,
    Structure,
    Objective,
    Pet,
    Projectile,
}

/// Authoritative combat and movement values already compiled for the match tick rate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CombatStats {
    pub max_health: u32,
    pub max_resource: u32,
    pub move_speed_mm_per_tick: u32,
    pub physical_reduction_bps: u16,
    pub magic_reduction_bps: u16,
    /// Nominal critical-strike chance. It is *nominal* on purpose: the roll runs under pseudo-random
    /// distribution, so this is the long-run average rather than a per-swing probability.
    pub crit_chance_bps: u16,
    /// Total damage on a critical strike, as a multiplier: 20,000 bps = 200 % damage.
    pub crit_damage_bps: u16,
    /// Percentage penetration of the target's PHYSICAL mitigation. It scales the target's reduction, it
    /// does not subtract an armour value — this kernel models mitigation directly in basis points, and a
    /// separate armour stat would be a second representation of the same quantity that could disagree.
    pub physical_penetration_bps: u16,
    pub magic_penetration_bps: u16,
    /// Fraction of damage actually applied that is returned to the attacker as health.
    pub lifesteal_bps: u16,
}

impl CombatStats {
    pub(crate) fn validate(self) -> Result<(), RuntimeError> {
        if self.max_health == 0 {
            return Err(RuntimeError::InvalidActor("max health must be positive"));
        }
        if self.physical_reduction_bps >= BASIS_POINTS || self.magic_reduction_bps >= BASIS_POINTS {
            return Err(RuntimeError::InvalidActor(
                "damage reduction must be below 10,000 basis points",
            ));
        }
        Ok(())
    }

    /// Resolve authored base stats plus a set of modifiers into the stats the simulation actually uses.
    ///
    /// **This function is the determinism contract.** With integer arithmetic the evaluation order decides
    /// the result, so the order is fixed here, documented, and tested — never left to insertion order:
    ///
    /// 1. **Flat** magnitudes are summed. Integer addition is associative and commutative, so this stage is
    ///    order-independent by construction.
    /// 2. **PercentAdd** magnitudes are summed into ONE multiplier applied once. Summing before dividing
    ///    means +10% and +20% compose to exactly +30%, and only one truncation happens.
    /// 3. **PercentMult** magnitudes apply sequentially, ordered **by magnitude** (ties by ID). Truncating
    ///    multiplication is not commutative — `7 × 1.5 → 10 × 0.5 → 5` but `7 × 0.5 → 3 × 1.5 → 4` — so a
    ///    total order is mandatory. Ordering by magnitude rather than by ID makes the result a function of
    ///    the modifier SET, so applying the same buffs in a different sequence cannot yield different stats.
    ///    Equal magnitudes commute exactly, so the ID tiebreak never changes the value.
    /// 4. **Override** replaces the value outright, and the **highest modifier ID wins**. This stage is
    ///    deliberately order-DEPENDENT: "most recently applied wins" is the intended semantics for an
    ///    override, and there is no set-determined key that expresses it. It is still fully deterministic —
    ///    the same operation sequence always produces the same IDs — but two different sequences applying
    ///    the same overrides can legitimately differ.
    /// 5. The result is clamped into the stat's legal domain. Clamping is a safety property, not a detail:
    ///    it is what stops a stacked reduction modifier from reaching 100% and making an actor immortal.
    ///
    /// Division truncates toward zero. Every intermediate is `i64`, which cannot overflow for `u32` bases
    /// and basis-point multipliers, and saturating arithmetic backstops the impossible case.
    /// Authored stats grown to `level`.
    ///
    /// Level 1 is the identity — which is both the semantics (level 1 *is* the authored hero) and the fast
    /// path the frame-boundary audit needs, since every minion, structure, objective and pet in a match is
    /// permanently level 1 and would otherwise pay for five saturating multiplies per actor per tick.
    ///
    /// Growth is clamped into the same [`StatKind::domain`] bounds as modifiers, so no stack of growth can
    /// reach 100 % mitigation any more than a stack of buffs can.
    #[must_use]
    pub fn with_growth(self, growth: StatGrowth, level: u8) -> Self {
        if level <= 1 {
            return self;
        }
        let steps = u64::from(level - 1);
        let grow = |base: u32, per_level: u32, kind: StatKind| -> u32 {
            let (minimum, maximum) = kind.domain();
            let raised = u64::from(base).saturating_add(u64::from(per_level).saturating_mul(steps));
            let clamped = i64::try_from(raised)
                .unwrap_or(i64::MAX)
                .clamp(minimum, maximum);
            u32::try_from(clamped).unwrap_or(0)
        };
        Self {
            max_health: grow(
                self.max_health,
                growth.max_health_per_level,
                StatKind::MaxHealth,
            ),
            max_resource: grow(
                self.max_resource,
                growth.max_resource_per_level,
                StatKind::MaxResource,
            ),
            move_speed_mm_per_tick: grow(
                self.move_speed_mm_per_tick,
                growth.move_speed_mm_per_tick_per_level,
                StatKind::MoveSpeed,
            ),
            physical_reduction_bps: u16::try_from(grow(
                u32::from(self.physical_reduction_bps),
                u32::from(growth.physical_reduction_bps_per_level),
                StatKind::PhysicalReduction,
            ))
            .unwrap_or(BASIS_POINTS - 1),
            magic_reduction_bps: u16::try_from(grow(
                u32::from(self.magic_reduction_bps),
                u32::from(growth.magic_reduction_bps_per_level),
                StatKind::MagicReduction,
            ))
            .unwrap_or(BASIS_POINTS - 1),
            // Growth deliberately does not scale the offensive stats: per-level crit and penetration are
            // an item-and-ability concern, and a hero that criticals harder purely for surviving is a
            // balance decision nobody authored.
            crit_chance_bps: self.crit_chance_bps,
            crit_damage_bps: self.crit_damage_bps,
            physical_penetration_bps: self.physical_penetration_bps,
            magic_penetration_bps: self.magic_penetration_bps,
            lifesteal_bps: self.lifesteal_bps,
        }
    }

    #[must_use]
    pub fn with_modifiers(self, modifiers: &[StatModifier]) -> Self {
        Self {
            max_health: resolve_stat(self.max_health, StatKind::MaxHealth, modifiers),
            max_resource: resolve_stat(self.max_resource, StatKind::MaxResource, modifiers),
            move_speed_mm_per_tick: resolve_stat(
                self.move_speed_mm_per_tick,
                StatKind::MoveSpeed,
                modifiers,
            ),
            physical_reduction_bps: u16::try_from(resolve_stat(
                u32::from(self.physical_reduction_bps),
                StatKind::PhysicalReduction,
                modifiers,
            ))
            .unwrap_or(BASIS_POINTS - 1),
            magic_reduction_bps: u16::try_from(resolve_stat(
                u32::from(self.magic_reduction_bps),
                StatKind::MagicReduction,
                modifiers,
            ))
            .unwrap_or(BASIS_POINTS - 1),
            crit_chance_bps: resolve_bps(self.crit_chance_bps, StatKind::CritChance, modifiers),
            crit_damage_bps: resolve_bps(self.crit_damage_bps, StatKind::CritDamage, modifiers),
            physical_penetration_bps: resolve_bps(
                self.physical_penetration_bps,
                StatKind::PhysicalPenetration,
                modifiers,
            ),
            magic_penetration_bps: resolve_bps(
                self.magic_penetration_bps,
                StatKind::MagicPenetration,
                modifiers,
            ),
            lifesteal_bps: resolve_bps(self.lifesteal_bps, StatKind::Lifesteal, modifiers),
        }
    }

    /// A stat block that contributes nothing.
    ///
    /// Used when a damage source no longer exists — a projectile outliving its caster, for instance. It
    /// deliberately has zero crit chance, zero penetration and zero lifesteal, so a dead attacker's hit
    /// lands as a plain one rather than inheriting whatever the last live actor happened to have.
    #[must_use]
    pub const fn inert() -> Self {
        Self {
            max_health: 1,
            max_resource: 0,
            move_speed_mm_per_tick: 0,
            physical_reduction_bps: 0,
            magic_reduction_bps: 0,
            crit_chance_bps: 0,
            crit_damage_bps: BASIS_POINTS,
            physical_penetration_bps: 0,
            magic_penetration_bps: 0,
            lifesteal_bps: 0,
        }
    }

    /// The mitigation this actor applies to one damage school, after the attacker's penetration.
    ///
    /// Penetration SCALES the reduction rather than subtracting an armour value, and it can drive the
    /// reduction to zero but never below it — mirroring the documented rule that penetration cannot make
    /// effective armour negative.
    #[must_use]
    pub fn reduction_after_penetration(self, school: DamageSchool, attacker: Self) -> u16 {
        let (reduction, penetration) = match school {
            DamageSchool::Physical => (
                self.physical_reduction_bps,
                attacker.physical_penetration_bps,
            ),
            DamageSchool::Magic => (self.magic_reduction_bps, attacker.magic_penetration_bps),
            DamageSchool::True => return 0,
        };
        let remaining =
            u32::from(BASIS_POINTS).saturating_sub(u32::from(penetration.min(BASIS_POINTS)));
        let scaled = u32::from(reduction) * remaining / u32::from(BASIS_POINTS);
        u16::try_from(scaled).unwrap_or(BASIS_POINTS - 1)
    }
}

/// Resolve a basis-point stat, saturating into `u16` at its clamped domain.
fn resolve_bps(base: u16, kind: StatKind, modifiers: &[StatModifier]) -> u16 {
    let (_, maximum) = kind.domain();
    let resolved = resolve_stat(u32::from(base), kind, modifiers);
    u16::try_from(resolved).unwrap_or_else(|_| u16::try_from(maximum).unwrap_or(u16::MAX))
}

/// What killing this actor is worth.
///
/// Authored on the spawn and carried immutably on the actor, rather than looked up through
/// [`ActorProvenance`]: `Authored` provenance carries no content anchor at all and `Dynamic` carries only
/// an ordinal, so a provenance lookup would work for wave minions and for no hero, tower, or objective in
/// the crate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Bounty {
    pub gold: u32,
    pub experience: u32,
}

impl Bounty {
    pub const NONE: Self = Self {
        gold: 0,
        experience: 0,
    };

    pub(crate) const fn validate(self) -> Result<(), RuntimeError> {
        if self.gold > MAX_BOUNTY_GOLD || self.experience > MAX_BOUNTY_EXPERIENCE {
            return Err(RuntimeError::InvalidActor(
                "bounty exceeds its hard ceiling",
            ));
        }
        Ok(())
    }
}

/// Per-level stat growth, applied as `authored + growth × (level − 1)`.
///
/// Growth is authored and immutable, so the stats a level produces stay **re-derivable** from data a
/// checkpoint cannot forge — which is what lets `base_stats` be a derived cache rather than a mutable
/// number a tampered payload could assert freely.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatGrowth {
    pub max_health_per_level: u32,
    pub max_resource_per_level: u32,
    pub move_speed_mm_per_tick_per_level: u32,
    pub physical_reduction_bps_per_level: u16,
    pub magic_reduction_bps_per_level: u16,
}

impl StatGrowth {
    pub const NONE: Self = Self {
        max_health_per_level: 0,
        max_resource_per_level: 0,
        move_speed_mm_per_tick_per_level: 0,
        physical_reduction_bps_per_level: 0,
        magic_reduction_bps_per_level: 0,
    };

    pub(crate) const fn validate(self) -> Result<(), RuntimeError> {
        if self.max_health_per_level > MAX_STAT_GROWTH_PER_LEVEL
            || self.max_resource_per_level > MAX_STAT_GROWTH_PER_LEVEL
            || self.move_speed_mm_per_tick_per_level > MAX_STAT_GROWTH_PER_LEVEL
            || self.physical_reduction_bps_per_level as u32 > MAX_STAT_GROWTH_PER_LEVEL
            || self.magic_reduction_bps_per_level as u32 > MAX_STAT_GROWTH_PER_LEVEL
        {
            return Err(RuntimeError::InvalidActor(
                "stat growth exceeds its hard ceiling",
            ));
        }
        Ok(())
    }
}

/// Read-only projection of one scoreboard line.
///
/// A line is opened for every hero and is **never removed**, so it outlives an actor that despawns. That is
/// the point: a scoreboard whose rows vanish when a body does cannot report the match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScoreView {
    pub actor: ActorId,
    pub owner: Option<PlayerId>,
    pub team: TeamId,
    /// Earned plus passive. The two are stored separately for tamper resistance, not for the client.
    pub gold: u32,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub kill_streak: u16,
    pub best_kill_streak: u16,
}

/// Why gold was granted.
///
/// Passive income is deliberately absent: it is exactly derivable from the tick and the config, so an
/// event per hero per accrual would be pure noise on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoldReason {
    Kill,
    Shutdown,
    Assist,
}

/// One resolvable actor attribute. Every variant maps to exactly one [`CombatStats`] field.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StatKind {
    MaxHealth,
    MaxResource,
    MoveSpeed,
    PhysicalReduction,
    MagicReduction,
    CritChance,
    CritDamage,
    PhysicalPenetration,
    MagicPenetration,
    Lifesteal,
}

impl StatKind {
    /// The legal domain for a resolved value. Reductions stay strictly below 100% so no stack of modifiers
    /// can produce an immortal actor, and max health stays positive so `validate` can never be violated by
    /// a modifier.
    const fn domain(self) -> (i64, i64) {
        match self {
            Self::MaxHealth => (1, u32::MAX as i64),
            Self::MaxResource | Self::MoveSpeed => (0, u32::MAX as i64),
            Self::PhysicalReduction | Self::MagicReduction => (0, BASIS_POINTS as i64 - 1),
            // Chance and penetration are fractions of a whole and may reach it; a 100 % crit chance and a
            // 100 % penetration are both legal end-states. Reductions above are the exception, and stay
            // strictly below whole so no stack can produce an immortal actor.
            Self::CritChance | Self::PhysicalPenetration | Self::MagicPenetration => {
                (0, BASIS_POINTS as i64)
            }
            // Lifesteal is bounded well above 100 % so a build can exceed it, but not unboundedly.
            Self::Lifesteal => (0, 4 * BASIS_POINTS as i64),
            // A critical strike may not deal LESS than a normal hit, so the multiplier floors at whole.
            Self::CritDamage => (BASIS_POINTS as i64, 10 * BASIS_POINTS as i64),
        }
    }
}

/// How one modifier changes its stat. Magnitudes are signed: a slow is a negative `PercentAdd`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierOp {
    Flat { magnitude: i32 },
    PercentAdd { bps: i32 },
    PercentMult { bps: i32 },
    Override { value: u32 },
}

/// Monotonic never-reused identity for one applied modifier. Allocation order defines both the sequential
/// `PercentMult` order and override precedence, which is why it must never be reused.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModifierId(pub u64);

impl ModifierId {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// One crowd-control restriction. These are the genre's load-bearing interaction verbs: they decide what an
/// actor may *do*, independently of what its stats are.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ControlKind {
    /// Blocks movement, casting, and attacking, and interrupts anything already in progress.
    Stun,
    /// Blocks movement only.
    Root,
    /// Blocks casting only. Does not interrupt a cast already resolving.
    Silence,
    /// Blocks basic attacks only.
    Disarm,
}

impl ControlKind {
    const fn bit(self) -> u8 {
        match self {
            Self::Stun => 1,
            Self::Root => 1 << 1,
            Self::Silence => 1 << 2,
            Self::Disarm => 1 << 3,
        }
    }

    pub(crate) const ALL: [Self; 4] = [Self::Stun, Self::Root, Self::Silence, Self::Disarm];
}

/// A set of active restrictions, held as a bitmask.
///
/// A mask rather than a list on purpose: union is order-free and idempotent, so combining the restrictions of
/// several overlapping effects cannot depend on iteration order — the same reasoning that made stat stage 3
/// order by magnitude.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ControlMask(u8);

impl ControlMask {
    pub const EMPTY: Self = Self(0);
    const VALID_BITS: u8 = 0b1111;

    #[must_use]
    pub fn from_kinds(kinds: &[ControlKind]) -> Self {
        Self(kinds.iter().fold(0, |bits, kind| bits | kind.bit()))
    }

    #[must_use]
    pub const fn contains(self, kind: ControlKind) -> bool {
        self.0 & kind.bit() != 0
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Reject unknown bits rather than silently masking them: an undefined restriction decoded from a
    /// checkpoint is corruption, not a future feature to tolerate.
    pub(crate) const fn from_bits(bits: u8) -> Option<Self> {
        if bits & !Self::VALID_BITS == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    #[must_use]
    pub fn kinds(self) -> Vec<ControlKind> {
        ControlKind::ALL
            .into_iter()
            .filter(|kind| self.contains(*kind))
            .collect()
    }
}

/// Monotonic never-reused identity for one applied status effect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StatusEffectId(pub u64);

impl StatusEffectId {
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Read-only projection of one active status effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusEffectView {
    pub id: StatusEffectId,
    pub expires_at: Tick,
    pub controls: ControlMask,
    pub modifiers: Vec<ModifierId>,
}

/// One modifier bound to one actor. Lifetime is deliberately absent: this layer resolves *how* stats
/// combine. Timed expiry, stacking rules, and dispel belong to the status-effect layer above it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatModifier {
    pub id: ModifierId,
    pub kind: StatKind,
    pub op: ModifierOp,
}

fn resolve_stat(base: u32, kind: StatKind, modifiers: &[StatModifier]) -> u32 {
    let (minimum, maximum) = kind.domain();
    let mut flat_sum: i64 = 0;
    let mut percent_add: i64 = 0;
    let mut override_value: Option<(ModifierId, u32)> = None;

    // Stage 1 and 2: commutative sums, plus the highest-ID override.
    for modifier in modifiers.iter().filter(|entry| entry.kind == kind) {
        match modifier.op {
            ModifierOp::Flat { magnitude } => {
                flat_sum = flat_sum.saturating_add(i64::from(magnitude));
            }
            ModifierOp::PercentAdd { bps } => {
                percent_add = percent_add.saturating_add(i64::from(bps));
            }
            ModifierOp::PercentMult { .. } => {}
            ModifierOp::Override { value } => {
                if override_value.is_none_or(|(current, _)| modifier.id > current) {
                    override_value = Some((modifier.id, value));
                }
            }
        }
    }

    let mut value = i64::from(base).saturating_add(flat_sum);
    let combined = i64::from(BASIS_POINTS).saturating_add(percent_add).max(0);
    value = value.saturating_mul(combined) / i64::from(BASIS_POINTS);

    // Stage 3: sequential, so it needs a total order — and the order must be derived from the modifier SET,
    // not from when each was applied, or the same buffs in a different sequence would resolve differently.
    // Sorting by magnitude achieves that; the ID tiebreak only makes the sort total, and equal magnitudes
    // commute exactly so it can never change the result.
    let mut multiplicative: Vec<(i32, ModifierId)> = modifiers
        .iter()
        .filter(|entry| entry.kind == kind)
        .filter_map(|entry| match entry.op {
            ModifierOp::PercentMult { bps } => Some((bps, entry.id)),
            _ => None,
        })
        .collect();
    multiplicative.sort_unstable();
    for (bps, _) in multiplicative {
        let factor = i64::from(BASIS_POINTS)
            .saturating_add(i64::from(bps))
            .max(0);
        value = value.saturating_mul(factor) / i64::from(BASIS_POINTS);
    }

    // Stage 4 then 5.
    if let Some((_, replacement)) = override_value {
        value = i64::from(replacement);
    }
    u32::try_from(value.clamp(minimum, maximum)).unwrap_or(0)
}

/// Damage channel used to select a target's compiled mitigation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DamageSchool {
    Physical,
    Magic,
    True,
}

/// Legal relationship between caster and affected unit.
///
/// For a unit-aimed ability this filters the named target. For a point-aimed ability it filters every actor
/// the impact footprint covers, so the same vocabulary decides friendly fire for an area effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbilityTargeting {
    SelfOnly,
    Ally,
    Enemy,
    AnyUnit,
}

/// What form of target a cast command must carry.
///
/// This is the *form* axis; [`AbilityTargeting`] is the *relation* axis. Keeping them separate is what lets
/// one ground-targeted ability heal allies and another damage enemies without duplicating either enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbilityAim {
    /// The command names a unit (or the caster). Range is measured to that unit.
    Unit,
    /// The command names a map point. Range is measured to that point.
    ///
    /// A direction is *derived* from caster→point rather than carried separately: every real touch client
    /// sends a world point under the finger, and a separate direction vector would be a second
    /// representation of the same intent that could disagree with it.
    Point,
}

/// How an admitted ability effect reaches its victims.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbilityDelivery {
    /// Lands on the cast's resolution tick, at the named unit or point.
    Instant,
    /// Spawns a travelling entity. It resolves on the first legal actor its swept path touches, or at the
    /// end of its flight. It is deliberately **not** homing: the flight line is fixed at launch, so a
    /// skillshot can be dodged and can strike a body that steps into it.
    Projectile {
        speed_mm_per_tick: u32,
        hit_radius_mm: u32,
    },
}

/// The footprint one admitted effect covers when it lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImpactShape {
    /// Exactly the unit the cast named, or the single body a projectile struck.
    Single,
    /// Every actor within `radius_mm` of the impact point that satisfies the ability's relation filter.
    Circle { radius_mm: u32 },
}

/// MOB-1's bounded effect set. Later gates extend this centrally with statuses and summons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AbilityEffect {
    Damage { amount: u32, school: DamageSchool },
    Heal { amount: u32 },
}

/// Immutable compiled ability definition shared by server, client prediction, and replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AbilitySpec {
    pub id: AbilityId,
    pub aim: AbilityAim,
    pub targeting: AbilityTargeting,
    pub range_mm: u32,
    pub resource_cost: u32,
    pub cooldown_ticks: u32,
    pub cast_ticks: u16,
    pub delivery: AbilityDelivery,
    pub impact: ImpactShape,
    pub effect: AbilityEffect,
    /// Added to the effect magnitude for each rank above the first.
    ///
    /// The magnitude is resolved **when the cast lands or the projectile launches**, never re-read later.
    /// That is what keeps a rank-up from retroactively rewriting a missile already in the air, which would
    /// contradict the "decided at launch" contract the projectile is built on.
    pub per_rank_amount: u32,
}

impl AbilitySpec {
    /// A unit-targeted, instantly-resolving, single-victim ability — the MOB-1 shape.
    ///
    /// Every ability authored before aim/delivery/impact existed had exactly these three values, so this
    /// constructor is what keeps the older call sites honest instead of spraying three defaults through them.
    #[must_use]
    pub const fn unit_targeted(
        id: AbilityId,
        targeting: AbilityTargeting,
        range_mm: u32,
        resource_cost: u32,
        cooldown_ticks: u32,
        cast_ticks: u16,
        effect: AbilityEffect,
    ) -> Self {
        Self {
            id,
            aim: AbilityAim::Unit,
            targeting,
            range_mm,
            resource_cost,
            cooldown_ticks,
            cast_ticks,
            delivery: AbilityDelivery::Instant,
            impact: ImpactShape::Single,
            effect,
            per_rank_amount: 0,
        }
    }

    /// The effect magnitude this ability produces at `rank`.
    ///
    /// Saturating rather than wrapping, and bounded by the authored per-rank ceiling, so a maxed ability
    /// cannot overflow into a trivial number.
    #[must_use]
    pub fn amount_at_rank(self, rank: u8) -> u32 {
        let base = match self.effect {
            AbilityEffect::Damage { amount, .. } | AbilityEffect::Heal { amount } => amount,
        };
        let steps = u32::from(rank.saturating_sub(1));
        base.saturating_add(self.per_rank_amount.saturating_mul(steps))
    }

    pub(crate) fn validate(self) -> Result<(), RuntimeError> {
        let amount = match self.effect {
            AbilityEffect::Damage { amount, .. } | AbilityEffect::Heal { amount } => amount,
        };
        if amount == 0 {
            return Err(RuntimeError::InvalidAbility(
                self.id,
                "effect amount must be positive",
            ));
        }
        if matches!(self.aim, AbilityAim::Point)
            && matches!(self.targeting, AbilityTargeting::SelfOnly)
        {
            return Err(RuntimeError::InvalidAbility(
                self.id,
                "a point-aimed ability cannot be self-only",
            ));
        }
        // An instant point cast with a single-victim impact names no unit and covers no area, so it could
        // never affect anything. Refusing it at authoring time is the difference between a designer seeing
        // an error and a player seeing an ability that silently does nothing.
        if matches!(self.aim, AbilityAim::Point)
            && matches!(self.delivery, AbilityDelivery::Instant)
            && matches!(self.impact, ImpactShape::Single)
        {
            return Err(RuntimeError::InvalidAbility(
                self.id,
                "an instant point-aimed ability needs an area impact to reach anything",
            ));
        }
        if let AbilityDelivery::Projectile {
            speed_mm_per_tick,
            hit_radius_mm,
        } = self.delivery
        {
            if speed_mm_per_tick == 0 || speed_mm_per_tick > MAX_PROJECTILE_SPEED_MM_PER_TICK {
                return Err(RuntimeError::InvalidAbility(
                    self.id,
                    "projectile speed must be positive and within the arithmetic safety ceiling",
                ));
            }
            if hit_radius_mm == 0 || hit_radius_mm > MAX_PROJECTILE_RADIUS_MM {
                return Err(RuntimeError::InvalidAbility(
                    self.id,
                    "projectile hit radius must be positive and within the arithmetic safety ceiling",
                ));
            }
            if self.range_mm == 0 {
                return Err(RuntimeError::InvalidAbility(
                    self.id,
                    "a projectile ability needs a positive range to travel",
                ));
            }
        }
        if let ImpactShape::Circle { radius_mm } = self.impact {
            if radius_mm == 0 || radius_mm > MAX_IMPACT_RADIUS_MM {
                return Err(RuntimeError::InvalidAbility(
                    self.id,
                    "an area impact radius must be positive and within the arithmetic safety ceiling",
                ));
            }
        }
        if self.per_rank_amount > MAX_ABILITY_PER_RANK_AMOUNT {
            return Err(RuntimeError::InvalidAbility(
                self.id,
                "per-rank magnitude exceeds its hard ceiling",
            ));
        }
        Ok(())
    }

    /// The flight speed and hit radius of this ability's projectile, if it has one.
    pub(crate) const fn projectile(self) -> Option<(u32, u32)> {
        match self.delivery {
            AbilityDelivery::Instant => None,
            AbilityDelivery::Projectile {
                speed_mm_per_tick,
                hit_radius_mm,
            } => Some((speed_mm_per_tick, hit_radius_mm)),
        }
    }
}

/// Cooked one-shot basic attack. Repeated attacks are explicit intents rather than an implicit auto-loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BasicAttackSpec {
    pub range_mm: u32,
    pub damage: u32,
    pub school: DamageSchool,
    pub windup_ticks: u16,
    pub cooldown_ticks: u32,
}

impl BasicAttackSpec {
    pub(crate) const fn validate(self) -> Result<(), RuntimeError> {
        if self.range_mm == 0 {
            return Err(RuntimeError::InvalidActor(
                "basic-attack range must be positive",
            ));
        }
        if self.damage == 0 {
            return Err(RuntimeError::InvalidActor(
                "basic-attack damage must be positive",
            ));
        }
        Ok(())
    }
}

/// Cooked lifecycle behavior after authoritative death classification.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeathRule {
    #[default]
    StayDead,
    Despawn,
    Respawn {
        delay_ticks: u32,
        at: Vec2Mm,
    },
    ObjectiveVictory {
        winner: TeamId,
    },
}

/// Immutable creation provenance carried by authoritative actor state and deterministic evidence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActorProvenance {
    #[default]
    Authored,
    Dynamic {
        spawn_ordinal: u64,
    },
    Wave {
        wave_id: u32,
        unit_slot: u16,
        spawn_ordinal: u64,
    },
}

/// Provenance request for a server-spawned actor. The runtime supplies the immutable monotonic ordinal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DynamicActorProvenance {
    #[default]
    Generic,
    Wave {
        wave_id: u32,
        unit_slot: u16,
    },
}

/// One actor in the cooked initial match state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActorSpawn {
    pub id: ActorId,
    pub owner: Option<PlayerId>,
    pub team: TeamId,
    pub kind: ActorKind,
    pub position: Vec2Mm,
    pub stats: CombatStats,
    pub growth: StatGrowth,
    pub bounty: Bounty,
    pub abilities: Vec<AbilityId>,
    pub basic_attack: Option<BasicAttackSpec>,
    pub death_rule: DeathRule,
}

/// Runtime-spawned unowned actor template. The authoritative allocator supplies its never-reused ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DynamicActorSpawn {
    pub provenance: DynamicActorProvenance,
    pub team: TeamId,
    pub kind: ActorKind,
    pub position: Vec2Mm,
    pub stats: CombatStats,
    pub growth: StatGrowth,
    pub bounty: Bounty,
    pub abilities: Vec<AbilityId>,
    pub basic_attack: Option<BasicAttackSpec>,
    pub death_rule: DeathRule,
}

/// Bounded match settings. Content may override these only through a validated target profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchConfig {
    pub tick_rate_hz: u16,
    pub map_bounds: RectMm,
    pub max_players: u32,
    pub max_actors: u32,
    pub max_abilities_per_actor: u32,
    pub max_commands_per_tick: u32,
    pub max_commands_per_player_per_tick: u32,
    pub max_rejection_events_per_frame: u32,
    pub max_command_lead_ticks: u16,
    pub max_match_ticks: Tick,
    pub max_lanes: u16,
    pub max_lane_points_per_lane: u16,
    pub max_wave_schedules: u16,
    pub max_units_per_wave: u16,
    pub max_modifiers_per_actor: u16,
    pub max_status_effects_per_actor: u16,
    pub max_projectiles: u16,
    pub max_score_entries: u16,
    pub max_damage_credits_per_actor: u16,
    pub assist_window_ticks: u32,
    pub xp_share_radius_mm: u32,
    pub passive_gold_per_100s: u32,
    // New budgets are inserted BEFORE this field on purpose: `truncation_and_both_size_ceilings_fail_closed`
    // locates the checkpoint-byte ceiling by walking `encode_config` and subtracting four, on the stated
    // premise that this is the last field written. Appending after it silently retargets that test.
    pub max_checkpoint_bytes: u32,
}

impl MatchConfig {
    /// Reference profile for the first mobile competitive slice. It is a validation target, not a measured
    /// performance claim.
    #[must_use]
    pub const fn competitive_mobile() -> Self {
        Self {
            tick_rate_hz: 30,
            map_bounds: RectMm::new(
                Vec2Mm::new(-500_000, -500_000),
                Vec2Mm::new(500_000, 500_000),
            ),
            max_players: 16,
            max_actors: 512,
            max_abilities_per_actor: 8,
            max_commands_per_tick: 128,
            max_commands_per_player_per_tick: 8,
            max_rejection_events_per_frame: 128,
            max_command_lead_ticks: 8,
            max_match_ticks: 30 * 60 * 60,
            max_lanes: 1,
            max_lane_points_per_lane: 64,
            max_wave_schedules: 8,
            max_units_per_wave: 16,
            max_modifiers_per_actor: 16,
            max_status_effects_per_actor: 8,
            max_projectiles: 64,
            max_score_entries: 16,
            max_damage_credits_per_actor: 8,
            // 15 s at 30 Hz - League of Legends' documented Summoner's Rift assist window, the only such
            // duration any source states numerically.
            assist_window_ticks: 450,
            xp_share_radius_mm: 8_000,
            // 20.4 gold per 10 s, League's shipped base income, expressed per 100 s to stay integral.
            passive_gold_per_100s: 204,
            max_checkpoint_bytes: 4 * 1024 * 1024,
        }
    }

    pub(crate) fn validate(self) -> Result<(), RuntimeError> {
        if self.tick_rate_hz == 0 {
            return Err(RuntimeError::InvalidConfig("tick rate must be positive"));
        }
        if !self.map_bounds.is_valid() {
            return Err(RuntimeError::InvalidConfig(
                "map bounds must have positive area",
            ));
        }
        if self.max_players == 0
            || self.max_actors == 0
            || self.max_abilities_per_actor == 0
            || self.max_commands_per_tick == 0
            || self.max_commands_per_player_per_tick == 0
            || self.max_rejection_events_per_frame == 0
            || self.max_lanes == 0
            || self.max_lane_points_per_lane < 2
            || self.max_wave_schedules == 0
            || self.max_units_per_wave == 0
            || self.max_modifiers_per_actor == 0
            || self.max_status_effects_per_actor == 0
            || self.max_projectiles == 0
            || self.max_score_entries == 0
            || self.max_damage_credits_per_actor == 0
            || self.assist_window_ticks == 0
            || self.xp_share_radius_mm == 0
            // `passive_gold_per_100s` is deliberately NOT in this chain: zero passive income is a
            // legitimate profile, and folding it in would make an income stream silently mandatory.
            || self.max_checkpoint_bytes == 0
        {
            return Err(RuntimeError::InvalidConfig(
                "runtime and authored-content budgets must be positive",
            ));
        }
        if self.max_commands_per_player_per_tick > self.max_commands_per_tick {
            return Err(RuntimeError::InvalidConfig(
                "per-player command budget cannot exceed the global tick budget",
            ));
        }
        if self.max_players > MAX_PLAYER_BUDGET
            || self.max_actors > MAX_ACTOR_BUDGET
            || self.max_abilities_per_actor > MAX_ABILITIES_PER_ACTOR_BUDGET
            || self.max_commands_per_tick > MAX_COMMANDS_PER_TICK_BUDGET
            || self.max_commands_per_player_per_tick > MAX_COMMANDS_PER_PLAYER_PER_TICK_BUDGET
            || self.max_rejection_events_per_frame > MAX_REJECTION_EVENTS_PER_FRAME_BUDGET
            || self.max_command_lead_ticks > MAX_COMMAND_LEAD_TICKS
            || self.max_lanes > MAX_LANE_BUDGET
            || self.max_lane_points_per_lane > MAX_LANE_POINTS_PER_LANE_BUDGET
            || self.max_wave_schedules > MAX_WAVE_SCHEDULE_BUDGET
            || self.max_units_per_wave > MAX_UNITS_PER_WAVE_BUDGET
            || self.max_modifiers_per_actor > MAX_MODIFIERS_PER_ACTOR_BUDGET
            || self.max_status_effects_per_actor > MAX_STATUS_EFFECTS_PER_ACTOR_BUDGET
            || self.max_projectiles > MAX_PROJECTILE_BUDGET
            || self.max_score_entries > MAX_SCORE_ENTRY_BUDGET
            || self.max_damage_credits_per_actor > MAX_DAMAGE_CREDITS_PER_ACTOR_BUDGET
            || self.assist_window_ticks > MAX_ASSIST_WINDOW_TICKS
            || self.xp_share_radius_mm > MAX_XP_SHARE_RADIUS_MM
            || self.passive_gold_per_100s > MAX_PASSIVE_GOLD_PER_100S
        {
            return Err(RuntimeError::InvalidConfig(
                "target profile exceeds a hard decoded-state budget",
            ));
        }
        if self.max_command_lead_ticks == 0 {
            return Err(RuntimeError::InvalidConfig(
                "command lead window must be positive",
            ));
        }
        if self.max_match_ticks == 0 {
            return Err(RuntimeError::InvalidConfig(
                "maximum match duration must be positive",
            ));
        }
        if self.max_match_ticks > MAX_MATCH_TICKS {
            return Err(RuntimeError::InvalidConfig(
                "maximum match duration exceeds the hard runtime ceiling",
            ));
        }
        if self.max_checkpoint_bytes > MAX_RUNTIME_CHECKPOINT_BYTES {
            return Err(RuntimeError::InvalidConfig(
                "checkpoint budget exceeds the 16 MiB hard ceiling",
            ));
        }
        Ok(())
    }
}

impl Default for MatchConfig {
    fn default() -> Self {
        Self::competitive_mobile()
    }
}

/// High-level authoritative match lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchPhase {
    Setup,
    Active,
    Complete {
        outcome: MatchOutcome,
        reason: MatchEndReason,
    },
}

/// Legal terminal result. A draw is first-class so simultaneous objectives and the time limit never require
/// the host to invent a winner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchOutcome {
    Winner(TeamId),
    Draw,
}

/// Stable reason for an authoritative terminal result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchEndReason {
    ObjectiveDestroyed,
    TimeLimit,
    Host,
}

/// Invalid authored/runtime setup. Player command failures use `CommandRejectReason` instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeError {
    InvalidConfig(&'static str),
    InvalidAbility(AbilityId, &'static str),
    DuplicateAbility(AbilityId),
    InvalidActor(&'static str),
    DuplicateActor(ActorId),
    PlayerBudgetExceeded,
    ActorBudgetExceeded,
    ActorIdExhausted,
    ModifierIdExhausted,
    StatusEffectIdExhausted,
    UnknownStatusEffect(StatusEffectId),
    UnknownActor(ActorId),
    UnknownModifier(ModifierId),
    UnknownAbility(AbilityId),
    ActorOutOfBounds(ActorId),
    InvalidPhase(&'static str),
    CommandBudgetCannotReserveRoster,
    InvalidInternalIntents(&'static str),
    MatchDurationExceeded,
    TickOverflow,
    ScoreBudgetExceeded,
    UnknownScoreEntry(ActorId),
    InvalidAbilityRank(&'static str),
    InvalidProgression(&'static str),
}

impl Display for RuntimeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfig(reason) => write!(formatter, "invalid match config: {reason}"),
            Self::InvalidAbility(id, reason) => {
                write!(formatter, "invalid ability {}: {reason}", id.get())
            }
            Self::DuplicateAbility(id) => write!(formatter, "duplicate ability {}", id.get()),
            Self::InvalidActor(reason) => write!(formatter, "invalid actor: {reason}"),
            Self::DuplicateActor(id) => write!(formatter, "duplicate actor {}", id.get()),
            Self::PlayerBudgetExceeded => formatter.write_str("player roster budget exceeded"),
            Self::ActorBudgetExceeded => formatter.write_str("actor budget exceeded"),
            Self::ActorIdExhausted => formatter.write_str("dynamic actor ID space exhausted"),
            Self::ModifierIdExhausted => formatter.write_str("stat modifier ID space exhausted"),
            Self::StatusEffectIdExhausted => {
                formatter.write_str("status effect ID space exhausted")
            }
            Self::UnknownStatusEffect(id) => {
                write!(formatter, "unknown status effect {}", id.get())
            }
            Self::UnknownActor(id) => write!(formatter, "unknown actor {}", id.get()),
            Self::UnknownModifier(id) => write!(formatter, "unknown stat modifier {}", id.get()),
            Self::UnknownAbility(id) => write!(formatter, "unknown ability {}", id.get()),
            Self::ActorOutOfBounds(id) => {
                write!(formatter, "actor {} is outside map bounds", id.get())
            }
            Self::InvalidPhase(reason) => write!(formatter, "invalid match phase: {reason}"),
            Self::CommandBudgetCannotReserveRoster => formatter.write_str(
                "global command budget cannot reserve the per-player quota for the authored roster",
            ),
            Self::InvalidInternalIntents(reason) => {
                write!(formatter, "invalid internal actor intents: {reason}")
            }
            Self::MatchDurationExceeded => {
                formatter.write_str("maximum authoritative match duration reached")
            }
            Self::TickOverflow => formatter.write_str("authoritative tick overflow"),
            Self::ScoreBudgetExceeded => formatter.write_str("scoreboard entry budget exceeded"),
            Self::UnknownScoreEntry(id) => {
                write!(formatter, "no scoreboard entry for actor {}", id.get())
            }
            Self::InvalidAbilityRank(reason) => {
                write!(formatter, "invalid ability rank: {reason}")
            }
            Self::InvalidProgression(reason) => {
                write!(formatter, "invalid progression: {reason}")
            }
        }
    }
}

impl Error for RuntimeError {}
