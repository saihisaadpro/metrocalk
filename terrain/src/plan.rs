//! Compiling a sentence into a plan of [`TerrainOperation`]s.
//!
//! ## Two kinds of sentence
//!
//! * **Creation** — "a 4 km eroded alpine valley with a river". There is no world yet, or the author wants a
//!   different one. This goes to [`crate::describe`], which writes a whole recipe.
//! * **Modification** — "raise this mountain", "widen the river", "flatten the ground here", "make this
//!   valley traversable". There *is* a world, and the sentence changes part of it. This becomes operations.
//!
//! [`compile`] decides which by looking for a modifying verb, and says which it chose, so an author who
//! typed a modification into an empty world gets told that rather than a surprise new world.
//!
//! ## Where "this" comes from
//!
//! Deixis — "this", "here", "that mountain" — resolves against a [`PlanContext`] carrying where the author
//! is pointing. The viewport already ray-casts the cursor against the terrain for the sculpt brush; that
//! same hit is the anchor. Without a cursor, "this" falls back to the centre of the view, and the plan says
//! so rather than silently picking the world origin.
//!
//! ## Why this is a lexicon and not a model
//!
//! The same reason as [`crate::describe`]: determinism. The output of this module is a list of structs that
//! a language model could equally well emit — and when one does, it enters through the same
//! [`crate::operation::resolve`] gate, gets the same validation, and produces the same undo step. The
//! grammar here is the floor, not the ceiling.

use crate::describe;
use crate::feature::FeatureKind;
use crate::operation::{Constraint, Rect, Target, TerrainOperation, Verb};
use crate::recipe::{SplineKind, TerrainRecipe};
use serde::{Deserialize, Serialize};

/// What the author is looking at when they say "this".
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanContext {
    /// Where the cursor meets the ground, if it does.
    pub cursor: Option<[f32; 2]>,
    /// Where the camera is looking, as a fallback for "this".
    pub focus: [f32; 2],
    /// Default radius for "here", in metres.
    pub here_radius_m: f32,
    /// How high the ground is at the anchor, when the caller knows.
    ///
    /// Levelling verbs need an absolute height to level *to*, and the recipe alone cannot supply one — the
    /// layer stack has to be evaluated to know how high the ground is anywhere. The viewport's cursor
    /// ray-cast already computes exactly that as a by-product of aiming the sculpt brush, so this carries it
    /// rather than making the operation guess. `None` means the caller genuinely does not know, and a verb
    /// that needs it refuses rather than inventing a number.
    pub ground_m: Option<f32>,
}

impl PlanContext {
    /// The anchor for a deictic reference, and whether it came from the cursor.
    #[must_use]
    pub fn anchor(&self) -> ([f32; 2], bool) {
        self.cursor.map_or((self.focus, false), |c| (c, true))
    }
}

impl Default for PlanContext {
    fn default() -> Self {
        Self {
            cursor: None,
            focus: [0.0, 0.0],
            here_radius_m: 150.0,
            ground_m: None,
        }
    }
}

/// What a sentence turned into.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Intent {
    /// Build a whole world from the description.
    Create(Box<describe::Reading>),
    /// Change part of the world that exists.
    Modify(Vec<TerrainOperation>),
}

/// The compiled plan, inspectable before anything runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    /// Creation or modification.
    pub intent: Intent,
    /// Every phrase that was understood, and what it meant.
    pub understood: Vec<describe::Understood>,
    /// Words that carried no meaning.
    pub unused: Vec<String>,
    /// Notes the author should see — e.g. that "this" fell back to the view centre.
    pub notes: Vec<String>,
}

impl Plan {
    /// The operations, if this is a modification.
    #[must_use]
    pub fn operations(&self) -> &[TerrainOperation] {
        match &self.intent {
            Intent::Modify(ops) => ops,
            Intent::Create(_) => &[],
        }
    }

    /// Whether the sentence asked to build a whole world.
    #[must_use]
    pub fn is_creation(&self) -> bool {
        matches!(self.intent, Intent::Create(_))
    }
}

/// A modifying verb and the phrases that mean it.
struct VerbWord {
    phrase: &'static str,
    verb: Verb,
}

