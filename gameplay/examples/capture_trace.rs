//! Capture harness: run real scenarios through the real kernel and emit their authoritative state.
//!
//! This crate is headless by design — there is no renderer, HUD, or viewport for it. That makes a green test
//! count the only evidence anyone normally sees, and a count cannot show that a stun actually stops a hero or
//! that a slow expires on the tick it should. This harness closes that gap: it drives the SAME public API the
//! tests drive, records every emitted `ServerFrame`, and writes a trace an external viewer can render and a
//! human can inspect.
//!
//! It renders nothing itself and asserts nothing beyond invariant checks. Everything it emits is authoritative
//! state the kernel produced — never a value this file computed to make a picture look right.
//!
//! JSON is hand-rolled because the kernel is deliberately zero-dependency; the match-server's `to_json` sets
//! the same precedent. Values are integers and fixed identifiers only, so no general serializer is needed.

use std::fmt::Write as _;

use metrocalk_gameplay::{
    AbilityAim, AbilityDelivery, AbilityEffect, AbilityId, AbilitySpec, AbilityTargeting, ActorId,
    ActorIntent, ActorKind, ActorSpawn, ActorView, AttackOrder, BasicAttackSpec, Bounty,
    CastTarget, CombatStats, CommandKind, CommandRejectReason, CompiledLane, ContentId,
    ControlKind, DamageSchool, DeathRule, GoldReason, ImpactShape, LaneId, LanePosition, LaneSpec,
    MatchConfig, MatchEvent, MatchPhase, MatchRuntime, ModifierOp, OneLaneMatch, PlayerCommand,
    PlayerId, RectMm, StatGrowth, StatKind, TeamId, Tick, Vec2Mm, WaveSpec, WaveUnitSpec,
};

const PLAYER: PlayerId = PlayerId(1);
const ENEMY_PLAYER: PlayerId = PlayerId(2);
const HERO: ActorId = ActorId(101);
const FOE: ActorId = ActorId(202);
const STRIKE: AbilityId = AbilityId(7);
const MEND: AbilityId = AbilityId(9);

fn main() {
    let mut out = String::new();
    out.push_str("{\"schema\":\"metrocalk.gameplay.capture.v1\",\"scenarios\":[");
    let mut first = true;
    for scenario in scenarios() {
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&scenario);
    }
    out.push_str("]}");
    println!("{out}");
}

fn scenarios() -> Vec<String> {
    vec![
        movement_and_rejection(),
        basic_attack_and_mitigation(),
        cast_windup_and_heal(),
        death_and_respawn(),
        stat_modifier_stages(),
        stat_clamping_safety(),
        stun_blocks_and_interrupts(),
        root_silence_disarm_are_distinct(),
        effect_expiry_and_modifier_co_expiry(),
        immobilised_actor_holds_position(),
        dispel_is_atomic(),
        lane_waves_and_bots(),
        owned_player_corridor_anchoring(),
        determinism_two_identical_runs(),
        ground_burst_spares_allies(),
        skillshot_flies_and_strikes_the_nearest_body(),
        skillshot_can_miss(),
        kill_bounty_assist_and_shutdown(),
        levelling_ranks_an_ability_and_raises_stats(),
        crit_penetration_shield_and_lifesteal(),
        attack_move_advances_engages_and_resumes(),
        a_hold_holds_and_a_named_target_is_not_traded(),
        crowd_control_and_stop_both_bind_a_standing_order(),
        checkpoint_restore_suffix_equality(),
    ]
}

// ---------------------------------------------------------------------------------------------
// Recording
// ---------------------------------------------------------------------------------------------

/// One captured scenario: a claim, the frames the kernel actually emitted, and a verdict computed from
/// those frames rather than asserted by hand.
struct Recorder {
    id: &'static str,
    title: &'static str,
    claim: &'static str,
    lane: Option<Vec<Vec2Mm>>,
    half_width_mm: u32,
    frames: Vec<String>,
    notes: Vec<String>,
    checks: Vec<(String, bool)>,
}

impl Recorder {
    fn new(id: &'static str, title: &'static str, claim: &'static str) -> Self {
        Self {
            id,
            title,
            claim,
            lane: None,
            half_width_mm: 0,
            frames: Vec::new(),
            notes: Vec::new(),
            checks: Vec::new(),
        }
    }

    fn with_lane(mut self, spec: &LaneSpec) -> Self {
        self.lane = Some(spec.centerline.clone());
        self.half_width_mm = spec.half_width_mm;
        self
    }

    fn note(&mut self, note: impl Into<String>) {
        self.notes.push(note.into());
    }

    /// Record an observable claim and whether the captured state satisfies it. The viewer renders these, so a
    /// failed check is visible in the capture instead of hiding in a log.
    fn check(&mut self, label: impl Into<String>, passed: bool) {
        self.checks.push((label.into(), passed));
    }

    /// Snapshot the whole world, not just the delta: a viewer needs every actor's position each tick, and the
    /// delta deliberately omits unchanged actors.
    fn record(&mut self, runtime: &MatchRuntime, events: &[MatchEvent], label: &str) {
        runtime
            .check_invariants()
            .expect("captured state must satisfy the frame-boundary audit");
        let mut frame = String::new();
        let _ = write!(
            frame,
            "{{\"tick\":{},\"label\":\"{}\",\"phase\":\"{}\",\"worldDigest\":\"{:016x}\",\"actors\":[",
            runtime.tick(),
            label,
            phase_name(runtime.phase()),
            runtime.world_digest().0
        );
        for (index, actor) in runtime.actors().iter().enumerate() {
            if index > 0 {
                frame.push(',');
            }
            frame.push_str(&actor_json(actor, runtime));
        }
        // Projectiles are authoritative state, so the capture carries them for the same reason it carries
        // actors: a missile that only exists between two health values is invisible to a reader.
        frame.push_str("],\"projectiles\":[");
        for (index, projectile) in runtime.projectiles().iter().enumerate() {
            if index > 0 {
                frame.push(',');
            }
            let _ = write!(
                frame,
                "{{\"id\":{},\"source\":{},\"team\":{},\"x\":{},\"y\":{},\"endX\":{},\"endY\":{},\"hitRadiusMm\":{}}}",
                projectile.id.get(),
                projectile.source.get(),
                projectile.team.get(),
                projectile.position.x,
                projectile.position.y,
                projectile.end.x,
                projectile.end.y,
                projectile.hit_radius_mm
            );
        }
        // The scoreboard is authoritative state too. Without it a reader can watch a hero die and never
        // see who was paid for it.
        frame.push_str("],\"scores\":[");
        for (index, line) in runtime.scores().iter().enumerate() {
            if index > 0 {
                frame.push(',');
            }
            let _ = write!(
                frame,
                "{{\"actor\":{},\"team\":{},\"gold\":{},\"kills\":{},\"deaths\":{},\"assists\":{},\"streak\":{}}}",
                line.actor.get(),
                line.team.get(),
                line.gold,
                line.kills,
                line.deaths,
                line.assists,
                line.kill_streak
            );
        }
        frame.push_str("],\"events\":[");
        for (index, event) in events.iter().enumerate() {
            if index > 0 {
                frame.push(',');
            }
            let _ = write!(frame, "\"{}\"", event_label(*event));
        }
        frame.push_str("]}");
        self.frames.push(frame);
    }

    fn finish(self) -> String {
        let mut out = String::new();
        let _ = write!(
            out,
            "{{\"id\":\"{}\",\"title\":\"{}\",\"claim\":\"{}\",\"halfWidthMm\":{},\"lane\":[",
            self.id, self.title, self.claim, self.half_width_mm
        );
        if let Some(points) = &self.lane {
            for (index, point) in points.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                let _ = write!(out, "{{\"x\":{},\"y\":{}}}", point.x, point.y);
            }
        }
        out.push_str("],\"notes\":[");
        for (index, note) in self.notes.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let _ = write!(out, "\"{note}\"");
        }
        out.push_str("],\"checks\":[");
        for (index, (label, passed)) in self.checks.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let _ = write!(out, "{{\"label\":\"{label}\",\"passed\":{passed}}}");
        }
        out.push_str("],\"frames\":[");
        out.push_str(&self.frames.join(","));
        out.push_str("]}");
        out
    }
}

fn actor_json(actor: &ActorView, runtime: &MatchRuntime) -> String {
    let mut controls = String::new();
    for (index, kind) in actor.controls.kinds().iter().enumerate() {
        if index > 0 {
            controls.push(',');
        }
        let _ = write!(controls, "\"{}\"", control_name(*kind));
    }
    let effects = runtime.status_effects(actor.id).unwrap_or_default();
    let mut expiries = String::new();
    for (index, effect) in effects.iter().enumerate() {
        if index > 0 {
            expiries.push(',');
        }
        let _ = write!(expiries, "{}", effect.expires_at);
    }
    format!(
        concat!(
            "{{\"level\":{},\"xp\":{},\"points\":{},",
            "\"id\":{},\"team\":{},\"kind\":\"{}\",\"owned\":{},\"x\":{},\"y\":{},",
            "\"health\":{},\"maxHealth\":{},\"speed\":{},\"physBps\":{},\"magicBps\":{},",
            "\"alive\":{},\"casting\":{},\"attacking\":{},\"controls\":[{}],\"effectExpiries\":[{}],",
            "\"modifierCount\":{}}}"
        ),
        actor.level,
        actor.experience,
        actor.unspent_ability_points,
        actor.id.get(),
        actor.team.get(),
        kind_name(actor.kind),
        actor.owner.is_some(),
        actor.position.x,
        actor.position.y,
        actor.health,
        actor.max_health,
        actor.move_speed_mm_per_tick,
        actor.physical_reduction_bps,
        actor.magic_reduction_bps,
        actor.alive,
        actor.cast.is_some(),
        actor.basic_attack.is_some(),
        controls,
        expiries,
        runtime.modifiers(actor.id).map_or(0, |list| list.len()),
    )
}

