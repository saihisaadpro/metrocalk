//! Cook tests.
//!
//! Every scene here is built by committing ops through the **real** engine — the same path the editor's
//! inspector and gizmo use — so these exercise the authoring workflow rather than a hand-built fixture of
//! the cook's own input types. A test that constructed [`CookedMatch`] directly would prove the kernel
//! adapter and nothing about the thing that was actually missing.

use super::*;
use metrocalk_core::Op;
use metrocalk_ecs::FlecsWorld;

fn engine() -> Engine<FlecsWorld> {
    let world = FlecsWorld::new();
    let mut engine = Engine::new(world, 1);
    engine.clear_history();
    engine
}

/// A scene that cooks: the starter match, authored exactly as the shell authors it.
fn starter() -> Engine<FlecsWorld> {
    let mut engine = engine();
    author_starter_match(&mut engine).expect("author");
    engine
}

/// Overwrite one authored field on the single entity carrying `component`.
fn set_on(engine: &mut Engine<FlecsWorld>, component: &str, field: &str, value: FieldValue) {
    let id = authored_with(engine, component)
        .first()
        .expect("component present")
        .0;
    engine
        .commit(
            "test-edit",
            vec![Op::SetField {
                entity: id,
                component: component.to_owned(),
                field: field.to_owned(),
                value,
            }],
        )
        .expect("commit");
}

/// Overwrite one authored field on the nth entity carrying `component`, in authored order.
fn set_on_nth(
    engine: &mut Engine<FlecsWorld>,
    component: &str,
    nth: usize,
    field: &str,
    value: FieldValue,
) {
    let id = authored_with(engine, component)[nth].0;
    engine
        .commit(
            "test-edit",
            vec![Op::SetField {
                entity: id,
                component: component.to_owned(),
                field: field.to_owned(),
                value,
            }],
        )
        .expect("commit");
}

/// Move the nth entity carrying `component` — a position lives on `Transform`, not on the match
/// component, so this is what a gizmo drag actually commits.
fn move_nth(engine: &mut Engine<FlecsWorld>, component: &str, nth: usize, axis: &str, value: f64) {
    let id = authored_with(engine, component)[nth].0;
    engine
        .commit(
            "test-move",
            vec![Op::SetField {
                entity: id,
                component: "Transform".to_owned(),
                field: axis.to_owned(),
                value: FieldValue::Number(value),
            }],
        )
        .expect("commit");
}

fn cooked(engine: &Engine<FlecsWorld>) -> CookedMatch {
    let outcome = cook_match(engine);
    assert!(
        outcome.ok(),
        "expected a clean cook, got {:?}",
        outcome.diagnostics
    );
    outcome.cooked.expect("artifact")
}

// ── the workflow ─────────────────────────────────────────────────────────────────────────────────────

#[test]
fn the_starter_match_cooks_and_runs() {
    let engine = starter();
    let outcome = cook_match(&engine);
    assert!(
        outcome.ok(),
        "the scene the editor authors must cook on the first try: {:?}",
        outcome.diagnostics
    );
    assert!(
        outcome.diagnostics.is_empty(),
        "and without warnings: {:?}",
        outcome.diagnostics
    );

    let cooked = outcome.cooked.expect("artifact");
    assert_eq!(cooked.schema_version, MATCH_COOK_SCHEMA_VERSION);
    assert_eq!(cooked.actors.len(), 3);
    assert_eq!(cooked.waves.len(), 1);
    assert_eq!(cooked.lane.centerline, vec![(0, 0), (12_000, 0)]);
    assert_eq!(cooked.lane.half_width_mm, 900);

    // And the kernel actually accepts it.
    let mut game = cooked.build().expect("the kernel builds from the cook");
    for _ in 0..48 {
        game.step().expect("step");
    }
    assert_eq!(game.runtime().tick(), 48);
}

#[test]
fn authored_edits_reach_the_running_match() {
    // The whole point of the cook: change a number in the inspector, and the match changes.
    let mut engine = starter();
    let before = cooked(&engine);
    let hero_before = before
        .actors
        .iter()
        .find(|a| a.owned)
        .expect("a player hero")
        .clone();

    // The hero is the third authored actor; halve its authored speed.
    set_on_nth(
        &mut engine,
        MATCH_ACTOR,
        2,
        "speed",
        FieldValue::Number(3.9),
    );
    let after = cooked(&engine);
    let hero_after = after
        .actors
        .iter()
        .find(|a| a.owned)
        .expect("a player hero");

    assert_eq!(hero_before.move_speed_mm_per_tick, 260);
    assert_eq!(hero_after.move_speed_mm_per_tick, 130);
    assert_ne!(
        before.digest, after.digest,
        "an authored change that matters must change the cooked digest"
    );

    // …and the kernel sees the authored value, not a hard-coded one.
    let game = after.build().expect("build");
    let actors = game.runtime().actors();
    let hero = actors.iter().find(|a| a.owner.is_some()).expect("hero");
    assert_eq!(hero.move_speed_mm_per_tick, 130);
}

