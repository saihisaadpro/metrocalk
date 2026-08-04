// Implement this feature in accordance with the Engine UI/UX Architecture Constitution.

//! Public-API resilience evidence for the deterministic gameplay kernel.
//!
//! The ordinary test proves reproducibility and state-shape invariants over a bounded command mix. It is
//! not a throughput claim. The full-duration/full-actor test is deliberately release-only and ignored: it
//! becomes evidence only when a human or CI gate explicitly runs that exact test and records the result.

use std::time::Instant;

use metrocalk_gameplay::{
    AbilityAim, AbilityDelivery, AbilityEffect, AbilityId, AbilitySpec, AbilityTargeting, ActorId,
    ActorIntent, ActorKind, ActorSpawn, BasicAttackSpec, Bounty, CastTarget, CombatStats,
    CommandKind, CommandReceipt, CommandRejection, DamageSchool, DeathRule, FrameDigest,
    ImpactShape, MatchConfig, MatchEndReason, MatchOutcome, MatchPhase, MatchRuntime,
    PlayerCommand, PlayerId, ServerFrame, StatGrowth, TeamId, Vec2Mm, WorldDigest,
};

const RESILIENCE_SEED: u64 = 0x6d6f_6232_5f72_6573;
const RESILIENCE_TICKS: u64 = 256;
const PLAYER_ONE: PlayerId = PlayerId(11);
const PLAYER_TWO: PlayerId = PlayerId(22);
const UNKNOWN_PLAYER: PlayerId = PlayerId(9_999);
const HERO_ONE: ActorId = ActorId(101);
const HERO_TWO: ActorId = ActorId(202);
const STRIKE: AbilityId = AbilityId(7);

const SOAK_TICKS: u64 = 108_000;
const SOAK_ACTORS: u64 = 512;
const PERFORMANCE_WARMUP_TICKS: u64 = 1_000;
const PERFORMANCE_SAMPLE_TICKS: usize = 10_000;
const PERFORMANCE_P99_BUDGET_NS: u64 = 10_000_000;

#[derive(Clone, Copy)]
struct SeededIntegers(u64);

impl SeededIntegers {
    const fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u32(&mut self) -> u32 {
        // A fixed LCG is sufficient here: this is reproducible input generation, not gameplay randomness.
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 32) as u32
    }

    fn coordinate(&mut self) -> i32 {
        i32::try_from(self.next_u32() % 10_001).expect("bounded coordinate fits i32") - 5_000
    }
}

#[derive(Debug, PartialEq, Eq)]
struct ResilienceEvidence {
    submit_outcomes: Vec<Result<CommandReceipt, CommandRejection>>,
    frames: Vec<ServerFrame>,
    accepted: usize,
    rejected: usize,
}

fn stats(health: u32, resource: u32, speed: u32) -> CombatStats {
    CombatStats {
        max_health: health,
        max_resource: resource,
        move_speed_mm_per_tick: speed,
        physical_reduction_bps: 0,
        magic_reduction_bps: 0,
    }
}

fn resilience_runtime() -> MatchRuntime {
    let mut config = MatchConfig::competitive_mobile();
    config.max_match_ticks = RESILIENCE_TICKS + 16;

    let mut runtime = MatchRuntime::new(config, RESILIENCE_SEED).expect("valid resilience config");
    runtime
        .register_ability(AbilitySpec {
            id: STRIKE,
            aim: AbilityAim::Unit,
            delivery: AbilityDelivery::Instant,
            impact: ImpactShape::Single,
            targeting: AbilityTargeting::Enemy,
            range_mm: 50_000,
            resource_cost: 3,
            cooldown_ticks: 3,
            cast_ticks: 1,
            per_rank_amount: 0,
            effect: AbilityEffect::Damage {
                amount: 7,
                school: DamageSchool::Magic,
            },
        })
        .expect("valid strike definition");

    let attack = BasicAttackSpec {
        range_mm: 50_000,
        damage: 5,
        school: DamageSchool::Physical,
        windup_ticks: 1,
        cooldown_ticks: 2,
    };
    for actor in [
        ActorSpawn {
            id: HERO_ONE,
            owner: Some(PLAYER_ONE),
            team: TeamId(0),
            kind: ActorKind::Hero,
            position: Vec2Mm::new(-1_000, 0),
            growth: StatGrowth::NONE,
            bounty: Bounty::NONE,
            stats: stats(1_000_000, 1_000_000, 37),
            abilities: vec![STRIKE],
            basic_attack: Some(attack),
            death_rule: DeathRule::StayDead,
        },
        ActorSpawn {
            id: HERO_TWO,
            owner: Some(PLAYER_TWO),
            team: TeamId(1),
            kind: ActorKind::Hero,
            position: Vec2Mm::new(1_000, 0),
            growth: StatGrowth::NONE,
            bounty: Bounty::NONE,
            stats: stats(1_000_000, 1_000_000, 41),
            abilities: vec![STRIKE],
            basic_attack: Some(attack),
            death_rule: DeathRule::StayDead,
        },
    ] {
        runtime.spawn_actor(&actor).expect("valid resilience actor");
    }
    runtime
}

