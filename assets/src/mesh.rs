//! The project's **internal** mesh representation — the trait-wrapped output of any importer
//! ([`crate::source`]). No foreign decoder types appear here (invariant 5): a `MeshAsset` is plain
//! geometry + materials, so the store, the GPU packer, and any future backend speak only this.

/// An axis-aligned bounding box over a mesh's positions — the cheap spatial summary the editor uses
/// to frame/scale an imported asset, and the headless test asserts an import produced sane geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    /// Component-wise minimum corner.
    pub min: [f32; 3],
    /// Component-wise maximum corner.
    pub max: [f32; 3],
}

impl Bounds {
    /// The bounds of an empty mesh — an inverted box so the first point initializes it correctly.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            min: [f32::INFINITY; 3],
            max: [f32::NEG_INFINITY; 3],
        }
    }

    /// Expand to include `p`.
    pub fn expand(&mut self, p: [f32; 3]) {
        for ((mn, mx), &v) in self.min.iter_mut().zip(self.max.iter_mut()).zip(&p) {
            *mn = mn.min(v);
            *mx = mx.max(v);
        }
    }

    /// The center of the box (or the origin if the box is empty/degenerate).
    #[must_use]
    pub fn center(&self) -> [f32; 3] {
        if self.min[0] > self.max[0] {
            return [0.0; 3];
        }
        [
            (self.min[0] + self.max[0]) * 0.5,
            (self.min[1] + self.max[1]) * 0.5,
            (self.min[2] + self.max[2]) * 0.5,
        ]
    }

    /// The largest extent across the three axes (0 for an empty box) — the "size" used to normalize
    /// an imported asset to a sensible viewport scale.
    #[must_use]
    pub fn max_extent(&self) -> f32 {
        if self.min[0] > self.max[0] {
            return 0.0;
        }
        let mut m = 0.0f32;
        for (hi, lo) in self.max.iter().zip(&self.min) {
            m = m.max(hi - lo);
        }
        m
    }
}

/// One material's base appearance — PBR base-color factor + an optional base-color texture (an index
/// into [`MeshAsset::textures`]). The render path bakes `base_color` per-primitive today; texture
/// sampling is the next render increment (the texture is decoded + carried so the data path is real).
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    /// Linear RGBA base color factor `[0,1]`.
    pub base_color: [f32; 4],
    /// Index into [`MeshAsset::textures`] of the base-color texture, if any.
    pub base_color_texture: Option<usize>,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            base_color: [0.8, 0.8, 0.8, 1.0],
            base_color_texture: None,
        }
    }
}

/// A decoded texture — tightly-packed RGBA8 (`width * height * 4` bytes). Always RGBA8 so the data
/// path is uniform across native + wasm (PNG decoded by our pinned pure-Rust `image`); GPU-compressed
/// KTX2 normalization is a native-only step (basis-universal is C-FFI — see the asset ADR).
#[derive(Clone, Debug, PartialEq)]
pub struct Texture {
    /// Pixel width.
    pub width: u32,
    /// Pixel height.
    pub height: u32,
    /// `width * height * 4` bytes, RGBA8 row-major.
    pub rgba8: Vec<u8>,
}

/// One drawable piece of a mesh — a vertex stream + indices + the material index it uses. A glTF
/// "primitive". Positions are always present; normals/uvs may be empty (the packer fills sane
/// defaults). Indices are always present (the importer triangulates/synthesizes a sequential index
/// list when a primitive ships none).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Primitive {
    /// Per-vertex positions.
    pub positions: Vec<[f32; 3]>,
    /// Per-vertex normals (empty ⇒ packer derives flat normals).
    pub normals: Vec<[f32; 3]>,
    /// Per-vertex texcoord 0 (empty ⇒ packer fills `[0,0]`).
    pub uvs: Vec<[f32; 2]>,
    /// Triangle-list indices into the vertex arrays.
    pub indices: Vec<u32>,
    /// Index into [`MeshAsset::materials`].
    pub material: usize,
    /// Per-vertex skin **joints** (glTF `JOINTS_0`, ≤4 influences) — indices into
    /// [`MeshAsset::skeleton`]'s joints, **already remapped to the skeleton's topological order** by the
    /// importer (M9.3 / G3). Empty ⇒ the primitive is not skinned (a static mesh).
    pub joints: Vec<[u16; 4]>,
    /// Per-vertex skin **weights** (glTF `WEIGHTS_0`, parallel to [`Self::joints`]). Empty ⇒ not skinned.
    pub weights: Vec<[f32; 4]>,
}

/// A fully-imported asset — the working, internal object an entity references by handle. Geometry +
/// materials + textures, no foreign types, `wasm32`-portable.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MeshAsset {
    /// A human label (the glTF mesh/node name, or a fallback) — for the inspector / logs.
    pub name: String,
    /// The drawable primitives.
    pub primitives: Vec<Primitive>,
    /// Materials, indexed by [`Primitive::material`].
    pub materials: Vec<Material>,
    /// Decoded textures, indexed by [`Material::base_color_texture`].
    pub textures: Vec<Texture>,
    /// The rig (M9.3 / G3): a [`metrocalk_skeleton::Skeleton`] mapped from the glTF `skin` — joints
    /// (topologically ordered), their bind-pose local TRS, and `inverseBindMatrices`. `None` ⇒ a static
    /// (un-rigged) mesh. The per-vertex `JOINTS_0`/`WEIGHTS_0` on each [`Primitive`] index into it. The
    /// foreign `gltf::` types stay behind the importer wrapper — this is our own type (invariant 5).
    pub skeleton: Option<metrocalk_skeleton::Skeleton>,
}

impl MeshAsset {
    /// Total vertex count across primitives.
    #[must_use]
    pub fn vertex_count(&self) -> usize {
        self.primitives.iter().map(|p| p.positions.len()).sum()
    }

    /// Total index count across primitives.
    #[must_use]
    pub fn index_count(&self) -> usize {
        self.primitives.iter().map(|p| p.indices.len()).sum()
    }

    /// Total triangle count.
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.index_count() / 3
    }

    /// The asset's axis-aligned bounds over all primitive positions.
    #[must_use]
    pub fn bounds(&self) -> Bounds {
        let mut b = Bounds::empty();
        for prim in &self.primitives {
            for &p in &prim.positions {
                b.expand(p);
            }
        }
        b
    }
}
