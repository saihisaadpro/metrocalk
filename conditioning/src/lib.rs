//! Production mesh conditioning behind stable, project-owned contracts.
//!
//! The portable default contains MikkTSpace tangent generation and deterministic validation/reporting.
//! Native desktop builds additionally enable xatlas charting/packing and meshoptimizer QEM through
//! adapters; no foreign type crosses this crate's public surface.

pub mod bake;
pub mod simplify;
pub mod tangent;
pub mod uv;

pub use bake::{
    BakeConfig, BakeError, BakeMaps, BakeObserver, BakeProgress, BakeReport, BakeResult,
    BakeSource, BakeStage, BakeTransform, CpuTextureBaker, TextureBaker,
};
#[cfg(feature = "qem")]
pub use simplify::MeshoptQemSimplifier;
pub use simplify::{MeshSimplifier, SimplifyConfig, SimplifyError, SimplifyReport, SimplifyResult};
pub use tangent::{generate_mikktspace_tangents, TangentError, TangentReport};
pub use uv::{ChartUvConfig, ChartUvError, ChartUvReport, ChartUvResult, UvUnwrapper};
