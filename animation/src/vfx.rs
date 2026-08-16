//! **Visual effects** — a particle is a PURE FUNCTION of `(recipe, index, time)`, never accumulated state.
//!
//! Every engine on the market simulates particles by stepping stored state: position += velocity * dt,
//! frame after frame. That is the obvious design and it is the wrong one *here*, for three reasons this
//! engine already committed to:
//!
//! * **Deterministic replay.** A stepped simulation drifts — float accumulation over 10,000 frames is
//!   not reproducible across machines, and a recorded input replay would diverge visually. A closed-form
//!   particle is bit-identical on every machine at every frame, forever.
//! * **Scrubbing.** The timeline can jump to t = 3.2 s. A stepped sim must re-run 192 frames to answer;
//!   a closed-form one evaluates 3.2 directly. Cutscenes and the animation scrubber both need this.
//! * **No state to own.** Nothing to allocate, nothing to reset, nothing to leak between Play sessions,
//!   and nothing that can survive a Stop and corrupt the next run.
//!
//! The cost is stated plainly rather than hidden: closed-form particles cannot collide with the world or
//! influence each other. Those are the two things this module will never do. Everything else — gravity,
//! drag, turbulence, orbit, colour and size over life, bursts, continuous emission — is expressible as a
//! function of age, and is here.
//!
//! Randomness is *hashed*, not drawn: particle `i` gets its jitter from a hash of `i`, so there is no RNG
//! state, no seed to thread, and two machines agree by construction.
//!
//! The user never sees any of this. They see cards — 🔥 Fire, 💥 Burst, ✨ Sparkle — because an author
//! wants to say WHAT HAPPENED, not to wire a node graph.

use serde::{Deserialize, Serialize};

/// The most particles ONE effect may ask for. A stated ceiling, refused with a reason rather than
/// silently truncated — and low enough that a scene full of effects still costs one draw call each.
pub const MAX_PARTICLES: u32 = 512;
/// The most effects one object may carry.
pub const MAX_LAYERS: usize = 4;
/// Longest a single particle may live, seconds.
pub const MAX_LIFETIME: f32 = 30.0;
/// The brightest a single particle may be, in linear HDR. Well above the catalogue's hottest card
/// (a 9.0 explosion core) and well below `f16` saturation at 65504.
pub const MAX_RADIANCE: f32 = 512.0;

/// How the particles are laid out when they are born — the "shape of the effect" in one word.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmitShape {
    /// All from one spot.
    #[default]
    Point,
    /// Filling the object's volume — fire that wraps a whole crate.
    Volume,
    /// From the object's surface outward.
    Shell,
    /// A flat disc at the base — smoke pooling, a magic circle.
    Disc,
    /// A cone upward — a fountain, a jet.
    Cone,
    /// Straight up in a thin column.
    Column,
}

/// What the particles do once alive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Motion {
    /// Outward from the origin at birth speed, slowed by drag.
    #[default]
    Radiate,
    /// Buoyant — accelerates upward, like heat.
    Rise,
    /// Ballistic — thrown out, pulled down.
    Fall,
    /// Circling the origin.
    Orbit,
    /// Left behind as the object moves; barely moves on its own.
    Trail,
    /// Outward along the GROUND PLANE only — a flat ring, never a sphere.
    ///
    /// `Radiate` picks a direction on the whole sphere, so a "shockwave racing along the ground"
    /// authored with it climbed almost a full unit while it spread. The card promised flat; this is
    /// what flat needs.
    Ripple,
}

/// How a particle is drawn. The renderer maps this to a blend mode: `Additive` for anything that
/// EMITS light (it is drawn into the linear-HDR scene, so bloom picks it up for free), `Soft` for
/// anything that BLOCKS light, like smoke.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Look {
    /// Glowing — fire, sparks, magic. Additive; blooms.
    #[default]
    Additive,
    /// Occluding — smoke, dust, steam. Alpha-blended.
    Soft,
}

/// When a layer runs. This is per-LAYER and not per-object on purpose: one object commonly carries a
/// flame that burns the whole time AND an explosion that fires once when it is hit. Hanging a single
/// `playing` boolean off the object made those two mutually exclusive — adding the explosion silently
/// put the fire out, and adding the fire made the explosion detonate at Play start with nothing having
/// touched the object.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FxTrigger {
    /// Runs for the whole of Play.
    #[default]
    Always,
    /// Fires once, at the moment this object is collected or otherwise leaves the game.
    WhenCollected,
    /// Fires once, at the moment this object takes damage.
    WhenHit,
    /// Runs while a rule holds `Vfx.playing` true.
    OnCue,
}