#[test]
fn moving_an_actor_with_the_gizmo_moves_it_in_the_match() {
    let mut engine = starter();
    // Authoring a position is an ordinary Transform edit — exactly what a gizmo drag commits.
    move_nth(&mut engine, MATCH_ACTOR, 2, "x", 4.25);
    let cooked = cooked(&engine);
    let hero = cooked.actors.iter().find(|a| a.owned).expect("hero");
    assert_eq!(hero.x_mm, 4_250);

    let game = cooked.build().expect("build");
    let actors = game.runtime().actors();
    let runtime_hero = actors.iter().find(|a| a.owner.is_some()).expect("hero");
    assert_eq!(runtime_hero.position.x, 4_250);
}

#[test]
fn every_cooked_actor_traces_back_to_its_authored_entity() {
    let engine = starter();
    let cooked = cooked(&engine);
    let authored: Vec<String> = authored_with(&engine, MATCH_ACTOR)
        .into_iter()
        .map(|(id, _)| id.to_loro_key())
        .collect();
    for actor in &cooked.actors {
        assert!(
            authored.contains(&actor.source),
            "cooked actor {} points at {} which is not an authored entity",
            actor.id,
            actor.source
        );
        assert_eq!(cooked.source_of(actor.id), Some(actor.source.as_str()));
    }
    // Ids are unique — a collision would silently merge two actors.
    let mut ids: Vec<u64> = cooked.actors.iter().map(|a| a.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), cooked.actors.len());
}

// ── determinism ──────────────────────────────────────────────────────────────────────────────────────

#[test]
fn cooking_is_deterministic() {
    let engine = starter();
    let first = cooked(&engine);
    for _ in 0..8 {
        let again = cooked(&engine);
        assert_eq!(
            first, again,
            "the same document must cook to the same bytes"
        );
        assert_eq!(first.digest, again.digest);
    }
}

#[test]
fn two_documents_authored_the_same_way_cook_identically() {
    // Different peer ids, so the entity keys differ — the artifact must not.
    let mut a = Engine::new(FlecsWorld::new(), 1);
    let mut b = Engine::new(FlecsWorld::new(), 7);
    author_starter_match(&mut a).expect("author");
    author_starter_match(&mut b).expect("author");
    let (ca, cb) = (cooked(&a), cooked(&b));
    assert_eq!(
        ca.digest, cb.digest,
        "the digest covers the definitions, not the document's identity"
    );
    assert_eq!(ca.actors.len(), cb.actors.len());
    // The traceability map does differ — it points into each document.
    assert_ne!(ca.actors[0].source, cb.actors[0].source);
}

#[test]
fn the_cook_never_mutates_the_document() {
    let mut engine = starter();
    let before: Vec<_> = engine
        .entity_ids()
        .into_iter()
        .map(|id| (id, engine.components_of(id)))
        .collect();
    let undo_depth_before = {
        // Cooking must not push anything undoable. If it did, one undo would take back a *user* edit.
        let _ = cook_match(&engine);
        engine.undo()
    };
    assert!(
        undo_depth_before,
        "the starter-match authoring commit is still the top of the undo stack"
    );
    // Undoing the authoring removed the match — proving the only thing on the stack was the author's own
    // transaction, never anything the cook added.
    assert!(!scene_has_match(&engine));

    engine.redo();
    let after: Vec<_> = engine
        .entity_ids()
        .into_iter()
        .map(|id| (id, engine.components_of(id)))
        .collect();
    assert_eq!(before.len(), after.len());
}

