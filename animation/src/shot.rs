//! **Shots** — a camera pose is a pure function of (recipe, tick, live subject), never a baked keyframe.
//!
//! Why a recipe rather than keyframes, stated as fact rather than taste:
//!
//! * the timeline can only author SCALAR channels, so a hand-keyed camera orbit is four independent
//!   `qx/qy/qz/qw` tracks lerped componentwise — which takes the wrong path through the rotation;
//! * a baked pose frames where the subject *was*. Solve it per tick and the shot survives the subject
//!   moving, being re-parented, or being spawned mid-run. That is the whole Cinemachine lesson.
//!
//! The user picks WORDS — a shot size, an angle, a move — and the maths turns them into a camera. Nobody
//! types a distance in metres or a quaternion. Everything here is deterministic (fixed 1/60 dt, integer
//! ticks, no wall clock, no RNG), renderer-free and Loro-free, so it unit-tests in isolation and replays
//! bit-identically.

use serde::{Deserialize, Serialize};

/// How much of the frame the subject should fill — the only "distance" vocabulary the user sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShotSize {
    /// The subject is a speck in its world — establish the place.
    ExtremeWide,
    /// The whole subject with generous air around it.
    Wide,
    /// The subject fills most of the height.
    Full,
    /// Closer — detail reads.
    Medium,
    /// Tight.
    Close,
    /// Tighter than the subject — drama.
    ExtremeClose,
}

impl ShotSize {
    /// The fraction of the frame the subject should occupy. These are the numbers that make "close"
    /// mean something without the user ever typing one.
    #[must_use]
    pub fn occupancy(self) -> f32 {
        match self {
            Self::ExtremeWide => 0.12,
            Self::Wide => 0.25,
            Self::Full => 0.55,
            Self::Medium => 0.72,
            Self::Close => 0.85,
            Self::ExtremeClose => 1.15,
        }
    }

    /// One step wider — the graceful fallback when something blocks the view.
    #[must_use]
    pub fn wider(self) -> Self {
        match self {
            Self::ExtremeClose => Self::Close,
            Self::Close => Self::Medium,
            Self::Medium => Self::Full,
            Self::Full => Self::Wide,
            Self::Wide | Self::ExtremeWide => Self::ExtremeWide,
        }
    }
}

/// Where the camera stands, expressed RELATIVE TO THE SUBJECT'S FACING — so "front" stays the front
/// after the subject turns around.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShotAngle {
    /// Facing the subject head-on.
    Front,
    /// The workhorse: off to one side, slightly above.
    ThreeQuarter,
    /// Directly to the side.
    Profile,
    /// Over its shoulder, looking where it looks.
    Behind,
    /// From below — the subject looms.
    Low,
    /// From above — the subject is small.
    High,
}

impl ShotAngle {
    /// `(yaw offset from the subject's forward, pitch)` in degrees.
    #[must_use]
    pub fn offsets(self) -> (f32, f32) {
        match self {
            Self::Front => (180.0, 8.0),
            Self::ThreeQuarter => (135.0, 10.0),
            Self::Profile => (90.0, 5.0),
            Self::Behind => (0.0, 12.0),
            Self::Low => (135.0, -18.0),
            Self::High => (135.0, 40.0),
        }
    }
}

/// What the camera DOES over the shot's length. A closed vocabulary — eight verbs, no free-form splines.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShotMove {
    /// Locked off.
    Hold,
    /// Creep toward the subject.
    PushIn,
    /// Drift away.
    PullOut,
    /// Circle the subject.
    Orbit,
    /// Rise while holding the aim.
    CraneUp,
    /// Descend while holding the aim.
    CraneDown,
}

/// One shot: who it is about, how it is framed, what it does, and for how long.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotRecipe {
    /// Stable id (never renumbered on reorder).
    pub id: String,
    /// Whose shot this is — a loro key, or a `$subject`/`$other` sentinel resolved at Play.
    pub subject: String,
    /// Framing.
    pub size: ShotSize,
    /// Camera placement relative to the subject's facing.
    pub angle: ShotAngle,
    /// The move.
    #[serde(default = "hold")]
    pub motion: ShotMove,
    /// How strong the move is, 0..1 (a push-in of 0.35 closes 35% of the distance).
    #[serde(default = "default_amount")]
    pub amount: f32,
    /// Length in seconds.
    pub seconds: f32,
}

fn hold() -> ShotMove {
    ShotMove::Hold
}
fn default_amount() -> f32 {
    0.35
}

/// The one global dial. Picking a mood sets pacing and camera weight together, so a user tunes the
/// FEEL of a cutscene with a single choice instead of six numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mood {
    /// Long shots, long blends, heavy camera.
    Calm,
    /// The default.
    #[default]
    Normal,
    /// Short shots, fast cuts, light camera.
    Tense,
}

impl Mood {
    /// Runtime scale applied to authored shot lengths. `Normal` is exactly 1.0 for schema and playback
    /// compatibility; Calm deliberately gives production shots enough breathing room for a three-minute
    /// industrial film without making authors hand-edit every card.
    #[must_use]
    pub fn duration_scale(self) -> f32 {
        match self {
            Self::Calm => 2.5,
            Self::Normal => 1.0,
            Self::Tense => 0.75,
        }
    }

    /// Blend length between shots, seconds.
    #[must_use]
    pub fn blend_seconds(self) -> f32 {
        match self {
            Self::Calm => 0.9,
            Self::Normal => 0.6,
            Self::Tense => 0.25,
        }
    }
    /// The shortest a shot may be before it reads as a mistake.
    #[must_use]
    pub fn min_shot_seconds(self) -> f32 {
        match self {
            Self::Calm => 2.0,
            Self::Normal => 1.4,
            Self::Tense => 0.8,
        }
    }
}

/// A whole cutscene: an ordered shot list plus the one dial.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cutscene {
    /// Schema version, so a future reader can migrate rather than guess.
    #[serde(default = "one")]
    pub version: u32,
    /// The shots, in order.
    #[serde(default)]
    pub shots: Vec<ShotRecipe>,
    /// Pacing + weight.
    #[serde(default)]
    pub mood: Mood,
    /// Black bars, because a cutscene should look like one.
    #[serde(default)]
    pub letterbox: bool,
}

/// One stable playback lookup: the live shot, its local progress, and an optional transition from the
/// preceding shot in this same cutscene. A first shot never carries `blend_from`, which keeps separately
/// directed cutscenes as hard cuts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShotPlayback {
    /// Index of the live shot.
    pub index: usize,
    /// Local progress through the live shot, in `0..=1`.
    pub progress: f32,
    /// `(previous shot index, transition progress)` during the opening blend window.
    pub blend_from: Option<(usize, f32)>,
}

fn one() -> u32 {
    1
}

// `#[serde(default = "one")]` only fires when a field is MISSING from incoming JSON; a derived
// `Default` would still hand out `version: 0`, and since a fresh cutscene starts from `default()` every
// authored file was being stamped version 0. Caught by reading the raw `source` blob in a screenshot of
// the Inspector. Written by hand so the stored version and the schema version cannot drift.
impl Default for Cutscene {
    fn default() -> Self {
        Self {
            version: one(),
            shots: Vec::new(),
            mood: Mood::default(),
            letterbox: false,
        }
    }
}

/// The most shots one cutscene may hold — the stated ceiling, refused with a reason rather than
/// silently truncated.
pub const MAX_SHOTS: usize = 12;
/// Shot length bounds, seconds.
pub const MIN_SECONDS: f32 = 0.2;
/// The longest a single shot may run.
pub const MAX_SECONDS: f32 = 20.0;

impl Cutscene {
    /// Effective runtime length of one shot after the cutscene mood's pacing is applied.
    #[must_use]
    pub fn effective_shot_seconds(&self, index: usize) -> Option<f32> {
        let authored = self.shots.get(index)?.seconds;
        let authored = if authored.is_finite() {
            authored.clamp(MIN_SECONDS, MAX_SECONDS)
        } else {
            MIN_SECONDS
        };
        Some(authored * self.mood.duration_scale())
    }

    /// Total effective running time in seconds. For `Mood::Normal`, valid authored timing is unchanged.
    #[must_use]
    pub fn seconds(&self) -> f32 {
        (0..self.shots.len())
            .filter_map(|index| self.effective_shot_seconds(index))
            .sum()
    }

    /// Resolve playback timing once, including the within-cutscene incoming blend window.
    #[must_use]
    pub fn playback_at(&self, t: f32) -> Option<ShotPlayback> {
        let mut start = 0.0;
        for index in 0..self.shots.len() {
            let duration = self.effective_shot_seconds(index)?;
            let end = start + duration;
            if t < end {
                let local = (t - start).max(0.0);
                let progress = (local / duration.max(1.0e-6)).clamp(0.0, 1.0);
                let blend_from = if index == 0 {
                    None
                } else {
                    let blend_span = self.mood.blend_seconds().min(duration * 0.5);
                    (blend_span > 1.0e-6 && local < blend_span)
                        .then(|| (index - 1, (local / blend_span).clamp(0.0, 1.0)))
                };
                return Some(ShotPlayback {
                    index,
                    progress,
                    blend_from,
                });
            }
            start = end;
        }
        None
    }

    /// Which shot is live at `t` seconds, and how far through it we are (0..1).
    #[must_use]
    pub fn shot_at(&self, t: f32) -> Option<(usize, f32)> {
        self.playback_at(t)
            .map(|playback| (playback.index, playback.progress))
    }

