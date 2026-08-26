//! Cinematics — a cutscene is a **shot list**, and a shot is a sentence, not a curve.
//!
//! The gesture is the role card's: select an object, click "Hero shot", and one undoable commit gives
//! you a framed camera move you can immediately watch. Nobody types a distance, a quaternion, or a
//! keyframe — the user picks WORDS (how close, from where, what the camera does) and
//! [`metrocalk_animation::shot`] turns them into a pose, solved fresh each tick against the subject's
//! live position so the shot survives the subject moving.
//!
//! Storage is the `PlayIf` pattern: canonical JSON on the object's own `Cinematic.source`, written only
//! by the validated commands here. Playing one is an ordinary `SetField Cinematic.playing = true` from a
//! rule — which is why the closed action vocabulary never had to grow to gain cutscenes.

use metrocalk_animation::shot::{
    Cutscene, Mood, ShotAngle, ShotMove, ShotRecipe, ShotSize, MAX_SHOTS,
};
use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::World;
use serde::{Deserialize, Serialize};

/// The component holding an object's cutscene (registered in the stdlib).
pub const CINEMA_COMPONENT: &str = "Cinematic";

/// One shot card, in the author's language — the [`crate::role_intent::RoleSpec`] sibling, so the UI
/// renders cinematics with exactly the grammar roles and conditions already use.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotSpec {
    /// Stable kind key — AND the card's icon name (ADR-137).
    ///
    /// The catalog used to carry a separate `icon: &'static str` holding one character per entry, and
    /// in all fifty-nine entries across the five catalogs that character was a per-`kind` constant:
    /// the field was one contract stated twice, in two languages, which is exactly the drift ADR-134
    /// is about. It was also thirty-five colour EMOJI, which the editor is a monochrome light
    /// workbench and cannot draw. `editor/src/theme/icons.tsx` keys its drawings on these kinds, so
    /// there is nothing left to keep in sync — `check-icon-vocab.mjs` fails if a kind here has no
    /// mark there.
    pub kind: &'static str,
    /// Card label.
    pub label: &'static str,
    /// What it is for, one line.
    pub blurb: &'static str,
    /// What the shot will look like, spelled out — the legible-cost line.
    pub adds: &'static str,
}

/// The shot catalogue. Each card is a whole framing decision, so the first click already looks good.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "a fifteen-card data table; splitting it hides the catalogue"
)]
pub fn shot_specs() -> Vec<ShotSpec> {
    vec![
        ShotSpec {
            kind: "establish",
            label: "Establishing",
            blurb: "Show where we are before we look at anything closely",
            adds: "a wide, slowly pulling-out shot from the front",
        },
        ShotSpec {
            kind: "hero",
            label: "Hero shot",
            blurb: "The workhorse — three-quarters on, pushing in",
            adds: "a full-body three-quarter shot that creeps closer",
        },
        ShotSpec {
            kind: "closeup",
            label: "Close-up",
            blurb: "Tight and still — for the moment that matters",
            adds: "a close, locked-off shot in profile",
        },
        ShotSpec {
            kind: "orbit",
            label: "Show it off",
            blurb: "Circle the object so every side reads",
            adds: "a medium shot orbiting a quarter turn",
        },
        ShotSpec {
            kind: "reveal",
            label: "Crane reveal",
            blurb: "Lift away to show the world around it",
            adds: "a full shot craning upward",
        },
        ShotSpec {
            kind: "looming",
            label: "Looming",
            blurb: "From below, so the subject towers",
            adds: "a low-angle medium shot pushing in",
        },
        ShotSpec {
            kind: "vista",
            label: "The vista",
            blurb: "The subject is a speck in its world — scale, before anything else",
            adds: "an extreme-wide, locked-off shot from the front",
        },
        ShotSpec {
            kind: "overshoulder",
            label: "Over the shoulder",
            blurb: "Stand where it stands and look where it looks",
            adds: "a medium shot from behind, holding steady",
        },
        ShotSpec {
            kind: "birdseye",
            label: "Bird's eye",
            blurb: "Look down on it — the map view, the trap closing",
            adds: "a wide shot from high above, craning down",
        },
        ShotSpec {
            kind: "dropin",
            label: "Drop in",
            blurb: "Fall out of the sky onto the subject",
            adds: "a full shot craning down from overhead",
        },
        ShotSpec {
            kind: "confront",
            label: "Face to face",
            blurb: "Head-on and closing — the moment before something happens",
            adds: "a close, front-on shot pushing in hard",
        },
        ShotSpec {
            kind: "detail",
            label: "The detail",
            blurb: "Tighter than the subject — one thing, very large",
            adds: "an extreme close-up, locked off, three-quarters on",
        },
        ShotSpec {
            kind: "pullback",
            label: "Pull back",
            blurb: "Start on it, end on everything around it",
            adds: "a medium shot pulling all the way out",
        },
        ShotSpec {
            kind: "sweep",
            label: "Side sweep",
            blurb: "Track past it in profile — motion without a cut",
            adds: "a full shot in profile, orbiting a third of a turn",
        },
    ]
}

