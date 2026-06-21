//! **Neural auto-rig as OFFLINE asset-prep** (M9.5 / G5 deliverable 4, ADR-029) — the importer that
//! ingests a neural auto-rigger's output (UniRig / RigAnything / Make-It-Animatable) and **bakes a
//! standard skeleton + LBS weights**, so the character then runs **entirely on G3's deterministic LBS**.
//! **The neural network is offline asset prep (CUDA/cloud, like text-to-3D), NEVER a runtime dependency**
//! — no NN, no learned field, ever touches the per-frame path. This is the highest-payoff, lowest-risk
//! neural integration (B.4): we steal the auto-rigging, run the deterministic LBS ourselves.
//!
//! It sits behind the **M4 [`MeshSource`] trait** (invariant 5 / ADR-014), exactly like the glTF backend.
//! The input is a self-contained `MTKRIG` blob — the structured, **arbitrary-shape** prediction a neural
//! rigger emits: joints in **any order**, per-vertex influences of **arbitrary count** (often > 4), and
//! **un-normalized** weights. The bake fixes all of that into the standard LBS contract G3 consumes:
//! topologically sorted joints (parent before child), **≤ 4 influences** per vertex, **normalized** to a
//! partition of unity, `inverseBindMatrices` computed from the bind pose. No foreign type crosses out.

// Index casts are bounded by MAX_ELEMENTS (checked before use); joint counts are tiny. `b`/`r` short
// names are canonical for the byte buffer / reader.
#![allow(clippy::cast_possible_truncation, clippy::similar_names)]

use metrocalk_skeleton::{Joint, Skeleton, Transform};

use crate::mesh::{Material, MeshAsset, Primitive};
use crate::source::{ImportError, MeshSource, MAX_ELEMENTS, MAX_IMPORT_BYTES};

/// Magic + version for the self-contained neural-auto-rig blob (the structured prediction the offline
/// rigger emits, ingested here and baked to standard LBS).
const MAGIC: &[u8; 8] = b"MTKRIG01";

/// One predicted joint: its parent (an index into the prediction's joint list, **any order** — the bake
/// topo-sorts), and its bind-pose local TRS. The neural rigger predicts the skeleton; we treat the
/// predicted pose as the bind and compute `inverseBindMatrices` from it (the procedural-rig path).
#[derive(Clone, Debug, PartialEq)]
pub struct AutoRigJoint {
    /// Parent joint index in the prediction's order, or `None` for a root.
    pub parent: Option<usize>,
    /// The joint's bind-pose local transform.
    pub local_bind: Transform,
}

/// A neural auto-rigger's structured output: a predicted skeleton + per-vertex influences of arbitrary
/// count/normalization (parallel to the mesh's vertices). The deterministic bake turns this into a
/// standard, G3-consumable rig.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AutoRig {
    /// Predicted joints, in the prediction's (arbitrary) order.
    pub joints: Vec<AutoRigJoint>,
    /// Per-vertex predicted influences `(joint_index, weight)` — **any count, any normalization**.
    pub influences: Vec<Vec<(u32, f32)>>,
}

/// The neural-auto-rig importer (behind the [`MeshSource`] trait). Decodes the `MTKRIG` blob and
/// **bakes** it to a standard LBS [`MeshAsset`] — the neural step already ran offline; this is pure,
/// deterministic asset prep.
#[derive(Debug, Default, Clone, Copy)]
pub struct NeuralRigImporter;

impl NeuralRigImporter {
    /// Construct the importer.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl MeshSource for NeuralRigImporter {
    fn format(&self) -> &'static str {
        "neural-autorig/mtkrig"
    }

    fn import(&self, bytes: &[u8]) -> Result<MeshAsset, ImportError> {
        if bytes.len() > MAX_IMPORT_BYTES {
            return Err(ImportError::TooLarge {
                bytes: bytes.len(),
                limit: MAX_IMPORT_BYTES,
            });
        }
        let (positions, indices, rig) = decode_mtkrig(bytes)?;
        bake_standard_lbs(positions, indices, &rig)
    }
}

