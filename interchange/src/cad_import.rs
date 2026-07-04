//! The **universal CAD import pipeline** (M15.7 / ADR-077) — "import CAD like importing a texture."
//!
//! The motivating failure (the bar to beat, exactly): a real CATIA 3DEXPERIENCE **3DXML** (221 MB, 1,280
//! parts) imported into Unreal/Datasmith brought in **1** part; the ~1,280 native parts were **0-triangle
//! shells** (silent). The recommended rescue — re-export as **STEP AP242** — imported **nothing** (a black
//! screen). The kernel was never the problem (Datasmith runs on HOOPS Exchange, the best CAD SDK on the
//! market): the failure is a **fragile, single-strategy, all-or-nothing, SILENT pipeline** around a great
//! kernel. This module is the opposite: a pipeline that **CANNOT produce a black screen** and **NEVER
//! silently drops a part**.
//!
//! **The four guarantees (all structural, headless-assertable — not UI copy):**
//! 1. **Never-empty.** Every part gets a placed, renderable mesh — exact B-rep, the embedded tessellation
//!    cache, or a **bounding proxy at its real assembly transform**. A part that would 0-triangle-fail in
//!    Datasmith still appears. ([`CadImport::never_empty`].)
//! 2. **Never-silent.** Every part is classified in a structured, queryable per-part report — exact-B-rep ·
//!    tessellation-only · proxy · access-denied · failed — each with a **reason** + a **fix path**.
//!    ([`CadImport::never_silent`], [`PartReport`].)
//! 3. **Multi-strategy per-part cascade** (no single point of failure): exact B-rep → embedded tessellation
//!    → AI reconstruction (a confidence-scored, opt-in seam) → bounding proxy. The winning strategy is
//!    recorded per part. ([`resolve_part`].)
//! 4. **Substrate-native.** Geometry-hash **dedup → instancing** (CAD is bolt-heavy: 1,280 instances of 572
//!    unique parts), a **content-addressed re-import diff** ("which of 1,280 parts changed", [`diff`]), and
//!    **provenance** (source hash · format · per-part strategy · fidelity). The import lands as resumable,
//!    revertible ops on the op-stream (the editor-shell wiring).
//!
//! Honest scope (the ADR-070/077 boundary — stated, never papered over). Native "no exception" (decoding a
//! proprietary CATIA `V5_CFV3`/`CB0001` rep, an NX/Creo/SolidWorks part) needs a licensed exchange kernel
//! (Spatial 3D InterOp for the CATIA case, or HOOPS Exchange) behind the [`CadReader`] trait — a
//! native/server-only seam, out of the determinism guarantee (like OCCT, ADR-070). What this pipeline
//! delivers WITHOUT the kernel, that Datasmith did not: never-empty, never-silent, a per-part fix path, and
//! substrate-native ops. The neutral tier (STEP AP242 planar B-rep) is fully ours (the pure-Rust Part-21
//! reader, [`crate::StepInterchange`]); mesh (glTF/STL/OBJ) is direct. AVOID hand-writing a CATIA/NX reader
//! (research §6). The AI mesh→B-rep tier is a labeled, opt-in candidate, never a silent auto-replace.

use crate::{Units, UnsupportedNote};
use metrocalk_csg::{box_mesh, TriMesh};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The maximum assembly-tree depth the traversal descends before returning a bounded error — a crafted
/// deeply-nested product structure (an occurrence-graph bomb) is an explained error, never a stack overflow.
pub const MAX_ASSEMBLY_DEPTH: u32 = 512;

// ============================================================================================
// The neutral pipeline types (our types only — no foreign STEP/zip/xml leak, invariant 5)
// ============================================================================================

/// The strategy that produced a part's geometry — the multi-strategy cascade's *winner*, recorded per part.
/// The order is the cascade order (best fidelity first): exact → cache → AI → proxy.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ImportStrategy {
    /// Exact B-rep tessellated (the kernel-free planar subset, or the licensed-kernel seam).
    ExactBrep,
    /// The file's embedded tessellation/visualization cache (open XML 3DRep · JT LOD · STEP tessellation ·
    /// glTF mesh) — rendered instantly (the "like a texture" core), exact B-rep not resolved.
    TessellationCache,
    /// An AI mesh→B-rep reconstruction candidate (confidence-scored, opt-in — CAD-Recode-class). In this
    /// build the reconstruction service is a **seam**; a part that *could* be reconstructed is recorded and
    /// falls to the proxy with the opt-in fix path (never a silent auto-replace).
    AiReconstruction,
    /// A bounding proxy at the part's real assembly transform — the never-empty floor when no geometry is
    /// decodable (a proprietary rep needing the kernel · encrypted · missing · genuinely failed).
    BoundingProxy,
}

impl ImportStrategy {
    /// A stable token for the report / ECS query (never drifting UI copy).
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            Self::ExactBrep => "exact-brep",
            Self::TessellationCache => "tessellation-cache",
            Self::AiReconstruction => "ai-reconstruction",
            Self::BoundingProxy => "bounding-proxy",
        }
    }
}

