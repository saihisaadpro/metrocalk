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
            } else if shot.seconds < self.mood.min_shot_seconds() {
                out.push(format!(
                    "shot {n} is short for this mood ({:.1}s) — it may feel rushed",
                    shot.seconds
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> SubjectSample {
        SubjectSample {
            center: [0.0, 0.0, 0.0],
            half_extent: [0.5, 0.5, 0.5],
            forward: [0.0, 0.0, 1.0],
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

        let distances: Vec<f32> = (0..=100)
            .map(|frame| dist(eye_at(frame as f32 / 100.0), sample.center))
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

        for step in 0..=400 {
            let time = step as f32 * 0.009_9;
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
}