/// Verbs that mean "change what is already there". Longest phrase wins.
const VERBS: &[VerbWord] = &[
    VerbWord {
        phrase: "make it traversable",
        verb: Verb::MakeTraversable,
    },
    VerbWord {
        phrase: "make this traversable",
        verb: Verb::MakeTraversable,
    },
    VerbWord {
        phrase: "suitable for vehicles",
        verb: Verb::MakeTraversable,
    },
    VerbWord {
        phrase: "suitable for vehicle traversal",
        verb: Verb::MakeTraversable,
    },
    VerbWord {
        phrase: "vehicle traversal",
        verb: Verb::MakeTraversable,
    },
    VerbWord {
        phrase: "traversable",
        verb: Verb::MakeTraversable,
    },
    VerbWord {
        phrase: "drivable",
        verb: Verb::MakeTraversable,
    },
    VerbWord {
        phrase: "passable",
        verb: Verb::MakeTraversable,
    },
    VerbWord {
        phrase: "raise",
        verb: Verb::Raise,
    },
    VerbWord {
        phrase: "heighten",
        verb: Verb::Raise,
    },
    VerbWord {
        phrase: "taller",
        verb: Verb::Raise,
    },
    VerbWord {
        phrase: "higher",
        verb: Verb::Raise,
    },
    VerbWord {
        phrase: "lower",
        verb: Verb::Lower,
    },
    VerbWord {
        phrase: "shorter",
        verb: Verb::Lower,
    },
    VerbWord {
        phrase: "deepen",
        verb: Verb::Lower,
    },
    VerbWord {
        phrase: "sink",
        verb: Verb::Lower,
    },
    VerbWord {
        phrase: "widen",
        verb: Verb::Widen,
    },
    VerbWord {
        phrase: "wider",
        verb: Verb::Widen,
    },
    VerbWord {
        phrase: "broaden",
        verb: Verb::Widen,
    },
    VerbWord {
        phrase: "bigger",
        verb: Verb::Widen,
    },
    VerbWord {
        phrase: "larger",
        verb: Verb::Widen,
    },
    VerbWord {
        phrase: "narrow",
        verb: Verb::Narrow,
    },
    VerbWord {
        phrase: "narrower",
        verb: Verb::Narrow,
    },
    VerbWord {
        phrase: "smaller",
        verb: Verb::Narrow,
    },
    VerbWord {
        phrase: "shrink",
        verb: Verb::Narrow,
    },
    VerbWord {
        phrase: "flatten",
        verb: Verb::Flatten,
    },
    VerbWord {
        phrase: "level",
        verb: Verb::Flatten,
    },
    VerbWord {
        phrase: "smooth",
        verb: Verb::Smooth,
    },
    VerbWord {
        phrase: "soften",
        verb: Verb::Smooth,
    },
    VerbWord {
        phrase: "roughen",
        verb: Verb::Roughen,
    },
    VerbWord {
        phrase: "rougher",
        verb: Verb::Roughen,
    },
    VerbWord {
        phrase: "plant",
        verb: Verb::Plant,
    },
    VerbWord {
        phrase: "forest",
        verb: Verb::Plant,
    },
    VerbWord {
        phrase: "afforest",
        verb: Verb::Plant,
    },
    VerbWord {
        phrase: "clear",
        verb: Verb::Clear,
    },
    VerbWord {
        phrase: "deforest",
        verb: Verb::Clear,
    },
    VerbWord {
        phrase: "flood",
        verb: Verb::Flood,
    },
    VerbWord {
        phrase: "remove",
        verb: Verb::Remove,
    },
    VerbWord {
        phrase: "delete",
        verb: Verb::Remove,
    },
    VerbWord {
        phrase: "rename",
        verb: Verb::Rename,
    },
    VerbWord {
        phrase: "move",
        verb: Verb::Move,
    },
    VerbWord {
        phrase: "add",
        verb: Verb::Add,
    },
    VerbWord {
        phrase: "put",
        verb: Verb::Add,
    },
];