    /// Everything wrong with this cutscene, in the author's language. The continuity checks are the
    /// part no mainstream tool ships: an engine that knows film grammar can warn about a jump cut, and
    /// a warning is cheaper than a viewer noticing.
    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.shots.len() > MAX_SHOTS {
            out.push(format!(
                "a cutscene can hold at most {MAX_SHOTS} shots — this one has {}",
                self.shots.len()
            ));
        }
        for (i, shot) in self.shots.iter().enumerate() {
            let n = i + 1;
            if shot.seconds < MIN_SECONDS {
                out.push(format!(
                    "shot {n} is shorter than {MIN_SECONDS}s — it will not read"
                ));
            }
            if shot.seconds > MAX_SECONDS {
                out.push(format!("shot {n} runs longer than {MAX_SECONDS}s"));
            } else if self
                .effective_shot_seconds(i)
                .is_some_and(|effective| effective < self.mood.min_shot_seconds())
            {
                // Against the EFFECTIVE length, not the authored one. The mood is the dial that decides
                // how long a shot actually runs — Calm stretches every shot by 2.5x — so judging the
                // authored number against a per-mood minimum asks whether a shot is too short while
                // ignoring the only thing that sets its length. It made the two shortest cards in the
                // catalogue (`closeup` 1.8s, `detail` 1.6s) permanently un-authorable in a Calm
                // cutscene, where they in fact run 4.5s and 4.0s: a director following the tool's own
                // warnings could not put a close-up in a calm sequence at all.
                out.push(format!(
                    "shot {n} runs {:.1}s at this pacing — it may feel rushed",
                    self.effective_shot_seconds(i).unwrap_or(shot.seconds)
                ));
            }
        }
        // A jump cut: the same subject, nearly the same framing, cut together. The classic amateur tell.
        for pair in self.shots.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if a.subject == b.subject && a.size == b.size && a.angle == b.angle {
                out.push(format!(
                    "shots on \"{}\" are framed identically back to back — that reads as a jump cut; \
                     change the size or the angle",
                    a.subject
                ));
            }
        }
        // No establishing shot: opening tight leaves the viewer lost.
        if let Some(first) = self.shots.first() {
            if matches!(first.size, ShotSize::Close | ShotSize::ExtremeClose)
                && self.shots.len() > 1
            {
                out.push(
                    "the cutscene opens tight — a wide shot first would establish where we are"
                        .into(),
                );
            }
        }
        out
    }
}

/// What the solver needs to know about the thing being filmed, sampled LIVE each tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SubjectSample {
    /// World-space centre.
    pub center: [f32; 3],
    /// Half-extent of its bounding box; the solver uses the bounding SPHERE so no corner escapes frame.
    pub half_extent: [f32; 3],
    /// The subject's forward (unit). Angles are relative to this, so "front" follows the subject.
    pub forward: [f32; 3],
    /// The room this subject is standing in, when the scene has one.
    ///
    /// It rides on the sample rather than being a separate argument for the same reason
    /// [`CAMERA_FLOOR`] is applied inside [`solve_shot_adjusted`]: the planner and the runtime have to
    /// be looking at the same camera, and a value that reaches one of them by a different route than
    /// the other is exactly how a pose gets validated at one place and filmed at another.
    pub stage: Stage,
}

/// Where a camera is allowed to stand — what the world says about the room, not about the subject.
///
/// [`Stage::OPEN`] is a scene with no room at all, and is the whole of the old behaviour: an engine that
/// has nothing to say here says this, and every placement is then exactly what the direction solved.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stage {
    /// The interior box a camera may stand in, `(lo, hi)`, **already inset by whatever margin the room
    /// wants off its own surfaces**. The solver keeps the eye inside it and does not reason about
    /// thickness, cladding or clearance: that is the room's business, and a solver that had opinions
    /// about it would be a second place where the margin is decided.
    pub room: Option<([f32; 3], [f32; 3])>,
}

impl Stage {
    /// A scene with no room — every camera position is reachable.
    pub const OPEN: Self = Self { room: None };

    /// Bring `eye` inside the room, or leave it exactly where it is.
    ///
    /// Returns the unchanged pose whenever honouring the room would put the lens inside the subject:
    /// a wide card clamped against a near wall can end up closer to a large subject than the framing
    /// solve ever allows, and a camera inside the thing it is filming is a worse frame than the
    /// too-distant one it was trying to fix. Declining leaves the existing negotiation to handle it,
    /// which is what happens today, so this can only ever improve a placement or leave it alone.
    #[must_use]
    fn confine(self, eye: [f32; 3], centre: [f32; 3], min_range: f32) -> [f32; 3] {
        let Some((lo, hi)) = self.room else {
            return eye;
        };
        if !lo.iter().chain(hi.iter()).all(|v| v.is_finite()) {
            return eye;
        }
        let mut confined = eye;
        for axis in 0..3 {
            if lo[axis] > hi[axis] {
                return eye; // a degenerate room is not a room
            }
            confined[axis] = confined[axis].clamp(lo[axis], hi[axis]);
        }
        // EXACT, AND EXACT IS THE POINT. `clamp` returns its input BIT-IDENTICALLY when the value is
        // already inside the range, so this asks "did any axis actually move?" — the one question an
        // epsilon would answer wrongly, by discarding a real clamp of half a millimetre. Introduced
        // with the room in `f557beb` and denied by `clippy::float_cmp` in the workspace job, which is
        // the lint's escape hatch existing for exactly this case.
        #[allow(
            clippy::float_cmp,
            reason = "clamp() is bit-identical when it does nothing; the question is whether it did"
        )]
        if confined == eye {
            return eye;
        }
        let d = [
            confined[0] - centre[0],
            confined[1] - centre[1],
            confined[2] - centre[2],
        ];
        let range = d[0].mul_add(d[0], d[1].mul_add(d[1], d[2] * d[2])).sqrt();
        if !range.is_finite() || range < min_range {
            return eye;
        }
        confined
    }
}

impl SubjectSample {
    /// The bounding-sphere radius — deliberately not the longest half-edge, which under-covers corners.
    #[must_use]
    pub fn radius(&self) -> f32 {
        let [x, y, z] = self.half_extent;
        (x * x + y * y + z * z).sqrt().max(0.05)
    }
}

/// The camera pose a shot resolves to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraSample {
    /// Where the camera is.
    pub eye: [f32; 3],
    /// Where it aims.
    pub look_at: [f32; 3],
    /// Vertical field of view, degrees.
    pub fov_deg: f32,
}

/// The distance at which a sphere of `radius` fills `occupancy` of the frame — the same tangency solve
/// the viewport's frame-all uses, generalised so a shot size can drive it.
#[must_use]
pub fn fit_distance(radius: f32, fov_deg: f32, aspect: f32, occupancy: f32) -> f32 {
    let half_v = (fov_deg.clamp(1.0, 179.0) * 0.5).to_radians();
    let half_h = (half_v.tan() * aspect.max(0.1)).atan();
    let half = half_v.min(half_h).max(0.01);
    (radius / half.sin() / occupancy.max(0.01)).clamp(0.15, 5_000.0)
}

/// Solve one shot at `progress` (0..1 through the shot) against a live subject sample.
///
/// The move is applied as a progress-driven modulation of the base placement, so a push-in is a
/// continuous change of distance rather than two keyframes with a lerp between them.
#[must_use]
pub fn solve_shot(
    shot: &ShotRecipe,
    subject: SubjectSample,
    progress: f32,
    aspect: f32,
    fov_deg: f32,
) -> CameraSample {
    let p = progress.clamp(0.0, 1.0);
    let amount = shot.amount.clamp(0.0, 1.0);
    let radius = subject.radius();
    let base = fit_distance(radius, fov_deg, aspect, shot.size.occupancy());

    // The move changes distance / height / yaw over the shot.
    let (dist_scale, height_extra, yaw_extra) = match shot.motion {
        ShotMove::Hold => (1.0, 0.0, 0.0),
        ShotMove::PushIn => (1.0 - amount * p, 0.0, 0.0),
        ShotMove::PullOut => (1.0 + amount * p, 0.0, 0.0),
        ShotMove::Orbit => (1.0, 0.0, amount * p * 90.0),
        ShotMove::CraneUp => (1.0, amount * p * radius * 4.0, 0.0),
        ShotMove::CraneDown => (1.0, -amount * p * radius * 2.0, 0.0),
    };
    let distance = (base * dist_scale).max(radius * 1.05);

    // Angles are relative to the subject's own facing.
    let fwd = subject.forward;
    let subject_yaw = fwd[0].atan2(fwd[2]);
    let (yaw_off, pitch_deg) = shot.angle.offsets();
    let yaw = subject_yaw + (yaw_off + yaw_extra).to_radians();
    let pitch = pitch_deg.to_radians();

    let horizontal = distance * pitch.cos();
    let eye = [
        subject.center[0] + horizontal * yaw.sin(),
        subject.center[1] + distance * pitch.sin() + height_extra,
        subject.center[2] + horizontal * yaw.cos(),
    ];

    // Headroom: aim slightly above centre so the subject sits on the upper third rather than dead
    // centre — the single cheapest thing that stops a shot looking amateur.
    let look_at = [
        subject.center[0],
        subject.center[1] + subject.half_extent[1] * 0.25,
        subject.center[2],
    ];

    CameraSample {
        eye,
        look_at,
        fov_deg,
    }
}

/// Solve a production camera shot with cinematic ease applied to its within-shot progress.
///
/// [`solve_shot`] deliberately remains the linear geometric primitive so authoring tools can inspect
/// exact progress. Playback uses this entry point: it preserves the same endpoints while removing the
/// mechanical instant start and stop of a linear camera move.
#[must_use]
pub fn solve_shot_eased(
    shot: &ShotRecipe,
    subject: SubjectSample,
    progress: f32,
    aspect: f32,
    fov_deg: f32,
) -> CameraSample {
    solve_shot(shot, subject, ease_in_out(progress), aspect, fov_deg)
}

