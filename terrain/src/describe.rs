//! Describe-to-build: a sentence in, a full terrain recipe out.
//!
//! ## Why it is built this way
//!
//! The published work on text-driven terrain splits cleanly in two. Diffusion-style systems (MESA, 2025)
//! learn text → elevation *pixels*; they produce a beautiful static sample and, as that paper says itself,
//! are "not designed for interactive terrain authoring" — the output is a finished heightmap you cannot take
//! apart. The other family (LandCraft, AAAI 2026) goes text → a **structured intermediate representation** of
//! the landscape → a procedural module that synthesises it. The second is the one that leaves you with
//! something editable, and it is the only one that fits this engine at all: a terrain here is already a
//! `TerrainRecipe`, so "describe it" means *write the recipe*, and every knob stays where the author can
//! reach it afterwards.
//!
//! So this module is the text → structured-brief → recipe path, and the structured brief is a [`Brief`]:
//! landform, relief, scale, climate, water, vegetation and a handful of named features. Nothing here is a
//! neural network, nothing calls out to a service, and the same sentence produces byte-identical output on
//! every machine — the determinism invariant the rest of the terrain system depends on is not weakened by
//! adding a friendlier front door.
//!
//! ## Honest about what it read
//!
//! A generator that silently ignores half of what you typed is worse than one that asks. [`read`] therefore
//! returns not just the brief but **what it understood and which of your words it did not use**, each tied to
//! the phrase that triggered it. The panel shows both, so "why did I not get a river?" has an answer on
//! screen instead of requiring a guess. This is the same rule the rest of the editor follows: state the cost,
//! and never fail silently.
//!
//! ## The LLM seam
//!
//! [`Brief`] is `serde`, and [`build`] takes nothing else. A model that emits a `Brief` — from a much looser
//! sentence than the lexicon here can parse — drops straight in without touching the synthesis half, and the
//! result stays deterministic because the brief, not the prose, is what generates the world. That is the
//! upgrade path; it is not needed for this to be useful.

use crate::preset;
use crate::recipe::{
    BiomeRule, Blend, ErosionSettings, Layer, LayerKind, MaterialLayer, ScatterProto, ScatterRule,
    SplineDef, SplineKind, TerrainRecipe, WaterConfig,
};
use serde::{Deserialize, Serialize};

/// The broad shape of the land — the one thing a description must imply, because everything else is a
/// modifier on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Landform {
    /// Level ground, near enough to build on.
    Plains,
    /// Warped, rounded hills.
    Hills,
    /// Ridged, eroded mountains.
    Mountains,
    /// Wind-blown dune fields over a basin.
    Dunes,
    /// Land in a sea.
    Islands,
    /// A high plateau cut by terraced gorges.
    Canyon,
}

impl Landform {
    /// The preset this landform starts from. Starting from a tuned preset rather than from nothing is what
    /// makes a described terrain look as good as a picked one.
    fn preset_id(self) -> &'static str {
        match self {
            Self::Plains => "flat",
            Self::Hills => "rolling-hills",
            Self::Mountains => "alpine",
            Self::Dunes => "dunes",
            Self::Islands => "archipelago",
            Self::Canyon => "canyon",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Plains => "plains",
            Self::Hills => "hills",
            Self::Mountains => "mountains",
            Self::Dunes => "dunes",
            Self::Islands => "islands in a sea",
            Self::Canyon => "canyon and mesa",
        }
    }
}

/// Climate decides the palette and which species grow — the two things that make the same landform read as
/// a different place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Climate {
    /// Green, damp, broadleaf.
    Temperate,
    /// Dry, pale, sparse scrub.
    Arid,
    /// Cold, grey rock and snow, conifers.
    Alpine,
    /// Lush, saturated, palms.
    Tropical,
    /// White, near-barren.
    Arctic,
    /// Olive and stone, dry grass.
    Mediterranean,
}

/// Colours for the roles a material can play. Applying a palette by *role* rather than by index is what lets
/// a climate repaint any preset without breaking the biome rules that reference materials by position.
struct Palette {
    grass: [f32; 3],
    dry: [f32; 3],
    rock: [f32; 3],
    sand: [f32; 3],
    snow: [f32; 3],
    soil: [f32; 3],
    /// Canopy species name; drives both the label and the generated stand-in mesh's shape.
    tree: &'static str,
    /// Understorey species name.
    shrub: &'static str,
    /// Baseline vegetation multiplier for this climate.
    lushness: f32,
}

impl Climate {
    fn palette(self) -> Palette {
        match self {
            Self::Temperate => Palette {
                grass: [0.21, 0.33, 0.14],
                dry: [0.42, 0.41, 0.22],
                rock: [0.37, 0.36, 0.34],
                sand: [0.62, 0.56, 0.42],
                snow: [0.86, 0.89, 0.94],
                soil: [0.28, 0.23, 0.17],
                tree: "Broadleaf Trees",
                shrub: "Shrubs",
                lushness: 1.0,
            },
            Self::Arid => Palette {
                grass: [0.44, 0.42, 0.24],
                dry: [0.58, 0.50, 0.31],
                rock: [0.47, 0.37, 0.29],
                sand: [0.76, 0.65, 0.44],
                snow: [0.88, 0.86, 0.80],
                soil: [0.44, 0.33, 0.24],
                tree: "Desert Scrub",
                shrub: "Desert Scrub",
                lushness: 0.28,
            },
            Self::Alpine => Palette {
                grass: [0.23, 0.30, 0.17],
                dry: [0.40, 0.39, 0.31],
                rock: [0.33, 0.33, 0.32],
                sand: [0.52, 0.52, 0.50],
                snow: [0.88, 0.91, 0.95],
                soil: [0.26, 0.24, 0.21],
                tree: "Conifers",
                shrub: "Alpine Shrubs",
                lushness: 0.8,
            },
            Self::Tropical => Palette {
                grass: [0.16, 0.36, 0.13],
                dry: [0.36, 0.40, 0.18],
                rock: [0.34, 0.34, 0.30],
                sand: [0.83, 0.77, 0.58],
                snow: [0.90, 0.92, 0.92],
                soil: [0.24, 0.19, 0.13],
                tree: "Palms",
                shrub: "Undergrowth",
                lushness: 1.8,
            },
            Self::Arctic => Palette {
                grass: [0.40, 0.44, 0.42],
                dry: [0.52, 0.54, 0.53],
                rock: [0.36, 0.37, 0.39],
                sand: [0.66, 0.69, 0.72],
                snow: [0.92, 0.95, 0.98],
                soil: [0.34, 0.35, 0.36],
                tree: "Conifers",
                shrub: "Arctic Shrubs",
                lushness: 0.18,
            },
            Self::Mediterranean => Palette {
                grass: [0.33, 0.36, 0.19],
                dry: [0.54, 0.48, 0.28],
                rock: [0.53, 0.49, 0.42],
                sand: [0.72, 0.64, 0.47],
                snow: [0.88, 0.89, 0.90],
                soil: [0.38, 0.29, 0.20],
                tree: "Olive Trees",
                shrub: "Maquis Shrubs",
                lushness: 0.7,
            },
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Temperate => "temperate",
            Self::Arid => "arid",
            Self::Alpine => "alpine",
            Self::Tropical => "tropical",
            Self::Arctic => "arctic",
            Self::Mediterranean => "mediterranean",
        }
    }
}

