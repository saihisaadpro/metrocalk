//! **Colour management** — spaces are declared, never assumed.
//!
//! ## What was already right, and what was not
//!
//! This engine already had a coherent linear pipeline: the scene renders into an `Rgba16Float` target,
//! everything upstream of the resolve is scene-linear radiance, the tone curve runs exactly once, and
//! base-colour textures upload as `Rgba8UnormSrgb` while roughness/metallic/AO/normal upload as
//! `Rgba8Unorm` — so **data maps already bypassed the colour transform**, which is the single most
//! commonly botched thing in a real-time renderer.
//!
//! What was missing is not the maths. It is that every one of those decisions was an **implicit
//! convention applied at a call site**: nothing declared a texture's colour space, nothing could
//! override it, nothing persisted it, and nothing could tell you what the working space was. A
//! pipeline whose colour behaviour is correct-by-habit is one refactor away from being wrong, and it
//! cannot be audited by the person whose shot looks off.
//!
//! This module makes the decisions explicit, exact and inspectable.
//!
//! ## Scope, stated plainly
//!
//! This is **not** OpenColorIO. OCIO's Rust bindings (`ocio-rs`/`ocio-sys`) are FFI to the C++ library,
//! which needs cmake and OCIO's own dependency tree; see [`ocio_status`]. What OCIO uniquely provides
//! is (a) reading a studio's config file and (b) arbitrary LUT/CTF transforms. What it does NOT
//! uniquely provide is the primary-conversion matrices and transfer functions, which are published
//! exactly and implemented here to the published values, verified against reference numbers in tests.
//!
//! So, precisely: **conversion** between sRGB, linear Rec.709, ACEScg (AP1), ACES2065-1 (AP0) and
//! Rec.2020 is exact and reference-tested, and the role -> colour-space policy has one home that the
//! renderer's uploader derives from.
//!
//! ## What changed, and what is still absent (ADR-109)
//!
//! **The renderer now genuinely shades in the selected working space.** [`WorkingSpace`] hands out the
//! two matrices and the luminance weights the renderer needs; every colour that carries light is
//! converted into that space before it takes part in a colour computation, and the frame is converted
//! back exactly once, immediately before the view transform — which declares the space it is defined on
//! ([`ViewTransform::input_space`]) rather than leaving it to be assumed. Linear Rec.709 is the
//! identity, so selecting it is provably a no-op.
//!
//! **A source space can be declared per asset** where the file cannot say it itself — see
//! [`decide`] for the precedence and [`source_to_working`] for the composed matrix. The renderer wires
//! this for the ENVIRONMENT (Radiance `.hdr` has no required primaries header); a per-texture override
//! has no storage in the asset library yet, and `colour_status` reports those as two separate flags
//! rather than one flattering word.
//!
//! **Still absent, each for a stated technical reason, not an omission:** loading a studio
//! `.ocio` config (see [`ocio_status`] — two independent blockers), the ACES 2.0 Output Transform (see
//! [`aces2_status`]), and HDR/wide-gamut presentation (wgpu 29's `SurfaceConfiguration` has no
//! colour-space field; that arrived in wgpu 30). Every one of those is reported `false` by
//! `colour_status`, and this paragraph has to keep agreeing with those flags — an earlier draft claimed
//! "an ACEScg working space ... [is] real and available" when the renderer could not shade in AP1,
//! which is the exact contradiction that makes a reader pick whichever half they hoped for.

use serde::{Deserialize, Serialize};

/// A named colour space an asset can be authored in, or the engine can work in.
///
/// Deliberately a closed set. An open string would let a typo become a silent mis-transform, and the
/// whole point here is that a colour decision is checkable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColourSpace {
    /// sRGB with its piecewise transfer function — what a PNG/JPEG of a photograph is.
    Srgb,
    /// Linear light with Rec.709/sRGB primaries. The engine's default working space.
    ///
    /// The `Default`, and the choice is deliberate: a defaulted colour space appears where something
    /// went unstated, and "scene-linear Rec.709" is the assumption the rest of the engine already
    /// makes. Defaulting to sRGB instead would mean an unstated space silently acquires a transfer
    /// function.
    #[default]
    LinearRec709,
    /// Linear light with AP1 primaries (ACEScg). The film/VFX working space.
    AcesCg,
    /// Linear light with AP0 primaries (ACES2065-1). The ACES archival/interchange space.
    Aces2065_1,
    /// Linear light with Rec.2020 primaries — wide-gamut delivery.
    LinearRec2020,
    /// NOT COLOUR. A normal map, roughness, metallic, AO, a mask, a height field, an ID.
    ///
    /// The most important member of this enum: applying a transfer function to a roughness map is the
    /// classic error, and naming the non-colour case is how it stays impossible rather than unlikely.
    Data,
}

impl ColourSpace {
    /// The author-facing name.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Srgb => "sRGB",
            Self::LinearRec709 => "Linear Rec.709",
            Self::AcesCg => "ACEScg (AP1)",
            Self::Aces2065_1 => "ACES2065-1 (AP0)",
            Self::LinearRec2020 => "Linear Rec.2020",
            Self::Data => "Raw data (not colour)",
        }
    }

    /// True when this carries light rather than measurements.
    #[must_use]
    pub fn is_colour(self) -> bool {
        self != Self::Data
    }

    /// True when values are already linear in light (no transfer function to undo).
    #[must_use]
    pub fn is_linear(self) -> bool {
        matches!(
            self,
            Self::LinearRec709 | Self::AcesCg | Self::Aces2065_1 | Self::LinearRec2020 | Self::Data
        )
    }

    /// Every space, for a UI to offer.
    #[must_use]
    pub fn all() -> &'static [Self] {
        &[
            Self::Srgb,
            Self::LinearRec709,
            Self::AcesCg,
            Self::Aces2065_1,
            Self::LinearRec2020,
            Self::Data,
        ]
    }
}

/// A working space the renderer can operate in. A strict subset of [`ColourSpace`]: you cannot render
/// in "sRGB" or in "data", and making that unrepresentable is better than validating it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkingSpace {
    /// Linear Rec.709. The default: it is what the existing renderer, its lights and its authored
    /// colours already mean, so it is also the choice that changes nothing until asked.
    #[default]
    LinearRec709,
    /// ACEScg. Wider gamut, the film/VFX standard working space.
    AcesCg,
}

impl WorkingSpace {
    /// As a full colour space.
    #[must_use]
    pub fn space(self) -> ColourSpace {
        match self {
            Self::LinearRec709 => ColourSpace::LinearRec709,
            Self::AcesCg => ColourSpace::AcesCg,
        }
    }
    /// The author-facing name.
    #[must_use]
    pub fn label(self) -> &'static str {
        self.space().label()
    }
}

/// How the working space is shown on a display.
///
/// Named for what they ARE, not aspirationally. `AcesFit` is the Narkowicz analytic approximation of
/// the ACES RRT+ODT, which is what the engine actually runs — calling it "ACES" would overstate it,
/// and a colourist comparing against a real ACES ODT would find the difference and rightly distrust
/// everything else the app claimed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ViewTransform {
    /// Narkowicz ACES-like filmic curve — contrasty, film-ish. The default for cinematic work.
    #[default]
    AcesFit,
    /// Khronos PBR Neutral — preserves authored albedo, the right choice for CAD and product review
    /// where "is this the colour I specified" matters more than mood.
    PbrNeutral,
}

impl ViewTransform {
    /// The author-facing name.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::AcesFit => "Filmic (ACES-like)",
            Self::PbrNeutral => "Neutral (Khronos PBR)",
        }
    }
    /// What it is for, one line.
    #[must_use]
    pub fn blurb(self) -> &'static str {
        match self {
            Self::AcesFit => {
                "An analytic approximation of the ACES look — contrasty highlights. Not a reference \
                 ACES ODT; see the colour documentation."
            }
            Self::PbrNeutral => {
                "Preserves authored albedo, so a specified colour reads back as itself. The right \
                 choice for CAD and product review."
            }
        }
    }
}

/// Why an asset ended up in the colour space it did.
///
/// Provenance matters as much as the value: "I chose this" and "the engine guessed this" need
/// different amounts of trust, and a person debugging a wrong-looking texture needs to know which.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColourOrigin {
    /// The file said so (an ICC profile, an EXR chromaticities attribute, a glTF declaration).
    Declared,
    /// Inferred from what the texture is FOR, by the documented policy in [`infer_for_role`].
    InferredFromRole,
    /// A person chose it.
    Manual,
}

/// A texture's colour space plus how it was decided — the explicit metadata this module exists for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColourTag {
    /// The space the pixels are in.
    pub space: ColourSpace,
    /// How that was decided.
    pub origin: ColourOrigin,
}

