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

/// A camera the author PLACED, in world space — the pose the viewport was standing at when they said
/// "shoot from here".
///
/// A different KIND of answer from a size and an angle, which is why it is a separate field and not a
/// seventh [`ShotAngle`]. The card vocabulary describes a placement *relative to the subject's facing*
/// and re-solves it every tick, so it survives the subject moving; this is an absolute pose in the
/// world, and it is absolute on purpose — the author looked at a frame and said *that one*. The moment
/// a solver were allowed to reinterpret it, the promise the gesture makes would be false.
///
/// The `fov_deg` travels with it for the same reason: it is the lens they framed through, and a shot
/// filmed at the cutscene runtime's 50° when it was composed at the viewport's 45° is a different
/// picture.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotCamera {
    /// Where the camera stands, world units.
    pub eye: [f32; 3],
    /// The point it is aimed at, world units.
    pub look_at: [f32; 3],
    /// Vertical field of view, degrees.
    pub fov_deg: f32,
    /// ADR-195 — THE HEAD TURNS, THE TRIPOD DOES NOT MOVE. `None` is a locked-off camera: the pose
    /// above is filmed exactly as authored however far the subject walks, which is what every
    /// placed camera did before this field existed and what every document authored without it
    /// still opens as.
    ///
    /// `Some(offset)` is a camera on a **panning head**: the eye stays bolted where the author put
    /// it and only the aim follows, at `subject.center + offset`. It stores the OFFSET rather than
    /// a bare flag because an author does not aim at a centroid — they aim at a head, a tool tip,
    /// the corner of a pallet. Re-aiming at the centre the instant tracking is switched on would
    /// change the frame they just composed, which is the one thing this must not do; carrying the
    /// offset makes switching it on **bit-identical** while the subject is where it was, and only
    /// motion changes the picture.
    ///
    /// One field, not two, so it is impossible to be following without a framing to preserve.
    #[serde(default)]
    pub track: Option<[f32; 3]>,
}

/// The shortest stand-off an authored camera may be stored at. Below this the eye and the aim are the
/// same point to a renderer, and the look direction is undefined.
pub const MIN_AUTHORED_RANGE: f32 = 1.0e-3;

impl ShotCamera {
    /// True when this pose describes a camera that can actually be built.
    ///
    /// Checked where the pose is STORED, not where it is filmed: a shot carrying a degenerate camera
    /// is a shot that renders a black frame weeks later with nothing said at the time.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        if !self
            .eye
            .iter()
            .chain(self.look_at.iter())
            .chain(std::iter::once(&self.fov_deg))
            .all(|v| v.is_finite())
        {
            return false;
        }
        if !(1.0..=179.0).contains(&self.fov_deg) {
            return false;
        }
        self.range() >= MIN_AUTHORED_RANGE
    }

    /// Where this camera is aiming RIGHT NOW, given where its subject is right now.
    ///
    /// The authored point for a locked-off camera; the subject's live centre plus the stored
    /// framing offset for a panning head. The eye is never consulted here and never moves —
    /// re-placing a camera the author placed is exactly the reinterpretation the doc comment above
    /// forbids, and "follow it" is a request to turn the head, not to move the tripod.
    ///
    /// FALLS BACK when the live aim would collapse onto the eye. A subject that walks into the lens
    /// leaves no look direction at all, and a camera with an undefined forward vector renders a
    /// frame of nothing; the authored aim is the one point known to be a legal stand-off from this
    /// eye, because [`Self::is_usable`] checked it where it was stored.
    #[must_use]
    pub fn aim_at(&self, subject_center: [f32; 3]) -> [f32; 3] {
        let Some(offset) = self.track else {
            return self.look_at;
        };
        let live = [
            subject_center[0] + offset[0],
            subject_center[1] + offset[1],
            subject_center[2] + offset[2],
        ];
        let d = [
            self.eye[0] - live[0],
            self.eye[1] - live[1],
            self.eye[2] - live[2],
        ];
        let range = d[0].mul_add(d[0], d[1].mul_add(d[1], d[2] * d[2])).sqrt();
        if range.is_finite() && range >= MIN_AUTHORED_RANGE {
            live
        } else {
            self.look_at
        }
    }

    /// Put this camera's head on the subject, recording the framing it has right now as the offset
    /// to preserve. Idempotent at rest, and the inverse of [`Self::locked_off`].
    #[must_use]
    pub fn following(self, subject_center: [f32; 3]) -> Self {
        Self {
            track: Some([
                self.look_at[0] - subject_center[0],
                self.look_at[1] - subject_center[1],
                self.look_at[2] - subject_center[2],
            ]),
            ..self
        }
    }

    /// Lock the head off: the authored aim, filmed as authored, wherever the subject goes.
    #[must_use]
    pub fn locked_off(self) -> Self {
        Self {
            track: None,
            ..self
        }
    }

    /// True when this camera's head follows its subject.
    #[must_use]
    pub fn is_following(&self) -> bool {
        self.track.is_some()
    }

    /// The stand-off: how far the eye is from what it aims at.
    #[must_use]
    pub fn range(&self) -> f32 {
        let d = [
            self.eye[0] - self.look_at[0],
            self.eye[1] - self.look_at[1],
            self.eye[2] - self.look_at[2],
        ];
        d[0].mul_add(d[0], d[1].mul_add(d[1], d[2] * d[2])).sqrt()
    }
}

/// The 30-degree rule, which is the actual grammar the card check approximates: cutting between two
/// views of one subject that differ by less than this reads as a jolt rather than a cut.
pub const JUMP_CUT_MIN_DEGREES: f32 = 30.0;
/// ...unless the camera also moved substantially closer or further away. A straight punch-in on the
/// same axis is a deliberate, legible cut, so a stand-off that changed by this factor excuses the
/// angle. Set at the ratio between two neighbouring shot cards, so the two paths agree about what
/// counts as a different framing.
pub const JUMP_CUT_MIN_RANGE_RATIO: f32 = 1.5;
/// How far two aims may drift apart, as a fraction of the nearer stand-off, before the two shots are
/// simply looking at different things and no continuity question arises.
const JUMP_CUT_AIM_TOLERANCE: f32 = 0.2;

/// Do two placed cameras cut together flatly — the 30-degree rule, asked of poses instead of cards.
#[must_use]
fn placed_cut_is_flat(a: ShotCamera, b: ShotCamera) -> bool {
    let (ra, rb) = (a.range(), b.range());
    let near = ra.min(rb);
    if near < MIN_AUTHORED_RANGE {
        return false;
    }
    let aim_drift = {
        let d = [
            a.look_at[0] - b.look_at[0],
            a.look_at[1] - b.look_at[1],
            a.look_at[2] - b.look_at[2],
        ];
        d[0].mul_add(d[0], d[1].mul_add(d[1], d[2] * d[2])).sqrt()
    };
    if aim_drift > near * JUMP_CUT_AIM_TOLERANCE {
        return false;
    }
    if ra.max(rb) / near >= JUMP_CUT_MIN_RANGE_RATIO {
        return false;
    }
    // The angle the two eyes subtend at the shared aim — measured from `a`'s aim for both, so a small
    // permitted drift cannot flip the verdict by changing which origin each vector is taken from.
    let to = |camera: ShotCamera| {
        let d = [
            camera.eye[0] - a.look_at[0],
            camera.eye[1] - a.look_at[1],
            camera.eye[2] - a.look_at[2],
        ];
        let len = d[0]
            .mul_add(d[0], d[1].mul_add(d[1], d[2] * d[2]))
            .sqrt()
            .max(MIN_AUTHORED_RANGE);
        [d[0] / len, d[1] / len, d[2] / len]
    };
    let (u, v) = (to(a), to(b));
    let dot = u[0]
        .mul_add(v[0], u[1].mul_add(v[1], u[2] * v[2]))
        .clamp(-1.0, 1.0);
    dot.acos().to_degrees() < JUMP_CUT_MIN_DEGREES
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
    /// A camera the author placed by eye, or `None` to solve the placement from `size` and `angle`.
    ///
    /// `Option` and not a sentinel pose, because "the card decides" is a different KIND of answer from
    /// a set of coordinates — the same reasoning that makes [`RenderSettings::height`] an `Option`.
    /// `#[serde(default)]` so every cutscene authored before this field existed opens on the cards.
    ///
    /// `size` and `angle` stay on the recipe while this is `Some`, untouched and inert: clearing the
    /// camera returns the shot to exactly the framing it was authored with, and a gesture that
    /// destroyed the card would be one the author cannot undo by pressing the other button.
    #[serde(default)]
    pub camera: Option<ShotCamera>,
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
    /// The shape of the frame this cutscene is DELIVERED in.
    ///
    /// A shot is composed against an aspect ratio -- [`solve_shot`] takes one, and it decides how far
    /// back the camera must stand for the subject to fill the frame. Before this field the only aspect
    /// anyone could supply was the editor window's, so every shot was composed for whatever shape the
    /// author's stage happened to be at that instant: open the bottom dock and the same shot was
    /// composed for a different film. `Viewport` keeps that behaviour and says so; every other value
    /// pins the composition to a delivery frame and lets the stage draw the bars around it.
    #[serde(default)]
    pub delivery: Delivery,
    /// ADR-190 -- how this cutscene is rendered: what it delivers, at what rate, at what size, called
    /// what. Beside [`Cutscene::delivery`] for the reason `delivery` is here at all -- these are the
    /// author's answers about the DELIVERABLE, and a deliverable is a property of the cut and not of
    /// the dialog that last happened to be open.
    #[serde(default)]
    pub render: RenderSettings,
}

/// The frame a cutscene is composed and delivered in.
///
/// Serialised as its camelCase name, so a stored cutscene carries the author's choice rather than a
/// float nobody can read back. `Viewport` is not a ratio at all: it means "whatever the author is
/// looking through", which is the only honest answer before a delivery frame has been chosen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Delivery {
    /// Compose for the visible stage, whatever shape it currently is. No bars.
    #[default]
    Viewport,
    /// 16:9 - broadcast and web.
    Widescreen,
    /// 2.39:1 - anamorphic scope.
    Scope,
    /// 4:3 - academy.
    Academy,
    /// 1:1 - square.
    Square,
    /// 9:16 - vertical.
    Vertical,
}

impl Delivery {
    /// The delivery frame's aspect ratio (width / height), or `None` for [`Delivery::Viewport`], whose
    /// ratio is a property of the author's window rather than of the cutscene.
    #[must_use]
    pub fn ratio(self) -> Option<f32> {
        match self {
            Self::Viewport => None,
            Self::Widescreen => Some(16.0 / 9.0),
            Self::Scope => Some(2.39),
            Self::Academy => Some(4.0 / 3.0),
            Self::Square => Some(1.0),
            Self::Vertical => Some(9.0 / 16.0),
        }
    }

    /// What a user calls this frame. Plain enough to put in a control and in a read-out.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Viewport => "Match viewport",
            Self::Widescreen => "16:9 widescreen",
            Self::Scope => "2.39:1 scope",
            Self::Academy => "4:3 academy",
            Self::Square => "1:1 square",
            Self::Vertical => "9:16 vertical",
        }
    }

    /// The wire name, matching the serde representation. One list, so the catalogue a UI renders and
    /// the value it sends back cannot drift apart.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Viewport => "viewport",
            Self::Widescreen => "widescreen",
            Self::Scope => "scope",
            Self::Academy => "academy",
            Self::Square => "square",
            Self::Vertical => "vertical",
        }
    }

    /// Every delivery frame, in the order a picker should offer them.
    #[must_use]
    pub fn all() -> [Self; 6] {
        [
            Self::Viewport,
            Self::Widescreen,
            Self::Scope,
            Self::Academy,
            Self::Square,
            Self::Vertical,
        ]
    }

    /// Parse a wire name. Unknown names are refused rather than silently defaulted: a cutscene whose
    /// delivery frame quietly became "viewport" would be composed for a different film than the one
    /// the author asked for, and nothing would say so.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::all().into_iter().find(|d| d.key() == key)
    }
}