#[test]
fn digest_changes_only_when_the_definitions_change() {
    let mut engine = starter();
    let before = cooked(&engine).digest;

    // A rename is authoring metadata, not gameplay: the runtime definitions are identical.
    let settings_id = authored_with(&engine, MATCH_SETTINGS)[0].0;
    engine
        .commit(
            "rename",
            vec![Op::SetField {
                entity: settings_id,
                component: NAME_COMPONENT.to_owned(),
                field: NAME_FIELD.to_owned(),
                value: FieldValue::Str("Renamed".into()),
            }],
        )
        .expect("commit");
    assert_eq!(cooked(&engine).digest, before);

    // Widening the corridor is a gameplay change.
    set_on(
        &mut engine,
        MATCH_LANE,
        "halfWidth",
        FieldValue::Number(1.5),
    );
    assert_ne!(cooked(&engine).digest, before);
}

// ── units ────────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn metres_become_millimetres_and_speeds_become_per_tick() {
    let mut engine = starter();
    set_on(
        &mut engine,
        MATCH_LANE,
        "halfWidth",
        FieldValue::Number(1.234),
    );
    set_on_nth(
        &mut engine,
        MATCH_ACTOR,
        2,
        "speed",
        FieldValue::Number(9.0),
    );
    let cooked = cooked(&engine);
    assert_eq!(cooked.lane.half_width_mm, 1_234);
    // 9 m/s at 30 Hz = 300 mm per tick.
    let hero = cooked.actors.iter().find(|a| a.owned).expect("hero");
    assert_eq!(hero.move_speed_mm_per_tick, 300);
}

#[test]
fn a_whole_number_authored_as_an_integer_is_read_as_one() {
    // JSON round-trips a whole number to `Integer`, not `Number`. A reader matching only one arm
    // silently falls back to a default; this asserts both arms are handled.
    let mut engine = starter();
    set_on(&mut engine, MATCH_LANE, "halfWidth", FieldValue::Integer(2));
    assert_eq!(cooked(&engine).lane.half_width_mm, 2_000);
}

#[test]
fn a_position_authored_under_the_schema_alias_still_cooks() {
    // The document writes `x`/`y`/`z`; the registry schema declares `px`/`py`/`pz`. Both must work.
    let mut engine = starter();
    let hero = authored_with(&engine, MATCH_ACTOR)[2].0;
    engine
        .commit(
            "alias",
            vec![
                Op::RemoveField {
                    entity: hero,
                    component: "Transform".into(),
                    field: "x".into(),
                },
                Op::RemoveField {
                    entity: hero,
                    component: "Transform".into(),
                    field: "z".into(),
                },
                Op::SetField {
                    entity: hero,
                    component: "Transform".into(),
                    field: "px".into(),
                    value: FieldValue::Number(3.0),
                },
                Op::SetField {
                    entity: hero,
                    component: "Transform".into(),
                    field: "pz".into(),
                    value: FieldValue::Number(0.0),
                },
            ],
        )
        .expect("commit");
    let cooked = cooked(&engine);
    assert_eq!(cooked.actors.iter().find(|a| a.owned).unwrap().x_mm, 3_000);
}

#[test]
fn a_speed_too_small_to_move_is_warned_about_not_silently_zeroed() {
    let mut engine = starter();
    // 0.01 m/s at 30 Hz rounds to 0 mm/tick.
    set_on_nth(
        &mut engine,
        MATCH_ACTOR,
        2,
        "speed",
        FieldValue::Number(0.01),
    );
    let outcome = cook_match(&engine);
    assert!(
        outcome.ok(),
        "it still cooks — it is a warning, not a refusal"
    );
    assert!(outcome.codes().contains(&"speed-rounds-to-zero".to_owned()));
}

#[test]
fn acquisition_range_defaults_to_twice_reach_and_is_honoured_when_authored() {
    // The field arrived AFTER these scenes could be authored, so absent must mean a sensible default and
    // not a refusal. A default equal to reach would make a standing order look broken - the unit would
    // only ever engage what had already walked into its face - so the default is deliberately wider.
    let mut engine = starter();
    let defaulted = cooked(&engine);
    let attacker = defaulted
        .actors
        .iter()
        .find_map(|actor| actor.attack)
        .expect("the starter match authors an attacker");
    assert_eq!(
        attacker.acquisition_range_mm,
        attacker.range_mm * 2,
        "an unauthored acquisition range must default to twice reach"
    );

    // Authored, it is used verbatim. Set on every authored actor rather than guessing which one carries
    // the weapon: the ones without an attack ignore it, and the test stays correct if the starter changes.
    for index in 0..authored_with(&engine, MATCH_ACTOR).len() {
        set_on_nth(
            &mut engine,
            MATCH_ACTOR,
            index,
            "attackAcquisitionRange",
            FieldValue::Number(9.0),
        );
    }
    let authored = cooked(&engine);
    assert!(
        authored
            .actors
            .iter()
            .filter_map(|actor| actor.attack)
            .any(|attack| attack.acquisition_range_mm == 9_000),
        "an authored acquisition range must reach the artifact"
    );
}

