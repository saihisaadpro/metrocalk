//! Visual effects — the author picks a WORD, not a node graph.
//!
//! Niagara and VFX Graph both make an effect a separate asset, authored in a separate graph editor,
//! dropped into the scene, and then triggered from code. Three tools and a script for "this crate is on
//! fire". The gesture here is the role card's: select the crate, click 🔥 Fire, press Play — it burns.
//!
//! Two things make that more than a preset library:
//!
//! * **Effects respond to the game.** A card can be *attached to a moment* rather than left running:
//!   "when it's picked up", "when it's hit", "when it's beaten". That writes an ordinary condition-free
//!   `SetField Vfx.playing = true` action onto the rule the object's role already owns — so the closed
//!   action vocabulary never grew a verb, and the effect is undoable, replayable and inspectable like
//!   everything else.
//! * **Effects scale themselves.** Every recipe is written in units of the HOST object's radius, so 🔥
//!   on a matchstick and 🔥 on a barn are both right with the author touching no numbers.
//!
//! Storage is the `PlayIf`/`Cinematic` pattern: canonical JSON on the object's own `Vfx.source`,
//! written only by the validated commands here.

use metrocalk_animation::vfx::{
    EffectRecipe, EffectStack, EmitShape, FxTrigger, Look, Motion, MAX_LAYERS, MAX_PARTICLES,
};
use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::World;
use serde::{Deserialize, Serialize};

/// The component holding an object's effects (registered in the stdlib).
pub const VFX_COMPONENT: &str = "Vfx";

/// One effect card, in the author's language — the [`crate::role_intent::RoleSpec`] sibling.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectSpec {
    /// Stable kind key.
    pub kind: &'static str,
    /// Card label.
    pub label: &'static str,
    /// What it is for, one line.
    pub blurb: &'static str,
    /// Card glyph.
    pub icon: &'static str,
    /// What it will look like, spelled out — the legible-cost line.
    pub adds: &'static str,
    /// True when the card is a one-shot (fires at a moment, rather than running).
    pub burst: bool,
}

/// When an effect runs. Re-exported from the animation kernel, because the trigger is now stored ON
/// THE LAYER: one object routinely wants a flame that burns the whole time and an explosion that fires
/// once when it is hit, and a single per-object `playing` boolean made those mutually exclusive.
pub type Trigger = FxTrigger;

/// The catalogue. Each card is a complete effect, so the first click already looks good — the numbers
/// here are the whole reason the author never has to type any.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "a nineteen-card data table; splitting it hides the catalogue"
)]
pub fn effect_specs() -> Vec<EffectSpec> {
    vec![
        EffectSpec {
            kind: "fire",
            label: "Fire",
            blurb: "It burns — rising, flickering, warm",
            icon: "🔥",
            adds: "a rising flame that cools from white-hot to ember red",
            burst: false,
        },
        EffectSpec {
            kind: "smoke",
            label: "Smoke",
            blurb: "Dark, slow, expanding — pair it with fire",
            icon: "💨",
            adds: "a slow dark plume that spreads as it climbs",
            burst: false,
        },
        EffectSpec {
            kind: "sparkle",
            label: "Sparkle",
            blurb: "Magic, treasure, something worth walking toward",
            icon: "✨",
            adds: "bright motes orbiting the object",
            burst: false,
        },
        EffectSpec {
            kind: "explosion",
            label: "Explosion",
            blurb: "One shot — a violent outward burst",
            icon: "💥",
            adds: "a one-shot blast of hot debris that falls away",
            burst: true,
        },
        EffectSpec {
            kind: "sparks",
            label: "Sparks",
            blurb: "One shot — metal on metal, a hit landing",
            icon: "⚡",
            adds: "a one-shot spray of fast sparks that arc and drop",
            burst: true,
        },
        EffectSpec {
            kind: "pickup",
            label: "Pick-up pop",
            blurb: "One shot — the little flourish a collectible deserves",
            icon: "🎉",
            adds: "a one-shot ring of bright motes rising and fading",
            burst: true,
        },
        EffectSpec {
            kind: "fountain",
            label: "Fountain",
            blurb: "Arcing up and falling back — water, lava, coins",
            icon: "⛲",
            adds: "an arcing jet that falls back under gravity",
            burst: false,
        },
        EffectSpec {
            kind: "dust",
            label: "Dust",
            blurb: "Grounded and subtle — settled air, a dirt floor",
            icon: "🍂",
            adds: "slow motes drifting near the base",
            burst: false,
        },
        EffectSpec {
            kind: "aura",
            label: "Aura",
            blurb: "A healing, powered-up, or cursed glow",
            icon: "💚",
            adds: "a soft rising column of light around the object",
            burst: false,
        },
        EffectSpec {
            kind: "steam",
            label: "Steam",
            blurb: "Hot and wet — a vent, a kettle, a cooling engine",
            icon: "♨",
            adds: "a pale plume that billows and thins as it climbs",
            burst: false,
        },
        EffectSpec {
            kind: "embers",
            label: "Embers",
            blurb: "What is left after the fire — slow, drifting, orange",
            icon: "🔶",
            adds: "sparse glowing motes rising slowly and fading",
            burst: false,
        },
        EffectSpec {
            kind: "rain",
            label: "Rain",
            blurb: "Falling weather around the object",
            icon: "🌧",
            adds: "fast pale streaks falling from above",
            burst: false,
        },
        EffectSpec {
            kind: "snow",
            label: "Snow",
            blurb: "Slow falling weather that drifts sideways",
            icon: "❄",
            adds: "slow white flakes drifting down and across",
            burst: false,
        },
        EffectSpec {
            kind: "splash",
            label: "Splash",
            blurb: "One shot — something hit water",
            icon: "💧",
            adds: "a one-shot crown of droplets thrown up and falling back",
            burst: true,
        },
        EffectSpec {
            kind: "shockwave",
            label: "Shockwave",
            blurb: "One shot — a flat ring racing outward along the ground",
            icon: "💫",
            adds: "a one-shot expanding ring at the base",
            burst: true,
        },
        EffectSpec {
            kind: "confetti",
            label: "Confetti",
            blurb: "One shot — celebration, a level cleared",
            icon: "🎊",
            adds: "a one-shot burst of coloured flecks thrown up and falling",
            burst: true,
        },
        EffectSpec {
            kind: "poison",
            label: "Poison cloud",
            blurb: "A hazard you can see — pair it with the Hazard role",
            icon: "☠",
            adds: "a low sickly-green cloud that spreads and lingers",
            burst: false,
        },
        EffectSpec {
            kind: "bubbles",
            label: "Bubbles",
            blurb: "Underwater, a potion, something fermenting",
            icon: "🫧",
            adds: "slow round motes rising and growing",
            burst: false,
        },
        EffectSpec {
            kind: "portal",
            label: "Portal",
            blurb: "Something otherworldly is open here",
            icon: "🌀",
            adds: "a tight ring of motes circling fast, edge-lit violet",
            burst: false,
        },
    ]
}