/// Derive finite perspective clip planes around a filmed subject.
///
/// The near plane follows the closest visible surface without crossing it, while the far plane keeps
/// generous room behind the subject. Inputs are sanitised because imported industrial scenes can span
/// millimetres to kilometres, and a non-finite projection poisons the entire frame.
#[must_use]
pub fn cinematic_clip_planes(subject_radius: f32, camera_distance: f32) -> (f32, f32) {
    const MIN_NEAR: f32 = 0.001;
    const MAX_SAFE_RADIUS: f32 = 10_000_000.0;
    const MAX_SAFE_DISTANCE: f32 = MAX_SAFE_RADIUS * 10.0;

    let radius = if subject_radius.is_finite() {
        subject_radius.abs().clamp(MIN_NEAR, MAX_SAFE_RADIUS)
    } else {
        0.05
    };
    let distance = if camera_distance.is_finite() {
        camera_distance
            .abs()
            .clamp(radius * 1.01, MAX_SAFE_DISTANCE)
    } else {
        radius * 2.0
    };
    let closest_surface = (distance - radius).max(MIN_NEAR);
    let near = (distance * 0.01).min(closest_surface * 0.5).max(MIN_NEAR);
    let far = (distance + radius * 4.0).max(near * 2.0);
    (near, far)
}

/// Ease in and out — the shape a camera move should have. Linear starts and stops look mechanical.
#[must_use]
pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        2.0 * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
    }
}

/// Blend two poses. The EYE and the LOOK-AT are interpolated separately — never a quaternion — which is
/// what keeps the subject framed through a transition instead of the camera swinging past and correcting.
#[must_use]
pub fn blend(a: CameraSample, b: CameraSample, t: f32) -> CameraSample {
    let k = ease_in_out(t);
    let mix = |x: f32, y: f32| x + (y - x) * k;
    CameraSample {
        eye: [
            mix(a.eye[0], b.eye[0]),
            mix(a.eye[1], b.eye[1]),
            mix(a.eye[2], b.eye[2]),
        ],
        look_at: [
            mix(a.look_at[0], b.look_at[0]),
            mix(a.look_at[1], b.look_at[1]),
            mix(a.look_at[2], b.look_at[2]),
        ],
        fov_deg: mix(a.fov_deg, b.fov_deg),
    }
}

impl ShotPlayback {
    /// Blend an already-solved current pose from the previous solved endpoint when this lookup falls in
    /// an intra-cutscene transition. The previous pose is explicit, so callers cannot accidentally carry
    /// state across separately directed cutscenes.
    #[must_use]
    pub fn blend_camera(
        self,
        previous: Option<CameraSample>,
        current: CameraSample,
    ) -> CameraSample {
        match (self.blend_from, previous) {
            (Some((_index, progress)), Some(previous)) => blend(previous, current, progress),
            _ => current,
        }
    }
}

// ── Placing a camera in a scene that is FULL ────────────────────────────────────────────────────
//
// Everything above solves a shot against ONE object: its bounds, its facing, and nothing else. That is
// correct film geometry and it is not enough in a 15,711-part industrial assembly, where the placement a
// close shot asks for is very often *inside the part next door*. The first delivered factory film had
// roughly four of fifteen sampled frames obscured — one solid dark red (inside a machine housing), two
// black, one a louvre filling the frame from a few centimetres away — because the solver guards against
// entering its own subject and has no notion of anything else in the world.
//
// The fix is a negotiation, not a new camera model: the pure solver proposes, the world disposes. The
// engine answers three questions about a candidate placement (Is the camera inside something? Can it see
// the subject? Is there anything BEHIND the subject to see it against?) and the planner walks a fixed,
// deterministic ladder of alternatives until it finds one the world does not object to.
//
// Three properties this deliberately has:
//
// * **It is decided ONCE PER SHOT, not per tick.** A per-tick re-solve would let the choice flip between
//   frames and the camera would visibly jump mid-shot — trading an obscured shot for a broken one.
// * **It judges the whole camera PATH.** A push-in that starts in clear air and ends inside a wall is
//   exactly what produced the solid-red frame, so candidates are sampled across the shot's progress and
//   scored on their worst moment, not their first.
// * **It never fails.** If nothing is clear, the authored placement is returned unchanged. A shot that
//   cannot be saved should look like the director asked for it, not like the engine gave up somewhere
//   else.

/// What the world reports about one candidate camera placement. Both fractions are in `0..=1`, and all
/// three are cheap for an engine holding a scene BVH to answer together — which is why this is one reply
/// rather than three queries.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vantage {
    /// The camera stands inside — or within touching distance of — geometry that is not its subject.
    /// This is the outright disqualifier: it is what a solid-colour frame looks like from the outside.
    pub eye_inside: bool,
    /// Fraction of the sample rays fired from the eye at the subject that arrive without crossing
    /// something else first. `1.0` is a clean view; `0.0` is completely blocked.
    pub clear: f32,
    /// Fraction of the frame's sample directions that find scene content BEYOND the subject — the
    /// backing it is read against. A shot aimed out of the building scores near zero: correctly framed,
    /// perfectly lit, and it reads as a part floating in a void.
    ///
    /// Measured from the subject outwards, not from the lens. Measured from the lens, a wall twenty
    /// centimetres in front of the camera counts as a rich backdrop — which is the opposite of the truth
    /// and made the worst frames in the second film score as the best.
    pub backing: f32,
    /// Fraction of the frame's sample directions filled by something that is NOT the subject and sits
    /// far closer than it — the camera is boxed in.
    ///
    /// [`Self::eye_inside`] only catches a camera actually buried in a part. The frame that survives it
    /// is the one standing a few centimetres clear of a large flat surface: perfectly outside everything,
    /// a clean line to a subject visible past the edge, and nine tenths of the picture is the side of the
    /// machine next door.
    pub crowded: f32,
}

impl Vantage {
    /// A vantage with nothing in the way and nothing behind — what an empty world reports, and the
    /// value a caller with no occlusion data should hand back so behaviour is unchanged.
    pub const OPEN: Self = Self {
        eye_inside: false,
        clear: 1.0,
        backing: 0.0,
        crowded: 0.0,
    };

    /// Usable at all: the camera is not buried, and most of the subject is actually in view.
    ///
    /// The threshold is deliberately not 1.0. The engine answers this against world bounding boxes, which
    /// over-report occlusion for anything long and diagonal — a handrail's box covers a great deal of air.
    /// Demanding a perfectly clean set of rays would reject placements a viewer would call clear and push
    /// every shot in a dense cell out to the widest card, which is its own kind of broken film.
    #[must_use]
    pub fn acceptable(&self) -> bool {
        !self.eye_inside && self.clear >= MIN_CLEAR_FRACTION && self.crowded <= MAX_CROWDED_FRACTION
    }

    /// Rank among acceptable placements. Seeing the subject dominates; having something behind it is a
    /// genuine but secondary preference, so a clear shot against emptiness still beats a half-blocked
    /// one with a rich background.
    #[must_use]
    pub fn score(&self) -> f32 {
        if self.eye_inside {
            return f32::NEG_INFINITY;
        }
        self.clear + self.backing * BACKING_WEIGHT - self.crowded * CROWDING_WEIGHT
    }
}

/// How much of the subject must be reachable before a placement counts as a view of it.
pub const MIN_CLEAR_FRACTION: f32 = 0.6;
/// How much a well-backed frame is worth relative to an unobstructed one. Under 1.0 on purpose: a
/// background is composition, a clear line to the subject is the shot.
const BACKING_WEIGHT: f32 = 0.35;
/// How much of the frame may be filled by a near foreground part before the shot is a picture of that
/// part instead of its subject.
///
/// Not zero: a foreground element is good composition — an industrial film wants to look past a railing
/// or a column at the machine beyond. Four of nine sample directions is where "framed past something"
/// becomes "framed into something".
pub const MAX_CROWDED_FRACTION: f32 = 0.45;
/// What near clutter costs in the ranking, beyond the outright rejection above. Heavier than backing is
/// worth, so a placement can never trade a crowded frame for a richer background.
const CROWDING_WEIGHT: f32 = 0.6;
/// Below this, the frame is not "sparse" — it is aimed out of the building.
///
/// A third of the sample directions finding *nothing at all*, floor included, is not a compositional
/// choice; it is the shot where a correctly framed, correctly lit machine hangs in an empty grey field
/// because the camera happened to end up on the outward side of it. The first delivered film had several,
/// and unlike an obstructed frame they pass every automated check there is — the subject is right there,
/// in focus, unobscured.
///
/// It is deliberately NOT part of [`Vantage::acceptable`]. Backing is a preference: a shot with no
/// backdrop is worse than one with, and still far better than one looking at a wall. All this threshold
/// decides is whether the planner stops at the authored placement or keeps looking for a better angle
/// on the same subject — and if none exists, the authored placement is what it comes back with.
pub const MIN_BACKING_FRACTION: f32 = 1.0 / 3.0;
/// How much better an alternative must score before it displaces the incumbent. Without it, float noise
/// between two equivalent placements would decide the shot.
const TIE_MARGIN: f32 = 1.0e-3;
/// The head start the directed placement carries. Small: enough that a marginally richer background
/// cannot quietly rotate a shot away from the angle that was asked for, not enough to keep a genuinely
/// empty frame when a full one is available at the same framing.
const AUTHORED_BONUS: f32 = 0.05;
/// What each step away from the authored framing costs in the ranking.
///
/// Larger than [`BACKING_WEIGHT`] ON PURPOSE, and that is the whole point of the number: a wider frame
/// contains more of the scene, so it scores better on backing almost by definition. Without a cost that
/// outweighs the most a background can be worth, "prefer the better-composed frame" quietly becomes
/// "prefer the wider frame", and a film that fought to earn its close-ups loses them all back to a
/// scoring artefact.
///
/// So widening is a FALLBACK, never a preference: a wider card can only win when every closer one was
/// rejected outright — buried, or with no view of the subject at all.
const WIDENING_PENALTY: f32 = 0.4;