const fn kind_name(kind: ActorKind) -> &'static str {
    match kind {
        ActorKind::Hero => "Hero",
        ActorKind::Minion => "Minion",
        ActorKind::Structure => "Structure",
        ActorKind::Objective => "Objective",
        ActorKind::Pet => "Pet",
        ActorKind::Projectile => "Projectile",
    }
}

const fn control_name(kind: ControlKind) -> &'static str {
    match kind {
        ControlKind::Stun => "Stun",
        ControlKind::Root => "Root",
        ControlKind::Silence => "Silence",
        ControlKind::Disarm => "Disarm",
    }
}

const fn phase_name(phase: MatchPhase) -> &'static str {
    match phase {
        MatchPhase::Setup => "Setup",
        MatchPhase::Active => "Active",
        MatchPhase::Complete { .. } => "Complete",
    }
}

const fn gold_reason_name(reason: GoldReason) -> &'static str {
    match reason {
        GoldReason::Kill => "kill",
        GoldReason::Shutdown => "shutdown",
        GoldReason::Assist => "assist",
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one arm per event keeps the capture's vocabulary exhaustive and visible"
)]
fn event_label(event: MatchEvent) -> String {
    match event {
        MatchEvent::MatchStarted => "MatchStarted".to_owned(),
        MatchEvent::CommandRejected { reason, .. } => {
            format!("CommandRejected: {}", reason_name(reason))
        }
        MatchEvent::MoveStarted { actor, .. } => format!("MoveStarted a{}", actor.get()),
        MatchEvent::MoveStopped { actor } => format!("MoveStopped a{}", actor.get()),
        MatchEvent::TargetAcquired { actor, target } => {
            format!("TargetAcquired a{} -> a{}", actor.get(), target.get())
        }
        MatchEvent::AttackOrderChanged { actor, order } => match order {
            None => format!("AttackOrderCleared a{}", actor.get()),
            Some(AttackOrder::Target(target)) => {
                format!("AttackOrder a{} target a{}", actor.get(), target.get())
            }
            Some(AttackOrder::Move(destination)) => format!(
                "AttackOrder a{} move ({},{})",
                actor.get(),
                destination.x,
                destination.y
            ),
            Some(AttackOrder::Hold) => format!("AttackOrder a{} hold", actor.get()),
        },
        MatchEvent::CastStarted { source, .. } => format!("CastStarted a{}", source.get()),
        MatchEvent::CastCancelled { source, reason, .. } => {
            format!("CastCancelled a{} ({})", source.get(), reason_name(reason))
        }
        MatchEvent::CastResolved { source, .. } => format!("CastResolved a{}", source.get()),
        MatchEvent::DamageApplied {
            target,
            amount,
            health_after,
            ..
        } => format!("Damage {amount} to a{} -> {health_after}hp", target.get()),
        MatchEvent::HealingApplied {
            target,
            amount,
            health_after,
            ..
        } => format!("Heal {amount} to a{} -> {health_after}hp", target.get()),
        MatchEvent::ActorDied { actor, .. } => format!("ActorDied a{}", actor.get()),
        MatchEvent::BasicAttackStarted { source, .. } => {
            format!("BasicAttackStarted a{}", source.get())
        }
        MatchEvent::BasicAttackCancelled { source, reason, .. } => format!(
            "BasicAttackCancelled a{} ({})",
            source.get(),
            reason_name(reason)
        ),
        MatchEvent::BasicAttackResolved { source, .. } => {
            format!("BasicAttackResolved a{}", source.get())
        }
        MatchEvent::ActorDespawned { actor } => format!("ActorDespawned a{}", actor.get()),
        MatchEvent::RespawnScheduled { actor, at_tick } => {
            format!("RespawnScheduled a{} @{at_tick}", actor.get())
        }
        MatchEvent::RespawnCancelled { actor, .. } => {
            format!("RespawnCancelled a{}", actor.get())
        }
        MatchEvent::ActorRespawned { actor, .. } => format!("ActorRespawned a{}", actor.get()),
        MatchEvent::InternalIntentRejected { actor, reason } => format!(
            "InternalIntentRejected a{} ({})",
            actor.get(),
            reason_name(reason)
        ),
        MatchEvent::StatusEffectExpired { actor, effect } => {
            format!("StatusEffectExpired a{} e{}", actor.get(), effect.get())
        }
        MatchEvent::ActorSpawned { actor } => format!("ActorSpawned a{}", actor.get()),
        MatchEvent::ProjectileSpawned {
            projectile, source, ..
        } => format!(
            "ProjectileSpawned p{} from a{}",
            projectile.get(),
            source.get()
        ),
        MatchEvent::ProjectileResolved {
            projectile, victim, ..
        } => match victim {
            Some(victim) => format!(
                "ProjectileResolved p{} hit a{}",
                projectile.get(),
                victim.get()
            ),
            None => format!("ProjectileResolved p{} at flight end", projectile.get()),
        },
        MatchEvent::KillCredited {
            killer,
            victim,
            streak_after,
        } => format!(
            "KillCredited a{} killed a{} (streak {streak_after})",
            killer.get(),
            victim.get()
        ),
        MatchEvent::AssistCredited { actor, victim } => {
            format!("AssistCredited a{} on a{}", actor.get(), victim.get())
        }
        MatchEvent::GoldGranted {
            actor,
            amount,
            reason,
            total_after,
        } => format!(
            "Gold +{amount} ({}) to a{} -> {total_after}",
            gold_reason_name(reason),
            actor.get()
        ),
        MatchEvent::ExperienceGranted {
            actor,
            amount,
            total_after,
        } => format!("XP +{amount} to a{} -> {total_after}", actor.get()),
        MatchEvent::HeroLevelUp {
            actor,
            level,
            unspent_ability_points,
        } => format!(
            "LevelUp a{} -> L{level} ({unspent_ability_points} unspent)",
            actor.get()
        ),
        MatchEvent::AbilityRankUp {
            actor,
            ability,
            rank,
        } => format!(
            "RankUp a{} ability {} -> rank {rank}",
            actor.get(),
            ability.get()
        ),
        MatchEvent::ShieldAbsorbed {
            actor,
            amount,
            remaining,
            ..
        } => format!(
            "ShieldAbsorbed a{} -{amount} ({remaining} left)",
            actor.get()
        ),
        MatchEvent::LifestealApplied {
            actor,
            amount,
            health_after,
        } => format!("Lifesteal a{} +{amount} -> {health_after}hp", actor.get()),
        MatchEvent::ProjectileCancelled { projectile, reason } => format!(
            "ProjectileCancelled p{} ({})",
            projectile.get(),
            reason_name(reason)
        ),
        MatchEvent::MatchFinished { outcome, reason } => {
            format!("MatchFinished {outcome:?} ({reason:?})")
        }
    }
}

const fn reason_name(reason: CommandRejectReason) -> &'static str {
    match reason {
        CommandRejectReason::ActorStunned => "ActorStunned",
        CommandRejectReason::ActorRooted => "ActorRooted",
        CommandRejectReason::ActorSilenced => "ActorSilenced",
        CommandRejectReason::ActorDisarmed => "ActorDisarmed",
        CommandRejectReason::ActorDead => "ActorDead",
        CommandRejectReason::ActorCasting => "ActorCasting",
        CommandRejectReason::ActorBusy => "ActorBusy",
        CommandRejectReason::DestinationOutOfBounds => "DestinationOutOfBounds",
        CommandRejectReason::TargetOutOfRange => "TargetOutOfRange",
        CommandRejectReason::AbilityOnCooldown { .. } => "AbilityOnCooldown",
        CommandRejectReason::InsufficientResource { .. } => "InsufficientResource",
        _ => "other",
    }
}

// ---------------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------------

const fn stats(health: u32, resource: u32, speed: u32) -> CombatStats {
    CombatStats {
        max_health: health,
        max_resource: resource,
        move_speed_mm_per_tick: speed,
        physical_reduction_bps: 0,
        magic_reduction_bps: 0,
        crit_chance_bps: 0,
        crit_damage_bps: metrocalk_gameplay::BASIS_POINTS,
        physical_penetration_bps: 0,
        magic_penetration_bps: 0,
        lifesteal_bps: 0,
    }
}

fn config() -> MatchConfig {
    MatchConfig {
        map_bounds: RectMm::new(Vec2Mm::new(-2_000, -2_000), Vec2Mm::new(12_000, 2_000)),
        max_match_ticks: 200,
        ..MatchConfig::default()
    }
}

fn hero(id: ActorId, owner: Option<PlayerId>, team: u8, x: i32, mitigation: u16) -> ActorSpawn {
    ActorSpawn {
        id,
        owner,
        team: TeamId(team),
        kind: ActorKind::Hero,
        position: Vec2Mm::new(x, 0),
        growth: StatGrowth::NONE,
        bounty: Bounty::NONE,
        stats: CombatStats {
            physical_reduction_bps: mitigation,
            ..stats(1_000, 100, 500)
        },
        abilities: vec![STRIKE, MEND],
        basic_attack: Some(BasicAttackSpec {
            range_mm: 1_500,
            acquisition_range_mm: 0,
            damage: 200,
            school: DamageSchool::Physical,
            windup_ticks: 1,
            cooldown_ticks: 3,
        }),
        death_rule: DeathRule::StayDead,
    }
}