/// The fidelity a part reached — the honesty class, queryable ("show tessellation-only parts").
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub enum PartFidelity {
    /// Exact B-rep tessellated (precision retained; the B-rep is kept as ECS entities by the wiring).
    ExactBrep,
    /// The embedded tessellation cache only — visualization-grade, exact B-rep unresolved.
    TessellationOnly,
    /// An AI-reconstructed candidate at the given confidence (0..=1) — labeled, opt-in.
    Reconstructed(f32),
    /// A bounding proxy — the shape is unknown (a proprietary rep the kernel would decode), but the part's
    /// *position* is known and it is placed + reported.
    Proxy,
    /// The geometry export was blocked at the source (encrypted / DRM'd) — reported, never a silent empty.
    AccessDenied,
    /// A genuine failure (a missing referenced file, a corrupt rep) — surfaced with a diagnosis + fix.
    Failed,
}

impl PartFidelity {
    /// A stable token for the report / ECS query.
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            Self::ExactBrep => "exact-brep",
            Self::TessellationOnly => "tessellation-only",
            Self::Reconstructed(_) => "ai-reconstructed",
            Self::Proxy => "proxy",
            Self::AccessDenied => "access-denied",
            Self::Failed => "failed",
        }
    }
    /// `true` when the part carries real (exact or cached) geometry — not a proxy/denied/failed placeholder.
    #[must_use]
    pub fn is_real_geometry(self) -> bool {
        matches!(
            self,
            Self::ExactBrep | Self::TessellationOnly | Self::Reconstructed(_)
        )
    }
}

/// One part in the import report — **every** part is accounted for here ("explain every no" applied to
/// import). The Datasmith anti-pattern inverted: nothing silent, ever.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct PartReport {
    /// A stable per-occurrence id (the source's instance id) — the report/diff key.
    pub id: u64,
    /// The human name (CATIA `V_Name` / STEP product name / the reference name).
    pub name: String,
    /// The **unique geometry reference** key (the CATIA `Reference3D` id / the STEP solid id) — the dedup key:
    /// many occurrences of one reference share one tessellated mesh (GPU-instanced).
    pub reference: String,
    /// The winning strategy (the cascade result).
    pub strategy: ImportStrategy,
    /// The fidelity reached.
    pub fidelity: PartFidelity,
    /// Why this fidelity — plain language (no engine jargon), surfaced to the user.
    pub reason: String,
    /// A one-click fix path when the fidelity is below exact — re-export as STEP AP242 · enable the licensed
    /// kernel · accept the AI candidate · unlock the DRM. `None` when nothing better is available.
    pub fix: Option<String>,
    /// The part's **world transform** (column-major 4×4), composed down the assembly tree — the real pivot /
    /// placement (never the assembly-origin collapse Datasmith produces).
    pub transform: [f64; 16],
    /// The index into [`CadImport::meshes`] this part renders — **always `Some`** (never-empty is structural:
    /// a proxy is a real mesh). A part is never a black hole.
    pub mesh: Option<usize>,
}

impl PartReport {
    /// `true` when this part's mesh is real geometry (not the shared proxy box).
    #[must_use]
    pub fn is_real_geometry(&self) -> bool {
        self.fidelity.is_real_geometry()
    }
}

/// One **unique** tessellated mesh (deduped by geometry hash) — many [`PartReport`]s can instance it.
#[derive(Clone, PartialEq, Debug)]
pub struct CadMesh {
    /// The welded, deterministic triangle mesh (single-threaded, exact-coordinate weld — hash-stable).
    pub tris: TriMesh,
    /// The stable geometry hash — the dedup key, the re-import-diff content address, and the hashed-mesh
    /// regression-corpus value (same file → same hash across runs/threads/machines).
    pub hash: u64,
    /// `true` if this is the shared bounding-proxy box (not decoded geometry).
    pub is_proxy: bool,
}

/// The result of importing a CAD file — never-empty + never-silent by construction.
#[derive(Clone, PartialEq, Debug)]
pub struct CadImport {
    /// A display name (the assembly / root product name).
    pub name: String,
    /// The source format tag (`"CATIA-3DXML"` / `"STEP-AP242"` / `"glTF"` …).
    pub source_format: String,
    /// The declared units (normalized; the M8.3 scale check consumes this).
    pub units: Units,
    /// **Provenance:** a stable content hash of the source bytes (the O(1) re-import identity + the audit).
    pub source_hash: u64,
    /// The **unique** tessellated meshes (deduped) — `parts[i].mesh` indexes here; repeated parts instance.
    pub meshes: Vec<CadMesh>,
    /// **Every** part, classified — the never-silent report.
    pub parts: Vec<PartReport>,
    /// The raw source instance/relationship count (the CATIA `Instance3D` edge count — the research's "1,280"
    /// figure). This is the assembly-graph size; `part_count()` is the fully-expanded leaf-geometry
    /// **placement** count (nested instancing multiplies it), which is what a viewer draws.
    pub total_occurrences: usize,
    /// The number of top-level **products** in the file (the 3DXML forest roots — a `.3dxml` can carry many
    /// disconnected product trees; the declared root names only one, so a never-drop importer shows them all).
    /// `1` for a single-scene source (STEP/mesh).
    pub products: usize,
    /// Structural assembly nodes with no leaf geometry (sub-assemblies) — counted so the accounting is honest
    /// (they are the tree, not parts; not rendered, not a failure).
    pub structural_nodes: usize,
    /// Every unsupported/approximated feature at the *scene* level, explained (never a silent drop).
    pub notes: Vec<UnsupportedNote>,
}

