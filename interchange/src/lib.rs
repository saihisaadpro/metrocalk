//! `metrocalk-interchange` — the robotics/VFX import tier (M8.5). The portable formats (URDF now;
//! USD-Physics next) wrapped behind the project-owned [`Interchange`] trait so **no `urdf_rs` / `openusd`
//! type crosses this crate's public surface** (invariant 5, CI grep-gated — the same discipline as
//! `rapier::`-in-`/physics` and `gltf::`-in-`/assets`, ADR-001/014).
//!
//! An import maps a foreign format onto our OWN neutral types ([`SceneImport`]) which carry the `/physics`
//! boundary types ([`metrocalk_physics::BodyDesc`] etc. — themselves rapier-free), so the editor turns an
//! import into **registry components** (M1.3/M8.2) with zero foreign coupling: a link → a `RigidBody` + a
//! `Collider`, a joint → our `Joint`, the file's units → the M8.3 scale/unit check. **Every unsupported
//! feature is an explained note** ([`UnsupportedNote`]), never a silent drop (ADR-016) — the accuracy of an
//! import is sacred, so we report what we approximated.
//!
//! **Honest scope (June 2026):** URDF is real *now* (`urdf-rs`, the standard Rust parser) and leads here;
//! USD-Physics (the pre-1.0 `openusd` crate) follows behind the same trait; MJCF is a *probe + seam* (no
//! mature Rust parser), not a committed round-trip.

use metrocalk_physics::{Quat, Vec3};
use serde::{Deserialize, Serialize};

mod step;
mod urdf;
mod usd;
pub use step::{
    gdt_entity_name, gdt_token, round_trip_deviation, CadEdge, CadFace, CadInterchange, CadPmi,
    CadScene, CadSolid, FaceKind, StepError, StepInterchange, MAX_ENTITIES as STEP_MAX_ENTITIES,
    MAX_STEP_BYTES, ROUND_TRIP_BUDGET,
};
pub use urdf::UrdfInterchange;
pub use usd::UsdInterchange;

// Re-export the /physics boundary types a SceneImport carries, so a consumer (the editor) maps an import
// to registry components depending only on `metrocalk-interchange` — no separate /physics dep needed.
pub use metrocalk_physics::{BodyKind, ColliderDesc, ColliderShape, JointDesc};

/// The stage units a format declares — the M8.5 deliverable-2 ground truth feeding M8.3's scale check.
/// URDF is SI (1 m / 1 kg per unit); USD declares `metersPerUnit` / `kilogramsPerUnit` (often cm / g).
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct Units {
    pub meters_per_unit: f64,
    pub kilograms_per_unit: f64,
}

impl Units {
    /// SI — the URDF assumption (everything in meters + kilograms).
    pub const SI: Self = Self {
        meters_per_unit: 1.0,
        kilograms_per_unit: 1.0,
    };
    /// `true` when the source isn't already SI metres — the scale-reconciliation flag M8.3 surfaces.
    #[must_use]
    pub fn needs_reconciliation(self) -> bool {
        (self.meters_per_unit - 1.0).abs() > 1e-9 || (self.kilograms_per_unit - 1.0).abs() > 1e-9
    }
}

/// One imported rigid body — a neutral record the editor maps to `Transform` + `RigidBody` + `Collider`
/// registry components. Carries the world transform (computed via forward kinematics for URDF's joint
/// tree) + the declared mass + the (first) collider shape; extra colliders/offsets are surfaced as notes.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ImportedBody {
    pub name: String,
    pub kind: BodyKind,
    pub translation: Vec3,
    pub rotation: Quat,
    /// The declared mass (kg, in scene units after reconciliation) — `None` ⇒ derive from volume × density.
    pub mass: Option<f64>,
    /// The body's collider, mapped to our open shape enum. `None` ⇒ no collision geometry (a note is added).
    pub collider: Option<ColliderDesc>,
}

/// One imported joint connecting two [`ImportedBody`] (by index) — mapped to our [`JointDesc`].
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ImportedJoint {
    pub name: String,
    pub parent: usize,
    pub child: usize,
    pub joint: JointDesc,
    /// The declared limit `(lower, upper)` in radians/metres — recorded for provenance even though the
    /// current joint model doesn't enforce it (a note is added so that's never a silent loss).
    pub limit: Option<(f64, f64)>,
}