impl FxTrigger {
    /// True when this layer fires at a MOMENT rather than running.
    #[must_use]
    pub fn is_moment(self) -> bool {
        matches!(self, Self::WhenCollected | Self::WhenHit)
    }

    /// The author-facing name.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Always => "the whole time",
            Self::WhenCollected => "when it's picked up",
            Self::WhenHit => "when it's hit",
            Self::OnCue => "when a rule says so",
        }
    }

    /// Parse from the UI's string, defaulting rather than failing — an unknown trigger is a UI bug, not
    /// a reason to refuse the author's click.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "whenCollected" => Self::WhenCollected,
            "whenHit" => Self::WhenHit,
            "onCue" => Self::OnCue,
            _ => Self::Always,
        }
    }
}

/// One effect layer. A "🔥 Fire" card is exactly one of these with the fields already chosen — which is
/// the whole point: the author picks a word, not twenty numbers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectRecipe {
    /// Stable id (never renumbered on reorder).
    pub id: String,
    /// The card this came from — carried so the UI can name it back in the author's own words.
    pub kind: String,
    /// How many particles are alive at once.
    pub count: u32,
    /// Seconds each particle lives.
    pub lifetime: f32,
    /// Where they are born.
    #[serde(default)]
    pub shape: EmitShape,
    /// What they do.
    #[serde(default)]
    pub motion: Motion,
    /// How they are drawn.
    #[serde(default)]
    pub look: Look,
    /// Birth speed, world units per second.
    pub speed: f32,
    /// Downward pull (negative = buoyant).
    pub gravity: f32,
    /// How quickly motion bleeds off, 0..1 per second.
    pub drag: f32,
    /// Wobble amplitude — the difference between a clean jet and living fire.
    pub turbulence: f32,
    /// Particle radius at birth, world units.
    pub size: f32,
    /// Radius multiplier at death (a puff of smoke grows, a spark shrinks).
    pub size_end: f32,
    /// Linear HDR colour at birth. Values ABOVE 1.0 are intentional — that is what makes a spark bloom.
    pub color: [f32; 3],
    /// Linear HDR colour at death.
    pub color_end: [f32; 3],
    /// Opacity at birth, 0..1.
    pub alpha: f32,
    /// One-shot (a burst) rather than a continuous stream.
    #[serde(default)]
    pub burst: bool,
    /// WHEN this layer runs. Per-layer; see [`FxTrigger`].
    #[serde(default)]
    pub trigger: FxTrigger,
    /// How far from the object's centre the emitter sits (fraction of its size).
    #[serde(default)]
    pub spread: f32,
}

impl Default for EffectRecipe {
    fn default() -> Self {
        Self {
            id: String::new(),
            kind: "sparkle".into(),
            count: 64,
            lifetime: 1.2,
            shape: EmitShape::Point,
            motion: Motion::Radiate,
            look: Look::Additive,
            speed: 2.0,
            gravity: 0.0,
            drag: 0.5,
            turbulence: 0.3,
            size: 0.08,
            size_end: 0.5,
            color: [2.0, 1.4, 0.6],
            color_end: [0.8, 0.2, 0.05],
            alpha: 1.0,
            burst: false,
            trigger: FxTrigger::Always,
            spread: 0.5,
        }
    }
}

/// The most particles the WHOLE SCENE may draw in one frame, across every object and every burst.
/// [`MAX_PARTICLES`] bounds one stack; without this, fifty objects each carrying a legal four-card
/// stack resolve ~17,000 particles on the engine thread inside the 60 Hz tick and re-upload a megabyte
/// every frame. The cap is reported rather than silently applied — see [`SceneBudget`].
pub const MAX_SCENE_PARTICLES: usize = 6_000;

/// Tracks the scene-wide particle spend across every `resolve_stack` call in one frame, so the cap is a
/// property of the FRAME rather than of whichever object happened to be resolved first.
#[derive(Debug, Default, Clone, Copy)]
pub struct SceneBudget {
    spent: usize,
    dropped: usize,
}

impl SceneBudget {
    /// A fresh frame.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// Particles resolved so far this frame.
    #[must_use]
    pub fn spent(&self) -> usize {
        self.spent
    }
    /// Particles the cap refused this frame. Non-zero means the scene is over budget and something
    /// visible was dropped — a caller that reports 0 is telling the truth about a complete frame.
    #[must_use]
    pub fn dropped(&self) -> usize {
        self.dropped
    }
    fn take(&mut self, want: usize) -> usize {
        let room = MAX_SCENE_PARTICLES.saturating_sub(self.spent);
        let granted = want.min(room);
        self.spent += granted;
        self.dropped += want - granted;
        granted
    }
}