/// ADR-182 -- what a render DELIVERS.
///
/// TWO AND NOT FIVE. A movie is the thing a person can double-click; a sequence is the thing a
/// compositor can take. Every other container anybody would name is one of those two wearing a
/// different extension, and offering five would be four ways to ask the same question.
///
/// [`Self::Movie`] IS THE DEFAULT, for the reason 1080 is the default height and "as on screen" is
/// not: a render is the thing that leaves the editor, and what leaves it should be watchable without
/// a second program.
///
/// ADR-190 MOVED IT HERE, beside [`Delivery`]. It began life in the shell next to the code that
/// validates it, which was right while it was an argument to one command; once [`RenderSettings`]
/// put it in the document it became part of the cutscene's own vocabulary, and a document type that
/// lives in the layer above the document is a type the document cannot be serialised without.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RenderFormat {
    /// One H.264 MP4, encoded by the platform's own encoder.
    #[default]
    Movie,
    /// One lossless PNG per frame -- ADR-175's delivery, and still the right one for a compositor.
    Sequence,
}

impl RenderFormat {
    /// The wire name, matching the serde representation. One list, so a catalogue a UI renders and the
    /// value it sends back cannot drift apart.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Movie => "movie",
            Self::Sequence => "sequence",
        }
    }

    /// What a user calls it.
    ///
    /// A NAME AND NOT A SENTENCE. These are the words a picker draws, and the picker sits in a
    /// three-column field grid beside a help line that already says what the format costs -- so a
    /// label carrying the explanation too is the stutter ADR-175 found in the render ledger, and it
    /// was one worse than that: `PNG sequence -- one file per frame` does not fit the control, so the
    /// engine's own vocabulary reached the author as `PNG sequence - one file per fran`. A `<select>`
    /// clips its option text natively, inside its own box, which is why no `unclipped` claim over
    /// that control could see it and only the capture did.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Movie => "Movie (MP4)",
            Self::Sequence => "PNG sequence",
        }
    }

    /// Both, in the order a picker should offer them.
    #[must_use]
    pub fn all() -> [Self; 2] {
        [Self::Movie, Self::Sequence]
    }

    /// Read a wire name. Unknown names are refused rather than silently defaulted, for the reason
    /// [`Delivery::from_key`] refuses one: a render that quietly became a sequence when the author
    /// asked for a movie would deliver the wrong thing and say nothing.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::all().into_iter().find(|f| f.key() == key)
    }
}

/// The default rate: the one the rest of the world calls "film", and the one the mood's blend seconds
/// were chosen against.
pub const DEFAULT_RENDER_FPS: u32 = 24;

/// The default output height.
///
/// 1080 AND NOT "AS ON SCREEN", which is what the render dialog shipped with. A render is a DELIVERY --
/// the thing that leaves the editor -- and the size a stage happens to be after the author opened a
/// dock is not a delivery format. The old default silently made every sequence as tall as the window,
/// which on a laptop with both docks open is around 400 lines: a film nobody can use, produced by a
/// dialog that never asked.
pub const DEFAULT_RENDER_HEIGHT: u32 = 1080;

/// ADR-190 -- the answers a render needs, remembered on the cutscene it renders.
///
/// WHY THE DOCUMENT AND NOT THE DIALOG. Before this, every one of these was a `useState` seeded from a
/// constant on every open, so a person who had decided their cut delivers a 1440 scope sequence called
/// `weld-line-master` re-decided it, four controls at a time, every single render -- and none of the
/// four survived closing the dialog, let alone closing the editor. They are not preferences about the
/// tool; they are statements about the deliverable, exactly like [`Cutscene::delivery`], which has
/// lived in the document since ADR-166 for the same reason.
///
/// SESSION-SCOPED MEMORY WOULD NOT DO. It satisfies "reopen the dialog" and fails the thing that
/// matters: a cut is authored across days, and the answer to "what does this deliver" has to still be
/// there tomorrow. Being in the document also makes each change undoable and each one saved by the
/// same code path that saves everything else -- no second persistence mechanism, no new file.
///
/// EVERY FIELD IS RE-VALIDATED WHERE IT IS USED, not trusted from here. A hand-edited document can
/// carry `fps: 7`; `plan_render` refuses it by name, the same way it refuses one that arrived from a
/// UI. The document remembers an answer -- it does not get to authorise one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RenderSettings {
    /// What the render delivers.
    pub format: RenderFormat,
    /// Frames per second.
    pub fps: u32,
    /// The output height, or `None` for "as on screen" -- whatever the stage currently measures.
    /// `None` and not a sentinel `0`, because "the window decides" is a different KIND of answer from
    /// a number and the two behave differently everywhere they are read.
    pub height: Option<u32>,
    /// The file name, without an extension and before [`sanitise`](crate::shot) -- what the author
    /// typed, kept as they typed it.
    ///
    /// EMPTY MEANS "THE OBJECT'S NAME", and empty is what a fresh cutscene starts as. Storing the
    /// resolved name instead would freeze the object's name at the instant the cutscene was first
    /// rendered, so renaming the assembly would leave last month's name on every future file -- a
    /// stale copy of a fact that already lives somewhere else.
    pub name: String,
    /// Where the files go. Empty means "ask me", which is what every render did before ADR-190.
    ///
    /// A PATH IS THE ONE ANSWER HERE THAT IS ABOUT THE MACHINE and not about the film, and it is
    /// still stored beside the other four rather than in a preferences file, because "render this cut
    /// again" means "render it where it went last time" and the cut is the thing being rendered. The
    /// consequence is stated rather than hidden: a project opened on another machine carries a path
    /// that is not there, so the side that USES this treats a folder that does not exist exactly like
    /// an empty one -- it asks. Nothing fails, and nothing silently writes somewhere unexpected.
    #[serde(default)]
    pub folder: String,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            format: RenderFormat::Movie,
            fps: DEFAULT_RENDER_FPS,
            height: Some(DEFAULT_RENDER_HEIGHT),
            name: String::new(),
            folder: String::new(),
        }
    }
}

impl RenderSettings {
    /// The file stem to use for `owner_name`'s cut: what the author typed, or the object's own name
    /// when they have typed nothing.
    ///
    /// One function, because the dialog's field, the plan's read-out and the writer all need the same
    /// answer and a second copy of "…or the object name if blank" is how a file lands called `""`.
    #[must_use]
    pub fn stem_for(&self, owner_name: &str) -> String {
        let typed = self.name.trim();
        if typed.is_empty() {
            owner_name.to_string()
        } else {
            typed.to_string()
        }
    }
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
            delivery: Delivery::default(),
            render: RenderSettings::default(),
        }
    }
}

/// One tick of the engine's clock, seconds. Playback advances on the replay-stamped frame counter at
/// 60 Hz, so this is the finest step any moment of a cutscene can differ by.
pub const TICK_SECONDS: f32 = 1.0 / 60.0;

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

    /// How long the shot at `index` takes to become itself, seconds.
    ///
    /// A cutscene's first shot never blends — separately directed cutscenes stay hard cuts — and no
    /// blend may eat more than half the shot it opens. `playback_at` reads this rather than
    /// recomputing it, so "when is this shot on screen" has one answer: a UI that seeks past the
    /// window and the solver that draws it cannot disagree about where the window ends.
    #[must_use]
    pub fn blend_into(&self, index: usize) -> f32 {
        if index == 0 {
            return 0.0;
        }
        let Some(duration) = self.effective_shot_seconds(index) else {
            return 0.0;
        };
        self.mood.blend_seconds().min(duration * 0.5).max(0.0)
    }

    /// The first instant at which the shot at `index` is on screen ALONE, seconds on the cutscene
    /// clock — what "open shot 3" has to mean.
    ///
    /// Not the shot's start: at its start the transition weight is zero, so the frame there is
    /// entirely the shot BEFORE. And not the end of the blend window either, because
    /// `start + blend_into(index)` is computed in `f32` and lands a hair BELOW the strict `local <
    /// blend_span` test — measured, `Some((0, 0.9999998))` rather than `None`. One frame past it is
    /// the honest answer and needs no epsilon: `TICK_SECONDS` is the engine's own clock, so this is
    /// literally the first frame playback draws the shot by itself.
    #[must_use]
    pub fn opens_at(&self, index: usize) -> f32 {
        let start: f32 = (0..index)
            .filter_map(|i| self.effective_shot_seconds(i))
            .sum();
        let blend = self.blend_into(index);
        if blend > 0.0 {
            start + blend + TICK_SECONDS
        } else {
            start
        }
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
                let blend_span = self.blend_into(index);
                let blend_from = (index > 0 && blend_span > 1.0e-6 && local < blend_span)
                    .then(|| (index - 1, (local / blend_span).clamp(0.0, 1.0)));
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
    ///
    /// Subjects are named by their key, which is a fact about the document and not a word an author
    /// has ever seen. Use [`Self::problems_named`] wherever a display name can be resolved.
    #[must_use]
    pub fn problems(&self) -> Vec<ShotProblem> {
        self.problems_named(&|_| None)
    }

    /// The same list, with each shot's subject resolved to a display name by the caller — the exact
    /// pairing `reply_for` and `reply_with_names` already keep for the shot sentences.
    ///
    /// A cutscene stores `ShotRecipe::subject` as an entity key (`"412_3"`), so the jump-cut warning
    /// used to read *shots on "412_3" are framed identically*: the one line in the whole panel written
    /// in the document's vocabulary instead of the author's. The names live in the shell, which is why
    /// this is a parameter and not a lookup.
    #[must_use]
    pub fn problems_named(
        &self,
        subject_name: &dyn Fn(&str) -> Option<String>,
    ) -> Vec<ShotProblem> {
        let mut out = Vec::new();
        if self.shots.len() > MAX_SHOTS {
            out.push(ShotProblem::about_the_cut(format!(
                "a cutscene can hold at most {MAX_SHOTS} shots — this one has {}",
                self.shots.len()
            )));
        }
        for (i, shot) in self.shots.iter().enumerate() {
            let n = i + 1;
            if shot.seconds < MIN_SECONDS {
                out.push(ShotProblem::about(
                    i,
                    format!("shot {n} is shorter than {MIN_SECONDS}s — it will not read"),
                ));
            }
            if shot.seconds > MAX_SECONDS {
                out.push(ShotProblem::about(
                    i,
                    format!("shot {n} runs longer than {MAX_SECONDS}s"),
                ));
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
                out.push(ShotProblem::about(
                    i,
                    format!(
                        "shot {n} runs {:.1}s at this pacing — it may feel rushed",
                        self.effective_shot_seconds(i).unwrap_or(shot.seconds)
                    ),
                ));
            }
        }
        // A jump cut: the same subject, nearly the same framing, cut together. The classic amateur tell.
        //
        // ASKED OF WHATEVER DECIDES THE FRAME. For a card that is the size and the angle; for a placed
        // camera those two fields are inert, so comparing them would report a jump cut between two
        // completely different hand-framed views and stay silent about two identical ones. The
        // continuity check has to read the same thing the camera does or it is checking a decoration.
        for (first, pair) in self.shots.windows(2).enumerate() {
            let (a, b) = (&pair[0], &pair[1]);
            if a.subject != b.subject {
                continue;
            }
            let identical = match (a.camera, b.camera) {
                (Some(x), Some(y)) => x.is_usable() && y.is_usable() && placed_cut_is_flat(x, y),
                (None, None) => a.size == b.size && a.angle == b.angle,
                // One placed, one from a card: never the same frame by construction.
                _ => false,
            };
            if identical {
                // WHICH shots, always — the numbers are what the author needs to reach the control,
                // and they are the half of this sentence that never depends on a name being
                // resolvable. Four identical shots produce three warnings; naming only the subject
                // made all three byte-identical and none of them said where to look.
                let (m, n) = (first + 1, first + 2);
                // FILED UNDER THE SECOND SHOT, not the first. The warning is about a CUT, which
                // belongs to neither shot alone — and the one an author changes to fix it is the
                // second, because re-framing the first only moves the identical pair one place
                // earlier. So the control this warning now carries points at the shot whose size
                // or angle is the fix.
                out.push(ShotProblem::about(
                    first + 1,
                    match subject_name(&a.subject) {
                        Some(who) => format!(
                            "shots {m} and {n} on \"{who}\" are framed identically back to back — \
                             that reads as a jump cut; change the size or the angle"
                        ),
                        None => format!(
                            "shots {m} and {n} are framed identically back to back — that reads \
                             as a jump cut; change the size or the angle"
                        ),
                    },
                ));
            }
        }
        // No establishing shot: opening tight leaves the viewer lost. A placed opener is exempt for
        // the reason above — its `size` is a leftover card, and warning about it would be advice about
        // a control the author cannot even reach while the camera is theirs.
        if let Some(first) = self.shots.first() {
            if first.camera.is_none()
                && matches!(first.size, ShotSize::Close | ShotSize::ExtremeClose)
                && self.shots.len() > 1
            {
                out.push(ShotProblem::about(
                    0,
                    "the cutscene opens tight — a wide shot first would establish where we are",
                ));
            }
        }
        out
    }
}