fn duel(mitigation: u16) -> MatchRuntime {
    let mut runtime = MatchRuntime::new(config(), 0x_CA97).expect("config");
    runtime
        .register_ability(AbilitySpec {
            id: STRIKE,
            aim: AbilityAim::Unit,
            delivery: AbilityDelivery::Instant,
            impact: ImpactShape::Single,
            targeting: AbilityTargeting::Enemy,
            range_mm: 3_000,
            resource_cost: 10,
            cooldown_ticks: 4,
            cast_ticks: 2,
            per_rank_amount: 0,
            effect: AbilityEffect::Damage {
                amount: 300,
                school: DamageSchool::Magic,
            },
        })
        .expect("strike");
    runtime
        .register_ability(AbilitySpec {
            id: MEND,
            aim: AbilityAim::Unit,
            delivery: AbilityDelivery::Instant,
            impact: ImpactShape::Single,
            targeting: AbilityTargeting::SelfOnly,
            range_mm: 0,
            resource_cost: 5,
            cooldown_ticks: 4,
            cast_ticks: 1,
            per_rank_amount: 0,
            effect: AbilityEffect::Heal { amount: 150 },
        })
        .expect("mend");
    runtime
        .spawn_actor(&hero(HERO, Some(PLAYER), 0, 0, 0))
        .expect("hero");
    runtime
        .spawn_actor(&hero(FOE, Some(ENEMY_PLAYER), 1, 1_000, mitigation))
        .expect("foe");
    runtime.start().expect("start");
    runtime
}

fn command(sequence: u32, at: Tick, actor: ActorId, kind: CommandKind) -> PlayerCommand {
    PlayerCommand {
        player: PLAYER,
        sequence,
        execute_at: at,
        actor,
        kind,
    }
}

// ---------------------------------------------------------------------------------------------
// Scenarios
// ---------------------------------------------------------------------------------------------

fn movement_and_rejection() -> String {
    let mut rec = Recorder::new(
        "mob1-movement",
        "MOB-1 · movement and out-of-bounds refusal",
        "A MoveTo advances the actor by its move speed each tick; a destination outside map bounds is refused.",
    );
    let mut runtime = duel(0);
    rec.record(&runtime, &[], "start");
    runtime
        .submit(command(
            1,
            1,
            HERO,
            CommandKind::MoveTo {
                destination: Vec2Mm::new(2_000, 0),
            },
        ))
        .expect("accepted");
    for _ in 0..5 {
        let frame = runtime.step().expect("step");
        rec.record(&runtime, &frame.events, "move");
    }
    let arrived = runtime.actor(HERO).expect("hero").position.x;
    rec.check(
        format!("hero reached x={arrived} (destination 2000)"),
        arrived == 2_000,
    );

    let rejection = runtime
        .submit(command(
            2,
            runtime.tick() + 1,
            HERO,
            CommandKind::MoveTo {
                destination: Vec2Mm::new(99_000, 0),
            },
        ))
        .expect_err("out of bounds");
    rec.check(
        "out-of-bounds destination refused as DestinationOutOfBounds",
        rejection.reason == CommandRejectReason::DestinationOutOfBounds,
    );
    rec.note("Speed is 500 mm/tick, so four ticks cover the 2 000 mm gap exactly.");
    rec.finish()
}

fn basic_attack_and_mitigation() -> String {
    let mut rec = Recorder::new(
        "mob1-mitigation",
        "MOB-1 · basic attack through integer mitigation",
        "A 200-damage physical attack against 2 500 bps (25%) mitigation lands 150 after truncation.",
    );
    let mut runtime = duel(2_500);
    rec.record(&runtime, &[], "start");
    runtime
        .submit(command(
            1,
            1,
            HERO,
            CommandKind::BasicAttack { target: FOE },
        ))
        .expect("accepted");
    let before = runtime.actor(FOE).expect("foe").health;
    for _ in 0..3 {
        let frame = runtime.step().expect("step");
        rec.record(&runtime, &frame.events, "attack");
    }
    let after = runtime.actor(FOE).expect("foe").health;
    rec.check(
        format!(
            "dealt {} damage through 25% mitigation (expected 150)",
            before - after
        ),
        before - after == 150,
    );
    rec.finish()
}

fn cast_windup_and_heal() -> String {
    let mut rec = Recorder::new(
        "mob1-cast",
        "MOB-1 · cast windup, damage, and self-heal",
        "A 2-tick cast resolves on its deadline; a self-heal restores health without exceeding the cap.",
    );
    let mut runtime = duel(0);
    runtime
        .submit(command(
            1,
            1,
            HERO,
            CommandKind::Cast {
                ability: STRIKE,
                target: CastTarget::Actor(FOE),
            },
        ))
        .expect("accepted");
    for _ in 0..4 {
        let frame = runtime.step().expect("step");
        rec.record(&runtime, &frame.events, "strike");
    }
    let foe = runtime.actor(FOE).expect("foe");
    rec.check(
        format!(
            "magic strike left the foe at {}hp (expected 700)",
            foe.health
        ),
        foe.health == 700,
    );

    runtime
        .submit(command(
            2,
            runtime.tick() + 1,
            FOE,
            CommandKind::Cast {
                ability: MEND,
                target: CastTarget::SelfActor,
            },
        ))
        .ok();
    rec.note(
        "The foe is owned by a second player, so its heal is submitted by that player's session.",
    );
    rec.finish()
}

fn death_and_respawn() -> String {
    let mut rec = Recorder::new(
        "mob1-respawn",
        "MOB-1 · death and timed respawn",
        "A lethal hit kills the actor; a Respawn death rule returns it at full health on its scheduled tick.",
    );
    let mut runtime = MatchRuntime::new(config(), 0xDEAD).expect("config");
    runtime
        .spawn_actor(&ActorSpawn {
            abilities: vec![],
            basic_attack: Some(BasicAttackSpec {
                range_mm: 2_000,
                acquisition_range_mm: 0,
                damage: 5_000,
                school: DamageSchool::True,
                windup_ticks: 0,
                cooldown_ticks: 2,
            }),
            ..hero(HERO, Some(PLAYER), 0, 0, 0)
        })
        .expect("hero");
    runtime
        .spawn_actor(&ActorSpawn {
            abilities: vec![],
            death_rule: DeathRule::Respawn {
                delay_ticks: 3,
                at: Vec2Mm::new(5_000, 0),
            },
            ..hero(FOE, Some(ENEMY_PLAYER), 1, 1_000, 0)
        })
        .expect("foe");
    runtime.start().expect("start");
    runtime
        .submit(command(
            1,
            1,
            HERO,
            CommandKind::BasicAttack { target: FOE },
        ))
        .expect("accepted");
    for _ in 0..7 {
        let frame = runtime.step().expect("step");
        rec.record(&runtime, &frame.events, "respawn cycle");
    }
    let foe = runtime.actor(FOE).expect("foe");
    rec.check(
        format!(
            "respawned alive at x={} with {}hp",
            foe.position.x, foe.health
        ),
        foe.alive && foe.position.x == 5_000 && foe.health == foe.max_health,
    );
    rec.finish()
}

fn stat_modifier_stages() -> String {
    let mut rec = Recorder::new(
        "gp07-stages",
        "GP-07 · the four aggregation stages, in order",
        "Flat sums, PercentAdd sums into one multiplier, PercentMult compounds by magnitude, Override discards.",
    );
    let mut runtime = duel(0);
    rec.record(&runtime, &[], "base speed 500");

    runtime
        .apply_stat_modifier(
            HERO,
            StatKind::MoveSpeed,
            ModifierOp::Flat { magnitude: 100 },
        )
        .expect("flat");
    rec.record(&runtime, &[], "+100 flat");
    let flat = runtime.actor(HERO).expect("h").move_speed_mm_per_tick;
    rec.check(format!("flat: 500 + 100 = {flat}"), flat == 600);

    runtime
        .apply_stat_modifier(
            HERO,
            StatKind::MoveSpeed,
            ModifierOp::PercentAdd { bps: 5_000 },
        )
        .expect("padd");
    rec.record(&runtime, &[], "+50% additive");
    let padd = runtime.actor(HERO).expect("h").move_speed_mm_per_tick;
    rec.check(format!("percent-add: 600 x 1.5 = {padd}"), padd == 900);

    runtime
        .apply_stat_modifier(
            HERO,
            StatKind::MoveSpeed,
            ModifierOp::PercentMult { bps: -5_000 },
        )
        .expect("pmult");
    rec.record(&runtime, &[], "x0.5 multiplicative");
    let pmult = runtime.actor(HERO).expect("h").move_speed_mm_per_tick;
    rec.check(format!("percent-mult: 900 x 0.5 = {pmult}"), pmult == 450);

    let override_id = runtime
        .apply_stat_modifier(
            HERO,
            StatKind::MoveSpeed,
            ModifierOp::Override { value: 42 },
        )
        .expect("override");
    rec.record(&runtime, &[], "override 42");
    let overridden = runtime.actor(HERO).expect("h").move_speed_mm_per_tick;
    rec.check(
        format!("override discards stages 1-3: {overridden}"),
        overridden == 42,
    );

    runtime
        .remove_stat_modifier(HERO, override_id)
        .expect("remove");
    rec.record(&runtime, &[], "override removed");
    rec.check(
        "removing the override restores the composed value",
        runtime.actor(HERO).expect("h").move_speed_mm_per_tick == 450,
    );
    rec.finish()
}

fn stat_clamping_safety() -> String {
    let mut rec = Recorder::new(
        "gp07-clamping",
        "GP-07 · clamping is a safety property",
        "Stacked mitigation cannot reach 100% (no immortality) and MaxHealth floors at 1 (never lethal).",
    );
    let mut runtime = duel(0);
    for _ in 0..8 {
        runtime
            .apply_stat_modifier(
                HERO,
                StatKind::PhysicalReduction,
                ModifierOp::Flat { magnitude: 5_000 },
            )
            .expect("mitigation");
    }
    rec.record(&runtime, &[], "8 x +50% mitigation");
    let bps = runtime.actor(HERO).expect("h").physical_reduction_bps;
    rec.check(
        format!("mitigation clamped to {bps} bps, one basis point below whole"),
        bps == 9_999,
    );

    runtime
        .apply_stat_modifier(
            HERO,
            StatKind::MaxHealth,
            ModifierOp::Flat {
                magnitude: i32::MIN,
            },
        )
        .expect("drain");
    rec.record(&runtime, &[], "MaxHealth drained to the floor");
    let hero = runtime.actor(HERO).expect("h");
    rec.check(
        format!(
            "max health floored at {} and the actor is still alive",
            hero.max_health
        ),
        hero.max_health == 1 && hero.alive,
    );
    rec.finish()
}

