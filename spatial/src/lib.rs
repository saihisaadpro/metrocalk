//! `metrocalk-spatial` — **the scene-space subsystem**: one canonical transform, one hierarchy
//! evaluator, one ray builder, one picking pipeline, one manipulator, one tolerance policy.
//!
//! This crate exists because those six things were previously six *implicit* things, spread across a
//! renderer, a tauri command layer, a document bridge and a React app, each with its own idea of what
//! a transform is and where the cursor points. When "where is this object" has more than one answer,
//! the answers disagree — quietly, and in ways that read as UI bugs.
//!
//! # The coordinate contract
//!
//! Stated once, so importers and shaders can be checked against it rather than against each other.
//!
//! | | |
//! |---|---|
//! | Handedness | **Right-handed** |
//! | Up axis | **+Y** |
//! | Forward (view space) | **−Z** (the camera looks down −Z) |
//! | Clip depth | **`[0, 1]`** (wgpu/D3D), not `[-1, 1]` |
//! | NDC | `x` right, `y` **up**; `(-1,-1)` is bottom-left |
//! | Screen/pointer | `y` **down** from the top-left of the surface |
//! | World unit | **1 metre** |
//! | Angles | **radians** at every API boundary; degrees only in UI strings |
//! | Euler order | `R = Rz·Ry·Rx` — extrinsic XYZ (see [`transform::quat_to_euler_xyz`]) |
//! | Matrices | **column-major**, `m[col][row]`; `T·R·S` composition |
//! | Rotation storage | **unit quaternion** `[x, y, z, w]`; Euler is a display lens only |
//! | Canonical precision | **f64** on the CPU; f32 reaches the GPU only via [`origin::RenderOrigin`] |
//!
//! Importers convert **into** this space at their boundary ([`convert`]). A format's own convention
//! never enters the scene graph — the alternative is a scene where "up" depends on which file an
//! object came from.
//!
//! # The two pipelines
//!
//! ```text
//! authored TRS ──► TransformGraph ──► world matrix ──► RenderOrigin ──► f32 GPU transform
//!  (f64, local)     (hierarchy)        (f64, exact)     (rebased)        (no jitter)
//!
//! pointer ──► Viewport ──► NDC ──► Camera ray ──► SceneBvh ──► MeshBvh ──► HitResult ──► selection
//!  (logical)   (dpi+rect)          (f64, exact)   (broad)      (narrow)    (canonical)
//! ```
//!
//! Every arrow is a function in this crate, and each has tests that pin the property the next stage
//! depends on.

// Math-heavy crate: x/y/z/w, i/k/u/v and a/b are the canonical names here; precise float literals in
// tests read better un-separated; and index→f64 conversions in geometry loops lose nothing.
#![allow(
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::unreadable_literal,
    clippy::cast_precision_loss,
    clippy::module_name_repetitions,
    // Snapping, version stamps and grid rebasing produce EXACT values; comparing them exactly is the
    // correct test, and an epsilon there would hide a real change.
    clippy::float_cmp,
    // `for i in 0..3 { out[i] = a[i] + b[i] }` is the clearest way to write component-wise vector
    // maths. The iterator-zip rewrite this lint suggests is longer and harder to check by eye, which
    // is the wrong trade in a module whose correctness is checked by reading the arithmetic.
    clippy::needless_range_loop
)]

pub mod bounds;
pub mod bvh;
pub mod camera;
pub mod convert;
pub mod epsilon;
pub mod hierarchy;
pub mod manip;
pub mod origin;
pub mod pick;
pub mod ray;
pub mod snap;
pub mod transform;

pub use bounds::{Aabb, Obb};
pub use bvh::{MeshBvh, MeshHit, SceneBvh};
pub use camera::{Camera, Orbit, Projection, Viewport};
pub use convert::{AxisConvention, UnitScale};
pub use hierarchy::{NodeFlags, NodeId, ReparentMode, TransformGraph};
pub use manip::{
    DragConstraint, GizmoHandle, GizmoMode, GizmoSpace, Manipulator, PivotMode, TransformDelta,
};
pub use origin::RenderOrigin;
pub use pick::{
    ClickModifiers, HitKind, HitResult, HitSource, PickFilter, PickGeometry, PickMesh, PickObject,
    PickRequest, PickScene, Picker, RegionMode, ScreenRect, SelectionModel, TransparencyPolicy,
};
pub use ray::Ray;
pub use snap::{SnapMode, SnapSettings};
pub use transform::{Decomposed, Mat4, Quat, Transform, Vec3};
