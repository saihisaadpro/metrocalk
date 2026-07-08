//! **Persistent re-import identity** (M15.10, ADR-080) — match the parts of an *edited* CAD re-import to the
//! parts of the previous import **from geometry alone**, so the engine can re-bind every user override
//! (material / collider / script / selection / joint) onto the part that survived the edit.
//!
//! **Why geometry, not IDs:** native persistent IDs do NOT survive translation — a STEP export discards the
//! feature tree ("dead" B-rep; Kripac 1997) and renumbers entities — so the source `id`/`reference` the M15.7
//! [`crate::cad_import::diff`] keys on is unreliable across a re-export. This module rebuilds identity from a
//! **rotation/translation-invariant geometric fingerprint** (volume · surface area · the principal second
//! moments · triangle count · B-rep surface-type histogram) plus a **coincidence bootstrap** (a part that was
//! filleted stays roughly in place → world-centroid proximity is a strong prior).
//!
//! **Honesty (carried verbatim from `universal-cad-import-research.md` § 2026-07):** the shipping
//! state-of-the-art matcher ("B-rep Matching", Jones et al. SIGGRAPH 2023, inside Onshape) is **learned** and
//! lands ~87–95 % correct on real edits with a small (~1–2 %) but **dangerous** *wrong-match* rate — a wrong
//! match SILENTLY corrupts an override, a miss throws a VISIBLE error. So this analytic matcher is tuned to
//! **prefer a MISS over a WRONG match**: an ambiguous or weak best-match is DECLINED (the old part's overrides
//! are flagged for the user, never silently rebound onto the wrong part), and a middle-confidence match is
//! **surfaced for adjudication**, never auto-applied. What ships here is the **deterministic analytic
//! fallback**; a learned model is the named, confidence-gated upgrade behind the same [`PartMatcher`] seam.
//!
//! **Determinism (ADR-020 boundary):** the raw f64 fingerprint derivation uses transcendentals (eigenvalues),
//! which are native-per-ISA at the ULP — so every value that a MATCH DECISION branches on is **quantized to a
//! canonical integer grid** (`quantize`) before it is compared or ordered, exactly the M15.6 witness-config
//! discipline. Same version pair ⇒ same matches ⇒ same re-bound overrides, cross-ISA.

// The geometry math (covariance, eigenvalues, transforms) indexes parallel `[_;3]`/`[[_;3];3]` arrays by
// short loop counters — the clearest form for the formulas; the pedantic single-char / range-loop lints are
// noise here (mirrors `analytic.rs`).
#![allow(clippy::many_single_char_names, clippy::needless_range_loop)]

use crate::cad_import::{CadImport, PartReport};
use metrocalk_csg::TriMesh;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A **rotation- and translation-invariant** geometric fingerprint of one part's mesh — the "changed-but-same"
/// identity key (the byte-hash of [`crate::cad_import::CadMesh`] is the O(1) "unchanged" key; this is what a
/// filleted/edited part still shares with its previous self). Every field is invariant to the part's placement
/// because it is computed from the LOCAL mesh about its own centroid.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct PartFingerprint {
    /// Enclosed signed volume (divergence theorem over the triangles). A fillet/boss changes it slightly, a
    /// total replacement changes it a lot. `0` for an open/degenerate shell.
    pub volume: f64,
    /// Total surface area (Σ triangle areas) — robust even for open shells where volume is meaningless.
    pub area: f64,
    /// The three principal second moments (eigenvalues of the mass-covariance about the centroid), sorted
    /// ascending — the shape's "aspect": a rod, a plate, and a cube have very different triples. Rotation- and
    /// translation-invariant by construction (eigenvalues of a centroid covariance).
    pub moments: [f64; 3],
    /// Triangle count — a coarse topology proxy (a re-tessellation at the same deflection is stable; a real
    /// edit shifts it).
    pub tri_count: u32,
    /// B-rep surface-type histogram `[planar, cylindrical, conical, spherical, toroidal]` — present only for an
    /// exact-B-rep part (M15.8 [`crate::analytic::AnalyticSurface`]); all-zero for a tessellation-only part.
    /// A strong discriminator when available (a bracket and a shaft differ sharply here).
    pub surface_hist: [u32; 5],
    /// **Chirality / handedness** — `+1`, `-1`, or `0` (achiral/ambiguous). The other fields (volume · area ·
    /// principal moments) are IDENTICAL for a part and its mirror twin (a left/right bracket pair), so without
    /// this a re-import would happily cross-match a left part onto its right twin and corrupt the override.
    /// The sign of the eigenvalue-ordered principal frame, each axis oriented by its third-moment (skewness)
    /// sign — a rotation-invariant pseudo-scalar that FLIPS under reflection. `0` when the part is
    /// reflection-symmetric along a principal axis (genuinely achiral) or its axes are degenerate (a
    /// near-symmetric part — then chirality carries no information and must not gate the match).
    pub chirality: i8,
}

impl PartFingerprint {
    /// An empty/degenerate fingerprint (a proxy box or a part with no real geometry) — matches only trivially.
    #[must_use]
    pub fn degenerate() -> Self {
        Self {
            volume: 0.0,
            area: 0.0,
            moments: [0.0; 3],
            tri_count: 0,
            surface_hist: [0; 5],
            chirality: 0,
        }
    }
}

/// Everything the matcher needs about one part, extracted once from a [`CadImport`]. The `mesh_hash` is the
/// byte-hash fast-path key; the `fingerprint` + `world_centroid` are the geometric-match keys.
#[derive(Clone, Debug)]
pub struct PartIdentity {
    /// The source part id (the previous import's `PartReport.id`) — the key an override is currently bound to.
    pub id: u64,
    /// The source geometry reference (the dedup key).
    pub reference: String,
    /// The content-address of the part's mesh — `Some` for real geometry, `None` for a proxy. Two parts with
    /// the same `(reference, mesh_hash)` are BYTE-identical ⇒ the O(1) "unchanged" fast path.
    pub mesh_hash: Option<u64>,
    /// The part's world centroid (its local centroid pushed through its placement) — the coincidence-bootstrap
    /// prior (an edited part stays roughly here).
    pub world_centroid: [f64; 3],
    /// The rotation/translation-invariant geometric fingerprint.
    pub fingerprint: PartFingerprint,
    /// The human name (a weak tiebreak hint — never load-bearing; a re-export can rename).
    pub name: String,
    /// The assembly occurrence this part sits under (a neighborhood prior — a part usually keeps its parent).
    pub parent: Option<u64>,
}

