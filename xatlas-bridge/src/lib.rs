//! Safe, narrow bindings to the pinned vendored xatlas implementation.
//!
//! The upstream Rust crate runs bindgen during every build, which makes packaged Windows and CI builds
//! depend on a developer-global libclang installation. This project-owned bridge instead exposes only
//! the operations Metrocalk needs through a stable C ABI, checks all lengths before FFI, and returns owned
//! Rust vectors. No foreign pointer escapes this crate.

use std::ffi::c_void;
use std::fmt;
use std::ptr::NonNull;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct FfiVertex {
    atlas_index: i32,
    chart_index: i32,
    uv: [f32; 2],
    xref: u32,
}

#[expect(
    unsafe_code,
    reason = "declarations are isolated behind the safe Atlas owner"
)]
unsafe extern "C" {
    fn metrocalk_xatlas_create() -> *mut c_void;
    fn metrocalk_xatlas_destroy(handle: *mut c_void);
    fn metrocalk_xatlas_add_mesh(
        handle: *mut c_void,
        positions: *const f32,
        normals: *const f32,
        vertex_count: u32,
        indices: *const u32,
        index_count: u32,
        mesh_count_hint: u32,
    ) -> i32;
    fn metrocalk_xatlas_generate(
        handle: *mut c_void,
        normal_seam_weight: f32,
        max_iterations: u32,
        fix_winding: u8,
        padding: u32,
        texels_per_unit: f32,
        resolution: u32,
        block_align: u8,
        brute_force: u8,
    );
    fn metrocalk_xatlas_width(handle: *const c_void) -> u32;
    fn metrocalk_xatlas_height(handle: *const c_void) -> u32;
    fn metrocalk_xatlas_atlas_count(handle: *const c_void) -> u32;
    fn metrocalk_xatlas_chart_count(handle: *const c_void) -> u32;
    fn metrocalk_xatlas_mesh_count(handle: *const c_void) -> u32;
    fn metrocalk_xatlas_texels_per_unit(handle: *const c_void) -> f32;
    fn metrocalk_xatlas_utilization(handle: *const c_void, atlas_index: u32) -> f32;
    fn metrocalk_xatlas_mesh_vertex_count(handle: *const c_void, mesh_index: u32) -> u32;
    fn metrocalk_xatlas_mesh_index_count(handle: *const c_void, mesh_index: u32) -> u32;
    fn metrocalk_xatlas_copy_mesh_vertices(
        handle: *const c_void,
        mesh_index: u32,
        output: *mut FfiVertex,
        capacity: u32,
    ) -> u8;
    fn metrocalk_xatlas_copy_mesh_indices(
        handle: *const c_void,
        mesh_index: u32,
        output: *mut u32,
        capacity: u32,
    ) -> u8;
}

/// One output chart vertex. UV values are in atlas-pixel coordinates until the caller normalizes them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AtlasVertex {
    /// Atlas page index.
    pub atlas_index: i32,
    /// Chart index within the output.
    pub chart_index: i32,
    /// Packed pixel coordinates.
    pub uv: [f32; 2],
    /// Source vertex index copied by xatlas.
    pub xref: u32,
}

/// One owned output mesh corresponding to one input mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct AtlasMesh {
    /// Chart-split output vertices.
    pub vertices: Vec<AtlasVertex>,
    /// Triangle indices into `vertices`.
    pub indices: Vec<u32>,
}

/// Chart and pack options used by Metrocalk's production path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GenerateOptions {
    pub normal_seam_weight: f32,
    pub max_iterations: u32,
    pub fix_winding: bool,
    pub padding: u32,
    pub texels_per_unit: f32,
    pub resolution: u32,
    pub block_align: bool,
    pub brute_force: bool,
}

/// Safe bridge failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum XatlasError {
    Allocation,
    InvalidInput(&'static str),
    AddMesh(i32),
    InvalidOutput(&'static str),
}

impl fmt::Display for XatlasError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Allocation => f.write_str("xatlas allocation failed"),
            Self::InvalidInput(reason) => write!(f, "invalid xatlas input: {reason}"),
            Self::AddMesh(code) => write!(f, "xatlas AddMesh failed with code {code}"),
            Self::InvalidOutput(reason) => write!(f, "invalid xatlas output: {reason}"),
        }
    }
}