/// Everything one object emits: an ordered stack of layers (fire AND smoke AND embers is three layers,
/// which is how real effects are built) plus the one global dial.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectStack {
    /// Schema version, so a future reader can migrate rather than guess.
    #[serde(default = "one")]
    pub version: u32,
    /// The layers, in draw order.
    #[serde(default)]
    pub layers: Vec<EffectRecipe>,
}

fn one() -> u32 {
    1
}

// See the note on `Cutscene::default` — a derived `Default` would stamp every authored effect stack
// with version 0, because `#[serde(default)]` only fills a field MISSING from incoming JSON.
impl Default for EffectStack {
    fn default() -> Self {
        Self {
            version: one(),
            layers: Vec::new(),
        }
    }
}

impl EffectRecipe {
    /// The count actually used, clamped into `1..=MAX_PARTICLES`.
    ///
    /// Every arithmetic path downstream assumed the stored count was already in range. It is not:
    /// `source` is an ordinary string field an author (or a plugin, or a newer build) can write, and a
    /// count of 100,000 made the stagger denominator overflow a `u16` and collapse to 1 — particle *i*
    /// was then born *i* SECONDS after Play started, so a "100,000-particle fire" rendered as one dot.
    /// Clamping here means the assumption is enforced rather than asserted in a comment.
    #[must_use]
    pub fn live_count(&self) -> u32 {
        self.count.clamp(1, MAX_PARTICLES)
    }

    /// The moment this layer's last particle is finally gone, in seconds from its start. A burst's last
    /// particle is BORN at `0.15·life`, so it dies at `1.15·life` — the retirement test used to say
    /// `life + 0.4`, which cut a long burst off while 15% of it was still at full opacity.
    #[must_use]
    pub fn total_seconds(&self) -> f32 {
        self.lifetime.clamp(0.05, MAX_LIFETIME) * 1.15
    }
}

impl EffectStack {
    /// Total particles this stack asks the renderer for.
    #[must_use]
    pub fn budget(&self) -> u32 {
        self.layers.iter().map(EffectRecipe::live_count).sum()
    }

    /// Things worth telling the author, in their language — never blocking, always specific.
    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let budget = self.budget();
        if budget > MAX_PARTICLES {
            out.push(format!(
                "that's {budget} particles at once — over the {MAX_PARTICLES} budget, so the newest layers will be thinned"
            ));
        }
        if self.layers.len() > 1 && self.layers.iter().all(|l| matches!(l.look, Look::Additive)) {
            out.push(
                "every layer glows — adding a soft layer (smoke, dust) gives the glow something to read against"
                    .into(),
            );
        }
        if self.layers.iter().any(|l| l.burst) && self.layers.iter().all(|l| l.burst) {
            out.push("all of these are one-shot bursts — they will fire once and be gone".into());
        }
        out
    }
}

/// One particle, resolved. The renderer turns this into a camera-facing quad; nothing here knows about
/// wgpu, buffers, or the scene.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParticleSample {
    /// World position.
    pub position: [f32; 3],
    /// World radius.
    pub radius: f32,
    /// Linear HDR colour, already faded.
    pub color: [f32; 3],
    /// Opacity, already faded — 0.0 means "do not draw".
    pub alpha: f32,
}

/// Where the effect is anchored and how big the thing carrying it is.
#[derive(Clone, Copy, Debug)]
pub struct EmitterSample {
    /// World-space origin (the object's centre).
    pub origin: [f32; 3],
    /// The object's radius — the effect scales itself to what it is attached to, so 🔥 on a matchstick
    /// and 🔥 on a barn are both right without the author touching a number.
    pub radius: f32,
}

/// A hashed pseudo-random in `0.0..1.0` — deterministic, stateless, and identical on every machine.
/// FNV-1a, the same "identity from content" discipline the animation tracks and shot ids use.
#[must_use]
fn hashed(index: u32, salt: u32) -> f32 {
    let mut h: u32 = 0x811c_9dc5;
    for byte in index.to_le_bytes().into_iter().chain(salt.to_le_bytes()) {
        h ^= u32::from(byte);
        h = h.wrapping_mul(0x0100_0193);
    }
    // Take the top 24 bits: the low bits of FNV are the weakest.
    #[allow(clippy::cast_precision_loss)] // 24 bits is exactly representable in f32
    {
        ((h >> 8) as f32) / 16_777_216.0
    }
}

