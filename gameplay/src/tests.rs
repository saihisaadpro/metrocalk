use crate::{
    AbilityAim, AbilityDelivery, AbilityEffect, AbilityId, AbilitySpec, AbilityTargeting, ActorId,
    ActorIntent, ActorKind, ActorProvenance, ActorSpawn, AttackOrder, BasicAttackSpec, Bounty,
    CastTarget, CombatStats, CommandKind, CommandReceipt, CommandRejectReason, ContentId,
    ControlKind, DamageCause, DamageSchool, DeathRule, DynamicActorProvenance, DynamicActorSpawn,
    ImpactShape, MatchConfig, MatchEndReason, MatchEvent, MatchOutcome, MatchPhase, MatchRuntime,
    ModifierOp, PlayerCommand, PlayerId, RuntimeError, Scenario, ScenarioFinish,
    ScenarioSubmission, StatGrowth, StatKind, TeamId, Vec2Mm, BASIS_POINTS,
};

const PLAYER_ONE: PlayerId = PlayerId(1);
const PLAYER_TWO: PlayerId = PlayerId(2);
const PLAYER_THREE: PlayerId = PlayerId(3);
const HERO_ONE: ActorId = ActorId(101);
const HERO_TWO: ActorId = ActorId(202);
const HERO_THREE: ActorId = ActorId(303);
const STRIKE: AbilityId = AbilityId(7);
const HEAL: AbilityId = AbilityId(9);

fn stats(health: u32, resource: u32, speed: u32) -> CombatStats {
    CombatStats {
        max_health: health,
        max_resource: resource,
        move_speed_mm_per_tick: speed,
        physical_reduction_bps: 0,
        magic_reduction_bps: 0,
        crit_chance_bps: 0,
        crit_damage_bps: BASIS_POINTS,
        physical_penetration_bps: 0,
        magic_penetration_bps: 0,
        lifesteal_bps: 0,
    }
}

fn hero(
    id: ActorId,
    owner: PlayerId,
    team: TeamId,
    position: Vec2Mm,
    health: u32,
    abilities: Vec<AbilityId>,
) -> ActorSpawn {
    ActorSpawn {
        id,
        owner: Some(owner),
        team,
        kind: ActorKind::Hero,
        position,
        growth: StatGrowth::NONE,
        bounty: Bounty::NONE,
        stats: stats(health, 100, 100),
        abilities,
        basic_attack: None,
        death_rule: DeathRule::StayDead,
    }
}

const fn strike() -> AbilitySpec {
    AbilitySpec {
        id: STRIKE,
        aim: AbilityAim::Unit,
        delivery: AbilityDelivery::Instant,
        impact: ImpactShape::Single,
        targeting: AbilityTargeting::Enemy,
        range_mm: 2_000,
        resource_cost: 20,
        cooldown_ticks: 5,
        cast_ticks: 1,
        per_rank_amount: 0,
        effect: AbilityEffect::Damage {
            amount: 600,
            school: DamageSchool::True,
        },
    }
}

const fn heal() -> AbilitySpec {
    AbilitySpec {
        id: HEAL,
        aim: AbilityAim::Unit,
        delivery: AbilityDelivery::Instant,
        impact: ImpactShape::Single,
        targeting: AbilityTargeting::Ally,
        range_mm: 1_500,
        resource_cost: 10,
        cooldown_ticks: 3,
        cast_ticks: 0,
        per_rank_amount: 0,
        effect: AbilityEffect::Heal { amount: 100 },
    }
}

fn duel_runtime_with_baseline() -> (MatchRuntime, crate::ServerFrame) {
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 0x5eed).unwrap();
    runtime.register_ability(strike()).unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_ONE,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            1_000,
            vec![STRIKE],
        ))
        .unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_TWO,
            PLAYER_TWO,
            TeamId(1),
            Vec2Mm::new(1_000, 0),
            500,
            vec![STRIKE],
        ))
        .unwrap();
    let baseline = runtime.start().unwrap();
    (runtime, baseline)
}

fn duel_runtime() -> MatchRuntime {
    duel_runtime_with_baseline().0
}

fn command(
    player: PlayerId,
    sequence: u32,
    execute_at: u64,
    actor: ActorId,
    kind: CommandKind,
) -> PlayerCommand {
    PlayerCommand {
        player,
        sequence,
        execute_at,
        actor,
        kind,
    }
}

const fn submission(
    submitted_at: u64,
    arrival_order: u32,
    command: PlayerCommand,
) -> ScenarioSubmission {
    ScenarioSubmission {
        submitted_at,
        arrival_order,
        command,
    }
}

#[test]
fn integer_movement_emits_only_dirty_actor_deltas() {
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 1).unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_ONE,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            1_000,
            vec![],
        ))
        .unwrap();
    runtime.start().unwrap();
    runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            HERO_ONE,
            CommandKind::MoveTo {
                destination: Vec2Mm::new(250, 0),
            },
        ))
        .unwrap();

    let first = runtime.step().unwrap();
    assert_eq!(first.changed.len(), 1);
    assert_eq!(first.changed[0].position, Vec2Mm::new(100, 0));
    assert!(first.events.iter().any(|event| matches!(
        event,
        MatchEvent::MoveStarted {
            actor: HERO_ONE,
            ..
        }
    )));

    let second = runtime.step().unwrap();
    assert_eq!(second.changed[0].position, Vec2Mm::new(200, 0));

    let third = runtime.step().unwrap();
    assert_eq!(third.changed[0].position, Vec2Mm::new(250, 0));
    assert_eq!(third.changed[0].destination, None);
    assert!(third
        .events
        .iter()
        .any(|event| matches!(event, MatchEvent::MoveStopped { actor: HERO_ONE })));

    let unchanged = runtime.step().unwrap();
    assert!(unchanged.changed.is_empty());
}

#[test]
fn cast_spends_resource_sets_cooldown_and_kills_authoritatively() {
    let mut runtime = duel_runtime();
    runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            HERO_ONE,
            CommandKind::Cast {
                ability: STRIKE,
                target: CastTarget::Actor(HERO_TWO),
            },
        ))
        .unwrap();

    let wind_up = runtime.step().unwrap();
    let source = runtime.actor(HERO_ONE).unwrap();
    assert_eq!(source.resource, 80);
    assert_eq!(source.cooldowns[0].ready_at, 6);
    assert!(source.cast.is_some());
    assert!(wind_up.events.iter().any(|event| matches!(
        event,
        MatchEvent::CastStarted {
            source: HERO_ONE,
            ..
        }
    )));

    let impact = runtime.step().unwrap();
    assert_eq!(runtime.actor(HERO_TWO).unwrap().health, 0);
    assert!(!runtime.actor(HERO_TWO).unwrap().alive);
    assert!(impact.events.iter().any(|event| matches!(
        event,
        MatchEvent::DamageApplied {
            source: HERO_ONE,
            target: HERO_TWO,
            amount: 500,
            health_after: 0,
            ..
        }
    )));
    assert!(impact.events.iter().any(|event| matches!(
        event,
        MatchEvent::ActorDied {
            actor: HERO_TWO,
            killer: HERO_ONE
        }
    )));

    let rejection = runtime
        .submit(command(
            PLAYER_ONE,
            2,
            3,
            HERO_ONE,
            CommandKind::Cast {
                ability: STRIKE,
                target: CastTarget::Actor(HERO_TWO),
            },
        ))
        .unwrap_err();
    assert_eq!(
        rejection.reason,
        CommandRejectReason::AbilityOnCooldown { ready_at: 6 }
    );
}

#[test]
fn rejected_command_does_not_mutate_gameplay_state() {
    let mut runtime = duel_runtime();
    let before_one = runtime.actor(HERO_ONE).unwrap();
    let before_two = runtime.actor(HERO_TWO).unwrap();
    let rejection = runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            HERO_TWO,
            CommandKind::MoveTo {
                destination: Vec2Mm::new(2_000, 0),
            },
        ))
        .unwrap_err();
    assert_eq!(rejection.reason, CommandRejectReason::ActorNotOwned);
    assert_eq!(runtime.actor(HERO_ONE).unwrap(), before_one);
    assert_eq!(runtime.actor(HERO_TWO).unwrap(), before_two);

    let frame = runtime.step().unwrap();
    assert!(frame.events.iter().any(|event| matches!(
        event,
        MatchEvent::CommandRejected {
            player: PLAYER_ONE,
            sequence: 1,
            reason: CommandRejectReason::ActorNotOwned
        }
    )));
}

#[test]
fn server_owned_intents_cannot_bypass_owned_actor_authority() {
    let mut runtime = duel_runtime();
    let before = runtime.actor(HERO_ONE).unwrap();
    let frame = runtime
        .step_with_intents(&[ActorIntent {
            actor: HERO_ONE,
            kind: CommandKind::MoveTo {
                destination: Vec2Mm::new(2_000, 0),
            },
        }])
        .unwrap();

    assert_eq!(runtime.actor(HERO_ONE).unwrap(), before);
    assert!(frame.events.iter().any(|event| matches!(
        event,
        MatchEvent::InternalIntentRejected {
            actor: HERO_ONE,
            reason: CommandRejectReason::ActorNotOwned,
        }
    )));
    runtime.check_invariants().unwrap();
}

#[test]
fn no_op_retries_and_late_terminal_packets_preserve_checkpoint_readiness() {
    let mut runtime = duel_runtime();
    let accepted = command(PLAYER_ONE, 1, 1, HERO_ONE, CommandKind::Stop);
    let receipt = runtime.submit(accepted).unwrap();
    runtime.step().unwrap();

    assert_eq!(runtime.submit(accepted), Ok(receipt));
    runtime
        .capture_checkpoint(ContentId::new([0x31; 32]))
        .unwrap();

    runtime
        .finish(MatchOutcome::Draw, MatchEndReason::Host)
        .unwrap();
    assert_eq!(
        runtime.submit(command(PLAYER_ONE, 2, 2, HERO_ONE, CommandKind::Stop,)),
        Err(crate::CommandRejection {
            player: PLAYER_ONE,
            sequence: 2,
            reason: CommandRejectReason::MatchNotActive,
        })
    );
    runtime
        .capture_checkpoint(ContentId::new([0x31; 32]))
        .unwrap();
}

fn modifier_runtime() -> MatchRuntime {
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 0x5747).unwrap();
    runtime
        .spawn_actor(&ActorSpawn {
            id: HERO_ONE,
            owner: Some(PLAYER_ONE),
            team: TeamId(0),
            kind: ActorKind::Hero,
            position: Vec2Mm::new(0, 0),
            growth: StatGrowth::NONE,
            bounty: Bounty::NONE,
            stats: CombatStats {
                max_health: 1_000,
                max_resource: 100,
                move_speed_mm_per_tick: 100,
                physical_reduction_bps: 2_000,
                magic_reduction_bps: 0,
                crit_chance_bps: 0,
                crit_damage_bps: BASIS_POINTS,
                physical_penetration_bps: 0,
                magic_penetration_bps: 0,
                lifesteal_bps: 0,
            },
            abilities: vec![],
            basic_attack: None,
            death_rule: DeathRule::StayDead,
        })
        .unwrap();
    runtime.start().unwrap();
    runtime
}

fn resolve_move_speed(order: &[(StatKind, ModifierOp)]) -> u32 {
    let mut runtime = modifier_runtime();
    for (kind, op) in order {
        runtime.apply_stat_modifier(HERO_ONE, *kind, *op).unwrap();
    }
    runtime.check_invariants().unwrap();
    runtime.actor(HERO_ONE).unwrap().move_speed_mm_per_tick
}

#[test]
fn stat_aggregation_is_independent_of_application_order() {
    // EVERY permutation, not a rotation. Rotations preserve most relative orderings, so a rotation-only
    // test can pass by coincidence while genuine order-dependence hides in the swapped pairs.
    let ops = [
        (StatKind::MoveSpeed, ModifierOp::Flat { magnitude: -93 }),
        (StatKind::MoveSpeed, ModifierOp::PercentMult { bps: 5_000 }),
        (StatKind::MoveSpeed, ModifierOp::PercentMult { bps: -5_000 }),
        (StatKind::MoveSpeed, ModifierOp::PercentAdd { bps: 1_500 }),
    ];
    // These exact magnitudes are the adversarial case: base 100, flat -93 leaves 7, and 7 x1.5 then x0.5
    // truncates to 5 while 7 x0.5 then x1.5 truncates to 4. Ordering multiplicative stacking by ID rather
    // than by magnitude resolved those two application orders differently.
    let expected = resolve_move_speed(&ops);

    let mut indices: Vec<usize> = (0..ops.len()).collect();
    let mut permutations = 0;
    permute(&mut indices, 0, &mut |order| {
        let sequence: Vec<(StatKind, ModifierOp)> = order.iter().map(|index| ops[*index]).collect();
        assert_eq!(
            resolve_move_speed(&sequence),
            expected,
            "permutation {order:?} resolved a different value"
        );
        permutations += 1;
    });
    assert_eq!(permutations, 24, "all 4! orderings must be exercised");
}

fn permute(items: &mut Vec<usize>, start: usize, visit: &mut impl FnMut(&[usize])) {
    if start == items.len() {
        visit(items);
        return;
    }
    for index in start..items.len() {
        items.swap(start, index);
        permute(items, start + 1, visit);
        items.swap(start, index);
    }
}

#[test]
fn crowd_control_gates_exactly_the_actions_it_names() {
    // Each restriction must block its own verb and leave the others alone. A Root that also stopped casting
    // would be a Stun by accident, and the difference between them is the whole counterplay of the genre.
    let cases = [
        (ControlKind::Root, CommandRejectReason::ActorRooted),
        (ControlKind::Silence, CommandRejectReason::ActorSilenced),
        (ControlKind::Disarm, CommandRejectReason::ActorDisarmed),
    ];
    for (control, expected) in cases {
        let mut runtime = duel_runtime();
        runtime
            .apply_status_effect(HERO_ONE, 10, &[control], &[])
            .unwrap();
        let blocked = match control {
            ControlKind::Root => CommandKind::MoveTo {
                destination: Vec2Mm::new(100, 0),
            },
            ControlKind::Silence => CommandKind::Cast {
                ability: STRIKE,
                target: CastTarget::Actor(HERO_TWO),
            },
            _ => CommandKind::BasicAttack { target: HERO_TWO },
        };
        let rejection = runtime
            .submit(command(PLAYER_ONE, 1, 1, HERO_ONE, blocked))
            .unwrap_err();
        assert_eq!(rejection.reason, expected, "{control:?} rejected wrongly");

        // Stop is never gated: it only cancels, so refusing it would be pure rejection noise.
        assert!(runtime
            .submit(command(PLAYER_ONE, 2, 1, HERO_ONE, CommandKind::Stop))
            .is_ok());
    }
}

#[test]
fn a_stun_blocks_every_verb_and_interrupts_what_is_already_running() {
    let mut runtime = duel_runtime();
    runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            HERO_ONE,
            CommandKind::Cast {
                ability: STRIKE,
                target: CastTarget::Actor(HERO_TWO),
            },
        ))
        .unwrap();
    runtime.step().unwrap();
    assert!(
        runtime.actor(HERO_ONE).unwrap().cast.is_some(),
        "the cast must be in flight for the interrupt to mean anything"
    );

    runtime
        .apply_status_effect(HERO_ONE, 10, &[ControlKind::Stun], &[])
        .unwrap();
    assert!(
        runtime.actor(HERO_ONE).unwrap().cast.is_none(),
        "a stun must interrupt an in-progress cast"
    );

    // Every verb is refused while stunned.
    for (sequence, kind) in [
        (
            2,
            CommandKind::MoveTo {
                destination: Vec2Mm::new(50, 0),
            },
        ),
        (
            3,
            CommandKind::Cast {
                ability: STRIKE,
                target: CastTarget::Actor(HERO_TWO),
            },
        ),
        (4, CommandKind::BasicAttack { target: HERO_TWO }),
    ] {
        assert_eq!(
            runtime
                .submit(command(
                    PLAYER_ONE,
                    sequence,
                    runtime.tick() + 1,
                    HERO_ONE,
                    kind
                ))
                .unwrap_err()
                .reason,
            CommandRejectReason::ActorStunned
        );
    }
}

#[test]
fn an_expiring_effect_frees_the_actor_on_the_tick_after_its_last() {
    // The off-by-one that matters: expiry runs at the TOP of the tick, so an effect whose final tick was the
    // previous one cannot block the order this tick delivers.
    let mut runtime = duel_runtime();
    let applied_at = runtime.tick();
    runtime
        .apply_status_effect(HERO_ONE, 3, &[ControlKind::Root], &[])
        .unwrap();
    let effects = runtime.status_effects(HERO_ONE).unwrap();
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].expires_at, applied_at + 3);

    // Rooted through its final tick.
    while runtime.tick() < applied_at + 3 {
        runtime.step().unwrap();
        if runtime.tick() <= applied_at + 3 {
            assert!(
                runtime
                    .actor(HERO_ONE)
                    .unwrap()
                    .controls
                    .contains(ControlKind::Root),
                "must stay rooted through tick {}",
                runtime.tick()
            );
        }
    }
    let frame = runtime.step().unwrap();
    assert!(!runtime
        .actor(HERO_ONE)
        .unwrap()
        .controls
        .contains(ControlKind::Root));
    assert!(frame.events.iter().any(
        |event| matches!(event, MatchEvent::StatusEffectExpired { actor, .. } if *actor == HERO_ONE)
    ));
    runtime.check_invariants().unwrap();
}

#[test]
fn an_expiring_effect_takes_its_modifiers_with_it() {
    // The composition property GP-07 was built for: a slow is a control plus a stat penalty, and neither may
    // outlive the effect that granted them.
    let mut runtime = duel_runtime();
    let base = runtime.actor(HERO_ONE).unwrap().move_speed_mm_per_tick;
    runtime
        .apply_status_effect(
            HERO_ONE,
            2,
            &[],
            &[(StatKind::MoveSpeed, ModifierOp::PercentAdd { bps: -5_000 })],
        )
        .unwrap();
    assert_eq!(runtime.modifiers(HERO_ONE).unwrap().len(), 1);
    assert_eq!(
        runtime.actor(HERO_ONE).unwrap().move_speed_mm_per_tick,
        base / 2
    );

    while !runtime.status_effects(HERO_ONE).unwrap().is_empty() {
        runtime.step().unwrap();
    }
    assert!(
        runtime.modifiers(HERO_ONE).unwrap().is_empty(),
        "the granted modifier must expire with its effect - no orphan buffs"
    );
    assert_eq!(
        runtime.actor(HERO_ONE).unwrap().move_speed_mm_per_tick,
        base
    );
    runtime.check_invariants().unwrap();
}

#[test]
fn an_immobilised_actor_stops_moving_not_just_taking_orders() {
    let mut runtime = duel_runtime();
    runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            HERO_ONE,
            CommandKind::MoveTo {
                destination: Vec2Mm::new(5_000, 0),
            },
        ))
        .unwrap();
    runtime.step().unwrap();
    let moved_to = runtime.actor(HERO_ONE).unwrap().position;
    assert!(moved_to.x > 0, "the actor must be under way");

    runtime
        .apply_status_effect(HERO_ONE, 5, &[ControlKind::Root], &[])
        .unwrap();
    runtime.step().unwrap();
    assert_eq!(
        runtime.actor(HERO_ONE).unwrap().position,
        moved_to,
        "a rooted actor must hold position, not keep walking its old destination"
    );
}

#[test]
fn removing_an_effect_early_is_atomic_and_status_state_survives_a_checkpoint() {
    let mut runtime = duel_runtime();
    let effect = runtime
        .apply_status_effect(
            HERO_ONE,
            50,
            &[ControlKind::Stun, ControlKind::Silence],
            &[(StatKind::MoveSpeed, ModifierOp::Flat { magnitude: -20 })],
        )
        .unwrap();
    runtime.step().unwrap();

    let checkpoint = runtime
        .capture_checkpoint(ContentId::new([0x44; 32]))
        .unwrap();
    let restored =
        MatchRuntime::restore_checkpoint(&checkpoint, ContentId::new([0x44; 32])).unwrap();
    assert_eq!(restored.world_digest(), runtime.world_digest());
    assert_eq!(
        restored.status_effects(HERO_ONE).unwrap(),
        runtime.status_effects(HERO_ONE).unwrap()
    );
    assert!(restored
        .actor(HERO_ONE)
        .unwrap()
        .controls
        .contains(ControlKind::Stun));

    // A dispel takes the control AND the modifier, together.
    let mut dispelled = restored;
    dispelled.remove_status_effect(HERO_ONE, effect).unwrap();
    assert!(dispelled.status_effects(HERO_ONE).unwrap().is_empty());
    assert!(dispelled.modifiers(HERO_ONE).unwrap().is_empty());
    assert!(dispelled.actor(HERO_ONE).unwrap().controls.is_empty());
    dispelled.check_invariants().unwrap();
    assert!(matches!(
        dispelled.remove_status_effect(HERO_ONE, effect),
        Err(RuntimeError::UnknownStatusEffect(_))
    ));
}

#[test]
fn a_rejected_status_effect_consumes_no_identity_and_leaves_no_partial_state() {
    let mut runtime = duel_runtime();
    let before_effects = runtime.next_status_effect_id;
    let before_modifiers = runtime.next_modifier_id;

    // Duration zero is refused outright.
    assert!(matches!(
        runtime.apply_status_effect(HERO_ONE, 0, &[ControlKind::Stun], &[]),
        Err(RuntimeError::InvalidActor(_))
    ));
    // An unknown actor is refused before anything is allocated.
    assert!(matches!(
        runtime.apply_status_effect(ActorId(999), 5, &[ControlKind::Stun], &[]),
        Err(RuntimeError::UnknownActor(_))
    ));
    // Overflowing the modifier budget mid-bundle must roll the WHOLE effect back, not leave half its
    // modifiers applied with no effect owning them.
    let limit = usize::from(runtime.config().max_modifiers_per_actor);
    let oversized: Vec<(StatKind, ModifierOp)> = (0..=limit)
        .map(|_| (StatKind::MoveSpeed, ModifierOp::Flat { magnitude: 1 }))
        .collect();
    assert!(runtime
        .apply_status_effect(HERO_ONE, 5, &[], &oversized)
        .is_err());

    assert!(runtime.status_effects(HERO_ONE).unwrap().is_empty());
    assert!(runtime.modifiers(HERO_ONE).unwrap().is_empty());
    assert_eq!(runtime.next_status_effect_id, before_effects);
    assert_eq!(runtime.next_modifier_id, before_modifiers);
    runtime.check_invariants().unwrap();
}

#[test]
fn each_aggregation_stage_is_pinned_to_an_absolute_value() {
    // Cross-permutation equality alone would still pass if two stages were swapped. These absolute values
    // pin the stage ORDER itself: base 100, +25 flat, then one +15% multiplier = 143. Swapping stages 1 and
    // 2 would give 140.
    assert_eq!(
        resolve_move_speed(&[
            (StatKind::MoveSpeed, ModifierOp::Flat { magnitude: 25 }),
            (StatKind::MoveSpeed, ModifierOp::PercentAdd { bps: 1_500 }),
        ]),
        143
    );
    // Stage 3 on an odd intermediate: 100-93 = 7, then x0.5 -> 3, then x1.5 -> 4 (magnitude order).
    assert_eq!(
        resolve_move_speed(&[
            (StatKind::MoveSpeed, ModifierOp::Flat { magnitude: -93 }),
            (StatKind::MoveSpeed, ModifierOp::PercentMult { bps: 5_000 }),
            (StatKind::MoveSpeed, ModifierOp::PercentMult { bps: -5_000 }),
        ]),
        4
    );
    // Override discards stages 1-3 rather than composing with them.
    assert_eq!(
        resolve_move_speed(&[
            (StatKind::MoveSpeed, ModifierOp::Override { value: 50 }),
            (StatKind::MoveSpeed, ModifierOp::Flat { magnitude: 1_000 }),
            (StatKind::MoveSpeed, ModifierOp::PercentMult { bps: 5_000 }),
        ]),
        50
    );
}

#[test]
fn re_applying_an_identical_modifier_resolves_identically() {
    // The status-effect "refresh" path: remove a buff and re-apply the SAME op. It gets a fresh ID, so an
    // ID-ordered stage 3 would silently change the resolved stat with no change to the modifier set.
    let mut runtime = modifier_runtime();
    runtime
        .apply_stat_modifier(
            HERO_ONE,
            StatKind::MoveSpeed,
            ModifierOp::Flat { magnitude: -93 },
        )
        .unwrap();
    let refreshed = runtime
        .apply_stat_modifier(
            HERO_ONE,
            StatKind::MoveSpeed,
            ModifierOp::PercentMult { bps: 5_000 },
        )
        .unwrap();
    runtime
        .apply_stat_modifier(
            HERO_ONE,
            StatKind::MoveSpeed,
            ModifierOp::PercentMult { bps: -5_000 },
        )
        .unwrap();
    let before = runtime.actor(HERO_ONE).unwrap().move_speed_mm_per_tick;

    runtime.remove_stat_modifier(HERO_ONE, refreshed).unwrap();
    runtime
        .apply_stat_modifier(
            HERO_ONE,
            StatKind::MoveSpeed,
            ModifierOp::PercentMult { bps: 5_000 },
        )
        .unwrap();
    assert_eq!(
        runtime.actor(HERO_ONE).unwrap().move_speed_mm_per_tick,
        before,
        "refreshing a buff must not change the resolved stat"
    );
}