fn record_submission(
    runtime: &mut MatchRuntime,
    command: PlayerCommand,
    outcomes: &mut Vec<Result<CommandReceipt, CommandRejection>>,
) {
    outcomes.push(runtime.submit(command));
}

fn generate_primary_command(
    generator: &mut SeededIntegers,
    logical_tick: u64,
    current_tick: u64,
    player_one_sequence: &mut u32,
    player_two_sequence: &mut u32,
    unknown_sequence: &mut u32,
) -> PlayerCommand {
    let first_player = generator.next_u32() & 1 == 0;
    let (player, owned_actor, enemy_actor, sequence) = if first_player {
        *player_one_sequence = player_one_sequence
            .checked_add(1)
            .expect("bounded sequence");
        (PLAYER_ONE, HERO_ONE, HERO_TWO, *player_one_sequence)
    } else {
        *player_two_sequence = player_two_sequence
            .checked_add(1)
            .expect("bounded sequence");
        (PLAYER_TWO, HERO_TWO, HERO_ONE, *player_two_sequence)
    };

    let selector = if logical_tick == 0 {
        0
    } else {
        generator.next_u32() % 12
    };
    let mut command = PlayerCommand {
        player,
        sequence,
        execute_at: current_tick + 1,
        actor: owned_actor,
        kind: CommandKind::Stop,
    };
    match selector {
        0 => {}
        1 => {
            command.kind = CommandKind::MoveTo {
                destination: Vec2Mm::new(generator.coordinate(), generator.coordinate()),
            };
        }
        2 => {
            command.kind = CommandKind::Cast {
                ability: STRIKE,
                target: CastTarget::Actor(enemy_actor),
            };
        }
        3 => {
            command.kind = CommandKind::BasicAttack {
                target: enemy_actor,
            }
        }
        4 => command.actor = enemy_actor,
        5 => {
            command.kind = CommandKind::MoveTo {
                destination: Vec2Mm::new(600_000, 0),
            };
        }
        6 => command.execute_at = current_tick,
        7 => command.execute_at = current_tick + 9,
        8 => {
            *unknown_sequence = unknown_sequence.checked_add(1).expect("bounded sequence");
            command.player = UNKNOWN_PLAYER;
            command.sequence = *unknown_sequence;
        }
        9 => {
            command.kind = CommandKind::Cast {
                ability: AbilityId(999),
                target: CastTarget::Actor(enemy_actor),
            };
        }
        10 => {
            command.kind = CommandKind::BasicAttack {
                target: owned_actor,
            }
        }
        11 => command.actor = ActorId(99_999),
        _ => unreachable!("selector is reduced modulo twelve"),
    }
    command
}

fn run_seeded_command_mix() -> ResilienceEvidence {
    let mut runtime = resilience_runtime();
    let mut frames = Vec::with_capacity(
        usize::try_from(RESILIENCE_TICKS + 1).expect("bounded frame count fits usize"),
    );
    frames.push(runtime.start().expect("resilience match starts"));
    runtime
        .check_invariants()
        .expect("baseline satisfies public invariants");

    // At most three outcomes per tick are retained. This deliberate bound makes exact cross-run comparison
    // useful without turning the ordinary test into an unbounded replay/soak collector.
    let mut submit_outcomes = Vec::with_capacity(
        usize::try_from(RESILIENCE_TICKS * 3).expect("bounded outcome count fits usize"),
    );
    let mut generator = SeededIntegers::new(RESILIENCE_SEED);
    let mut player_one_sequence = 0_u32;
    let mut player_two_sequence = 0_u32;
    let mut unknown_sequence = 0_u32;

    for logical_tick in 0..RESILIENCE_TICKS {
        let current_tick = runtime.tick();
        let command = generate_primary_command(
            &mut generator,
            logical_tick,
            current_tick,
            &mut player_one_sequence,
            &mut player_two_sequence,
            &mut unknown_sequence,
        );

        record_submission(&mut runtime, command, &mut submit_outcomes);

        // Exact retries are part of the protocol contract and must reproduce the cached result without
        // introducing a second mutation or causal event.
        if logical_tick % 23 == 0 && command.player != UNKNOWN_PLAYER {
            record_submission(&mut runtime, command, &mut submit_outcomes);
        }

        // This scheduled invalid envelope guarantees rejection coverage independent of the generated mix.
        if logical_tick % 31 == 0 {
            unknown_sequence = unknown_sequence.checked_add(1).expect("bounded sequence");
            record_submission(
                &mut runtime,
                PlayerCommand {
                    player: UNKNOWN_PLAYER,
                    sequence: unknown_sequence,
                    execute_at: current_tick + 1,
                    actor: HERO_ONE,
                    kind: CommandKind::Stop,
                },
                &mut submit_outcomes,
            );
        }

        let frame = runtime.step().expect("bounded resilience tick advances");
        runtime
            .check_invariants()
            .unwrap_or_else(|error| panic!("invariant failure after tick {}: {error}", frame.tick));
        frames.push(frame);
    }

    let accepted = submit_outcomes
        .iter()
        .filter(|outcome| outcome.is_ok())
        .count();
    let rejected = submit_outcomes.len() - accepted;
    ResilienceEvidence {
        submit_outcomes,
        frames,
        accepted,
        rejected,
    }
}