/// The card's numbers. Everything is in units of the HOST's radius, which is why one card fits every
/// object size.
#[allow(
    clippy::too_many_lines,
    reason = "a nine-card data table; splitting it hides the catalogue"
)]
fn recipe_for(kind: &str, owner: EntityId, index: usize) -> Option<EffectRecipe> {
    let base = |id: String| EffectRecipe {
        id,
        kind: kind.to_string(),
        ..EffectRecipe::default()
    };
    // The OWNER is part of the identity. Without it every object's first fire layer carried the same
    // id, so ids were unique per object and globally ambiguous — fine until the first consumer keys off
    // them (per-layer selection, a diff, a CRDT merge), which is the worst time to find out.
    let id = format!(
        "fx-{:08x}",
        stable_hash(&format!("{kind}:{}:{index}", owner.to_loro_key()))
    );
    let r = match kind {
        "fire" => EffectRecipe {
            count: 72,
            lifetime: 1.0,
            shape: EmitShape::Disc,
            motion: Motion::Rise,
            look: Look::Additive,
            speed: 1.6,
            gravity: -0.8,
            drag: 1.1,
            turbulence: 0.55,
            size: 0.22,
            size_end: 0.30,
            // Above 1.0 on purpose: this is a linear-HDR scene, so the excess is what blooms.
            color: [6.0, 2.2, 0.5],
            color_end: [1.1, 0.12, 0.02],
            alpha: 0.85,
            burst: false,
            spread: 0.45,
            ..base(id.clone())
        },
        "smoke" => EffectRecipe {
            count: 52,
            lifetime: 2.6,
            shape: EmitShape::Disc,
            motion: Motion::Rise,
            look: Look::Soft,
            speed: 0.7,
            gravity: -0.15,
            drag: 0.7,
            turbulence: 0.8,
            // Bigger and lighter than the first pass, which measured 40 particles on screen and was
            // still almost invisible in a capture: a near-black puff at 0.42 alpha, one seventh of the
            // host's radius across, simply does not read against a bright floor. Smoke lit by ambient
            // light IS mid-grey, so this is more physical as well as more legible.
            size: 0.46,
            size_end: 2.4,
            color: [0.34, 0.33, 0.32],
            color_end: [0.13, 0.13, 0.15],
            alpha: 0.55,
            burst: false,
            spread: 0.4,
            ..base(id.clone())
        },
        "sparkle" => EffectRecipe {
            count: 48,
            lifetime: 1.6,
            shape: EmitShape::Shell,
            motion: Motion::Orbit,
            look: Look::Additive,
            speed: 1.4,
            gravity: 0.0,
            drag: 0.2,
            turbulence: 0.25,
            size: 0.07,
            size_end: 0.2,
            color: [3.2, 3.0, 5.0],
            color_end: [0.6, 0.9, 2.0],
            alpha: 1.0,
            burst: false,
            spread: 1.1,
            ..base(id.clone())
        },
        "explosion" => EffectRecipe {
            count: 110,
            lifetime: 1.1,
            shape: EmitShape::Point,
            motion: Motion::Radiate,
            look: Look::Additive,
            speed: 9.0,
            gravity: 7.0,
            drag: 2.6,
            turbulence: 0.2,
            size: 0.20,
            size_end: 0.35,
            color: [9.0, 3.4, 0.7],
            color_end: [0.9, 0.10, 0.02],
            alpha: 1.0,
            burst: true,
            spread: 0.2,
            ..base(id.clone())
        },
        "sparks" => EffectRecipe {
            count: 64,
            lifetime: 0.6,
            shape: EmitShape::Point,
            motion: Motion::Fall,
            look: Look::Additive,
            speed: 7.0,
            gravity: 14.0,
            drag: 1.2,
            turbulence: 0.1,
            size: 0.045,
            size_end: 0.15,
            color: [8.0, 5.0, 1.6],
            color_end: [2.0, 0.5, 0.05],
            alpha: 1.0,
            burst: true,
            spread: 0.15,
            ..base(id.clone())
        },
        "pickup" => EffectRecipe {
            count: 44,
            lifetime: 0.75,
            shape: EmitShape::Disc,
            motion: Motion::Rise,
            look: Look::Additive,
            speed: 3.6,
            gravity: 2.0,
            drag: 2.0,
            turbulence: 0.15,
            size: 0.09,
            size_end: 0.25,
            color: [3.0, 5.5, 2.4],
            color_end: [0.5, 1.6, 0.7],
            alpha: 1.0,
            burst: true,
            spread: 0.7,
            ..base(id.clone())
        },
        "fountain" => EffectRecipe {
            count: 90,
            lifetime: 1.8,
            shape: EmitShape::Cone,
            motion: Motion::Fall,
            look: Look::Additive,
            speed: 5.0,
            gravity: 9.0,
            drag: 0.15,
            turbulence: 0.1,
            size: 0.08,
            size_end: 0.6,
            color: [0.6, 1.8, 3.4],
            color_end: [0.15, 0.5, 1.1],
            alpha: 0.8,
            burst: false,
            spread: 0.25,
            ..base(id.clone())
        },
        "dust" => EffectRecipe {
            count: 36,
            lifetime: 3.2,
            shape: EmitShape::Disc,
            motion: Motion::Trail,
            look: Look::Soft,
            speed: 0.35,
            gravity: -0.05,
            drag: 0.9,
            turbulence: 0.6,
            size: 0.10,
            size_end: 1.4,
            color: [0.42, 0.38, 0.31],
            color_end: [0.24, 0.22, 0.19],
            alpha: 0.30,
            burst: false,
            spread: 1.3,
            ..base(id.clone())
        },
        // ── the SOFT (alpha-blended) family ──────────────────────────────────────────────────
        // Steam, snow and rain exist partly to give the occluding pipeline real users: until these
        // landed every shipped card but smoke and dust was additive, so the alpha path had no
        // coverage at all.
        "steam" => EffectRecipe {
            count: 46,
            lifetime: 2.2,
            shape: EmitShape::Disc,
            motion: Motion::Rise,
            look: Look::Soft,
            speed: 1.1,
            gravity: -0.35,
            drag: 0.8,
            turbulence: 0.7,
            size: 0.22,
            size_end: 2.2,
            color: [0.82, 0.85, 0.88],
            color_end: [0.55, 0.6, 0.66],
            alpha: 0.34,
            burst: false,
            spread: 0.3,
            ..base(id.clone())
        },
        "embers" => EffectRecipe {
            count: 28,
            lifetime: 2.8,
            shape: EmitShape::Volume,
            motion: Motion::Rise,
            look: Look::Additive,
            speed: 0.55,
            gravity: -0.3,
            drag: 0.6,
            turbulence: 0.9,
            size: 0.045,
            size_end: 0.4,
            color: [5.0, 1.5, 0.25],
            color_end: [0.7, 0.08, 0.01],
            alpha: 1.0,
            burst: false,
            spread: 0.8,
            ..base(id.clone())
        },
        "rain" => EffectRecipe {
            count: 120,
            lifetime: 1.0,
            // Born ABOVE the object and falling through it: `spread` is wide and gravity does the rest.
            shape: EmitShape::Volume,
            motion: Motion::Fall,
            look: Look::Soft,
            speed: 0.2,
            gravity: 22.0,
            drag: 0.05,
            size: 0.035,
            size_end: 1.0,
            turbulence: 0.05,
            color: [0.62, 0.72, 0.85],
            color_end: [0.45, 0.55, 0.7],
            alpha: 0.5,
            burst: false,
            spread: 2.2,
            ..base(id.clone())
        },
        "snow" => EffectRecipe {
            count: 80,
            lifetime: 4.0,
            shape: EmitShape::Volume,
            motion: Motion::Fall,
            look: Look::Soft,
            speed: 0.15,
            gravity: 0.9,
            drag: 0.4,
            turbulence: 1.1,
            size: 0.05,
            size_end: 1.0,
            color: [0.95, 0.96, 1.0],
            color_end: [0.8, 0.84, 0.92],
            alpha: 0.75,
            burst: false,
            spread: 2.4,
            ..base(id.clone())
        },
        "splash" => EffectRecipe {
            count: 70,
            lifetime: 1.0,
            shape: EmitShape::Disc,
            motion: Motion::Fall,
            look: Look::Additive,
            speed: 4.2,
            gravity: 11.0,
            drag: 0.3,
            turbulence: 0.1,
            size: 0.06,
            size_end: 0.5,
            color: [1.4, 2.4, 3.6],
            color_end: [0.3, 0.7, 1.2],
            alpha: 0.85,
            burst: true,
            spread: 0.45,
            ..base(id.clone())
        },
        "shockwave" => EffectRecipe {
            count: 96,
            lifetime: 0.65,
            // A flat ring: born on a disc at the base and driven outward ALONG THE GROUND with heavy
            // drag, so it races away and stops rather than drifting. `Radiate` was wrong here — it
            // picks a direction on the whole sphere, so the "ground ripple" climbed as it spread.
            shape: EmitShape::Disc,
            motion: Motion::Ripple,
            look: Look::Additive,
            speed: 13.0,
            gravity: 0.0,
            drag: 4.5,
            turbulence: 0.0,
            size: 0.10,
            size_end: 0.25,
            color: [3.0, 3.6, 6.0],
            color_end: [0.4, 0.6, 1.4],
            alpha: 0.9,
            burst: true,
            spread: 0.15,
            ..base(id.clone())
        },
        "confetti" => EffectRecipe {
            count: 90,
            lifetime: 2.4,
            shape: EmitShape::Point,
            motion: Motion::Fall,
            look: Look::Soft,
            speed: 5.5,
            gravity: 6.5,
            drag: 0.9,
            turbulence: 1.4,
            size: 0.05,
            size_end: 0.9,
            color: [1.0, 0.55, 0.75],
            color_end: [0.45, 0.7, 0.95],
            alpha: 0.95,
            burst: true,
            spread: 0.3,
            ..base(id.clone())
        },
        "poison" => EffectRecipe {
            count: 54,
            lifetime: 3.4,
            shape: EmitShape::Disc,
            motion: Motion::Rise,
            look: Look::Soft,
            speed: 0.4,
            gravity: -0.08,
            drag: 1.3,
            turbulence: 0.75,
            size: 0.28,
            size_end: 2.0,
            color: [0.32, 0.60, 0.18],
            color_end: [0.14, 0.30, 0.10],
            alpha: 0.40,
            burst: false,
            spread: 1.0,
            ..base(id.clone())
        },
        "bubbles" => EffectRecipe {
            count: 34,
            lifetime: 2.6,
            shape: EmitShape::Column,
            motion: Motion::Rise,
            look: Look::Soft,
            speed: 0.85,
            gravity: -0.5,
            drag: 0.25,
            turbulence: 0.45,
            size: 0.07,
            size_end: 1.8,
            color: [0.72, 0.88, 0.98],
            color_end: [0.85, 0.95, 1.0],
            alpha: 0.30,
            burst: false,
            spread: 0.4,
            ..base(id.clone())
        },
        "portal" => EffectRecipe {
            count: 66,
            lifetime: 1.3,
            shape: EmitShape::Shell,
            motion: Motion::Orbit,
            look: Look::Additive,
            speed: 3.4,
            gravity: 0.0,
            drag: 0.1,
            turbulence: 0.15,
            size: 0.055,
            size_end: 0.3,
            color: [3.4, 0.9, 5.5],
            color_end: [0.9, 0.2, 2.2],
            alpha: 1.0,
            burst: false,
            spread: 1.0,
            ..base(id.clone())
        },
        "aura" => EffectRecipe {
            count: 52,
            lifetime: 1.9,
            shape: EmitShape::Shell,
            motion: Motion::Rise,
            look: Look::Additive,
            speed: 0.9,
            gravity: -0.2,
            drag: 0.8,
            turbulence: 0.2,
            size: 0.11,
            size_end: 0.45,
            color: [0.8, 4.2, 1.6],
            color_end: [0.15, 1.0, 0.5],
            alpha: 0.7,
            burst: false,
            spread: 0.95,
            ..base(id.clone())
        },
        _ => return None,
    };
    Some(r)
}