/// Something the cinematics layer will not do, said in the author's language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CinemaError {
    /// No such card.
    UnknownShot(String),
    /// No such pacing mood.
    UnknownMood(String),
    /// The object is gone.
    MissingEntity,
    /// The shot list is full.
    TooMany(String),
    /// The write failed.
    Blocked(String),
}

impl std::fmt::Display for CinemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownShot(k) => write!(f, "there is no shot called \"{k}\""),
            Self::UnknownMood(mood) => {
                write!(f, "there is no cinematic mood called \"{mood}\"")
            }
            Self::MissingEntity => write!(f, "that object is no longer in the scene"),
            Self::TooMany(m) | Self::Blocked(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for CinemaError {}

/// What a completed cinematics command answers with.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CinemaReply {
    /// The object whose cutscene changed.
    pub entity: Option<String>,
    /// How many shots it now holds.
    pub shots: usize,
    /// Total running time, seconds.
    pub seconds: f32,
    /// The authored pacing dial currently driving effective duration and transitions.
    pub mood: Mood,
    /// The whole cutscene read back as sentences — one line per shot.
    pub reads: Vec<String>,
    /// Continuity warnings, in plain language (a jump cut, opening tight, a rushed shot).
    pub problems: Vec<String>,
    /// Friendly summary.
    pub message: String,
    /// Set iff nothing changed.
    pub reason: Option<String>,
}

impl CinemaReply {
    /// A refusal that changed nothing, explained.
    #[must_use]
    pub fn refusal(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self {
            message: reason.clone(),
            reason: Some(reason),
            ..Self::default()
        }
    }
}

/// Read an object's cutscene (empty when it has none, or when the JSON is unreadable — a corrupt blob
/// is never trusted into the runtime).
#[must_use]
pub fn cutscene_of<W: World>(engine: &Engine<W>, entity: EntityId) -> Cutscene {
    engine
        .resolved_components(entity)
        .get(CINEMA_COMPONENT)
        .and_then(|fields| match fields.get("source") {
            Some(FieldValue::Str(json)) => serde_json::from_str::<Cutscene>(json).ok(),
            _ => None,
        })
        .unwrap_or_default()
}