/// The correction applied to an authored shot so it lands somewhere a camera can actually stand.
///
/// Stored rather than baked into a pose because the subject keeps moving: the shot must follow it, so the
/// escape has to be expressed relative to the authored placement and re-applied every tick.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotAdjustment {
    /// The framing actually used — the authored size, or a wider one if the close placement was buried.
    pub size: ShotSize,
    /// Degrees added to the shot's yaw, so the camera can step around an obstruction while keeping the
    /// authored height, distance and move.
    pub yaw_offset_deg: f32,
    /// How many rungs of the ladder were climbed to reach this placement. `0` means the authored
    /// placement was used as written — reported so a run can say how much of its direction survived
    /// contact with the scene.
    pub steps: u8,
}

impl ShotAdjustment {
    /// The identity adjustment for one shot — its own authored size, no yaw change.
    #[must_use]
    pub fn authored(shot: &ShotRecipe) -> Self {
        Self {
            size: shot.size,
            yaw_offset_deg: 0.0,
            steps: 0,
        }
    }

    /// True when the planner left the direction alone.
    #[must_use]
    pub fn is_authored(&self, shot: &ShotRecipe) -> bool {
        self.size == shot.size && self.yaw_offset_deg == 0.0
    }
}

/// The yaw detours tried at each framing, in the order tried. Small steps first, and each sign before
/// the next magnitude, so the camera prefers the smallest move away from the authored angle that works
/// and the result is symmetric rather than biased to one side of the subject.
const YAW_LADDER_DEG: [f32; 9] = [0.0, 30.0, -30.0, 60.0, -60.0, 90.0, -90.0, 140.0, -140.0];

/// Progress points a candidate is judged at. A camera move sweeps a PATH, and it is the end of a push-in
/// that ends up inside the machine, so judging only the opening pose is judging the wrong thing.
const PATH_SAMPLES: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

/// The lowest a production camera may stand. A low angle on a subject resting on the ground otherwise
/// puts the eye UNDER the floor, looking up through it at the underside of the scene. Clamped rather
/// than forbidden: the shot still looks up, it just does it from somewhere a camera could be.
pub const CAMERA_FLOOR: f32 = 0.15;

/// Solve a shot at `progress` with a planner's correction applied, as it will actually be filmed.
///
/// Deliberately separate from [`solve_shot_eased`]: the correction is chosen once for the shot and
/// carried, so the per-tick path stays a pure function of progress and the camera keeps moving smoothly.
///
/// The floor clamp lives HERE rather than at the point of use, because the planner and the runtime have
/// to be looking at the same camera. It used to be applied after solving, in the playback loop — so a
/// low-angle candidate was judged clear at a position under the floor and then filmed from somewhere
/// else, which is a validation of a pose that never appears on screen.
#[must_use]
pub fn solve_shot_adjusted(
    shot: &ShotRecipe,
    adjustment: ShotAdjustment,
    subject: SubjectSample,
    progress: f32,
    aspect: f32,
    fov_deg: f32,
) -> CameraSample {
    let adjusted = ShotRecipe {
        size: adjustment.size,
        ..shot.clone()
    };
    let mut pose = solve_shot_eased(&adjusted, subject, progress, aspect, fov_deg);
    if adjustment.yaw_offset_deg != 0.0 {
        pose = rotate_eye_about_subject(pose, subject.center, adjustment.yaw_offset_deg);
    }
    // The yaw swing preserves height, so the order of these two does not matter — but the clamp is last
    // regardless, so nothing downstream of it can put the eye back under the floor.
    if pose.eye[1] < CAMERA_FLOOR {
        pose.eye[1] = CAMERA_FLOOR;
    }
    // ...and then into the room, if the scene has one. Same reasoning as the floor, one scale up: a
    // wide card on a 262 m assembly solves to a stand-off of hundreds of metres, which is outside any
    // building, and a camera there films the outside of a shed. Confined, the same card becomes a view
    // ALONG the hall with the line receding into it — the subject occupies less of the frame and the
    // frame occupies more of the plant, which is the trade a factory establishing shot actually makes.
    //
    // Last, and inside the solve, for the same reason the floor is: the planner judges what is filmed.
    pose.eye = subject
        .stage
        .confine(pose.eye, subject.center, subject.radius() * 1.05);
    pose
}

/// Swing the eye around the subject's vertical axis, keeping its height and its distance. The aim is
/// untouched, so the subject stays exactly as framed — only the side we look from changes.
fn rotate_eye_about_subject(pose: CameraSample, center: [f32; 3], yaw_deg: f32) -> CameraSample {
    let (sin, cos) = yaw_deg.to_radians().sin_cos();
    let dx = pose.eye[0] - center[0];
    let dz = pose.eye[2] - center[2];
    CameraSample {
        eye: [
            center[0] + dx * cos + dz * sin,
            pose.eye[1],
            center[2] - dx * sin + dz * cos,
        ],
        ..pose
    }
}

