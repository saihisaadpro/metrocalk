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
    /// Total running time in seconds.
    #[must_use]
    pub fn seconds(&self) -> f32 {
        self.shots.iter().map(|s| s.seconds).sum()
    }

    /// Which shot is live at `t` seconds, and how far through it we are (0..1).
    #[must_use]
    pub fn shot_at(&self, t: f32) -> Option<(usize, f32)> {
        let mut start = 0.0;
        for (i, shot) in self.shots.iter().enumerate() {
            let end = start + shot.seconds.max(MIN_SECONDS);
            if t < end {
                let span = (end - start).max(1.0e-6);
                return Some((i, ((t - start) / span).clamp(0.0, 1.0)));
            }
            start = end;
        }
        None
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
