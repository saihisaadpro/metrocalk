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
    Cutscene, Delivery, Mood, ShotAngle, ShotMove, ShotRecipe, ShotSize, MAX_SECONDS, MAX_SHOTS,
    MIN_SECONDS,
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

/// One choice on one framing axis, in the author's language.
///
/// The shot inspector's three dropdowns are published by the side that VALIDATES them. The
/// alternative — and what a first draft of the panel did — is a `const SIZES = [...]` in TypeScript
/// mirroring a `match` in Rust, which is the same one-contract-stated-twice ADR-134 is about: rename
/// a variant and the option list keeps offering a word the engine will refuse.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FramingOption {
    /// The wire value — the serde name of the enum variant.
    pub value: &'static str,
    /// The word the author reads.
    pub label: &'static str,
    /// What choosing it does, one line.
    pub blurb: &'static str,
}

/// Everything the shot inspector may offer, and the bounds it must respect.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FramingCatalog {
    /// How much of the frame the subject fills.
    pub sizes: Vec<FramingOption>,
    /// Where the camera stands, relative to the subject's facing.
    pub angles: Vec<FramingOption>,
    /// What the camera does over the shot's length.
    pub motions: Vec<FramingOption>,
    /// The shortest a shot may be authored, seconds.
    pub min_seconds: f32,
    /// The longest, seconds.
    pub max_seconds: f32,
    /// The most shots one cutscene may hold.
    pub max_shots: usize,
    /// The moves for which `amount` changes nothing. The strength control is disabled — with a
    /// reason — on these, rather than offering a dial that silently does nothing.
    pub still_motions: Vec<&'static str>,
    /// The frames a cutscene may be DELIVERED in — the shape the shots are composed for.
    ///
    /// Published from [`Delivery::all`] rather than listed here, so the picker, the parser and the
    /// aspect ratio the solver fits against are one list. A second copy in TypeScript is how an option
    /// comes to offer a word the engine refuses.
    pub deliveries: Vec<FramingOption>,
}

/// One line of what choosing a delivery frame does. Beside `Delivery::label`, not inside it, because
/// this sentence is authoring guidance and the label is the name of a thing.
fn delivery_blurb(delivery: Delivery) -> &'static str {
    match delivery {
        Delivery::Viewport => "Compose for the stage as it is now — no bars, and the framing follows the panels you open",
        Delivery::Widescreen => "The broadcast and web default",
        Delivery::Scope => "Anamorphic scope — the widest frame, and the most bar",
        Delivery::Academy => "Classic 4:3, taller than it is wide is not",
        Delivery::Square => "Square, for feeds that crop everything else",
        Delivery::Vertical => "Vertical, for a phone held upright",
    }
}

/// The framing vocabulary, published once.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "an eighteen-option data table; splitting it hides the vocabulary, as for `shot_specs`"
)]
pub fn framing_catalog() -> FramingCatalog {
    FramingCatalog {
        sizes: vec![
            FramingOption {
                value: "extreme_wide",
                label: "Distant",
                blurb: "The subject is a speck in its world — scale before anything else",
            },
            FramingOption {
                value: "wide",
                label: "Wide",
                blurb: "The whole subject with generous air around it",
            },
            FramingOption {
                value: "full",
                label: "Full",
                blurb: "The subject fills most of the height",
            },
            FramingOption {
                value: "medium",
                label: "Medium",
                blurb: "Closer — detail starts to read",
            },
            FramingOption {
                value: "close",
                label: "Close",
                blurb: "Tight on the subject",
            },
            FramingOption {
                value: "extreme_close",
                label: "Very close",
                blurb: "Tighter than the subject — one thing, very large",
            },
        ],
        angles: vec![
            FramingOption {
                value: "front",
                label: "Front",
                blurb: "Facing the subject head-on",
            },
            FramingOption {
                value: "three_quarter",
                label: "Three-quarter",
                blurb: "The workhorse — off to one side, slightly above",
            },
            FramingOption {
                value: "profile",
                label: "Profile",
                blurb: "Directly to the side",
            },
            FramingOption {
                value: "behind",
                label: "Behind",
                blurb: "Over its shoulder, looking where it looks",
            },
            FramingOption {
                value: "low",
                label: "From below",
                blurb: "The subject towers over the camera",
            },
            FramingOption {
                value: "high",
                label: "From above",
                blurb: "Looking down — the subject is small",
            },
        ],
        motions: vec![
            FramingOption {
                value: "hold",
                label: "Hold",
                blurb: "Locked off — the camera does not move",
            },
            FramingOption {
                value: "push_in",
                label: "Push in",
                blurb: "Creep toward the subject",
            },
            FramingOption {
                value: "pull_out",
                label: "Pull out",
                blurb: "Drift away to show what is around it",
            },
            FramingOption {
                value: "orbit",
                label: "Orbit",
                blurb: "Circle the subject so every side reads",
            },
            FramingOption {
                value: "crane_up",
                label: "Crane up",
                blurb: "Rise while holding the aim",
            },
            FramingOption {
                value: "crane_down",
                label: "Crane down",
                blurb: "Descend while holding the aim",
            },
        ],
        min_seconds: MIN_SECONDS,
        max_seconds: MAX_SECONDS,
        max_shots: MAX_SHOTS,
        still_motions: vec!["hold"],
        deliveries: Delivery::all()
            .into_iter()
            .map(|delivery| FramingOption {
                value: delivery.key(),
                label: delivery.label(),
                blurb: delivery_blurb(delivery),
            })
            .collect(),
    }
}