impl ColourTag {
    /// A tag from the documented role policy.
    #[must_use]
    pub fn inferred(space: ColourSpace) -> Self {
        Self {
            space,
            origin: ColourOrigin::InferredFromRole,
        }
    }
    /// A tag the file itself declared.
    #[must_use]
    pub fn declared(space: ColourSpace) -> Self {
        Self {
            space,
            origin: ColourOrigin::Declared,
        }
    }
    /// A tag a person chose.
    #[must_use]
    pub fn manual(space: ColourSpace) -> Self {
        Self {
            space,
            origin: ColourOrigin::Manual,
        }
    }
    /// One line a UI can show without further formatting.
    #[must_use]
    pub fn describe(self) -> String {
        let how = match self.origin {
            ColourOrigin::Declared => "declared by the file",
            ColourOrigin::InferredFromRole => "assumed from what it is used for",
            ColourOrigin::Manual => "set by you",
        };
        format!("{} — {how}", self.space.label())
    }
}

/// What a texture is used for. The input to the inference policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TextureRole {
    /// Base colour / albedo / diffuse.
    BaseColour,
    /// Emissive radiance.
    Emissive,
    /// Tangent-space normals.
    Normal,
    /// Metallic and/or roughness.
    MetallicRoughness,
    /// Ambient occlusion.
    Occlusion,
    /// A mask, ID, height, displacement or other measurement.
    Mask,
    /// An HDR environment panorama.
    Environment,
}

/// **THE POLICY**, in one place, documented and testable.
///
/// The rule is not a preference: a texture that carries *light* gets a transfer function, and a texture
/// that carries *numbers* does not. Applying sRGB decode to a roughness map darkens the mid-tones by
/// roughly 25–40%, which reads as "the material looks wrong" and is almost never traced back to colour
/// management. Naming the policy here means it can be pointed at, argued with, and tested.
#[must_use]
pub fn infer_for_role(role: TextureRole) -> ColourTag {
    match role {
        // Light: authored by eye in an sRGB paint program.
        TextureRole::BaseColour | TextureRole::Emissive => ColourTag::inferred(ColourSpace::Srgb),
        // Measurements. Never transformed.
        TextureRole::Normal
        | TextureRole::MetallicRoughness
        | TextureRole::Occlusion
        | TextureRole::Mask => ColourTag::inferred(ColourSpace::Data),
        // A Radiance .hdr / EXR panorama is already scene-linear. Its PRIMARIES are conventionally
        // Rec.709 and the format does not record them, so this is an assumption — flagged as inferred
        // precisely so a facility working in AP0 can override it rather than discover it.
        TextureRole::Environment => ColourTag::inferred(ColourSpace::LinearRec709),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════
// Transfer functions — exact, per spec.
// ═══════════════════════════════════════════════════════════════════════════════════════════════

/// sRGB EOTF: encoded value → linear light. IEC 61966-2-1.
///
/// The threshold is **0.04045**, the constant the standard prints, which is also what glTF 2.0 and every
/// GPU's hardware sRGB unit implement. An earlier draft of this function used 0.040448236…, the value
/// that makes the two segments exactly continuous. That number is a *derivation*, not the standard: the
/// published curve is only approximately continuous, by design. The difference between the two is below
/// 1e-8 in output, so this changes no pixel — but the renderer's own copy of this function used 0.04045
/// while this one did not, and two mirrors of one transfer function disagreeing about a constant is
/// exactly the drift this module exists to end. There is now one definition; see `render.rs`, which
/// calls it rather than keeping a second.
#[must_use]
pub fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// sRGB OETF: linear light → encoded value.
#[must_use]
pub fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════
// Primary conversions. Published matrices, row-major, applied as m * v.
//
// These are the standard ACES/colour-science values including the Bradford chromatic adaptation
// between D65 (Rec.709) and D60 (ACES). They are transcribed rather than derived so they match every
// other implementation in a pipeline bit-for-bit at the precision that matters.
//
// `excessive_precision` is allowed DELIBERATELY. Several of these published coefficients carry more
// significant digits than an f32 can hold, and clippy would have us write the truncated value. But the
// reason to transcribe a matrix instead of deriving it is so a reviewer can lay it beside the
// specification and see that it matches. Rounding the source to whatever f32 happens to store makes
// that comparison fail for a reader while changing nothing for the compiler, which rounds identically
// either way. The literal is documentation; the stored value is unaffected.
// ═══════════════════════════════════════════════════════════════════════════════════════════════

/// Linear Rec.709 (D65) → ACEScg / AP1 (D60), Bradford-adapted.
#[allow(clippy::excessive_precision)] // published digits kept verbatim; see above
pub const REC709_TO_AP1: [[f32; 3]; 3] = [
    [0.613_097_4, 0.339_523_1, 0.047_379_5],
    [0.070_193_7, 0.916_353_8, 0.013_452_5],
    [0.020_615_6, 0.109_569_8, 0.869_814_7],
];

/// ACEScg / AP1 (D60) → linear Rec.709 (D65), Bradford-adapted.
#[allow(clippy::excessive_precision)] // published digits kept verbatim; see above
pub const AP1_TO_REC709: [[f32; 3]; 3] = [
    [1.705_051_0, -0.621_792_1, -0.083_258_8],
    [-0.130_256_4, 1.140_802_9, -0.010_546_5],
    [-0.024_003_3, -0.128_968_9, 1.152_972_3],
];

/// AP1 → AP0 (both D60).
#[allow(clippy::excessive_precision)] // published digits kept verbatim; see above
pub const AP1_TO_AP0: [[f32; 3]; 3] = [
    [0.695_452_2, 0.140_678_6, 0.163_869_2],
    [0.044_794_8, 0.859_671_1, 0.095_534_1],
    [-0.005_525_8, 0.004_025_7, 1.001_500_1],
];

/// AP0 → AP1 (both D60).
#[allow(clippy::excessive_precision)] // published digits kept verbatim; see above
pub const AP0_TO_AP1: [[f32; 3]; 3] = [
    [1.451_439_2, -0.236_510_7, -0.214_928_5],
    [-0.076_553_8, 1.176_229_2, -0.099_675_4],
    [0.008_316_2, -0.006_034_7, 0.997_718_5],
];

/// Linear Rec.709 (D65) → linear Rec.2020 (D65).
#[allow(clippy::excessive_precision)] // published digits kept verbatim; see above
pub const REC709_TO_REC2020: [[f32; 3]; 3] = [
    [0.627_403_9, 0.329_283_0, 0.043_313_1],
    [0.069_097_2, 0.919_540_4, 0.011_362_4],
    [0.016_391_4, 0.088_013_2, 0.895_595_4],
];

/// Linear Rec.2020 (D65) → linear Rec.709 (D65).
#[allow(clippy::excessive_precision)] // published digits kept verbatim; see above
pub const REC2020_TO_REC709: [[f32; 3]; 3] = [
    [1.660_490_9, -0.587_641_1, -0.072_849_8],
    [-0.124_550_5, 1.132_899_8, -0.008_349_3],
    [-0.018_154_1, -0.100_597_9, 1.118_752_0],
];

/// Apply a row-major 3x3 to an f64 colour. Used by the chromaticity derivation in the tests, where f32
/// rounding would blur the very disagreement the check exists to find.
#[must_use]
pub fn apply64(m: [[f64; 3]; 3], c: [f64; 3]) -> [f64; 3] {
    [
        m[0][0].mul_add(c[0], m[0][1].mul_add(c[1], m[0][2] * c[2])),
        m[1][0].mul_add(c[0], m[1][1].mul_add(c[1], m[1][2] * c[2])),
        m[2][0].mul_add(c[0], m[2][1].mul_add(c[1], m[2][2] * c[2])),
    ]
}

/// Apply a row-major 3x3 to a colour.
#[must_use]
pub fn apply(m: [[f32; 3]; 3], c: [f32; 3]) -> [f32; 3] {
    [
        m[0][0].mul_add(c[0], m[0][1].mul_add(c[1], m[0][2] * c[2])),
        m[1][0].mul_add(c[0], m[1][1].mul_add(c[1], m[1][2] * c[2])),
        m[2][0].mul_add(c[0], m[2][1].mul_add(c[1], m[2][2] * c[2])),
    ]
}

/// Convert a colour from `from` into `to`.
///
/// [`ColourSpace::Data`] is INERT in both directions: converting a roughness map is meaningless, and
/// returning it untouched is the only correct answer. This is the guarantee the whole module exists
/// to make — it is asserted in the tests rather than left to call-site discipline.
#[must_use]
pub fn convert(c: [f32; 3], from: ColourSpace, to: ColourSpace) -> [f32; 3] {
    use ColourSpace as C;
    if from == to || from == C::Data || to == C::Data {
        return c;
    }
    // 1. Undo any transfer function to reach linear light in the SOURCE primaries.
    let lin = if from == C::Srgb {
        [
            srgb_to_linear(c[0]),
            srgb_to_linear(c[1]),
            srgb_to_linear(c[2]),
        ]
    } else {
        c
    };
    // 2. Cross to linear Rec.709 as the hub, then out. One hub keeps the matrix count linear in the
    //    number of spaces instead of quadratic, and every leg is a published matrix.
    let hub = match from {
        C::Srgb | C::LinearRec709 => lin,
        C::AcesCg => apply(AP1_TO_REC709, lin),
        C::Aces2065_1 => apply(AP1_TO_REC709, apply(AP0_TO_AP1, lin)),
        C::LinearRec2020 => apply(REC2020_TO_REC709, lin),
        C::Data => unreachable!("returned above"),
    };
    let out = match to {
        C::Srgb | C::LinearRec709 => hub,
        C::AcesCg => apply(REC709_TO_AP1, hub),
        C::Aces2065_1 => apply(AP1_TO_AP0, apply(REC709_TO_AP1, hub)),
        C::LinearRec2020 => apply(REC709_TO_REC2020, hub),
        C::Data => unreachable!("returned above"),
    };
    // 3. Re-apply a transfer function if the destination is encoded.
    if to == C::Srgb {
        [
            linear_to_srgb(out[0]),
            linear_to_srgb(out[1]),
            linear_to_srgb(out[2]),
        ]
    } else {
        out
    }
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════
// The working space, as the renderer consumes it.
// ═══════════════════════════════════════════════════════════════════════════════════════════════

/// The 3x3 that changes nothing. Named so the Rec.709 working space is the SAME code path as ACEScg
/// rather than a branch around it: a branch is what lets one of the two drift.
pub const IDENTITY3: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Rec.709 luminance weights (ITU-R BT.709 §3, the Y row of its RGB→XYZ matrix).
pub const REC709_LUMINANCE: [f32; 3] = [0.212_6, 0.715_2, 0.072_2];

/// AP1 luminance weights — the Y row of the AP1→XYZ matrix, i.e. what "how bright is this" means when
/// the values are in ACEScg. Using the Rec.709 weights on AP1 data is the classic working-space bug:
/// bloom then extracts the wrong pixels and an auto-exposure meters the wrong scene.
#[allow(clippy::excessive_precision)] // published digits kept verbatim; see the matrix note above
pub const AP1_LUMINANCE: [f32; 3] = [0.272_228_7, 0.674_081_8, 0.053_689_5];

impl WorkingSpace {
    /// Linear Rec.709 → this working space. The engine's authored values (glTF factors, light colours,
    /// UI-picked colours, imported CAD colours) all mean linear Rec.709, so this is THE matrix that has
    /// to be applied to each of them before any colour computation.
    #[must_use]
    pub fn from_rec709(self) -> [[f32; 3]; 3] {
        match self {
            Self::LinearRec709 => IDENTITY3,
            Self::AcesCg => REC709_TO_AP1,
        }
    }

    /// This working space → linear Rec.709, for handing the frame to a view transform defined on
    /// Rec.709 primaries. See [`ViewTransform::input_space`].
    #[must_use]
    pub fn to_rec709(self) -> [[f32; 3]; 3] {
        match self {
            Self::LinearRec709 => IDENTITY3,
            Self::AcesCg => AP1_TO_REC709,
        }
    }

    /// What "luminance" means in this space.
    #[must_use]
    pub fn luminance_weights(self) -> [f32; 3] {
        match self {
            Self::LinearRec709 => REC709_LUMINANCE,
            Self::AcesCg => AP1_LUMINANCE,
        }
    }

    /// Every working space, for a UI to offer.
    #[must_use]
    pub fn all() -> &'static [Self] {
        &[Self::LinearRec709, Self::AcesCg]
    }

    /// Parse the wire name a command receives. Returns `None` rather than silently defaulting, because
    /// a typo that quietly selects Rec.709 is a colour bug wearing a successful reply.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "linearrec709" | "linear-rec709" | "rec709" | "linear" | "srgb-linear" => {
                Some(Self::LinearRec709)
            }
            "acescg" | "aces-cg" | "ap1" | "aces" => Some(Self::AcesCg),
            _ => None,
        }
    }

    /// The wire name, matching the serde representation.
    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            Self::LinearRec709 => "linearRec709",
            Self::AcesCg => "acesCg",
        }
    }
}