fn digests(frames: &[ServerFrame]) -> Vec<(WorldDigest, FrameDigest)> {
    frames
        .iter()
        .map(|frame| (frame.world_digest, frame.frame_digest))
        .collect()
}

#[test]
fn seeded_command_mix_is_bounded_reproducible_and_invariant_safe() {
    let first = run_seeded_command_mix();
    let second = run_seeded_command_mix();

    assert!(
        first.accepted > 0,
        "the mix must exercise accepted commands"
    );
    assert!(
        first.rejected > 0,
        "the mix must exercise rejected commands"
    );
    assert_eq!(first.submit_outcomes, second.submit_outcomes);
    assert_eq!(first.frames, second.frames);
    assert_eq!(digests(&first.frames), digests(&second.frames));
    assert_eq!(
        first.frames.len(),
        usize::try_from(RESILIENCE_TICKS + 1).expect("bounded frame count fits usize")
    );
}

fn fold_digest(accumulator: u64, frame: &ServerFrame) -> u64 {
    accumulator
        .wrapping_mul(1_099_511_628_211)
        .wrapping_add(frame.world_digest.0)
        .rotate_left(13)
        ^ frame.frame_digest.0
}

struct FullLoadFixture {
    runtime: MatchRuntime,
    initial_intents: Vec<ActorIntent>,
    actor_count: usize,
    rolling_digest: u64,
}

fn require_release_evidence() {
    assert!(
        !std::hint::black_box(cfg!(debug_assertions)),
        "this evidence boundary must be run with --release"
    );
}

fn full_load_fixture() -> FullLoadFixture {
    let config = MatchConfig::competitive_mobile();
    assert_eq!(config.max_match_ticks, SOAK_TICKS);
    assert_eq!(u64::from(config.max_actors), SOAK_ACTORS);
    let mut runtime = MatchRuntime::new(config, RESILIENCE_SEED).expect("valid full-load config");
    let actor_count = usize::try_from(SOAK_ACTORS).expect("declared actor count fits usize");
    let mut initial_intents = Vec::with_capacity(actor_count);

    for index in 0..SOAK_ACTORS {
        let id = ActorId(index + 1);
        let y = -255_500 + i32::try_from(index).expect("declared actor index fits i32") * 1_000;
        runtime
            .spawn_actor(&ActorSpawn {
                id,
                owner: None,
                team: TeamId(0),
                kind: ActorKind::Minion,
                position: Vec2Mm::new(-400_000, y),
                growth: StatGrowth::NONE,
                bounty: Bounty::NONE,
                stats: stats(100, 0, 1),
                abilities: Vec::new(),
                basic_attack: None,
                death_rule: DeathRule::StayDead,
            })
            .expect("full declared actor budget is valid");
        initial_intents.push(ActorIntent {
            actor: id,
            kind: CommandKind::MoveTo {
                destination: Vec2Mm::new(400_000, y),
            },
        });
    }

    let baseline = runtime.start().expect("full-load match starts");
    assert_eq!(baseline.changed.len(), actor_count);
    runtime
        .check_invariants()
        .expect("full-load baseline satisfies invariants");
    FullLoadFixture {
        runtime,
        initial_intents,
        actor_count,
        rolling_digest: fold_digest(0xcbf2_9ce4_8422_2325, &baseline),
    }
}

fn start_full_load_movement(fixture: &mut FullLoadFixture) {
    let frame = fixture
        .runtime
        .step_with_intents(&fixture.initial_intents)
        .expect("initial full-load movement intents are valid");
    fixture.rolling_digest = fold_digest(fixture.rolling_digest, &frame);
    fixture
        .runtime
        .check_invariants()
        .expect("initial full-load movement satisfies invariants");
    assert_eq!(fixture.runtime.live_actor_count(), fixture.actor_count);
}