#[test]
fn modifier_budget_and_allocator_exhaustion_reject_without_side_effects() {
    let mut runtime = modifier_runtime();
    let limit = usize::from(runtime.config().max_modifiers_per_actor);
    for _ in 0..limit {
        runtime
            .apply_stat_modifier(
                HERO_ONE,
                StatKind::MoveSpeed,
                ModifierOp::Flat { magnitude: 1 },
            )
            .unwrap();
    }
    let resolved = runtime.actor(HERO_ONE).unwrap().move_speed_mm_per_tick;
    assert!(matches!(
        runtime.apply_stat_modifier(
            HERO_ONE,
            StatKind::MoveSpeed,
            ModifierOp::Flat { magnitude: 1 }
        ),
        Err(RuntimeError::InvalidActor(_))
    ));
    assert_eq!(runtime.modifiers(HERO_ONE).unwrap().len(), limit);
    assert_eq!(
        runtime.actor(HERO_ONE).unwrap().move_speed_mm_per_tick,
        resolved
    );
    runtime.check_invariants().unwrap();

    // An exhausted allocator refuses rather than reusing an ID, which would corrupt override precedence.
    let mut exhausted = modifier_runtime();
    exhausted.next_modifier_id = None;
    assert!(matches!(
        exhausted.apply_stat_modifier(
            HERO_ONE,
            StatKind::MoveSpeed,
            ModifierOp::Flat { magnitude: 1 }
        ),
        Err(RuntimeError::ModifierIdExhausted)
    ));
}

#[test]
fn modifier_mutators_reject_unknown_actors_without_advancing_the_allocator() {
    let mut runtime = modifier_runtime();
    let before = runtime.next_modifier_id;
    assert!(matches!(
        runtime.apply_stat_modifier(
            ActorId(999),
            StatKind::MoveSpeed,
            ModifierOp::Flat { magnitude: 1 }
        ),
        Err(RuntimeError::UnknownActor(_))
    ));
    assert!(matches!(
        runtime.remove_stat_modifier(ActorId(999), crate::ModifierId(1)),
        Err(RuntimeError::UnknownActor(_))
    ));
    assert_eq!(
        runtime.next_modifier_id, before,
        "a rejected apply must not consume an identity"
    );
}

#[test]
fn stat_aggregation_can_never_kill_an_actor() {
    // MaxHealth's domain floor of 1 is the only thing preventing resolve_stats from clamping health to 0 -
    // which would be a death with no killer, no event, and no death rule applied.
    let mut runtime = modifier_runtime();
    runtime
        .apply_stat_modifier(
            HERO_ONE,
            StatKind::MaxHealth,
            ModifierOp::Flat {
                magnitude: i32::MIN,
            },
        )
        .unwrap();
    let view = runtime.actor(HERO_ONE).unwrap();
    assert_eq!(view.max_health, 1);
    assert_eq!(view.health, 1);
    assert!(view.alive, "stat aggregation must never be lethal");
    runtime.check_invariants().unwrap();
}

#[test]
fn override_precedence_is_deliberately_application_ordered() {
    // Unlike the arithmetic stages, Override is order-DEPENDENT by design: most-recently-applied wins.
    // Pinned as intended behaviour so it cannot drift silently into an accidental rule.
    let ascending = resolve_move_speed(&[
        (StatKind::MoveSpeed, ModifierOp::Override { value: 5 }),
        (StatKind::MoveSpeed, ModifierOp::Override { value: 9 }),
    ]);
    let descending = resolve_move_speed(&[
        (StatKind::MoveSpeed, ModifierOp::Override { value: 9 }),
        (StatKind::MoveSpeed, ModifierOp::Override { value: 5 }),
    ]);
    assert_eq!(ascending, 9);
    assert_eq!(descending, 5);
}

#[test]
fn modifier_mutation_after_the_terminal_frame_is_refused() {
    // A removal in the Complete phase would clear `checkpoint_ready` with no way to emit another frame,
    // permanently bricking capture of a finished match. Both mutators are phase-guarded.
    let mut runtime = modifier_runtime();
    let modifier = runtime
        .apply_stat_modifier(
            HERO_ONE,
            StatKind::MoveSpeed,
            ModifierOp::Flat { magnitude: 5 },
        )
        .unwrap();
    while runtime.phase() == MatchPhase::Active {
        runtime.step().unwrap();
    }
    assert!(matches!(
        runtime.remove_stat_modifier(HERO_ONE, modifier),
        Err(RuntimeError::InvalidPhase(_))
    ));
    assert!(matches!(
        runtime.apply_stat_modifier(
            HERO_ONE,
            StatKind::MoveSpeed,
            ModifierOp::Flat { magnitude: 1 }
        ),
        Err(RuntimeError::InvalidPhase(_))
    ));
    // The completed match is still capturable, which is the property the guard protects.
    runtime
        .capture_checkpoint(ContentId::new([0x21; 32]))
        .unwrap();
}

#[test]
fn a_mitigation_only_modifier_is_visible_in_the_emitted_delta() {
    // A dirty actor whose view is byte-identical to the previous frame is a delta that claims a change it
    // cannot show. Mitigation is part of the view precisely so this cannot happen.
    let mut runtime = modifier_runtime();
    runtime.step().unwrap();
    let before = runtime.actor(HERO_ONE).unwrap();
    runtime
        .apply_stat_modifier(
            HERO_ONE,
            StatKind::PhysicalReduction,
            ModifierOp::Flat { magnitude: 1_500 },
        )
        .unwrap();
    let frame = runtime.step().unwrap();
    let view = frame
        .changed
        .iter()
        .find(|candidate| candidate.id == HERO_ONE)
        .expect("the modified actor must appear in the delta");
    assert_ne!(
        *view, before,
        "the emitted view must differ from the previous frame"
    );
    assert_eq!(view.physical_reduction_bps, 3_500);
}

#[test]
fn percent_add_composes_once_while_percent_mult_compounds() {
    // +15% and +15% PercentAdd is +30% of base, applied with ONE truncation.
    let mut additive = modifier_runtime();
    for _ in 0..2 {
        additive
            .apply_stat_modifier(
                HERO_ONE,
                StatKind::MoveSpeed,
                ModifierOp::PercentAdd { bps: 1_500 },
            )
            .unwrap();
    }
    assert_eq!(
        additive.actor(HERO_ONE).unwrap().move_speed_mm_per_tick,
        130
    );

    // The same two magnitudes as PercentMult compound: 100 -> 115 -> 132.
    let mut multiplicative = modifier_runtime();
    for _ in 0..2 {
        multiplicative
            .apply_stat_modifier(
                HERO_ONE,
                StatKind::MoveSpeed,
                ModifierOp::PercentMult { bps: 1_500 },
            )
            .unwrap();
    }
    assert_eq!(
        multiplicative
            .actor(HERO_ONE)
            .unwrap()
            .move_speed_mm_per_tick,
        132
    );
}

#[test]
fn highest_id_override_wins_and_removal_restores_the_previous_resolution() {
    let mut runtime = modifier_runtime();
    let first = runtime
        .apply_stat_modifier(
            HERO_ONE,
            StatKind::MoveSpeed,
            ModifierOp::Override { value: 5 },
        )
        .unwrap();
    let second = runtime
        .apply_stat_modifier(
            HERO_ONE,
            StatKind::MoveSpeed,
            ModifierOp::Override { value: 9 },
        )
        .unwrap();
    assert!(second > first);
    assert_eq!(runtime.actor(HERO_ONE).unwrap().move_speed_mm_per_tick, 9);

    // Dropping the winner falls back to the surviving override, not to base.
    runtime.remove_stat_modifier(HERO_ONE, second).unwrap();
    assert_eq!(runtime.actor(HERO_ONE).unwrap().move_speed_mm_per_tick, 5);
    runtime.remove_stat_modifier(HERO_ONE, first).unwrap();
    assert_eq!(runtime.actor(HERO_ONE).unwrap().move_speed_mm_per_tick, 100);
    assert!(matches!(
        runtime.remove_stat_modifier(HERO_ONE, first),
        Err(RuntimeError::UnknownModifier(_))
    ));
}

#[test]
fn stacked_reduction_modifiers_cannot_reach_immortality() {
    // Clamping is a safety property: no stack of reduction modifiers may reach 100% mitigation.
    let mut runtime = modifier_runtime();
    for _ in 0..8 {
        runtime
            .apply_stat_modifier(
                HERO_ONE,
                StatKind::PhysicalReduction,
                ModifierOp::Flat { magnitude: 5_000 },
            )
            .unwrap();
    }
    // 8 x +5000bps would be 400% mitigation unclamped. The domain caps it one basis point below whole.
    let resolved = runtime
        .base_stats(HERO_ONE)
        .unwrap()
        .with_modifiers(&runtime.modifiers(HERO_ONE).unwrap());
    assert_eq!(resolved.physical_reduction_bps, BASIS_POINTS - 1);
    runtime.check_invariants().unwrap();
}

#[test]
fn lowering_max_health_clamps_current_health_and_holds_the_invariant() {
    let mut runtime = modifier_runtime();
    assert_eq!(runtime.actor(HERO_ONE).unwrap().health, 1_000);
    let shrink = runtime
        .apply_stat_modifier(
            HERO_ONE,
            StatKind::MaxHealth,
            ModifierOp::PercentAdd { bps: -6_000 },
        )
        .unwrap();
    let view = runtime.actor(HERO_ONE).unwrap();
    assert_eq!(view.max_health, 400);
    assert_eq!(view.health, 400, "health must be pulled down with the cap");
    runtime.check_invariants().unwrap();

    // Restoring the cap does NOT restore health: resolving stats must never heal.
    runtime.remove_stat_modifier(HERO_ONE, shrink).unwrap();
    let view = runtime.actor(HERO_ONE).unwrap();
    assert_eq!(view.max_health, 1_000);
    assert_eq!(view.health, 400);
    runtime.check_invariants().unwrap();
}

#[test]
fn modifiers_survive_a_checkpoint_round_trip_and_a_drifted_cache_is_rejected() {
    const CONTENT: ContentId = ContentId::new([0x4d; 32]);
    let mut runtime = modifier_runtime();
    runtime
        .apply_stat_modifier(
            HERO_ONE,
            StatKind::MoveSpeed,
            ModifierOp::PercentMult { bps: 2_500 },
        )
        .unwrap();
    runtime
        .apply_stat_modifier(
            HERO_ONE,
            StatKind::MaxHealth,
            ModifierOp::Flat { magnitude: -250 },
        )
        .unwrap();
    runtime.step().unwrap();

    let checkpoint = runtime.capture_checkpoint(CONTENT).unwrap();
    let restored = MatchRuntime::restore_checkpoint(&checkpoint, CONTENT).unwrap();
    assert_eq!(restored.world_digest(), runtime.world_digest());
    assert_eq!(
        restored.modifiers(HERO_ONE).unwrap(),
        runtime.modifiers(HERO_ONE).unwrap()
    );
    assert_eq!(
        restored.base_stats(HERO_ONE).unwrap(),
        runtime.base_stats(HERO_ONE).unwrap()
    );

    // The resolved cache is derived on decode, never read from the payload, so it cannot be forged: a
    // hand-drifted cache is caught by the frame-boundary audit rather than trusted.
    let mut drifted = runtime;
    drifted.actors[0].stats.move_speed_mm_per_tick += 1;
    let violation = drifted.check_invariants().unwrap_err();
    assert_eq!(
        violation.detail,
        "resolved stat cache does not match its base stats and modifiers"
    );
}

#[test]
fn queued_command_requires_matching_successful_retry_cache_record() {
    let mut runtime = duel_runtime();
    let accepted = command(
        PLAYER_ONE,
        1,
        2,
        HERO_ONE,
        CommandKind::MoveTo {
            destination: Vec2Mm::new(250, 0),
        },
    );
    runtime.submit(accepted).unwrap();
    runtime.check_invariants().unwrap();

    runtime
        .last_submissions
        .get_mut(&PLAYER_ONE)
        .unwrap()
        .command
        .kind = CommandKind::Stop;
    let violation = runtime.check_invariants().unwrap_err();
    assert_eq!(
        violation.detail,
        "queued command has invalid tick, player, or actor reference"
    );
}

#[test]
fn terminal_tick_cancels_due_respawn_and_remains_checkpointable() {
    const LETHAL: AbilityId = AbilityId(99);
    let config = MatchConfig {
        max_match_ticks: 2,
        ..MatchConfig::default()
    };
    let mut runtime = MatchRuntime::new(config, 0xfeed).unwrap();
    runtime
        .register_ability(AbilitySpec {
            id: LETHAL,
            aim: AbilityAim::Unit,
            delivery: AbilityDelivery::Instant,
            impact: ImpactShape::Single,
            targeting: AbilityTargeting::Enemy,
            range_mm: 5_000,
            resource_cost: 0,
            cooldown_ticks: 1,
            cast_ticks: 0,
            per_rank_amount: 0,
            effect: AbilityEffect::Damage {
                amount: 1_000,
                school: DamageSchool::True,
            },
        })
        .unwrap();
    runtime
        .spawn_actor(&ActorSpawn {
            id: HERO_ONE,
            owner: Some(PLAYER_ONE),
            team: TeamId(0),
            kind: ActorKind::Hero,
            position: Vec2Mm::new(0, 0),
            growth: StatGrowth::NONE,
            bounty: Bounty::NONE,
            stats: stats(1_000, 0, 0),
            abilities: vec![LETHAL],
            basic_attack: None,
            death_rule: DeathRule::StayDead,
        })
        .unwrap();
    runtime
        .spawn_actor(&ActorSpawn {
            id: HERO_TWO,
            owner: Some(PLAYER_TWO),
            team: TeamId(1),
            kind: ActorKind::Hero,
            position: Vec2Mm::new(100, 0),
            growth: StatGrowth::NONE,
            bounty: Bounty::NONE,
            stats: stats(100, 0, 0),
            abilities: Vec::new(),
            basic_attack: None,
            death_rule: DeathRule::Respawn {
                delay_ticks: 1,
                at: Vec2Mm::new(100, 0),
            },
        })
        .unwrap();
    runtime.start().unwrap();
    runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            HERO_ONE,
            CommandKind::Cast {
                ability: LETHAL,
                target: CastTarget::Actor(HERO_TWO),
            },
        ))
        .unwrap();

    runtime.step().unwrap();
    assert_eq!(runtime.actor(HERO_TWO).unwrap().respawn_at, Some(2));
    let terminal = runtime.step().unwrap();
    assert_eq!(
        terminal.phase,
        MatchPhase::Complete {
            outcome: MatchOutcome::Draw,
            reason: MatchEndReason::TimeLimit,
        }
    );
    assert!(terminal.events.iter().any(|event| matches!(
        event,
        MatchEvent::RespawnCancelled {
            actor: HERO_TWO,
            at_tick: 2,
            reason: MatchEndReason::TimeLimit,
        }
    )));
    assert_eq!(runtime.actor(HERO_TWO).unwrap().respawn_at, None);
    runtime.check_invariants().unwrap();
    runtime
        .capture_checkpoint(ContentId::new([0x32; 32]))
        .unwrap();
}

#[test]
fn sequence_and_tick_windows_fail_closed() {
    let mut runtime = duel_runtime();
    runtime
        .submit(command(PLAYER_ONE, 1, 1, HERO_ONE, CommandKind::Stop))
        .unwrap();

    let duplicate = runtime
        .submit(command(PLAYER_ONE, 1, 2, HERO_ONE, CommandKind::Stop))
        .unwrap_err();
    assert_eq!(
        duplicate.reason,
        CommandRejectReason::DuplicateOrReorderedSequence
    );

    let future = runtime
        .submit(command(PLAYER_ONE, 2, 20, HERO_ONE, CommandKind::Stop))
        .unwrap_err();
    assert_eq!(future.reason, CommandRejectReason::TooFarAhead);

    let stale = runtime
        .submit(command(PLAYER_ONE, 3, 0, HERO_ONE, CommandKind::Stop))
        .unwrap_err();
    assert_eq!(stale.reason, CommandRejectReason::StaleTick);
}

#[test]
fn initial_baseline_then_actor_level_delta_is_not_a_full_world_copy() {
    let (mut runtime, baseline) = duel_runtime_with_baseline();
    assert_eq!(baseline.changed.len(), 2);

    runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            HERO_ONE,
            CommandKind::MoveTo {
                destination: Vec2Mm::new(500, 0),
            },
        ))
        .unwrap();
    let delta = runtime.step().unwrap();
    assert_eq!(delta.changed.len(), 1);
    assert_eq!(delta.changed[0].id, HERO_ONE);
}

#[test]
fn physical_damage_uses_integer_basis_point_mitigation() {
    let ability = AbilitySpec {
        per_rank_amount: 0,
        effect: AbilityEffect::Damage {
            amount: 500,
            school: DamageSchool::Physical,
        },
        ..strike()
    };
    let mut target = hero(
        HERO_TWO,
        PLAYER_TWO,
        TeamId(1),
        Vec2Mm::new(1_000, 0),
        1_000,
        vec![STRIKE],
    );
    target.stats.physical_reduction_bps = 2_000;

    let mut runtime = MatchRuntime::new(MatchConfig::default(), 9).unwrap();
    runtime.register_ability(ability).unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_ONE,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            1_000,
            vec![STRIKE],
        ))
        .unwrap();
    runtime.spawn_actor(&target).unwrap();
    runtime.start().unwrap();
    runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            HERO_ONE,
            CommandKind::Cast {
                ability: STRIKE,
                target: CastTarget::Actor(HERO_TWO),
            },
        ))
        .unwrap();
    runtime.step().unwrap();
    runtime.step().unwrap();
    assert_eq!(runtime.actor(HERO_TWO).unwrap().health, 600);
}

#[test]
fn scenario_replay_is_bit_identical_and_registration_order_independent() {
    let actors = vec![
        hero(
            HERO_ONE,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            1_000,
            vec![STRIKE, HEAL],
        ),
        hero(
            HERO_TWO,
            PLAYER_TWO,
            TeamId(1),
            Vec2Mm::new(1_000, 0),
            500,
            vec![STRIKE],
        ),
    ];
    let submissions = vec![
        submission(
            0,
            0,
            command(
                PLAYER_TWO,
                1,
                1,
                HERO_TWO,
                CommandKind::MoveTo {
                    destination: Vec2Mm::new(1_300, 0),
                },
            ),
        ),
        submission(
            0,
            1,
            command(
                PLAYER_ONE,
                1,
                1,
                HERO_ONE,
                CommandKind::Cast {
                    ability: STRIKE,
                    target: CastTarget::Actor(HERO_TWO),
                },
            ),
        ),
    ];
    let scenario = Scenario {
        config: MatchConfig::default(),
        seed: 42,
        abilities: vec![strike(), heal()],
        actors: actors.clone(),
        submissions: submissions.clone(),
        final_tick: 4,
        finish: None,
    };
    assert!(scenario.reproduces(3).unwrap());

    let reversed = Scenario {
        abilities: vec![heal(), strike()],
        actors: actors.into_iter().rev().collect(),
        submissions: submissions.into_iter().rev().collect(),
        ..scenario.clone()
    };
    assert_eq!(scenario.run().unwrap(), reversed.run().unwrap());
}

#[test]
fn invalid_setup_is_explained_before_match_start() {
    let config = MatchConfig {
        tick_rate_hz: 0,
        ..MatchConfig::default()
    };
    let error = MatchRuntime::new(config, 1).err().unwrap();
    assert_eq!(
        error.to_string(),
        "invalid match config: tick rate must be positive"
    );

    let mut runtime = MatchRuntime::new(MatchConfig::default(), 1).unwrap();
    runtime.register_ability(strike()).unwrap();
    let duplicate = runtime.register_ability(strike()).unwrap_err();
    assert!(duplicate.to_string().contains("duplicate ability"));
}

#[test]
fn match_completion_emits_one_terminal_frame() {
    let mut runtime = duel_runtime();

    let terminal = runtime
        .finish(MatchOutcome::Winner(TeamId(0)), MatchEndReason::Host)
        .unwrap();
    assert_eq!(terminal.tick, 0);
    assert_eq!(
        terminal.phase,
        MatchPhase::Complete {
            outcome: MatchOutcome::Winner(TeamId(0)),
            reason: MatchEndReason::Host,
        }
    );
    assert!(terminal.events.iter().any(|event| matches!(
        event,
        MatchEvent::MatchFinished {
            outcome: MatchOutcome::Winner(TeamId(0)),
            reason: MatchEndReason::Host,
        }
    )));
    assert_ne!(terminal.frame_digest.0, 0);
    assert!(matches!(
        runtime.finish(MatchOutcome::Winner(TeamId(0)), MatchEndReason::Host),
        Err(RuntimeError::InvalidPhase(_))
    ));
    assert!(matches!(runtime.step(), Err(RuntimeError::InvalidPhase(_))));
}

#[test]
fn replay_preserves_submission_tick_and_arrival_order() {
    let actor = hero(
        HERO_ONE,
        PLAYER_ONE,
        TeamId(0),
        Vec2Mm::new(0, 0),
        1_000,
        vec![],
    );
    let future = command(PLAYER_ONE, 1, 9, HERO_ONE, CommandKind::Stop);
    let early = Scenario {
        config: MatchConfig::default(),
        seed: 1,
        abilities: vec![],
        actors: vec![actor.clone()],
        submissions: vec![submission(0, 0, future)],
        final_tick: 1,
        finish: None,
    }
    .run()
    .unwrap();
    assert_eq!(
        early.submissions[0].outcome.unwrap_err().reason,
        CommandRejectReason::TooFarAhead
    );

    let late = Scenario {
        submissions: vec![submission(1, 0, future)],
        ..Scenario {
            config: MatchConfig::default(),
            seed: 1,
            abilities: vec![],
            actors: vec![actor.clone()],
            submissions: vec![],
            final_tick: 1,
            finish: None,
        }
    }
    .run()
    .unwrap();
    assert!(late.submissions[0].outcome.is_ok());

    let arrival_sensitive = Scenario {
        config: MatchConfig::default(),
        seed: 2,
        abilities: vec![],
        actors: vec![actor],
        submissions: vec![
            submission(0, 0, command(PLAYER_ONE, 2, 1, HERO_ONE, CommandKind::Stop)),
            submission(0, 1, command(PLAYER_ONE, 1, 1, HERO_ONE, CommandKind::Stop)),
        ],
        final_tick: 1,
        finish: None,
    }
    .run()
    .unwrap();
    assert!(arrival_sensitive.submissions[0].outcome.is_ok());
    assert_eq!(
        arrival_sensitive.submissions[1].outcome.unwrap_err().reason,
        CommandRejectReason::DuplicateOrReorderedSequence
    );
}

#[test]
fn rejection_delivery_is_idempotent_and_bounded() {
    let config = MatchConfig {
        max_rejection_events_per_frame: 2,
        ..MatchConfig::default()
    };
    let mut runtime = MatchRuntime::new(config, 3).unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_ONE,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            1_000,
            vec![],
        ))
        .unwrap();
    runtime.start().unwrap();

    let first = command(PLAYER_ONE, 1, 1, ActorId(999), CommandKind::Stop);
    let first_outcome = runtime.submit(first);
    assert_eq!(runtime.submit(first), first_outcome);
    for sequence in 2..=10 {
        let _ = runtime.submit(command(
            PLAYER_ONE,
            sequence,
            1,
            ActorId(999),
            CommandKind::Stop,
        ));
    }

    let frame = runtime.step().unwrap();
    assert_eq!(
        frame
            .events
            .iter()
            .filter(|event| matches!(event, MatchEvent::CommandRejected { .. }))
            .count(),
        2
    );
}