/// Row-major 3x3 product, `a * b` — apply `b` first, then `a`.
///
/// Exists so a source→working conversion can be COMPOSED into one matrix instead of applied as two
/// per-pixel multiplies. Two matrices in the shader would also mean two places for a source override to
/// be forgotten.
#[must_use]
pub fn mul3(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut out = [[0.0_f32; 3]; 3];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    out
}

impl ColourSpace {
    /// The matrix taking this space's values into linear Rec.709, when that is a pure matrix.
    ///
    /// `None` for [`ColourSpace::Srgb`] (there is a transfer function to undo first, so a matrix alone
    /// would be wrong) and for [`ColourSpace::Data`] (there is nothing to convert). Returning `None`
    /// rather than the identity is deliberate: a caller that wants a matrix and gets one silently for
    /// sRGB would double-encode, and the compiler should make them decide.
    #[must_use]
    pub fn linear_to_rec709(self) -> Option<[[f32; 3]; 3]> {
        match self {
            Self::LinearRec709 => Some(IDENTITY3),
            Self::AcesCg => Some(AP1_TO_REC709),
            Self::Aces2065_1 => Some(mul3(AP1_TO_REC709, AP0_TO_AP1)),
            Self::LinearRec2020 => Some(REC2020_TO_REC709),
            Self::Srgb | Self::Data => None,
        }
    }

    /// Every space a floating-point source (an EXR/Radiance environment) can be declared in — the ones
    /// [`ColourSpace::linear_to_rec709`] can answer for.
    #[must_use]
    pub fn linear_options() -> &'static [Self] {
        &[
            Self::LinearRec709,
            Self::AcesCg,
            Self::Aces2065_1,
            Self::LinearRec2020,
        ]
    }

    /// Parse the wire name a command receives; `None` rather than a silent default.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "srgb" => Some(Self::Srgb),
            "linearrec709" | "linear-rec709" | "rec709" | "linear" => Some(Self::LinearRec709),
            "acescg" | "aces-cg" | "ap1" => Some(Self::AcesCg),
            "aces2065-1" | "aces20651" | "ap0" => Some(Self::Aces2065_1),
            "linearrec2020" | "rec2020" => Some(Self::LinearRec2020),
            "data" => Some(Self::Data),
            _ => None,
        }
    }

    /// The wire name, matching the serde representation.
    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            Self::Srgb => "srgb",
            Self::LinearRec709 => "linearRec709",
            Self::AcesCg => "acesCg",
            Self::Aces2065_1 => "aces2065_1",
            Self::LinearRec2020 => "linearRec2020",
            Self::Data => "data",
        }
    }
}

/// The matrix that takes a LINEAR source space straight into the working space, composed once.
///
/// `None` when the source is not a linear colour space — the caller has a transfer function to deal
/// with, and pretending otherwise is how an environment map gets decoded twice.
#[must_use]
pub fn source_to_working(source: ColourSpace, working: WorkingSpace) -> Option<[[f32; 3]; 3]> {
    Some(mul3(working.from_rec709(), source.linear_to_rec709()?))
}

/// Transpose a row-major 3x3 into the column-major, 16-byte-column layout a WGSL `mat3x3<f32>` uniform
/// requires.
///
/// Two conversions in one function on purpose. The matrices in this module are written row-major so a
/// reviewer can lay them beside the published table; WGSL wants columns, each padded to a `vec4`. Doing
/// both here means a shader upload cannot get one right and the other wrong — which is a silent
/// transpose, and a transposed primaries matrix looks *almost* right, which is the worst kind of wrong.
#[must_use]
pub fn wgsl_mat3(m: [[f32; 3]; 3]) -> [[f32; 4]; 3] {
    [
        [m[0][0], m[1][0], m[2][0], 0.0],
        [m[0][1], m[1][1], m[2][1], 0.0],
        [m[0][2], m[1][2], m[2][2], 0.0],
    ]
}

impl ViewTransform {
    /// The colour space this transform is DEFINED on.
    ///
    /// A view transform is not space-agnostic: the Khronos PBR Neutral reference curve is specified on
    /// linear Rec.709/sRGB primaries, and the Narkowicz fit was published as a curve applied to the same.
    /// So when the renderer works in AP1, the frame must be brought back to Rec.709 before the curve —
    /// otherwise the curve's per-channel behaviour is being applied to channels that do not mean what it
    /// assumes, which shows up as a hue shift in saturated highlights. Declaring the input space here is
    /// what lets [`crate::colour`]'s consumers convert instead of assume.
    #[must_use]
    pub fn input_space(self) -> ColourSpace {
        match self {
            Self::AcesFit | Self::PbrNeutral => ColourSpace::LinearRec709,
        }
    }