/// What the description asked for in the way of water.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaterWish {
    /// No water at all.
    Dry,
    /// Standing water in the hollows only.
    Lakes,
    /// A coastline: the sea comes well up the land.
    Sea,
    /// Mostly water, with land breaking the surface.
    Flooded,
}

/// The structured reading of a description — the whole input to [`build`].
///
/// Deliberately small and boring: six decisions and four flags. A brief that could express everything a
/// recipe can would just be the recipe, and then describing a terrain would be as much work as authoring it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Brief {
    /// The broad shape.
    pub landform: Landform,
    /// The palette and species.
    pub climate: Climate,
    /// Multiplier on every layer's amplitude. `1.0` keeps the landform's own relief.
    pub relief: f32,
    /// World extent in metres, when the description gave one.
    pub world_size_m: Option<f32>,
    /// What water to put in.
    pub water: WaterWish,
    /// Multiplier on scatter density. `0.0` is barren.
    pub vegetation: f32,
    /// Run the erosion bake. `None` keeps whatever the landform does by default.
    pub erosion: Option<bool>,
    /// Add sedimentary benches.
    pub terraces: bool,
    /// Carve a river across the world.
    pub river: bool,
    /// Grade a road across the world.
    pub road: bool,
    /// Name for the terrain.
    pub name: String,
    /// Seed, so the same sentence can give a different world on demand.
    pub seed: u64,
}

impl Default for Brief {
    fn default() -> Self {
        Self {
            landform: Landform::Hills,
            climate: Climate::Temperate,
            relief: 1.0,
            world_size_m: None,
            water: WaterWish::Lakes,
            vegetation: 1.0,
            erosion: None,
            terraces: false,
            river: false,
            road: false,
            name: "Described Terrain".into(),
            seed: 1,
        }
    }
}

/// One thing the reader took from the text, and the words that made it do so.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Understood {
    /// The phrase in the description that matched.
    pub phrase: String,
    /// What it was taken to mean, in the author's language.
    pub meaning: String,
}

/// The result of reading a description: the brief, what was understood, and what was left over.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    /// The structured brief, ready for [`build`].
    pub brief: Brief,
    /// Every intent taken from the text, in the order the phrases appear.
    pub understood: Vec<Understood>,
    /// Words that carried no meaning here. Shown to the author rather than discarded, so an unsupported
    /// request looks like an unsupported request instead of like a bug.
    pub unused: Vec<String>,
}

/// What a phrase means.
#[derive(Clone, Copy)]
enum Sense {
    Form(Landform),
    Clim(Climate),
    Relief(f32),
    Water(WaterWish),
    /// Sets how much grows here: "forested", "barren".
    Veg(f32),
    /// Scales whatever grows here: "sparse", "densely". Multiplied in, so "sparse woodland" is woodland at
    /// low density rather than the two words fighting over one slot — which is what a person means by it.
    VegScale(f32),
    /// A named species. It implies a climate, but only as a HINT: an explicit climate word always wins, so
    /// "arctic pines" is arctic. Naming a species must still count as understood — the live run showed
    /// "conifer" and "palms" being reported as unused words while the climate silently produced exactly
    /// those, which is the most confusing possible answer.
    Species(Climate),
    Erode(bool),
    Terraces,
    River,
    Road,
    /// A qualitative size, in metres.
    Size(f32),
}