/// How one part of the previous import maps onto the re-imported assembly — the per-part matcher verdict.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum MatchKind {
    /// Byte-identical geometry at the same placement — the O(1) fast path; the override is trivially still
    /// valid, no re-tessellation.
    Unchanged,
    /// Byte-identical geometry, different placement — re-bind the override, no re-tessellation.
    Moved,
    /// Different geometry, matched with HIGH confidence (a fillet/boss/small edit) — auto re-bind the override.
    Strong,
    /// Matched with MIDDLE confidence — **surfaced for the user to confirm/reject**, never auto-applied to a
    /// load-bearing override (the honest fallback: prefer a question over a silent wrong bind).
    LowConfidence,
    /// No acceptable match (a total replacement / heavy edit) — the old part is treated as REMOVED: its
    /// overrides are **preserved + flagged** for the user, never silently dropped. Prefer-miss-over-wrong.
    Miss,
}

/// One entry in a re-import plan: the previous part `old_id`, the re-imported part it maps to (`new_id`, `None`
/// for a miss), the confidence `[0,1]`, and the verdict.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct PartMatch {
    /// The previous import's part id (the override is currently bound to this).
    pub old_id: u64,
    /// The re-imported part id this maps to — `None` for a [`MatchKind::Miss`] (nothing to bind to).
    pub new_id: Option<u64>,
    /// Match confidence in `[0,1]` (`1.0` for a byte-hash `Unchanged`/`Moved`).
    pub confidence: f64,
    /// The verdict.
    pub kind: MatchKind,
}

/// The result of matching a re-import against the previous import — everything the override re-binder + the
/// report + the adjudication UX need, as structured data (no drifting copy).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ReimportPlan {
    /// One entry per PREVIOUS part (its verdict). `Strong`/`Moved`/`Unchanged` → auto re-bind; `LowConfidence`
    /// → adjudicate; `Miss` → the old part's overrides are flagged (see [`Self::flagged_removed`]).
    pub matches: Vec<PartMatch>,
    /// The re-imported part ids that are NEW (matched by nothing) — they land bare/empty.
    pub added: Vec<u64>,
}

impl ReimportPlan {
    /// The previous parts whose overrides must be **preserved + flagged** ("this was on a part that no longer
    /// exists — reassign or discard?") — a [`MatchKind::Miss`], never silently dropped.
    #[must_use]
    pub fn flagged_removed(&self) -> Vec<u64> {
        self.matches
            .iter()
            .filter(|m| m.kind == MatchKind::Miss)
            .map(|m| m.old_id)
            .collect()
    }

    /// The matches the user must confirm/reject before a load-bearing override is re-bound (the adjudication
    /// surface). Never auto-applied.
    #[must_use]
    pub fn needs_adjudication(&self) -> Vec<&PartMatch> {
        self.matches
            .iter()
            .filter(|m| m.kind == MatchKind::LowConfidence)
            .collect()
    }

    /// The matches that auto re-bind (byte-hash or high-confidence geometric) — the overrides that survive
    /// with no user action.
    #[must_use]
    pub fn auto_rebinds(&self) -> Vec<&PartMatch> {
        self.matches
            .iter()
            .filter(|m| {
                matches!(
                    m.kind,
                    MatchKind::Unchanged | MatchKind::Moved | MatchKind::Strong
                ) && m.new_id.is_some()
            })
            .collect()
    }

    /// The re-bind target for a previous part id — `Some(new_id)` iff it auto re-binds. `None` for a miss, a
    /// low-confidence (must adjudicate first), or an unknown id. The override re-binder calls this.
    #[must_use]
    pub fn rebind_target(&self, old_id: u64) -> Option<u64> {
        self.matches
            .iter()
            .find(|m| m.old_id == old_id)
            .and_then(|m| {
                matches!(
                    m.kind,
                    MatchKind::Unchanged | MatchKind::Moved | MatchKind::Strong
                )
                .then_some(m.new_id)
                .flatten()
            })
    }
}

// ── Confidence thresholds (prefer-miss-over-wrong) ──────────────────────────────────────────────────────────
/// At/above this combined confidence a geometric match auto re-binds (a small edit — fillet/boss/hole).
const STRONG_THRESHOLD: f64 = 0.82;
/// In `[LOW, STRONG)` the match is surfaced for adjudication; below `LOW` it is a MISS (prefer-miss). The band
/// is deliberately wide so a heavily-edited part lands in a MISS, not a silent wrong bind.
const LOW_THRESHOLD: f64 = 0.55;

/// The fingerprint of a triangle mesh, with an optional B-rep face list for the surface-type histogram.
#[must_use]
pub fn fingerprint(tris: &TriMesh, faces: Option<&[crate::step::CadFace]>) -> PartFingerprint {
    if tris.triangles.is_empty() {
        return PartFingerprint::degenerate();
    }
    let (volume, centroid, cov) = volume_centroid_covariance(tris);
    let area = surface_area(tris);
    let moments = sym3_eigenvalues_sorted(cov);
    let chirality = chirality_sign(tris, centroid, cov, moments);
    #[allow(clippy::cast_possible_truncation)]
    let tri_count = tris.triangles.len() as u32;
    let mut surface_hist = [0u32; 5];
    if let Some(faces) = faces {
        use crate::analytic::AnalyticSurface;
        for f in faces {
            let slot = match &f.surface {
                None => 0, // planar
                Some(AnalyticSurface::Cylinder { .. }) => 1,
                Some(AnalyticSurface::Cone { .. }) => 2,
                Some(AnalyticSurface::Sphere { .. }) => 3,
                Some(AnalyticSurface::Torus { .. }) => 4,
            };
            surface_hist[slot] += 1;
        }
    }
    PartFingerprint {
        volume,
        area,
        moments,
        tri_count,
        surface_hist,
        chirality,
    }
}

/// Extract the [`PartIdentity`] list from an import — the matcher's input (computed once per side).
#[must_use]
pub fn identities(import: &CadImport) -> Vec<PartIdentity> {
    import
        .parts
        .iter()
        .map(|p| identity_of(p, import))
        .collect()
}

fn identity_of(p: &PartReport, import: &CadImport) -> PartIdentity {
    let mesh = p.mesh.and_then(|i| import.meshes.get(i));
    let (fp, local_centroid, mesh_hash) = match mesh {
        Some(m) if !m.is_proxy => {
            let (_v, c, _cov) = volume_centroid_covariance(&m.tris);
            let faces = brep_faces(p);
            (fingerprint(&m.tris, faces.as_deref()), c, Some(m.hash))
        }
        _ => (PartFingerprint::degenerate(), [0.0; 3], None),
    };
    PartIdentity {
        id: p.id,
        reference: p.reference.clone(),
        mesh_hash,
        world_centroid: apply_transform(&p.transform, local_centroid),
        fingerprint: fp,
        name: p.name.clone(),
        parent: p.parent,
    }
}

/// The B-rep face list of a part, if it carries exact geometry (for the surface-type histogram). `PartReport`
/// keeps only a mesh index, so the exact faces are reachable only when the report is paired with its raw
/// source; here we conservatively return `None` (tessellation-only fingerprint) unless a caller supplies them.
/// The surface histogram is a BONUS discriminator, never the sole key — volume/area/moments carry the match.
fn brep_faces(_p: &PartReport) -> Option<Vec<crate::step::CadFace>> {
    None
}