fn size_from_wire(value: &str) -> Option<ShotSize> {
    Some(match value {
        "extreme_wide" => ShotSize::ExtremeWide,
        "wide" => ShotSize::Wide,
        "full" => ShotSize::Full,
        "medium" => ShotSize::Medium,
        "close" => ShotSize::Close,
        "extreme_close" => ShotSize::ExtremeClose,
        _ => return None,
    })
}

fn angle_from_wire(value: &str) -> Option<ShotAngle> {
    Some(match value {
        "front" => ShotAngle::Front,
        "three_quarter" => ShotAngle::ThreeQuarter,
        "profile" => ShotAngle::Profile,
        "behind" => ShotAngle::Behind,
        "low" => ShotAngle::Low,
        "high" => ShotAngle::High,
        _ => return None,
    })
}

fn motion_from_wire(value: &str) -> Option<ShotMove> {
    Some(match value {
        "hold" => ShotMove::Hold,
        "push_in" => ShotMove::PushIn,
        "pull_out" => ShotMove::PullOut,
        "orbit" => ShotMove::Orbit,
        "crane_up" => ShotMove::CraneUp,
        "crane_down" => ShotMove::CraneDown,
        _ => return None,
    })
}

/// Something the cinematics layer will not do, said in the author's language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CinemaError {
    /// No such card.
    UnknownShot(String),
    /// No such pacing mood.
    UnknownMood(String),
    /// No such delivery frame.
    UnknownDelivery(String),
    /// The object is gone.
    MissingEntity,
    /// The shot list is full.
    TooMany(String),
    /// The index does not name a shot in this cutscene.
    NoSuchShot,
    /// A number outside the bounds the engine states, said WITH the bounds.
    OutOfRange(String),
    /// A framing word this vocabulary does not have, named with the axis it was offered on.
    UnknownFraming {
        /// "shot size", "camera angle" or "camera move".
        axis: &'static str,
        /// What the caller sent.
        value: String,
    },
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
            Self::UnknownDelivery(frame) => {
                write!(f, "there is no delivery frame called \"{frame}\"")
            }
            Self::MissingEntity => write!(f, "that object is no longer in the scene"),
            Self::NoSuchShot => write!(f, "that shot is already gone"),
            Self::UnknownFraming { axis, value } => {
                write!(f, "there is no {axis} called \"{value}\"")
            }
            Self::TooMany(m) | Self::OutOfRange(m) | Self::Blocked(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for CinemaError {}

/// One shot as a TIMELINE can draw it — the structured form of the sentence.
///
/// The panel used to be handed `reads: Vec<String>` and a single total, so a cutscene could only ever
/// render as a bulleted list: no per-shot length to show, no start time to lay a chip against, no
/// framing to edit in place, and no way to say which of two shots the author was looking at. Every
/// number here already existed inside [`Cutscene`] — [`Cutscene::effective_shot_seconds`] has been
/// there since the first cutscene shipped. The reply simply never carried them across the boundary,
/// so the engine knew what time it was and the editor did not.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotRow {
    /// The shot's stable id — a reorder moves it, never renumbers it.
    pub id: String,
    /// Its position in the list, 0-based, so a control can act on it without counting.
    pub index: usize,
    /// The sentence, unchanged from what the bulleted list used to show.
    pub reads: String,
    /// The AUTHORED length in seconds — the number the duration control edits.
    pub seconds: f32,
    /// What it actually runs for once the cutscene's mood has scaled it. `Calm` is 2.5x, so these
    /// two are usually different numbers and only one of them is a duration on the timeline.
    pub effective_seconds: f32,
    /// Where the shot starts on the cutscene's own clock, seconds — the chip's left edge.
    pub start_seconds: f32,
    /// The first instant this shot is on screen ALONE, seconds on the cutscene clock — what a UI
    /// seeks to when the user opens this shot. Absolute, so no caller does arithmetic on a boundary
    /// the solver owns.
    pub open_seconds: f32,
    /// How long this shot takes to become itself, seconds — its opening blend, `0.0` on the first.
    ///
    /// On the wire because `start_seconds` is the one instant of a shot at which that shot is NOT
    /// what you see: the transition weight there is zero, so the frame is the END of the shot
    /// before. "Open shot 3" has to mean `start_seconds + blend_seconds`, and the number that
    /// defines the window comes from the side that draws it.
    pub blend_seconds: f32,
    /// How much of the frame the subject fills.
    pub size: ShotSize,
    /// Where the camera stands, relative to the subject's facing.
    pub angle: ShotAngle,
    /// What the camera does over the shot.
    pub motion: ShotMove,
    /// How strong that move is, 0..1. Inert for `hold`.
    pub amount: f32,
    /// The object this shot FRAMES — not necessarily the one the cutscene hangs on.
    pub subject: String,
    /// That object's display name. Resolved per shot rather than per cutscene: a shot may frame
    /// something other than its owner, and before this every line in the list was captioned with the
    /// OWNER's name, so an establishing wide of the whole assembly read as a wide of the one part.
    pub subject_name: String,
}

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
    /// The frame this cutscene is composed and delivered in.
    pub delivery: Delivery,
    /// The whole cutscene read back as sentences — one line per shot.
    ///
    /// Derived from `rows` at construction, in one place, so the two cannot disagree. It stays on the
    /// reply because three `.exe` E2E specs read it as the cheapest possible assertion that a shot
    /// was authored at all.
    pub reads: Vec<String>,
    /// The same shots with their numbers — what the timeline draws and the shot inspector edits.
    pub rows: Vec<ShotRow>,
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
        return Err(CinemaError::NoSuchShot);
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

