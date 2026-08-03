use crate::{
    AbilityEffect, AbilityId, AbilitySpec, AbilityTargeting, ActorId, ActorIntent, ActorKind,
    ActorProvenance, ActorSpawn, BasicAttackSpec, CastTarget, CombatStats, CommandKind,
    CommandRejectReason, ContentId, ControlKind, DamageCause, DamageSchool, DeathRule,
    DynamicActorProvenance, DynamicActorSpawn, MatchConfig, MatchEndReason, MatchEvent,
    MatchOutcome, MatchPhase, MatchRuntime, ModifierOp, PlayerCommand, PlayerId, RuntimeError,
    Scenario, ScenarioFinish, ScenarioSubmission, StatKind, TeamId, Vec2Mm, BASIS_POINTS,
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
        stats: stats(health, 100, 100),
        abilities,
        basic_attack: None,
        death_rule: DeathRule::StayDead,
    }
}

const fn strike() -> AbilitySpec {
    AbilitySpec {
        id: STRIKE,
        targeting: AbilityTargeting::Enemy,
        range_mm: 2_000,
        resource_cost: 20,
        cooldown_ticks: 5,
        cast_ticks: 1,
        effect: AbilityEffect::Damage {
            amount: 600,
            school: DamageSchool::True,
        },
    }
}

const fn heal() -> AbilitySpec {
    AbilitySpec {
        id: HEAL,
        targeting: AbilityTargeting::Ally,
        range_mm: 1_500,
        resource_cost: 10,
        cooldown_ticks: 3,
        cast_ticks: 0,
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
            stats: CombatStats {
                max_health: 1_000,
                max_resource: 100,
                move_speed_mm_per_tick: 100,
                physical_reduction_bps: 2_000,
                magic_reduction_bps: 0,
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
            targeting: AbilityTargeting::Enemy,
            range_mm: 5_000,
            resource_cost: 0,
            cooldown_ticks: 1,
            cast_ticks: 0,
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
            ..strike()
        };
        let delayed_heal = AbilitySpec {
            cast_ticks: 1,
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