    /// Every view transform, for a UI to offer.
    #[must_use]
    pub fn all() -> &'static [Self] {
        &[Self::AcesFit, Self::PbrNeutral]
    }

    /// Parse the wire name a command receives.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "acesfit" | "aces-fit" | "filmic" | "cinematic" | "film" => Some(Self::AcesFit),
            "pbrneutral" | "pbr-neutral" | "neutral" | "cad" => Some(Self::PbrNeutral),
            _ => None,
        }
    }

    /// The wire name, matching the serde representation.
    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            Self::AcesFit => "acesFit",
            Self::PbrNeutral => "pbrNeutral",
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════
// The decision: one policy, with precedence and a reason.
// ═══════════════════════════════════════════════════════════════════════════════════════════════

/// What is known about one asset when its colour space is decided.
///
/// Modelled as a request rather than three arguments so that adding a source of truth (an ICC profile,
/// a KTX2 data-format descriptor) is a field with a documented precedence, not another `if` at a call
/// site that some other importer will forget.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ColourQuery {
    /// What the texture is for. The one thing every importer knows.
    pub role: Option<TextureRole>,
    /// What the FILE said, when the format records it and the reader preserves it.
    pub declared: Option<ColourSpace>,
    /// What a person chose, in the asset inspector.
    pub chosen: Option<ColourSpace>,
}

/// A decided colour space, why, and the sentence to show the person whose texture looks wrong.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ColourDecision {
    /// The space the pixels are to be interpreted in.
    pub space: ColourSpace,
    /// How it was decided.
    pub origin: ColourOrigin,
    /// Plain language, naming the evidence — "sRGB, because this image is bound as base colour".
    pub reason: String,
    /// Set when the request asked for something the policy refused, with the refusal explained. A
    /// refusal is reported rather than swallowed: silently ignoring an override is how a person spends
    /// an afternoon re-exporting a texture that was never the problem.
    pub refused: Option<String>,
}

impl ColourDecision {
    /// The tag form, for the parts of the engine that only need the two facts.
    #[must_use]
    pub fn tag(&self) -> ColourTag {
        ColourTag {
            space: self.space,
            origin: self.origin,
        }
    }
}

/// **THE PRECEDENCE**, in one place: a person's choice, then the file's own declaration, then the role
/// policy, then the conservative fallback.
///
/// The one rule that is not a preference: an override may not turn measurements into colour. Roughness
/// is not "roughness in sRGB" — a texture bound as roughness is data, and the way to make it colour is
/// to change what it is USED FOR, not to relabel its encoding. The prompt for this work states it as
/// "distinguish changing the texture role from changing colour encoding"; here that distinction is
/// enforced rather than documented, and the attempt comes back as a refusal with the reason.
#[must_use]
pub fn decide(query: ColourQuery) -> ColourDecision {
    let role = query.role;
    let policy = role.map_or(
        // No role at all: the conservative fallback. sRGB is the right guess for an 8-bit image a human
        // made, and it is the guess that is visible when wrong (too dark / too light) rather than the
        // one that is subtly wrong forever.
        ColourTag::inferred(ColourSpace::Srgb),
        infer_for_role,
    );
    let is_data_role = policy.space == ColourSpace::Data;

    if let Some(chosen) = query.chosen {
        if is_data_role && chosen != ColourSpace::Data {
            let role_name = role.map_or("this texture", TextureRole::label);
            return ColourDecision {
                space: ColourSpace::Data,
                origin: ColourOrigin::InferredFromRole,
                reason: format!(
                    "Raw data (not colour), because {role_name} carries measurements, not light."
                ),
                refused: Some(format!(
                    "{} was not applied: {role_name} is data. Change what the texture is used for if it \
                     should carry colour — relabelling its encoding would apply a transfer function to \
                     measurements.",
                    chosen.label()
                )),
            };
        }
        return ColourDecision {
            space: chosen,
            origin: ColourOrigin::Manual,
            reason: format!("{} — set by you.", chosen.label()),
            refused: None,
        };
    }

    if let Some(declared) = query.declared {
        if is_data_role && declared != ColourSpace::Data {
            return ColourDecision {
                space: ColourSpace::Data,
                origin: ColourOrigin::InferredFromRole,
                reason: format!(
                    "Raw data (not colour), because {} carries measurements. The file declared {}, \
                     which is ignored for a data channel.",
                    role.map_or("this texture", TextureRole::label),
                    declared.label()
                ),
                refused: None,
            };
        }
        return ColourDecision {
            space: declared,
            origin: ColourOrigin::Declared,
            reason: format!("{} — declared by the file.", declared.label()),
            refused: None,
        };
    }

    ColourDecision {
        space: policy.space,
        origin: policy.origin,
        reason: match role {
            Some(r) => format!("{} — {}", policy.space.label(), r.why()),
            None => format!(
                "{} — assumed, because nothing said otherwise and this is what an 8-bit image a person \
                 painted almost always is.",
                policy.space.label()
            ),
        },
        refused: None,
    }
}

impl TextureRole {
    /// The author-facing name.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::BaseColour => "base colour",
            Self::Emissive => "emissive",
            Self::Normal => "the normal map",
            Self::MetallicRoughness => "metallic/roughness",
            Self::Occlusion => "ambient occlusion",
            Self::Mask => "a mask",
            Self::Environment => "the environment",
        }
    }

    /// Why the policy says what it says, as a clause that completes "<space> — …".
    #[must_use]
    pub fn why(self) -> &'static str {
        match self {
            Self::BaseColour => {
                "because this image is bound as base colour, which glTF requires to be sRGB-encoded."
            }
            Self::Emissive => {
                "because this image is bound as emissive, which glTF requires to be sRGB-encoded."
            }
            Self::Normal => "because a normal map stores directions, not light.",
            Self::MetallicRoughness => {
                "because metallic/roughness stores measurements; a transfer function here darkens the \
                 mid-tones and reads as 'the material looks wrong'."
            }
            Self::Occlusion => "because occlusion stores a visibility fraction, not light.",
            Self::Mask => "because a mask stores a number, not light.",
            Self::Environment => {
                "because an HDR/EXR panorama is already scene-linear. Its primaries are assumed \
                 Rec.709 — the format does not record them, so override this if your facility \
                 authors in another gamut."
            }
        }
    }

    /// True when a person may choose this texture's colour space at all. Offering the control for data
    /// would be offering a way to break it.
    #[must_use]
    pub fn accepts_colour_override(self) -> bool {
        infer_for_role(self).space != ColourSpace::Data
    }

    /// Every role, for a UI to offer.
    #[must_use]
    pub fn all() -> &'static [Self] {
        &[
            Self::BaseColour,
            Self::Emissive,
            Self::Normal,
            Self::MetallicRoughness,
            Self::Occlusion,
            Self::Mask,
            Self::Environment,
        ]
    }
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════
// Presentation state — a viewing choice, kept apart from scene truth.
// ═══════════════════════════════════════════════════════════════════════════════════════════════

/// How the project is being LOOKED at. Not what it is.
///
/// The distinction is the whole point: which tone curve a person prefers is not a fact about the
/// geometry, so it must not dirty the document, enter undo, or change the content hash — and it must
/// still be there tomorrow morning. Both halves of that are requirements; a system that satisfies only
/// one of them is the usual failure.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresentationState {
    /// What the renderer works in.
    pub working: WorkingSpace,
    /// How the working space is shown.
    pub view: ViewTransform,
    /// The linear multiplier applied before the view transform.
    pub exposure: f32,
}

impl Default for PresentationState {
    fn default() -> Self {
        Self {
            working: WorkingSpace::default(),
            view: ViewTransform::default(),
            // The renderer's own default, restated here rather than imported: this crate cannot depend
            // on the shell. `the_presentation_default_matches_the_renderer` in render.rs pins them
            // together, so a drift is a test failure and not a look change nobody notices.
            exposure: 0.45,
        }
    }
}

impl PresentationState {
    /// A 64-bit identity for this state — the thing an async render result must carry so a consumer can
    /// tell whether the picture it got was made from the state it asked about.
    ///
    /// FNV-1a over the canonical bytes, and the exposure goes in as its bit pattern so two exposures
    /// that differ below display threshold still hash differently. An over-sensitive hash costs a
    /// re-render; an under-sensitive one returns a picture of the wrong state, which is the bug.
    #[must_use]
    pub fn hash(self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |byte: u8| {
            h ^= u64::from(byte);
            h = h.wrapping_mul(0x1000_0000_01b3);
        };
        for b in self.working.wire().bytes() {
            eat(b);
        }
        eat(0xff);
        for b in self.view.wire().bytes() {
            eat(b);
        }
        eat(0xff);
        for b in self.exposure.to_bits().to_le_bytes() {
            eat(b);
        }
        h
    }
}

/// Which implementation performs a colour transform.
///
/// An enum rather than a bool because the honest answer has three values, and a "use OCIO" checkbox
/// that silently falls back to the built-in transforms is precisely the lie this module exists to
/// prevent — a menu backed by a different transform than the one it names.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransformBackend {
    /// The published matrices and curves implemented here, reference-tested against the specifications.
    BuiltIn,
    /// A studio's own `.ocio` configuration, through the OpenColorIO library.
    Ocio,
}