/// A change to one shot's framing. Every axis is optional: the inspector edits one at a time, and an
/// absent axis means "leave it alone", never "reset it".
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FramingEdit {
    /// A [`FramingCatalog::sizes`] value.
    pub size: Option<String>,
    /// A [`FramingCatalog::angles`] value.
    pub angle: Option<String>,
    /// A [`FramingCatalog::motions`] value.
    pub motion: Option<String>,
    /// How strong the move is, 0..=1.
    pub amount: Option<f32>,
}

/// The ops that set ONE shot's authored length.
///
/// Rounded to the tenth of a second the panel shows and the sentence reads back, so the number the
/// author sees is the number that was stored — a slider that stores 2.4999998 and reads back "2.5s"
/// is a control the user cannot return to a value they just left.
///
/// # Errors
/// [`CinemaError::NoSuchShot`] for an index that is not there, [`CinemaError::OutOfRange`] outside
/// the stated bounds (which the message names), [`CinemaError::MissingEntity`] if the object is gone.
pub fn set_shot_seconds_ops<W: World>(
    engine: &Engine<W>,
    entity: EntityId,
    index: usize,
    seconds: f32,
) -> Result<(Vec<Op>, Cutscene), CinemaError> {
    if !engine.entity_exists(entity) {
        return Err(CinemaError::MissingEntity);
    }
    if !seconds.is_finite() || seconds < MIN_SECONDS || seconds > MAX_SECONDS {
        return Err(CinemaError::OutOfRange(format!(
            "a shot runs between {MIN_SECONDS}s and {MAX_SECONDS}s — {seconds:.1}s is outside that"
        )));
    }
    let mut cut = cutscene_of(engine, entity);
    let shot = cut.shots.get_mut(index).ok_or(CinemaError::NoSuchShot)?;
    shot.seconds = (seconds * 10.0).round() / 10.0;
    Ok((write_ops(entity, &cut, false), cut))
}

/// The ops that move one shot to another position in the list.
///
/// Order IS the sequence, and until this existed the only way to change it was to delete a shot and
/// add it again — which appends at the end, so "move shot 1 later" meant re-authoring everything
/// after it.
///
/// # Errors
/// [`CinemaError::NoSuchShot`] for an index that is not there; [`CinemaError::OutOfRange`] when the
/// move would change nothing.
pub fn move_shot_ops<W: World>(
    engine: &Engine<W>,
    entity: EntityId,
    from: usize,
    to: usize,
) -> Result<(Vec<Op>, Cutscene), CinemaError> {
    if !engine.entity_exists(entity) {
        return Err(CinemaError::MissingEntity);
    }
    let mut cut = cutscene_of(engine, entity);
    if from >= cut.shots.len() {
        return Err(CinemaError::NoSuchShot);
    }
    // Clamped rather than refused: "move the last shot later" is a gesture with an obvious meaning
    // and no destination, and the control that sends it is already disabled at the ends.
    let to = to.min(cut.shots.len() - 1);
    if from == to {
        return Err(CinemaError::OutOfRange(
            "that shot is already in that position".into(),
        ));
    }
    let shot = cut.shots.remove(from);
    cut.shots.insert(to, shot);
    Ok((write_ops(entity, &cut, false), cut))
}