/// One warning about a cutscene, and the shot it is about.
///
/// **Why the number is a field and not just a word in the sentence.** Every producer here already
/// wrote *"shot 2 ..."* into its prose, because an author cannot act on a warning without knowing
/// which shot it names. A reader can act on that; a CONTROL cannot. The panel drawing these lists had
/// no way to put a button beside *"shot 2 has nowhere good to film from"* that did anything to shot 2
/// — it held a sentence, and the shot number in it was as opaque as any other word. So the advice
/// ended in *"frame it yourself"* and the author went hunting for the shot the engine had just
/// identified.
///
/// It is also what lets the three producers below be read in one order. They answer different
/// questions — the document's own continuity, a placed camera's view, a negotiated placement's — and
/// were appended in producer order, so a cut with faults of two kinds listed *"shot 3 ..."* above
/// *"shot 1 ..."*. [`in_shot_order`] merges them, which needs exactly this field.
///
/// `shot` is `None` for the faults that belong to no single shot — there is one, and it is about the
/// cut being longer than a cutscene may be.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotProblem {
    /// The shot this is about, zero-based, or `None` when the fault is the whole cut's.
    pub shot: Option<usize>,
    /// The sentence, in the author's language — unchanged by this being structured.
    pub message: String,
}

impl ShotProblem {
    /// A fault of shot `index`.
    #[must_use]
    pub fn about(index: usize, message: impl Into<String>) -> Self {
        Self {
            shot: Some(index),
            message: message.into(),
        }
    }

    /// A fault of the cut as a whole, belonging to no one shot.
    #[must_use]
    pub fn about_the_cut(message: impl Into<String>) -> Self {
        Self {
            shot: None,
            message: message.into(),
        }
    }
}

/// Put a cut's warnings into the order its author reads the timeline in.
///
/// The three producers each sweep the shots in order, so each list is already sorted; concatenating
/// them is not. A cut whose shot 1 opens tight and whose shot 3 is boxed in listed the boxed-in one
/// first, because it came from the producer that runs last — and a list that is sorted by which
/// function noticed the fault is a list sorted by nothing the reader can see.
///
/// STABLE, so two faults of one shot keep the order their producer chose: those are ranked
/// worst-first inside each producer, and re-sorting by shot must not throw that away. Cut-wide faults
/// lead, because a cutscene that holds more shots than it may is a fact about all of them.
#[must_use]
pub fn in_shot_order(mut problems: Vec<ShotProblem>) -> Vec<ShotProblem> {
    problems.sort_by_key(|problem| problem.shot.map_or(0, |shot| shot + 1));
    problems
}

/// Where along a placed shot's move the world is asked about it.
///
/// The authored pose, the middle and the end. A placed camera is never re-placed, so this is not a
/// search like [`PATH_SAMPLES`] — it is three questions, and the middle one is there because an orbit
/// swings THROUGH the thing it will end up clear of. Three and not five: this is a diagnostic paid for
/// on every read of a cutscene, not a per-candidate cost paid once per film.
const PLACED_PATH_SAMPLES: [f32; 3] = [0.0, 0.5, 1.0];

/// What the WORLD has to say about the shots whose cameras the author placed — the half
/// [`Cutscene::problems`] structurally cannot answer, in the same vocabulary and on the same list.
///
/// **Why this is a separate function and not more of `problems()`.** `problems()` is a pure reading of
/// the document: two shots framed alike, a shot too short for its pacing, an opener that starts tight.
/// Every one of those is decidable from the cutscene alone. Whether a camera is standing inside a wall
/// is not a fact about the cutscene at all — it is a fact about the other 15,710 parts of an imported
/// factory, and the only thing that can answer it is the engine holding the scene. So the words live
/// here, beside the rest of the vocabulary, and the measurement is supplied by the caller.
///
/// **Why only placed cameras.** A card shot's placement is already negotiated against exactly this
/// measurement — [`plan_shot`] walks a ladder of alternatives until the world stops objecting, and
/// widens or swings the shot until it does. Warning about a card shot would be advice about something
/// the engine has already handled, and about controls (`size`, `angle`) whose values the author did not
/// choose. A placed camera is the one case where the engine deliberately does NOT correct anything —
/// see [`solve_shot_adjusted`] — and a promise not to interfere is exactly what obliges it to speak.
///
/// **Never blocking, never corrective.** Nothing here moves a camera. One line per shot, worst fault
/// first, because a camera buried in a machine cannot see its subject either and saying both is saying
/// the same thing twice.
///
/// `subject_center` is where that shot's subject is standing (the panning head's aim needs it, and a
/// locked-off camera ignores it); `look` is the world's verdict on one pose, the same answer
/// [`plan_shot`] negotiates against. A caller with no occlusion data hands back [`Vantage::OPEN`] and
/// this returns nothing at all.
#[must_use]
pub fn placed_camera_problems(
    cut: &Cutscene,
    mut subject_center: impl FnMut(usize, &ShotRecipe) -> [f32; 3],
    mut look: impl FnMut(usize, &CameraSample) -> Vantage,
) -> Vec<ShotProblem> {
    let mut out = Vec::new();
    for (index, shot) in cut.shots.iter().enumerate() {
        let Some(camera) = shot.camera.filter(ShotCamera::is_usable) else {
            continue;
        };
        let center = subject_center(index, shot);
        let still = matches!(shot.motion, ShotMove::Hold) || shot.amount.clamp(0.0, 1.0) == 0.0;
        let samples: &[f32] = if still {
            &PLACED_PATH_SAMPLES[..1]
        } else {
            &PLACED_PATH_SAMPLES
        };
        // The worst moment decides, and WHICH moment it was changes the sentence: "your camera is in
        // a wall" and "your push-in ends in a wall" are different faults with different fixes, and a
        // message that cannot tell them apart sends the author to re-place a camera that was fine.
        let mut worst: Option<(bool, Vantage)> = None;
        for &progress in samples {
            let pose = solve_placed_shot(shot, camera, center, ease_in_out(progress));
            let vantage = look(index, &pose);
            let moving = progress > 0.0;
            match worst {
                Some((_, held)) if held.score() <= vantage.score() => {}
                _ => worst = Some((moving, vantage)),
            }
        }
        let Some((moving, vantage)) = worst else {
            continue;
        };
        let n = index + 1;
        // At the authored pose the author is looking at the fault on the stage; anywhere else they are
        // not, because what the stage shows them is the frame they placed and not the frame the move
        // arrives at. Two sentences, because "your camera is in a wall" and "your push-in ends in a
        // wall" are different faults with different fixes — one moves the tripod, the other softens
        // the move — and a message that cannot tell them apart sends the author to the wrong control.
        if vantage.eye_inside {
            out.push(ShotProblem::about(index, if moving {
                format!(
                    "shot {n}'s move takes its camera inside something — those frames will be solid \
                     colour"
                )
            } else {
                format!("shot {n}'s camera is inside something — that frame will be solid colour")
            }));
        } else if vantage.clear < MIN_CLEAR_FRACTION {
            let hidden = (1.0 - vantage.clear.clamp(0.0, 1.0)) * 100.0;
            out.push(ShotProblem::about(index, if moving {
                format!(
                    "shot {n}'s move puts {hidden:.0}% of its subject behind something else before it \
                     ends"
                )
            } else {
                format!(
                    "shot {n}'s subject is mostly hidden from where its camera stands — {hidden:.0}% \
                     of it is behind something else"
                )
            }));
        } else if vantage.crowded > MAX_CROWDED_FRACTION {
            out.push(ShotProblem::about(index, if moving {
                format!(
                    "shot {n}'s move takes its camera into a corner — most of the frame it ends on is \
                     whatever it is standing against"
                )
            } else {
                format!(
                    "shot {n}'s camera is boxed in — most of that frame is whatever it is standing \
                     against"
                )
            }));
        }
    }
    out
}