/// The lexicon. Longest phrase wins, so "dry river bed" cannot be read as "river".
///
/// Ordinary English for landscapes, not a DSL: the author should be able to type the sentence they would say
/// to a colleague. Where a word implies two things it appears once with the stronger sense and the weaker one
/// is left to the landform default — "desert" means arid, and the dune *shape* only if nothing else said
/// otherwise, which is handled in [`read`].
const LEXICON: &[(&str, Sense)] = &[
    // ── landform ──────────────────────────────────────────────────────────────────────────────────────
    ("rolling hills", Sense::Form(Landform::Hills)),
    ("gentle hills", Sense::Form(Landform::Hills)),
    ("foothills", Sense::Form(Landform::Hills)),
    ("hills", Sense::Form(Landform::Hills)),
    ("hilly", Sense::Form(Landform::Hills)),
    ("hillside", Sense::Form(Landform::Hills)),
    ("mountain range", Sense::Form(Landform::Mountains)),
    ("mountains", Sense::Form(Landform::Mountains)),
    ("mountainous", Sense::Form(Landform::Mountains)),
    ("mountain", Sense::Form(Landform::Mountains)),
    ("peaks", Sense::Form(Landform::Mountains)),
    ("summits", Sense::Form(Landform::Mountains)),
    ("ridges", Sense::Form(Landform::Mountains)),
    ("massif", Sense::Form(Landform::Mountains)),
    ("sand dunes", Sense::Form(Landform::Dunes)),
    ("dune field", Sense::Form(Landform::Dunes)),
    ("dunes", Sense::Form(Landform::Dunes)),
    ("erg", Sense::Form(Landform::Dunes)),
    ("archipelago", Sense::Form(Landform::Islands)),
    ("islands", Sense::Form(Landform::Islands)),
    ("island", Sense::Form(Landform::Islands)),
    ("atoll", Sense::Form(Landform::Islands)),
    ("coastline", Sense::Form(Landform::Islands)),
    ("coastal", Sense::Form(Landform::Islands)),
    ("coast", Sense::Form(Landform::Islands)),
    ("canyon", Sense::Form(Landform::Canyon)),
    ("canyons", Sense::Form(Landform::Canyon)),
    ("gorge", Sense::Form(Landform::Canyon)),
    ("mesa", Sense::Form(Landform::Canyon)),
    ("badlands", Sense::Form(Landform::Canyon)),
    ("plateau", Sense::Form(Landform::Canyon)),
    ("plains", Sense::Form(Landform::Plains)),
    ("prairie", Sense::Form(Landform::Plains)),
    ("steppe", Sense::Form(Landform::Plains)),
    ("meadow", Sense::Form(Landform::Plains)),
    ("floodplain", Sense::Form(Landform::Plains)),
    // ── climate ───────────────────────────────────────────────────────────────────────────────────────
    ("temperate", Sense::Clim(Climate::Temperate)),
    ("alpine", Sense::Clim(Climate::Alpine)),
    ("subalpine", Sense::Clim(Climate::Alpine)),
    ("desert", Sense::Clim(Climate::Arid)),
    ("arid", Sense::Clim(Climate::Arid)),
    ("dry", Sense::Clim(Climate::Arid)),
    ("parched", Sense::Clim(Climate::Arid)),
    ("tropical", Sense::Clim(Climate::Tropical)),
    ("jungle", Sense::Clim(Climate::Tropical)),
    ("rainforest", Sense::Clim(Climate::Tropical)),
    ("arctic", Sense::Clim(Climate::Arctic)),
    ("polar", Sense::Clim(Climate::Arctic)),
    ("tundra", Sense::Clim(Climate::Arctic)),
    ("frozen", Sense::Clim(Climate::Arctic)),
    ("snowy", Sense::Clim(Climate::Arctic)),
    ("mediterranean", Sense::Clim(Climate::Mediterranean)),
    // ── relief ────────────────────────────────────────────────────────────────────────────────────────
    ("perfectly flat", Sense::Relief(0.02)),
    ("dead flat", Sense::Relief(0.02)),
    ("flat", Sense::Relief(0.08)),
    ("level", Sense::Relief(0.08)),
    ("almost flat", Sense::Relief(0.2)),
    ("gently rolling", Sense::Relief(0.5)),
    ("gentle", Sense::Relief(0.55)),
    ("soft", Sense::Relief(0.6)),
    ("shallow", Sense::Relief(0.6)),
    ("low", Sense::Relief(0.6)),
    ("moderate", Sense::Relief(1.0)),
    ("steep", Sense::Relief(1.5)),
    ("rugged", Sense::Relief(1.6)),
    ("craggy", Sense::Relief(1.7)),
    ("jagged", Sense::Relief(1.8)),
    ("dramatic", Sense::Relief(1.8)),
    ("towering", Sense::Relief(2.2)),
    ("enormous", Sense::Relief(2.2)),
    ("colossal", Sense::Relief(2.6)),
    // ── water ─────────────────────────────────────────────────────────────────────────────────────────
    ("no water", Sense::Water(WaterWish::Dry)),
    ("waterless", Sense::Water(WaterWish::Dry)),
    ("bone dry", Sense::Water(WaterWish::Dry)),
    ("lakes", Sense::Water(WaterWish::Lakes)),
    ("lake", Sense::Water(WaterWish::Lakes)),
    ("tarns", Sense::Water(WaterWish::Lakes)),
    ("ponds", Sense::Water(WaterWish::Lakes)),
    ("sea", Sense::Water(WaterWish::Sea)),
    ("ocean", Sense::Water(WaterWish::Sea)),
    ("beaches", Sense::Water(WaterWish::Sea)),
    ("beach", Sense::Water(WaterWish::Sea)),
    ("shoreline", Sense::Water(WaterWish::Sea)),
    ("lagoon", Sense::Water(WaterWish::Sea)),
    ("flooded", Sense::Water(WaterWish::Flooded)),
    ("submerged", Sense::Water(WaterWish::Flooded)),
    ("drowned", Sense::Water(WaterWish::Flooded)),
    // ── vegetation ────────────────────────────────────────────────────────────────────────────────────
    // Species: what grows there implies where it is.
    ("palm trees", Sense::Species(Climate::Tropical)),
    ("palms", Sense::Species(Climate::Tropical)),
    ("palm", Sense::Species(Climate::Tropical)),
    ("mangroves", Sense::Species(Climate::Tropical)),
    ("conifers", Sense::Species(Climate::Alpine)),
    ("conifer", Sense::Species(Climate::Alpine)),
    ("pines", Sense::Species(Climate::Alpine)),
    ("pine", Sense::Species(Climate::Alpine)),
    ("spruce", Sense::Species(Climate::Alpine)),
    ("fir", Sense::Species(Climate::Alpine)),
    ("oaks", Sense::Species(Climate::Temperate)),
    ("oak", Sense::Species(Climate::Temperate)),
    ("beech", Sense::Species(Climate::Temperate)),
    ("birch", Sense::Species(Climate::Temperate)),
    ("broadleaf", Sense::Species(Climate::Temperate)),
    ("cacti", Sense::Species(Climate::Arid)),
    ("cactus", Sense::Species(Climate::Arid)),
    ("saguaro", Sense::Species(Climate::Arid)),
    ("olives", Sense::Species(Climate::Mediterranean)),
    ("olive", Sense::Species(Climate::Mediterranean)),
    ("cypress", Sense::Species(Climate::Mediterranean)),
    ("lichen", Sense::Species(Climate::Arctic)),
    ("moss", Sense::Species(Climate::Arctic)),
    ("barren", Sense::Veg(0.0)),
    ("bare", Sense::Veg(0.0)),
    ("lifeless", Sense::Veg(0.0)),
    ("no trees", Sense::Veg(0.0)),
    ("no vegetation", Sense::Veg(0.0)),
    ("wooded", Sense::Veg(1.3)),
    ("forested", Sense::Veg(1.6)),
    ("forest", Sense::Veg(1.6)),
    ("forests", Sense::Veg(1.6)),
    ("woodland", Sense::Veg(1.4)),
    ("jungle canopy", Sense::Veg(2.2)),
    ("lush", Sense::Veg(2.0)),
    ("grassland", Sense::Veg(0.9)),
    ("sparse", Sense::VegScale(0.3)),
    ("sparsely", Sense::VegScale(0.3)),
    ("scattered", Sense::VegScale(0.5)),
    ("patchy", Sense::VegScale(0.55)),
    ("dense", Sense::VegScale(1.6)),
    ("densely", Sense::VegScale(1.6)),
    ("thick", Sense::VegScale(1.6)),
    ("thickly", Sense::VegScale(1.6)),
    ("heavily wooded", Sense::VegScale(1.8)),
    ("overgrown", Sense::VegScale(1.9)),
    // ── features ──────────────────────────────────────────────────────────────────────────────────────
    ("heavily eroded", Sense::Erode(true)),
    ("eroded", Sense::Erode(true)),
    ("weathered", Sense::Erode(true)),
    ("water carved", Sense::Erode(true)),
    ("pristine", Sense::Erode(false)),
    ("smooth", Sense::Erode(false)),
    ("terraced", Sense::Terraces),
    ("terraces", Sense::Terraces),
    ("stepped", Sense::Terraces),
    ("layered rock", Sense::Terraces),
    ("sedimentary", Sense::Terraces),
    ("river", Sense::River),
    ("stream", Sense::River),
    ("creek", Sense::River),
    ("valley floor", Sense::River),
    ("road", Sense::Road),
    ("track", Sense::Road),
    ("highway", Sense::Road),
    ("trail", Sense::Road),
    // ── qualitative size ──────────────────────────────────────────────────────────────────────────────
    ("tiny", Sense::Size(512.0)),
    ("small", Sense::Size(1024.0)),
    ("medium", Sense::Size(2048.0)),
    ("large", Sense::Size(4096.0)),
    ("huge", Sense::Size(6144.0)),
    ("vast", Sense::Size(8192.0)),
    ("enormous world", Sense::Size(8192.0)),
];

