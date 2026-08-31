//! **What this engine can actually read and write** — one declared registry, not folklore.
//!
//! Before this existed the answer to "does Metrocalk support X?" was spread across four places that
//! disagreed with each other: a hard-coded extension list in the native file dialog, the magic-byte
//! sniffer in `metrocalk_assets::import::detect`, a set of Cargo features that silently change what is
//! compiled in, and a handful of Tauri commands. The observable results were exactly what you would
//! predict from that:
//!
//! * **STEP AP242 export was fully implemented, tested, and unreachable** — `pmi_step::export_step`
//!   round-trips semantic PMI, and no command called it.
//! * **KTX2 was importable and absent from the file dialog**, so the only way in was automation.
//! * **Audio imported and was then silently discarded** by a `matches!(.., Mesh)` in the landing path.
//! * **HDR environment import existed with no command at all.**
//!
//! None of those are hard problems. They are the predictable cost of having no single place that says
//! what the engine does. This module is that place: every format is one [`FormatSpec`], the dialog
//! filter is derived from it, the UI lists it, and a format that is compiled out says so IN THE SAME
//! LIST rather than vanishing — because "we don't support that" and "that button silently does nothing"
//! are very different messages to give someone with a file in their hand.
//!
//! ## Fidelity is declared, never implied
//!
//! Interchange is lossy and pretending otherwise is how pipelines break. Every entry carries a
//! [`Fidelity`] and, where it matters, the specific thing it will not carry. A user deciding whether to
//! route a part through this engine needs that up front, not in a support thread.

use serde::Serialize;

/// Which way a format flows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Direction {
    /// The engine can read it.
    Import,
    /// The engine can write it.
    Export,
    /// Both, though not necessarily at the same fidelity — see [`FormatSpec::note`].
    Both,
}

impl Direction {
    /// True when this format can be read.
    #[must_use]
    pub fn reads(self) -> bool {
        matches!(self, Self::Import | Self::Both)
    }
    /// True when this format can be written.
    #[must_use]
    pub fn writes(self) -> bool {
        matches!(self, Self::Export | Self::Both)
    }
}

/// How much of the source survives. Ordered worst→best so a UI can sort or colour by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Fidelity {
    /// Recognised and explained, but not decoded here — a licensed kernel or a native/server seam.
    /// The file is never silently dropped; the user is told what is missing and why.
    Seam,
    /// A stated, tested subset of the format. Everything outside it is reported, never guessed.
    Subset,
    /// Everything the engine's own scene model can represent survives.
    Full,
}

impl Fidelity {
    /// The author-facing word.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Seam => "explained seam",
            Self::Subset => "declared subset",
            Self::Full => "full",
        }
    }
}

/// What a format carries. Used to answer "will my rig survive?" without reading source code.
#[allow(
    clippy::struct_excessive_bools,
    reason = "a capability CHECKLIST - each flag is an independent yes/no a user asks about, and               collapsing them into a bitflag would make the JSON the UI reads unreadable"
)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Carries {
    /// Triangle geometry.
    pub geometry: bool,
    /// A named parent/child tree, not a flat pile.
    pub hierarchy: bool,
    /// Surface appearance (at least base colour; see the note for how much more).
    pub materials: bool,
    /// Image maps.
    pub textures: bool,
    /// Joints, weights, bind poses.
    pub skinning: bool,
    /// Time-varying channels.
    pub animation: bool,
    /// Cameras and/or lights.
    pub cameras: bool,
    /// Engineering metadata — PMI/GD&T, part numbers, tolerances.
    pub metadata: bool,
    /// Physics bodies, colliders, joints.
    pub physics: bool,
}

/// One format the engine has an opinion about.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatSpec {
    /// Stable key.
    pub id: &'static str,
    /// What a person calls it.
    pub label: &'static str,
    /// File extensions, lowercase, no dot. First is canonical.
    pub extensions: &'static [&'static str],
    /// Which domain expects it — used to group the list.
    pub domain: &'static str,
    /// Which way it flows.
    pub direction: Direction,
    /// How much survives.
    pub fidelity: Fidelity,
    /// What it carries.
    pub carries: Carries,
    /// The honest sentence: what works, and specifically what does not.
    pub note: &'static str,
    /// False when this build was compiled without the feature that implements it. Still listed, so a
    /// user with such a file is told "this build cannot" rather than "unrecognised".
    pub available: bool,
}

const NONE: Carries = Carries {
    geometry: false,
    hierarchy: false,
    materials: false,
    textures: false,
    skinning: false,
    animation: false,
    cameras: false,
    metadata: false,
    physics: false,
};