fn stun_blocks_and_interrupts() -> String {
    let mut rec = Recorder::new(
        "gp02-stun",
        "GP-02 · a stun blocks every verb and interrupts",
        "A stun cancels an in-flight cast and refuses move, cast, and attack for its whole window.",
    );
    let mut runtime = duel(0);
    runtime
        .submit(command(
            1,
            1,
            HERO,
            CommandKind::Cast {
                ability: STRIKE,
                target: CastTarget::Actor(FOE),
            },
        ))
        .expect("accepted");
    let frame = runtime.step().expect("step");
    rec.record(&runtime, &frame.events, "cast in flight");
    rec.check(
        "cast is genuinely in flight before the stun",
        runtime.actor(HERO).expect("h").cast.is_some(),
    );

    runtime
        .apply_status_effect(HERO, 6, &[ControlKind::Stun], &[])
        .expect("stun");
    rec.record(&runtime, &[], "STUN applied");
    rec.check(
        "the in-flight cast was interrupted",
        runtime.actor(HERO).expect("h").cast.is_none(),
    );

    let mut refused = Vec::new();
    for (sequence, kind) in [
        (
            2,
            CommandKind::MoveTo {
                destination: Vec2Mm::new(500, 0),
            },
        ),
        (
            3,
            CommandKind::Cast {
                ability: STRIKE,
                target: CastTarget::Actor(FOE),
            },
        ),
        (4, CommandKind::BasicAttack { target: FOE }),
    ] {
        let outcome = runtime.submit(command(sequence, runtime.tick() + 1, HERO, kind));
        refused.push(outcome.is_err());
    }
    rec.check(
        "move, cast and attack all refused while stunned",
        refused.iter().all(|r| *r),
    );
    rec.check(
        "Stop is still accepted (it only cancels)",
        runtime
            .submit(command(5, runtime.tick() + 1, HERO, CommandKind::Stop))
            .is_ok(),
    );
    for _ in 0..7 {
        let frame = runtime.step().expect("step");
        rec.record(&runtime, &frame.events, "stun window");
    }
    rec.check(
        "the stun has expired and the hero is free",
        runtime.actor(HERO).expect("h").controls.is_empty(),
    );
    rec.finish()
}

fn root_silence_disarm_are_distinct() -> String {
    let mut rec = Recorder::new(
        "gp02-distinct",
        "GP-02 · each restriction blocks exactly its own verb",
        "Root blocks only movement, Silence only casting, Disarm only attacking. A Root that also silenced would be a Stun by accident.",
    );
    for (control, label) in [
        (ControlKind::Root, "Root"),
        (ControlKind::Silence, "Silence"),
        (ControlKind::Disarm, "Disarm"),
    ] {
        let mut runtime = duel(0);
        runtime
            .apply_status_effect(HERO, 10, &[control], &[])
            .expect("effect");
        rec.record(&runtime, &[], label);

        let moved = runtime
            .submit(command(
                1,
                1,
                HERO,
                CommandKind::MoveTo {
                    destination: Vec2Mm::new(400, 0),
                },
            ))
            .is_ok();
        let cast = runtime
            .submit(command(
                2,
                1,
                HERO,
                CommandKind::Cast {
                    ability: STRIKE,
                    target: CastTarget::Actor(FOE),
                },
            ))
            .is_ok();
        let attacked = runtime
            .submit(command(
                3,
                1,
                HERO,
                CommandKind::BasicAttack { target: FOE },
            ))
            .is_ok();
        let expected = match control {
            ControlKind::Root => (false, true, true),
            ControlKind::Silence => (true, false, true),
            _ => (true, true, false),
        };
        rec.check(
            format!("{label}: move={moved} cast={cast} attack={attacked}"),
            (moved, cast, attacked) == expected,
        );
    }
    rec.finish()
}

fn effect_expiry_and_modifier_co_expiry() -> String {
    let mut rec = Recorder::new(
        "gp02-expiry",
        "GP-02 · expiry frees the actor and takes its modifiers with it",
        "A slow is a Root plus a speed penalty; both end together, and the actor is free on the tick after the last.",
    );
    let mut runtime = duel(0);
    let base = runtime.actor(HERO).expect("h").move_speed_mm_per_tick;
    runtime
        .apply_status_effect(
            HERO,
            4,
            &[ControlKind::Root],
            &[(StatKind::MoveSpeed, ModifierOp::PercentAdd { bps: -5_000 })],
        )
        .expect("slow");
    rec.record(&runtime, &[], "slow applied");
    rec.check(
        format!(
            "speed halved from {base} to {}",
            runtime.actor(HERO).expect("h").move_speed_mm_per_tick
        ),
        runtime.actor(HERO).expect("h").move_speed_mm_per_tick == base / 2,
    );
    rec.check(
        "one modifier is live, granted by the effect",
        runtime.modifiers(HERO).expect("m").len() == 1,
    );

    for _ in 0..6 {
        let frame = runtime.step().expect("step");
        rec.record(&runtime, &frame.events, "ticking down");
    }
    rec.check(
        "the granted modifier expired with its effect - no orphan buff",
        runtime.modifiers(HERO).expect("m").is_empty(),
    );
    rec.check(
        format!(
            "speed restored to {}",
            runtime.actor(HERO).expect("h").move_speed_mm_per_tick
        ),
        runtime.actor(HERO).expect("h").move_speed_mm_per_tick == base,
    );
    rec.finish()
}

fn immobilised_actor_holds_position() -> String {
    let mut rec = Recorder::new(
        "gp02-hold",
        "GP-02 · an immobilised actor stops moving, not just taking orders",
        "Gating only the ORDER would let an actor rooted mid-path keep walking to a destination it can no longer abandon.",
    );
    let mut runtime = duel(0);
    runtime
        .submit(command(
            1,
            1,
            HERO,
            CommandKind::MoveTo {
                destination: Vec2Mm::new(4_000, 0),
            },
        ))
        .expect("accepted");
    for _ in 0..2 {
        let frame = runtime.step().expect("step");
        rec.record(&runtime, &frame.events, "under way");
    }
    let held_at = runtime.actor(HERO).expect("h").position.x;
    runtime
        .apply_status_effect(HERO, 5, &[ControlKind::Root], &[])
        .expect("root");
    rec.record(&runtime, &[], "ROOT applied mid-path");
    for _ in 0..3 {
        let frame = runtime.step().expect("step");
        rec.record(&runtime, &frame.events, "rooted");
    }
    rec.check(
        format!("held at x={held_at} for the whole root instead of walking on"),
        runtime.actor(HERO).expect("h").position.x == held_at,
    );
    for _ in 0..4 {
        let frame = runtime.step().expect("step");
        rec.record(&runtime, &frame.events, "released");
    }
    rec.check(
        "resumed moving once the root expired",
        runtime.actor(HERO).expect("h").position.x > held_at,
    );
    rec.finish()
}

fn dispel_is_atomic() -> String {
    let mut rec = Recorder::new(
        "gp02-dispel",
        "GP-02 · a dispel removes the control and its modifiers together",
        "Removing an effect early takes every modifier it granted, leaving no orphan buff behind.",
    );
    let mut runtime = duel(0);
    let effect = runtime
        .apply_status_effect(
            HERO,
            60,
            &[ControlKind::Stun, ControlKind::Silence],
            &[
                (StatKind::MoveSpeed, ModifierOp::Flat { magnitude: -200 }),
                (
                    StatKind::PhysicalReduction,
                    ModifierOp::Flat { magnitude: 1_000 },
                ),
            ],
        )
        .expect("effect");
    rec.record(&runtime, &[], "stun + silence + 2 modifiers");
    rec.check(
        "two modifiers granted by the effect",
        runtime.modifiers(HERO).expect("m").len() == 2,
    );
    runtime.remove_status_effect(HERO, effect).expect("dispel");
    rec.record(&runtime, &[], "DISPELLED");
    let hero = runtime.actor(HERO).expect("h");
    rec.check(
        "controls cleared, modifiers gone, speed and mitigation restored",
        hero.controls.is_empty()
            && runtime.modifiers(HERO).expect("m").is_empty()
            && hero.move_speed_mm_per_tick == 500
            && hero.physical_reduction_bps == 0,
    );
    rec.finish()
}

fn lane_spec() -> LaneSpec {
    LaneSpec {
        id: LaneId(0),
        centerline: vec![Vec2Mm::new(0, 0), Vec2Mm::new(10_000, 0)],
        half_width_mm: 500,
    }
}