impl std::error::Error for XatlasError {}

/// Unique owner of one native atlas. No raw pointer can be observed by callers.
pub struct Atlas {
    handle: NonNull<c_void>,
}

impl fmt::Debug for Atlas {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Atlas").finish_non_exhaustive()
    }
}

impl Atlas {
    /// Allocate an empty atlas.
    pub fn new() -> Result<Self, XatlasError> {
        #[expect(
            unsafe_code,
            reason = "constructor owns the returned native allocation"
        )]
        let handle = unsafe { metrocalk_xatlas_create() };
        NonNull::new(handle)
            .map(|handle| Self { handle })
            .ok_or(XatlasError::Allocation)
    }

    /// Add a copied indexed triangle mesh.
    pub fn add_mesh(
        &mut self,
        positions: &[f32],
        normals: &[f32],
        indices: &[u32],
        mesh_count_hint: u32,
    ) -> Result<(), XatlasError> {
        if positions.is_empty() || !positions.len().is_multiple_of(3) {
            return Err(XatlasError::InvalidInput(
                "positions must be non-empty xyz triples",
            ));
        }
        if normals.len() != positions.len() {
            return Err(XatlasError::InvalidInput(
                "normals must match the position stream",
            ));
        }
        if indices.is_empty() || !indices.len().is_multiple_of(3) {
            return Err(XatlasError::InvalidInput(
                "indices must be a non-empty triangle list",
            ));
        }
        let vertex_count = u32::try_from(positions.len() / 3)
            .map_err(|_| XatlasError::InvalidInput("vertex count exceeds u32"))?;
        let index_count = u32::try_from(indices.len())
            .map_err(|_| XatlasError::InvalidInput("index count exceeds u32"))?;
        #[expect(
            unsafe_code,
            reason = "validated slices stay alive while AddMesh synchronously copies them"
        )]
        let code = unsafe {
            metrocalk_xatlas_add_mesh(
                self.handle.as_ptr(),
                positions.as_ptr(),
                normals.as_ptr(),
                vertex_count,
                indices.as_ptr(),
                index_count,
                mesh_count_hint,
            )
        };
        if code == 0 {
            Ok(())
        } else {
            Err(XatlasError::AddMesh(code))
        }
    }

    /// Compute charts and pack them.
    pub fn generate(&mut self, options: GenerateOptions) {
        #[expect(
            unsafe_code,
            reason = "the unique Atlas owner serializes the native mutation"
        )]
        unsafe {
            metrocalk_xatlas_generate(
                self.handle.as_ptr(),
                options.normal_seam_weight,
                options.max_iterations,
                u8::from(options.fix_winding),
                options.padding,
                options.texels_per_unit,
                options.resolution,
                u8::from(options.block_align),
                u8::from(options.brute_force),
            );
        }
    }

    /// Output atlas width.
    pub fn width(&self) -> u32 {
        self.scalar(metrocalk_xatlas_width)
    }

    /// Output atlas height.
    pub fn height(&self) -> u32 {
        self.scalar(metrocalk_xatlas_height)
    }

    /// Number of atlas pages.
    pub fn atlas_count(&self) -> u32 {
        self.scalar(metrocalk_xatlas_atlas_count)
    }

    /// Total chart count.
    pub fn chart_count(&self) -> u32 {
        self.scalar(metrocalk_xatlas_chart_count)
    }

    /// Effective texel density selected by xatlas.
    pub fn texels_per_unit(&self) -> f32 {
        #[expect(unsafe_code, reason = "read-only getter on the live owned handle")]
        unsafe {
            metrocalk_xatlas_texels_per_unit(self.handle.as_ptr())
        }
    }

    /// Page utilization in `[0,1]`.
    pub fn utilization(&self, atlas_index: u32) -> f32 {
        #[expect(unsafe_code, reason = "read-only getter on the live owned handle")]
        unsafe {
            metrocalk_xatlas_utilization(self.handle.as_ptr(), atlas_index)
        }
    }

    /// Copy all output meshes into owned Rust storage.
    pub fn meshes(&self) -> Result<Vec<AtlasMesh>, XatlasError> {
        let mesh_count = self.scalar(metrocalk_xatlas_mesh_count);
        let mut meshes = Vec::with_capacity(mesh_count as usize);
        for mesh_index in 0..mesh_count {
            let vertex_count = self.scalar_indexed(metrocalk_xatlas_mesh_vertex_count, mesh_index);
            let index_count = self.scalar_indexed(metrocalk_xatlas_mesh_index_count, mesh_index);
            if vertex_count == 0 || index_count == 0 {
                return Err(XatlasError::InvalidOutput("an output mesh is empty"));
            }
            let mut ffi_vertices = vec![FfiVertex::default(); vertex_count as usize];
            let mut indices = vec![0u32; index_count as usize];
            #[expect(
                unsafe_code,
                reason = "vectors are exactly sized and native copy functions enforce capacities"
            )]
            let copied = unsafe {
                metrocalk_xatlas_copy_mesh_vertices(
                    self.handle.as_ptr(),
                    mesh_index,
                    ffi_vertices.as_mut_ptr(),
                    vertex_count,
                ) != 0
                    && metrocalk_xatlas_copy_mesh_indices(
                        self.handle.as_ptr(),
                        mesh_index,
                        indices.as_mut_ptr(),
                        index_count,
                    ) != 0
            };
            if !copied {
                return Err(XatlasError::InvalidOutput("native output copy failed"));
            }
            let vertices = ffi_vertices
                .into_iter()
                .map(|vertex| AtlasVertex {
                    atlas_index: vertex.atlas_index,
                    chart_index: vertex.chart_index,
                    uv: vertex.uv,
                    xref: vertex.xref,
                })
                .collect();
            meshes.push(AtlasMesh { vertices, indices });
        }
        Ok(meshes)
    }

    fn scalar(&self, getter: unsafe extern "C" fn(*const c_void) -> u32) -> u32 {
        #[expect(unsafe_code, reason = "read-only getter on the live owned handle")]
        unsafe {
            getter(self.handle.as_ptr())
        }
    }

    fn scalar_indexed(
        &self,
        getter: unsafe extern "C" fn(*const c_void, u32) -> u32,
        index: u32,
    ) -> u32 {
        #[expect(unsafe_code, reason = "read-only getter on the live owned handle")]
        unsafe {
            getter(self.handle.as_ptr(), index)
        }
    }
}