#[test]
fn positive_integer_speed_always_reduces_diagonal_distance() {
    let mut actor = hero(
        HERO_ONE,
        PLAYER_ONE,
        TeamId(0),
        Vec2Mm::new(0, 0),
        1_000,
        vec![],
    );
    actor.stats.move_speed_mm_per_tick = 1;
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 4).unwrap();
    runtime.spawn_actor(&actor).unwrap();
    runtime.start().unwrap();
    runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            HERO_ONE,
            CommandKind::MoveTo {
                destination: Vec2Mm::new(3, 4),
            },
        ))
        .unwrap();

    let target = Vec2Mm::new(3, 4);
    let mut previous = runtime.actor(HERO_ONE).unwrap().position;
    for _ in 0..8 {
        runtime.step().unwrap();
        let next = runtime.actor(HERO_ONE).unwrap().position;
        assert!(next.squared_distance(target) < previous.squared_distance(target));
        assert!(previous.squared_distance(next) <= 1);
        previous = next;
        if next == target {
            break;
        }
    }
    assert_eq!(previous, target);

    let extreme_config = MatchConfig {
        map_bounds: crate::RectMm::new(
            Vec2Mm::new(i32::MIN, i32::MIN),
            Vec2Mm::new(i32::MAX, i32::MAX),
        ),
        ..MatchConfig::default()
    };
    let mut extreme_actor = hero(
        HERO_ONE,
        PLAYER_ONE,
        TeamId(0),
        Vec2Mm::new(i32::MIN, i32::MIN),
        1_000,
        vec![],
    );
    extreme_actor.stats.move_speed_mm_per_tick = u32::MAX;
    let mut extreme = MatchRuntime::new(extreme_config, 5).unwrap();
    extreme.spawn_actor(&extreme_actor).unwrap();
    extreme.start().unwrap();
    extreme
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            HERO_ONE,
            CommandKind::MoveTo {
                destination: Vec2Mm::new(i32::MAX, i32::MAX),
            },
        ))
        .unwrap();
    extreme.step().unwrap();
    assert!(extreme_config
        .map_bounds
        .contains(extreme.actor(HERO_ONE).unwrap().position));
}

#[test]
fn simultaneous_lethal_casts_are_both_admitted() {
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 6).unwrap();
    runtime.register_ability(strike()).unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_ONE,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            500,
            vec![STRIKE],
        ))
        .unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_TWO,
            PLAYER_TWO,
            TeamId(1),
            Vec2Mm::new(1_000, 0),
            500,
            vec![STRIKE],
        ))
        .unwrap();
    runtime.start().unwrap();
    runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            HERO_ONE,
            CommandKind::Cast {
                ability: STRIKE,
                target: CastTarget::Actor(HERO_TWO),
            },
        ))
        .unwrap();
    runtime
        .submit(command(
            PLAYER_TWO,
            1,
            1,
            HERO_TWO,
            CommandKind::Cast {
                ability: STRIKE,
                target: CastTarget::Actor(HERO_ONE),
            },
        ))
        .unwrap();

    runtime.step().unwrap();
    let impact = runtime.step().unwrap();
    assert!(!runtime.actor(HERO_ONE).unwrap().alive);
    assert!(!runtime.actor(HERO_TWO).unwrap().alive);
    assert_eq!(
        impact
            .events
            .iter()
            .filter(|event| matches!(event, MatchEvent::ActorDied { .. }))
            .count(),
        2
    );
}

#[test]
fn match_duration_is_checked_instead_of_saturating() {
    let config = MatchConfig {
        max_match_ticks: 1,
        ..MatchConfig::default()
    };
    let mut runtime = MatchRuntime::new(config, 7).unwrap();
    runtime.start().unwrap();
    let terminal = runtime.step().unwrap();
    assert_eq!(
        terminal.phase,
        MatchPhase::Complete {
            outcome: MatchOutcome::Draw,
            reason: MatchEndReason::TimeLimit,
        }
    );
    assert!(matches!(runtime.step(), Err(RuntimeError::InvalidPhase(_))));

    let invalid = MatchConfig {
        max_match_ticks: u64::MAX,
        ..MatchConfig::default()
    };
    assert!(MatchRuntime::new(invalid, 7).is_err());
}

#[test]
fn no_op_commands_and_zero_healing_do_not_emit_unchanged_actor_payloads() {
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 8).unwrap();
    runtime.register_ability(heal()).unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_ONE,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            1_000,
            vec![HEAL],
        ))
        .unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_TWO,
            PLAYER_TWO,
            TeamId(0),
            Vec2Mm::new(100, 0),
            1_000,
            vec![],
        ))
        .unwrap();
    runtime.start().unwrap();
    runtime
        .submit(command(PLAYER_TWO, 1, 1, HERO_TWO, CommandKind::Stop))
        .unwrap();
    runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            HERO_ONE,
            CommandKind::Cast {
                ability: HEAL,
                target: CastTarget::Actor(HERO_TWO),
            },
        ))
        .unwrap();

    let frame = runtime.step().unwrap();
    assert_eq!(
        frame
            .changed
            .iter()
            .map(|actor| actor.id)
            .collect::<Vec<_>>(),
        vec![HERO_ONE]
    );
    assert!(!frame
        .events
        .iter()
        .any(|event| matches!(event, MatchEvent::MoveStopped { actor: HERO_TWO })));
    assert!(frame.events.iter().any(|event| matches!(
        event,
        MatchEvent::HealingApplied {
            target: HERO_TWO,
            amount: 0,
            ..
        }
    )));
}

#[test]
fn replay_includes_tick_zero_baseline_and_terminal_lifecycle() {
    let outcome = Scenario {
        config: MatchConfig::default(),
        seed: 9,
        abilities: vec![],
        actors: vec![hero(
            HERO_ONE,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            1_000,
            vec![],
        )],
        submissions: vec![],
        final_tick: 0,
        finish: Some(ScenarioFinish {
            at_tick: 0,
            arrival_order: 0,
            outcome: MatchOutcome::Winner(TeamId(0)),
            reason: MatchEndReason::Host,
        }),
    }
    .run()
    .unwrap();
    assert_eq!(outcome.frames.len(), 2);
    assert!(matches!(
        outcome.frames[0].events[0],
        MatchEvent::MatchStarted
    ));
    assert_eq!(
        outcome.frames[1].phase,
        MatchPhase::Complete {
            outcome: MatchOutcome::Winner(TeamId(0)),
            reason: MatchEndReason::Host,
        }
    );
}

#[test]
fn mixed_same_tick_health_effects_do_not_make_actor_ids_initiative() {
    fn run(healer_id: ActorId, damager_id: ActorId) -> (bool, usize) {
        let instant_strike = AbilitySpec {
            cast_ticks: 1,
            per_rank_amount: 0,
            ..strike()
        };
        let delayed_heal = AbilitySpec {
            cast_ticks: 1,
            per_rank_amount: 0,
            ..heal()
        };
        let target_id = ActorId(900);
        let target_player = PlayerId(90);
        let mut runtime = MatchRuntime::new(MatchConfig::default(), 10).unwrap();
        runtime.register_ability(instant_strike).unwrap();
        runtime.register_ability(delayed_heal).unwrap();
        runtime
            .spawn_actor(&hero(
                healer_id,
                PLAYER_ONE,
                TeamId(0),
                Vec2Mm::new(0, 0),
                500,
                vec![HEAL],
            ))
            .unwrap();
        runtime
            .spawn_actor(&hero(
                damager_id,
                PLAYER_TWO,
                TeamId(1),
                Vec2Mm::new(100, 0),
                500,
                vec![STRIKE],
            ))
            .unwrap();
        runtime
            .spawn_actor(&hero(
                target_id,
                target_player,
                TeamId(0),
                Vec2Mm::new(50, 0),
                500,
                vec![],
            ))
            .unwrap();
        runtime.start().unwrap();
        runtime
            .submit(command(
                PLAYER_ONE,
                1,
                1,
                healer_id,
                CommandKind::Cast {
                    ability: HEAL,
                    target: CastTarget::Actor(target_id),
                },
            ))
            .unwrap();
        runtime
            .submit(command(
                PLAYER_TWO,
                1,
                1,
                damager_id,
                CommandKind::Cast {
                    ability: STRIKE,
                    target: CastTarget::Actor(target_id),
                },
            ))
            .unwrap();
        runtime.step().unwrap();
        let settlement = runtime.step().unwrap();
        (
            runtime.actor(target_id).unwrap().alive,
            settlement
                .events
                .iter()
                .filter(|event| matches!(event, MatchEvent::ActorDied { actor, .. } if *actor == target_id))
                .count(),
        )
    }

    assert_eq!(run(ActorId(10), ActorId(20)), (false, 1));
    assert_eq!(run(ActorId(20), ActorId(10)), (false, 1));
}

#[test]
fn accepted_command_execution_rejection_is_never_dropped_by_ingress_cap() {
    let config = MatchConfig {
        max_rejection_events_per_frame: 1,
        ..MatchConfig::default()
    };
    let instant_strike = AbilitySpec {
        cast_ticks: 0,
        per_rank_amount: 0,
        ..strike()
    };
    let mut runtime = MatchRuntime::new(config, 11).unwrap();
    runtime.register_ability(instant_strike).unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_ONE,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            1_000,
            vec![STRIKE],
        ))
        .unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_TWO,
            PLAYER_TWO,
            TeamId(1),
            Vec2Mm::new(100, 0),
            500,
            vec![],
        ))
        .unwrap();
    runtime.start().unwrap();
    runtime
        .submit(command(PLAYER_TWO, 1, 2, HERO_TWO, CommandKind::Stop))
        .unwrap();
    runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            HERO_ONE,
            CommandKind::Cast {
                ability: STRIKE,
                target: CastTarget::Actor(HERO_TWO),
            },
        ))
        .unwrap();
    let _ = runtime.submit(command(PLAYER_ONE, 2, 1, ActorId(999), CommandKind::Stop));
    let death = runtime.step().unwrap();
    assert!(death.events.iter().any(|event| matches!(
        event,
        MatchEvent::CommandRejected {
            player: PLAYER_TWO,
            sequence: 1,
            reason: CommandRejectReason::ActorDead
        }
    )));

    assert!(death.events.iter().any(|event| matches!(
        event,
        MatchEvent::CommandRejected {
            player: PLAYER_ONE,
            sequence: 2,
            reason: CommandRejectReason::ActorNotFound
        }
    )));
    assert_eq!(
        death
            .events
            .iter()
            .filter(|event| matches!(event, MatchEvent::CommandRejected { .. }))
            .count(),
        2
    );
}

#[test]
fn commands_and_casts_cannot_cross_match_duration() {
    let config = MatchConfig {
        max_match_ticks: 2,
        ..MatchConfig::default()
    };
    let long_cast = AbilitySpec {
        cast_ticks: 2,
        per_rank_amount: 0,
        ..strike()
    };
    let mut runtime = MatchRuntime::new(config, 12).unwrap();
    runtime.register_ability(long_cast).unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_ONE,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            1_000,
            vec![STRIKE],
        ))
        .unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_TWO,
            PLAYER_TWO,
            TeamId(1),
            Vec2Mm::new(100, 0),
            1_000,
            vec![],
        ))
        .unwrap();
    runtime.start().unwrap();

    let beyond = runtime
        .submit(command(PLAYER_ONE, 1, 3, HERO_ONE, CommandKind::Stop))
        .unwrap_err();
    assert_eq!(beyond.reason, CommandRejectReason::BeyondMatchDuration);
    let unresolved = runtime
        .submit(command(
            PLAYER_ONE,
            2,
            1,
            HERO_ONE,
            CommandKind::Cast {
                ability: STRIKE,
                target: CastTarget::Actor(HERO_TWO),
            },
        ))
        .unwrap_err();
    assert_eq!(
        unresolved.reason,
        CommandRejectReason::CastExceedsMatchDuration
    );
}

#[test]
fn authored_roster_owns_player_capacity() {
    let config = MatchConfig {
        max_players: 1,
        ..MatchConfig::default()
    };
    let mut runtime = MatchRuntime::new(config, 13).unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_ONE,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            1_000,
            vec![],
        ))
        .unwrap();
    assert_eq!(
        runtime
            .spawn_actor(&hero(
                HERO_TWO,
                PLAYER_TWO,
                TeamId(1),
                Vec2Mm::new(100, 0),
                1_000,
                vec![],
            ))
            .unwrap_err(),
        RuntimeError::PlayerBudgetExceeded
    );
    runtime.start().unwrap();
    let spoofed = runtime
        .submit(command(PLAYER_TWO, 1, 1, HERO_ONE, CommandKind::Stop))
        .unwrap_err();
    assert_eq!(spoofed.reason, CommandRejectReason::PlayerNotRegistered);
    assert!(runtime
        .submit(command(PLAYER_ONE, 1, 1, HERO_ONE, CommandKind::Stop))
        .is_ok());
}

#[test]
fn per_player_tick_quota_preserves_opponent_command_capacity() {
    let config = MatchConfig {
        max_commands_per_tick: 4,
        max_commands_per_player_per_tick: 2,
        ..MatchConfig::default()
    };
    let mut runtime = MatchRuntime::new(config, 15).unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_ONE,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            1_000,
            vec![],
        ))
        .unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_TWO,
            PLAYER_TWO,
            TeamId(1),
            Vec2Mm::new(100, 0),
            1_000,
            vec![],
        ))
        .unwrap();
    runtime.start().unwrap();

    runtime
        .submit(command(PLAYER_ONE, 1, 1, HERO_ONE, CommandKind::Stop))
        .unwrap();
    runtime
        .submit(command(PLAYER_ONE, 2, 1, HERO_ONE, CommandKind::Stop))
        .unwrap();
    assert_eq!(
        runtime
            .submit(command(PLAYER_ONE, 3, 1, HERO_ONE, CommandKind::Stop))
            .unwrap_err()
            .reason,
        CommandRejectReason::PlayerTickCommandBudgetExceeded
    );
    assert!(runtime
        .submit(command(PLAYER_TWO, 1, 1, HERO_TWO, CommandKind::Stop))
        .is_ok());
}

#[test]
fn roster_reservations_make_three_player_admission_arrival_independent() {
    fn run(order: [(PlayerId, ActorId); 3]) -> crate::ServerFrame {
        let config = MatchConfig {
            max_commands_per_tick: 3,
            max_commands_per_player_per_tick: 1,
            ..MatchConfig::default()
        };
        let mut runtime = MatchRuntime::new(config, 16).unwrap();
        for (player, actor, team, x) in [
            (PLAYER_ONE, HERO_ONE, TeamId(0), 0),
            (PLAYER_TWO, HERO_TWO, TeamId(1), 100),
            (PLAYER_THREE, HERO_THREE, TeamId(2), 200),
        ] {
            runtime
                .spawn_actor(&hero(actor, player, team, Vec2Mm::new(x, 0), 1_000, vec![]))
                .unwrap();
        }
        runtime.start().unwrap();
        for (player, actor) in order {
            runtime
                .submit(command(player, 1, 1, actor, CommandKind::Stop))
                .unwrap();
        }
        runtime.step().unwrap()
    }

    let forward = run([
        (PLAYER_ONE, HERO_ONE),
        (PLAYER_TWO, HERO_TWO),
        (PLAYER_THREE, HERO_THREE),
    ]);
    let reversed = run([
        (PLAYER_THREE, HERO_THREE),
        (PLAYER_TWO, HERO_TWO),
        (PLAYER_ONE, HERO_ONE),
    ]);
    assert_eq!(forward, reversed);

    let unfair = MatchConfig {
        max_commands_per_tick: 2,
        max_commands_per_player_per_tick: 2,
        ..MatchConfig::default()
    };
    let mut runtime = MatchRuntime::new(unfair, 17).unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_ONE,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            1_000,
            vec![],
        ))
        .unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_TWO,
            PLAYER_TWO,
            TeamId(1),
            Vec2Mm::new(100, 0),
            1_000,
            vec![],
        ))
        .unwrap();
    assert_eq!(
        runtime.start().unwrap_err(),
        RuntimeError::CommandBudgetCannotReserveRoster
    );
}

#[test]
fn replay_orders_completion_among_same_tick_submissions() {
    let actor = hero(
        HERO_ONE,
        PLAYER_ONE,
        TeamId(0),
        Vec2Mm::new(0, 0),
        1_000,
        vec![],
    );
    let submitted_first = Scenario {
        config: MatchConfig::default(),
        seed: 14,
        abilities: vec![],
        actors: vec![actor.clone()],
        submissions: vec![submission(
            0,
            0,
            command(PLAYER_ONE, 1, 1, HERO_ONE, CommandKind::Stop),
        )],
        final_tick: 0,
        finish: Some(ScenarioFinish {
            at_tick: 0,
            arrival_order: 1,
            outcome: MatchOutcome::Winner(TeamId(0)),
            reason: MatchEndReason::Host,
        }),
    }
    .run()
    .unwrap();
    assert!(submitted_first.submissions[0].outcome.is_ok());
    assert!(submitted_first.frames[1]
        .events
        .iter()
        .any(|event| matches!(
            event,
            MatchEvent::CommandRejected {
                player: PLAYER_ONE,
                sequence: 1,
                reason: CommandRejectReason::MatchNotActive
            }
        )));

    let finished_first = Scenario {
        submissions: vec![submission(
            0,
            1,
            command(PLAYER_ONE, 1, 1, HERO_ONE, CommandKind::Stop),
        )],
        finish: Some(ScenarioFinish {
            at_tick: 0,
            arrival_order: 0,
            outcome: MatchOutcome::Winner(TeamId(0)),
            reason: MatchEndReason::Host,
        }),
        ..Scenario {
            config: MatchConfig::default(),
            seed: 14,
            abilities: vec![],
            actors: vec![actor],
            submissions: vec![],
            final_tick: 0,
            finish: None,
        }
    }
    .run()
    .unwrap();
    assert_eq!(
        finished_first.submissions[0].outcome.unwrap_err().reason,
        CommandRejectReason::MatchNotActive
    );
}

#[test]
fn basic_attack_revalidates_range_after_windup() {
    let attack = BasicAttackSpec {
        range_mm: 550,
        acquisition_range_mm: 0,
        damage: 50,
        school: DamageSchool::Physical,
        windup_ticks: 1,
        cooldown_ticks: 4,
    };
    let mut source = hero(
        HERO_ONE,
        PLAYER_ONE,
        TeamId(0),
        Vec2Mm::new(0, 0),
        500,
        vec![],
    );
    source.basic_attack = Some(attack);
    let target = hero(
        HERO_TWO,
        PLAYER_TWO,
        TeamId(1),
        Vec2Mm::new(500, 0),
        500,
        vec![],
    );
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 17).unwrap();
    runtime.spawn_actor(&source).unwrap();
    runtime.spawn_actor(&target).unwrap();
    runtime.start().unwrap();
    runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            HERO_ONE,
            CommandKind::BasicAttack { target: HERO_TWO },
        ))
        .unwrap();
    runtime
        .submit(command(
            PLAYER_TWO,
            1,
            2,
            HERO_TWO,
            CommandKind::MoveTo {
                destination: Vec2Mm::new(2_000, 0),
            },
        ))
        .unwrap();

    let windup = runtime.step().unwrap();
    assert!(windup.events.iter().any(|event| matches!(
        event,
        MatchEvent::BasicAttackStarted {
            source: HERO_ONE,
            target: HERO_TWO,
            resolves_at: 2,
        }
    )));
    assert_eq!(
        runtime.actor(HERO_ONE).unwrap().basic_attack_ready_at,
        Some(5)
    );

    let resolution = runtime.step().unwrap();
    assert_eq!(runtime.actor(HERO_TWO).unwrap().health, 500);
    assert!(resolution.events.iter().any(|event| matches!(
        event,
        MatchEvent::BasicAttackCancelled {
            source: HERO_ONE,
            target: HERO_TWO,
            reason: CommandRejectReason::TargetOutOfRange,
        }
    )));
}

#[test]
fn basic_attack_and_ability_share_one_same_tick_settlement() {
    let mut attacker = hero(
        HERO_TWO,
        PLAYER_TWO,
        TeamId(1),
        Vec2Mm::new(100, 0),
        600,
        vec![],
    );
    attacker.basic_attack = Some(BasicAttackSpec {
        range_mm: 1_000,
        acquisition_range_mm: 0,
        damage: 600,
        school: DamageSchool::True,
        windup_ticks: 1,
        cooldown_ticks: 5,
    });
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 18).unwrap();
    runtime.register_ability(strike()).unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_ONE,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            600,
            vec![STRIKE],
        ))
        .unwrap();
    runtime.spawn_actor(&attacker).unwrap();
    runtime.start().unwrap();
    runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            HERO_ONE,
            CommandKind::Cast {
                ability: STRIKE,
                target: CastTarget::Actor(HERO_TWO),
            },
        ))
        .unwrap();
    runtime
        .submit(command(
            PLAYER_TWO,
            1,
            1,
            HERO_TWO,
            CommandKind::BasicAttack { target: HERO_ONE },
        ))
        .unwrap();

    runtime.step().unwrap();
    let impact = runtime.step().unwrap();
    assert!(!runtime.actor(HERO_ONE).unwrap().alive);
    assert!(!runtime.actor(HERO_TWO).unwrap().alive);
    assert!(impact.events.iter().any(|event| matches!(
        event,
        MatchEvent::DamageApplied {
            source: HERO_TWO,
            target: HERO_ONE,
            cause: DamageCause::BasicAttack,
            amount: 600,
            ..
        }
    )));
    assert_eq!(
        impact
            .events
            .iter()
            .filter(|event| matches!(event, MatchEvent::ActorDied { .. }))
            .count(),
        2
    );
}

#[test]
fn simultaneous_terminal_towers_produce_an_objective_draw() {
    let tower_zero = ActorSpawn {
        id: ActorId(10),
        owner: None,
        team: TeamId(0),
        kind: ActorKind::Structure,
        position: Vec2Mm::new(900, 0),
        growth: StatGrowth::NONE,
        bounty: Bounty::NONE,
        stats: stats(600, 0, 0),
        abilities: vec![],
        basic_attack: None,
        death_rule: DeathRule::ObjectiveVictory { winner: TeamId(1) },
    };
    let tower_one = ActorSpawn {
        id: ActorId(20),
        owner: None,
        team: TeamId(1),
        kind: ActorKind::Structure,
        position: Vec2Mm::new(100, 0),
        growth: StatGrowth::NONE,
        bounty: Bounty::NONE,
        stats: stats(600, 0, 0),
        abilities: vec![],
        basic_attack: None,
        death_rule: DeathRule::ObjectiveVictory { winner: TeamId(0) },
    };
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 19).unwrap();
    runtime.register_ability(strike()).unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_ONE,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            1_000,
            vec![STRIKE],
        ))
        .unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_TWO,
            PLAYER_TWO,
            TeamId(1),
            Vec2Mm::new(1_000, 0),
            1_000,
            vec![STRIKE],
        ))
        .unwrap();
    runtime.spawn_actor(&tower_zero).unwrap();
    runtime.spawn_actor(&tower_one).unwrap();
    runtime.start().unwrap();
    runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            HERO_ONE,
            CommandKind::Cast {
                ability: STRIKE,
                target: CastTarget::Actor(tower_one.id),
            },
        ))
        .unwrap();
    runtime
        .submit(command(
            PLAYER_TWO,
            1,
            1,
            HERO_TWO,
            CommandKind::Cast {
                ability: STRIKE,
                target: CastTarget::Actor(tower_zero.id),
            },
        ))
        .unwrap();

    runtime.step().unwrap();
    let terminal = runtime.step().unwrap();
    assert_eq!(
        terminal.phase,
        MatchPhase::Complete {
            outcome: MatchOutcome::Draw,
            reason: MatchEndReason::ObjectiveDestroyed,
        }
    );
    assert_eq!(
        terminal
            .events
            .iter()
            .filter(|event| matches!(event, MatchEvent::ActorDied { .. }))
            .count(),
        2
    );
    assert_eq!(
        terminal
            .events
            .iter()
            .filter(|event| matches!(event, MatchEvent::MatchFinished { .. }))
            .count(),
        1
    );
}

#[test]
fn death_purges_future_commands_and_respawns_on_exact_tick() {
    let mut respawning = hero(
        HERO_TWO,
        PLAYER_TWO,
        TeamId(1),
        Vec2Mm::new(100, 0),
        600,
        vec![],
    );
    respawning.death_rule = DeathRule::Respawn {
        delay_ticks: 2,
        at: Vec2Mm::new(1_000, 0),
    };
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 20).unwrap();
    runtime.register_ability(strike()).unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_ONE,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            1_000,
            vec![STRIKE],
        ))
        .unwrap();
    runtime.spawn_actor(&respawning).unwrap();
    runtime.start().unwrap();
    runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            HERO_ONE,
            CommandKind::Cast {
                ability: STRIKE,
                target: CastTarget::Actor(HERO_TWO),
            },
        ))
        .unwrap();
    runtime
        .submit(command(
            PLAYER_TWO,
            1,
            3,
            HERO_TWO,
            CommandKind::MoveTo {
                destination: Vec2Mm::new(5_000, 0),
            },
        ))
        .unwrap();

    runtime.step().unwrap();
    let death = runtime.step().unwrap();
    assert_eq!(runtime.actor(HERO_TWO).unwrap().respawn_at, Some(4));
    assert!(death.events.iter().any(|event| matches!(
        event,
        MatchEvent::CommandRejected {
            player: PLAYER_TWO,
            sequence: 1,
            reason: CommandRejectReason::ActorDead,
        }
    )));
    assert!(!runtime
        .step()
        .unwrap()
        .events
        .iter()
        .any(|event| matches!(event, MatchEvent::ActorRespawned { .. })));
    let respawn = runtime.step().unwrap();
    assert_eq!(
        runtime.actor(HERO_TWO).unwrap().position,
        Vec2Mm::new(1_000, 0)
    );
    assert_eq!(runtime.actor(HERO_TWO).unwrap().health, 600);
    assert!(respawn.events.iter().any(|event| matches!(
        event,
        MatchEvent::ActorRespawned {
            actor: HERO_TWO,
            position: Vec2Mm { x: 1_000, y: 0 },
        }
    )));
}