/// Words that mean nothing here and should not be reported as unused — reporting "a", "with" and "the" as
/// unrecognised would bury the one word that actually was.
const STOPWORDS: &[&str] = &[
    "a",
    "an",
    "the",
    "and",
    "or",
    "with",
    "of",
    "in",
    "on",
    "at",
    "to",
    "for",
    "some",
    "few",
    "very",
    "quite",
    "rather",
    "really",
    "world",
    "terrain",
    "landscape",
    "scene",
    "map",
    "area",
    "make",
    "create",
    "build",
    "generate",
    "give",
    "me",
    "i",
    "want",
    "need",
    "please",
    "it",
    "its",
    "that",
    "this",
    "there",
    "is",
    "are",
    "be",
    "with",
    "across",
    "around",
    "over",
    "under",
    "up",
    "down",
    "km",
    "m",
    "metres",
    "meters",
    "kilometre",
    "kilometres",
    "kilometer",
    "kilometers",
    "wide",
    "square",
    "big",
    "little",
    "plus",
    "also",
    "then",
    "but",
    "not",
    "have",
    "has",
    "kind",
    "sort",
    "like",
    "looks",
    "looking",
    "feel",
    "style",
    "lots",
    "many",
    "much",
    "more",
    "less",
];

/// Split into lowercase word tokens, keeping numbers (with decimal points) whole.
fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric()
            || (ch == '.' && !cur.is_empty() && cur.ends_with(|c: char| c.is_ascii_digit()))
        {
            cur.push(ch.to_ascii_lowercase());
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    // A trailing "." on a number is punctuation, not a decimal point.
    for t in &mut out {
        if t.ends_with('.') {
            t.pop();
        }
    }
    // Split a glued number and unit — "2km" is how people actually type it, and leaving it as one token
    // would make the extent silently unreadable.
    let mut split = Vec::with_capacity(out.len());
    for t in out {
        let digits = t
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .filter(|&i| i > 0 && t[..i].parse::<f32>().is_ok());
        if let Some(i) = digits {
            split.push(t[..i].to_string());
            split.push(t[i..].to_string());
        } else {
            split.push(t);
        }
    }
    split
}

/// Read a numeric extent like "2 km", "1.5km", "800 m", "4000".
///
/// Returns the metres and how many tokens it consumed. Bare numbers are metres unless they are small enough
/// that metres would be absurd — "a 4 km world" and "a 4000 m world" should both work, and so should "4".
fn read_size(tokens: &[String], i: usize) -> Option<(f32, usize)> {
    let n: f32 = tokens.get(i)?.parse().ok()?;
    if !n.is_finite() || n <= 0.0 {
        return None;
    }
    let unit = tokens.get(i + 1).map_or("", String::as_str);
    let (metres, used) = match unit {
        "km" | "kilometre" | "kilometres" | "kilometer" | "kilometers" => (n * 1000.0, 2),
        "m" | "metre" | "metres" | "meter" | "meters" => (n, 2),
        // A bare number under 64 cannot be a useful extent in metres, so it is kilometres.
        _ if n < 64.0 => (n * 1000.0, 1),
        _ => (n, 1),
    };
    Some((metres.clamp(256.0, 16_384.0), used))
}

/// Read a description into a [`Brief`], reporting what was understood and what was not.
///
/// Order-independent and case-insensitive. Later phrases win over earlier ones for the same slot, which is
/// what "hills, actually mountains" should do.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn read(text: &str) -> Reading {
    let tokens = tokenize(text);
    let mut used = vec![false; tokens.len()];
    let mut brief = Brief::default();
    let mut understood: Vec<Understood> = Vec::new();
    // Which slots the text spoke to, so defaults can be distinguished from decisions.
    let (mut said_form, mut said_clim, mut said_water, mut said_veg, mut said_relief) =
        (false, false, false, false, false);
    // Accumulated separately from the base so the result does not depend on which word came first.
    let mut veg_scale = 1.0f32;
    // A climate suggested by a named species, applied only if nothing stated one outright.
    let mut species: Option<Climate> = None;

    // Longest phrase first, so a two-word phrase is never shadowed by one of its words.
    let mut lex: Vec<&(&str, Sense)> = LEXICON.iter().collect();
    lex.sort_by_key(|(p, _)| std::cmp::Reverse(p.split(' ').count()));

    for i in 0..tokens.len() {
        if used[i] {
            continue;
        }
        // A numeric extent, before the lexicon: "2 km" is two tokens that mean one thing.
        if let Some((metres, n)) = read_size(&tokens, i) {
            brief.world_size_m = Some(metres);
            understood.push(Understood {
                phrase: tokens[i..(i + n).min(tokens.len())].join(" "),
                meaning: format!("{metres:.0} m across"),
            });
            used[i..(i + n).min(tokens.len())].fill(true);
            continue;
        }
        for (phrase, sense) in &lex {
            let words: Vec<&str> = phrase.split(' ').collect();
            if i + words.len() > tokens.len() {
                continue;
            }
            if !words.iter().enumerate().all(|(k, w)| tokens[i + k] == *w) {
                continue;
            }
            if (i..i + words.len()).any(|k| used[k]) {
                continue;
            }
            let meaning = match *sense {
                Sense::Form(f) => {
                    brief.landform = f;
                    said_form = true;
                    format!("landform: {}", f.label())
                }
                Sense::Clim(c) => {
                    brief.climate = c;
                    said_clim = true;
                    format!("climate: {}", c.label())
                }
                Sense::Relief(r) => {
                    brief.relief = r;
                    said_relief = true;
                    format!("relief ×{r:.2}")
                }
                Sense::Water(w) => {
                    brief.water = w;
                    said_water = true;
                    match w {
                        WaterWish::Dry => "no water".into(),
                        WaterWish::Lakes => "water in the hollows".into(),
                        WaterWish::Sea => "a sea with a shoreline".into(),
                        WaterWish::Flooded => "mostly water".into(),
                    }
                }
                Sense::Veg(v) => {
                    brief.vegetation = v;
                    said_veg = true;
                    if v == 0.0 {
                        "no vegetation".into()
                    } else {
                        format!("vegetation ×{v:.2}")
                    }
                }
                Sense::VegScale(v) => {
                    veg_scale *= v;
                    format!("density ×{v:.2}")
                }
                Sense::Species(c) => {
                    species = Some(c);
                    // "an alpine climate", not "a alpine climate" — the reading is read by a person.
                    let article = if c.label().starts_with(['a', 'e', 'i', 'o', 'u']) {
                        "an"
                    } else {
                        "a"
                    };
                    format!("species suggest {article} {} climate", c.label())
                }
                Sense::Erode(e) => {
                    brief.erosion = Some(e);
                    if e { "erosion on" } else { "erosion off" }.into()
                }
                Sense::Terraces => {
                    brief.terraces = true;
                    "sedimentary benches".into()
                }
                Sense::River => {
                    brief.river = true;
                    "a river".into()
                }
                Sense::Road => {
                    brief.road = true;
                    "a road".into()
                }
                Sense::Size(m) => {
                    brief.world_size_m = Some(m);
                    format!("{m:.0} m across")
                }
            };
            understood.push(Understood {
                phrase: (*phrase).to_string(),
                meaning,
            });
            used[i..i + words.len()].fill(true);
            break;
        }
    }

    // ── implications between slots ────────────────────────────────────────────────────────────────────
    // A named species sets the climate only when none was stated: "arctic pines" stays arctic.
    if !said_clim {
        if let Some(c) = species {
            brief.climate = c;
            said_clim = true;
        }
    }
    // Said a climate but no shape? The climate implies one, which is what "a desert" should give you.
    if !said_form && said_clim {
        brief.landform = match brief.climate {
            Climate::Arid => Landform::Dunes,
            Climate::Alpine | Climate::Arctic => Landform::Mountains,
            Climate::Tropical => Landform::Islands,
            Climate::Temperate | Climate::Mediterranean => Landform::Hills,
        };
    }
    // Said a shape but no climate? The shape suggests one.
    if !said_clim && said_form {
        brief.climate = match brief.landform {
            Landform::Dunes | Landform::Canyon => Climate::Arid,
            Landform::Mountains => Climate::Alpine,
            Landform::Islands => Climate::Tropical,
            Landform::Hills | Landform::Plains => Climate::Temperate,
        };
    }
    // Water follows the landform unless the text said otherwise: islands need a sea, dunes and canyons are
    // dry, everything else gets water only where the ground already dips below it.
    if !said_water {
        brief.water = match brief.landform {
            Landform::Islands => WaterWish::Sea,
            Landform::Dunes => WaterWish::Dry,
            _ => WaterWish::Lakes,
        };
    }
    // Vegetation defaults to what the climate supports rather than to "some".
    if !said_veg {
        brief.vegetation = brief.climate.palette().lushness;
    }
    // "Barren" is absolute: a density word must not be able to bring plants back to a world that said it
    // has none.
    if brief.vegetation > 0.0 {
        brief.vegetation = (brief.vegetation * veg_scale).clamp(0.02, 4.0);
    }
    // "Flat" said of plains is the shape, not a further flattening.
    if said_relief && brief.landform == Landform::Plains && brief.relief < 0.2 {
        brief.relief = 0.0;
    }

    brief.name = name_for(&brief);
    brief.seed = seed_from(text);

    let unused = tokens
        .iter()
        .enumerate()
        .filter(|(i, t)| !used[*i] && !STOPWORDS.contains(&t.as_str()) && t.parse::<f32>().is_err())
        .map(|(_, t)| t.clone())
        .collect::<Vec<_>>();

    Reading {
        brief,
        understood,
        unused,
    }
}