/// Compiled in iff the `fbx` feature reached `metrocalk-assets`.
const fn fbx_available() -> bool {
    cfg!(feature = "assets-fbx")
}
/// Compiled in iff the `ktx2` feature reached `metrocalk-assets`.
const fn ktx2_available() -> bool {
    cfg!(feature = "assets-ktx2")
}
/// Compiled in iff the `3dxml` feature reached `metrocalk-interchange`.
const fn threedxml_available() -> bool {
    cfg!(feature = "interchange-3dxml")
}

/// Every format the engine has an opinion about, in one list.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "a declarative table; splitting it hides the very thing it exists to make visible"
)]
pub fn format_catalog() -> Vec<FormatSpec> {
    vec![
        // ── Real-time / DCC ──────────────────────────────────────────────────────────────────
        FormatSpec {
            id: "gltf",
            label: "glTF 2.0 / GLB",
            extensions: &["glb", "gltf"],
            domain: "Real-time",
            direction: Direction::Both,
            fidelity: Fidelity::Full,
            carries: Carries {
                geometry: true,
                hierarchy: true,
                materials: true,
                textures: true,
                skinning: true,
                animation: true,
                ..NONE
            },
            note: "The engine's best-supported path, both directions. Metallic-roughness PBR with \
                   embedded textures on export.",
            available: true,
        },
        FormatSpec {
            id: "obj",
            label: "Wavefront OBJ",
            extensions: &["obj"],
            domain: "Real-time",
            direction: Direction::Import,
            fidelity: Fidelity::Subset,
            carries: Carries {
                geometry: true,
                ..NONE
            },
            note: "Geometry only. OBJ has no hierarchy, no animation and no PBR; a companion .mtl is \
                   not read.",
            available: true,
        },
        FormatSpec {
            id: "fbx",
            label: "Autodesk FBX",
            extensions: &["fbx"],
            domain: "Real-time",
            direction: Direction::Import,
            fidelity: Fidelity::Seam,
            carries: Carries {
                geometry: true,
                hierarchy: true,
                materials: true,
                skinning: true,
                animation: true,
                ..NONE
            },
            note: if fbx_available() {
                "Read through the native ufbx reader."
            } else {
                "This build was compiled without the FBX reader. Convert to glTF, or use a build with \
                 the `fbx` feature."
            },
            available: fbx_available(),
        },
        // ── Images and textures ──────────────────────────────────────────────────────────────
        FormatSpec {
            id: "image",
            label: "PNG / JPEG",
            extensions: &["png", "jpg", "jpeg"],
            domain: "Textures",
            direction: Direction::Import,
            fidelity: Fidelity::Full,
            carries: Carries {
                geometry: true,
                textures: true,
                ..NONE
            },
            note: "Placed as a textured quad. Treated as sRGB colour data.",
            available: true,
        },
        FormatSpec {
            id: "ktx2",
            label: "KTX2 / Basis Universal",
            extensions: &["ktx2"],
            domain: "Textures",
            direction: Direction::Import,
            fidelity: Fidelity::Seam,
            carries: Carries {
                textures: true,
                ..NONE
            },
            note: if ktx2_available() {
                "Supercompressed GPU texture, transcoded on import."
            } else {
                "This build was compiled without the Basis transcoder. Supply PNG/JPEG instead."
            },
            available: ktx2_available(),
        },
        FormatSpec {
            id: "hdr",
            label: "Radiance HDR (equirectangular)",
            extensions: &["hdr"],
            domain: "Textures",
            direction: Direction::Import,
            fidelity: Fidelity::Subset,
            carries: NONE,
            note: "An environment map for image-based lighting. Equirectangular layout only.",
            available: true,
        },
        // ── CAD / engineering ────────────────────────────────────────────────────────────────
        FormatSpec {
            id: "step",
            label: "STEP AP242",
            extensions: &["stp", "step"],
            domain: "CAD",
            direction: Direction::Both,
            fidelity: Fidelity::Subset,
            carries: Carries {
                geometry: true,
                hierarchy: true,
                materials: true,
                metadata: true,
                ..NONE
            },
            note: "Pure-Rust reader and writer. Planar faces plus analytic cylinders, cones, spheres \
                   and tori tessellate exactly; trimmed NURBS and freeform faces are reported per face \
                   and left to a licensed kernel. Semantic PMI (GD&T) round-trips machine-readable, \
                   never downgraded to a graphical annotation.",
            available: true,
        },
        FormatSpec {
            id: "3dxml",
            label: "CATIA 3DXML",
            extensions: &["3dxml"],
            domain: "CAD",
            direction: Direction::Import,
            fidelity: Fidelity::Subset,
            carries: Carries {
                geometry: true,
                hierarchy: true,
                metadata: true,
                ..NONE
            },
            note: if threedxml_available() {
                "Product structure, assembly tree and instance transforms are read in full. CATIA's \
                 proprietary .3DRep tessellation is not decodable here — if a sibling STEP file is \
                 present it is used automatically for the geometry, otherwise each affected part is \
                 reported as a placed proxy."
            } else {
                "This build was compiled without the 3DXML container reader."
            },
            available: threedxml_available(),
        },
        // ── Simulation / robotics ────────────────────────────────────────────────────────────
        FormatSpec {
            id: "urdf",
            label: "URDF",
            extensions: &["urdf", "xml"],
            domain: "Simulation",
            direction: Direction::Import,
            fidelity: Fidelity::Subset,
            carries: Carries {
                hierarchy: true,
                physics: true,
                ..NONE
            },
            note: "Links, joints and collision shapes with forward kinematics resolved to world poses. \
                   Visual geometry, inertia tensors and actuation are not read. Prismatic, floating and \
                   planar joints are declined with a reason rather than approximated.",
            available: true,
        },
        FormatSpec {
            id: "usd",
            label: "OpenUSD (USD-Physics)",
            extensions: &["usda", "usd"],
            domain: "Simulation",
            direction: Direction::Both,
            fidelity: Fidelity::Subset,
            carries: Carries {
                geometry: true,
                hierarchy: true,
                physics: true,
                ..NONE
            },
            note: "Export writes a USDA scene. Import reads ASCII .usda only, and only the USD-Physics \
                   subset: units, rigid bodies and primitive colliders. Binary .usdc, zipped .usdz and \
                   USD's composition system (references, payloads, variants, layers) are a declared \
                   seam — and composition, not geometry, is what USD is actually for, so this is an \
                   interop path and not a claim to speak USD.",
            available: true,
        },
    ]
}