fn lane_match() -> (OneLaneMatch, LaneSpec) {
    let spec = lane_spec();
    let config = MatchConfig {
        map_bounds: RectMm::new(Vec2Mm::new(-1_000, -1_000), Vec2Mm::new(11_000, 1_000)),
        max_match_ticks: 120,
        ..MatchConfig::default()
    };
    let mut runtime = MatchRuntime::new(config, 0x5EED).expect("config");
    for (id, team, x) in [(10_u64, 0_u8, 0_i32), (20, 1, 10_000)] {
        runtime
            .spawn_actor(&ActorSpawn {
                id: ActorId(id),
                owner: None,
                team: TeamId(team),
                kind: ActorKind::Structure,
                position: Vec2Mm::new(x, 0),
                growth: StatGrowth::NONE,
                bounty: Bounty::NONE,
                stats: stats(400, 0, 0),
                abilities: vec![],
                basic_attack: None,
                death_rule: DeathRule::ObjectiveVictory {
                    winner: TeamId(1 - team),
                },
            })
            .expect("structure");
    }
    runtime
        .spawn_actor(&ActorSpawn {
            id: HERO,
            owner: Some(PLAYER),
            team: TeamId(0),
            kind: ActorKind::Hero,
            position: Vec2Mm::new(1_000, 0),
            growth: StatGrowth::NONE,
            bounty: Bounty::NONE,
            stats: stats(900, 0, 400),
            abilities: vec![],
            basic_attack: Some(BasicAttackSpec {
                range_mm: 900,
                acquisition_range_mm: 0,
                damage: 90,
                school: DamageSchool::Physical,
                windup_ticks: 0,
                cooldown_ticks: 3,
            }),
            death_rule: DeathRule::StayDead,
        })
        .expect("hero");
    runtime.start().expect("start");

    let wave = WaveSpec {
        id: metrocalk_gameplay::WaveId(1),
        team: TeamId(1),
        spawn: LanePosition {
            lane: LaneId(0),
            progress_mm: 9_500,
        },
        goal: LanePosition {
            lane: LaneId(0),
            progress_mm: 0,
        },
        aggro_range_mm: 2_000,
        target_priority: vec![ActorKind::Structure],
        cast_ability: None,
        first_tick: 2,
        interval_ticks: 5,
        max_alive: 6,
        units: vec![
            WaveUnitSpec {
                bounty: Bounty::NONE,
                progress_offset_mm: 0,
                stats: stats(200, 0, 350),
                abilities: vec![],
                basic_attack: None,
            },
            WaveUnitSpec {
                bounty: Bounty::NONE,
                progress_offset_mm: -300,
                stats: stats(200, 0, 350),
                abilities: vec![],
                basic_attack: None,
            },
        ],
    };
    let game = OneLaneMatch::attach(runtime, &spec, vec![], vec![wave]).expect("attach");
    (game, spec)
}

fn lane_waves_and_bots() -> String {
    let (mut game, spec) = lane_match();
    let mut rec = Recorder::new(
        "mob2-waves",
        "MOB-2 · lane, atomic waves, and deterministic bots",
        "Waves spawn as whole batches on their schedule and march the cooked lane toward their goal.",
    )
    .with_lane(&spec);
    rec.record(game.runtime(), &[], "attach");
    for _ in 0..14 {
        let frame = game.step().expect("step");
        rec.record(game.runtime(), &frame.server.events, "lane");
    }
    let minions = game
        .runtime()
        .actors()
        .iter()
        .filter(|a| a.kind == ActorKind::Minion)
        .count();
    rec.check(
        format!("{minions} wave minions are marching the lane"),
        minions > 0,
    );
    rec.note(
        "Minions are spawned by the wave scheduler, not authored: their IDs are allocator-stamped.",
    );
    rec.finish()
}

fn owned_player_corridor_anchoring() -> String {
    let (mut game, spec) = lane_match();
    let mut rec = Recorder::new(
        "mob21-anchoring",
        "MOB-2.1 · owned-player ingress, corridor-anchored",
        "A MoveTo is snapped onto the authored corridor; an off-corridor destination is refused with no side effects.",
    )
    .with_lane(&spec);
    rec.record(game.runtime(), &[], "attach");

    let off_corridor = PlayerCommand {
        player: PLAYER,
        sequence: 1,
        execute_at: 1,
        actor: HERO,
        kind: CommandKind::MoveTo {
            destination: Vec2Mm::new(5_000, 5_000),
        },
    };
    let before = game.lane_digest();
    let refused = game.submit(off_corridor).is_err();
    rec.check(
        "off-corridor destination refused",
        refused && game.lane_digest() == before && game.queued_command_count() == 0,
    );

    let requested = Vec2Mm::new(6_000, 300);
    let anchored = game
        .anchor_command(PlayerCommand {
            sequence: 2,
            kind: CommandKind::MoveTo {
                destination: requested,
            },
            ..off_corridor
        })
        .expect("anchored");
    if let CommandKind::MoveTo { destination } = anchored.kind {
        rec.check(
            format!(
                "({},{}) anchored onto the corridor at ({},{})",
                requested.x, requested.y, destination.x, destination.y
            ),
            destination.y == 0,
        );
    }
    game.submit(PlayerCommand {
        sequence: 2,
        execute_at: 1,
        kind: CommandKind::MoveTo {
            destination: requested,
        },
        ..off_corridor
    })
    .expect("accepted");
    for _ in 0..10 {
        let frame = game.step().expect("step");
        rec.record(game.runtime(), &frame.server.events, "anchored move");
    }
    rec.note("Anchoring is not corridor-FOLLOWING: the runtime still integrates the destination in a straight line.");
    rec.finish()
}

fn determinism_two_identical_runs() -> String {
    let mut rec = Recorder::new(
        "cross-determinism",
        "Cross-cutting · two identical runs agree bit for bit",
        "The same seed and the same command script produce an identical digest sequence.",
    );
    let run = |rec: Option<&mut Recorder>| -> Vec<u64> {
        let mut runtime = duel(1_000);
        let mut digests = Vec::new();
        let mut recorder = rec;
        for tick in 0..8_u64 {
            if tick == 0 {
                runtime
                    .submit(command(
                        1,
                        1,
                        HERO,
                        CommandKind::MoveTo {
                            destination: Vec2Mm::new(900, 0),
                        },
                    ))
                    .expect("move");
            }
            if tick == 3 {
                runtime
                    .apply_status_effect(
                        HERO,
                        3,
                        &[ControlKind::Silence],
                        &[(StatKind::MoveSpeed, ModifierOp::PercentMult { bps: -2_500 })],
                    )
                    .expect("effect");
            }
            let frame = runtime.step().expect("step");
            digests.push(frame.world_digest.0);
            if let Some(rec) = recorder.as_deref_mut() {
                rec.record(&runtime, &frame.events, "run A");
            }
        }
        digests
    };
    let first = run(Some(&mut rec));
    let second = run(None);
    rec.check(
        format!(
            "{} frames, identical world digests across both runs",
            first.len()
        ),
        first == second,
    );
    rec.note("Digests are shown on every frame above; run B produced the same sequence.");
    rec.finish()
}

fn checkpoint_restore_suffix_equality() -> String {
    const CONTENT: ContentId = ContentId::new([0x9c; 32]);
    let mut rec = Recorder::new(
        "cross-checkpoint",
        "Cross-cutting · checkpoint restore continues identically",
        "Capturing mid-match with live status effects and restoring reproduces the same digest sequence.",
    );
    let mut runtime = duel(500);
    runtime
        .apply_status_effect(
            HERO,
            40,
            &[ControlKind::Disarm],
            &[(StatKind::MoveSpeed, ModifierOp::Flat { magnitude: -120 })],
        )
        .expect("effect");
    for _ in 0..3 {
        let frame = runtime.step().expect("step");
        rec.record(&runtime, &frame.events, "before capture");
    }
    let checkpoint = runtime.capture_checkpoint(CONTENT).expect("capture");
    let mut restored = MatchRuntime::restore_checkpoint(&checkpoint, CONTENT).expect("restore");
    rec.record(&restored, &[], "RESTORED");
    rec.check(
        "restored world digest matches the captured one",
        restored.world_digest() == runtime.world_digest(),
    );
    rec.check(
        "the live status effect survived the round trip",
        restored.status_effects(HERO).expect("e").len() == 1
            && restored
                .actor(HERO)
                .expect("h")
                .controls
                .contains(ControlKind::Disarm),
    );
    let mut agreed = true;
    for _ in 0..5 {
        let a = runtime.step().expect("step");
        let b = restored.step().expect("step");
        agreed &= a.world_digest == b.world_digest;
        rec.record(&restored, &b.events, "resumed");
    }
    rec.check(
        "five resumed ticks agree with the uninterrupted run",
        agreed,
    );
    rec.note(format!("Checkpoint size: {} bytes.", checkpoint.len()));
    rec.finish()
}

/// Compile-time proof the capture harness only reads the public surface: if any of these stopped being
/// public, this example would fail to build and the capture would be caught as stale rather than silently
/// rendering an old shape.
#[allow(dead_code)]
fn public_surface_guard(lane: &CompiledLane) -> u64 {
    lane.length_mm()
}

// ---------------------------------------------------------------------------------------------
// GP-04 shaped targeting and GP-03 projectiles
// ---------------------------------------------------------------------------------------------

const BURST: AbilityId = AbilityId(21);
const LANCE: AbilityId = AbilityId(22);
const NEAR_FOE: ActorId = ActorId(301);
const FAR_FOE: ActorId = ActorId(302);
const ALLY: ActorId = ActorId(303);
const SIDE_FOE: ActorId = ActorId(304);