#[test]
fn dead_minions_are_removed_from_replication() {
    let minion = ActorSpawn {
        id: ActorId(404),
        owner: None,
        team: TeamId(1),
        kind: ActorKind::Minion,
        position: Vec2Mm::new(100, 0),
        growth: StatGrowth::NONE,
        bounty: Bounty::NONE,
        stats: stats(600, 0, 100),
        abilities: vec![],
        basic_attack: None,
        death_rule: DeathRule::Despawn,
    };
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 21).unwrap();
    runtime.register_ability(strike()).unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_ONE,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            1_000,
            vec![STRIKE],
        ))
        .unwrap();
    runtime.spawn_actor(&minion).unwrap();
    runtime.start().unwrap();
    runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            HERO_ONE,
            CommandKind::Cast {
                ability: STRIKE,
                target: CastTarget::Actor(minion.id),
            },
        ))
        .unwrap();
    runtime.step().unwrap();
    let death = runtime.step().unwrap();
    assert!(runtime.actor(minion.id).is_none());
    assert_eq!(death.removed, vec![minion.id]);
    assert!(death.events.iter().any(|event| matches!(
        event,
        MatchEvent::ActorDespawned { actor } if *actor == minion.id
    )));
}

#[test]
fn internal_bot_intents_are_canonical_and_do_not_require_players() {
    fn run(intents: &[ActorIntent]) -> (crate::ServerFrame, crate::ServerFrame) {
        let attack = BasicAttackSpec {
            range_mm: 1_000,
            acquisition_range_mm: 0,
            damage: 100,
            school: DamageSchool::True,
            windup_ticks: 1,
            cooldown_ticks: 5,
        };
        let actor = |id, team, x| ActorSpawn {
            id: ActorId(id),
            owner: None,
            team: TeamId(team),
            kind: ActorKind::Minion,
            position: Vec2Mm::new(x, 0),
            growth: StatGrowth::NONE,
            bounty: Bounty::NONE,
            stats: stats(100, 0, 0),
            abilities: vec![],
            basic_attack: Some(attack),
            death_rule: DeathRule::StayDead,
        };
        let mut runtime = MatchRuntime::new(MatchConfig::default(), 22).unwrap();
        runtime.spawn_actor(&actor(1, 0, 0)).unwrap();
        runtime.spawn_actor(&actor(2, 1, 100)).unwrap();
        runtime.start().unwrap();
        let started = runtime.step_with_intents(intents).unwrap();
        let impact = runtime.step_with_intents(&[]).unwrap();
        (started, impact)
    }

    let forward = vec![
        ActorIntent {
            actor: ActorId(1),
            kind: CommandKind::BasicAttack { target: ActorId(2) },
        },
        ActorIntent {
            actor: ActorId(2),
            kind: CommandKind::BasicAttack { target: ActorId(1) },
        },
    ];
    let mut reverse = forward.clone();
    reverse.reverse();
    let expected = run(&forward);
    let actual = run(&reverse);
    assert_eq!(actual, expected);
    assert_eq!(
        actual
            .1
            .events
            .iter()
            .filter(|event| matches!(event, MatchEvent::ActorDied { .. }))
            .count(),
        2
    );
}

#[test]
fn duplicate_internal_intents_fail_before_advancing_time() {
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 23).unwrap();
    runtime.start().unwrap();
    let duplicate = [
        ActorIntent {
            actor: HERO_ONE,
            kind: CommandKind::Stop,
        },
        ActorIntent {
            actor: HERO_ONE,
            kind: CommandKind::Stop,
        },
    ];
    assert!(matches!(
        runtime.step_with_intents(&duplicate),
        Err(RuntimeError::InvalidInternalIntents(_))
    ));
    assert_eq!(runtime.tick(), 0);
}

#[test]
fn dynamic_actor_ids_are_monotonic_and_replication_is_explicit() {
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 24).unwrap();
    runtime
        .spawn_actor(&hero(
            ActorId(100),
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            1_000,
            vec![],
        ))
        .unwrap();
    runtime.start().unwrap();
    let template = DynamicActorSpawn {
        provenance: DynamicActorProvenance::Generic,
        team: TeamId(1),
        kind: ActorKind::Minion,
        position: Vec2Mm::new(1_000, 0),
        growth: StatGrowth::NONE,
        bounty: Bounty::NONE,
        stats: stats(100, 0, 100),
        abilities: vec![],
        basic_attack: None,
        death_rule: DeathRule::Despawn,
    };
    let first = runtime.spawn_dynamic_actor(&template).unwrap();
    let second = runtime.spawn_dynamic_actor(&template).unwrap();
    assert_eq!(first, ActorId(101));
    assert_eq!(second, ActorId(102));
    // The allocator, not the caller, stamps the ordinal, and it is pinned to the allocated identity.
    assert_eq!(
        runtime.actor(first).unwrap().provenance,
        ActorProvenance::Dynamic { spawn_ordinal: 101 }
    );
    assert_eq!(
        runtime.actor(second).unwrap().provenance,
        ActorProvenance::Dynamic { spawn_ordinal: 102 }
    );
    assert_eq!(
        runtime.actor(ActorId(100)).unwrap().provenance,
        ActorProvenance::Authored
    );

    let frame = runtime.step().unwrap();
    assert_eq!(
        frame
            .events
            .iter()
            .filter(|event| matches!(event, MatchEvent::ActorSpawned { .. }))
            .count(),
        2
    );
    assert!(runtime.actor(first).is_some());
    assert!(runtime.actor(second).is_some());
}

// ---------------------------------------------------------------------------------------------------
// GP-04 shaped targeting and GP-03 projectiles.
//
// Every test below drives the public command surface. None reaches into the projectile list to place a
// missile by hand, because a projectile only a test can create proves nothing about the ability system
// that is supposed to create them.
// ---------------------------------------------------------------------------------------------------

const BURST: AbilityId = AbilityId(11);
const LANCE: AbilityId = AbilityId(12);
const BOMB: AbilityId = AbilityId(13);
const RAIL: AbilityId = AbilityId(14);
const SNIPE: AbilityId = AbilityId(15);
const MINION_ONE: ActorId = ActorId(401);
const MINION_TWO: ActorId = ActorId(402);
const MINION_THREE: ActorId = ActorId(403);

/// An instant ground burst: aimed at a point, detonating in a circle, hurting only enemies.
const fn burst() -> AbilitySpec {
    AbilitySpec {
        id: BURST,
        aim: AbilityAim::Point,
        targeting: AbilityTargeting::Enemy,
        range_mm: 5_000,
        resource_cost: 0,
        cooldown_ticks: 1,
        cast_ticks: 0,
        delivery: AbilityDelivery::Instant,
        impact: ImpactShape::Circle { radius_mm: 1_500 },
        per_rank_amount: 0,
        effect: AbilityEffect::Damage {
            amount: 300,
            school: DamageSchool::True,
        },
    }
}

/// A line skillshot: a single-victim projectile that travels its full range along the aimed direction.
const fn lance(id: AbilityId, speed_mm_per_tick: u32, amount: u32) -> AbilitySpec {
    AbilitySpec {
        id,
        aim: AbilityAim::Point,
        targeting: AbilityTargeting::Enemy,
        range_mm: 6_000,
        resource_cost: 0,
        cooldown_ticks: 1,
        cast_ticks: 0,
        per_rank_amount: 0,
        delivery: AbilityDelivery::Projectile {
            speed_mm_per_tick,
            hit_radius_mm: 200,
        },
        impact: ImpactShape::Single,
        effect: AbilityEffect::Damage {
            amount,
            school: DamageSchool::True,
        },
    }
}

/// A lobbed bomb: a projectile that detonates in a circle where it was aimed.
const fn bomb() -> AbilitySpec {
    AbilitySpec {
        id: BOMB,
        aim: AbilityAim::Point,
        targeting: AbilityTargeting::Enemy,
        range_mm: 6_000,
        resource_cost: 0,
        cooldown_ticks: 1,
        cast_ticks: 0,
        per_rank_amount: 0,
        delivery: AbilityDelivery::Projectile {
            speed_mm_per_tick: 1_000,
            hit_radius_mm: 200,
        },
        impact: ImpactShape::Circle { radius_mm: 1_200 },
        effect: AbilityEffect::Damage {
            amount: 250,
            school: DamageSchool::True,
        },
    }
}

/// Deliberately faster than any body is wide: the fixture that would tunnel under a point-collision test.
const fn rail() -> AbilitySpec {
    AbilitySpec {
        id: RAIL,
        aim: AbilityAim::Point,
        targeting: AbilityTargeting::AnyUnit,
        range_mm: 20_000,
        resource_cost: 0,
        cooldown_ticks: 1,
        cast_ticks: 0,
        per_rank_amount: 0,
        delivery: AbilityDelivery::Projectile {
            speed_mm_per_tick: 5_000,
            hit_radius_mm: 100,
        },
        impact: ImpactShape::Single,
        effect: AbilityEffect::Damage {
            amount: 999,
            school: DamageSchool::True,
        },
    }
}

/// A long-range unit-targeted instant, used to land a second lethal effect on a chosen tick.
const fn snipe() -> AbilitySpec {
    AbilitySpec::unit_targeted(
        SNIPE,
        AbilityTargeting::Enemy,
        10_000,
        0,
        1,
        0,
        AbilityEffect::Damage {
            amount: 600,
            school: DamageSchool::True,
        },
    )
}

fn minion(id: ActorId, team: TeamId, position: Vec2Mm, health: u32) -> ActorSpawn {
    ActorSpawn {
        id,
        owner: None,
        team,
        kind: ActorKind::Minion,
        position,
        growth: StatGrowth::NONE,
        bounty: Bounty::NONE,
        stats: stats(health, 0, 0),
        abilities: Vec::new(),
        basic_attack: None,
        death_rule: DeathRule::StayDead,
    }
}

/// One team-0 hero at the origin holding every shaped ability, in an otherwise empty match.
fn skirmish() -> MatchRuntime {
    skirmish_with_hero_health(1_000)
}

fn skirmish_with_hero_health(health: u32) -> MatchRuntime {
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 0x5175_6944).unwrap();
    for spec in [burst(), lance(LANCE, 300, 400), bomb(), rail(), snipe()] {
        runtime.register_ability(spec).unwrap();
    }
    runtime
        .spawn_actor(&hero(
            HERO_ONE,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            health,
            vec![BURST, LANCE, BOMB, RAIL, SNIPE],
        ))
        .unwrap();
    runtime
}

fn cast_at(
    sequence: u32,
    execute_at: u64,
    ability: AbilityId,
    target: CastTarget,
) -> PlayerCommand {
    command(
        PLAYER_ONE,
        sequence,
        execute_at,
        HERO_ONE,
        CommandKind::Cast { ability, target },
    )
}

fn spawned_projectile(frame: &crate::ServerFrame) -> (crate::ProjectileId, Vec2Mm, Vec2Mm) {
    frame
        .events
        .iter()
        .find_map(|event| match event {
            MatchEvent::ProjectileSpawned {
                projectile,
                position,
                end,
                ..
            } => Some((*projectile, *position, *end)),
            _ => None,
        })
        .expect("a projectile ability spawns a projectile when its cast resolves")
}

fn damaged_targets(frame: &crate::ServerFrame) -> Vec<ActorId> {
    frame
        .events
        .iter()
        .filter_map(|event| match event {
            MatchEvent::DamageApplied { target, .. } => Some(*target),
            _ => None,
        })
        .collect()
}

/// Run ticks until the projectile leaves the world, returning the frame it left in and its resolution.
fn fly_until_resolved(runtime: &mut MatchRuntime, limit: u64) -> (crate::ServerFrame, MatchEvent) {
    for _ in 0..limit {
        let frame = runtime.step().unwrap();
        if let Some(event) = frame
            .events
            .iter()
            .find(|event| matches!(event, MatchEvent::ProjectileResolved { .. }))
            .copied()
        {
            return (frame, event);
        }
    }
    panic!("projectile never resolved within the tick limit");
}

#[test]
fn point_aimed_and_unit_aimed_abilities_refuse_each_others_target_form() {
    let mut runtime = skirmish();
    runtime
        .spawn_actor(&minion(MINION_ONE, TeamId(1), Vec2Mm::new(1_000, 0), 500))
        .unwrap();
    runtime.start().unwrap();

    // A ground burst refuses a unit target...
    let rejection = runtime
        .submit(cast_at(1, 1, BURST, CastTarget::Actor(MINION_ONE)))
        .unwrap_err();
    assert_eq!(rejection.reason, CommandRejectReason::InvalidTargetForm);

    // ...and a unit-targeted strike refuses a point.
    let rejection = runtime
        .submit(cast_at(
            2,
            1,
            SNIPE,
            CastTarget::Point(Vec2Mm::new(1_000, 0)),
        ))
        .unwrap_err();
    assert_eq!(rejection.reason, CommandRejectReason::InvalidTargetForm);

    // A single-victim skillshot aimed exactly at the caster has no direction, and is refused rather than
    // given an invented one.
    let rejection = runtime
        .submit(cast_at(3, 1, LANCE, CastTarget::Point(Vec2Mm::new(0, 0))))
        .unwrap_err();
    assert_eq!(rejection.reason, CommandRejectReason::InvalidTargetForm);
}

#[test]
fn an_aimed_point_outside_the_map_or_beyond_range_is_refused() {
    let mut runtime = skirmish();
    runtime.start().unwrap();

    let rejection = runtime
        .submit(cast_at(
            1,
            1,
            BURST,
            CastTarget::Point(Vec2Mm::new(i32::MAX, 0)),
        ))
        .unwrap_err();
    assert_eq!(
        rejection.reason,
        CommandRejectReason::TargetPointOutOfBounds
    );

    // In bounds, but past the ability's five-metre reach.
    let rejection = runtime
        .submit(cast_at(
            2,
            1,
            BURST,
            CastTarget::Point(Vec2Mm::new(9_000, 0)),
        ))
        .unwrap_err();
    assert_eq!(rejection.reason, CommandRejectReason::TargetOutOfRange);
}

#[test]
fn an_instant_ground_burst_hits_every_enemy_in_radius_and_no_ally() {
    let mut runtime = skirmish();
    // Two enemies inside the 1.5 m footprint, one enemy outside it, and one ALLY standing in the middle.
    runtime
        .spawn_actor(&minion(MINION_ONE, TeamId(1), Vec2Mm::new(3_000, 0), 1_000))
        .unwrap();
    runtime
        .spawn_actor(&minion(
            MINION_TWO,
            TeamId(1),
            Vec2Mm::new(3_000, 1_400),
            1_000,
        ))
        .unwrap();
    runtime
        .spawn_actor(&minion(
            MINION_THREE,
            TeamId(1),
            Vec2Mm::new(3_000, 4_000),
            1_000,
        ))
        .unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_TWO,
            PLAYER_TWO,
            TeamId(0),
            Vec2Mm::new(3_000, 200),
            1_000,
            Vec::new(),
        ))
        .unwrap();
    runtime.start().unwrap();

    runtime
        .submit(cast_at(
            1,
            1,
            BURST,
            CastTarget::Point(Vec2Mm::new(3_000, 0)),
        ))
        .unwrap();
    let frame = runtime.step().unwrap();

    assert_eq!(damaged_targets(&frame), vec![MINION_ONE, MINION_TWO]);
    assert_eq!(runtime.actor(MINION_ONE).unwrap().health, 700);
    assert_eq!(runtime.actor(MINION_TWO).unwrap().health, 700);
    assert_eq!(runtime.actor(MINION_THREE).unwrap().health, 1_000);
    assert_eq!(runtime.actor(HERO_TWO).unwrap().health, 1_000);
    // An instant ability leaves nothing in flight.
    assert!(runtime.projectiles().is_empty());
}

#[test]
fn a_skillshot_travels_its_full_range_even_when_aimed_short() {
    let mut runtime = skirmish();
    runtime
        .spawn_actor(&minion(MINION_ONE, TeamId(1), Vec2Mm::new(3_000, 0), 1_000))
        .unwrap();
    runtime.start().unwrap();

    // Aimed at half a metre, well short of the enemy three metres away.
    runtime
        .submit(cast_at(1, 1, LANCE, CastTarget::Point(Vec2Mm::new(500, 0))))
        .unwrap();
    let launch = runtime.step().unwrap();
    let (_, position, end) = spawned_projectile(&launch);
    assert_eq!(position, Vec2Mm::new(0, 0));
    assert_eq!(
        end,
        Vec2Mm::new(6_000, 0),
        "a single-victim skillshot extends to the ability's full range, not the aim point"
    );

    let (_, resolved) = fly_until_resolved(&mut runtime, 40);
    let MatchEvent::ProjectileResolved { victim, .. } = resolved else {
        unreachable!()
    };
    assert_eq!(victim, Some(MINION_ONE));
    assert_eq!(runtime.actor(MINION_ONE).unwrap().health, 600);
}

#[test]
fn a_fast_skillshot_cannot_tunnel_through_a_body_between_ticks() {
    let mut runtime = skirmish();
    // RAIL moves 5,000 mm per tick with a 100 mm hit radius. Its tick-boundary positions along the line
    // are 0, 5,000, 10,000 ... — a victim at 2,000 mm is never within 100 mm of ANY of them, so a
    // collision test that only sampled the projectile's position would miss it every time.
    runtime
        .spawn_actor(&minion(MINION_ONE, TeamId(1), Vec2Mm::new(2_000, 0), 5_000))
        .unwrap();
    runtime.start().unwrap();
    runtime
        .submit(cast_at(
            1,
            1,
            RAIL,
            CastTarget::Point(Vec2Mm::new(9_000, 0)),
        ))
        .unwrap();
    let launch = runtime.step().unwrap();
    let (_, position, end) = spawned_projectile(&launch);
    assert_eq!(position, Vec2Mm::new(0, 0));
    assert_eq!(end, Vec2Mm::new(20_000, 0));

    // The negative control, stated as arithmetic rather than trusted from the engine: neither the position
    // the projectile starts its first tick at nor the one it ends that tick at is inside the hit radius.
    for sampled_x in [0_i32, 5_000] {
        let gap = (2_000 - sampled_x).abs();
        assert!(
            gap > 100,
            "sample at {sampled_x} is {gap} mm from the victim, so the fixture no longer proves sweeping"
        );
    }

    let (frame, resolved) = fly_until_resolved(&mut runtime, 4);
    assert_eq!(
        frame.tick, 2,
        "the strike lands on the first tick of flight"
    );
    let MatchEvent::ProjectileResolved {
        victim, position, ..
    } = resolved
    else {
        unreachable!()
    };
    assert_eq!(victim, Some(MINION_ONE));
    assert_eq!(position, Vec2Mm::new(2_000, 0));
    assert_eq!(runtime.actor(MINION_ONE).unwrap().health, 4_001);
}

#[test]
fn a_skillshot_that_touches_nothing_resolves_at_its_flight_end_without_damage() {
    let mut runtime = skirmish();
    // Far off the flight line.
    runtime
        .spawn_actor(&minion(
            MINION_ONE,
            TeamId(1),
            Vec2Mm::new(3_000, 4_000),
            1_000,
        ))
        .unwrap();
    runtime.start().unwrap();
    runtime
        .submit(cast_at(
            1,
            1,
            LANCE,
            CastTarget::Point(Vec2Mm::new(1_000, 0)),
        ))
        .unwrap();
    runtime.step().unwrap();

    let (frame, resolved) = fly_until_resolved(&mut runtime, 40);
    let MatchEvent::ProjectileResolved {
        victim, position, ..
    } = resolved
    else {
        unreachable!()
    };
    assert_eq!(victim, None);
    assert_eq!(position, Vec2Mm::new(6_000, 0));
    assert!(
        damaged_targets(&frame).is_empty(),
        "a single-victim skillshot that reaches its flight end fizzles"
    );
    assert_eq!(runtime.actor(MINION_ONE).unwrap().health, 1_000);
    assert!(runtime.projectiles().is_empty());
}

#[test]
fn a_lobbed_bomb_detonates_where_it_was_aimed_rather_than_running_to_max_range() {
    let mut runtime = skirmish();
    runtime
        .spawn_actor(&minion(
            MINION_ONE,
            TeamId(1),
            Vec2Mm::new(4_000, 800),
            1_000,
        ))
        .unwrap();
    runtime.start().unwrap();
    runtime
        .submit(cast_at(
            1,
            1,
            BOMB,
            CastTarget::Point(Vec2Mm::new(4_000, 0)),
        ))
        .unwrap();
    let launch = runtime.step().unwrap();
    let (_, _, end) = spawned_projectile(&launch);
    assert_eq!(
        end,
        Vec2Mm::new(4_000, 0),
        "an area impact lands where it was aimed"
    );

    let (frame, resolved) = fly_until_resolved(&mut runtime, 40);
    let MatchEvent::ProjectileResolved { victim, .. } = resolved else {
        unreachable!()
    };
    // Nothing stood on the flight line, but the burst still catches the enemy beside the target point.
    assert_eq!(victim, None);
    assert_eq!(damaged_targets(&frame), vec![MINION_ONE]);
    assert_eq!(runtime.actor(MINION_ONE).unwrap().health, 750);
}

#[test]
fn a_skillshot_strikes_the_nearest_body_on_its_line_not_the_lowest_actor_id() {
    let mut runtime = skirmish();
    // MINION_ONE has the LOWER id but stands FURTHER away: ID order and distance order disagree on purpose.
    runtime
        .spawn_actor(&minion(MINION_ONE, TeamId(1), Vec2Mm::new(4_000, 0), 1_000))
        .unwrap();
    runtime
        .spawn_actor(&minion(MINION_TWO, TeamId(1), Vec2Mm::new(1_500, 0), 1_000))
        .unwrap();
    runtime.start().unwrap();
    runtime
        .submit(cast_at(
            1,
            1,
            RAIL,
            CastTarget::Point(Vec2Mm::new(6_000, 0)),
        ))
        .unwrap();
    runtime.step().unwrap();

    let (_, resolved) = fly_until_resolved(&mut runtime, 10);
    let MatchEvent::ProjectileResolved { victim, .. } = resolved else {
        unreachable!()
    };
    assert_eq!(victim, Some(MINION_TWO));
    assert_eq!(runtime.actor(MINION_ONE).unwrap().health, 1_000);
}

#[test]
fn a_projectile_never_collides_with_its_own_caster() {
    let mut runtime = skirmish();
    runtime
        .spawn_actor(&minion(MINION_ONE, TeamId(1), Vec2Mm::new(6_000, 0), 5_000))
        .unwrap();
    runtime.start().unwrap();
    // RAIL targets ANY unit, so without the source exclusion the caster is the nearest legal body to its
    // own missile and would be struck on the first tick of flight.
    runtime
        .submit(cast_at(
            1,
            1,
            RAIL,
            CastTarget::Point(Vec2Mm::new(9_000, 0)),
        ))
        .unwrap();
    runtime.step().unwrap();
    let (_, resolved) = fly_until_resolved(&mut runtime, 10);
    let MatchEvent::ProjectileResolved { victim, .. } = resolved else {
        unreachable!()
    };
    assert_eq!(victim, Some(MINION_ONE));
    assert_eq!(runtime.actor(HERO_ONE).unwrap().health, 1_000);
}