#[test]
fn an_acquisition_range_shorter_than_reach_is_refused_by_name() {
    // The kernel refuses this pair outright. Catching it here means the author is told which FIELD to fix
    // instead of meeting a runtime error at start with no object attached to it.
    let mut engine = starter();
    for index in 0..authored_with(&engine, MATCH_ACTOR).len() {
        set_on_nth(
            &mut engine,
            MATCH_ACTOR,
            index,
            "attackAcquisitionRange",
            FieldValue::Number(0.1),
        );
    }
    let outcome = cook_match(&engine);
    assert!(!outcome.ok());
    let message = format!("{:?}", outcome.diagnostics);
    assert!(message.contains("attackAcquisitionRange"), "{message}");
    assert!(message.contains("attackRange"), "{message}");
}

#[test]
fn the_starter_hero_has_an_ability_and_it_reaches_the_kernel() {
    // The gap this closes: the cook emitted `abilities: vec![]` for EVERY actor and never called
    // `register_ability`, so the kernel's whole ability system — casts, projectiles, impact shapes,
    // ranks — was reachable only from tests. A cooked artifact that merely CARRIES an ability would
    // still leave that true, so this asserts all the way through to a built runtime.
    let engine = starter();
    let cooked = cooked(&engine);

    let hero = cooked
        .actors
        .iter()
        .find(|actor| actor.owned)
        .expect("the starter authors one owned hero");
    let ability = hero
        .ability
        .expect("the hero must have an authored ability");
    assert!(ability.damage > 0 && ability.range_mm > 0);
    // Authored as a travelling bolt, not an instant hit: a projectile can MISS, and that is the part of
    // the kernel a guaranteed hit would leave unexercised.
    assert!(
        ability.projectile_speed_mm_per_tick > 0,
        "the starter ability must be a projectile so the flight path is actually exercised"
    );

    // ...and the built runtime knows it, which is the claim that matters.
    let game = cooked.build().expect("the starter match builds");
    let registered = game
        .runtime()
        .ability(metrocalk_gameplay::AbilityId(ability.id))
        .expect("the cook must REGISTER the ability, not merely name it on the spawn");
    assert_eq!(registered.range_mm, ability.range_mm);
    assert!(matches!(
        registered.delivery,
        metrocalk_gameplay::AbilityDelivery::Projectile { .. }
    ));

    // And the hero's own spawn names it, or the actor could never cast it.
    let hero_actor = game
        .runtime()
        .actors()
        .into_iter()
        .find(|a| a.owner.is_some())
        .expect("the hero is in the running match");
    assert!(
        hero_actor
            .cooldowns
            .iter()
            .any(|c| c.ability == metrocalk_gameplay::AbilityId(ability.id)),
        "the hero must be equipped with the ability it was authored"
    );
}

#[test]
fn an_ability_that_deals_nothing_is_refused_by_name() {
    // Zero damage is the one value that means "you meant to delete this", so it is reported against the
    // FIELD rather than silently cooking an ability that can never do anything.
    let mut engine = starter();
    for index in 0..authored_with(&engine, MATCH_ACTOR).len() {
        set_on_nth(
            &mut engine,
            MATCH_ACTOR,
            index,
            "abilityDamage",
            FieldValue::Integer(0),
        );
    }
    let outcome = cook_match(&engine);
    assert!(!outcome.ok());
    assert!(format!("{:?}", outcome.diagnostics).contains("abilityDamage"));
}

// ── refusals ─────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn an_empty_scene_says_what_to_do_rather_than_failing_obscurely() {
    let engine = engine();
    let outcome = cook_match(&engine);
    assert!(!outcome.ok());
    assert_eq!(outcome.codes(), vec!["no-match-settings".to_owned()]);
    let message = &outcome.diagnostics[0].message;
    assert!(
        message.contains("Match Settings"),
        "the message must name the thing to add: {message}"
    );
}