fn nearest_rank(sorted_samples: &[u64], percentile: u64) -> u64 {
    assert!(!sorted_samples.is_empty());
    assert!((1..=100).contains(&percentile));
    let count = u64::try_from(sorted_samples.len()).expect("sample count fits u64");
    let rank = count
        .checked_mul(percentile)
        .expect("bounded percentile multiplication")
        .div_ceil(100);
    let index = usize::try_from(rank - 1).expect("nearest-rank index fits usize");
    sorted_samples[index]
}

#[test]
#[ignore = "release-only full-duration/full-actor gate; run explicitly with cargo test --release -p metrocalk-gameplay --test mob2_resilience -- --ignored"]
fn release_only_108000_tick_512_live_actor_constant_memory_soak() {
    require_release_evidence();
    let mut fixture = full_load_fixture();
    start_full_load_movement(&mut fixture);

    // Frames are folded and dropped immediately. No per-tick frame/checkpoint history is retained, so peak
    // test-owned memory is O(actor count), not O(actor count * 108,000 ticks).
    for tick in 2..=SOAK_TICKS {
        let frame = fixture
            .runtime
            .step()
            .expect("full-duration soak tick advances");
        fixture
            .runtime
            .check_invariants()
            .unwrap_or_else(|error| panic!("invariant failure after soak tick {tick}: {error}"));
        assert_eq!(fixture.runtime.live_actor_count(), fixture.actor_count);
        fixture.rolling_digest = fold_digest(fixture.rolling_digest, &frame);
    }

    assert_eq!(fixture.runtime.tick(), SOAK_TICKS);
    assert_eq!(
        fixture.runtime.phase(),
        MatchPhase::Complete {
            outcome: MatchOutcome::Draw,
            reason: MatchEndReason::TimeLimit,
        }
    );
    assert_ne!(fixture.rolling_digest, 0xcbf2_9ce4_8422_2325);
}

#[test]
#[ignore = "release-only 512-actor tick-work gate; run explicitly with cargo test --release -p metrocalk-gameplay --test mob2_resilience release_only_512_actor_tick_work_p99_gate -- --ignored --exact --nocapture"]
fn release_only_512_actor_tick_work_p99_gate() {
    require_release_evidence();
    let mut fixture = full_load_fixture();
    start_full_load_movement(&mut fixture);

    for _ in 0..PERFORMANCE_WARMUP_TICKS {
        let frame = fixture.runtime.step().expect("warmup tick advances");
        fixture.rolling_digest = fold_digest(fixture.rolling_digest, &frame);
    }
    fixture
        .runtime
        .check_invariants()
        .expect("warmup preserves runtime invariants");

    // Only MatchRuntime::step is inside the measured interval. Samples are one fixed 10,000-u64 allocation;
    // each frame is folded and dropped before the next tick, so evidence memory cannot grow with frame size.
    let mut samples_ns = Vec::with_capacity(PERFORMANCE_SAMPLE_TICKS);
    for _ in 0..PERFORMANCE_SAMPLE_TICKS {
        let started = Instant::now();
        let frame = fixture.runtime.step().expect("measured tick advances");
        let elapsed_ns = u64::try_from(started.elapsed().as_nanos())
            .expect("one measured tick duration fits u64 nanoseconds");
        samples_ns.push(elapsed_ns);
        fixture.rolling_digest = fold_digest(fixture.rolling_digest, &frame);
    }
    fixture
        .runtime
        .check_invariants()
        .expect("measured sample preserves runtime invariants");
    assert_eq!(fixture.runtime.live_actor_count(), fixture.actor_count);

    samples_ns.sort_unstable();
    let p50_ns = nearest_rank(&samples_ns, 50);
    let p95_ns = nearest_rank(&samples_ns, 95);
    let p99_ns = nearest_rank(&samples_ns, 99);
    let max_ns = *samples_ns.last().expect("declared sample is non-empty");
    println!(
        "{{\"schema\":\"metrocalk.gameplay.tick-work.v1\",\"actors\":{},\"warmup_ticks\":{},\"sample_ticks\":{},\"unit\":\"ns\",\"p50_ns\":{},\"p95_ns\":{},\"p99_ns\":{},\"max_ns\":{},\"p99_budget_ns\":{},\"rolling_digest\":\"{:016x}\"}}",
        fixture.actor_count,
        PERFORMANCE_WARMUP_TICKS,
        PERFORMANCE_SAMPLE_TICKS,
        p50_ns,
        p95_ns,
        p99_ns,
        max_ns,
        PERFORMANCE_P99_BUDGET_NS,
        fixture.rolling_digest,
    );
    assert!(
        p99_ns <= PERFORMANCE_P99_BUDGET_NS,
        "512-actor p99 tick work {p99_ns} ns exceeds the 10 ms provisional budget"
    );
}