impl Drop for Atlas {
    fn drop(&mut self) {
        #[expect(
            unsafe_code,
            reason = "Drop releases the unique native allocation exactly once"
        )]
        unsafe {
            metrocalk_xatlas_destroy(self.handle.as_ptr());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwraps_a_quad_without_libclang_or_bindgen() {
        let positions = [
            -1.0, -1.0, 0.0, 1.0, -1.0, 0.0, 1.0, 1.0, 0.0, -1.0, 1.0, 0.0,
        ];
        let normals = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0];
        let indices = [0, 1, 2, 0, 2, 3];
        let mut atlas = Atlas::new().expect("atlas");
        atlas
            .add_mesh(&positions, &normals, &indices, 1)
            .expect("mesh");
        atlas.generate(GenerateOptions {
            normal_seam_weight: 1_001.0,
            max_iterations: 2,
            fix_winding: true,
            padding: 2,
            texels_per_unit: 0.0,
            resolution: 128,
            block_align: true,
            brute_force: true,
        });
        assert_eq!(atlas.atlas_count(), 1);
        assert!(atlas.width() > 0 && atlas.height() > 0);
        assert!(atlas.chart_count() > 0);
        assert!((0.0..=1.0).contains(&atlas.utilization(0)));
        let meshes = atlas.meshes().expect("owned output");
        assert_eq!(meshes.len(), 1);
        assert_eq!(meshes[0].indices.len(), 6);
        assert!(meshes[0].vertices.iter().all(|vertex| vertex.xref < 4));
    }
}