/// **The bake** (the deterministic core, neural-output-shape-agnostic): topologically sort the predicted
/// joints (parent before child — FK is one forward pass), reduce each vertex to its **top-4** influences,
/// **normalize** to a partition of unity, and compute `inverseBindMatrices` from the bind pose. The
/// result runs entirely on G3's LBS — no NN at runtime.
///
/// # Errors
/// [`ImportError`] on no geometry, no joints, or over-large element counts.
pub fn bake_standard_lbs(
    positions: Vec<[f32; 3]>,
    indices: Vec<u32>,
    rig: &AutoRig,
) -> Result<MeshAsset, ImportError> {
    if positions.is_empty() {
        return Err(ImportError::NoGeometry);
    }
    guard_count(positions.len())?;
    guard_count(indices.len())?;
    let n_joints = rig.joints.len();
    if n_joints == 0 {
        return Err(ImportError::Malformed("auto-rig has no joints".into()));
    }
    guard_count(n_joints)?;

    // ── Topologically sort the predicted joints (parent precedes child) ───────────────────────────
    let parent_of: Vec<Option<usize>> = rig.joints.iter().map(|j| j.parent).collect();
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); n_joints];
    let mut stack: Vec<usize> = Vec::new();
    for (slot, parent) in parent_of.iter().enumerate() {
        match parent {
            // A parent index out of range is treated as a root (defensive against a malformed prediction).
            Some(p) if *p < n_joints => children[*p].push(slot),
            _ => stack.push(slot),
        }
    }
    stack.reverse();
    let mut order: Vec<usize> = Vec::with_capacity(n_joints);
    while let Some(slot) = stack.pop() {
        order.push(slot);
        for &c in children[slot].iter().rev() {
            stack.push(c);
        }
    }
    // A cyclic prediction could leave joints unvisited — append them so indices stay valid.
    if order.len() < n_joints {
        let seen: std::collections::HashSet<usize> = order.iter().copied().collect();
        order.extend((0..n_joints).filter(|s| !seen.contains(s)));
    }
    let mut remap = vec![0usize; n_joints];
    for (new_idx, &old) in order.iter().enumerate() {
        remap[old] = new_idx;
    }
    let mut skel = Skeleton {
        joints: order
            .iter()
            .map(|&old| Joint {
                parent: parent_of[old].filter(|p| *p < n_joints).map(|p| remap[p]),
                local_bind: rig.joints[old].local_bind,
                inverse_bind: [[0.0; 4]; 4], // filled below
            })
            .collect(),
    };
    // Bind pose → inverseBindMatrices (so a bound vertex is unmoved at rest; the procedural-rig path).
    skel.recompute_inverse_binds();

    // ── Reduce each vertex to top-4 normalized influences (remapped to topo order) ────────────────
    let nverts = positions.len();
    let mut joints = vec![[0u16; 4]; nverts];
    let mut weights = vec![[0.0f32; 4]; nverts];
    for v in 0..nverts {
        let mut inf: Vec<(usize, f32)> = rig
            .influences
            .get(v)
            .map(|list| {
                list.iter()
                    .filter(|&&(j, w)| (j as usize) < n_joints && w > 0.0)
                    .map(|&(j, w)| (remap[j as usize], w))
                    .collect()
            })
            .unwrap_or_default();
        // Keep the 4 strongest (stable: weight desc, then joint index).
        inf.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        inf.truncate(4);
        let sum: f32 = inf.iter().map(|&(_, w)| w).sum();
        if sum > 0.0 {
            for (slot, &(j, w)) in inf.iter().enumerate() {
                joints[v][slot] = j as u16;
                weights[v][slot] = w / sum;
            }
        } else {
            // An unweighted vertex → bind fully to the root (joint 0 after topo-sort) so LBS is defined.
            weights[v][0] = 1.0;
        }
    }

    Ok(MeshAsset {
        name: "auto-rigged".to_string(),
        primitives: vec![Primitive {
            positions,
            normals: Vec::new(),
            uvs: Vec::new(),
            indices,
            material: 0,
            joints,
            weights,
        }],
        materials: vec![Material::default()],
        textures: Vec::new(),
        skeleton: Some(skel),
    })
}