impl TransformBackend {
    /// The author-facing name.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::BuiltIn => "Built-in (published transforms)",
            Self::Ocio => "OpenColorIO (studio config)",
        }
    }

    /// Whether this build can actually run it.
    #[must_use]
    pub fn available(self) -> bool {
        match self {
            Self::BuiltIn => true,
            Self::Ocio => ocio_status().available,
        }
    }
}

/// Whether a real OCIO config can be used, and if not, precisely why.
///
/// Reported rather than hidden: a facility whose whole pipeline is defined by an `.ocio` file needs to
/// know before they start, not after a shot comes back wrong. Structured so that wiring `ocio-rs` in a
/// build that HAS the toolchain flips one function rather than rewriting the colour layer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OcioStatus {
    /// True when an `.ocio` config can be loaded.
    pub available: bool,
    /// What is and is not possible, in the author's language.
    pub detail: String,
}

/// The current OCIO situation for this build.
///
/// Every clause below is a checked fact as of 2026-08-16, not a recollection:
///
/// * OpenColorIO's current release is **2.5.2** (2026-05-13), and its GPU path (`GpuShaderDesc`) emits
///   GLSL, HLSL, MSL, Cg and OSL — **there is no WGSL target**. So even with the library present, this
///   renderer could not take OCIO's generated shader directly; the integration would be a baked
///   shaper+3D LUT sampled from WGSL, or a transpile, and each of those is an approximation that owes
///   a quantified error before it may be called "the studio's transform".
/// * The maintained Rust bindings are `ocio-rs`/`ocio-sys` 0.2.1, wrapping OCIO 2.5.2. They are FFI to
///   the C++ library.
/// * **`cmake` is not on this machine's PATH** (checked, not assumed), and neither is `vcpkg`, so the
///   native library cannot be built here at all.
///
/// The honest consequence is that a studio config cannot drive this viewport, and saying otherwise —
/// or showing a display/view menu backed by the built-in curve — would be worse than the gap.
#[must_use]
pub fn ocio_status() -> OcioStatus {
    OcioStatus {
        available: false,
        detail: "Loading a studio .ocio config is not available in this build, for two separate \
                 reasons. (1) The Rust bindings (ocio-rs 0.2.1) wrap the C++ OpenColorIO library, \
                 which needs a cmake toolchain this machine does not have. (2) OpenColorIO 2.5's GPU \
                 path emits GLSL, HLSL, MSL, Cg and OSL — not WGSL — so this wgpu renderer would have \
                 to sample a baked LUT or transpile, and neither may be labelled as your config's \
                 transform without a measured error figure. What IS here: the colour spaces, primary \
                 conversions and transfer functions are the published exact ones and are \
                 reference-tested against the chromaticities they derive from, so converting between \
                 sRGB, linear Rec.709, ACEScg, ACES2065-1 and Rec.2020 is exact, and the renderer can \
                 work in ACEScg. What is missing is reading YOUR config file, its displays/views/looks, \
                 and arbitrary LUT/CTF transforms from it."
            .into(),
    }
}