/// A readable name, so the outliner does not fill with "Described Terrain (3)".
fn name_for(b: &Brief) -> String {
    let form = match b.landform {
        Landform::Plains => "Plains",
        Landform::Hills => "Hills",
        Landform::Mountains => "Peaks",
        Landform::Dunes => "Dunes",
        Landform::Islands => "Isles",
        Landform::Canyon => "Canyon",
    };
    let clim = match b.climate {
        Climate::Temperate => "Temperate",
        Climate::Arid => "Desert",
        Climate::Alpine => "Alpine",
        Climate::Tropical => "Tropical",
        Climate::Arctic => "Arctic",
        Climate::Mediterranean => "Mediterranean",
    };
    format!("{clim} {form}")
}

/// A seed derived from the text, so the same sentence always gives the same world and a different sentence
/// gives a different one. FNV-1a — no dependency, and stable across builds and machines.
fn seed_from(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.trim().to_ascii_lowercase().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    // Zero is a legal seed but reads like "unset" in the UI.
    h % 1_000_000 + 1
}

/// Multiply a layer's contribution to the height. Terraces and curves have no amplitude to scale — they
/// reshape what is already there — so they are left alone rather than silently mangled.
fn scale_amplitude(layer: &mut Layer, f: f32) {
    match &mut layer.kind {
        LayerKind::Constant { height } => *height *= f,
        LayerKind::Fbm { amplitude, .. }
        | LayerKind::Ridged { amplitude, .. }
        | LayerKind::Billow { amplitude, .. }
        | LayerKind::Cellular { amplitude, .. }
        | LayerKind::Heightmap { amplitude, .. } => *amplitude *= f,
        LayerKind::Erosion(e) => e.amplitude *= f,
        LayerKind::Terrace { .. } | LayerKind::Curve { .. } => {}
    }
}

/// Which palette role a material's name plays. Name-based, because that is the only thing shared across
/// presets — and a material called "Beach Sand" wants the sand colour whatever index it happens to sit at.
fn role_colour(name: &str, p: &Palette) -> Option<([f32; 3], f32)> {
    let n = name.to_ascii_lowercase();
    if n.contains("snow") || n.contains("ice") {
        Some((p.snow, 0.45))
    } else if n.contains("sand") {
        Some((p.sand, 0.93))
    } else if n.contains("dry") || n.contains("scree") || n.contains("strata") || n.contains("silt")
    {
        Some((p.dry, 0.86))
    } else if n.contains("rock") || n.contains("cliff") || n.contains("gravel") {
        Some((p.rock, 0.74))
    } else if n.contains("grass") || n.contains("meadow") {
        Some((p.grass, 0.86))
    } else if n.contains("soil") || n.contains("dirt") || n.contains("floor") || n.contains("bed") {
        Some((p.soil, 0.90))
    } else if n.contains("ground") {
        Some((p.grass, 0.88))
    } else {
        None
    }
}