/// Every extension that can be OPENED by this build, deduplicated and sorted — the native file
/// dialog's filter, derived rather than hand-maintained so it cannot drift from what works.
#[must_use]
pub fn import_extensions() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = format_catalog()
        .iter()
        .filter(|f| f.available && f.direction.reads())
        .flat_map(|f| f.extensions.iter().copied())
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Every `format` argument the `scene_export` command will act on.
///
/// It lives here rather than inline in the command because there are two statements of this set and
/// they must agree: the command's guard, and the export UI, which derives its argument from a
/// format's canonical extension so it never carries a second hand-written table. A writer landing
/// with a canonical extension missing from this list would appear in the export dialog and refuse.
pub const EXPORT_ARGS: &[&str] = &["glb", "usda", "usd", "step", "stp"];

/// True when [`EXPORT_ARGS`] contains `arg` (case-insensitive, leading dot tolerated).
#[must_use]
pub fn export_arg_accepted(arg: &str) -> bool {
    let lower = arg.trim_start_matches('.').to_ascii_lowercase();
    EXPORT_ARGS.contains(&lower.as_str())
}

/// The spec matching a file extension (case-insensitive), if any.
#[must_use]
pub fn spec_for_extension(ext: &str) -> Option<FormatSpec> {
    let lower = ext.trim_start_matches('.').to_ascii_lowercase();
    format_catalog()
        .into_iter()
        .find(|f| f.extensions.iter().any(|e| *e == lower))
}