/// A hashed value in `-1.0..1.0`.
fn hashed_signed(index: u32, salt: u32) -> f32 {
    hashed(index, salt).mul_add(2.0, -1.0)
}

/// A cheap deterministic wobble — two out-of-phase sines, which reads as turbulence without a noise
/// texture or a table.
fn wobble(seed: f32, t: f32) -> f32 {
    let a = (t * 2.7 + seed * std::f32::consts::TAU).sin();
    let b = (t * 4.1 + seed * (2.0 * std::f32::consts::TAU)).sin();
    a.mul_add(0.6, b * 0.4)
}

/// How far through its life particle `index` is at `time`, in `0.0..1.0`, or `None` if it is not alive.
///
/// Continuous emitters stagger births evenly across the lifetime, so the stream is dense from the first
/// frame instead of arriving as a visible clump. A burst emits everything at t = 0 and is over.
#[must_use]
pub fn particle_age(recipe: &EffectRecipe, index: u32, time: f32) -> Option<f32> {
    particle_age_of(recipe, recipe.live_count(), index, time)
}

/// [`particle_age`] with the stream size stated explicitly.
///
/// `resolve_stack` may GRANT fewer particles than a layer asked for (the budget). Staggering births
/// across the requested count while only walking the granted one bunched every surviving particle into
/// a narrow slice of the life cycle, so a thinned plume blinked on and off instead of streaming. The
/// stream is spread across the particles that will actually be drawn.
#[must_use]
pub fn particle_age_of(recipe: &EffectRecipe, stream: u32, index: u32, time: f32) -> Option<f32> {
    let life = recipe.lifetime.clamp(0.05, MAX_LIFETIME);
    let stream = stream.clamp(1, MAX_PARTICLES);
    if !time.is_finite() || time < 0.0 {
        return None;
    }
    #[allow(clippy::cast_precision_loss)] // both are clamped to <= MAX_PARTICLES = 512
    let stagger = (index % stream) as f32 / stream as f32;
    if recipe.burst {
        // Everything born at once, with a small hashed delay so it does not look mechanical.
        let born = hashed(index, 0x51) * life * 0.15;
        let age = (time - born) / life;
        return (0.0..1.0).contains(&age).then_some(age);
    }
    // Continuous: particle `index` is reborn every `life` seconds, offset by its stagger, and the
    // stream is PRE-WARMED — at t = 0 it looks the way it will look forever, as though it had already
    // been running.
    //
    // The previous version returned `None` for any particle whose stagger had not come round yet, so a
    // 2.6 s smoke plume had a third of its particles at t = 0.9 s and only reached full density after a
    // full lifetime. Every continuous effect was visibly thin for its first seconds — a fire that
    // catches, a plume that has to build. The doc comment already claimed "dense from the first frame";
    // the code did not do it. `rem_euclid` wraps the negative phase instead of discarding it, which
    // costs nothing and is still a pure function of (index, time).
    Some(((time / life) - stagger).rem_euclid(1.0))
}

/// Resolve particle `index` of `recipe` at `time`, anchored on `emitter`.
///
/// Returns `None` when the particle is not alive. Never panics and never returns a non-finite value: a
/// NaN that reached the instance buffer would take the whole viewport down, so every arithmetic result
/// is checked before it leaves.
#[must_use]
pub fn particle_at(
    recipe: &EffectRecipe,
    emitter: EmitterSample,
    index: u32,
    time: f32,
) -> Option<ParticleSample> {
    particle_at_of(recipe, emitter, recipe.live_count(), index, time)
}