/// The card's framing decisions, as a shot on `subject`.
#[allow(
    clippy::too_many_lines,
    reason = "a fifteen-card data table; splitting it hides the catalogue"
)]
fn recipe_for(kind: &str, subject: &str, index: usize) -> Option<ShotRecipe> {
    let (size, angle, motion, amount, seconds) = match kind {
        // THREE-QUARTER, NOT FRONT — because perspective does what distance cannot.
        //
        // A wide card points the camera at whatever it frames and steps back until the subject fits.
        // Broadside to a long subject, "fits" is a ribbon: the production weld line is ~262 m long and a
        // few metres tall, so at any distance that fits its length it occupies a few per cent of the
        // frame's height, with featureless ground below and empty background above. Measured on the
        // delivered film, plant-framed wides read an interdecile luma range of 129-135 — the yellow
        // floor against the dark void, which is real contrast and no information — and a mean edge
        // energy of 0.31-0.38, near the bottom of the whole film.
        //
        // Every genuinely well-filled frame in either delivered film is a three-quarter or profile view
        // with the line RECEDING into the picture (edge energy 3.5-5.1). Turning the camera is what puts
        // a long subject into depth; there is no camera distance that does it.
        "establish" => (
            ShotSize::Wide,
            ShotAngle::ThreeQuarter,
            ShotMove::PullOut,
            0.3,
            2.5,
        ),
        "hero" => (
            ShotSize::Full,
            ShotAngle::ThreeQuarter,
            ShotMove::PushIn,
            0.35,
            2.5,
        ),
        "closeup" => (
            ShotSize::Close,
            ShotAngle::Profile,
            ShotMove::Hold,
            0.0,
            1.8,
        ),
        "orbit" => (
            ShotSize::Medium,
            ShotAngle::ThreeQuarter,
            ShotMove::Orbit,
            0.5,
            3.0,
        ),
        "reveal" => (
            ShotSize::Full,
            ShotAngle::ThreeQuarter,
            ShotMove::CraneUp,
            0.6,
            3.0,
        ),
        "looming" => (ShotSize::Medium, ShotAngle::Low, ShotMove::PushIn, 0.3, 2.0),
        // Three-quarter for the same reason as `establish`: the widest card in the vocabulary is the one
        // that most needs the subject put into depth rather than laid flat across the frame.
        "vista" => (
            ShotSize::ExtremeWide,
            ShotAngle::ThreeQuarter,
            ShotMove::Hold,
            0.0,
            3.0,
        ),
        "overshoulder" => (
            ShotSize::Medium,
            ShotAngle::Behind,
            ShotMove::Hold,
            0.0,
            2.2,
        ),
        "birdseye" => (
            ShotSize::Wide,
            ShotAngle::High,
            ShotMove::CraneDown,
            0.45,
            3.0,
        ),
        "dropin" => (
            ShotSize::Full,
            ShotAngle::High,
            ShotMove::CraneDown,
            0.7,
            2.2,
        ),
        "confront" => (
            ShotSize::Close,
            ShotAngle::Front,
            ShotMove::PushIn,
            0.5,
            2.0,
        ),
        "detail" => (
            ShotSize::ExtremeClose,
            ShotAngle::ThreeQuarter,
            ShotMove::Hold,
            0.0,
            1.6,
        ),
        "pullback" => (
            ShotSize::Medium,
            ShotAngle::ThreeQuarter,
            ShotMove::PullOut,
            0.8,
            3.2,
        ),
        "sweep" => (
            ShotSize::Full,
            ShotAngle::Profile,
            ShotMove::Orbit,
            0.33,
            3.0,
        ),
        _ => return None,
    };
    Some(ShotRecipe {
        // Stable, order-independent id: reordering never renumbers, and two peers adding at once
        // converge instead of colliding.
        id: format!(
            "shot-{:08x}",
            stable_hash(&format!("{kind}:{subject}:{index}"))
        ),
        subject: subject.to_string(),
        size,
        angle,
        motion,
        amount,
        seconds,
    })
}