#[test]
fn a_refusal_names_the_authored_entity_and_field() {
    let mut engine = starter();
    set_on_nth(
        &mut engine,
        MATCH_ACTOR,
        2,
        "health",
        FieldValue::Integer(0),
    );
    let outcome = cook_match(&engine);
    assert!(!outcome.ok());
    let hero_key = authored_with(&engine, MATCH_ACTOR)[2].0.to_loro_key();
    let diagnostic = outcome
        .errors()
        .find(|d| d.entity.as_deref() == Some(hero_key.as_str()))
        .expect("the refusal points at the hero the author can click");
    assert_eq!(diagnostic.component.as_deref(), Some(MATCH_ACTOR));
    assert_eq!(diagnostic.field.as_deref(), Some("health"));
    assert_eq!(diagnostic.code, "out-of-range");
}

#[test]
fn a_failed_cook_produces_no_artifact_at_all() {
    // Half-cooking would let a partially-updated runtime start.
    let mut engine = starter();
    set_on_nth(
        &mut engine,
        MATCH_ACTOR,
        2,
        "health",
        FieldValue::Integer(0),
    );
    assert!(cook_match(&engine).cooked.is_none());
}

#[test]
fn every_refusal_the_kernel_would_raise_is_caught_at_author_time() {
    // Each case: an authored edit, and the diagnostic code it must produce. If the cook let any of these
    // through, the author would instead see the kernel's anonymous `InvalidDefinition` string.
    type Case = (
        &'static str,
        Box<dyn Fn(&mut Engine<FlecsWorld>)>,
        &'static str,
    );
    let cases: Vec<Case> = vec![
        (
            "a lane with no width",
            Box::new(|e| set_on(e, MATCH_LANE, "halfWidth", FieldValue::Number(0.0))),
            "out-of-range",
        ),
        (
            "a wave spawning before the match starts",
            Box::new(|e| set_on(e, MATCH_WAVE, "firstTick", FieldValue::Integer(0))),
            "out-of-range",
        ),
        (
            "a wave with no interval",
            Box::new(|e| set_on(e, MATCH_WAVE, "intervalTicks", FieldValue::Integer(0))),
            "out-of-range",
        ),
        (
            "a batch larger than the live cap",
            Box::new(|e| set_on(e, MATCH_WAVE, "unitCount", FieldValue::Integer(99))),
            "wave-batch-exceeds-cap",
        ),
        (
            "a wave spawning past the end of the lane",
            Box::new(|e| set_on(e, MATCH_WAVE, "spawnProgress", FieldValue::Number(99.0))),
            "past-lane-end",
        ),
        (
            "a wave spawning before the start of the lane",
            Box::new(|e| set_on(e, MATCH_WAVE, "spawnProgress", FieldValue::Number(-1.0))),
            "out-of-range",
        ),
        (
            "armour that would make an actor invulnerable",
            Box::new(|e| set_on_nth(e, MATCH_ACTOR, 2, "armourBps", FieldValue::Integer(10_000))),
            "out-of-range",
        ),
        (
            "an actor standing outside the play area",
            Box::new(|e| move_nth(e, MATCH_ACTOR, 2, "x", 500.0)),
            "outside-play-area",
        ),
        (
            "an actor standing off the lane corridor",
            Box::new(|e| move_nth(e, MATCH_ACTOR, 2, "z", 1.8)),
            "outside-lane-corridor",
        ),
        (
            "an unknown role",
            Box::new(|e| {
                set_on_nth(e, MATCH_ACTOR, 2, "role", FieldValue::Str("wizard".into()));
            }),
            "unknown-role",
        ),
        (
            "a position that is not a number",
            Box::new(|e| move_nth(e, MATCH_ACTOR, 2, "x", f64::NAN)),
            "not-finite",
        ),
        (
            "a position past what millimetres can represent",
            Box::new(|e| move_nth(e, MATCH_ACTOR, 2, "x", 9.9e9)),
            "out-of-range",
        ),
        (
            "a play area with no width",
            Box::new(|e| set_on(e, MATCH_SETTINGS, "boundsMaxX", FieldValue::Number(-2.0))),
            "empty-bounds",
        ),
        (
            "a wave live cap beyond the actor budget",
            Box::new(|e| set_on(e, MATCH_WAVE, "maxAlive", FieldValue::Integer(1_000))),
            "actor-budget-exceeded",
        ),
    ];

    for (what, edit, expected) in cases {
        let mut engine = starter();
        edit(&mut engine);
        let outcome = cook_match(&engine);
        assert!(!outcome.ok(), "{what} must not cook");
        assert!(
            outcome.codes().contains(&expected.to_owned()),
            "{what}: expected `{expected}`, got {:?}",
            outcome.codes()
        );
    }
}