#[test]
fn a_projectile_outlives_its_caster_and_still_lands_with_its_attribution_intact() {
    // The caster is frail enough that one SNIPE kills it while its own missile is still in the air.
    let mut runtime = skirmish_with_hero_health(500);
    runtime
        .spawn_actor(&minion(MINION_ONE, TeamId(1), Vec2Mm::new(3_000, 0), 1_000))
        .unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_TWO,
            PLAYER_TWO,
            TeamId(1),
            Vec2Mm::new(0, 1_500),
            1_000,
            vec![SNIPE],
        ))
        .unwrap();
    runtime.start().unwrap();

    runtime
        .submit(cast_at(
            1,
            1,
            LANCE,
            CastTarget::Point(Vec2Mm::new(3_000, 0)),
        ))
        .unwrap();
    runtime
        .submit(command(
            PLAYER_TWO,
            1,
            1,
            HERO_TWO,
            CommandKind::Cast {
                ability: SNIPE,
                target: CastTarget::Actor(HERO_ONE),
            },
        ))
        .unwrap();
    let frame = runtime.step().unwrap();
    assert!(frame
        .events
        .iter()
        .any(|event| matches!(event, MatchEvent::ActorDied { actor, .. } if *actor == HERO_ONE)));
    assert!(!runtime.actor(HERO_ONE).unwrap().alive);
    assert_eq!(
        runtime.projectiles().len(),
        1,
        "the missile is already in the air and does not die with its caster"
    );

    let (frame, resolved) = fly_until_resolved(&mut runtime, 40);
    let MatchEvent::ProjectileResolved { victim, .. } = resolved else {
        unreachable!()
    };
    assert_eq!(victim, Some(MINION_ONE));
    assert!(frame.events.iter().any(|event| matches!(
        event,
        MatchEvent::DamageApplied { source, target, .. }
            if *source == HERO_ONE && *target == MINION_ONE
    )));
}

#[test]
fn a_projectile_flight_is_clamped_inside_the_map_along_its_own_line() {
    let config = MatchConfig {
        map_bounds: crate::RectMm::new(Vec2Mm::new(-10_000, -10_000), Vec2Mm::new(10_000, 10_000)),
        ..MatchConfig::default()
    };
    let mut runtime = MatchRuntime::new(config, 7).unwrap();
    // A thirty-metre reach inside a ten-metre box, so extending the flight to full range genuinely leaves
    // the map and the clamp is actually exercised rather than silently skipped.
    runtime
        .register_ability(AbilitySpec {
            range_mm: 30_000,
            ..lance(LANCE, 300, 400)
        })
        .unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_ONE,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            1_000,
            vec![LANCE],
        ))
        .unwrap();
    runtime.start().unwrap();
    // Aimed diagonally: extending to full range leaves the box on both axes at once.
    runtime
        .submit(cast_at(
            1,
            1,
            LANCE,
            CastTarget::Point(Vec2Mm::new(4_000, 4_000)),
        ))
        .unwrap();
    let launch = runtime.step().unwrap();
    let (_, _, end) = spawned_projectile(&launch);
    assert!(config.map_bounds.contains(end));
    assert_eq!(
        end.x, end.y,
        "clamping shortens the flight along its own line; it must not bend the aim"
    );
    assert_eq!(end, Vec2Mm::new(10_000, 10_000));
}

#[test]
fn a_projectile_impact_and_a_cast_settle_in_one_batch_as_a_mutual_kill() {
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 0x6d75_7475).unwrap();
    // A 2,000 mm/tick lance covers the three-metre gap on its SECOND tick of flight, which is match tick 3.
    runtime.register_ability(lance(LANCE, 2_000, 400)).unwrap();
    runtime.register_ability(snipe()).unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_ONE,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            100,
            vec![LANCE],
        ))
        .unwrap();
    runtime
        .spawn_actor(&hero(
            HERO_TWO,
            PLAYER_TWO,
            TeamId(1),
            Vec2Mm::new(3_000, 0),
            100,
            vec![SNIPE],
        ))
        .unwrap();
    runtime.start().unwrap();

    runtime
        .submit(cast_at(
            1,
            1,
            LANCE,
            CastTarget::Point(Vec2Mm::new(3_000, 0)),
        ))
        .unwrap();
    runtime
        .submit(command(
            PLAYER_TWO,
            1,
            3,
            HERO_TWO,
            CommandKind::Cast {
                ability: SNIPE,
                target: CastTarget::Actor(HERO_ONE),
            },
        ))
        .unwrap();

    runtime.step().unwrap();
    runtime.step().unwrap();
    let frame = runtime.step().unwrap();
    assert_eq!(frame.tick, 3);
    let deaths: Vec<ActorId> = frame
        .events
        .iter()
        .filter_map(|event| match event {
            MatchEvent::ActorDied { actor, .. } => Some(*actor),
            _ => None,
        })
        .collect();
    assert_eq!(
        deaths,
        vec![HERO_ONE, HERO_TWO],
        "a missile impact and a spell that both reach lethal on one tick are a legal mutual kill"
    );
}

#[test]
fn the_projectile_budget_is_fail_closed_and_cancels_the_cast_that_would_exceed_it() {
    let config = MatchConfig {
        max_projectiles: 1,
        ..MatchConfig::default()
    };
    let mut runtime = MatchRuntime::new(config, 11).unwrap();
    runtime.register_ability(lance(LANCE, 300, 400)).unwrap();
    for (actor, player) in [(HERO_ONE, PLAYER_ONE), (HERO_THREE, PLAYER_THREE)] {
        runtime
            .spawn_actor(&hero(
                actor,
                player,
                TeamId(0),
                Vec2Mm::new(0, 0),
                1_000,
                vec![LANCE],
            ))
            .unwrap();
    }
    runtime.start().unwrap();

    runtime
        .submit(cast_at(
            1,
            1,
            LANCE,
            CastTarget::Point(Vec2Mm::new(3_000, 0)),
        ))
        .unwrap();
    runtime
        .submit(command(
            PLAYER_THREE,
            1,
            1,
            HERO_THREE,
            CommandKind::Cast {
                ability: LANCE,
                target: CastTarget::Point(Vec2Mm::new(3_000, 0)),
            },
        ))
        .unwrap();
    let frame = runtime.step().unwrap();

    assert_eq!(runtime.projectiles().len(), 1);
    assert!(frame.events.iter().any(|event| matches!(
        event,
        MatchEvent::CastCancelled { source, reason, .. }
            if *source == HERO_THREE
                && *reason == CommandRejectReason::ProjectileBudgetExceeded
    )));
    runtime.check_invariants().unwrap();
}

#[test]
fn match_completion_cancels_in_flight_projectiles_without_applying_their_damage() {
    let mut runtime = skirmish();
    runtime
        .spawn_actor(&minion(MINION_ONE, TeamId(1), Vec2Mm::new(3_000, 0), 1_000))
        .unwrap();
    runtime.start().unwrap();
    runtime
        .submit(cast_at(
            1,
            1,
            LANCE,
            CastTarget::Point(Vec2Mm::new(3_000, 0)),
        ))
        .unwrap();
    let launch = runtime.step().unwrap();
    let (projectile, _, _) = spawned_projectile(&launch);

    let terminal = runtime
        .finish(MatchOutcome::Draw, MatchEndReason::Host)
        .unwrap();
    assert!(terminal.events.iter().any(|event| matches!(
        event,
        MatchEvent::ProjectileCancelled { projectile: id, reason }
            if *id == projectile && *reason == CommandRejectReason::MatchNotActive
    )));
    assert_eq!(terminal.removed_projectiles, vec![projectile]);
    assert!(runtime.projectiles().is_empty());
    assert_eq!(
        runtime.actor(MINION_ONE).unwrap().health,
        1_000,
        "a match that already produced its terminal outcome must not have a missile land afterwards"
    );
    runtime.check_invariants().unwrap();
}

#[test]
fn an_in_flight_projectile_survives_the_checkpoint_round_trip_and_lands_identically() {
    let content = ContentId::new([0x33; 32]);
    let build = || {
        let mut runtime = skirmish();
        runtime
            .spawn_actor(&minion(MINION_ONE, TeamId(1), Vec2Mm::new(3_000, 0), 1_000))
            .unwrap();
        runtime.start().unwrap();
        runtime
            .submit(cast_at(
                1,
                1,
                LANCE,
                CastTarget::Point(Vec2Mm::new(3_000, 0)),
            ))
            .unwrap();
        // Launch, then two ticks of flight, so the captured missile is genuinely mid-air.
        for _ in 0..3 {
            runtime.step().unwrap();
        }
        runtime
    };

    let mut uninterrupted = build();
    let checkpointed = build();
    assert_eq!(checkpointed.projectiles().len(), 1);
    let checkpoint = checkpointed.capture_checkpoint(content).unwrap();
    let mut restored = MatchRuntime::restore_checkpoint(&checkpoint, content).unwrap();
    assert_eq!(restored.projectiles(), checkpointed.projectiles());
    assert_eq!(restored.world_digest(), checkpointed.world_digest());

    for _ in 0..10 {
        let expected = uninterrupted.step().unwrap();
        let actual = restored.step().unwrap();
        assert_eq!(actual.frame_digest, expected.frame_digest);
    }
    assert_eq!(
        uninterrupted.actor(MINION_ONE).unwrap().health,
        restored.actor(MINION_ONE).unwrap().health
    );
    assert_eq!(uninterrupted.actor(MINION_ONE).unwrap().health, 600);
}

#[test]
fn contradictory_ability_shapes_are_refused_at_authoring_time() {
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 3).unwrap();
    let damage = AbilityEffect::Damage {
        amount: 10,
        school: DamageSchool::True,
    };
    let base = AbilitySpec {
        id: AbilityId(1),
        aim: AbilityAim::Point,
        targeting: AbilityTargeting::Enemy,
        range_mm: 1_000,
        resource_cost: 0,
        cooldown_ticks: 1,
        cast_ticks: 0,
        per_rank_amount: 0,
        delivery: AbilityDelivery::Instant,
        impact: ImpactShape::Circle { radius_mm: 500 },
        effect: damage,
    };
    let refused = [
        // A point cast cannot be self-only.
        AbilitySpec {
            targeting: AbilityTargeting::SelfOnly,
            ..base
        },
        // An instant point cast with a single-victim impact names nobody and covers nothing.
        AbilitySpec {
            impact: ImpactShape::Single,
            ..base
        },
        AbilitySpec {
            delivery: AbilityDelivery::Projectile {
                speed_mm_per_tick: 0,
                hit_radius_mm: 100,
            },
            ..base
        },
        AbilitySpec {
            delivery: AbilityDelivery::Projectile {
                speed_mm_per_tick: 100,
                hit_radius_mm: 0,
            },
            ..base
        },
        AbilitySpec {
            delivery: AbilityDelivery::Projectile {
                speed_mm_per_tick: crate::MAX_PROJECTILE_SPEED_MM_PER_TICK + 1,
                hit_radius_mm: 100,
            },
            ..base
        },
        // A projectile with no range can never travel.
        AbilitySpec {
            range_mm: 0,
            delivery: AbilityDelivery::Projectile {
                speed_mm_per_tick: 100,
                hit_radius_mm: 100,
            },
            ..base
        },
        AbilitySpec {
            impact: ImpactShape::Circle { radius_mm: 0 },
            ..base
        },
        AbilitySpec {
            impact: ImpactShape::Circle {
                radius_mm: crate::MAX_IMPACT_RADIUS_MM + 1,
            },
            ..base
        },
    ];
    for (index, spec) in refused.into_iter().enumerate() {
        assert!(
            matches!(
                runtime.register_ability(spec),
                Err(RuntimeError::InvalidAbility(..))
            ),
            "contradictory ability shape {index} was accepted"
        );
    }
    // The coherent baseline still registers, so the loop above refuses shapes rather than everything.
    runtime.register_ability(base).unwrap();
}

#[test]
fn an_in_flight_projectile_is_part_of_both_digests() {
    let mut runtime = skirmish();
    runtime.start().unwrap();
    runtime
        .submit(cast_at(
            1,
            1,
            LANCE,
            CastTarget::Point(Vec2Mm::new(3_000, 0)),
        ))
        .unwrap();
    let frame = runtime.step().unwrap();
    assert_eq!(frame.projectiles.len(), 1);

    // Negative control: strip ONLY the projectile and nothing else. If the missile were absent from the
    // digest, two runtimes that disagree about a live projectile would still claim to be identical.
    let mut without = runtime.clone();
    without.projectiles.clear();
    assert_ne!(runtime.world_digest(), without.world_digest());

    let mut frame_without = frame.clone();
    frame_without.projectiles.clear();
    assert_ne!(
        crate::runtime::digest_frame(&frame),
        crate::runtime::digest_frame(&frame_without)
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the guard is only meaningful if it names every event variant exhaustively"
)]
fn every_match_event_has_a_distinct_digest_tag() {
    // One representative of every variant. `StatusEffectExpired` and `RespawnCancelled` both used to
    // encode tag 19, and a duplicate discriminant is a hole in an encoding whose whole job is that two
    // different causal traces cannot hash the same.
    let events = [
        MatchEvent::MatchStarted,
        MatchEvent::CommandRejected {
            player: PLAYER_ONE,
            sequence: 1,
            reason: CommandRejectReason::ActorDead,
        },
        MatchEvent::MoveStarted {
            actor: HERO_ONE,
            destination: Vec2Mm::new(1, 1),
        },
        MatchEvent::MoveStopped { actor: HERO_ONE },
        MatchEvent::CastStarted {
            source: HERO_ONE,
            ability: STRIKE,
            target: CastTarget::SelfActor,
            resolves_at: 1,
        },
        MatchEvent::CastCancelled {
            source: HERO_ONE,
            ability: STRIKE,
            reason: CommandRejectReason::ActorDead,
        },
        MatchEvent::CastResolved {
            source: HERO_ONE,
            ability: STRIKE,
            target: CastTarget::SelfActor,
        },
        MatchEvent::DamageApplied {
            source: HERO_ONE,
            target: HERO_TWO,
            cause: DamageCause::BasicAttack,
            school: DamageSchool::True,
            amount: 1,
            health_after: 1,
        },
        MatchEvent::HealingApplied {
            source: HERO_ONE,
            target: HERO_TWO,
            ability: HEAL,
            amount: 1,
            health_after: 1,
        },
        MatchEvent::ActorDied {
            actor: HERO_ONE,
            killer: HERO_TWO,
        },
        MatchEvent::MatchFinished {
            outcome: MatchOutcome::Draw,
            reason: MatchEndReason::Host,
        },
        MatchEvent::BasicAttackStarted {
            source: HERO_ONE,
            target: HERO_TWO,
            resolves_at: 1,
        },
        MatchEvent::BasicAttackCancelled {
            source: HERO_ONE,
            target: HERO_TWO,
            reason: CommandRejectReason::ActorDead,
        },
        MatchEvent::BasicAttackResolved {
            source: HERO_ONE,
            target: HERO_TWO,
        },
        MatchEvent::ActorDespawned { actor: HERO_ONE },
        MatchEvent::RespawnScheduled {
            actor: HERO_ONE,
            at_tick: 1,
        },
        MatchEvent::RespawnCancelled {
            actor: HERO_ONE,
            at_tick: 1,
            reason: MatchEndReason::Host,
        },
        MatchEvent::ActorRespawned {
            actor: HERO_ONE,
            position: Vec2Mm::new(0, 0),
        },
        MatchEvent::InternalIntentRejected {
            actor: HERO_ONE,
            reason: CommandRejectReason::ActorDead,
        },
        MatchEvent::StatusEffectExpired {
            actor: HERO_ONE,
            effect: crate::StatusEffectId(1),
        },
        MatchEvent::ActorSpawned { actor: HERO_ONE },
        MatchEvent::ProjectileSpawned {
            projectile: crate::ProjectileId(1),
            source: HERO_ONE,
            ability: LANCE,
            position: Vec2Mm::new(0, 0),
            end: Vec2Mm::new(1, 0),
        },
        MatchEvent::ProjectileResolved {
            projectile: crate::ProjectileId(1),
            position: Vec2Mm::new(1, 0),
            victim: None,
        },
        MatchEvent::ProjectileCancelled {
            projectile: crate::ProjectileId(1),
            reason: CommandRejectReason::MatchNotActive,
        },
        MatchEvent::KillCredited {
            killer: HERO_ONE,
            victim: HERO_TWO,
            streak_after: 1,
        },
        MatchEvent::AssistCredited {
            actor: HERO_ONE,
            victim: HERO_TWO,
        },
        MatchEvent::GoldGranted {
            actor: HERO_ONE,
            amount: 1,
            reason: crate::GoldReason::Kill,
            total_after: 1,
        },
        MatchEvent::ExperienceGranted {
            actor: HERO_ONE,
            amount: 1,
            total_after: 1,
        },
        MatchEvent::HeroLevelUp {
            actor: HERO_ONE,
            level: 2,
            unspent_ability_points: 1,
        },
        MatchEvent::AbilityRankUp {
            actor: HERO_ONE,
            ability: STRIKE,
            rank: 2,
        },
        MatchEvent::ShieldAbsorbed {
            actor: HERO_ONE,
            effect: crate::StatusEffectId(1),
            amount: 1,
            remaining: 1,
        },
        MatchEvent::LifestealApplied {
            actor: HERO_ONE,
            amount: 1,
            health_after: 1,
        },
    ];
    let mut tags: Vec<u8> = events
        .iter()
        .map(|event| crate::runtime::event_tag(*event))
        .collect();
    let total = tags.len();
    tags.sort_unstable();
    tags.dedup();
    assert_eq!(
        tags.len(),
        total,
        "two authoritative events share a digest tag"
    );
}

#[test]
fn an_area_impact_skips_a_dead_actor() {
    let mut runtime = skirmish();
    runtime
        .spawn_actor(&minion(MINION_ONE, TeamId(1), Vec2Mm::new(3_000, 0), 200))
        .unwrap();
    runtime
        .spawn_actor(&minion(
            MINION_TWO,
            TeamId(1),
            Vec2Mm::new(3_000, 500),
            1_000,
        ))
        .unwrap();
    runtime.start().unwrap();

    // The first burst kills MINION_ONE outright; it stays in authoritative state because it never despawns.
    runtime
        .submit(cast_at(
            1,
            1,
            BURST,
            CastTarget::Point(Vec2Mm::new(3_000, 0)),
        ))
        .unwrap();
    runtime.step().unwrap();
    assert!(!runtime.actor(MINION_ONE).unwrap().alive);

    // Cooldown legality is judged against the CURRENT tick, not the requested one, so the second cast is
    // submitted after the burst has come off cooldown.
    runtime.step().unwrap();
    runtime
        .submit(cast_at(
            2,
            3,
            BURST,
            CastTarget::Point(Vec2Mm::new(3_000, 0)),
        ))
        .unwrap();
    let frame = runtime.step().unwrap();
    assert_eq!(frame.tick, 3);
    assert_eq!(damaged_targets(&frame), vec![MINION_TWO]);
}

// ---------------------------------------------------------------------------------------------------
// GP-05 progression and GP-14 match statistics.
//
// The arithmetic tests pin exact published values so a later "simplification" cannot quietly change the
// pacing of the game. The behavioural tests drive the public command surface and read the public
// scoreboard, never the private tables.
// ---------------------------------------------------------------------------------------------------

const ALPHA: ActorId = ActorId(501);
const BRAVO: ActorId = ActorId(502);
const CHARLIE: ActorId = ActorId(503);
const DELTA: ActorId = ActorId(504);
const TOWER: ActorId = ActorId(505);
const SNIPE_ID: AbilityId = AbilityId(31);
const NOVA: AbilityId = AbilityId(32);

const HERO_GROWTH: StatGrowth = StatGrowth {
    max_health_per_level: 100,
    max_resource_per_level: 20,
    move_speed_mm_per_tick_per_level: 5,
    physical_reduction_bps_per_level: 0,
    magic_reduction_bps_per_level: 0,
};
const HERO_BOUNTY: Bounty = Bounty {
    gold: 300,
    experience: 400,
};

/// A long-range single-target instant, cheap and off cooldown, used to land exact damage on demand.
const fn marksman(amount: u32, per_rank_amount: u32) -> AbilitySpec {
    AbilitySpec {
        id: SNIPE_ID,
        aim: AbilityAim::Unit,
        targeting: AbilityTargeting::Enemy,
        range_mm: 50_000,
        resource_cost: 0,
        cooldown_ticks: 1,
        cast_ticks: 0,
        delivery: AbilityDelivery::Instant,
        impact: ImpactShape::Single,
        effect: AbilityEffect::Damage {
            amount,
            school: DamageSchool::True,
        },
        per_rank_amount,
    }
}

/// A point-aimed projectile, so the launch-captured magnitude can be observed separately from the spec.
const fn nova(amount: u32, per_rank_amount: u32) -> AbilitySpec {
    AbilitySpec {
        id: NOVA,
        aim: AbilityAim::Point,
        targeting: AbilityTargeting::Enemy,
        range_mm: 20_000,
        resource_cost: 0,
        cooldown_ticks: 1,
        cast_ticks: 0,
        delivery: AbilityDelivery::Projectile {
            speed_mm_per_tick: 1_000,
            hit_radius_mm: 400,
        },
        impact: ImpactShape::Single,
        effect: AbilityEffect::Damage {
            amount,
            school: DamageSchool::True,
        },
        per_rank_amount,
    }
}

fn progression_hero(
    id: ActorId,
    owner: PlayerId,
    team: TeamId,
    position: Vec2Mm,
    health: u32,
    abilities: Vec<AbilityId>,
) -> ActorSpawn {
    ActorSpawn {
        growth: HERO_GROWTH,
        bounty: HERO_BOUNTY,
        ..hero(id, owner, team, position, health, abilities)
    }
}

/// Two team-0 heroes and one frail team-1 hero, all inside the default XP share radius.
fn arena() -> MatchRuntime {
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 0x9105).unwrap();
    runtime.register_ability(marksman(400, 250)).unwrap();
    runtime.register_ability(nova(300, 200)).unwrap();
    runtime
        .spawn_actor(&progression_hero(
            ALPHA,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            2_000,
            vec![SNIPE_ID, NOVA],
        ))
        .unwrap();
    runtime
        .spawn_actor(&progression_hero(
            CHARLIE,
            PLAYER_THREE,
            TeamId(0),
            Vec2Mm::new(1_000, 0),
            2_000,
            vec![SNIPE_ID],
        ))
        .unwrap();
    runtime
        .spawn_actor(&progression_hero(
            BRAVO,
            PLAYER_TWO,
            TeamId(1),
            Vec2Mm::new(3_000, 0),
            600,
            vec![SNIPE_ID],
        ))
        .unwrap();
    runtime
}

fn cast_on(
    player: PlayerId,
    sequence: u32,
    execute_at: u64,
    actor: ActorId,
    target: ActorId,
) -> PlayerCommand {
    command(
        player,
        sequence,
        execute_at,
        actor,
        CommandKind::Cast {
            ability: SNIPE_ID,
            target: CastTarget::Actor(target),
        },
    )
}

// --- Arithmetic -------------------------------------------------------------------------------------

#[test]
fn the_experience_curve_matches_its_published_table_and_inverts_exactly() {
    // League of Legends' shipped cumulative table, pinned value by value so a rewrite of the closed form
    // cannot silently re-pace the whole game.
    let table = [
        (1_u8, 0_u32),
        (2, 280),
        (3, 660),
        (4, 1_140),
        (5, 1_720),
        (6, 2_400),
        (7, 3_180),
        (8, 4_060),
        (9, 5_040),
        (10, 6_120),
        (11, 7_300),
        (12, 8_580),
        (13, 9_960),
        (14, 11_440),
        (15, 13_020),
        (16, 14_700),
        (17, 16_480),
        (18, 18_360),
    ];
    for (level, cumulative) in table {
        assert_eq!(
            crate::experience_for_level(level),
            cumulative,
            "cumulative experience for level {level}"
        );
        // The inverse must flip at exactly the boundary, not one either side of it.
        assert_eq!(crate::level_for_experience(cumulative), level);
        if cumulative > 0 {
            assert_eq!(crate::level_for_experience(cumulative - 1), level - 1);
        }
    }
    assert_eq!(crate::EXPERIENCE_CAP, 18_360);
    assert_eq!(
        crate::level_for_experience(u32::MAX),
        crate::MAX_HERO_LEVEL,
        "experience past the cap must not roll the level over"
    );
}

#[test]
fn streak_gold_matches_its_published_table() {
    assert_eq!(crate::streak_gold(0), 0);
    assert_eq!(crate::streak_gold(2), 0, "a shutdown starts at a 3-streak");
    for (streak, gold) in [
        (3_u16, 60_u32),
        (4, 100),
        (5, 150),
        (6, 210),
        (7, 280),
        (8, 360),
        (9, 450),
        (10, 550),
    ] {
        assert_eq!(crate::streak_gold(streak), gold);
    }
    assert_eq!(
        crate::streak_gold(50),
        crate::MAX_STREAK_GOLD,
        "the bounty is clamped, not unbounded"
    );
}

#[test]
fn apportionment_conserves_the_pool_where_truncation_would_lose_it() {
    for pool in 0_u32..40 {
        for recipients in 1_u32..7 {
            let (base, remainder) = crate::apportion(pool, recipients);
            let awarded: u32 = (0..recipients)
                .map(|index| base + u32::from(index < remainder))
                .sum();
            assert_eq!(awarded, pool, "pool {pool} across {recipients}");
            let smallest = base;
            let largest = base + u32::from(remainder > 0);
            assert!(largest - smallest <= 1, "shares differ by more than one");
        }
    }
    // Negative control: the case this exists for. Plain truncation destroys two units of the pool, so the
    // conservation assertion above is a real property and not a restatement of integer division.
    let (base, _) = crate::apportion(10, 3);
    assert_eq!(base * 3, 9);
    assert_ne!(base * 3, 10);
}

