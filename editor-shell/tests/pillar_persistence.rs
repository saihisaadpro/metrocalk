//! **Does the user's work survive a reload?** — a `.mtk` round-trip over everything the three pillars
//! write (bar item B7 from the production audit).
//!
//! The audit's finding was blunt: `project_save_open.rs` covers exactly one M10.3 HealthBar scene, so
//! not one field of any pillar had ever been proven to come back. That matters most for the newest
//! plumbing: `RuleData.any_of` and `RuleData.subject` ride their own Loro slots written this session,
//! and a rule field with no read-back is precisely the kind of thing that vanishes silently on open —
//! the game still loads, it just quietly stops being the game you authored.
//!
//! Each test below fails if a single field is dropped.

use std::path::PathBuf;

use metrocalk_core::rules::{Action, CompareOp, Condition, RuleData, RuleId, SUBJECT_ENTITY};
use metrocalk_core::{Engine, EntityId, FieldValue, Op, Registry};
use metrocalk_ecs::FlecsWorld;

use metrocalk_editor_shell::capscene::{CapResolver, CapScene};
use metrocalk_editor_shell::condition_intent::{
    add_clause_ops, build_clause, clauses_of, ClauseRequest,
};
use metrocalk_editor_shell::project;
use metrocalk_editor_shell::role_intent::{assign_role, role_of, ROLE_COMPONENT};

fn engine_with_resolver() -> (Engine<FlecsWorld>, CapScene) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    engine.set_capability_resolver(Box::new(CapResolver::from_scene(&scene)));
    (engine, scene)
}

fn stdlib() -> Registry<FlecsWorld> {
    let mut registry = Registry::new(FlecsWorld::new());
    for meta in metrocalk_core::stdlib::standard_components() {
        registry.register(meta).expect("stdlib registers");
    }
    for event in metrocalk_core::stdlib::standard_events() {
        registry.register_event(event);
    }
    for action in metrocalk_core::stdlib::standard_actions() {
        registry.register_action(action);
    }
    registry
}

fn spawn(engine: &mut Engine<FlecsWorld>, x: f64) -> EntityId {
    let id = engine.alloc_entity_id();
    engine
        .commit(
            "spawn",
            vec![
                Op::CreateEntity { id, parent: None },
                Op::SetField {
                    entity: id,
                    component: "Transform".into(),
                    field: "x".into(),
                    value: FieldValue::Number(x),
                },
            ],
        )
        .expect("spawns");
    id
}

fn temp_mtk(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("metrocalk-pillar-{}-{tag}.mtk", std::process::id()))
}