impl CadImport {
    /// **Never-empty (structural):** every reported part has a placed, renderable mesh — a proxy is a real
    /// mesh, so a part is never a 0-triangle black hole. This is the guarantee Datasmith violates.
    #[must_use]
    pub fn never_empty(&self) -> bool {
        !self.parts.is_empty()
            && self.parts.iter().all(|p| {
                p.mesh.is_some_and(|i| {
                    self.meshes
                        .get(i)
                        .is_some_and(|m| m.tris.triangle_count() > 0)
                })
            })
    }

    /// **Never-silent (structural):** every part carries a non-empty diagnosis (`reason`), and every
    /// below-exact part carries a `fix` path. A part is never left unexplained.
    #[must_use]
    pub fn never_silent(&self) -> bool {
        self.parts.iter().all(|p| {
            !p.reason.trim().is_empty()
                && (p.fidelity == PartFidelity::ExactBrep || p.fix.is_some())
        })
    }

    /// Total part **placement** count — every geometry-bearing occurrence, reported (the fully-expanded
    /// assembly forest). This is what a viewer draws; Datasmith drew 1 of these.
    #[must_use]
    pub fn part_count(&self) -> usize {
        self.parts.len()
    }

    /// The number of **unique geometries** (distinct part references) — the dedup denominator: this many
    /// tessellations serve all `part_count()` placements (GPU-instanced). CAD is bolt-heavy, so this is far
    /// smaller than the placement count.
    #[must_use]
    pub fn unique_geometry_count(&self) -> usize {
        self.parts
            .iter()
            .map(|p| p.reference.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    }

    /// The count of parts at each fidelity — the report headline ("1,280 parts → 596 exact, 684
    /// tessellation-only, 0 failed").
    #[must_use]
    pub fn fidelity_counts(&self) -> FidelityCounts {
        let mut c = FidelityCounts::default();
        for p in &self.parts {
            match p.fidelity {
                PartFidelity::ExactBrep => c.exact_brep += 1,
                PartFidelity::TessellationOnly => c.tessellation_only += 1,
                PartFidelity::Reconstructed(_) => c.reconstructed += 1,
                PartFidelity::Proxy => c.proxy += 1,
                PartFidelity::AccessDenied => c.access_denied += 1,
                PartFidelity::Failed => c.failed += 1,
            }
        }
        c
    }

    /// The **instancing** win: `(unique_meshes, total_instances)` — CAD is bolt-heavy, so a huge assembly
    /// tessellates each *unique* solid once and GPU-instances the repeats (the min-spec story). For the
    /// crane: 1,280 instances of far fewer unique meshes.
    #[must_use]
    pub fn instancing(&self) -> (usize, usize) {
        let instances = self.parts.iter().filter(|p| p.mesh.is_some()).count();
        (self.meshes.len(), instances)
    }

    /// The renderable instance list: `(mesh_index, world_transform)` per part — what the renderer draws
    /// (GPU-instanced by `mesh_index`). Every part contributes (never-empty).
    #[must_use]
    pub fn instances(&self) -> Vec<(usize, [f64; 16])> {
        self.parts
            .iter()
            .filter_map(|p| p.mesh.map(|i| (i, p.transform)))
            .collect()
    }

    /// A human summary line for the report header.
    #[must_use]
    pub fn summary(&self) -> String {
        let c = self.fidelity_counts();
        let (uniq, inst) = self.instancing();
        format!(
            "{}: {} part placements · {} unique geometries · {} product(s) → {} exact-B-rep, {} \
             tessellation-only, {} AI-reconstructed, {} proxy, {} access-denied, {} failed ({} unique meshes \
             for {} instances; {} structural nodes)",
            self.source_format,
            self.part_count(),
            self.unique_geometry_count(),
            self.products,
            c.exact_brep,
            c.tessellation_only,
            c.reconstructed,
            c.proxy,
            c.access_denied,
            c.failed,
            uniq,
            inst,
            self.structural_nodes,
        )
    }

    /// Query: the parts matching a fidelity token (the relational-ECS "show tessellation-only parts").
    #[must_use]
    pub fn parts_with_fidelity(&self, token: &str) -> Vec<&PartReport> {
        self.parts
            .iter()
            .filter(|p| p.fidelity.token() == token)
            .collect()
    }
}

/// The per-fidelity part tally.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct FidelityCounts {
    pub exact_brep: usize,
    pub tessellation_only: usize,
    pub reconstructed: usize,
    pub proxy: usize,
    pub access_denied: usize,
    pub failed: usize,
}