#[test]
fn a_scene_with_no_player_hero_is_refused() {
    let mut engine = starter();
    set_on_nth(
        &mut engine,
        MATCH_ACTOR,
        2,
        "owned",
        FieldValue::Bool(false),
    );
    let outcome = cook_match(&engine);
    assert!(!outcome.ok());
    assert!(outcome.codes().contains(&"no-player-hero".to_owned()));
}

#[test]
fn two_player_heroes_are_refused() {
    let mut engine = starter();
    // Make a core the player's too — a second owned actor.
    set_on_nth(&mut engine, MATCH_ACTOR, 0, "owned", FieldValue::Bool(true));
    let outcome = cook_match(&engine);
    assert!(!outcome.ok());
    assert!(outcome
        .codes()
        .contains(&"multiple-player-heroes".to_owned()));
    // …and specifically because a structure cannot be owned.
    assert!(outcome.codes().contains(&"owned-non-hero".to_owned()));
}

#[test]
fn a_second_match_settings_object_is_refused_by_name() {
    let mut engine = starter();
    author_starter_match(&mut engine).expect("author a second one");
    let outcome = cook_match(&engine);
    assert!(!outcome.ok());
    assert!(outcome
        .codes()
        .contains(&"duplicate-match-settings".to_owned()));
}

#[test]
fn a_lane_with_one_waypoint_is_refused() {
    let mut engine = starter();
    let second = authored_with(&engine, LANE_WAYPOINT)[1].0;
    engine
        .commit("delete", vec![Op::DeleteEntity { id: second }])
        .expect("commit");
    let outcome = cook_match(&engine);
    assert!(!outcome.ok());
    assert!(outcome.codes().contains(&"lane-too-short".to_owned()));
}

#[test]
fn two_waypoints_in_the_same_place_are_refused() {
    let mut engine = starter();
    move_nth(&mut engine, LANE_WAYPOINT, 1, "x", 0.0);
    let outcome = cook_match(&engine);
    assert!(!outcome.ok());
    assert!(outcome
        .codes()
        .contains(&"duplicate-waypoint-position".to_owned()));
}

#[test]
fn errors_sort_above_warnings() {
    let mut engine = starter();
    set_on_nth(
        &mut engine,
        MATCH_ACTOR,
        0,
        "speed",
        FieldValue::Number(5.0),
    ); // warning
    set_on_nth(
        &mut engine,
        MATCH_ACTOR,
        2,
        "health",
        FieldValue::Integer(0),
    ); // error
    let outcome = cook_match(&engine);
    let severities: Vec<CookSeverity> = outcome.diagnostics.iter().map(|d| d.severity).collect();
    let first_warning = severities
        .iter()
        .position(|s| *s == CookSeverity::Warning)
        .expect("a warning");
    assert!(
        severities[..first_warning]
            .iter()
            .all(|s| *s == CookSeverity::Error),
        "blocking problems must come first: {severities:?}"
    );
}

// ── the artifact ─────────────────────────────────────────────────────────────────────────────────────

#[test]
fn the_artifact_round_trips_through_json_unchanged() {
    let engine = starter();
    let cooked = cooked(&engine);
    let json = serde_json::to_string(&cooked).expect("serialize");
    let back: CookedMatch = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(
        cooked, back,
        "the artifact must be storable and inspectable"
    );
    assert_eq!(digest_of(&back), digest_of(&cooked));
}

#[test]
fn a_stale_schema_version_is_refused_rather_than_replayed() {
    let engine = starter();
    let mut cooked = cooked(&engine);
    cooked.schema_version = MATCH_COOK_SCHEMA_VERSION + 1;
    let Err(error) = cooked.build() else {
        panic!("a future artifact must not build")
    };
    assert!(error.contains("schema version"), "{error}");
}

#[test]
fn the_digest_is_not_part_of_what_it_covers() {
    let engine = starter();
    let mut cooked = cooked(&engine);
    let computed = digest_of(&cooked);
    cooked.digest = "tampered".into();
    assert_eq!(
        digest_of(&cooked),
        computed,
        "the digest must be over the definitions, not over itself"
    );
}

// ── geometry helpers ─────────────────────────────────────────────────────────────────────────────────