/// Landform nouns a target can name.
const NOUNS: &[(&str, FeatureKind)] = &[
    ("mountain", FeatureKind::Mountain),
    ("peak", FeatureKind::Mountain),
    ("summit", FeatureKind::Mountain),
    ("hill", FeatureKind::Hill),
    ("knoll", FeatureKind::Hill),
    ("ridge", FeatureKind::Ridge),
    ("crest", FeatureKind::Ridge),
    ("spur", FeatureKind::Ridge),
    ("valley", FeatureKind::Valley),
    ("glen", FeatureKind::Valley),
    ("corrie", FeatureKind::Valley),
    ("gully", FeatureKind::Valley),
    ("basin", FeatureKind::Basin),
    ("hollow", FeatureKind::Basin),
    ("plateau", FeatureKind::Plateau),
    ("tableland", FeatureKind::Plateau),
    ("cliff", FeatureKind::Cliff),
    ("escarpment", FeatureKind::Cliff),
    ("crater", FeatureKind::Crater),
    ("pad", FeatureKind::Pad),
    ("clearing", FeatureKind::Clearing),
    ("glade", FeatureKind::Clearing),
    ("woodland", FeatureKind::Forest),
    ("wood", FeatureKind::Forest),
    ("forest", FeatureKind::Forest),
    ("zone", FeatureKind::Zone),
];

/// Words that make a change bigger or smaller, as a multiplier on the default amount.
const DEGREES: &[(&str, f32)] = &[
    ("slightly", 0.35),
    ("a little", 0.35),
    ("a bit", 0.4),
    ("somewhat", 0.6),
    ("a lot", 1.8),
    ("much", 1.6),
    ("far", 1.8),
    ("dramatically", 2.4),
    ("massively", 3.0),
    ("hugely", 2.6),
    ("twice", 2.0),
    ("double", 2.0),
    ("half", 0.5),
];