/// Match a re-imported assembly (`after`) against the previous import (`before`) — the M15.10 core. Two layers:
/// **(1)** a byte-hash fast path (`(reference, mesh_hash)` identical ⇒ `Unchanged`/`Moved`, O(1), confidence
/// 1.0); **(2)** a confidence-scored geometric match over the remainder, **preferring a MISS over a WRONG
/// match** (a weak best-match is declined, a middle one is surfaced for adjudication). Deterministic: all
/// decision inputs are quantized (`quantize`) so the greedy order + verdicts are cross-ISA stable.
#[must_use]
pub fn match_reimport(before: &CadImport, after: &CadImport) -> ReimportPlan {
    let old = identities(before);
    let new = identities(after);
    match_identities(&old, &new)
}

/// [`match_reimport`] on already-extracted identities (so the engine can fingerprint its live parts without a
/// second parse). The whole matcher lives here — pure, deterministic, unit-testable.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn match_identities(old: &[PartIdentity], new: &[PartIdentity]) -> ReimportPlan {
    // Assembly scale for the spatial-proximity normalization: the diagonal of all parts' world centroids.
    let scale = assembly_scale(old, new);

    let mut new_taken = vec![false; new.len()];
    let mut matches: Vec<PartMatch> = Vec::with_capacity(old.len());
    let mut matched_old = vec![false; old.len()];

    // ── Layer 1: byte-hash fast path (O(n) over parts, O(1) per compare). A part whose (reference, mesh_hash)
    // is unique + identical on both sides is Unchanged (same placement) or Moved (different placement). We
    // index the `new` side by its byte key; a key shared by >1 part on either side is ambiguous → left to the
    // geometric layer (instancing means many parts share a mesh hash, but the reference+hash+centroid picks
    // the placement). ──────────────────────────────────────────────────────────────────────────────────────
    let mut new_by_key: BTreeMap<(String, u64), Vec<usize>> = BTreeMap::new();
    for (j, n) in new.iter().enumerate() {
        if let Some(h) = n.mesh_hash {
            new_by_key
                .entry((n.reference.clone(), h))
                .or_default()
                .push(j);
        }
    }
    for (i, o) in old.iter().enumerate() {
        let Some(h) = o.mesh_hash else { continue };
        let key = (o.reference.clone(), h);
        let Some(cands) = new_by_key.get(&key) else {
            continue;
        };
        // Pick the un-taken candidate closest to the old placement (handles instancing: same mesh, N places).
        let best = cands
            .iter()
            .copied()
            .filter(|&j| !new_taken[j])
            .min_by(|&a, &b| {
                let da = quantize(centroid_dist(o.world_centroid, new[a].world_centroid));
                let db = quantize(centroid_dist(o.world_centroid, new[b].world_centroid));
                da.cmp(&db).then(a.cmp(&b))
            });
        if let Some(j) = best {
            new_taken[j] = true;
            matched_old[i] = true;
            let moved = quantize(centroid_dist(o.world_centroid, new[j].world_centroid))
                > quantize(1e-6 * scale);
            matches.push(PartMatch {
                old_id: o.id,
                new_id: Some(new[j].id),
                confidence: 1.0,
                kind: if moved {
                    MatchKind::Moved
                } else {
                    MatchKind::Unchanged
                },
            });
        }
    }

    // ── Layer 2: geometric match over the remainder. Score every (un-matched old, un-taken new) pair, then
    // GREEDILY assign best-confidence-first (each part used once), quantized so the order is deterministic. ──
    let mut pairs: Vec<(u64, usize, usize, f64)> = Vec::new(); // (qconf, i, j, raw_conf)
    for (i, o) in old.iter().enumerate() {
        if matched_old[i] || o.mesh_hash.is_none() {
            continue;
        }
        for (j, n) in new.iter().enumerate() {
            if new_taken[j] || n.mesh_hash.is_none() {
                continue;
            }
            let c = confidence(o, n, scale);
            if c >= LOW_THRESHOLD {
                pairs.push((quantize(c), i, j, c));
            }
        }
    }
    // Highest quantized confidence first; ties broken by (old id, new id) for determinism.
    pairs.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(old[a.1].id.cmp(&old[b.1].id))
            .then(new[a.2].id.cmp(&new[b.2].id))
    });
    for (_q, i, j, raw) in pairs {
        if matched_old[i] || new_taken[j] {
            continue;
        }
        matched_old[i] = true;
        new_taken[j] = true;
        let kind = if raw >= STRONG_THRESHOLD {
            MatchKind::Strong
        } else {
            MatchKind::LowConfidence
        };
        matches.push(PartMatch {
            old_id: old[i].id,
            new_id: Some(new[j].id),
            confidence: raw,
            kind,
        });
    }

    // Any old part still unmatched is a MISS (prefer-miss: no acceptable geometric match) — its overrides are
    // flagged, never silently dropped. Proxies (no mesh_hash) with no byte match also fall here.
    for (i, o) in old.iter().enumerate() {
        if !matched_old[i] {
            matches.push(PartMatch {
                old_id: o.id,
                new_id: None,
                confidence: 0.0,
                kind: MatchKind::Miss,
            });
        }
    }

    // Any un-taken new part is ADDED (bare/empty).
    let mut added: Vec<u64> = new
        .iter()
        .enumerate()
        .filter(|(j, _)| !new_taken[*j])
        .map(|(_, n)| n.id)
        .collect();
    added.sort_unstable();

    // Sort matches by old id for a stable, queryable plan.
    matches.sort_by_key(|m| m.old_id);
    ReimportPlan { matches, added }
}

// ── Confidence scoring ──────────────────────────────────────────────────────────────────────────────────────