/// An explained "no" — an unsupported or approximated feature, surfaced so an import never silently drops
/// or distorts (ADR-016; the accuracy discipline). `feature` is what was seen, `detail` is the why + what
/// we did instead.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct UnsupportedNote {
    pub feature: String,
    pub detail: String,
}

/// The neutral result of importing a scene — our types only, no foreign leak. The editor instantiates the
/// bodies/joints as registry components and shows the notes + the unit reconciliation.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct SceneImport {
    pub name: String,
    pub format: String,
    pub units: Units,
    pub bodies: Vec<ImportedBody>,
    pub joints: Vec<ImportedJoint>,
    /// Every unsupported/approximated feature, explained.
    pub notes: Vec<UnsupportedNote>,
}

/// An import that couldn't be honored — surfaced, never hidden (the explain discipline).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InterchangeError {
    /// The source couldn't be parsed (malformed XML/USD) — carries the parser's reason.
    Parse(String),
    /// The source parsed but described nothing importable (no links/prims).
    Empty(String),
}

impl std::fmt::Display for InterchangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(why) => write!(f, "interchange parse error: {why}"),
            Self::Empty(why) => write!(f, "nothing to import: {why}"),
        }
    }
}

impl std::error::Error for InterchangeError {}

/// The project-owned interchange seam — the one boundary between the engine and the foreign import crates
/// (`urdf_rs`, `openusd`). No foreign type appears in any signature here (invariant 5). An impl parses its
/// format and returns the neutral [`SceneImport`].
pub trait Interchange {
    /// The format name (provenance / notes).
    fn format(&self) -> &'static str;
    /// Parse `source` bytes into a neutral [`SceneImport`]. Text formats decode `source` as UTF-8.
    fn import(&self, source: &[u8]) -> Result<SceneImport, InterchangeError>;
}

// ── small pose math (our plain-array boundary — no glam dep, matching /physics) ──────────────────────

/// URDF roll-pitch-yaw (fixed-axis XYZ ≡ intrinsic ZYX) → our `[x,y,z,w]` quaternion.
#[must_use]
pub(crate) fn rpy_to_quat(rpy: [f64; 3]) -> Quat {
    let (cr, sr) = (rpy[0] * 0.5).cos_sin();
    let (cp, sp) = (rpy[1] * 0.5).cos_sin();
    let (cy, sy) = (rpy[2] * 0.5).cos_sin();
    [
        sr * cp * cy - cr * sp * sy,
        cr * sp * cy + sr * cp * sy,
        cr * cp * sy - sr * sp * cy,
        cr * cp * cy + sr * sp * sy,
    ]
}

/// Hamilton product `a ∘ b` (apply `b` then `a`), `[x,y,z,w]`.
#[must_use]
pub(crate) fn quat_mul(a: Quat, b: Quat) -> Quat {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// Rotate `v` by unit quaternion `q`.
#[must_use]
#[allow(clippy::many_single_char_names)] // x/y/z/w are the canonical quaternion component names
pub(crate) fn quat_rotate(q: Quat, v: Vec3) -> Vec3 {
    let [x, y, z, w] = q;
    let tx = 2.0 * (y * v[2] - z * v[1]);
    let ty = 2.0 * (z * v[0] - x * v[2]);
    let tz = 2.0 * (x * v[1] - y * v[0]);
    [
        v[0] + w * tx + (y * tz - z * ty),
        v[1] + w * ty + (z * tx - x * tz),
        v[2] + w * tz + (x * ty - y * tx),
    ]
}

/// A rigid pose (translation + rotation), composed down the kinematic tree.
#[derive(Clone, Copy)]
pub(crate) struct Pose {
    pub t: Vec3,
    pub q: Quat,
}

impl Pose {
    pub(crate) const IDENTITY: Self = Self {
        t: [0.0; 3],
        q: [0.0, 0.0, 0.0, 1.0],
    };
    /// `self ∘ local` — `local` expressed in `self`'s frame, returned in the parent frame.
    pub(crate) fn compose(self, local_t: Vec3, local_q: Quat) -> Self {
        let r = quat_rotate(self.q, local_t);
        Self {
            t: [self.t[0] + r[0], self.t[1] + r[1], self.t[2] + r[2]],
            q: quat_mul(self.q, local_q),
        }
    }
}

/// `(cos, sin)` of a half-angle — a tiny helper so `rpy_to_quat` reads cleanly.
trait CosSin {
    fn cos_sin(self) -> (f64, f64);
}
impl CosSin for f64 {
    fn cos_sin(self) -> (f64, f64) {
        (self.cos(), self.sin())
    }
}