/// A tiny stable hash (FNV-1a) — the same "identity from content" discipline the animation tracks use.
fn stable_hash(s: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in s.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// The ops that append one shot to `entity`'s cutscene. The caller commits, so a shot is one undo step
/// and can ride a larger transaction.
///
/// # Errors
/// [`CinemaError`] — an unknown card, or the shot ceiling reached (refused with the way out).
pub fn add_shot_ops<W: World>(
    engine: &Engine<W>,
    entity: EntityId,
    kind: &str,
    subject: EntityId,
) -> Result<(Vec<Op>, Cutscene), CinemaError> {
    if !engine.entity_exists(entity) {
        return Err(CinemaError::MissingEntity);
    }
    let mut cut = cutscene_of(engine, entity);
    if cut.shots.len() >= MAX_SHOTS {
        return Err(CinemaError::TooMany(format!(
            "a cutscene can hold at most {MAX_SHOTS} shots — remove one first"
        )));
    }
    // Probe upward for a free id. A bare hash of the list LENGTH collides after a removal:
    // add, add, remove(1), add sees len()==1 again and re-mints the surviving shot's id. Still a pure
    // function of (kind, subject, current state), so replay agrees, and now unique by construction.
    let key = subject.to_loro_key();
    let taken: Vec<&str> = cut.shots.iter().map(|s| s.id.as_str()).collect();
    let mut probe = cut.shots.len();
    let recipe = loop {
        let candidate = recipe_for(kind, &key, probe)
            .ok_or_else(|| CinemaError::UnknownShot(kind.to_string()))?;
        if !taken.contains(&candidate.id.as_str()) {
            break candidate;
        }
        probe += 1;
        if probe > cut.shots.len() + MAX_SHOTS + 8 {
            return Err(CinemaError::Blocked(
                "could not give that shot a unique name — remove one and try again".into(),
            ));
        }
    };
    cut.shots.push(recipe);
    Ok((write_ops(entity, &cut, true), cut))
}

/// The ops that remove one shot by index.
///
/// # Errors
/// [`CinemaError::TooMany`] when the index isn't there (said plainly).
pub fn remove_shot_ops<W: World>(
    engine: &Engine<W>,
    entity: EntityId,
    index: usize,
) -> Result<(Vec<Op>, Cutscene), CinemaError> {
    let mut cut = cutscene_of(engine, entity);
    if index >= cut.shots.len() {
        return Err(CinemaError::TooMany("that shot is already gone".into()));
    }
    cut.shots.remove(index);
    if cut.shots.is_empty() {
        return Ok((
            vec![Op::RemoveComponent {
                entity,
                component: CINEMA_COMPONENT.into(),
            }],
            cut,
        ));
    }
    Ok((write_ops(entity, &cut, false), cut))
}

/// The ops that set the one global dial.
///
/// # Errors
/// [`CinemaError::UnknownMood`] when the mood name is not one of calm/normal/tense.
pub fn set_mood_ops<W: World>(
    engine: &Engine<W>,
    entity: EntityId,
    mood: &str,
) -> Result<(Vec<Op>, Cutscene), CinemaError> {
    let mut cut = cutscene_of(engine, entity);
    cut.mood = match mood {
        "calm" => Mood::Calm,
        "normal" => Mood::Normal,
        "tense" => Mood::Tense,
        other => return Err(CinemaError::UnknownMood(other.to_string())),
    };
    Ok((write_ops(entity, &cut, false), cut))
}

/// Write the cutscene back.
///
/// `arm` says whether this write should also set `playing = true`. Only ADDING a shot arms a cutscene:
/// removing one, or changing the mood, used to silently re-arm a cutscene the author had deliberately
/// held back for a rule to trigger — so deleting a shot handed the camera away at the next Play.
fn write_ops(entity: EntityId, cut: &Cutscene, arm: bool) -> Vec<Op> {
    let json = serde_json::to_string(cut).unwrap_or_else(|_| "{}".into());
    vec![
        Op::SetField {
            entity,
            component: CINEMA_COMPONENT.into(),
            field: "source".into(),
            value: FieldValue::Str(json),
        },
        Op::SetField {
            entity,
            component: CINEMA_COMPONENT.into(),
            field: "seconds".into(),
            value: FieldValue::Number(f64::from(cut.seconds())),
        },
        // Authored TRUE = "this is an opening shot; roll it when Play begins". A rule can set it
        // false to hold the cutscene back, or true again to trigger it later - an ordinary SetField,
        // which is exactly why cutscenes needed no new action verb. Play itself never writes this.
        Op::SetField {
            entity,
            component: CINEMA_COMPONENT.into(),
            field: "playing".into(),
            value: FieldValue::Bool(arm),
        },
    ]
}

/// Render one shot as the sentence the user reads back.
#[must_use]
pub fn describe_shot(shot: &ShotRecipe, subject_name: &str) -> String {
    describe_shot_with_seconds(shot, subject_name, shot.seconds)
}

fn describe_shot_with_seconds(
    shot: &ShotRecipe,
    subject_name: &str,
    effective_seconds: f32,
) -> String {
    let size = match shot.size {
        ShotSize::ExtremeWide => "a distant",
        ShotSize::Wide => "a wide",
        ShotSize::Full => "a full",
        ShotSize::Medium => "a medium",
        ShotSize::Close => "a close",
        ShotSize::ExtremeClose => "a very close",
    };
    let angle = match shot.angle {
        ShotAngle::Front => "from the front",
        ShotAngle::ThreeQuarter => "from three-quarters",
        ShotAngle::Profile => "in profile",
        ShotAngle::Behind => "from behind",
        ShotAngle::Low => "from below",
        ShotAngle::High => "from above",
    };
    let motion = match shot.motion {
        ShotMove::Hold => "holding still",
        ShotMove::PushIn => "pushing in",
        ShotMove::PullOut => "pulling out",
        ShotMove::Orbit => "orbiting",
        ShotMove::CraneUp => "craning up",
        ShotMove::CraneDown => "craning down",
    };
    format!("{size} shot of {subject_name} {angle}, {motion} — {effective_seconds:.1}s")
}

/// Build the reply for a cutscene, including the continuity warnings.
#[must_use]
pub fn reply_for(entity: EntityId, cut: &Cutscene, name: &str, message: String) -> CinemaReply {
    CinemaReply {
        entity: Some(entity.to_loro_key()),
        shots: cut.shots.len(),
        seconds: cut.seconds(),
        mood: cut.mood,
        reads: cut
            .shots
            .iter()
            .enumerate()
            .map(|(index, shot)| {
                describe_shot_with_seconds(
                    shot,
                    name,
                    cut.effective_shot_seconds(index).unwrap_or(shot.seconds),
                )
            })
            .collect(),
        problems: cut.problems(),
        message,
        reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capscene::{CapResolver, CapScene};
    use metrocalk_ecs::FlecsWorld;

    fn world() -> (Engine<FlecsWorld>, CapScene) {
        let mut ecs = FlecsWorld::new();
        let scene = CapScene::intern(&mut ecs);
        let mut engine = Engine::new(ecs, 1);
        engine.set_capability_resolver(Box::new(CapResolver::from_scene(&scene)));
        (engine, scene)
    }

    fn spawn(engine: &mut Engine<FlecsWorld>) -> EntityId {
        let id = engine.alloc_entity_id();
        engine
            .commit("spawn", vec![Op::CreateEntity { id, parent: None }])
            .expect("spawns");
        id
    }

    #[test]
    fn one_card_click_is_one_undoable_shot_that_reads_as_a_sentence() {
        let (mut engine, _scene) = world();
        let hero = spawn(&mut engine);
        let (ops, cut) = add_shot_ops(&engine, hero, "hero", hero).expect("shot");
        engine.commit("cinema-shot", ops).expect("commits");

        assert_eq!(cut.shots.len(), 1);
        let line = describe_shot(&cut.shots[0], "Hero");
        assert!(line.contains("full shot of Hero"), "{line}");
        assert!(line.contains("pushing in"), "{line}");

        let stored = cutscene_of(&engine, hero);
        assert_eq!(stored.shots.len(), 1, "it round-trips through the document");

        assert!(engine.undo(), "one Ctrl-Z");
        assert!(cutscene_of(&engine, hero).shots.is_empty());
    }

    #[test]
    fn the_shot_catalogue_is_broad_and_every_card_frames_differently() {
        use metrocalk_animation::shot::{solve_shot, SubjectSample};
        let specs = shot_specs();
        assert!(specs.len() >= 14, "only {} shot cards", specs.len());

        let subject = SubjectSample {
            center: [0.0, 1.0, 0.0],
            half_extent: [0.5, 1.0, 0.5],
            forward: [0.0, 0.0, 1.0],
            stage: metrocalk_animation::shot::Stage::OPEN,
        };
        // Solve each card at the same instant and require the resulting cameras to be genuinely
        // different places. Two cards that put the camera within 30cm of each other are one card with
        // two names, and the author would never be able to tell them apart.
        let mut poses: Vec<(&str, [f32; 3])> = Vec::new();
        for spec in &specs {
            let r = recipe_for(spec.kind, "1_1", 0)
                .unwrap_or_else(|| panic!("card {} has no recipe", spec.kind));
            let p = solve_shot(&r, subject, 0.5, 16.0 / 9.0, 50.0);
            assert!(
                p.eye.iter().all(|v| v.is_finite()) && p.look_at.iter().all(|v| v.is_finite()),
                "{} produced a non-finite pose",
                spec.kind
            );
            for (other, prev) in &poses {
                let d = ((p.eye[0] - prev[0]).powi(2)
                    + (p.eye[1] - prev[1]).powi(2)
                    + (p.eye[2] - prev[2]).powi(2))
                .sqrt();
                assert!(
                    d > 0.3,
                    "\"{}\" and \"{}\" film from the same place",
                    other,
                    spec.kind
                );
            }
            poses.push((spec.kind, p.eye));
        }
    }

    #[test]
    fn every_move_actually_moves_the_way_its_card_says() {
        use metrocalk_animation::shot::{solve_shot, SubjectSample};
        let subject = SubjectSample {
            center: [0.0, 1.0, 0.0],
            half_extent: [0.5, 1.0, 0.5],
            forward: [0.0, 0.0, 1.0],
            stage: metrocalk_animation::shot::Stage::OPEN,
        };
        let solve = |kind: &str, t: f32| {
            let r = recipe_for(kind, "1_1", 0).expect("recipe");
            solve_shot(&r, subject, t, 16.0 / 9.0, 50.0)
        };
        let dist = |p: &metrocalk_animation::shot::CameraSample| {
            ((p.eye[0] - p.look_at[0]).powi(2)
                + (p.eye[1] - p.look_at[1]).powi(2)
                + (p.eye[2] - p.look_at[2]).powi(2))
            .sqrt()
        };
        // Push-ins close in.
        for kind in ["hero", "looming", "confront"] {
            assert!(
                dist(&solve(kind, 0.95)) < dist(&solve(kind, 0.05)),
                "{kind} should push in"
            );
        }
        // Pull-outs back off.
        for kind in ["establish", "pullback"] {
            assert!(
                dist(&solve(kind, 0.95)) > dist(&solve(kind, 0.05)),
                "{kind} should pull out"
            );
        }
        // Cranes change height without changing the aim.
        assert!(
            solve("reveal", 0.95).eye[1] > solve("reveal", 0.05).eye[1],
            "reveal should rise"
        );
        for kind in ["birdseye", "dropin"] {
            assert!(
                solve(kind, 0.95).eye[1] < solve(kind, 0.05).eye[1],
                "{kind} should crane down"
            );
        }
        // Orbits swing around it at a steady distance.
        for kind in ["orbit", "sweep"] {
            let (a, b) = (solve(kind, 0.05), solve(kind, 0.95));
            let swing = ((a.eye[0] - b.eye[0]).powi(2) + (a.eye[2] - b.eye[2]).powi(2)).sqrt();
            assert!(
                swing > 0.5,
                "{kind} should travel around the subject, moved {swing}"
            );
            assert!(
                (dist(&a) - dist(&b)).abs() < 0.5,
                "{kind} should hold its distance while orbiting"
            );
        }
        // Locked-off shots do not drift.
        for kind in ["closeup", "vista", "overshoulder", "detail"] {
            let (a, b) = (solve(kind, 0.05), solve(kind, 0.95));
            let drift = ((a.eye[0] - b.eye[0]).powi(2)
                + (a.eye[1] - b.eye[1]).powi(2)
                + (a.eye[2] - b.eye[2]).powi(2))
            .sqrt();
            assert!(drift < 0.05, "{kind} is locked off but drifted {drift}");
        }
    }

    #[test]
    fn a_tighter_card_really_does_stand_closer() {
        use metrocalk_animation::shot::{solve_shot, SubjectSample};
        let subject = SubjectSample {
            center: [0.0, 1.0, 0.0],
            half_extent: [0.5, 1.0, 0.5],
            forward: [0.0, 0.0, 1.0],
            stage: metrocalk_animation::shot::Stage::OPEN,
        };
        let d = |kind: &str| {
            let r = recipe_for(kind, "1_1", 0).expect("recipe");
            let p = solve_shot(&r, subject, 0.0, 16.0 / 9.0, 50.0);
            ((p.eye[0] - p.look_at[0]).powi(2)
                + (p.eye[1] - p.look_at[1]).powi(2)
                + (p.eye[2] - p.look_at[2]).powi(2))
            .sqrt()
        };
        // The whole size vocabulary, ordered. If this ever inverts, "close-up" stops meaning anything.
        let ordered = ["vista", "establish", "hero", "orbit", "closeup", "detail"];
        for pair in ordered.windows(2) {
            assert!(
                d(pair[0]) > d(pair[1]),
                "{} should stand further back than {} ({:.2} vs {:.2})",
                pair[0],
                pair[1],
                d(pair[0]),
                d(pair[1])
            );
        }
    }

    #[test]
    fn shots_keep_stable_ids_and_the_ceiling_is_refused_with_a_way_out() {
        let (mut engine, _scene) = world();
        let hero = spawn(&mut engine);
        let mut ids = Vec::new();
        for _ in 0..MAX_SHOTS {
            let (ops, cut) = add_shot_ops(&engine, hero, "hero", hero).expect("under the cap");
            engine.commit("cinema-shot", ops).expect("commits");
            ids.push(cut.shots.last().expect("a shot").id.clone());
        }
        // Every shot has its own id — a reorder can never renumber them into each other.
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), ids.len(), "ids are distinct: {ids:?}");

        let msg = add_shot_ops(&engine, hero, "hero", hero)
            .expect_err("over the cap")
            .to_string();
        assert!(msg.contains("at most"), "{msg}");
    }

    #[test]
    fn an_unknown_card_is_named_and_removing_the_last_shot_drops_the_component() {
        let (mut engine, _scene) = world();
        let hero = spawn(&mut engine);
        let msg = add_shot_ops(&engine, hero, "dolly_zoom", hero)
            .expect_err("unknown")
            .to_string();
        assert!(msg.contains("dolly_zoom"), "{msg}");

        let (ops, _) = add_shot_ops(&engine, hero, "closeup", hero).expect("shot");
        engine.commit("cinema-shot", ops).expect("commits");
        let (ops, _) = remove_shot_ops(&engine, hero, 0).expect("remove");
        engine.commit("cinema-clear", ops).expect("commits");
        assert!(
            !engine.components_of(hero).contains_key(CINEMA_COMPONENT),
            "no empty husk left behind"
        );
    }

    #[test]
    fn calm_mood_is_one_undoable_authored_change_and_readback_reports_effective_time() {
        let (mut engine, _scene) = world();
        let hero = spawn(&mut engine);
        let (ops, _) = add_shot_ops(&engine, hero, "hero", hero).expect("shot");
        engine.commit("cinema-shot", ops).expect("commits");
        let normal_seconds = cutscene_of(&engine, hero).seconds();

        let (ops, calm) = set_mood_ops(&engine, hero, "calm").expect("known mood");
        engine.commit("cinema-mood", ops).expect("commits");
        let reply = reply_for(hero, &calm, "Hero", "Calm pacing".into());
        assert_eq!(reply.mood, Mood::Calm);
        assert!((reply.seconds - normal_seconds * 2.5).abs() < 1.0e-5);
        assert!(reply.reads[0].contains(&format!("{:.1}s", reply.seconds)));

        assert!(engine.undo(), "one Ctrl-Z restores the previous mood");
        assert_eq!(cutscene_of(&engine, hero).mood, Mood::Normal);
        assert!(matches!(
            set_mood_ops(&engine, hero, "dreamy"),
            Err(CinemaError::UnknownMood(value)) if value == "dreamy"
        ));
    }

    #[test]
    fn the_continuity_checker_warns_about_a_jump_cut_in_a_real_authored_list() {
        let (mut engine, _scene) = world();
        let hero = spawn(&mut engine);
        // The same card twice in a row IS the amateur mistake — identical framing, cut together.
        for _ in 0..2 {
            let (ops, _) = add_shot_ops(&engine, hero, "closeup", hero).expect("shot");
            engine.commit("cinema-shot", ops).expect("commits");
        }
        let cut = cutscene_of(&engine, hero);
        let reply = reply_for(hero, &cut, "Hero", "ok".into());
        assert!(
            reply.problems.iter().any(|p| p.contains("jump cut")),
            "{:?}",
            reply.problems
        );
    }
}