/// FNV-1a — the same "identity from content" discipline the shot ids and animation tracks use.
fn stable_hash(s: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in s.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Something the VFX layer will not do, said in the author's language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VfxError {
    /// No such card.
    UnknownEffect(String),
    /// The object is gone.
    MissingEntity,
    /// The layer stack is full.
    TooMany(String),
    /// The write failed.
    Blocked(String),
}

impl std::fmt::Display for VfxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownEffect(k) => write!(f, "there is no effect called \"{k}\""),
            Self::MissingEntity => write!(f, "that object is no longer in the scene"),
            Self::TooMany(m) | Self::Blocked(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for VfxError {}

/// What a completed VFX command answers with.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VfxReply {
    /// The object whose effects changed.
    pub entity: Option<String>,
    /// How many layers it now has.
    pub layers: usize,
    /// Particles at once, across the whole stack.
    pub particles: u32,
    /// The effect read back as sentences — one line per layer.
    pub reads: Vec<String>,
    /// Warnings in plain language (over budget, all-glow, all-one-shot).
    pub problems: Vec<String>,
    /// Friendly summary.
    pub message: String,
    /// Set iff nothing changed.
    pub reason: Option<String>,
}

impl VfxReply {
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

/// Read an object's effects. Empty when it has none, and empty when the stored JSON is unreadable —
/// a corrupt blob is never trusted into the runtime.
#[must_use]
pub fn stack_of<W: World>(engine: &Engine<W>, entity: EntityId) -> EffectStack {
    read_stack(engine, entity).unwrap_or_default()
}

/// The same read, but able to tell "there is nothing here" from "there is something here I cannot
/// read". The difference matters because every authoring command is a READ-MODIFY-WRITE of the whole
/// blob: treating an unparseable `source` as empty means the next click silently overwrites work the
/// user can still see in the Inspector. `Ok(None)` = no effects; `Err` = present but unreadable.
///
/// # Errors
/// [`VfxError::Blocked`] when a `source` string exists but is not a valid `EffectStack`.
pub fn read_stack_strict<W: World>(
    engine: &Engine<W>,
    entity: EntityId,
) -> Result<EffectStack, VfxError> {
    match engine
        .resolved_components(entity)
        .get(VFX_COMPONENT)
        .and_then(|fields| fields.get("source").cloned())
    {
        Some(FieldValue::Str(json)) if !json.trim().is_empty() => {
            serde_json::from_str::<EffectStack>(&json).map_err(|e| {
                VfxError::Blocked(format!(
                    "this object's effects can't be read ({e}) — fix or clear the Vfx source field \
                     in the Inspector, and nothing here will overwrite it in the meantime"
                ))
            })
        }
        _ => Ok(EffectStack::default()),
    }
}

fn read_stack<W: World>(engine: &Engine<W>, entity: EntityId) -> Option<EffectStack> {
    read_stack_strict(engine, entity).ok()
}

/// One layer, in a sentence.
#[must_use]
pub fn describe_layer(layer: &EffectRecipe) -> String {
    let card = effect_specs()
        .into_iter()
        .find(|s| s.kind == layer.kind)
        .map_or_else(|| layer.kind.clone(), |s| format!("{} {}", s.icon, s.label));
    let when = if layer.burst {
        "one shot".to_string()
    } else {
        format!("{:.1}s per particle", layer.lifetime)
    };
    // The MOMENT is the most important thing about a layer, so it is in the sentence rather than
    // hidden in a dropdown that has since moved on to the next card.
    format!(
        "{card} — {}, {} particles, {when}",
        layer.trigger.label(),
        layer.live_count()
    )
}

/// The ops that add one effect layer to `entity`. The caller commits, so an effect is one undo step.
///
/// # Errors
/// [`VfxError`] — an unknown card, or the layer ceiling reached (refused with the way out).
pub fn add_effect_ops<W: World>(
    engine: &Engine<W>,
    entity: EntityId,
    kind: &str,
    trigger: Trigger,
) -> Result<(Vec<Op>, EffectStack), VfxError> {
    if !engine.entity_exists(entity) {
        return Err(VfxError::MissingEntity);
    }
    let mut stack = read_stack_strict(engine, entity)?;
    if stack.layers.len() >= MAX_LAYERS {
        return Err(VfxError::TooMany(format!(
            "that object already has {MAX_LAYERS} effects — remove one first, or put the next effect on a different object"
        )));
    }
    // The id must be UNIQUE, and it must be the same on every machine that replays this action. A bare
    // hash of the list length is neither: remove layer 0 then add the same card again and the length is
    // back where it started, minting a byte-identical id for a second layer — two indistinguishable
    // effects, superimposed pixel-for-pixel, quietly costing double the particle budget. Probe upward
    // from the length for the first free slot: still a pure function of (kind, subject, current state),
    // so replay agrees, and now collision-free by construction.
    let taken: Vec<&str> = stack.layers.iter().map(|l| l.id.as_str()).collect();
    let mut probe = stack.layers.len();
    let recipe = loop {
        let candidate = recipe_for(kind, entity, probe)
            .ok_or_else(|| VfxError::UnknownEffect(kind.to_string()))?;
        if !taken.contains(&candidate.id.as_str()) {
            break candidate;
        }
        probe += 1;
        if probe > stack.layers.len() + MAX_LAYERS + 8 {
            return Err(VfxError::Blocked(
                "could not give that effect a unique name — remove one and try again".into(),
            ));
        }
    };
    stack.layers.push(EffectRecipe { trigger, ..recipe });
    let json = serde_json::to_string(&stack)
        .map_err(|e| VfxError::Blocked(format!("the effect could not be stored: {e}")))?;

    // `playing` is now ONLY the cue switch for "when a rule says so" layers — an ordinary field a rule
    // flips, which is still why the closed action vocabulary never grew a verb. "The whole time" layers
    // run because their own trigger says so, and moment layers fire from the moment itself. Writing one
    // shared boolean per object is what used to make a second card switch the first one off.
    Ok((
        vec![
            Op::SetField {
                entity,
                component: VFX_COMPONENT.into(),
                field: "source".into(),
                value: FieldValue::Str(json),
            },
            Op::SetField {
                entity,
                component: VFX_COMPONENT.into(),
                field: "playing".into(),
                value: FieldValue::Bool(false),
            },
            Op::SetField {
                entity,
                component: VFX_COMPONENT.into(),
                field: "intensity".into(),
                value: FieldValue::Number(1.0),
            },
        ],
        stack,
    ))
}

/// The ops that remove layer `index`. Removing the last layer clears the component's payload rather
/// than leaving an empty husk that would resurrect on the next read.
///
/// # Errors
/// [`VfxError`] — the object is gone, or there is no such layer.
pub fn remove_effect_ops<W: World>(
    engine: &Engine<W>,
    entity: EntityId,
    index: usize,
) -> Result<(Vec<Op>, EffectStack), VfxError> {
    if !engine.entity_exists(entity) {
        return Err(VfxError::MissingEntity);
    }
    let mut stack = read_stack_strict(engine, entity)?;
    if index >= stack.layers.len() {
        return Err(VfxError::Blocked(
            "that effect is already gone — nothing to remove".into(),
        ));
    }
    stack.layers.remove(index);
    // Removing the LAST layer drops the whole component, rather than leaving `{"layers":[]}` behind.
    // The husk was invisible to this panel (which reads the parsed stack and shows nothing) but fully
    // visible in the Inspector, which projects every attached component — so the user was left with an
    // editable JSON blob and no button that could remove it. It also made `✕` and Ctrl-Z produce
    // different documents for the same outcome, which then differ byte-for-byte in the saved file.
    // Matches what `cinema_intent` and `condition_intent` already do.
    if stack.layers.is_empty() {
        return Ok((
            vec![Op::RemoveComponent {
                entity,
                component: VFX_COMPONENT.into(),
            }],
            stack,
        ));
    }
    let json = serde_json::to_string(&stack)
        .map_err(|e| VfxError::Blocked(format!("the effect could not be stored: {e}")))?;
    Ok((
        vec![Op::SetField {
            entity,
            component: VFX_COMPONENT.into(),
            field: "source".into(),
            value: FieldValue::Str(json),
        }],
        stack,
    ))
}

/// Build the answer for a stack — the sentences, the budget and the warnings in one place, so every
/// command reports the same way.
#[must_use]
pub fn vfx_reply(entity: EntityId, stack: &EffectStack, message: impl Into<String>) -> VfxReply {
    let particles = stack.budget().min(MAX_PARTICLES);
    VfxReply {
        entity: Some(entity.to_loro_key()),
        layers: stack.layers.len(),
        particles,
        reads: stack.layers.iter().map(describe_layer).collect(),
        problems: stack.problems(),
        message: message.into(),
        reason: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capscene::{CapResolver, CapScene};
    use metrocalk_ecs::FlecsWorld;

    fn engine() -> Engine<FlecsWorld> {
        let mut ecs = FlecsWorld::new();
        let scene = CapScene::intern(&mut ecs);
        let mut engine = Engine::new(ecs, 1);
        engine.set_capability_resolver(Box::new(CapResolver::from_scene(&scene)));
        engine
    }

    fn spawn(engine: &mut Engine<FlecsWorld>) -> EntityId {
        let id = engine.alloc_entity_id();
        engine
            .commit("spawn", vec![Op::CreateEntity { id, parent: None }])
            .expect("spawns");
        id
    }

    #[test]
    fn every_card_produces_a_usable_recipe() {
        for spec in effect_specs() {
            let r = recipe_for(spec.kind, EntityId::from_loro_key("1_1").expect("key"), 0)
                .unwrap_or_else(|| panic!("card {} has no recipe", spec.kind));
            assert_eq!(r.burst, spec.burst, "{} burst flag disagrees", spec.kind);
            assert!(r.count > 0 && r.count <= MAX_PARTICLES, "{}", spec.kind);
            assert!(r.lifetime > 0.0, "{}", spec.kind);
            assert!(!r.id.is_empty(), "{}", spec.kind);
        }
    }

    #[test]
    fn the_catalogue_is_broad_and_every_card_is_distinct() {
        // A catalogue of near-duplicates is a long menu, not a wide one. Two cards that resolve to the
        // same numbers are a copy-paste slip in a hand-written table, and nothing else would catch it.
        let specs = effect_specs();
        assert!(specs.len() >= 18, "only {} effect cards", specs.len());

        let owner = EntityId::from_loro_key("1_1").expect("key");
        let mut seen: Vec<(String, EffectRecipe)> = Vec::new();
        for spec in &specs {
            let r = recipe_for(spec.kind, owner, 0)
                .unwrap_or_else(|| panic!("card {} has no recipe", spec.kind));
            for (other, prev) in &seen {
                let same = prev.count == r.count
                    && prev.shape == r.shape
                    && prev.motion == r.motion
                    && prev.look == r.look
                    && (prev.speed - r.speed).abs() < 1.0e-6
                    && (prev.gravity - r.gravity).abs() < 1.0e-6;
                assert!(
                    !same,
                    "\"{}\" and \"{}\" are the same effect",
                    other, spec.kind
                );
            }
            seen.push((spec.kind.to_string(), r));
        }
    }

    #[test]
    fn the_occluding_pipeline_has_real_users() {
        // Every card but two used to be additive, so the alpha-blended pipeline shipped with no
        // coverage: a measurement of a live scene read `soft = 0` every single time.
        let owner = EntityId::from_loro_key("1_1").expect("key");
        let soft: Vec<&str> = effect_specs()
            .iter()
            .filter(|s| recipe_for(s.kind, owner, 0).is_some_and(|r| matches!(r.look, Look::Soft)))
            .map(|s| s.kind)
            .collect();
        assert!(soft.len() >= 5, "only {soft:?} use the soft path");
        assert!(soft.contains(&"smoke"), "smoke must occlude, not glow");
    }

    #[test]
    fn each_card_does_what_its_name_says() {
        use metrocalk_animation::vfx::{particle_at, EmitterSample};
        let owner = EntityId::from_loro_key("1_1").expect("key");
        // Follow ONE particle across ITS OWN life, which is what "fire rises" actually claims.
        //
        // The first version of this test took the MEAN height of everything alive at time t and
        // compared two times. That worked only because continuous streams used to fill in gradually,
        // so an early sample caught nothing but freshly-born particles down at the emitter. Once the
        // streams were pre-warmed the mean stopped moving — a steady-state plume has the same age
        // distribution at every instant, which is the whole point of pre-warming — and the test failed
        // on code that had just got better. Sampling one particle by age measures the actual claim.
        let at = |kind: &str, frac: f32| {
            let r = recipe_for(kind, owner, 0).expect("recipe");
            let e = EmitterSample {
                origin: [0.0; 3],
                radius: 1.0,
            };
            // Particle 0 has stagger 0, so at `frac * lifetime` it is exactly `frac` through its life.
            let t = frac * r.lifetime.max(0.05);
            particle_at(&r, e, 0, t)
                .unwrap_or_else(|| panic!("{kind} particle 0 was not alive at {frac} of its life"))
                .position[1]
        };
        // Things that rise, rise.
        for kind in ["fire", "smoke", "steam", "embers", "aura"] {
            assert!(at(kind, 0.7) > at(kind, 0.05), "{kind} should rise");
        }
        // Things that fall, fall.
        for kind in ["rain", "snow"] {
            assert!(at(kind, 0.7) < at(kind, 0.05), "{kind} should fall");
        }
        // A shockwave is FLAT: it races outward without climbing.
        let flat = (at("shockwave", 0.6) - at("shockwave", 0.05)).abs();
        assert!(flat < 0.35, "a shockwave should stay flat, moved {flat}");
    }

    #[test]
    fn every_one_shot_card_is_marked_as_one_and_ends() {
        use metrocalk_animation::vfx::{particle_at, EmitterSample};
        let owner = EntityId::from_loro_key("1_1").expect("key");
        let e = EmitterSample {
            origin: [0.0; 3],
            radius: 1.0,
        };
        for spec in effect_specs().iter().filter(|s| s.burst) {
            let r = recipe_for(spec.kind, owner, 0).expect("recipe");
            assert!(r.burst, "{} is advertised as one-shot", spec.kind);
            let after = r.total_seconds() + 0.05;
            assert!(
                (0..r.live_count()).all(|i| particle_at(&r, e, i, after).is_none()),
                "{} is still alive past its own tail",
                spec.kind
            );
        }
    }

    #[test]
    fn an_effect_round_trips_through_the_document() {
        let mut e = engine();
        let id = spawn(&mut e);
        let (ops, _) = add_effect_ops(&e, id, "fire", Trigger::Always).expect("adds");
        e.commit("fx", ops).expect("commits");
        let back = stack_of(&e, id);
        assert_eq!(back.layers.len(), 1);
        assert_eq!(back.layers[0].kind, "fire");
    }

    #[test]
    fn the_layer_ceiling_is_refused_with_a_way_out() {
        let mut e = engine();
        let id = spawn(&mut e);
        for _ in 0..MAX_LAYERS {
            let (ops, _) = add_effect_ops(&e, id, "sparkle", Trigger::Always).expect("adds");
            e.commit("fx", ops).expect("commits");
        }
        let err = add_effect_ops(&e, id, "sparkle", Trigger::Always).expect_err("refuses");
        let msg = err.to_string();
        assert!(msg.contains("remove one"), "not actionable: {msg}");
    }

    #[test]
    fn an_unknown_card_is_named_not_silently_ignored() {
        let mut e = engine();
        let id = spawn(&mut e);
        let err = add_effect_ops(&e, id, "banana", Trigger::Always).expect_err("refuses");
        assert!(err.to_string().contains("banana"));
    }

    #[test]
    fn removing_the_last_layer_turns_the_effect_off() {
        let mut e = engine();
        let id = spawn(&mut e);
        let (ops, _) = add_effect_ops(&e, id, "fire", Trigger::Always).expect("adds");
        e.commit("fx", ops).expect("commits");
        let (ops, stack) = remove_effect_ops(&e, id, 0).expect("removes");
        e.commit("fx-rm", ops).expect("commits");
        assert!(stack.layers.is_empty());
        // and it does not resurrect on the next read
        assert!(stack_of(&e, id).layers.is_empty());
    }

    #[test]
    fn a_moment_triggered_effect_does_not_start_with_play() {
        let mut e = engine();
        let id = spawn(&mut e);
        let (ops, _) = add_effect_ops(&e, id, "sparks", Trigger::WhenHit).expect("adds");
        let playing = ops.iter().find_map(|op| match op {
            Op::SetField { field, value, .. } if field == "playing" => Some(value.clone()),
            _ => None,
        });
        assert_eq!(playing, Some(FieldValue::Bool(false)));
    }

    #[test]
    fn a_second_card_does_not_switch_the_first_one_off() {
        // The bug this replaced: `playing` was one boolean per OBJECT, so adding "explosion when hit"
        // to a burning crate set it false and silently put the fire out — and adding the fire first
        // made the explosion detonate at Play start with nothing having touched the crate.
        let mut e = engine();
        let id = spawn(&mut e);
        let (ops, _) = add_effect_ops(&e, id, "fire", Trigger::Always).expect("fire");
        e.commit("fx", ops).expect("commits");
        let (ops, _) = add_effect_ops(&e, id, "explosion", Trigger::WhenHit).expect("boom");
        e.commit("fx", ops).expect("commits");

        let stack = stack_of(&e, id);
        assert_eq!(stack.layers.len(), 2);
        // Each layer keeps its OWN moment.
        assert_eq!(stack.layers[0].trigger, Trigger::Always);
        assert_eq!(stack.layers[1].trigger, Trigger::WhenHit);
    }

    #[test]
    fn remove_then_add_does_not_mint_a_duplicate_id() {
        // Ids were a hash of the list LENGTH, so this exact sequence produced two byte-identical
        // layers rendering pixel-for-pixel on top of each other at double the particle cost.
        let mut e = engine();
        let id = spawn(&mut e);
        for _ in 0..2 {
            let (ops, _) = add_effect_ops(&e, id, "fire", Trigger::Always).expect("adds");
            e.commit("fx", ops).expect("commits");
        }
        let (ops, _) = remove_effect_ops(&e, id, 0).expect("removes");
        e.commit("fx", ops).expect("commits");
        let (ops, _) = add_effect_ops(&e, id, "fire", Trigger::Always).expect("adds again");
        e.commit("fx", ops).expect("commits");

        let stack = stack_of(&e, id);
        assert_eq!(stack.layers.len(), 2);
        assert_ne!(
            stack.layers[0].id, stack.layers[1].id,
            "two layers share an id: {:?}",
            stack.layers[0].id
        );
    }

    #[test]
    fn two_objects_do_not_share_a_layer_id() {
        let mut e = engine();
        let a = spawn(&mut e);
        let b = spawn(&mut e);
        for id in [a, b] {
            let (ops, _) = add_effect_ops(&e, id, "fire", Trigger::Always).expect("adds");
            e.commit("fx", ops).expect("commits");
        }
        assert_ne!(stack_of(&e, a).layers[0].id, stack_of(&e, b).layers[0].id);
    }

    #[test]
    fn an_unreadable_source_is_refused_rather_than_overwritten() {
        // A read-modify-write over a value that silently defaults on a parse failure DESTROYS the
        // original. `source` is an ordinary editable string field, so this is reachable by hand.
        let mut e = engine();
        let id = spawn(&mut e);
        e.commit(
            "corrupt",
            vec![Op::SetField {
                entity: id,
                component: VFX_COMPONENT.into(),
                field: "source".into(),
                value: FieldValue::Str("{ not json".into()),
            }],
        )
        .expect("commits");
        let err = add_effect_ops(&e, id, "fire", Trigger::Always).expect_err("refuses");
        let msg = err.to_string();
        assert!(msg.contains("can't be read"), "not actionable: {msg}");
        assert!(msg.contains("Inspector"), "no way out offered: {msg}");
    }

    #[test]
    fn removing_the_last_layer_leaves_no_husk_behind() {
        let mut e = engine();
        let id = spawn(&mut e);
        let (ops, _) = add_effect_ops(&e, id, "fire", Trigger::Always).expect("adds");
        e.commit("fx", ops).expect("commits");
        let (ops, _) = remove_effect_ops(&e, id, 0).expect("removes");
        e.commit("fx-rm", ops).expect("commits");
        // Not merely "reads as empty" — the component itself is gone, so the Inspector does not show
        // an editable JSON blob the panel offers no way to delete.
        assert!(
            !e.resolved_components(id).contains_key(VFX_COMPONENT),
            "an empty Vfx husk survived the last removal"
        );
    }

    #[test]
    fn every_layer_reads_back_as_a_sentence() {
        let mut e = engine();
        let id = spawn(&mut e);
        for kind in ["fire", "smoke"] {
            let (ops, _) = add_effect_ops(&e, id, kind, Trigger::Always).expect("adds");
            e.commit("fx", ops).expect("commits");
        }
        let reply = vfx_reply(id, &stack_of(&e, id), "done");
        assert_eq!(reply.reads.len(), 2);
        assert!(reply.reads[0].contains("Fire"), "{:?}", reply.reads);
        assert!(reply.reads[1].contains("Smoke"), "{:?}", reply.reads);
        assert!(reply.particles > 0);
    }
}