// ============================================================================================
// What a reader emits per part (the cascade input) + the cascade
// ============================================================================================

/// One raw part a [`CadReader`] discovered, before the never-empty/never-silent policy runs. This cleanly
/// separates *what the reader found* from *the pipeline's guarantee* — each is tested independently.
#[derive(Clone, Debug)]
pub struct RawPart {
    /// The stable per-occurrence id.
    pub id: u64,
    /// The human name.
    pub name: String,
    /// The unique geometry reference key (dedup).
    pub reference: String,
    /// The world transform (column-major 4×4).
    pub transform: [f64; 16],
    /// The best geometry the reader could resolve for this part.
    pub source: PartSource,
}

/// The geometry a reader resolved for a part — the cascade routes each variant to a strategy/fidelity.
#[derive(Clone, Debug)]
pub enum PartSource {
    /// Exact planar B-rep faces (from STEP) — tessellated exactly + deterministically here.
    ExactBrep(Vec<crate::step::CadFace>),
    /// An embedded/open tessellation cache already decoded to a mesh (open XML 3DRep · glTF · JT LOD).
    Tessellation(TriMesh),
    /// A proprietary tessellation cache the reader **cannot** decode without the licensed kernel — carries
    /// the detected encoding (e.g. `"CATIA V5_CFV3/CB0001"`). Placed as a proxy at its transform + diagnosed.
    ProprietaryRep {
        /// The detected proprietary encoding (from the rep's magic bytes).
        encoding: String,
    },
    /// The geometry was blocked at the source (encrypted / DRM'd).
    Encrypted,
    /// A referenced geometry file is missing / unresolved — a diagnosed failure, never silent.
    Missing {
        /// What was referenced but not found.
        detail: String,
    },
}

/// Run the multi-strategy cascade on one [`RawPart`], turning it into a [`PartReport`] and (always) assigning
/// it a mesh — real geometry when available, else the shared bounding proxy (never-empty). `meshes`/`by_hash`
/// intern unique geometry (dedup); `proxy_idx` lazily interns the one shared proxy box.
#[allow(clippy::too_many_lines)] // the cascade is one clear match over the strategy variants — cohesive
fn resolve_part(
    raw: RawPart,
    meshes: &mut Vec<CadMesh>,
    by_hash: &mut BTreeMap<u64, Vec<usize>>,
    proxy_idx: &mut Option<usize>,
) -> PartReport {
    // Intern a real mesh by geometry hash (dedup). Returns None for a degenerate (0-triangle) mesh so the
    // cascade falls through to the proxy — a 0-triangle "success" is exactly the Datasmith silent failure.
    // The hash buckets a chain; on a hit we VERIFY structural equality before aliasing — a 64-bit hash
    // collision must NEVER silently merge two distinct geometries (that would be silent geometry loss
    // masquerading as an exact success). Identical geometry shares one mesh; a genuine collision keeps both.
    let intern_real =
        |tris: TriMesh, meshes: &mut Vec<CadMesh>, by_hash: &mut BTreeMap<u64, Vec<usize>>| {
            if tris.triangle_count() == 0 {
                return None;
            }
            let h = mesh_hash(&tris);
            let chain = by_hash.entry(h).or_default();
            for &idx in chain.iter() {
                let m = &meshes[idx];
                if m.tris.positions == tris.positions && m.tris.triangles == tris.triangles {
                    return Some(idx); // exact same geometry → share the mesh
                }
            }
            let i = meshes.len();
            chain.push(i);
            meshes.push(CadMesh {
                tris,
                hash: h,
                is_proxy: false,
            });
            Some(i)
        };

    // The cascade: exact B-rep → embedded tessellation cache → AI (seam) → bounding proxy. Record the winner.
    let (strategy, fidelity, mesh, reason, fix): (
        ImportStrategy,
        PartFidelity,
        Option<usize>,
        String,
        Option<String>,
    ) = match raw.source {
        PartSource::ExactBrep(faces) => {
            let tris = tessellate_faces(&faces);
            match intern_real(tris, meshes, by_hash) {
                Some(i) => (
                    ImportStrategy::ExactBrep,
                    PartFidelity::ExactBrep,
                    Some(i),
                    "exact B-rep tessellated (planar faces, single-threaded + deterministic; the B-rep is \
                     kept as referenceable entities)"
                        .into(),
                    None,
                ),
                // Exact B-rep but no planar faces tessellated here → all curved/NURBS → the OCCT seam; proxy.
                None => (
                    ImportStrategy::BoundingProxy,
                    PartFidelity::Proxy,
                    Some(intern_proxy(meshes, proxy_idx)),
                    "exact B-rep is entirely curved/NURBS faces — planar tessellation is empty; exact \
                     curved tessellation is the OpenCascade native/server seam (placed at its transform)"
                        .into(),
                    Some(
                        "enable the OpenCascade kernel behind the CadReader trait for curved-face \
                         tessellation"
                            .into(),
                    ),
                ),
            }
        }
        PartSource::Tessellation(tris) => match intern_real(tris, meshes, by_hash) {
            Some(i) => (
                ImportStrategy::TessellationCache,
                PartFidelity::TessellationOnly,
                Some(i),
                "embedded tessellation cache rendered instantly (visualization mesh; exact B-rep not \
                 resolved)"
                    .into(),
                Some("re-export as STEP AP242 to resolve exact B-rep + semantic PMI".into()),
            ),
            None => (
                ImportStrategy::BoundingProxy,
                PartFidelity::Failed,
                Some(intern_proxy(meshes, proxy_idx)),
                "the embedded tessellation cache was present but degenerate (0 triangles) — placed as a \
                 proxy, diagnosed, never a silent empty shell"
                    .into(),
                Some("re-export as STEP AP242 / verify the source tessellation".into()),
            ),
        },
        PartSource::ProprietaryRep { encoding } => (
            ImportStrategy::BoundingProxy,
            PartFidelity::Proxy,
            Some(intern_proxy(meshes, proxy_idx)),
            format!(
                "{encoding}: proprietary tessellation the licensed CAD kernel decodes — placed at its real \
                 assembly transform, accounted for, never a silent 0-triangle shell (the Datasmith failure)"
            ),
            Some(
                "re-export the part as STEP AP242, or enable the licensed kernel (Spatial 3D InterOp for \
                 CATIA / HOOPS Exchange) behind the CadReader trait for exact geometry"
                    .into(),
            ),
        ),
        PartSource::Encrypted => (
            ImportStrategy::BoundingProxy,
            PartFidelity::AccessDenied,
            Some(intern_proxy(meshes, proxy_idx)),
            "encrypted / DRM-protected — geometry export was blocked at the source; placed as a labeled \
             proxy, never a silent empty shell"
                .into(),
            Some("unlock or re-export the part without DRM".into()),
        ),
        PartSource::Missing { detail } => (
            ImportStrategy::BoundingProxy,
            PartFidelity::Failed,
            Some(intern_proxy(meshes, proxy_idx)),
            format!("{detail} — the referenced geometry is missing/unresolved; placed as a proxy + diagnosed"),
            Some("re-export with geometry embedded, or repair the missing reference".into()),
        ),
    };

    PartReport {
        id: raw.id,
        name: raw.name,
        reference: raw.reference,
        strategy,
        fidelity,
        reason,
        fix,
        transform: raw.transform,
        mesh,
    }
}