#[test]
fn integer_sqrt_is_the_exact_floor() {
    for value in 0_u128..2_000 {
        let root = integer_sqrt(value);
        assert!(root * root <= value, "{root}^2 > {value}");
        assert!(
            (root + 1) * (root + 1) > value,
            "{root} is not the floor of sqrt({value})"
        );
    }
    // And at the magnitudes millimetre coordinates actually reach.
    for value in [1_u128 << 40, (1 << 40) + 1, u128::from(u64::MAX)] {
        let root = integer_sqrt(value);
        assert!(root * root <= value);
        assert!((root + 1) * (root + 1) > value);
    }
}

#[test]
fn distance_to_a_segment_is_measured_perpendicular_and_clamped_at_the_ends() {
    let a = Vec2Mm::new(0, 0);
    let b = Vec2Mm::new(10_000, 0);
    // Beside the middle: the perpendicular distance.
    assert_eq!(distance_to_segment(a, b, Vec2Mm::new(5_000, 900)), 900);
    // On the line: zero.
    assert_eq!(distance_to_segment(a, b, Vec2Mm::new(5_000, 0)), 0);
    // Past the end: measured from the endpoint, not from the infinite line.
    assert_eq!(distance_to_segment(a, b, Vec2Mm::new(13_000, 0)), 3_000);
    // A degenerate segment falls back to point distance rather than dividing by zero.
    assert_eq!(distance_to_segment(a, a, Vec2Mm::new(0, 5_000)), 5_000);
}

// ── property-style sweeps ────────────────────────────────────────────────────────────────────────────

#[test]
fn any_authored_position_inside_the_corridor_cooks_and_builds() {
    // A deterministic sweep over the authored space rather than one sampled point: for every position on
    // the lane that an author could drag the hero to, the cook must succeed AND the kernel must accept it.
    // These two agreeing is the invariant that keeps the cook from being a rubber stamp.
    let mut checked = 0;
    for x_tenths in 0..=120 {
        for z_tenths in [-8_i64, -4, 0, 4, 8] {
            let mut engine = starter();
            #[allow(clippy::cast_precision_loss)]
            let x = f64::from(x_tenths) / 10.0;
            #[allow(clippy::cast_precision_loss)]
            let z = z_tenths as f64 / 10.0;
            move_nth(&mut engine, MATCH_ACTOR, 2, "x", x);
            move_nth(&mut engine, MATCH_ACTOR, 2, "z", z);
            let outcome = cook_match(&engine);
            assert!(
                outcome.ok(),
                "the hero at ({x}, {z}) is inside the corridor but did not cook: {:?}",
                outcome.diagnostics
            );
            outcome
                .cooked
                .expect("artifact")
                .build()
                .unwrap_or_else(|e| {
                    panic!("the kernel refused a cook the author was allowed: {e}")
                });
            checked += 1;
        }
    }
    assert_eq!(checked, 121 * 5);
}

#[test]
fn any_authored_position_outside_the_corridor_is_refused_by_the_cook_not_the_kernel() {
    // The mirror of the sweep above: everything the cook refuses, the kernel would also have refused —
    // so the cook is not inventing restrictions the runtime does not have.
    for z_tenths in [-30_i64, -20, -10, 10, 20, 30] {
        let mut engine = starter();
        #[allow(clippy::cast_precision_loss)]
        let z = z_tenths as f64 / 10.0;
        move_nth(&mut engine, MATCH_ACTOR, 2, "z", z);
        let outcome = cook_match(&engine);
        assert!(
            !outcome.ok(),
            "the hero at z={z} is off the lane and must be refused"
        );
        assert!(outcome
            .codes()
            .contains(&"outside-lane-corridor".to_owned()));
    }
}

#[test]
fn every_authored_speed_cooks_to_a_representable_per_tick_value() {
    for hundredths in 0..=2_000_u32 {
        let mut engine = starter();
        let speed = f64::from(hundredths) / 100.0;
        set_on_nth(
            &mut engine,
            MATCH_ACTOR,
            2,
            "speed",
            FieldValue::Number(speed),
        );
        let outcome = cook_match(&engine);
        assert!(
            outcome.ok(),
            "speed {speed} m/s must cook: {:?}",
            outcome.diagnostics
        );
        let hero_speed = outcome
            .cooked
            .expect("artifact")
            .actors
            .iter()
            .find(|a| a.owned)
            .expect("hero")
            .move_speed_mm_per_tick;
        let expected = (speed * 1_000.0 / 30.0).round();
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let expected = expected as u32;
        assert_eq!(hero_speed, expected, "speed {speed} m/s");
    }
}