#[test]
fn a_role_its_tuned_values_and_its_only_if_clauses_all_survive_save_and_open() {
    let path = temp_mtk("roles");
    let _ = std::fs::remove_file(&path);

    let (mut a, scene) = engine_with_resolver();
    let registry = stdlib();
    let coin = spawn(&mut a, 1.0);
    let key = spawn(&mut a, 3.0);
    assign_role(&mut a, &scene, &registry, coin, "collectible").expect("coin role");
    assign_role(&mut a, &scene, &registry, key, "collectible").expect("key role");

    // A tuned value the user typed (Round 6) — this is the field that was inert until Round 7.
    a.commit(
        "tune",
        vec![Op::SetField {
            entity: coin,
            component: ROLE_COMPONENT.into(),
            field: "points".into(),
            value: FieldValue::Integer(10),
        }],
    )
    .expect("tune commits");

    // An authored "only if" clause (Round 7).
    let clause = build_clause(
        &a,
        &ClauseRequest {
            kind: "other_gone".into(),
            object: Some(key.to_loro_key()),
            ..ClauseRequest::default()
        },
    )
    .expect("clause builds");
    let ops = add_clause_ops(&a, coin, clause, false).expect("clause ops");
    a.commit("only-if", ops).expect("clause commits");

    project::save(&a, &path).expect("save");

    // Reopen into a FRESH engine — the real "close the app and come back" path.
    let (mut b, _scene_b) = engine_with_resolver();
    project::open_into(&mut b, &path).expect("open");

    assert_eq!(
        role_of(&b, coin).as_deref(),
        Some("collectible"),
        "the role came back"
    );
    assert_eq!(
        b.get_field(coin, ROLE_COMPONENT, "points"),
        Some(FieldValue::Integer(10)),
        "the tuned Points value came back — not reset to the default"
    );
    let restored = clauses_of(&b, coin);
    assert_eq!(restored.all.len(), 1, "the only-if clause came back");
    assert_eq!(
        restored.all[0].entity,
        key.to_loro_key(),
        "and it still points at the same object"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_rules_or_group_and_subject_pin_survive_save_and_open() {
    let path = temp_mtk("ruleslots");
    let _ = std::fs::remove_file(&path);

    let (mut a, _scene) = engine_with_resolver();
    let target = spawn(&mut a, 0.0);
    let key = target.to_loro_key();

    // Both fields are new Loro slots written this session. If either write or read is missing, the
    // rule still loads — just quietly without its OR group or its pin, which changes the game.
    let rule = RuleData {
        name: "gated".into(),
        enabled: true,
        event: "Touched".into(),
        conditions: vec![Condition {
            entity: key.clone(),
            component: ROLE_COMPONENT.into(),
            field: "active".into(),
            op: CompareOp::Eq,
            value: FieldValue::Bool(true),
        }],
        any_of: vec![
            Condition {
                entity: key.clone(),
                component: "KillCounter".into(),
                field: "count".into(),
                op: CompareOp::Ge,
                value: FieldValue::Integer(3),
            },
            Condition {
                entity: key.clone(),
                component: ROLE_COMPONENT.into(),
                field: "role".into(),
                op: CompareOp::Eq,
                value: FieldValue::Str("player".into()),
            },
        ],
        subject: Some(key.clone()),
        actions: vec![Action {
            action: "SetField".into(),
            entity: SUBJECT_ENTITY.into(),
            component: ROLE_COMPONENT.into(),
            field: "active".into(),
            value: FieldValue::Bool(false),
        }],
    };
    a.commit(
        "author rule",
        vec![Op::SetRule {
            id: RuleId::new("r_gated"),
            rule: rule.clone(),
        }],
    )
    .expect("rule commits");

    project::save(&a, &path).expect("save");
    let (mut b, _scene_b) = engine_with_resolver();
    project::open_into(&mut b, &path).expect("open");

    let back = b.rule(&RuleId::new("r_gated")).expect("the rule came back");
    assert_eq!(back.conditions.len(), 1, "the AND clause survived");
    assert_eq!(back.any_of.len(), 2, "the OR group survived the reload");
    assert_eq!(
        back.subject.as_deref(),
        Some(key.as_str()),
        "the subject pin survived the reload"
    );
    assert_eq!(back, rule, "the whole rule round-trips byte-for-byte");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn clearing_an_or_group_persists_as_cleared_rather_than_leaving_a_stale_slot() {
    let path = temp_mtk("cleared");
    let _ = std::fs::remove_file(&path);

    let (mut a, _scene) = engine_with_resolver();
    let target = spawn(&mut a, 0.0);
    let id = RuleId::new("r_clear");
    let base = RuleData {
        name: "gated".into(),
        enabled: true,
        event: "Touched".into(),
        conditions: vec![],
        any_of: vec![Condition {
            entity: target.to_loro_key(),
            component: "KillCounter".into(),
            field: "count".into(),
            op: CompareOp::Ge,
            value: FieldValue::Integer(3),
        }],
        subject: Some(target.to_loro_key()),
        actions: vec![Action {
            action: "AdjustCounter".into(),
            entity: target.to_loro_key(),
            component: "KillCounter".into(),
            field: "count".into(),
            value: FieldValue::Integer(1),
        }],
    };
    a.commit(
        "author",
        vec![Op::SetRule {
            id: id.clone(),
            rule: base.clone(),
        }],
    )
    .expect("commits");

    // Now the user removes the alternative and the pin — overwriting the SAME rule id.
    let cleared = RuleData {
        any_of: vec![],
        subject: None,
        ..base
    };
    a.commit(
        "clear",
        vec![Op::SetRule {
            id: id.clone(),
            rule: cleared.clone(),
        }],
    )
    .expect("commits");

    project::save(&a, &path).expect("save");
    let (mut b, _scene_b) = engine_with_resolver();
    project::open_into(&mut b, &path).expect("open");

    let back = b.rule(&id).expect("rule");
    assert!(
        back.any_of.is_empty(),
        "a removed OR group must not resurrect from a stale slot"
    );
    assert!(back.subject.is_none(), "a removed pin must stay removed");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn what_a_cut_delivers_survives_save_and_open() {
    // ADR-190's closing gate, and the half of it a unit test cannot reach. "The rate, the size and
    // the name surviving a reopen AND a restart" was owed by ADR-177 and then by ADR-182, which
    // stated the reason plainly: session-scoped memory satisfies "reopen the dialog" and fails the
    // thing that matters. This is the failing half — a real `.mtk` written to disk, a SECOND engine
    // built from nothing, and the four answers read back out of it.
    //
    // Before this pass every one of them was a `useState` in the render dialog seeded from a
    // constant, so the correct expectation for this test on the old code is four defaults.
    let path = temp_mtk("cinema-render-settings");
    let _ = std::fs::remove_file(&path);

    let (mut a, _scene) = engine_with_resolver();
    let rig = spawn(&mut a, 2.0);
    for kind in ["establish", "hero"] {
        let (ops, _) =
            metrocalk_editor_shell::add_shot_ops(&a, rig, kind, rig).expect("shot lands");
        a.commit("shot", ops).expect("shot commits");
    }

    // NOT THE DEFAULTS, in all four: a test that stored `movie / 24 / 1080 / ""` would pass on an
    // engine that had thrown the settings away and re-derived them.
    let (ops, _) = metrocalk_editor_shell::set_render_ops(
        &a,
        rig,
        "sequence",
        60,
        Some(2160),
        "weld-master",
        r"X:\Renders\Skid Weld Line",
    )
    .expect("five answers the engine offers");
    a.commit("cinema-render", ops).expect("settings commit");
    // ...and the delivery frame beside them, because the whole claim is that these live where
    // `delivery` already did.
    let (ops, _) =
        metrocalk_editor_shell::set_delivery_ops(&a, rig, "scope").expect("a real frame");
    a.commit("cinema-delivery", ops).expect("delivery commits");

    let before = metrocalk_editor_shell::cutscene_of(&a, rig);
    project::save(&a, &path).expect("save");

    let (mut b, _scene_b) = engine_with_resolver();
    project::open_into(&mut b, &path).expect("open");
    let after = metrocalk_editor_shell::cutscene_of(&b, rig);

    assert_eq!(after, before, "a cutscene changed across a reload");
    assert_eq!(
        after.render.format,
        metrocalk_animation::shot::RenderFormat::Sequence,
        "what the cut delivers"
    );
    assert_eq!(after.render.fps, 60, "the rate");
    assert_eq!(after.render.height, Some(2160), "the size");
    assert_eq!(after.render.name, "weld-master", "the name");
    assert_eq!(
        after.render.folder, r"X:\Renders\Skid Weld Line",
        "the destination"
    );
    assert_eq!(
        after.delivery,
        metrocalk_animation::shot::Delivery::Scope,
        "the delivery frame these were stored beside"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_camera_the_author_placed_survives_save_and_open() {
    // ADR-192's closing gate. The gesture's whole promise is "film what I am looking at", and a pose
    // that came back one metre off — or came back as the card it replaced — would break that promise
    // silently: the project opens, the shot is there, and only the picture is somebody else's.
    //
    // A REAL `.mtk` and a SECOND engine built from nothing, for the reason ADR-190 gave: an
    // in-memory struct handed back to its own author proves nothing about the wire format, and the
    // pose crosses JSON on the way to disk.
    let path = temp_mtk("cinema-placed-camera");
    let _ = std::fs::remove_file(&path);

    let (mut a, _scene) = engine_with_resolver();
    let rig = spawn(&mut a, 2.0);
    for kind in ["establish", "hero"] {
        let (ops, _) =
            metrocalk_editor_shell::add_shot_ops(&a, rig, kind, rig).expect("shot lands");
        a.commit("shot", ops).expect("shot commits");
    }

    // Deliberately not round numbers and deliberately not the viewport's own default lens: a test
    // whose pose was `[0,0,0]` at 55 degrees would pass on an engine that had stored nothing and
    // re-derived a camera from scratch.
    let placed = metrocalk_animation::shot::ShotCamera {
        eye: [7.4, 2.9, -5.1],
        look_at: [0.2, 1.35, 0.4],
        fov_deg: 38.5,
    };
    let (ops, _) =
        metrocalk_editor_shell::set_shot_camera_ops(&a, rig, 1, placed).expect("the pose stores");
    a.commit("cinema-camera", ops).expect("camera commits");

    let before = metrocalk_editor_shell::cutscene_of(&a, rig);
    project::save(&a, &path).expect("save");

    let (mut b, _scene_b) = engine_with_resolver();
    project::open_into(&mut b, &path).expect("open");
    let after = metrocalk_editor_shell::cutscene_of(&b, rig);

    assert_eq!(after, before, "a cutscene changed across a reload");
    assert_eq!(after.shots[1].camera, Some(placed), "the placed camera");
    assert_eq!(
        after.shots[0].camera, None,
        "the shot beside it is unplaced"
    );
    // AND THE POSE IS WHAT GETS FILMED. Reading the field back is not the claim; the claim is that
    // the solver in the reopened project stands the camera exactly there.
    let filmed = metrocalk_animation::shot::solve_shot(
        &after.shots[1],
        metrocalk_animation::shot::SubjectSample {
            center: [0.0, 0.0, 0.0],
            half_extent: [1.0, 1.0, 1.0],
            forward: [0.0, 0.0, 1.0],
            stage: metrocalk_animation::shot::Stage::OPEN,
        },
        0.0,
        16.0 / 9.0,
        50.0,
    );
    // BITS, not `==` on the arrays: the claim is that nothing was recomputed on the way through the
    // document, so an epsilon here would accept a pose that had been re-derived and landed close.
    assert_eq!(
        filmed.eye.map(f32::to_bits),
        placed.eye.map(f32::to_bits),
        "the reopened shot films from somewhere else"
    );
    assert_eq!(
        filmed.look_at.map(f32::to_bits),
        placed.look_at.map(f32::to_bits)
    );
    assert!(
        (filmed.fov_deg - 38.5).abs() < f32::EPSILON,
        "the lens they framed through"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_cutscene_authored_before_render_settings_existed_opens_on_the_defaults() {
    // THE MIGRATION, on a real file. Every `.mtk` written before this pass carries a `Cinematic`
    // whose `source` blob has no `render` key; `#[serde(default)]` is what makes those open, and a
    // test that only ever reads documents this build wrote would never exercise it. Here the blob is
    // written BY HAND into the same field the intent commands write, so it is the old shape rather
    // than a new one with a field removed.
    let path = temp_mtk("cinema-render-legacy");
    let _ = std::fs::remove_file(&path);

    let (mut a, _scene) = engine_with_resolver();
    let rig = spawn(&mut a, 2.0);
    let (ops, _) = metrocalk_editor_shell::add_shot_ops(&a, rig, "hero", rig).expect("shot lands");
    a.commit("shot", ops).expect("shot commits");

    let legacy = {
        let cut = metrocalk_editor_shell::cutscene_of(&a, rig);
        let mut json: serde_json::Value = serde_json::to_value(&cut).expect("serialises");
        json.as_object_mut().expect("an object").remove("render");
        serde_json::to_string(&json).expect("re-serialises")
    };
    assert!(!legacy.contains("render"), "the old shape has no such key");
    a.commit(
        "legacy-cutscene",
        vec![Op::SetField {
            entity: rig,
            component: metrocalk_editor_shell::CINEMA_COMPONENT.into(),
            field: "source".into(),
            value: FieldValue::Str(legacy),
        }],
    )
    .expect("the old blob commits");

    project::save(&a, &path).expect("save");
    let (mut b, _scene_b) = engine_with_resolver();
    project::open_into(&mut b, &path).expect("open");
    let after = metrocalk_editor_shell::cutscene_of(&b, rig);

    assert_eq!(after.shots.len(), 1, "the cut itself still reads");
    assert_eq!(
        after.render,
        metrocalk_animation::shot::RenderSettings::default(),
        "a document from before the field opens on the engine's own answers"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn an_edited_cutscene_survives_save_and_open_down_to_the_second() {
    // The sibling test above proves a cutscene AUTHORED FROM CARDS comes back. Everything a user can
    // now change about one afterwards — a shot's length, its place in the sequence, its framing, and
    // which object it films — is a different question, and it is the one requirement 10 of the
    // capability brief names: "verify the result survives restart/reload". A length that reverts to
    // its card's default on reopen is the worst kind of loss, because the project opens, the shot is
    // there, and only the timing is quietly someone else's.
    let path = temp_mtk("cinema-edited");
    let _ = std::fs::remove_file(&path);

    let (mut a, _scene) = engine_with_resolver();
    let statue = spawn(&mut a, 2.0);
    let hall = spawn(&mut a, 40.0);

    // An establishing wide of the HALL, then two shots of the statue — ordinary film grammar, and the
    // shape that had no user path at all before this pass.
    for (kind, framed) in [("establish", hall), ("hero", statue), ("closeup", statue)] {
        let (ops, _) =
            metrocalk_editor_shell::add_shot_ops(&a, statue, kind, framed).expect("shot lands");
        a.commit("shot", ops).expect("shot commits");
    }

    let (ops, _) =
        metrocalk_editor_shell::set_shot_seconds_ops(&a, statue, 1, 7.4).expect("in range");
    a.commit("seconds", ops).expect("length commits");
    let (ops, _) = metrocalk_editor_shell::set_shot_framing_ops(
        &a,
        statue,
        2,
        &metrocalk_editor_shell::FramingEdit {
            angle: Some("low".into()),
            motion: Some("orbit".into()),
            amount: Some(0.75),
            ..metrocalk_editor_shell::FramingEdit::default()
        },
    )
    .expect("known words");
    a.commit("framing", ops).expect("framing commits");
    let (ops, _) = metrocalk_editor_shell::move_shot_ops(&a, statue, 2, 0).expect("a real move");
    a.commit("move", ops).expect("reorder commits");
    let (ops, _) = metrocalk_editor_shell::set_mood_ops(&a, statue, "calm").expect("known mood");
    a.commit("mood", ops).expect("mood commits");
    // RE-AIMED AFTER THE FACT, which is what the subject picker actually does: the hero shot was
    // authored on the statue and is then pointed at the hall. Different from authoring it framed
    // that way, and it has to survive on its own — a re-aim that reverted on reopen would leave the
    // project opening with the right number of shots, the right lengths, and one of them quietly
    // filming something else.
    // Index 2: the reorder above put the close-up first, so the hero shot the length was set on is
    // now the third, and it is the one being re-aimed.
    let (ops, _) =
        metrocalk_editor_shell::set_shot_subject_ops(&a, statue, 2, hall).expect("a live object");
    a.commit("subject", ops).expect("subject commits");

    let before = metrocalk_editor_shell::cutscene_of(&a, statue);
    assert_eq!(before.shots.len(), 3);

    project::save(&a, &path).expect("save");

    let (mut b, _scene_b) = engine_with_resolver();
    project::open_into(&mut b, &path).expect("open");
    let after = metrocalk_editor_shell::cutscene_of(&b, statue);

    // Not "roughly the same" — the SAME, as the sibling test puts it.
    assert_eq!(after, before, "an edited cutscene changed across a reload");

    // ...and then the specifics, so a refactor that guts one field still fails here rather than
    // passing on a structural equality between two equally-wrong values.
    assert_eq!(
        after.mood,
        metrocalk_animation::shot::Mood::Calm,
        "the pacing dial"
    );
    // The re-framed close-up was moved to the FRONT, so the order is the authored one.
    assert_eq!(
        after.shots[0].angle,
        metrocalk_animation::shot::ShotAngle::Low
    );
    assert_eq!(
        after.shots[0].motion,
        metrocalk_animation::shot::ShotMove::Orbit
    );
    assert!(
        (after.shots[0].amount - 0.75).abs() < 1.0e-5,
        "move strength"
    );
    // The 7.4s length is on the shot it was set on, which the reorder pushed one place later — and
    // which the picker then re-aimed, so this is also the assertion that a re-aim leaves a length
    // alone.
    assert!(
        (after.shots[2].seconds - 7.4).abs() < 1.0e-5,
        "the authored length came back as {}",
        after.shots[2].seconds
    );
    // Calm scales it 2.5x at playback and leaves the AUTHORED number alone — both halves survive.
    assert!((after.effective_shot_seconds(2).expect("a shot") - 18.5).abs() < 1.0e-4);
    // And the establishing shot still films the HALL, not its own cutscene's owner — as does the
    // hero shot the picker re-aimed at it after the fact. The reorder moved the close-up to the
    // front, so the two hall shots are now at 1 and 2.
    assert_eq!(after.shots[1].subject, hall.to_loro_key());
    assert_eq!(
        after.shots[2].subject,
        hall.to_loro_key(),
        "a shot re-aimed after it was authored came back aimed somewhere else"
    );
    assert!(
        after
            .shots
            .iter()
            .any(|s| s.subject == statue.to_loro_key()),
        "the statue's own shots are still its own"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_cutscene_and_its_effects_survive_save_and_open() {
    // The two newest pillars write canonical JSON into `Cinematic.source` / `Vfx.source`. A blob field
    // that does not round-trip fails in the worst possible way: the project opens, the object is there,
    // and the cutscene and the fire are simply gone with no error anywhere.
    let path = temp_mtk("cinema-vfx");
    let _ = std::fs::remove_file(&path);

    let (mut a, _scene) = engine_with_resolver();
    let statue = spawn(&mut a, 2.0);

    let (ops, _) = metrocalk_editor_shell::add_shot_ops(&a, statue, "hero", statue).expect("shot");
    a.commit("shot", ops).expect("shot commits");
    let (ops, _) =
        metrocalk_editor_shell::add_shot_ops(&a, statue, "orbit", statue).expect("shot 2");
    a.commit("shot", ops).expect("shot 2 commits");

    let (ops, _) = metrocalk_editor_shell::add_effect_ops(
        &a,
        statue,
        "fire",
        metrocalk_editor_shell::VfxTrigger::Always,
    )
    .expect("fire");
    a.commit("fx", ops).expect("fire commits");
    let (ops, _) = metrocalk_editor_shell::add_effect_ops(
        &a,
        statue,
        "smoke",
        metrocalk_editor_shell::VfxTrigger::Always,
    )
    .expect("smoke");
    a.commit("fx", ops).expect("smoke commits");

    let before_cut = metrocalk_editor_shell::cutscene_of(&a, statue);
    let before_fx = metrocalk_editor_shell::stack_of(&a, statue);
    assert_eq!(before_cut.shots.len(), 2);
    assert_eq!(before_fx.layers.len(), 2);

    project::save(&a, &path).expect("save");

    let (mut b, _scene_b) = engine_with_resolver();
    project::open_into(&mut b, &path).expect("open");

    let after_cut = metrocalk_editor_shell::cutscene_of(&b, statue);
    let after_fx = metrocalk_editor_shell::stack_of(&b, statue);
    // Not "roughly the same" — the SAME. A shot's framing and an effect's colour ramp are the whole
    // artifact, so anything less than structural equality is a silent loss.
    assert_eq!(
        after_cut, before_cut,
        "the cutscene changed across a reload"
    );
    assert_eq!(after_fx, before_fx, "the effects changed across a reload");
    // and the specifics, spelled out, so a future refactor that guts one field still fails here
    assert_eq!(after_cut.shots[0].subject, statue.to_loro_key());
    assert_eq!(after_fx.layers[0].kind, "fire");
    assert_eq!(after_fx.layers[1].kind, "smoke");
    assert!(
        after_fx.layers[0].color[0] > 1.0,
        "the HDR ramp must survive"
    );

    let _ = std::fs::remove_file(&path);
}