/// Intern the single shared bounding-proxy box (a unit cube). All proxies instance this **one** mesh — the
/// never-empty floor costs one mesh for thousands of proxy parts (GPU-instanced). The box is centered at the
/// origin; each proxy part's `transform` places it at its real assembly position.
fn intern_proxy(meshes: &mut Vec<CadMesh>, proxy_idx: &mut Option<usize>) -> usize {
    if let Some(i) = *proxy_idx {
        return i;
    }
    // A unit cube (half-extent 0.5 in scene units). Sizing for visibility is the wiring/visual pass; the
    // headless never-empty guarantee only needs a non-degenerate placed mesh.
    let tris = box_mesh([0.0, 0.0, 0.0], [0.5, 0.5, 0.5]);
    let h = mesh_hash(&tris);
    let i = meshes.len();
    meshes.push(CadMesh {
        tris,
        hash: h,
        is_proxy: true,
    });
    *proxy_idx = Some(i);
    i
}

/// Assemble a [`CadImport`] from a reader's raw parts — runs the cascade over each part (never-empty +
/// never-silent by construction) and dedups geometry (instancing). This is the pipeline's core policy; a
/// reader is only responsible for producing correct [`RawPart`]s.
#[must_use]
pub fn build_import(
    name: String,
    source_format: String,
    units: Units,
    source_hash: u64,
    raw_parts: Vec<RawPart>,
    structural_nodes: usize,
    notes: Vec<UnsupportedNote>,
) -> CadImport {
    let mut meshes: Vec<CadMesh> = Vec::new();
    let mut by_hash: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    let mut proxy_idx: Option<usize> = None;
    let mut parts = Vec::with_capacity(raw_parts.len());
    for raw in raw_parts {
        parts.push(resolve_part(raw, &mut meshes, &mut by_hash, &mut proxy_idx));
    }
    let total_occurrences = parts.len();
    CadImport {
        name,
        source_format,
        units,
        source_hash,
        meshes,
        total_occurrences,
        products: 1,
        parts,
        structural_nodes,
        notes,
    }
}

// ============================================================================================
// Deterministic tessellation + stable hashing (the hashed-mesh regression corpus)
// ============================================================================================