/// Reject a count over [`MAX_ELEMENTS`] before it can allocate (the decode-bomb guard).
fn guard_count(count: usize) -> Result<(), ImportError> {
    if count > MAX_ELEMENTS {
        Err(ImportError::TooManyElements {
            count,
            limit: MAX_ELEMENTS,
        })
    } else {
        Ok(())
    }
}

/// `(positions, indices, auto-rig)` decoded from an `MTKRIG` blob.
type DecodedRig = (Vec<[f32; 3]>, Vec<u32>, AutoRig);

/// Decode an `MTKRIG` blob into `(positions, indices, AutoRig)`. Every read is bounds-checked; any
/// shortfall or bad magic is a flat [`ImportError::Malformed`] (no foreign error type, no panic).
fn decode_mtkrig(bytes: &[u8]) -> Result<DecodedRig, ImportError> {
    let mut r = Reader::new(bytes);
    if r.take(8)? != MAGIC {
        return Err(ImportError::Malformed("not an MTKRIG blob".into()));
    }
    let nverts = r.u32()? as usize;
    guard_count(nverts)?;
    let mut positions = Vec::with_capacity(nverts);
    for _ in 0..nverts {
        positions.push([r.f32()?, r.f32()?, r.f32()?]);
    }
    let ntris = r.u32()? as usize;
    guard_count(ntris.saturating_mul(3))?;
    let mut indices = Vec::with_capacity(ntris * 3);
    for _ in 0..ntris * 3 {
        indices.push(r.u32()?);
    }
    let njoints = r.u32()? as usize;
    guard_count(njoints)?;
    let mut joints = Vec::with_capacity(njoints);
    for _ in 0..njoints {
        let parent = r.i32()?;
        let local_bind = Transform {
            translation: [r.f32()?, r.f32()?, r.f32()?],
            rotation: [r.f32()?, r.f32()?, r.f32()?, r.f32()?],
            scale: [r.f32()?, r.f32()?, r.f32()?],
        };
        joints.push(AutoRigJoint {
            parent: usize::try_from(parent).ok(),
            local_bind,
        });
    }
    let nvert2 = r.u32()? as usize;
    if nvert2 != nverts {
        return Err(ImportError::Malformed(
            "MTKRIG vertex/influence count mismatch".into(),
        ));
    }
    let mut influences = Vec::with_capacity(nverts);
    for _ in 0..nverts {
        let ninf = r.u16()? as usize;
        let mut list = Vec::with_capacity(ninf);
        for _ in 0..ninf {
            list.push((r.u32()?, r.f32()?));
        }
        influences.push(list);
    }
    Ok((positions, indices, AutoRig { joints, influences }))
}

/// A bounds-checked little-endian byte cursor — every read returns [`ImportError::Malformed`] on a
/// shortfall, so a truncated/hostile blob can never panic or over-read.
struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Self {
        Self { b, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], ImportError> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| ImportError::Malformed("MTKRIG length overflow".into()))?;
        let slice = self
            .b
            .get(self.pos..end)
            .ok_or_else(|| ImportError::Malformed("MTKRIG truncated".into()))?;
        self.pos = end;
        Ok(slice)
    }
    fn u16(&mut self) -> Result<u16, ImportError> {
        let s = self.take(2)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Result<u32, ImportError> {
        let s = self.take(4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn i32(&mut self) -> Result<i32, ImportError> {
        let s = self.take(4)?;
        Ok(i32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn f32(&mut self) -> Result<f32, ImportError> {
        Ok(f32::from_bits(self.u32()?))
    }
}