/// The combined match confidence in `[0,1]` between a previous part `o` and a candidate `n`: a shape-similarity
/// term (volume · area · moments · tri-count · surface histogram) fused with a coincidence-bootstrap spatial
/// term (an edited part stays roughly in place). Tuned so a fillet/boss/hole scores `Strong` and a total
/// replacement scores below `LOW` (a MISS) — prefer-miss-over-wrong.
fn confidence(o: &PartIdentity, n: &PartIdentity, scale: f64) -> f64 {
    let fo = &o.fingerprint;
    let fn_ = &n.fingerprint;
    if fo.tri_count == 0 || fn_.tri_count == 0 {
        return 0.0; // a degenerate/proxy part can't be geometrically matched
    }
    // CHIRALITY GATE: a part and its mirror twin share volume/area/moments but have OPPOSITE handedness. If
    // both sides are definitively chiral (±1) and DISAGREE, they are a left/right pair, not the same edited
    // part — force a MISS (prefer-miss-over-wrong; a wrong bind onto a mirror twin silently corrupts). A `0`
    // on either side (achiral or a near-symmetric part whose axes are ambiguous) carries no info → no gate.
    if fo.chirality != 0 && fn_.chirality != 0 && fo.chirality != fn_.chirality {
        return 0.0;
    }
    // Relative similarity of a scalar pair: 1 when equal, → 0 as they diverge (symmetric relative difference).
    let rel = |a: f64, b: f64| -> f64 {
        let d = (a - b).abs();
        let m = a.abs().max(b.abs()).max(1e-12);
        (1.0 - d / m).max(0.0)
    };
    let vol = rel(fo.volume, fn_.volume);
    let area = rel(fo.area, fn_.area);
    let mom = (0..3)
        .map(|k| rel(fo.moments[k], fn_.moments[k]))
        .sum::<f64>()
        / 3.0;
    let tris = rel(f64::from(fo.tri_count), f64::from(fn_.tri_count));
    // Surface-type histogram cosine similarity (1 when the same mix of planar/cyl/cone/sphere/torus). Only
    // when BOTH sides carry B-rep faces; else neutral (doesn't penalize tessellation-only parts).
    let surf = surface_similarity(&fo.surface_hist, &fn_.surface_hist);
    // Shape term: moments + volume/area dominate (they carry the real shape); tri-count is a coarse assist.
    let shape = 0.34 * mom + 0.24 * vol + 0.24 * area + 0.08 * tris + 0.10 * surf;

    // Coincidence bootstrap: an edited part barely moves. Full credit within ~2 % of assembly scale, decaying
    // to 0 by ~40 %. A part that jumped across the assembly is unlikely to be "the same edited part".
    let d = centroid_dist(o.world_centroid, n.world_centroid);
    let near = if scale <= 0.0 {
        1.0
    } else {
        let r = d / scale;
        (1.0 - (r - 0.02).max(0.0) / 0.38).clamp(0.0, 1.0)
    };

    // Fuse: shape carries the verdict, spatial proximity gates it (a shape twin across the room is suspicious;
    // a shape twin in place is the edit). Weighted product-ish blend keeps a low spatial term from passing on
    // shape alone.
    let base = 0.70 * shape + 0.30 * near;
    // A name match is a weak nudge (never load-bearing — a re-export can rename).
    let name_bonus = if !o.name.is_empty() && o.name == n.name {
        0.04
    } else {
        0.0
    };
    (base + name_bonus).clamp(0.0, 1.0)
}

fn surface_similarity(a: &[u32; 5], b: &[u32; 5]) -> f64 {
    let sa: u32 = a.iter().sum();
    let sb: u32 = b.iter().sum();
    if sa == 0 || sb == 0 {
        return 1.0; // no B-rep info on one side → neutral (don't penalize)
    }
    let mut dot = 0.0;
    let (mut na, mut nb) = (0.0, 0.0);
    for k in 0..5 {
        let (x, y) = (f64::from(a[k]), f64::from(b[k]));
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na <= 0.0 || nb <= 0.0 {
        1.0
    } else {
        (dot / (na.sqrt() * nb.sqrt())).clamp(0.0, 1.0)
    }
}

// ── Geometry: volume / centroid / covariance / eigenvalues (pure, deterministic) ────────────────────────────

/// The enclosed signed volume, the centroid, and the mass-covariance (second-moment matrix) about the centroid
/// of a triangle mesh. Divergence-theorem decomposition into tetrahedra from the origin (Blow & Binstock 2004
/// covariance form) — exact for a closed mesh, a graceful approximation for an open shell (volume→~0, the
/// covariance still describes the surface's spatial spread, which is what the fingerprint needs).
fn volume_centroid_covariance(tris: &TriMesh) -> (f64, [f64; 3], [[f64; 3]; 3]) {
    // Canonical unit-tetrahedron covariance (about the origin), scaled per-tet by det(A).
    const C: [[f64; 3]; 3] = [
        [1.0 / 60.0, 1.0 / 120.0, 1.0 / 120.0],
        [1.0 / 120.0, 1.0 / 60.0, 1.0 / 120.0],
        [1.0 / 120.0, 1.0 / 120.0, 1.0 / 60.0],
    ];
    let mut vol = 0.0;
    let mut centroid = [0.0; 3];
    let mut cov = [[0.0; 3]; 3];
    for t in &tris.triangles {
        let (Some(&a), Some(&b), Some(&c)) = (
            tris.positions.get(t[0] as usize),
            tris.positions.get(t[1] as usize),
            tris.positions.get(t[2] as usize),
        ) else {
            continue;
        };
        // det([a b c]) = 6 × signed volume of the tet (origin, a, b, c).
        let det = a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0]);
        let tet_vol = det / 6.0;
        vol += tet_vol;
        // Tet centroid = (a+b+c)/4 (the 4th vertex is the origin) → volume-weighted running centroid.
        for k in 0..3 {
            centroid[k] += tet_vol * (a[k] + b[k] + c[k]) / 4.0;
        }
        // Covariance contribution: det × A · C · Aᵀ, where A = [a b c] (columns).
        let cols = [a, b, c];
        for r in 0..3 {
            for s in 0..3 {
                // (A·C·Aᵀ)[r][s] = Σ_{p,q} A[r][p] · C[p][q] · A[s][q]
                let mut v = 0.0;
                for p in 0..3 {
                    for q in 0..3 {
                        v += cols[p][r] * C[p][q] * cols[q][s];
                    }
                }
                cov[r][s] += det * v;
            }
        }
    }
    if vol.abs() > 1e-15 {
        for k in 0..3 {
            centroid[k] /= vol;
        }
    }
    // Translate the covariance to the centroid (parallel-axis for second moments): C_c = C_o − V·(c⊗c).
    for r in 0..3 {
        for s in 0..3 {
            cov[r][s] -= vol * centroid[r] * centroid[s];
        }
    }
    (vol.abs(), centroid, cov)
}

fn surface_area(tris: &TriMesh) -> f64 {
    let mut area = 0.0;
    for t in &tris.triangles {
        let (Some(&a), Some(&b), Some(&c)) = (
            tris.positions.get(t[0] as usize),
            tris.positions.get(t[1] as usize),
            tris.positions.get(t[2] as usize),
        ) else {
            continue;
        };
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let cr = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        area += 0.5 * (cr[0] * cr[0] + cr[1] * cr[1] + cr[2] * cr[2]).sqrt();
    }
    area
}