/// Build the recipe a brief describes.
///
/// Starts from the landform's preset and applies the brief as modifiers, which is why a described terrain has
/// the same tuned biome bands and scatter rules a picked one does. Nothing here is hidden: the result is an
/// ordinary recipe whose every layer, material and rule is visible in the panel afterwards.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn build(brief: &Brief) -> TerrainRecipe {
    let mut r = preset::by_id(brief.landform.preset_id())
        .unwrap_or_else(|| preset::by_id("rolling-hills").expect("built-in preset"));
    r.name.clone_from(&brief.name);
    r.seed = brief.seed;

    if let Some(size) = brief.world_size_m {
        // Snap to a whole number of chunks: a world that is not a chunk multiple has a ragged last row.
        let chunks = (size / r.chunk_size_m).round().max(4.0);
        r.world_size_m = chunks * r.chunk_size_m;
    }

    // ── relief ────────────────────────────────────────────────────────────────────────────────────────
    if (brief.relief - 1.0).abs() > f32::EPSILON {
        for layer in &mut r.layers {
            // The base plate sets where the ground *is*; scaling it would sink the world rather than flatten
            // it. Only the layers that add shape are scaled.
            let is_base = matches!(layer.blend, Blend::Replace);
            if !is_base {
                scale_amplitude(layer, brief.relief);
            }
        }
    }

    // ── erosion ───────────────────────────────────────────────────────────────────────────────────────
    match brief.erosion {
        Some(true)
            if !r
                .layers
                .iter()
                .any(|l| matches!(l.kind, LayerKind::Erosion(_))) =>
        {
            r.layers.push(Layer::new(
                "Erosion",
                LayerKind::Erosion(ErosionSettings {
                    cell_m: 4.0,
                    droplets_per_cell: 0.6,
                    thermal_iterations: 3,
                    amplitude: 0.9,
                    ..ErosionSettings::default()
                }),
            ));
        }
        Some(false) => {
            for l in &mut r.layers {
                if matches!(l.kind, LayerKind::Erosion(_)) {
                    l.enabled = false;
                }
            }
        }
        _ => {}
    }

    // ── terraces ──────────────────────────────────────────────────────────────────────────────────────
    if brief.terraces
        && !r
            .layers
            .iter()
            .any(|l| matches!(l.kind, LayerKind::Terrace { .. }))
    {
        // Before any erosion, so the benches get weathered rather than sitting on top looking cut.
        let at = r
            .layers
            .iter()
            .position(|l| matches!(l.kind, LayerKind::Erosion(_)))
            .unwrap_or(r.layers.len());
        r.layers.insert(
            at,
            Layer::new(
                "Benches",
                LayerKind::Terrace {
                    step_m: (12.0 * brief.relief).clamp(3.0, 40.0),
                    sharpness: 0.78,
                },
            ),
        );
    }

    // ── water ─────────────────────────────────────────────────────────────────────────────────────────
    // Expressed against the land's own range, so "a coastline" means the same thing on a 40 m hill field as
    // on a 900 m massif rather than putting the sea at an arbitrary absolute height.
    let (lo, hi) = estimate_range(&r);
    r.water = match brief.water {
        WaterWish::Dry => WaterConfig {
            enabled: false,
            ..r.water
        },
        WaterWish::Lakes => WaterConfig {
            enabled: true,
            sea_level_m: lo + (hi - lo) * 0.04,
            shore_blend_m: 1.5,
            deep_m: 6.0,
        },
        WaterWish::Sea => WaterConfig {
            enabled: true,
            sea_level_m: lo + (hi - lo) * 0.22,
            shore_blend_m: 2.5,
            deep_m: 10.0,
        },
        WaterWish::Flooded => WaterConfig {
            enabled: true,
            sea_level_m: lo + (hi - lo) * 0.55,
            shore_blend_m: 3.0,
            deep_m: 14.0,
        },
    };

    // ── palette ───────────────────────────────────────────────────────────────────────────────────────
    let p = brief.climate.palette();
    for m in &mut r.materials {
        if let Some((albedo, roughness)) = role_colour(&m.name, &p) {
            m.albedo = albedo;
            m.roughness = roughness;
        }
    }
    // An arctic world wants snow available even on a preset that has none.
    if brief.climate == Climate::Arctic && !r.materials.iter().any(|m| m.name.contains("Snow")) {
        r.materials.push(MaterialLayer {
            detail_strength: 0.25,
            ..MaterialLayer::new("Snow", p.snow, 0.45)
        });
        let snow = r.materials.len() - 1;
        r.biomes.push(
            BiomeRule::by_height(
                "Snowfield",
                [lo, lo + (hi - lo) * 0.25, hi + 1000.0, hi + 2000.0],
                snow,
            )
            // Not on the steepest faces: snow does not hold on a cliff.
            .with_slope([0.0, 0.0, 0.6, 0.95]),
        );
    }

    // ── vegetation ────────────────────────────────────────────────────────────────────────────────────
    if brief.vegetation <= f32::EPSILON {
        for s in &mut r.scatter {
            s.enabled = false;
        }
    } else {
        for s in &mut r.scatter {
            s.density_per_hectare *= brief.vegetation;
            // The view distance must never outrun the terrain the props stand on.
            s.view_distance_m = s.view_distance_m.min(r.lod.max_view_distance_m);
        }
        // Rename the canopy and understorey to the climate's species. The stand-in generator picks a mesh
        // shape from the prototype's NAME, so this is what makes a tropical island grow palms and an alpine
        // slope grow conifers without a single new asset.
        for proto in &mut r.protos {
            let n = proto.name.to_ascii_lowercase();
            if n.contains("tree")
                || n.contains("conifer")
                || n.contains("pine")
                || n.contains("palm")
            {
                proto.name = p.tree.to_string();
            } else if n.contains("shrub") || n.contains("brush") || n.contains("bush") {
                proto.name = p.shrub.to_string();
            }
        }
        // A landform with no scatter rules at all (the flat plate) still deserves ground cover when the
        // description asked for vegetation.
        if r.scatter.is_empty() {
            r.protos.push(ScatterProto::new(p.tree, "", 2.2, 11.0));
            let tree = r.protos.len() - 1;
            r.protos
                .push(ScatterProto::new("Grass Tufts", "", 0.3, 0.5));
            let tuft = r.protos.len() - 1;
            let biomes: Vec<usize> = (0..r.biomes.len()).collect();
            r.scatter.push(ScatterRule {
                biomes: biomes.clone(),
                slope_max: 0.5,
                cluster: Some([180.0, 0.5, 0.15]),
                view_distance_m: r.lod.max_view_distance_m.min(1000.0),
                ..ScatterRule::new(p.tree, tree, 60.0 * brief.vegetation)
            });
            r.scatter.push(ScatterRule {
                biomes,
                slope_max: 0.7,
                view_distance_m: 80.0,
                ..ScatterRule::new("Ground Cover", tuft, 900.0 * brief.vegetation)
            });
        }
    }

    // ── routes ────────────────────────────────────────────────────────────────────────────────────────
    // Placed as a diagonal across the world: a described route is a starting point the author then drags
    // into place, not a guess at where they wanted it. Stated in the reading, so it is not a surprise.
    let s = r.world_size_m;
    if brief.river {
        r.splines.push(SplineDef {
            name: "River".into(),
            kind: SplineKind::River,
            points: vec![
                [s * 0.12, 0.0, s * 0.18],
                [s * 0.38, 0.0, s * 0.42],
                [s * 0.55, 0.0, s * 0.62],
                [s * 0.86, 0.0, s * 0.88],
            ],
            width_m: (s * 0.006).clamp(6.0, 28.0),
            falloff_m: 14.0,
            depth_m: 3.5,
            // The channel finds its own descending height; the points only say where it runs.
            use_point_height: false,
            material_layer: None,
            clear_scatter_m: 6.0,
            enabled: true,
        });
    }
    if brief.road {
        let road_mat = r.materials.iter().position(|m| {
            let n = m.name.to_ascii_lowercase();
            n.contains("road") || n.contains("gravel")
        });
        r.splines.push(SplineDef {
            name: "Road".into(),
            kind: SplineKind::Road,
            points: vec![
                [s * 0.08, 0.0, s * 0.72],
                [s * 0.34, 0.0, s * 0.58],
                [s * 0.62, 0.0, s * 0.5],
                [s * 0.92, 0.0, s * 0.34],
            ],
            width_m: 12.0,
            falloff_m: 10.0,
            depth_m: 0.0,
            use_point_height: false,
            material_layer: road_mat,
            clear_scatter_m: 10.0,
            enabled: true,
        });
    }

    r
}