/// [`particle_at`] with the stream size stated explicitly — see [`particle_age_of`].
#[must_use]
#[allow(clippy::too_many_lines)] // one closed-form solve; splitting it would hide the physics
pub fn particle_at_of(
    recipe: &EffectRecipe,
    emitter: EmitterSample,
    stream: u32,
    index: u32,
    time: f32,
) -> Option<ParticleSample> {
    let age = particle_age_of(recipe, stream, index, time)?;
    let life = recipe.lifetime.clamp(0.05, MAX_LIFETIME);
    // Absolute seconds this particle has been alive — the variable the physics is actually in.
    let t = age * life;
    let scale = emitter.radius.max(0.05);

    // ── birth position ──────────────────────────────────────────────────────────────────────────
    let (ux, uy, uz) = (
        hashed_signed(index, 1),
        hashed_signed(index, 2),
        hashed_signed(index, 3),
    );
    let spread = recipe.spread.clamp(0.0, 4.0) * scale;
    let birth = match recipe.shape {
        EmitShape::Point => [0.0, 0.0, 0.0],
        EmitShape::Volume => [ux * spread, uy * spread, uz * spread],
        EmitShape::Shell => {
            let len = (ux * ux + uy * uy + uz * uz).sqrt().max(1.0e-4);
            [ux / len * spread, uy / len * spread, uz / len * spread]
        }
        EmitShape::Disc => {
            let angle = hashed(index, 4) * std::f32::consts::TAU;
            let r = hashed(index, 5).sqrt() * spread;
            [angle.cos() * r, -scale * 0.9, angle.sin() * r]
        }
        EmitShape::Cone | EmitShape::Column => {
            let angle = hashed(index, 6) * std::f32::consts::TAU;
            let r = hashed(index, 7).sqrt() * spread * 0.35;
            [angle.cos() * r, -scale * 0.5, angle.sin() * r]
        }
    };

    // ── velocity at birth ───────────────────────────────────────────────────────────────────────
    let speed = recipe.speed * (0.6 + hashed(index, 8) * 0.8);
    let velocity = match recipe.motion {
        Motion::Radiate => {
            let len = (ux * ux + uy * uy + uz * uz).sqrt().max(1.0e-4);
            [ux / len * speed, uy / len * speed, uz / len * speed]
        }
        Motion::Rise => [
            hashed_signed(index, 9) * speed * 0.25,
            speed,
            hashed_signed(index, 10) * speed * 0.25,
        ],
        Motion::Fall => {
            let angle = hashed(index, 11) * std::f32::consts::TAU;
            let out = speed * (0.35 + hashed(index, 12) * 0.65);
            [angle.cos() * out, speed, angle.sin() * out]
        }
        Motion::Orbit => {
            let angle = hashed(index, 13) * std::f32::consts::TAU;
            [-angle.sin() * speed, speed * 0.15, angle.cos() * speed]
        }
        Motion::Ripple => {
            let angle = hashed(index, 21) * std::f32::consts::TAU;
            [angle.cos() * speed, 0.0, angle.sin() * speed]
        }
        Motion::Trail => [
            hashed_signed(index, 14) * speed * 0.3,
            hashed_signed(index, 15) * speed * 0.3,
            hashed_signed(index, 16) * speed * 0.3,
        ],
    };

    // ── integrate, in closed form ───────────────────────────────────────────────────────────────
    // Exponential drag has an exact solution, so this is not an approximation of a stepped sim — it IS
    // the trajectory: x(t) = v0 * (1 - e^(-kt)) / k, plus ½gt².
    let k = recipe.drag.clamp(0.0, 8.0);
    let travel = if k > 1.0e-3 {
        (1.0 - (-k * t).exp()) / k
    } else {
        t
    };
    let drop = 0.5 * recipe.gravity * t * t;

    let mut position = [
        velocity[0].mul_add(travel, birth[0]),
        velocity[1].mul_add(travel, birth[1]) - drop,
        velocity[2].mul_add(travel, birth[2]),
    ];

    // Orbit is angular, so it is applied as a rotation rather than a straight line.
    if matches!(recipe.motion, Motion::Orbit) {
        let angle0 = hashed(index, 13) * std::f32::consts::TAU;
        let radius = spread.max(scale * 0.8) * (0.6 + hashed(index, 17) * 0.6);
        let omega = recipe.speed * 0.6;
        let angle = angle0 + omega * t;
        position[0] = angle.cos() * radius;
        position[2] = angle.sin() * radius;
    }

    // Turbulence: a wobble that GROWS with age, so a flame is tight at its base and ragged at its tip.
    let turb = recipe.turbulence.clamp(0.0, 4.0) * scale * age;
    if turb > 0.0 {
        position[0] += wobble(hashed(index, 18), t) * turb;
        position[1] += wobble(hashed(index, 19), t) * turb * 0.4;
        position[2] += wobble(hashed(index, 20), t) * turb;
    }

    // ── appearance over life ────────────────────────────────────────────────────────────────────
    let size_end = recipe.size_end.clamp(0.0, 8.0);
    let radius = recipe.size.clamp(0.001, 8.0) * scale * (1.0 - age).mul_add(1.0, age * size_end);
    // Colour is the one recipe field with a legitimate reason to exceed 1.0 (that excess IS the bloom),
    // which is exactly why it needs an upper bound: the HDR target is `Rgba16Float`, so a finite-but-huge
    // value like 1e38 stores as +Inf, and the ACES curve downstream evaluates Inf/Inf = NaN and paints
    // the bright core black. MAX_RADIANCE is far above anything the catalogue asks for and far below f16
    // saturation, so it never touches an authored effect and always saves a hostile one.
    let mix = |a: f32, b: f32| (a + (b - a) * age).clamp(0.0, MAX_RADIANCE);
    let color = [
        mix(recipe.color[0], recipe.color_end[0]),
        mix(recipe.color[1], recipe.color_end[1]),
        mix(recipe.color[2], recipe.color_end[2]),
    ];
    // Fade IN quickly and OUT slowly — a particle that pops into existence at full opacity reads as a
    // glitch, and one that vanishes at full opacity reads as a dropped frame.
    let fade_in = (age / 0.12).min(1.0);
    let fade_out = ((1.0 - age) / 0.35).min(1.0);
    let alpha = recipe.alpha.clamp(0.0, 1.0) * fade_in * fade_out;

    let world = [
        emitter.origin[0] + position[0],
        emitter.origin[1] + position[1],
        emitter.origin[2] + position[2],
    ];
    // The NaN gate. Anything non-finite is dropped here rather than uploaded.
    if !world.iter().all(|v| v.is_finite())
        || !radius.is_finite()
        || !alpha.is_finite()
        || !color.iter().all(|c| c.is_finite())
    {
        return None;
    }
    Some(ParticleSample {
        position: world,
        radius: radius.max(0.001),
        color,
        alpha: alpha.clamp(0.0, 1.0),
    })
}