/// Tessellate a set of planar [`CadFace`](crate::step::CadFace)s into one welded [`TriMesh`] — the same
/// single-threaded, exact-coordinate weld + outward-orientation as [`crate::CadScene::tessellate`], but for
/// **one part** (so a per-part mesh dedups + hashes independently). Curved faces are skipped (the OCCT seam).
#[must_use]
#[allow(clippy::cast_precision_loss)] // polygon vertex counts are tiny
pub fn tessellate_faces(faces: &[crate::step::CadFace]) -> TriMesh {
    use crate::step::FaceKind;
    let mut weld: BTreeMap<[u64; 3], u32> = BTreeMap::new();
    let mut positions: Vec<[f64; 3]> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();

    // Part centroid (for outward orientation of a convex-ish body).
    let mut sc = [0.0f64; 3];
    let mut nc = 0.0f64;
    for face in faces {
        for v in &face.outer {
            for k in 0..3 {
                sc[k] += v[k];
            }
            nc += 1.0;
        }
    }
    if nc > 0.0 {
        for s in &mut sc {
            *s /= nc;
        }
    }

    let mut vid = |p: [f64; 3], positions: &mut Vec<[f64; 3]>| -> u32 {
        let key = [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
        if let Some(&i) = weld.get(&key) {
            return i;
        }
        let i = u32::try_from(positions.len()).unwrap_or(u32::MAX);
        positions.push(p);
        weld.insert(key, i);
        i
    };

    for face in faces {
        if face.kind != FaceKind::Planar || face.outer.len() < 3 {
            continue;
        }
        let mut fc = [0.0f64; 3];
        for v in &face.outer {
            for k in 0..3 {
                fc[k] += v[k];
            }
        }
        let inv = 1.0 / (face.outer.len() as f64);
        for c in &mut fc {
            *c *= inv;
        }
        let out_dir = [fc[0] - sc[0], fc[1] - sc[1], fc[2] - sc[2]];
        let i0 = vid(face.outer[0], &mut positions);
        for w in 1..face.outer.len() - 1 {
            let ia = vid(face.outer[w], &mut positions);
            let ib = vid(face.outer[w + 1], &mut positions);
            push_outward(&positions, &mut triangles, [i0, ia, ib], out_dir);
        }
    }
    TriMesh::new(positions, triangles)
}

#[allow(clippy::many_single_char_names)]
fn push_outward(
    positions: &[[f64; 3]],
    triangles: &mut Vec<[u32; 3]>,
    tri: [u32; 3],
    out_dir: [f64; 3],
) {
    let p = |i: u32| positions[i as usize];
    let (a, b, c) = (p(tri[0]), p(tri[1]), p(tri[2]));
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ];
    if n[0] * n[0] + n[1] * n[1] + n[2] * n[2] == 0.0 {
        return;
    }
    let dot = n[0] * out_dir[0] + n[1] * out_dir[1] + n[2] * out_dir[2];
    if dot >= 0.0 {
        triangles.push(tri);
    } else {
        triangles.push([tri[0], tri[2], tri[1]]);
    }
}

/// A stable, deterministic hash of a triangle mesh — the dedup key, the re-import content address, and the
/// hashed-mesh regression-corpus value. FNV-1a over the exact f64 bit-patterns (in the deterministic welded
/// order) + the index list, so **same file → same hash across runs and machines** (no float tolerance, no
/// platform `HashMap` seed; the tessellation is single-threaded, so vertex order is not thread-dependent). A
/// parallel/relative-deflection tessellation that reordered vertices would change this hash — exactly the
/// drift the corpus catches. (Cross-ISA bit-equality is asserted only same-machine here; a cross-ISA corpus
/// matrix is the owed CI leg.)
#[must_use]
pub fn mesh_hash(m: &TriMesh) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325; // FNV offset basis
    let mix = |x: u64, h: &mut u64| {
        *h ^= x;
        *h = h.wrapping_mul(0x0000_0100_0000_01b3); // FNV prime
    };
    mix(m.positions.len() as u64, &mut h);
    for p in &m.positions {
        for c in p {
            mix(c.to_bits(), &mut h);
        }
    }
    mix(m.triangles.len() as u64, &mut h);
    for t in &m.triangles {
        for &i in t {
            mix(u64::from(i), &mut h);
        }
    }
    h
}

/// A stable content hash of the source bytes — the import's provenance identity + the O(1) re-import key.
#[must_use]
pub fn source_hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

// ============================================================================================
// O(1) per-part re-import diff (content-addressed — "which of 1,280 parts changed")
// ============================================================================================

/// How one part changed between two imports of the same assembly (keyed by the stable part id).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum PartChange {
    /// Present in both, identical geometry + transform (a content-address match — no re-tessellation needed).
    Unchanged,
    /// Geometry changed (a different mesh hash) — re-tessellate this part only.
    GeometryChanged,
    /// Only the placement changed (same geometry, different transform) — re-instance, no re-tessellation.
    Moved,
    /// New in the second import.
    Added,
    /// Gone from the second import.
    Removed,
}

/// One entry in a re-import diff.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct PartDiff {
    /// The part id.
    pub id: u64,
    /// The part name (from whichever import has it).
    pub name: String,
    /// What changed.
    pub change: PartChange,
}