/// Why the built-in view transforms are not called ACES 2, stated once so no UI has to paraphrase it.
///
/// ACES 2.0 was released for end users in 2025 and is a single unified Output Transform, not the old
/// RRT+ODT pair: ACES→JMh (a simplified Hellwig 2022 appearance model), a tone scale on J, chroma
/// compression on M, gamut compression on J and M, white limiting, then display encoding. Its reference
/// implementation is CTL in `aces-aswf/aces-core` and runs to roughly 1,700 lines before the support
/// libraries; there is no published WGSL or HLSL port, and OCIO 2.5 ships it as native C++ rather than
/// as anything portable.
///
/// Implementing that from a description — with no reference imagery in this repository to check the
/// result against — would produce a curve that could not be told apart from a mistake, under a name a
/// colourist would trust. So it is named as absent, precisely.
#[must_use]
pub fn aces2_status() -> OcioStatus {
    OcioStatus {
        available: false,
        detail: "The ACES 2.0 Output Transform is not implemented. The 'Filmic (ACES-like)' view is the \
                 Narkowicz analytic fit, which approximates the LOOK of the older ACES 1 RRT+ODT and is \
                 named for what it is. ACES 2.0 is a different architecture — ACES→JMh, tone scale, \
                 chroma compression, gamut compression, white limiting, display encoding — whose \
                 reference implementation is ~1,700 lines of CTL with no portable GPU port, and this \
                 repository holds no ACES reference imagery to verify a port against. An unverified \
                 curve carrying the name 'ACES 2' would be worse than its absence."
            .into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Published reference points for the sRGB transfer function.
    #[test]
    fn the_srgb_transfer_function_matches_its_reference_values() {
        // Black and white are exact by definition.
        assert!((srgb_to_linear(0.0)).abs() < 1e-7);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
        // Mid-grey: sRGB 0.5 is ~0.2140 linear. This is THE number people check.
        assert!(
            (srgb_to_linear(0.5) - 0.214_041).abs() < 1e-4,
            "sRGB 0.5 → {}",
            srgb_to_linear(0.5)
        );
        // 18% linear grey encodes to ~0.4626 sRGB — the photographic reference.
        assert!(
            (linear_to_srgb(0.18) - 0.462_1).abs() < 1e-3,
            "linear 0.18 → {}",
            linear_to_srgb(0.18)
        );
        // The piecewise join is continuous — a discontinuity here shows up as banding in dark areas.
        let t = 0.040_448_237;
        assert!((srgb_to_linear(t) - srgb_to_linear(t + 1e-6)).abs() < 1e-5);
    }

    #[test]
    fn the_transfer_function_round_trips() {
        for i in 0u8..=100 {
            let v = f32::from(i) / 100.0;
            let back = linear_to_srgb(srgb_to_linear(v));
            assert!((back - v).abs() < 1e-4, "{v} → {back}");
        }
    }

    #[test]
    fn white_stays_white_across_every_gamut() {
        // The single strongest check on a set of primary matrices: a chromatic-adaptation error shows
        // up immediately as a white point that drifts off neutral.
        for space in [
            ColourSpace::AcesCg,
            ColourSpace::Aces2065_1,
            ColourSpace::LinearRec2020,
        ] {
            let w = convert([1.0, 1.0, 1.0], ColourSpace::LinearRec709, space);
            for (i, c) in w.iter().enumerate() {
                assert!(
                    (c - 1.0).abs() < 0.005,
                    "{} white channel {i} drifted to {c}",
                    space.label()
                );
            }
        }
    }

    #[test]
    fn every_gamut_conversion_round_trips() {
        let probes = [
            [0.18, 0.18, 0.18],
            [0.8, 0.2, 0.1],
            [0.05, 0.6, 0.35],
            [4.0, 2.0, 0.5], // HDR: above 1.0 must survive too
        ];
        for space in [
            ColourSpace::AcesCg,
            ColourSpace::Aces2065_1,
            ColourSpace::LinearRec2020,
        ] {
            for p in probes {
                let there = convert(p, ColourSpace::LinearRec709, space);
                let back = convert(there, space, ColourSpace::LinearRec709);
                for i in 0..3 {
                    assert!(
                        (back[i] - p[i]).abs() < 2e-3,
                        "{} channel {i}: {} → {} → {}",
                        space.label(),
                        p[i],
                        there[i],
                        back[i]
                    );
                }
            }
        }
    }

    /// Build the RGB->XYZ matrix for a set of primaries and a white point, by the standard method.
    ///
    /// This exists so the tests can check the transcribed matrices against **different data**. Asserting
    /// that `convert([1,0,0], Rec709, AcesCg)` equals the first column of `REC709_TO_AP1` proves only
    /// that matrix multiplication works: swap in a wrong matrix and the "reference" numbers get swapped
    /// with it. Chromaticities are an independent source — they are the primaries' actual definition,
    /// and the matrix is supposed to be their consequence.
    fn rgb_to_xyz(primaries: [[f64; 2]; 3], white: [f64; 2]) -> [[f64; 3]; 3] {
        let mut basis = [[0.0_f64; 3]; 3];
        for (col, chroma) in primaries.iter().enumerate() {
            let (cx, cy) = (chroma[0], chroma[1]);
            basis[0][col] = cx / cy;
            basis[1][col] = 1.0;
            basis[2][col] = (1.0 - cx - cy) / cy;
        }
        // Scale each primary column so the three together sum to the white point.
        let scale = solve3(basis, white_xyz(white));
        for row in &mut basis {
            for (col, cell) in row.iter_mut().enumerate() {
                *cell *= scale[col];
            }
        }
        basis
    }

    /// A white point's chromaticity as XYZ at unit luminance.
    fn white_xyz(white: [f64; 2]) -> [f64; 3] {
        [
            white[0] / white[1],
            1.0,
            (1.0 - white[0] - white[1]) / white[1],
        ]
    }

    /// Solve `matrix * x = rhs` by Gauss-Jordan with partial pivoting. Small and explicit on purpose.
    fn solve3(matrix: [[f64; 3]; 3], rhs: [f64; 3]) -> [f64; 3] {
        let mut aug = [[0.0_f64; 4]; 3];
        for (row, (src, val)) in aug.iter_mut().zip(matrix.iter().zip(rhs.iter())) {
            row[..3].copy_from_slice(src);
            row[3] = *val;
        }
        for col in 0..3 {
            let pivot = (col..3)
                .max_by(|&lhs, &rhs_i| aug[lhs][col].abs().total_cmp(&aug[rhs_i][col].abs()))
                .unwrap();
            aug.swap(col, pivot);
            let lead = aug[col][col];
            for cell in &mut aug[col] {
                *cell /= lead;
            }
            let pivot_row = aug[col];
            for (row_index, row) in aug.iter_mut().enumerate() {
                if row_index != col {
                    let factor = row[col];
                    for (cell, pivot_cell) in row.iter_mut().zip(pivot_row.iter()) {
                        *cell -= factor * pivot_cell;
                    }
                }
            }
        }
        [aug[0][3], aug[1][3], aug[2][3]]
    }

    fn mul3(lhs: [[f64; 3]; 3], rhs: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
        let mut out = [[0.0_f64; 3]; 3];
        for (row_index, row) in out.iter_mut().enumerate() {
            for (col, cell) in row.iter_mut().enumerate() {
                *cell = (0..3).map(|k| lhs[row_index][k] * rhs[k][col]).sum();
            }
        }
        out
    }

    fn invert3(matrix: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
        let mut out = [[0.0_f64; 3]; 3];
        for col in 0..3 {
            let mut unit = [0.0; 3];
            unit[col] = 1.0;
            let solved = solve3(matrix, unit);
            for (row_index, row) in out.iter_mut().enumerate() {
                row[col] = solved[row_index];
            }
        }
        out
    }

    #[test]
    fn the_transcribed_matrices_agree_with_the_published_chromaticities() {
        // INDEPENDENT DATA. These are the primaries and white points from the specifications, not the
        // matrices under test. Deriving a matrix from them and comparing is a real check: change a
        // matrix constant and this fails, because the chromaticities did not change with it.
        //
        // Rec.709 / sRGB (ITU-R BT.709), D65.
        let rec709 = [[0.640, 0.330], [0.300, 0.600], [0.150, 0.060]];
        let d65 = [0.312_7, 0.329_0];
        // ACES AP1 (SMPTE ST 2065-4), D60 "ACES white".
        let ap1 = [[0.713, 0.293], [0.165, 0.830], [0.128, 0.044]];
        let d60 = [0.321_68, 0.337_67];
        // ACES AP0 (SMPTE ST 2065-1), same white.
        let ap0 = [[0.7347, 0.2653], [0.0, 1.0], [0.0001, -0.0770]];
        // ITU-R BT.2020, D65.
        let rec2020 = [[0.708, 0.292], [0.170, 0.797], [0.131, 0.046]];

        // The Bradford cone-response matrix — also published data, not derived from ours.
        let bradford = [
            [0.8951, 0.2664, -0.1614],
            [-0.7502, 1.7135, 0.0367],
            [0.0389, -0.0685, 1.0296],
        ];
        let xyz_of = |w: [f64; 2]| [w[0] / w[1], 1.0, (1.0 - w[0] - w[1]) / w[1]];
        let adapt = |from: [f64; 2], to: [f64; 2]| {
            let cf = apply64(bradford, xyz_of(from));
            let ct = apply64(bradford, xyz_of(to));
            let scale = [
                [ct[0] / cf[0], 0.0, 0.0],
                [0.0, ct[1] / cf[1], 0.0],
                [0.0, 0.0, ct[2] / cf[2]],
            ];
            mul3(invert3(bradford), mul3(scale, bradford))
        };

        let check = |name: &str, derived: [[f64; 3]; 3], actual: [[f32; 3]; 3]| {
            for r in 0..3 {
                for c in 0..3 {
                    let d = derived[r][c];
                    let a = f64::from(actual[r][c]);
                    assert!(
                        (d - a).abs() < 2e-4,
                        "{name}[{r}][{c}]: chromaticities imply {d:.6}, constant says {a:.6}"
                    );
                }
            }
        };

        let m709 = rgb_to_xyz(rec709, d65);
        let m_ap1 = rgb_to_xyz(ap1, d60);
        let m_ap0 = rgb_to_xyz(ap0, d60);
        let m2020 = rgb_to_xyz(rec2020, d65);

        check(
            "REC709_TO_AP1",
            mul3(invert3(m_ap1), mul3(adapt(d65, d60), m709)),
            REC709_TO_AP1,
        );
        check(
            "AP1_TO_REC709",
            mul3(invert3(m709), mul3(adapt(d60, d65), m_ap1)),
            AP1_TO_REC709,
        );
        // AP1 and AP0 share a white point, so no adaptation belongs in these two.
        check("AP1_TO_AP0", mul3(invert3(m_ap0), m_ap1), AP1_TO_AP0);
        check("AP0_TO_AP1", mul3(invert3(m_ap1), m_ap0), AP0_TO_AP1);
        // As do Rec.709 and Rec.2020.
        check(
            "REC709_TO_REC2020",
            mul3(invert3(m2020), m709),
            REC709_TO_REC2020,
        );
        check(
            "REC2020_TO_REC709",
            mul3(invert3(m709), m2020),
            REC2020_TO_REC709,
        );
    }

    #[test]
    fn converting_between_gamuts_preserves_luminance() {
        // A second independent constraint, from the published luminance weights rather than from the
        // matrices: Rec.709's Y row is (0.2126, 0.7152, 0.0722) and AP1's is
        // (0.272229, 0.674082, 0.053690). A colour's brightness cannot change merely because it was
        // described in different primaries, so a wrong row shows up here as an energy change even when
        // that row still sums to one — which is all `white_stays_white` would have caught.
        let y709 = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let y_ap1 = |c: [f32; 3]| 0.272_229 * c[0] + 0.674_082 * c[1] + 0.053_690 * c[2];
        for c in [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.18, 0.18, 0.18],
            [0.7, 0.3, 0.1],
            [4.0, 2.5, 0.9],
        ] {
            let out = convert(c, ColourSpace::LinearRec709, ColourSpace::AcesCg);
            let (a, b) = (y709(c), y_ap1(out));
            assert!(
                (a - b).abs() < 0.006 * (1.0 + a),
                "luminance moved: {c:?} Y={a:.5} -> {out:?} Y={b:.5}"
            );
        }
    }

    #[test]
    fn a_saturated_red_lands_inside_the_wider_gamut() {
        // The qualitative check the old "published reference" test was really making. Kept, but no
        // longer pretending to verify the matrix against an outside source — the two tests above do
        // that, against data the matrices are not derived from.
        let out = convert(
            [1.0, 0.0, 0.0],
            ColourSpace::LinearRec709,
            ColourSpace::AcesCg,
        );
        assert!(
            out[1] > 0.0 && out[2] > 0.0,
            "a wider gamut has no negative-free pure red: {out:?}"
        );
        assert!(
            out[0] > out[1] && out[0] > out[2],
            "red must stay red: {out:?}"
        );
    }

    #[test]
    // Exact equality is the CLAIM here, not a sloppy comparison. "Data came back within an epsilon"
    // would pass for a conversion that quietly round-tripped the values through a matrix and landed
    // near where it started — which is exactly the bug this test exists to forbid. Data must come back
    // bit-identical because nothing touched it.
    #[allow(clippy::float_cmp)]
    fn data_is_never_transformed_in_either_direction() {
        // The most important test in this module. A roughness map that receives an sRGB decode is
        // darkened ~25-40% in the mid-tones, reads as "the material looks wrong", and is almost never
        // traced back to colour management.
        let roughness = [0.5, 0.5, 0.5];
        for other in ColourSpace::all() {
            assert_eq!(
                convert(roughness, ColourSpace::Data, *other),
                roughness,
                "data was transformed on the way OUT to {}",
                other.label()
            );
            assert_eq!(
                convert(roughness, *other, ColourSpace::Data),
                roughness,
                "data was transformed on the way IN from {}",
                other.label()
            );
        }
    }

    #[test]
    fn the_role_policy_gives_light_a_curve_and_measurements_none() {
        use TextureRole as R;
        for role in [R::BaseColour, R::Emissive] {
            assert_eq!(infer_for_role(role).space, ColourSpace::Srgb, "{role:?}");
        }
        for role in [R::Normal, R::MetallicRoughness, R::Occlusion, R::Mask] {
            assert_eq!(
                infer_for_role(role).space,
                ColourSpace::Data,
                "{role:?} must not receive a colour transform"
            );
            assert!(!infer_for_role(role).space.is_colour());
        }
        // An HDR panorama is already linear — decoding it again would square the transfer function.
        let env = infer_for_role(R::Environment);
        assert!(env.space.is_linear());
        assert_eq!(env.origin, ColourOrigin::InferredFromRole);
    }

    #[test]
    fn every_tag_can_say_how_it_was_decided() {
        // "I chose this" and "the engine guessed" need different trust, and the person debugging a
        // wrong-looking texture is exactly who needs to tell them apart.
        assert!(ColourTag::manual(ColourSpace::Srgb)
            .describe()
            .contains("set by you"));
        assert!(ColourTag::declared(ColourSpace::AcesCg)
            .describe()
            .contains("declared"));
        assert!(infer_for_role(TextureRole::BaseColour)
            .describe()
            .contains("assumed"));
    }

    #[test]
    fn a_working_space_is_always_linear_and_never_data() {
        for w in [WorkingSpace::LinearRec709, WorkingSpace::AcesCg] {
            assert!(w.space().is_linear(), "{} must be linear", w.label());
            assert!(w.space().is_colour(), "{} must carry light", w.label());
        }
    }

    // ── the working space ─────────────────────────────────────────────────────────────────────────

    #[test]
    // Exact equality is the CLAIM, as it is for `data_is_never_transformed_in_either_direction`: the
    // Rec.709 path must be bit-identical, not merely close. "Within an epsilon" would pass for a
    // pipeline that quietly multiplied by a near-identity matrix — which is precisely the regression
    // this test exists to forbid.
    #[allow(clippy::float_cmp)]
    fn the_rec709_working_space_is_exactly_the_identity() {
        // The property the whole migration rests on: adopting a working space changes nothing until
        // someone selects a different one, so every capture taken before it is still comparable.
        assert_eq!(WorkingSpace::LinearRec709.from_rec709(), IDENTITY3);
        assert_eq!(WorkingSpace::LinearRec709.to_rec709(), IDENTITY3);
        assert_eq!(
            WorkingSpace::LinearRec709.luminance_weights(),
            REC709_LUMINANCE
        );
    }

    #[test]
    fn a_working_space_round_trips_including_the_values_a_zero_to_one_test_would_miss() {
        // Negative and over-range values are the point: scene-linear radiance is not in [0,1], and a
        // conversion that only works there is a conversion that clips the highlights it exists to keep.
        // A saturated Rec.709 blue lands OUTSIDE AP1's positive octant on one channel — legitimately —
        // and it must survive the trip back.
        for w in WorkingSpace::all() {
            for probe in [
                [0.0, 0.0, 0.0],
                [0.18, 0.18, 0.18],
                [1.0, 1.0, 1.0],
                [12.0, 8.0, 3.0],  // over-range highlight
                [0.0, 0.0, 1.0],   // gamut corner
                [-0.02, 0.4, 0.9], // already out of gamut on the way in
            ] {
                let there = apply(w.from_rec709(), probe);
                let back = apply(w.to_rec709(), there);
                for i in 0..3 {
                    assert!(
                        (back[i] - probe[i]).abs() < 1e-3 * (1.0 + probe[i].abs()),
                        "{} channel {i}: {} → {} → {}",
                        w.label(),
                        probe[i],
                        there[i],
                        back[i]
                    );
                }
            }
        }
    }

    #[test]
    // The `assert_ne!` below is an exact comparison on purpose: AP1's weights must not BE Rec.709's.
    // Any difference at all satisfies it, so tolerance is meaningless here.
    #[allow(clippy::float_cmp)]
    fn the_working_space_luminance_weights_are_its_own() {
        // AP1's Y row, not Rec.709's. Using the wrong one is the working-space bug that reads as a
        // mis-tuned bloom threshold rather than as a colour error.
        let ap1 = WorkingSpace::AcesCg.luminance_weights();
        assert_ne!(ap1, REC709_LUMINANCE);
        // They must sum to 1 — a set of luminance weights that does not is an exposure change hiding in
        // a colour matrix.
        for w in [REC709_LUMINANCE, ap1] {
            assert!(
                (w[0] + w[1] + w[2] - 1.0).abs() < 1e-5,
                "{w:?} must sum to 1"
            );
        }
        // And they must AGREE with the primaries matrix they claim to describe: converting a colour
        // into AP1 and metering it with these weights has to give the same brightness as metering the
        // original with Rec.709's. Independent data, not a restatement.
        for c in [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.3, 0.6, 0.9],
            [5.0, 2.0, 0.4],
        ] {
            let y709: f32 = (0..3).map(|i| REC709_LUMINANCE[i] * c[i]).sum();
            let in_ap1 = apply(WorkingSpace::AcesCg.from_rec709(), c);
            let y_ap1: f32 = (0..3).map(|i| ap1[i] * in_ap1[i]).sum();
            assert!(
                (y709 - y_ap1).abs() < 0.006 * (1.0 + y709),
                "brightness changed with the description: {c:?} {y709} vs {y_ap1}"
            );
        }
    }

    #[test]
    fn a_view_transform_declares_the_space_it_is_defined_on() {
        // Both curves are specified on linear Rec.709 primaries. Declaring it is what lets the resolve
        // convert instead of assume — the assumption is invisible on a grey ramp and shows as a hue
        // shift on a saturated highlight.
        for v in ViewTransform::all() {
            assert_eq!(v.input_space(), ColourSpace::LinearRec709, "{}", v.label());
        }
    }

    #[test]
    fn the_wire_names_round_trip_and_a_typo_is_refused() {
        for w in WorkingSpace::all() {
            assert_eq!(WorkingSpace::parse(w.wire()), Some(*w));
        }
        for v in ViewTransform::all() {
            assert_eq!(ViewTransform::parse(v.wire()), Some(*v));
        }
        // A near-miss must NOT quietly select the default: silently rendering in Rec.709 while the
        // caller believes it asked for ACEScg is a colour bug wearing a successful reply.
        for typo in ["acesgc", "ap2", "", "linear rec 709", "aces2"] {
            assert_eq!(WorkingSpace::parse(typo), None, "{typo} must be refused");
        }
    }

    // ── the policy ────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn the_precedence_is_person_then_file_then_role() {
        let role = Some(TextureRole::BaseColour);
        // Nothing known but the role.
        let d = decide(ColourQuery {
            role,
            ..ColourQuery::default()
        });
        assert_eq!(d.space, ColourSpace::Srgb);
        assert_eq!(d.origin, ColourOrigin::InferredFromRole);
        assert!(d.reason.contains("base colour"), "{}", d.reason);

        // The file declares something else — it wins over the role policy.
        let d = decide(ColourQuery {
            role,
            declared: Some(ColourSpace::LinearRec709),
            chosen: None,
        });
        assert_eq!(d.space, ColourSpace::LinearRec709);
        assert_eq!(d.origin, ColourOrigin::Declared);

        // A person overrides both.
        let d = decide(ColourQuery {
            role,
            declared: Some(ColourSpace::LinearRec709),
            chosen: Some(ColourSpace::AcesCg),
        });
        assert_eq!(d.space, ColourSpace::AcesCg);
        assert_eq!(d.origin, ColourOrigin::Manual);
        assert!(d.refused.is_none());
    }

    #[test]
    fn a_colour_space_can_never_be_forced_onto_a_data_channel() {
        // THE structural rule. Roughness is not "roughness in sRGB"; the way to make a texture carry
        // colour is to change what it is USED FOR. An override that silently applied here would put a
        // transfer function on measurements — the exact failure this module exists to prevent — and one
        // that was silently IGNORED would send someone re-exporting a texture that was never at fault.
        for role in [
            TextureRole::Normal,
            TextureRole::MetallicRoughness,
            TextureRole::Occlusion,
            TextureRole::Mask,
        ] {
            let d = decide(ColourQuery {
                role: Some(role),
                declared: Some(ColourSpace::Srgb), // even a file that says so
                chosen: Some(ColourSpace::Srgb),   // even a person who insists
            });
            assert_eq!(d.space, ColourSpace::Data, "{role:?} must stay data");
            let refused = d
                .refused
                .expect("the refusal must be REPORTED, not swallowed");
            assert!(refused.contains("used for"), "{refused}");
            assert!(!role.accepts_colour_override(), "{role:?}");
        }
        // And the colour-bearing roles DO accept one.
        for role in [
            TextureRole::BaseColour,
            TextureRole::Emissive,
            TextureRole::Environment,
        ] {
            assert!(role.accepts_colour_override(), "{role:?}");
        }
    }

    #[test]
    fn every_decision_can_explain_itself_in_words_a_person_can_act_on() {
        for role in TextureRole::all() {
            let d = decide(ColourQuery {
                role: Some(*role),
                ..ColourQuery::default()
            });
            assert!(
                d.reason.len() > 20 && d.reason.contains("because"),
                "{role:?} gives no usable reason: {}",
                d.reason
            );
            assert_eq!(d.tag().space, d.space);
        }
        // With no role at all, the fallback is conservative AND says it is a guess.
        let d = decide(ColourQuery::default());
        assert_eq!(d.space, ColourSpace::Srgb);
        assert!(d.reason.contains("assumed"), "{}", d.reason);
    }

    // ── presentation state ────────────────────────────────────────────────────────────────────────

    #[test]
    fn the_presentation_round_trips_through_json_and_defaults_to_the_current_look() {
        let p = PresentationState {
            working: WorkingSpace::AcesCg,
            view: ViewTransform::PbrNeutral,
            exposure: 1.25,
        };
        let json = serde_json::to_string(&p).expect("serialises");
        let back: PresentationState = serde_json::from_str(&json).expect("parses");
        assert_eq!(back, p);
        // The default changes nothing: adopting this record cannot alter how an existing project looks.
        let d = PresentationState::default();
        assert_eq!(d.working, WorkingSpace::LinearRec709);
        assert_eq!(d.view, ViewTransform::AcesFit);
    }

    #[test]
    fn the_presentation_hash_notices_every_dimension_that_changes_the_image() {
        let base = PresentationState::default();
        let mut seen = std::collections::HashSet::new();
        seen.insert(base.hash());
        for other in [
            PresentationState {
                working: WorkingSpace::AcesCg,
                ..base
            },
            PresentationState {
                view: ViewTransform::PbrNeutral,
                ..base
            },
            PresentationState {
                exposure: base.exposure + 0.01,
                ..base
            },
        ] {
            assert!(
                seen.insert(other.hash()),
                "{other:?} hashes the same as something else — a render result carrying this hash \
                 could not tell the two states apart"
            );
        }
        // Stable across calls, or every asynchronous render would miss forever.
        assert_eq!(base.hash(), PresentationState::default().hash());
    }

    #[test]
    fn a_declared_source_space_composes_into_one_matrix_that_agrees_with_doing_it_in_two_steps() {
        // The environment override. Composing is not an optimisation here — it is what makes the sky,
        // the ambient and every reflection read ONE matrix, so a declaration cannot take effect in the
        // reflection and not in the sky behind it. It has to equal the two-step answer exactly enough
        // that no one can tell which was used.
        for source in ColourSpace::linear_options() {
            for working in WorkingSpace::all() {
                let composed =
                    source_to_working(*source, *working).expect("a linear source composes");
                for probe in [[0.18, 0.2, 0.22], [4.0, 1.0, 0.25], [0.0, 0.0, 0.0]] {
                    let one_step = apply(composed, probe);
                    let two_step = apply(
                        working.from_rec709(),
                        apply(source.linear_to_rec709().unwrap(), probe),
                    );
                    for i in 0..3 {
                        assert!(
                            (one_step[i] - two_step[i]).abs() < 1e-5,
                            "{} → {} channel {i}: {} vs {}",
                            source.label(),
                            working.label(),
                            one_step[i],
                            two_step[i]
                        );
                    }
                }
            }
        }
        // A non-linear source has no matrix, and saying so is what stops an environment being decoded
        // twice. sRGB has a transfer function; data is not colour at all.
        for refused in [ColourSpace::Srgb, ColourSpace::Data] {
            assert!(refused.linear_to_rec709().is_none(), "{}", refused.label());
            assert!(source_to_working(refused, WorkingSpace::AcesCg).is_none());
        }
        // And the default declaration is the identity — an untouched project renders identically.
        assert_eq!(
            source_to_working(ColourSpace::LinearRec709, WorkingSpace::LinearRec709),
            Some(IDENTITY3)
        );
    }

    #[test]
    fn a_malformed_presentation_record_is_refused_rather_than_half_applied() {
        // The sidecar is a file on disk that a person can edit, an older build can have written, and a
        // sync tool can truncate. None of those may produce a half-applied colour state: serde either
        // parses the whole record or none of it, and the caller falls back to the documented defaults
        // with a diagnostic (see `restore_presentation` in the app).
        for junk in [
            "",
            "{}",
            "null",
            "[1,2,3]",
            r#"{"working":"acesCg"}"#, // truncated
            r#"{"working":"ap7","view":"acesFit","exposure":1.0}"#, // a space that does not exist
            r#"{"working":"acesCg","view":"aces2","exposure":1.0}"#, // a view that does not exist
            r#"{"working":"acesCg","view":"acesFit","exposure":"a"}"#, // wrong type
        ] {
            assert!(
                serde_json::from_str::<PresentationState>(junk).is_err(),
                "{junk} was accepted; a partially-understood colour state is worse than none"
            );
        }
        // A record from a FUTURE build — every field this one knows, plus one it does not — is
        // accepted, and that asymmetry is deliberate. Refusing it would drop a working, complete
        // presentation because a later version learned a new word; ignoring the unknown field restores
        // everything this build can honour. The distinction that matters is between "incomplete" (above:
        // refused, because a half-applied colour state is worse than none) and "complete plus extra"
        // (here: honoured). A `look` this build cannot apply is simply not applied — and since the
        // status command reports the live state rather than the file, nothing claims otherwise.
        let forward: PresentationState = serde_json::from_str(
            r#"{"working":"acesCg","view":"acesFit","exposure":1.0,"look":"showLUT"}"#,
        )
        .expect("a complete record with an unknown extra must still restore");
        assert_eq!(forward.working, WorkingSpace::AcesCg);
        assert_eq!(forward.view, ViewTransform::AcesFit);
    }

    #[test]
    fn no_transfer_function_or_matrix_produces_a_non_finite_value_on_any_input() {
        // A property sweep over the ranges a scene-linear pipeline actually sees, including the ones a
        // [0,1] test never reaches. NaN or an infinity anywhere here becomes a black or white pixel
        // that survives every functional test — and `powf` on a negative base is exactly how one
        // appears, which is why the transfer functions are checked on negatives too.
        let mut probes: Vec<f32> = vec![
            // The two piecewise thresholds are in here on purpose: a transfer function's discontinuity
            // is where a NaN hides. `65504.0` is the largest finite f16, i.e. the top of what the HDR
            // scene target can hold.
            -1e4,
            -1.0,
            -0.05,
            -1e-8,
            0.0,
            1e-8,
            0.003_130_8,
            0.040_45,
            0.18,
            0.5,
            1.0,
            4.0,
            65504.0,
        ];
        // A deterministic sweep rather than a random one: same inputs every run, no seed to record.
        for i in 0u8..64 {
            probes.push(f32::from(i) / 63.0);
        }
        for v in probes {
            assert!(srgb_to_linear(v).is_finite(), "srgb_to_linear({v})");
            assert!(linear_to_srgb(v).is_finite(), "linear_to_srgb({v})");
            for from in ColourSpace::all() {
                for to in ColourSpace::all() {
                    let out = convert([v, v * 0.5, -v], *from, *to);
                    assert!(
                        out.iter().all(|c| c.is_finite()),
                        "convert({v}) {} → {}: {out:?}",
                        from.label(),
                        to.label()
                    );
                }
            }
        }
    }

    #[test]
    fn the_wgsl_column_layout_is_the_transpose_and_is_padded() {
        let cols = wgsl_mat3(REC709_TO_AP1);
        for r in 0..3 {
            for c in 0..3 {
                assert!((cols[c][r] - REC709_TO_AP1[r][c]).abs() < 1e-9);
            }
        }
        assert!(cols.iter().all(|c| c[3] == 0.0));
    }

    #[test]
    fn the_backends_report_what_this_build_can_actually_run() {
        assert!(TransformBackend::BuiltIn.available());
        // Reported from the same place the status text comes from, so a menu cannot claim one thing
        // while the note beneath it says another.
        assert_eq!(TransformBackend::Ocio.available(), ocio_status().available);
    }

    #[test]
    fn aces2_is_named_as_absent_with_the_reason_rather_than_approximated_under_its_name() {
        let s = aces2_status();
        assert!(!s.available);
        // It must name what the real thing IS, or the reader cannot tell an honest gap from ignorance.
        for expected in ["JMh", "gamut compression", "CTL"] {
            assert!(s.detail.contains(expected), "{}", s.detail);
        }
        // And the shipped curve must not be called ACES 2 anywhere.
        assert!(!ViewTransform::AcesFit.label().contains('2'));
        assert!(ViewTransform::AcesFit.blurb().contains("Not a reference"));
    }

    #[test]
    fn the_ocio_gap_is_reported_precisely_rather_than_implied() {
        let s = ocio_status();
        assert!(!s.available);
        // It must say what IS possible, not only what is not — otherwise a reader concludes there is
        // no colour management at all, which is the opposite of true.
        assert!(s.detail.contains("cmake"), "{}", s.detail);
        assert!(s.detail.contains("reference-tested"), "{}", s.detail);
        assert!(s.detail.contains("ACEScg"), "{}", s.detail);
    }
}