/// What the WORLD has to say about the shots whose cameras the engine chose — the other half of
/// [`placed_camera_problems`], and the larger one.
///
/// **Why this is separate from `placed_camera_problems`, and why the sentences differ.** They report
/// the same three faults from opposite situations, and the situation is what makes the message
/// actionable. A placed camera is a pose the author framed by eye and the engine promised not to touch,
/// so "your camera is in a wall" sends them to move the tripod they put there. A card shot has no
/// camera the author can point at: [`plan_shot`] chose the placement, and by the time this can fire it
/// has already tried **every framing and every yaw on the ladder** and found nothing
/// [`Vantage::acceptable`] — so telling that author to change the size or the angle would be advice
/// the engine has already taken, fifty-four times, on their behalf. The only fixes left are outside the
/// card: frame it by hand, or film something else.
///
/// **It can only fire on the least-bad path.** Every other exit from [`plan_shot`] returns a placement
/// the world did not object to, or the identity adjustment carrying [`Vantage::OPEN`]. So an
/// unacceptable [`ShotAdjustment::settled`] is *by construction* the case where the whole ladder was
/// walked and nothing survived it — which is what licenses the sentences to say so.
///
/// `settled` is the caller's plan for shot `index`, normally the same [`ShotAdjustment`] the runtime
/// will film with. `None` means the caller did not measure this shot, and nothing is said about it —
/// the same fail-quiet contract [`plan_shot`] keeps for a world with no occlusion data.
///
/// Never blocking, never corrective: one line per shot, worst fault first, because a camera buried in a
/// machine cannot see its subject either and saying both is saying the same thing twice.
#[must_use]
pub fn card_shot_problems(
    cut: &Cutscene,
    mut settled: impl FnMut(usize, &ShotRecipe) -> Option<ShotAdjustment>,
) -> Vec<ShotProblem> {
    let mut out = Vec::new();
    for (index, shot) in cut.shots.iter().enumerate() {
        // A placed camera is the other function's business, and asking here would report the
        // planner's untouched identity adjustment as a clean bill of health for a pose it never judged.
        if shot.camera.is_some_and(|camera| camera.is_usable()) {
            continue;
        }
        let Some(plan) = settled(index, shot) else {
            continue;
        };
        let vantage = plan.settled;
        if vantage.acceptable() {
            continue;
        }
        let n = index + 1;
        // ONE LEAD, THREE SPECIFICS, and the lead is the half that matters. What the author must
        // learn is that the search has already been done — otherwise the obvious response to any of
        // these is to try another size, which is the fifty-four things the engine just tried. The
        // specific fault is about the placement it SETTLED on, and the wording says so rather than
        // claiming every rejected candidate failed the same way; they did not, and the difference
        // would be a sentence the engine cannot support.
        let lead = format!(
            "shot {n} has nowhere good to film from — the engine tried every framing \
                            and angle, and"
        );
        // AND THE FIRST HALF OF THE ADVICE IS NOW A PLACE, not an instruction to go and find one.
        // "Frame it yourself" was the only fix this sentence could offer, and it started the
        // author from wherever the viewport happened to be — which, on a 262 m import, is a manual
        // orbit to a part they cannot see, to redo by hand the search the engine has just
        // finished. The least-bad placement is in `ShotAdjustment` and `worst_moment` recovers the
        // instant the rest of this sentence describes, so the advice can name a control that
        // stands there.
        let out_of = "press \"Take me there\" to stand where it tried, then frame it \
                      yourself with \"Shoot from this view\" — or film a different part";
        if vantage.eye_inside {
            out.push(ShotProblem::about(
                index,
                format!(
                "{lead} in the best one it found the camera is inside something, so those frames \
                 will be solid colour; {out_of}"
            ),
            ));
        } else if vantage.clear < MIN_CLEAR_FRACTION {
            let hidden = (1.0 - vantage.clear.clamp(0.0, 1.0)) * 100.0;
            out.push(ShotProblem::about(
                index,
                format!(
                "{lead} in the best one it found {hidden:.0}% of the subject is behind something \
                 else; {out_of}"
            ),
            ));
        } else if vantage.crowded > MAX_CROWDED_FRACTION {
            out.push(ShotProblem::about(
                index,
                format!(
                    "{lead} in the best one it found most of the frame is whatever the camera is \
                 standing against rather than the subject; {out_of}"
                ),
            ));
        }
    }
    out
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
    if let Some(camera) = shot.camera.filter(ShotCamera::is_usable) {
        return solve_placed_shot(shot, camera, subject.center, progress);
    }
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

/// Solve a shot whose camera the author placed: the stored pose, with the move applied around it.
///
/// THE MOVE IS THE WHOLE POINT OF DOING IT THIS WAY. A placed camera could have been a pose and
/// nothing else — an escape hatch, one static frame, the card vocabulary switched off. Instead the
/// same six verbs keep working, measured against the shot's OWN stand-off rather than the subject's
/// radius, so "place the camera here and push in" is a sentence the engine can film. That is the
/// difference between an override and a first-class shot.
///
/// The aim is held fixed through every verb. A crane that re-aimed as it rose would be a camera on a
/// boom with nobody operating the head, and the author's frame is the one thing this must preserve.
///
/// ADR-195 — `subject_center` is consulted ONLY through [`ShotCamera::aim_at`], and that returns the
/// authored point unless the author asked for a panning head. So a locked-off camera is still a pure
/// function of its own pose: the subject can be sampled anywhere at all and the frame does not move.
fn solve_placed_shot(
    shot: &ShotRecipe,
    camera: ShotCamera,
    subject_center: [f32; 3],
    progress: f32,
) -> CameraSample {
    // THE MOVE SCALES OFF THE SHOT'S OWN STAND-OFF, not off the subject. On the card path a crane
    // rises `radius * 4.0` from a distance of roughly `radius * 4.9`, and falls `radius * 2.0` — so
    // the verbs are really "about eight tenths of the stand-off up" and "about four tenths down".
    // Re-derived here against `range` rather than re-tuned by eye, so a placed crane feels like the
    // crane the author already knows.
    const CRANE_UP_FRACTION: f32 = 0.8;
    const CRANE_DOWN_FRACTION: f32 = 0.4;
    /// The closest a push-in may bring a placed camera, as a fraction of its authored stand-off.
    /// A card's full-strength push-in floors at `radius * 1.05` out of a base of roughly
    /// `radius * 4.9`, so it keeps about a fifth of its distance; this keeps the same fifth.
    const MIN_PLACED_PUSH_FRACTION: f32 = 0.2;

    let p = progress.clamp(0.0, 1.0);
    let amount = shot.amount.clamp(0.0, 1.0);
    // THE LIVE AIM, and every length below is measured against it. A push-in on a panning head
    // closes a fifth of the distance to where the subject IS, not to where it was standing when the
    // author pressed the button — the two are the same shot only while nothing moves, and a move
    // solved against a stale sight line is a camera sliding past its subject.
    let look_at = camera.aim_at(subject_center);
    let range = {
        let d = [
            camera.eye[0] - look_at[0],
            camera.eye[1] - look_at[1],
            camera.eye[2] - look_at[2],
        ];
        d[0].mul_add(d[0], d[1].mul_add(d[1], d[2] * d[2])).sqrt()
    };

    let (dist_scale, height_extra, yaw_extra) = match shot.motion {
        ShotMove::Hold => (1.0, 0.0, 0.0),
        ShotMove::PushIn => (1.0 - amount * p, 0.0, 0.0),
        ShotMove::PullOut => (1.0 + amount * p, 0.0, 0.0),
        ShotMove::Orbit => (1.0, 0.0, amount * p * 90.0),
        ShotMove::CraneUp => (1.0, amount * p * range * CRANE_UP_FRACTION, 0.0),
        ShotMove::CraneDown => (1.0, -amount * p * range * CRANE_DOWN_FRACTION, 0.0),
    };

    // Slide along the placed sight line. Floored so a full-strength push-in stops at a distance a
    // camera could still be, rather than arriving at the point it aims at — the same job `radius *
    // 1.05` does on the card path, expressed against the only length this shot has. The number is
    // read OFF the card path rather than chosen: a full-strength card push-in ends at about a fifth
    // of the distance it started from, so a placed one does too.
    let mut eye = camera.eye;
    #[allow(
        clippy::float_cmp,
        reason = "the identity scale must leave the authored pose BIT-IDENTICAL, which is the question"
    )]
    if dist_scale != 1.0 {
        let scale = dist_scale.max(MIN_PLACED_PUSH_FRACTION);
        eye = [
            look_at[0] + (camera.eye[0] - look_at[0]) * scale,
            look_at[1] + (camera.eye[1] - look_at[1]) * scale,
            look_at[2] + (camera.eye[2] - look_at[2]) * scale,
        ];
    }
    let mut pose = CameraSample {
        eye,
        look_at,
        fov_deg: camera.fov_deg,
    };
    if yaw_extra != 0.0 {
        pose = rotate_eye_about_subject(pose, look_at, yaw_extra);
    }
    pose.eye[1] += height_extra;
    pose
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
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