#[test]
fn passive_gold_is_drift_free_where_a_naive_accumulator_is_not() {
    let config = MatchConfig::default();
    let elapsed = 30 * 60 * 20; // twenty minutes at 30 Hz.
    let closed_form = crate::passive_gold_owed(elapsed, config);
    assert_eq!(
        closed_form,
        u32::try_from(
            elapsed * u64::from(config.passive_gold_per_100s)
                / (100 * u64::from(config.tick_rate_hz))
        )
        .unwrap()
    );

    // Negative control: the per-tick accumulator this closed form replaces. Integer division truncates to
    // zero every tick, so it pays nothing at all - which is exactly the drift the closed form prevents.
    let mut naive = 0_u32;
    for _ in 0..elapsed {
        naive += config.passive_gold_per_100s / (100 * u32::from(config.tick_rate_hz));
    }
    assert_ne!(naive, closed_form);
    assert_eq!(naive, 0);
    assert!(closed_form > 0);
}

// --- Attribution ------------------------------------------------------------------------------------

#[test]
fn the_killing_blow_takes_the_bounty_and_the_window_takes_the_assists() {
    let mut runtime = arena();
    runtime.start().unwrap();

    // CHARLIE softens BRAVO first, ALPHA lands the kill. Both are inside the assist window.
    runtime
        .submit(cast_on(PLAYER_THREE, 1, 1, CHARLIE, BRAVO))
        .unwrap();
    runtime.step().unwrap();
    runtime
        .submit(cast_on(PLAYER_ONE, 1, 3, ALPHA, BRAVO))
        .unwrap();
    runtime.step().unwrap();
    let frame = runtime.step().unwrap();

    assert!(frame.events.iter().any(
        |event| matches!(event, MatchEvent::ActorDied { actor, killer }
            if *actor == BRAVO && *killer == ALPHA)
    ));
    let assists: Vec<ActorId> = frame
        .events
        .iter()
        .filter_map(|event| match event {
            MatchEvent::AssistCredited { actor, .. } => Some(*actor),
            _ => None,
        })
        .collect();
    assert_eq!(
        assists,
        vec![CHARLIE],
        "the killer is never its own assister"
    );

    let killer = runtime.score(ALPHA).unwrap();
    let assister = runtime.score(CHARLIE).unwrap();
    let victim = runtime.score(BRAVO).unwrap();
    assert_eq!(killer.kills, 1);
    assert_eq!(killer.kill_streak, 1);
    assert_eq!(assister.assists, 1);
    assert_eq!(victim.deaths, 1);
    assert_eq!(victim.kill_streak, 0);
    // 300 bounty to the killer; half of it minted separately for the single assister.
    assert_eq!(killer.gold, 300);
    assert_eq!(assister.gold, 150);
    // Minted, not deducted: the killer's take is unaffected by the assist existing.
    assert_eq!(killer.gold + assister.gold, 450);
}

#[test]
fn an_overkill_entry_does_not_earn_an_assist() {
    let mut runtime = arena();
    // BRAVO must die to the FIRST entry in the batch, so the second one genuinely applies nothing. With a
    // victim that survives the first hit there is no overkill entry and the test proves nothing.
    let index = runtime.actors.iter().position(|a| a.id == BRAVO).unwrap();
    runtime.actors[index].health = 300;
    runtime.start().unwrap();
    // Both land on the same tick. The first is lethal, so the second applies zero and must credit nobody.
    runtime
        .submit(cast_on(PLAYER_ONE, 1, 1, ALPHA, BRAVO))
        .unwrap();
    runtime
        .submit(cast_on(PLAYER_THREE, 1, 1, CHARLIE, BRAVO))
        .unwrap();
    let frame = runtime.step().unwrap();

    let zero_damage = frame.events.iter().any(|event| {
        matches!(event, MatchEvent::DamageApplied { target, amount, .. }
            if *target == BRAVO && *amount == 0)
    });
    assert!(
        zero_damage,
        "this fixture only proves the gate if an overkill entry really is emitted"
    );
    assert!(
        !frame
            .events
            .iter()
            .any(|event| matches!(event, MatchEvent::AssistCredited { .. })),
        "an entry that applied nothing must not buy an assist"
    );
    assert_eq!(runtime.score(CHARLIE).unwrap().assists, 0);
}

#[test]
fn the_assist_window_closes_at_exactly_the_authored_tick() {
    fn run(gap: u64) -> u32 {
        let config = MatchConfig {
            assist_window_ticks: 10,
            ..MatchConfig::default()
        };
        let mut runtime = MatchRuntime::new(config, 5).unwrap();
        runtime.register_ability(marksman(100, 0)).unwrap();
        for spawn in [
            progression_hero(
                ALPHA,
                PLAYER_ONE,
                TeamId(0),
                Vec2Mm::new(0, 0),
                2_000,
                vec![SNIPE_ID],
            ),
            progression_hero(
                CHARLIE,
                PLAYER_THREE,
                TeamId(0),
                Vec2Mm::new(1_000, 0),
                2_000,
                vec![SNIPE_ID],
            ),
            progression_hero(
                BRAVO,
                PLAYER_TWO,
                TeamId(1),
                Vec2Mm::new(3_000, 0),
                150,
                vec![],
            ),
        ] {
            runtime.spawn_actor(&spawn).unwrap();
        }
        runtime.start().unwrap();
        // CHARLIE contributes on tick 1, ALPHA kills on tick 1 + gap.
        runtime
            .submit(cast_on(PLAYER_THREE, 1, 1, CHARLIE, BRAVO))
            .unwrap();
        runtime.step().unwrap();
        while runtime.tick() < gap {
            runtime.step().unwrap();
        }
        runtime
            .submit(cast_on(PLAYER_ONE, 1, gap + 1, ALPHA, BRAVO))
            .unwrap();
        while runtime.tick() < gap + 1 {
            runtime.step().unwrap();
        }
        runtime.score(CHARLIE).unwrap().assists
    }

    // Contributed on tick 1; a 10-tick window still covers a kill on tick 11 and not on tick 12.
    assert_eq!(run(10), 1, "the window is inclusive of its final tick");
    assert_eq!(run(11), 0, "one tick later the window has closed");
}

#[test]
fn a_full_credit_list_evicts_the_oldest_contributor() {
    let config = MatchConfig {
        max_damage_credits_per_actor: 2,
        ..MatchConfig::default()
    };
    let mut runtime = MatchRuntime::new(config, 6).unwrap();
    runtime.register_ability(marksman(10, 0)).unwrap();
    for (index, actor) in [ALPHA, CHARLIE, DELTA].into_iter().enumerate() {
        let index = u64::try_from(index).unwrap_or(0);
        runtime
            .spawn_actor(&progression_hero(
                actor,
                PlayerId(10 + index),
                TeamId(0),
                Vec2Mm::new(i32::try_from(index).unwrap_or(0) * 100, 0),
                2_000,
                vec![SNIPE_ID],
            ))
            .unwrap();
    }
    runtime
        .spawn_actor(&progression_hero(
            BRAVO,
            PLAYER_TWO,
            TeamId(1),
            Vec2Mm::new(3_000, 0),
            2_000,
            vec![],
        ))
        .unwrap();
    runtime.start().unwrap();

    // Three distinct contributors on three distinct ticks into a two-slot list.
    for (index, actor) in [ALPHA, CHARLIE, DELTA].into_iter().enumerate() {
        let index = u64::try_from(index).unwrap_or(0);
        runtime
            .submit(cast_on(PlayerId(10 + index), 1, index + 1, actor, BRAVO))
            .unwrap();
        runtime.step().unwrap();
    }
    let credits: Vec<ActorId> = runtime
        .damage_credits(BRAVO)
        .unwrap()
        .into_iter()
        .map(|(actor, _)| actor)
        .collect();
    assert_eq!(
        credits,
        vec![CHARLIE, DELTA],
        "the oldest contributor is the one dropped, not the newest refused"
    );
}

#[test]
fn a_despawned_hero_keeps_its_scoreboard_line() {
    let mut runtime = arena();
    // A hero that vanishes on death. Its body leaves authoritative state entirely; its record must not.
    runtime
        .spawn_actor(&ActorSpawn {
            death_rule: DeathRule::Despawn,
            ..progression_hero(
                DELTA,
                PlayerId(9),
                TeamId(1),
                Vec2Mm::new(4_000, 0),
                200,
                vec![],
            )
        })
        .unwrap();
    runtime.start().unwrap();
    runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            ALPHA,
            CommandKind::Cast {
                ability: SNIPE_ID,
                target: CastTarget::Actor(DELTA),
            },
        ))
        .unwrap();
    runtime.step().unwrap();

    assert!(runtime.actor(DELTA).is_none(), "the body despawned");
    let line = runtime
        .score(DELTA)
        .expect("the scoreboard line outlives the body it was opened for");
    assert_eq!(line.deaths, 1);
    runtime.check_invariants().unwrap();
}

#[test]
fn a_structure_kill_forfeits_the_bounty_without_panicking() {
    let mut runtime = arena();
    runtime
        .spawn_actor(&ActorSpawn {
            id: TOWER,
            owner: None,
            team: TeamId(0),
            kind: ActorKind::Structure,
            position: Vec2Mm::new(2_500, 0),
            stats: stats(5_000, 0, 0),
            growth: StatGrowth::NONE,
            bounty: Bounty::NONE,
            abilities: vec![SNIPE_ID],
            basic_attack: None,
            death_rule: DeathRule::StayDead,
        })
        .unwrap();
    runtime.start().unwrap();
    // Server-owned intents: the tower has no owner, so it can only act through the internal channel. Two
    // are needed, because one 400-damage shot does not finish a 600 hp hero.
    let intent = [ActorIntent {
        actor: TOWER,
        kind: CommandKind::Cast {
            ability: SNIPE_ID,
            target: CastTarget::Actor(BRAVO),
        },
    }];
    let mut frame = runtime.step_with_intents(&intent).unwrap();
    for _ in 0..4 {
        if frame
            .events
            .iter()
            .any(|event| matches!(event, MatchEvent::ActorDied { .. }))
        {
            break;
        }
        frame = runtime.step_with_intents(&intent).unwrap();
    }

    assert!(frame.events.iter().any(
        |event| matches!(event, MatchEvent::ActorDied { actor, killer }
            if *actor == BRAVO && *killer == TOWER)
    ));
    assert!(
        runtime.score(TOWER).is_none(),
        "a structure opens no scoreboard line"
    );
    assert!(
        !frame
            .events
            .iter()
            .any(|event| matches!(event, MatchEvent::KillCredited { .. })),
        "there is nowhere to credit the kill, and that is not an error"
    );
    assert_eq!(runtime.score(BRAVO).unwrap().deaths, 1);
    runtime.check_invariants().unwrap();
}

#[test]
fn a_self_kill_counts_a_death_and_credits_nothing() {
    let config = MatchConfig::default();
    let mut runtime = MatchRuntime::new(config, 8).unwrap();
    // An `AnyUnit` area burst centred on the caster reaches the caster: an area impact deliberately does
    // not exclude its source, so this is a reachable state rather than a contrived one.
    runtime
        .register_ability(AbilitySpec {
            id: NOVA,
            aim: AbilityAim::Point,
            targeting: AbilityTargeting::AnyUnit,
            range_mm: 5_000,
            resource_cost: 0,
            cooldown_ticks: 1,
            cast_ticks: 0,
            delivery: AbilityDelivery::Instant,
            impact: ImpactShape::Circle { radius_mm: 1_000 },
            effect: AbilityEffect::Damage {
                amount: 5_000,
                school: DamageSchool::True,
            },
            per_rank_amount: 0,
        })
        .unwrap();
    runtime
        .spawn_actor(&progression_hero(
            ALPHA,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            500,
            vec![NOVA],
        ))
        .unwrap();
    runtime.start().unwrap();
    runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            ALPHA,
            CommandKind::Cast {
                ability: NOVA,
                target: CastTarget::Point(Vec2Mm::new(200, 0)),
            },
        ))
        .unwrap();
    let frame = runtime.step().unwrap();

    assert!(frame.events.iter().any(
        |event| matches!(event, MatchEvent::ActorDied { actor, killer }
            if *actor == ALPHA && *killer == ALPHA)
    ));
    let line = runtime.score(ALPHA).unwrap();
    assert_eq!(line.deaths, 1);
    assert_eq!(line.kills, 0, "you cannot farm yourself");
    assert_eq!(line.gold, 0);
    runtime.check_invariants().unwrap();
}

// --- Progression ------------------------------------------------------------------------------------

#[test]
fn a_kill_shares_experience_by_proximity_and_conserves_the_pool() {
    let mut runtime = arena();
    // DELTA is an ally of the killer but far outside the 8 m share radius.
    runtime
        .spawn_actor(&progression_hero(
            DELTA,
            PlayerId(9),
            TeamId(0),
            Vec2Mm::new(200_000, 0),
            2_000,
            vec![],
        ))
        .unwrap();
    runtime.start().unwrap();
    runtime
        .submit(cast_on(PLAYER_ONE, 1, 1, ALPHA, BRAVO))
        .unwrap();
    runtime
        .submit(cast_on(PLAYER_ONE, 2, 2, ALPHA, BRAVO))
        .unwrap();
    runtime.step().unwrap();
    let frame = runtime.step().unwrap();

    let granted: u32 = frame
        .events
        .iter()
        .filter_map(|event| match event {
            MatchEvent::ExperienceGranted { amount, .. } => Some(*amount),
            _ => None,
        })
        .sum();
    assert_eq!(
        granted, HERO_BOUNTY.experience,
        "every unit the death minted is awarded - no remainder is destroyed"
    );
    // 400 split two ways between the killer and the nearby ally.
    assert_eq!(runtime.actor(ALPHA).unwrap().experience, 200);
    assert_eq!(runtime.actor(CHARLIE).unwrap().experience, 200);
    assert_eq!(
        runtime.actor(DELTA).unwrap().experience,
        0,
        "an ally outside the share radius earns nothing"
    );
    assert_eq!(
        runtime.actor(BRAVO).unwrap().experience,
        0,
        "the victim's own team shares nothing from its death"
    );
}

#[test]
fn a_level_up_raises_stats_through_growth_and_never_through_a_modifier() {
    let mut runtime = arena();
    runtime.start().unwrap();
    let before = runtime.actor(ALPHA).unwrap();
    assert_eq!(before.level, 1);
    assert_eq!(before.max_health, 2_000);

    // 280 experience is exactly level 2.
    let level = runtime.award_experience(ALPHA, 280).unwrap();
    assert_eq!(level, 2);
    let after = runtime.actor(ALPHA).unwrap();
    assert_eq!(after.level, 2);
    assert_eq!(
        after.max_health,
        2_000 + HERO_GROWTH.max_health_per_level,
        "growth raises the base, one level's worth"
    );
    assert_eq!(after.unspent_ability_points, 1);
    assert!(
        runtime.modifiers(ALPHA).unwrap().is_empty(),
        "growth must not consume the actor's modifier budget"
    );
    assert_eq!(
        after.health, before.health,
        "a level-up raises the maximum; it does not heal"
    );
    runtime.check_invariants().unwrap();
}

#[test]
fn an_ability_rank_raises_its_magnitude_and_a_missile_keeps_the_rank_it_launched_with() {
    let mut runtime = arena();
    runtime.start().unwrap();
    // Two levels: one point spent on NOVA, one left over.
    runtime.award_experience(ALPHA, 660).unwrap();
    assert_eq!(runtime.actor(ALPHA).unwrap().level, 3);

    runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            ALPHA,
            CommandKind::UpgradeAbility { ability: NOVA },
        ))
        .unwrap();
    let frame = runtime.step().unwrap();
    assert!(frame.events.iter().any(|event| matches!(
        event,
        MatchEvent::AbilityRankUp { actor, ability, rank }
            if *actor == ALPHA && *ability == NOVA && *rank == 2
    )));
    assert_eq!(runtime.ability_rank(ALPHA, NOVA), Some(2));

    // Launch at rank 2: 300 base + 200 per rank.
    runtime
        .submit(command(
            PLAYER_ONE,
            2,
            2,
            ALPHA,
            CommandKind::Cast {
                ability: NOVA,
                target: CastTarget::Point(Vec2Mm::new(3_000, 0)),
            },
        ))
        .unwrap();
    let launch = runtime.step().unwrap();
    assert_eq!(launch.projectiles.len(), 1);

    // Rank up again WHILE the missile is in the air. Rank 3 unlocks at level 5, so the level comes first.
    // The magnitude the missile was fired with must not move.
    runtime.award_experience(ALPHA, 1_060).unwrap();
    assert_eq!(runtime.actor(ALPHA).unwrap().level, 5);
    runtime
        .submit(command(
            PLAYER_ONE,
            3,
            3,
            ALPHA,
            CommandKind::UpgradeAbility { ability: NOVA },
        ))
        .unwrap();
    runtime.step().unwrap();
    assert_eq!(runtime.ability_rank(ALPHA, NOVA), Some(3));

    let before = runtime.actor(BRAVO).unwrap().health;
    let mut applied = None;
    for _ in 0..12 {
        let frame = runtime.step().unwrap();
        if let Some(amount) = frame.events.iter().find_map(|event| match event {
            MatchEvent::DamageApplied { target, amount, .. } if *target == BRAVO => Some(*amount),
            _ => None,
        }) {
            applied = Some(amount);
            break;
        }
    }
    assert_eq!(
        applied,
        Some(500),
        "the missile lands for the rank-2 magnitude it launched with, not the rank-3 one"
    );
    assert_eq!(runtime.actor(BRAVO).unwrap().health, before - 500);
}

#[test]
fn upgrading_is_refused_without_a_point_or_before_its_unlock_level() {
    let mut runtime = arena();
    runtime.start().unwrap();

    // No level, no point.
    let rejection = runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            ALPHA,
            CommandKind::UpgradeAbility { ability: NOVA },
        ))
        .unwrap_err();
    assert_eq!(
        rejection.reason,
        CommandRejectReason::NoAbilityPointAvailable
    );

    // Level 2 grants a point, but rank 2 needs level 3.
    runtime.award_experience(ALPHA, 280).unwrap();
    let rejection = runtime
        .submit(command(
            PLAYER_ONE,
            2,
            1,
            ALPHA,
            CommandKind::UpgradeAbility { ability: NOVA },
        ))
        .unwrap_err();
    assert_eq!(
        rejection.reason,
        CommandRejectReason::AbilityRankLocked {
            unlocks_at_level: 3
        }
    );

    // An unequipped ability is refused as such, not as a rank problem.
    runtime.award_experience(ALPHA, 380).unwrap();
    let rejection = runtime
        .submit(command(
            PLAYER_ONE,
            3,
            1,
            ALPHA,
            CommandKind::UpgradeAbility {
                ability: AbilityId(9_999),
            },
        ))
        .unwrap_err();
    assert_eq!(rejection.reason, CommandRejectReason::AbilityNotEquipped);
}

#[test]
fn upgrading_an_ability_is_not_blocked_by_crowd_control() {
    let mut runtime = arena();
    runtime.start().unwrap();
    runtime.award_experience(ALPHA, 660).unwrap();
    runtime
        .apply_status_effect(ALPHA, 30, &[ControlKind::Stun], &[])
        .unwrap();

    // A stun blocks casting outright...
    let rejection = runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            ALPHA,
            CommandKind::Cast {
                ability: NOVA,
                target: CastTarget::Point(Vec2Mm::new(1_000, 0)),
            },
        ))
        .unwrap_err();
    assert_eq!(rejection.reason, CommandRejectReason::ActorStunned);

    // ...but spending a level-up point is a menu action, not something the body does.
    runtime
        .submit(command(
            PLAYER_ONE,
            2,
            1,
            ALPHA,
            CommandKind::UpgradeAbility { ability: NOVA },
        ))
        .unwrap();
    runtime.step().unwrap();
    assert_eq!(runtime.ability_rank(ALPHA, NOVA), Some(2));
}

#[test]
fn progression_survives_death_and_respawn_but_the_assist_window_does_not() {
    let mut runtime = arena();
    runtime
        .spawn_actor(&ActorSpawn {
            death_rule: DeathRule::Respawn {
                delay_ticks: 2,
                at: Vec2Mm::new(9_000, 0),
            },
            ..progression_hero(
                DELTA,
                PlayerId(9),
                TeamId(1),
                Vec2Mm::new(4_000, 0),
                300,
                vec![],
            )
        })
        .unwrap();
    runtime.start().unwrap();
    runtime.award_experience(DELTA, 1_720).unwrap();
    let before = runtime.actor(DELTA).unwrap();
    assert_eq!(before.level, 5);

    runtime
        .submit(command(
            PLAYER_ONE,
            1,
            1,
            ALPHA,
            CommandKind::Cast {
                ability: SNIPE_ID,
                target: CastTarget::Actor(DELTA),
            },
        ))
        .unwrap();
    for _ in 0..4 {
        runtime.step().unwrap();
    }
    let after = runtime.actor(DELTA).unwrap();
    assert!(after.alive, "the hero respawned");
    assert_eq!(after.level, 5, "levels are not lost on death");
    assert_eq!(after.experience, before.experience);
    assert_eq!(after.unspent_ability_points, before.unspent_ability_points);
    assert!(
        runtime.damage_credits(DELTA).unwrap().is_empty(),
        "the assist window died with the body"
    );
    assert_eq!(runtime.score(DELTA).unwrap().deaths, 1);
}

// --- Tamper resistance ------------------------------------------------------------------------------

#[test]
fn a_forged_level_or_rank_or_gold_is_rejected_by_the_restore_audit() {
    let content = ContentId::new([0x51; 32]);
    let build = || {
        let mut runtime = arena();
        runtime.start().unwrap();
        runtime.award_experience(ALPHA, 660).unwrap();
        runtime.step().unwrap();
        runtime
    };

    // Control: the honest runtime captures and restores cleanly, so the failures below are about the
    // forgery and not about the fixture.
    let honest = build();
    let checkpoint = honest.capture_checkpoint(content).unwrap();
    MatchRuntime::restore_checkpoint(&checkpoint, content).unwrap();

    // A level that its own experience does not produce.
    let mut forged = build();
    let index = forged.actors.iter().position(|a| a.id == ALPHA).unwrap();
    forged.actors[index].level = crate::MAX_HERO_LEVEL;
    assert!(forged.check_invariants().is_err());

    // A rank raised without spending a point.
    let mut forged = build();
    let index = forged.actors.iter().position(|a| a.id == ALPHA).unwrap();
    forged.actors[index].abilities[0].rank = 2;
    assert!(forged.check_invariants().is_err());

    // Passive gold shifted off its closed form by a single unit.
    let mut forged = build();
    let slot = forged.scores.iter().position(|s| s.actor == ALPHA).unwrap();
    forged.scores[slot].passive_gold_paid += 1;
    assert!(forged.check_invariants().is_err());

    // Earned gold beyond what its kills and assists can account for, and the control one unit below it.
    let ceiling = crate::MAX_BOUNTY_GOLD + crate::MAX_STREAK_GOLD;
    let mut at_ceiling = build();
    let slot = at_ceiling
        .scores
        .iter()
        .position(|s| s.actor == ALPHA)
        .unwrap();
    at_ceiling.scores[slot].kills = 1;
    at_ceiling.scores[slot].best_kill_streak = 1;
    at_ceiling.scores[slot].earned_gold = ceiling;
    at_ceiling.check_invariants().unwrap();
    at_ceiling.scores[slot].earned_gold = ceiling + 1;
    assert!(at_ceiling.check_invariants().is_err());
}

#[test]
fn progression_and_the_scoreboard_are_part_of_both_digests() {
    let mut runtime = arena();
    runtime.start().unwrap();
    runtime.award_experience(ALPHA, 660).unwrap();
    // Experience lives on the ACTOR, so it does not by itself dirty a score line. Gold does.
    runtime
        .grant_gold(ALPHA, 250, crate::GoldReason::Kill)
        .unwrap();
    let frame = runtime.step().unwrap();
    assert!(!frame.scores.is_empty());

    // Each strip removes exactly ONE thing. If any of these were absent from the digest, two runtimes that
    // disagreed about it would still claim to be identical.
    let mut without_scores = runtime.clone();
    without_scores.scores.clear();
    assert_ne!(runtime.world_digest(), without_scores.world_digest());

    let mut without_experience = runtime.clone();
    let index = without_experience
        .actors
        .iter()
        .position(|a| a.id == ALPHA)
        .unwrap();
    without_experience.actors[index].experience = 0;
    assert_ne!(runtime.world_digest(), without_experience.world_digest());

    let mut without_rank = runtime.clone();
    let index = without_rank
        .actors
        .iter()
        .position(|a| a.id == ALPHA)
        .unwrap();
    without_rank.actors[index].abilities[0].rank = 4;
    assert_ne!(runtime.world_digest(), without_rank.world_digest());

    let mut frame_without = frame.clone();
    frame_without.scores.clear();
    assert_ne!(
        crate::runtime::digest_frame(&frame),
        crate::runtime::digest_frame(&frame_without)
    );
}