/// The three eigenvalues of a symmetric 3×3 matrix, sorted ascending — the analytic trigonometric method
/// (Smith 1961; deterministic, no iteration). Used for the rotation-invariant principal moments.
fn sym3_eigenvalues_sorted(m: [[f64; 3]; 3]) -> [f64; 3] {
    let p1 = m[0][1].powi(2) + m[0][2].powi(2) + m[1][2].powi(2);
    let mut e = if p1 <= 1e-30 {
        // Already diagonal.
        [m[0][0], m[1][1], m[2][2]]
    } else {
        let q = (m[0][0] + m[1][1] + m[2][2]) / 3.0;
        let p2 = (m[0][0] - q).powi(2) + (m[1][1] - q).powi(2) + (m[2][2] - q).powi(2) + 2.0 * p1;
        let p = (p2 / 6.0).sqrt();
        // B = (1/p)·(M − q·I); r = det(B)/2, clamped to [-1,1] for numerical safety.
        let b = [
            [(m[0][0] - q) / p, m[0][1] / p, m[0][2] / p],
            [m[1][0] / p, (m[1][1] - q) / p, m[1][2] / p],
            [m[2][0] / p, m[2][1] / p, (m[2][2] - q) / p],
        ];
        let det_b = b[0][0] * (b[1][1] * b[2][2] - b[1][2] * b[2][1])
            - b[0][1] * (b[1][0] * b[2][2] - b[1][2] * b[2][0])
            + b[0][2] * (b[1][0] * b[2][1] - b[1][1] * b[2][0]);
        let r = (det_b / 2.0).clamp(-1.0, 1.0);
        let phi = r.acos() / 3.0;
        let e1 = q + 2.0 * p * phi.cos();
        let e3 = q + 2.0 * p * (phi + 2.0 * std::f64::consts::PI / 3.0).cos();
        let e2 = 3.0 * q - e1 - e3;
        [e1, e2, e3]
    };
    e.sort_by(f64::total_cmp);
    e
}