#[test]
#[allow(clippy::too_many_lines)] // a flat adversarial table, not branching logic
fn no_authored_scene_can_make_the_cook_panic() {
    // Adversarial field values on every match component. The cook must always return an outcome —
    // never panic, never overflow, never divide by zero.
    let hostile = [
        FieldValue::Number(f64::NAN),
        FieldValue::Number(f64::INFINITY),
        FieldValue::Number(f64::NEG_INFINITY),
        FieldValue::Number(f64::MAX),
        FieldValue::Number(f64::MIN),
        FieldValue::Number(-0.0),
        FieldValue::Integer(i64::MAX),
        FieldValue::Integer(i64::MIN),
        FieldValue::Integer(-1),
        FieldValue::Bool(true),
        FieldValue::Str(String::new()),
        FieldValue::Str("\u{0}\u{feff}not a number".into()),
    ];
    let targets: [(&str, &[&str]); 4] = [
        (
            MATCH_SETTINGS,
            &[
                "boundsMinX",
                "boundsMinZ",
                "boundsMaxX",
                "boundsMaxZ",
                "tickRateHz",
                "maxTicks",
                "seed",
            ],
        ),
        (MATCH_LANE, &["halfWidth"]),
        (
            MATCH_ACTOR,
            &[
                "role",
                "team",
                "health",
                "speed",
                "armourBps",
                "attackRange",
                "attackDamage",
                "attackWindupTicks",
                "attackCooldownTicks",
                "respawnDelayTicks",
                "owned",
                "objective",
            ],
        ),
        (
            MATCH_WAVE,
            &[
                "team",
                "spawnProgress",
                "goalProgress",
                "aggroRange",
                "firstTick",
                "intervalTicks",
                "maxAlive",
                "unitCount",
                "unitSpacing",
                "unitHealth",
                "unitSpeed",
                "unitAttackRange",
                "unitAttackDamage",
                "unitAttackWindupTicks",
                "unitAttackCooldownTicks",
            ],
        ),
    ];

    let mut cooked_anyway = 0;
    let mut refused = 0;
    for (component, fields) in targets {
        for field in fields {
            for value in &hostile {
                let mut engine = starter();
                let id = authored_with(&engine, component)[0].0;
                engine
                    .commit(
                        "hostile",
                        vec![Op::SetField {
                            entity: id,
                            component: component.to_owned(),
                            field: (*field).to_owned(),
                            value: value.clone(),
                        }],
                    )
                    .expect("commit");
                let outcome = cook_match(&engine);
                // If it cooked, the kernel must accept it — a cook that produces an artifact the
                // runtime then rejects is worse than one that refuses up front.
                if let Some(artifact) = outcome.cooked {
                    artifact.build().unwrap_or_else(|e| {
                        panic!("{component}.{field} = {value:?} cooked but the kernel refused: {e}")
                    });
                    cooked_anyway += 1;
                } else {
                    assert!(
                        !outcome.diagnostics.is_empty(),
                        "{component}.{field} = {value:?} refused without saying why"
                    );
                    refused += 1;
                }
            }
        }
    }
    assert_eq!(cooked_anyway + refused, 35 * hostile.len());
    assert!(
        refused > 0,
        "the sweep must actually exercise the refusal paths"
    );
}

#[test]
fn unrelated_objects_in_the_scene_do_not_change_the_cook() {
    // A match is authored inside a real project, beside geometry, lights and whatever else is there. The
    // cook selects by component, so none of that may reach the runtime — and adding it must not perturb
    // the digest, or every unrelated edit would invalidate a cooked artifact.
    let engine = starter();
    let before = cooked(&engine);

    let mut busy = starter();
    let mut ops = Vec::new();
    for index in 0..2_000 {
        let id = busy.alloc_entity_id();
        ops.push(Op::CreateEntity { id, parent: None });
        for (field, value) in [("x", f64::from(index)), ("y", 1.0), ("z", 2.0)] {
            ops.push(Op::SetField {
                entity: id,
                component: "Transform".into(),
                field: field.into(),
                value: FieldValue::Number(value),
            });
        }
        ops.push(Op::SetField {
            entity: id,
            component: "MeshRenderer".into(),
            field: "mesh".into(),
            value: FieldValue::Str("mtkasset:prop".into()),
        });
    }
    busy.commit("bystanders", ops).expect("commit");

    let after = cooked(&busy);
    assert_eq!(
        before.digest, after.digest,
        "2,000 unrelated objects changed the cooked definitions"
    );
    assert_eq!(after.actors.len(), 3, "an unrelated object became an actor");
}