/// Explain a file this build cannot open, in the author's language — including the case that matters
/// most, where the engine DOES know the format but this build was compiled without it.
#[must_use]
pub fn explain_unsupported(file_name: &str) -> String {
    let ext = file_name.rsplit('.').next().unwrap_or_default();
    match spec_for_extension(ext) {
        Some(spec) if !spec.available => {
            format!("{} is a format Metrocalk knows — {}", spec.label, spec.note)
        }
        Some(spec) if !spec.direction.reads() => {
            format!("{} is an export-only format here.", spec.label)
        }
        Some(spec) => format!(
            "{} should have opened — the file may be damaged or empty.",
            spec.label
        ),
        None => {
            let known = import_extensions().join(", ");
            format!("Metrocalk can't open a \".{ext}\" file. It can open: {known}.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dialog_filter_is_derived_from_the_catalogue_not_hand_written() {
        // The bug this prevents: KTX2 was importable and missing from the hand-written filter, so the
        // only way to open one was automation. A derived list cannot drift from what works.
        let exts = import_extensions();
        for spec in format_catalog() {
            if spec.available && spec.direction.reads() {
                for e in spec.extensions {
                    assert!(
                        exts.contains(e),
                        "{} can be read but {e} is not offered",
                        spec.id
                    );
                }
            }
        }
    }

    #[test]
    fn an_unavailable_format_is_still_listed_and_explains_itself() {
        // "Unrecognised file" is the wrong answer for a format the engine knows but this build lacks.
        for spec in format_catalog().iter().filter(|f| !f.available) {
            assert!(
                spec.note.contains("build") || spec.note.contains("Convert"),
                "{} is unavailable but does not say what to do about it: {}",
                spec.id,
                spec.note
            );
        }
    }

    #[test]
    fn every_entry_states_what_it_costs() {
        for spec in format_catalog() {
            assert!(!spec.note.is_empty(), "{} has no fidelity note", spec.id);
            assert!(
                spec.note.len() > 30,
                "{}'s note is too thin to be honest: {}",
                spec.id,
                spec.note
            );
            assert!(!spec.extensions.is_empty(), "{} has no extensions", spec.id);
            assert!(!spec.domain.is_empty(), "{} has no domain", spec.id);
        }
    }

    #[test]
    fn a_declared_subset_or_seam_never_claims_full_fidelity() {
        // The whole point of the Fidelity axis is that it cannot be quietly inflated.
        let step = spec_for_extension("stp").expect("STEP is catalogued");
        assert_eq!(step.fidelity, Fidelity::Subset);
        assert!(step.note.contains("NURBS"), "STEP must name its exclusion");
        let usd = spec_for_extension("usda").expect("USD is catalogued");
        assert_eq!(usd.fidelity, Fidelity::Subset);
        assert!(
            usd.note.contains("composition"),
            "USD must name its exclusion"
        );
    }

    #[test]
    fn extensions_are_lowercase_dotless_and_unique_across_the_catalogue() {
        let mut seen: Vec<&str> = Vec::new();
        for spec in format_catalog() {
            for e in spec.extensions {
                assert!(!e.starts_with('.'), "{e} should not carry its dot");
                assert_eq!(*e, e.to_ascii_lowercase(), "{e} should be lowercase");
                assert!(
                    !seen.contains(e),
                    "{e} is claimed by two formats — the dialog and the sniffer would disagree"
                );
                seen.push(e);
            }
        }
    }

    #[test]
    fn an_unknown_file_is_told_what_the_engine_can_open() {
        let msg = explain_unsupported("turbine.sldprt");
        assert!(msg.contains("sldprt"), "{msg}");
        assert!(msg.contains("step") || msg.contains("stp"), "{msg}");
    }

    #[test]
    fn every_writable_format_is_addressable_by_its_canonical_extension() {
        // The pairing this exists to make: the catalogue declares which formats can be WRITTEN, and
        // `scene_export`'s guard declares which `format` arguments it will act on. Those were two
        // statements in two files that nothing compared, and the export dialog now bridges them by
        // deriving its argument from `extensions[0]` rather than restating the mapping. A writer
        // added with a canonical extension this guard does not accept would therefore be offered in
        // the dialog and refuse at the click — which is the failure this turns into a red test.
        for spec in format_catalog() {
            if !spec.direction.writes() {
                continue;
            }
            let canonical = spec.extensions[0];
            assert!(
                export_arg_accepted(canonical),
                "{} can be written but scene_export rejects its canonical extension '{canonical}'",
                spec.id
            );
        }
    }

    #[test]
    fn the_export_argument_set_refuses_a_spelling_outside_it() {
        // The negative control. Without it the assertion above would still pass if
        // `export_arg_accepted` returned true for everything, and a gate that accepts everything
        // reports the same green as one that compared the right things.
        assert!(export_arg_accepted("STEP"), "case must not matter");
        assert!(export_arg_accepted(".usda"), "a leading dot is tolerated");
        assert!(
            !export_arg_accepted("gltf"),
            "'gltf' is a real extension of a writable format and is still NOT an accepted argument"
        );
        assert!(!export_arg_accepted("fbx"));
        assert!(!export_arg_accepted(""));
    }

    #[test]
    fn step_is_declared_in_both_directions() {
        // The gap this exists to close: a complete, tested STEP writer with no way to reach it.
        let step = spec_for_extension("step").expect("catalogued");
        assert!(step.direction.writes(), "STEP export must be declared");
        assert!(step.direction.reads());
    }
}