/// A cheap estimate of the height range a recipe spans, from its layers' declared amplitudes.
///
/// Used only to place the water line proportionally. It is an estimate on purpose: sampling the real field
/// here would mean compiling the terrain twice, and the water line is an author-visible number they can
/// correct in one edit.
fn estimate_range(r: &TerrainRecipe) -> (f32, f32) {
    let mut base = 0.0f32;
    let mut span = 0.0f32;
    for l in &r.layers {
        if !l.enabled {
            continue;
        }
        match &l.kind {
            LayerKind::Constant { height } => {
                if matches!(l.blend, Blend::Replace) {
                    base = *height;
                } else {
                    base += *height;
                }
            }
            LayerKind::Fbm { amplitude, .. }
            | LayerKind::Ridged { amplitude, .. }
            | LayerKind::Billow { amplitude, .. }
            | LayerKind::Cellular { amplitude, .. }
            | LayerKind::Heightmap { amplitude, .. } => span += amplitude.abs(),
            _ => {}
        }
    }
    (base, base + span.max(1.0))
}

/// Read a description and build the terrain it asks for, in one call.
#[must_use]
pub fn compile(text: &str) -> (TerrainRecipe, Reading) {
    let reading = read(text);
    let mut recipe = build(&reading.brief);
    // Provenance: kept on the recipe rather than beside it, so it round-trips through the document and
    // survives a save, an undo and a reopen without any extra plumbing.
    recipe.description = text.trim().to_string();
    (recipe, reading)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::Terrain;
    use std::collections::BTreeMap;

    fn compiled(text: &str) -> Terrain {
        let (r, _) = compile(text);
        Terrain::compile(r, BTreeMap::new()).expect("a described terrain must compile")
    }

    #[test]
    fn the_shape_words_pick_the_landform() {
        for (text, want) in [
            ("rolling hills", Landform::Hills),
            ("a towering mountain range", Landform::Mountains),
            ("endless sand dunes", Landform::Dunes),
            ("a tropical archipelago", Landform::Islands),
            ("a terraced canyon", Landform::Canyon),
            ("flat plains", Landform::Plains),
        ] {
            assert_eq!(read(text).brief.landform, want, "{text}");
        }
    }

    #[test]
    fn a_climate_alone_implies_a_shape_and_a_shape_implies_a_climate() {
        // "a desert" is a complete request: it should not need the word "dunes" in it.
        let d = read("a desert").brief;
        assert_eq!(d.landform, Landform::Dunes);
        assert_eq!(d.climate, Climate::Arid);
        // And the other way round: mountains without a stated climate are alpine, not tropical.
        let m = read("steep mountains").brief;
        assert_eq!(m.climate, Climate::Alpine);
        // An explicit climate overrides the implication rather than being overridden by it.
        let t = read("tropical mountains").brief;
        assert_eq!(t.landform, Landform::Mountains);
        assert_eq!(t.climate, Climate::Tropical);
    }

    #[test]
    fn scale_is_read_in_both_words_and_numbers() {
        assert_eq!(read("a small island").brief.world_size_m, Some(1024.0));
        assert_eq!(read("a 4 km valley").brief.world_size_m, Some(4000.0));
        assert_eq!(read("2km of hills").brief.world_size_m, Some(2000.0));
        assert_eq!(read("800 m of dunes").brief.world_size_m, Some(800.0));
        // A bare small number is kilometres, because 3 m of terrain is not a request anyone makes.
        assert_eq!(read("3 wide").brief.world_size_m, Some(3000.0));
        // And the extent lands on a whole number of chunks, or the last row is ragged.
        let (r, _) = compile("a 4 km mountain range");
        assert!(
            (r.world_size_m / r.chunk_size_m).fract().abs() < 1e-6,
            "{} is not a whole number of {} m chunks",
            r.world_size_m,
            r.chunk_size_m
        );
    }

    #[test]
    fn relief_words_change_the_actual_height() {
        let gentle = compiled("gentle hills");
        let rugged = compiled("rugged jagged hills");
        let (mut g, mut j) = (0.0f32, 0.0f32);
        for i in 0..40 {
            let (x, z) = (i as f32 * 37.0, 400.0);
            g = g.max(gentle.height(x, z).abs());
            j = j.max(rugged.height(x, z).abs());
        }
        assert!(j > g * 1.5, "rugged {j} should tower over gentle {g}");
    }

    #[test]
    fn a_barren_description_places_nothing_and_a_lush_one_places_more() {
        let (barren, _) = compile("barren rocky peaks");
        assert!(
            barren.scatter.iter().all(|s| !s.enabled),
            "barren must plant nothing"
        );
        // A density word MODIFIES the cover word rather than replacing it, so "sparse woodland" is woodland,
        // thinly planted — and the result does not depend on which word came first.
        let (sparse, _) = compile("sparse woodland hills");
        assert_eq!(
            read("woodland hills, sparse").brief.vegetation,
            read("sparse woodland hills").brief.vegetation
        );
        let (lush, _) = compile("densely forested hills");
        let total = |r: &TerrainRecipe| -> f32 {
            r.scatter
                .iter()
                .filter(|s| s.enabled)
                .map(|s| s.density_per_hectare)
                .sum()
        };
        assert!(
            total(&lush) > total(&sparse) * 2.0,
            "lush {} vs sparse {}",
            total(&lush),
            total(&sparse)
        );
    }

    #[test]
    fn the_climate_repaints_the_ground_and_changes_the_species() {
        let (arctic, _) = compile("an arctic mountain range with conifers");
        // The palette is applied by ROLE, so a preset's own material names keep working.
        let rock = arctic
            .materials
            .iter()
            .find(|m| m.name.contains("Rock"))
            .expect("rock");
        assert!(rock.albedo[2] >= rock.albedo[0], "arctic rock reads cold");
        assert!(arctic.materials.iter().any(|m| m.name.contains("Snow")));

        let (tropical, _) = compile("a lush tropical island");
        assert!(
            tropical.protos.iter().any(|p| p.name.contains("Palm")),
            "a tropical island grows palms: {:?}",
            tropical.protos.iter().map(|p| &p.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn water_is_placed_against_the_lands_own_range() {
        let (dry, _) = compile("a bone dry desert");
        assert!(!dry.water.enabled);

        let (coast, _) = compile("a coastline with beaches");
        assert!(coast.water.enabled);

        // The same wish on two very different reliefs must put the sea in a comparable *place*, not at the
        // same absolute height — the bug this proportional placement exists to avoid.
        let (small, _) = compile("gentle coastal hills");
        let (big, _) = compile("towering coastal mountains");
        assert!(
            big.water.sea_level_m > small.water.sea_level_m,
            "a taller world needs a higher sea line: {} vs {}",
            big.water.sea_level_m,
            small.water.sea_level_m
        );

        // Flooding must actually submerge ground that the dry version left above water.
        let flooded = compiled("a flooded valley");
        let normal = compiled("a valley");
        let (mut wet, mut total) = (0, 0);
        for i in 0..30 {
            for k in 0..30 {
                let (x, z) = (i as f32 * 60.0, k as f32 * 60.0);
                total += 1;
                if flooded.water_depth(x, z) > normal.water_depth(x, z) {
                    wet += 1;
                }
            }
        }
        assert!(wet * 3 > total, "flooding covered only {wet}/{total}");
    }

    #[test]
    fn named_features_appear_as_real_recipe_entries() {
        let (r, reading) = compile("eroded terraced mountains with a river and a road");
        assert!(r
            .layers
            .iter()
            .any(|l| matches!(l.kind, LayerKind::Erosion(_)) && l.enabled));
        assert!(r
            .layers
            .iter()
            .any(|l| matches!(l.kind, LayerKind::Terrace { .. })));
        assert_eq!(r.splines.len(), 2);
        assert!(r.splines.iter().any(|s| s.kind == SplineKind::River));
        assert!(r.splines.iter().any(|s| s.kind == SplineKind::Road));
        // Benches must sit BEFORE the erosion, or they read as cut rather than weathered.
        let bench = r
            .layers
            .iter()
            .position(|l| matches!(l.kind, LayerKind::Terrace { .. }));
        let erode = r
            .layers
            .iter()
            .position(|l| matches!(l.kind, LayerKind::Erosion(_)));
        assert!(
            bench < erode,
            "benches {bench:?} must precede erosion {erode:?}"
        );
        // And every one of those was reported, not applied silently.
        let said = reading
            .understood
            .iter()
            .map(|u| u.meaning.clone())
            .collect::<Vec<_>>()
            .join("; ");
        for want in ["erosion on", "sedimentary benches", "a river", "a road"] {
            assert!(said.contains(want), "{want} missing from: {said}");
        }
    }

    #[test]
    fn smoothness_can_switch_erosion_off_as_well_as_on() {
        // Alpine ships WITH erosion, so "pristine" has to be able to remove it — a one-way flag would make
        // the word do nothing.
        let (r, _) = compile("pristine smooth mountains");
        assert!(
            r.layers
                .iter()
                .filter(|l| matches!(l.kind, LayerKind::Erosion(_)))
                .all(|l| !l.enabled),
            "pristine must disable the erosion the preset brought"
        );
    }

    #[test]
    fn naming_a_species_is_understood_and_suggests_a_climate() {
        // The live run reported "conifer" and "palms" as UNUSED while the climate silently produced exactly
        // those species — the most confusing possible answer, and the reason this exists.
        let r = read("a dense conifer forest on steep hills");
        assert_eq!(r.brief.climate, Climate::Alpine);
        assert!(!r.unused.contains(&"conifer".to_string()), "{:?}", r.unused);
        let p = read("an island with palms");
        assert_eq!(p.brief.climate, Climate::Tropical);
        assert!(!p.unused.contains(&"palms".to_string()), "{:?}", p.unused);
        // A stated climate always beats the hint: "arctic pines" is arctic, not alpine.
        assert_eq!(read("arctic pines").brief.climate, Climate::Arctic);
        // The reading is prose a person reads, so the article has to agree.
        let said = r
            .understood
            .iter()
            .map(|u| u.meaning.clone())
            .collect::<Vec<_>>()
            .join("; ");
        assert!(said.contains("an alpine climate"), "{said}");
        // And the species actually reaches the prototypes.
        let (recipe, _) = compile("an island with palms");
        assert!(
            recipe.protos.iter().any(|x| x.name.contains("Palm")),
            "{:?}",
            recipe.protos.iter().map(|x| &x.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn it_says_what_it_did_not_understand() {
        let reading = read("a spooky haunted mountain range with 4 km of wizards");
        assert_eq!(reading.brief.landform, Landform::Mountains);
        assert_eq!(reading.brief.world_size_m, Some(4000.0));
        // The words it could not use are named, so the author knows why there are no wizards.
        assert!(
            reading.unused.contains(&"wizards".to_string()),
            "{:?}",
            reading.unused
        );
        assert!(reading.unused.contains(&"haunted".to_string()));
        // And the noise words are not reported as failures.
        assert!(!reading.unused.contains(&"a".to_string()));
        assert!(!reading.unused.contains(&"with".to_string()));
    }

    #[test]
    fn the_same_sentence_is_the_same_world_and_a_different_one_is_not() {
        let a = read("a rugged alpine valley with a river");
        let b = read("a rugged alpine valley with a river");
        assert_eq!(a.brief, b.brief);
        assert_eq!(a.brief.seed, b.brief.seed);
        // Whitespace and case are not part of the meaning.
        assert_eq!(
            read("  A Rugged Alpine Valley With A River ").brief.seed,
            a.brief.seed
        );
        // A different description is a different world.
        assert_ne!(
            read("a rugged alpine valley with a road").brief.seed,
            a.brief.seed
        );

        // Bit-identical geometry from the same words, twice compiled.
        let x = compiled("a rugged alpine valley with a river");
        let y = compiled("a rugged alpine valley with a river");
        for i in 0..48 {
            let (px, pz) = (i as f32 * 41.0, 512.0);
            assert_eq!(x.height(px, pz).to_bits(), y.height(px, pz).to_bits());
        }
    }

    #[test]
    fn an_empty_or_nonsense_description_still_gives_a_usable_world() {
        // The alternative — an error — makes the feature something you can fail at.
        for text in ["", "   ", "asdfgh qwerty", "?!"] {
            let (r, reading) = compile(text);
            assert!(!r.layers.is_empty(), "{text:?} produced no layers");
            let t = Terrain::compile(r, BTreeMap::new()).expect("must compile");
            assert!(t.height(100.0, 100.0).is_finite(), "{text:?}");
            let _ = reading;
        }
    }

    #[test]
    fn every_phrase_in_the_lexicon_produces_a_valid_recipe() {
        // The whole lexicon, one phrase at a time: no word may produce a recipe the engine then rejects.
        // Validation only — it is the cheap half, and compiling 120 worlds (several with erosion bakes) would
        // make this test slower than the whole rest of the crate's suite for no extra coverage.
        for (phrase, _) in LEXICON {
            let text = format!("a {phrase} world");
            let (r, _) = compile(&text);
            let report = crate::validate::validate(&r);
            assert!(
                !report.has_blocking(),
                "{text:?} produced blocking issues: {:?}",
                report.issues
            );
        }
    }

    #[test]
    fn the_hardest_descriptions_compile_into_real_terrain() {
        // A representative spread rather than the whole lexicon: every landform, both extremes of scale, and
        // the compound sentence that exercises most of the modifiers at once.
        for text in [
            "a 6 km heavily eroded terraced arid canyon with a river, a road and sparse scrub",
            "a tiny flat barren arctic plain with no water",
            "a vast densely forested temperate hill country with lakes",
            "a lush tropical archipelago with beaches",
            "towering jagged alpine peaks above the snow line",
            "an 800 m dune field, bone dry",
        ] {
            let (r, _) = compile(text);
            let report = crate::validate::validate(&r);
            assert!(!report.has_blocking(), "{text:?}: {:?}", report.issues);
            let t =
                Terrain::compile(r, BTreeMap::new()).unwrap_or_else(|e| panic!("{text:?}: {e:?}"));
            assert!(t.height(200.0, 200.0).is_finite(), "{text:?}");
        }
    }
}