/// The ops that re-frame one shot in place.
///
/// The catalogue card is a whole framing decision so the FIRST click looks good; this is how the
/// author disagrees with it afterwards without losing the shot's position, its length or its id.
///
/// # Errors
/// [`CinemaError::NoSuchShot`], [`CinemaError::UnknownFraming`] naming the axis and the word, or
/// [`CinemaError::OutOfRange`] for a strength outside 0..=1.
pub fn set_shot_framing_ops<W: World>(
    engine: &Engine<W>,
    entity: EntityId,
    index: usize,
    edit: &FramingEdit,
) -> Result<(Vec<Op>, Cutscene), CinemaError> {
    if !engine.entity_exists(entity) {
        return Err(CinemaError::MissingEntity);
    }
    // Every word is resolved BEFORE anything is written, so a request naming one good axis and one
    // bad one changes neither — a half-applied edit is the kind of state an undo cannot describe.
    let size = edit
        .size
        .as_deref()
        .map(|v| {
            size_from_wire(v).ok_or_else(|| CinemaError::UnknownFraming {
                axis: "shot size",
                value: v.to_string(),
            })
        })
        .transpose()?;
    let angle = edit
        .angle
        .as_deref()
        .map(|v| {
            angle_from_wire(v).ok_or_else(|| CinemaError::UnknownFraming {
                axis: "camera angle",
                value: v.to_string(),
            })
        })
        .transpose()?;
    let motion = edit
        .motion
        .as_deref()
        .map(|v| {
            motion_from_wire(v).ok_or_else(|| CinemaError::UnknownFraming {
                axis: "camera move",
                value: v.to_string(),
            })
        })
        .transpose()?;
    if let Some(amount) = edit.amount {
        if !amount.is_finite() || !(0.0..=1.0).contains(&amount) {
            return Err(CinemaError::OutOfRange(
                "move strength runs from 0 to 1".into(),
            ));
        }
    }

    let mut cut = cutscene_of(engine, entity);
    let shot = cut.shots.get_mut(index).ok_or(CinemaError::NoSuchShot)?;
    if let Some(size) = size {
        shot.size = size;
    }
    if let Some(angle) = angle {
        shot.angle = angle;
    }
    if let Some(motion) = motion {
        shot.motion = motion;
    }
    if let Some(amount) = edit.amount {
        shot.amount = (amount * 100.0).round() / 100.0;
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
    // PACING SCALES SHOT LENGTHS, AND THERE ARE NONE. Without this the mood control on an object with
    // no cutscene wrote a `Cinematic` component holding an empty shot list — the exact husk
    // `remove_shot_ops` goes out of its way to delete when the last shot is removed, arriving by the
    // other door. It also made an undoable commit that changed nothing a user could see.
    if cut.shots.is_empty() {
        return Err(CinemaError::OutOfRange(
            "pacing scales shot lengths, and this object has no shots yet — add one first".into(),
        ));
    }
    cut.mood = match mood {
        "calm" => Mood::Calm,
        "normal" => Mood::Normal,
        "tense" => Mood::Tense,
        other => return Err(CinemaError::UnknownMood(other.to_string())),
    };
    Ok((write_ops(entity, &cut, false), cut))
}

/// Set the frame the cutscene is composed and DELIVERED in (one undoable commit).
///
/// Refused on an empty cutscene for the same reason pacing is: the delivery frame is an instruction to
/// the shot solver, so setting one where there are no shots writes a `Cinematic` husk and changes
/// nothing a user can see.
pub fn set_delivery_ops<W: World>(
    engine: &Engine<W>,
    entity: EntityId,
    delivery: &str,
) -> Result<(Vec<Op>, Cutscene), CinemaError> {
    let mut cut = cutscene_of(engine, entity);
    if cut.shots.is_empty() {
        return Err(CinemaError::OutOfRange(
            "a delivery frame is what the shots are composed for, and this object has no shots yet — add one first".into(),
        ));
    }
    cut.delivery = Delivery::from_key(delivery)
        .ok_or_else(|| CinemaError::UnknownDelivery(delivery.into()))?;
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

/// The rows a timeline draws: each shot with its own length, its start on the cutscene clock, its
/// framing, and the name of whatever it films.
fn rows_of(
    cut: &Cutscene,
    owner_name: &str,
    subject_name: &dyn Fn(&str) -> Option<String>,
) -> Vec<ShotRow> {
    let mut start = 0.0_f32;
    let mut rows = Vec::with_capacity(cut.shots.len());
    for (index, shot) in cut.shots.iter().enumerate() {
        let effective = cut.effective_shot_seconds(index).unwrap_or(shot.seconds);
        // An unresolvable subject falls back to the cutscene's owner rather than to a raw loro key:
        // "1_37" in a shot list is not a name, and the sentence is the thing the author reads.
        let who = subject_name(&shot.subject).unwrap_or_else(|| owner_name.to_string());
        rows.push(ShotRow {
            id: shot.id.clone(),
            index,
            reads: describe_shot_with_seconds(shot, &who, effective),
            seconds: shot.seconds,
            effective_seconds: effective,
            start_seconds: start,
            open_seconds: cut.opens_at(index),
            blend_seconds: cut.blend_into(index),
            size: shot.size,
            angle: shot.angle,
            motion: shot.motion,
            amount: shot.amount,
            subject: shot.subject.clone(),
            subject_name: who,
        });
        start += effective;
    }
    rows
}

/// Build the reply for a cutscene, including the continuity warnings.
///
/// Every shot is captioned with the cutscene's owner. Correct only while every shot films its owner —
/// see [`reply_with_names`], which the shell uses.
#[must_use]
pub fn reply_for(entity: EntityId, cut: &Cutscene, name: &str, message: String) -> CinemaReply {
    reply_with_names(entity, cut, name, message, &|_| None)
}

/// The same reply, with each shot's own subject resolved to a display name by the caller.
#[must_use]
pub fn reply_with_names(
    entity: EntityId,
    cut: &Cutscene,
    owner_name: &str,
    message: String,
    subject_name: &dyn Fn(&str) -> Option<String>,
) -> CinemaReply {
    let rows = rows_of(cut, owner_name, subject_name);
    CinemaReply {
        entity: Some(entity.to_loro_key()),
        shots: cut.shots.len(),
        seconds: cut.seconds(),
        mood: cut.mood,
        delivery: cut.delivery,
        // One producer. `reads` is the flattened projection of `rows`, never a second computation of
        // the same sentences.
        reads: rows.iter().map(|row| row.reads.clone()).collect(),
        rows,
        problems: cut.problems(),
        message,
        reason: None,
    }
}

/// What the cutscene timeline's viewport preview answers with: the frame that is now on the stage.
///
/// WHY THE POSE IS ON THE WIRE. `eye`/`look_at`/`fov_deg` are not decoration and not a debug read —
/// they are the only externally checkable evidence that the preview moved the camera to a *particular*
/// place rather than to any place. A screenshot proves something is drawn; these three numbers are
/// what a test can assert against the same solver Play runs, which is what makes "the preview is the
/// frame Play films" a claim rather than a hope.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CinemaPreviewReply {
    /// Whether the cutscene camera is holding the viewport right now. `false` after a successful
    /// exit AND after any refusal, so one field answers "who has the camera" in every case.
    pub active: bool,
    /// The object whose cutscene is on screen.
    pub entity: Option<String>,
    /// Where on the cutscene clock this frame is, seconds — clamped into the cut, so the number the
    /// panel echoes back is the number that was actually filmed rather than the one asked for.
    pub seconds: f32,
    /// Which shot is on screen, 0-based. `None` when nothing is being previewed.
    pub shot_index: Option<usize>,
    /// How many shots the cutscene holds, so "shot 2 of 5" can be said by a surface that never read
    /// the shot list — the stage badge is 700px from the panel that did.
    pub shots: usize,
    /// That shot's sentence, from the same producer the timeline's clips are labelled by.
    pub reads: String,
    /// The display name of the object the shot FRAMES — not necessarily the cutscene's owner.
    pub subject_name: String,
    /// How far through the shot this moment is, 0..1.
    pub progress: f32,
    /// True while this frame is a transition between two shots, so the read-out can say so instead of
    /// naming one shot and drawing the other.
    pub blending: bool,
    /// Where the camera stands, world units.
    pub eye: [f32; 3],
    /// The point it is aimed at.
    pub look_at: [f32; 3],
    /// Vertical field of view, degrees.
    pub fov_deg: f32,
    /// Friendly summary.
    pub message: String,
    /// Set iff the camera did not move, and says why.
    pub reason: Option<String>,
}

impl CinemaPreviewReply {
    /// A refusal that moved nothing, explained. `active: false` is part of the contract: a refusal
    /// must never leave a control believing the viewport is held.
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

/// The last instant of a cutscene that is still INSIDE one of its shots.
///
/// [`Cutscene::playback_at`] is a half-open lookup — `t < end` — so the total running time is the
/// first moment that belongs to no shot, and asking for it answers `None`. Play never notices,
/// because that is exactly how it learns the cut is over. A scrubbed playhead does: dragging it to
/// the right-hand end of the lane is the most ordinary gesture there is, and it would have gone dark.
/// One epsilon, defined once, so the panel and the shell cannot disagree about where the end is.
#[must_use]
pub fn preview_time(cut: &Cutscene, seconds: f32) -> f32 {
    let total = cut.seconds();
    if !total.is_finite() || total <= 0.0 {
        return 0.0;
    }
    // A millisecond is finer than any frame the engine draws and coarser than f32 noise at the
    // durations a cut can hold (12 shots x 20 s x 2.5 = 600 s, where an ulp is ~6e-5).
    let last = (total - 1.0e-3).max(0.0);
    if seconds.is_finite() {
        seconds.clamp(0.0, last)
    } else {
        0.0
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

    /// Exact float equality, stated as BITS.
    ///
    /// `clippy::float_cmp` is right about what it usually catches — a value accumulated through
    /// arithmetic compared with `==` — and wrong about this: `preview_time` RETURNS these constants
    /// on the branches under test, it does not compute toward them. An epsilon would accept a clamp
    /// that missed the end of the cut by a hair, which is the whole failure the tests exist for.
    /// Bits rather than `==` also refuses `-0.0`, which `==` would wave through.
    fn exactly(actual: f32, expected: f32) -> bool {
        actual.to_bits() == expected.to_bits()
    }

    /// A cutscene of three hero shots, authored through the same command the panel uses.
    fn three_shot_cut(engine: &mut Engine<FlecsWorld>, owner: EntityId) -> Cutscene {
        for _ in 0..3 {
            let (ops, _) = add_shot_ops(engine, owner, "hero", owner).expect("shot");
            engine.commit("cinema-shot", ops).expect("commits");
        }
        cutscene_of(engine, owner)
    }

    #[test]
    fn a_playhead_dragged_to_the_very_end_still_lands_inside_a_shot() {
        let (mut engine, _scene) = world();
        let owner = spawn(&mut engine);
        let cut = three_shot_cut(&mut engine, owner);
        let total = cut.seconds();
        assert!(total > 0.0);

        // The failure this exists to stop: `playback_at` is half-open, so the total running time is
        // the first instant belonging to NO shot. Play never notices because that is exactly how it
        // learns the cut is over; a playhead dragged to the right-hand end of the lane would have
        // gone dark on the most ordinary gesture the timeline has.
        assert!(
            cut.playback_at(total).is_none(),
            "the premise: the end of the cut is past the last shot"
        );
        let at = preview_time(&cut, total);
        assert!(at < total, "clamped inside: {at} vs {total}");
        let playback = cut
            .playback_at(at)
            .expect("the clamped moment is inside a shot");
        assert_eq!(
            playback.index,
            cut.shots.len() - 1,
            "and it is inside the LAST shot, not wherever rounding landed"
        );

        // Far past the end clamps to the same instant — one end, not a scale of them.
        assert!(exactly(preview_time(&cut, total + 500.0), at));
    }

    #[test]
    fn opening_a_shot_means_the_instant_it_becomes_itself_not_the_instant_it_starts() {
        let (mut engine, _scene) = world();
        let owner = spawn(&mut engine);
        let cut = three_shot_cut(&mut engine, owner);
        let rows = rows_of(&cut, "Owner", &|_| None);

        // The first shot never blends: opening it IS its start.
        assert!(exactly(rows[0].blend_seconds, 0.0));
        let opening = cut.playback_at(rows[0].start_seconds).expect("inside");
        assert_eq!(opening.index, 0);
        assert!(opening.blend_from.is_none());

        for row in &rows[1..] {
            assert!(
                row.blend_seconds > 0.0,
                "shot {} opens over nothing",
                row.index
            );
            // THE DEFECT THIS PINS, measured on the packaged `.exe` before it was fixed: at a shot's
            // START the transition weight is zero, so `blend_camera` returns the PREVIOUS shot's
            // pose in full. Clicking clip 3 showed the end of shot 2, and re-framing shot 3 from
            // there moved the camera 0.0000 units.
            let at_start = cut.playback_at(row.start_seconds).expect("inside");
            assert_eq!(at_start.index, row.index);
            assert_eq!(
                at_start.blend_from,
                Some((row.index - 1, 0.0)),
                "the start of a shot is entirely the shot before it"
            );

            // ...and at the instant the engine calls its opening, it is the shot, alone.
            let opened = cut.playback_at(row.open_seconds).expect("inside");
            assert_eq!(opened.index, row.index);
            assert_eq!(
                opened.blend_from, None,
                "shot {} is still blending after its own blend window",
                row.index
            );
        }
    }

    #[test]
    fn preview_time_refuses_to_produce_a_moment_that_is_not_on_the_clock() {
        let (mut engine, _scene) = world();
        let owner = spawn(&mut engine);
        let cut = three_shot_cut(&mut engine, owner);

        assert!(
            exactly(preview_time(&cut, -4.0), 0.0),
            "before the start is the start"
        );
        assert!(
            exactly(preview_time(&cut, f32::NAN), 0.0),
            "NaN is not a moment"
        );
        assert!(
            exactly(preview_time(&cut, f32::NEG_INFINITY), 0.0),
            "nor is negative infinity"
        );
        assert!(
            preview_time(&cut, f32::INFINITY) < cut.seconds(),
            "nor is infinity"
        );
        // A moment genuinely inside the cut is handed back UNCHANGED: a clamp that quietly rounded
        // every request would make the panel's read-out disagree with the frame on the stage.
        let inside = cut.seconds() * 0.5;
        assert!((preview_time(&cut, inside) - inside).abs() < 1.0e-6);

        // An empty cutscene has no moments at all, and answers with the only number it can.
        assert!(exactly(preview_time(&Cutscene::default(), 3.0), 0.0));
    }

    #[test]
    fn a_refused_preview_never_claims_the_viewport() {
        // `active` is the field every surface keys off. A refusal that left it true would leave a
        // stage badge offering an exit from a preview that never started.
        let refusal = CinemaPreviewReply::refusal("Play is driving the camera");
        assert!(!refusal.active);
        assert_eq!(refusal.shot_index, None);
        assert_eq!(
            refusal.reason.as_deref(),
            Some("Play is driving the camera")
        );
        assert_eq!(refusal.message, "Play is driving the camera");
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
    fn pacing_on_an_object_with_no_shots_is_refused_rather_than_writing_an_empty_husk() {
        let (mut engine, _scene) = world();
        let hero = spawn(&mut engine);
        let msg = set_mood_ops(&engine, hero, "calm")
            .expect_err("nothing to pace")
            .to_string();
        assert!(msg.contains("no shots yet"), "{msg}");
        assert!(
            !engine.components_of(hero).contains_key(CINEMA_COMPONENT),
            "a refused mood must leave no component behind"
        );
    }

    #[test]
    fn a_shot_length_is_editable_between_the_stated_bounds_and_refused_outside_them() {
        let (mut engine, _scene) = world();
        let hero = spawn(&mut engine);
        let (ops, _) = add_shot_ops(&engine, hero, "hero", hero).expect("shot");
        engine.commit("cinema-shot", ops).expect("commits");
        assert!((cutscene_of(&engine, hero).shots[0].seconds - 2.5).abs() < 1.0e-5);

        let (ops, cut) = set_shot_seconds_ops(&engine, hero, 0, 6.4).expect("in range");
        engine.commit("cinema-seconds", ops).expect("commits");
        assert!((cut.shots[0].seconds - 6.4).abs() < 1.0e-5);
        assert!((cutscene_of(&engine, hero).seconds() - 6.4).abs() < 1.0e-5);
        assert!(engine.undo(), "one Ctrl-Z restores the authored length");
        assert!((cutscene_of(&engine, hero).shots[0].seconds - 2.5).abs() < 1.0e-5);

        // Rounded to the tenth the panel reads back, so the number shown is the number stored.
        let (ops, cut) = set_shot_seconds_ops(&engine, hero, 0, 3.333_333).expect("in range");
        engine.commit("cinema-seconds", ops).expect("commits");
        assert!(
            (cut.shots[0].seconds - 3.3).abs() < 1.0e-5,
            "{}",
            cut.shots[0].seconds
        );

        // Both bounds refuse, and the refusal names them.
        for bad in [0.05_f32, 25.0, f32::NAN] {
            let msg = set_shot_seconds_ops(&engine, hero, 0, bad)
                .expect_err("out of range")
                .to_string();
            assert!(msg.contains("0.2s") && msg.contains("20s"), "{msg}");
        }
        assert!(matches!(
            set_shot_seconds_ops(&engine, hero, 7, 2.0),
            Err(CinemaError::NoSuchShot)
        ));
    }

    #[test]
    fn a_shot_can_be_moved_along_the_sequence_and_keeps_its_own_id() {
        let (mut engine, _scene) = world();
        let hero = spawn(&mut engine);
        for kind in ["establish", "hero", "closeup"] {
            let (ops, _) = add_shot_ops(&engine, hero, kind, hero).expect("shot");
            engine.commit("cinema-shot", ops).expect("commits");
        }
        let before: Vec<String> = cutscene_of(&engine, hero)
            .shots
            .iter()
            .map(|s| s.id.clone())
            .collect();

        let (ops, cut) = move_shot_ops(&engine, hero, 0, 2).expect("a real move");
        engine.commit("cinema-move", ops).expect("commits");
        let after: Vec<String> = cut.shots.iter().map(|s| s.id.clone()).collect();
        assert_eq!(
            after,
            vec![before[1].clone(), before[2].clone(), before[0].clone()]
        );
        // The ids TRAVELLED. A reorder that renumbered them would make every id in the document a
        // statement about position rather than identity, and two peers editing at once would collide.
        let mut sorted = after.clone();
        sorted.sort();
        let mut expected = before.clone();
        expected.sort();
        assert_eq!(sorted, expected);

        assert!(engine.undo(), "one Ctrl-Z restores the order");
        let restored: Vec<String> = cutscene_of(&engine, hero)
            .shots
            .iter()
            .map(|s| s.id.clone())
            .collect();
        assert_eq!(restored, before);

        // A move that would change nothing is refused rather than committed as an empty undo step.
        assert!(matches!(
            move_shot_ops(&engine, hero, 1, 1),
            Err(CinemaError::OutOfRange(_))
        ));
        // Past the end clamps to the end; past the start of the list does not exist.
        let (_, clamped) = move_shot_ops(&engine, hero, 0, 99).expect("clamped, not refused");
        assert_eq!(clamped.shots[2].id, before[0]);
        assert!(matches!(
            move_shot_ops(&engine, hero, 9, 0),
            Err(CinemaError::NoSuchShot)
        ));
    }

    #[test]
    fn a_shot_is_re_framed_in_place_and_a_bad_word_changes_nothing() {
        let (mut engine, _scene) = world();
        let hero = spawn(&mut engine);
        let (ops, _) = add_shot_ops(&engine, hero, "hero", hero).expect("shot");
        engine.commit("cinema-shot", ops).expect("commits");
        let id = cutscene_of(&engine, hero).shots[0].id.clone();

        let (ops, cut) = set_shot_framing_ops(
            &engine,
            hero,
            0,
            &FramingEdit {
                size: Some("close".into()),
                motion: Some("orbit".into()),
                amount: Some(0.5),
                ..FramingEdit::default()
            },
        )
        .expect("known words");
        engine.commit("cinema-framing", ops).expect("commits");
        let shot = &cut.shots[0];
        assert_eq!(shot.size, ShotSize::Close);
        assert_eq!(shot.motion, ShotMove::Orbit);
        assert!((shot.amount - 0.5).abs() < 1.0e-5);
        // The axes NOT named are untouched, and the shot keeps its identity and its length.
        assert_eq!(shot.angle, ShotAngle::ThreeQuarter);
        assert!((shot.seconds - 2.5).abs() < 1.0e-5);
        assert_eq!(shot.id, id);

        // A request naming one good axis and one bad one writes NEITHER — a half-applied edit is a
        // state no undo step can describe.
        let err = set_shot_framing_ops(
            &engine,
            hero,
            0,
            &FramingEdit {
                angle: Some("front".into()),
                motion: Some("dolly_zoom".into()),
                ..FramingEdit::default()
            },
        )
        .expect_err("unknown move");
        assert!(err.to_string().contains("dolly_zoom"), "{err}");
        assert!(err.to_string().contains("camera move"), "{err}");
        assert_eq!(
            cutscene_of(&engine, hero).shots[0].angle,
            ShotAngle::ThreeQuarter
        );

        assert!(matches!(
            set_shot_framing_ops(
                &engine,
                hero,
                0,
                &FramingEdit {
                    amount: Some(4.0),
                    ..FramingEdit::default()
                }
            ),
            Err(CinemaError::OutOfRange(_))
        ));
    }

    #[test]
    fn the_framing_catalogue_offers_exactly_the_words_the_commands_accept() {
        // The catalogue and the validator are one contract stated in two `match` arms; this is the
        // check that compares them, and it is the whole reason the catalogue is published from Rust
        // rather than written out again in TypeScript.
        let (mut engine, _scene) = world();
        let hero = spawn(&mut engine);
        let (ops, _) = add_shot_ops(&engine, hero, "hero", hero).expect("shot");
        engine.commit("cinema-shot", ops).expect("commits");

        let catalog = framing_catalog();
        assert_eq!(catalog.sizes.len(), 6);
        assert_eq!(catalog.angles.len(), 6);
        assert_eq!(catalog.motions.len(), 6);
        for option in &catalog.sizes {
            set_shot_framing_ops(
                &engine,
                hero,
                0,
                &FramingEdit {
                    size: Some(option.value.into()),
                    ..FramingEdit::default()
                },
            )
            .unwrap_or_else(|e| panic!("size \"{}\" is offered and refused: {e}", option.value));
        }
        for option in &catalog.angles {
            set_shot_framing_ops(
                &engine,
                hero,
                0,
                &FramingEdit {
                    angle: Some(option.value.into()),
                    ..FramingEdit::default()
                },
            )
            .unwrap_or_else(|e| panic!("angle \"{}\" is offered and refused: {e}", option.value));
        }
        for option in &catalog.motions {
            set_shot_framing_ops(
                &engine,
                hero,
                0,
                &FramingEdit {
                    motion: Some(option.value.into()),
                    ..FramingEdit::default()
                },
            )
            .unwrap_or_else(|e| panic!("move \"{}\" is offered and refused: {e}", option.value));
        }
        // ...and the wire values really are the serde names, so a round-trip through the document
        // reads back the option the user picked rather than a default.
        let json = serde_json::to_string(&cutscene_of(&engine, hero)).expect("serialises");
        assert!(
            json.contains("\"three_quarter\"") || json.contains("\"high\""),
            "{json}"
        );
        assert!(
            catalog.still_motions.contains(&"hold"),
            "the panel disables move strength from this list"
        );
        assert!((catalog.min_seconds - MIN_SECONDS).abs() < f32::EPSILON);
        assert!((catalog.max_seconds - MAX_SECONDS).abs() < f32::EPSILON);
        assert_eq!(catalog.max_shots, MAX_SHOTS);
    }

    #[test]
    fn every_row_carries_its_own_start_and_length_and_the_name_of_what_it_films() {
        let (mut engine, _scene) = world();
        let hero = spawn(&mut engine);
        let other = spawn(&mut engine);
        let (ops, _) =
            add_shot_ops(&engine, hero, "establish", other).expect("frames the OTHER one");
        engine.commit("cinema-shot", ops).expect("commits");
        let (ops, _) = add_shot_ops(&engine, hero, "closeup", hero).expect("shot");
        engine.commit("cinema-shot", ops).expect("commits");

        let cut = cutscene_of(&engine, hero);
        let other_key = other.to_loro_key();
        let reply = reply_with_names(hero, &cut, "Weld Gun 7", String::new(), &|key| {
            (key == other_key).then(|| "Assembly Hall".to_string())
        });

        assert_eq!(reply.rows.len(), 2);
        // Laid end to end on the cutscene's own clock.
        assert!((reply.rows[0].start_seconds - 0.0).abs() < 1.0e-5);
        assert!((reply.rows[0].effective_seconds - 2.5).abs() < 1.0e-5);
        assert!((reply.rows[1].start_seconds - 2.5).abs() < 1.0e-5);
        assert!((reply.rows[1].effective_seconds - 1.8).abs() < 1.0e-5);
        assert!(
            (reply.rows[1].start_seconds + reply.rows[1].effective_seconds - reply.seconds).abs()
                < 1.0e-5,
            "the last row must end where the cutscene does"
        );
        // THE NAME IS PER SHOT. Every line used to be captioned with the cutscene's owner, so an
        // establishing wide of the hall read as a wide of the one part inside it.
        assert!(
            reply.rows[0].reads.contains("Assembly Hall"),
            "{}",
            reply.rows[0].reads
        );
        assert!(
            reply.rows[1].reads.contains("Weld Gun 7"),
            "{}",
            reply.rows[1].reads
        );
        assert_eq!(reply.rows[0].subject_name, "Assembly Hall");
        // ...and `reads` is the flattened projection of the rows, never a second computation.
        assert_eq!(
            reply.reads,
            reply
                .rows
                .iter()
                .map(|r| r.reads.clone())
                .collect::<Vec<_>>()
        );

        // Calm stretches every length by 2.5x, and the rows say so rather than the panel guessing.
        let (ops, calm) = set_mood_ops(&engine, hero, "calm").expect("known mood");
        engine.commit("cinema-mood", ops).expect("commits");
        let calm_rows = reply_for(hero, &calm, "Weld Gun 7", String::new()).rows;
        assert!(
            (calm_rows[0].seconds - 2.5).abs() < 1.0e-5,
            "authored is unchanged"
        );
        assert!((calm_rows[0].effective_seconds - 6.25).abs() < 1.0e-4);
        assert!((calm_rows[1].start_seconds - 6.25).abs() < 1.0e-4);
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