/// The **content-addressed re-import diff**: compare two imports of the same assembly part-by-part (O(n) over
/// parts, each comparison O(1) on the content-addressed geometry hash + transform) → which parts changed. No
/// tool ships per-part CAD import diffing; the content-addressed store makes it cheap. Re-importing 1,280
/// unchanged parts must be 1,280 `Unchanged` (never a full re-tessellation).
#[must_use]
pub fn diff(before: &CadImport, after: &CadImport) -> Vec<PartDiff> {
    // Index each side by (part id, geometry REFERENCE) → (geometry hash, transform, name). Keying on the
    // reference too means: (a) a proxy whose underlying (undecoded) source rep changed from A to B at the
    // same transform is NOT a false `Unchanged` (its reference changed → a distinct key → Removed+Added, both
    // "changed"); and (b) a 64-bit `id` path-hash collision between two DISTINCT references can't silently
    // collapse two parts into one key.
    type Key = (u64, String);
    let index = |imp: &CadImport| -> BTreeMap<Key, (Option<u64>, [u64; 16], String)> {
        imp.parts
            .iter()
            .map(|p| {
                let gh = p.mesh.and_then(|i| imp.meshes.get(i)).map(|m| m.hash);
                let mut t = [0u64; 16];
                for (k, v) in p.transform.iter().enumerate() {
                    t[k] = v.to_bits();
                }
                ((p.id, p.reference.clone()), (gh, t, p.name.clone()))
            })
            .collect()
    };
    let a = index(before);
    let b = index(after);

    let mut out = Vec::new();
    for (k, (gha, ta, name)) in &a {
        match b.get(k) {
            None => out.push(PartDiff {
                id: k.0,
                name: name.clone(),
                change: PartChange::Removed,
            }),
            Some((ghb, tb, name_b)) => {
                let change = if gha != ghb {
                    PartChange::GeometryChanged
                } else if ta != tb {
                    PartChange::Moved
                } else {
                    PartChange::Unchanged
                };
                out.push(PartDiff {
                    id: k.0,
                    name: name_b.clone(),
                    change,
                });
            }
        }
    }
    for (k, (_, _, name)) in &b {
        if !a.contains_key(k) {
            out.push(PartDiff {
                id: k.0,
                name: name.clone(),
                change: PartChange::Added,
            });
        }
    }
    out
}

// ============================================================================================
// The CadReader trait (the pipeline seam — invariant 5) + the readers
// ============================================================================================

/// An import that couldn't be honored at the *file* level — surfaced, never hidden. (Per-*part* failures are
/// never errors: they are reported parts, the whole point.)
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CadError {
    /// The container/format couldn't be recognized or opened.
    Unrecognized(String),
    /// The file is malformed at the container level (a corrupt ZIP, a missing product structure).
    Malformed(String),
    /// The file exceeds a size/entity cap (the decode-bomb guard).
    TooLarge(String),
}

impl std::fmt::Display for CadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unrecognized(s) => write!(f, "unrecognized CAD container: {s}"),
            Self::Malformed(s) => write!(f, "malformed CAD file: {s}"),
            Self::TooLarge(s) => write!(f, "CAD file too large: {s}"),
        }
    }
}

impl std::error::Error for CadError {}

/// The project-owned universal-import seam (invariant 5) — the one boundary between the pipeline and a
/// format backend. Native readers (CATIA/NX/…) ride a licensed kernel *behind this trait*; the neutral tier
/// (STEP) and mesh tier are pure-Rust impls. **No foreign type** (`zip::`, `quick_xml::`, a STEP-lib type)
/// crosses this surface — a reader returns the neutral [`CadImport`].
pub trait CadReader {
    /// The format tag (provenance / the report header).
    fn format(&self) -> &'static str;
    /// Cheap magic-byte sniff — does this reader handle these bytes? (Extension-independent.)
    fn can_read(&self, bytes: &[u8]) -> bool;
    /// Read the bytes into a neutral [`CadImport`] (never-empty + never-silent; container errors explained,
    /// never a panic).
    fn read(&self, bytes: &[u8]) -> Result<CadImport, CadError>;
}

/// The STEP AP242 reader as a universal [`CadReader`]: each solid in the (pure-Rust, planar-B-rep) STEP scene
/// becomes a part with **exact** deterministic tessellation; curved-only solids fall to a proxy + the OCCT
/// seam. The neutral tier fully in our control — this is the leg that handles the STEP *re-export* that also
/// black-screened Unreal, never-empty + never-silent.
#[derive(Clone, Copy, Debug, Default)]
pub struct StepAssemblyReader;