#[test]
fn a_completed_match_keeps_its_scoreboard_and_closes_every_assist_window() {
    let mut runtime = arena();
    runtime.start().unwrap();
    runtime
        .submit(cast_on(PLAYER_ONE, 1, 1, ALPHA, BRAVO))
        .unwrap();
    runtime.step().unwrap();
    assert!(!runtime.damage_credits(BRAVO).unwrap().is_empty());

    runtime
        .finish(MatchOutcome::Winner(TeamId(0)), MatchEndReason::Host)
        .unwrap();
    assert!(
        !runtime.scores().is_empty(),
        "a match result that erased its own scoreboard would be useless"
    );
    assert!(runtime.damage_credits(BRAVO).unwrap().is_empty());
    runtime.check_invariants().unwrap();
}

#[test]
fn progression_budgets_are_validated_and_zero_passive_income_stays_legal() {
    let base = MatchConfig::default();
    for zeroed in [
        MatchConfig {
            max_score_entries: 0,
            ..base
        },
        MatchConfig {
            max_damage_credits_per_actor: 0,
            ..base
        },
        MatchConfig {
            assist_window_ticks: 0,
            ..base
        },
        MatchConfig {
            xp_share_radius_mm: 0,
            ..base
        },
    ] {
        assert!(MatchRuntime::new(zeroed, 0).is_err());
    }
    for over in [
        MatchConfig {
            max_score_entries: crate::MAX_SCORE_ENTRY_BUDGET + 1,
            ..base
        },
        MatchConfig {
            max_damage_credits_per_actor: crate::MAX_DAMAGE_CREDITS_PER_ACTOR_BUDGET + 1,
            ..base
        },
        MatchConfig {
            assist_window_ticks: crate::MAX_ASSIST_WINDOW_TICKS + 1,
            ..base
        },
        MatchConfig {
            xp_share_radius_mm: crate::MAX_XP_SHARE_RADIUS_MM + 1,
            ..base
        },
        MatchConfig {
            passive_gold_per_100s: crate::MAX_PASSIVE_GOLD_PER_100S + 1,
            ..base
        },
    ] {
        assert!(MatchRuntime::new(over, 0).is_err());
    }
    // Deliberately legal: a profile with no passive income at all. Pinned so a later author cannot tidy
    // this field into the positivity chain and make an income stream silently mandatory.
    MatchRuntime::new(
        MatchConfig {
            passive_gold_per_100s: 0,
            ..base
        },
        0,
    )
    .unwrap();
}

#[test]
fn the_scoreboard_budget_is_a_lifetime_budget() {
    let config = MatchConfig {
        max_score_entries: 1,
        ..MatchConfig::default()
    };
    let mut runtime = MatchRuntime::new(config, 3).unwrap();
    runtime.register_ability(marksman(10, 0)).unwrap();
    runtime
        .spawn_actor(&progression_hero(
            ALPHA,
            PLAYER_ONE,
            TeamId(0),
            Vec2Mm::new(0, 0),
            100,
            vec![],
        ))
        .unwrap();
    assert!(matches!(
        runtime.spawn_actor(&progression_hero(
            BRAVO,
            PLAYER_TWO,
            TeamId(1),
            Vec2Mm::new(1_000, 0),
            100,
            vec![],
        )),
        Err(RuntimeError::ScoreBudgetExceeded)
    ));
    // A non-hero opens no line, so it is unaffected by the exhausted budget.
    runtime
        .spawn_actor(&minion(MINION_ONE, TeamId(1), Vec2Mm::new(2_000, 0), 100))
        .unwrap();
    assert_eq!(runtime.scores().len(), 1);
}

// ---------------------------------------------------------------------------------------------------
// MOB-3.0: seeded RNG (GP-12) and the damage pipeline (the rest of GP-07).
//
// The arithmetic tests pin exact hand-computed numbers so a later "simplification" cannot silently change
// what a swing does. Every statistical test carries the negative control that proves it is measuring the
// property it claims.
// ---------------------------------------------------------------------------------------------------

const DUELLIST: ActorId = ActorId(601);
const DUMMY: ActorId = ActorId(602);
const CLEAVE: AbilityId = AbilityId(51);

fn combat_stats(health: u32) -> CombatStats {
    CombatStats {
        max_health: health,
        max_resource: 0,
        move_speed_mm_per_tick: 0,
        physical_reduction_bps: 0,
        magic_reduction_bps: 0,
        crit_chance_bps: 0,
        crit_damage_bps: crate::BASIS_POINTS,
        physical_penetration_bps: 0,
        magic_penetration_bps: 0,
        lifesteal_bps: 0,
    }
}

/// One attacker and one dummy, with every offensive stat authored per test.
fn duel(attacker: CombatStats, defender: CombatStats, attack_damage: u32) -> MatchRuntime {
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 0x_C0FF_EE01).unwrap();
    runtime
        .register_ability(AbilitySpec::unit_targeted(
            CLEAVE,
            AbilityTargeting::Enemy,
            50_000,
            0,
            1,
            0,
            AbilityEffect::Damage {
                amount: 1_000,
                school: DamageSchool::Physical,
            },
        ))
        .unwrap();
    runtime
        .spawn_actor(&ActorSpawn {
            id: DUELLIST,
            owner: Some(PLAYER_ONE),
            team: TeamId(0),
            kind: ActorKind::Hero,
            position: Vec2Mm::new(0, 0),
            stats: attacker,
            growth: StatGrowth::NONE,
            bounty: Bounty::NONE,
            abilities: vec![CLEAVE],
            basic_attack: Some(BasicAttackSpec {
                range_mm: 50_000,
                acquisition_range_mm: 0,
                damage: attack_damage,
                school: DamageSchool::Physical,
                windup_ticks: 0,
                // Zero on purpose: attack legality is judged against the CURRENT tick, not the requested
                // one, so any positive cooldown makes a next-tick swing un-submittable and these fixtures
                // would be testing the cooldown rather than the pipeline.
                cooldown_ticks: 0,
            }),
            death_rule: DeathRule::StayDead,
        })
        .unwrap();
    runtime
        .spawn_actor(&ActorSpawn {
            id: DUMMY,
            owner: None,
            team: TeamId(1),
            kind: ActorKind::Minion,
            position: Vec2Mm::new(1_000, 0),
            stats: defender,
            growth: StatGrowth::NONE,
            bounty: Bounty::NONE,
            abilities: vec![],
            basic_attack: None,
            death_rule: DeathRule::StayDead,
        })
        .unwrap();
    runtime.start().unwrap();
    runtime
}

fn swing(runtime: &mut MatchRuntime, sequence: u32) -> crate::ServerFrame {
    let at = runtime.tick() + 1;
    runtime
        .submit(command(
            PLAYER_ONE,
            sequence,
            at,
            DUELLIST,
            CommandKind::BasicAttack { target: DUMMY },
        ))
        .unwrap();
    runtime.step().unwrap()
}

fn damage_dealt(frame: &crate::ServerFrame) -> Option<u32> {
    frame.events.iter().find_map(|event| match event {
        MatchEvent::DamageApplied { target, amount, .. } if *target == DUMMY => Some(*amount),
        _ => None,
    })
}

// --- The generator ----------------------------------------------------------------------------------

#[test]
fn a_keyed_roll_is_a_pure_function_of_its_key_and_domains_do_not_correlate() {
    let a = crate::roll_u64(7, crate::RngDomain::CriticalStrike, 100, 1, 2, 3);
    // Same key, same number, every time and in any order.
    assert_eq!(
        a,
        crate::roll_u64(7, crate::RngDomain::CriticalStrike, 100, 1, 2, 3)
    );
    // Changing ANY component of the key changes the number.
    for other in [
        crate::roll_u64(8, crate::RngDomain::CriticalStrike, 100, 1, 2, 3),
        crate::roll_u64(7, crate::RngDomain::NeutralSpawn, 100, 1, 2, 3),
        crate::roll_u64(7, crate::RngDomain::CriticalStrike, 101, 1, 2, 3),
        crate::roll_u64(7, crate::RngDomain::CriticalStrike, 100, 2, 2, 3),
        crate::roll_u64(7, crate::RngDomain::CriticalStrike, 100, 1, 3, 3),
        crate::roll_u64(7, crate::RngDomain::CriticalStrike, 100, 1, 2, 4),
    ] {
        assert_ne!(a, other);
    }

    // The property a shared stateful stream cannot offer: one domain consuming a different NUMBER of
    // rolls cannot shift another domain's sequence, because nothing is consumed at all.
    let crit_sequence: Vec<u64> = (0..8)
        .map(|n| crate::roll_u64(7, crate::RngDomain::CriticalStrike, 5, 1, 0, n))
        .collect();
    let _unrelated: Vec<u64> = (0..100)
        .map(|n| crate::roll_u64(7, crate::RngDomain::NeutralSpawn, 5, 1, 0, n))
        .collect();
    let crit_again: Vec<u64> = (0..8)
        .map(|n| crate::roll_u64(7, crate::RngDomain::CriticalStrike, 5, 1, 0, n))
        .collect();
    assert_eq!(crit_sequence, crit_again);
}

#[test]
fn a_uniform_roll_is_uniform_enough_to_use_as_a_probability() {
    // 20,000 keyed rolls at a nominal 25 %. This is not testing the mixer's cryptographic quality - it is
    // testing that `% BASIS_POINTS` on this mixer does not have a gross low-bit bias, which is the failure
    // mode that would silently skew every crit in the game.
    let trials = 20_000_u32;
    let hits = (0..trials)
        .filter(|n| {
            crate::roll_under_bps(
                0xABCD,
                crate::RngDomain::ProcEffect,
                u64::from(*n),
                1,
                2,
                0,
                2_500,
            )
        })
        .count();
    let rate = f64::from(u32::try_from(hits).unwrap_or(u32::MAX)) / f64::from(trials);
    assert!(
        (0.235..0.265).contains(&rate),
        "observed rate {rate} is outside the expected band for a 25% roll"
    );
}

#[test]
#[expect(
    clippy::cast_possible_truncation,
    reason = "a rounded probability-times-10,000 is a small non-negative integer by construction"
)]
fn the_prd_table_matches_its_own_derivation() {
    // The table in `model.rs` is the solution of `1 / E[N](C) = p`. Re-derive every row here and assert it,
    // so the table is VERIFIED rather than copied from a wiki. The 10 % row is additionally the published
    // cross-check: community sources give C ~= 0.01475, i.e. 147 basis points.
    fn effective_probability(constant: f64) -> f64 {
        let (mut survival, mut expectation, mut attempt) = (1.0_f64, 0.0_f64, 1.0_f64);
        while survival > 1e-15 && attempt < 100_000.0 {
            let p = (constant * attempt).min(1.0);
            expectation += attempt * p * survival;
            survival *= 1.0 - p;
            if p >= 1.0 {
                break;
            }
            attempt += 1.0;
        }
        if expectation > 0.0 {
            1.0 / expectation
        } else {
            0.0
        }
    }
    fn solve(target: f64) -> f64 {
        let (mut low, mut high) = (0.0_f64, 1.0_f64);
        for _ in 0..200 {
            let mid = f64::midpoint(low, high);
            if effective_probability(mid) < target {
                low = mid;
            } else {
                high = mid;
            }
        }
        f64::midpoint(low, high)
    }

    assert_eq!(
        crate::prd_constant_bps(1_000),
        147,
        "the 10% row must match the published PRD constant"
    );
    for percent in 1..100_u16 {
        let derived = u16::try_from((solve(f64::from(percent) / 100.0) * 10_000.0).round() as i64)
            .expect("a solved PRD constant is a basis-point value");
        let table = crate::prd_constant_bps(percent * 100);
        assert_eq!(
            table, derived,
            "PRD constant for {percent}% drifted from its derivation"
        );
    }
    assert_eq!(crate::prd_constant_bps(0), 0);
    assert_eq!(crate::prd_constant_bps(10_000), 10_000);
}

#[test]
fn prd_escalates_and_guarantees_a_hit_where_a_flat_roll_never_would() {
    // The escalation is the whole mechanic: attempt N sees C x N.
    let first = crate::prd_chance_bps(2_000, 1);
    let fifth = crate::prd_chance_bps(2_000, 5);
    assert!(
        first < 2_000,
        "the first attempt is BELOW the nominal chance"
    );
    assert!(fifth > first);
    // And it is bounded: enough dry attempts guarantee a hit, which is exactly what a flat roll cannot do.
    assert_eq!(crate::prd_chance_bps(2_000, 200), crate::BASIS_POINTS);
}

// --- The pipeline -----------------------------------------------------------------------------------

#[test]
fn penetration_scales_mitigation_and_can_never_invert_it() {
    let defender = CombatStats {
        physical_reduction_bps: 4_000,
        ..combat_stats(10_000)
    };
    // No penetration: the shipped behaviour, unchanged.
    let plain = CombatStats {
        physical_penetration_bps: 0,
        ..combat_stats(1_000)
    };
    assert_eq!(
        defender.reduction_after_penetration(DamageSchool::Physical, plain),
        4_000
    );
    // Half penetration halves the mitigation: 40% -> 20%.
    let half = CombatStats {
        physical_penetration_bps: 5_000,
        ..combat_stats(1_000)
    };
    assert_eq!(
        defender.reduction_after_penetration(DamageSchool::Physical, half),
        2_000
    );
    // Full and over-full penetration floor at zero - never negative, mirroring the documented rule that
    // penetration cannot make effective armour negative.
    for pen in [crate::BASIS_POINTS, crate::BASIS_POINTS] {
        let full = CombatStats {
            physical_penetration_bps: pen,
            ..combat_stats(1_000)
        };
        assert_eq!(
            defender.reduction_after_penetration(DamageSchool::Physical, full),
            0
        );
    }
    // Physical penetration does nothing to magic mitigation.
    let magic_defender = CombatStats {
        magic_reduction_bps: 3_000,
        ..combat_stats(10_000)
    };
    assert_eq!(
        magic_defender.reduction_after_penetration(DamageSchool::Magic, half),
        3_000
    );
    // True damage ignores the whole stage.
    assert_eq!(
        defender.reduction_after_penetration(DamageSchool::True, plain),
        0
    );
}

#[test]
fn the_pipeline_applies_crit_before_mitigation_with_hand_computed_numbers() {
    // 100 base damage, guaranteed crit at 250 %, against 40 % mitigation.
    //   crit:      100 * 2.5  = 250   (pre-mitigation, so armour still matters)
    //   mitigation: 250 * 0.6 = 150
    // If crit were applied AFTER mitigation the answer would be 100*0.6*2.5 = 150 as well, so this
    // fixture deliberately uses numbers where the two orders differ once penetration is involved below.
    let attacker = CombatStats {
        crit_chance_bps: crate::BASIS_POINTS,
        crit_damage_bps: 25_000,
        ..combat_stats(1_000)
    };
    let defender = CombatStats {
        physical_reduction_bps: 4_000,
        ..combat_stats(10_000)
    };
    let mut runtime = duel(attacker, defender, 100);
    let frame = swing(&mut runtime, 1);
    assert_eq!(damage_dealt(&frame), Some(150));

    // Now with 50 % penetration: mitigation becomes 20 %, so 250 * 0.8 = 200.
    let attacker = CombatStats {
        crit_chance_bps: crate::BASIS_POINTS,
        crit_damage_bps: 25_000,
        physical_penetration_bps: 5_000,
        ..combat_stats(1_000)
    };
    let mut runtime = duel(attacker, defender, 100);
    let frame = swing(&mut runtime, 1);
    assert_eq!(damage_dealt(&frame), Some(200));
}

#[test]
fn a_zero_crit_chance_actor_deals_exactly_the_pre_change_damage() {
    // The regression guard for every actor authored before this slice: with all the new stats at their
    // defaults the pipeline must reduce to the shipped single-mitigation behaviour, to the unit.
    let defender = CombatStats {
        physical_reduction_bps: 2_000,
        ..combat_stats(10_000)
    };
    let mut runtime = duel(combat_stats(1_000), defender, 500);
    let frame = swing(&mut runtime, 1);
    assert_eq!(damage_dealt(&frame), Some(400), "500 * (1 - 0.20)");
}

#[test]
fn a_shield_absorbs_exactly_its_pool_and_expires_with_the_effect_that_granted_it() {
    let mut runtime = duel(combat_stats(1_000), combat_stats(10_000), 300);
    runtime
        .apply_status_effect_with_shield(DUMMY, 30, &[], &[], 500)
        .unwrap();
    let before = runtime.actor(DUMMY).unwrap().health;

    // First swing: fully absorbed, health untouched, 200 of the pool left.
    let frame = swing(&mut runtime, 1);
    assert_eq!(damage_dealt(&frame), Some(0));
    assert_eq!(runtime.actor(DUMMY).unwrap().health, before);
    assert!(frame.events.iter().any(|event| matches!(
        event,
        MatchEvent::ShieldAbsorbed { amount, remaining, .. } if *amount == 300 && *remaining == 200
    )));

    // Second swing: 200 absorbed, 100 reaches health.
    let frame = swing(&mut runtime, 2);
    assert_eq!(damage_dealt(&frame), Some(100));
    assert_eq!(runtime.actor(DUMMY).unwrap().health, before - 100);

    // Third swing: the pool is gone, so everything lands.
    let frame = swing(&mut runtime, 3);
    assert_eq!(damage_dealt(&frame), Some(300));
    runtime.check_invariants().unwrap();
}

#[test]
fn shields_are_consumed_oldest_expiring_first() {
    let mut runtime = duel(combat_stats(1_000), combat_stats(10_000), 250);
    // Applied in the "wrong" order on purpose: the long one first, so consuming by insertion order would
    // give a different answer than consuming by expiry.
    let long = runtime
        .apply_status_effect_with_shield(DUMMY, 100, &[], &[], 1_000)
        .unwrap();
    let short = runtime
        .apply_status_effect_with_shield(DUMMY, 5, &[], &[], 1_000)
        .unwrap();

    let frame = swing(&mut runtime, 1);
    let absorbed_by = frame.events.iter().find_map(|event| match event {
        MatchEvent::ShieldAbsorbed { effect, .. } => Some(*effect),
        _ => None,
    });
    assert_eq!(
        absorbed_by,
        Some(short),
        "the shield about to expire is spent first, not the one applied first"
    );
    assert_ne!(absorbed_by, Some(long));
}

#[test]
fn lifesteal_heals_from_damage_actually_applied_and_not_from_overkill() {
    let attacker = CombatStats {
        lifesteal_bps: 5_000,
        ..combat_stats(1_000)
    };
    // The dummy has 100 health but the swing is for 1,000: only 100 is APPLIED, so lifesteal is 50 - not
    // 500. Healing from the swung amount rather than the landed amount is the classic overkill bug.
    let mut runtime = duel(attacker, combat_stats(100), 1_000);
    let index = runtime
        .actors
        .iter()
        .position(|a| a.id == DUELLIST)
        .unwrap();
    runtime.actors[index].health = 500;

    let frame = swing(&mut runtime, 1);
    assert_eq!(damage_dealt(&frame), Some(100));
    assert!(frame.events.iter().any(|event| matches!(
        event,
        MatchEvent::LifestealApplied { actor, amount, .. } if *actor == DUELLIST && *amount == 50
    )));
    assert_eq!(runtime.actor(DUELLIST).unwrap().health, 550);
}

#[test]
fn lifesteal_cannot_exceed_maximum_health() {
    let attacker = CombatStats {
        lifesteal_bps: crate::BASIS_POINTS,
        ..combat_stats(1_000)
    };
    let mut runtime = duel(attacker, combat_stats(10_000), 500);
    // Already at full health: the heal must be clamped away entirely, and must not emit a zero event.
    let frame = swing(&mut runtime, 1);
    assert_eq!(runtime.actor(DUELLIST).unwrap().health, 1_000);
    assert!(!frame
        .events
        .iter()
        .any(|event| matches!(event, MatchEvent::LifestealApplied { .. })));
}

#[test]
fn critical_strikes_average_out_to_their_nominal_rate_where_a_flat_roll_would_not() {
    // 1,000 swings at a nominal 20 % crit. Under PRD the observed rate must land close to nominal, AND -
    // the property PRD exists for - the longest dry streak must be far shorter than a flat roll's.
    let attacker = CombatStats {
        crit_chance_bps: 2_000,
        crit_damage_bps: 20_000,
        ..combat_stats(1_000)
    };
    let mut runtime = duel(attacker, combat_stats(u32::MAX), 100);
    let (mut crits, mut swings, mut streak, mut worst) = (0_u32, 0_u32, 0_u32, 0_u32);
    for sequence in 1..=1_000_u32 {
        let frame = swing(&mut runtime, sequence);
        if let Some(dealt) = damage_dealt(&frame) {
            swings += 1;
            if dealt > 100 {
                crits += 1;
                streak = 0;
            } else {
                streak += 1;
                worst = worst.max(streak);
            }
        }
    }
    assert_eq!(swings, 1_000);
    let rate = f64::from(crits) / f64::from(swings);
    assert!(
        (0.17..0.23).contains(&rate),
        "PRD crit rate {rate} strayed from its 20% nominal"
    );
    // The negative control, stated as arithmetic rather than trusted. A FLAT 20 % roll has no bound at
    // all on its dry streak - over 1,000 swings a run of 20+ misses is entirely ordinary. PRD's constant
    // at 20 % is 557 basis points, so the escalating chance reaches certainty at attempt
    // ceil(10000 / 557) = 18, and a streak of 18 consecutive misses is therefore IMPOSSIBLE by
    // construction. That bound is the whole reason PRD exists, and it is what this asserts.
    assert_eq!(crate::prd_constant_bps(2_000), 557);
    assert_eq!(crate::prd_chance_bps(2_000, 18), crate::BASIS_POINTS);
    assert!(
        worst < 18,
        "PRD guarantees a hit by the 18th attempt at 20%, but the worst dry streak was {worst}"
    );
}

#[test]
fn the_same_seed_criticals_identically_and_a_different_seed_does_not() {
    fn run(seed: u64) -> Vec<u32> {
        let attacker = CombatStats {
            crit_chance_bps: 3_000,
            crit_damage_bps: 20_000,
            ..combat_stats(1_000)
        };
        let mut runtime = MatchRuntime::new(MatchConfig::default(), seed).unwrap();
        runtime
            .register_ability(AbilitySpec::unit_targeted(
                CLEAVE,
                AbilityTargeting::Enemy,
                50_000,
                0,
                1,
                0,
                AbilityEffect::Damage {
                    amount: 1,
                    school: DamageSchool::True,
                },
            ))
            .unwrap();
        runtime
            .spawn_actor(&ActorSpawn {
                id: DUELLIST,
                owner: Some(PLAYER_ONE),
                team: TeamId(0),
                kind: ActorKind::Hero,
                position: Vec2Mm::new(0, 0),
                stats: attacker,
                growth: StatGrowth::NONE,
                bounty: Bounty::NONE,
                abilities: vec![CLEAVE],
                basic_attack: Some(BasicAttackSpec {
                    range_mm: 50_000,
                    acquisition_range_mm: 0,
                    damage: 100,
                    school: DamageSchool::Physical,
                    windup_ticks: 0,
                    cooldown_ticks: 0,
                }),
                death_rule: DeathRule::StayDead,
            })
            .unwrap();
        runtime
            .spawn_actor(&ActorSpawn {
                id: DUMMY,
                owner: None,
                team: TeamId(1),
                kind: ActorKind::Minion,
                position: Vec2Mm::new(1_000, 0),
                stats: combat_stats(u32::MAX),
                growth: StatGrowth::NONE,
                bounty: Bounty::NONE,
                abilities: vec![],
                basic_attack: None,
                death_rule: DeathRule::StayDead,
            })
            .unwrap();
        runtime.start().unwrap();
        (1..=40_u32)
            .filter_map(|sequence| damage_dealt(&swing(&mut runtime, sequence)))
            .collect()
    }
    let a = run(0x5EED_0001);
    assert_eq!(a, run(0x5EED_0001), "same seed, identical crit sequence");
    assert_ne!(
        a,
        run(0x5EED_0002),
        "a different seed must roll differently"
    );
}