/// Split into lowercase word tokens.
fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric() && c != '.')
        .filter(|t| !t.is_empty())
        .map(|t| t.trim_matches('.').to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

fn contains_phrase(toks: &[String], phrase: &str) -> Option<usize> {
    let words: Vec<&str> = phrase.split(' ').collect();
    toks.windows(words.len().max(1))
        .position(|w| w.iter().zip(&words).all(|(a, b)| a == b))
}

/// Read a distance in metres from the sentence: "by 50 m", "30 metres", "2 km".
fn read_metres(toks: &[String]) -> Option<f32> {
    for i in 0..toks.len() {
        let Ok(n) = toks[i].parse::<f32>() else {
            continue;
        };
        if !n.is_finite() || n <= 0.0 {
            continue;
        }
        return match toks.get(i + 1).map(String::as_str) {
            Some("km" | "kilometre" | "kilometres" | "kilometer" | "kilometers") => {
                Some(n * 1000.0)
            }
            // Metres, explicitly or by default: a bare number next to a height verb means metres, so
            // "raise it 40" and "raise it by 40 m" are the same request.
            _ => Some(n),
        };
    }
    None
}

/// Read a percentage or "1 in 5" style grade.
fn read_slope(toks: &[String]) -> Option<f32> {
    for i in 0..toks.len() {
        if let Ok(n) = toks[i].parse::<f32>() {
            if matches!(
                toks.get(i + 1).map(String::as_str),
                Some("percent" | "pc" | "grade")
            ) && n > 0.0
            {
                return Some(n / 100.0);
            }
        }
    }
    None
}

fn degree(toks: &[String]) -> Option<f32> {
    DEGREES
        .iter()
        .find(|(p, _)| contains_phrase(toks, p).is_some())
        .map(|(_, f)| *f)
}

/// Resolve what the sentence is pointing at.
/// Words that can never be part of a landform's proper name.
const NAME_NOISE: &[&str] = &[
    "raise",
    "lower",
    "widen",
    "narrow",
    "flatten",
    "level",
    "smooth",
    "soften",
    "roughen",
    "rougher",
    "plant",
    "clear",
    "flood",
    "remove",
    "delete",
    "rename",
    "move",
    "add",
    "put",
    "make",
    "the",
    "a",
    "an",
    "and",
    "to",
    "of",
    "by",
    "it",
    "this",
    "that",
    "here",
    "with",
    "more",
    "less",
    "m",
    "km",
    "metres",
    "meters",
    "percent",
    "grade",
    "for",
    "traversable",
    "suitable",
    "vehicles",
    "vehicle",
    "deepen",
    "sink",
    "shrink",
    "bigger",
    "larger",
    "smaller",
    "taller",
    "higher",
    "shorter",
    "broaden",
    "wider",
    "narrower",
    "heighten",
    "afforest",
    "deforest",
    "please",
    "then",
    "also",
    "but",
];

fn read_target(
    toks: &[String],
    ctx: &PlanContext,
    notes: &mut Vec<String>,
) -> (Target, Option<FeatureKind>) {
    let (anchor, from_cursor) = ctx.anchor();
    let deictic = [
        "this", "that", "here", "it", "these", "those", "area", "spot", "ground",
    ]
    .iter()
    .any(|d| toks.iter().any(|t| t == d));

    // A named landform in quotes or after "the": try the longest run of capitalised-looking words. The
    // lexicon cannot know a world's proper nouns, so name matching is left to `operation::resolve`, which
    // has the recipe — here we only decide that a NAME was meant.
    let noun = NOUNS
        .iter()
        .find(|(w, _)| toks.iter().any(|t| t == w))
        .map(|(_, k)| *k);

    // A route noun beats a landform noun: "widen the river" is about the river.
    let route_kind = if toks
        .iter()
        .any(|t| t == "river" || t == "stream" || t == "burn")
    {
        Some(SplineKind::River)
    } else if toks
        .iter()
        .any(|t| t == "road" || t == "track" || t == "highway")
    {
        Some(SplineKind::Road)
    } else {
        None
    };

    if deictic && !from_cursor {
        notes.push(
            "“this” used the centre of the view — point at the ground to be precise".to_string(),
        );
    }

    if let Some(kind) = route_kind {
        return (Target::NearestRoute { kind, near: anchor }, None);
    }
    match noun {
        // A landform noun with a pointing word means "the one I am looking at".
        Some(k) => (
            Target::NearestOfKind {
                kind: k,
                near: anchor,
            },
            Some(k),
        ),
        None if deictic => (
            Target::Here {
                at: anchor,
                radius_m: ctx.here_radius_m,
            },
            None,
        ),
        None => {
            // No landform noun and nothing to point at: whatever content words remain are a NAME. This is
            // what makes "raise Ben Nevis" work — and what makes "raise the volcano" fail BY NAME, so the
            // refusal can list the landforms that do exist instead of a generic "needs something to act on".
            let name: Vec<&str> = toks
                .iter()
                .map(String::as_str)
                .filter(|t| !NAME_NOISE.contains(t) && t.parse::<f32>().is_err())
                .collect();
            if name.is_empty() {
                (Target::World, None)
            } else {
                (
                    Target::Named {
                        name: name.join(" "),
                    },
                    None,
                )
            }
        }
    }
}

/// Compile a sentence into a plan.
///
/// `recipe` is the world as it stands — `None` when there is none yet, which is what makes a modification
/// sentence explain itself instead of silently building a world.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn compile(text: &str, recipe: Option<&TerrainRecipe>, ctx: &PlanContext) -> Plan {
    let toks = tokens(text);
    let mut notes = Vec::new();
    let mut understood = Vec::new();
    let mut used: Vec<bool> = vec![false; toks.len()];

    // Find every modifying verb, longest phrase first so "make this traversable" beats "make".
    let mut verbs: Vec<&VerbWord> = VERBS.iter().collect();
    verbs.sort_by_key(|v| std::cmp::Reverse(v.phrase.split(' ').count()));
    let mut found: Vec<(usize, Verb, &'static str)> = Vec::new();
    for v in &verbs {
        if let Some(at) = contains_phrase(&toks, v.phrase) {
            let n = v.phrase.split(' ').count();
            if (at..at + n).any(|i| used.get(i).copied().unwrap_or(true)) {
                continue;
            }
            for i in at..at + n {
                if let Some(u) = used.get_mut(i) {
                    *u = true;
                }
            }
            found.push((at, v.verb, v.phrase));
        }
    }
    found.sort_by_key(|(at, _, _)| *at);

    // "forest" and "clear" are also creation words ("a forested valley"); treat them as modifying verbs
    // only when the sentence is otherwise about changing something.
    let structural = found
        .iter()
        .any(|(_, v, _)| !matches!(v, Verb::Plant | Verb::Clear | Verb::Add));

    if found.is_empty() || (!structural && recipe.is_none()) {
        // A creation sentence.
        let reading = describe::read(text);
        return Plan {
            understood: reading.understood.clone(),
            unused: reading.unused.clone(),
            intent: Intent::Create(Box::new(reading)),
            notes,
        };
    }

    if recipe.is_none() {
        notes.push(
            "there is no terrain yet — describe the world you want first, then refine it"
                .to_string(),
        );
    }

    let (target, noun_kind) = read_target(&toks, ctx, &mut notes);
    let metres = read_metres(&toks);
    let slope = read_slope(&toks);
    let deg = degree(&toks);
    let seed = describe::read(text).brief.seed;

    let mut ops = Vec::new();
    for (_, verb, phrase) in &found {
        let mut op = TerrainOperation::new(*verb, target.clone());
        op.seed = seed;
        op.source = (*phrase).to_string();

        match verb {
            Verb::Raise | Verb::Lower => {
                op.params.delta_m = Some(metres.unwrap_or(60.0) * deg.unwrap_or(1.0));
            }
            Verb::Widen => op.params.factor = Some(deg.unwrap_or(1.4).max(1.01)),
            Verb::Narrow => op.params.factor = Some(1.0 / deg.unwrap_or(1.4).max(1.01)),
            Verb::Smooth => op.params.factor = Some(0.4),
            Verb::Roughen => op.params.factor = Some(deg.unwrap_or(1.8).max(1.01)),
            Verb::Flatten => {
                op.params.height_m = metres;
                op.params.kind = Some(FeatureKind::Pad);
            }
            Verb::Remove | Verb::Rename | Verb::Move => {}
            Verb::Flood => op.params.height_m = Some(metres.unwrap_or(0.0)),
            Verb::Plant => {
                op.params.kind = Some(FeatureKind::Forest);
                op.params.factor = Some(deg.unwrap_or(1.0) * 3.0);
                op.params.radius_m = metres;
            }
            Verb::Clear => {
                op.params.kind = Some(FeatureKind::Clearing);
                op.params.factor = Some(0.0);
                op.params.radius_m = metres;
            }
            Verb::MakeTraversable => {
                // A default a vehicle can actually climb. Named explicitly so the number is arguable rather
                // than buried: 25 % is a steep but drivable forest track.
                op.constraints.push(Constraint::Traversable {
                    max_slope: slope.unwrap_or(0.25),
                });
                // Level toward the ground the author is pointing at. An explicit height in the sentence
                // ("make this traversable at 200 m") still wins; this only fills the blank.
                op.params.height_m = op.params.height_m.or(metres).or(ctx.ground_m);
            }
            Verb::Add => {
                op.params.kind = noun_kind.or(Some(FeatureKind::Hill));
                op.params.radius_m = metres;
                // "add a mountain here" places it where the author is pointing.
                let (anchor, _) = ctx.anchor();
                op.bounds = Some(Rect::around(
                    anchor,
                    metres.unwrap_or(ctx.here_radius_m * 2.0),
                ));
            }
        }

        // A verb that CREATES acts where the author is pointing, never over the whole world — "flatten this
        // area" must not level the map. An explicit region in the sentence still wins.
        if !verb.needs_existing() && op.bounds.is_none() {
            let (anchor, _) = ctx.anchor();
            op.bounds = Some(Rect::around(
                anchor,
                op.params.radius_m.unwrap_or(ctx.here_radius_m),
            ));
        }

        understood.push(describe::Understood {
            phrase: (*phrase).to_string(),
            meaning: format!("{} {}", verb.label(), target_label(&op.target)),
        });
        ops.push(op);
    }

    if let Some(m) = metres {
        understood.push(describe::Understood {
            phrase: format!("{m:.0} m"),
            meaning: "the amount to change by".into(),
        });
    }
    if let Some(s) = slope {
        understood.push(describe::Understood {
            phrase: format!("{:.0}%", s * 100.0),
            meaning: "the steepest grade allowed".into(),
        });
    }

    // Unused words: everything the verb/target/number pass did not consume, minus the connective noise.
    let consumed: Vec<&str> = NOUNS
        .iter()
        .map(|(w, _)| *w)
        .chain([
            "this", "that", "here", "it", "the", "a", "an", "and", "to", "of", "make", "more",
        ])
        .chain(DEGREES.iter().map(|(w, _)| *w))
        .chain([
            "river",
            "road",
            "stream",
            "track",
            "highway",
            "burn",
            "by",
            "m",
            "km",
            "metres",
            "meters",
            "percent",
            "grade",
            "for",
            "vehicles",
            "vehicle",
            "suitable",
            "while",
            "preserving",
            "with",
        ])
        .collect();
    let unused: Vec<String> = toks
        .iter()
        .enumerate()
        .filter(|(i, t)| {
            !used[*i]
                && !consumed.contains(&t.as_str())
                && t.parse::<f32>().is_err()
                && !DEGREES
                    .iter()
                    .any(|(p, _)| p.split(' ').any(|w| w == t.as_str()))
        })
        .map(|(_, t)| t.clone())
        .collect();

    Plan {
        intent: Intent::Modify(ops),
        understood,
        unused,
        notes,
    }
}

fn target_label(t: &Target) -> String {
    match t {
        Target::World => "the whole terrain".into(),
        Target::Feature { id } => format!("landform {id}"),
        Target::Named { name } => format!("“{name}”"),
        Target::NearestOfKind { kind, .. } => format!("the {} you are pointing at", kind.label()),
        Target::Region { .. } => "that region".into(),
        Target::Here { radius_m, .. } => format!("the ground within {radius_m:.0} m"),
        Target::Route { index } => format!("route {index}"),
        Target::NearestRoute { kind, .. } => match kind {
            SplineKind::River => "the nearest river".into(),
            SplineKind::Road => "the nearest road".into(),
            SplineKind::Pad => "the nearest pad".into(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feature::{Skeleton, WorldFeature};
    use crate::operation::{apply, resolve};

    fn ctx_at(x: f32, z: f32) -> PlanContext {
        PlanContext {
            cursor: Some([x, z]),
            focus: [x, z],
            here_radius_m: 150.0,
            // A pointing author is standing somewhere, and the viewport's ray-cast knows how high.
            ground_m: Some(120.0),
        }
    }

    fn world() -> TerrainRecipe {
        let mut r = TerrainRecipe {
            world_size_m: 2048.0,
            ..TerrainRecipe::default()
        };
        let mut m = WorldFeature::new(
            1,
            "Ben Cruachan",
            FeatureKind::Mountain,
            Skeleton::Point { x: 800.0, z: 800.0 },
        );
        m.extent_m = 250.0;
        m.amplitude_m = 300.0;
        r.features.push(m);
        r.next_feature_id = 2;
        r
    }

    #[test]
    fn a_sentence_with_no_verb_is_a_creation() {
        let p = compile(
            "a 4 km eroded alpine valley with a river",
            None,
            &PlanContext::default(),
        );
        assert!(p.is_creation());
        assert!(p.operations().is_empty());
    }

    #[test]
    fn raise_this_mountain_becomes_one_operation_on_the_right_landform() {
        let r = world();
        let p = compile(
            "raise this mountain by 120 m",
            Some(&r),
            &ctx_at(820.0, 790.0),
        );
        assert!(!p.is_creation());
        let ops = p.operations();
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].verb, Verb::Raise);
        assert!((ops[0].params.delta_m.unwrap() - 120.0).abs() < 1e-3);
        // It resolves to the mountain under the cursor and applies.
        let res = resolve(&ops[0], &r).expect("resolve");
        assert_eq!(res.feature, Some(1));
        let mut r2 = r.clone();
        apply(&res, &mut r2).expect("apply");
        assert!((r2.features[0].amplitude_m - 420.0).abs() < 1e-3);
    }

    #[test]
    fn widen_the_river_targets_the_route_not_a_landform() {
        let mut r = world();
        r.splines.push(crate::recipe::SplineDef {
            name: "Awe".into(),
            kind: SplineKind::River,
            points: vec![[0.0, 0.0, 500.0], [2000.0, 0.0, 700.0]],
            width_m: 10.0,
            falloff_m: 8.0,
            depth_m: 2.0,
            use_point_height: false,
            material_layer: None,
            clear_scatter_m: 5.0,
            enabled: true,
        });
        let p = compile("widen the river", Some(&r), &ctx_at(900.0, 600.0));
        let ops = p.operations();
        assert_eq!(ops.len(), 1);
        assert!(matches!(
            ops[0].target,
            Target::NearestRoute {
                kind: SplineKind::River,
                ..
            }
        ));
        let res = resolve(&ops[0], &r).expect("resolve");
        assert_eq!(res.route, Some(0));
        assert_eq!(res.feature, None, "a river is not a landform");
    }

    #[test]
    fn flatten_this_area_scopes_to_where_the_author_is_pointing() {
        let r = world();
        let p = compile("flatten this area", Some(&r), &ctx_at(400.0, 400.0));
        let ops = p.operations();
        assert_eq!(ops[0].verb, Verb::Flatten);
        let res = resolve(&ops[0], &r).expect("resolve");
        assert!(res.region.contains([400.0, 400.0]));
        // And it does NOT scope to the whole world — the point of a local edit.
        assert!(res.region.max[0] - res.region.min[0] < r.world_size_m * 0.5);
    }

    #[test]
    fn make_this_valley_suitable_for_vehicles_carries_a_measurable_constraint() {
        // The headline sentence from the brief.
        let mut r = world();
        let mut v = WorldFeature::new(
            2,
            "Glen Etive",
            FeatureKind::Valley,
            Skeleton::Path {
                points: vec![[200.0, 1200.0], [900.0, 1400.0]],
            },
        );
        v.extent_m = 220.0;
        v.amplitude_m = 140.0;
        r.features.push(v);
        r.next_feature_id = 3;

        let p = compile(
            "make this valley suitable for vehicle traversal",
            Some(&r),
            &ctx_at(500.0, 1300.0),
        );
        let ops = p.operations();
        assert_eq!(ops.len(), 1, "{ops:?}");
        assert_eq!(ops[0].verb, Verb::MakeTraversable);
        let limit = ops[0]
            .constraints
            .iter()
            .copied()
            .find_map(Constraint::slope_limit)
            .expect("a slope limit");
        assert!((limit - 0.25).abs() < 1e-6);
        // It resolves onto the valley, not the mountain a kilometre away.
        let res = resolve(&ops[0], &r).expect("resolve");
        assert_eq!(res.feature, Some(2));
    }

    #[test]
    fn an_explicit_grade_overrides_the_default() {
        let r = world();
        let p = compile(
            "make this traversable at 10 percent",
            Some(&r),
            &ctx_at(800.0, 800.0),
        );
        let limit = p.operations()[0]
            .constraints
            .iter()
            .copied()
            .find_map(Constraint::slope_limit)
            .expect("limit");
        assert!((limit - 0.10).abs() < 1e-6, "{limit}");
    }

    #[test]
    fn making_ground_traversable_levels_toward_that_ground_and_never_to_sea_level() {
        // The regression this exists for: `FeatureOp::Flatten` is an ABSOLUTE lerp toward `target_m`, and
        // MakeTraversable used to default that to 0.0. On any world whose ground is not near y = 0 —
        // which is every alpine or plateau world the panel itself offers as an example — "make this
        // valley traversable" therefore cut the region down to sea level instead of reducing its grade.
        // A corridor is a change of slope, not a hole.
        let mut r = world();
        let ctx = ctx_at(800.0, 800.0);
        let ground = ctx.ground_m.expect("the fixture points at real ground");
        let p = compile("make this traversable", Some(&r), &ctx);
        let op = &p.operations()[0];
        let res = resolve(op, &r).expect("resolve");
        apply(&res, &mut r).expect("apply");
        let made = r.features.last().expect("a corridor was created");
        assert!(
            (made.target_m - ground).abs() < 1e-6,
            "levelled to {} but the ground there is {ground}",
            made.target_m
        );

        // And when nobody knows how high the ground is, it says so rather than picking zero.
        let mut blind = world();
        let before = blind.features.len();
        let next_id = blind.next_feature_id;
        let bp = compile(
            "make this traversable",
            Some(&blind),
            &PlanContext {
                cursor: Some([800.0, 800.0]),
                focus: [800.0, 800.0],
                here_radius_m: 150.0,
                ground_m: None,
            },
        );
        let bop = &bp.operations()[0];
        let bres = resolve(bop, &blind).expect("resolve");
        let err = apply(&bres, &mut blind).expect_err("it must refuse, not level to zero");
        assert!(
            err.reason.contains("how high the ground is"),
            "{:?}",
            err.reason
        );
        // A refusal must leave the world EXACTLY as it found it — no corridor, and no id burned.
        assert_eq!(
            blind.features.len(),
            before,
            "a refusal must not leave a crater"
        );
        assert_eq!(
            blind.next_feature_id, next_id,
            "a refusal must not consume an id"
        );
    }

    #[test]
    fn degree_words_scale_the_change() {
        let r = world();
        let small = compile("raise this slightly", Some(&r), &ctx_at(800.0, 800.0));
        let big = compile("raise this dramatically", Some(&r), &ctx_at(800.0, 800.0));
        let d = |p: &Plan| p.operations()[0].params.delta_m.unwrap();
        assert!(d(&big) > d(&small) * 4.0, "{} vs {}", d(&big), d(&small));
    }

    #[test]
    fn several_verbs_in_one_sentence_become_several_operations_in_order() {
        let r = world();
        let p = compile(
            "widen this valley and make it traversable",
            Some(&r),
            &ctx_at(800.0, 800.0),
        );
        let ops = p.operations();
        assert_eq!(ops.len(), 2, "{ops:?}");
        assert_eq!(ops[0].verb, Verb::Widen);
        assert_eq!(ops[1].verb, Verb::MakeTraversable);
    }

    #[test]
    fn a_modification_with_no_world_says_so_instead_of_building_one() {
        let p = compile("raise this mountain", None, &PlanContext::default());
        assert!(!p.is_creation(), "it is still a modification");
        assert!(
            p.notes.iter().any(|n| n.contains("no terrain yet")),
            "{:?}",
            p.notes
        );
    }

    #[test]
    fn without_a_cursor_this_falls_back_and_says_it_did() {
        let r = world();
        let ctx = PlanContext {
            cursor: None,
            focus: [1024.0, 1024.0],
            here_radius_m: 150.0,
            ground_m: None,
        };
        let p = compile("flatten this", Some(&r), &ctx);
        assert!(
            p.notes.iter().any(|n| n.contains("centre of the view")),
            "{:?}",
            p.notes
        );
    }

    #[test]
    fn planting_is_a_modification_when_the_sentence_is_about_changing_things() {
        let r = world();
        let p = compile("plant a forest here", Some(&r), &ctx_at(500.0, 500.0));
        assert!(!p.is_creation());
        assert_eq!(p.operations()[0].verb, Verb::Plant);
        // But "a forested valley" with no world is still a description of a world to build.
        let c = compile("a forested valley", None, &PlanContext::default());
        assert!(c.is_creation());
    }

    #[test]
    fn a_landform_can_be_addressed_by_its_proper_name() {
        // What makes named landforms worth having: "raise Ben Cruachan" finds it without a cursor.
        let r = world();
        let p = compile(
            "raise Ben Cruachan by 90 m",
            Some(&r),
            &PlanContext::default(),
        );
        let ops = p.operations();
        assert_eq!(ops.len(), 1);
        assert!(
            matches!(&ops[0].target, Target::Named { name } if name.contains("cruachan")),
            "{:?}",
            ops[0].target
        );
        let res = crate::operation::resolve(&ops[0], &r).expect("resolve");
        assert_eq!(res.feature, Some(1));
    }

    #[test]
    fn an_unknown_name_is_refused_by_that_name_not_generically() {
        // The live run caught this: "raise the volcano" refused with "needs something to act on", which
        // tells the author nothing. It must fail BY NAME so the refusal can list what does exist.
        let r = world();
        let p = compile(
            "raise the volcano by 400 m",
            Some(&r),
            &PlanContext::default(),
        );
        let err = crate::operation::resolve(&p.operations()[0], &r).expect_err("must refuse");
        assert!(err.reason.contains("volcano"), "{}", err.reason);
        assert!(err.suggestion.unwrap_or_default().contains("Ben Cruachan"));
    }

    #[test]
    fn it_reports_the_words_it_could_not_use() {
        let r = world();
        let p = compile(
            "raise this mountain with magic sparkles",
            Some(&r),
            &ctx_at(800.0, 800.0),
        );
        assert!(p.unused.contains(&"sparkles".to_string()), "{:?}", p.unused);
        assert!(p.unused.contains(&"magic".to_string()));
        assert!(
            !p.unused.contains(&"mountain".to_string()),
            "mountain was used"
        );
    }

    #[test]
    fn every_operation_carries_the_phrase_that_produced_it() {
        let r = world();
        let p = compile("widen this and raise it", Some(&r), &ctx_at(800.0, 800.0));
        for op in p.operations() {
            assert!(!op.source.is_empty(), "{op:?} has no source phrase");
        }
    }
}