impl CadReader for StepAssemblyReader {
    fn format(&self) -> &'static str {
        "STEP-AP242"
    }

    fn can_read(&self, bytes: &[u8]) -> bool {
        // The ISO-10303-21 wrapper (allow a UTF-8 BOM / leading whitespace).
        let head = &bytes[..bytes.len().min(256)];
        std::str::from_utf8(head)
            .map(|s| s.contains("ISO-10303-21"))
            .unwrap_or(false)
    }

    fn read(&self, bytes: &[u8]) -> Result<CadImport, CadError> {
        if bytes.len() > MAX_STEP_ASSEMBLY_BYTES {
            return Err(CadError::TooLarge(format!(
                "{} bytes > {MAX_STEP_ASSEMBLY_BYTES} STEP cap",
                bytes.len()
            )));
        }
        // Decode as text. STEP is ASCII for all structural tokens (keywords, `#refs`, numbers, parens), but
        // real files are often Latin-1/Windows-1252 in string LITERALS (an accented part name). A hard UTF-8
        // gate would reject an otherwise-parseable file over one stray high byte — a black screen for a file
        // that is 99.99% ASCII. So borrow when it is valid UTF-8 (the common case, zero-copy) and otherwise
        // decode byte→char (Latin-1), consistent with the parser's own `c as char` handling.
        let owned;
        let text: &str = if let Ok(s) = std::str::from_utf8(bytes) {
            s
        } else {
            owned = bytes.iter().map(|&b| b as char).collect::<String>();
            &owned
        };
        let entities = crate::step::parse_entities(text).map_err(|e| map_step_error(&e))?;
        let src = source_hash(bytes);
        // STEP length unit is millimetres by convention.
        let units = Units {
            meters_per_unit: 0.001,
            kilograms_per_unit: 1.0,
        };

        // (A) Tessellated-assembly first — the embedded-tessellation + placement leg (curved commercial CAD:
        // cylinders/NURBS a planar reader can't cover, but the open tessellation cache + the assembly
        // transforms are readable). This is the leg the 262 MB STEP re-export that black-screened Unreal takes.
        let tess = crate::step::parse_tessellated_assembly(&entities);
        if !tess.is_empty() {
            let name =
                crate::step::file_name(&entities).unwrap_or_else(|| "STEP assembly".to_string());
            let mut raw_parts = Vec::with_capacity(tess.len());
            for (i, p) in tess.into_iter().enumerate() {
                raw_parts.push(RawPart {
                    id: i as u64,
                    name: p.name,
                    reference: p.reference,
                    transform: p.transform,
                    source: PartSource::Tessellation(p.mesh),
                });
            }
            return Ok(build_import(
                name,
                "STEP-AP242".into(),
                units,
                src,
                raw_parts,
                0,
                Vec::new(),
            ));
        }

        // (B) Planar B-rep fallback — the small-file exact leg (a hand/simple AP242 part with no tessellation).
        let scene = crate::step::interpret(&entities).map_err(|e| map_step_error(&e))?;
        let mut raw_parts = Vec::with_capacity(scene.solids.len());
        for solid in &scene.solids {
            raw_parts.push(RawPart {
                id: solid.id,
                name: format!("solid #{}", solid.id),
                reference: format!("step-solid-{}", solid.id),
                transform: IDENTITY_4X4,
                source: PartSource::ExactBrep(solid.faces.clone()),
            });
        }
        Ok(build_import(
            scene.name,
            "STEP-AP242".into(),
            scene.units,
            src,
            raw_parts,
            0,
            scene.notes,
        ))
    }
}

/// Cap for the tessellated-assembly STEP path — a commercial-CAD assembly with embedded tessellation is
/// large (the M15.7 bar file's STEP re-export is 262 MB), so this is far above the planar-subset
/// [`crate::MAX_STEP_BYTES`] (64 MB) cap, but still bounded (the decode-bomb guard).
pub const MAX_STEP_ASSEMBLY_BYTES: usize = 1024 * 1024 * 1024;

/// Map a low-level [`crate::step::StepError`] to the pipeline's [`CadError`].
fn map_step_error(e: &crate::step::StepError) -> CadError {
    match e {
        crate::step::StepError::TooLarge { .. }
        | crate::step::StepError::TooManyEntities { .. } => CadError::TooLarge(e.to_string()),
        crate::step::StepError::Empty(_) => CadError::Unrecognized(e.to_string()),
        _ => CadError::Malformed(e.to_string()),
    }
}

/// The column-major 4×4 identity.
pub const IDENTITY_4X4: [f64; 16] = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, //
    0.0, 0.0, 0.0, 1.0,
];

/// Column-major 4×4 multiply `a · b` (apply `b` then `a`) — for composing assembly transforms down the tree.
#[must_use]
pub fn mat4_mul(a: &[f64; 16], b: &[f64; 16]) -> [f64; 16] {
    let mut r = [0.0f64; 16];
    for col in 0..4 {
        for row in 0..4 {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[k * 4 + row] * b[col * 4 + k];
            }
            r[col * 4 + row] = s;
        }
    }
    r
}

/// The translation component of a column-major 4×4 (the part's world position — for the pivot/units checks).
#[must_use]
pub fn translation_of(m: &[f64; 16]) -> [f64; 3] {
    [m[12], m[13], m[14]]
}

#[cfg(feature = "3dxml")]
mod threedxml;
#[cfg(feature = "3dxml")]
pub use threedxml::ThreeDxmlReader;

#[cfg(test)]
mod tests;