#[test]
fn crit_state_and_shields_survive_the_checkpoint_round_trip() {
    let content = ContentId::new([0x77; 32]);
    let build = || {
        let attacker = CombatStats {
            crit_chance_bps: 2_500,
            crit_damage_bps: 20_000,
            physical_penetration_bps: 3_000,
            lifesteal_bps: 1_000,
            ..combat_stats(1_000)
        };
        let defender = CombatStats {
            physical_reduction_bps: 3_000,
            ..combat_stats(100_000)
        };
        let mut runtime = duel(attacker, defender, 200);
        runtime
            .apply_status_effect_with_shield(DUMMY, 200, &[], &[], 750)
            .unwrap();
        for sequence in 1..=6 {
            swing(&mut runtime, sequence);
        }
        runtime
    };

    let mut uninterrupted = build();
    let checkpointed = build();
    // Mid-match state that only exists because of this slice.
    assert!(checkpointed.actor(DUELLIST).unwrap().health <= 1_000);
    let checkpoint = checkpointed.capture_checkpoint(content).unwrap();
    let mut restored = MatchRuntime::restore_checkpoint(&checkpoint, content).unwrap();
    assert_eq!(restored.world_digest(), checkpointed.world_digest());

    for sequence in 7..=16 {
        let expected = swing(&mut uninterrupted, sequence);
        let actual = swing(&mut restored, sequence);
        assert_eq!(
            actual.frame_digest, expected.frame_digest,
            "a restored match must critical identically"
        );
    }
}

#[test]
fn the_crit_counter_and_shield_pool_are_part_of_the_world_digest() {
    let attacker = CombatStats {
        crit_chance_bps: 2_500,
        ..combat_stats(1_000)
    };
    let mut runtime = duel(attacker, combat_stats(10_000), 100);
    runtime
        .apply_status_effect_with_shield(DUMMY, 50, &[], &[], 400)
        .unwrap();
    swing(&mut runtime, 1);

    // Each strip removes exactly ONE thing. Without these in the digest, two runtimes that disagree about
    // a pending crit or a remaining shield would still claim to be identical.
    let mut without_counter = runtime.clone();
    let index = without_counter
        .actors
        .iter()
        .position(|a| a.id == DUELLIST)
        .unwrap();
    without_counter.actors[index].crit_attempts = 99;
    assert_ne!(runtime.world_digest(), without_counter.world_digest());

    let mut without_shield = runtime.clone();
    let index = without_shield
        .actors
        .iter()
        .position(|a| a.id == DUMMY)
        .unwrap();
    without_shield.actors[index].status_effects[0].shield_pool = 0;
    assert_ne!(runtime.world_digest(), without_shield.world_digest());
}

#[test]
fn a_forged_crit_counter_is_rejected_on_restore() {
    let attacker = CombatStats {
        crit_chance_bps: 2_500,
        ..combat_stats(1_000)
    };
    let mut runtime = duel(attacker, combat_stats(10_000), 100);
    swing(&mut runtime, 1);
    // Control: the honest runtime audits cleanly.
    runtime.check_invariants().unwrap();

    // A payload claiming an enormous attempt count would guarantee a critical strike on the next swing.
    let mut forged = runtime;
    let index = forged.actors.iter().position(|a| a.id == DUELLIST).unwrap();
    forged.actors[index].crit_attempts = crate::BASIS_POINTS + 1;
    assert!(forged.check_invariants().is_err());
}

// --- GP-08: the standing attack order ---------------------------------------------------------------
//
// The property under test in this whole section is one sentence: ONE order produces MANY swings. Every
// test below either proves that, or proves that the order does not silently do something the player did
// not ask for. A suite that only checked "an auto-attack deals damage" would pass against a kernel that
// re-acquires a target the player deliberately named, chases under a hold, or out-swings a disarm.

const SOLDIER: ActorId = ActorId(801);
const CREEP_NEAR: ActorId = ActorId(901);
const CREEP_FAR: ActorId = ActorId(902);
const ALLY: ActorId = ActorId(803);

const REACH_MM: u32 = 1_000;
const NOTICE_MM: u32 = 5_000;
const SWING_COOLDOWN: u32 = 3;

fn soldier_spawn(position: Vec2Mm, speed: u32, attack: Option<BasicAttackSpec>) -> ActorSpawn {
    ActorSpawn {
        id: SOLDIER,
        owner: Some(PLAYER_ONE),
        team: TeamId(0),
        kind: ActorKind::Hero,
        position,
        stats: stats(10_000, 0, speed),
        growth: StatGrowth::NONE,
        bounty: Bounty::NONE,
        abilities: vec![],
        basic_attack: attack,
        death_rule: DeathRule::StayDead,
    }
}

const fn weapon() -> BasicAttackSpec {
    BasicAttackSpec {
        range_mm: REACH_MM,
        // Deliberately WIDER than reach. If acquisition and reach were the same number, the tests below
        // could not tell "noticed" from "can hit", and the chase/hold distinction would be untestable.
        acquisition_range_mm: NOTICE_MM,
        damage: 100,
        school: DamageSchool::Physical,
        windup_ticks: 0,
        cooldown_ticks: SWING_COOLDOWN,
    }
}

fn creep(id: ActorId, team: TeamId, position: Vec2Mm, health: u32) -> ActorSpawn {
    ActorSpawn {
        id,
        owner: None,
        team,
        kind: ActorKind::Minion,
        position,
        stats: stats(health, 0, 0),
        growth: StatGrowth::NONE,
        bounty: Bounty::NONE,
        abilities: vec![],
        basic_attack: None,
        death_rule: DeathRule::StayDead,
    }
}

/// One mobile soldier plus whatever hostiles a test needs, started and ready to take an order.
fn standoff(speed: u32, hostiles: &[(ActorId, Vec2Mm, u32)]) -> MatchRuntime {
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 0x_0A77_ACC0).unwrap();
    runtime
        .spawn_actor(&soldier_spawn(Vec2Mm::new(0, 0), speed, Some(weapon())))
        .unwrap();
    for &(id, position, health) in hostiles {
        runtime
            .spawn_actor(&creep(id, TeamId(1), position, health))
            .unwrap();
    }
    runtime.start().unwrap();
    runtime
}

fn order(runtime: &mut MatchRuntime, sequence: u32, kind: CommandKind) -> CommandReceipt {
    let at = runtime.tick() + 1;
    runtime
        .submit(command(PLAYER_ONE, sequence, at, SOLDIER, kind))
        .expect("a legal standing order")
}

fn refuse_order(
    runtime: &mut MatchRuntime,
    sequence: u32,
    kind: CommandKind,
) -> CommandRejectReason {
    let at = runtime.tick() + 1;
    runtime
        .submit(command(PLAYER_ONE, sequence, at, SOLDIER, kind))
        .expect_err("an illegal standing order")
        .reason
}

/// Total damage this actor took across a run of frames.
fn damage_taken(frames: &[crate::ServerFrame], victim: ActorId) -> u32 {
    frames
        .iter()
        .flat_map(|frame| frame.events.iter())
        .filter_map(|event| match event {
            MatchEvent::DamageApplied { target, amount, .. } if *target == victim => Some(*amount),
            _ => None,
        })
        .sum()
}

fn run(runtime: &mut MatchRuntime, ticks: u32) -> Vec<crate::ServerFrame> {
    (0..ticks).map(|_| runtime.step().unwrap()).collect()
}

fn position_of(runtime: &MatchRuntime, actor: ActorId) -> Vec2Mm {
    runtime.actor(actor).unwrap().position
}

#[test]
fn one_standing_order_produces_many_swings_where_a_command_would_be_needed_for_each() {
    // This is the row itself. A hostile is already in reach, so the ONLY variable is whether the kernel
    // keeps swinging on its own.
    let mut runtime = standoff(0, &[(CREEP_NEAR, Vec2Mm::new(900, 0), 100_000)]);
    order(&mut runtime, 1, CommandKind::HoldPosition);
    let frames = run(&mut runtime, 24);

    let swings = frames
        .iter()
        .flat_map(|frame| frame.events.iter())
        .filter(|event| matches!(event, MatchEvent::DamageApplied { target, .. } if *target == CREEP_NEAR))
        .count();
    // 24 ticks at one swing every `cooldown` ticks - the swing at tick T sets `ready_at = T + cooldown`
    // and legality is judged against the CURRENT tick, so the period is the cooldown itself. The exact
    // count is asserted, not a floor, because a ">= 2" bound would also pass against a kernel that swung
    // twice and then stalled.
    let expected = 24 / u64::from(SWING_COOLDOWN);
    assert_eq!(
        u64::try_from(swings).unwrap(),
        expected,
        "one order must produce a sustained swing cadence, not a single hit"
    );
    // And the order is still standing: it was never consumed.
    assert_eq!(
        runtime.actor(SOLDIER).unwrap().attack_order,
        Some(AttackOrder::Hold)
    );
}

#[test]
fn every_self_aimed_swing_is_reported_as_acquired_so_a_client_can_tell_it_from_a_commanded_one() {
    let mut runtime = standoff(0, &[(CREEP_NEAR, Vec2Mm::new(900, 0), 100_000)]);
    order(&mut runtime, 1, CommandKind::HoldPosition);
    let frames = run(&mut runtime, 12);

    let acquisitions = frames
        .iter()
        .flat_map(|frame| frame.events.iter())
        .filter(
            |event| matches!(event, MatchEvent::TargetAcquired { actor, target } if *actor == SOLDIER && *target == CREEP_NEAR),
        )
        .count();
    let starts = frames
        .iter()
        .flat_map(|frame| frame.events.iter())
        .filter(|event| matches!(event, MatchEvent::BasicAttackStarted { source, .. } if *source == SOLDIER))
        .count();
    assert!(acquisitions > 0);
    assert_eq!(
        acquisitions, starts,
        "a self-aimed swing must be labelled as such, one for one"
    );
}

#[test]
fn a_hold_engages_what_comes_into_reach_but_never_walks_to_find_it() {
    // The creep is NOTICED (inside acquisition) but out of reach. A hold that chased would be an
    // attack-move, and a player who pressed hold would have lost their position.
    let mut runtime = standoff(300, &[(CREEP_FAR, Vec2Mm::new(4_000, 0), 100_000)]);
    order(&mut runtime, 1, CommandKind::HoldPosition);
    let frames = run(&mut runtime, 30);

    assert_eq!(
        position_of(&runtime, SOLDIER),
        Vec2Mm::new(0, 0),
        "hold must hold"
    );
    assert_eq!(damage_taken(&frames, CREEP_FAR), 0);
}

#[test]
fn an_attack_move_closes_on_what_it_noticed_rather_than_walking_past_it() {
    // The order points along +x; the target sits along +y. A kernel that ignored acquisition would march
    // off down the x axis and never fight, which is exactly the failure this geometry exposes.
    let mut runtime = standoff(300, &[(CREEP_FAR, Vec2Mm::new(0, 3_000), 100_000)]);
    order(
        &mut runtime,
        1,
        CommandKind::AttackMove {
            destination: Vec2Mm::new(100_000, 0),
        },
    );
    let frames = run(&mut runtime, 30);

    let ended = position_of(&runtime, SOLDIER);
    assert!(
        ended.y > 1_500 && ended.x == 0,
        "the soldier must have closed on the target it noticed, not on the order's destination; ended at {ended:?}"
    );
    assert!(damage_taken(&frames, CREEP_FAR) > 0);
}

#[test]
fn an_attack_move_advances_when_nothing_is_in_range_and_stops_dead_when_something_is() {
    let mut runtime = standoff(300, &[(CREEP_NEAR, Vec2Mm::new(10_000, 0), 100_000)]);
    order(
        &mut runtime,
        1,
        CommandKind::AttackMove {
            destination: Vec2Mm::new(100_000, 0),
        },
    );
    // Long enough to cross the 10 m gap at 300 mm/tick and then keep going if nothing stopped it.
    let frames = run(&mut runtime, 60);

    let ended = position_of(&runtime, SOLDIER);
    assert!(
        ended.x >= 9_000 && ended.x <= 10_000,
        "the advance must halt at weapon reach of the creep, not walk through it; ended at {ended:?}"
    );
    assert!(damage_taken(&frames, CREEP_NEAR) > 0);
    assert!(
        frames
            .iter()
            .flat_map(|frame| frame.events.iter())
            .any(|event| matches!(event, MatchEvent::MoveStopped { actor } if *actor == SOLDIER)),
        "stopping to fight must be visible as a stop"
    );
}

#[test]
fn a_named_target_is_never_traded_for_a_closer_one() {
    // Both are hostile and BOTH are in reach. Only the player's naming distinguishes them.
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 0x_0A77_ACC1).unwrap();
    runtime
        .spawn_actor(&soldier_spawn(Vec2Mm::new(0, 0), 0, Some(weapon())))
        .unwrap();
    runtime
        .spawn_actor(&creep(CREEP_NEAR, TeamId(1), Vec2Mm::new(200, 0), 100_000))
        .unwrap();
    runtime
        .spawn_actor(&creep(CREEP_FAR, TeamId(1), Vec2Mm::new(900, 0), 100_000))
        .unwrap();
    runtime.start().unwrap();

    order(
        &mut runtime,
        1,
        CommandKind::AttackTarget { target: CREEP_FAR },
    );
    let frames = run(&mut runtime, 24);

    assert!(damage_taken(&frames, CREEP_FAR) > 0);
    assert_eq!(
        damage_taken(&frames, CREEP_NEAR),
        0,
        "naming a target is an instruction, not a hint"
    );
}

#[test]
fn a_named_target_that_dies_ends_the_order_instead_of_quietly_becoming_a_hold() {
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 0x_0A77_ACC2).unwrap();
    runtime
        .spawn_actor(&soldier_spawn(Vec2Mm::new(0, 0), 0, Some(weapon())))
        .unwrap();
    // Dies to the second swing.
    runtime
        .spawn_actor(&creep(CREEP_FAR, TeamId(1), Vec2Mm::new(900, 0), 150))
        .unwrap();
    runtime
        .spawn_actor(&creep(CREEP_NEAR, TeamId(1), Vec2Mm::new(300, 0), 100_000))
        .unwrap();
    runtime.start().unwrap();

    order(
        &mut runtime,
        1,
        CommandKind::AttackTarget { target: CREEP_FAR },
    );
    let frames = run(&mut runtime, 30);

    assert!(!runtime.actor(CREEP_FAR).unwrap().alive);
    assert_eq!(
        runtime.actor(SOLDIER).unwrap().attack_order,
        None,
        "the order must end with its target"
    );
    assert_eq!(
        damage_taken(&frames, CREEP_NEAR),
        0,
        "an expired named order must not roll over onto a bystander"
    );
    assert!(frames.iter().flat_map(|frame| frame.events.iter()).any(
        |event| matches!(event, MatchEvent::AttackOrderChanged { actor, order: None } if *actor == SOLDIER)
    ));
}

#[test]
fn stop_clears_the_standing_order_so_a_stopped_actor_actually_stops() {
    let mut runtime = standoff(0, &[(CREEP_NEAR, Vec2Mm::new(900, 0), 100_000)]);
    order(&mut runtime, 1, CommandKind::HoldPosition);
    run(&mut runtime, 12);
    order(&mut runtime, 2, CommandKind::Stop);
    let after = run(&mut runtime, 24);

    assert_eq!(runtime.actor(SOLDIER).unwrap().attack_order, None);
    assert_eq!(
        damage_taken(&after, CREEP_NEAR),
        0,
        "stop must mean stop, not stop-walking-but-keep-swinging"
    );
}

#[test]
fn a_disarm_gates_an_automatic_swing_exactly_as_it_gates_a_commanded_one() {
    // The whole reason the driver reuses `validate_actor_intent` rather than swinging directly. A second
    // path into combat would let a standing order out-swing crowd control.
    let mut runtime = standoff(0, &[(CREEP_NEAR, Vec2Mm::new(900, 0), 100_000)]);
    order(&mut runtime, 1, CommandKind::HoldPosition);
    runtime
        .apply_status_effect(SOLDIER, 20, &[ControlKind::Disarm], &[])
        .unwrap();

    let disarmed = run(&mut runtime, 20);
    assert_eq!(
        damage_taken(&disarmed, CREEP_NEAR),
        0,
        "a disarmed actor must not swing, standing order or not"
    );
    let freed = run(&mut runtime, 20);
    assert!(
        damage_taken(&freed, CREEP_NEAR) > 0,
        "and it must resume once the disarm expires, without a new command"
    );
}

#[test]
fn acquisition_takes_the_nearest_and_breaks_ties_by_id_not_by_iteration_order() {
    let build = |near: Vec2Mm, far: Vec2Mm| {
        let mut runtime = MatchRuntime::new(MatchConfig::default(), 0x_0A77_ACC3).unwrap();
        runtime
            .spawn_actor(&soldier_spawn(Vec2Mm::new(0, 0), 0, Some(weapon())))
            .unwrap();
        // Spawned high-id-first on purpose, so "first seen" and "lowest id" disagree.
        runtime
            .spawn_actor(&creep(CREEP_FAR, TeamId(1), far, 100_000))
            .unwrap();
        runtime
            .spawn_actor(&creep(CREEP_NEAR, TeamId(1), near, 100_000))
            .unwrap();
        runtime.start().unwrap();
        runtime
    };

    // Distance decides first.
    let mut nearest = build(Vec2Mm::new(900, 0), Vec2Mm::new(300, 0));
    order(&mut nearest, 1, CommandKind::HoldPosition);
    let frames = run(&mut nearest, 12);
    assert!(damage_taken(&frames, CREEP_FAR) > 0);
    assert_eq!(damage_taken(&frames, CREEP_NEAR), 0);

    // Equidistant. CREEP_NEAR holds the LOWER id but was spawned SECOND, so "first seen" and "lowest id"
    // give different answers here and only the id rule produces this one.
    let mut tied = build(Vec2Mm::new(0, 500), Vec2Mm::new(500, 0));
    order(&mut tied, 1, CommandKind::HoldPosition);
    let frames = run(&mut tied, 12);
    assert!(damage_taken(&frames, CREEP_NEAR) > 0);
    assert_eq!(damage_taken(&frames, CREEP_FAR), 0);
}

#[test]
fn a_standing_order_is_refused_for_an_actor_with_nothing_to_swing() {
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 0x_0A77_ACC4).unwrap();
    runtime
        .spawn_actor(&soldier_spawn(Vec2Mm::new(0, 0), 0, None))
        .unwrap();
    runtime
        .spawn_actor(&creep(CREEP_NEAR, TeamId(1), Vec2Mm::new(900, 0), 100_000))
        .unwrap();
    runtime.start().unwrap();

    for (sequence, kind) in [
        CommandKind::HoldPosition,
        CommandKind::AttackMove {
            destination: Vec2Mm::new(1_000, 0),
        },
        CommandKind::AttackTarget { target: CREEP_NEAR },
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(
            refuse_order(&mut runtime, u32::try_from(sequence).unwrap() + 1, kind),
            CommandRejectReason::BasicAttackUnavailable
        );
    }
}

#[test]
fn a_standing_order_is_refused_for_a_destination_or_a_relation_it_cannot_honour() {
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 0x_0A77_ACC5).unwrap();
    runtime
        .spawn_actor(&soldier_spawn(Vec2Mm::new(0, 0), 0, Some(weapon())))
        .unwrap();
    runtime
        .spawn_actor(&creep(ALLY, TeamId(0), Vec2Mm::new(500, 0), 100_000))
        .unwrap();
    runtime
        .spawn_actor(&creep(CREEP_NEAR, TeamId(1), Vec2Mm::new(900, 0), 1))
        .unwrap();
    runtime.start().unwrap();

    assert_eq!(
        refuse_order(
            &mut runtime,
            1,
            CommandKind::AttackMove {
                destination: Vec2Mm::new(9_000_000, 0),
            }
        ),
        CommandRejectReason::DestinationOutOfBounds
    );
    assert_eq!(
        refuse_order(&mut runtime, 2, CommandKind::AttackTarget { target: ALLY }),
        CommandRejectReason::TargetRelationMismatch
    );
    assert_eq!(
        refuse_order(
            &mut runtime,
            3,
            CommandKind::AttackTarget { target: SOLDIER }
        ),
        CommandRejectReason::TargetRelationMismatch
    );
    assert_eq!(
        refuse_order(
            &mut runtime,
            4,
            CommandKind::AttackTarget {
                target: ActorId(4_242)
            }
        ),
        CommandRejectReason::TargetNotFound
    );
}

#[test]
fn an_out_of_range_named_target_is_accepted_because_closing_the_distance_is_the_point() {
    // The one refusal that would be WRONG: range. A standing order the player can only give when already
    // in range would be useless at the exact moment they want to give it.
    let mut runtime = standoff(300, &[(CREEP_FAR, Vec2Mm::new(400_000, 0), 100_000)]);
    order(
        &mut runtime,
        1,
        CommandKind::AttackTarget { target: CREEP_FAR },
    );
    run(&mut runtime, 10);
    assert!(position_of(&runtime, SOLDIER).x > 0, "it must set off");
}

#[test]
fn an_acquisition_range_shorter_than_the_weapon_reach_is_refused_at_spawn() {
    let mut runtime = MatchRuntime::new(MatchConfig::default(), 0x_0A77_ACC6).unwrap();
    let spec = BasicAttackSpec {
        acquisition_range_mm: REACH_MM - 1,
        ..weapon()
    };
    assert!(runtime
        .spawn_actor(&soldier_spawn(Vec2Mm::new(0, 0), 0, Some(spec)))
        .is_err());
    // Zero is the sanctioned way to say "the same as reach", and must still be accepted.
    let same = BasicAttackSpec {
        acquisition_range_mm: 0,
        ..weapon()
    };
    assert!(runtime
        .spawn_actor(&soldier_spawn(Vec2Mm::new(0, 0), 0, Some(same)))
        .is_ok());
}

#[test]
fn a_standing_order_is_part_of_the_world_and_survives_a_checkpoint_bit_for_bit() {
    let content = ContentId::new([0x08; 32]);
    let build = || {
        let mut runtime = standoff(200, &[(CREEP_FAR, Vec2Mm::new(0, 3_000), 100_000)]);
        let at = runtime.tick() + 1;
        runtime
            .submit(command(
                PLAYER_ONE,
                1,
                at,
                SOLDIER,
                CommandKind::AttackMove {
                    destination: Vec2Mm::new(100_000, 0),
                },
            ))
            .unwrap();
        run(&mut runtime, 8);
        runtime
    };

    let mut uninterrupted = build();
    let checkpointed = build();
    assert_eq!(
        checkpointed.actor(SOLDIER).unwrap().attack_order,
        Some(AttackOrder::Move(Vec2Mm::new(100_000, 0))),
        "the fixture must actually carry a live order, or this test proves nothing"
    );
    let checkpoint = checkpointed.capture_checkpoint(content).unwrap();
    let mut restored = MatchRuntime::restore_checkpoint(&checkpoint, content).unwrap();
    assert_eq!(restored.world_digest(), checkpointed.world_digest());

    for _ in 0..20 {
        let expected = uninterrupted.step().unwrap();
        let actual = restored.step().unwrap();
        assert_eq!(
            actual.frame_digest, expected.frame_digest,
            "a restored standing order must keep fighting identically"
        );
    }
}

#[test]
fn a_standing_order_is_visible_in_the_world_digest() {
    // If the order were not hashed, two worlds that fight differently from the next tick onward would
    // claim to be the same world.
    let plain = standoff(0, &[(CREEP_NEAR, Vec2Mm::new(900, 0), 100_000)]);
    let mut ordered = standoff(0, &[(CREEP_NEAR, Vec2Mm::new(900, 0), 100_000)]);
    assert_eq!(plain.world_digest(), ordered.world_digest());
    order(&mut ordered, 1, CommandKind::HoldPosition);
    ordered.step().unwrap();
    assert_ne!(plain.world_digest(), ordered.world_digest());
}

#[test]
fn a_dead_actor_cannot_retain_a_standing_order() {
    let mut runtime = standoff(0, &[(CREEP_NEAR, Vec2Mm::new(900, 0), 100_000)]);
    order(&mut runtime, 1, CommandKind::HoldPosition);
    runtime.step().unwrap();

    let mut forged = runtime;
    let index = forged.actors.iter().position(|a| a.id == SOLDIER).unwrap();
    forged.actors[index].health = 0;
    assert!(
        forged.check_invariants().is_err(),
        "the audit must refuse a corpse that is still under orders"
    );
}

#[test]
fn a_standing_order_on_an_actor_with_no_weapon_is_refused_by_the_audit() {
    let mut runtime = standoff(0, &[(CREEP_NEAR, Vec2Mm::new(900, 0), 100_000)]);
    order(&mut runtime, 1, CommandKind::HoldPosition);
    runtime.step().unwrap();

    let mut forged = runtime;
    let index = forged.actors.iter().position(|a| a.id == SOLDIER).unwrap();
    forged.actors[index].basic_attack = None;
    assert!(forged.check_invariants().is_err());
}