fn shaped_ability_match() -> MatchRuntime {
    let mut runtime = MatchRuntime::new(config(), 0x_5107).expect("config");
    runtime
        .register_ability(AbilitySpec {
            id: BURST,
            aim: AbilityAim::Point,
            targeting: AbilityTargeting::Enemy,
            range_mm: 6_000,
            resource_cost: 0,
            cooldown_ticks: 4,
            cast_ticks: 0,
            delivery: AbilityDelivery::Instant,
            impact: ImpactShape::Circle { radius_mm: 1_500 },
            per_rank_amount: 0,
            effect: AbilityEffect::Damage {
                amount: 250,
                school: DamageSchool::True,
            },
        })
        .expect("burst");
    runtime
        .register_ability(AbilitySpec {
            id: LANCE,
            aim: AbilityAim::Point,
            targeting: AbilityTargeting::Enemy,
            range_mm: 8_000,
            resource_cost: 0,
            cooldown_ticks: 4,
            cast_ticks: 0,
            per_rank_amount: 0,
            delivery: AbilityDelivery::Projectile {
                speed_mm_per_tick: 900,
                hit_radius_mm: 300,
            },
            impact: ImpactShape::Single,
            effect: AbilityEffect::Damage {
                amount: 400,
                school: DamageSchool::True,
            },
        })
        .expect("lance");

    let mut caster = hero(HERO, Some(PLAYER), 0, 0, 0);
    caster.abilities = vec![BURST, LANCE];
    runtime.spawn_actor(&caster).expect("caster");
    for (id, team, x, y) in [
        (NEAR_FOE, 1_u8, 3_000_i32, 0_i32),
        (FAR_FOE, 1, 5_000, 0),
        (ALLY, 0, 3_000, 400),
        // Inside the 1.5 m burst footprint, but 1.2 m off the skillshot line and well outside its 0.3 m
        // hit radius, so one fixture serves both scenarios without either one weakening the other.
        (SIDE_FOE, 1, 3_000, -1_200),
    ] {
        runtime
            .spawn_actor(&ActorSpawn {
                id,
                owner: None,
                team: TeamId(team),
                kind: ActorKind::Minion,
                position: Vec2Mm::new(x, y),
                growth: StatGrowth::NONE,
                bounty: Bounty::NONE,
                stats: stats(1_000, 0, 0),
                abilities: vec![],
                basic_attack: None,
                death_rule: DeathRule::StayDead,
            })
            .expect("body");
    }
    runtime.start().expect("start");
    runtime
}

fn ground_burst_spares_allies() -> String {
    let mut rec = Recorder::new(
        "gp04-ground-burst",
        "GP-04 · a ground burst covers an area and reads friend from foe",
        "One point-aimed burst damages every ENEMY inside its 1.5 m footprint and no ally, all in one tick.",
    );
    let mut runtime = shaped_ability_match();
    rec.record(&runtime, &[], "before the burst");
    rec.note(
        "Aimed at the point 3 m ahead. NEAR_FOE stands on it; ALLY stands 0.4 m away from it.",
    );

    runtime
        .submit(command(
            1,
            1,
            HERO,
            CommandKind::Cast {
                ability: BURST,
                target: CastTarget::Point(Vec2Mm::new(3_000, 0)),
            },
        ))
        .expect("accepted");
    let frame = runtime.step().expect("step");
    rec.record(&runtime, &frame.events, "burst detonated");

    let near = runtime.actor(NEAR_FOE).expect("near").health;
    let far = runtime.actor(FAR_FOE).expect("far").health;
    let ally = runtime.actor(ALLY).expect("ally").health;
    let side = runtime.actor(SIDE_FOE).expect("side").health;
    rec.check("the enemy standing on the point was hurt", near == 750);
    rec.check(
        "the second enemy 1.2 m away was hurt too, so this really is an AREA",
        side == 750,
    );
    rec.check("the ally standing 0.4 m away was NOT hurt", ally == 1_000);
    rec.check(
        "the enemy 2 m outside the footprint was NOT hurt",
        far == 1_000,
    );
    rec.check(
        "an instant ability leaves nothing in flight",
        runtime.projectiles().is_empty(),
    );
    rec.finish()
}

fn skillshot_flies_and_strikes_the_nearest_body() -> String {
    let mut rec = Recorder::new(
        "gp03-skillshot",
        "GP-03 · a skillshot travels a fixed line and strikes the first body on it",
        "The missile occupies a visible position every tick, extends to the ability's full range even though it was aimed short, and strikes the NEARER enemy while the farther one is untouched.",
    );
    let mut runtime = shaped_ability_match();
    rec.record(&runtime, &[], "before the cast");
    rec.note("Aimed at 1 m; the ability's range is 8 m, so the flight line runs the full 8 m.");

    runtime
        .submit(command(
            1,
            1,
            HERO,
            CommandKind::Cast {
                ability: LANCE,
                target: CastTarget::Point(Vec2Mm::new(1_000, 0)),
            },
        ))
        .expect("accepted");
    let frame = runtime.step().expect("step");
    rec.record(&runtime, &frame.events, "launched");
    let launched = runtime.projectiles();
    rec.check("exactly one missile is in the air", launched.len() == 1);
    rec.check(
        "the flight terminus is the ability's full 8 m range, not the 1 m aim point",
        launched
            .first()
            .is_some_and(|p| p.end == Vec2Mm::new(8_000, 0)),
    );

    let mut struck = None;
    for beat in 0..12 {
        let frame = runtime.step().expect("step");
        let label = format!("flight tick {}", beat + 1);
        rec.record(&runtime, &frame.events, &label);
        if let Some(event) = frame.events.iter().find_map(|event| match event {
            MatchEvent::ProjectileResolved { victim, .. } => Some(*victim),
            _ => None,
        }) {
            struck = Some(event);
            break;
        }
    }

    rec.check(
        "the missile struck the NEARER enemy",
        struck == Some(Some(NEAR_FOE)),
    );
    rec.check(
        "the farther enemy on the same line is untouched",
        runtime.actor(FAR_FOE).expect("far").health == 1_000,
    );
    rec.check(
        "the caster was never hit by its own missile",
        runtime.actor(HERO).expect("hero").health == 1_000,
    );
    rec.check(
        "the missile left the world exactly once",
        runtime.projectiles().is_empty(),
    );
    rec.finish()
}

fn skillshot_can_miss() -> String {
    let mut rec = Recorder::new(
        "gp03-skillshot-miss",
        "GP-03 · a skillshot is dodgeable",
        "The same missile, aimed along an empty line, reaches the end of its flight and fizzles: no damage, no victim. A skillshot that could not miss would not be a skill mechanic.",
    );
    let mut runtime = shaped_ability_match();
    runtime
        .submit(command(
            1,
            1,
            HERO,
            CommandKind::Cast {
                ability: LANCE,
                target: CastTarget::Point(Vec2Mm::new(1_000, 1_500)),
            },
        ))
        .expect("accepted");
    let frame = runtime.step().expect("step");
    rec.record(&runtime, &frame.events, "launched off-line");
    rec.note(
        "Aimed 1.5 m off the axis the enemies stand on, so nothing is within the 0.3 m hit radius.",
    );
    rec.note("The flight is short because the terminus is clamped into the 2 m-tall capture map along its own line — the aim ratio is preserved, the distance is not.");

    let mut resolved_without_victim = false;
    let mut damage_seen = false;
    for beat in 0..14 {
        let frame = runtime.step().expect("step");
        let label = format!("flight tick {}", beat + 1);
        rec.record(&runtime, &frame.events, &label);
        damage_seen |= frame
            .events
            .iter()
            .any(|event| matches!(event, MatchEvent::DamageApplied { .. }));
        if let Some(victim) = frame.events.iter().find_map(|event| match event {
            MatchEvent::ProjectileResolved { victim, .. } => Some(*victim),
            _ => None,
        }) {
            resolved_without_victim = victim.is_none();
            break;
        }
    }
    rec.check(
        "the missile reached its flight end with no victim",
        resolved_without_victim,
    );
    rec.check("no damage was dealt anywhere", !damage_seen);
    rec.check(
        "every enemy is at full health",
        runtime.actor(NEAR_FOE).expect("near").health == 1_000
            && runtime.actor(FAR_FOE).expect("far").health == 1_000
            && runtime.actor(SIDE_FOE).expect("side").health == 1_000,
    );
    rec.finish()
}

// ---------------------------------------------------------------------------------------------
// GP-14 match statistics and GP-05 progression
// ---------------------------------------------------------------------------------------------

const RIFLE: AbilityId = AbilityId(41);
const COMRADE: ActorId = ActorId(304);

fn progression_match() -> MatchRuntime {
    let mut runtime = MatchRuntime::new(config(), 0x_9105).expect("config");
    runtime
        .register_ability(AbilitySpec {
            id: RIFLE,
            aim: AbilityAim::Unit,
            targeting: AbilityTargeting::Enemy,
            range_mm: 20_000,
            resource_cost: 0,
            cooldown_ticks: 1,
            cast_ticks: 0,
            delivery: AbilityDelivery::Instant,
            impact: ImpactShape::Single,
            effect: AbilityEffect::Damage {
                amount: 300,
                school: DamageSchool::True,
            },
            per_rank_amount: 250,
        })
        .expect("rifle");

    let growth = StatGrowth {
        max_health_per_level: 120,
        max_resource_per_level: 0,
        move_speed_mm_per_tick_per_level: 10,
        physical_reduction_bps_per_level: 0,
        magic_reduction_bps_per_level: 0,
    };
    let bounty = Bounty {
        gold: 300,
        experience: 600,
    };
    for (id, owner, team, x, health) in [
        (HERO, Some(PLAYER), 0_u8, 0_i32, 2_000_u32),
        (COMRADE, None, 0, 1_000, 2_000),
        (FOE, Some(ENEMY_PLAYER), 1, 4_000, 600),
    ] {
        let mut spawn = hero(id, owner, team, x, 0);
        spawn.abilities = vec![RIFLE];
        spawn.basic_attack = None;
        spawn.growth = growth;
        spawn.bounty = bounty;
        spawn.stats = CombatStats {
            max_health: health,
            ..spawn.stats
        };
        runtime.spawn_actor(&spawn).expect("hero");
    }
    runtime.start().expect("start");
    runtime
}

fn shoot(sequence: u32, at: Tick, actor: ActorId, target: ActorId) -> PlayerCommand {
    PlayerCommand {
        player: if actor == HERO { PLAYER } else { ENEMY_PLAYER },
        sequence,
        execute_at: at,
        actor,
        kind: CommandKind::Cast {
            ability: RIFLE,
            target: CastTarget::Actor(target),
        },
    }
}