/// The **chirality (handedness) pseudo-scalar** of a mesh: `+1`, `-1`, or `0` (achiral / ambiguous). Volume,
/// area, and the principal moments are IDENTICAL for a part and its mirror twin — this is the ONE fingerprint
/// term that flips under reflection, so a left bracket doesn't cross-match its right twin on re-import.
///
/// Construction: the eigenvalue-ordered principal frame `[e0,e1,e2]` (ascending), each axis **oriented by the
/// sign of its third moment** (skewness) so the frame is reflection-covariant, then `sign(det[e0 e1 e2])`.
/// Returns `0` (no chirality gate) when the shape is reflection-symmetric along a principal axis (third moment
/// ≈ 0 → genuinely achiral, e.g. a box) OR its principal axes are DEGENERATE (near-equal eigenvalues → the
/// axes aren't well-defined, the near-symmetric case — chirality must not gate an already-hard match).
fn chirality_sign(tris: &TriMesh, centroid: [f64; 3], cov: [[f64; 3]; 3], evals: [f64; 3]) -> i8 {
    let spread = evals[2] - evals[0];
    if spread <= 1e-12 {
        return 0; // spherical inertia — achiral
    }
    // Degenerate principal axes (a plate/rod/near-symmetric part): the eigenvectors of a repeated eigenvalue
    // are an arbitrary basis of the eigenspace → the frame (and its determinant) is meaningless. Don't gate.
    if (evals[1] - evals[0]) < 0.03 * spread || (evals[2] - evals[1]) < 0.03 * spread {
        return 0;
    }
    // The largest-extent axis for the third-moment tolerance (scale-aware).
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in &tris.positions {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let ext = (0..3)
        .map(|k| hi[k] - lo[k])
        .fold(0.0_f64, f64::max)
        .max(1e-9);
    #[allow(clippy::cast_precision_loss)]
    // a vertex count → f64 divisor; precision is irrelevant here
    let n = tris.positions.len().max(1) as f64;

    let mut frame = [[0.0; 3]; 3];
    for i in 0..3 {
        let mut e = eigenvector(cov, evals[i]);
        // Orient by the third central moment along this axis: Σ ((v−c)·e)³ / N, normalized by ext³.
        let mut m3 = 0.0;
        for p in &tris.positions {
            let d = (p[0] - centroid[0]) * e[0]
                + (p[1] - centroid[1]) * e[1]
                + (p[2] - centroid[2]) * e[2];
            m3 += d * d * d;
        }
        let m3n = m3 / (n * ext * ext * ext);
        if m3n.abs() < 1e-4 {
            return 0; // reflection-symmetric along this axis → achiral (a box, a symmetric bracket)
        }
        if m3n < 0.0 {
            e = [-e[0], -e[1], -e[2]];
        }
        frame[i] = e;
    }
    // det([e0 e1 e2]) = e0 · (e1 × e2) → ±1 for an orthonormal frame; its sign is the handedness.
    let cross = [
        frame[1][1] * frame[2][2] - frame[1][2] * frame[2][1],
        frame[1][2] * frame[2][0] - frame[1][0] * frame[2][2],
        frame[1][0] * frame[2][1] - frame[1][1] * frame[2][0],
    ];
    let det = frame[0][0] * cross[0] + frame[0][1] * cross[1] + frame[0][2] * cross[2];
    if det > 0.0 {
        1
    } else if det < 0.0 {
        -1
    } else {
        0
    }
}

/// A unit eigenvector of a symmetric 3×3 `m` for the (known) eigenvalue `lambda` — the null space of
/// `(m − λI)`, found as the longest cross product of its rows (rank-2 for a simple eigenvalue). Deterministic.
fn eigenvector(m: [[f64; 3]; 3], lambda: f64) -> [f64; 3] {
    let r = [
        [m[0][0] - lambda, m[0][1], m[0][2]],
        [m[1][0], m[1][1] - lambda, m[1][2]],
        [m[2][0], m[2][1], m[2][2] - lambda],
    ];
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let cands = [cross(r[0], r[1]), cross(r[1], r[2]), cross(r[2], r[0])];
    // The most numerically robust null-space direction = the longest cross product.
    let best = cands
        .iter()
        .copied()
        .max_by(|a, b| {
            let la = a[0] * a[0] + a[1] * a[1] + a[2] * a[2];
            let lb = b[0] * b[0] + b[1] * b[1] + b[2] * b[2];
            la.partial_cmp(&lb).unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or([1.0, 0.0, 0.0]);
    let len = (best[0] * best[0] + best[1] * best[1] + best[2] * best[2]).sqrt();
    if len <= 1e-30 {
        [1.0, 0.0, 0.0]
    } else {
        [best[0] / len, best[1] / len, best[2] / len]
    }
}

// ── small helpers ───────────────────────────────────────────────────────────────────────────────────────────

fn apply_transform(m: &[f64; 16], p: [f64; 3]) -> [f64; 3] {
    // Column-major 4×4 × (p,1).
    [
        m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
        m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
        m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
    ]
}

fn centroid_dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The assembly's spatial scale = the diagonal of the bounding box of all parts' world centroids (both sides).
/// Normalizes the coincidence-bootstrap distance so the matcher is unit-agnostic.
fn assembly_scale(old: &[PartIdentity], new: &[PartIdentity]) -> f64 {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in old.iter().chain(new.iter()) {
        for k in 0..3 {
            lo[k] = lo[k].min(p.world_centroid[k]);
            hi[k] = hi[k].max(p.world_centroid[k]);
        }
    }
    if !lo[0].is_finite() {
        return 1.0;
    }
    let d = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt();
    d.max(1e-9)
}

/// Quantize a confidence/decision scalar to a canonical integer grid (1e-6 steps) so a MATCH DECISION never
/// branches on sub-ULP transcendental noise — the ADR-020 native-derivation / portable-decision boundary (the
/// M15.6 witness-config discipline applied to matching). Same version pair ⇒ identical quantized order ⇒
/// identical matches cross-ISA.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn quantize(x: f64) -> u64 {
    // Confidences are in [0,1]; distances are ≥0 and normalized by scale. Shift so small negatives (if any)
    // don't wrap; 1e6 resolution is far above the transcendental ULP noise.
    ((x.max(0.0)) * 1_000_000.0).round() as u64
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // the determinism/exactness assertions compare quantized or exact values on purpose
mod tests {
    use super::*;

    /// An axis-aligned box centred at the origin with the given half-extents → a closed 12-triangle mesh.
    fn box_mesh(hx: f64, hy: f64, hz: f64) -> TriMesh {
        let p = [
            [-hx, -hy, -hz],
            [hx, -hy, -hz],
            [hx, hy, -hz],
            [-hx, hy, -hz],
            [-hx, -hy, hz],
            [hx, -hy, hz],
            [hx, hy, hz],
            [-hx, hy, hz],
        ];
        // Outward-facing (CCW) triangles.
        let t = [
            [0u32, 3, 2],
            [0, 2, 1], // -z
            [4, 5, 6],
            [4, 6, 7], // +z
            [0, 1, 5],
            [0, 5, 4], // -y
            [2, 3, 7],
            [2, 7, 6], // +y
            [1, 2, 6],
            [1, 6, 5], // +x
            [0, 4, 7],
            [0, 7, 3], // -x
        ];
        TriMesh {
            positions: p.to_vec(),
            triangles: t.to_vec(),
        }
    }

    /// Rotate a mesh's vertices about z by `deg` and translate by `t` — same solid, different placement.
    fn place(mesh: &TriMesh, deg: f64, t: [f64; 3]) -> TriMesh {
        let r = deg.to_radians();
        let (c, s) = (r.cos(), r.sin());
        let positions = mesh
            .positions
            .iter()
            .map(|p| {
                [
                    c * p[0] - s * p[1] + t[0],
                    s * p[0] + c * p[1] + t[1],
                    p[2] + t[2],
                ]
            })
            .collect();
        TriMesh {
            positions,
            triangles: mesh.triangles.clone(),
        }
    }

    /// An IRREGULAR (chiral) tetrahedron — 4 generic points with no reflection symmetry → a genuine
    /// left/right pair under mirroring. Its `mirror` (x negated, winding flipped to stay outward) has the
    /// SAME volume/area/moments but the OPPOSITE chirality sign.
    fn chiral_tet(mirror: bool) -> TriMesh {
        let sx = if mirror { -1.0 } else { 1.0 };
        let v = [
            [0.0, 0.0, 0.0],
            [2.0 * sx, 0.2, 0.1],
            [0.3 * sx, 1.4, 0.2],
            [0.5 * sx, 0.4, 1.7],
        ];
        // Faces wound outward; when mirrored (sx<0) reverse each winding so normals stay outward.
        let f = if mirror {
            [[0u32, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]]
        } else {
            [[0u32, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]]
        };
        TriMesh {
            positions: v.to_vec(),
            triangles: f.to_vec(),
        }
    }

    #[test]
    fn chirality_distinguishes_a_mirror_pair_that_shares_all_other_invariants() {
        let left = fingerprint(&chiral_tet(false), None);
        let right = fingerprint(&chiral_tet(true), None);
        // The scalar invariants are identical (the mirror shares them) — that's why chirality is NEEDED.
        assert_eq!(
            quantize(left.volume),
            quantize(right.volume),
            "mirror shares volume"
        );
        assert_eq!(
            quantize(left.area),
            quantize(right.area),
            "mirror shares area"
        );
        for k in 0..3 {
            assert_eq!(
                quantize(left.moments[k]),
                quantize(right.moments[k]),
                "mirror shares moment {k}"
            );
        }
        // …but the chirality sign is OPPOSITE and definite (both non-zero).
        assert!(
            left.chirality != 0 && right.chirality != 0,
            "a chiral tet is definitively handed"
        );
        assert_eq!(
            left.chirality, -right.chirality,
            "the mirror flips handedness: {} vs {}",
            left.chirality, right.chirality
        );
    }

    #[test]
    fn a_box_is_achiral_so_chirality_never_falsely_gates_a_symmetric_part() {
        // A symmetric part (a box) must report chirality 0 — else a fillet that breaks a near-symmetry could
        // flip a spurious sign and wrongly MISS a true match.
        assert_eq!(
            fingerprint(&box_mesh(1.0, 0.6, 0.3), None).chirality,
            0,
            "a box is achiral"
        );
    }

    #[test]
    fn a_mirror_twin_does_not_cross_match_its_pair() {
        // THE REAL-CAD FAILURE MODE: an assembly with a left AND a right bracket. On re-import the matcher
        // must NOT bind old-left → new-right (a silent override corruption). The chirality gate forces a miss.
        let left = chiral_tet(false);
        let right = chiral_tet(true);
        // Old scene: only the LEFT bracket (id 1). Re-import: only the RIGHT twin at the same spot (id 9).
        let old = vec![ident(1, &left, Some(0x1E11), [0.0, 0.0, 0.0])];
        let new = vec![ident(9, &right, Some(0x1247), [0.05, 0.0, 0.0])];
        let plan = match_identities(&old, &new);
        let m = plan.matches.iter().find(|x| x.old_id == 1).unwrap();
        assert_eq!(
            m.kind,
            MatchKind::Miss,
            "the left bracket does NOT cross-match its right twin: {m:?}"
        );
        assert!(
            plan.added.contains(&9),
            "the right twin is a new part, not a re-bind target"
        );
    }

    fn ident(id: u64, mesh: &TriMesh, hash: Option<u64>, centroid: [f64; 3]) -> PartIdentity {
        PartIdentity {
            id,
            reference: format!("r{id}"),
            mesh_hash: hash,
            world_centroid: centroid,
            fingerprint: fingerprint(mesh, None),
            name: format!("part{id}"),
            parent: None,
        }
    }

    #[test]
    fn fingerprint_is_rotation_and_translation_invariant() {
        // The whole premise: a part's identity survives placement. A box, and the SAME box rotated 37° +
        // translated 5 units, must fingerprint IDENTICALLY (to the quantized decision grid).
        let a = box_mesh(1.0, 0.6, 0.3);
        let b = place(&a, 37.0, [5.0, -2.0, 1.5]);
        let (fa, fb) = (fingerprint(&a, None), fingerprint(&b, None));
        assert_eq!(quantize(fa.volume), quantize(fb.volume), "volume invariant");
        assert_eq!(quantize(fa.area), quantize(fb.area), "area invariant");
        for k in 0..3 {
            assert_eq!(
                quantize(fa.moments[k]),
                quantize(fb.moments[k]),
                "principal moment {k} invariant under rotation+translation"
            );
        }
        // The volume is right (2·1 × 2·0.6 × 2·0.3 = 1.44).
        assert!(
            (fa.volume - 1.44).abs() < 1e-9,
            "closed-box volume: {}",
            fa.volume
        );
    }

    #[test]
    fn a_small_edit_scores_strong_a_heavy_edit_misses_never_wrong() {
        // A cube in place; the re-import has (a) the same cube slightly shrunk [a fillet/small edit] AND
        // (b) a long rod where nothing was. The cube must STRONG-match its shrunk self, and NOTHING must
        // wrong-match the rod (prefer-miss): the rod is ADDED, not bound to the cube's overrides.
        let cube = box_mesh(1.0, 1.0, 1.0);
        let shrunk = box_mesh(0.96, 0.96, 0.96); // ~a small edit — volume −11%
        let rod = box_mesh(0.1, 0.1, 3.0); // a totally different shape, elsewhere

        let old = vec![ident(1, &cube, Some(0xAAAA), [0.0, 0.0, 0.0])];
        let new = vec![
            ident(10, &shrunk, Some(0xBBBB), [0.0, 0.0, 0.0]), // same place, new hash (edited)
            ident(11, &rod, Some(0xCCCC), [8.0, 0.0, 0.0]),    // far away, different shape
        ];
        let plan = match_identities(&old, &new);

        let cube_match = plan.matches.iter().find(|m| m.old_id == 1).unwrap();
        assert_eq!(
            cube_match.kind,
            MatchKind::Strong,
            "small edit strong-matches: {cube_match:?}"
        );
        assert_eq!(
            cube_match.new_id,
            Some(10),
            "the cube re-binds to its shrunk self"
        );
        assert!(cube_match.confidence >= STRONG_THRESHOLD);
        // The rod is ADDED (nothing wrong-matched to it), and it is NOT the cube's target.
        assert!(
            plan.added.contains(&11),
            "the rod is a new part: {:?}",
            plan.added
        );
        assert_eq!(
            plan.rebind_target(1),
            Some(10),
            "the cube's overrides re-bind to the matched part"
        );
    }

    #[test]
    fn a_replacement_prefers_a_miss_over_a_wrong_bind() {
        // The dangerous case: an old cube is DELETED and a very different rod takes roughly its place. The
        // matcher must MISS the cube (flag its overrides) rather than silently rebind them onto the rod.
        let cube = box_mesh(1.0, 1.0, 1.0);
        let rod = box_mesh(0.08, 0.08, 4.0); // wildly different moments, ~same spot
        let old = vec![ident(1, &cube, Some(0xAAAA), [0.0, 0.0, 0.0])];
        let new = vec![ident(9, &rod, Some(0xDDDD), [0.2, 0.0, 0.0])];
        let plan = match_identities(&old, &new);
        let m = plan.matches.iter().find(|x| x.old_id == 1).unwrap();
        assert_eq!(
            m.kind,
            MatchKind::Miss,
            "a heavy shape change misses (never a wrong bind): {m:?}"
        );
        assert_eq!(
            plan.rebind_target(1),
            None,
            "no override is silently re-bound"
        );
        assert_eq!(
            plan.flagged_removed(),
            vec![1],
            "the deleted part's overrides are FLAGGED, not lost"
        );
        assert!(plan.added.contains(&9), "the rod is new");
    }

    #[test]
    fn byte_identical_is_the_o1_fast_path_unchanged_or_moved() {
        let cube = box_mesh(1.0, 1.0, 1.0);
        let old = vec![
            ident(1, &cube, Some(0xAAAA), [0.0, 0.0, 0.0]),
            ident(2, &cube, Some(0xAAAA), [10.0, 0.0, 0.0]), // an instance of the same mesh elsewhere
        ];
        // Re-import: part 1 unchanged, part 2 moved (same hash, new place).
        let new = vec![
            ident(1, &cube, Some(0xAAAA), [0.0, 0.0, 0.0]),
            ident(2, &cube, Some(0xAAAA), [10.0, 3.0, 0.0]),
        ];
        // References must match for the byte-key: fix them to the SAME references across imports.
        let mut old = old;
        let mut new = new;
        for p in old.iter_mut().chain(new.iter_mut()) {
            p.reference = "sharedbox".into();
        }
        let plan = match_identities(&old, &new);
        let m1 = plan.matches.iter().find(|m| m.old_id == 1).unwrap();
        let m2 = plan.matches.iter().find(|m| m.old_id == 2).unwrap();
        assert_eq!(
            m1.kind,
            MatchKind::Unchanged,
            "same hash + same place = O(1) unchanged"
        );
        assert_eq!(m1.confidence, 1.0);
        assert_eq!(
            m2.kind,
            MatchKind::Moved,
            "same hash + new place = moved (re-bind, no re-tessellation)"
        );
        assert_eq!(
            plan.rebind_target(2),
            Some(2),
            "the moved instance still re-binds"
        );
    }

    #[test]
    fn added_and_removed_are_diffed_and_flagged() {
        let cube = box_mesh(1.0, 1.0, 1.0);
        let plate = box_mesh(2.0, 2.0, 0.1);
        let old = vec![
            ident(1, &cube, Some(0xAAAA), [0.0, 0.0, 0.0]),
            ident(2, &plate, Some(0xBBBB), [5.0, 0.0, 0.0]), // this one gets deleted
        ];
        let new = vec![
            ident(1, &cube, Some(0xAAAA), [0.0, 0.0, 0.0]), // unchanged
            ident(3, &cube, Some(0xEEEE), [9.0, 0.0, 0.0]), // a NEW bracket
        ];
        let mut old = old;
        let mut new = new;
        // Make part 1's reference match across imports so it byte-matches; leave 2/3 distinct.
        old[0].reference = "cube".into();
        new[0].reference = "cube".into();
        let plan = match_identities(&old, &new);
        assert!(
            plan.flagged_removed().contains(&2),
            "the deleted plate is flagged: {:?}",
            plan.flagged_removed()
        );
        assert!(
            plan.added.contains(&3),
            "the new bracket is added: {:?}",
            plan.added
        );
        assert_eq!(
            plan.matches.iter().find(|m| m.old_id == 1).unwrap().kind,
            MatchKind::Unchanged
        );
    }

    /// A `PartIdentity` with an explicit surface histogram (to measure the histogram's discriminating lift).
    fn ident_h(
        id: u64,
        mesh: &TriMesh,
        hash: u64,
        centroid: [f64; 3],
        hist: [u32; 5],
    ) -> PartIdentity {
        let mut fp = fingerprint(mesh, None);
        fp.surface_hist = hist;
        PartIdentity {
            id,
            reference: format!("r{id}"),
            mesh_hash: Some(hash),
            world_centroid: centroid,
            fingerprint: fp,
            name: format!("part{id}"),
            parent: None,
        }
    }

    #[test]
    fn near_symmetric_adjudication_rate_is_measured_and_deterministic() {
        // D4 (ADR-080 convergence): the HONEST accuracy ceiling of the analytic descriptor on a
        // symmetric-part-heavy assembly. Near-square plates have near-equal principal moments (soft eigenvalue
        // ordering) + chirality 0 → the geometric term is weak, the match leans on volume/area + the spatial
        // bootstrap. Build 12 thin plates edited (each thinned a distinct amount so the byte-hash differs and
        // the GEOMETRIC layer runs), re-imported at their own positions; measure the true-match band.
        let count = 12usize;
        // `spacing` controls how much the spatial bootstrap can disambiguate: DISTINCT (3.0 units apart — a
        // realistic factory layout) vs CLUSTERED (0.15 apart — the spatial prior saturates for every
        // candidate, so the near-symmetric SHAPE DESCRIPTOR alone must decide — its honest ceiling).
        let build = |spacing: f64| {
            let mut old = Vec::new();
            let mut new = Vec::new();
            for i in 0..count {
                #[allow(clippy::cast_precision_loss)]
                let x = i as f64 * spacing;
                // A near-square plate; each one a hair different so they're not literally identical.
                #[allow(clippy::cast_precision_loss)]
                let s = 1.0 + (i as f64) * 0.01;
                let plate = box_mesh(s, s * 0.99, 0.08);
                #[allow(clippy::cast_precision_loss)]
                let edited = box_mesh(s, s * 0.99, 0.075 - (i as f64) * 0.0005); // thinned (a small edit)
                old.push(ident(
                    i as u64,
                    &plate,
                    Some(0x1000 + i as u64),
                    [x, 0.0, 0.0],
                ));
                new.push(ident(
                    100 + i as u64,
                    &edited,
                    Some(0x2000 + i as u64),
                    [x, 0.0, 0.0],
                ));
            }
            (old, new)
        };

        let measure = |spacing: f64| {
            let (old, new) = build(spacing);
            let plan = match_identities(&old, &new);
            let (mut strong, mut adj, mut miss, mut wrong) = (0usize, 0usize, 0usize, 0usize);
            for i in 0..count {
                let m = plan.matches.iter().find(|m| m.old_id == i as u64).unwrap();
                // The TRUE match for old i is new (100+i) at the same position.
                let true_new = Some(100 + i as u64);
                match m.kind {
                    MatchKind::Strong | MatchKind::Unchanged | MatchKind::Moved => {
                        if m.new_id == true_new {
                            strong += 1;
                        } else {
                            wrong += 1; // matched, but to the WRONG part — the corruption we must never do
                        }
                    }
                    MatchKind::LowConfidence => {
                        if m.new_id == true_new {
                            adj += 1;
                        } else {
                            wrong += 1;
                        }
                    }
                    MatchKind::Miss => miss += 1,
                }
            }
            (strong, adj, miss, wrong)
        };

        // Determinism: same assembly → same bands, both layouts (the ADR-020 quantized-decision gate).
        assert_eq!(
            measure(3.0),
            measure(3.0),
            "distinct-layout measurement is deterministic"
        );
        assert_eq!(
            measure(0.15),
            measure(0.15),
            "clustered-layout measurement is deterministic"
        );

        for (label, spacing) in [
            ("DISTINCT positions", 3.0),
            ("CLUSTERED (spatial-prior saturated)", 0.15),
        ] {
            let (strong, adj, miss, wrong) = measure(spacing);
            #[allow(clippy::cast_precision_loss)]
            let adj_rate = adj as f64 / count as f64;
            eprintln!(
                "[D4 near-symmetric analytic descriptor · {label}] {count} plates: strong(auto)={strong} \
                 adjudicate={adj} miss(false)={miss} wrong={wrong} → adjudication_rate={:.1}%",
                adj_rate * 100.0
            );
            // THE INVIOLABLE GATE, in BOTH layouts: a true match is NEVER silently bound to the WRONG part
            // (prefer-miss-over-wrong; the histogram/learned matcher lifts the adjudicate/miss band — it never
            // needs to relax this). A wrong-bind here would be a silent override corruption = the milestone FAIL.
            assert_eq!(
                wrong, 0,
                "{label}: no near-symmetric true match wrong-binds — corruption is impossible"
            );
        }
    }

    #[test]
    fn the_surface_histogram_lifts_a_near_moment_equal_confusable_pair() {
        // D5 (ADR-080 convergence): the histogram's discriminating lift. Two parts with NEAR-EQUAL moments but
        // DIFFERENT surface-type mixes — a square plate (all planar) and a puck of matching moments (planar
        // caps + a cylindrical rim). Placed so their positions are ambiguous (both near the origin), the
        // shape-only term confuses them; the surface histogram separates them.
        let plate = box_mesh(1.0, 1.0, 0.1);
        let puck = box_mesh(1.0, 1.0, 0.1); // same mesh → same moments (the worst confusable case)
        let plate_hist = [6, 0, 0, 0, 0]; // 6 planar faces
        let puck_hist = [2, 4, 0, 0, 0]; // 2 planar caps + a cylindrical rim (4 quads)

        // Old scene: a plate (id 1) and a puck (id 2), co-located-ish. Re-import: both EDITED (new hashes) at
        // the same spots. Without the histogram the matcher could swap them; with it, each keeps identity.
        let old = vec![
            ident_h(1, &plate, 0xA1, [0.0, 0.0, 0.0], plate_hist),
            ident_h(2, &puck, 0xB1, [0.2, 0.0, 0.0], puck_hist),
        ];
        let new = vec![
            ident_h(11, &plate, 0xA2, [0.0, 0.0, 0.0], plate_hist),
            ident_h(12, &puck, 0xB2, [0.2, 0.0, 0.0], puck_hist),
        ];
        let plan = match_identities(&old, &new);
        // The plate maps to the plate, the puck to the puck — the histogram broke the moment tie.
        assert_eq!(
            plan.rebind_target(1),
            Some(11),
            "the plate keeps identity (histogram-disambiguated)"
        );
        assert_eq!(
            plan.rebind_target(2),
            Some(12),
            "the puck keeps identity (not swapped with the plate)"
        );
    }

    #[test]
    fn matching_is_deterministic() {
        // Same version pair → byte-identical plan (the CI determinism gate).
        let cube = box_mesh(1.0, 1.0, 1.0);
        let shrunk = box_mesh(0.9, 0.9, 0.9);
        let old = vec![
            ident(1, &cube, Some(1), [0.0, 0.0, 0.0]),
            ident(2, &cube, Some(2), [4.0, 0.0, 0.0]),
        ];
        let new = vec![
            ident(1, &shrunk, Some(3), [0.0, 0.0, 0.0]),
            ident(2, &shrunk, Some(4), [4.0, 0.0, 0.0]),
        ];
        let a = match_identities(&old, &new);
        let b = match_identities(&old, &new);
        assert_eq!(a, b, "same input → identical plan");
    }
}