/// [`Vantage::OPEN`], and written by hand because the DERIVED default would be the opposite reading.
///
/// A derived `Default` is all-zeroes — `clear: 0.0`, which is "the subject is completely blocked", the
/// single worst verdict this type can carry. Everything downstream of an unmeasured placement (a
/// document saved before the field existed, a caller with no occlusion data) would then read as the
/// most alarming thing the engine can say, and the whole vocabulary is built the other way round: a
/// caller that cannot answer hands back `OPEN` and nothing changes.
impl Default for Vantage {
    fn default() -> Self {
        Self::OPEN
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
    /// WHAT THE WORLD SAID ABOUT THE PLACEMENT THIS ADJUSTMENT SETTLED ON, at its worst moment.
    ///
    /// The negotiation's own verdict on its own answer, carried out of the search instead of being
    /// thrown away inside it. [`plan_shot`] scores fifty-four placements and, when none of them is
    /// [`Vantage::acceptable`], films the least bad — a decision that is right (a shot has to be filmed
    /// from somewhere) and was, until this field existed, completely silent: the run recorded it in a
    /// developer diagnostic and the author found out weeks later, from the file.
    ///
    /// [`Vantage::OPEN`] is the unmeasured reading — the identity adjustment, a placed camera the
    /// planner returns untouched, or a caller with no occlusion data — and it is acceptable, so nothing
    /// downstream ever warns about a placement nobody looked at.
    #[serde(default)]
    pub settled: Vantage,
}

impl ShotAdjustment {
    /// The identity adjustment for one shot — its own authored size, no yaw change, and no verdict.
    #[must_use]
    pub fn authored(shot: &ShotRecipe) -> Self {
        Self {
            size: shot.size,
            yaw_offset_deg: 0.0,
            steps: 0,
            settled: Vantage::OPEN,
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
    // A PLACED CAMERA IS NOT NEGOTIATED. Every correction below — the widening, the yaw detour, the
    // floor, the room — exists because a card is a description the solver turns into a position, and
    // the solver cannot see the other 15,710 parts of a factory. An author who framed the shot by eye
    // has already looked at the result: the obstruction, the height and the wall are all things they
    // were staring at when they pressed the button. Correcting that is not help, it is the engine
    // quietly filming a different shot from the one on screen.
    if shot.camera.is_some_and(|camera| camera.is_usable()) {
        return solve_shot_eased(shot, subject, progress, aspect, fov_deg);
    }
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

/// ADR-200 — WHERE this shot is filmed from, and at which instant it is worst.
///
/// The pose an author needs to be standing at to see what a warning is about. [`plan_shot`] judges
/// every candidate at [`PATH_SAMPLES`] and keeps the verdict from the worst-scoring one, because a
/// camera path is only as good as the frame a viewer would most object to — and it keeps only the
/// verdict, so the moment that produced it was gone by the time anything could point at it. This
/// re-walks the same five samples for a placement already chosen, with the same comparison, and
/// hands back the pose and the progress.
///
/// **The same comparison, deliberately.** `score() < worst` is strict, so ties go to the earliest
/// sample exactly as they do inside the search. A function that agreed with `plan_shot` about the
/// verdict but not about which frame produced it would take an author to a frame that looks fine
/// and tell them it is the problem.
///
/// A still shot solves to one pose at every sample, so this degenerates to the opening and needs no
/// special case. A placed camera is returned untouched by [`solve_shot_adjusted`], so this reports
/// the author's own pose — which is the right answer for the placed half of the warning vocabulary:
/// standing there IS the solid-colour frame.
///
/// `look` is the world's verdict on one pose, the same one [`plan_shot`] negotiated against. A caller
/// with no occlusion data hands back [`Vantage::OPEN`] every time, and this then returns the opening
/// pose — the honest answer when nothing can be measured.
#[must_use]
pub fn worst_moment(
    shot: &ShotRecipe,
    adjustment: ShotAdjustment,
    subject: SubjectSample,
    aspect: f32,
    fov_deg: f32,
    mut look: impl FnMut(&CameraSample, f32) -> Vantage,
) -> (f32, CameraSample, Vantage) {
    let mut worst: Option<(f32, CameraSample, Vantage)> = None;
    for progress in PATH_SAMPLES {
        let pose = solve_shot_adjusted(shot, adjustment, subject, progress, aspect, fov_deg);
        let vantage = look(&pose, progress);
        if worst.is_none_or(|(_, _, held)| vantage.score() < held.score()) {
            worst = Some((progress, pose, vantage));
        }
    }
    worst.unwrap_or_else(|| {
        // PATH_SAMPLES is a non-empty constant, so this is unreachable; written as a value rather
        // than an unwrap so a future edit to that constant cannot turn a diagnostic into a panic.
        let pose = solve_shot_adjusted(shot, adjustment, subject, 0.0, aspect, fov_deg);
        (0.0, pose, Vantage::OPEN)
    })
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
    // The ladder walks framings and yaws, and a placed camera has neither: its size is inert and its
    // yaw is the author's. Returning the identity here is the same decision `solve_shot_adjusted`
    // makes, stated once at the top so no candidate is ever scored against a shot that cannot move.
    if shot.camera.is_some_and(|camera| camera.is_usable()) {
        return ShotAdjustment::authored(shot);
    }
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
            let mut candidate = ShotAdjustment {
                size,
                yaw_offset_deg: yaw,
                steps,
                settled: Vantage::OPEN,
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
                worst_backing = worst_backing.min(vantage.backing);
                // The verdict is kept, not just the number derived from it. `worst_score` is what the
                // ranking uses; `settled` is what the author is told, and "your camera is buried" and
                // "your subject is behind something" are different sentences a single float cannot
                // tell apart. Selected by the SAME comparison, so the moment that decided the ranking
                // is the moment that gets described.
                let score = vantage.score();
                if score < worst_score {
                    worst_score = score;
                    candidate.settled = vantage;
                }
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
            camera: None,
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
                .any(|problem| problem.message.contains("rushed"))
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
            delivery: Delivery::Scope,
            render: RenderSettings::default(),
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
            delivery: Delivery::Scope,
            render: RenderSettings::default(),
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
            delivery: Delivery::Scope,
            render: RenderSettings::default(),
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
            delivery: Delivery::Scope,
            render: RenderSettings::default(),
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
            delivery: Delivery::Viewport,
            render: RenderSettings::default(),
        };
        let problems = jump.problems();
        assert!(
            problems.iter().any(|p| p.message.contains("jump cut")),
            "an identical back-to-back framing is a jump cut: {problems:?}"
        );
        assert!(
            problems.iter().any(|p| p.message.contains("opens tight")),
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
            delivery: Delivery::Scope,
            render: RenderSettings::default(),
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
                    settled: Vantage::OPEN,
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

    // ---------------------------------------------------------------------------------------------
    // ADR-192 — a camera the author PLACED. Every test here asks the same question a different way:
    // is the pose that gets filmed the pose that was on screen when they pressed the button?
    // ---------------------------------------------------------------------------------------------

    fn placed(motion: ShotMove, amount: f32) -> ShotRecipe {
        ShotRecipe {
            motion,
            amount,
            camera: Some(ShotCamera {
                eye: [6.0, 3.0, 8.0],
                look_at: [0.0, 0.5, 0.0],
                fov_deg: 45.0,
                track: None,
            }),
            ..shot(ShotSize::Close, ShotAngle::Profile, ShotMove::Hold)
        }
    }

    #[test]
    fn a_placed_camera_is_filmed_exactly_where_it_was_put() {
        let s = placed(ShotMove::Hold, 0.35);
        let stored = s.camera.unwrap();
        // Every progress, and every aspect a delivery frame can hand it: a held placement is not a
        // function of any of them, and the day it becomes one the gesture's promise is false.
        for progress in [0.0, 0.25, 0.5, 0.75, 1.0] {
            for aspect in [16.0 / 9.0, 2.39, 1.0] {
                let pose = solve_shot(&s, cube(), progress, aspect, 50.0);
                assert_eq!(pose.eye.map(f32::to_bits), stored.eye.map(f32::to_bits));
                assert_eq!(
                    pose.look_at.map(f32::to_bits),
                    stored.look_at.map(f32::to_bits)
                );
                // The LENS THEY FRAMED THROUGH, not the cutscene runtime's 50 degrees.
                assert_eq!(pose.fov_deg.to_bits(), 45.0_f32.to_bits());
            }
        }
    }

    #[test]
    fn the_size_and_the_angle_stop_deciding_anything_once_a_camera_is_placed() {
        let stored = ShotCamera {
            eye: [6.0, 3.0, 8.0],
            look_at: [0.0, 0.5, 0.0],
            fov_deg: 45.0,
            track: None,
        };
        let mut poses = Vec::new();
        for size in [
            ShotSize::ExtremeWide,
            ShotSize::Medium,
            ShotSize::ExtremeClose,
        ] {
            for angle in [ShotAngle::Front, ShotAngle::Low, ShotAngle::High] {
                let s = ShotRecipe {
                    camera: Some(stored),
                    ..shot(size, angle, ShotMove::Hold)
                };
                poses.push(solve_shot(&s, cube(), 0.5, 16.0 / 9.0, 50.0));
            }
        }
        assert!(
            poses.windows(2).all(|p| p[0] == p[1]),
            "a card still moved a placed camera: {poses:?}"
        );
    }

    #[test]
    fn a_placed_camera_still_pushes_in_along_its_own_sight_line() {
        let s = placed(ShotMove::PushIn, 0.5);
        let stored = s.camera.unwrap();
        let start = solve_shot(&s, cube(), 0.0, 16.0 / 9.0, 50.0);
        let end = solve_shot(&s, cube(), 1.0, 16.0 / 9.0, 50.0);
        assert_eq!(start.eye.map(f32::to_bits), stored.eye.map(f32::to_bits));
        // Half the stand-off closed, and the aim never moved: that is what a push-in IS.
        assert!(
            (dist(end.eye, stored.look_at) - stored.range() * 0.5).abs() < 1.0e-3,
            "a 0.5 push-in should halve the {}m stand-off; it ended at {}m",
            stored.range(),
            dist(end.eye, stored.look_at)
        );
        assert_eq!(
            end.look_at.map(f32::to_bits),
            stored.look_at.map(f32::to_bits)
        );
    }

    #[test]
    fn a_full_strength_push_in_on_a_placed_camera_stops_where_a_card_would() {
        let s = placed(ShotMove::PushIn, 1.0);
        let stored = s.camera.unwrap();
        let end = solve_shot(&s, cube(), 1.0, 16.0 / 9.0, 50.0);
        let left = dist(end.eye, stored.look_at) / stored.range();
        assert!(
            (0.15..=0.25).contains(&left),
            "a full-strength placed push-in kept {left} of its stand-off; a card keeps about a fifth"
        );
    }

    #[test]
    fn a_placed_camera_orbits_about_its_own_aim_and_keeps_its_stand_off() {
        let s = placed(ShotMove::Orbit, 1.0);
        let stored = s.camera.unwrap();
        let end = solve_shot(&s, cube(), 1.0, 16.0 / 9.0, 50.0);
        assert!((dist(end.eye, stored.look_at) - stored.range()).abs() < 1.0e-3);
        assert!(
            (end.eye[1] - stored.eye[1]).abs() < 1.0e-4,
            "an orbit changed the camera height"
        );
        assert!(
            !placed_cut_is_flat(
                stored,
                ShotCamera {
                    eye: end.eye,
                    ..stored
                }
            ),
            "a full-strength orbit is a 90-degree move and cannot read as a flat cut"
        );
    }

    #[test]
    fn a_placed_crane_holds_its_aim_while_it_rises() {
        let up = solve_shot(
            &placed(ShotMove::CraneUp, 1.0),
            cube(),
            1.0,
            16.0 / 9.0,
            50.0,
        );
        let down = solve_shot(
            &placed(ShotMove::CraneDown, 1.0),
            cube(),
            1.0,
            16.0 / 9.0,
            50.0,
        );
        let stored = placed(ShotMove::Hold, 0.0).camera.unwrap();
        assert!(up.eye[1] > stored.eye[1] + 1.0);
        assert!(down.eye[1] < stored.eye[1] - 1.0);
        for pose in [up, down] {
            assert_eq!(
                pose.look_at.map(f32::to_bits),
                stored.look_at.map(f32::to_bits),
                "a crane re-aimed itself; the author's frame is the one thing it must hold"
            );
        }
    }

    #[test]
    fn the_planner_does_not_negotiate_a_placed_camera() {
        // A world with a wall around the subject: anything standing closer than twelve units is
        // buried in it. On the card path that drives the ladder — a close card has to widen until it
        // is far enough out — and a placed camera has already been judged, by eye, by the author.
        //
        // Deliberately NOT a constant refusal. A `look` that objects to every candidate equally
        // scores them all the same, and the least-bad tie-break then hands back the authored
        // placement anyway: the negative control would pass while proving nothing.
        let hostile = |pose: &CameraSample, _: f32| {
            let far = dist(pose.eye, [0.0, 0.0, 0.0]) > 12.0;
            Vantage {
                eye_inside: !far,
                clear: if far { 1.0 } else { 0.0 },
                backing: 1.0,
                crowded: 0.0,
            }
        };
        let s = placed(ShotMove::Hold, 0.35);
        let plan = plan_shot(&s, cube(), 16.0 / 9.0, 50.0, hostile);
        assert!(plan.is_authored(&s));
        assert_eq!(plan.steps, 0);

        let card = shot(ShotSize::ExtremeClose, ShotAngle::Low, ShotMove::Hold);
        let card_plan = plan_shot(&card, cube(), 16.0 / 9.0, 50.0, hostile);
        assert!(
            !card_plan.is_authored(&card),
            "the negative control failed: the ladder did not move a CARD in a hostile world, so \
             the placed-camera result above proves nothing"
        );
    }

    #[test]
    fn neither_the_floor_nor_the_room_moves_a_placed_camera() {
        let underground = ShotCamera {
            eye: [4.0, -3.0, 4.0],
            look_at: [0.0, 0.0, 0.0],
            fov_deg: 40.0,
            track: None,
        };
        let s = ShotRecipe {
            camera: Some(underground),
            ..shot(ShotSize::Medium, ShotAngle::Front, ShotMove::Hold)
        };
        let boxed_in = SubjectSample {
            stage: Stage {
                room: Some(([-6.0, 0.2, -6.0], [6.0, 4.0, 6.0])),
            },
            ..cube()
        };
        let pose = solve_shot_adjusted(
            &s,
            ShotAdjustment::authored(&s),
            boxed_in,
            0.0,
            16.0 / 9.0,
            50.0,
        );
        assert_eq!(
            pose.eye.map(f32::to_bits),
            underground.eye.map(f32::to_bits)
        );

        // The negative control: the same room DOES move a card, so the assertion above is about the
        // placement and not about a room that never confines anything.
        let card = shot(ShotSize::ExtremeWide, ShotAngle::Low, ShotMove::Hold);
        let card_pose = solve_shot_adjusted(
            &card,
            ShotAdjustment::authored(&card),
            boxed_in,
            0.0,
            16.0 / 9.0,
            50.0,
        );
        let unconfined = solve_shot(&card, cube(), 0.0, 16.0 / 9.0, 50.0);
        assert!(
            dist(card_pose.eye, unconfined.eye) > 1.0e-3,
            "the negative control failed: the room did not move a card either"
        );
    }

    #[test]
    fn a_degenerate_placed_camera_falls_back_to_the_card_rather_than_filming_nothing() {
        for bad in [
            ShotCamera {
                eye: [1.0, 1.0, 1.0],
                look_at: [1.0, 1.0, 1.0],
                fov_deg: 45.0,
                track: None,
            },
            ShotCamera {
                eye: [f32::NAN, 0.0, 0.0],
                look_at: [0.0, 0.0, 0.0],
                fov_deg: 45.0,
                track: None,
            },
            ShotCamera {
                eye: [3.0, 0.0, 0.0],
                look_at: [0.0, 0.0, 0.0],
                fov_deg: 0.0,
                track: None,
            },
        ] {
            assert!(!bad.is_usable());
            let s = ShotRecipe {
                camera: Some(bad),
                ..shot(ShotSize::Medium, ShotAngle::Front, ShotMove::Hold)
            };
            let card = shot(ShotSize::Medium, ShotAngle::Front, ShotMove::Hold);
            assert_eq!(
                solve_shot(&s, cube(), 0.5, 16.0 / 9.0, 50.0),
                solve_shot(&card, cube(), 0.5, 16.0 / 9.0, 50.0),
            );
        }
    }

    #[test]
    fn the_jump_cut_warning_reads_the_poses_once_the_cameras_are_placed() {
        let base = ShotCamera {
            eye: [0.0, 2.0, 10.0],
            look_at: [0.0, 0.0, 0.0],
            fov_deg: 45.0,
            track: None,
        };
        let swung = |degrees: f32| {
            let (sin, cos) = degrees.to_radians().sin_cos();
            ShotCamera {
                eye: [10.0 * sin, 2.0, 10.0 * cos],
                ..base
            }
        };
        let cut_of = |a: Option<ShotCamera>, b: Option<ShotCamera>| Cutscene {
            version: 1,
            shots: vec![
                ShotRecipe {
                    camera: a,
                    ..shot(ShotSize::Wide, ShotAngle::Front, ShotMove::Hold)
                },
                ShotRecipe {
                    id: "shot-2".into(),
                    camera: b,
                    ..shot(ShotSize::Wide, ShotAngle::Front, ShotMove::Hold)
                },
            ],
            mood: Mood::Normal,
            delivery: Delivery::Widescreen,
            render: RenderSettings::default(),
        };
        let flags = |c: &Cutscene| c.problems().iter().any(|p| p.message.contains("jump cut"));

        // Ten degrees apart: two frames a viewer reads as one jolt — and BOTH carry the same card,
        // which is exactly the case the old size-and-angle check could not see.
        assert!(flags(&cut_of(Some(base), Some(swung(10.0)))));
        // Past the thirty-degree rule: a real cut.
        assert!(!flags(&cut_of(Some(base), Some(swung(40.0)))));
        // The same axis, but a genuine punch-in.
        assert!(!flags(&cut_of(
            Some(base),
            Some(ShotCamera {
                eye: [0.0, 0.8, 4.0],
                ..base
            })
        )));
        // One placed, one from a card: never the same frame by construction.
        assert!(!flags(&cut_of(None, Some(base))));
        // ...and the card path is untouched.
        assert!(flags(&cut_of(None, None)));
    }

    #[test]
    fn a_placed_opener_is_not_told_it_opened_tight() {
        let tight = shot(ShotSize::ExtremeClose, ShotAngle::Front, ShotMove::Hold);
        let two = |first: ShotRecipe| Cutscene {
            version: 1,
            shots: vec![
                first,
                ShotRecipe {
                    id: "shot-2".into(),
                    ..shot(ShotSize::Wide, ShotAngle::Profile, ShotMove::Hold)
                },
            ],
            mood: Mood::Normal,
            delivery: Delivery::Widescreen,
            render: RenderSettings::default(),
        };
        let opens_tight = |c: &Cutscene| {
            c.problems()
                .iter()
                .any(|p| p.message.contains("opens tight"))
        };
        assert!(opens_tight(&two(tight.clone())));
        assert!(!opens_tight(&two(ShotRecipe {
            camera: Some(ShotCamera {
                eye: [0.0, 2.0, 40.0],
                look_at: [0.0, 0.0, 0.0],
                fov_deg: 45.0,
                track: None,
            }),
            ..tight
        })));
    }

    #[test]
    fn a_cutscene_authored_before_placed_cameras_opens_on_its_cards() {
        let blob = r#"{"version":1,"shots":[{"id":"shot-1","subject":"1_0","size":"wide",
            "angle":"front","motion":"hold","amount":0.35,"seconds":2.0}],"mood":"normal",
            "delivery":"widescreen"}"#;
        let cut: Cutscene = serde_json::from_str(blob).expect("an older cutscene still opens");
        assert_eq!(cut.shots[0].camera, None);
        assert_eq!(
            solve_shot(&cut.shots[0], cube(), 0.5, 16.0 / 9.0, 50.0),
            solve_shot(
                &shot(ShotSize::Wide, ShotAngle::Front, ShotMove::Hold),
                cube(),
                0.5,
                16.0 / 9.0,
                50.0
            )
        );
    }

    #[test]
    fn a_placed_camera_survives_a_round_trip_through_the_document() {
        let s = placed(ShotMove::Orbit, 0.6);
        let json = serde_json::to_string(&s).expect("serialise");
        assert!(
            json.contains("\"lookAt\""),
            "wire names stay camelCase: {json}"
        );
        let back: ShotRecipe = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back, s);
    }