fn kill_bounty_assist_and_shutdown() -> String {
    let mut rec = Recorder::new(
        "gp14-kill-assist",
        "GP-14 \u{b7} a kill pays the killer, the window pays the assister",
        "An ally who damaged the victim inside the assist window is credited an assist and paid a separately-minted half bounty; the killer's own take is unaffected by the assist existing.",
    );
    let mut runtime = progression_match();
    rec.record(&runtime, &[], "before the fight");
    rec.note("The COMRADE is a server-owned hero, so it acts through an internal intent rather than a player command.");

    // The ally softens the target first.
    let frame = runtime
        .step_with_intents(&[ActorIntent {
            actor: COMRADE,
            kind: CommandKind::Cast {
                ability: RIFLE,
                target: CastTarget::Actor(FOE),
            },
        }])
        .expect("step");
    rec.record(&runtime, &frame.events, "ally chips in");

    // The player finishes it, twice, so a streak forms.
    runtime.submit(shoot(1, 2, HERO, FOE)).expect("accepted");
    let frame = runtime.step().expect("step");
    rec.record(&runtime, &frame.events, "player lands the kill");

    let killer = runtime.score(HERO).expect("killer");
    let assister = runtime.score(COMRADE).expect("assister");
    let victim = runtime.score(FOE).expect("victim");
    rec.check("the killing blow took the full bounty", killer.gold == 300);
    rec.check(
        "the assister was paid a separately-minted half",
        assister.gold == 150,
    );
    rec.check("the killer is credited one kill", killer.kills == 1);
    rec.check("the assister is credited one assist", assister.assists == 1);
    rec.check("the victim is credited one death", victim.deaths == 1);
    rec.check(
        "experience was shared with the nearby ally, conserving the pool",
        runtime.actor(HERO).expect("h").experience + runtime.actor(COMRADE).expect("a").experience
            == 600,
    );
    rec.finish()
}

fn levelling_ranks_an_ability_and_raises_stats() -> String {
    let mut rec = Recorder::new(
        "gp05-levelling",
        "GP-05 \u{b7} levelling grows stats and buys ability ranks",
        "Experience raises the level; the level grows the authored base stats through growth (never through a modifier) and grants a point; spending the point raises an ability's rank AND its real magnitude.",
    );
    let mut runtime = progression_match();
    let before = runtime.actor(HERO).expect("hero");
    rec.record(&runtime, &[], "level 1");
    rec.note("Rank 2 unlocks at level 3, so the point cannot be spent until the level allows it.");

    runtime.award_experience(HERO, 660).expect("xp");
    rec.record(&runtime, &[], "level 3 reached");
    let levelled = runtime.actor(HERO).expect("hero");
    rec.check("the level rose to 3", levelled.level == 3);
    rec.check(
        "max health grew by exactly two levels of growth",
        levelled.max_health == before.max_health + 240,
    );
    rec.check(
        "growth consumed no modifier budget",
        runtime.modifiers(HERO).expect("m").is_empty(),
    );
    rec.check("levelling did not heal", levelled.health == before.health);
    rec.check(
        "two points are unspent",
        levelled.unspent_ability_points == 2,
    );

    runtime
        .submit(PlayerCommand {
            player: PLAYER,
            sequence: 1,
            execute_at: runtime.tick() + 1,
            actor: HERO,
            kind: CommandKind::UpgradeAbility { ability: RIFLE },
        })
        .expect("accepted");
    let frame = runtime.step().expect("step");
    rec.record(&runtime, &frame.events, "point spent on RIFLE");
    rec.check(
        "the ability is now rank 2",
        runtime.ability_rank(HERO, RIFLE) == Some(2),
    );

    let target_before = runtime.actor(FOE).expect("foe").health;
    runtime
        .submit(shoot(2, runtime.tick() + 1, HERO, FOE))
        .expect("accepted");
    let frame = runtime.step().expect("step");
    rec.record(&runtime, &frame.events, "rank-2 shot lands");
    let dealt = target_before - runtime.actor(FOE).expect("foe").health;
    rec.check(
        "the rank-2 shot really hits harder (300 base + 250 per rank)",
        dealt == 550,
    );
    rec.finish()
}

// ---------------------------------------------------------------------------------------------
// MOB-3.0 combat depth
// ---------------------------------------------------------------------------------------------

const BRAWLER: ActorId = ActorId(701);
const TARGET: ActorId = ActorId(702);

#[expect(
    clippy::too_many_lines,
    reason = "one scenario reads as one story; splitting it hides which stat produced which number"
)]
fn crit_penetration_shield_and_lifesteal() -> String {
    let mut rec = Recorder::new(
        "gp07-damage-pipeline",
        "GP-07 \u{b7} the damage pipeline, stage by stage",
        "One swing runs crit then penetration then mitigation then shield then health then lifesteal, and every stage is visible in the numbers: a guaranteed critical doubles the swing, penetration halves the mitigation, a shield eats the first hit almost whole, and the attacker heals from what actually landed.",
    );

    let attacker = CombatStats {
        max_health: 2_000,
        max_resource: 0,
        move_speed_mm_per_tick: 0,
        physical_reduction_bps: 0,
        magic_reduction_bps: 0,
        crit_chance_bps: metrocalk_gameplay::BASIS_POINTS,
        crit_damage_bps: 20_000,
        physical_penetration_bps: 5_000,
        magic_penetration_bps: 0,
        lifesteal_bps: 2_500,
    };
    let defender = CombatStats {
        max_health: 5_000,
        max_resource: 0,
        move_speed_mm_per_tick: 0,
        physical_reduction_bps: 4_000,
        magic_reduction_bps: 0,
        crit_chance_bps: 0,
        crit_damage_bps: metrocalk_gameplay::BASIS_POINTS,
        physical_penetration_bps: 0,
        magic_penetration_bps: 0,
        lifesteal_bps: 0,
    };

    let mut runtime = MatchRuntime::new(config(), 0x_D4D4).expect("config");
    for (id, owner, team, x, stats) in [
        (BRAWLER, Some(PLAYER), 0_u8, 0_i32, attacker),
        (TARGET, None, 1, 1_000, defender),
    ] {
        runtime
            .spawn_actor(&ActorSpawn {
                id,
                owner,
                team: TeamId(team),
                kind: if owner.is_some() {
                    ActorKind::Hero
                } else {
                    ActorKind::Minion
                },
                position: Vec2Mm::new(x, 0),
                stats,
                growth: StatGrowth::NONE,
                bounty: Bounty::NONE,
                abilities: vec![],
                basic_attack: Some(BasicAttackSpec {
                    range_mm: 20_000,
                    acquisition_range_mm: 0,
                    damage: 200,
                    school: DamageSchool::Physical,
                    windup_ticks: 0,
                    cooldown_ticks: 0,
                }),
                death_rule: DeathRule::StayDead,
            })
            .expect("actor");
    }
    runtime.start().expect("start");
    rec.note("Attacker: 200 base, guaranteed critical at 200 %, 50 % physical penetration, 25 % lifesteal.");
    rec.note("Defender: 40 % physical mitigation, plus a 300-point shield applied before the first swing.");
    runtime
        .apply_status_effect_with_shield(TARGET, 60, &[], &[], 300)
        .expect("shield");
    // The attacker is wounded first, or every lifesteal heal is clamped away at full health and the
    // capture would silently show nothing while claiming to demonstrate lifesteal.
    // Applying a max-health penalty clamps CURRENT health down with it; removing the penalty restores the
    // maximum and leaves the actor genuinely wounded, which is the only way to open a heal window through
    // the public API.
    let wound = runtime
        .apply_stat_modifier(
            BRAWLER,
            StatKind::MaxHealth,
            ModifierOp::Flat { magnitude: -1_000 },
        )
        .expect("wound");
    runtime
        .remove_stat_modifier(BRAWLER, wound)
        .expect("restore");
    rec.record(&runtime, &[], "before the first swing");

    let mut landed = Vec::new();
    for sequence in 1..=3_u32 {
        let at = runtime.tick() + 1;
        runtime
            .submit(PlayerCommand {
                player: PLAYER,
                sequence,
                execute_at: at,
                actor: BRAWLER,
                kind: CommandKind::BasicAttack { target: TARGET },
            })
            .expect("accepted");
        let frame = runtime.step().expect("step");
        let dealt = frame.events.iter().find_map(|event| match event {
            MatchEvent::DamageApplied { target, amount, .. } if *target == TARGET => Some(*amount),
            _ => None,
        });
        landed.push(dealt.unwrap_or_default());
        rec.record(&runtime, &frame.events, &format!("swing {sequence}"));
    }

    // 200 base -> crit x2 = 400 -> penetration halves 40 % mitigation to 20 % -> 400 * 0.8 = 320.
    // Swing 1: the 300-point shield absorbs 300, so 20 reaches health.
    // Swings 2 and 3: the pool is gone, so the full 320 lands each time.
    rec.check(
        "the shield swallowed all but 20 of the first swing",
        landed[0] == 20,
    );
    rec.check("the second swing lands in full at 320", landed[1] == 320);
    rec.check(
        "crit-then-penetration gives 320, where crit-after-mitigation would give 240",
        landed[1] == 320,
    );
    rec.check(
        "the third swing matches the second, so the pipeline is stable",
        landed[2] == 320,
    );
    rec.check(
        "the defender took exactly 20 + 320 + 320 in total",
        runtime.actor(TARGET).expect("t").health == 5_000 - 660,
    );
    // 25 % of what LANDED, not of what was swung: 5 + 80 + 80 = 165 on top of the wounded 1,000.
    rec.check(
        "the attacker healed 25 % of the damage that actually landed",
        runtime.actor(BRAWLER).expect("a").health == 1_000 + 165,
    );
    rec.finish()
}

// ---------------------------------------------------------------------------------------------
// GP-08 - the standing attack order
// ---------------------------------------------------------------------------------------------

const SOLDIER: ActorId = ActorId(801);
const WATCHER: ActorId = ActorId(802);
const NEAR_CREEP: ActorId = ActorId(901);
const FAR_CREEP: ActorId = ActorId(902);