/// Choose the placement this shot will actually use, by asking the world about a fixed ladder of
/// alternatives and keeping the best one it does not object to.
///
/// `look` is the world's answer for one candidate `(pose, progress)`; an engine with no occlusion data
/// can return [`Vantage::OPEN`] and this function then always returns the authored placement, so wiring
/// it in can never change an existing scene's direction by itself.
///
/// The ladder is walked in a fixed order and every candidate is scored, so the result is a pure function
/// of the inputs and replays bit-identically — the same contract the rest of this module keeps.
#[must_use]
pub fn plan_shot(
    shot: &ShotRecipe,
    subject: SubjectSample,
    aspect: f32,
    fov_deg: f32,
    mut look: impl FnMut(&CameraSample, f32) -> Vantage,
) -> ShotAdjustment {
    let mut sizes = Vec::with_capacity(6);
    let mut size = shot.size;
    loop {
        sizes.push(size);
        let wider = size.wider();
        if wider == size {
            break;
        }
        size = wider;
    }

    let mut best: Option<(f32, ShotAdjustment)> = None;
    // THE LEAST BAD PLACEMENT THE SEARCH FOUND, for when nothing is acceptable at all.
    //
    // This used to be thrown away. Every unacceptable candidate was `continue`d past, and if the whole
    // ladder failed the planner returned the authored placement — which is itself one of the candidates
    // that just failed, and is not the least bad one, only the one that happened to be written down.
    // The search evaluated fifty-four placements, scored every one of them, and then discarded all of
    // that to fall back on an arbitrary member of the losing set.
    //
    // Measured on the production factory: nine of thirty shots were filmed at a placement the engine
    // itself reported as `acceptable: false`, and those shots are where the film's remaining illegible
    // seconds are.
    let mut least_bad: Option<(f32, ShotAdjustment)> = None;
    let mut steps: u8 = 0;
    for (widened, size) in sizes.into_iter().enumerate() {
        #[allow(clippy::cast_precision_loss)] // at most five steps exist
        let widening_cost = widened as f32 * WIDENING_PENALTY;
        for yaw in YAW_LADDER_DEG {
            let candidate = ShotAdjustment {
                size,
                yaw_offset_deg: yaw,
                steps,
            };
            steps = steps.saturating_add(1);
            // The worst moment of the move decides. A candidate is only as good as the frame the
            // viewer would most object to, and a camera path is judged as a path.
            let mut worst_score = f32::INFINITY;
            let mut worst_backing = f32::INFINITY;
            let mut all_acceptable = true;
            for progress in PATH_SAMPLES {
                let pose = solve_shot_adjusted(shot, candidate, subject, progress, aspect, fov_deg);
                let vantage = look(&pose, progress);
                all_acceptable &= vantage.acceptable();
                worst_score = worst_score.min(vantage.score());
                worst_backing = worst_backing.min(vantage.backing);
            }
            if !all_acceptable {
                // Rank it anyway. A shot has to be filmed from somewhere, and "the best of a bad set"
                // is a strictly better answer than "the first one somebody wrote down".
                //
                // The authored bonus does NOT apply: it exists to stop the planner shopping around when
                // the direction as written is already fine, and on this path it is known not to be.
                //
                // ONE STEP OF WIDENING, AT MOST, ON THIS PATH.
                //
                // `WIDENING_PENALTY` (0.4) is set against `BACKING_WEIGHT` (0.35) so a richer background
                // can never buy a wider frame. It does NOT bound `clear`, which carries weight 1.0. In
                // the acceptable regime that is harmless, because every surviving candidate is already
                // above `MIN_CLEAR_FRACTION` and the spread between them is small. Here the spread is
                // the whole range: the placements this branch exists for measure clear 0.00 and crowded
                // 1.00, so a candidate several steps wider can gain around 1.6 of score — enough to pay
                // for three or four steps of widening and still win.
                //
                // The shots that reach this branch are the film's mechanism close-ups. Widening them to
                // Wide would raise the legibility fraction by turning each into a distant view of a
                // small part on an empty floor: the exact frame the authoring-time rule eliminates,
                // re-created at runtime where that rule cannot see it. It would satisfy "camera paths do
                // not clip geometry" by spending "professional industrial visualisation standard", and
                // the fraction alone could not tell those two apart.
                if widened <= 1 {
                    let ranked = worst_score - widening_cost;
                    if least_bad
                        .is_none_or(|(least_bad_score, _)| ranked > least_bad_score + TIE_MARGIN)
                    {
                        least_bad = Some((ranked, candidate));
                    }
                }
                continue;
            }
            let as_directed = candidate.size == shot.size && candidate.yaw_offset_deg == 0.0;
            // The direction as written, in a frame that has something in it: nothing can improve on
            // that, so stop rather than shop around for a marginally richer background.
            if as_directed && worst_backing >= MIN_BACKING_FRACTION {
                return ShotAdjustment {
                    steps: 0,
                    ..candidate
                };
            }
            let ranked =
                worst_score - widening_cost + if as_directed { AUTHORED_BONUS } else { 0.0 };
            if best.is_none_or(|(best_score, _)| ranked > best_score + TIE_MARGIN) {
                best = Some((
                    ranked,
                    if as_directed {
                        ShotAdjustment {
                            steps: 0,
                            ..candidate
                        }
                    } else {
                        candidate
                    },
                ));
            }
        }
    }
    best.map_or_else(
        || {
            least_bad.map_or_else(
                || ShotAdjustment::authored(shot),
                // `steps` carries the WHOLE count, so this case is legible in the record. It used to
                // report zero — the same value as "the authored placement was fine and nothing was
                // rejected before it" — which made a shot the planner could not place at all
                // indistinguishable from a shot it never needed to touch. Those are opposite situations
                // and a run report that renders them identically will be read as the happier one.
                |(_, candidate)| ShotAdjustment { steps, ..candidate },
            )
        },
        |(_, candidate)| candidate,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> SubjectSample {
        SubjectSample {
            center: [0.0, 0.0, 0.0],
            half_extent: [0.5, 0.5, 0.5],
            forward: [0.0, 0.0, 1.0],
            stage: Stage::OPEN,
        }
    }

    fn shot(size: ShotSize, angle: ShotAngle, motion: ShotMove) -> ShotRecipe {
        ShotRecipe {
            id: "shot-1".into(),
            subject: "1_0".into(),
            size,
            angle,
            motion,
            amount: 0.35,
            seconds: 2.0,
        }
    }

    fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    }

    #[test]
    fn a_closer_size_puts_the_camera_nearer_and_the_order_never_inverts() {
        let sizes = [
            ShotSize::ExtremeWide,
            ShotSize::Wide,
            ShotSize::Full,
            ShotSize::Medium,
            ShotSize::Close,
            ShotSize::ExtremeClose,
        ];
        let distances: Vec<f32> = sizes
            .iter()
            .map(|s| {
                let cam = solve_shot(
                    &shot(*s, ShotAngle::ThreeQuarter, ShotMove::Hold),
                    cube(),
                    0.0,
                    16.0 / 9.0,
                    50.0,
                );
                dist(cam.eye, cube().center)
            })
            .collect();
        for pair in distances.windows(2) {
            assert!(
                pair[1] < pair[0],
                "each step must move closer: {distances:?}"
            );
        }
    }

    #[test]
    fn angles_are_relative_to_the_subjects_facing() {
        // Turn the subject around; a "front" shot must follow it, not stay put.
        let mut facing_z = cube();
        facing_z.forward = [0.0, 0.0, 1.0];
        let mut facing_x = cube();
        facing_x.forward = [1.0, 0.0, 0.0];

        let s = shot(ShotSize::Full, ShotAngle::Front, ShotMove::Hold);
        let a = solve_shot(&s, facing_z, 0.0, 16.0 / 9.0, 50.0);
        let b = solve_shot(&s, facing_x, 0.0, 16.0 / 9.0, 50.0);
        assert!(
            dist(a.eye, b.eye) > 0.5,
            "the camera should move when the subject turns"
        );
        // Both stand the same distance away — only the direction changed.
        assert!((dist(a.eye, cube().center) - dist(b.eye, cube().center)).abs() < 1.0e-3);
    }

    #[test]
    fn a_push_in_closes_distance_over_the_shot_and_never_enters_the_subject() {
        let s = shot(ShotSize::Full, ShotAngle::ThreeQuarter, ShotMove::PushIn);
        let start = solve_shot(&s, cube(), 0.0, 16.0 / 9.0, 50.0);
        let end = solve_shot(&s, cube(), 1.0, 16.0 / 9.0, 50.0);
        let d0 = dist(start.eye, cube().center);
        let d1 = dist(end.eye, cube().center);
        assert!(d1 < d0, "a push-in must end closer than it started");
        assert!(
            d1 > cube().radius(),
            "and must never end inside the subject"
        );
    }

    /// A Calm cutscene runs every shot 2.5x longer than authored, so the pacing warning has to be about
    /// what the viewer sees, not about the number typed in. Before this, the two shortest cards in the
    /// catalogue could not be used in a Calm sequence without the tool reporting a problem for a shot
    /// that in fact holds for four and a half seconds.
    #[test]
    fn the_pacing_warning_judges_the_length_a_shot_actually_runs() {
        let close_up = |seconds: f32| ShotRecipe {
            seconds,
            ..shot(ShotSize::Close, ShotAngle::Profile, ShotMove::Hold)
        };
        let with_mood = |mood: Mood, seconds: f32| Cutscene {
            shots: vec![
                shot(ShotSize::Wide, ShotAngle::Front, ShotMove::PullOut),
                close_up(seconds),
            ],
            mood,
            ..Cutscene::default()
        };
        let rushed = |cut: &Cutscene| {
            cut.problems()
                .iter()
                .any(|problem| problem.contains("rushed"))
        };

        // 1.8s authored is 4.5s on screen in Calm -- more than twice the 2.0s Calm minimum.
        assert!(
            !rushed(&with_mood(Mood::Calm, 1.8)),
            "{:?}",
            with_mood(Mood::Calm, 1.8).problems()
        );
        // And a shot that really is too short still reports: 0.5s authored is 1.25s on screen, under
        // Calm's own 2.0s floor. The fix must not turn the warning off, only aim it at the right number.
        assert!(
            rushed(&with_mood(Mood::Calm, 0.5)),
            "0.5s authored is 1.25s on screen in Calm, under the 2.0s minimum"
        );
        // Tense compresses to 0.75x, so a shot that is comfortable when authored can still be rushed.
        assert!(
            rushed(&with_mood(Mood::Tense, 1.0)),
            "1.0s authored is 0.75s on screen in Tense, under the 0.8s minimum"
        );
        assert!(!rushed(&with_mood(Mood::Normal, 2.0)));
    }

    #[test]
    fn eased_push_in_is_monotonic_and_slower_at_both_endpoints_than_mid_shot() {
        let s = shot(ShotSize::Full, ShotAngle::ThreeQuarter, ShotMove::PushIn);
        let sample = cube();
        let eye_at = |progress| solve_shot_eased(&s, sample, progress, 16.0 / 9.0, 50.0).eye;
        let step = 0.01;
        let start_velocity = dist(eye_at(0.0), eye_at(step)) / step;
        let mid_velocity = dist(eye_at(0.5 - step), eye_at(0.5 + step)) / (step * 2.0);
        let end_velocity = dist(eye_at(1.0 - step), eye_at(1.0)) / step;
        assert!(
            start_velocity < mid_velocity,
            "ease-in must start slower than mid-shot: start={start_velocity}, mid={mid_velocity}"
        );
        assert!(
            end_velocity < mid_velocity,
            "ease-out must end slower than mid-shot: end={end_velocity}, mid={mid_velocity}"
        );

        let distances: Vec<f32> = (0..=100u8)
            .map(|frame| dist(eye_at(f32::from(frame) / 100.0), sample.center))
            .collect();
        for pair in distances.windows(2) {
            assert!(
                pair[1] <= pair[0] + 1.0e-5,
                "an eased push-in must never move away from its subject: {pair:?}"
            );
        }
        assert!(distances[100] < distances[0], "the move must make progress");
    }

    #[test]
    fn cinematic_clip_planes_are_finite_for_tiny_and_factory_scale_subjects() {
        for (radius, distance) in [(0.001, 0.008), (120.0, 420.0)] {
            let (near, far) = cinematic_clip_planes(radius, distance);
            assert!(near.is_finite() && far.is_finite());
            assert!(near > 0.0 && far > near, "invalid planes: {near}..{far}");
            assert!(
                near < distance - radius,
                "near plane must stay in front of the subject"
            );
            assert!(
                far > distance + radius,
                "far plane must stay behind the subject"
            );
        }
    }

    #[test]
    fn the_solver_is_deterministic_and_frames_with_headroom() {
        let s = shot(ShotSize::Close, ShotAngle::ThreeQuarter, ShotMove::Orbit);
        let first = solve_shot(&s, cube(), 0.42, 16.0 / 9.0, 50.0);
        for _ in 0..100 {
            let again = solve_shot(&s, cube(), 0.42, 16.0 / 9.0, 50.0);
            assert_eq!(first, again, "same inputs, same pose — every time");
        }
        // The aim sits ABOVE the centre: headroom, not a dead-centre snapshot.
        assert!(first.look_at[1] > cube().center[1]);
    }

    #[test]
    fn a_blend_starts_at_a_ends_at_b_and_keeps_the_subject_framed() {
        let a = CameraSample {
            eye: [0.0, 0.0, -5.0],
            look_at: [0.0; 3],
            fov_deg: 50.0,
        };
        let b = CameraSample {
            eye: [5.0, 2.0, 0.0],
            look_at: [1.0, 0.0, 0.0],
            fov_deg: 35.0,
        };
        assert_eq!(blend(a, b, 0.0), a);
        assert_eq!(blend(a, b, 1.0), b);
        let mid = blend(a, b, 0.5);
        // The look-at is interpolated too, so the framing travels with the camera.
        assert!(mid.look_at[0] > 0.0 && mid.look_at[0] < 1.0);
        assert!(mid.fov_deg > 35.0 && mid.fov_deg < 50.0);
    }

    #[test]
    fn the_playhead_walks_the_shot_list_and_ends() {
        let cut = Cutscene {
            version: 1,
            shots: vec![
                ShotRecipe {
                    seconds: 2.0,
                    ..shot(ShotSize::Wide, ShotAngle::Front, ShotMove::Hold)
                },
                ShotRecipe {
                    id: "shot-2".into(),
                    seconds: 3.0,
                    ..shot(ShotSize::Close, ShotAngle::Profile, ShotMove::PushIn)
                },
            ],
            mood: Mood::Normal,
            letterbox: true,
        };
        assert!((cut.seconds() - 5.0).abs() < 1.0e-6);
        assert_eq!(cut.shot_at(0.0).map(|(i, _)| i), Some(0));
        assert_eq!(cut.shot_at(2.5).map(|(i, _)| i), Some(1));
        assert_eq!(cut.shot_at(9.0), None, "past the end is over, not clamped");
        let (_, progress) = cut.shot_at(3.5).expect("inside shot 2");
        assert!(
            (progress - 0.5).abs() < 1.0e-3,
            "half way through a 3s shot"
        );
    }

    #[test]
    fn calm_effective_timing_carries_three_legal_ten_shot_acts_past_three_minutes() {
        let ten_shot_act = |mood| Cutscene {
            version: 1,
            shots: (0..10)
                .map(|index| ShotRecipe {
                    id: format!("shot-{index}"),
                    seconds: 2.5,
                    ..shot(ShotSize::Full, ShotAngle::ThreeQuarter, ShotMove::PushIn)
                })
                .collect(),
            mood,
            letterbox: true,
        };
        let normal = ten_shot_act(Mood::Normal);
        let calm = ten_shot_act(Mood::Calm);
        assert!((normal.seconds() - 25.0).abs() < 1.0e-6);
        assert!((calm.seconds() - 62.5).abs() < 1.0e-6);
        assert!(
            calm.seconds() * 3.0 > 180.0,
            "thirty standard Calm shots must clear a three-minute delivery"
        );
    }

    #[test]
    fn playback_lookup_uses_effective_time_and_only_blends_within_the_same_cutscene() {
        let cut = Cutscene {
            version: 1,
            shots: vec![
                ShotRecipe {
                    seconds: 2.0,
                    ..shot(ShotSize::Wide, ShotAngle::Front, ShotMove::Hold)
                },
                ShotRecipe {
                    id: "shot-2".into(),
                    seconds: 2.0,
                    ..shot(ShotSize::Close, ShotAngle::Profile, ShotMove::PushIn)
                },
            ],
            mood: Mood::Normal,
            letterbox: true,
        };
        assert_eq!(cut.playback_at(0.0).unwrap().blend_from, None);
        let boundary = cut.playback_at(2.0).expect("second shot starts");
        assert_eq!(boundary.index, 1);
        assert!(boundary.progress.abs() < 1.0e-6);
        assert_eq!(boundary.blend_from, Some((0, 0.0)));
        let middle = cut.playback_at(2.3).expect("inside transition");
        let (from, blend_progress) = middle.blend_from.expect("incoming blend");
        assert_eq!(from, 0);
        assert!((blend_progress - 0.5).abs() < 1.0e-5);
        assert!(
            cut.playback_at(2.61).unwrap().blend_from.is_none(),
            "the current shot must own the camera after the blend window"
        );

        let separate_act = Cutscene {
            shots: vec![shot(ShotSize::Close, ShotAngle::Front, ShotMove::Hold)],
            ..Cutscene::default()
        };
        assert_eq!(
            separate_act.playback_at(0.0).unwrap().blend_from,
            None,
            "a separately directed act starts with a hard cut"
        );
    }

    #[test]
    fn intra_cutscene_camera_transition_is_continuous_and_finite_at_both_endpoints() {
        let cut = Cutscene {
            version: 1,
            shots: vec![
                ShotRecipe {
                    seconds: 2.0,
                    ..shot(ShotSize::Wide, ShotAngle::Front, ShotMove::PullOut)
                },
                ShotRecipe {
                    id: "shot-2".into(),
                    seconds: 2.0,
                    ..shot(ShotSize::Close, ShotAngle::Profile, ShotMove::PushIn)
                },
            ],
            mood: Mood::Normal,
            letterbox: true,
        };
        let camera_at = |time| {
            let playback = cut.playback_at(time).expect("inside cutscene");
            let current = solve_shot_eased(
                &cut.shots[playback.index],
                cube(),
                playback.progress,
                16.0 / 9.0,
                50.0,
            );
            let previous = playback.blend_from.map(|(index, _)| {
                solve_shot_eased(&cut.shots[index], cube(), 1.0, 16.0 / 9.0, 50.0)
            });
            playback.blend_camera(previous, current)
        };

        let before = camera_at(2.0 - 1.0e-4);
        let boundary = camera_at(2.0);
        let after = camera_at(2.0 + 1.0e-4);
        assert!(dist(before.eye, boundary.eye) < 0.01);
        assert!(dist(boundary.eye, after.eye) < 0.01);

        for step in 0..=400u16 {
            let time = f32::from(step) * 0.009_9;
            let camera = camera_at(time);
            assert!(
                camera
                    .eye
                    .into_iter()
                    .chain(camera.look_at)
                    .chain([camera.fov_deg])
                    .all(f32::is_finite),
                "transition produced a non-finite camera at {time}s: {camera:?}"
            );
        }
    }

    #[test]
    fn continuity_problems_are_named_in_the_authors_language() {
        let jump = Cutscene {
            version: 1,
            shots: vec![
                shot(ShotSize::Close, ShotAngle::Front, ShotMove::Hold),
                ShotRecipe {
                    id: "shot-2".into(),
                    ..shot(ShotSize::Close, ShotAngle::Front, ShotMove::Hold)
                },
            ],
            mood: Mood::Normal,
            letterbox: false,
        };
        let problems = jump.problems();
        assert!(
            problems.iter().any(|p| p.contains("jump cut")),
            "an identical back-to-back framing is a jump cut: {problems:?}"
        );
        assert!(
            problems.iter().any(|p| p.contains("opens tight")),
            "opening on a close-up should suggest establishing first: {problems:?}"
        );

        let good = Cutscene {
            version: 1,
            shots: vec![
                ShotRecipe {
                    seconds: 2.0,
                    ..shot(ShotSize::Wide, ShotAngle::Front, ShotMove::Hold)
                },
                ShotRecipe {
                    id: "shot-2".into(),
                    seconds: 2.0,
                    ..shot(ShotSize::Close, ShotAngle::Profile, ShotMove::PushIn)
                },
            ],
            mood: Mood::Normal,
            letterbox: true,
        };
        assert!(good.problems().is_empty(), "{:?}", good.problems());
    }

    // ── The planner: a camera placed in a scene that is FULL ────────────────────────────────────
    //
    // The world is stubbed by a closure here on purpose. `plan_shot` is a POLICY over answers it does
    // not compute, so the thing worth testing is the policy: does it leave a clear shot alone, does it
    // escape a buried one, does it prefer the smallest escape, and does it stay deterministic.

    /// A world that buries the camera whenever the eye is within `radius` of `blob`, and reports a clear
    /// view otherwise. Stands in for "the placement is inside the machine next door".
    fn blob_world(blob: [f32; 3], radius: f32) -> impl FnMut(&CameraSample, f32) -> Vantage {
        move |pose: &CameraSample, _progress: f32| {
            let d = dist(pose.eye, blob);
            Vantage {
                eye_inside: d < radius,
                clear: if d < radius { 0.0 } else { 1.0 },
                backing: 0.5,
                crowded: 0.0,
            }
        }
    }

    #[test]
    fn an_unobstructed_shot_is_left_exactly_as_directed() {
        let s = shot(ShotSize::Close, ShotAngle::ThreeQuarter, ShotMove::Hold);
        let plan = plan_shot(&s, cube(), 16.0 / 9.0, 50.0, |_, _| Vantage::OPEN);
        assert!(
            plan.is_authored(&s),
            "an open world must not redirect anything: {plan:?}"
        );
        assert_eq!(plan.steps, 0);
        // And the adjusted solve is then byte-identical to the plain one.
        assert_eq!(
            solve_shot_adjusted(&s, plan, cube(), 0.4, 16.0 / 9.0, 50.0),
            solve_shot_eased(&s, cube(), 0.4, 16.0 / 9.0, 50.0),
        );
    }

    /// The defect this whole mechanism exists for: a close placement that lands inside the part next
    /// door. The engine must come back with a placement that is NOT inside it — and must still be
    /// looking at the same subject.
    #[test]
    fn a_buried_camera_escapes_and_still_frames_its_subject() {
        let s = shot(ShotSize::Close, ShotAngle::ThreeQuarter, ShotMove::Hold);
        let authored = solve_shot_eased(&s, cube(), 0.0, 16.0 / 9.0, 50.0);
        // Put a machine housing exactly where the directed camera wanted to stand.
        let plan = plan_shot(&s, cube(), 16.0 / 9.0, 50.0, blob_world(authored.eye, 1.2));
        assert!(
            !plan.is_authored(&s),
            "the planner must have moved: {plan:?}"
        );
        let escaped = solve_shot_adjusted(&s, plan, cube(), 0.0, 16.0 / 9.0, 50.0);
        assert!(
            dist(escaped.eye, authored.eye) > 1.2,
            "the escape must leave the obstruction: {escaped:?}"
        );
        // Still a shot OF the cube: same aim, and framed no further away than the widest card allows.
        // Bit-for-bit, not within a tolerance: an escape moves the EYE, and the aim is carried through
        // untouched, so anything but identity here means the subject was re-framed behind our back.
        assert_eq!(
            escaped.look_at.map(f32::to_bits),
            authored.look_at.map(f32::to_bits)
        );
        let widest = solve_shot_eased(
            &ShotRecipe {
                size: ShotSize::ExtremeWide,
                ..s.clone()
            },
            cube(),
            0.0,
            16.0 / 9.0,
            50.0,
        );
        assert!(
            dist(escaped.eye, cube().center) <= dist(widest.eye, cube().center) + 1.0e-3,
            "the escape must not exceed the widest authored framing"
        );
    }

    /// A yaw detour is cheaper than a whole framing change: the shot the director asked for survives
    /// better if we walk around the obstruction than if we back away from the subject.
    #[test]
    fn the_smallest_escape_that_works_is_the_one_taken() {
        let s = shot(ShotSize::Close, ShotAngle::Profile, ShotMove::Hold);
        let authored = solve_shot_eased(&s, cube(), 0.0, 16.0 / 9.0, 50.0);
        let plan = plan_shot(&s, cube(), 16.0 / 9.0, 50.0, blob_world(authored.eye, 0.9));
        assert_eq!(
            plan.size,
            ShotSize::Close,
            "a small obstruction must not cost the framing: {plan:?}"
        );
        assert!(
            (plan.yaw_offset_deg.abs() - 30.0).abs() < 1.0e-3,
            "the first rung of the yaw ladder should have been enough: {plan:?}"
        );
    }

    /// The frame that produced the solid dark red: clear at the start of the push-in, inside the machine
    /// at the end of it. A planner that judges only the opening pose accepts this shot.
    #[test]
    fn a_push_in_that_ends_inside_something_is_rejected_before_it_is_filmed() {
        let s = shot(ShotSize::Medium, ShotAngle::Front, ShotMove::PushIn);
        let ends_at = solve_shot_eased(&s, cube(), 1.0, 16.0 / 9.0, 50.0);
        let starts_at = solve_shot_eased(&s, cube(), 0.0, 16.0 / 9.0, 50.0);
        assert!(
            dist(starts_at.eye, ends_at.eye) > 0.3,
            "the fixture needs a move with real travel"
        );
        // A housing tight around the END of the move only — the opening pose is in clear air.
        let radius = dist(starts_at.eye, ends_at.eye) * 0.5;
        let plan = plan_shot(
            &s,
            cube(),
            16.0 / 9.0,
            50.0,
            blob_world(ends_at.eye, radius),
        );
        assert!(
            !plan.is_authored(&s),
            "a move that ends buried must be rejected even though it starts clear: {plan:?}"
        );
        // And every sampled point of the chosen path really is outside the housing.
        for step in 0..=20u8 {
            let progress = f32::from(step) / 20.0;
            let pose = solve_shot_adjusted(&s, plan, cube(), progress, 16.0 / 9.0, 50.0);
            assert!(
                dist(pose.eye, ends_at.eye) >= radius * 0.999,
                "the chosen path re-enters the obstruction at progress {progress}"
            );
        }
    }

    /// The planner and the runtime have to be judging the same camera. A low-angle shot on a subject
    /// resting on the ground solves to an eye under the floor; clamping that AFTER the placement was
    /// approved means the frame that was validated is not the frame that gets filmed.
    #[test]
    fn a_filmed_pose_never_stands_under_the_floor() {
        let mut on_the_ground = cube();
        on_the_ground.center = [0.0, 0.5, 0.0];
        let s = shot(ShotSize::Full, ShotAngle::Low, ShotMove::CraneDown);
        let plan = ShotAdjustment::authored(&s);
        let mut clamped_any = false;
        for step in 0..=20u8 {
            let progress = f32::from(step) / 20.0;
            let pose = solve_shot_adjusted(&s, plan, on_the_ground, progress, 16.0 / 9.0, 50.0);
            assert!(
                pose.eye[1] >= CAMERA_FLOOR - 1.0e-6,
                "progress {progress} put the camera at {} — under the floor",
                pose.eye[1]
            );
            clamped_any |= (pose.eye[1] - CAMERA_FLOOR).abs() < 1.0e-6;
        }
        assert!(
            clamped_any,
            "the fixture must actually reach the floor, or this proves nothing"
        );
        // And the planner sees the clamped pose, not the one under the floor.
        let mut lowest = f32::INFINITY;
        let _ = plan_shot(&s, on_the_ground, 16.0 / 9.0, 50.0, |pose, _| {
            lowest = lowest.min(pose.eye[1]);
            Vantage::OPEN
        });
        assert!(lowest >= CAMERA_FLOOR - 1.0e-6, "planner saw {lowest}");
    }

    /// The other failure the first film shipped, and the one no automated check catches: a machine that
    /// is correctly framed, correctly lit, unobscured — and hanging in an empty grey field, because the
    /// camera ended up on the outward side of it. The planner must turn around and shoot back INTO the
    /// factory when doing so costs nothing else.
    #[test]
    fn a_shot_aimed_out_of_the_building_is_turned_back_into_it() {
        let s = shot(ShotSize::Wide, ShotAngle::Front, ShotMove::Hold);
        let authored = solve_shot_eased(&s, cube(), 0.0, 16.0 / 9.0, 50.0);
        // Everything is clear from everywhere; the only difference between placements is whether the
        // rest of the plant is behind the subject or behind the camera.
        let plan = plan_shot(&s, cube(), 16.0 / 9.0, 50.0, |pose, _| Vantage {
            eye_inside: false,
            clear: 1.0,
            backing: if dist(pose.eye, authored.eye) < 1.0 {
                0.0
            } else {
                1.0
            },
            crowded: 0.0,
        });
        assert!(
            !plan.is_authored(&s),
            "an empty frame with a full one available is not the shot: {plan:?}"
        );
        assert_eq!(
            plan.size,
            ShotSize::Wide,
            "turning around must not cost the framing: {plan:?}"
        );
    }

    /// The trap this ladder sets for itself: a wider frame contains more of the scene, so it scores
    /// better on background almost by definition. Left alone, "prefer the better-composed frame" becomes
    /// "prefer the wider frame", and a film that fought to earn its close-ups loses every one of them to
    /// a scoring artefact. Widening has to be a fallback, not a preference.
    #[test]
    fn a_richer_background_further_out_never_buys_a_wider_shot() {
        let s = shot(ShotSize::Close, ShotAngle::ThreeQuarter, ShotMove::Hold);
        let close_distance = dist(
            solve_shot_eased(&s, cube(), 0.0, 16.0 / 9.0, 50.0).eye,
            cube().center,
        );
        // Everywhere is clear; the further out the camera stands, the more it can see behind the
        // subject. Exactly the gradient a naive score would climb all the way to the widest card.
        let plan = plan_shot(&s, cube(), 16.0 / 9.0, 50.0, |pose, _| Vantage {
            eye_inside: false,
            clear: 1.0,
            backing: (dist(pose.eye, cube().center) / (close_distance * 8.0)).clamp(0.0, 1.0),
            crowded: 0.0,
        });
        assert_eq!(
            plan.size,
            ShotSize::Close,
            "the framing that was directed must survive a better-backed wider one: {plan:?}"
        );

        // But when the close cards genuinely cannot be stood in, widening still happens — so the
        // penalty is a preference, not a prohibition.
        let blocked_near = plan_shot(&s, cube(), 16.0 / 9.0, 50.0, |pose, _| {
            let out = dist(pose.eye, cube().center);
            Vantage {
                eye_inside: out < close_distance * 1.5,
                clear: if out < close_distance * 1.5 { 0.0 } else { 1.0 },
                backing: 0.9,
                crowded: 0.0,
            }
        });
        assert_ne!(blocked_near.size, ShotSize::Close, "{blocked_near:?}");
        assert!(
            dist(
                solve_shot_adjusted(&s, blocked_near, cube(), 0.0, 16.0 / 9.0, 50.0).eye,
                cube().center
            ) >= close_distance * 1.5,
            "the fallback must actually clear the obstruction"
        );
    }

    /// The failure the SECOND film was made of, and the one the first version of this mechanism could
    /// not see: the camera a few centimetres clear of a large flat surface. It is outside everything, it
    /// has a clean line to a subject visible past the edge — and nine tenths of the picture is the side
    /// of the machine next door.
    #[test]
    fn a_camera_boxed_in_by_near_geometry_is_rejected_though_it_sees_its_subject() {
        let boxed_in = Vantage {
            eye_inside: false,
            clear: 1.0,
            backing: 1.0,
            crowded: 0.9,
        };
        assert!(
            !boxed_in.acceptable(),
            "a frame that is nine tenths foreground is not a shot of its subject: {boxed_in:?}"
        );
        // A foreground element is composition, not clutter: an industrial film wants to look past a
        // railing or a column. Under the threshold stays acceptable.
        let framed_past_something = Vantage {
            crowded: MAX_CROWDED_FRACTION,
            ..boxed_in
        };
        assert!(
            framed_past_something.acceptable(),
            "{framed_past_something:?}"
        );
        // Clutter always COSTS. Two frames alike in every other way rank by how much of the picture is
        // foreground, so among acceptable placements the planner still walks away from the busier one.
        // (It is deliberately not true that an empty frame beats a well-backed one with some foreground
        // in it: a backdrop plus a foreground element is depth, which is what an industrial shot wants.)
        let open = Vantage {
            crowded: 0.0,
            ..framed_past_something
        };
        assert!(open.score() > framed_past_something.score());
        // And a unit of clutter costs more than a unit of background is worth, so the ladder can never
        // buy a busier frame with a richer backdrop.
        const { assert!(CROWDING_WEIGHT > BACKING_WEIGHT) };
    }

    /// End to end: a placement the world calls crowded must be escaped, exactly like a buried one.
    #[test]
    fn the_planner_escapes_a_crowded_frame() {
        let s = shot(ShotSize::Close, ShotAngle::ThreeQuarter, ShotMove::Hold);
        let authored = solve_shot_eased(&s, cube(), 0.0, 16.0 / 9.0, 50.0);
        let plan = plan_shot(&s, cube(), 16.0 / 9.0, 50.0, |pose, _| Vantage {
            eye_inside: false,
            clear: 1.0,
            backing: 1.0,
            // Everything near the directed placement is jammed against a surface; elsewhere is open.
            crowded: if dist(pose.eye, authored.eye) < 1.0 {
                0.9
            } else {
                0.0
            },
        });
        assert!(!plan.is_authored(&s), "{plan:?}");
        assert!(
            dist(
                solve_shot_adjusted(&s, plan, cube(), 0.0, 16.0 / 9.0, 50.0).eye,
                authored.eye
            ) >= 1.0,
            "the escape must leave the crowded region"
        );
    }

    /// The same mechanism must not become a licence to redirect. Where the whole scene is uniformly
    /// backed, the angle that was asked for is the angle that is filmed.
    #[test]
    fn a_uniformly_backed_scene_never_redirects_anything() {
        for size in [
            ShotSize::ExtremeWide,
            ShotSize::Full,
            ShotSize::ExtremeClose,
        ] {
            for angle in [ShotAngle::Front, ShotAngle::Behind, ShotAngle::High] {
                let s = shot(size, angle, ShotMove::Orbit);
                let plan = plan_shot(&s, cube(), 16.0 / 9.0, 50.0, |_, _| Vantage {
                    eye_inside: false,
                    clear: 1.0,
                    backing: 0.8,
                    crowded: 0.0,
                });
                assert!(
                    plan.is_authored(&s),
                    "{size:?}/{angle:?} was redirected: {plan:?}"
                );
            }
        }
        // And a scene with no backing anywhere has nothing better to offer, so it keeps the direction
        // rather than spinning the camera looking for a backdrop that does not exist.
        let s = shot(ShotSize::Full, ShotAngle::ThreeQuarter, ShotMove::Hold);
        assert!(plan_shot(&s, cube(), 16.0 / 9.0, 50.0, |_, _| Vantage::OPEN).is_authored(&s));
    }

    /// Backing is a preference, never a requirement: a clear view of the subject against nothing still
    /// beats a half-blocked view of it against a rich background.
    #[test]
    fn a_clear_view_outranks_a_better_background() {
        let empty_but_clear = Vantage {
            eye_inside: false,
            clear: 1.0,
            backing: 0.0,
            crowded: 0.0,
        };
        let backed_but_blocked = Vantage {
            eye_inside: false,
            clear: 0.61,
            backing: 1.0,
            crowded: 0.0,
        };
        assert!(empty_but_clear.acceptable() && backed_but_blocked.acceptable());
        assert!(empty_but_clear.score() > backed_but_blocked.score());
        // A buried eye is disqualified outright, however good everything else looks.
        let buried = Vantage {
            eye_inside: true,
            clear: 1.0,
            backing: 1.0,
            crowded: 0.0,
        };
        assert!(!buried.acceptable());
        assert!(buried.score().is_infinite() && buried.score().is_sign_negative());
    }

    /// With everything blocked EQUALLY there is no information to act on, and the planner must hand back
    /// the direction as written rather than park the camera somewhere arbitrary.
    ///
    /// Note what this does and does not pin: the world here answers the same vantage for every
    /// candidate, so all fifty-four tie and the first — the authored one — wins the tie. It is a test
    /// about indifference. The case where the bad placements differ from each other is
    /// [`Self::the_least_bad_placement_is_taken_when_nothing_is_acceptable`], and until that test
    /// existed this one was read as covering both.
    #[test]
    fn a_hopeless_scene_returns_the_authored_shot_unchanged() {
        let s = shot(ShotSize::Close, ShotAngle::Behind, ShotMove::Orbit);
        let plan = plan_shot(&s, cube(), 16.0 / 9.0, 50.0, |_, _| Vantage {
            eye_inside: true,
            clear: 0.0,
            backing: 0.0,
            crowded: 1.0,
        });
        assert!(plan.is_authored(&s), "{plan:?}");
        assert_eq!(
            solve_shot_adjusted(&s, plan, cube(), 0.7, 16.0 / 9.0, 50.0),
            solve_shot_eased(&s, cube(), 0.7, 16.0 / 9.0, 50.0),
        );
    }

    /// When nothing is acceptable, film from the least bad place the search found — not from the one
    /// that happened to be written down.
    ///
    /// THE FAILURE THIS IS BUILT FROM. On the production factory, nine of thirty shots were filmed at a
    /// placement the engine itself reported `acceptable: false` for, and those shots are where the
    /// film's remaining illegible seconds are. The planner had evaluated fifty-four placements for each
    /// of them, scored every one, found none acceptable — and then discarded all of that and returned
    /// the authored placement, which is simply one arbitrary member of the losing set.
    ///
    /// The world here is hopeless everywhere but not equally so: the authored placement is buried, and
    /// one yaw further round is merely crowded. A viewer would rather watch the crowded one.
    #[test]
    fn the_least_bad_placement_is_taken_when_nothing_is_acceptable() {
        let s = shot(ShotSize::Close, ShotAngle::Front, ShotMove::Hold);
        let authored_eye = solve_shot_eased(&s, cube(), 0.5, 16.0 / 9.0, 50.0).eye;
        let plan = plan_shot(&s, cube(), 16.0 / 9.0, 50.0, |pose, _| {
            // Anywhere near where the direction asked for, the camera is inside something. Elsewhere it
            // is merely boxed in — bad, unacceptable, and visibly better than being buried.
            if dist(pose.eye, authored_eye) < 0.05 {
                Vantage {
                    eye_inside: true,
                    clear: 0.0,
                    backing: 0.0,
                    crowded: 1.0,
                }
            } else {
                Vantage {
                    eye_inside: false,
                    clear: 0.5,
                    backing: 0.5,
                    crowded: 1.0,
                }
            }
        });
        assert!(
            !plan.is_authored(&s),
            "the authored placement was buried and something less bad existed: {plan:?}"
        );
        // And the record has to SAY the search failed. `steps` used to be zero here, which is the same
        // value it reports when the authored placement was fine and nothing was rejected before it —
        // so a shot the planner could not place at all read identically to one it never had to touch.
        assert!(
            plan.steps > 0,
            "a shot filmed from the least bad of a losing set must not report zero rejections: {plan:?}"
        );
        // AND IT MUST NOT BUY LEGIBILITY WITH THE CLOSE VOCABULARY. On this path `clear` (weight 1.0) is
        // unbounded by `WIDENING_PENALTY` (0.4), so an unconstrained search would happily step three or
        // four sizes wider — turning the film's mechanism close-ups into distant views of small parts on
        // an empty floor, which scores better and shows less.
        assert!(
            plan.size == s.size || plan.size == s.size.wider(),
            "the fallback may step back at most one size: directed {:?}, filmed {:?}",
            s.size,
            plan.size
        );
    }

    /// The same contract the rest of this module keeps: same inputs, same answer, every time — a
    /// cutscene is replayed by tick index and a camera that shopped around differently on replay would
    /// desynchronise the film from its recording.
    #[test]
    fn planning_is_deterministic_and_bounded() {
        let s = shot(ShotSize::ExtremeClose, ShotAngle::Low, ShotMove::PushIn);
        let authored = solve_shot_eased(&s, cube(), 0.0, 16.0 / 9.0, 50.0);
        let first = plan_shot(&s, cube(), 16.0 / 9.0, 50.0, blob_world(authored.eye, 2.0));
        for _ in 0..50 {
            let again = plan_shot(&s, cube(), 16.0 / 9.0, 50.0, blob_world(authored.eye, 2.0));
            assert_eq!(first, again, "the planner must not shop around differently");
        }
        // The ladder is finite: six framings by nine yaws, and every rung is reachable in a u8.
        let mut calls = 0usize;
        let _ = plan_shot(&s, cube(), 16.0 / 9.0, 50.0, |_, _| {
            calls += 1;
            Vantage {
                eye_inside: true,
                clear: 0.0,
                backing: 0.0,
                crowded: 1.0,
            }
        });
        assert_eq!(calls, 6 * 9 * 5, "one query per candidate per path sample");
    }

    /// A yaw detour swings the camera around the subject; it must not quietly change how big the
    /// subject is in frame, or "step around the obstruction" would silently become "back away".
    #[test]
    fn a_yaw_detour_keeps_the_framing_distance_and_the_height() {
        let s = shot(ShotSize::Medium, ShotAngle::ThreeQuarter, ShotMove::Hold);
        let straight = solve_shot_adjusted(
            &s,
            ShotAdjustment::authored(&s),
            cube(),
            0.5,
            16.0 / 9.0,
            50.0,
        );
        for yaw in [30.0_f32, -60.0, 140.0] {
            let swung = solve_shot_adjusted(
                &s,
                ShotAdjustment {
                    size: s.size,
                    yaw_offset_deg: yaw,
                    steps: 1,
                },
                cube(),
                0.5,
                16.0 / 9.0,
                50.0,
            );
            assert!(
                (dist(swung.eye, cube().center) - dist(straight.eye, cube().center)).abs() < 1.0e-3,
                "yaw {yaw} changed the distance"
            );
            assert!(
                (swung.eye[1] - straight.eye[1]).abs() < 1.0e-4,
                "yaw {yaw} changed the height"
            );
            assert_eq!(
                swung.look_at.map(f32::to_bits),
                straight.look_at.map(f32::to_bits),
                "yaw {yaw} changed the aim"
            );
        }
    }
}