    // ---------------------------------------------------------------------------------------------
    // ADR-195 — the head turns, the tripod does not move.
    // ---------------------------------------------------------------------------------------------

    /// The subject `cube()` describes, moved.
    fn cube_at(center: [f32; 3]) -> SubjectSample {
        SubjectSample { center, ..cube() }
    }

    #[test]
    fn switching_the_head_on_changes_nothing_while_the_subject_is_where_it_was() {
        // THE WHOLE PROMISE OF STORING AN OFFSET. If turning tracking on moved the frame, an author
        // would have to re-compose every shot they asked to follow — and would learn that only when
        // they played it back.
        let locked = placed(ShotMove::Hold, 0.35);
        let following = ShotRecipe {
            camera: Some(locked.camera.unwrap().following(cube().center)),
            ..locked.clone()
        };
        assert!(following.camera.unwrap().is_following());
        for motion in [
            ShotMove::Hold,
            ShotMove::PushIn,
            ShotMove::PullOut,
            ShotMove::Orbit,
            ShotMove::CraneUp,
            ShotMove::CraneDown,
        ] {
            for progress in [0.0, 0.25, 0.5, 0.75, 1.0] {
                let a = solve_shot(
                    &ShotRecipe {
                        motion,
                        ..locked.clone()
                    },
                    cube(),
                    progress,
                    16.0 / 9.0,
                    50.0,
                );
                let b = solve_shot(
                    &ShotRecipe {
                        motion,
                        ..following.clone()
                    },
                    cube(),
                    progress,
                    16.0 / 9.0,
                    50.0,
                );
                assert_eq!(a, b, "{motion:?} at {progress} moved when the head came on");
            }
        }
    }