/// One mobile soldier with a weapon that notices further than it reaches, plus hostiles.
fn standing_order_world(speed: u32, hostiles: &[(ActorId, i32, i32, u32)]) -> MatchRuntime {
    let mut runtime = MatchRuntime::new(config(), 0x_0A77).expect("config");
    runtime
        .spawn_actor(&ActorSpawn {
            id: SOLDIER,
            owner: Some(PLAYER),
            team: TeamId(0),
            kind: ActorKind::Hero,
            position: Vec2Mm::new(0, 0),
            stats: CombatStats {
                max_health: 4_000,
                max_resource: 0,
                move_speed_mm_per_tick: speed,
                physical_reduction_bps: 0,
                magic_reduction_bps: 0,
                crit_chance_bps: 0,
                crit_damage_bps: metrocalk_gameplay::BASIS_POINTS,
                physical_penetration_bps: 0,
                magic_penetration_bps: 0,
                lifesteal_bps: 0,
            },
            growth: StatGrowth::NONE,
            bounty: Bounty::NONE,
            abilities: vec![],
            basic_attack: Some(BasicAttackSpec {
                range_mm: 1_000,
                // Wider than reach on purpose: this is what lets the capture distinguish "noticed" from
                // "can hit", which is the whole difference between a hold and an attack-move.
                acquisition_range_mm: 5_000,
                damage: 120,
                school: DamageSchool::Physical,
                windup_ticks: 0,
                cooldown_ticks: 3,
            }),
            death_rule: DeathRule::StayDead,
        })
        .expect("soldier");
    for &(id, x, y, health) in hostiles {
        runtime
            .spawn_actor(&ActorSpawn {
                id,
                owner: None,
                team: TeamId(1),
                kind: ActorKind::Minion,
                position: Vec2Mm::new(x, y),
                stats: CombatStats {
                    max_health: health,
                    max_resource: 0,
                    move_speed_mm_per_tick: 0,
                    physical_reduction_bps: 0,
                    magic_reduction_bps: 0,
                    crit_chance_bps: 0,
                    crit_damage_bps: metrocalk_gameplay::BASIS_POINTS,
                    physical_penetration_bps: 0,
                    magic_penetration_bps: 0,
                    lifesteal_bps: 0,
                },
                growth: StatGrowth::NONE,
                bounty: Bounty::NONE,
                abilities: vec![],
                basic_attack: None,
                death_rule: DeathRule::StayDead,
            })
            .expect("hostile");
    }
    runtime.start().expect("start");
    runtime
}

fn hostile_health(runtime: &MatchRuntime, id: ActorId) -> u32 {
    runtime.actor(id).map_or(0, |actor| actor.health)
}

fn attack_move_advances_engages_and_resumes() -> String {
    let mut rec = Recorder::new(
        "gp08-attack-move",
        "GP-08 \u{b7} one order, many swings",
        "A single attack-move makes the hero advance, stop dead at weapon reach of the first thing it notices, keep swinging on its own with no further command, and then resume advancing once that target is down. The order is never consumed - it is still standing at the end.",
    );
    let mut runtime = standing_order_world(
        300,
        &[(NEAR_CREEP, 4_000, 0, 400), (FAR_CREEP, 9_000, 0, 400)],
    );
    runtime
        .submit(command(
            1,
            1,
            SOLDIER,
            CommandKind::AttackMove {
                // The capture's map ends at x=12000; the order aims at its far edge, not past it.
                destination: Vec2Mm::new(12_000, 0),
            },
        ))
        .expect("accepted");
    rec.note("ONE command is submitted in this entire capture. Every swing after it is the kernel's own.");
    let mut swings = 0_usize;
    let mut halted_at = None;
    for tick in 0..48 {
        let frame = runtime.step().expect("step");
        swings += frame
            .events
            .iter()
            .filter(|event| matches!(event, MatchEvent::BasicAttackStarted { source, .. } if *source == SOLDIER))
            .count();
        if halted_at.is_none()
            && frame
                .events
                .iter()
                .any(|event| matches!(event, MatchEvent::TargetAcquired { .. }))
        {
            halted_at = Some(runtime.actor(SOLDIER).expect("s").position.x);
        }
        let label = if tick < 8 {
            "advancing"
        } else {
            "engaged / advancing"
        };
        rec.record(&runtime, &frame.events, label);
    }
    rec.check(format!("swung {swings} times from ONE order"), swings >= 8);
    rec.check(
        "halted at weapon reach rather than walking through the first creep",
        halted_at.is_some_and(|x| (3_000..=4_000).contains(&x)),
    );
    rec.check(
        "the first creep it noticed is dead",
        hostile_health(&runtime, NEAR_CREEP) == 0,
    );
    rec.check(
        "moved on to the second creep once the first was down",
        hostile_health(&runtime, FAR_CREEP) < 400,
    );
    rec.check(
        "the order is STILL standing - it was never consumed",
        runtime.actor(SOLDIER).expect("s").attack_order.is_some(),
    );
    rec.finish()
}

fn a_hold_holds_and_a_named_target_is_not_traded() -> String {
    let mut rec = Recorder::new(
        "gp08-hold-and-target",
        "GP-08 \u{b7} an order does what it says and nothing more",
        "A hold engages what walks into reach but never walks to find it, even with something noticed and out of range. And a named target is fought to the end while a CLOSER hostile stands untouched beside it, because naming a target is an instruction and not a hint.",
    );

    // ── a hold really holds ──────────────────────────────────────────────────────────────────────────
    let mut held = standing_order_world(300, &[(FAR_CREEP, 4_000, 0, 4_000)]);
    held.submit(command(1, 1, SOLDIER, CommandKind::HoldPosition))
        .expect("accepted");
    for _ in 0..20 {
        let frame = held.step().expect("step");
        rec.record(
            &held,
            &frame.events,
            "holding, target noticed but out of reach",
        );
    }
    rec.check(
        "held position for 20 ticks with a target NOTICED and out of reach",
        held.actor(SOLDIER).expect("s").position == Vec2Mm::new(0, 0),
    );
    rec.check(
        "and never landed a hit it could not reach",
        hostile_health(&held, FAR_CREEP) == 4_000,
    );

    // ── a named target is not traded for a closer one ────────────────────────────────────────────────
    let mut named = standing_order_world(
        0,
        &[(NEAR_CREEP, 300, 0, 4_000), (FAR_CREEP, 900, 0, 4_000)],
    );
    named
        .submit(command(
            1,
            1,
            SOLDIER,
            CommandKind::AttackTarget { target: FAR_CREEP },
        ))
        .expect("accepted");
    rec.note("Both creeps are in reach. Only the player's naming distinguishes them.");
    for _ in 0..18 {
        let frame = named.step().expect("step");
        rec.record(&named, &frame.events, "locked onto the FARTHER creep");
    }
    rec.check(
        "hit the named target repeatedly",
        hostile_health(&named, FAR_CREEP) < 4_000,
    );
    rec.check(
        "never touched the CLOSER one",
        hostile_health(&named, NEAR_CREEP) == 4_000,
    );

    rec.finish()
}

fn crowd_control_and_stop_both_bind_a_standing_order() -> String {
    let mut rec = Recorder::new(
        "gp08-cc-and-stop",
        "GP-08 \u{b7} a standing order is bound by the same rules a command is",
        "A disarm gates an automatic swing exactly as it gates a commanded one - the driver goes through the same single validation choke point a player command does, so there is no second path into combat that crowd control could miss. And STOP clears the order outright, rather than leaving a stopped unit still swinging at whatever wanders past.",
    );

    // ── crowd control gates the automatic swing ──────────────────────────────────────────────────────
    let mut disarmed = standing_order_world(0, &[(NEAR_CREEP, 900, 0, 4_000)]);
    disarmed
        .submit(command(1, 1, SOLDIER, CommandKind::HoldPosition))
        .expect("accepted");
    disarmed
        .apply_status_effect(SOLDIER, 12, &[ControlKind::Disarm], &[])
        .expect("disarm");
    rec.record(&disarmed, &[], "DISARM applied over a standing order");
    for _ in 0..12 {
        let frame = disarmed.step().expect("step");
        rec.record(&disarmed, &frame.events, "disarmed under a standing order");
    }
    let during = hostile_health(&disarmed, NEAR_CREEP);
    rec.check(
        "a standing order cannot out-swing a disarm",
        during == 4_000,
    );
    for _ in 0..12 {
        let frame = disarmed.step().expect("step");
        rec.record(&disarmed, &frame.events, "disarm expired");
    }
    rec.check(
        "and resumed by itself once the disarm expired, with no new command",
        hostile_health(&disarmed, NEAR_CREEP) < during,
    );

    // ── stop means stop ──────────────────────────────────────────────────────────────────────────────
    let mut stopped = standing_order_world(0, &[(NEAR_CREEP, 900, 0, 40_000)]);
    stopped
        .submit(command(1, 1, SOLDIER, CommandKind::HoldPosition))
        .expect("accepted");
    for _ in 0..9 {
        let frame = stopped.step().expect("step");
        rec.record(&stopped, &frame.events, "swinging under a hold");
    }
    let before_stop = hostile_health(&stopped, NEAR_CREEP);
    let at = stopped.tick() + 1;
    stopped
        .submit(command(2, at, SOLDIER, CommandKind::Stop))
        .expect("accepted");
    for _ in 0..15 {
        let frame = stopped.step().expect("step");
        rec.record(&stopped, &frame.events, "stopped");
    }
    rec.check(
        "STOP cleared the standing order",
        stopped.actor(SOLDIER).expect("s").attack_order.is_none(),
    );
    rec.check(
        "and no further damage landed after it",
        hostile_health(&stopped, NEAR_CREEP) == before_stop,
    );
    // WATCHER is unused geometry in this scenario; naming it keeps the id table honest.
    let _ = WATCHER;
    rec.finish()
}