/// Resolve a whole stack at `time`, appending into `out`. Respects [`MAX_PARTICLES`] across the WHOLE
/// stack — the budget is a property of the effect, not of one layer, so a four-layer inferno cannot
/// quietly cost four times what a one-layer one does.
pub fn resolve_stack(
    stack: &EffectStack,
    emitter: EmitterSample,
    time: f32,
    out: &mut Vec<(ParticleSample, Look)>,
) {
    let mut budget = SceneBudget::new();
    resolve_layers(stack.layers.iter(), emitter, time, &mut budget, out);
}

/// Resolve an explicit set of layers against a SHARED frame budget.
///
/// The budget is threaded rather than reset per object so the cap is a property of the frame. Layers
/// are also given the count the budget actually granted, so a thinned layer stays a stream.
pub fn resolve_layers<'a>(
    layers: impl Iterator<Item = &'a EffectRecipe>,
    emitter: EmitterSample,
    time: f32,
    budget: &mut SceneBudget,
    out: &mut Vec<(ParticleSample, Look)>,
) {
    let mut per_object = 0u32;
    for layer in layers.take(MAX_LAYERS) {
        let want = layer.live_count();
        let object_room = MAX_PARTICLES.saturating_sub(per_object);
        let granted = u32::try_from(budget.take(want.min(object_room) as usize)).unwrap_or(0);
        if granted == 0 {
            continue;
        }
        per_object += granted;
        for i in 0..granted {
            if let Some(p) = particle_at_of(layer, emitter, granted, i, time) {
                if p.alpha > 0.002 {
                    out.push((p, layer.look));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fire() -> EffectRecipe {
        EffectRecipe {
            id: "f".into(),
            kind: "fire".into(),
            count: 48,
            lifetime: 1.1,
            shape: EmitShape::Disc,
            motion: Motion::Rise,
            look: Look::Additive,
            speed: 1.8,
            gravity: -0.6,
            drag: 0.9,
            turbulence: 0.5,
            size: 0.18,
            size_end: 0.35,
            color: [4.0, 1.6, 0.35],
            color_end: [1.2, 0.15, 0.02],
            alpha: 0.9,
            burst: false,
            trigger: FxTrigger::Always,
            spread: 0.5,
        }
    }

    const EMITTER: EmitterSample = EmitterSample {
        origin: [1.0, 2.0, 3.0],
        radius: 1.0,
    };

    #[test]
    fn a_particle_is_a_pure_function_of_index_and_time() {
        // The whole determinism claim, as an assertion: same inputs, byte-identical outputs, with no
        // state carried between the two calls.
        let r = fire();
        for i in 0..48 {
            let a = particle_at(&r, EMITTER, i, 0.63);
            let b = particle_at(&r, EMITTER, i, 0.63);
            assert_eq!(a, b, "particle {i} was not reproducible");
        }
    }

    #[test]
    fn seeking_forward_equals_stepping_forward() {
        // The scrubbing claim: evaluating t=2.0 directly gives exactly what you would have got by
        // arriving there. A stepped simulation cannot promise this.
        let r = fire();
        let direct = particle_at(&r, EMITTER, 7, 2.0);
        for t in [0.1, 0.4, 0.9, 1.5, 1.9] {
            let _ = particle_at(&r, EMITTER, 7, t);
        }
        assert_eq!(direct, particle_at(&r, EMITTER, 7, 2.0));
    }

    #[test]
    fn fire_rises_and_cools() {
        let r = fire();
        // Find a particle that is young at t and older later within the same life cycle.
        let young = particle_at(&r, EMITTER, 0, 0.05).expect("alive");
        let old = particle_at(&r, EMITTER, 0, 0.9).expect("still alive");
        assert!(old.position[1] > young.position[1], "fire must rise");
        assert!(old.color[0] < young.color[0], "and cool as it goes");
    }

    #[test]
    fn a_burst_fires_once_and_is_gone() {
        let mut r = fire();
        r.burst = true;
        r.lifetime = 1.0;
        assert!(particle_at(&r, EMITTER, 3, 0.2).is_some(), "alive early");
        assert!(
            particle_at(&r, EMITTER, 3, 3.0).is_none(),
            "a burst must not loop"
        );
    }

    #[test]
    fn a_continuous_stream_is_at_full_density_from_the_very_first_frame() {
        // A plume that takes a whole lifetime to fill in reads as the effect "starting up", which is
        // wrong for anything already burning when Play begins.
        let r = fire();
        let at = |t: f32| {
            (0..r.live_count())
                .filter(|i| particle_at(&r, EMITTER, *i, t).is_some())
                .count()
        };
        let settled = at(9.7);
        assert_eq!(
            at(0.0),
            settled,
            "frame 0 is thinner than the settled stream"
        );
        assert_eq!(at(0.25), settled, "still filling in a quarter second later");
    }

    #[test]
    fn a_continuous_emitter_is_still_alive_much_later() {
        let r = fire();
        let alive = (0..48)
            .filter(|i| particle_at(&r, EMITTER, *i, 57.3).is_some())
            .count();
        assert!(alive > 30, "a stream must keep streaming, got {alive}");
    }

    #[test]
    fn the_effect_scales_itself_to_what_it_is_attached_to() {
        // 🔥 on a matchstick and 🔥 on a barn must both look right with the author touching nothing.
        let r = fire();
        let small = particle_at(
            &r,
            EmitterSample {
                origin: [0.0; 3],
                radius: 0.2,
            },
            5,
            0.5,
        )
        .expect("alive");
        let big = particle_at(
            &r,
            EmitterSample {
                origin: [0.0; 3],
                radius: 5.0,
            },
            5,
            0.5,
        )
        .expect("alive");
        assert!(big.radius > small.radius * 5.0, "size must follow the host");
    }

    #[test]
    fn nothing_non_finite_ever_escapes() {
        // Hostile input: the recipe is user-adjacent data, and one NaN in the instance buffer takes the
        // whole viewport down.
        let mut r = fire();
        r.speed = f32::NAN;
        r.gravity = f32::INFINITY;
        r.size = f32::NAN;
        for i in 0..48 {
            for t in [0.0, 0.5, 1.0, 9.5] {
                if let Some(p) = particle_at(&r, EMITTER, i, t) {
                    assert!(p.position.iter().all(|v| v.is_finite()));
                    assert!(p.radius.is_finite() && p.alpha.is_finite());
                }
            }
        }
        // And a non-finite CLOCK is refused rather than propagated.
        assert!(particle_at(&fire(), EMITTER, 0, f32::NAN).is_none());
    }

    #[test]
    fn the_budget_is_a_property_of_the_whole_stack() {
        let stack = EffectStack {
            version: 1,
            layers: vec![
                EffectRecipe {
                    count: 400,
                    ..fire()
                },
                EffectRecipe {
                    count: 400,
                    ..fire()
                },
                EffectRecipe {
                    count: 400,
                    ..fire()
                },
            ],
        };
        let mut out = Vec::new();
        resolve_stack(&stack, EMITTER, 0.5, &mut out);
        assert!(
            out.len() <= MAX_PARTICLES as usize,
            "budget blown: {}",
            out.len()
        );
        // and it says so, in the author's language
        assert!(stack.problems().iter().any(|p| p.contains("budget")));
    }

    #[test]
    fn particles_fade_in_and_out_rather_than_popping() {
        let r = fire();
        let born = particle_at(&r, EMITTER, 0, 0.001).expect("alive");
        let mid = particle_at(&r, EMITTER, 0, 0.5).expect("alive");
        let dying = particle_at(&r, EMITTER, 0, 1.09).expect("alive");
        assert!(born.alpha < mid.alpha, "must fade in");
        assert!(dying.alpha < mid.alpha, "must fade out");
    }

    #[test]
    fn a_fresh_stack_carries_the_current_schema_version_not_zero() {
        // A version-0 stamp on every file written today would make any future migration misread the
        // whole corpus. `serde(default)` does not cover the `Default` impl, so this is asserted.
        assert_eq!(EffectStack::default().version, 1);
        let json = serde_json::to_string(&EffectStack::default()).expect("serialises");
        assert!(json.contains("\"version\":1"), "{json}");
    }

    #[test]
    fn an_absurd_count_cannot_collapse_the_stream() {
        // The overflow that made particle `i` appear `i` SECONDS in: a hostile count must be clamped,
        // not trusted, and every particle must still be alive within one lifetime.
        let mut r = fire();
        r.count = 100_000;
        assert_eq!(r.live_count(), MAX_PARTICLES);
        let alive = (0..MAX_PARTICLES)
            .filter(|i| particle_at(&r, EMITTER, *i, 1.2).is_some())
            .count();
        assert!(alive > (MAX_PARTICLES / 2) as usize, "only {alive} alive");
    }

    #[test]
    fn a_thinned_layer_still_streams_instead_of_pulsing() {
        // Grant a layer far fewer particles than it asked for; the survivors must still be spread
        // across the whole life cycle, not bunched into one slice of it.
        let r = EffectRecipe {
            count: 400,
            ..fire()
        };
        let granted = 40u32;
        let ages: Vec<f32> = (0..granted)
            .filter_map(|i| particle_age_of(&r, granted, i, 3.0))
            .collect();
        assert_eq!(ages.len(), granted as usize);
        let lo = ages.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = ages.iter().copied().fold(0.0_f32, f32::max);
        assert!(hi - lo > 0.8, "ages span only {:.2}", hi - lo);
    }

    #[test]
    fn one_hostile_colour_cannot_poison_the_tone_curve() {
        let mut r = fire();
        r.color = [1.0e38, 0.0, 0.0];
        r.color_end = [1.0e38, 0.0, 0.0];
        let p = particle_at(&r, EMITTER, 0, 0.3).expect("alive");
        assert!(p.color[0] <= MAX_RADIANCE, "{}", p.color[0]);
        assert!(p.color.iter().all(|c| c.is_finite()));
    }

    #[test]
    fn the_budget_is_a_property_of_the_frame_not_of_one_object() {
        // Fifty objects each carrying a legal stack must not resolve 17,000 particles.
        let stack = EffectStack {
            version: 1,
            layers: vec![fire(), fire(), fire(), fire()],
        };
        let mut budget = SceneBudget::new();
        let mut out = Vec::new();
        for _ in 0..50 {
            resolve_layers(stack.layers.iter(), EMITTER, 0.5, &mut budget, &mut out);
        }
        assert!(
            budget.spent() <= MAX_SCENE_PARTICLES,
            "spent {}",
            budget.spent()
        );
        assert!(out.len() <= MAX_SCENE_PARTICLES);
        // and it SAYS it dropped something rather than silently truncating
        assert!(budget.dropped() > 0);
    }

    #[test]
    fn a_burst_is_retired_only_after_its_last_particle_is_gone() {
        let mut r = fire();
        r.burst = true;
        r.lifetime = 10.0;
        // The last particle is born at 0.15*life, so something is still alive past `life`.
        assert!(
            particle_at(&r, EMITTER, 3, 10.4).is_some()
                || (0..48).any(|i| particle_at(&r, EMITTER, i, 10.4).is_some()),
            "a long burst still has particles after life+0.4"
        );
        // total_seconds must cover them all
        assert!((0..48).all(|i| particle_at(&r, EMITTER, i, r.total_seconds() + 0.01).is_none()));
    }

    #[test]
    fn a_stack_round_trips_through_json() {
        let stack = EffectStack {
            version: 1,
            layers: vec![fire()],
        };
        let json = serde_json::to_string(&stack).expect("serialises");
        let back: EffectStack = serde_json::from_str(&json).expect("parses");
        assert_eq!(back, stack);
    }
}