    #[test]
    #[allow(
        clippy::float_cmp,
        reason = "\"the tripod did not move\" is a BIT-IDENTICAL claim; a tolerance here would pass \
                  an implementation that re-solved the eye and landed near it"
    )]
    fn a_following_head_re_aims_and_never_re_places() {
        let locked = placed(ShotMove::Hold, 0.35);
        let camera = locked.camera.unwrap();
        let following = ShotRecipe {
            camera: Some(camera.following(cube().center)),
            ..locked.clone()
        };
        let walked = [4.0, 0.0, -2.0];
        let pose = solve_shot(&following, cube_at(walked), 0.5, 16.0 / 9.0, 50.0);

        // The tripod is bolted down: bit-identical to the pose the author placed.
        assert_eq!(pose.eye, camera.eye, "the eye moved");
        assert_eq!(pose.fov_deg, camera.fov_deg, "the lens changed");
        // ...and the aim moved by exactly what the subject did, so the framing the author composed
        // — a hair above centre, or wherever they put it — is preserved rather than re-derived.
        for ((got, authored), moved) in pose.look_at.iter().zip(camera.look_at).zip(walked) {
            assert!(
                (got - (authored + moved)).abs() < 1.0e-4,
                "{:?} is not the authored aim carried by the subject",
                pose.look_at
            );
        }

        // THE NEGATIVE CONTROL: the same subject walk, locked off, films the authored frame.
        let held = solve_shot(&locked, cube_at(walked), 0.5, 16.0 / 9.0, 50.0);
        assert_eq!(held.eye, camera.eye);
        assert_eq!(
            held.look_at, camera.look_at,
            "a locked-off camera followed the subject"
        );
    }

    #[test]
    #[allow(
        clippy::float_cmp,
        reason = "the authored pose is preserved BIT-IDENTICALLY or it is not preserved"
    )]
    fn a_following_push_in_closes_on_where_the_subject_is_now() {
        // A move solved against the stale sight line would slide past a subject that has walked.
        let base = placed(ShotMove::PushIn, 1.0);
        let camera = base.camera.unwrap();
        let following = ShotRecipe {
            camera: Some(camera.following(cube().center)),
            ..base
        };
        let walked = [-6.0, 0.0, 0.0];
        let end = solve_shot(&following, cube_at(walked), 1.0, 16.0 / 9.0, 50.0);
        let live_aim = [
            camera.look_at[0] + walked[0],
            camera.look_at[1] + walked[1],
            camera.look_at[2] + walked[2],
        ];
        // A full-strength push-in keeps a fifth of the stand-off — measured to the LIVE aim.
        let started = dist(camera.eye, live_aim);
        assert!(
            (dist(end.eye, live_aim) - started * 0.2).abs() < 1.0e-3,
            "the push-in closed on the wrong point: {end:?}"
        );
        assert_eq!(end.look_at, live_aim);
    }

    #[test]
    #[allow(
        clippy::float_cmp,
        reason = "the authored pose is preserved BIT-IDENTICALLY or it is not preserved"
    )]
    fn a_subject_that_walks_into_the_lens_falls_back_to_the_authored_aim() {
        let base = placed(ShotMove::Hold, 0.0);
        let camera = base.camera.unwrap().following(cube().center);
        // Put the subject exactly where the offset lands the aim ON the eye.
        let onto_the_lens = [
            camera.eye[0] - camera.track.unwrap()[0],
            camera.eye[1] - camera.track.unwrap()[1],
            camera.eye[2] - camera.track.unwrap()[2],
        ];
        assert_eq!(camera.aim_at(onto_the_lens), camera.look_at);
        let pose = solve_shot(
            &ShotRecipe {
                camera: Some(camera),
                ..base
            },
            cube_at(onto_the_lens),
            0.5,
            16.0 / 9.0,
            50.0,
        );
        assert_eq!(pose.eye, camera.eye);
        assert_eq!(pose.look_at, camera.look_at);
    }

    #[test]
    #[allow(
        clippy::float_cmp,
        reason = "the authored pose is preserved BIT-IDENTICALLY or it is not preserved"
    )]
    fn a_head_can_be_switched_off_again_and_locking_off_is_the_inverse() {
        let camera = placed(ShotMove::Hold, 0.0).camera.unwrap();
        assert!(!camera.is_following());
        let following = camera.following([1.0, 2.0, 3.0]);
        assert!(following.is_following());
        assert_eq!(following.locked_off(), camera);
    }

    #[test]
    #[allow(
        clippy::float_cmp,
        reason = "the authored pose is preserved BIT-IDENTICALLY or it is not preserved"
    )]
    fn a_cutscene_authored_before_heads_existed_opens_locked_off() {
        // The one compatibility question: `track` must default, not fail, and must default to the
        // behaviour every placed camera already had.
        let json = r#"{"eye":[6.0,3.0,8.0],"lookAt":[0.0,0.5,0.0],"fovDeg":45.0}"#;
        let back: ShotCamera = serde_json::from_str(json).expect("deserialise");
        assert!(!back.is_following());
        assert_eq!(back.aim_at([100.0, 100.0, 100.0]), back.look_at);
    }

    #[test]
    fn a_following_head_survives_a_round_trip_through_the_document() {
        let s = ShotRecipe {
            camera: Some(
                placed(ShotMove::Orbit, 0.6)
                    .camera
                    .unwrap()
                    .following([1.0, 2.0, 3.0]),
            ),
            ..placed(ShotMove::Orbit, 0.6)
        };
        let json = serde_json::to_string(&s).expect("serialise");
        assert!(json.contains("\"track\""), "{json}");
        assert_eq!(
            serde_json::from_str::<ShotRecipe>(&json).expect("deserialise"),
            s
        );
    }

    // ---------------------------------------------------------------------------------------------
    // ADR-195 — the world, asked about a camera the engine has promised not to move.
    // ---------------------------------------------------------------------------------------------

    fn cut_of(shots: Vec<ShotRecipe>) -> Cutscene {
        Cutscene {
            version: 1,
            shots,
            mood: Mood::Normal,
            delivery: Delivery::Widescreen,
            render: RenderSettings::default(),
        }
    }

    /// A world that reports a solid ball of geometry centred on `blob`, as `SceneState::vantage`
    /// would: buried inside it, and blind to the subject from anywhere within twice its radius.
    fn world_with_a_blob_at(
        blob: [f32; 3],
        radius: f32,
    ) -> impl FnMut(usize, &CameraSample) -> Vantage {
        move |_, pose| {
            let d = dist(pose.eye, blob);
            Vantage {
                eye_inside: d < radius,
                clear: if d < radius * 2.0 { 0.1 } else { 1.0 },
                backing: 0.5,
                crowded: 0.0,
            }
        }
    }

    #[test]
    fn an_open_world_has_nothing_to_say_about_a_placed_camera() {
        // THE NEGATIVE CONTROL, first: every message below has to be caused by the world objecting,
        // not by the shot merely having a placed camera.
        let cut = cut_of(vec![placed(ShotMove::PushIn, 0.8)]);
        assert!(
            placed_camera_problems(&cut, |_, _| cube().center, |_, _| Vantage::OPEN).is_empty()
        );
    }

    #[test]
    fn a_camera_parked_in_a_wall_says_so_and_names_the_shot() {
        let cut = cut_of(vec![placed(ShotMove::Hold, 0.0)]);
        let eye = cut.shots[0].camera.unwrap().eye;
        let said =
            placed_camera_problems(&cut, |_, _| cube().center, world_with_a_blob_at(eye, 1.0));
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0]
                .message
                .starts_with("shot 1's camera is inside something"),
            "{said:?}"
        );
    }

    #[test]
    fn the_move_is_judged_and_not_only_the_frame_on_the_stage() {
        // A push-in that starts in clear air and ends inside the machine next door — the exact
        // failure the card path's `PATH_SAMPLES` exists for, on the one path that is never planned.
        let cut = cut_of(vec![placed(ShotMove::PushIn, 1.0)]);
        let shot = &cut.shots[0];
        let ends_at = solve_shot(shot, cube(), 1.0, 16.0 / 9.0, 50.0).eye;
        let said = placed_camera_problems(
            &cut,
            |_, _| cube().center,
            world_with_a_blob_at(ends_at, 0.5),
        );
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0]
                .message
                .contains("move takes its camera inside something"),
            "the message must send the author to the move, not to the tripod: {said:?}"
        );

        // ...and a HOLD at the same placement is silent, because it never goes there.
        let held = cut_of(vec![placed(ShotMove::Hold, 1.0)]);
        assert!(placed_camera_problems(
            &held,
            |_, _| cube().center,
            world_with_a_blob_at(ends_at, 0.5)
        )
        .is_empty());
    }

    #[test]
    fn a_blocked_view_is_reported_with_how_much_of_the_subject_is_gone() {
        let cut = cut_of(vec![placed(ShotMove::Hold, 0.0)]);
        let said = placed_camera_problems(
            &cut,
            |_, _| cube().center,
            |_, _| Vantage {
                clear: 0.25,
                ..Vantage::OPEN
            },
        );
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(said[0].message.contains("75%"), "{said:?}");
        // At the threshold itself the shot is acceptable and nothing is said.
        assert!(placed_camera_problems(
            &cut,
            |_, _| cube().center,
            |_, _| Vantage {
                clear: MIN_CLEAR_FRACTION,
                ..Vantage::OPEN
            }
        )
        .is_empty());
    }

    #[test]
    fn a_boxed_in_camera_is_reported_once_and_never_twice_for_one_shot() {
        let cut = cut_of(vec![placed(ShotMove::Hold, 0.0)]);
        let said = placed_camera_problems(
            &cut,
            |_, _| cube().center,
            |_, _| Vantage {
                eye_inside: true,
                clear: 0.0,
                backing: 0.0,
                crowded: 0.9,
            },
        );
        assert_eq!(
            said.len(),
            1,
            "three faults on one camera are one sentence: {said:?}"
        );
        assert!(said[0].message.contains("inside something"), "{said:?}");
        let crowded = placed_camera_problems(
            &cut,
            |_, _| cube().center,
            |_, _| Vantage {
                crowded: 0.9,
                ..Vantage::OPEN
            },
        );
        assert!(crowded[0].message.contains("boxed in"), "{crowded:?}");
    }

    #[test]
    fn a_card_shot_is_never_reported_because_the_planner_already_moved_it() {
        // The planner walks a ladder until the world stops objecting. Repeating its findings here
        // would be advice about `size` and `angle` values the author did not choose.
        let cut = cut_of(vec![shot(
            ShotSize::Close,
            ShotAngle::Front,
            ShotMove::PushIn,
        )]);
        assert!(placed_camera_problems(
            &cut,
            |_, _| cube().center,
            |_, _| Vantage {
                eye_inside: true,
                clear: 0.0,
                backing: 0.0,
                crowded: 1.0
            }
        )
        .is_empty());
    }

    #[test]
    fn the_world_is_asked_about_the_pose_a_following_head_actually_films() {
        // The diagnostic has to measure the SAME camera the runtime films, or a shot that follows
        // its subject into the open would keep being reported as buried where it used to point.
        let base = placed(ShotMove::PushIn, 1.0);
        let camera = base.camera.unwrap().following(cube().center);
        let cut = cut_of(vec![ShotRecipe {
            camera: Some(camera),
            ..base
        }]);
        let walked = [-20.0, 0.0, 0.0];
        // A blob where the LOCKED-OFF push-in would have ended. With the head following a subject
        // that walked away, the move now ends somewhere else entirely.
        let stale_end = solve_shot(
            &cut_of(vec![ShotRecipe {
                camera: Some(camera.locked_off()),
                ..cut.shots[0].clone()
            }])
            .shots[0],
            cube(),
            1.0,
            16.0 / 9.0,
            50.0,
        )
        .eye;
        assert!(placed_camera_problems(
            &cut,
            |_, _| cube_at(walked).center,
            world_with_a_blob_at(stale_end, 0.5)
        )
        .is_empty());
        // ...and it IS reported where the following move really goes.
        let live_end = solve_shot(&cut.shots[0], cube_at(walked), 1.0, 16.0 / 9.0, 50.0).eye;
        assert!(!placed_camera_problems(
            &cut,
            |_, _| cube_at(walked).center,
            world_with_a_blob_at(live_end, 0.5)
        )
        .is_empty());
    }

    #[test]
    fn every_placed_shot_in_a_cutscene_is_asked_about_and_numbered() {
        let cut = cut_of(vec![
            shot(ShotSize::Wide, ShotAngle::Front, ShotMove::Hold),
            placed(ShotMove::Hold, 0.0),
            placed(ShotMove::Hold, 0.0),
        ]);
        let said = placed_camera_problems(
            &cut,
            |_, _| cube().center,
            |_, _| Vantage {
                eye_inside: true,
                ..Vantage::OPEN
            },
        );
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(said[0].message.starts_with("shot 2's"), "{said:?}");
        assert!(said[1].message.starts_with("shot 3's"), "{said:?}");
    }

    // ---------------------------------------------------------------------------------------------
    // ADR-197 — the negotiation's own verdict, said out loud.
    // ---------------------------------------------------------------------------------------------

    /// A world that answers the same thing everywhere, so the ladder cannot escape it. `plan_shot`
    /// then walks all fifty-four rungs and comes back with the least bad — which is the whole
    /// situation this vocabulary exists to describe.
    fn a_world_that_is_always(vantage: Vantage) -> impl FnMut(&CameraSample, f32) -> Vantage {
        move |_: &CameraSample, _: f32| vantage
    }

    /// The plan for each shot of a cut, negotiated against `world` — what the shell hands
    /// [`card_shot_problems`], computed the way the runtime computes it.
    fn plans_of(
        cut: &Cutscene,
        mut world: impl FnMut(&CameraSample, f32) -> Vantage,
    ) -> Vec<Option<ShotAdjustment>> {
        cut.shots
            .iter()
            .map(|shot| Some(plan_shot(shot, cube(), 16.0 / 9.0, 50.0, &mut world)))
            .collect()
    }

    /// Everywhere is inside a machine — the world that leaves the ladder nothing to find.
    const NOWHERE_TO_STAND: Vantage = Vantage {
        eye_inside: true,
        clear: 0.0,
        backing: 0.0,
        crowded: 1.0,
    };

    #[test]
    fn an_open_world_has_nothing_to_say_about_a_negotiated_placement() {
        // THE NEGATIVE CONTROL. Every message below must be caused by the world objecting to every
        // placement on the ladder, not by a card shot merely existing.
        let cut = cut_of(vec![shot(
            ShotSize::Close,
            ShotAngle::Front,
            ShotMove::Hold,
        )]);
        let plans = plans_of(&cut, |_, _| Vantage::OPEN);
        assert!(card_shot_problems(&cut, |i, _| plans[i]).is_empty());
    }

    #[test]
    fn the_identity_adjustment_carries_no_verdict_and_so_reports_nothing() {
        // A caller that never measured must be indistinguishable from one that measured and found
        // nothing wrong. This is why `Vantage`'s `Default` is written by hand: the DERIVED one is
        // `clear: 0.0`, which would make every unmeasured shot shout.
        assert_eq!(Vantage::default(), Vantage::OPEN);
        let cut = cut_of(vec![shot(
            ShotSize::Close,
            ShotAngle::Front,
            ShotMove::Hold,
        )]);
        let authored = ShotAdjustment::authored(&cut.shots[0]);
        assert!(authored.settled.acceptable());
        assert!(card_shot_problems(&cut, |_, _| Some(authored)).is_empty());
        // And an unanswered shot is silent rather than assumed good OR assumed bad.
        assert!(card_shot_problems(&cut, |_, _| None).is_empty());
    }

    #[test]
    fn the_plan_carries_the_verdict_of_the_placement_it_actually_settled_on() {
        // The field is only worth anything if it describes THE FILMED PLACEMENT. Checked by re-asking
        // the same world about the pose `solve_shot_adjusted` produces for the plan that came back.
        let s = shot(ShotSize::Close, ShotAngle::Front, ShotMove::PushIn);
        let hostile = Vantage {
            eye_inside: false,
            clear: 0.15,
            backing: 0.4,
            crowded: 0.0,
        };
        let plan = plan_shot(
            &s,
            cube(),
            16.0 / 9.0,
            50.0,
            a_world_that_is_always(hostile),
        );
        assert!(!plan.settled.acceptable(), "{plan:?}");
        assert_eq!(plan.settled, hostile, "{plan:?}");
        let filmed = solve_shot_adjusted(&s, plan, cube(), 0.5, 16.0 / 9.0, 50.0);
        let mut world = a_world_that_is_always(hostile);
        assert_eq!(world(&filmed, 0.5), plan.settled);
    }

    #[test]
    fn the_verdict_describes_the_worst_moment_of_the_move_not_the_first() {
        // A push-in that starts clear and ends buried is a buried shot. `worst_score` already decided
        // the ranking on that moment; the sentence has to be about the same one.
        let s = shot(ShotSize::Medium, ShotAngle::Front, ShotMove::PushIn);
        let plan = plan_shot(&s, cube(), 16.0 / 9.0, 50.0, |_, progress| {
            if progress >= 1.0 {
                NOWHERE_TO_STAND
            } else {
                Vantage::OPEN
            }
        });
        assert!(plan.settled.eye_inside, "{plan:?}");
    }

    #[test]
    fn a_shot_the_planner_cannot_place_anywhere_says_so_and_names_the_shot() {
        let cut = cut_of(vec![shot(
            ShotSize::Close,
            ShotAngle::Front,
            ShotMove::Hold,
        )]);
        let plans = plans_of(&cut, a_world_that_is_always(NOWHERE_TO_STAND));
        let said = card_shot_problems(&cut, |i, _| plans[i]);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0]
                .message
                .starts_with("shot 1 has nowhere good to film from"),
            "{said:?}"
        );
        assert!(
            said[0].message.contains("the camera is inside something"),
            "{said:?}"
        );
        // The advice must be the one that is LEFT. Changing the size or the angle is what the engine
        // has already done, on every rung of the ladder, on the author's behalf.
        assert!(said[0].message.contains("Shoot from this view"), "{said:?}");
        assert!(!said[0].message.contains("change the size"), "{said:?}");
    }

    #[test]
    fn a_subject_hidden_from_every_angle_reports_how_much_of_it_is_behind_something() {
        let cut = cut_of(vec![shot(
            ShotSize::Medium,
            ShotAngle::Front,
            ShotMove::Hold,
        )]);
        let plans = plans_of(
            &cut,
            a_world_that_is_always(Vantage {
                eye_inside: false,
                clear: 0.25,
                backing: 0.5,
                crowded: 0.0,
            }),
        );
        let said = card_shot_problems(&cut, |i, _| plans[i]);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0]
                .message
                .contains("75% of the subject is behind something else"),
            "{said:?}"
        );
    }

    #[test]
    fn a_placement_that_is_boxed_in_everywhere_says_that_instead() {
        let cut = cut_of(vec![shot(
            ShotSize::Close,
            ShotAngle::Profile,
            ShotMove::Hold,
        )]);
        let plans = plans_of(
            &cut,
            a_world_that_is_always(Vantage {
                eye_inside: false,
                clear: 1.0,
                backing: 0.5,
                crowded: 0.9,
            }),
        );
        let said = card_shot_problems(&cut, |i, _| plans[i]);
        assert_eq!(said.len(), 1, "{said:?}");
        assert!(
            said[0]
                .message
                .contains("most of the frame is whatever the camera is standing against"),
            "{said:?}"
        );
    }

    #[test]
    fn a_negotiation_that_succeeded_is_silent_however_far_it_had_to_go() {
        // THE DISTINCTION THE WHOLE FEATURE RESTS ON. The ladder running is not the fault; the ladder
        // running OUT is. A shot rescued by a detour is a shot the engine handled, and warning about
        // it would be advice about controls whose values the author did not choose.
        let s = shot(ShotSize::Close, ShotAngle::Front, ShotMove::Hold);
        let authored = solve_shot_eased(&s, cube(), 0.0, 16.0 / 9.0, 50.0);
        let cut = cut_of(vec![s.clone()]);
        let plans = plans_of(&cut, blob_world(authored.eye, 1.2));
        let plan = plans[0].expect("planned");
        assert!(
            !plan.is_authored(&s),
            "the fixture must force a detour: {plan:?}"
        );
        assert!(plan.settled.acceptable(), "{plan:?}");
        assert!(card_shot_problems(&cut, |i, _| plans[i]).is_empty());
    }

    #[test]
    fn a_placed_camera_is_never_reported_by_this_half() {
        // Both halves run over the same shot list, so a shot that could be described twice would be.
        // `plan_shot` returns a placed camera's identity adjustment untouched — an OPEN verdict about
        // a pose it never judged — and reporting that as a clean bill of health would be worse than
        // saying nothing at all.
        let cut = cut_of(vec![placed(ShotMove::Hold, 0.0)]);
        let plans = plans_of(&cut, a_world_that_is_always(NOWHERE_TO_STAND));
        assert!(card_shot_problems(&cut, |i, _| plans[i]).is_empty());
    }

    #[test]
    fn every_card_shot_in_a_cutscene_is_asked_about_and_numbered() {
        let cut = cut_of(vec![
            placed(ShotMove::Hold, 0.0),
            shot(ShotSize::Close, ShotAngle::Front, ShotMove::Hold),
            shot(ShotSize::Medium, ShotAngle::Profile, ShotMove::Hold),
        ]);
        let plans = plans_of(&cut, a_world_that_is_always(NOWHERE_TO_STAND));
        let said = card_shot_problems(&cut, |i, _| plans[i]);
        assert_eq!(said.len(), 2, "{said:?}");
        assert!(
            said[0].message.starts_with("shot 2 has nowhere good"),
            "{said:?}"
        );
        assert!(
            said[1].message.starts_with("shot 3 has nowhere good"),
            "{said:?}"
        );
    }

    #[test]
    fn a_plan_survives_a_serde_round_trip_and_an_older_one_reads_as_unmeasured() {
        let plan = ShotAdjustment {
            size: ShotSize::Wide,
            yaw_offset_deg: 30.0,
            steps: 4,
            settled: Vantage {
                eye_inside: false,
                clear: 0.25,
                backing: 0.5,
                crowded: 0.1,
            },
        };
        let json = serde_json::to_string(&plan).expect("serialise");
        assert!(json.contains("\"settled\""), "{json}");
        assert_eq!(
            serde_json::from_str::<ShotAdjustment>(&json).expect("deserialise"),
            plan
        );
        // A record written before the field existed must read as "nobody measured", not as "totally
        // blocked" — the reason `Vantage: Default` is written by hand.
        let older: ShotAdjustment =
            serde_json::from_str(r#"{"size":"wide","yawOffsetDeg":0.0,"steps":0}"#)
                .expect("deserialise an older plan");
        assert_eq!(older.settled, Vantage::OPEN);
    }

    // ── The jump-cut warning names shots, and names its subject only when a name exists ──────────

    #[test]
    fn the_jump_cut_warning_names_the_two_shots_and_never_leaks_an_entity_key() {
        let mut a = shot(ShotSize::Wide, ShotAngle::Front, ShotMove::Hold);
        a.subject = "412_3".into();
        let mut b = a.clone();
        b.id = "shot-2".into();
        let cut = cut_of(vec![
            shot(ShotSize::Wide, ShotAngle::Front, ShotMove::Hold),
            a,
            b,
        ]);
        let bare = cut.problems();
        let jump = bare
            .iter()
            .find(|p| p.message.contains("jump cut"))
            .expect("a jump cut");
        assert!(
            jump.message
                .starts_with("shots 2 and 3 are framed identically"),
            "{jump:?}"
        );
        assert!(
            !bare.iter().any(|p| p.message.contains("412_3")),
            "an entity key reached the author: {bare:?}"
        );
        let named = cut.problems_named(&|key| (key == "412_3").then(|| "Weld Gun 7".to_string()));
        assert!(
            named.iter().any(|p| p
                .message
                .starts_with("shots 2 and 3 on \"Weld Gun 7\" are framed identically")),
            "{named:?}"
        );
    }

    // ---------------------------------------------------------------------------------------------
    // ADR-200 — a warning knows which shot it is about, and where that shot films from.
    // ---------------------------------------------------------------------------------------------

    #[test]
    fn every_continuity_warning_names_the_shot_a_control_would_act_on() {
        // One cut carrying faults at two different shots, so the numbers cannot both be right by
        // accident.
        let mut rushed = shot(ShotSize::Wide, ShotAngle::Front, ShotMove::Hold);
        rushed.seconds = 0.1;
        let cut = cut_of(vec![
            shot(ShotSize::Close, ShotAngle::Front, ShotMove::Hold),
            shot(ShotSize::Medium, ShotAngle::Profile, ShotMove::Hold),
            rushed,
        ]);
        let said = cut.problems();
        let opens = said
            .iter()
            .find(|p| p.message.contains("opens tight"))
            .expect("opens tight");
        assert_eq!(opens.shot, Some(0), "{said:?}");
        let short = said
            .iter()
            .find(|p| p.message.contains("shorter than"))
            .expect("too short");
        assert_eq!(
            short.shot,
            Some(2),
            "the number in the sentence and the number in the field have to agree: {said:?}"
        );
        assert!(short.message.starts_with("shot 3 "), "{said:?}");
    }

    #[test]
    fn a_jump_cut_is_filed_under_the_shot_whose_framing_is_the_fix() {
        let cut = cut_of(vec![
            shot(ShotSize::Wide, ShotAngle::Front, ShotMove::Hold),
            shot(ShotSize::Medium, ShotAngle::Profile, ShotMove::Hold),
            shot(ShotSize::Medium, ShotAngle::Profile, ShotMove::Hold),
        ]);
        let said = cut.problems();
        let jump = said
            .iter()
            .find(|p| p.message.contains("jump cut"))
            .expect("a jump cut");
        // The sentence names both shots; the field names the SECOND, which is the one an author
        // re-frames. Filing it under the first would point every control at a shot that is fine.
        assert!(jump.message.starts_with("shots 2 and 3"), "{said:?}");
        assert_eq!(jump.shot, Some(2), "{said:?}");
    }

    #[test]
    fn the_over_count_belongs_to_no_shot() {
        let cut = cut_of(
            (0..=MAX_SHOTS)
                .map(|_| shot(ShotSize::Wide, ShotAngle::Front, ShotMove::Hold))
                .collect(),
        );
        let said = cut.problems();
        let over = said
            .iter()
            .find(|p| p.message.contains("at most"))
            .expect("the over-count");
        assert_eq!(over.shot, None, "{said:?}");
    }

    #[test]
    fn a_cut_with_faults_of_two_kinds_reads_in_shot_order() {
        // THE DEFECT: the producers are appended, so a placed-camera fault at shot 3 printed above a
        // continuity fault at shot 1 — a list ordered by which function noticed it.
        let cut = cut_of(vec![
            shot(ShotSize::Close, ShotAngle::Front, ShotMove::Hold),
            shot(ShotSize::Wide, ShotAngle::Front, ShotMove::Hold),
            placed(ShotMove::Hold, 0.0),
        ]);
        let mut said = cut.problems();
        said.extend(placed_camera_problems(
            &cut,
            |_, _| cube().center,
            |_, _| Vantage {
                eye_inside: true,
                ..Vantage::OPEN
            },
        ));
        assert_eq!(
            said.iter().map(|p| p.shot).collect::<Vec<_>>(),
            vec![Some(0), Some(2)],
            "the fixture has to carry one fault of each kind for the merge to mean anything"
        );
        let merged = in_shot_order(said);
        assert!(
            merged.windows(2).all(|w| w[0].shot <= w[1].shot),
            "{merged:?}"
        );
    }

    #[test]
    fn in_shot_order_leads_with_the_cut_and_keeps_two_faults_of_one_shot_in_producer_order() {
        let merged = in_shot_order(vec![
            ShotProblem::about(2, "second of shot 3"),
            ShotProblem::about(0, "shot 1"),
            ShotProblem::about_the_cut("the whole cut"),
            ShotProblem::about(2, "third of shot 3"),
        ]);
        assert_eq!(
            merged.iter().map(|p| p.message.as_str()).collect::<Vec<_>>(),
            vec![
                "the whole cut",
                "shot 1",
                "second of shot 3",
                "third of shot 3"
            ],
            "cut-wide leads, and two faults of one shot keep the order their producer ranked them in"
        );
    }

    #[test]
    fn worst_moment_agrees_with_the_search_about_which_frame_produced_the_verdict() {
        // A push-in whose END is inside a machine. `plan_shot` keeps the verdict from the worst
        // sample; this has to hand back the sample that produced it, or the author is taken to a
        // frame that looks fine and told it is the problem.
        //
        // The world objects to the LAST instant of any move and to nothing else, so no rung of the
        // ladder can escape it: a fixture keyed on the authored pose's end point is escaped by the
        // first yaw detour, and the search then honestly reports a clean placement.
        let card = shot(ShotSize::Medium, ShotAngle::Front, ShotMove::PushIn);
        let mut world = |_: &CameraSample, progress: f32| {
            if progress > 0.9 {
                NOWHERE_TO_STAND
            } else {
                Vantage::OPEN
            }
        };
        let plan = plan_shot(&card, cube(), 16.0 / 9.0, 50.0, &mut world);
        let (progress, pose, vantage) =
            worst_moment(&card, plan, cube(), 16.0 / 9.0, 50.0, &mut world);
        assert_eq!(
            vantage, plan.settled,
            "the verdict at the moment returned must be the verdict the plan carries"
        );
        assert!(
            (progress - 1.0).abs() < 1.0e-6,
            "the fault is at the end of the move, not at its opening: {progress}"
        );
        assert!(
            dist(
                pose.eye,
                solve_shot_adjusted(&card, plan, cube(), progress, 16.0 / 9.0, 50.0).eye
            ) < 1.0e-4,
            "the pose has to be the one the runtime films at that instant"
        );
    }

    #[test]
    fn worst_moment_of_a_still_shot_is_its_one_pose() {
        let card = shot(ShotSize::Medium, ShotAngle::Front, ShotMove::Hold);
        let plan = ShotAdjustment::authored(&card);
        let (progress, pose, _) = worst_moment(
            &card,
            plan,
            cube(),
            16.0 / 9.0,
            50.0,
            a_world_that_is_always(Vantage::OPEN),
        );
        assert!(progress.abs() < 1.0e-6, "ties go to the earliest sample");
        assert!(dist(pose.eye, solve_shot(&card, cube(), 0.0, 16.0 / 9.0, 50.0).eye) < 1.0e-4);
    }

    #[test]
    fn worst_moment_of_a_placed_camera_is_the_pose_the_author_put_there() {
        // The placed half of the vocabulary points at this too, and a planner correction must not
        // reach it: standing at the author's own eye IS the solid-colour frame they are warned about.
        let cut = cut_of(vec![placed(ShotMove::Hold, 0.0)]);
        let card = &cut.shots[0];
        let camera = card.camera.expect("placed");
        let (_, pose, _) = worst_moment(
            card,
            ShotAdjustment {
                size: ShotSize::ExtremeWide,
                yaw_offset_deg: 90.0,
                steps: 12,
                settled: NOWHERE_TO_STAND,
            },
            cube(),
            16.0 / 9.0,
            50.0,
            a_world_that_is_always(Vantage::OPEN),
        );
        assert!(
            dist(pose.eye, camera.eye) < 1.0e-4,
            "an adjustment must not move a placed camera"
        );
    }

    #[test]
    fn the_advice_for_a_shot_with_nowhere_to_stand_names_both_controls() {
        let cut = cut_of(vec![shot(
            ShotSize::Close,
            ShotAngle::Front,
            ShotMove::Hold,
        )]);
        let plans = plans_of(&cut, a_world_that_is_always(NOWHERE_TO_STAND));
        let said = card_shot_problems(&cut, |i, _| plans[i]);
        assert_eq!(said.len(), 1, "{said:?}");
        assert_eq!(said[0].shot, Some(0));
        assert!(
            said[0].message.contains("\"Take me there\""),
            "the advice must name the control that goes to the placement it just described: {said:?}"
        );
        assert!(
            said[0].message.contains("\"Shoot from this view\""),
            "{said:?}"
        );
    }
}
